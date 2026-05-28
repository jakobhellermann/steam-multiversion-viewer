// TODO(ai-review): review for style and correctness
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
use transform::unity::serializedfile::diff;

const APP_ID: u32 = 367520;
const DEPOT_ID: u32 = 367523;
const BASE_MANIFEST: u64 = 708613018541602983;
const TARGET_MANIFEST: u64 = 5829533265112705522;
const BRANCH: &str = "public";

// const PATH: &str = "hollow_knight_Data/globalgamemanagers";
const PATH: &str = "hollow_knight_Data/level100";

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
    let auth = Arc::new(auth);

    let config = Config::load_or_default()?;
    let store = DepotStore::new(config.store_root.as_std_path().to_path_buf());
    let base = store
        .open_depot_manifest(auth.clone(), APP_ID, DEPOT_ID, BASE_MANIFEST, BRANCH)
        .await?;
    let target = store
        .open_depot_manifest(auth, APP_ID, DEPOT_ID, TARGET_MANIFEST, BRANCH)
        .await?;

    let started = Instant::now();
    let tree = tokio::task::spawn_blocking(move || -> Result<_> {
        let base_game_files = SteamDepotGameFiles::new(Arc::new(base))?;
        let base_data_dir = base_game_files.data_dir().display().to_string();
        let base_tpk = TypeTreeCache::new(TpkTypeTreeBlob::embedded());
        let base_env = Environment::new(base_game_files, base_tpk);

        let target_game_files = SteamDepotGameFiles::new(Arc::new(target))?;
        let target_data_dir = target_game_files.data_dir().display().to_string();
        let target_tpk = TypeTreeCache::new(TpkTypeTreeBlob::embedded());
        let target_env = Environment::new(target_game_files, target_tpk);

        Ok(diff::build_diff(
            &base_env,
            &base_data_dir,
            &target_env,
            &target_data_dir,
            PATH,
        )?)
    })
    .await??;
    let elapsed = started.elapsed();

    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    walk(&tree.root, &mut counts);
    println!(
        "\nbuild_diff: {:?} kind={} root_status={:?}",
        elapsed, tree.kind, tree.root.status
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
