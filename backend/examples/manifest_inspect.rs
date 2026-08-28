// TODO(ai-review): review for style and correctness
//! Fetches a single manifest and prints structural stats (entry counts by
//! kind, files without chunks or with size 0).
//!
//! `STEAM_USERNAME=… STEAM_PASSWORD=… APP_ID=… DEPOT_ID=… MANIFEST=… [BRANCH=public] cargo run --release --example manifest_inspect`
use std::sync::Arc;

use anyhow::Result;
use steam_depot_vfs::DepotStore;
use steam_depot_vfs::session::LazyCachedAuth;
use steam_multiversion_viewer::config::Config;
use steam_vent_depot::DepotFileKind;

fn env_str(name: &str) -> Result<String> {
    std::env::var(name).map_err(|_| anyhow::anyhow!("missing env var {name}"))
}

fn env_parsed<T: std::str::FromStr>(name: &str) -> Result<T>
where
    T::Err: std::fmt::Display,
{
    let raw = env_str(name)?;
    raw.parse()
        .map_err(|e| anyhow::anyhow!("{name}={raw:?} is not parseable: {e}"))
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,steam_multiversion_viewer=info".into()),
        )
        .init();

    let app_id: u32 = env_parsed("APP_ID")?;
    let depot_id: u32 = env_parsed("DEPOT_ID")?;
    let manifest_id: u64 = env_parsed("MANIFEST")?;
    let branch = std::env::var("BRANCH").unwrap_or_else(|_| "public".to_string());

    let auth = Arc::new(
        LazyCachedAuth::prepare(
            LazyCachedAuth::default_refresh_token_cache(),
            env_str("STEAM_USERNAME")?,
            env_str("STEAM_PASSWORD")?,
        )
        .await?,
    );

    let config = Config::load_or_default()?;
    let store = DepotStore::new(config.store_root.as_std_path().to_path_buf());
    let snap = store
        .open_depot_manifest(auth, app_id, depot_id, manifest_id, &branch)
        .await?;
    let manifest = snap.manifest();

    let mut files = 0usize;
    let mut dirs = 0usize;
    let mut symlinks = 0usize;

    let mut files_without_chunks: Vec<&str> = Vec::new();
    let mut files_size_zero: Vec<&str> = Vec::new();

    for f in &manifest.files {
        match &f.kind {
            DepotFileKind::Directory => dirs += 1,
            DepotFileKind::Symlink { .. } => symlinks += 1,
            DepotFileKind::File { chunks, .. } => {
                files += 1;
                if chunks.is_empty() {
                    files_without_chunks.push(&f.path);
                }
                if f.size == 0 {
                    files_size_zero.push(&f.path);
                }
            }
        }
    }

    fn report(label: &str, paths: &[&str]) {
        println!("{label}: {}", paths.len());
        for p in paths.iter().take(10) {
            println!("    {p}");
        }
        if paths.len() > 10 {
            println!("    … +{} more", paths.len() - 10);
        }
    }

    println!("app {app_id} depot {depot_id} manifest {manifest_id} branch {branch}");
    println!(
        "entries: {} (files {files}, dirs {dirs}, symlinks {symlinks})",
        manifest.files.len()
    );
    println!();
    report("files with zero chunks", &files_without_chunks);
    report("files with size 0", &files_size_zero);

    Ok(())
}
