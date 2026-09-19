// TODO(ai-review): review for style and correctness
//! Time the three phases of a bundle structured-diff request end-to-end.
//!
//! Each phase is printed on its own line so the cost of a fresh
//! request (login, depot key fetch, globalgamemanagers read) is split
//! from the steady-state per-request work (build_diff itself).
//!
//! To reproduce the cold-path numbers from a backend daemon's first
//! request after a restart, force the relevant caches to miss:
//!
//! - **CDN discover / connection**: kill the process; this cache is
//!   per-process only (`LazyCachedAuth::inner: OnceCell`).
//! - **Depot key**: same — per-process cache (`LazyDepotKey`).
//! - **`globalgamemanagers` chunk**: delete the chunk file. The path
//!   is `<store_root>/chunks/<sha>` and you can find the sha by
//!   running this example once warm and grepping the timetree output
//!   for `cdn.get` under the `unity_version` span.
//!
//! Setting `RUST_LOG=info,steam_depot_vfs=info,steam_vent=info` shows
//! the underlying steam-depot-vfs log lines (`establishing
//! connection`, `discovering cdn servers`, `fetching depot key`)
//! interleaved with the timetree breakdown.
use std::collections::BTreeMap;
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
use transform::structured::{Node, NodeStatus};
use transform::unity::bundle;

const APP_ID: u32 = 1030300;
const DEPOT_ID: u32 = 1030301;
const BASE_MANIFEST: u64 = 4421626056705534276;
const TARGET_MANIFEST: u64 = 4354007652312393230;
const BASE_BRANCH: &str = "public";
const TARGET_BRANCH: &str = "public-beta";

const PATH: &str = "Hollow Knight Silksong_Data/StreamingAssets/aa/StandaloneWindows64/dataassets_assets_assets/dataassets/costs.bundle";

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        // `steam_depot_vfs=info` so the "establishing connection /
        // discovering cdn servers / fetching depot key" lines show
        // up — those are the lines that explain the cold-path
        // seconds the timetree alone wouldn't account for.
        "info,steam_multiversion_viewer=debug,steam_depot_vfs=info,rabex_env=info".into()
    });
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_timetree::layer().with_min(Duration::from_micros(500)))
        .init();

    let started_auth = Instant::now();
    let auth = LazyCachedAuth::prepare(
        LazyCachedAuth::default_refresh_token_cache(),
        std::env::var("STEAM_USERNAME").expect("missing STEAM_USERNAME"),
        std::env::var("STEAM_PASSWORD").expect("missing STEAM_PASSWORD"),
    )
    .await?;
    let auth = Arc::new(auth);
    println!(
        "\n[phase] auth.prepare (login + CDN discover): {:?}",
        started_auth.elapsed()
    );

    let config = Config::load_or_default()?;
    let store_root = match std::env::var("STORE_ROOT") {
        Ok(p) => std::path::PathBuf::from(p),
        Err(_) => config.store_root.as_std_path().to_path_buf(),
    };
    println!("[cfg] store_root = {}", store_root.display());
    let store = DepotStore::new(store_root);

    let started_open = Instant::now();
    let (base_snap, target_snap) = {
        let auth_b = auth.clone();
        let auth_t = auth.clone();
        let store_ref = &store;
        tokio::try_join!(
            async move {
                anyhow::Ok(
                    store_ref
                        .open_depot_manifest(auth_b, APP_ID, DEPOT_ID, BASE_MANIFEST, BASE_BRANCH)
                        .await?,
                )
            },
            async move {
                anyhow::Ok(
                    store_ref
                        .open_depot_manifest(
                            auth_t,
                            APP_ID,
                            DEPOT_ID,
                            TARGET_MANIFEST,
                            TARGET_BRANCH,
                        )
                        .await?,
                )
            },
        )?
    };
    println!(
        "[phase] open_depot_manifest (both, parallel, incl depot key fetch on cold (app,depot)): {:?}",
        started_open.elapsed()
    );

    let started_diff = Instant::now();
    let tree = tokio::task::spawn_blocking(move || -> Result<_> {
        let base_game_files = SteamDepotGameFiles::new(Arc::new(base_snap))?;
        let base_data_dir = base_game_files.data_dir().display().to_string();
        let base_tpk = TypeTreeCache::new(TpkTypeTreeBlob::embedded());
        let base_env = Environment::new(base_game_files, base_tpk);

        let target_game_files = SteamDepotGameFiles::new(Arc::new(target_snap))?;
        let target_data_dir = target_game_files.data_dir().display().to_string();
        let target_tpk = TypeTreeCache::new(TpkTypeTreeBlob::embedded());
        let target_env = Environment::new(target_game_files, target_tpk);

        bundle::build_diff(
            &base_env,
            &base_data_dir,
            &target_env,
            &target_data_dir,
            PATH,
        )
    })
    .await??;
    let elapsed = started_diff.elapsed();
    println!("[phase] build_diff (spawn_blocking): {:?}", elapsed);

    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    walk(&tree.root, &mut counts);
    println!(
        "\nbuild_diff: {:?} root_status={:?}",
        elapsed, tree.root.status
    );
    for (k, v) in counts {
        println!("  {k:<10} {v}");
    }

    Ok(())
}

fn walk(node: &Node, counts: &mut BTreeMap<&'static str, usize>) {
    if let Some(s) = node.status {
        let key = match s {
            NodeStatus::Added => "added",
            NodeStatus::Removed => "removed",
            NodeStatus::Changed => "changed",
            NodeStatus::Unchanged => "unchanged",
        };
        *counts.entry(key).or_default() += 1;
    }
    for child in &node.children {
        walk(child, counts);
    }
}
