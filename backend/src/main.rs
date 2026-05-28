mod config;
mod http;
mod routes;
mod state;
mod steam;

use std::path::{Path, PathBuf};

use anyhow::Result;
use axum::Json;
use axum::extract::{MatchedPath, Request};
use axum::middleware::{self, Next};
use axum::response::{Html, Response};
use axum::routing::get;
use directories::ProjectDirs;
use std::time::Instant;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;

use crate::state::AppState;

#[derive(OpenApi)]
#[openapi(info(
    title = "steam-multiversion-viewer",
    version = "0.1.0",
    description = "
Browse Steam depot manifests across versions, branches, and apps — \
list owned games, inspect manifest contents, diff files between \
manifests, and (optionally) expose the depot as a FUSE mount.",
))]
struct ApiDoc;

async fn scalar_html() -> Html<&'static str> {
    Html(include_str!("../static/scalar.html"))
}

#[tokio::main]
async fn main() -> Result<()> {
    let log_path = setup_logging()?;
    tracing::info!(log_file = %log_path.display(), "Starting...");

    let state = AppState::init().await?;

    let (api_router, openapi) =
        routes::register(OpenApiRouter::with_openapi(ApiDoc::openapi())).split_for_parts();

    let app = api_router
        .route(
            "/api/openapi.json",
            get({
                let openapi = openapi.clone();
                async || Json(openapi)
            }),
        )
        .route(
            "/api/downloads/events",
            get(routes::downloads::downloads_events),
        )
        .route("/api/docs", get(scalar_html))
        .layer(middleware::from_fn(http_log))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:6556").await?;
    tracing::info!("listening on http://{}", listener.local_addr()?);
    axum::serve(listener, app).await?;

    Ok(())
}

async fn http_log(req: Request, next: Next) -> Response {
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| req.uri().path().to_string());
    let start = Instant::now();
    let res = next.run(req).await;
    let status = res.status();
    let time = format!("{}ms", start.elapsed().as_millis());
    if status.is_success() {
        tracing::info!(route, status = status.as_u16(), time, "request");
    } else {
        tracing::warn!(route, status = status.as_u16(), time, "request");
    }
    res
}

fn setup_logging() -> Result<PathBuf> {
    let stdout_filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            format!(
                "{}=debug,steam_vent_depot=info,steam_depot_vfs=info,tower_http=info",
                env!("CARGO_CRATE_NAME")
            )
            .into()
        });
    // Span CLOSE events are useful for perf debugging but spammy on a
    // busy chunk fetch; keep them in the file log only.
    let stdout_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_filter(stdout_filter);

    let dirs = ProjectDirs::from("", "", "steam-multiversion-viewer");
    let log_dir = dirs
        .as_ref()
        .map(|d| d.state_dir().unwrap_or_else(|| d.cache_dir()))
        .unwrap_or(Path::new("logs"));
    std::fs::create_dir_all(log_dir)?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let file_name = format!("app.{ts}.log");
    let log_path = log_dir.join(&file_name);
    let file = std::fs::File::create(&log_path)?;
    let file_filter = tracing_subscriber::EnvFilter::new(
        "steam_multiversion_viewer=debug,steam_vent=debug,steam_vent_depot=debug,steam_depot_vfs=debug,tower_http=info",
    );
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
        .with_filter(file_filter);

    tracing_subscriber::registry()
        .with(stdout_layer)
        .with(file_layer)
        .init();

    Ok(log_path)
}
