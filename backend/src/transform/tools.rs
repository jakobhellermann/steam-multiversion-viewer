// TODO(ai-review): review for style and correctness
//! Per-file-extension transformer definitions. Each entry is a CLI tool
//! the backend can run on a binary file to produce a text rendering
//! (decompiled source, exported symbols, …). Scaffolding for caching,
//! tempfiles, and process spawn lives in `mod.rs` — this module only
//! declares "which tool runs on what extension".

use crate::transform::{CliTool, Transformer};

/// Resolve a file path to the transformer that should produce its text
/// rendering. Returns `None` when no transformer is registered.
pub fn transformer_for(path: &str) -> Option<Transformer> {
    let p = std::path::Path::new(path);
    let ext = p
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());
    if let Some(t) = ext.as_deref().and_then(extension_transformer) {
        return Some(t);
    }
    let file_name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
    filename_transformer(file_name)
}

fn extension_transformer(ext: &str) -> Option<Transformer> {
    match ext {
        "dll" | "exe" => Some(Transformer::Dll),
        "so" => Some(Transformer::Cli(&NM_DYNAMIC)),
        #[cfg(feature = "unity")]
        "assets" => Some(Transformer::UnitySerialized),
        #[cfg(feature = "unity")]
        "bundle" | "unity3d" => Some(Transformer::UnityBundle),
        _ => None,
    }
}

/// Files without a dispatchable extension. Today only unity asset
/// conventions live here; gated behind the `unity` feature so the
/// detection lookup goes away entirely when the feature is off.
fn filename_transformer(name: &str) -> Option<Transformer> {
    #[cfg(feature = "unity")]
    {
        if crate::unity::is_unity_serialized_filename(name) {
            return Some(Transformer::UnitySerialized);
        }
    }
    let _ = name;
    None
}

const NM_DYNAMIC: CliTool = CliTool {
    cmd: "nm",
    args_before_path: &["-D", "--defined-only", "-C"],
    output_mime: "text/plain",
};
