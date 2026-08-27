// TODO(ai-review): review for style and correctness
//! `native.dll` is `-nostdlib -nostartfiles`: exactly one export, one import, no CRT noise.
//! `native2.dll` is the same source plus a second export (`sub`); same one import.
//! `native3.dll`/`native4.dll` only differ in an `.rdata` string constant of equal length.

use crate::dll::native::{build_diff_tree, build_tree};
use crate::structured::NodeStatus;

const NATIVE_DLL: &[u8] = include_bytes!("native.dll");
const NATIVE_DLL_2: &[u8] = include_bytes!("native2.dll");
const NATIVE_DLL_3: &[u8] = include_bytes!("native3.dll");
const NATIVE_DLL_4: &[u8] = include_bytes!("native4.dll");

#[test]
fn native_dll_is_not_managed() {
    assert!(!crate::dll::is_managed_pe(NATIVE_DLL));
}

#[test]
fn native_dll_tree_has_sections_export_and_import() {
    let tree = build_tree(NATIVE_DLL, "tiny2.dll").unwrap();
    assert_eq!(tree.root.label, "tiny2.dll");
    assert_eq!(tree.root.children.len(), 3);

    let sections = &tree.root.children[0];
    assert_eq!(sections.label, "Sections");
    assert_eq!(sections.badge.as_deref(), Some("6"));
    let section_names: Vec<&str> = sections.children.iter().map(|n| n.label.as_str()).collect();
    assert_eq!(
        section_names,
        [".text", ".rdata", ".pdata", ".xdata", ".edata", ".idata"]
    );

    let exports = &tree.root.children[1];
    assert_eq!(exports.label, "Exports");
    assert_eq!(exports.badge.as_deref(), Some("1"));
    assert_eq!(exports.children.len(), 1);
    assert_eq!(exports.children[0].label, "add");

    let imports = &tree.root.children[2];
    assert_eq!(imports.label, "Imports");
    assert_eq!(imports.badge.as_deref(), Some("1"));
    assert_eq!(imports.children.len(), 1);
    assert_eq!(imports.children[0].label, "KERNEL32.dll");
    assert_eq!(imports.children[0].badge.as_deref(), Some("1"));
    assert_eq!(imports.children[0].children.len(), 1);
    assert_eq!(imports.children[0].children[0].label, "GetLastError");
}

#[test]
fn native_dll_diff_shows_added_export_and_changed_sections() {
    let tree = build_diff_tree(NATIVE_DLL, NATIVE_DLL_2, "tiny2.dll").unwrap();
    assert_eq!(tree.root.status, Some(NodeStatus::Changed));
    assert_eq!(tree.root.children.len(), 2);

    let sections = &tree.root.children[0];
    assert_eq!(sections.label, "Sections");
    assert_eq!(sections.status, Some(NodeStatus::Changed));
    let changed_sections: Vec<&str> = sections.children.iter().map(|n| n.label.as_str()).collect();
    assert_eq!(changed_sections, [".text", ".pdata", ".xdata", ".edata"]);
    assert!(
        sections
            .children
            .iter()
            .all(|n| n.status == Some(NodeStatus::Changed))
    );

    let exports = &tree.root.children[1];
    assert_eq!(exports.label, "Exports");
    assert_eq!(exports.children.len(), 1);
    assert_eq!(exports.children[0].label, "sub");
    assert_eq!(exports.children[0].status, Some(NodeStatus::Added));

    // Same single import on both sides -> no Imports group at all.
    assert!(tree.root.children.iter().all(|c| c.label != "Imports"));
}

#[test]
fn native_dll_diff_is_symmetric_for_removed() {
    let tree = build_diff_tree(NATIVE_DLL_2, NATIVE_DLL, "tiny2.dll").unwrap();
    let exports = tree
        .root
        .children
        .iter()
        .find(|c| c.label == "Exports")
        .unwrap();
    assert_eq!(exports.children.len(), 1);
    assert_eq!(exports.children[0].label, "sub");
    assert_eq!(exports.children[0].status, Some(NodeStatus::Removed));
}

#[test]
fn native_dll_diff_detects_same_size_content_change() {
    let tree = build_diff_tree(NATIVE_DLL_3, NATIVE_DLL_4, "tiny4.dll").unwrap();
    let sections = tree
        .root
        .children
        .iter()
        .find(|c| c.label == "Sections")
        .unwrap();
    let rdata = sections
        .children
        .iter()
        .find(|n| n.label == ".rdata")
        .unwrap();
    assert_eq!(rdata.status, Some(NodeStatus::Changed));
    // Same size on both sides -> no "from → to" arrow in the badge.
    assert_eq!(rdata.badge.as_deref(), Some("80 B"));
}
