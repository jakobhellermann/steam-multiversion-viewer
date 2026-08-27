// TODO(ai-review): review for style and correctness
//! `native.dll` is `-nostdlib -nostartfiles`: exactly one export, one import, no CRT noise.

use crate::dll::native::build_tree;

const NATIVE_DLL: &[u8] = include_bytes!("native.dll");

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
