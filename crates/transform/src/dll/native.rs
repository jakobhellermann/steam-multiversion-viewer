// TODO(ai-review): review for style and correctness
//! PE export/import tree for a native `.exe`/`.dll`, like `nm -D` for `.so`.

use std::collections::BTreeMap;

use object::{Object, ObjectSection};

use crate::TransformError;
use crate::structured::{Node, StructuredTree, human_bytes};

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
