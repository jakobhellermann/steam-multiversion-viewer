// TODO(ai-review): review for style and correctness
//! insta snapshots for the bundle tree / diff builders. Each test
//! assembles a [`super::fixtures::BundleBuilder`] from `Scene` bytes
//! (built via the SF test fixtures), runs `build_*_from_bundle`, and
//! yaml-snapshots the result.

use super::fixtures::{BundleBuilder, with_bundle};
use crate::unity::bundle::diff::build_diff_from_bundles;
use crate::unity::bundle::tree::build_tree_from_bundle;
use crate::unity::serializedfile::test::fixtures::{Scene, SceneNode};

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
    // target: same SF path but Player renamed → SF differs.
    // The resS blob is only on base (Added); the shared blob has
    // identical size on both sides (Unchanged, pruned from output).
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
