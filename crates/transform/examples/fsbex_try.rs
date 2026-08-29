// TODO(ai-review): review for style and correctness
//! Scratch check: does `fsbex` actually decode a real embedded FSB5
//! blob end to end? Writes the first few streams to `.ogg`/`.wav`
//! next to the input `.bank`. Not part of the permanent exploration
//! surface — gated behind the `audio` feature.

use std::io::{BufWriter, Cursor};

use fsbex::{AudioFormat, Bank};

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let data = std::fs::read(&path).unwrap();
    let parsed = transform::fmod::parse(&data).unwrap();
    let blobs = parsed.embedded_fsb5().expect("has SNDH");
    println!("{} embedded FSB5 blob(s)", blobs.len());

    for (blob_i, blob) in blobs.iter().enumerate() {
        let bank = Bank::new(Cursor::new(*blob)).expect("fsbex parse");
        println!(
            "blob {blob_i}: {} streams, format {:?}",
            bank.num_streams(),
            bank.format()
        );
        let ext = match bank.format() {
            AudioFormat::Vorbis => "ogg",
            _ => "wav",
        };
        for (i, stream) in bank.into_iter().enumerate().take(5) {
            let name = stream
                .name()
                .map(str::to_string)
                .unwrap_or_else(|| format!("stream_{i}"));
            let out_path = format!("{path}.blob{blob_i}.{name}.{ext}");
            let out = BufWriter::new(std::fs::File::create(&out_path).unwrap());
            match stream.write(out) {
                Ok(_) => println!("  [{i}] {name} -> {out_path}"),
                Err(e) => println!("  [{i}] {name} FAILED: {e}"),
            }
        }
    }
}
