// TODO(ai-review): review for style and correctness
//! Top-level `.bank` file: the chunk tree, its `STDT` string table (if
//! present), and the embedded FSB5 audio blobs.

use anyhow::{Context, Result};

use super::chunks::{Chunk, parse_bank};
use super::sound_data::parse_sound_data_header;
use super::string_table::{StringTable, parse_string_table};

const TAG_STDT: &[u8; 4] = b"STDT";
const TAG_SNDH: &[u8; 4] = b"SNDH";
const TAG_SND: &[u8; 4] = b"SND ";

pub struct Bank<'a> {
    data: &'a [u8],
    pub chunks: Vec<Chunk>,
    pub string_table: Option<StringTable>,
}

pub fn parse(data: &[u8]) -> Result<Bank<'_>> {
    let chunks = parse_bank(data)?;
    let string_table = find_chunk(&chunks, TAG_STDT)
        .map(|c| parse_string_table(c.bytes(data)))
        .transpose()
        .context("decoding STDT string table")?;
    Ok(Bank {
        data,
        chunks,
        string_table,
    })
}

impl<'a> Bank<'a> {
    /// Every embedded FSB5 blob. Sliced by the `SNDH` chunk's recorded
    /// `(offset, length)` pairs — absolute positions in the file, not
    /// necessarily equal to their matching `SND ` chunk's payload start
    /// (see `sound_data`). Falls back to each `SND ` chunk's whole
    /// payload when there's no `SNDH` at all, on the assumption an
    /// older/different-shaped bank might skip it — unverified, since
    /// every real file seen so far has had one.
    pub fn embedded_fsb5(&self) -> Result<Vec<&'a [u8]>> {
        match find_chunk(&self.chunks, TAG_SNDH) {
            Some(sndh) => parse_sound_data_header(sndh.bytes(self.data))?
                .into_iter()
                .map(|range| {
                    let start = range.offset as usize;
                    let end = start
                        .checked_add(range.length as usize)
                        .filter(|&end| end <= self.data.len())
                        .with_context(|| {
                            format!(
                                "SNDH range {start}..+{} out of bounds ({} byte file)",
                                range.length,
                                self.data.len()
                            )
                        })?;
                    Ok(&self.data[start..end])
                })
                .collect(),
            None => {
                let mut found = Vec::new();
                find_chunks(&self.chunks, TAG_SND, &mut found);
                Ok(found.into_iter().map(|c| c.bytes(self.data)).collect())
            }
        }
    }
}

fn find_chunk<'c>(chunks: &'c [Chunk], tag: &[u8; 4]) -> Option<&'c Chunk> {
    for c in chunks {
        if &c.tag == tag {
            return Some(c);
        }
        if let Some(found) = find_chunk(&c.children, tag) {
            return Some(found);
        }
    }
    None
}

fn find_chunks<'c>(chunks: &'c [Chunk], tag: &[u8; 4], out: &mut Vec<&'c Chunk>) {
    for c in chunks {
        if &c.tag == tag {
            out.push(c);
        }
        find_chunks(&c.children, tag, out);
    }
}
