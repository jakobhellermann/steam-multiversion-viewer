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
    /// (see `sound_data`).
    ///
    /// UNVERIFIED for banks with no `SNDH` at all: every real file seen
    /// so far has had one, so there's nothing to confirm "whole `SND `
    /// chunk payload" is even the right fallback boundary — and the
    /// one real file we did check shows the FSB5 start isn't reliably
    /// at the chunk's own payload start, so this fallback could easily
    /// share that problem. Only the "no `SND ` chunks either" case is
    /// trivially correct (there's nothing to guess at), so that one
    /// still returns an empty list instead of panicking.
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
                if found.is_empty() {
                    Ok(Vec::new())
                } else {
                    // let found = ...; // (computed above)
                    // Ok(found.into_iter().map(|c| c.bytes(self.data)).collect())
                    todo!(
                        "found {} SND chunk(s) but no SNDH — extracting FSB5 by chunk \
                         boundary alone has never been verified against a real file",
                        found.len()
                    )
                }
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
