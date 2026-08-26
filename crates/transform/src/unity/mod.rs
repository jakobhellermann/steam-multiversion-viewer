// TODO(ai-review): review for style and correctness
//! All unity-asset specific code: serializedfile/bundle parsing via
//! rabex, the conventional-filename detection used by the transformer
//! dispatch, and any future unity-aware diffs.
//!
//! Gated behind the `unity` cargo feature so the backend can be built
//! without pulling rabex (and its transitive unity-specific deps) at
//! all.

use rabex_env::Environment;
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;

pub mod bundle;
pub mod game_specific;
pub mod game_version;
pub mod secure_player_prefs;
pub mod serializedfile;

/// Dump a unity serialized-file as text using a prebuilt `env`.
/// Synchronous because rabex's I/O trampolines through
/// `block_in_place`/`block_on`; callers from async context must wrap in
/// `tokio::task::spawn_blocking`.
pub fn dump_unity_serialized<R: EnvResolver, P: TypeTreeProvider>(
    env: &Environment<R, P>,
    data_dir: &str,
    path: &str,
) -> Result<String, anyhow::Error> {
    let relative = path.strip_prefix(&format!("{data_dir}/")).unwrap_or(path);
    let file = env.load_serialized(relative)?;

    let mut out = String::new();
    serializedfile::format::format_class_stats(&mut out, &file);
    out.push('\n');
    serializedfile::format::format_hierarchy(&mut out, &file)?;
    Ok(out)
}

/// True for unity serialized-file conventions that don't carry a
/// dispatchable extension. Pure naming heuristic — the file's *bytes*
/// aren't consulted.
pub fn is_unity_serialized_filename(name: &str) -> bool {
    if is_unity_level(name) {
        return true;
    }
    matches!(
        name,
        "globalgamemanagers" | "unity_builtin_extra" | "unity default resources"
    )
}

fn is_unity_level(name: &str) -> bool {
    let rest = match name.strip_prefix("level") {
        Some(rest) => rest,
        None => return false,
    };
    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(non_snake_case)]
    fn unity_serialized_matches_levelN() {
        assert!(is_unity_serialized_filename("level0"));
        assert!(is_unity_serialized_filename("level203"));
    }

    #[test]
    fn unity_serialized_matches_well_known() {
        assert!(is_unity_serialized_filename("globalgamemanagers"));
        assert!(is_unity_serialized_filename("unity_builtin_extra"));
        assert!(is_unity_serialized_filename("unity default resources"));
    }

    #[test]
    fn unity_serialized_rejects_others() {
        assert!(!is_unity_serialized_filename("level"));
        assert!(!is_unity_serialized_filename("level0.assets"));
        assert!(!is_unity_serialized_filename("levels"));
        assert!(!is_unity_serialized_filename("LEVEL0"));
    }
}
