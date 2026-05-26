// TODO(ai-review): review for style and correctness
//! Drive the viewer's `unity::bundle::build_tree` + `unity::dump_value::dump_bundle_object_json`
//! exactly as the `/file/structured` and `/file/structured/node` routes
//! do, but with no axum / no AppState / no download manager in the
//! way. Useful for profiling or stepping through the rabex-env side of
//! a single bundle in isolation.
//!
//! Hardcoded to Hollow Knight Silksong's `costs.bundle` per the
//! example URL the user pinged; edit the consts at the top for any
//! other depot.
//!
//! Run with:
//! ```sh
//! cargo run -p steam-multiversion-viewer --example bundle_structured_standalone --release
//! ```
//!
//! Needs `STEAM_USERNAME` / `STEAM_PASSWORD` in the env (the auth
//! handshake then cache-promotes a refresh token).

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use steam_depot_vfs::DepotStore;
use steam_depot_vfs::session::LazyCachedAuth;
use steam_multiversion_viewer::unity::{bundle, dump_value};

/// Local depot store; same convention the viewer config uses by
/// default. Override if your store lives somewhere else.
const STORE_ROOT: &str = "/home/jakob/.local/share/steam-multiversion-viewer/store";

const APP_ID: u32 = 1030300;
const DEPOT_ID: u32 = 1030303;
const MANIFEST_ID: u64 = 7921642076658611197;
const BRANCH: &str = "public";

/// Manifest-relative path of the bundle file (matches the `?path=`
/// search param on the structured-view URL).
const BUNDLE_PATH: &str = "Hollow Knight Silksong_Data/StreamingAssets/aa/StandaloneLinux64/dataassets_assets_assets/dataassets/costs.bundle";

/// One specific archive entry inside the bundle + an object inside it
/// (the `archive:<entry>/obj:<pid>` node id the route would receive).
const ARCHIVE_ENTRY: &str = "CAB-1e29d7e3d94b56f1c2801e198547d035";
const OBJECT_PATH_ID: i64 = -5298865543675552381;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    // One indented line per span on close: `  build_loose_section{..}  1.21s`.
    // Tracing's own `FmtSpan::CLOSE` floods the terminal with ANSI/timestamps and
    // a fully-qualified scope chain per line — useless for eyeballing where the
    // wall time goes. `tracing-tree` indents nicely but doesn't print elapsed on
    // close in its current release.
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        "info,steam_multiversion_viewer=debug,rabex_env=info,steam_depot_vfs=warn,rabex_env=warn".into()
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

    let store = DepotStore::new(STORE_ROOT.into());
    let manifest_store = Arc::new(
        store
            .open_depot_manifest(Arc::new(auth), APP_ID, DEPOT_ID, MANIFEST_ID, BRANCH)
            .await?,
    );

    // ---- /file/structured ---------------------------------------------------
    let ms = manifest_store.clone();
    let started = Instant::now();
    let tree = tokio::task::spawn_blocking(move || bundle::build_tree(ms, BUNDLE_PATH)).await??;
    println!(
        "\nbuild_tree: {:?} kind={} entries={}\n",
        started.elapsed(),
        tree.kind,
        tree.root.children.len()
    );

    // ---- /file/structured/node ----------------------------------------------
    let ms = manifest_store.clone();
    let started = Instant::now();
    let json = tokio::task::spawn_blocking(move || {
        dump_value::dump_bundle_object_json(ms, BUNDLE_PATH, ARCHIVE_ENTRY, OBJECT_PATH_ID)
    })
    .await??;
    println!(
        "dump_bundle_object_json: {:?} bytes={}",
        started.elapsed(),
        json.len()
    );

    Ok(())
}
