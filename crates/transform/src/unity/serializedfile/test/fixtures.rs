// TODO(ai-review): review for style and correctness
//! In-memory SerializedFile fixtures for snapshot tests.
//!
//! `SerializedFileBuilder` lets us assemble tiny scenes (a handful of
//! GameObjects + Transforms, optionally an AssetBundle) without
//! touching any depot / disk. Each fixture is written to a `Vec<u8>`
//! and then re-opened via a minimal in-memory [`EnvResolver`] so the
//! production `build_root_node` / `diff_sections` code paths run
//! against a real rabex `SerializedFileHandle`.

use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::path::{Path, PathBuf};

use rabex_env::Environment;
use rabex_env::env::Data;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::files::serializedfile::builder::SerializedFileBuilder;
use rabex_env::rabex::files::serializedfile::{
    Endianness, LocalSerializedObjectIdentifier, SerializedType, build_common_offset_map,
};
use rabex_env::rabex::objects::pptr::{FileId, PathId};
use rabex_env::rabex::objects::{ClassId, ClassIdType, PPtr, TypedPPtr};
use rabex_env::rabex::tpk::TpkTypeTreeBlob;
use rabex_env::rabex::typetree::typetree_cache::sync::TypeTreeCache;
use rabex_env::rabex::typetree::{TypeTreeNode, TypeTreeProvider};
use rabex_env::rabex::{UnityVersion, serde_typetree};
use rabex_env::resolver::EnvResolver;
use rabex_env::unity::types::{ComponentPair, GameObject, MonoBehaviour, MonoScript, Transform};
use serde::Serialize;

/// Default unity version used by every fixture. Picked because the
/// embedded TPK has full coverage and it matches what the prod
/// builder example uses.
pub(crate) const TEST_UNITY_VERSION: &str = "2022.3.0f1";

/// Tiny in-memory [`EnvResolver`]. One path → one byte buffer; no
/// listing semantics beyond the fixture set.
pub(crate) struct MemResolver {
    files: HashMap<PathBuf, Vec<u8>>,
}

impl MemResolver {
    pub(crate) fn single(path: &str, bytes: Vec<u8>) -> Self {
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
pub(crate) struct Scene {
    roots: Vec<SceneNode>,
    /// Loose AssetBundle entry if requested. Placed at path id 1 (the
    /// builder requires AssetBundle there).
    asset_bundle_name: Option<String>,
}

pub(crate) struct SceneNode {
    name: &'static str,
    /// MonoBehaviour scripts attached to this gameobject. Each entry
    /// produces one MB instance plus one MonoScript (deduplicated by
    /// `(namespace, class_name)` at write time so two MBs sharing a
    /// script share a single MonoScript path id).
    scripts: Vec<ScriptRef>,
    children: Vec<SceneNode>,
}

#[derive(Clone, Hash, PartialEq, Eq)]
pub(crate) struct ScriptRef {
    pub(crate) namespace: &'static str,
    pub(crate) class_name: &'static str,
}

impl Scene {
    pub(crate) fn new() -> Self {
        Self {
            roots: Vec::new(),
            asset_bundle_name: None,
        }
    }

    pub(crate) fn with_root(mut self, node: SceneNode) -> Self {
        self.roots.push(node);
        self
    }

    pub(crate) fn with_asset_bundle(mut self, name: &str) -> Self {
        self.asset_bundle_name = Some(name.to_owned());
        self
    }

    pub(crate) fn write(&self) -> Vec<u8> {
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
    pub(crate) fn new(name: &'static str) -> Self {
        Self {
            name,
            scripts: Vec::new(),
            children: Vec::new(),
        }
    }

    pub(crate) fn with_child(mut self, child: SceneNode) -> Self {
        self.children.push(child);
        self
    }

    /// Attach a MonoBehaviour referencing the script identified by
    /// `namespace.class_name`. Use an empty namespace for global types.
    pub(crate) fn with_script(mut self, namespace: &'static str, class_name: &'static str) -> Self {
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
    m_Container: BTreeMap<String, AssetInfo>,
    m_MainAsset: AssetInfo,
    m_RuntimeCompatibility: u32,
    m_AssetBundleName: String,
    m_Dependencies: Vec<String>,
    m_IsStreamedSceneAssetBundle: bool,
    m_ExplicitDataLayout: i32,
    m_PathFlags: i32,
    m_SceneHashes: BTreeMap<String, String>,
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
pub(crate) fn with_handle<R>(
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
// Engine-typed fixtures
// -----------------------------------------------------------------------

/// Serializable `LensFlare` matching the embedded TPK's typetree for
/// `2022.3.0f1`. Used as a loose-object fixture so dump tests get to
/// see a real `ColorRGBA { r,g,b,a:float }` come out of the
/// `serde_typetree` deserialiser (and thus drive the
/// `simplify_for_dump` color-marker rewrite end to end).
#[derive(Serialize)]
#[allow(non_snake_case)]
pub(crate) struct LensFlare {
    pub m_GameObject: TypedPPtr<GameObject>,
    pub m_Enabled: u8,
    pub m_Flare: PPtr,
    pub m_Color: ColorRgba,
    pub m_Brightness: f32,
    pub m_FadeSpeed: f32,
    pub m_IgnoreLayers: BitField,
    pub m_Directional: bool,
}
impl ClassIdType for LensFlare {
    const CLASS_ID: ClassId = ClassId::LensFlare;
}

#[derive(Serialize)]
#[allow(non_snake_case)]
pub(crate) struct ColorRgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
pub(crate) struct BitField {
    pub m_Bits: u32,
}

/// Build a tiny serialized file containing one loose `LensFlare`
/// with the given color. Returns `(bytes, path_id)` so the test can
/// drive `dump_object_json_from_handle` against the known id.
pub(crate) fn scene_with_lens_flare(color: ColorRgba) -> (Vec<u8>, PathId) {
    let unity_version: UnityVersion = TEST_UNITY_VERSION.parse().unwrap();
    let tpk = TypeTreeCache::new(TpkTypeTreeBlob::embedded());
    let common = build_common_offset_map(&tpk.inner, &unity_version);
    let mut sfb = SerializedFileBuilder::new(&unity_version, &tpk, &common, true);
    let lens = LensFlare {
        m_GameObject: TypedPPtr::null(),
        m_Enabled: 1,
        m_Flare: PPtr::default(),
        m_Color: color,
        m_Brightness: 1.0,
        m_FadeSpeed: 3.0,
        m_IgnoreLayers: BitField { m_Bits: 0 },
        m_Directional: false,
    };
    let path_id = sfb.add_object(&lens).unwrap();
    (sfb.write_vec().unwrap(), path_id)
}

/// MonoBehaviour body with arbitrary appended fields. Mirrors the base
/// MB layout (m_GameObject, m_Enabled, m_Script, m_Name) plus a
/// ColorRGBA and an `int → int` map — picked because both shapes
/// drive interesting branches in `simplify_for_dump`.
#[derive(Serialize)]
#[allow(non_snake_case)]
pub(crate) struct CustomMbBody {
    pub m_GameObject: TypedPPtr<GameObject>,
    pub m_Enabled: u8,
    pub m_Script: TypedPPtr<MonoScript>,
    pub m_Name: String,
    pub m_TintColor: ColorRgba,
    pub m_Lookup: BTreeMap<i32, i32>,
}

/// Build a leaf typetree node. Defaults are fine for serialization —
/// the embedded TPK's MetaFlags etc. only matter at parse-time, the
/// serializer dispatches on `m_Type` strings and walks `children`.
// m_MetaFlag has to be Some — TypeTreeNode::hash unwraps it. 0 is fine
// since no alignment bits matter for the simple scalar/map shapes we
// build here.
fn tt_leaf(ty: &str, name: &str) -> TypeTreeNode {
    TypeTreeNode {
        m_Type: ty.to_string(),
        m_Name: name.to_string(),
        m_MetaFlag: Some(0),
        m_Index: Some(0),
        ..Default::default()
    }
}

fn tt_node(ty: &str, name: &str, children: Vec<TypeTreeNode>) -> TypeTreeNode {
    TypeTreeNode {
        m_Type: ty.to_string(),
        m_Name: name.to_string(),
        m_MetaFlag: Some(0),
        m_Index: Some(0),
        children,
        ..Default::default()
    }
}

/// Re-stamp `m_Level` on every node based on depth from the root.
/// The TT serializer flattens the tree to a sequence and uses
/// `m_Level` to recover parent/child structure when reading back.
fn fix_levels(node: &mut TypeTreeNode, depth: u8) {
    node.m_Level = depth;
    for child in &mut node.children {
        fix_levels(child, depth + 1);
    }
}

/// Build a SerializedFile containing one MonoBehaviour with a custom
/// typetree that appends a `ColorRGBA m_TintColor` and a
/// `map<int,int> m_Lookup` after the standard four MB fields. Returns
/// `(bytes, path_id)` so the dump test knows where to look.
pub(crate) fn scene_with_custom_mb(body: CustomMbBody) -> (Vec<u8>, PathId) {
    let unity_version: UnityVersion = TEST_UNITY_VERSION.parse().unwrap();
    let tpk = TypeTreeCache::new(TpkTypeTreeBlob::embedded());
    let common = build_common_offset_map(&tpk.inner, &unity_version);
    let mut sfb = SerializedFileBuilder::new(&unity_version, &tpk, &common, true);

    // Register a MonoScript that the MonoBehaviour can point at —
    // gets m_ScriptTypeIndex 0 in the brand-new m_ScriptTypes.
    let script = MonoScript {
        m_Name: "CustomBehaviour".to_owned(),
        m_ExecutionOrder: 0,
        m_PropertiesHash: [0; 16],
        m_ClassName: "CustomBehaviour".to_owned(),
        m_Namespace: "Test".to_owned(),
        m_AssemblyName: "Assembly-CSharp.dll".to_owned(),
    };
    let script_path_id = sfb.add_object(&script).unwrap();
    let script_types = sfb.serialized.m_ScriptTypes.as_mut().unwrap();
    let script_type_index: i16 = script_types.len().try_into().unwrap();
    script_types.push(LocalSerializedObjectIdentifier {
        m_LocalSerializedFileIndex: FileId::LOCAL,
        m_LocalIdentifierInFile: script_path_id,
    });

    // Clone the base MB typetree and append our two custom fields.
    let base_mb = sfb
        .typetree_provider
        .get_typetree_node(ClassId::MonoBehaviour, &unity_version)
        .expect("embedded TPK is missing MonoBehaviour")
        .into_owned();
    let mut extended = base_mb;
    extended.children.push(tt_node(
        "ColorRGBA",
        "m_TintColor",
        vec![
            tt_leaf("float", "r"),
            tt_leaf("float", "g"),
            tt_leaf("float", "b"),
            tt_leaf("float", "a"),
        ],
    ));
    extended.children.push(tt_node(
        "map",
        "m_Lookup",
        vec![tt_node(
            "Array",
            "Array",
            vec![
                tt_leaf("int", "size"),
                tt_node(
                    "pair",
                    "data",
                    vec![tt_leaf("int", "first"), tt_leaf("int", "second")],
                ),
            ],
        )],
    ));
    // The TT serializer encodes the tree shape via per-node `m_Level`
    // (root=0, direct child=1, …). Our synthesized appended nodes have
    // m_Level=0 from `Default::default()`, which would confuse the
    // reader when it tries to recover parent/child relationships.
    // Walk the whole TT post-append and stamp the right levels.
    fix_levels(&mut extended, 0);
    let mut ty = SerializedType::simple(ClassId::MonoBehaviour, Some(extended));
    ty.m_ScriptTypeIndex = script_type_index;
    let mb_type_id = sfb.add_type_uncached(ty);

    let body = CustomMbBody {
        m_Script: TypedPPtr::local(script_path_id),
        ..body
    };
    // `add_object_with` would re-fetch the *base* MB typetree from
    // the TPK and ignore our extended one, so the m_TintColor /
    // m_Lookup fields wouldn't find matching TT children. Serialize
    // directly against the extended TT we just registered, then hand
    // the bytes off via the untyped slot.
    let extended_tt = &sfb.serialized.m_Types[mb_type_id as usize]
        .m_Type
        .as_ref()
        .expect("type tree present on m_Types entry we just added");
    let data = serde_typetree::to_vec_endianed(&body, extended_tt, Endianness::Little).unwrap();
    let mb_path_id = sfb.get_next_path_id();
    sfb.add_object_untyped_with(
        mb_path_id,
        ClassId::MonoBehaviour,
        mb_type_id,
        std::borrow::Cow::Owned(data),
    )
    .unwrap();

    (sfb.write_vec().unwrap(), mb_path_id)
}
