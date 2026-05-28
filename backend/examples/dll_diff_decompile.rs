// TODO(ai-review): review for style and correctness
//! Run `dll_diff::diff` against Hollow Knight's `Assembly-CSharp.dll`
//! between the two depot manifests we use as ground-truth, then for
//! every type dll-diff reports as `Changed` invoke `ilspycmd -t` on
//! both sides (via the production
//! [`steam_multiversion_viewer::dll::decompile_type`] path, so the
//! same on-disk cache the viewer uses gets populated) and write the
//! unified diff to a per-type file under `OUT_DIR`. Browsing the
//! `OUT_DIR` afterwards lets us eyeball what dll-diff flags as
//! changed vs. what actually changed in C#.
//!
//! Run with:
//! ```sh
//! cargo run -p steam-multiversion-viewer --example dll_diff_decompile --release
//! ```
//!
//! Needs `STEAM_USERNAME` / `STEAM_PASSWORD` plus `ilspycmd` on
//! PATH (same dependency the route handler has).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use dll_diff::Status;
use steam_depot_vfs::DepotStore;
use steam_depot_vfs::session::LazyCachedAuth;
use steam_multiversion_viewer::config::Config;
use steam_multiversion_viewer::dll;
use steam_multiversion_viewer::unity::serializedfile::dump_value::dump_object_json_unified_diff;
use tokio::sync::Semaphore;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const APP_ID: u32 = 367520;
const DEPOT_ID: u32 = 367523;
const FROM_MANIFEST: u64 = 5829533265112705522;
const TO_MANIFEST: u64 = 708613018541602983;
const BRANCH: &str = "public";

const DLL_PATH: &str = "hollow_knight_Data/Managed/Assembly-CSharp.dll";

/// Where the per-type unified diffs go. Created on demand.
const OUT_DIR: &str = "./dll-diff-output";

/// Cap on concurrent `decompile_type` calls (after the warmup the
/// cache hits in microseconds, so this only really gates the very
/// first run). Higher → more ilspycmd processes if the cache misses.
const PARALLEL_DECOMPILES: usize = 8;

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
                .with_min(Duration::from_millis(5))
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
    let from_manifest = store
        .open_depot_manifest(auth.clone(), APP_ID, DEPOT_ID, FROM_MANIFEST, BRANCH)
        .await?;
    let to_manifest = store
        .open_depot_manifest(auth, APP_ID, DEPOT_ID, TO_MANIFEST, BRANCH)
        .await?;

    // Bytes + the manifest-recorded sha1 — `decompile_type` keys its
    // cache on the sha, so feeding the wrong one would silently
    // duplicate everything in the store.
    let (from_bytes, from_sha) = read_dll(&from_manifest).await?;
    let (to_bytes, to_sha) = read_dll(&to_manifest).await?;
    println!(
        "fetched: from={} bytes (sha={}), to={} bytes (sha={})",
        from_bytes.len(),
        hex(&from_sha),
        to_bytes.len(),
        hex(&to_sha),
    );

    // Pre-warm the per-type cache for both sides synchronously. One
    // ilspycmd -p pass per DLL produces ~all types in ~30-60s vs.
    // 2000+ individual `ilspycmd -t` invocations later.
    let warm_started = Instant::now();
    let (from_warm, to_warm) = tokio::try_join!(
        dll::run_full_decompile(&config.store_root, &from_sha, &from_bytes),
        dll::run_full_decompile(&config.store_root, &to_sha, &to_bytes),
    )?;
    println!(
        "warm-up: {:?} (from cached {} types, to cached {} types)",
        warm_started.elapsed(),
        from_warm,
        to_warm
    );

    // Run dll-diff on the spawn_blocking pool because both
    // Resolution::parse calls are CPU-heavy.
    let diff_started = Instant::now();
    let from_for_diff = from_bytes.clone();
    let to_for_diff = to_bytes.clone();
    let diff =
        tokio::task::spawn_blocking(move || dll_diff::diff(&from_for_diff, &to_for_diff)).await??;
    println!(
        "diff: {:?} {} types total",
        diff_started.elapsed(),
        diff.types.len()
    );

    // Collect every Changed FQN, drop compiler-generated and nested
    // ones (ilspy renders nested types as part of their outer .cs file
    // — emitting per-nested diffs would just duplicate the outer's).
    let mut entity_names: HashSet<String> = HashSet::new();
    for entry in &diff.types {
        if !is_compiler_generated(&entry.fqn) {
            entity_names.insert(entry.fqn.clone());
        }
    }
    let mut changed: Vec<&str> = diff
        .types
        .iter()
        .filter(|t| t.status == Status::Changed)
        .map(|t| t.fqn.as_str())
        .filter(|f| !is_compiler_generated(f))
        .filter(|f| outer_of(f, &entity_names) == *f)
        .collect();
    changed.sort();
    println!(
        "decompile + diff: {} top-level changed types (compiler-gen + nested-inner dropped)",
        changed.len()
    );

    let out_dir = PathBuf::from(OUT_DIR);
    std::fs::create_dir_all(&out_dir).context("creating OUT_DIR")?;
    // Wipe stale outputs from previous runs.
    for entry in std::fs::read_dir(&out_dir)? {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) == Some("diff") {
            let _ = std::fs::remove_file(&path);
        }
    }

    let store_root = config.store_root.clone();
    let from_bytes = Arc::new(from_bytes);
    let to_bytes = Arc::new(to_bytes);
    let sem = Arc::new(Semaphore::new(PARALLEL_DECOMPILES));

    let work_started = Instant::now();
    let mut tasks = tokio::task::JoinSet::new();
    let total = changed.len();
    for fqn in changed {
        let fqn = fqn.to_string();
        let store_root = store_root.clone();
        let from_bytes = from_bytes.clone();
        let to_bytes = to_bytes.clone();
        let out_dir = out_dir.clone();
        let permit = sem.clone().acquire_owned().await?;
        tasks.spawn(async move {
            let _permit = permit;
            decompile_and_write(
                &store_root,
                &from_sha,
                &from_bytes,
                &to_sha,
                &to_bytes,
                &fqn,
                &out_dir,
            )
            .await
        });
    }

    let mut ok = 0usize;
    let mut failed = 0usize;
    while let Some(joined) = tasks.join_next().await {
        match joined? {
            Ok(()) => ok += 1,
            Err(e) => {
                failed += 1;
                eprintln!("  decompile failed: {e}");
            }
        }
        if (ok + failed).is_multiple_of(50) {
            eprintln!("  progress: {}/{total} (failed {failed})", ok + failed);
        }
    }
    println!(
        "wrote {ok} diffs, {failed} failures in {:?}",
        work_started.elapsed()
    );
    println!("output dir: {}", out_dir.display());

    Ok(())
}

async fn read_dll(
    manifest: &steam_depot_vfs::fs::DepotManifestStore<
        impl steam_depot_vfs::chunk_store::ChunkStore,
    >,
) -> Result<(Vec<u8>, [u8; 20])> {
    let file = manifest
        .manifest()
        .files
        .iter()
        .find(|f| f.path == DLL_PATH)
        .with_context(|| format!("file not in manifest: {DLL_PATH}"))?;
    let sha = file
        .sha
        .with_context(|| format!("file has no content sha: {DLL_PATH}"))?;
    let bytes = manifest.read_full(DLL_PATH).await?.to_vec();
    Ok((bytes, sha))
}

async fn decompile_and_write(
    store_root: &camino::Utf8Path,
    from_sha: &[u8; 20],
    from_bytes: &[u8],
    to_sha: &[u8; 20],
    to_bytes: &[u8],
    fqn: &str,
    out_dir: &PathBuf,
) -> Result<()> {
    let (from_text, to_text) = tokio::try_join!(
        dll::decompile_type(store_root, from_sha, from_bytes, fqn),
        dll::decompile_type(store_root, to_sha, to_bytes, fqn),
    )?;
    if from_text == to_text {
        // dll-diff said Changed but ilspy renders the same C# — a
        // false positive at our granularity. Write it anyway with an
        // empty diff body so it's countable; the file size makes it
        // obvious.
    }
    let diff = dump_object_json_unified_diff(&to_text, &from_text, "to", "from");
    let path = out_dir.join(format!("{}.diff", sanitize_fqn(fqn)));
    std::fs::write(&path, diff).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn hex(b: &[u8; 20]) -> String {
    let mut s = String::with_capacity(40);
    for byte in b {
        use std::fmt::Write as _;
        let _ = write!(s, "{byte:02x}");
    }
    s
}

fn sanitize_fqn(fqn: &str) -> String {
    fqn.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect()
}

/// Convention copy from `backend/src/dll/mod.rs::is_compiler_generated`.
fn is_compiler_generated(name: &str) -> bool {
    name.contains('<') || name.contains('>')
}

/// Map a fully-qualified type name to the outermost containing type —
/// same logic as `backend/src/dll/mod.rs::outer_of`. Walk dot
/// positions; the first prefix that's itself a known entity is the
/// outermost type.
fn outer_of(type_name: &str, entities: &HashSet<String>) -> String {
    for (idx, _) in type_name.match_indices('.') {
        let prefix = &type_name[..idx];
        if entities.contains(prefix) {
            return prefix.to_string();
        }
    }
    type_name.to_string()
}
