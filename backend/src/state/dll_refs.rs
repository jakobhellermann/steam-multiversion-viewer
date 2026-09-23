// TODO(ai-review): review for style and correctness
//! Materialised sibling-assembly directories for ilspycmd's `-r`.
//!
//! Without resolvable references ilspy emits `//IL_xxxx: Unknown result
//! type` stubs and skips expression folding, and it resolves references
//! only from directories on disk. Siblings are stored content-addressed
//! in `{store_root}/refs/pool/<file-sha>` and hardlinked per manifest
//! into `{store_root}/refs/<manifest-id>/<dir>/`.
//!
//! Concurrent materialisations of one directory need no lock: pool
//! writes are atomic, an existing hardlink counts as done, and the
//! `done` marker is written last.

use std::sync::Arc;

use camino::{Utf8Path, Utf8PathBuf};
use steam_depot_vfs::FileType;
use steam_depot_vfs::chunk_store::ChunkStore;
use steam_depot_vfs::fs::DepotManifestStore;

use super::Snapshot;

/// Sibling downloads in flight at once.
const CONCURRENT_FETCHES: usize = 8;

/// Materialise the `*.dll` siblings of `dll_path`'s manifest directory
/// and return their on-disk location; `None` when there are none.
/// `dll_path` may name the file under another platform's build folder.
pub async fn ensure_dll_refs<C: ChunkStore>(
    store_root: &Utf8Path,
    snapshot: &DepotManifestStore<C>,
    dll_path: &str,
) -> Result<Option<Utf8PathBuf>, std::io::Error> {
    let manifest = snapshot.manifest();
    let dll_path = &crate::platform_paths::find_corresponding(manifest, dll_path)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("not in manifest: {dll_path}"),
            )
        })?
        .path;
    let dir = parent_of(dll_path);
    let siblings: Vec<(&str, [u8; 20])> = manifest
        .files
        .iter()
        .filter(|f| f.file_type() == FileType::File && &f.path != dll_path)
        .filter(|f| parent_of(&f.path) == dir && has_dll_extension(&f.path))
        .filter_map(|f| f.sha().map(|h| (f.path.as_str(), h.0)))
        .collect();
    if siblings.is_empty() {
        return Ok(None);
    }

    let refs_dir = store_root
        .join("refs")
        .join(manifest.manifest_id.to_string())
        .join(dir);
    let done = refs_dir.join("done");
    if tokio::fs::try_exists(&done).await? {
        return Ok(Some(refs_dir));
    }
    tokio::fs::create_dir_all(&refs_dir).await?;
    let pool = store_root.join("refs").join("pool");

    for batch in siblings.chunks(CONCURRENT_FETCHES) {
        let links = batch
            .iter()
            .map(|&(path, sha)| link_sibling(snapshot, path, sha, &pool, &refs_dir));
        futures_util::future::try_join_all(links).await?;
    }
    tokio::fs::write(&done, b"").await?;
    Ok(Some(refs_dir))
}

/// In the background, materialise `dll_path`'s siblings and run the bulk
/// decompile against them, so later per-type lookups hit the cache they read.
pub fn warm_with_refs(
    store_root: Utf8PathBuf,
    snapshot: Arc<Snapshot>,
    dll_path: String,
    dll_sha: [u8; 20],
    dll_bytes: Vec<u8>,
) {
    tokio::spawn(async move {
        match ensure_dll_refs(&store_root, &snapshot, &dll_path).await {
            Ok(refs) => transform::dll::warm_full_decompile(&store_root, dll_sha, dll_bytes, refs),
            Err(e) => tracing::error!(%e, dll_path, "dll refs materialisation failed; not warming"),
        }
    });
}

/// Fetch `path` into the pool unless its blob is there, then hardlink it into `refs_dir`.
async fn link_sibling<C: ChunkStore>(
    snapshot: &DepotManifestStore<C>,
    path: &str,
    sha: [u8; 20],
    pool: &Utf8Path,
    refs_dir: &Utf8Path,
) -> Result<(), std::io::Error> {
    let blob = pool.join(transform::cache::sha_hex(&sha));
    if !tokio::fs::try_exists(&blob).await? {
        let bytes = snapshot.read_full(path).await?;
        let blob = blob.clone();
        tokio::task::spawn_blocking(move || {
            transform::cache::write_atomic(blob.as_std_path(), &bytes)
        })
        .await??;
    }
    match tokio::fs::hard_link(&blob, refs_dir.join(basename_of(path))).await {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => Err(e),
        _ => Ok(()),
    }
}

fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn basename_of(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

fn has_dll_extension(path: &str) -> bool {
    path.len() >= 4 && path[path.len() - 4..].eq_ignore_ascii_case(".dll")
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn dll_extension_matches_case_insensitive() {
        assert!(has_dll_extension("Managed/Unity.dll"));
        assert!(has_dll_extension("Managed/Unity.DLL"));
        assert!(!has_dll_extension("Managed/Unity.dll.mdb"));
        assert!(!has_dll_extension("Managed/Unity.dl"));
    }

    #[test]
    fn parent_and_basename_split() {
        assert_eq!(parent_of("a/b/c.dll"), "a/b");
        assert_eq!(parent_of("c.dll"), "");
        assert_eq!(basename_of("a/b/c.dll"), "c.dll");
    }
}
