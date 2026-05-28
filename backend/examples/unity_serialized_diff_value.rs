// TODO(ai-review): review for style and correctness
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use steam_depot_vfs::DepotStore;
use steam_depot_vfs::session::LazyCachedAuth;
use steam_multiversion_viewer::config::Config;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use transform::unity::serializedfile::dump_value;

const APP_ID: u32 = 367520;
const DEPOT_ID: u32 = 367523;
const BASE_MANIFEST: u64 = 708613018541602983;
const TARGET_MANIFEST: u64 = 5829533265112705522;
const BRANCH: &str = "public";

const PATH: &str = "hollow_knight_Data/globalgamemanagers";
const BASE_PATH_ID: i64 = 11;
const TARGET_PATH_ID: i64 = 11;

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
    let (base_text, target_text) =
        tokio::task::spawn_blocking(move || -> Result<(String, String)> {
            let b = dump_value::dump_object_json(
                Arc::new(base),
                PATH,
                BASE_PATH_ID,
                dump_value::DumpSide::Base,
            )?;
            let t = dump_value::dump_object_json(
                Arc::new(target),
                PATH,
                TARGET_PATH_ID,
                dump_value::DumpSide::Target,
            )?;
            Ok((b, t))
        })
        .await??;
    let dump_elapsed = started.elapsed();

    let diff_started = Instant::now();
    let diff =
        dump_value::dump_object_json_unified_diff(&base_text, &target_text, "base", "target");
    let diff_elapsed = diff_started.elapsed();

    println!(
        "\ndump_object_json (both sides): {:?} base_bytes={} target_bytes={}",
        dump_elapsed,
        base_text.len(),
        target_text.len()
    );
    println!("unified_diff: {:?} {} bytes\n", diff_elapsed, diff.len());
    println!("{diff}");

    Ok(())
}
