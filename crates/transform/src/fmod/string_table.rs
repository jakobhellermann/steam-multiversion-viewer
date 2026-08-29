// TODO(ai-review): review for style and correctness
//! Decoder for the `STDT` (`STRINGDATA`) chunk: a packed radix tree
//! resolving event/bus/parameter GUIDs to their human-readable project
//! path (e.g. `event:/Music/Level01`). Only present when the project's
//! `.strings.bank` is available alongside the bank being read — FMOD
//! Studio splits it out into its own file by convention.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};

use super::reader::{Reader, format_guid};

/// Sentinel used both for "this node has no string segment" and "this
/// node has no parent" (root).
const SENTINEL_24: u32 = 0x00FF_FFFF;

pub struct StringTable {
    guid_to_path: HashMap<[u8; 16], String>,
}

impl StringTable {
    pub fn lookup(&self, guid: &[u8; 16]) -> Option<&str> {
        self.guid_to_path.get(guid).map(String::as_str)
    }

    /// The resolved path, or the GUID's dashed display if this table
    /// doesn't (or can't — see [`parse_string_table`]) cover it.
    pub fn lookup_or_guid(&self, guid: &[u8; 16]) -> String {
        self.lookup(guid)
            .map(str::to_owned)
            .unwrap_or_else(|| format_guid(guid))
    }

    pub fn len(&self) -> usize {
        self.guid_to_path.len()
    }

    pub fn is_empty(&self) -> bool {
        self.guid_to_path.is_empty()
    }
}

struct PackedNode {
    /// Byte offset into the string blob, or `SENTINEL_24` if this node
    /// contributes no text of its own (an intermediate branch node).
    string_offset: u32,
}

impl PackedNode {
    fn has_string(&self) -> bool {
        self.string_offset != SENTINEL_24
    }
}

pub fn parse_string_table(data: &[u8]) -> Result<StringTable> {
    let mut r = Reader::new(data);
    let table_type = r.read_u32()?;
    // `EStringTableType`: 0 = 32-bit radix tree, 1 = 24-bit radix tree.
    // Only the 24-bit variant is reverse-engineered (here and in the
    // reference parser this was ported from) — bail loud rather than
    // guess at the 32-bit layout.
    if table_type != 1 {
        bail!("unsupported string table type {table_type} (only the 24-bit radix tree is decoded)");
    }

    let nodes: Vec<PackedNode> = r.read_elem_list(|r| {
        let key_info = r.read_u32()?;
        let _child_info = r.read_u32()?; // tree topology, unused for lookup
        Ok(PackedNode {
            string_offset: key_info & SENTINEL_24,
        })
    })?;
    let guids: Vec<[u8; 16]> = r.read_elem_list(|r| r.read_guid())?;
    let blob_len = r.read_x16()? as usize;
    let blob = r.read_bytes(blob_len)?;
    let leaf_indices = r.read_u24_array()?;
    let parent_indices = r.read_u24_array()?;

    if leaf_indices.len() != guids.len() {
        bail!(
            "string table corrupt: {} leaf indices for {} guids",
            leaf_indices.len(),
            guids.len()
        );
    }
    if parent_indices.len() != nodes.len() {
        bail!(
            "string table corrupt: {} parent indices for {} nodes",
            parent_indices.len(),
            nodes.len()
        );
    }

    let mut guid_to_path = HashMap::with_capacity(guids.len());
    for (guid, &leaf) in guids.iter().zip(&leaf_indices) {
        let path = resolve_path(&nodes, &parent_indices, blob, leaf)?;
        guid_to_path.insert(*guid, path);
    }
    Ok(StringTable { guid_to_path })
}

/// Walk from a leaf up to the root via `parent_indices`, collecting each
/// visited node's string segment, then reverse to get root-to-leaf order
/// (e.g. `"event:/"` + `"Music/"` + `"Level01"`).
fn resolve_path(
    nodes: &[PackedNode],
    parent_indices: &[u32],
    blob: &[u8],
    leaf: u32,
) -> Result<String> {
    let mut segments = Vec::new();
    let mut node = leaf;
    let mut guard = 0;
    while node != SENTINEL_24 {
        let n = nodes
            .get(node as usize)
            .with_context(|| format!("string table node index {node} out of range"))?;
        if n.has_string() {
            segments.push(read_cstr(blob, n.string_offset as usize));
        }
        node = *parent_indices
            .get(node as usize)
            .with_context(|| format!("no parent entry for node {node}"))?;
        guard += 1;
        if guard > 100_000 {
            bail!("string table parent chain too long (cycle?)");
        }
    }
    segments.reverse();
    Ok(segments.concat())
}

fn read_cstr(blob: &[u8], offset: usize) -> String {
    let tail = blob.get(offset..).unwrap_or(&[]);
    let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
    String::from_utf8_lossy(&tail[..end]).into_owned()
}
