// TODO(ai-review): review for style and correctness
//! PE export/import tree for a native `.exe`/`.dll`, like `nm -D` for `.so`.

use std::collections::BTreeMap;

use object::{Object, ObjectSection};

use crate::TransformError;
use crate::structured::{Node, NodeStatus, StructuredTree, human_bytes};

pub fn build_tree(bytes: &[u8], file_label: &str) -> Result<StructuredTree, TransformError> {
    let file = object::File::parse(bytes)
        .map_err(|e| TransformError::Other(format!("failed to parse PE: {e}")))?;

    let mut children = Vec::new();
    if let Some(node) = sections_node(&file) {
        children.push(node);
    }
    if let Some(node) = exports_node(&file) {
        children.push(node);
    }
    if let Some(node) = imports_node(&file) {
        children.push(node);
    }

    Ok(StructuredTree {
        root: Node {
            id: format!("file:{file_label}"),
            label: file_label.to_string(),
            children,
            ..Node::default()
        },
    })
}

fn sections_node(file: &object::File) -> Option<Node> {
    let children = file
        .sections()
        .map(|s| Node {
            id: format!("section:{}", s.name().unwrap_or("?")),
            label: s.name().unwrap_or("?").to_string(),
            badge: Some(human_bytes(s.size())),
            ..Node::default()
        })
        .collect::<Vec<_>>();
    if children.is_empty() {
        return None;
    }
    Some(Node {
        id: "sections".to_string(),
        label: "Sections".to_string(),
        badge: Some(children.len().to_string()),
        children,
        ..Node::default()
    })
}

fn exports_node(file: &object::File) -> Option<Node> {
    let mut names: Vec<String> = file
        .exports()
        .ok()?
        .into_iter()
        .map(|e| String::from_utf8_lossy(e.name()).into_owned())
        .collect();
    if names.is_empty() {
        return None;
    }
    names.sort();
    let children = names
        .into_iter()
        .map(|name| Node {
            id: format!("export:{name}"),
            label: name,
            ..Node::default()
        })
        .collect::<Vec<_>>();
    Some(Node {
        id: "exports".to_string(),
        label: "Exports".to_string(),
        badge: Some(children.len().to_string()),
        children,
        ..Node::default()
    })
}

fn imports_node(file: &object::File) -> Option<Node> {
    let imports = file.imports().ok()?;
    if imports.is_empty() {
        return None;
    }
    let mut by_library: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for import in imports {
        by_library
            .entry(String::from_utf8_lossy(import.library()).into_owned())
            .or_default()
            .push(String::from_utf8_lossy(import.name()).into_owned());
    }
    let children = by_library
        .into_iter()
        .map(|(library, mut names)| {
            names.sort();
            let children = names
                .into_iter()
                .map(|name| Node {
                    id: format!("import:{library}:{name}"),
                    label: name,
                    ..Node::default()
                })
                .collect::<Vec<_>>();
            Node {
                id: format!("importlib:{library}"),
                badge: Some(children.len().to_string()),
                label: library,
                default_collapsed: true,
                children,
                ..Node::default()
            }
        })
        .collect::<Vec<_>>();
    Some(Node {
        id: "imports".to_string(),
        label: "Imports".to_string(),
        badge: Some(children.len().to_string()),
        children,
        ..Node::default()
    })
}

/// GNU-`diff` sense: only in `to` is Added, only in `from` is Removed.
pub fn build_diff_tree(
    from_bytes: &[u8],
    to_bytes: &[u8],
    file_label: &str,
) -> Result<StructuredTree, TransformError> {
    let from = object::File::parse(from_bytes)
        .map_err(|e| TransformError::Other(format!("failed to parse PE: {e}")))?;
    let to = object::File::parse(to_bytes)
        .map_err(|e| TransformError::Other(format!("failed to parse PE: {e}")))?;

    let mut children = Vec::new();
    if let Some(node) = diff_sections_node(&from, &to) {
        children.push(node);
    }
    if let Some(node) = diff_exports_node(&from, &to) {
        children.push(node);
    }
    if let Some(node) = diff_imports_node(&from, &to) {
        children.push(node);
    }
    let status = aggregate_status(&children);

    Ok(StructuredTree {
        root: Node {
            id: format!("file:{file_label}"),
            label: file_label.to_string(),
            status: Some(status),
            children,
            ..Node::default()
        },
    })
}

fn diff_sections_node<'d>(from: &object::File<'d>, to: &object::File<'d>) -> Option<Node> {
    // Same-size sections can still differ (e.g. `.rsrc` version info),
    // so Changed is decided on actual bytes, not just size.
    let mut from_map: BTreeMap<String, (u64, &'d [u8])> = from
        .sections()
        .map(|s| {
            let name = s.name().unwrap_or("?").to_string();
            (name, (s.size(), s.data().unwrap_or(&[])))
        })
        .collect();
    let mut children: Vec<Node> = Vec::new();
    for s in to.sections() {
        let name = s.name().unwrap_or("?").to_string();
        let to_size = s.size();
        let to_data = s.data().unwrap_or(&[]);
        let node = match from_map.remove(&name) {
            Some((_, from_data)) if from_data == to_data => Node {
                status: Some(NodeStatus::Unchanged),
                badge: Some(human_bytes(to_size)),
                ..section_leaf(&name)
            },
            Some((from_size, _)) => Node {
                status: Some(NodeStatus::Changed),
                badge: Some(if from_size == to_size {
                    human_bytes(to_size)
                } else {
                    format!("{} → {}", human_bytes(from_size), human_bytes(to_size))
                }),
                ..section_leaf(&name)
            },
            None => Node {
                status: Some(NodeStatus::Added),
                badge: Some(human_bytes(to_size)),
                ..section_leaf(&name)
            },
        };
        children.push(node);
    }
    for (name, (from_size, _)) in from_map {
        children.push(Node {
            status: Some(NodeStatus::Removed),
            badge: Some(human_bytes(from_size)),
            ..section_leaf(&name)
        });
    }
    diff_group_node("sections", "Sections", children)
}

fn section_leaf(name: &str) -> Node {
    Node {
        id: format!("section:{name}"),
        label: name.to_string(),
        ..Node::default()
    }
}

fn diff_exports_node(from: &object::File, to: &object::File) -> Option<Node> {
    let from_names: std::collections::BTreeSet<String> = from
        .exports()
        .unwrap_or_default()
        .into_iter()
        .map(|e| String::from_utf8_lossy(e.name()).into_owned())
        .collect();
    let to_names: std::collections::BTreeSet<String> = to
        .exports()
        .unwrap_or_default()
        .into_iter()
        .map(|e| String::from_utf8_lossy(e.name()).into_owned())
        .collect();
    let children = diff_name_set(&from_names, &to_names, "export");
    diff_group_node("exports", "Exports", children)
}

fn diff_imports_node(from: &object::File, to: &object::File) -> Option<Node> {
    let group = |file: &object::File| -> BTreeMap<String, std::collections::BTreeSet<String>> {
        let mut by_library: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
        for import in file.imports().unwrap_or_default() {
            by_library
                .entry(String::from_utf8_lossy(import.library()).into_owned())
                .or_default()
                .insert(String::from_utf8_lossy(import.name()).into_owned());
        }
        by_library
    };
    let mut from_libs = group(from);
    let to_libs = group(to);

    let mut children: Vec<Node> = Vec::new();
    for (library, to_names) in &to_libs {
        let from_names = from_libs.remove(library).unwrap_or_default();
        let lib_children = diff_name_set(&from_names, to_names, "import");
        let Some(node) = diff_group_node(&format!("importlib:{library}"), library, lib_children)
        else {
            continue;
        };
        children.push(Node {
            default_collapsed: true,
            ..node
        });
    }
    for (library, from_names) in from_libs {
        let lib_children = diff_name_set(&from_names, &std::collections::BTreeSet::new(), "import");
        if let Some(node) = diff_group_node(&format!("importlib:{library}"), &library, lib_children)
        {
            children.push(Node {
                default_collapsed: true,
                ..node
            });
        }
    }
    diff_group_node("imports", "Imports", children)
}

/// Set-diff two name collections into status-tagged leaves, `id_prefix:name`.
fn diff_name_set(
    from: &std::collections::BTreeSet<String>,
    to: &std::collections::BTreeSet<String>,
    id_prefix: &str,
) -> Vec<Node> {
    let mut children = Vec::new();
    for name in to {
        let status = if from.contains(name) {
            NodeStatus::Unchanged
        } else {
            NodeStatus::Added
        };
        children.push(Node {
            id: format!("{id_prefix}:{name}"),
            label: name.clone(),
            status: Some(status),
            ..Node::default()
        });
    }
    for name in from.difference(to) {
        children.push(Node {
            id: format!("{id_prefix}:{name}"),
            label: name.clone(),
            status: Some(NodeStatus::Removed),
            ..Node::default()
        });
    }
    children
}

/// Groups status-tagged `children`, pruning Unchanged and dropping if empty.
fn diff_group_node(id: &str, label: &str, mut children: Vec<Node>) -> Option<Node> {
    children.retain(|c| !matches!(c.status, Some(NodeStatus::Unchanged)));
    if children.is_empty() {
        return None;
    }
    let status = aggregate_status(&children);
    Some(Node {
        id: id.to_string(),
        label: label.to_string(),
        badge: Some(children.len().to_string()),
        status: Some(status),
        children,
        ..Node::default()
    })
}

fn aggregate_status(children: &[Node]) -> NodeStatus {
    if children
        .iter()
        .any(|c| !matches!(c.status, Some(NodeStatus::Unchanged) | None))
    {
        NodeStatus::Changed
    } else {
        NodeStatus::Unchanged
    }
}
