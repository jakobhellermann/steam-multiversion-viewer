// TODO(ai-review): review for style and correctness
//! In-memory Unity bundle fixtures. Builds `.unity3d`-style bytes
//! through the low-level [`write_bundle`] API so per-entry flags can
//! be set explicitly (the stock `BundleFileBuilder::add_file` forces
//! every entry to flag 4, which would mark blob entries as
//! SerializedFiles and short-circuit the dispatch we want to test).
//!
//! `Scene` from the sibling [`crate::unity::serializedfile::test::fixtures`]
//! is used to populate SerializedFile entries.

use std::io::Cursor;

use rabex_env::Environment;
use rabex_env::rabex::UnityVersion;
use rabex_env::rabex::files::bundlefile::{
    BundleFileBuilder, BundleFileReader, CompressionType, ExtractionConfig,
};
use rabex_env::rabex::files::unityfile::FileEntry;
use rabex_env::rabex::tpk::TpkTypeTreeBlob;
use rabex_env::rabex::typetree::typetree_cache::sync::TypeTreeCache;

use crate::unity::serializedfile::test::fixtures::{MemResolver, TEST_UNITY_VERSION};

/// One entry to add to a test bundle. Either an embedded SerializedFile
/// (`Scene` already written to bytes) or a raw blob of explicit size.
pub(super) enum BundleEntry {
    Serialized { path: String, bytes: Vec<u8> },
    Blob { path: String, bytes: Vec<u8> },
}

pub(super) struct BundleBuilder {
    entries: Vec<BundleEntry>,
}

impl BundleBuilder {
    pub(super) fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub(super) fn add_serialized(mut self, path: &str, bytes: Vec<u8>) -> Self {
        self.entries.push(BundleEntry::Serialized {
            path: path.to_owned(),
            bytes,
        });
        self
    }

    pub(super) fn add_blob(mut self, path: &str, bytes: Vec<u8>) -> Self {
        self.entries.push(BundleEntry::Blob {
            path: path.to_owned(),
            bytes,
        });
        self
    }

    pub(super) fn write(self) -> Vec<u8> {
        let unity_version: UnityVersion = TEST_UNITY_VERSION.parse().unwrap();
        let mut builder = BundleFileBuilder::unityfs(7, &unity_version);

        for entry in self.entries {
            let (path, bytes, flags) = match entry {
                BundleEntry::Serialized { path, bytes } => {
                    (path, bytes, FileEntry::FLAG_SERIALIZEDFILE)
                }
                BundleEntry::Blob { path, bytes } => (path, bytes, 0),
            };

            builder
                .add_file_with_flags(&path, &mut bytes.as_slice(), flags)
                .unwrap();
        }

        let mut out = Cursor::new(Vec::new());
        builder.write(&mut out, CompressionType::None).unwrap();
        out.into_inner()
    }
}

/// Open bundle bytes via a fresh `Environment` and hand the parsed
/// reader to `f`.
pub(super) fn with_bundle<R>(
    bytes: Vec<u8>,
    f: impl FnOnce(
        &Environment<MemResolver, TypeTreeCache<TpkTypeTreeBlob>>,
        &BundleFileReader<Cursor<Vec<u8>>>,
    ) -> R,
) -> R {
    let resolver = MemResolver::single("__unused__", Vec::new());
    let tpk = TypeTreeCache::new(TpkTypeTreeBlob::embedded());
    let env = Environment::new(resolver, tpk);
    let unity_version: UnityVersion = TEST_UNITY_VERSION.parse().unwrap();
    let config = ExtractionConfig::default().with_fallback_unity_version(unity_version);
    let bundle = BundleFileReader::from_reader(Cursor::new(bytes), &config).unwrap();
    f(&env, &bundle)
}
