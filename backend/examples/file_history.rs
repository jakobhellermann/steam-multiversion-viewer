//! Calls the backend file-history entry point for one file.
//!
//! `cargo run -p steam-multiversion-viewer --example file_history`

use anyhow::{Context, Result};
use steam_multiversion_viewer::history::{FileHistoryRequest, HistoryManifest, build_file_history};
use steam_multiversion_viewer::state::AppState;
use steam_multiversion_viewer::steam::{self, AppId, DepotId, ManifestId};

const APP_ID: u32 = 1030300;
const DEPOT_ID: u32 = 1030301;
const CURRENT_MANIFEST: u64 = 4421626056705534276;
const PATH: &str = "Hollow Knight Silksong_Data/boot.config";

#[tokio::main]
async fn main() -> Result<()> {
    let state = AppState::init().await?;
    let session = steam::auth::saved_session().context("no saved Steam session")?;
    let (account, connection) = steam::auth::resume(session).await?;
    state.set_steam(steam::SteamClient::new(account, connection));

    let previous_manifest = std::env::var("PREVIOUS_MANIFEST")
        .context("set PREVIOUS_MANIFEST to an older manifest id")?
        .parse()
        .context("PREVIOUS_MANIFEST must be a u64")?;
    let request = FileHistoryRequest {
        current: manifest(CURRENT_MANIFEST),
        previous: vec![manifest(previous_manifest)],
        path: PATH.to_owned(),
        node_id: None,
    };
    let history = build_file_history(&state, AppId(APP_ID), &request)
        .await
        .map_err(|error| anyhow::anyhow!(error.message))?;
    println!("{}", serde_json::to_string_pretty(&history)?);
    Ok(())
}

fn manifest(id: u64) -> HistoryManifest {
    HistoryManifest {
        depot_id: DepotId(DEPOT_ID),
        manifest_id: ManifestId(id),
        branch: "public".to_owned(),
    }
}
