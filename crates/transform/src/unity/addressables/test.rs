// TODO(ai-review): review for style and correctness
//! insta snapshots for the addressables tree/diff builders, over
//! synthetic catalogs — no binary catalog fixture needed because the
//! builders work off the parsed `AddressablesCatalog` struct.

use std::collections::HashMap;
use std::sync::Arc;

use rabex_env::addressables::catalog::{
    AddressablesCatalog, AssemblyClass, AssetBundleRequestOptions, CommonInfo, Hash128,
    ObjectInitializationData, ResourceLocation,
};

const AB_PROVIDER: &str = "UnityEngine.ResourceManagement.ResourceProviders.AssetBundleProvider";
const BA_PROVIDER: &str = "UnityEngine.ResourceManagement.ResourceProviders.BundledAssetProvider";

fn arc(s: &str) -> Arc<String> {
    Arc::from(s.to_owned())
}

fn class(name: &str) -> AssemblyClass {
    AssemblyClass {
        m_AssemblyName: arc("Assembly-CSharp"),
        m_ClassName: arc(name),
    }
}

fn abro(bundle_name: &str, size: u32) -> AssetBundleRequestOptions {
    AssetBundleRequestOptions {
        hash: Hash128::from_u32s([0, 0, 0, 0]),
        crc: 0,
        common_info: CommonInfo {
            timeout: 0,
            redirect_limit: 0,
            retry_count: 0,
            flags: 0,
        },
        bundle_name: arc(bundle_name),
        bundle_size: size,
    }
}

/// An AssetBundleProvider location for `StandaloneWindows64/<group>.bundle`;
/// its ABRO carries the stable bundle name, its primary key the file
/// name including the content hash.
fn bundle_location(group: &str, hash: &str, name: &str, size: u32) -> Arc<ResourceLocation> {
    Arc::new(ResourceLocation {
        internal_id: arc(&format!(
            "{{UnityEngine.AddressableAssets.Addressables.RuntimePath}}\\StandaloneWindows64\\{group}.bundle"
        )),
        provider_id: arc(AB_PROVIDER),
        dependencies: Vec::new(),
        data: Some(abro(name, size)),
        dependency_hash_code: 0,
        primary_key: arc(&format!("{group}_{hash}.bundle")),
        type_: class("IAssetBundleResource"),
    })
}

/// A BundledAssetProvider location resolving into `bundle`.
fn asset_location(
    type_name: &str,
    internal_id: &str,
    bundle: &Arc<ResourceLocation>,
) -> Arc<ResourceLocation> {
    Arc::new(ResourceLocation {
        internal_id: arc(internal_id),
        provider_id: arc(BA_PROVIDER),
        dependencies: vec![Arc::clone(bundle)],
        data: None,
        dependency_hash_code: 0,
        primary_key: arc(internal_id),
        type_: class(type_name),
    })
}

fn catalog(resources: Vec<(String, Vec<Arc<ResourceLocation>>)>) -> AddressablesCatalog {
    AddressablesCatalog {
        locator_id: arc("test"),
        build_result_hash: arc("hash"),
        instance_provider_data: ObjectInitializationData {
            id: arc(""),
            object_type: class("x"),
            data: arc(""),
        },
        scene_provider_data: ObjectInitializationData {
            id: arc(""),
            object_type: class("x"),
            data: arc(""),
        },
        resource_provider_data: Vec::new(),
        resources: resources
            .into_iter()
            .map(|(k, v)| (arc(k.as_str()), v))
            .collect::<HashMap<_, _>>(),
    }
}

#[test]
fn tree_keys() {
    let scenes_bundle = bundle_location(
        "scenes_scenes_scenes/bone_03",
        "2be02a4c3c388aa804c5e27c1c61b803",
        "a874959409ad38db2ca2f6d6e2e6b7c9",
        606_928,
    );
    let atlas_bundle = bundle_location(
        "atlases_assets",
        "925f32bd8c05421a3ccb60c371036f72",
        "94b2ee223104f6444fdf139f7d9ffc43",
        4_096,
    );
    let cat = catalog(vec![
        (
            "scenes_scenes_scenes/bone_03_2be02a4c3c388aa804c5e27c1c61b803.bundle".to_string(),
            vec![Arc::clone(&scenes_bundle)],
        ),
        (
            "atlases_assets_925f32bd8c05421a3ccb60c371036f72.bundle".to_string(),
            vec![Arc::clone(&atlas_bundle)],
        ),
        (
            "Scenes/Bone_03".to_string(),
            vec![asset_location(
                "SceneInstance",
                "Assets/Scenes/Bone_03.unity",
                &scenes_bundle,
            )],
        ),
        (
            "Scenes/Hornet/Bone_03".to_string(),
            vec![asset_location(
                "SceneInstance",
                "Assets/Scenes/Hornet/Bone_03.unity",
                &scenes_bundle,
            )],
        ),
        (
            "AreaBone".to_string(),
            vec![
                asset_location(
                    "SceneInstance",
                    "Assets/Scenes/Bone_03.unity",
                    &scenes_bundle,
                ),
                asset_location("TextAsset", "Assets/Text/bone.json", &atlas_bundle),
            ],
        ),
    ]);

    let view = crate::unity::addressables::view(&cat);
    let tree = crate::structured::StructuredTree {
        root: crate::unity::addressables::tree::build_root(&view, "catalog.bin"),
    };
    insta::assert_yaml_snapshot!(tree);
}

#[test]
fn diff_rebuilt_bundle_pairs_instead_of_add_remove() {
    // The same bundle (stable name) rebuilt with a different content
    // hash: bundle row + self-key row both `changed`, the self-key
    // `mod:`-paired across the differing file names. The scene key is
    // unchanged — its dependency keeps the stable name.
    let base_bundle = bundle_location(
        "scenes_scenes_scenes/bone_03",
        "2be02a4c3c388aa804c5e27c1c61b803",
        "a874959409ad38db2ca2f6d6e2e6b7c9",
        100,
    );
    let target_bundle = bundle_location(
        "scenes_scenes_scenes/bone_03",
        "99999999999999999999999999999999",
        "a874959409ad38db2ca2f6d6e2e6b7c9",
        200,
    );
    let base = catalog(vec![
        (
            "scenes_scenes_scenes/bone_03_2be02a4c3c388aa804c5e27c1c61b803.bundle".to_string(),
            vec![Arc::clone(&base_bundle)],
        ),
        (
            "Scenes/Bone_03".to_string(),
            vec![asset_location(
                "SceneInstance",
                "Assets/Scenes/Bone_03.unity",
                &base_bundle,
            )],
        ),
    ]);
    let target = catalog(vec![
        (
            "scenes_scenes_scenes/bone_03_99999999999999999999999999999999.bundle".to_string(),
            vec![Arc::clone(&target_bundle)],
        ),
        (
            "Scenes/Bone_03".to_string(),
            vec![asset_location(
                "SceneInstance",
                "Assets/Scenes/Bone_03.unity",
                &target_bundle,
            )],
        ),
    ]);

    let base = crate::unity::addressables::view(&base);
    let target = crate::unity::addressables::view(&target);
    let tree =
        crate::unity::addressables::diff::build_diff_from_views(&base, &target, "catalog.bin");
    insta::assert_yaml_snapshot!(tree);
}

#[test]
fn diff_added_and_removed_keys() {
    // A scene key only in base (Added), a label only in target
    // (Removed), and one whose dependency bundle moved (changed).
    let base_bundle = bundle_location("a", "00000000000000000000000000000001", "aaa", 10);
    let other_bundle = bundle_location("b", "00000000000000000000000000000002", "bbb", 20);
    let base = catalog(vec![
        (
            "a_00000000000000000000000000000001.bundle".to_string(),
            vec![Arc::clone(&base_bundle)],
        ),
        (
            "b_00000000000000000000000000000002.bundle".to_string(),
            vec![Arc::clone(&other_bundle)],
        ),
        (
            "Scenes/OnlyBase".to_string(),
            vec![asset_location(
                "SceneInstance",
                "Assets/Scenes/OnlyBase.unity",
                &base_bundle,
            )],
        ),
        (
            "Scenes/Moved".to_string(),
            vec![asset_location(
                "SceneInstance",
                "Assets/Scenes/Moved.unity",
                &base_bundle,
            )],
        ),
    ]);
    let target_bundle = bundle_location("a", "00000000000000000000000000000009", "aaa", 10);
    let target = catalog(vec![
        (
            "a_00000000000000000000000000000009.bundle".to_string(),
            vec![Arc::clone(&target_bundle)],
        ),
        (
            "b_00000000000000000000000000000002.bundle".to_string(),
            vec![Arc::clone(&other_bundle)],
        ),
        (
            "Scenes/OnlyTarget".to_string(),
            vec![asset_location(
                "SceneInstance",
                "Assets/Scenes/OnlyTarget.unity",
                &target_bundle,
            )],
        ),
        (
            "Scenes/Moved".to_string(),
            vec![asset_location(
                "SceneInstance",
                "Assets/Scenes/Moved.unity",
                &other_bundle,
            )],
        ),
    ]);

    let base = crate::unity::addressables::view(&base);
    let target = crate::unity::addressables::view(&target);
    let tree =
        crate::unity::addressables::diff::build_diff_from_views(&base, &target, "catalog.bin");
    insta::assert_yaml_snapshot!(tree);
}
