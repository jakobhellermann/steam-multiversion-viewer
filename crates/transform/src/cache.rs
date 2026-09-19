// TODO(ai-review): review for style and correctness
//! On-disk cache of "render this binary as text" results.
//!
//! Each transformer reads bytes from a depot file and produces one or
//! more text artifacts (e.g. ilspycmd's `-l` class listing plus a `-t`
//! decompilation per type). Outputs live under
//! `{store_root}/transforms/{sha}/{artifact}.txt.zst` keyed by the
//! file's manifest sha plus a per-artifact name — manifest shas are
//! content addresses, so the same file across multiple manifests
//! shares one directory. Tool definitions live in `tools.rs`.

use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;

use camino::Utf8Path;
use tokio::process::Command;

/// A CLI tool that takes a file path as its last positional argument
/// and writes the text rendering to stdout. Lives as a `const` in
/// `tools.rs`.
pub struct CliTool {
    pub cmd: &'static str,
    pub args_before_path: &'static [&'static str],
    pub output_mime: &'static str,
}

/// Which kind of transformer should run for a file. Dispatched by
/// `tools::transformer_for`; the actual "run + cache" call lives in
/// the route handler so each variant can build the right input
/// (raw bytes, full manifest store, etc).
pub enum Transformer {
    /// External program — feed it the file bytes, capture stdout.
    Cli(&'static CliTool),
    /// In-process unity rabex dump. The route is responsible for
    /// handing the manifest store + relative path to
    /// `crate::unity::dump_unity_serialized`.
    #[cfg(feature = "unity")]
    UnitySerialized,
    /// Unity asset bundle (`*.bundle`, `*.unity3d`). Multiple
    /// SerializedFiles inside one container; structured view delegates
    /// to [`crate::unity::bundle::build_tree`].
    #[cfg(feature = "unity")]
    UnityBundle,
    /// .NET assembly — produces a namespace tree via
    /// [`crate::dll::tree::build_tree`] and lazy per-type decompiles
    /// via [`crate::dll::decompile_type`]. There's no
    /// `/file/transformed`-style single-blob output for this one;
    /// callers route through `/file/structured` instead.
    Dll,
}

impl Transformer {
    pub fn output_mime(&self) -> &'static str {
        match self {
            Self::Cli(t) => t.output_mime,
            #[cfg(feature = "unity")]
            Self::UnitySerialized => "text/plain",
            #[cfg(feature = "unity")]
            Self::UnityBundle => "text/plain",
            Self::Dll => "text/x-csharp",
        }
    }

    /// Human-readable name for route-layer error messages.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Cli(t) => t.cmd,
            #[cfg(feature = "unity")]
            Self::UnitySerialized => "unity serialized file",
            #[cfg(feature = "unity")]
            Self::UnityBundle => "unity bundle",
            Self::Dll => ".NET assembly",
        }
    }
}

#[derive(Debug)]
pub enum TransformError {
    Io(std::io::Error),
    ToolNotFound {
        cmd: &'static str,
    },
    ToolFailed {
        cmd: &'static str,
        stderr: String,
    },
    /// Wrong file kind, discovered only after reading the bytes — 415, not 500.
    Unsupported(String),
    Other(String),
}

impl std::fmt::Display for TransformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::ToolNotFound { cmd } => write!(f, "{cmd} not found on PATH"),
            Self::ToolFailed { cmd, stderr } => write!(f, "{cmd} failed: {stderr}"),
            Self::Unsupported(msg) => f.write_str(msg),
            Self::Other(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for TransformError {}

impl From<std::io::Error> for TransformError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Default artifact name for the legacy "transform a binary to one
/// text blob" pathway (whole-DLL ilspy dump, nm symbol list, …).
/// New transformers should use [`cache_artifact_path`] directly with a
/// more descriptive name (e.g. `"list"`, `"types/HeroController"`).
pub const ARTIFACT_MAIN: &str = "main";

/// Cache path for one artifact of a transformed file:
/// `<store_root>/transforms/<sha-hex>/<artifact>.txt.zst`. `artifact`
/// may contain `/` to nest further (e.g. `types/Foo.Bar`).
pub fn cache_artifact_path(store_root: &Utf8Path, sha: &[u8; 20], artifact: &str) -> PathBuf {
    let mut hex = String::with_capacity(40);
    for b in sha {
        use std::fmt::Write as _;
        write!(&mut hex, "{b:02x}").expect("write to String");
    }
    store_root
        .as_std_path()
        .join("transforms")
        .join(hex)
        .join(format!("{artifact}.txt.zst"))
}

/// Try to read a cached artifact. Returns `None` when not yet computed;
/// returns `Err` on filesystem trouble.
pub fn read_cached_artifact(
    store_root: &Utf8Path,
    sha: &[u8; 20],
    artifact: &str,
) -> Result<Option<String>, std::io::Error> {
    let path = cache_artifact_path(store_root, sha, artifact);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path)?;
    let decoded = zstd::decode_all(bytes.as_slice())?;
    Ok(Some(String::from_utf8_lossy(&decoded).into_owned()))
}

/// Persist `text` as a cached artifact under
/// `transforms/<sha>/<artifact>.txt.zst`. Atomic via tmp + rename so a
/// background bulk-warm (e.g. `ilspycmd -p`) and a foreground per-type
/// fetch can race without interleaving bytes mid-file.
pub fn write_cached_artifact(
    store_root: &Utf8Path,
    sha: &[u8; 20],
    artifact: &str,
    text: &str,
) -> Result<(), std::io::Error> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);

    let path = cache_artifact_path(store_root, sha, artifact);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // zstd level 3 — fast both ways, ~5× compression on typical text
    // tool output. Decompression is single-digit ms for normal sizes.
    let compressed = zstd::encode_all(text.as_bytes(), 3)?;
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("zst.tmp.{}.{seq}", std::process::id()));
    std::fs::write(&tmp, &compressed)?;
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Backwards-compat alias for [`read_cached_artifact`] with the
/// default [`ARTIFACT_MAIN`] name — used by the existing whole-file
/// transform routes that don't carry an artifact id.
pub fn read_cached(
    store_root: &Utf8Path,
    sha: &[u8; 20],
) -> Result<Option<String>, std::io::Error> {
    read_cached_artifact(store_root, sha, ARTIFACT_MAIN)
}

/// Run `tool` on `input_bytes`, write the compressed result to the
/// cache under the [`ARTIFACT_MAIN`] name, return the produced text.
pub async fn run_and_cache(
    store_root: &Utf8Path,
    tool: &CliTool,
    sha: &[u8; 20],
    input_bytes: &[u8],
) -> Result<String, TransformError> {
    let text = run(tool, input_bytes).await?;
    write_cached_artifact(store_root, sha, ARTIFACT_MAIN, &text)?;
    Ok(text)
}

/// Builds the command for a CLI tool. The packaged app has no console, so
/// on Windows each console child would flash its own terminal window.
pub(crate) fn tool_command(cmd: &str) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(cmd);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

async fn run(tool: &CliTool, input_bytes: &[u8]) -> Result<String, TransformError> {
    let tmp = tempfile_for(input_bytes)?;
    let child = tool_command(tool.cmd)
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
pub struct TempInput(std::path::PathBuf);

impl TempInput {
    pub fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempInput {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub fn tempfile_for(bytes: &[u8]) -> Result<TempInput, std::io::Error> {
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
