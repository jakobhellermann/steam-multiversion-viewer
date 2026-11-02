// TODO(ai-review): review for style and correctness
//! Links from decompiled C# to the types it names, in this DLL or the
//! manifest's other ones.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use bytes::Bytes;
use transform::dll::refs::TypeRef;
use transform::structured::{DiffSide, TextLink};

use super::Result;
use crate::http::ApiError;
use crate::state::Snapshot;

/// Where a type named in decompiled C# is defined.
pub(crate) enum TypeTarget {
    ThisDll,
    Dll(String),
    /// In an assembly the manifest doesn't ship.
    Unavailable,
}

/// The types named by the decompiled source of `type_name`, and at the
/// same index the DLL defining each, following type forwarders.
pub(crate) async fn link_targets(
    snapshot: &Snapshot,
    path: &str,
    bytes: Bytes,
    type_name: &str,
) -> Result<(Vec<TypeRef>, Vec<TypeTarget>)> {
    // A forwarding chain longer than this is a cycle.
    const MAX_FORWARDS: usize = 8;

    let name = type_name.to_string();
    let refs = tokio::task::spawn_blocking(move || transform::dll::refs::type_refs(&bytes, &name))
        .await
        .map_err(|e| ApiError::internal(format!("type-refs task panicked: {e}")))?
        .map_err(ApiError::from_transform)?;

    let dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
    let mut assemblies: HashMap<String, Option<ShippedAssembly>> = HashMap::new();
    let mut targets = Vec::with_capacity(refs.len());
    'refs: for r in &refs {
        let mut assembly = r.assembly.clone();
        for _ in 0..MAX_FORWARDS {
            let Some(name) = assembly else {
                targets.push(TypeTarget::ThisDll);
                continue 'refs;
            };
            if !assemblies.contains_key(&name) {
                let shipped = shipped_assembly(snapshot, dir, &name).await?;
                assemblies.insert(name.clone(), shipped);
            }
            let Some(shipped) = &assemblies[&name] else {
                targets.push(TypeTarget::Unavailable);
                continue 'refs;
            };
            match shipped.forwarders.get(&r.fqn) {
                Some(next) => assembly = Some(next.clone()),
                None => {
                    targets.push(TypeTarget::Dll(shipped.path.clone()));
                    continue 'refs;
                }
            }
        }
        return Err(ApiError::internal(format!(
            "type forwarders of {} form a cycle",
            r.fqn
        )));
    }
    Ok((refs, targets))
}

/// A link as a byte range of the text it was found in.
#[derive(Clone)]
pub(crate) struct ByteLink {
    pub range: Range<usize>,
    pub node_id: String,
    pub file: Option<String>,
    pub side: Option<DiffSide>,
}

/// Link every type name in `text` that resolves to an available type.
/// Unavailable ones still take part in resolution, so a name they make
/// ambiguous stays unlinked. Hands `text` back alongside.
pub(crate) async fn link_type_names(
    text: String,
    (refs, targets): (Vec<TypeRef>, Vec<TypeTarget>),
    type_name: &str,
) -> Result<(String, Vec<ByteLink>)> {
    let type_name = type_name.to_string();
    tokio::task::spawn_blocking(move || {
        let located = transform::dll::links::locate_type_names(&text, &refs, &type_name)?;
        let links = located
            .into_iter()
            .filter_map(|(range, i)| {
                let file = match &targets[i] {
                    TypeTarget::ThisDll => None,
                    TypeTarget::Dll(path) => Some(path.clone()),
                    TypeTarget::Unavailable => return None,
                };
                Some(ByteLink {
                    range,
                    node_id: format!("type:{}", refs[i].fqn),
                    file,
                    side: None,
                })
            })
            .collect();
        Ok::<_, transform::TransformError>((text, links))
    })
    .await
    .map_err(|e| ApiError::internal(format!("type-links task panicked: {e}")))?
    .map_err(ApiError::from_transform)
}

/// The wire form of `links` (byte ranges of `text`, sorted by start).
pub(crate) fn to_text_links(text: &str, links: Vec<ByteLink>) -> Vec<TextLink> {
    let byte_ranges: Vec<_> = links.iter().map(|l| l.range.clone()).collect();
    let ranges = transform::structured::utf16_ranges(text, &byte_ranges);
    links
        .into_iter()
        .zip(ranges)
        .map(|(link, range)| TextLink {
            start: range.start,
            end: range.end,
            node_id: link.node_id,
            file: link.file,
            side: link.side,
        })
        .collect()
}

struct ShippedAssembly {
    path: String,
    forwarders: Arc<HashMap<String, String>>,
}

/// `None` if the manifest has no `<dir>/<assembly>.dll`.
async fn shipped_assembly(
    snapshot: &Snapshot,
    dir: &str,
    assembly: &str,
) -> Result<Option<ShippedAssembly>> {
    let path = format!("{dir}/{assembly}.dll");
    let Some(file) = crate::platform_paths::find_corresponding(snapshot.manifest(), &path) else {
        return Ok(None);
    };
    let path = file.path.clone();
    let sha = file
        .sha()
        .map(|h| h.0)
        .ok_or_else(|| ApiError::internal(format!("no sha for {path}")))?;
    let forwarders = match transform::dll::refs::cached_forwarded_types(&sha) {
        Some(cached) => cached,
        None => {
            let bytes = snapshot.read_full(&path).await?;
            tokio::task::spawn_blocking(move || {
                transform::dll::refs::forwarded_types_memoized(sha, &bytes)
            })
            .await
            .map_err(|e| ApiError::internal(format!("forwarders task panicked: {e}")))?
            .map_err(ApiError::from_transform)?
        }
    };
    Ok(Some(ShippedAssembly { path, forwarders }))
}
