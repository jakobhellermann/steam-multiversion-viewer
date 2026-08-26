// TODO(ai-review): review for style and correctness
//! JSON dump of a single Unity object, with the wire-format markers
//! from [`super::markers`] applied.
//!
//! The dump is what the structured-view's right-hand panel renders.
//! It needs to be:
//!
//! - JSON-safe (no non-string map keys),
//! - cross-referenceable (PPtrs become links), and
//! - visual where possible (rgba blobs become swatches).
//!
//! [`simplify_for_dump`] does all three in one traversal.
//!
//! Errors on PPtr resolution are swallowed per-pptr — a bad ref
//! shouldn't break the whole preview.

use std::collections::BTreeMap;

use anyhow::Result;
use rabex_env::Environment;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::objects::ClassId;
use rabex_env::rabex::objects::pptr::{PPtr, PathId};
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;
use rabex_env::unity::types::{GameObject, MonoBehaviour};
use serde_value::Value;

use crate::unity::game_specific::{self, playmaker};

use super::markers::{
    as_file_id, as_path_id, classref_marker, color_hex_from_map, color_marker, pptr_from_map,
    pptr_marker, shape_marker,
};

/// Per-call hooks that influence what a dump returns. Defaulted to
/// "render everything as JSON" so existing tests and CLI tools don't
/// have to thread state they don't care about. Production callers in
/// the backend can opt into class-id-specific rewrites by setting
/// fields like [`DumpOptions::spp_key`].
#[derive(Default, Clone, Copy)]
pub struct DumpOptions<'a> {
    /// `Some(key)` enables `SecPlayerPrefs`-style decryption for
    /// TextAssets: if the asset's `m_Script` decodes as
    /// `base64(AES-256-ECB-PKCS7(utf8))` under this key, the dump
    /// returns the decrypted plaintext instead of the JSON object.
    pub spp_key: Option<&'a [u8]>,
    /// Resolves the enum and layer names a `PlayMakerFSM` carries as
    /// bare integers. `None` leaves them numeric.
    pub playmaker_game: Option<&'a dyn playmaker::GameContextSource>,
}

/// MIME type for a JSON object dump — the regular fall-through return
/// of `dump_object_json` / `dump_bundle_object_json`.
pub const MIME_JSON: &str = "application/json";

/// MIME type for a decrypted TextAsset body that doesn't look like
/// anything more specific.
pub const MIME_PLAIN: &str = "text/plain";

/// MIME type for a decrypted TextAsset body that starts with `<` and
/// ends with `>` — most of the localised language sheets ship as XML
/// so flagging them lets shiki pick the right grammar in the preview.
pub const MIME_XML: &str = "application/xml";

/// Lightweight content sniff for decrypted TextAsset bodies. Just the
/// "looks like XML" check for now — anything else stays `text/plain`.
fn sniff_mime(body: &str) -> &'static str {
    let trimmed = body.trim();
    if trimmed.starts_with('<') && trimmed.ends_with('>') {
        MIME_XML
    } else {
        MIME_PLAIN
    }
}

/// Read the object at `path_id` and pretty-print it as JSON using the
/// typetree, applying [`simplify_for_dump`] on the way out. Returns
/// `(mime, body)` — usually the JSON `application/json` body, but
/// class-id-specific rewrites enabled via [`DumpOptions`] (e.g. a
/// decrypted TextAsset under `spp_key`) can substitute a different
/// representation with a matching MIME.
#[tracing::instrument(skip_all, fields(path, path_id))]
pub fn dump_object_json<R: EnvResolver, P: TypeTreeProvider>(
    env: &Environment<R, P>,
    data_dir: &str,
    path: &str,
    path_id: PathId,
    opts: DumpOptions<'_>,
) -> Result<(&'static str, String)> {
    let relative = path.strip_prefix(&format!("{data_dir}/")).unwrap_or(path);
    let file = env.load_serialized(relative)?;
    dump_object_json_from_handle(&file, data_dir, "", path_id, opts)
}

/// Lazy node content for one shader sub-program in a plain
/// SerializedFile: decompress + decode the program at `blob_index` for
/// `platform`.
pub fn dump_shader_program<R: EnvResolver, P: TypeTreeProvider>(
    env: &Environment<R, P>,
    data_dir: &str,
    path: &str,
    path_id: PathId,
    platform: u32,
    blob_index: u32,
) -> Result<(&'static str, String)> {
    let relative = path.strip_prefix(&format!("{data_dir}/")).unwrap_or(path);
    let file = env.load_serialized(relative)?;
    let value = file.object_at::<Value>(path_id)?.read()?;
    let version = env.unity_version()?.version_tuple();
    super::shader::decode_one_program(&value, platform, blob_index, version)
        .ok_or_else(|| anyhow::anyhow!("could not decode shader program {blob_index}"))
}

/// Pretty-print one object from an already-opened SerializedFile. Used
/// both by the prod manifest-store path above and by tests that
/// assemble files in memory.
pub(crate) fn dump_object_json_from_handle<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    data_dir: &str,
    local_ref_prefix: &str,
    path_id: PathId,
    opts: DumpOptions<'_>,
) -> Result<(&'static str, String)> {
    // Use serde_value::Value as the intermediate — unlike
    // serde_json::Value it has a Bytes variant, so non-UTF-8 string
    // fields (TextAssets that store binary blobs, savegame payloads
    // smuggled through MonoBehaviour, …) deserialize instead of failing
    // with "invalid type: byte array". serde_json then re-encodes
    // Bytes(Vec<u8>) as a JSON array of integers, matching what `jq`
    // already does for similar cases.
    let object = file.object_at::<Value>(path_id)?;
    let class_id = object.class_id();
    let mut value = object.read()?;
    if class_id == ClassId::TextAsset
        && let Some(plain) = try_decrypt_textasset(&value, opts.spp_key)
    {
        return Ok((sniff_mime(&plain), plain));
    }
    if class_id == ClassId::Shader
        && let Some(json) = dump_shader(file, data_dir, local_ref_prefix, &mut value)
    {
        return Ok((MIME_JSON, json));
    }
    if let Some(dumped) = game_specific::try_dump(file, class_id, path_id, opts)? {
        return Ok(dumped);
    }
    simplify_for_dump(file, data_dir, local_ref_prefix, &mut value);
    if class_id == ClassId::MonoScript {
        link_monoscript_classname(data_dir, &mut value);
    }
    Ok((MIME_JSON, serde_json::to_string_pretty(&value)?))
}

/// Shader node content: drop the opaque program blob fields (the
/// programs are navigable as tree children, their source served lazily
/// per node via [`super::shader::decode_one_program`]) and dump the rest
/// (`m_ParsedForm`, properties, …) as usual. `None` for a shader without
/// the blob fields, so the caller falls back to the plain JSON dump.
fn dump_shader<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    data_dir: &str,
    local_ref_prefix: &str,
    value: &mut Value,
) -> Option<String> {
    let Value::Map(map) = value else { return None };
    if !map.contains_key(&svalue_str("compressedBlob")) {
        return None;
    }
    for key in [
        "compressedBlob",
        "offsets",
        "compressedLengths",
        "decompressedLengths",
    ] {
        map.remove(&svalue_str(key));
    }
    simplify_for_dump(file, data_dir, local_ref_prefix, value);
    serde_json::to_string_pretty(value).ok()
}

/// If `value` (a TextAsset) has an `m_Script` that's a base64
/// `SecurePlayerPrefs` blob decryptable under `key`, return the
/// plaintext. `None` keeps the regular JSON dump path. The `TextAsset`
/// class-id gate lives at the call site; here it's just a base64 + AES
/// round-trip.
fn try_decrypt_textasset(value: &Value, key: Option<&[u8]>) -> Option<String> {
    let key = key?;
    // `m_Script` is the encrypted payload. The typetree dumps it as a
    // String when the bytes happen to be valid UTF-8 (true for base64
    // blobs); leave Bytes-variant assets alone since they're not what
    // SecurePlayerPrefs writes anyway.
    let Value::Map(map) = value else { return None };
    let script = map.get(&svalue_str("m_Script"))?;
    let Value::String(blob) = script else {
        return None;
    };
    super::super::secure_player_prefs::decrypt(key, blob)
}

/// For a `MonoScript` object, rewrite its `m_ClassName` string into a
/// `classref` marker linking into the decompiled
/// `<DataDir>/Managed/<Assembly>.dll`. Graceful no-op if the object
/// doesn't carry the expected `m_ClassName` / `m_AssemblyName` string
/// fields (corrupt dump, stripped typetree, …) so the plain string
/// survives instead of a broken marker. The `MonoScript` class-id gate
/// lives at the call site.
///
/// `value` is the already-[`simplify_for_dump`]'d map, so the three
/// fields we read are plain `Value::String`s — we mutate `m_ClassName`
/// in place and leave the rest of the object untouched.
fn link_monoscript_classname(data_dir: &str, value: &mut Value) {
    // `assembly_name()` / `full_name()` mirror rabex's `MonoScript`
    // helpers: the assembly always carries a `.dll` suffix, and the FQN
    // joins a non-empty namespace onto the class name.
    let Some(class_name) = lookup_str(value, "m_ClassName") else {
        return;
    };
    if class_name.is_empty() {
        return;
    }
    let assembly_raw = lookup_str(value, "m_AssemblyName").unwrap_or_default();
    let assembly = if assembly_raw.ends_with(".dll") {
        assembly_raw
    } else {
        format!("{assembly_raw}.dll")
    };
    let namespace = lookup_str(value, "m_Namespace").unwrap_or_default();
    let fqn = if namespace.is_empty() {
        class_name.clone()
    } else {
        format!("{namespace}.{class_name}")
    };

    let Value::Map(map) = value else { return };
    let file = format!("{data_dir}/Managed/{assembly}");
    let marker = classref_marker(&format!("type:{fqn}"), &fqn, &file);
    map.insert(svalue_str("m_ClassName"), svalue_str(marker));
}

/// Bundle equivalent of [`dump_object_json`]: parse `bundle_bytes`,
/// extract `archive_entry`, then dump `path_id` from it. Callers
/// must read the bundle bytes themselves (typically via
/// `env.game_files.read_path`) so the I/O stays visible at the route
/// layer where it can be scheduled inside `spawn_blocking`.
#[tracing::instrument(skip_all, fields(archive_entry, path_id))]
pub fn dump_bundle_object_json<R: EnvResolver, P: TypeTreeProvider>(
    env: &Environment<R, P>,
    data_dir: &str,
    bundle_bytes: rabex_env::env::Data,
    archive_entry: &str,
    path_id: PathId,
    opts: DumpOptions<'_>,
) -> Result<(&'static str, String)> {
    let (file, class_id, value) = read_bundle_object(env, &bundle_bytes, archive_entry, path_id)?;
    if class_id == ClassId::TextAsset
        && let Some(plain) = try_decrypt_textasset(&value, opts.spp_key)
    {
        return Ok((sniff_mime(&plain), plain));
    }
    if let Some(dumped) = game_specific::try_dump(&file, class_id, path_id, opts)? {
        return Ok(dumped);
    }
    let archive_prefix = format!("archive:{archive_entry}/");
    let mut value = value;
    if class_id == ClassId::Shader
        && let Some(json) = dump_shader(&file, data_dir, &archive_prefix, &mut value)
    {
        return Ok((MIME_JSON, json));
    }
    {
        let _span = tracing::info_span!("simplify_for_dump").entered();
        simplify_for_dump(&file, data_dir, &archive_prefix, &mut value);
        if class_id == ClassId::MonoScript {
            link_monoscript_classname(data_dir, &mut value);
        }
    }
    let json = {
        let _span = tracing::info_span!("serialize_json").entered();
        serde_json::to_string_pretty(&value)?
    };
    Ok((MIME_JSON, json))
}

/// Parse a bundle, extract `archive_entry`, and read `path_id` as a
/// dynamic `Value` — the shared setup for the bundle dump paths.
fn read_bundle_object<'env, R: EnvResolver, P: TypeTreeProvider>(
    env: &'env Environment<R, P>,
    bundle_bytes: &rabex_env::env::Data,
    archive_entry: &str,
    path_id: PathId,
) -> Result<(SerializedFileHandle<'env, R, P>, ClassId, Value)> {
    use std::io::Cursor;

    use rabex_env::env::Data;
    use rabex_env::rabex::files::SerializedFile;
    use rabex_env::rabex::files::bundlefile::{BundleFileReader, ExtractionConfig};

    let unity_version = env.unity_version()?.clone();
    let bundle = BundleFileReader::from_reader(
        Cursor::new(bundle_bytes.as_ref()),
        &ExtractionConfig::default().with_fallback_unity_version(unity_version.clone()),
    )?;
    let entry_bytes = bundle
        .read_at(archive_entry)?
        .ok_or_else(|| anyhow::anyhow!("entry {archive_entry} not found in bundle"))?;
    let mut sf = SerializedFile::from_reader(&mut Cursor::new(entry_bytes.as_slice()))?;
    // Bundle entry SerializedFiles usually omit the unity version (it
    // lives at the bundle level), but typetree reads resolve against the
    // file's own version — backfill from the env.
    if sf.m_UnityVersion.is_none() {
        sf.m_UnityVersion = Some(unity_version);
    }
    let file = env.insert_cache(archive_entry.into(), sf, Data::InMemory(entry_bytes));
    let (class_id, value) = {
        let object = file.object_at::<Value>(path_id)?;
        (object.class_id(), object.read()?)
    };
    Ok((file, class_id, value))
}

/// Lazy node content for one shader sub-program inside a bundle.
pub fn dump_bundle_shader_program<R: EnvResolver, P: TypeTreeProvider>(
    env: &Environment<R, P>,
    bundle_bytes: rabex_env::env::Data,
    archive_entry: &str,
    path_id: PathId,
    platform: u32,
    blob_index: u32,
) -> Result<(&'static str, String)> {
    let version = env.unity_version()?.version_tuple();
    let (_file, _class_id, value) = read_bundle_object(env, &bundle_bytes, archive_entry, path_id)?;
    super::shader::decode_one_program(&value, platform, blob_index, version)
        .ok_or_else(|| anyhow::anyhow!("could not decode shader program {blob_index}"))
}

/// Single-pass rewrite of a deserialised object tree:
///
/// - PPtr-shaped maps → `__PPTR__…` marker string,
/// - rgba color maps → `__COLOR__#rrggbbaa` marker string,
/// - non-string-keyed maps → `[{key,value}, …]` list (JSON has no
///   non-string keys),
///
/// in that order of priority per map node. Everything else recurses.
pub(crate) fn simplify_for_dump<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    data_dir: &str,
    // Tree-id prefix for same-file pptrs ("" for bare SerializedFiles,
    // `archive:<entry>/` when dumping an entry inside a bundle). Goes
    // directly into the marker's `ref` field so the frontend's
    // hash-link matches the tree-row id.
    local_ref_prefix: &str,
    value: &mut Value,
) {
    match value {
        Value::Map(map) => {
            // Whole-map rewrites — short-circuit before recursing into
            // children we're about to throw away.
            if let Some(pptr) = pptr_from_map(map) {
                *value = qualify_pptr(file, data_dir, local_ref_prefix, pptr);
                return;
            }
            if let Some(hex) = color_hex_from_map(map) {
                *value = svalue_str(color_marker(&hex));
                return;
            }
            // Take + rebuild so we can mutate keys (BTreeMap keys are
            // immutable in place). This also lets us notice if any
            // key turns out non-string, which means we need to flatten
            // the map into a {key,value} sequence on the way back up.
            // Mixed-key maps happen in practice (e.g. ScriptMapper's
            // `m_ObjectToName` resolves most pptrs to `__MARK__…`
            // strings but stragglers fall back to a
            // `{$target,error}` placeholder map) — we flatten as soon
            // as *any* non-string key shows up so serde_json doesn't
            // explode further down.
            let taken = std::mem::take(map);
            let mut any_non_string = false;
            for (mut k, mut v) in taken {
                simplify_for_dump(file, data_dir, local_ref_prefix, &mut k);
                // A sprite's `m_PhysicsShape` (list of polygons of 2D
                // points) becomes a `shape` marker the frontend draws as
                // a small outline preview.
                if matches!(&k, Value::String(s) if s == "m_PhysicsShape")
                    && let Some(marker) = physics_shape_marker(&v)
                {
                    v = svalue_str(marker);
                }
                // Null pptr keys land here as `Value::Unit`; JSON
                // object keys can't be null, so swap in an explicit
                // sentinel that matches the regular pptr null shape.
                if matches!(k, Value::Unit) {
                    k = svalue_str(pptr_marker("", "", "", ""));
                }
                simplify_for_dump(file, data_dir, local_ref_prefix, &mut v);
                if !matches!(k, Value::String(_)) {
                    any_non_string = true;
                }
                map.insert(k, v);
            }
            if any_non_string {
                let taken = std::mem::take(map);
                let pairs: Vec<Value> = taken
                    .into_iter()
                    .map(|(k, v)| {
                        let mut pair = BTreeMap::new();
                        pair.insert(svalue_str("key"), k);
                        pair.insert(svalue_str("value"), v);
                        Value::Map(pair)
                    })
                    .collect();
                *value = Value::Seq(pairs);
            }
        }
        Value::Seq(items) => {
            for v in items {
                simplify_for_dump(file, data_dir, local_ref_prefix, v);
            }
        }
        Value::Newtype(inner) => simplify_for_dump(file, data_dir, local_ref_prefix, inner),
        Value::Option(Some(inner)) => {
            simplify_for_dump(file, data_dir, local_ref_prefix, inner);
        }
        _ => {}
    }
}

/// Resolve a PPtr and render it as a `pptr` marker string. Failures
/// collapse to either `Value::Unit` (null pptr) or a small
/// `{ $target, error }` placeholder so the surrounding JSON keeps its
/// shape.
fn qualify_pptr<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    data_dir: &str,
    // Prefix glued onto the local `obj:<pathid>` so bundle ids
    // (`archive:<entry>/obj:N`) match the tree-row ids the frontend
    // sees. Empty for bare SerializedFiles. External pptrs ignore it
    // — they're addressed by depot path + bare `obj:<pathid>`.
    local_ref_prefix: &str,
    pptr: PPtr,
) -> Value {
    let Some(pptr) = pptr.optional() else {
        return Value::Unit;
    };
    let obj = match file.deref(pptr.typed::<Value>()) {
        Ok(o) => o,
        Err(e) => {
            // Surface the actual failure (missing external entry, file
            // load error, missing path-id, …) so the placeholder
            // doesn't just say "unresolved" with no clue why.
            let reason = format!("{e:#}");
            tracing::warn!(pptr = ?pptr, %reason, "qualify_pptr deref failed");
            return pptr_placeholder(pptr, &reason);
        }
    };
    let class_id_raw = obj.class_id();
    // For MonoBehaviours the engine class name (`MonoBehaviour`) is
    // uselessly generic — the actual script name is what users think
    // of as the type. Substitute it into the marker's type suffix when
    // available; everything else keeps the engine class name.
    let class_id = if class_id_raw == ClassId::MonoBehaviour {
        obj.cast::<MonoBehaviour>()
            .mono_script()
            .ok()
            .flatten()
            .map(|s| s.full_name().into_owned())
            .unwrap_or_else(|| format!("{class_id_raw:?}"))
    } else {
        format!("{class_id_raw:?}")
    };
    let data = obj.read().ok();
    let target = data
        .as_ref()
        .map(|data| display_name(&obj, data))
        .unwrap_or_else(|| "(unreadable)".to_string());

    // A plain `Transform` isn't listed as its own tree row — it's the
    // hierarchy scaffolding, so the tree builder skips it (see
    // `tree.rs`) and only the GameObject it hangs on gets a row. Point
    // the `ref` at that GameObject's path id so the hash-jump lands
    // somewhere. The `m_GameObject` pptr is file-local (`m_FileID == 0`)
    // so its path id is already in the target file's id space, same as
    // `pptr.m_PathID`. Everything else — other components (Renderer,
    // MonoBehaviour, …, which *do* get their own rows) and loose assets
    // — keeps its own path id. RectTransform is deliberately left out:
    // it carries layout fields the tree doesn't surface, so it's a
    // candidate for its own row later.
    let ref_pid = if class_id_raw == ClassId::Transform {
        data.as_ref()
            .and_then(|data| lookup(data, "m_GameObject"))
            .and_then(pptr_from_value)
            .and_then(|p| p.optional())
            .map(|p| p.m_PathID)
            .unwrap_or(pptr.m_PathID)
    } else {
        pptr.m_PathID
    };

    // `ref` always points at the target's tree-row id. Local refs get
    // the caller's prefix (`""` outside bundles, `archive:X/` inside);
    // external refs use a bare `obj:<pathid>` unless they resolve to
    // an addressables bundle (then `archive:<entry>/obj:<pathid>` so
    // the route lands on the right SerializedFile inside).
    let (file_part, ref_part) = if pptr.is_local() {
        (String::new(), format!("{local_ref_prefix}obj:{ref_pid}"))
    } else {
        let raw_name = pptr
            .file_identifier(file.file)
            .map(|ext| ext.pathName.clone())
            .unwrap_or_default();
        external_target(file.env, data_dir, &raw_name, ref_pid)
    };
    svalue_str(pptr_marker(&ref_part, &target, &class_id, &file_part))
}

/// Resolve an external pptr target into (depot-path, ref-id).
///
/// Three cases:
/// - `Library/foo` → `<DataDir>/Resources/foo`, ref `obj:<pathid>`.
/// - `archive:/<bundle>/<file>` (an addressables CAB) → look the
///   bundle CAB up in [`AddressablesData::cab_to_bundle`] to find
///   the depot-relative bundle path; ref becomes
///   `archive:<file>/obj:<pathid>` to land inside the right
///   SerializedFile in the bundle's tree.
/// - Anything else (plain depot-relative names) → `<DataDir>/<name>`,
///   ref `obj:<pathid>`.
///
/// On any failure for the addressables case (no settings, no bundle
/// for this CAB, etc) we fall back to the raw name so the frontend
/// gets a deterministic — and visibly wrong — depot path rather than
/// a silently broken link.
fn external_target<R: EnvResolver, P: TypeTreeProvider>(
    env: &rabex_env::Environment<R, P>,
    data_dir: &str,
    raw_name: &str,
    path_id: PathId,
) -> (String, String) {
    use std::path::Path;

    use rabex_env::addressables::ArchivePath;

    let bare_ref = format!("obj:{path_id}");

    let Some(archive) = ArchivePath::try_parse(Path::new(raw_name)).ok().flatten() else {
        return (external_to_depot_path(data_dir, raw_name), bare_ref);
    };

    let addressables = env.addressables().ok().flatten();
    let aa_build = env.addressables_build_folder().ok().flatten();
    let bundle_rel = addressables.and_then(|a| a.cab_to_bundle.get(archive.bundle));
    let (Some(aa_build), Some(bundle_rel)) = (aa_build, bundle_rel) else {
        return (external_to_depot_path(data_dir, raw_name), bare_ref);
    };

    let depot_path = format!("{data_dir}/{}/{}", aa_build.display(), bundle_rel.display());
    (
        depot_path,
        format!("archive:{}/obj:{path_id}", archive.file),
    )
}

/// Translate an external-file identifier (as ilspy / rabex prints
/// them, e.g. `Library/unity default resources`) to the manifest-
/// relative depot path the frontend can navigate to. Mirrors the
/// rabex-env-steam-depot-vfs path resolver.
fn external_to_depot_path(data_dir: &str, name: &str) -> String {
    // Unity prefixes builtin-resource file ids with `Library/`; the
    // depot layout has them under `<DataDir>/Resources/` instead.
    // Other names already are data-dir-relative.
    if let Some(rest) = name.strip_prefix("Library/") {
        format!("{data_dir}/Resources/{rest}")
    } else {
        format!("{data_dir}/{name}")
    }
}

fn pptr_placeholder(pptr: PPtr, reason: &str) -> Value {
    let mut map = BTreeMap::new();
    map.insert(
        svalue_str("$target"),
        svalue_str(format!(
            "PPtr<file={:?},path={}>",
            pptr.m_FileID, pptr.m_PathID
        )),
    );
    map.insert(svalue_str("error"), svalue_str(reason));
    Value::Map(map)
}

/// Produce a human-readable label for an object: its `m_Name` plus,
/// where applicable, the gameobject hierarchy path it lives on. Name
/// alone stays unquoted (`Foo`); the moment we tack on a path we
/// quote it to make the boundary obvious (`'Foo' on /Player/Hand`).
fn display_name<R: EnvResolver, P: TypeTreeProvider>(
    object: &rabex_env::handle::ObjectRefHandle<'_, Value, R, P>,
    val: &Value,
) -> String {
    use std::fmt::Write as _;
    let m_name = lookup_str(val, "m_Name").unwrap_or_default();
    // Resolve the owning gameobject's hierarchy path. Each failure here
    // silently degrades a component's label to a bare PathID, so log
    // where it breaks instead of swallowing (transform crate must be in
    // the tracing filter to see it).
    let go_path = lookup(val, "m_GameObject")
        .and_then(pptr_from_value)
        .and_then(|p| p.optional())
        .and_then(|p| {
            match object.file.deref_optional(p.typed::<GameObject>()) {
                Ok(Some(go)) => Some(go),
                Ok(None) => {
                    tracing::warn!(path_id = ?object.path_id(), "display_name: m_GameObject deref returned None");
                    None
                }
                Err(err) => {
                    tracing::warn!(path_id = ?object.path_id(), reason = format!("{err:#}"), "display_name: m_GameObject deref failed");
                    None
                }
            }
        })
        .and_then(|go| match go.path() {
            Ok(path) => Some(path),
            Err(err) => {
                tracing::warn!(path_id = ?object.path_id(), reason = format!("{err:#}"), "display_name: gameobject path() failed");
                None
            }
        });

    let mut out = String::new();
    match (m_name.is_empty(), go_path) {
        (false, Some(path)) => {
            let _ = write!(&mut out, "'{m_name}' on {path}");
        }
        (false, None) => out.push_str(&m_name),
        (true, Some(path)) => out.push_str(&path),
        (true, None) => {}
    }
    if out.is_empty() {
        out.push_str(&format!("PathID={}", object.path_id()));
    }
    out
}

fn lookup<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    let Value::Map(map) = v else {
        return None;
    };
    map.get(&Value::String(key.to_string()))
}

fn lookup_str(v: &Value, key: &str) -> Option<String> {
    match lookup(v, key)? {
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn pptr_from_value(v: &Value) -> Option<PPtr> {
    let Value::Map(map) = v else {
        return None;
    };
    let file_id = as_file_id(map.get(&Value::String("m_FileID".to_string()))?)?;
    let path_id = as_path_id(map.get(&Value::String("m_PathID".to_string()))?)?;
    Some(PPtr::new(file_id, path_id))
}

fn svalue_str(s: impl Into<String>) -> Value {
    Value::String(s.into())
}

/// Build a `shape` marker from a `m_PhysicsShape` value (a list of
/// polygons, each a list of `{x, y}` points). `None` when the value
/// isn't that shape or is empty, leaving the field as-is.
fn physics_shape_marker(value: &Value) -> Option<String> {
    let Value::Seq(polygons) = value else {
        return None;
    };
    let polygons: Option<Vec<Vec<(f32, f32)>>> = polygons
        .iter()
        .map(|poly| match poly {
            Value::Seq(points) => points.iter().map(point_xy).collect(),
            _ => None,
        })
        .collect();
    let polygons = polygons?;
    if polygons.iter().all(Vec::is_empty) {
        return None;
    }
    Some(shape_marker(&polygons))
}

fn point_xy(v: &Value) -> Option<(f32, f32)> {
    let Value::Map(map) = v else { return None };
    let x = as_f32(map.get(&svalue_str("x"))?)?;
    let y = as_f32(map.get(&svalue_str("y"))?)?;
    Some((x, y))
}

fn as_f32(v: &Value) -> Option<f32> {
    match v {
        Value::F32(f) => Some(*f),
        Value::F64(f) => Some(*f as f32),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: f32, y: f32) -> Value {
        let mut m = BTreeMap::new();
        m.insert(svalue_str("x"), Value::F32(x));
        m.insert(svalue_str("y"), Value::F32(y));
        Value::Map(m)
    }

    #[test]
    fn physics_shape_marker_from_polygons() {
        let shape = Value::Seq(vec![Value::Seq(vec![point(0.0, 1.0), point(2.0, 3.0)])]);
        assert_eq!(
            physics_shape_marker(&shape).as_deref(),
            Some("__MARK__shape\u{241e}0,1;2,3")
        );
    }

    #[test]
    fn physics_shape_marker_rejects_empty_and_wrong_shapes() {
        assert!(physics_shape_marker(&Value::Seq(vec![])).is_none());
        assert!(physics_shape_marker(&Value::Seq(vec![Value::Seq(vec![])])).is_none());
        assert!(physics_shape_marker(&svalue_str("nope")).is_none());
        // A list of non-point maps is not a shape.
        let mut m = BTreeMap::new();
        m.insert(svalue_str("foo"), Value::F32(1.0));
        assert!(physics_shape_marker(&Value::Seq(vec![Value::Seq(vec![Value::Map(m)])])).is_none());
    }
}
