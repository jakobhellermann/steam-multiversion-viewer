mod config;
mod downloads;
mod error;
mod extra_manifests;
mod http;
mod mount;
mod routes;
mod state;
mod steam;
mod store_index;

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
use utoipa_axum::routes;

use crate::state::AppState;

#[derive(OpenApi)]
#[openapi(info(title = "steam-multiversion-viewer", version = "0.1.0"))]
struct ApiDoc;

async fn scalar_html() -> Html<&'static str> {
    Html(include_str!("static/scalar.html"))
}

#[tokio::main]
async fn main() -> Result<()> {
    let log_path = setup_logging()?;
    tracing::info!(log_file = %log_path.display(), "Starting...");

    let state = AppState::init().await?;

    let (api_router, openapi) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(routes::library::library))
        .routes(routes!(routes::library::app_info))
        .routes(routes!(routes::library::manifest_info))
        .routes(routes!(routes::library::manifest_files))
        .routes(routes!(routes::library::manifest_statuses))
        .routes(routes!(routes::files::manifest_file))
        .routes(routes!(routes::files::manifest_file_raw))
        .routes(routes!(routes::files::manifest_file_transformed))
        .routes(routes!(routes::structured::manifest_file_structured))
        .routes(routes!(routes::structured::manifest_file_structured_node))
        .routes(routes!(routes::diff::manifest_diff))
        .routes(routes!(routes::diff::file_diff_targets))
        .routes(routes!(routes::diff::manifest_file_diff))
        .routes(routes!(routes::diff::manifest_file_structured_diff))
        .routes(routes!(routes::diff::manifest_file_structured_diff_node))
        .routes(routes!(routes::downloads::manifest_download))
        .routes(routes!(routes::downloads::downloads_snapshot))
        .routes(routes!(routes::downloads::downloads_cancel))
        .routes(routes!(
            routes::config::get_config,
            routes::config::patch_config
        ))
        .routes(routes!(
            routes::extra_manifests::get_extra_manifests,
            routes::extra_manifests::put_extra_manifests
        ))
        .routes(routes!(routes::extra_manifests::delete_extra_manifest))
        .routes(routes!(routes::mount::start))
        .routes(routes!(routes::mount::stop))
        .routes(routes!(routes::mount::status))
        .split_for_parts();

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
