// TODO(ai-review): review for style and correctness
//! Decoder for the `SNDH` (SoundDataHeader) chunk: one `(offset,
//! length)` byte range per embedded FSB5 blob, in the same order as
//! the `SND ` chunks they describe.
//!
//! These offsets are **absolute positions in the whole file**, and,
//! cross-checked against a real bank, do not necessarily line up with
//! where the matching `SND ` chunk's own payload starts — one real
//! `ui.bank` had a 28-byte gap between the chunk payload start and the
//! actual `"FSB5"` magic `SNDH` pointed past. So this is the
//! authoritative way to locate embedded audio, not a cross-check on
//! the chunk tree.

use anyhow::Result;

use super::reader::Reader;

pub struct SoundDataRange {
    pub offset: u32,
    pub length: u32,
}

pub fn parse_sound_data_header(data: &[u8]) -> Result<Vec<SoundDataRange>> {
    let mut r = Reader::new(data);
    r.read_elem_list(|r| {
        let offset = r.read_u32()?;
        let length = r.read_u32()?;
        Ok(SoundDataRange { offset, length })
    })
}
