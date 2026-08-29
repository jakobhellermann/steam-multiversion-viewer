// TODO(ai-review): review for style and correctness
//! XNA/MonoGame compiled content (`.xnb`): header, optional LZX/LZ4 decompression, and the type-reader manifest.
//! The manifest is self-describing (each reader is a literal .NET type name); the payload per reader isn't, so only [`texture`] and [`audio`] are actually decoded, everything else stays an opaque leaf.
//! Layout cross-checked against the FEZ modding wiki and archiveteam.org's format notes.

pub mod audio;
pub mod texture;

use crate::TransformError;
use crate::structured::{Node, StructuredTree, human_bytes};

const HIDEF_MASK: u8 = 0x1;
const COMPRESSED_LZ4_MASK: u8 = 0x40;
const COMPRESSED_LZX_MASK: u8 = 0x80;
/// `"XNB" + platform + version + flags + fileSize(u32) + decompressedSize(u32)`
const COMPRESSED_PROLOGUE_SIZE: usize = 14;

#[derive(Debug)]
pub struct Header {
    pub platform: char,
    pub format_version: u8,
    pub hidef: bool,
    pub compressed: bool,
}

/// One entry of the type-reader manifest: the literal (assembly-qualified)
/// .NET type name plus the version the writer stamped it with.
#[derive(Debug)]
pub struct ReaderEntry {
    pub type_name: String,
    pub version: i32,
}

#[derive(Debug)]
pub struct Content {
    pub header: Header,
    pub readers: Vec<ReaderEntry>,
    pub shared_resource_count: u32,
    /// Index into `readers` for the primary object, plus its raw
    /// payload bytes. `None` index means the primary object is null.
    pub primary_reader: Option<usize>,
    pub primary_payload: Vec<u8>,
}

/// A byte cursor with the little-endian + 7-bit-varint primitives XNB
/// uses everywhere (matches .NET's `BinaryReader`/`Read7BitEncodedInt`).
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], TransformError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&end| end <= self.data.len())
            .ok_or_else(|| TransformError::Other("xnb: unexpected end of file".to_string()))?;
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, TransformError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, TransformError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn i32(&mut self) -> Result<i32, TransformError> {
        Ok(self.u32()? as i32)
    }

    /// .NET's `Read7BitEncodedInt`: little-endian groups of 7 bits, MSB
    /// of each byte set while more bytes follow.
    fn var_u32(&mut self) -> Result<u32, TransformError> {
        let mut result = 0u32;
        let mut shift = 0;
        loop {
            let byte = self.u8()?;
            result |= u32::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
            if shift >= 35 {
                return Err(TransformError::Other("xnb: varint too long".to_string()));
            }
        }
    }

    fn string(&mut self) -> Result<String, TransformError> {
        let len = self.var_u32()? as usize;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|e| TransformError::Other(format!("xnb: invalid string: {e}")))
    }
}

/// Parse the header, decompress the body if needed, and split off the
/// type-reader manifest + primary object's raw bytes.
pub fn parse(bytes: &[u8]) -> Result<Content, TransformError> {
    let mut c = Cursor::new(bytes);
    if c.take(3)? != b"XNB" {
        return Err(TransformError::Unsupported("not an XNB file".to_string()));
    }
    let platform = c.u8()? as char;
    let format_version = c.u8()?;
    let flags = c.u8()?;
    let header = Header {
        platform,
        format_version,
        hidef: flags & HIDEF_MASK != 0,
        compressed: flags & (COMPRESSED_LZX_MASK | COMPRESSED_LZ4_MASK) != 0,
    };
    let file_size = c.u32()? as usize;
    if file_size != bytes.len() {
        return Err(TransformError::Other(format!(
            "xnb: header file size {file_size} doesn't match actual size {}",
            bytes.len()
        )));
    }

    let body: std::borrow::Cow<[u8]> = if !header.compressed {
        std::borrow::Cow::Borrowed(&bytes[c.pos..])
    } else {
        let decompressed_size = c.u32()? as usize;
        let compressed = &bytes[COMPRESSED_PROLOGUE_SIZE..];
        let out = if flags & COMPRESSED_LZX_MASK != 0 {
            decompress_lzx(compressed, decompressed_size)?
        } else {
            // Unverified against a real file, unlike the LZX path: bail loud rather than guess.
            return Err(TransformError::Unsupported(
                "xnb: LZ4-compressed content isn't supported yet".to_string(),
            ));
        };
        if out.len() != decompressed_size {
            return Err(TransformError::Other(format!(
                "xnb: decompressed to {} bytes, header said {decompressed_size}",
                out.len()
            )));
        }
        std::borrow::Cow::Owned(out)
    };

    let mut c = Cursor::new(&body);
    let reader_count = c.var_u32()?;
    let mut readers = Vec::with_capacity(reader_count as usize);
    for _ in 0..reader_count {
        let type_name = c.string()?;
        let version = c.i32()?;
        readers.push(ReaderEntry { type_name, version });
    }
    let shared_resource_count = c.var_u32()?;

    let reader_index = c.var_u32()?;
    let primary_reader = (reader_index != 0).then(|| reader_index as usize - 1);
    // Correctly delimits the primary object only when there are no shared resources: locating the boundary needs each reader's exact payload layout.
    let primary_payload = body[c.pos..].to_vec();

    Ok(Content {
        header,
        readers,
        shared_resource_count,
        primary_reader,
        primary_payload,
    })
}

/// XNA's LZX chunking: a `[flag][frame_size][block_size]` header (or a 2-byte short form) precedes each compressed chunk, up to 0x8000 (32 KiB) decompressed bytes per chunk.
fn decompress_lzx(mut compressed: &[u8], decompressed_size: usize) -> Result<Vec<u8>, TransformError> {
    use lzxd::{Lzxd, WindowSize};
    let mut lzx = Lzxd::new(WindowSize::KB64);
    let mut out = Vec::with_capacity(decompressed_size);
    while !compressed.is_empty() && out.len() < decompressed_size {
        let read_be16 = |b: &[u8]| -> Result<u16, TransformError> {
            b.first_chunk::<2>()
                .map(|b| u16::from_be_bytes(*b))
                .ok_or_else(|| TransformError::Other("xnb: truncated lzx chunk header".to_string()))
        };
        let (frame_size, block_size, header_len) = if compressed[0] == 0xFF {
            let frame_size = read_be16(&compressed[1..])?;
            let block_size = read_be16(&compressed[3..])?;
            (frame_size, block_size, 5)
        } else {
            let block_size = read_be16(compressed)?;
            (0x8000, block_size, 2)
        };
        if frame_size == 0 || block_size == 0 {
            break;
        }
        compressed = &compressed[header_len..];
        let block = compressed
            .get(..block_size as usize)
            .ok_or_else(|| TransformError::Other("xnb: lzx block runs past end of file".to_string()))?;
        let chunk = lzx
            .decompress_next(block, frame_size as usize)
            .map_err(|e| TransformError::Other(format!("xnb: lzx decode failed: {e:?}")))?;
        out.extend_from_slice(chunk);
        compressed = &compressed[block_size as usize..];
    }
    Ok(out)
}

/// The reader's simple class name, stripped of namespace and the
/// trailing assembly-qualification (`", Microsoft.Xna.Framework, …"`).
/// Good enough to dispatch on and to display; not a full parse of
/// generic-reader names like `ListReader\`1[[...]]`.
fn simple_reader_name(type_name: &str) -> &str {
    // Split on the first comma outside `[...]` nesting: a generic reader's
    // own type args are assembly-qualified too, each wrapped in brackets,
    // and their commas would otherwise be mistaken for the outer type's.
    let mut depth = 0i32;
    let mut top_level_comma = None;
    for (i, c) in type_name.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
            ',' if depth == 0 => {
                top_level_comma = Some(i);
                break;
            }
            _ => {}
        }
    }
    let qualified = &type_name[..top_level_comma.unwrap_or(type_name.len())];
    let unparameterized = qualified.split('[').next().unwrap_or(qualified);
    let simple = unparameterized.rsplit('.').next().unwrap_or(unparameterized).trim();
    simple.trim_end_matches(|c: char| c == '`' || c.is_ascii_digit())
}

pub fn build_tree(bytes: &[u8], file_label: &str) -> Result<StructuredTree, TransformError> {
    let content = parse(bytes)?;

    let header_badge = format!(
        "{} · fmt {}{}{}",
        match content.header.platform {
            'w' => "Windows",
            'm' => "Windows Phone",
            'x' => "Xbox 360",
            'a' => "Android",
            'i' => "iOS",
            other => return Err(TransformError::Other(format!("xnb: unknown platform '{other}'"))),
        },
        content.header.format_version,
        if content.header.hidef { " · HiDef" } else { "" },
        if content.header.compressed { " · compressed" } else { "" },
    );

    let readers_node = Node {
        id: "readers".to_string(),
        label: "Type Readers".to_string(),
        badge: Some(content.readers.len().to_string()),
        children: content
            .readers
            .iter()
            .enumerate()
            .map(|(i, r)| Node {
                id: format!("reader:{i}"),
                label: simple_reader_name(&r.type_name).to_string(),
                badge: Some(format!("v{}", r.version)),
                ..Node::default()
            })
            .collect(),
        ..Node::default()
    };

    let mut children = vec![
        Node {
            id: "header".to_string(),
            label: "Header".to_string(),
            badge: Some(header_badge),
            ..Node::default()
        },
        readers_node,
    ];

    if content.shared_resource_count > 0 {
        children.push(Node {
            id: "shared-resources".to_string(),
            label: format!(
                "{} shared resource(s) (not parsed)",
                content.shared_resource_count
            ),
            ..Node::default()
        });
    }

    let reader_name = content
        .primary_reader
        .and_then(|i| content.readers.get(i))
        .map(|r| simple_reader_name(&r.type_name));

    let mut content_node = Node {
        id: "content".to_string(),
        label: reader_name.unwrap_or("(null)").to_string(),
        badge: Some(human_bytes(content.primary_payload.len() as u64)),
        has_content: true,
        ..Node::default()
    };
    // Only claim a renderable MIME type when there's exactly one primary object and no trailing shared resources.
    if content.shared_resource_count == 0 {
        match reader_name {
            Some("Texture2DReader") => content_node.content_mime = Some("image/png".to_string()),
            Some("SoundEffectReader") => content_node.content_mime = Some("audio/wav".to_string()),
            _ => {}
        }
    }
    children.push(content_node);

    Ok(StructuredTree {
        root: Node {
            id: format!("file:{file_label}"),
            label: file_label.to_string(),
            children,
            ..Node::default()
        },
    })
}

/// Marks a decode failure as expected (unsupported texture format, unhandled reader), not a bug, so the route can 415 instead of 500.
#[derive(Debug)]
pub struct XnbContentUnsupported(pub String);

impl std::fmt::Display for XnbContentUnsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for XnbContentUnsupported {}

/// Decoded media for the primary object's content node.
pub enum RenderedContent {
    Png(Vec<u8>),
    Wav(Vec<u8>),
}

/// Decodes the primary object into whatever media its reader produces.
/// Re-parses from scratch rather than caching, matching the Unity texture routes' pattern.
pub fn render_content(bytes: &[u8]) -> anyhow::Result<RenderedContent> {
    let content = parse(bytes).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let reader_name = content
        .primary_reader
        .and_then(|i| content.readers.get(i))
        .map(|r| simple_reader_name(&r.type_name));
    anyhow::ensure!(
        content.shared_resource_count == 0,
        XnbContentUnsupported(format!(
            "{} shared resource(s) alongside the primary object aren't supported",
            content.shared_resource_count
        ))
    );
    match reader_name {
        Some("Texture2DReader") => Ok(RenderedContent::Png(texture::decode_to_png(
            &content.primary_payload,
        )?)),
        Some("SoundEffectReader") => Ok(RenderedContent::Wav(audio::to_wav(
            &content.primary_payload,
        )?)),
        other => Err(XnbContentUnsupported(format!(
            "no renderer for {}",
            other.unwrap_or("(null)")
        ))
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_var_u32(out: &mut Vec<u8>, mut v: u32) {
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }

    fn write_string(out: &mut Vec<u8>, s: &str) {
        write_var_u32(out, s.len() as u32);
        out.extend_from_slice(s.as_bytes());
    }

    /// Builds a minimal, uncompressed, single-reader XNB with an empty primary payload, enough to exercise header + manifest parsing.
    fn sample_xnb(reader_type: &str) -> Vec<u8> {
        let mut body = Vec::new();
        write_var_u32(&mut body, 1); // reader count
        write_string(&mut body, reader_type);
        body.extend_from_slice(&7i32.to_le_bytes()); // version
        write_var_u32(&mut body, 0); // shared resource count
        write_var_u32(&mut body, 1); // primary object -> readers[0]

        let mut file = Vec::new();
        file.extend_from_slice(b"XNB");
        file.push(b'w');
        file.push(5);
        file.push(0); // flags: uncompressed
        file.extend_from_slice(&0u32.to_le_bytes()); // file size, patched below
        file.extend_from_slice(&body);
        let len = file.len() as u32;
        file[6..10].copy_from_slice(&len.to_le_bytes());
        file
    }

    #[test]
    fn parses_header_and_manifest() {
        let bytes = sample_xnb("Microsoft.Xna.Framework.Content.Texture2DReader, Microsoft.Xna.Framework");
        let content = parse(&bytes).unwrap();
        assert_eq!(content.header.platform, 'w');
        assert_eq!(content.header.format_version, 5);
        assert!(!content.header.compressed);
        assert_eq!(content.readers.len(), 1);
        assert_eq!(content.readers[0].version, 7);
        assert_eq!(
            simple_reader_name(&content.readers[0].type_name),
            "Texture2DReader"
        );
        assert_eq!(content.primary_reader, Some(0));
        assert!(content.primary_payload.is_empty());
    }

    #[test]
    fn rejects_lz4_as_unsupported() {
        // Flags = LZ4; this path reads only the decompressed-size field before bailing, so the body content doesn't matter.
        let mut bytes = sample_xnb("Foo");
        bytes[5] = COMPRESSED_LZ4_MASK;
        assert!(matches!(parse(&bytes), Err(TransformError::Unsupported(_))));
    }

    #[test]
    fn simple_reader_name_ignores_commas_inside_generic_args() {
        let generic = "Microsoft.Xna.Framework.Content.ListReader`1[[Microsoft.Xna.Framework.Rectangle, Microsoft.Xna.Framework, Version=4.0.0.0, Culture=neutral, PublicKeyToken=842cf8be1de50553]]";
        assert_eq!(simple_reader_name(generic), "ListReader");

        let generic_with_own_suffix = "Ns.ListReader`1[[Ns.Foo, Asm, Version=1.0.0.0]], mscorlib, Version=4.0.0.0";
        assert_eq!(simple_reader_name(generic_with_own_suffix), "ListReader");

        assert_eq!(
            simple_reader_name("Microsoft.Xna.Framework.Content.SoundEffectReader"),
            "SoundEffectReader"
        );
    }

    #[test]
    fn rejects_bad_magic() {
        let err = parse(b"not an xnb file at all").unwrap_err();
        assert!(matches!(err, TransformError::Unsupported(_)));
    }

    #[test]
    fn rejects_mismatched_file_size() {
        let mut bytes = sample_xnb("Foo");
        let bad_len = (bytes.len() as u32) + 1;
        bytes[6..10].copy_from_slice(&bad_len.to_le_bytes());
        assert!(parse(&bytes).is_err());
    }
}
