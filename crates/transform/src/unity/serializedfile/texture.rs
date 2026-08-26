// TODO(ai-review): review for style and correctness
//! Render a Unity `Texture2D` in a bundle to PNG. Streamed pixels must
//! live in the same bundle (cross-bundle `.resS` is unsupported).

use std::io::Cursor;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use rabex_env::Environment;
use rabex_env::addressables::ArchivePath;
use rabex_env::env::Data;
use rabex_env::rabex::files::bundlefile::{BundleFileReader, ExtractionConfig};
use rabex_env::rabex::objects::pptr::PathId;
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;
use rabex_texture::Texture2D;

use crate::unity::bundle::insert_archive_entry;

/// Decode the `Texture2D` at `path_id` in `archive_entry` and encode it as PNG.
#[tracing::instrument(skip_all, fields(archive_entry, path_id))]
pub fn render_bundle_texture_png<R: EnvResolver, P: TypeTreeProvider>(
    env: &Environment<R, P>,
    bundle_bytes: Data,
    archive_entry: &str,
    path_id: PathId,
) -> Result<Vec<u8>> {
    let bundle = BundleFileReader::from_reader(
        Cursor::new(bundle_bytes.as_ref()),
        &ExtractionConfig::default().with_fallback_unity_version(env.unity_version()?.clone()),
    )?;
    let file = insert_archive_entry(env, &bundle, archive_entry)?;
    let tex = file.object_at::<Texture2D>(path_id)?.read()?;

    let pixels = texture_pixels(&bundle, &tex)?;
    rabex_texture::to_png(&rabex_texture::decode(
        tex.m_TextureFormat,
        tex.m_Width as u32,
        tex.m_Height as u32,
        &pixels,
    )?)
}

/// Pixel buffer: inline `image data`, else the `m_StreamData` slice from this bundle.
fn texture_pixels<T: AsRef<[u8]>>(
    bundle: &BundleFileReader<Cursor<T>>,
    tex: &Texture2D,
) -> Result<Vec<u8>> {
    if !tex.image_data.is_empty() {
        return Ok(tex.image_data.clone());
    }
    let stream = &tex.m_StreamData;
    ensure!(
        !stream.path.is_empty(),
        "texture {} has no pixel data",
        tex.m_Name
    );
    let archive = ArchivePath::try_parse(Path::new(&stream.path))?
        .with_context(|| format!("unparseable m_StreamData path: {}", stream.path))?;
    let stream_bytes = bundle.read_at(archive.file)?.with_context(|| {
        format!(
            "streamed .resS entry {} not in this bundle (cross-bundle streams unsupported)",
            archive.file
        )
    })?;
    let (offset, size) = (stream.offset as usize, stream.size as usize);
    ensure!(
        offset + size <= stream_bytes.len(),
        "stream slice {offset}+{size} out of bounds ({} bytes)",
        stream_bytes.len()
    );
    Ok(stream_bytes[offset..offset + size].to_vec())
}
