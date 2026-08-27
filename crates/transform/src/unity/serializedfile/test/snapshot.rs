// TODO(ai-review): review for style and correctness
//! insta snapshots for the unity structured-tree and diff builders.
//!
//! Each test assembles a [`super::fixtures::Scene`] in memory, runs
//! the production tree / diff builders against it, and yaml-snapshots
//! the resulting [`StructuredTree`]. Snapshots live next to this file
//! under `snapshots/`.

use super::fixtures::{
    Scene, SceneNode, external_gameobject_with_transform, external_monoscript_file,
    external_shader_file, external_text_asset_file, loose_monobehaviour_referencing_external,
    loose_monobehaviour_with_script_typetree, preload_referencing_external,
    preload_referencing_local, preload_with_dependency, with_diff_handles, with_handle,
};
use crate::structured::{NodeStatus, StructuredTree};
use crate::unity::serializedfile::diff::diff_sections;
use crate::unity::serializedfile::tree::build_root_node;

const PATH: &str = "level0";

fn small_scene() -> Scene {
    Scene::new()
        .with_root(
            SceneNode::new("Player")
                .with_child(SceneNode::new("Camera"))
                .with_child(SceneNode::new("Weapon")),
        )
        .with_root(SceneNode::new("Light"))
        .with_asset_bundle("test_bundle")
}

#[test]
fn tree_small_scene() {
    let bytes = small_scene().write();
    let tree = with_handle(PATH, bytes, |handle| {
        let root = build_root_node(handle, PATH).unwrap();
        StructuredTree { root }
    });
    insta::assert_yaml_snapshot!(tree, @r#"
    root:
      id: "file:level0"
      label: level0
      kind: file
      badge: 9 objects
      children:
        - id: "section:class-stats"
          label: Class stats
          kind: section
          badge: 9 objects
          default_collapsed: true
          children:
            - id: "class:GameObject"
              label: GameObject
              kind: class-stat
              badge: ×4
              children: []
            - id: "class:Transform"
              label: Transform
              kind: class-stat
              badge: ×4
              children: []
            - id: "class:AssetBundle"
              label: AssetBundle
              kind: class-stat
              badge: ×1
              children: []
        - id: "section:hierarchy"
          label: Hierarchy
          kind: section
          badge: 2 roots
          children:
            - id: "obj:2"
              label: Player
              kind: gameobject
              default_collapsed: true
              include_descendants_on_match: true
              has_content: true
              children:
                - id: "obj:4"
                  label: Camera
                  kind: gameobject
                  default_collapsed: true
                  include_descendants_on_match: true
                  has_content: true
                  children: []
                - id: "obj:6"
                  label: Weapon
                  kind: gameobject
                  default_collapsed: true
                  include_descendants_on_match: true
                  has_content: true
                  children: []
            - id: "obj:8"
              label: Light
              kind: gameobject
              default_collapsed: true
              include_descendants_on_match: true
              has_content: true
              children: []
        - id: "section:loose"
          label: Loose components
          kind: section
          badge: "1"
          children:
            - id: "obj:1"
              label: test_bundle
              kind: component
              badge: AssetBundle
              facets:
                class: AssetBundle
              has_content: true
              children: []
    "#);
}

#[test]
fn diff_added_removed_renamed() {
    // base: Player has Camera + Weapon, plus a Light root.
    let base_bytes = small_scene().write();
    // target: Weapon renamed to Sword (changed), Camera removed (removed),
    // new Hat child added (added), Light unchanged, AssetBundle name
    // changed.
    let target_bytes = Scene::new()
        .with_root(
            SceneNode::new("Player")
                .with_child(SceneNode::new("Sword"))
                .with_child(SceneNode::new("Hat")),
        )
        .with_root(SceneNode::new("Light"))
        .with_asset_bundle("test_bundle_v2")
        .write();

    let tree = with_handle(PATH, base_bytes, |base| {
        with_handle(PATH, target_bytes, |target| {
            let (children, status) = diff_sections(base, target).unwrap();
            StructuredTree {
                root: crate::structured::Node {
                    id: format!("file:{PATH}"),
                    label: PATH.to_string(),
                    kind: "file".to_string(),
                    status: Some(status),
                    children,
                    ..Default::default()
                },
            }
        })
    });
    insta::assert_yaml_snapshot!(tree, @r#"
    root:
      id: "file:level0"
      label: level0
      kind: file
      status: changed
      children:
        - id: "section:hierarchy"
          label: Hierarchy
          kind: section
          badge: 2 roots
          status: changed
          children:
            - id: "obj:2"
              label: Player
              kind: gameobject
              default_collapsed: true
              status: changed
              has_content: true
              children:
                - id: "base:obj:4"
                  label: Camera
                  kind: gameobject
                  default_collapsed: true
                  status: added
                  has_content: true
                  children: []
                - id: "base:obj:6"
                  label: Weapon
                  kind: gameobject
                  default_collapsed: true
                  status: added
                  has_content: true
                  children: []
                - id: "target:obj:4"
                  label: Sword
                  kind: gameobject
                  default_collapsed: true
                  status: removed
                  has_content: true
                  children: []
                - id: "target:obj:6"
                  label: Hat
                  kind: gameobject
                  default_collapsed: true
                  status: removed
                  has_content: true
                  children: []
        - id: "section:loose"
          label: Loose components
          kind: section
          badge: 1 object
          status: changed
          children:
            - id: "obj:1"
              label: AssetBundle
              kind: component
              badge: "[1]"
              facets:
                class: AssetBundle
              status: changed
              has_content: true
              children: []
    "#);
}

#[test]
fn diff_external_pptr_renumber_to_same_name_is_unchanged() {
    // The two manifests reference the same external asset ("shared"),
    // but it sits at a different path id in each — a renumber, not a
    // content change. The loose PreloadData that points at it must
    // compare equal (resolved identity matches) and prune away entirely.
    let ext = "extern.assets";
    let (status, children) = with_diff_handles(
        &[
            (PATH, preload_referencing_external(ext, 5)),
            (ext, external_text_asset_file(5, "shared")),
        ],
        &[
            (PATH, preload_referencing_external(ext, 9)),
            (ext, external_text_asset_file(9, "shared")),
        ],
        |base, target| {
            let (children, status) = diff_sections(base, target).unwrap();
            (status, children)
        },
    );

    assert_eq!(status, NodeStatus::Unchanged);
    assert!(
        children.is_empty(),
        "expected every section pruned, got {} section(s)",
        children.len()
    );
}

#[test]
fn diff_external_shader_renumber_to_same_parsed_form_name_is_unchanged() {
    // The external target is a Shader, whose top-level `m_Name` is empty
    // — its name lives in `m_ParsedForm.m_Name`. Across the two manifests
    // the shader renumbers (5 → 9) but keeps the same parsed-form name, so
    // the referencing PreloadData must resolve to the same identity and
    // prune away. Regression for shader pptrs being reported as changed.
    let ext = "extern.assets";
    let (status, children) = with_diff_handles(
        &[
            (PATH, preload_referencing_external(ext, 5)),
            (ext, external_shader_file(5, "Hidden/FastBloom")),
        ],
        &[
            (PATH, preload_referencing_external(ext, 9)),
            (ext, external_shader_file(9, "Hidden/FastBloom")),
        ],
        |base, target| {
            let (children, status) = diff_sections(base, target).unwrap();
            (status, children)
        },
    );

    assert_eq!(status, NodeStatus::Unchanged);
    assert!(
        children.is_empty(),
        "expected every section pruned, got {} section(s)",
        children.len()
    );
}

#[test]
fn diff_external_shader_with_different_parsed_form_name_is_changed() {
    // Same shape, but the shader's parsed-form name differs across the
    // two manifests — a genuine reference change that must survive.
    let ext = "extern.assets";
    let (status, labels) = with_diff_handles(
        &[
            (PATH, preload_referencing_external(ext, 5)),
            (ext, external_shader_file(5, "Hidden/FastBloom")),
        ],
        &[
            (PATH, preload_referencing_external(ext, 9)),
            (ext, external_shader_file(9, "Hidden/SlowBloom")),
        ],
        |base, target| {
            let (children, status) = diff_sections(base, target).unwrap();
            let mut labels = Vec::new();
            collect_changed_labels(&children, &mut labels);
            (status, labels)
        },
    );

    assert_eq!(status, NodeStatus::Changed);
    assert_eq!(labels, vec!["PreloadData".to_string()]);
}

#[test]
fn diff_external_pptr_to_nameless_component_renumber_is_unchanged() {
    // The PreloadData points at a Transform in an external file. A
    // Transform has no `m_Name`, so its identity has to come from the
    // GameObject it hangs on. Across the two manifests the transform
    // (and its gameobject) renumber, but the gameobject name stays
    // "Bench" — a pure renumber that must collapse to unchanged.
    let ext = "extern.assets";
    let (status, children) = with_diff_handles(
        &[
            (PATH, preload_referencing_external(ext, 5)),
            (ext, external_gameobject_with_transform(5, "Bench")),
        ],
        &[
            (PATH, preload_referencing_external(ext, 9)),
            (ext, external_gameobject_with_transform(9, "Bench")),
        ],
        |base, target| {
            let (children, status) = diff_sections(base, target).unwrap();
            (status, children)
        },
    );

    assert_eq!(status, NodeStatus::Unchanged);
    assert!(
        children.is_empty(),
        "expected every section pruned, got {} section(s)",
        children.len()
    );
}

#[test]
fn diff_local_pptr_renumber_to_same_name_is_unchanged() {
    // Both manifests hold a "Bench" GameObject + a loose PreloadData
    // pointing at it with a *local* pptr (m_FileID == 0). The gameobject
    // sits at a different path id on each side — a pure local renumber.
    // The PreloadData must compare equal once local pptr identity is
    // resolved, leaving nothing to show.
    let (status, children) = with_diff_handles(
        &[(PATH, preload_referencing_local(5))],
        &[(PATH, preload_referencing_local(9))],
        |base, target| {
            let (children, status) = diff_sections(base, target).unwrap();
            (status, children)
        },
    );

    assert_eq!(status, NodeStatus::Unchanged);
    assert!(
        children.is_empty(),
        "expected every section pruned, got {} section(s)",
        children.len()
    );
}

#[test]
fn diff_external_pptr_to_differently_named_target_is_changed() {
    // Same shape, but now the external target has a different name on
    // each side — a genuine reference change that must survive.
    let ext = "extern.assets";
    let (status, labels) = with_diff_handles(
        &[
            (PATH, preload_referencing_external(ext, 5)),
            (ext, external_text_asset_file(5, "before")),
        ],
        &[
            (PATH, preload_referencing_external(ext, 9)),
            (ext, external_text_asset_file(9, "after")),
        ],
        |base, target| {
            let (children, status) = diff_sections(base, target).unwrap();
            let mut labels = Vec::new();
            collect_changed_labels(&children, &mut labels);
            (status, labels)
        },
    );

    assert_eq!(status, NodeStatus::Changed);
    assert_eq!(labels, vec!["PreloadData".to_string()]);
}

#[test]
fn diff_monobehaviour_renumber_stays_changed() {
    // A MonoBehaviour whose only byte difference is an external m_Script
    // renumber to the same-named script. The resolved-identity compare
    // *would* collapse it — but MBs often lack a complete (script-
    // specific) type tree, so we exclude them: byte-diff stays changed.
    let ext = "extern.assets";
    let (status, changed) = with_diff_handles(
        &[
            (PATH, loose_monobehaviour_referencing_external(ext, 5)),
            (ext, external_monoscript_file(5, "S")),
        ],
        &[
            (PATH, loose_monobehaviour_referencing_external(ext, 9)),
            (ext, external_monoscript_file(9, "S")),
        ],
        |base, target| {
            let (children, status) = diff_sections(base, target).unwrap();
            let mut labels = Vec::new();
            collect_changed_labels(&children, &mut labels);
            (status, labels.len())
        },
    );

    assert_eq!(status, NodeStatus::Changed);
    assert_eq!(changed, 1, "the MonoBehaviour must not collapse");
}

#[test]
fn diff_monobehaviour_renumber_with_script_typetree_is_unchanged() {
    // Same renumber-only diff as `diff_monobehaviour_renumber_stays_changed`,
    // but the MonoBehaviour carries a script-specific type tree (root
    // `m_Type` is the script class, not `"MonoBehaviour"`). Now `read()`
    // yields the full fields, the resolved-identity compare is trusted,
    // and the pure external-script renumber collapses to unchanged.
    let ext = "extern.assets";
    let (status, changed) = with_diff_handles(
        &[
            (PATH, loose_monobehaviour_with_script_typetree(ext, 5)),
            (ext, external_monoscript_file(5, "S")),
        ],
        &[
            (PATH, loose_monobehaviour_with_script_typetree(ext, 9)),
            (ext, external_monoscript_file(9, "S")),
        ],
        |base, target| {
            let (children, status) = diff_sections(base, target).unwrap();
            let mut labels = Vec::new();
            collect_changed_labels(&children, &mut labels);
            (status, labels.len())
        },
    );

    assert_eq!(status, NodeStatus::Unchanged);
    assert_eq!(changed, 0, "the MonoBehaviour renumber must collapse");
}

#[test]
fn diff_non_pptr_field_change_is_changed() {
    // Guard against over-collapse: a plain (non-PPtr) field difference
    // of equal length — so it slips past the byte-size shortcut and goes
    // through the deep compare — must still register as changed.
    let (status, changed) = with_diff_handles(
        &[(PATH, preload_with_dependency("aa"))],
        &[(PATH, preload_with_dependency("bb"))],
        |base, target| {
            let (children, status) = diff_sections(base, target).unwrap();
            let mut labels = Vec::new();
            collect_changed_labels(&children, &mut labels);
            (status, labels.len())
        },
    );

    assert_eq!(status, NodeStatus::Changed);
    assert_eq!(changed, 1);
}

/// First node with `label` anywhere in the forest.
fn find_node<'a>(
    nodes: &'a [crate::structured::Node],
    label: &str,
) -> Option<&'a crate::structured::Node> {
    for n in nodes {
        if n.label == label {
            return Some(n);
        }
        if let Some(found) = find_node(&n.children, label) {
            return Some(found);
        }
    }
    None
}

#[test]
fn diff_keeps_duplicate_same_script_components() {
    // A GameObject can carry several components of the same script
    // (e.g. three PlayMakerFSM). Each must get its own node — keying
    // components by script name alone silently collapses them onto one.
    let base_bytes = Scene::new()
        .with_root(
            SceneNode::new("RestBench")
                .with_script("", "PlayMakerFSM")
                .with_script("", "PlayMakerFSM")
                .with_script("", "PlayMakerFSM"),
        )
        .write();
    // Target lacks RestBench, so it diffs as a one-sided "added" subtree
    // — exercising the subtree_one_side / collect_components path.
    let target_bytes = Scene::new().with_root(SceneNode::new("Other")).write();

    let count = with_diff_handles(
        &[(PATH, base_bytes)],
        &[(PATH, target_bytes)],
        |base, target| {
            let (children, _status) = diff_sections(base, target).unwrap();
            let rest = find_node(&children, "RestBench").expect("RestBench node present");
            rest.children
                .iter()
                .filter(|c| c.kind == "component")
                .count()
        },
    );

    assert_eq!(
        count, 3,
        "all three PlayMakerFSM components must appear, not collapse to one"
    );
}

#[test]
fn diff_keeps_duplicate_components_on_matched_gameobject() {
    // A matched GameObject that dropped one of several same-script
    // components must reflect that as a removed/added component, not
    // vanish: keying by script name collapses base's 3 PlayMakerFSM and
    // target's 2 each to one, the pair looks unchanged, and the whole
    // GameObject is wrongly pruned as identical.
    let base_bytes = Scene::new()
        .with_root(
            SceneNode::new("RestBench")
                .with_script("", "PlayMakerFSM")
                .with_script("", "PlayMakerFSM")
                .with_script("", "PlayMakerFSM"),
        )
        .write();
    let target_bytes = Scene::new()
        .with_root(
            SceneNode::new("RestBench")
                .with_script("", "PlayMakerFSM")
                .with_script("", "PlayMakerFSM"),
        )
        .write();

    let desc = with_diff_handles(
        &[(PATH, base_bytes)],
        &[(PATH, target_bytes)],
        |base, target| {
            let (children, _status) = diff_sections(base, target).unwrap();
            match find_node(&children, "RestBench") {
                None => "RestBench absent (collapsed away)".to_string(),
                Some(rest) => rest
                    .children
                    .iter()
                    .filter(|c| c.kind == "component")
                    .map(|c| format!("{}:{:?}", c.label, c.status))
                    .collect::<Vec<_>>()
                    .join(", "),
            }
        },
    );
    // base has one more PlayMakerFSM than target → it shows up as Added.
    assert_eq!(desc, "PlayMakerFSM:Some(Added)");
}

#[test]
fn diff_auto_expands_the_single_changed_root() {
    // The scene has several roots but only one changed, so the diff
    // shows just that root — its single-child chain should open by
    // default even though the scene as a whole has many roots.
    let base = Scene::new()
        .with_root(SceneNode::new("Keep"))
        .with_root(
            SceneNode::new("Whole").with_child(SceneNode::new("Inner").with_script("", "Foo")),
        )
        .write();
    let target = Scene::new()
        .with_root(SceneNode::new("Keep"))
        .with_root(SceneNode::new("Whole").with_child(SceneNode::new("Inner")))
        .write();

    let (whole_collapsed, keep_present) = with_handle(PATH, base, |b| {
        with_handle(PATH, target, |t| {
            let (children, _status) = diff_sections(b, t).unwrap();
            let whole = find_node(&children, "Whole").expect("changed root present");
            (
                whole.default_collapsed,
                find_node(&children, "Keep").is_some(),
            )
        })
    });
    // Keep is unchanged → pruned, leaving Whole as the only visible root,
    // which therefore auto-expands.
    assert!(!keep_present, "unchanged root should be pruned");
    assert!(
        !whole_collapsed,
        "the single visible root should auto-expand"
    );
}

fn collect_changed_labels(nodes: &[crate::structured::Node], out: &mut Vec<String>) {
    for n in nodes {
        if n.kind == "component" && n.status == Some(NodeStatus::Changed) {
            out.push(n.label.clone());
        }
        collect_changed_labels(&n.children, out);
    }
}

#[test]
fn tree_with_monobehaviours() {
    // Two gameobjects, three MB attachments, two distinct scripts —
    // exercises the `MonoScript::full_name()` labeling and the script
    // de-dup in the registry (Player + Enemy share PlayerController).
    let bytes = Scene::new()
        .with_root(
            SceneNode::new("Player")
                .with_script("Game.Player", "PlayerController")
                .with_script("Game.Player", "Inventory"),
        )
        .with_root(SceneNode::new("Enemy").with_script("Game.Player", "PlayerController"))
        .write();
    let tree = with_handle(PATH, bytes, |handle| {
        let root = build_root_node(handle, PATH).unwrap();
        StructuredTree { root }
    });
    insta::assert_yaml_snapshot!(tree, @r#"
    root:
      id: "file:level0"
      label: level0
      kind: file
      badge: 9 objects
      children:
        - id: "section:class-stats"
          label: Class stats
          kind: section
          badge: 9 objects
          default_collapsed: true
          children:
            - id: "class:GameObject"
              label: GameObject
              kind: class-stat
              badge: ×2
              children: []
            - id: "class:Transform"
              label: Transform
              kind: class-stat
              badge: ×2
              children: []
            - id: "class:MonoBehaviour"
              label: MonoBehaviour
              kind: class-stat
              badge: ×3
              children: []
            - id: "class:MonoScript"
              label: MonoScript
              kind: class-stat
              badge: ×2
              children: []
        - id: "section:hierarchy"
          label: Hierarchy
          kind: section
          badge: 2 roots
          children:
            - id: "obj:1"
              label: Player
              kind: gameobject
              badge: 2 components
              default_collapsed: true
              include_descendants_on_match: true
              has_content: true
              children:
                - id: "obj:3"
                  label: Game.Player.PlayerController
                  kind: component
                  facets:
                    class: Game.Player.PlayerController
                  has_content: true
                  children: []
                - id: "obj:4"
                  label: Game.Player.Inventory
                  kind: component
                  facets:
                    class: Game.Player.Inventory
                  has_content: true
                  children: []
            - id: "obj:7"
              label: Enemy
              kind: gameobject
              badge: 1 component
              default_collapsed: true
              include_descendants_on_match: true
              has_content: true
              children:
                - id: "obj:9"
                  label: Game.Player.PlayerController
                  kind: component
                  facets:
                    class: Game.Player.PlayerController
                  has_content: true
                  children: []
        - id: "section:loose"
          label: Loose components
          kind: section
          badge: "2"
          children:
            - id: "obj:5"
              label: PlayerController
              kind: component
              badge: MonoScript
              facets:
                class: MonoScript
              has_content: true
              children: []
            - id: "obj:6"
              label: Inventory
              kind: component
              badge: MonoScript
              facets:
                class: MonoScript
              has_content: true
              children: []
    "#);
}

/// Regression test for the `pair_by_key` indexing bug. A parent
/// with three children that share a name (real-world example: Unity
/// scenes that instantiate the same prefab repeatedly under one
/// parent) is identical on both sides — the diff must therefore
/// prune the whole subtree to `Unchanged`. The previous
/// implementation kept a per-base nth-occurrence counter and called
/// `iter.nth(n)` on the *already-filtered* target list, so the
/// second occurrence on base resolved to the third unconsumed
/// target slot instead of the second. That left some pairs
/// matched-but-content-differs (path-ids swap), some base-only
/// (`added`), and some target-only (`removed`).
#[test]
fn diff_identical_same_named_siblings_prunes_clean() {
    let scene = || {
        Scene::new().with_root(
            SceneNode::new("Parent")
                .with_child(SceneNode::new("Floor"))
                .with_child(SceneNode::new("Floor"))
                .with_child(SceneNode::new("Floor")),
        )
    };
    let base_bytes = scene().write();
    let target_bytes = scene().write();
    let tree = with_handle(PATH, base_bytes, |base| {
        with_handle(PATH, target_bytes, |target| {
            let (children, status) = diff_sections(base, target).unwrap();
            StructuredTree {
                root: crate::structured::Node {
                    id: format!("file:{PATH}"),
                    label: PATH.to_string(),
                    kind: "file".to_string(),
                    status: Some(status),
                    children,
                    ..Default::default()
                },
            }
        })
    });
    insta::assert_yaml_snapshot!(tree, @r#"
    root:
      id: "file:level0"
      label: level0
      kind: file
      status: unchanged
      children: []
    "#);
}

#[test]
fn diff_identical_is_unchanged() {
    let bytes_a = small_scene().write();
    let bytes_b = small_scene().write();
    let tree = with_handle(PATH, bytes_a, |base| {
        with_handle(PATH, bytes_b, |target| {
            let (children, status) = diff_sections(base, target).unwrap();
            StructuredTree {
                root: crate::structured::Node {
                    id: format!("file:{PATH}"),
                    label: PATH.to_string(),
                    kind: "file".to_string(),
                    status: Some(status),
                    children,
                    ..Default::default()
                },
            }
        })
    });
    insta::assert_yaml_snapshot!(tree, @r#"
    root:
      id: "file:level0"
      label: level0
      kind: file
      status: unchanged
      children: []
    "#);
}

// -----------------------------------------------------------------------
// Value dump (dump_value.rs)
// -----------------------------------------------------------------------

use crate::unity::serializedfile::dump_value::dump_object_json_from_handle;
use crate::unity::serializedfile::format::{format_class_stats, format_hierarchy};

#[test]
fn dump_value_gameobject_with_components() {
    // GameObject + Transform + MonoBehaviour → exercises the
    // PPtr-marker rewrite (m_GameObject, m_Script, component refs).
    let bytes = Scene::new()
        .with_root(SceneNode::new("Player").with_script("Game.Player", "PlayerController"))
        .write();
    // Dump path id 1 (the first GameObject; matches the ordering in
    // tree_small_scene where the AssetBundle takes slot 1 — here we
    // have no AssetBundle so the GameObject lands at 1).
    let json = with_handle(PATH, bytes, |handle| {
        dump_object_json_from_handle(handle, "", "", 1, Default::default())
            .unwrap()
            .1
    });
    insta::assert_snapshot!(json, @r#"
    {
      "m_Component": [
        {
          "component": "__MARK__pptr␞obj:1␞Player␞Transform␞"
        },
        {
          "component": "__MARK__pptr␞obj:3␞Player␞Game.Player.PlayerController␞"
        }
      ],
      "m_IsActive": true,
      "m_Layer": 0,
      "m_Name": "Player",
      "m_Tag": 0
    }
    "#);
}

#[test]
fn dump_value_transform_with_pptrs() {
    // Transform's m_GameObject + m_Father PPtrs feed the qualify_pptr
    // path: local resolves to `__PPTR__` markers; null father stays
    // null.
    let bytes = Scene::new()
        .with_root(SceneNode::new("Parent").with_child(SceneNode::new("Child")))
        .write();
    // Path id 2 = root Transform (Parent: GO=1, T=2 ; Child: GO=3, T=4).
    let json = with_handle(PATH, bytes, |handle| {
        dump_object_json_from_handle(handle, "", "", 2, Default::default())
            .unwrap()
            .1
    });
    insta::assert_snapshot!(json, @r#"
    {
      "m_Children": [
        "__MARK__pptr␞obj:3␞Parent/Child␞Transform␞"
      ],
      "m_Father": null,
      "m_GameObject": "__MARK__pptr␞obj:1␞Parent␞GameObject␞",
      "m_LocalPosition": {
        "x": 0.0,
        "y": 0.0,
        "z": 0.0
      },
      "m_LocalRotation": {
        "w": 1.0,
        "x": 0.0,
        "y": 0.0,
        "z": 0.0
      },
      "m_LocalScale": {
        "x": 1.0,
        "y": 1.0,
        "z": 1.0
      }
    }
    "#);
}

#[test]
fn dump_value_assetbundle_singleton() {
    // AssetBundle sits at path id 1 when requested. Has a
    // BTreeMap<String, AssetInfo> container — exercises the map
    // recursion path with empty content.
    let bytes = Scene::new()
        .with_root(SceneNode::new("Player"))
        .with_asset_bundle("test_bundle")
        .write();
    let json = with_handle(PATH, bytes, |handle| {
        dump_object_json_from_handle(handle, "", "", 1, Default::default())
            .unwrap()
            .1
    });
    insta::assert_snapshot!(json, @r#"
    {
      "m_AssetBundleName": "",
      "m_Container": {},
      "m_Dependencies": [],
      "m_ExplicitDataLayout": 0,
      "m_IsStreamedSceneAssetBundle": false,
      "m_MainAsset": {
        "asset": null,
        "preloadIndex": 0,
        "preloadSize": 0
      },
      "m_Name": "test_bundle",
      "m_PathFlags": 7,
      "m_PreloadTable": [],
      "m_RuntimeCompatibility": 1,
      "m_SceneHashes": {}
    }
    "#);
}

// -----------------------------------------------------------------------
// Text format (format.rs)
// -----------------------------------------------------------------------

#[test]
fn format_text_dump() {
    // class-stats + hierarchy text rendering — covers both public
    // entry points and the recursive `format_node` walker.
    let bytes = Scene::new()
        .with_root(
            SceneNode::new("Player")
                .with_child(SceneNode::new("Camera"))
                .with_script("Game.Player", "PlayerController"),
        )
        .with_root(SceneNode::new("Light"))
        .write();
    let text = with_handle(PATH, bytes, |handle| {
        let mut out = String::new();
        format_class_stats(&mut out, handle);
        out.push('\n');
        format_hierarchy(&mut out, handle).unwrap();
        out
    });
    insta::assert_snapshot!(text, @"
    Class stats (8 objects):
           3  GameObject
           3  Transform
           1  MonoBehaviour
           1  MonoScript

    Player [1]
      - MonoBehaviour (3)
      Camera [4]
    Light [7]
    ");
}

#[test]
fn dump_value_unified_diff() {
    // Pure string helper — feed two JSON-looking blobs and check the
    // `--- target / +++ base` header + the 3-line context match the
    // wire format the diff-content route returns.
    let diff = crate::diff::unified_diff_text(
        "{\n  \"m_Name\": \"Player\",\n  \"m_Layer\": 0\n}",
        "{\n  \"m_Name\": \"Hero\",\n  \"m_Layer\": 0\n}",
        "base-label",
        "target-label",
    );
    insta::assert_snapshot!(diff, @r#"
    --- target-label
    +++ base-label
    @@ -1,4 +1,4 @@
     {
    -  "m_Name": "Hero",
    +  "m_Name": "Player",
       "m_Layer": 0
     }
    \ No newline at end of file
    "#);
}

#[test]
fn dump_value_rewrites_color_map_to_marker() {
    // Color-shaped map {r,g,b,a:F32} should rewrite to a single
    // `__MARK__color␞#rrggbbaa` string. Need any handle for the
    // signature; PPtr resolution doesn't fire on this input.
    use serde_value::Value;
    use std::collections::BTreeMap;

    let bytes = Scene::new().with_root(SceneNode::new("Player")).write();
    let out = with_handle(PATH, bytes, |handle| {
        let mut map = BTreeMap::new();
        map.insert(Value::String("r".into()), Value::F32(1.0));
        map.insert(Value::String("g".into()), Value::F32(0.5));
        map.insert(Value::String("b".into()), Value::F32(0.0));
        map.insert(Value::String("a".into()), Value::F32(1.0));
        let mut value = Value::Map(map);
        crate::unity::serializedfile::dump_value::simplify_for_dump(handle, "", "", &mut value);
        serde_json::to_string_pretty(&value).unwrap()
    });
    insta::assert_snapshot!(out, @r#""__MARK__color␞#ff8000ff""#);
}

#[test]
fn dump_value_flattens_non_string_keyed_map() {
    // A map with integer keys can't survive JSON serialization
    // verbatim — `simplify_for_dump` rewrites it into a sequence of
    // `{key, value}` pair objects.
    use serde_value::Value;
    use std::collections::BTreeMap;

    let bytes = Scene::new().with_root(SceneNode::new("Player")).write();
    let out = with_handle(PATH, bytes, |handle| {
        let mut map = BTreeMap::new();
        map.insert(Value::I32(7), Value::String("seven".into()));
        map.insert(Value::I32(42), Value::String("answer".into()));
        let mut value = Value::Map(map);
        crate::unity::serializedfile::dump_value::simplify_for_dump(handle, "", "", &mut value);
        serde_json::to_string_pretty(&value).unwrap()
    });
    insta::assert_snapshot!(out, @r#"
    [
      {
        "key": 7,
        "value": "seven"
      },
      {
        "key": 42,
        "value": "answer"
      }
    ]
    "#);
}

#[test]
fn dump_value_unit_key_becomes_null_pptr_sentinel() {
    // `Value::Unit` shows up as a null pptr resolved as a map key
    // (real-world example: a map<PPtr,…> with a null entry). JSON
    // forbids null keys, so the walker substitutes the all-empty
    // pptr-marker string instead.
    use serde_value::Value;
    use std::collections::BTreeMap;

    let bytes = Scene::new().with_root(SceneNode::new("Player")).write();
    let out = with_handle(PATH, bytes, |handle| {
        let mut map = BTreeMap::new();
        // Pre-resolved null pptr key → Value::Unit.
        map.insert(Value::Unit, Value::String("dangling".into()));
        let mut value = Value::Map(map);
        crate::unity::serializedfile::dump_value::simplify_for_dump(handle, "", "", &mut value);
        serde_json::to_string_pretty(&value).unwrap()
    });
    insta::assert_snapshot!(out, @r#"
    {
      "__MARK__pptr␞␞␞␞": "dangling"
    }
    "#);
}

#[test]
fn dump_value_color_via_lens_flare() {
    // Real `serde_typetree` → `serde_value` roundtrip for a LensFlare,
    // whose `m_Color` is the float-ColorRGBA shape. Validates the
    // hand-crafted color test above against an actual wire-format
    // pipeline.
    use super::fixtures::{ColorRgba, scene_with_lens_flare};
    let (bytes, path_id) = scene_with_lens_flare(ColorRgba {
        r: 1.0,
        g: 0.5,
        b: 0.0,
        a: 1.0,
    });
    let json = with_handle(PATH, bytes, |handle| {
        dump_object_json_from_handle(handle, "", "", path_id, Default::default())
            .unwrap()
            .1
    });
    insta::assert_snapshot!(json, @r#"
    {
      "m_Brightness": 1.0,
      "m_Color": "__MARK__color␞#ff8000ff",
      "m_Directional": false,
      "m_Enabled": 1,
      "m_FadeSpeed": 3.0,
      "m_Flare": null,
      "m_GameObject": null,
      "m_IgnoreLayers": {
        "m_Bits": 0
      }
    }
    "#);
}

#[test]
fn dump_value_custom_mb_with_color_and_map() {
    // Real wire-format end-to-end: a MonoBehaviour whose typetree we
    // hand-extend with a `ColorRGBA m_TintColor` and a
    // `map<int,int> m_Lookup`. Serialized, parsed back through rabex,
    // dumped — both the color marker and the non-string-key flatten
    // fire on real data instead of a hand-built `Value::Map`.
    use std::collections::BTreeMap;

    use super::fixtures::{ColorRgba, CustomMbBody, scene_with_custom_mb};
    use rabex_env::rabex::objects::TypedPPtr;

    let mut lookup = BTreeMap::new();
    lookup.insert(1, 100);
    lookup.insert(2, 200);
    let body = CustomMbBody {
        m_GameObject: TypedPPtr::null(),
        m_Enabled: 1,
        // Overwritten by the fixture with the actual MonoScript pptr.
        m_Script: TypedPPtr::null(),
        m_Name: "demo".to_owned(),
        m_TintColor: ColorRgba {
            r: 0.0,
            g: 1.0,
            b: 0.5,
            a: 1.0,
        },
        m_Lookup: lookup,
    };
    let (bytes, path_id) = scene_with_custom_mb(body);
    let json = with_handle(PATH, bytes, |handle| {
        dump_object_json_from_handle(handle, "", "", path_id, Default::default())
            .unwrap()
            .1
    });
    insta::assert_snapshot!(json, @r#"
    {
      "m_Enabled": 1,
      "m_GameObject": null,
      "m_Lookup": [
        {
          "key": 1,
          "value": 100
        },
        {
          "key": 2,
          "value": 200
        }
      ],
      "m_Name": "demo",
      "m_Script": "__MARK__pptr␞obj:1␞CustomBehaviour␞MonoScript␞",
      "m_TintColor": "__MARK__color␞#00ff80ff"
    }
    "#);
}

#[test]
fn dump_value_monoscript_classname_links_into_managed_dll() {
    // A MonoScript's m_ClassName is rewritten into a `classref` marker
    // pointing at the decompiled `<DataDir>/Managed/<Assembly>.dll`,
    // jumping to the `type:<FQN>` node. The fixture's MonoScript has an
    // empty namespace, so the FQN is the bare class name.
    let bytes = external_monoscript_file(7, "SceneManager");
    let json = with_handle(PATH, bytes, |handle| {
        dump_object_json_from_handle(handle, "hollow_knight_Data", "", 7, Default::default())
            .unwrap()
            .1
    });
    insta::assert_snapshot!(json, @r#"
    {
      "m_AssemblyName": "Assembly-CSharp.dll",
      "m_ClassName": "__MARK__classref␞type:SceneManager␞SceneManager␞hollow_knight_Data/Managed/Assembly-CSharp.dll",
      "m_ExecutionOrder": 0,
      "m_Name": "SceneManager",
      "m_Namespace": "",
      "m_PropertiesHash": {
        "bytes[0]": 0,
        "bytes[10]": 0,
        "bytes[11]": 0,
        "bytes[12]": 0,
        "bytes[13]": 0,
        "bytes[14]": 0,
        "bytes[15]": 0,
        "bytes[1]": 0,
        "bytes[2]": 0,
        "bytes[3]": 0,
        "bytes[4]": 0,
        "bytes[5]": 0,
        "bytes[6]": 0,
        "bytes[7]": 0,
        "bytes[8]": 0,
        "bytes[9]": 0
      }
    }
    "#);
}
