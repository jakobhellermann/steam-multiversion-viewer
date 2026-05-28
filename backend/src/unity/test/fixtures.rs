// TODO(ai-review): review for style and correctness
//! In-memory SerializedFile fixtures for snapshot tests.
//!
//! `SerializedFileBuilder` lets us assemble tiny scenes (a handful of
//! GameObjects + Transforms, optionally an AssetBundle) without
//! touching any depot / disk. Each fixture is written to a `Vec<u8>`
//! and then re-opened via a minimal in-memory [`EnvResolver`] so the
//! production `build_root_node` / `diff_sections` code paths run
//! against a real rabex `SerializedFileHandle`.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use rabex_env::Environment;
use rabex_env::env::Data;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::UnityVersion;
use rabex_env::rabex::files::serializedfile::builder::SerializedFileBuilder;
use rabex_env::rabex::files::serializedfile::{
    LocalSerializedObjectIdentifier, SerializedType, build_common_offset_map,
};
use rabex_env::rabex::objects::pptr::{FileId, PathId};
use rabex_env::rabex::objects::{ClassId, ClassIdType, PPtr, TypedPPtr};
use rabex_env::rabex::tpk::TpkTypeTreeBlob;
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::rabex::typetree::typetree_cache::sync::TypeTreeCache;
use rabex_env::resolver::EnvResolver;
use rabex_env::unity::types::{ComponentPair, GameObject, MonoBehaviour, MonoScript, Transform};
use serde::Serialize;

/// Default unity version used by every fixture. Picked because the
/// embedded TPK has full coverage and it matches what the prod
/// builder example uses.
pub(super) const TEST_UNITY_VERSION: &str = "2022.3.0f1";

/// Tiny in-memory [`EnvResolver`]. One path → one byte buffer; no
/// listing semantics beyond the fixture set.
pub(super) struct MemResolver {
    files: HashMap<PathBuf, Vec<u8>>,
}

impl MemResolver {
    pub(super) fn single(path: &str, bytes: Vec<u8>) -> Self {
        let mut files = HashMap::new();
        files.insert(PathBuf::from(path), bytes);
        Self { files }
    }
}

impl EnvResolver for MemResolver {
    type Reader<'a>
        = Cursor<&'a [u8]>
    where
        Self: 'a;

    fn read_path(&self, path: &Path) -> Result<Data, std::io::Error> {
        self.files
            .get(path)
            .cloned()
            .map(Data::InMemory)
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, format!("{}", path.display()))
            })
    }

    fn open_path(&self, path: &Path) -> Result<Self::Reader<'_>, std::io::Error> {
        self.files
            .get(path)
            .map(|v| Cursor::new(v.as_slice()))
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, format!("{}", path.display()))
            })
    }

    fn all_files(&self) -> Result<Vec<PathBuf>, std::io::Error> {
        Ok(self.files.keys().cloned().collect())
    }
}

/// Declarative scene description — one gameobject per entry, optional
/// child list. Build with [`Scene::root`] / [`Scene::child`] for
/// readability; the [`Scene::write`] step turns it into the bytes of a
/// SerializedFile.
pub(super) struct Scene {
    roots: Vec<SceneNode>,
    /// Loose AssetBundle entry if requested. Placed at path id 1 (the
    /// builder requires AssetBundle there).
    asset_bundle_name: Option<String>,
}

pub(super) struct SceneNode {
    name: &'static str,
    /// MonoBehaviour scripts attached to this gameobject. Each entry
    /// produces one MB instance plus one MonoScript (deduplicated by
    /// `(namespace, class_name)` at write time so two MBs sharing a
    /// script share a single MonoScript path id).
    scripts: Vec<ScriptRef>,
    children: Vec<SceneNode>,
}

#[derive(Clone, Hash, PartialEq, Eq)]
pub(super) struct ScriptRef {
    pub(super) namespace: &'static str,
    pub(super) class_name: &'static str,
}

impl Scene {
    pub(super) fn new() -> Self {
        Self {
            roots: Vec::new(),
            asset_bundle_name: None,
        }
    }

    pub(super) fn with_root(mut self, node: SceneNode) -> Self {
        self.roots.push(node);
        self
    }

    pub(super) fn with_asset_bundle(mut self, name: &str) -> Self {
        self.asset_bundle_name = Some(name.to_owned());
        self
    }

    pub(super) fn write(&self) -> Vec<u8> {
        let unity_version: UnityVersion = TEST_UNITY_VERSION.parse().unwrap();
        let tpk = TypeTreeCache::new(TpkTypeTreeBlob::embedded());
        let common = build_common_offset_map(&tpk.inner, &unity_version);
        let mut sfb = SerializedFileBuilder::new(&unity_version, &tpk, &common, true);

        // AssetBundle, if requested, must sit at path id 1 — reserve it
        // before any gameobject allocations push the cursor past 1.
        if let Some(name) = &self.asset_bundle_name {
            sfb.add_object_at(
                1,
                &AssetBundle {
                    m_Name: name.clone(),
                    m_RuntimeCompatibility: 1,
                    m_PathFlags: 7,
                    ..Default::default()
                },
            )
            .unwrap();
            // Move the auto-id cursor past the reserved slot.
            sfb.next_path_id = 2;
        }

        let mut scripts = ScriptRegistry::default();
        for root in &self.roots {
            write_node(&mut sfb, &mut scripts, root, TypedPPtr::null());
        }

        sfb.write_vec().unwrap()
    }
}

/// Deduplicates MonoScript objects across MBs that reference the same
/// `(namespace, class_name)`. Caches both the MonoScript path id and the
/// `m_TypeID` of the MonoBehaviour `SerializedType` whose
/// `m_ScriptTypeIndex` resolves to that script.
#[derive(Default)]
struct ScriptRegistry {
    /// (namespace, class) → (monoscript path id, mb type id)
    seen: HashMap<(&'static str, &'static str), (PathId, i32)>,
}

impl ScriptRegistry {
    /// Idempotently register a script. First call adds a MonoScript
    /// object + a MonoBehaviour `SerializedType` + an `m_ScriptTypes`
    /// entry pointing at the new MonoScript; subsequent calls with the
    /// same key are O(1) lookups.
    fn ensure<P: TypeTreeProvider>(
        &mut self,
        sfb: &mut SerializedFileBuilder<'_, P>,
        script: &ScriptRef,
    ) -> (PathId, i32) {
        let key = (script.namespace, script.class_name);
        if let Some(hit) = self.seen.get(&key) {
            return *hit;
        }

        // 1. MonoScript itself — engine class, goes through the normal
        //    type-cache path.
        let mut script_obj = MonoScript {
            m_Name: script.class_name.to_owned(),
            m_ExecutionOrder: 0,
            m_PropertiesHash: [0; 16],
            m_ClassName: script.class_name.to_owned(),
            m_Namespace: script.namespace.to_owned(),
            m_AssemblyName: "Assembly-CSharp.dll".to_owned(),
        };
        // Quiets the unused-mut lint if rabex ever stops needing field
        // mutation here — keeps the binding explicit.
        let _ = &mut script_obj;
        let script_path_id = sfb.add_object(&script_obj).unwrap();

        // 2. m_ScriptTypes entry — references the MonoScript by path id
        //    inside the current file.
        let script_types = sfb.serialized.m_ScriptTypes.as_mut().unwrap();
        let script_type_index: i16 = script_types.len().try_into().unwrap();
        script_types.push(LocalSerializedObjectIdentifier {
            m_LocalSerializedFileIndex: FileId::LOCAL,
            m_LocalIdentifierInFile: script_path_id,
        });

        // 3. SerializedType for this MonoBehaviour variant — same base
        //    MB typetree, but tagged with the script-type index so
        //    `script_type(obj)` can resolve it back to the MonoScript.
        let unity_version = sfb
            .serialized
            .m_UnityVersion
            .as_ref()
            .expect("builder always sets m_UnityVersion")
            .clone();
        let mb_tt = sfb
            .typetree_provider
            .get_typetree_node(ClassId::MonoBehaviour, &unity_version)
            .expect("embedded TPK is missing MonoBehaviour");
        let mut ty = SerializedType::simple(ClassId::MonoBehaviour, Some(mb_tt.into_owned()));
        ty.m_ScriptTypeIndex = script_type_index;
        let mb_type_id = sfb.add_type_uncached(ty);

        self.seen.insert(key, (script_path_id, mb_type_id));
        (script_path_id, mb_type_id)
    }
}

impl SceneNode {
    pub(super) fn new(name: &'static str) -> Self {
        Self {
            name,
            scripts: Vec::new(),
            children: Vec::new(),
        }
    }

    pub(super) fn with_child(mut self, child: SceneNode) -> Self {
        self.children.push(child);
        self
    }

    /// Attach a MonoBehaviour referencing the script identified by
    /// `namespace.class_name`. Use an empty namespace for global types.
    pub(super) fn with_script(mut self, namespace: &'static str, class_name: &'static str) -> Self {
        self.scripts.push(ScriptRef {
            namespace,
            class_name,
        });
        self
    }
}

/// Two-pass write so the parent's `m_Children` already knows every
/// child transform's path id. Returns the transform path id so the
/// caller can wire `m_Father`/`m_Children`.
fn write_node<P: TypeTreeProvider>(
    sfb: &mut SerializedFileBuilder<'_, P>,
    scripts: &mut ScriptRegistry,
    node: &SceneNode,
    parent: TypedPPtr<Transform>,
) -> PathId {
    // Reserve ids ahead of writing so `m_Component`/`m_Children` /
    // `m_GameObject` cross-references resolve before the underlying
    // objects exist.
    let go_id = sfb.get_next_path_id();
    let transform_id = sfb.get_next_path_id();

    // Reserve MonoBehaviour path ids up front too so the GameObject's
    // m_Component vector can name them in declaration order before
    // their bodies are written.
    let mb_ids: Vec<PathId> = node
        .scripts
        .iter()
        .map(|_| sfb.get_next_path_id())
        .collect();

    // Recurse children first; their transform ids fill our m_Children.
    let mut child_transform_ids: Vec<PathId> = Vec::new();
    for child in &node.children {
        let child_t_id = write_node(sfb, scripts, child, TypedPPtr::local(transform_id));
        child_transform_ids.push(child_t_id);
    }

    let mut components = vec![ComponentPair {
        component: PPtr::local(transform_id),
    }];
    for &mb_id in &mb_ids {
        components.push(ComponentPair {
            component: PPtr::local(mb_id),
        });
    }

    let go = GameObject {
        m_Component: components,
        m_Layer: 0,
        m_Name: node.name.to_owned(),
        m_Tag: 0,
        m_IsActive: true,
    };
    sfb.add_object_at(go_id, &go).unwrap();

    let transform = Transform {
        m_GameObject: TypedPPtr::local(go_id),
        m_LocalRotation: (0.0, 0.0, 0.0, 1.0),
        m_LocalPosition: (0.0, 0.0, 0.0),
        m_LocalScale: (1.0, 1.0, 1.0),
        m_Children: child_transform_ids
            .into_iter()
            .map(TypedPPtr::local)
            .collect(),
        m_Father: parent,
    };
    sfb.add_object_at(transform_id, &transform).unwrap();

    for (mb_id, script_ref) in mb_ids.iter().zip(&node.scripts) {
        let (script_path_id, mb_type_id) = scripts.ensure(sfb, script_ref);
        let mb = MonoBehaviour {
            m_GameObject: TypedPPtr::local(go_id),
            m_Enabled: 1,
            m_Script: TypedPPtr::local(script_path_id),
            m_Name: String::new(),
        };
        sfb.add_object_with(&mb, *mb_id, ClassId::MonoBehaviour, mb_type_id)
            .unwrap();
    }

    transform_id
}

/// Minimal AssetBundle for the loose section. We don't care about the
/// container — the diff/tree code only reads class id + m_Name.
#[derive(Debug, Serialize, Default)]
#[allow(non_snake_case)]
struct AssetBundle {
    m_Name: String,
    m_PreloadTable: Vec<PPtr>,
    m_Container: std::collections::BTreeMap<String, AssetInfo>,
    m_MainAsset: AssetInfo,
    m_RuntimeCompatibility: u32,
    m_AssetBundleName: String,
    m_Dependencies: Vec<String>,
    m_IsStreamedSceneAssetBundle: bool,
    m_ExplicitDataLayout: i32,
    m_PathFlags: i32,
    m_SceneHashes: std::collections::BTreeMap<String, String>,
}
impl ClassIdType for AssetBundle {
    const CLASS_ID: ClassId = ClassId::AssetBundle;
}

#[derive(Debug, Serialize, Default)]
#[allow(non_snake_case)]
struct AssetInfo {
    preloadIndex: i32,
    preloadSize: i32,
    asset: PPtr,
}

/// Open scene bytes via a fresh `Environment` and hand the resulting
/// handle to `f`. Closure-shaped so the env's lifetime brackets the
/// handle without callers having to thread it through manually.
pub(super) fn with_handle<R>(
    path: &str,
    bytes: Vec<u8>,
    f: impl FnOnce(&SerializedFileHandle<'_, MemResolver, TypeTreeCache<TpkTypeTreeBlob>>) -> R,
) -> R {
    let resolver = MemResolver::single(path, bytes);
    let tpk = TypeTreeCache::new(TpkTypeTreeBlob::embedded());
    let env = Environment::new(resolver, tpk);
    let handle = env.load_cached(path).unwrap();
    f(&handle)
}

// -----------------------------------------------------------------------
// Bundle fixtures
// -----------------------------------------------------------------------

use rabex_env::rabex::files::bundlefile::{
    BundleFileHeader, BundleFileReader, BundleSignature, CompressionType, ExtractionConfig,
    write_bundle,
};
use rabex_env::rabex::files::unityfile::FileEntry;

/// One entry to add to a test bundle. Either an embedded SerializedFile
/// (`Scene` already written to bytes) or a raw blob of explicit size.
pub(super) enum BundleEntry {
    Serialized { path: String, bytes: Vec<u8> },
    Blob { path: String, bytes: Vec<u8> },
}

/// Build a `.unity3d`-style bundle in memory. Uses the low-level
/// [`write_bundle`] API directly so we can set per-entry flags (the
/// stock `BundleFileBuilder::add_file` forces every entry to flag 4,
/// which would mark blob entries as SerializedFiles and break the
/// bundle-walk dispatch we want to test).
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
        // Concatenate every entry's bytes into one uncompressed blob and
        // build the directory entries with offsets pointing into it.
        let mut uncompressed: Vec<u8> = Vec::new();
        let mut dir: Vec<FileEntry> = Vec::new();
        for entry in self.entries {
            let (path, bytes, flags) = match entry {
                BundleEntry::Serialized { path, bytes } => (path, bytes, 4u32),
                BundleEntry::Blob { path, bytes } => (path, bytes, 0u32),
            };
            let offset = uncompressed.len() as i64;
            let size = bytes.len() as i64;
            uncompressed.extend_from_slice(&bytes);
            dir.push(FileEntry {
                offset,
                size,
                flags,
                path,
            });
        }

        let unity_version: UnityVersion = TEST_UNITY_VERSION.parse().unwrap();
        let header = BundleFileHeader {
            signature: BundleSignature::UnityFS,
            version: 7,
            unity_version: "5.x.x".to_owned(),
            unity_revision: Some(unity_version),
            size: 0,
        };
        let mut out = std::io::Cursor::new(Vec::new());
        write_bundle(
            &header,
            &mut out,
            CompressionType::None,
            CompressionType::None,
            &dir,
            &uncompressed,
        )
        .unwrap();
        out.into_inner()
    }
}

/// Open bundle bytes via a fresh `Environment` and hand the parsed
/// reader to `f`. Same closure shape as [`with_handle`] for the
/// per-file path; tests pick whichever fits.
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
