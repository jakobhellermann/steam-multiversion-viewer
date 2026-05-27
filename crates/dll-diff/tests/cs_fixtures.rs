// TODO(ai-review): review for style and correctness
//! C#-sourced fixture tests. Each subdirectory under `tests/fixtures/`
//! contains a `from.cs` + `to.cs` plus their pre-compiled DLLs
//! (`tests/fixtures/rebuild.sh` regenerates them; tests themselves do
//! not invoke `dotnet`).

use std::collections::HashMap;

use dll_diff::{Status, diff};

/// Run `diff` on a fixture and assert each expected entry. Statuses
/// for FQNs not listed in `expected` are not asserted, so cases can
/// focus on the type they care about and ignore the `<Module>` entry
/// dotnetdll surfaces for every assembly.
fn assert_diff(case: &str, expected: &[(&str, Status)]) {
    let from = std::fs::read(format!(
        "{}/tests/fixtures/{case}/from.dll",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let to = std::fs::read(format!(
        "{}/tests/fixtures/{case}/to.dll",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();

    let result = diff(&from, &to).unwrap();
    let actual: HashMap<&str, Status> = result
        .types
        .iter()
        .map(|t| (t.fqn.as_str(), t.status))
        .collect();

    let mut mismatches = Vec::new();
    for (fqn, want) in expected {
        match actual.get(fqn) {
            Some(got) if got == want => {}
            Some(got) => mismatches.push(format!("{fqn}: expected {want:?}, got {got:?}")),
            None => mismatches.push(format!(
                "{fqn}: expected {want:?}, missing (have {:?})",
                actual.keys().collect::<Vec<_>>()
            )),
        }
    }
    assert!(
        mismatches.is_empty(),
        "{case}:\n  {}",
        mismatches.join("\n  ")
    );
}

#[test]
fn unchanged() {
    assert_diff("unchanged", &[("Demo.Foo", Status::Unchanged)]);
}

#[test]
fn add_type() {
    assert_diff(
        "add_type",
        &[
            ("Demo.Keep", Status::Unchanged),
            ("Demo.New", Status::Added),
        ],
    );
}

#[test]
fn remove_type() {
    assert_diff(
        "remove_type",
        &[
            ("Demo.Keep", Status::Unchanged),
            ("Demo.Gone", Status::Removed),
        ],
    );
}

#[test]
fn field_change() {
    assert_diff("field_change", &[("Demo.Foo", Status::Changed)]);
}

#[test]
fn il_change() {
    assert_diff("il_change", &[("Demo.Foo", Status::Changed)]);
}

/// Reordering methods in source currently registers as `Changed`
/// because the signature is `Hash::hash(td)` which folds method
/// order in. Locked in as a regression marker — flip to `Unchanged`
/// if/when the signature is made order-independent.
#[test]
fn reorder_methods() {
    assert_diff("reorder_methods", &[("Demo.Foo", Status::Changed)]);
}

/// Nested types render with `.` (ilspy shape), and changes inside the
/// inner type surface on the inner FQN.
#[test]
fn nested() {
    assert_diff(
        "nested",
        &[
            ("Demo.Outer", Status::Unchanged),
            ("Demo.Outer.Inner", Status::Changed),
        ],
    );
}

#[test]
fn accessibility() {
    assert_diff("accessibility", &[("Demo.Foo", Status::Changed)]);
}

/// `Demo.Foo` is byte-identical on both sides; the `to` side just
/// has an unrelated decoy class in front of it that pushes Foo's
/// metadata-table indices around. ilspy decompiles both as
/// identical C# — dll-diff should say `Unchanged`. Currently flips
/// to `Changed` because `Hash::hash(td)` walks `TypeRefIndex(N)` /
/// `MethodRefIndex(N)` operands inside IL and their numeric values
/// differ across parses even though they resolve to the same
/// external entries. Pinned as a regression marker for the eventual
/// "resolve indices before hashing" fix; ignored until then.
#[test]
#[ignore = "cascading-index false positive — pending Hash-by-resolution fix"]
fn cascading_indices_unchanged() {
    assert_diff("cascading_indices", &[("Demo.Foo", Status::Unchanged)]);
}
