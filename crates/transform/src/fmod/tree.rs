// TODO(ai-review): review for style and correctness
//! `.bank` -> [`StructuredTree`]. One node per RIFF chunk, labeled by
//! its `LIST` sub-tag where it has one (e.g. `PROJ`, `IBSS`, `EVTS`)
//! and by its own tag otherwise (`BNKI`, `SND `, ...) — this is exactly
//! the generic chunk framing `chunks::parse_bank` already verified
//! against real files, nothing more: no `*BODY` payload is decoded, so
//! there's no per-node metadata (event names, parameters, ...) beyond
//! what the chunk tree and the `STDT` entry count already give us.
//!
//! Deliberately doesn't call [`super::Bank::embedded_fsb5`] — that
//! method can panic (`todo!()`) on a bank shape nobody's tested yet,
//! which is fine for the exploratory CLI but not for a route serving
//! real requests. Counts `SND ` chunks directly off the tree instead.

use crate::TransformError;
use crate::structured::{Node, StructuredTree, human_bytes};

use super::bank;
use super::chunks::Chunk;

/// Children past this count collapse by default — large lists (dozens
/// of near-identical `IBUS`/`WAV ` entries) would otherwise swamp the
/// tree on first open.
const COLLAPSE_THRESHOLD: usize = 8;

pub fn build_tree(bytes: &[u8], file_label: &str) -> Result<StructuredTree, TransformError> {
    let parsed = bank::parse(bytes)
        .map_err(|e| TransformError::Other(format!("failed to parse .bank: {e}")))?;

    let mut children = Vec::new();
    if let Some(table) = &parsed.string_table {
        children.push(Node {
            id: "fmod:strings".to_string(),
            label: "String Table".to_string(),
            badge: Some(format!("{} entries", table.len())),
            ..Node::default()
        });
    }
    let sound_chunk_count = count_tag(&parsed.chunks, b"SND ");
    if sound_chunk_count > 0 {
        children.push(Node {
            id: "fmod:sound-data".to_string(),
            label: "Embedded Audio".to_string(),
            badge: Some(format!("{sound_chunk_count} FSB5 blob(s)")),
            ..Node::default()
        });
    }
    children.extend(
        parsed
            .chunks
            .iter()
            .enumerate()
            .map(|(i, c)| chunk_node(c, &i.to_string())),
    );

    Ok(StructuredTree {
        root: Node {
            id: format!("file:{file_label}"),
            label: file_label.to_string(),
            children,
            ..Node::default()
        },
    })
}

fn count_tag(chunks: &[Chunk], tag: &[u8; 4]) -> usize {
    chunks
        .iter()
        .map(|c| usize::from(&c.tag == tag) + count_tag(&c.children, tag))
        .sum()
}

fn chunk_node(chunk: &Chunk, id_path: &str) -> Node {
    let label = chunk.list_id_str().unwrap_or_else(|| chunk.tag_str());
    let children: Vec<Node> = chunk
        .children
        .iter()
        .enumerate()
        .map(|(i, c)| chunk_node(c, &format!("{id_path}.{i}")))
        .collect();
    Node {
        id: format!("chunk:{id_path}"),
        label,
        badge: Some(human_bytes(chunk.size() as u64)),
        default_collapsed: children.len() > COLLAPSE_THRESHOLD,
        children,
        ..Node::default()
    }
}
