// TODO(ai-review): review for style and correctness
//! Server-Sent Events stream of background-work events: `stats` (aggregate
//! download progress), `chunks` (per-file `chunks_present` updates) and
//! `export_finished` (final [`ExportStatus`]).

use std::time::Duration;

use axum::extract::State;
use axum::response::Sse;
use axum::response::sse::{Event, KeepAlive};
use futures_util::{Stream, StreamExt};
use tokio_stream::wrappers::BroadcastStream;

use crate::state::AppState;
use crate::state::downloads::DownloadEvent;
use crate::state::export::ExportStatus;

enum AppEvent {
    Download(DownloadEvent),
    ExportFinished(ExportStatus),
}

/// Emits the current download stats as the first event on (re)connect.
pub async fn events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, axum::Error>>> {
    let initial = AppEvent::Download(DownloadEvent::Stats(state.downloads.current()));
    let downloads = BroadcastStream::new(state.downloads.subscribe())
        .filter_map(async |res| res.ok().map(AppEvent::Download));
    let exports = BroadcastStream::new(state.exports.subscribe())
        .filter_map(async |res| res.ok().map(AppEvent::ExportFinished));
    let stream = futures_util::stream::once(async move { initial })
        .chain(futures_util::stream::select(downloads, exports))
        .map(|evt| {
            let (name, payload) = match &evt {
                AppEvent::Download(DownloadEvent::Stats(s)) => ("stats", serde_json::to_value(s)),
                AppEvent::Download(DownloadEvent::Chunks(c)) => ("chunks", serde_json::to_value(c)),
                AppEvent::ExportFinished(s) => ("export_finished", serde_json::to_value(s)),
            };
            let payload = payload.map_err(axum::Error::new)?;
            Event::default()
                .event(name)
                .json_data(&payload)
                .map_err(axum::Error::new)
        });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}
