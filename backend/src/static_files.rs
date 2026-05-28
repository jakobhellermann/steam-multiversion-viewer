// TODO(ai-review): review for style and correctness
//! Serve the Vite-built frontend (`frontend/dist`) out of the same
//! axum binary. The whole asset tree is embedded into the binary at
//! compile time via `rust-embed`, so a release build is a single
//! self-contained executable.
//!
//! Routing:
//!   - `/`                → `index.html`
//!   - `/<asset>`         → byte-identical asset if present in `dist`
//!   - anything else      → `index.html` (SPA fallback so deep links
//!                          like `/apps/.../file` survive a reload —
//!                          the client-side router takes over)
//!
//! The /api/* tree is mounted *before* this fallback in `main.rs`, so
//! the SPA catch-all never shadows backend routes.
//!
//! Dev workflow stays unchanged: `vite dev` on 6555 with a proxy to
//! the backend on 6556. The embed only matters for release builds.

use axum::body::Body;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../frontend/dist"]
struct FrontendAssets;

/// Axum fallback handler — strips the leading slash, looks the path
/// up in [`FrontendAssets`], serves it with a matching `Content-Type`.
/// On miss falls back to `index.html` so single-page-app routes work
/// even on a hard reload.
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let candidate = if path.is_empty() { "index.html" } else { path };
    if let Some(res) = lookup(candidate) {
        return res;
    }
    // SPA fallback: any unknown path under the catch-all returns
    // `index.html` with a 200 so the tanstack router can resolve it
    // client-side. The router itself decides what to show, including
    // its own 404.
    lookup("index.html")
        .unwrap_or_else(|| (StatusCode::NOT_FOUND, "index.html missing from embed").into_response())
}

fn lookup(path: &str) -> Option<Response> {
    let file = FrontendAssets::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    Some(
        (
            [(header::CONTENT_TYPE, mime.as_ref().to_string())],
            Body::from(file.data.into_owned()),
        )
            .into_response(),
    )
}
