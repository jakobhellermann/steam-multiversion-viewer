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
        print_chunk(chunk, &bytes, bank.string_table.as_ref(), 0);
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

fn print_chunk(
    chunk: &Chunk,
    whole_file: &[u8],
    strings: Option<&fmod::StringTable>,
    depth: usize,
) {
    let indent = "  ".repeat(depth);
    let list_id = chunk
        .list_id_str()
        .map(|id| format!(" [{id}]"))
        .unwrap_or_default();

    // Best-effort label: most `*BODY` chunks lead with a 16-byte GUID
    // (confirmed for the handful checked against the FModBankParser
    // reference — EventNode, WaveformResourceNode, ParameterNode — but
    // not verified for every chunk type), so peeking at the first 16
    // bytes and resolving it against the string table is a heuristic,
    // not a guaranteed-correct decode of the payload.
    let guessed_name = strings.and_then(|t| {
        let payload = chunk.bytes(whole_file);
        let guid: [u8; 16] = payload.get(..16)?.try_into().ok()?;
        t.lookup(&guid).map(str::to_owned)
    });
    let label = guessed_name.map(|n| format!(" -> {n}")).unwrap_or_default();

    println!(
        "{indent}{}{list_id} ({} bytes){label}",
        chunk.tag_str(),
        chunk.size()
    );
    for child in &chunk.children {
        print_chunk(child, whole_file, strings, depth + 1);
    }
}
