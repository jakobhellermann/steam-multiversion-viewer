// TODO(ai-review): review for style and correctness
//! Per-file-extension transformer definitions. Each entry is a CLI tool
//! the backend can run on a binary file to produce a text rendering
//! (decompiled source, exported symbols, …). Scaffolding for caching,
//! tempfiles, and process spawn lives in `mod.rs` — this module only
//! declares "which tool runs on what extension".

use crate::transform::CliTool;

/// Resolve a file path to the CLI tool that should produce its text
/// rendering. Returns `None` when no transformer is registered.
pub fn transformer_for(path: &str) -> Option<&'static CliTool> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());
    match ext.as_deref() {
        Some("dll" | "exe") => Some(&DUMP_DLL),
        Some("so") => Some(&NM_DYNAMIC),
        _ => None,
    }
}

const DUMP_DLL: CliTool = CliTool {
    cmd: "dump-dll",
    args_before_path: &[],
    output_mime: "text/x-csharp",
};

const NM_DYNAMIC: CliTool = CliTool {
    cmd: "nm",
    args_before_path: &["-D", "--defined-only", "-C"],
    output_mime: "text/plain",
};
