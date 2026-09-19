// TODO(ai-review): review for style and correctness
//! For every type dll-diff reports as Changed but ilspy decompiles
//! identically (the "remaining false positives" from
//! `dll_diff_decompile`), bisect down to which method/field hashes
//! differ and categorise the cause:
//!   * `il-length-differs` — instruction count is different
//!   * `il-content-differs` — same length, different ops/operands
//!     * `field-only` — only a field hash differs
//!     * `type-header` — top-level type bits differ
//!
//! Helps decide whether to invest in narrowing further or call it
//! done.

use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::Result;
use dll_diff::dotnetdll::prelude::*;
use dll_diff::dotnetdll::resolved::types::TypeDefinition;
use dll_diff::sig::{field_hash, is_empty_cctor, method_hash};
use steam_depot_vfs::DepotStore;
use steam_depot_vfs::session::LazyCachedAuth;
use steam_multiversion_viewer::config::Config;

const APP_ID: u32 = 367520;
const DEPOT_ID: u32 = 367523;
const FROM_MANIFEST: u64 = 5829533265112705522;
const TO_MANIFEST: u64 = 708613018541602983;
const BRANCH: &str = "public";
const DLL_PATH: &str = "hollow_knight_Data/Managed/Assembly-CSharp.dll";

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let auth = LazyCachedAuth::prepare(
        LazyCachedAuth::default_refresh_token_cache(),
        std::env::var("STEAM_USERNAME").expect("missing STEAM_USERNAME"),
        std::env::var("STEAM_PASSWORD").expect("missing STEAM_PASSWORD"),
    )
    .await?;
    let auth = Arc::new(auth);

    let config = Config::load_or_default()?;
    let store = DepotStore::new(config.store_root.as_std_path().to_path_buf());
    let from_manifest = store
        .open_depot_manifest(auth.clone(), APP_ID, DEPOT_ID, FROM_MANIFEST, BRANCH)
        .await?;
    let to_manifest = store
        .open_depot_manifest(auth, APP_ID, DEPOT_ID, TO_MANIFEST, BRANCH)
        .await?;
    let (from_bytes, to_bytes) = tokio::try_join!(
        from_manifest.read_full(DLL_PATH),
        to_manifest.read_full(DLL_PATH)
    )?;
    let from_bytes = from_bytes.to_vec();
    let to_bytes = to_bytes.to_vec();

    // Enumerate false-positive FQNs by scanning dll-diff-output/ for
    // empty .diff files. Saves us from re-running the decompile
    // pipeline here.
    let empty: BTreeSet<String> = std::fs::read_dir("dll-diff-output")?
        .filter_map(|e| e.ok())
        .filter(|e| e.metadata().map(|m| m.len() == 0).unwrap_or(false))
        .filter_map(|e| {
            e.path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        })
        .collect();
    println!("{} empty-diff FQNs to classify", empty.len());

    tokio::task::spawn_blocking(move || -> Result<()> {
        let from_res = Resolution::parse(&from_bytes, ReadOptions::default())?;
        let to_res = Resolution::parse(&to_bytes, ReadOptions::default())?;
        classify(&from_res, &to_res, &empty);
        Ok(())
    })
    .await??;
    Ok(())
}

fn find<'a, 'b>(res: &'a Resolution<'b>, fqn: &str) -> Option<&'a TypeDefinition<'b>> {
    for (idx, td) in res.enumerate_type_definitions() {
        let name = match &td.namespace {
            Some(ns) if !ns.is_empty() => format!("{}.{}", ns, td.name),
            _ => td.name.to_string(),
        };
        if name == fqn {
            return Some(&res[idx]);
        }
    }
    None
}

#[derive(Default)]
struct Buckets {
    il_length_differs: Vec<String>,
    il_content_differs: Vec<String>,
    field_only: Vec<String>,
    multiple: Vec<String>,
    other: Vec<String>,
}

fn classify(from_res: &Resolution<'_>, to_res: &Resolution<'_>, fqns: &BTreeSet<String>) {
    let mut b = Buckets::default();
    for fqn in fqns {
        let Some(from) = find(from_res, fqn) else {
            b.other.push(fqn.clone());
            continue;
        };
        let Some(to) = find(to_res, fqn) else {
            b.other.push(fqn.clone());
            continue;
        };

        let mut differing_fields = 0;
        let mut from_fields: Vec<_> = from.fields.iter().collect();
        let mut to_fields: Vec<_> = to.fields.iter().collect();
        from_fields.sort_by(|a, b| a.name.cmp(&b.name));
        to_fields.sort_by(|a, b| a.name.cmp(&b.name));
        if from_fields.len() == to_fields.len() {
            for (a, b) in from_fields.iter().zip(to_fields.iter()) {
                if field_hash(a, from_res) != field_hash(b, to_res) {
                    differing_fields += 1;
                }
            }
        }

        let mut differing_methods_len = 0;
        let mut differing_methods_content = 0;
        // Mirror sig.rs: empty `.cctor` is filtered before hashing,
        // so the classifier has to do the same or its buckets won't
        // line up with what `dll-diff` actually reports.
        let mut from_methods: Vec<_> = from.methods.iter().filter(|m| !is_empty_cctor(m)).collect();
        let mut to_methods: Vec<_> = to.methods.iter().filter(|m| !is_empty_cctor(m)).collect();
        from_methods.sort_by(|a, b| {
            a.name.cmp(&b.name).then_with(|| {
                a.signature
                    .parameters
                    .len()
                    .cmp(&b.signature.parameters.len())
            })
        });
        to_methods.sort_by(|a, b| {
            a.name.cmp(&b.name).then_with(|| {
                a.signature
                    .parameters
                    .len()
                    .cmp(&b.signature.parameters.len())
            })
        });
        if from_methods.len() == to_methods.len() {
            for (ma, mb) in from_methods.iter().zip(to_methods.iter()) {
                if method_hash(ma, from_res) == method_hash(mb, to_res) {
                    continue;
                }
                let (la, lb) = match (&ma.body, &mb.body) {
                    (Some(ba), Some(bb)) => (ba.instructions.len(), bb.instructions.len()),
                    _ => (0, 0),
                };
                if la != lb {
                    differing_methods_len += 1;
                } else {
                    differing_methods_content += 1;
                }
            }
        }

        let total_diffs = differing_fields + differing_methods_len + differing_methods_content;
        match (
            total_diffs,
            differing_methods_len,
            differing_methods_content,
            differing_fields,
        ) {
            (0, _, _, _) => b.other.push(fqn.clone()),
            (1, 1, 0, 0) => b.il_length_differs.push(fqn.clone()),
            (1, 0, 1, 0) => b.il_content_differs.push(fqn.clone()),
            (1, 0, 0, 1) => b.field_only.push(fqn.clone()),
            _ => b.multiple.push(fqn.clone()),
        }
    }

    println!("\nclassification:");
    println!(
        "  il-length-differs  {:>3}  (one method, extra instruction(s))",
        b.il_length_differs.len()
    );
    println!(
        "  il-content-differs {:>3}  (same length, different ops)",
        b.il_content_differs.len()
    );
    println!("  field-only         {:>3}", b.field_only.len());
    println!("  multiple           {:>3}", b.multiple.len());
    println!(
        "  other              {:>3}  (header-only or missing on one side)",
        b.other.len()
    );

    // Drill into "other" — header-level diffs. Bisect by recomputing
    // the section hashes manually and reporting which one diverges.
    println!("\n=== other bucket details ===");
    for fqn in &b.other {
        let from = find(from_res, fqn);
        let to = find(to_res, fqn);
        match (from, to) {
            (Some(f), Some(t)) => describe_header_diff(fqn, f, t, from_res, to_res),
            (Some(_), None) => println!("{fqn}: missing in `to`"),
            (None, Some(_)) => println!("{fqn}: missing in `from`"),
            (None, None) => println!("{fqn}: missing on both sides"),
        }
    }
}

fn describe_header_diff(
    fqn: &str,
    from: &TypeDefinition<'_>,
    to: &TypeDefinition<'_>,
    from_res: &Resolution<'_>,
    to_res: &Resolution<'_>,
) {
    let mut diffs = Vec::new();
    if from.name != to.name {
        diffs.push("name".to_string());
    }
    if from.namespace != to.namespace {
        diffs.push("namespace".to_string());
    }
    if format!("{:?}", from.flags) != format!("{:?}", to.flags) {
        diffs.push(format!("flags ({:?} vs {:?})", from.flags, to.flags));
    }
    if from.encloser.is_some() != to.encloser.is_some() {
        diffs.push("encloser-set".to_string());
    }
    if (from.extends.is_some() != to.extends.is_some())
        || format!(
            "{:?}",
            from.extends.as_ref().map(|t| describe_extends(t, from_res))
        ) != format!(
            "{:?}",
            to.extends.as_ref().map(|t| describe_extends(t, to_res))
        )
    {
        diffs.push("extends".to_string());
    }
    if from.implements.len() != to.implements.len() {
        diffs.push(format!(
            "implements-count ({} vs {})",
            from.implements.len(),
            to.implements.len()
        ));
    }
    if from.generic_parameters.len() != to.generic_parameters.len() {
        diffs.push(format!(
            "generic-params-count ({} vs {})",
            from.generic_parameters.len(),
            to.generic_parameters.len()
        ));
    }
    if from.fields.len() != to.fields.len() {
        diffs.push(format!(
            "field-count ({} vs {})",
            from.fields.len(),
            to.fields.len()
        ));
    }
    if from.methods.len() != to.methods.len() {
        let from_names: BTreeSet<_> = from.methods.iter().map(|m| m.name.as_ref()).collect();
        let to_names: BTreeSet<_> = to.methods.iter().map(|m| m.name.as_ref()).collect();
        let only_from: Vec<&&str> = from_names.difference(&to_names).collect();
        let only_to: Vec<&&str> = to_names.difference(&from_names).collect();
        diffs.push(format!(
            "method-count ({} vs {}; only-from={only_from:?}, only-to={only_to:?})",
            from.methods.len(),
            to.methods.len()
        ));
    }
    if from.properties.len() != to.properties.len() {
        diffs.push(format!(
            "property-count ({} vs {})",
            from.properties.len(),
            to.properties.len()
        ));
    }
    if from.events.len() != to.events.len() {
        diffs.push(format!(
            "event-count ({} vs {})",
            from.events.len(),
            to.events.len()
        ));
    }
    if from.overrides.len() != to.overrides.len() {
        diffs.push(format!(
            "override-count ({} vs {})",
            from.overrides.len(),
            to.overrides.len()
        ));
    }
    println!("{fqn}:");
    if diffs.is_empty() {
        println!("  (no obvious source-level diff — likely property/event content)");
    } else {
        for d in diffs {
            println!("  {d}");
        }
    }
}

fn describe_extends(
    src: &dll_diff::dotnetdll::resolved::types::TypeSource<
        dll_diff::dotnetdll::resolved::types::MemberType,
    >,
    res: &Resolution<'_>,
) -> String {
    use dll_diff::dotnetdll::resolved::types::TypeSource;
    match src {
        TypeSource::User(UserType::Definition(idx)) => {
            let td = &res[*idx];
            format!("{}.{}", td.namespace.as_deref().unwrap_or(""), td.name)
        }
        TypeSource::User(UserType::Reference(idx)) => {
            let tr = &res[*idx];
            format!("{}.{}", tr.namespace.as_deref().unwrap_or(""), tr.name)
        }
        _ => "generic".to_string(),
    }
}
