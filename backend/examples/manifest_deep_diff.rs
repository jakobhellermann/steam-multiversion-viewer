// TODO(ai-review): review for style and correctness
//! Exercises the deep-compare code path (the `manifest_diff_deep`
//! route) standalone and prints timing: for every `changed` Unity file
//! between two manifests, build the structured diff and check whether
//! it's empty (no structured difference). Mirrors the route's logic —
//! env cached once per side, candidates diffed with bounded
//! concurrency — but reads through the VFS instead of the download
//! manager, so run it once warm (chunks already on disk) to measure the
//! pure diff work.
//!
//! `cargo run --release --example manifest_deep_diff`
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use futures_util::StreamExt;
use rabex_env::Environment;
use rabex_env::rabex::tpk::TpkTypeTreeBlob;
use rabex_env::rabex::typetree::typetree_cache::sync::TypeTreeCache;
use rabex_env::resolver::EnvResolver;
use rabex_env_steam_depot_vfs::SteamDepotGameFiles;
use steam_depot_vfs::DepotStore;
use steam_depot_vfs::session::LazyCachedAuth;
use steam_multiversion_viewer::config::Config;
use steam_vent_depot::{DepotFile, FileKind};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use transform::Transformer;
use transform::structured::{NodeStatus, StructuredTree};

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

const APP_ID: u32 = 1030300;
const DEPOT_ID: u32 = 1030303;
const BASE_MANIFEST: u64 = 7921642076658611197;
const TARGET_MANIFEST: u64 = 7780372375671997910;
const BRANCH: &str = "public";

type Env = Environment<SteamDepotGameFiles, TypeTreeCache<TpkTypeTreeBlob>>;

/// `$name` env var, parsed, or `default` if unset.
fn env_or<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "warn,steam_multiversion_viewer=info,transform=info".into());
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_timetree::layer().with_min(Duration::from_millis(10)))
        .init();

    let app_id: u32 = env_or("APP_ID", APP_ID);
    let depot_id: u32 = env_or("DEPOT_ID", DEPOT_ID);
    let base_manifest: u64 = env_or("BASE_MANIFEST", BASE_MANIFEST);
    let target_manifest: u64 = env_or("TARGET_MANIFEST", TARGET_MANIFEST);
    let branch = std::env::var("BRANCH").unwrap_or_else(|_| BRANCH.to_string());

    let auth = Arc::new(
        LazyCachedAuth::prepare(
            LazyCachedAuth::default_refresh_token_cache(),
            std::env::var("STEAM_USERNAME").expect("missing STEAM_USERNAME"),
            std::env::var("STEAM_PASSWORD").expect("missing STEAM_PASSWORD"),
        )
        .await?,
    );

    let config = Config::load_or_default()?;
    let store = DepotStore::new(config.store_root.as_std_path().to_path_buf());
    let base = Arc::new(
        store
            .open_depot_manifest(auth.clone(), app_id, depot_id, base_manifest, &branch)
            .await?,
    );
    let target = Arc::new(
        store
            .open_depot_manifest(auth, app_id, depot_id, target_manifest, &branch)
            .await?,
    );

    // Candidates: same fingerprint test as the route, 1:1.
    fn fp(f: &DepotFile) -> (FileKind, u64, Option<[u8; 20]>, Option<&str>) {
        (f.kind, f.size, f.sha, f.linktarget.as_deref())
    }
    let mut target_by_path = std::collections::HashMap::new();
    for f in &target.manifest().files {
        if !matches!(f.kind, FileKind::Directory) {
            target_by_path.insert(f.path.as_str(), f);
        }
    }
    let mut changed = 0usize;
    let mut candidates: Vec<String> = Vec::new();
    for f in &base.manifest().files {
        if matches!(f.kind, FileKind::Directory) {
            continue;
        }
        let Some(tf) = target_by_path.get(f.path.as_str()) else {
            continue;
        };
        if fp(f) == fp(tf) {
            continue;
        }
        changed += 1;
        if is_deep_comparable(&f.path) {
            candidates.push(f.path.clone());
        }
    }
    println!(
        "changed files: {changed}, of which deep-comparable (unity): {}",
        candidates.len()
    );

    // Env once per side (matches the route's cached scratch env).
    let base_gf = SteamDepotGameFiles::new(base.clone())?;
    let base_data_dir = base_gf.data_dir().display().to_string();
    let base_env = Arc::new(Environment::new(
        base_gf,
        TypeTreeCache::new(TpkTypeTreeBlob::embedded()),
    ));
    let target_gf = SteamDepotGameFiles::new(target.clone())?;
    let target_data_dir = target_gf.data_dir().display().to_string();
    let target_env = Arc::new(Environment::new(
        target_gf,
        TypeTreeCache::new(TpkTypeTreeBlob::embedded()),
    ));

    let concurrency: usize = std::env::var("CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    let started = Instant::now();
    let mut per_file: Vec<(String, Duration, bool)> = futures_util::stream::iter(candidates)
        .map(|path| {
            let base_env = base_env.clone();
            let target_env = target_env.clone();
            let base_data_dir = base_data_dir.clone();
            let target_data_dir = target_data_dir.clone();
            async move {
                tokio::task::spawn_blocking(move || {
                    let t = Instant::now();
                    let result = diff_one(
                        &base_env,
                        &base_data_dir,
                        &target_env,
                        &target_data_dir,
                        &path,
                    );
                    let elapsed = t.elapsed();
                    match result {
                        Ok(tree) => Some((path, elapsed, is_empty(&tree))),
                        Err(err) => {
                            eprintln!("  ! {path}: {err}");
                            None
                        }
                    }
                })
                .await
                .expect("blocking task panicked")
            }
        })
        .buffer_unordered(concurrency)
        .filter_map(|r| async move { r })
        .collect()
        .await;
    let wall = started.elapsed();

    let dropped = per_file.iter().filter(|(_, _, empty)| *empty).count();
    let kept = per_file.len() - dropped;
    let sum_build: Duration = per_file.iter().map(|(_, d, _)| *d).sum();
    println!(
        "\ndeep diff over {} unity files in {:?} (concurrency {concurrency})",
        per_file.len(),
        wall
    );
    println!("  kept (real diff):    {kept}");
    println!("  dropped (no diff):   {dropped}");
    println!(
        "  sum build time:      {:?}  (avg {:?}/file)",
        sum_build,
        sum_build
            .checked_div(per_file.len().max(1) as u32)
            .unwrap_or_default()
    );

    per_file.sort_by_key(|(_, d, _)| std::cmp::Reverse(*d));
    println!("\n  slowest:");
    for (path, d, empty) in per_file.iter().take(10) {
        let name = path.rsplit('/').next().unwrap_or(path);
        println!(
            "    {:>9?}  {}{}",
            d,
            name,
            if *empty { "  (empty)" } else { "" }
        );
    }

    Ok(())
}

fn is_deep_comparable(path: &str) -> bool {
    matches!(
        transform::tools::transformer_for(path),
        Some(Transformer::UnitySerialized | Transformer::UnityBundle)
    )
}

fn is_empty(tree: &StructuredTree) -> bool {
    tree.root.children.is_empty() && matches!(tree.root.status, None | Some(NodeStatus::Unchanged))
}

fn diff_one(
    base_env: &Env,
    base_data_dir: &str,
    target_env: &Env,
    target_data_dir: &str,
    path: &str,
) -> Result<StructuredTree> {
    match transform::tools::transformer_for(path) {
        Some(Transformer::UnitySerialized) => {
            Ok(transform::unity::serializedfile::diff::build_diff(
                base_env,
                base_data_dir,
                target_env,
                target_data_dir,
                path,
            )?)
        }
        Some(Transformer::UnityBundle) => {
            let base_rel = path
                .strip_prefix(&format!("{base_data_dir}/"))
                .unwrap_or(path);
            let target_rel = path
                .strip_prefix(&format!("{target_data_dir}/"))
                .unwrap_or(path);
            let base_bytes = base_env
                .game_files
                .read_path(std::path::Path::new(base_rel))?;
            let target_bytes = target_env
                .game_files
                .read_path(std::path::Path::new(target_rel))?;
            Ok(transform::unity::bundle::build_diff(
                base_env,
                base_bytes,
                target_env,
                target_bytes,
                path,
            )?)
        }
        _ => anyhow::bail!("not unity-deep-comparable: {path}"),
    }
}
