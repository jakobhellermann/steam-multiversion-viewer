// TODO(ai-review): review for style and correctness
//! insta snapshots for the dll tree / diff `build_root` helpers.
//! Pure functions over `EntityEntry` lists — no ilspy on PATH, no
//! actual decompilation.

use std::collections::HashMap;

use super::fixtures::{class, entity};
use crate::dll::EntityKind;
use crate::structured::{NodeStatus, StructuredTree};

fn wrap(root: crate::structured::Node) -> StructuredTree {
    StructuredTree { root }
}

#[test]
fn tree_namespace_grouping_with_unnamespaced_bucket() {
    // Mix of namespaced + unnamespaced types — unnamespaced rows live
    // under the synthetic "-" bucket (matches ILSpy convention).
    let entities = vec![
        class("UnityEngine.UI.Image"),
        class("UnityEngine.UI.Text"),
        class("UnityEngine.Vector3"),
        class("<Module>"),
    ];
    let tree = wrap(crate::dll::tree::build_root(
        "Assembly-CSharp.dll",
        &entities,
    ));
    insta::assert_yaml_snapshot!(tree, @r#"
    root:
      id: "file:Assembly-CSharp.dll"
      label: Assembly-CSharp.dll
      kind: file
      badge: 4 entities
      children:
        - id: "ns:-"
          label: "-"
          kind: namespace
          badge: "1"
          default_collapsed: true
          children:
            - id: "type:<Module>"
              label: "<Module>"
              kind: class
              facets:
                kind: class
              has_content: true
              children: []
        - id: "ns:UnityEngine"
          label: UnityEngine
          kind: namespace
          badge: "3"
          default_collapsed: true
          children:
            - id: "type:UnityEngine.Vector3"
              label: Vector3
              kind: class
              facets:
                kind: class
              has_content: true
              children: []
            - id: "ns:UnityEngine.UI"
              label: UI
              kind: namespace
              badge: "2"
              default_collapsed: true
              children:
                - id: "type:UnityEngine.UI.Image"
                  label: Image
                  kind: class
                  facets:
                    kind: class
                  has_content: true
                  children: []
                - id: "type:UnityEngine.UI.Text"
                  label: Text
                  kind: class
                  facets:
                    kind: class
                  has_content: true
                  children: []
    "#);
}

#[test]
fn tree_nested_types_attach_to_outer() {
    // `Outer.Inner` is a nested type because `Outer` itself is in the
    // listing; without that, the dot would split a namespace.
    let entities = vec![
        class("AchievementHandler"),
        entity(
            EntityKind::Delegate,
            "AchievementHandler.AchievementAwarded",
        ),
        class("UnityEngine.UI.Button"),
        entity(EntityKind::Enum, "UnityEngine.UI.Button.ButtonClickedEvent"),
    ];
    let tree = wrap(crate::dll::tree::build_root("x", &entities));
    insta::assert_yaml_snapshot!(tree, @r#"
    root:
      id: "file:x"
      label: x
      kind: file
      badge: 4 entities
      children:
        - id: "ns:-"
          label: "-"
          kind: namespace
          badge: "1"
          default_collapsed: true
          children:
            - id: "type:AchievementHandler"
              label: AchievementHandler
              kind: class
              facets:
                kind: class
              has_content: true
              children:
                - id: "type:AchievementHandler.AchievementAwarded"
                  label: AchievementAwarded
                  kind: delegate
                  hide_unless_matched: true
                  facets:
                    kind: delegate
                  has_content: true
                  children: []
        - id: "ns:UnityEngine"
          label: UnityEngine
          kind: namespace
          badge: "2"
          default_collapsed: true
          children:
            - id: "ns:UnityEngine.UI"
              label: UI
              kind: namespace
              badge: "2"
              default_collapsed: true
              children:
                - id: "type:UnityEngine.UI.Button"
                  label: Button
                  kind: class
                  facets:
                    kind: class
                  has_content: true
                  children:
                    - id: "type:UnityEngine.UI.Button.ButtonClickedEvent"
                      label: ButtonClickedEvent
                      kind: enum
                      hide_unless_matched: true
                      facets:
                        kind: enum
                      has_content: true
                      children: []
    "#);
}

#[test]
fn diff_added_removed_changed_unchanged() {
    let from = vec![
        class("Demo.Untouched"),
        class("Demo.Touched"),
        class("Demo.Gone"),
        class("Stable.A"),
        class("Stable.B"),
    ];
    let to = vec![
        class("Demo.Untouched"),
        class("Demo.Touched"),
        class("Demo.New"),
        class("Stable.A"),
        class("Stable.B"),
    ];
    let mut statuses = HashMap::new();
    statuses.insert("Demo.Untouched".into(), NodeStatus::Unchanged);
    statuses.insert("Demo.Touched".into(), NodeStatus::Changed);
    statuses.insert("Demo.Gone".into(), NodeStatus::Removed);
    statuses.insert("Demo.New".into(), NodeStatus::Added);
    statuses.insert("Stable.A".into(), NodeStatus::Unchanged);
    statuses.insert("Stable.B".into(), NodeStatus::Unchanged);
    // `Stable` is wholly unchanged → entire namespace prunes away.
    // One-sided rows get `base:` / `target:` id prefixes so the
    // content endpoint dumps the right side.
    let tree = wrap(crate::dll::diff::build_root(
        "Assembly-CSharp.dll",
        &from,
        &to,
        &statuses,
    ));
    insta::assert_yaml_snapshot!(tree, @r#"
    root:
      id: "file:Assembly-CSharp.dll"
      label: Assembly-CSharp.dll
      kind: file
      badge: 6 entities
      status: changed
      children:
        - id: "ns:Demo"
          label: Demo
          kind: namespace
          badge: "4"
          default_collapsed: true
          status: changed
          children:
            - id: "target:type:Demo.Gone"
              label: Gone
              kind: class
              facets:
                kind: class
              status: removed
              has_content: true
              children: []
            - id: "base:type:Demo.New"
              label: New
              kind: class
              facets:
                kind: class
              status: added
              has_content: true
              children: []
            - id: "type:Demo.Touched"
              label: Touched
              kind: class
              facets:
                kind: class
              status: changed
              has_content: true
              children: []
    "#);
}

#[test]
fn diff_identical_collapses_to_empty() {
    // All types unchanged → every namespace prunes; only the file
    // root remains.
    let from = vec![class("Demo.A"), class("Demo.B")];
    let to = from.clone();
    let mut statuses = HashMap::new();
    statuses.insert("Demo.A".into(), NodeStatus::Unchanged);
    statuses.insert("Demo.B".into(), NodeStatus::Unchanged);
    let tree = wrap(crate::dll::diff::build_root("x", &from, &to, &statuses));
    insta::assert_yaml_snapshot!(tree, @r#"
    root:
      id: "file:x"
      label: x
      kind: file
      badge: 2 entities
      status: unchanged
      children: []
    "#);
}
