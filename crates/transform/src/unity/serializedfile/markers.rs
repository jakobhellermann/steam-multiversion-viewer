// TODO(ai-review): review for style and correctness
//! Wire-format markers shared between the JSON dump (backend) and the
//! preview HTML rewriter (frontend `markers.ts`).
//!
//! Every marker rides through the dump as a JSON *string* of the form
//!
//! ```text
//! __MARK__<type>␞<payload>
//! ```
//!
//! `␞` (U+241E) is a printable codepoint JSON.stringify passes through
//! unescaped, so shiki sees the whole sentinel as one string token —
//! one regex on the frontend reliably picks it back up. `<type>` is a
//! short lower-case tag the frontend switches on; `<payload>` is
//! marker-specific (can itself contain `␞`-separated fields).
//!
//! Adding a new marker means three places:
//! - new type tag + format helper here,
//! - one arm in the [`super::dump_value::simplify_for_dump`] walker,
//! - matching case in `markers.ts`'s `renderMarker`.
//!
//! Currently shipped markers:
//!
//! - `pptr` — `__MARK__pptr␞<ref>␞<target>␞<type>␞<file>` (Unity PPtr)
//! - `color` — `__MARK__color␞#rrggbbaa` (rgba color)

use std::collections::BTreeMap;
use std::fmt::Write as _;

use rabex_env::rabex::objects::pptr::{FileId, PPtr, PathId};
use serde_value::Value;

/// Prefix every marker starts with. Picked to be conspicuous in a JSON
/// dump (no real Unity field starts with `__`) so accidental matches
/// are unlikely.
pub const MARK_PREFIX: &str = "__MARK__";
/// Field separator inside a marker's payload. `␞` (U+241E SYMBOL FOR
/// RECORD SEPARATOR) is printable, JSON.stringify-stable, and outside
/// any plausible user content.
pub const MARK_SEP: char = '\u{241e}';

pub const MARK_TYPE_PPTR: &str = "pptr";
pub const MARK_TYPE_COLOR: &str = "color";

/// Build a `pptr` marker. Empty fields are allowed (a null pptr lands
/// in a map key as the all-empty variant; see the walker).
///
/// `side` is `""` outside of diff dumps; in the diff content endpoint
/// it's `"base"` / `"target"` so the frontend can resolve the marker
/// against the correct manifest (a `-` line's pptr targets the
/// target side, a `+` line's the base side).
pub fn pptr_marker(ref_: &str, target: &str, type_id: &str, file: &str, side: &str) -> String {
    let mut out = String::with_capacity(
        MARK_PREFIX.len() + 7 + ref_.len() + target.len() + type_id.len() + file.len() + side.len(),
    );
    let _ = write!(
        &mut out,
        "{MARK_PREFIX}{MARK_TYPE_PPTR}{MARK_SEP}{ref_}{MARK_SEP}{target}{MARK_SEP}{type_id}{MARK_SEP}{file}{MARK_SEP}{side}",
    );
    out
}

/// Build a `color` marker. `hex` should already include the leading
/// `#` and be exactly 8 lower-case hex chars (`#rrggbbaa`); the
/// frontend regex is uniform on that shape.
pub fn color_marker(hex: &str) -> String {
    format!("{MARK_PREFIX}{MARK_TYPE_COLOR}{MARK_SEP}{hex}")
}

/// Detect a PPtr-shaped map (`{m_FileID: i32, m_PathID: i64}`).
pub fn pptr_from_map(map: &BTreeMap<Value, Value>) -> Option<PPtr> {
    if map.len() != 2 {
        return None;
    }
    let file_v = map.get(&Value::String("m_FileID".to_string()))?;
    let path_v = map.get(&Value::String("m_PathID".to_string()))?;
    Some(PPtr::new(as_file_id(file_v)?, as_path_id(path_v)?))
}

/// Detect a color-shaped map (`{r,g,b,a}` of floats) and return the
/// `#rrggbbaa` payload. HDR components above 1.0 clamp to `ff` — the
/// swatch is a visual hint, not a faithful reproduction.
pub fn color_hex_from_map(map: &BTreeMap<Value, Value>) -> Option<String> {
    if map.len() != 4 {
        return None;
    }
    let r = as_unit_float(map.get(&Value::String("r".to_string()))?)?;
    let g = as_unit_float(map.get(&Value::String("g".to_string()))?)?;
    let b = as_unit_float(map.get(&Value::String("b".to_string()))?)?;
    let a = as_unit_float(map.get(&Value::String("a".to_string()))?)?;
    let to_u8 = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Some(format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        to_u8(r),
        to_u8(g),
        to_u8(b),
        to_u8(a),
    ))
}

fn as_unit_float(v: &Value) -> Option<f64> {
    match v {
        Value::F32(x) => Some(*x as f64),
        Value::F64(x) => Some(*x),
        _ => None,
    }
}

/// PPtr fields in the typetree are `int m_FileID; SInt64 m_PathID;` so
/// the deserialiser always emits I32 + I64 — no need to handle the
/// other integer widths.
pub fn as_file_id(v: &Value) -> Option<FileId> {
    match v {
        Value::I32(x) => Some(FileId::new(*x)),
        _ => None,
    }
}

pub fn as_path_id(v: &Value) -> Option<PathId> {
    match v {
        Value::I64(x) => Some(*x),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(s: &str) -> Value {
        Value::String(s.to_string())
    }

    #[test]
    fn pptr_marker_round_trip() {
        let m = pptr_marker("obj:5", "Player/Camera", "Camera", "depot.dll", "base");
        // Wire shape: prefix + "pptr" + 5 sep-delimited fields.
        assert_eq!(
            m,
            format!(
                "{MARK_PREFIX}pptr{MARK_SEP}obj:5{MARK_SEP}Player/Camera{MARK_SEP}Camera{MARK_SEP}depot.dll{MARK_SEP}base",
            )
        );
    }

    #[test]
    fn color_marker_uses_hex_payload() {
        assert_eq!(
            color_marker("#ff8800ff"),
            format!("{MARK_PREFIX}color{MARK_SEP}#ff8800ff"),
        );
    }

    #[test]
    fn pptr_from_map_detects_canonical_shape() {
        let mut map = BTreeMap::new();
        map.insert(s("m_FileID"), Value::I32(0));
        map.insert(s("m_PathID"), Value::I64(42));
        let pptr = pptr_from_map(&map).unwrap();
        assert_eq!(pptr.m_PathID, 42);
    }

    #[test]
    fn pptr_from_map_rejects_other_shapes() {
        let mut map = BTreeMap::new();
        map.insert(s("m_FileID"), Value::I32(0));
        // Missing m_PathID.
        assert!(pptr_from_map(&map).is_none());

        // Wrong types for m_FileID / m_PathID.
        let mut map = BTreeMap::new();
        map.insert(s("m_FileID"), Value::I64(0));
        map.insert(s("m_PathID"), Value::I64(1));
        assert!(pptr_from_map(&map).is_none());
    }

    #[test]
    fn color_hex_from_map_canonical_and_clamping() {
        // 1.0 → ff per channel; 0.0 → 00; mid → 80-ish.
        let mut map = BTreeMap::new();
        map.insert(s("r"), Value::F32(1.0));
        map.insert(s("g"), Value::F32(0.0));
        map.insert(s("b"), Value::F32(0.5));
        map.insert(s("a"), Value::F32(1.0));
        assert_eq!(color_hex_from_map(&map).as_deref(), Some("#ff0080ff"));

        // HDR > 1.0 clamps to ff (visual hint, not faithful).
        let mut map = BTreeMap::new();
        map.insert(s("r"), Value::F32(3.5));
        map.insert(s("g"), Value::F32(1.0));
        map.insert(s("b"), Value::F32(1.0));
        map.insert(s("a"), Value::F32(1.0));
        assert_eq!(color_hex_from_map(&map).as_deref(), Some("#ffffffff"));
    }

    #[test]
    fn color_hex_from_map_rejects_wrong_shape() {
        // Three components — not a color.
        let mut map = BTreeMap::new();
        map.insert(s("r"), Value::F32(1.0));
        map.insert(s("g"), Value::F32(0.0));
        map.insert(s("b"), Value::F32(0.0));
        assert!(color_hex_from_map(&map).is_none());
    }
}
