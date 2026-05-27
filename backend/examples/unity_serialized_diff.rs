// TODO(ai-review): review for style and correctness
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use steam_depot_vfs::DepotStore;
use steam_depot_vfs::session::LazyCachedAuth;
use steam_multiversion_viewer::config::Config;
use steam_multiversion_viewer::structured::{Node, NodeStatus};
use steam_multiversion_viewer::unity::diff;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

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
    let tree = tokio::task::spawn_blocking(move || {
        diff::build_diff(Arc::new(base), Arc::new(target), PATH)
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
