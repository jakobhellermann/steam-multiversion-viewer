use anyhow::Result;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::objects::ClassId;
use rabex_env::rabex::objects::pptr::PathId;
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;
use rabex_env::unity::types::MonoBehaviour;

use crate::structured::Node;
use crate::unity::NameOnly;

/// Dispatches to the node builder for this engine class.
#[tracing::instrument(level = "debug", skip_all, fields(?class_id, ?path_id))]
pub(super) fn component_node<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    path_id: PathId,
    class_id: ClassId,
    loose: bool,
) -> Result<Node> {
    match class_id {
        ClassId::MonoBehaviour => monobehaviour_node(file, path_id),
        ClassId::Shader => shader_node(file, path_id),
        _ => ordinary_component_node(file, path_id, class_id, loose),
    }
}

fn ordinary_component_node<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    path_id: PathId,
    class_id: ClassId,
    loose: bool,
) -> Result<Node> {
    let class_label = format!("{class_id:?}");
    let display_label = if loose {
        read_m_name(file, path_id)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| class_label.clone())
    } else {
        class_label.clone()
    };
    let mut node = component_leaf(path_id, &display_label, &class_label);
    if class_id == ClassId::Texture2D {
        node.content_mime = Some("image/png".to_string());
    }
    if loose && display_label != class_label {
        node = node.with_badge(class_label);
    }
    Ok(node)
}

/// Entry point for script-specific MonoBehaviour nodes.
fn monobehaviour_node<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    path_id: PathId,
) -> Result<Node> {
    let handle = file.object_at::<MonoBehaviour>(path_id)?;
    let class_label = handle
        .mono_script()?
        .map(|script| script.full_name().into_owned())
        .unwrap_or_else(|| format!("{:?}", ClassId::MonoBehaviour));
    Ok(component_leaf(path_id, &class_label, &class_label))
}

fn shader_node<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    path_id: PathId,
) -> Result<Node> {
    let class_label = format!("{:?}", ClassId::Shader);
    let mut node = component_leaf(path_id, &class_label, &class_label);
    if let Ok(value) = file
        .object_at::<serde_value::Value>(path_id)
        .and_then(|h| h.read())
    {
        if let Some(name) = super::super::shader::parsed_form_name(&value) {
            node.label = name;
            node.badge = Some(class_label);
        }
        node.children =
            super::shader::shader_nodes(path_id, super::super::shader::program_groups(&value));
        node.default_collapsed = true;
        node.include_descendants_on_match = true;
    }
    Ok(node)
}

fn component_leaf(path_id: PathId, display_label: &str, class_label: &str) -> Node {
    let mut node = Node::leaf(format!("obj:{path_id}"), display_label, "component")
        .with_facet("class", class_label);
    node.has_content = true;
    node
}

fn read_m_name<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    path_id: PathId,
) -> Option<String> {
    file.object_at::<NameOnly>(path_id)
        .ok()?
        .read()
        .ok()
        .map(|n| n.m_Name)
}
