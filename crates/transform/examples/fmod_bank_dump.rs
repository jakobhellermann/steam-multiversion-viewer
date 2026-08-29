// TODO(ai-review): review for style and correctness
//! Dump an FMOD Studio `.bank` file's chunk tree and extract its
//! embedded FSB5 blob(s) to disk next to the input, for validating the
//! exploratory parser in `transform::fmod` against a real file.
//!
//! Run with: `cargo run -p transform --example fmod_bank_dump -- <bank>`

use transform::fmod;
use transform::fmod::Chunk;

fn main() {
    let path = std::env::args().nth(1).expect("usage: ... <bank>");
    let bytes = std::fs::read(&path).expect("read bank file");

    let bank = fmod::parse(&bytes).expect("parse .bank");

    match &bank.string_table {
        Some(t) => println!("string table: {} entries\n", t.len()),
        None => println!("no STDT string table (no sibling .strings.bank was baked in)\n"),
    }

    for chunk in &bank.chunks {
        print_chunk(chunk, 0);
    }

    let fsb5 = bank.embedded_fsb5().expect("resolve SNDH offsets");
    println!("\n{} embedded FSB5 blob(s)", fsb5.len());
    for (i, blob) in fsb5.iter().enumerate() {
        let magic = &blob[..blob.len().min(4)];
        if magic != b"FSB5" {
            println!(
                "  [{i}] {} bytes, magic {:?} (not \"FSB5\" — encrypted title, or this isn't really audio?)",
                blob.len(),
                String::from_utf8_lossy(magic)
            );
            continue;
        }
        let out_path = format!("{path}.sound{i}.fsb");
        std::fs::write(&out_path, blob).expect("write extracted fsb");
        println!("  [{i}] {} bytes -> {out_path}", blob.len());
    }
}

fn print_chunk(chunk: &Chunk, depth: usize) {
    let indent = "  ".repeat(depth);
    let list_id = chunk
        .list_id_str()
        .map(|id| format!(" [{id}]"))
        .unwrap_or_default();

    // No per-chunk name label here: guessing a chunk's GUID by peeking
    // at its first 16 bytes (see `guess_chunk_name` below) is unverified
    // beyond the one BNKI chunk it happened to look right for — left
    // unused rather than risk a label that reads as a decoded fact.
    println!(
        "{indent}{}{list_id} ({} bytes)",
        chunk.tag_str(),
        chunk.size()
    );
    for child in &chunk.children {
        print_chunk(child, depth + 1);
    }
}

/// UNVERIFIED, not called from `main`: peeking at a chunk's first 16
/// bytes as a GUID and resolving it via the string table only checked
/// out for one real chunk (`BNKI`, in `Master Bank.strings.bank`) — not
/// confirmed for `*BODY` chunks generally, whose actual layout (per
/// FModBankParser) is a fixed, per-node-type, per-FMOD-version struct
/// that a blind 16-byte peek can't tell apart from one that doesn't
/// lead with a GUID at all.
#[allow(dead_code)]
fn guess_chunk_name(
    _chunk: &Chunk,
    _whole_file: &[u8],
    _strings: &fmod::StringTable,
) -> Option<String> {
    // let payload = chunk.bytes(whole_file);
    // let guid: [u8; 16] = payload.get(..16)?.try_into().ok()?;
    // strings.lookup(&guid).map(str::to_owned)
    todo!(
        "only checked against one real chunk kind (BNKI) — verify per chunk tag before trusting this"
    )
}
