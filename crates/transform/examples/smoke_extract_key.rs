// TODO(ai-review): review for style and correctness
//! Smoke-test the `extract_key` helper against a real `Assembly-CSharp.dll`.
//! Pass the DLL path as the first arg; prints the decoded key (UTF-8)
//! or panics if extraction returns `None`.
//!
//! Run with: `cargo run -p transform --example dump_securecplayerprefs_il -- <DLL>`

use transform::unity::secure_player_prefs::extract_key;

fn main() {
    let path = std::env::args().nth(1).expect("usage: ... <DLL>");
    let bytes = std::fs::read(&path).unwrap();
    let key = extract_key(&bytes).expect("extract_key returned None");
    let pretty = std::str::from_utf8(&key).unwrap_or("<non-utf8>");
    println!("key bytes: {} (utf8: {pretty})", key.len());
}
