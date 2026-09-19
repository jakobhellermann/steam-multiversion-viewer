use std::collections::{BTreeMap, HashSet};

use anyhow::Result;
use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::objects::ClassId;
use rabex_env::rabex::objects::pptr::PathId;
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;

use crate::structured::{Node, NodeStatus};
use crate::unity::{NameOnly, class_label, content_mime_for_class, object_name};

use super::Side;
use super::hierarchy::Covered;

#[tracing::instrument(skip_all)]
pub(super) fn diff_loose<R: EnvResolver, P: TypeTreeProvider>(
    base_file: &SerializedFileHandle<'_, R, P>,
    target_file: &SerializedFileHandle<'_, R, P>,
    covered: &Covered,
) -> Result<Node> {
    let base_bodies = super::build_body_index(base_file);
    let target_bodies = super::build_body_index(target_file);

    let base_raw = collect_loose(base_file, &covered.base)?;
    let target_raw = collect_loose(target_file, &covered.target)?;

    let base_counts = class_id_counts(&base_raw);
    let target_counts = class_id_counts(&target_raw);
    let is_singleton = |class_id: ClassId| {
        base_counts.get(&class_id) == Some(&1) && target_counts.get(&class_id) == Some(&1)
    };
    let key_for = |raw: &RawLoose| LooseKey {
        label: raw.label.clone(),
        name: if is_singleton(raw.class_id) {
            String::new()
        } else if raw.name.is_empty() {
            format!("__pid:{}", raw.path_id)
        } else {
            raw.name.clone()
        },
    };
    let mut base_items: BTreeMap<LooseKey, LooseItem> = BTreeMap::new();
    for r in &base_raw {
        base_items.insert(
            key_for(r),
            LooseItem {
                path_id: r.path_id,
                class_id: r.class_id,
            },
        );
    }
    let mut target_items: BTreeMap<LooseKey, LooseItem> = BTreeMap::new();
    for r in &target_raw {
        target_items.insert(
            key_for(r),
            LooseItem {
                path_id: r.path_id,
                class_id: r.class_id,
            },
        );
    }

    let mut keys: Vec<LooseKey> = base_items
        .keys()
        .chain(target_items.keys())
        .cloned()
        .collect();
    keys.sort();
    keys.dedup();

    let mut children: Vec<Node> = Vec::new();
    for key in keys {
        let b = base_items.remove(&key);
        let t = target_items.remove(&key);
        let node = match (b, t) {
            (Some(b), Some(t)) => {
                let status = super::matched_status(
                    base_file,
                    target_file,
                    &base_bodies,
                    &target_bodies,
                    b.path_id,
                    t.path_id,
                );
                let id = super::matched_pair_id(b.path_id, t.path_id);
                let (label, badge) = loose_label(base_file, &key, &b);
                Node {
                    badge,
                    facets: [("class".to_string(), key.label.clone())]
                        .into_iter()
                        .collect(),
                    ..super::make_node(id, label, "component", status)
                }
            }
            (Some(b), None) => {
                let (label, badge) = loose_label(base_file, &key, &b);
                Node {
                    badge,
                    facets: [("class".to_string(), key.label.clone())]
                        .into_iter()
                        .collect(),
                    content_mime: content_mime_for_class(b.class_id),
                    ..super::make_node(
                        super::one_sided_id(Side::Base, b.path_id),
                        label,
                        "component",
                        NodeStatus::Added,
                    )
                }
            }
            (None, Some(t)) => {
                let (label, badge) = loose_label(target_file, &key, &t);
                Node {
                    badge,
                    facets: [("class".to_string(), key.label.clone())]
                        .into_iter()
                        .collect(),
                    content_mime: content_mime_for_class(t.class_id),
                    ..super::make_node(
                        super::one_sided_id(Side::Target, t.path_id),
                        label,
                        "component",
                        NodeStatus::Removed,
                    )
                }
            }
            (None, None) => unreachable!(),
        };
        children.push(node);
    }

    let badge = Some(format!(
        "{} {}",
        children.len(),
        super::pluralize(children.len(), "object")
    ));
    let pruned = super::prune_unchanged(children);
    let status = super::aggregate_status(&pruned);
    Ok(Node {
        badge,
        children: pruned,
        ..super::make_node("section:loose", "Loose components", "section", status)
    })
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct LooseKey {
    label: String,
    name: String,
}

struct LooseItem {
    path_id: PathId,
    class_id: ClassId,
}

struct RawLoose {
    class_id: ClassId,
    label: String,
    name: String,
    path_id: PathId,
}

fn class_id_counts(items: &[RawLoose]) -> std::collections::HashMap<ClassId, usize> {
    let mut m: std::collections::HashMap<ClassId, usize> = std::collections::HashMap::new();
    for r in items {
        *m.entry(r.class_id).or_default() += 1;
    }
    m
}

#[tracing::instrument(skip_all)]
fn collect_loose<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    covered: &HashSet<PathId>,
) -> Result<Vec<RawLoose>> {
    let mut out: Vec<RawLoose> = Vec::new();
    for obj in file.file.objects() {
        let path_id = obj.m_PathID;
        if covered.contains(&path_id) {
            continue;
        }
        let class_id = obj.m_ClassID;
        let _obj_span = tracing::info_span!("loose_object", ?class_id, path_id).entered();
        let name = file
            .object_at::<NameOnly>(path_id)
            .ok()
            .and_then(|h| h.read().ok())
            .map(|n| n.m_Name)
            .unwrap_or_default();
        let label = class_label(file, class_id, path_id);
        out.push(RawLoose {
            class_id,
            label,
            name,
            path_id,
        });
    }
    Ok(out)
}

/// Class as label, the object's name as badge — the tree's rule. The
/// name is read fresh for display only; the match key keeps its
/// singleton rule, so a renamed singleton stays one pair, badged
/// from the base side.
fn loose_label<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    key: &LooseKey,
    item: &LooseItem,
) -> (String, Option<String>) {
    (
        key.label.clone(),
        object_name(file, &key.label, item.path_id),
    )
}
