// TODO(ai-review): review for style and correctness
//! Smoke-test the `xnb` module against a real `.xnb` file. Prints the
//! header, type-reader manifest, and (for a decodable reader) writes
//! the rendered PNG/WAV next to the input as `<path>.out.{png,wav}`.
//!
//! Run with: `cargo run -p transform --example xnb_smoke -- <path.xnb>`

fn main() {
    let path = std::env::args().nth(1).expect("usage: ... <path.xnb>");
    let bytes = std::fs::read(&path).unwrap();

    let content = transform::xnb::parse(&bytes).expect("parse failed");
    println!("{:#?}", content.header);
    println!("readers:");
    for r in &content.readers {
        println!("  {} (v{})", r.type_name, r.version);
    }
    println!("shared resources: {}", content.shared_resource_count);
    println!("primary reader: {:?}", content.primary_reader);
    println!("primary payload: {} bytes", content.primary_payload.len());

    match transform::xnb::render_content(&bytes) {
        Ok(transform::xnb::RenderedContent::Png(png)) => {
            let out = format!("{path}.out.png");
            std::fs::write(&out, &png).unwrap();
            println!("wrote {out} ({} bytes)", png.len());
        }
        Ok(transform::xnb::RenderedContent::Wav(wav)) => {
            let out = format!("{path}.out.wav");
            std::fs::write(&out, &wav).unwrap();
            println!("wrote {out} ({} bytes)", wav.len());
        }
        Err(e) => println!("render_content: {e}"),
    }
}
