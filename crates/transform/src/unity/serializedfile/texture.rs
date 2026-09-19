// TODO(ai-review): review for style and correctness
//! Render a Unity `Texture2D` to PNG, from a bundle or a plain
//! serialized file. Streamed pixels come from the same bundle or the
//! `.resS` sibling; cross-bundle streams are unsupported.

use std::io::Cursor;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use rabex_env::Environment;
use rabex_env::addressables::ArchivePath;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::files::bundlefile::{BundleFileReader, ExtractionConfig};
use rabex_env::rabex::objects::pptr::PathId;
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;
use rabex_texture::Texture2D;

use crate::unity::bundle::insert_archive_entry;
use crate::unity::relative_to_data_dir;

/// Marks a texture-decode error as expected (no pixel data, unsupported
/// format), not a bug, so the route can 415 instead of 500.
#[derive(Debug)]
pub struct TextureUnsupported(pub String);

impl std::fmt::Display for TextureUnsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for TextureUnsupported {}

/// Decode the `Texture2D` at `path_id` in `archive_entry` of the bundle
/// at `bundle_path` (depot-absolute; resolved against `data_dir`) and
/// encode it as PNG.
#[tracing::instrument(skip_all, fields(archive_entry, path_id))]
pub fn render_bundle_texture_png<R: EnvResolver, P: TypeTreeProvider>(
    env: &Environment<R, P>,
    data_dir: &str,
    bundle_path: &str,
    archive_entry: &str,
    path_id: PathId,
) -> Result<Vec<u8>> {
    let bundle_bytes = env
        .game_files
        .read_path(Path::new(relative_to_data_dir(data_dir, bundle_path)))?;
    let bundle = BundleFileReader::from_reader(
        Cursor::new(bundle_bytes.as_ref()),
        &ExtractionConfig::default().with_fallback_unity_version(env.unity_version()?.clone()),
    )?;
    let file = insert_archive_entry(env, &bundle, archive_entry)?;
    decode_texture(&file, path_id, |ress_path| {
        let archive = ArchivePath::try_parse(Path::new(ress_path))?
            .with_context(|| format!("unparseable m_StreamData path: {ress_path}"))?;
        bundle
            .read_at(archive.file)?
            .with_context(|| format!("streamed .resS entry {} not in this bundle", archive.file))
    })
}

/// Decode the `Texture2D` at `path_id` in the serialized file `path` and encode it as PNG.
#[tracing::instrument(skip_all, fields(path, path_id))]
pub fn render_serialized_texture_png<R: EnvResolver, P: TypeTreeProvider>(
    env: &Environment<R, P>,
    data_dir: &str,
    path: &str,
    path_id: PathId,
) -> Result<Vec<u8>> {
    let relative = relative_to_data_dir(data_dir, path);
    let file = env.load_serialized(relative)?;
    decode_texture(&file, path_id, |ress_path| {
        let sibling = Path::new(relative).with_file_name(ress_path);
        Ok(env.game_files.read_path(&sibling)?.as_ref().to_vec())
    })
}

/// Read the texture, resolve its base-mip pixels (`read_ress` fetches
/// the full `.resS` buffer when streamed), and encode the decode as PNG.
fn decode_texture<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    path_id: PathId,
    read_ress: impl FnOnce(&str) -> Result<Vec<u8>>,
) -> Result<Vec<u8>> {
    let tex = file.object_at::<Texture2D>(path_id)?.read()?;
    let pixels = if !tex.image_data.is_empty() {
        tex.image_data.clone()
    } else {
        let stream = &tex.m_StreamData;
        ensure!(
            !stream.path.is_empty(),
            TextureUnsupported(format!("texture {} has no pixel data", tex.m_Name))
        );
        let bytes = read_ress(&stream.path)?;
        let (offset, size) = (stream.offset as usize, stream.size as usize);
        ensure!(
            offset + size <= bytes.len(),
            "stream slice {offset}+{size} out of bounds ({} bytes)",
            bytes.len()
        );
        bytes[offset..offset + size].to_vec()
    };
    ensure!(
        rabex_texture::TextureFormat::from_id(tex.m_TextureFormat).is_some(),
        TextureUnsupported(format!(
            "unsupported Texture2D format {}",
            tex.m_TextureFormat
        ))
    );
    rabex_texture::to_png(&rabex_texture::decode(
        tex.m_TextureFormat,
        tex.m_Width as u32,
        tex.m_Height as u32,
        &pixels,
    )?)
}
