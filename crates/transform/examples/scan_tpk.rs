// TODO(ai-review): review for style and correctness
//! Scan the embedded TPK at a fixed Unity version for engine classes
//! whose typetree contains the field shapes we need to exercise
//! [`crate::unity::serializedfile::dump_value::simplify_for_dump`]:
//!
//! - **`ColorRGBA`** sub-trees (r/g/b/a floats) — drive the color
//!   marker rewrite.
//! - **`map`** sub-trees with non-string keys (the wire shape that
//!   forces the `{key,value}` flattening branch).
//!
//! Run with: `cargo run --example scan_tpk -p transform`.
//!
//! Output is two lists of class names; pick something small and
//! easy to construct in a fixture (e.g. `Light` for color,
//! `Lightmap*` for maps).

use rabex_env::rabex::objects::ClassId;
use rabex_env::rabex::tpk::TpkTypeTreeBlob;
use rabex_env::rabex::typetree::TypeTreeNode;

const UNITY_VERSION: &str = "2022.3.0f1";

fn main() {
    let tpk = TpkTypeTreeBlob::embedded();
    let version = UNITY_VERSION.parse().unwrap();

    let mut color_classes: Vec<(String, Vec<String>)> = Vec::new();
    let mut map_classes: Vec<(String, Vec<String>)> = Vec::new();

    // Iterate over the engine class-id range. `ClassId::name()` returns
    // None for ids the embedded class-table doesn't know about, and
    // `get_typetree_node` returns None when the TPK has no entry —
    // either filter skips silently.
    for id in 0..=2000 {
        let class_id = ClassId(id);
        let Some(name) = class_id.name() else {
            continue;
        };
        if class_id == ClassId::MonoBehaviour {
            // MB tt is the base layout (no script fields); skip.
            continue;
        }
        let Some(tt) = tpk.get_typetree_node(class_id, &version) else {
            continue;
        };
        let mut colors = Vec::new();
        let mut maps = Vec::new();
        walk(&tt, "", &mut colors, &mut maps);
        if !colors.is_empty() {
            color_classes.push((name.to_string(), colors));
        }
        if !maps.is_empty() {
            map_classes.push((name.to_string(), maps));
        }
    }

    color_classes.sort_by_key(|(_, fields)| fields.len());
    map_classes.sort_by_key(|(_, fields)| fields.len());

    println!("# Classes with ColorRGBA fields (smallest first):\n");
    for (name, fields) in color_classes.iter().take(20) {
        println!("  {name}  ({})", fields.len());
        for f in fields {
            println!("    - {f}");
        }
    }

    println!("\n# Classes with non-string-keyed `map` fields:\n");
    for (name, fields) in map_classes.iter().take(20) {
        println!("  {name}  ({})", fields.len());
        for f in fields {
            println!("    - {f}");
        }
    }

    // Dump a few promising candidates' full TT so we can hand-write a
    // matching Rust struct.
    for class_name in ["CachedSpriteAtlas"] {
        let Some(class_id) = (0..=2000)
            .map(ClassId)
            .find(|c| c.name() == Some(class_name))
        else {
            continue;
        };
        let Some(tt) = tpk.get_typetree_node(class_id, &version) else {
            continue;
        };
        println!("\n# TT: {class_name}\n");
        dump_tt(&tt, 0);
    }
}

fn dump_tt(node: &TypeTreeNode, depth: usize) {
    let indent = "  ".repeat(depth);
    println!("{indent}{}: {}", node.m_Name, node.m_Type);
    for child in &node.children {
        dump_tt(child, depth + 1);
    }
}

/// Walk the typetree, collecting paths to `ColorRGBA` nodes and to
/// `map` nodes whose key type isn't a string. The wire shape of a
/// `map<K, V>` typetree is `map -> Array -> data: pair -> {first: K,
/// second: V}` (rabex names the inner record `pair`), so we recurse
/// into the array element and check the first child of the pair.
fn walk(node: &TypeTreeNode, path: &str, colors: &mut Vec<String>, maps: &mut Vec<String>) {
    let here = if path.is_empty() {
        node.m_Name.clone()
    } else {
        format!("{path}.{}", node.m_Name)
    };
    if node.m_Type == "ColorRGBA" {
        colors.push(here.clone());
    }
    if node.m_Type == "map" {
        if let Some(key_type) = map_key_type(node) {
            if key_type != "string" {
                maps.push(format!("{here} <{key_type},_>"));
            }
        }
    }
    for child in &node.children {
        walk(child, &here, colors, maps);
    }
}

/// Best-effort lookup of `map`'s key type. The TT shape is
/// `map → Array → data:pair → {first:K, second:V}`.
fn map_key_type(node: &TypeTreeNode) -> Option<&str> {
    let array = node.children.iter().find(|c| c.m_Type == "Array")?;
    let data = array.children.iter().find(|c| c.m_Name == "data")?;
    let first = data.children.iter().find(|c| c.m_Name == "first")?;
    Some(first.m_Type.as_str())
}
