// TODO(ai-review): review for style and correctness
//! insta snapshots for the unity structured-tree and diff builders.
//!
//! Each test assembles a [`super::fixtures::Scene`] in memory, runs
//! the production tree / diff builders against it, and yaml-snapshots
//! the resulting [`StructuredTree`]. Snapshots live next to this file
//! under `snapshots/`.

use super::fixtures::{BundleBuilder, Scene, SceneNode, with_bundle, with_handle};
use crate::structured::StructuredTree;
use crate::unity::bundle::{build_diff_from_bundles, build_tree_from_bundle};
use crate::unity::diff::diff_sections;
use crate::unity::tree::{TREE_KIND, build_root_node};

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
        StructuredTree {
            kind: TREE_KIND.to_string(),
            root,
        }
    });
    insta::assert_yaml_snapshot!(tree, @r#"
    kind: unity-serialized
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
              include_descendants_on_match: true
              has_content: true
              children:
                - id: "obj:4"
                  label: Camera
                  kind: gameobject
                  include_descendants_on_match: true
                  has_content: true
                  children: []
                - id: "obj:6"
                  label: Weapon
                  kind: gameobject
                  include_descendants_on_match: true
                  has_content: true
                  children: []
            - id: "obj:8"
              label: Light
              kind: gameobject
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
              badge: "[1]"
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
                kind: TREE_KIND.to_string(),
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
    kind: unity-serialized
    root:
      id: "file:level0"
      label: level0
      kind: file
      status: changed
      children:
        - id: "section:class-stats"
          label: Class stats
          kind: section
          badge: 9 objects
          default_collapsed: true
          status: unchanged
          children: []
        - id: "section:hierarchy"
          label: Hierarchy
          kind: section
          badge: 2 roots
          status: changed
          children:
            - id: "obj:2"
              label: Player
              kind: gameobject
              status: changed
              has_content: true
              children:
                - id: "base:obj:4"
                  label: Camera
                  kind: gameobject
                  status: added
                  has_content: true
                  children: []
                - id: "base:obj:6"
                  label: Weapon
                  kind: gameobject
                  status: added
                  has_content: true
                  children: []
                - id: "target:obj:4"
                  label: Sword
                  kind: gameobject
                  status: removed
                  has_content: true
                  children: []
                - id: "target:obj:6"
                  label: Hat
                  kind: gameobject
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
        StructuredTree {
            kind: TREE_KIND.to_string(),
            root,
        }
    });
    insta::assert_yaml_snapshot!(tree, @r#"
    kind: unity-serialized
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
              badge: "[5]"
              facets:
                class: MonoScript
              has_content: true
              children: []
            - id: "obj:6"
              label: Inventory
              kind: component
              badge: "[6]"
              facets:
                class: MonoScript
              has_content: true
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
                kind: TREE_KIND.to_string(),
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
    kind: unity-serialized
    root:
      id: "file:level0"
      label: level0
      kind: file
      status: unchanged
      children:
        - id: "section:class-stats"
          label: Class stats
          kind: section
          badge: 9 objects
          default_collapsed: true
          status: unchanged
          children: []
        - id: "section:hierarchy"
          label: Hierarchy
          kind: section
          badge: 2 roots
          status: unchanged
          children: []
        - id: "section:loose"
          label: Loose components
          kind: section
          badge: 1 object
          status: unchanged
          children: []
    "#);
}

// -----------------------------------------------------------------------
// Bundle tests
// -----------------------------------------------------------------------

const BUNDLE_PATH: &str = "level0.bundle";

#[test]
fn bundle_tree_one_sf_one_blob() {
    // One SerializedFile entry + one raw blob — exercises the two
    // bundle-dispatch branches in `build_tree_from_bundle` and the
    // human_bytes badge on the blob row.
    let scene_bytes = Scene::new().with_root(SceneNode::new("Player")).write();
    let bundle = BundleBuilder::new()
        .add_serialized("CAB-scene", scene_bytes)
        .add_blob("CAB-scene.resS", vec![0u8; 4096])
        .write();
    let tree = with_bundle(bundle, |env, bundle| {
        build_tree_from_bundle(env, bundle, BUNDLE_PATH).unwrap()
    });
    insta::assert_yaml_snapshot!(tree, @r#"
    kind: unity-serialized
    root:
      id: "file:level0.bundle"
      label: level0.bundle
      kind: bundle
      badge: 2 entries
      children:
        - id: "archive:CAB-scene"
          label: CAB-scene
          kind: archive
          badge: 2 objects
          children:
            - id: "archive:CAB-scene/section:class-stats"
              label: Class stats
              kind: section
              badge: 2 objects
              default_collapsed: true
              children:
                - id: "archive:CAB-scene/class:GameObject"
                  label: GameObject
                  kind: class-stat
                  badge: ×1
                  children: []
                - id: "archive:CAB-scene/class:Transform"
                  label: Transform
                  kind: class-stat
                  badge: ×1
                  children: []
            - id: "archive:CAB-scene/section:hierarchy"
              label: Hierarchy
              kind: section
              badge: 1 root
              children:
                - id: "archive:CAB-scene/obj:1"
                  label: Player
                  kind: gameobject
                  include_descendants_on_match: true
                  has_content: true
                  children: []
            - id: "archive:CAB-scene/section:loose"
              label: Loose components
              kind: section
              badge: "0"
              children: []
        - id: "blob:CAB-scene.resS"
          label: CAB-scene.resS
          kind: blob
          badge: 4.0 KiB
          children: []
    "#);
}

#[test]
fn bundle_diff_added_changed_unchanged() {
    let base_scene = Scene::new().with_root(SceneNode::new("Player")).write();
    // target: same scene path but Player renamed → SF differs.
    // Also rename one blob (gone from target → Added on base side),
    // and a third blob with identical size on both sides (Unchanged,
    // pruned).
    let target_scene = Scene::new().with_root(SceneNode::new("PlayerV2")).write();
    let base_bundle = BundleBuilder::new()
        .add_serialized("CAB-scene", base_scene)
        .add_blob("CAB-scene.resS", vec![0u8; 4096])
        .add_blob("CAB-scene.shared", vec![0u8; 256])
        .write();
    let target_bundle = BundleBuilder::new()
        .add_serialized("CAB-scene", target_scene)
        .add_blob("CAB-scene.shared", vec![0u8; 256])
        .write();

    let tree = with_bundle(base_bundle, |base_env, base| {
        with_bundle(target_bundle, |target_env, target| {
            build_diff_from_bundles(base_env, base, target_env, target, BUNDLE_PATH).unwrap()
        })
    });
    insta::assert_yaml_snapshot!(tree, @r#"
    kind: unity-serialized
    root:
      id: "file:level0.bundle"
      label: level0.bundle
      kind: bundle
      badge: 3 entries
      status: changed
      children:
        - id: "archive:CAB-scene"
          label: CAB-scene
          kind: archive
          status: changed
          children:
            - id: "archive:CAB-scene/section:class-stats"
              label: Class stats
              kind: section
              badge: 2 objects
              default_collapsed: true
              status: unchanged
              children: []
            - id: "archive:CAB-scene/section:hierarchy"
              label: Hierarchy
              kind: section
              badge: 1 root
              status: changed
              children:
                - id: "archive:CAB-scene/base:obj:1"
                  label: Player
                  kind: gameobject
                  status: added
                  has_content: true
                  children: []
                - id: "archive:CAB-scene/target:obj:1"
                  label: PlayerV2
                  kind: gameobject
                  status: removed
                  has_content: true
                  children: []
            - id: "archive:CAB-scene/section:loose"
              label: Loose components
              kind: section
              badge: 0 objects
              status: unchanged
              children: []
        - id: "blob:CAB-scene.resS"
          label: CAB-scene.resS
          kind: blob
          badge: 4.0 KiB
          status: added
          children: []
    "#);
}
