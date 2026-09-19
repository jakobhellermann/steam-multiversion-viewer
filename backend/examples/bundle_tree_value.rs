// TODO(ai-review): review for style and correctness
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use rabex_env::Environment;
use rabex_env::rabex::tpk::TpkTypeTreeBlob;
use rabex_env::rabex::typetree::typetree_cache::sync::TypeTreeCache;
use rabex_env_steam_depot_vfs::SteamDepotGameFiles;
use steam_depot_vfs::DepotStore;
use steam_depot_vfs::session::LazyCachedAuth;
use steam_multiversion_viewer::config::Config;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use transform::unity::serializedfile::dump_value;

const APP_ID: u32 = 1030300;
const DEPOT_ID: u32 = 1030303;
const MANIFEST_ID: u64 = 7921642076658611197;
const BRANCH: &str = "public";

const BUNDLE_PATH: &str = "Hollow Knight Silksong_Data/StreamingAssets/aa/StandaloneLinux64/dataassets_assets_assets/dataassets/costs.bundle";
const ARCHIVE_ENTRY: &str = "CAB-1e29d7e3d94b56f1c2801e198547d035";
const OBJECT_PATH_ID: i64 = -5298865543675552381;

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
    let json = tokio::task::spawn_blocking(move || -> Result<_> {
        let manifest_store = Arc::new(manifest_store);
        let game_files = SteamDepotGameFiles::new(manifest_store)?;
        let data_dir = game_files.data_dir().display().to_string();
        let tpk = TypeTreeCache::new(TpkTypeTreeBlob::embedded());
        let env = Environment::new(game_files, tpk);
        let (_mime, text) = dump_value::dump_bundle_object_json(
            &env,
            &data_dir,
            BUNDLE_PATH,
            ARCHIVE_ENTRY,
            OBJECT_PATH_ID,
            Default::default(),
        )?;
        Ok::<_, anyhow::Error>(text)
    })
    .await??;
    println!(
        "\ndump_bundle_object_json: {:?} bytes={}",
        started.elapsed(),
        json.len()
    );

    Ok(())
}
