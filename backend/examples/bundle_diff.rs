// TODO(ai-review): review for style and correctness
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use steam_depot_vfs::DepotStore;
use steam_depot_vfs::session::LazyCachedAuth;
use steam_multiversion_viewer::config::Config;
use steam_multiversion_viewer::structured::{Node, NodeStatus};
use steam_multiversion_viewer::unity::bundle;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

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

    // Mirror what the HTTP `manifest_file_structured_diff` endpoint
    // does: both snapshots opened in parallel via try_join (cheap when
    // cached, parallel CDN fetches otherwise), then the diff builder
    // dispatched into spawn_blocking. Anything we measure here is
    // close to what the request actually pays.
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
        "\nopen_manifest (both, parallel): {:?}",
        started_open.elapsed()
    );

    let started = Instant::now();
    let tree = tokio::task::spawn_blocking(move || {
        bundle::build_diff(Arc::new(base_snap), Arc::new(target_snap), PATH)
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
