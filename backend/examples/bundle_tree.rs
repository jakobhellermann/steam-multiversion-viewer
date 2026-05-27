// TODO(ai-review): review for style and correctness
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use steam_depot_vfs::DepotStore;
use steam_depot_vfs::session::LazyCachedAuth;
use steam_multiversion_viewer::config::Config;
use steam_multiversion_viewer::unity::bundle;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const APP_ID: u32 = 1030300;
const DEPOT_ID: u32 = 1030303;
const MANIFEST_ID: u64 = 7921642076658611197;
const BRANCH: &str = "public";

const BUNDLE_PATH: &str = "Hollow Knight Silksong_Data/StreamingAssets/aa/StandaloneLinux64/dataassets_assets_assets/dataassets/costs.bundle";

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        "info,steam_multiversion_viewer=debug,rabex_env=info,steam_depot_vfs=warn,rabex_env=warn"
            .into()
    });
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_timetree::layer().with_min(Duration::from_micros(500)))
        .init();

    let auth = LazyCachedAuth::prepare(
        LazyCachedAuth::default_refresh_token_cache(),
        std::env::var("STEAM_USERNAME").expect("missing STEAM_USERNAME"),
        std::env::var("STEAM_PASSWORD").expect("missing STEAM_PASSWORD"),
    )
    .await?;

    let config = Config::load_or_default()?;
    let store = DepotStore::new(config.store_root.as_std_path().to_path_buf());
    let manifest_store = store
        .open_depot_manifest(Arc::new(auth), APP_ID, DEPOT_ID, MANIFEST_ID, BRANCH)
        .await?;

    let started = Instant::now();
    let tree = tokio::task::spawn_blocking(move || {
        bundle::build_tree(Arc::new(manifest_store), BUNDLE_PATH)
    })
    .await??;
    println!(
        "\nbuild_tree: {:?} kind={} entries={}",
        started.elapsed(),
        tree.kind,
        tree.root.children.len()
    );

    Ok(())
}
