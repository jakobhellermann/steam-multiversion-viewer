// TODO(ai-review): review for style and correctness
//! Server-Sent Events stream of background-work events: `stats` (aggregate
//! download progress), `chunks` (per-file `chunks_present` updates),
//! `export_finished` (final [`ExportStatus`]), `auth` ([`AuthStatus`] on connect and on
//! every login change) and `deep_diff_progress` ([`DeepDiffProgress`]).

use std::time::Duration;

use axum::extract::State;
use axum::response::Sse;
use axum::response::sse::{Event, KeepAlive};
use futures_util::{Stream, StreamExt};
use tokio_stream::wrappers::{BroadcastStream, WatchStream};

use crate::routes::auth::AuthStatus;
use crate::routes::diff::manifest::DeepDiffProgress;
use crate::state::AppState;
use crate::state::downloads::DownloadEvent;
use crate::state::export::ExportStatus;

enum AppEvent {
    Download(DownloadEvent),
    ExportFinished(ExportStatus),
    Auth(AuthStatus),
    DeepDiffProgress(DeepDiffProgress),
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
    let (login, resuming) = state.auth_changes();
    let auth_state = state.clone();
    // The login watch yields its current value first, which delivers the status on connect.
    let auth = futures_util::stream::select(
        WatchStream::new(login),
        WatchStream::from_changes(resuming).map(|_| ()),
    )
    .map(move |()| AppEvent::Auth(AuthStatus::from_state(&auth_state)));
    let deep_diffs = BroadcastStream::new(state.deep_diff_progress.subscribe())
        .filter_map(async |res| res.ok().map(AppEvent::DeepDiffProgress));
    let stream = futures_util::stream::once(async move { initial })
        .chain(futures_util::stream::select(
            futures_util::stream::select(downloads, exports),
            futures_util::stream::select(auth, deep_diffs),
        ))
        .map(|evt| {
            let (name, payload) = match &evt {
                AppEvent::Download(DownloadEvent::Stats(s)) => ("stats", serde_json::to_value(s)),
                AppEvent::Download(DownloadEvent::Chunks(c)) => ("chunks", serde_json::to_value(c)),
                AppEvent::ExportFinished(s) => ("export_finished", serde_json::to_value(s)),
                AppEvent::Auth(s) => ("auth", serde_json::to_value(s)),
                AppEvent::DeepDiffProgress(p) => ("deep_diff_progress", serde_json::to_value(p)),
            };
            let payload = payload.map_err(axum::Error::new)?;
            Event::default()
                .event(name)
                .json_data(&payload)
                .map_err(axum::Error::new)
        });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}
