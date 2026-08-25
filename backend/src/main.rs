#![recursion_limit = "256"]
mod config;
mod http;
mod routes;
mod state;
mod static_files;
mod steam;
mod window;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Result;
use axum::Json;
use axum::extract::{MatchedPath, Request};
use axum::middleware::{self, Next};
use axum::response::{Html, Response};
use axum::routing::get;
use clap::Parser;
use directories::ProjectDirs;
use std::time::Instant;
use tokio::task::JoinHandle;
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
manifests, and (optionally) mount the depot as a filesystem.",
))]
struct ApiDoc;

#[derive(Parser)]
struct Args {
    /// Open the frontend in the default browser after starting
    #[arg(long)]
    open: bool,
    /// Open the frontend in a native webview window
    #[arg(long, conflicts_with = "open", default_value_t = cfg!(feature = "windowed"))]
    window: bool,
}

async fn scalar_html() -> Html<&'static str> {
    Html(include_str!("../static/scalar.html"))
}

fn main() -> Result<()> {
    let args = Args::parse();
    let log_path = setup_logging()?;
    tracing::info!(log_file = %log_path.display(), "Starting...");

    let runtime = tokio::runtime::Runtime::new()?;
    let (addr, server, state) = runtime.block_on(start_server())?;

    runtime.spawn(unmount_on_signal(state.clone()));

    if args.window {
        window::run(&format!("http://{addr}"), state, runtime.handle().clone());
    }

    if args.open {
        let url = format!("http://{addr}");
        runtime.spawn_blocking(move || {
            if let Err(e) = open::that(&url) {
                tracing::warn!("failed to open browser: {e}");
            }
        });
    }

    runtime.block_on(server)?
}

/// A mount outlives the process that served it and then hangs every
/// access to it, so a signalled shutdown unmounts before exiting.
/// SIGKILL is not covered — the next start recovers such a mount.
async fn unmount_on_signal(state: AppState) {
    #[cfg(unix)]
    {
        let mut sigterm =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(sigterm) => sigterm,
                Err(e) => {
                    tracing::warn!(%e, "cannot listen for SIGTERM; no unmount on shutdown");
                    return;
                }
            };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    if tokio::signal::ctrl_c().await.is_err() {
        return;
    }
    tracing::info!("signal received, unmounting before exit");
    state.mount.stop_on_shutdown().await;
    std::process::exit(0);
}

async fn start_server() -> Result<(SocketAddr, JoinHandle<Result<()>>, AppState)> {
    let state = AppState::init().await?;

    // Resume the session saved by the last successful login in the
    // background. Registered as a pending login so the UI shows progress
    // instead of an empty login form. Failures are non-fatal, the user
    // logs in via `/login`.
    if let Some(session) = steam::auth::saved_session() {
        let id = steam::auth::begin_startup_login(&state.pending_login, session.account().into());
        let state = state.clone();
        tokio::spawn(async move {
            match steam::auth::resume(session).await {
                Ok((account, connection)) => {
                    state.set_steam(steam::SteamClient::new(account, connection));
                }
                Err(err) => tracing::warn!(%err, "resuming saved session failed"),
            }
            steam::auth::clear_pending(&state.pending_login, id);
        });
    }

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
        .fallback(static_files::serve)
        .layer(middleware::from_fn(http_log))
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:6556").await?;
    let addr = listener.local_addr()?;
    tracing::info!("listening on http://{}", addr);

    let server = tokio::spawn(async move {
        axum::serve(listener, app).await?;
        Ok(())
    });

    Ok((addr, server, state))
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
                "{}=debug,transform=debug,steam_vent_depot=info,steam_depot_vfs=info,tower_http=info",
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
        "steam_multiversion_viewer=debug,transform=debug,steam_vent=debug,steam_vent_depot=debug,steam_depot_vfs=debug,tower_http=info",
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
