// TODO(ai-review): review for style and correctness
//! .NET assembly support via `ilspycmd`. Two operations:
//!
//! - [`list_entities`] runs `ilspycmd -l "c i s d e"` once per
//!   DLL-sha and caches the line-oriented output. Used to build the
//!   structured tree.
//! - [`decompile_type`] runs `ilspycmd -t <fully-qualified-name>` for
//!   a single type and caches the C# source under
//!   `transforms/<dll-sha>/types/<sanitised-name>`. Per the bench
//!   notes (`ilspycmd-bench.md`), per-type latency is dominated by the
//!   ~0.25 s assembly load plus a roughly linear cost in type size —
//!   fast enough on the hot path for table-browsing, so we don't try
//!   to pre-warm via `-p`.
//!
//! Both are async + use `tokio::process::Command`, so spawning is
//! safe from request handlers without `spawn_blocking`.

use std::process::Stdio;

use camino::Utf8Path;
use tokio::process::Command;

use crate::transform::{TempInput, TransformError, tempfile_for};

pub mod tree;

/// Where to find ilspycmd. Plain PATH lookup for now; revisit when
/// we ship installers.
const ILSPYCMD: &str = "ilspycmd";

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
/// qualified name as printed by [`list_entities`] (ilspy's `-t`
/// wants that exact form).
pub async fn decompile_type(
    store_root: &Utf8Path,
    dll_sha: &[u8; 20],
    dll_bytes: &[u8],
    type_name: &str,
) -> Result<String, TransformError> {
    let artifact = type_artifact_name(type_name);
    if let Some(cached) = crate::transform::read_cached_artifact(store_root, dll_sha, &artifact)? {
        return Ok(cached);
    }
    let text = run_ilspy(dll_bytes, &["-t", type_name]).await?;
    crate::transform::write_cached_artifact(store_root, dll_sha, &artifact, &text)?;
    Ok(text)
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
}
