// TODO(ai-review): review for style and correctness
//! Steam client access that waits out the saved-session resume at startup.

use std::sync::Arc;

use arc_swap::ArcSwapOption;
use steam_depot_vfs::{SteamAuth, SteamSession, VfsError};
use tokio::sync::watch;

use super::SteamClient;

/// The logged-in client; while the saved session is still being resumed, it is waited for.
#[derive(Clone)]
pub struct SteamAccess {
    client: Arc<ArcSwapOption<SteamClient>>,
    resuming: watch::Receiver<bool>,
}

impl SteamAccess {
    pub fn new(client: Arc<ArcSwapOption<SteamClient>>, resuming: watch::Receiver<bool>) -> Self {
        Self { client, resuming }
    }

    /// `None` when no client is set once no resume is running.
    pub async fn client(&self) -> Option<Arc<SteamClient>> {
        self.resuming
            .clone()
            .wait_for(|running| !running)
            .await
            .expect("the resume signal lives as long as the app state");
        self.client.load_full()
    }
}

impl SteamAuth for SteamAccess {
    async fn resolve(&self) -> steam_depot_vfs::Result<SteamSession> {
        let client = self
            .client()
            .await
            .ok_or_else(|| VfsError::Other("not logged in to Steam".into()))?;
        client.resolve().await
    }
}

/// Marks the saved-session resume as running until dropped, also when the resume panics.
pub struct ResumeGuard(Arc<watch::Sender<bool>>);

impl ResumeGuard {
    pub fn new(resuming: Arc<watch::Sender<bool>>) -> Self {
        resuming.send_replace(true);
        Self(resuming)
    }
}

impl Drop for ResumeGuard {
    fn drop(&mut self) {
        self.0.send_replace(false);
    }
}
