// TODO(ai-review): review for style and correctness
//! Per-type diff between two .NET assemblies. Given two DLL byte
//! slices, returns each type's status (added / removed / changed /
//! unchanged) keyed by its fully-qualified name.
//!
//! "Changed" is intentionally permissive in v1: a type's signature is
//! a flat string assembled from every dotnetdll field that signals
//! a change — accessibility, base type, implemented interfaces,
//! fields, properties, methods, IL bodies. Any byte-level difference
//! marks the type changed. False positives (cascading metadata-index
//! shifts inside IL operands) are accepted for now; tighten later by
//! resolving references through the [`dotnetdll::Resolution`] before
//! hashing.
//!
//! FQNs follow ilspy's `Outer.Inner` shape (no `+` separator for
//! nested types) so the output drops straight into the existing
//! dll-tree builder in the viewer.

use std::collections::HashMap;

use dotnetdll::dll::DLLError;
use dotnetdll::prelude::{ReadOptions, Resolution, TypeIndex};
use tracing::info_span;

pub use dotnetdll;

#[doc(hidden)]
pub mod sig;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to parse DLL: {0}")]
    DLL(#[from] DLLError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Added,
    Removed,
    Changed,
    Unchanged,
}

#[derive(Debug, Clone)]
pub struct TypeStatus {
    pub fqn: String,
    pub status: Status,
}

#[derive(Debug, Clone, Default)]
pub struct Diff {
    /// One entry per type that exists in either side, sorted by FQN.
    pub types: Vec<TypeStatus>,
}

/// Diff every type going **from** the `from` assembly **to** the `to`
/// assembly, in the GNU-`diff` sense: a type that exists in `to` only
/// is [`Status::Added`], one in `from` only is [`Status::Removed`].
///
/// Method bodies are parsed (so IL changes count); compiler-generated
/// types (`<...>`) are kept — callers that want them dropped can
/// filter on FQN.
#[tracing::instrument(skip_all)]
pub fn diff(from: &[u8], to: &[u8]) -> Result<Diff, Error> {
    let (from_sigs, to_sigs) = info_span!("preprocess").in_scope(|| {
        let from_span = info_span!("from", bytes = from.len());
        let to_span = info_span!("to", bytes = to.len());
        rayon::join(
            || from_span.in_scope(|| preprocess(from, "from")),
            || to_span.in_scope(|| preprocess(to, "to")),
        )
    });
    let from_sigs = from_sigs?;
    let to_sigs = to_sigs?;

    let types = info_span!(
        "compare_signatures",
        from_types = from_sigs.len(),
        to_types = to_sigs.len()
    )
    .in_scope(|| {
        let mut all_fqns: Vec<&str> = from_sigs
            .keys()
            .chain(to_sigs.keys())
            .map(|s| s.as_str())
            .collect();
        all_fqns.sort_unstable();
        all_fqns.dedup();

        let mut types: Vec<TypeStatus> = Vec::with_capacity(all_fqns.len());
        for fqn in all_fqns {
            let status = match (from_sigs.get(fqn), to_sigs.get(fqn)) {
                (None, Some(_)) => Status::Added,
                (Some(_), None) => Status::Removed,
                (Some(a), Some(b)) if a == b => Status::Unchanged,
                (Some(_), Some(_)) => Status::Changed,
                (None, None) => unreachable!(),
            };
            types.push(TypeStatus {
                fqn: fqn.to_string(),
                status,
            });
        }
        types
    });
    Ok(Diff { types })
}

/// Parse one DLL and turn it into the per-FQN signature map. One side
/// of the diff pipeline — `diff` runs two of these on rayon workers
/// so the slower side gates wall time, not the sum. `side` is plumbed
/// onto the inner spans as a field so flat trace output stays
/// unambiguous regardless of close-time order.
fn preprocess(bytes: &[u8], side: &'static str) -> Result<HashMap<String, u64>, Error> {
    let res =
        info_span!("parse", side).in_scope(|| Resolution::parse(bytes, ReadOptions::default()))?;
    let sigs = info_span!("signatures", side).in_scope(|| type_signatures(&res));
    Ok(sigs)
}

/// Build a `FQN → content-hash` map for every type definition in
/// `res`. The hash is `DefaultHasher::default().finish()` over
/// `TypeDefinition` and covers everything the resolved metadata model
/// exposes (flags, base/implements, fields, properties, events,
/// methods + IL bodies). Two types match iff their hashes match.
pub fn type_signatures(res: &Resolution<'_>) -> HashMap<String, u64> {
    let mut out: HashMap<String, u64> = HashMap::new();
    for (idx, td) in res.enumerate_type_definitions() {
        let fqn = type_fqn(idx, res);
        out.insert(fqn, sig::hash_type(td, res));
    }
    out
}

/// Fully-qualified name of a type, walking the encloser chain so
/// nested types render as `Outer.Inner` (matching `ilspycmd -l` and
/// the existing dll-tree builder).
fn type_fqn(idx: TypeIndex, res: &Resolution<'_>) -> String {
    let td = &res[idx];
    match td.encloser {
        Some(outer) => format!("{}.{}", type_fqn(outer, res), td.name),
        None => match &td.namespace {
            Some(ns) if !ns.is_empty() => format!("{}.{}", ns, td.name),
            _ => td.name.to_string(),
        },
    }
}
