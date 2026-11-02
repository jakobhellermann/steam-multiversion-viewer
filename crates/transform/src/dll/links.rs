// TODO(ai-review): review for style and correctness
//! Locate the type names in decompiled C# and resolve each to one of the
//! [`TypeRef`]s the decompiled type names.
//!
//! Positions come from the syntax tree. Resolution mirrors the C# name
//! lookup ilspy relied on when it left a name unqualified: the file's
//! `using`s, its namespace, the enclosing type and the generic argument
//! count. A name it can't pin to a single type stays unlinked.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use tree_sitter::{Node, Parser, TreeCursor};

use super::refs::TypeRef;
use crate::TransformError;

/// ilspy writes these as their C# keyword, so a bare name never means them.
const KEYWORD_TYPES: &[&str] = &[
    "System.Object",
    "System.String",
    "System.Boolean",
    "System.Char",
    "System.SByte",
    "System.Byte",
    "System.Int16",
    "System.UInt16",
    "System.Int32",
    "System.UInt32",
    "System.Int64",
    "System.UInt64",
    "System.Single",
    "System.Double",
    "System.Decimal",
    "System.Void",
];

/// Byte range of each type name in `source` with the index of its
/// [`TypeRef`] in `refs`, in source order. `self_fqn` is the decompiled type.
pub fn locate_type_names(
    source: &str,
    refs: &[TypeRef],
    self_fqn: &str,
) -> Result<Vec<(Range<usize>, usize)>, TransformError> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_c_sharp::LANGUAGE.into())
        .map_err(|e| TransformError::Other(format!("tree-sitter-c-sharp: {e}")))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| TransformError::Other("tree-sitter parse cancelled".into()))?;
    let root = tree.root_node();

    let mut scopes = HashSet::from([String::new(), self_fqn.to_string()]);
    collect_scopes(root, source, &mut scopes);
    let resolver = Resolver::new(refs, scopes);

    // `Node::parent` re-walks from the root, so the walk keeps the
    // ancestor path itself.
    let mut out = Vec::new();
    let mut cursor = root.walk();
    let mut path = vec![Step {
        node: root,
        field: None,
    }];
    loop {
        let node = cursor.node();
        if node.kind() == "identifier"
            && let Some(name) = type_name(&path, source)
            && let Some(idx) = resolver.resolve(&name)
        {
            out.push((node.byte_range(), idx));
        }
        if cursor.goto_first_child() {
            path.push(Step::at(&cursor));
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                *path.last_mut().expect("path holds the current node") = Step::at(&cursor);
                break;
            }
            if !cursor.goto_parent() {
                return Ok(out);
            }
            path.pop();
        }
    }
}

/// A node on the path from the root, with its field name in its parent.
#[derive(Clone, Copy)]
struct Step<'t> {
    node: Node<'t>,
    field: Option<&'t str>,
}

impl<'t> Step<'t> {
    fn at(cursor: &TreeCursor<'t>) -> Self {
        Step {
            node: cursor.node(),
            field: cursor.field_name(),
        }
    }
}

/// A type name as written at one identifier.
#[derive(Debug, PartialEq)]
struct WrittenName {
    /// `Object`, or `UnityEngine.Object` when qualified.
    dotted: String,
    /// Generic arguments written after it.
    arity: usize,
    /// In attribute position, where C# drops the `Attribute` suffix.
    attribute: bool,
}

/// The name the identifier ending `path` writes if it sits in a type
/// position or in a member access (`Mathf` in `Mathf.Clamp(…)`), which is
/// how static members are reached. Declared names and the like yield `None`.
fn type_name(path: &[Step], source: &str) -> Option<WrittenName> {
    let ident = path.last()?.node;
    let text = &source[ident.byte_range()];
    let mut i = path.len() - 1;
    let mut arity = 0;
    if i >= 1 && path[i - 1].node.kind() == "generic_name" {
        arity = generic_arity(path[i - 1].node);
        i -= 1;
    }
    let parent = path.get(i.checked_sub(1)?)?.node;
    let field = path[i].field;

    let dotted = match (parent.kind(), field) {
        ("qualified_name" | "member_access_expression", Some("name")) => {
            let qualifier = parent
                .child_by_field_name("qualifier")
                .or_else(|| parent.child_by_field_name("expression"))?;
            format!("{}.{text}", dotted_name(qualifier, source)?)
        }
        _ => text.to_string(),
    };

    // Where the whole qualified name this identifier is part of sits.
    let mut top = i;
    while top >= 1 && path[top - 1].node.kind() == "qualified_name" {
        top -= 1;
    }
    let top_parent = path.get(top.checked_sub(1)?)?.node.kind();
    let top_field = path[top].field;
    let attribute = top_parent == "attribute" && top_field == Some("name");
    let in_type_position = attribute
        || match top_parent {
            "base_list"
            | "type_argument_list"
            | "explicit_interface_specifier"
            | "constant_pattern"
            | "type_parameter_constraint" => true,
            "as_expression" => top_field == Some("right"),
            _ => matches!(top_field, Some("type" | "returns")),
        };
    let in_member_access =
        parent.kind() == "member_access_expression" && matches!(field, Some("expression" | "name"));
    (in_type_position || in_member_access).then_some(WrittenName {
        dotted,
        arity,
        attribute,
    })
}

/// `A.B.C` for a chain of identifiers, generic names and member accesses,
/// generic arguments dropped; `None` for anything else (a call, `this`, …).
fn dotted_name(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => Some(source[node.byte_range()].to_string()),
        "generic_name" => dotted_name(node.named_child(0)?, source),
        "qualified_name" | "member_access_expression" => {
            let left = node
                .child_by_field_name("qualifier")
                .or_else(|| node.child_by_field_name("expression"))?;
            let right = node.child_by_field_name("name")?;
            Some(format!(
                "{}.{}",
                dotted_name(left, source)?,
                dotted_name(right, source)?
            ))
        }
        _ => None,
    }
}

/// Commas + 1 inside the `type_argument_list`; counts `Dictionary<,>` too.
fn generic_arity(generic_name: Node) -> usize {
    let mut cursor = generic_name.walk();
    let Some(args) = generic_name
        .children(&mut cursor)
        .find(|c| c.kind() == "type_argument_list")
    else {
        return 0;
    };
    let mut cursor = args.walk();
    1 + args
        .children(&mut cursor)
        .filter(|c| c.kind() == ",")
        .count()
}

/// Namespaces names resolve in: the `using`s and every declared namespace
/// with its parents.
fn collect_scopes(root: Node, source: &str, scopes: &mut HashSet<String>) {
    let mut cursor = root.walk();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        match node.kind() {
            "using_directive" => {
                // `using X = Y;` and `using static X;` bring no namespace into scope.
                let mut c = node.walk();
                let is_plain = node
                    .children(&mut c)
                    .all(|ch| !matches!(ch.kind(), "=" | "static"));
                let mut c = node.walk();
                let name = node
                    .named_children(&mut c)
                    .find_map(|ch| dotted_name(ch, source));
                if is_plain && let Some(name) = name {
                    scopes.insert(name);
                }
            }
            "namespace_declaration" | "file_scoped_namespace_declaration" => {
                if let Some(name) = node
                    .child_by_field_name("name")
                    .and_then(|n| dotted_name(n, source))
                {
                    let mut prefix = String::new();
                    for segment in name.split('.') {
                        if !prefix.is_empty() {
                            prefix.push('.');
                        }
                        prefix.push_str(segment);
                        scopes.insert(prefix.clone());
                    }
                }
            }
            _ => {}
        }
        pending.extend(node.named_children(&mut cursor));
    }
}

struct Resolver<'r> {
    refs: &'r [TypeRef],
    by_name: HashMap<&'r str, Vec<usize>>,
    scopes: HashSet<String>,
}

impl<'r> Resolver<'r> {
    fn new(refs: &'r [TypeRef], scopes: HashSet<String>) -> Self {
        let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, r) in refs.iter().enumerate() {
            let name = r.fqn.rsplit_once('.').map_or(r.fqn.as_str(), |(_, n)| n);
            by_name.entry(name).or_default().push(i);
        }
        Resolver {
            refs,
            by_name,
            scopes,
        }
    }

    fn resolve(&self, name: &WrittenName) -> Option<usize> {
        if name.attribute
            && let Some(idx) = self.resolve_dotted(&format!("{}Attribute", name.dotted), name.arity)
        {
            return Some(idx);
        }
        self.resolve_dotted(&name.dotted, name.arity)
    }

    fn resolve_dotted(&self, dotted: &str, arity: usize) -> Option<usize> {
        let short = dotted.rsplit_once('.').map_or(dotted, |(_, n)| n);
        let qualified = short.len() != dotted.len();
        let candidates: Vec<usize> = self
            .by_name
            .get(short)?
            .iter()
            .copied()
            .filter(|&i| {
                let fqn = &self.refs[i].fqn;
                let matches = fqn == dotted
                    || fqn
                        .strip_suffix(dotted)
                        .is_some_and(|container| container.ends_with('.'));
                matches && (qualified || !KEYWORD_TYPES.contains(&fqn.as_str()))
            })
            .collect();
        let same_arity: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&i| self.refs[i].arity == arity)
            .collect();
        // `Action` and `Action<T>` share an FQN, and with it a link target.
        let mut pool = if same_arity.is_empty() {
            candidates
        } else {
            same_arity
        };
        let mut seen = HashSet::new();
        pool.retain(|&i| seen.insert(self.refs[i].fqn.as_str()));
        if pool.len() <= 1 {
            return pool.first().copied();
        }
        let container = |i: usize| {
            let fqn = &self.refs[i].fqn;
            fqn[..fqn.len() - dotted.len()].trim_end_matches('.')
        };
        let in_scope: Vec<usize> = pool
            .into_iter()
            .filter(|&i| self.scopes.contains(container(i)))
            .collect();
        match in_scope.as_slice() {
            [single] => Some(*single),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(fqn: &str, arity: usize) -> TypeRef {
        TypeRef {
            fqn: fqn.to_string(),
            arity,
            assembly: None,
        }
    }

    fn linked(source: &str, refs: &[TypeRef]) -> Vec<(String, String)> {
        locate_type_names(source, refs, "AlertRange")
            .unwrap()
            .into_iter()
            .map(|(range, i)| (source[range].to_string(), refs[i].fqn.clone()))
            .collect()
    }

    fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
        expected
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn links_type_positions_static_access_and_qualified_names() {
        let refs = [
            r("TrackTriggerObjects", 0),
            r("UnityEngine.Object", 0),
            r("UnityEngine.Transform", 0),
            r("UnityEngine.Mathf", 0),
            r("UnityEngine.SerializeField", 0),
            r("System.ObsoleteAttribute", 0),
            r("AlertRange.LineOfSightChecks", 0),
            r("UnityEngine.Color", 0),
        ];
        let source = r#"using System;
using UnityEngine;

public class AlertRange : TrackTriggerObjects
{
    [SerializeField]
    [Obsolete]
    private Transform target;

    private LineOfSightChecks lineOfSight;

    private void Foo()
    {
        float x = Mathf.Clamp(1f, 2f, 3f);
        UnityEngine.Object.Destroy(target);
        target.Color = "Transform";
    }
}"#;
        assert_eq!(
            linked(source, &refs),
            pairs(&[
                ("TrackTriggerObjects", "TrackTriggerObjects"),
                ("SerializeField", "UnityEngine.SerializeField"),
                ("Obsolete", "System.ObsoleteAttribute"),
                ("Transform", "UnityEngine.Transform"),
                ("LineOfSightChecks", "AlertRange.LineOfSightChecks"),
                ("Mathf", "UnityEngine.Mathf"),
                ("Object", "UnityEngine.Object"),
            ])
        );
    }

    #[test]
    fn disambiguates_by_usings_arity_and_keyword_types() {
        let refs = [
            r("System.Object", 0),
            r("UnityEngine.Object", 0),
            r("System.Random", 0),
            r("UnityEngine.Random", 0),
            r("System.Collections.IEnumerator", 0),
            r("System.Collections.Generic.IEnumerator", 1),
        ];
        let source = r#"using System;
using System.Collections;
using System.Collections.Generic;
using UnityEngine;

public class AlertRange
{
    private Object o;
    private Random ambiguous;
    private IEnumerator a;
    private IEnumerator<int> b;
}"#;
        assert_eq!(
            linked(source, &refs),
            pairs(&[
                ("Object", "UnityEngine.Object"),
                ("IEnumerator", "System.Collections.IEnumerator"),
                ("IEnumerator", "System.Collections.Generic.IEnumerator"),
            ])
        );
    }

    #[test]
    fn a_shared_short_name_resolves_through_the_only_imported_namespace() {
        let refs = [r("System.Random", 0), r("UnityEngine.Random", 0)];
        let source = "using UnityEngine;\npublic class AlertRange { private Random r; }";
        assert_eq!(
            linked(source, &refs),
            pairs(&[("Random", "UnityEngine.Random")])
        );
    }
}
