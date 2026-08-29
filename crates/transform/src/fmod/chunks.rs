// TODO(ai-review): review for style and correctness
//! Generic RIFF/LIST chunk tree walker for `.bank` files. Understands
//! only the container framing (`tag[4] + size:u32 + payload`, `LIST`
//! chunks recursing via an extra 4-byte sub-tag) — not the meaning of
//! any individual chunk's payload.

use anyhow::{Result, bail};

use super::reader::Reader;

const TAG_RIFF: &[u8; 4] = b"RIFF";
const FORM_FEV: &[u8; 4] = b"FEV ";
const TAG_LIST: &[u8; 4] = b"LIST";

/// One node in the chunk tree.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub tag: [u8; 4],
    /// Sub-tag identifying what kind of list this is (e.g. `EVNT` groups
    /// event bodies). `Some` only for `LIST` chunks.
    pub list_id: Option<[u8; 4]>,
    /// Byte range of this chunk's payload in the original buffer. For a
    /// `LIST` chunk this includes the 4-byte sub-tag ahead of `children`.
    pub payload: (usize, usize),
    pub children: Vec<Chunk>,
}

impl Chunk {
    pub fn tag_str(&self) -> String {
        tag_to_string(&self.tag)
    }

    pub fn list_id_str(&self) -> Option<String> {
        self.list_id.map(|t| tag_to_string(&t))
    }

    /// This chunk's payload bytes (for a `LIST` chunk, the sub-tag plus
    /// everything the children span).
    pub fn bytes<'a>(&self, whole_file: &'a [u8]) -> &'a [u8] {
        &whole_file[self.payload.0..self.payload.1]
    }

    pub fn size(&self) -> usize {
        self.payload.1 - self.payload.0
    }
}

/// Render a 4-byte tag as text, replacing non-printable bytes with `.`
/// (tags are ASCII by construction, but stay defensive on malformed
/// input rather than panicking on non-UTF8).
pub fn tag_to_string(tag: &[u8; 4]) -> String {
    tag.iter()
        .map(|&b| {
            if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                '.'
            }
        })
        .collect()
}

/// Parse a whole `.bank` file's RIFF header and top-level chunk tree.
pub fn parse_bank(data: &[u8]) -> Result<Vec<Chunk>> {
    let mut r = Reader::new(data);
    let riff = r.read_array::<4>()?;
    if &riff != TAG_RIFF {
        bail!("not a RIFF file (tag was {:?})", tag_to_string(&riff));
    }
    let riff_size = r.read_u32()? as usize;
    let form = r.read_array::<4>()?;
    if &form != FORM_FEV {
        bail!(
            "not an FMOD bank (form type was {:?}, expected \"FEV \")",
            tag_to_string(&form)
        );
    }
    let end = (8 + riff_size).min(data.len());
    parse_chunks(data, r.position(), end)
}

/// Walk a flat run of sibling chunks spanning `[start, end)`.
pub fn parse_chunks(data: &[u8], start: usize, end: usize) -> Result<Vec<Chunk>> {
    let mut chunks = Vec::new();
    let mut pos = start;
    let mut r = Reader::new(data);
    while pos + 8 <= end {
        r.seek(pos)?;
        let node_start = pos;
        let tag = r.read_array::<4>()?;
        let size = r.read_u32()? as usize;
        let header_end = r.position();
        let next = node_start + 8 + size;
        if next > end {
            bail!(
                "chunk {} at offset {node_start} overruns its parent ({next} > {end})",
                tag_to_string(&tag)
            );
        }
        // Standard RIFF/IFF convention: an odd-sized payload is followed
        // by one pad byte so the next chunk always starts on an even
        // offset. The pad byte isn't part of this chunk's data (or its
        // declared `size`), so only the *scan* position accounts for it.
        let next_aligned = next + (size % 2);
        if size == 0 {
            pos = next_aligned;
            continue;
        }

        let (list_id, children) = if &tag == TAG_LIST {
            let sub_tag = r.read_array::<4>()?;
            let children = parse_chunks(data, r.position(), next)?;
            (Some(sub_tag), children)
        } else {
            (None, Vec::new())
        };

        chunks.push(Chunk {
            tag,
            list_id,
            payload: (header_end, next),
            children,
        });
        pos = next_aligned;
    }
    Ok(chunks)
}
