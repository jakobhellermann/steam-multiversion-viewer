// TODO(ai-review): review for style and correctness
//! No `.bank` fixture is checked in here (they're copyrighted game
//! assets, not something to commit) — these hand-craft minimal byte
//! blobs matching the layouts verified against the FModBankParser
//! reference, plus two shapes found while validating against real
//! banks from a mounted depot (`examples/fmod_bank_dump.rs`): the RIFF
//! even-byte pad after an odd-sized chunk, and `SNDH` offsets landing
//! partway into a `SND ` chunk's payload rather than at its start.

use super::chunks::parse_chunks;
use super::reader::{Reader, format_guid};
use super::string_table::parse_string_table;
use super::{bank, chunks};

#[test]
fn x16_small_value_is_a_plain_u16() {
    let bytes = 5u16.to_le_bytes();
    let mut r = Reader::new(&bytes);
    assert_eq!(r.read_x16().unwrap(), 5);
}

#[test]
fn x16_large_value_extends_via_high_bit() {
    // 100000 doesn't fit in 15 bits: low word carries the low 15 bits
    // with the extension flag set, high word carries the rest.
    let value: u32 = 100_000;
    let low_word: u16 = ((value & 0x7FFF) as u16) | 0x8000;
    let high_word: u16 = (value >> 15) as u16;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&low_word.to_le_bytes());
    bytes.extend_from_slice(&high_word.to_le_bytes());

    let mut r = Reader::new(&bytes);
    assert_eq!(r.read_x16().unwrap(), value);
}

#[test]
fn guid_format_matches_dotnet_display() {
    // .NET Guid("01020304-0506-0708-090a-0b0c0d0e0f10").ToByteArray():
    // Data1/2/3 little-endian, Data4 (last 8 bytes) verbatim.
    let bytes: [u8; 16] = [
        0x04, 0x03, 0x02, 0x01, // Data1 = 0x01020304, LE
        0x06, 0x05, // Data2 = 0x0506, LE
        0x08, 0x07, // Data3 = 0x0708, LE
        0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10, // Data4, verbatim
    ];
    assert_eq!(format_guid(&bytes), "01020304-0506-0708-090a-0b0c0d0e0f10");
}

/// Builds `LIST "PROJ" { TEST(b"abcd") }` inside a `RIFF ... "FEV "`
/// wrapper, and confirms the walker recovers the exact tag, list-id and
/// payload bytes.
#[test]
fn chunk_walker_recurses_into_list() {
    let mut inner = Vec::new();
    inner.extend_from_slice(b"TEST");
    inner.extend_from_slice(&4u32.to_le_bytes());
    inner.extend_from_slice(b"abcd");

    let mut list_payload = Vec::new();
    list_payload.extend_from_slice(b"PROJ"); // list sub-tag
    list_payload.extend_from_slice(&inner);

    let mut body = Vec::new();
    body.extend_from_slice(b"LIST");
    body.extend_from_slice(&(list_payload.len() as u32).to_le_bytes());
    body.extend_from_slice(&list_payload);

    let mut file = Vec::new();
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(&((4 + body.len()) as u32).to_le_bytes()); // "FEV " + body
    file.extend_from_slice(b"FEV ");
    file.extend_from_slice(&body);

    let top = chunks::parse_bank(&file).unwrap();
    assert_eq!(top.len(), 1);
    let list = &top[0];
    assert_eq!(&list.tag, b"LIST");
    assert_eq!(list.list_id_str().as_deref(), Some("PROJ"));
    assert_eq!(list.children.len(), 1);

    let leaf = &list.children[0];
    assert_eq!(&leaf.tag, b"TEST");
    assert_eq!(leaf.bytes(&file), b"abcd");
}

#[test]
fn parse_chunks_rejects_chunk_overrunning_its_parent() {
    let mut file = Vec::new();
    file.extend_from_slice(b"TEST");
    file.extend_from_slice(&100u32.to_le_bytes()); // claims way more than is present
    file.extend_from_slice(b"ab");

    let err = parse_chunks(&file, 0, file.len()).unwrap_err();
    assert!(err.to_string().contains("overruns"));
}

/// An odd-sized payload is followed by one RIFF pad byte before the
/// next chunk (confirmed against a real `ui.bank`: a 31-byte `BUS `
/// chunk with a stray `0x00` before the following `LIST`). The pad
/// byte isn't part of either chunk's data.
#[test]
fn odd_sized_chunk_is_skipped_over_its_pad_byte() {
    let mut file = Vec::new();
    file.extend_from_slice(b"ODD1");
    file.extend_from_slice(&3u32.to_le_bytes());
    file.extend_from_slice(b"abc");
    file.push(0); // RIFF pad byte, not part of ODD1's declared size

    file.extend_from_slice(b"EVEN");
    file.extend_from_slice(&4u32.to_le_bytes());
    file.extend_from_slice(b"defg");

    let top = parse_chunks(&file, 0, file.len()).unwrap();
    assert_eq!(top.len(), 2);
    assert_eq!(&top[0].tag, b"ODD1");
    assert_eq!(top[0].bytes(&file), b"abc");
    assert_eq!(&top[1].tag, b"EVEN");
    assert_eq!(top[1].bytes(&file), b"defg");
}

/// Builds a minimal 24-bit-radix-tree `STDT` payload with exactly one
/// entry: GUID `[0; 16]` -> the single segment `"hello"` (one node,
/// which is both the leaf and the root).
fn one_entry_string_table_payload() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&1u32.to_le_bytes()); // EStringTableType::RadixTree_24Bit

    // Nodes: X16(raw=2) -> count=1, then unused payload-size u16, then
    // one FPackedNode { string_offset: 0, child_info: 0 }.
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // key_info (string_offset = 0)
    out.extend_from_slice(&0u32.to_le_bytes()); // child_info, unused

    // Guids: same X16(raw=2) -> count=1 encoding, then one all-zero GUID.
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 16]);

    // String blob: X16 length, then the raw (NUL-terminated) bytes.
    let blob = b"hello\0";
    out.extend_from_slice(&(blob.len() as u16).to_le_bytes());
    out.extend_from_slice(blob);

    // LeafIndices: plain X16 count (not shifted), then one u24 = 0.
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&[0x00, 0x00, 0x00]);

    // ParentIndices: same encoding, one u24 = SENTINEL (root, no parent).
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&[0xFF, 0xFF, 0xFF]);

    out
}

#[test]
fn string_table_resolves_single_segment_path() {
    let payload = one_entry_string_table_payload();
    let table = parse_string_table(&payload).unwrap();
    assert_eq!(table.len(), 1);
    assert_eq!(table.lookup(&[0u8; 16]), Some("hello"));
}

/// End-to-end: a synthetic `RIFF ... "FEV " { LIST "PROJ" { STDT, SND } }`
/// bank exercises the chunk walker, string-table decode and FSB5
/// extraction together.
#[test]
fn parse_bank_finds_string_table_and_sound_data() {
    let stdt_payload = one_entry_string_table_payload();
    let mut stdt_chunk = Vec::new();
    stdt_chunk.extend_from_slice(b"STDT");
    stdt_chunk.extend_from_slice(&(stdt_payload.len() as u32).to_le_bytes());
    stdt_chunk.extend_from_slice(&stdt_payload);

    let fsb5_stub = b"FSB5-stand-in-audio-bytes";
    let mut snd_chunk = Vec::new();
    snd_chunk.extend_from_slice(b"SND ");
    snd_chunk.extend_from_slice(&(fsb5_stub.len() as u32).to_le_bytes());
    snd_chunk.extend_from_slice(fsb5_stub);

    let mut list_payload = Vec::new();
    list_payload.extend_from_slice(b"PROJ");
    list_payload.extend_from_slice(&stdt_chunk);
    list_payload.extend_from_slice(&snd_chunk);

    let mut body = Vec::new();
    body.extend_from_slice(b"LIST");
    body.extend_from_slice(&(list_payload.len() as u32).to_le_bytes());
    body.extend_from_slice(&list_payload);

    let mut file = Vec::new();
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(&((4 + body.len()) as u32).to_le_bytes());
    file.extend_from_slice(b"FEV ");
    file.extend_from_slice(&body);

    let parsed = bank::parse(&file).unwrap();
    let table = parsed
        .string_table
        .as_ref()
        .expect("STDT chunk was present");
    assert_eq!(table.lookup(&[0u8; 16]), Some("hello"));

    let fsb = parsed.embedded_fsb5().unwrap();
    assert_eq!(fsb, vec![fsb5_stub.as_slice()]);
}

/// A real `ui.bank` had its `SND ` chunk's FSB5 payload start 28 bytes
/// past the chunk's own payload start, with `SNDH` recording the true
/// (absolute-file-offset, length) pair. Mirrors that shape at a smaller
/// scale to pin down that `embedded_fsb5` follows `SNDH`, not the `SND `
/// chunk boundary.
#[test]
fn embedded_fsb5_follows_sndh_absolute_offsets() {
    let fsb5_bytes = b"REALFSB5DATA";
    let mut snd_payload = vec![0u8; 8]; // stand-in for the real file's prefix
    snd_payload.extend_from_slice(fsb5_bytes);
    let mut snd_chunk = Vec::new();
    snd_chunk.extend_from_slice(b"SND ");
    snd_chunk.extend_from_slice(&(snd_payload.len() as u32).to_le_bytes());
    snd_chunk.extend_from_slice(&snd_payload);

    // Absolute file offset of `fsb5_bytes`: 12 (RIFF header) + 8 (LIST
    // header) + 4 ("PROJ") + 8 (SND chunk header) + 8 (in-payload
    // prefix) = 40.
    let fsb5_absolute_offset: u32 = 40;

    let mut sndh_payload = Vec::new();
    sndh_payload.extend_from_slice(&2u16.to_le_bytes()); // X16(raw=2) -> count=1
    sndh_payload.extend_from_slice(&0u16.to_le_bytes()); // unused payload size
    sndh_payload.extend_from_slice(&fsb5_absolute_offset.to_le_bytes());
    sndh_payload.extend_from_slice(&(fsb5_bytes.len() as u32).to_le_bytes());
    let mut sndh_chunk = Vec::new();
    sndh_chunk.extend_from_slice(b"SNDH");
    sndh_chunk.extend_from_slice(&(sndh_payload.len() as u32).to_le_bytes());
    sndh_chunk.extend_from_slice(&sndh_payload);

    let mut list_payload = Vec::new();
    list_payload.extend_from_slice(b"PROJ");
    list_payload.extend_from_slice(&snd_chunk);
    list_payload.extend_from_slice(&sndh_chunk);

    let mut body = Vec::new();
    body.extend_from_slice(b"LIST");
    body.extend_from_slice(&(list_payload.len() as u32).to_le_bytes());
    body.extend_from_slice(&list_payload);

    let mut file = Vec::new();
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(&((4 + body.len()) as u32).to_le_bytes());
    file.extend_from_slice(b"FEV ");
    file.extend_from_slice(&body);

    let parsed = bank::parse(&file).unwrap();
    let fsb = parsed.embedded_fsb5().unwrap();
    assert_eq!(fsb, vec![fsb5_bytes.as_slice()]);
}
