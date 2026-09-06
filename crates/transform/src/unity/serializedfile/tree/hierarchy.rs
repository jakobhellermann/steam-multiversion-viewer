use std::collections::{BTreeMap, HashSet};

use anyhow::Result;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::objects::ClassId;
use rabex_env::rabex::objects::pptr::PathId;
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;
use rabex_env::unity::types::{GameObject, Transform};

use crate::structured::Node;

pub(super) struct Section {
    pub(super) node: Node,
    pub(super) covered: HashSet<PathId>,
}

#[tracing::instrument(skip_all)]
pub(super) fn build_section<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
) -> Result<Section> {
    let mut transforms = BTreeMap::new();
    for handle in file.transforms() {
        let path_id = handle.path_id();
        let transform = handle.read()?;
        let gameobject = file.deref(transform.m_GameObject)?.read()?;
        transforms.insert(path_id, (transform, gameobject));
    }

    let mut covered = HashSet::new();
    let mut roots = Vec::new();
    for (path_id, (transform, gameobject)) in &transforms {
        if transform.m_Father.is_null() {
            roots.push(build_gameobject_node(
                file,
                &transforms,
                &mut covered,
                *path_id,
                transform,
                gameobject,
            )?);
        }
    }
    if let [only] = roots.as_mut_slice() {
        super::super::expand_single_child_chains(only);
    }
    let count = roots.len();
    Ok(Section {
        node: Node {
            id: "section:hierarchy".to_string(),
            label: "Hierarchy".to_string(),
            kind: "section".to_string(),
            badge: Some(format!("{count} {}", super::pluralize(count, "root"))),
            default_collapsed: false,
            children: roots,
            ..Default::default()
        },
        covered,
    })
}

fn build_gameobject_node<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    transforms: &BTreeMap<PathId, (Transform, GameObject)>,
    covered: &mut HashSet<PathId>,
    transform_path_id: PathId,
    transform: &Transform,
    gameobject: &GameObject,
) -> Result<Node> {
    let gameobject_path_id = transform.m_GameObject.m_PathID;
    covered.insert(gameobject_path_id);
    covered.insert(transform_path_id);

    let mut children = Vec::new();
    for component in &gameobject.m_Component {
        let component = component
            .component
            .deref_local::<()>(file.file, &file.env.tpk)?;
        let class_id = component.info.m_ClassID;
        let path_id = component.info.m_PathID;
        covered.insert(path_id);
        if path_id == transform_path_id
            && matches!(class_id, ClassId::Transform | ClassId::RectTransform)
        {
            continue;
        }
        children.push(super::components::component_node(
            file, path_id, class_id, false,
        )?);
    }
    for child in &transform.m_Children {
        if let Some((child_transform, child_gameobject)) = transforms.get(&child.m_PathID) {
            children.push(build_gameobject_node(
                file,
                transforms,
                covered,
                child.m_PathID,
                child_transform,
                child_gameobject,
            )?);
        }
    }

    let component_count = gameobject.m_Component.len().saturating_sub(1);
    Ok(Node {
        id: format!("obj:{gameobject_path_id}"),
        label: if gameobject.m_Name.is_empty() {
            "(unnamed)".to_string()
        } else {
            gameobject.m_Name.clone()
        },
        kind: "gameobject".to_string(),
        badge: (component_count > 0).then(|| {
            format!(
                "{component_count} {}",
                super::pluralize(component_count, "component")
            )
        }),
        default_collapsed: true,
        include_descendants_on_match: true,
        has_content: true,
        children,
        ..Default::default()
    })
}
