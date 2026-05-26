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
use std::sync::Arc;

use anyhow::Result;
use rabex_env::Environment;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::objects::pptr::{PPtr, PathId};
use rabex_env::rabex::tpk::TpkTypeTreeBlob;
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::rabex::typetree::typetree_cache::sync::TypeTreeCache;
use rabex_env::resolver::EnvResolver;
use rabex_env::unity::types::GameObject;
use rabex_env_steam_depot_vfs::SteamDepotGameFiles;
use serde_value::Value;
use steam_depot_vfs::chunk_store::ChunkStore;
use steam_depot_vfs::fs::DepotManifestStore;

use super::markers::{
    as_file_id, as_path_id, color_hex_from_map, color_marker, pptr_from_map, pptr_marker,
};

/// Read the object at `path_id` and pretty-print it as JSON using the
/// typetree, applying [`simplify_for_dump`] on the way out.
#[tracing::instrument(skip_all, fields(path, path_id))]
pub fn dump_object_json<C: ChunkStore + 'static>(
    manifest_store: Arc<DepotManifestStore<C>>,
    path: &str,
    path_id: PathId,
) -> Result<String> {
    let game_files = SteamDepotGameFiles::new(manifest_store)?;
    let data_dir = game_files.data_dir().display().to_string();
    let relative = path.strip_prefix(&format!("{data_dir}/")).unwrap_or(path);
    let tpk = TypeTreeCache::new(TpkTypeTreeBlob::embedded());
    let env = Environment::new(game_files, &tpk);
    let file = env.load_cached(relative)?;
    // Use serde_value::Value as the intermediate — unlike
    // serde_json::Value it has a Bytes variant, so non-UTF-8 string
    // fields (TextAssets that store binary blobs, savegame payloads
    // smuggled through MonoBehaviour, …) deserialize instead of failing
    // with "invalid type: byte array". serde_json then re-encodes
    // Bytes(Vec<u8>) as a JSON array of integers, matching what `jq`
    // already does for similar cases.
    let object = file.object_at::<Value>(path_id)?;
    let mut value = object.read()?;
    simplify_for_dump(&file, &data_dir, "", &mut value);
    Ok(serde_json::to_string_pretty(&value)?)
}

/// Bundle equivalent of [`dump_object_json`]: load the SerializedFile
/// at `archive_entry` inside the bundle at `bundle_path`, then dump
/// `path_id` from it. Builds its own [`Environment`] because the env
/// has to know about the inner SerializedFile by-path (PPtr resolution
/// inside the dump walks back through `env`).
#[tracing::instrument(skip_all, fields(bundle_path, archive_entry, path_id))]
pub fn dump_bundle_object_json<C: ChunkStore + 'static>(
    manifest_store: Arc<DepotManifestStore<C>>,
    bundle_path: &str,
    archive_entry: &str,
    path_id: PathId,
) -> Result<String> {
    use std::io::Cursor;

    use rabex_env::env::Data;
    use rabex_env::rabex::files::SerializedFile;
    use rabex_env::rabex::files::bundlefile::{BundleFileReader, ExtractionConfig};
    use rabex_env::resolver::EnvResolver;

    let game_files = SteamDepotGameFiles::new(manifest_store)?;
    let data_dir = game_files.data_dir().display().to_string();
    let relative = bundle_path
        .strip_prefix(&format!("{data_dir}/"))
        .unwrap_or(bundle_path);
    let bundle_bytes = {
        let _span = tracing::info_span!("read_bundle_bytes").entered();
        game_files.read_path(std::path::Path::new(relative))?
    };
    let tpk = TypeTreeCache::new(TpkTypeTreeBlob::embedded());
    let env = Environment::new(game_files, &tpk);
    let unity_version = {
        let _span = tracing::info_span!("unity_version").entered();
        env.unity_version()?.clone()
    };
    let bundle = {
        let _span = tracing::info_span!("parse_bundle_header").entered();
        BundleFileReader::from_reader(
            Cursor::new(bundle_bytes.as_ref()),
            &ExtractionConfig::default().with_fallback_unity_version(unity_version),
        )?
    };
    let entry_bytes = bundle
        .read_at(archive_entry)?
        .ok_or_else(|| anyhow::anyhow!("entry {archive_entry} not found in bundle"))?;
    let sf = {
        let _span =
            tracing::info_span!("parse_serializedfile", bytes = entry_bytes.len()).entered();
        SerializedFile::from_reader(&mut Cursor::new(entry_bytes.as_slice()))?
    };
    let file = env.insert_cache(archive_entry.into(), sf, Data::InMemory(entry_bytes));
    let value = {
        let _span = tracing::info_span!("read_object").entered();
        let object = file.object_at::<Value>(path_id)?;
        object.read()?
    };
    let value = {
        let _span = tracing::info_span!("simplify_for_dump").entered();
        let mut v = value;
        let archive_prefix = format!("archive:{archive_entry}/");
        simplify_for_dump(&file, &data_dir, &archive_prefix, &mut v);
        v
    };
    let json = {
        let _span = tracing::info_span!("serialize_json").entered();
        serde_json::to_string_pretty(&value)?
    };
    Ok(json)
}

/// Single-pass rewrite of a deserialised object tree:
///
/// - PPtr-shaped maps → `__PPTR__…` marker string,
/// - rgba color maps → `__COLOR__#rrggbbaa` marker string,
/// - non-string-keyed maps → `[{key,value}, …]` list (JSON has no
///   non-string keys),
///
/// in that order of priority per map node. Everything else recurses.
fn simplify_for_dump<R: EnvResolver, P: TypeTreeProvider>(
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
            // immutable in place). This also lets us notice if every
            // key turns out non-string, which means we need to flatten
            // the map into a {key,value} sequence on the way back up.
            let taken = std::mem::take(map);
            let mut all_non_string = !taken.is_empty();
            for (mut k, mut v) in taken {
                simplify_for_dump(file, data_dir, local_ref_prefix, &mut k);
                // Null pptr keys land here as `Value::Unit`; JSON
                // object keys can't be null, so swap in an explicit
                // sentinel that matches the regular pptr null shape.
                if matches!(k, Value::Unit) {
                    k = svalue_str(pptr_marker("", "", "", ""));
                }
                simplify_for_dump(file, data_dir, local_ref_prefix, &mut v);
                if all_non_string && matches!(k, Value::String(_)) {
                    all_non_string = false;
                }
                map.insert(k, v);
            }
            // Homogeneous non-string-keyed map — flatten. A mixed-key
            // map would be a real schema oddity, so we leave those
            // alone (and serde_json will fail loudly downstream).
            if all_non_string {
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
        Value::Option(opt) => {
            if let Some(inner) = opt {
                simplify_for_dump(file, data_dir, local_ref_prefix, inner);
            }
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
        Err(_) => return pptr_placeholder(pptr, "unresolved"),
    };
    let class_id = format!("{:?}", obj.class_id());
    let target = obj
        .read()
        .ok()
        .map(|data| display_name(&obj, &data))
        .unwrap_or_else(|| "(unreadable)".to_string());

    // `ref` always points at the target's tree-row id. Local refs get
    // the caller's prefix (`""` outside bundles, `archive:X/` inside);
    // external refs use a bare `obj:<pathid>` because the route URL
    // already targets a different file by depot path.
    let ref_part = if pptr.is_local() {
        format!("{local_ref_prefix}obj:{}", pptr.m_PathID)
    } else {
        format!("obj:{}", pptr.m_PathID)
    };
    let file_part = if !pptr.is_local() {
        pptr.file_identifier(file.file)
            .map(|ext| external_to_depot_path(data_dir, &ext.pathName))
            .unwrap_or_default()
    } else {
        String::new()
    };
    svalue_str(pptr_marker(&ref_part, &target, &class_id, &file_part))
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
    let go_path = lookup(val, "m_GameObject")
        .and_then(pptr_from_value)
        .and_then(|p| p.optional())
        .and_then(|p| {
            object
                .file
                .deref_optional(p.typed::<GameObject>())
                .ok()
                .flatten()
        })
        .and_then(|go| go.path().ok());

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
