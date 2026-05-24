// TODO(ai-review): review for style and correctness
//! On-disk cache of "render this binary as text" results.
//!
//! Each transformer reads bytes from a depot file and produces a text
//! representation (e.g. ilspycmd decompiles a .NET .dll into C# source).
//! Output is cached under `{store_root}/transforms/{sha}.txt.zst`
//! keyed by the file's manifest sha — manifest shas are content
//! addresses, so the same file across multiple manifests hits the same
//! entry. Tool definitions live in `tools.rs`.

use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;

use camino::Utf8Path;
use tokio::process::Command;

pub mod tools;

/// A CLI tool that takes a file path as its last positional argument
/// and writes the text rendering to stdout. Lives as a `const` in
/// `tools.rs`.
pub struct CliTool {
    pub cmd: &'static str,
    pub args_before_path: &'static [&'static str],
}

#[derive(Debug)]
pub enum TransformError {
    Io(std::io::Error),
    ToolNotFound { cmd: &'static str },
    ToolFailed { cmd: &'static str, stderr: String },
}

impl std::fmt::Display for TransformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::ToolNotFound { cmd } => write!(f, "{cmd} not found on PATH"),
            Self::ToolFailed { cmd, stderr } => write!(f, "{cmd} failed: {stderr}"),
        }
    }
}

impl std::error::Error for TransformError {}

impl From<std::io::Error> for TransformError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Path of the cached output for a given content sha.
fn cache_path(store_root: &Utf8Path, sha: &[u8; 20]) -> PathBuf {
    let mut hex = String::with_capacity(40);
    for b in sha {
        use std::fmt::Write as _;
        write!(&mut hex, "{b:02x}").expect("write to String");
    }
    store_root
        .as_std_path()
        .join("transforms")
        .join(format!("{hex}.txt.zst"))
}

/// Try to read the cached output for `sha`. Returns `None` when not yet
/// computed; returns `Err` on filesystem trouble.
pub fn read_cached(
    store_root: &Utf8Path,
    sha: &[u8; 20],
) -> Result<Option<String>, std::io::Error> {
    let path = cache_path(store_root, sha);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path)?;
    let decoded = zstd::decode_all(bytes.as_slice())?;
    Ok(Some(String::from_utf8_lossy(&decoded).into_owned()))
}

/// Run `tool` on `input_bytes`, write the compressed result to the
/// cache, return the produced text.
pub async fn run_and_cache(
    store_root: &Utf8Path,
    tool: &CliTool,
    sha: &[u8; 20],
    input_bytes: &[u8],
) -> Result<String, TransformError> {
    let text = run(tool, input_bytes).await?;
    let path = cache_path(store_root, sha);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // zstd level 3 — fast both ways, ~5× compression on typical text
    // tool output. Decompression is single-digit ms for normal sizes.
    let compressed = zstd::encode_all(text.as_bytes(), 3)?;
    std::fs::write(&path, &compressed)?;
    Ok(text)
}

async fn run(tool: &CliTool, input_bytes: &[u8]) -> Result<String, TransformError> {
    let tmp = tempfile_for(input_bytes)?;
    let child = Command::new(tool.cmd)
        .args(tool.args_before_path)
        .arg(tmp.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                TransformError::ToolNotFound { cmd: tool.cmd }
            } else {
                TransformError::Io(e)
            }
        })?;
    let output = child.wait_with_output().await?;
    if !output.status.success() {
        return Err(TransformError::ToolFailed {
            cmd: tool.cmd,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Hold the temp file in scope so it's removed when we're done with it.
struct TempInput(std::path::PathBuf);

impl TempInput {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempInput {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn tempfile_for(bytes: &[u8]) -> Result<TempInput, std::io::Error> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let path = std::env::temp_dir().join(format!("smv-transform-{pid}-{seq}"));
    let mut f = std::fs::File::create(&path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(TempInput(path))
}
