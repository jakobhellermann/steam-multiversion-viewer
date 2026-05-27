// TODO(ai-review): review for style and correctness
//! Drive `dll_diff::diff` against the two Hollow Knight Linux-depot
//! manifests used by the other examples: read
//! `Assembly-CSharp.dll` from each side, run the per-type metadata
//! diff, print a status breakdown plus the first changed FQNs.
//!
//! Run with:
//! ```sh
//! cargo run -p steam-multiversion-viewer --example dll_diff_assembly_csharp --release
//! ```
//!
//! Needs `STEAM_USERNAME` / `STEAM_PASSWORD` in the env.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use dll_diff::Status;
use steam_depot_vfs::DepotStore;
use steam_depot_vfs::session::LazyCachedAuth;
use steam_multiversion_viewer::config::Config;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const APP_ID: u32 = 367520;
const DEPOT_ID: u32 = 367523;
const BASE_MANIFEST: u64 = 708613018541602983;
const TARGET_MANIFEST: u64 = 5829533265112705522;
const BRANCH: &str = "public";

const DLL_PATH: &str = "hollow_knight_Data/Managed/Assembly-CSharp.dll";

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        "info,steam_multiversion_viewer=debug,steam_depot_vfs=warn,dotnetdll::convert::read=warn"
            .into()
    });
    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_timetree::layer()
                .with_min(Duration::from_micros(20))
                .with_target(true),
        )
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

    let fetch_started = Instant::now();
    let (base_bytes, target_bytes) =
        tokio::try_join!(base.read_full(DLL_PATH), target.read_full(DLL_PATH))?;
    let fetch_elapsed = fetch_started.elapsed();

    // dll_diff is from→to in GNU-diff sense: an addition is in `to`
    // and not in `from`. BASE_MANIFEST here is the newer 1.5.78 build,
    // TARGET_MANIFEST the older 2021 build, so target is `from`, base
    // is `to`.
    let diff_started = Instant::now();
    let from_bytes = target_bytes.to_vec();
    let to_bytes = base_bytes.to_vec();
    let diff =
        tokio::task::spawn_blocking(move || dll_diff::diff(&from_bytes, &to_bytes)).await??;
    let diff_elapsed = diff_started.elapsed();

    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for t in &diff.types {
        let k = match t.status {
            Status::Added => "added",
            Status::Removed => "removed",
            Status::Changed => "changed",
            Status::Unchanged => "unchanged",
        };
        *counts.entry(k).or_default() += 1;
    }

    println!(
        "\nfetch: {:?} base={} bytes, target={} bytes",
        fetch_elapsed,
        base_bytes.len(),
        target_bytes.len()
    );
    println!("diff:  {:?} {} types total", diff_elapsed, diff.types.len());
    for (k, v) in &counts {
        println!("  {k:<10} {v}");
    }

    let mut changed: Vec<&str> = diff
        .types
        .iter()
        .filter(|t| t.status == Status::Changed)
        .map(|t| t.fqn.as_str())
        .collect();
    changed.sort();
    let limit = 10;
    println!("\nfirst {limit} changed:");
    for fqn in changed.iter().take(limit) {
        println!("  {fqn}");
    }

    Ok(())
}
