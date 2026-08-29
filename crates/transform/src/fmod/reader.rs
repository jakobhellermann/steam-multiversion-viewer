// TODO(ai-review): review for style and correctness
//! Little-endian byte cursor plus the handful of FMOD-specific
//! primitives (`X16` varint, length-prefixed strings/arrays, GUIDs)
//! layered on top of plain fixed-width reads.

use anyhow::{Result, anyhow};

pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn seek(&mut self, pos: usize) -> Result<()> {
        if pos > self.data.len() {
            return Err(anyhow!(
                "seek past end of buffer: {pos} > {}",
                self.data.len()
            ));
        }
        self.pos = pos;
        Ok(())
    }

    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&end| end <= self.data.len())
            .ok_or_else(|| anyhow!("unexpected EOF: need {n} bytes at offset {}", self.pos))?;
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    pub fn read_array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self
            .read_bytes(N)?
            .try_into()
            .expect("length checked by read_bytes"))
    }

    pub fn read_u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.read_array()?))
    }

    pub fn read_u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.read_array()?))
    }

    /// FMOD's variable-length int ("X16"): 15 bits, extended to 31 bits
    /// when the high bit of the first `u16` is set.
    pub fn read_x16(&mut self) -> Result<u32> {
        let low = self.read_u16()?;
        if low & 0x8000 != 0 {
            let high = self.read_u16()?;
            Ok((u32::from(low) & 0x7FFF) | (u32::from(high) << 15))
        } else {
            Ok(u32::from(low))
        }
    }

    /// `X16`-length-prefixed UTF-8 string (distinct from the
    /// null-terminated strings packed into the string-table blob).
    ///
    /// UNVERIFIED: matches `FModReader.ReadString`, but nothing in this
    /// module calls it yet, so it's never been run against real bytes.
    pub fn read_string(&mut self) -> Result<String> {
        // let len = self.read_x16()? as usize;
        // let bytes = self.read_bytes(len)?;
        // Ok(String::from_utf8_lossy(bytes).into_owned())
        todo!("read_string has never been exercised against real bank bytes")
    }

    /// Raw 16-byte GUID, read verbatim. This matches .NET's
    /// `Guid.ToByteArray()` layout, so [`format_guid`] reproduces the
    /// same dashed display FMOD Studio shows.
    pub fn read_guid(&mut self) -> Result<[u8; 16]> {
        self.read_array()
    }

    /// `ReadElemListImp`: an `X16`-encoded `count << 1`, then a single
    /// (unused) `u16` element-payload-size, then `count` fixed-size
    /// elements.
    pub fn read_elem_list<T>(
        &mut self,
        mut read_elem: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<Vec<T>> {
        let raw = self.read_x16()?;
        let count = (raw >> 1) as usize;
        if count == 0 {
            return Ok(Vec::new());
        }
        self.read_u16()?; // per-element payload size, unused
        (0..count).map(|_| read_elem(self)).collect()
    }

    /// A packed array of 24-bit little-endian unsigned ints, counted by
    /// a plain (non-shifted) `X16` value.
    pub fn read_u24_array(&mut self) -> Result<Vec<u32>> {
        let count = self.read_x16()? as usize;
        (0..count)
            .map(|_| {
                let b = self.read_array::<3>()?;
                Ok(u32::from(b[0]) | (u32::from(b[1]) << 8) | (u32::from(b[2]) << 16))
            })
            .collect()
    }
}

/// Format a raw 16-byte FMOD GUID the way FMOD Studio / .NET display it:
/// the first three fields byte-swapped (they're little-endian ints on
/// disk), the last eight bytes verbatim.
pub fn format_guid(bytes: &[u8; 16]) -> String {
    let d1 = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    let d2 = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
    let d3 = u16::from_le_bytes(bytes[6..8].try_into().unwrap());
    format!(
        "{d1:08x}-{d2:04x}-{d3:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}
