// TODO(ai-review): review for style and correctness
//! .NET assembly support via `ilspycmd`. Three operations:
//!
//! - [`list_entities`] runs `ilspycmd -l "c i s d e"` once per
//!   DLL-sha and caches the line-oriented output. Used to build the
//!   structured tree.
//! - [`decompile_type`] runs `ilspycmd -t <fully-qualified-name>` for
//!   a single type and caches the C# source.
//! - [`warm_full_decompile`] runs `ilspycmd -p --nested-directories`
//!   in the background and populates the per-type cache in bulk.
//!   Optional — `decompile_type` already covers the user-visible
//!   latency; the warmer just makes follow-up clicks free once the
//!   `-p` pass finishes.
//!
//! Per the bench notes (`ilspycmd-bench.md`), per-type latency is
//! dominated by the ~0.25 s assembly load. `-p` pays ~13 s wall once
//! and decompiles every type; the crossover where it's worth firing
//! it lives at ~10 type fetches — so we just kick it off whenever a
//! user opens a DLL, dedup per-sha, and trust the on-disk cache to
//! coalesce.

use std::collections::HashSet;
use std::process::Stdio;
use std::sync::Mutex;

use camino::Utf8Path;
use tokio::process::Command;

use crate::transform::{TempInput, TransformError, tempfile_for};

pub mod diff;
pub mod tree;

#[cfg(test)]
mod test;

/// Where to find ilspycmd. Plain PATH lookup for now; revisit when
/// we ship installers.
const ILSPYCMD: &str = "ilspycmd";

/// Per-process set of DLL shas with a `-p` warmer in flight. Prevents
/// firing the (multi-second, multi-CPU) bulk pass twice for the same
/// file when, say, two browser tabs hit the same DLL at once.
static WARMUPS_IN_FLIGHT: Mutex<Option<HashSet<[u8; 20]>>> = Mutex::new(None);

fn warmups_lock() -> std::sync::MutexGuard<'static, Option<HashSet<[u8; 20]>>> {
    WARMUPS_IN_FLIGHT.lock().expect("warmups poisoned")
}

/// A type discovered by `ilspycmd -l`.
#[derive(Debug, Clone)]
pub struct EntityEntry {
    /// `Class`, `Interface`, `Struct`, `Delegate`, or `Enum`. ilspy
    /// prints them as the first whitespace-separated token.
    pub kind: EntityKind,
    /// Fully qualified name as ilspy prints it (e.g.
    /// `UnityEngine.UI.Image` or `<>f__AnonymousType0`).
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    Class,
    Interface,
    Struct,
    Delegate,
    Enum,
}

impl EntityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Class => "class",
            Self::Interface => "interface",
            Self::Struct => "struct",
            Self::Delegate => "delegate",
            Self::Enum => "enum",
        }
    }

    fn from_ilspy(label: &str) -> Option<Self> {
        match label {
            "Class" => Some(Self::Class),
            "Interface" => Some(Self::Interface),
            "Struct" => Some(Self::Struct),
            "Delegate" => Some(Self::Delegate),
            "Enum" => Some(Self::Enum),
            _ => None,
        }
    }
}

/// List every class/interface/struct/delegate/enum in the assembly.
/// Cached under `transforms/<dll-sha>/list.txt.zst` so repeated calls
/// are file-system instant.
pub async fn list_entities(
    store_root: &Utf8Path,
    dll_sha: &[u8; 20],
    dll_bytes: &[u8],
) -> Result<Vec<EntityEntry>, TransformError> {
    let raw = if let Some(cached) =
        crate::transform::read_cached_artifact(store_root, dll_sha, "list")?
    {
        cached
    } else {
        let raw = run_ilspy(dll_bytes, &["-l", "c i s d e"]).await?;
        crate::transform::write_cached_artifact(store_root, dll_sha, "list", &raw)?;
        raw
    };
    Ok(parse_entity_list(&raw))
}

/// Parse `Class Foo.Bar` / `Enum Baz` lines. Skips anything that
/// doesn't match the two-token shape — keeps the parser tolerant if
/// ilspy ever decorates the output (warnings on stderr already, but
/// stdout has been stable for years). Compiler-generated types
/// (`<Module>`, `<PrivateImplementationDetails>…`) are dropped: ilspy
/// can't decompile them anyway (empty for `<Module>`, hard error for
/// `<PrivateImplementationDetails>`), so they'd only be dead rows.
fn parse_entity_list(raw: &str) -> Vec<EntityEntry> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((kind, name)) = line.split_once(' ') else {
            continue;
        };
        let Some(kind) = EntityKind::from_ilspy(kind) else {
            continue;
        };
        if is_compiler_generated(name) {
            continue;
        }
        out.push(EntityEntry {
            kind,
            name: name.to_string(),
        });
    }
    out
}

/// C# convention: any identifier containing `<` or `>` was emitted by
/// the compiler. Covers `<Module>`, `<PrivateImplementationDetails>…`,
/// `<>f__AnonymousType0`, `<>c__DisplayClass0_0`, async-state-machine
/// classes like `<MoveNextAsync>d__7`, etc.
fn is_compiler_generated(type_name: &str) -> bool {
    type_name.contains('<') || type_name.contains('>')
}

/// Decompile a single type to C#. `type_name` must be the fully
/// qualified name as printed by [`list_entities`] (`Outer.Inner` for
/// nested types). For nested types we normalise both the cache key
/// and the ilspy invocation to the outer type: ilspy renders the
/// outer's full decompilation regardless of whether you ask for the
/// nested via `Outer+Inner` or just `Outer`, and the warmer's `-p`
/// pass only produces the outer file — so reusing that one cache
/// entry makes nested clicks free once the warmer has run.
pub async fn decompile_type(
    store_root: &Utf8Path,
    dll_sha: &[u8; 20],
    dll_bytes: &[u8],
    type_name: &str,
) -> Result<String, TransformError> {
    let resolved = resolve_outer(store_root, dll_sha, dll_bytes, type_name).await?;
    let artifact = type_artifact_name(&resolved);
    if let Some(cached) = crate::transform::read_cached_artifact(store_root, dll_sha, &artifact)? {
        return Ok(cached);
    }
    let text = run_ilspy(dll_bytes, &["-t", &resolved]).await?;
    crate::transform::write_cached_artifact(store_root, dll_sha, &artifact, &text)?;
    Ok(text)
}

/// Map a fully-qualified type name to the outermost containing type
/// (= what ilspy renders as one .cs file). `Foo.Bar.Inner.More` with
/// `Foo.Bar` as a known entity → `Foo.Bar`. Non-nested names come
/// back unchanged.
async fn resolve_outer(
    store_root: &Utf8Path,
    dll_sha: &[u8; 20],
    dll_bytes: &[u8],
    type_name: &str,
) -> Result<String, TransformError> {
    if !type_name.contains('.') {
        return Ok(type_name.to_string());
    }
    let entities = list_entities(store_root, dll_sha, dll_bytes).await?;
    let names: std::collections::HashSet<&str> = entities.iter().map(|e| e.name.as_str()).collect();
    Ok(outer_of(type_name, |s| names.contains(s)))
}

/// Pure helper for [`resolve_outer`]: walk dot positions; the first
/// prefix that is itself a known entity is the outermost type. Every
/// `.` after that delimits a nested type — drop the suffix.
/// `Ns.Outer.Inner` with `Ns.Outer` known → `Ns.Outer`.
fn outer_of(type_name: &str, is_entity: impl Fn(&str) -> bool) -> String {
    for (idx, _) in type_name.match_indices('.') {
        let prefix = &type_name[..idx];
        if is_entity(prefix) {
            return prefix.to_string();
        }
    }
    type_name.to_string()
}

/// Build a filesystem-safe artifact name from a fully-qualified type
/// name. We keep `.` for readability (cache layout shows the
/// namespace), but anything that's awkward on disk (`<`, `>`, `/`,
/// `\`, control chars, …) becomes `_`. The leading `types/` directory
/// is just for cache-clarity — `du -hs transforms/<sha>/types` shows
/// the per-type decompile footprint at a glance.
fn type_artifact_name(type_name: &str) -> String {
    let mut out = String::with_capacity(type_name.len() + 6);
    out.push_str("types/");
    for c in type_name.chars() {
        let ok = c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+' | '`');
        out.push(if ok { c } else { '_' });
    }
    out
}

async fn run_ilspy(bytes: &[u8], args: &[&str]) -> Result<String, TransformError> {
    let tmp = tempfile_for(bytes)?;
    spawn_ilspy(&tmp, args).await
}

/// Kick off a background `ilspycmd -p` pass for `dll_bytes` if one
/// isn't already running for this sha. Each successfully-decompiled
/// type ends up in the same on-disk cache that [`decompile_type`]
/// reads from, so user-facing latency for follow-up clicks drops to
/// the cache-hit path (~ms) once the warmer finishes.
///
/// Fire-and-forget by design: a tokio task is spawned and the
/// function returns immediately. Errors are logged via `tracing` and
/// don't propagate.
pub fn warm_full_decompile(store_root: &Utf8Path, dll_sha: [u8; 20], dll_bytes: Vec<u8>) {
    {
        let mut guard = warmups_lock();
        let set = guard.get_or_insert_with(HashSet::new);
        if !set.insert(dll_sha) {
            // Already warming this DLL — nothing to do.
            return;
        }
    }
    let store_root = store_root.to_path_buf();
    tokio::spawn(async move {
        let result = run_warmer(&store_root, &dll_sha, &dll_bytes).await;
        warmups_lock().as_mut().map(|s| s.remove(&dll_sha));
        match result {
            Ok(n) => tracing::info!(sha = ?dll_sha, types_cached = n, "dll warmer done"),
            Err(e) => tracing::warn!(sha = ?dll_sha, %e, "dll warmer failed"),
        }
    });
}

/// Blocking equivalent of [`warm_full_decompile`]: runs the warmer
/// in the caller's task and returns the number of types cached.
/// Useful when a script wants to pre-fill the cache before its own
/// per-type pass instead of racing against a background tokio task.
#[allow(dead_code)]
pub async fn run_full_decompile(
    store_root: &Utf8Path,
    dll_sha: &[u8; 20],
    dll_bytes: &[u8],
) -> Result<usize, TransformError> {
    run_warmer(store_root, dll_sha, dll_bytes).await
}

async fn run_warmer(
    store_root: &Utf8Path,
    dll_sha: &[u8; 20],
    dll_bytes: &[u8],
) -> Result<usize, TransformError> {
    let tmp_dll = tempfile_for(dll_bytes)?;
    let outdir = warmer_outdir()?;
    let status = Command::new(ILSPYCMD)
        .arg("-p")
        .arg("--nested-directories")
        .arg("-o")
        .arg(outdir.path())
        .arg(tmp_dll.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                TransformError::ToolNotFound { cmd: ILSPYCMD }
            } else {
                TransformError::Io(e)
            }
        })?;
    if !status.success() {
        return Err(TransformError::ToolFailed {
            cmd: ILSPYCMD,
            stderr: "ilspycmd -p exited non-zero".to_string(),
        });
    }
    // Walk the output tree: each `<namespace-path>/<TypeName>.cs`
    // becomes one cached artifact under
    // `transforms/<sha>/types/<namespace>.<TypeName>`. Anything that
    // doesn't end in `.cs` (e.g. the generated `.csproj`) is skipped.
    let mut count = 0usize;
    let mut stack: Vec<(std::path::PathBuf, String)> =
        vec![(outdir.path().to_path_buf(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if file_type.is_dir() {
                let name = match entry.file_name().to_str() {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                let sub_prefix = if prefix.is_empty() {
                    name
                } else {
                    format!("{prefix}.{name}")
                };
                stack.push((path, sub_prefix));
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(|s| s.to_string()) else {
                continue;
            };
            let Some(type_name) = name.strip_suffix(".cs") else {
                continue;
            };
            // Skip anything that started with `<>` — see
            // `is_compiler_generated`. ilspy writes them as `--`
            // prefixed files in -p output; the simpler check on the
            // produced fqn works for both shapes.
            if is_compiler_generated(type_name) {
                continue;
            }
            let fqn = if prefix.is_empty() {
                type_name.to_string()
            } else {
                format!("{prefix}.{type_name}")
            };
            // Skip if `-t` already filled this slot — saves the
            // compress/write roundtrip when both passes happened to
            // touch the same type.
            let artifact = type_artifact_name(&fqn);
            if crate::transform::read_cached_artifact(store_root, dll_sha, &artifact)
                .ok()
                .flatten()
                .is_some()
            {
                continue;
            }
            let text = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            if crate::transform::write_cached_artifact(store_root, dll_sha, &artifact, &text)
                .is_ok()
            {
                count += 1;
            }
        }
    }
    Ok(count)
}

/// Output directory for one warmer run. Cleaned up when the
/// `WarmerOutdir` value drops.
struct WarmerOutdir(std::path::PathBuf);

impl WarmerOutdir {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for WarmerOutdir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn warmer_outdir() -> Result<WarmerOutdir, std::io::Error> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let path = std::env::temp_dir().join(format!("smv-dll-warm-{pid}-{seq}"));
    std::fs::create_dir_all(&path)?;
    Ok(WarmerOutdir(path))
}

async fn spawn_ilspy(tmp: &TempInput, args: &[&str]) -> Result<String, TransformError> {
    let child = Command::new(ILSPYCMD)
        .args(args)
        .arg(tmp.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                TransformError::ToolNotFound { cmd: ILSPYCMD }
            } else {
                TransformError::Io(e)
            }
        })?;
    let output = child.wait_with_output().await?;
    if !output.status.success() {
        return Err(TransformError::ToolFailed {
            cmd: ILSPYCMD,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_lines() {
        let raw = "Class UnityEngine.Vector3\nInterface IFoo\nEnum MyEnum\n";
        let out = parse_entity_list(raw);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].name, "UnityEngine.Vector3");
        assert_eq!(out[0].kind, EntityKind::Class);
        assert_eq!(out[2].kind, EntityKind::Enum);
    }

    #[test]
    fn drops_compiler_generated() {
        let raw = "Class Foo\nClass <Module>\nClass <>f__AnonymousType0\nClass <PrivateImplementationDetails>{F1ED8DE2}\nStruct Bar\n";
        let out = parse_entity_list(raw);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].name, "Foo");
        assert_eq!(out[1].name, "Bar");
    }

    #[test]
    fn skips_unknown_kinds() {
        // Future-proof against any new lead token we don't recognise.
        let raw = "Class Foo\nWhatever Bar\nStruct Baz\n";
        let out = parse_entity_list(raw);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].kind, EntityKind::Struct);
    }

    #[test]
    fn artifact_name_sanitises() {
        assert_eq!(type_artifact_name("Foo.Bar"), "types/Foo.Bar");
        assert_eq!(
            type_artifact_name("<>f__AnonymousType0"),
            "types/__f__AnonymousType0",
        );
        assert_eq!(
            type_artifact_name("Ns.Inner+Nested"),
            "types/Ns.Inner+Nested"
        );
    }

    #[test]
    fn outer_of_strips_nested_suffix() {
        let known: std::collections::HashSet<&str> =
            ["Foo", "Foo.Bar", "Ns.Outer"].into_iter().collect();
        // Pure namespace (no entity-prefix matches) — left alone.
        assert_eq!(outer_of("Ns.NoSuch", |s| known.contains(s)), "Ns.NoSuch");
        // Outer.Inner → Outer.
        assert_eq!(outer_of("Foo.Bar", |s| known.contains(s)), "Foo");
        // Longer chain — first matching prefix wins.
        assert_eq!(outer_of("Foo.Bar.Baz", |s| known.contains(s)), "Foo");
        // Namespaced outer.
        assert_eq!(
            outer_of("Ns.Outer.Inner", |s| known.contains(s)),
            "Ns.Outer"
        );
        // No dots — pass-through.
        assert_eq!(outer_of("Foo", |s| known.contains(s)), "Foo");
    }
}
