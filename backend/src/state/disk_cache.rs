// TODO(ai-review): review for style and correctness
//! Computed or fetched JSON values persisted under `{store_root}/cache/<key>`, so
//! later requests and preloads can serve them without recomputing or asking Steam.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;
use serde::de::DeserializeOwned;

pub struct DiskCache {
    root: Utf8PathBuf,
}

impl DiskCache {
    pub fn new(store_root: &Utf8Path) -> Self {
        Self {
            root: store_root.join("cache"),
        }
    }

    /// The stored value and when it was written, or `None` if nothing is stored under `key`.
    pub fn read<T: DeserializeOwned>(
        &self,
        key: &str,
    ) -> Result<Option<(T, SystemTime)>, std::io::Error> {
        let path = self.root.join(key);
        let raw = match std::fs::read(&path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let written = std::fs::metadata(&path)?.modified()?;
        let value = serde_json::from_slice(&raw).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{path}: {e}"))
        })?;
        Ok(Some((value, written)))
    }

    pub fn write<T: Serialize>(&self, key: &str, value: &T) -> Result<(), std::io::Error> {
        let path = self.root.join(key);
        let parent = path.parent().expect("cache keys are relative file paths");
        std::fs::create_dir_all(parent)?;
        static NEXT_TMP: AtomicU64 = AtomicU64::new(0);
        let n = NEXT_TMP.fetch_add(1, Ordering::Relaxed);
        let tmp = path.with_extension(format!("tmp.{}.{n}", std::process::id()));
        std::fs::write(
            &tmp,
            serde_json::to_vec(value).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(&tmp, &path)
    }
}
