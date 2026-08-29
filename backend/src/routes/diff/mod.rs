// TODO(ai-review): review for style and correctness
//! Diff endpoints: [`manifest`] (path-level), [`text_diff`] (unified text diff), [`structured`] (per-object tree + node content).

use crate::steam::{DepotId, ManifestId};

pub mod manifest;
pub mod structured;
pub mod text_diff;

fn default_branch() -> String {
    "public".to_string()
}

/// `depot/<manifest id padded to 20> YYYY-MM-DD`, the u64-max width so ids line up.
fn diff_label(depot_id: DepotId, manifest_id: ManifestId, creation_time: u32) -> String {
    let date_fmt = time::macros::format_description!("[year]-[month]-[day]");
    let date = time::OffsetDateTime::from_unix_timestamp(creation_time as i64)
        .ok()
        .and_then(|d| d.format(&date_fmt).ok())
        .unwrap_or_else(|| "?".to_string());
    format!("{depot_id}/{:<20} {date}", manifest_id.0)
}
