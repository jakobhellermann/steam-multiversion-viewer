// TODO(ai-review): review for style and correctness
//! `SoundEffectReader` payload: an already-valid WAVEFORMATEX `fmt`
//! chunk plus raw sample data, just missing RIFF/WAVE framing.

use anyhow::{Result, ensure};

pub fn to_wav(payload: &[u8]) -> Result<Vec<u8>> {
    ensure!(payload.len() >= 4, "sound effect payload truncated");
    let fmt_len = u32::from_le_bytes(payload[0..4].try_into().unwrap()) as usize;
    let fmt_end = 4 + fmt_len;
    ensure!(payload.len() >= fmt_end + 4, "sound effect payload truncated");
    let fmt = &payload[4..fmt_end];
    let data_len = u32::from_le_bytes(payload[fmt_end..fmt_end + 4].try_into().unwrap()) as usize;
    let data_start = fmt_end + 4;
    ensure!(payload.len() >= data_start + data_len, "sound effect payload truncated");
    let data = &payload[data_start..data_start + data_len];

    let mut wav = Vec::with_capacity(12 + 8 + fmt.len() + 8 + data.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&((4 + 8 + fmt.len() + 8 + data.len()) as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    wav.extend_from_slice(fmt);
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(data);
    Ok(wav)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_fmt_and_data_into_a_valid_wav() {
        let fmt = [1, 0, 2, 0, 0x44, 0xac, 0, 0, 0x10, 0xb1, 2, 0, 4, 0, 16, 0];
        let data = [0u8, 1, 2, 3];
        let mut payload = Vec::new();
        payload.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        payload.extend_from_slice(&fmt);
        payload.extend_from_slice(&(data.len() as u32).to_le_bytes());
        payload.extend_from_slice(&data);

        let wav = to_wav(&payload).unwrap();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        let fmt_chunk_len = u32::from_le_bytes(wav[16..20].try_into().unwrap()) as usize;
        assert_eq!(fmt_chunk_len, fmt.len());
        assert_eq!(&wav[20..20 + fmt.len()], &fmt);
        let data_tag_at = 20 + fmt.len();
        assert_eq!(&wav[data_tag_at..data_tag_at + 4], b"data");
        assert_eq!(&wav[data_tag_at + 8..], &data);
    }

    #[test]
    fn rejects_truncated_payload() {
        assert!(to_wav(&[1, 0, 0, 0]).is_err());
    }
}
