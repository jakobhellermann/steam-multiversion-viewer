// TODO(ai-review): review for style and correctness
//! Extract a game's own shipping-version constant from
//! `Assembly-CSharp.dll`.
//!
//! Many older Unity games never set `PlayerSettings.bundleVersion` (it
//! stays "1.0"), but keep the real version in a
//! `public const string Constants.GAME_VERSION = "1.5.78.11833"`. A
//! `const` is a compile-time literal, so it lives in the .NET constant
//! table and can be read straight from metadata — no IL execution needed.

use dll_diff::dotnetdll::prelude::{ReadOptions, Resolution};
use dll_diff::dotnetdll::resolved::members::Constant;

/// Read `Constants.GAME_VERSION` (a `const string`) from the given
/// `Assembly-CSharp.dll` bytes. `None` if the assembly doesn't parse, the
/// type/field is absent, or the field isn't a string literal.
#[tracing::instrument(skip_all, fields(bytes = dll_bytes.len()))]
pub fn extract_game_version(dll_bytes: &[u8]) -> Option<String> {
    let res = Resolution::parse(dll_bytes, ReadOptions::default()).ok()?;

    // Match on the simple type name (namespaces vary between games) and a
    // literal `GAME_VERSION` field on it. Scan every matching type so a
    // namespaced `Constants` still resolves.
    let field = res
        .enumerate_type_definitions()
        .filter(|(_, td)| td.name == "Constants")
        .flat_map(|(_, td)| td.fields.iter())
        .find(|f| f.name == "GAME_VERSION" && f.literal)?;

    match &field.default {
        Some(Constant::String(utf16)) => String::from_utf16(utf16).ok(),
        _ => None,
    }
}
