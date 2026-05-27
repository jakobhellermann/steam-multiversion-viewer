// TODO(ai-review): review for style and correctness
//! Diagnostic for the remaining false positives. For a target FQN,
//! parse both `Assembly-CSharp.dll` sides, then for the named type
//! print per-field/per-method hashes computed via the public
//! `dll_diff::type_signatures` map at the type level plus a smaller
//! per-member breakdown so we can see which sub-piece differs.

use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::Result;
use dll_diff::dotnetdll::prelude::*;
use dll_diff::dotnetdll::resolved::types::TypeDefinition;
use steam_depot_vfs::DepotStore;
use steam_depot_vfs::session::LazyCachedAuth;
use steam_multiversion_viewer::config::Config;

const APP_ID: u32 = 367520;
const DEPOT_ID: u32 = 367523;
const FROM_MANIFEST: u64 = 5829533265112705522;
const TO_MANIFEST: u64 = 708613018541602983;
const BRANCH: &str = "public";
const DLL_PATH: &str = "hollow_knight_Data/Managed/Assembly-CSharp.dll";

const FQN: &str = "BossDoorChallengeUIBindingButton";

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

    tokio::task::spawn_blocking(move || -> Result<()> {
        let from_res = Resolution::parse(&from_bytes, ReadOptions::default())?;
        let to_res = Resolution::parse(&to_bytes, ReadOptions::default())?;
        compare(&from_res, &to_res);
        Ok(())
    })
    .await??;
    Ok(())
}

fn find<'a, 'b>(res: &'a Resolution<'b>, fqn: &str) -> &'a TypeDefinition<'b> {
    for (idx, td) in res.enumerate_type_definitions() {
        let name = match &td.namespace {
            Some(ns) if !ns.is_empty() => format!("{}.{}", ns, td.name),
            _ => td.name.to_string(),
        };
        if name == fqn {
            return &res[idx];
        }
    }
    panic!("not found: {fqn}");
}

fn compare(from_res: &Resolution<'_>, to_res: &Resolution<'_>) {
    let from = find(from_res, FQN);
    let to = find(to_res, FQN);
    println!("=== {FQN} ===");

    println!("\nfields:");
    let mut from_fields: Vec<_> = from.fields.iter().collect();
    let mut to_fields: Vec<_> = to.fields.iter().collect();
    from_fields.sort_by(|a, b| a.name.cmp(&b.name));
    to_fields.sort_by(|a, b| a.name.cmp(&b.name));
    if from_fields.len() != to_fields.len() {
        println!(
            "  COUNT differs: from={} to={}",
            from_fields.len(),
            to_fields.len()
        );
    }
    for (fa, fb) in from_fields.iter().zip(to_fields.iter()) {
        let ha = field_hash(fa, from_res);
        let hb = field_hash(fb, to_res);
        let marker = if ha == hb { "  " } else { "≠≠" };
        if ha != hb {
            println!(
                "  {marker} {} ⇄ {}  {:016x} vs {:016x}",
                fa.name, fb.name, ha, hb
            );
            println!("        from type: {}", describe_field_type(fa, from_res));
            println!("        to   type: {}", describe_field_type(fb, to_res));
        }
    }

    println!("\nmethods:");
    let mut from_methods: Vec<_> = from.methods.iter().collect();
    let mut to_methods: Vec<_> = to.methods.iter().collect();
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
    if from_methods.len() != to_methods.len() {
        println!(
            "  COUNT differs: from={} to={}",
            from_methods.len(),
            to_methods.len()
        );
        let from_names: BTreeSet<_> = from_methods.iter().map(|m| m.name.as_ref()).collect();
        let to_names: BTreeSet<_> = to_methods.iter().map(|m| m.name.as_ref()).collect();
        for only_from in from_names.difference(&to_names) {
            println!("    only in from: {only_from}");
        }
        for only_to in to_names.difference(&from_names) {
            println!("    only in to:   {only_to}");
        }
    }
    for (ma, mb) in from_methods.iter().zip(to_methods.iter()) {
        let ha = method_hash(ma, from_res);
        let hb = method_hash(mb, to_res);
        if ha != hb {
            println!(
                "  ≠≠ {}({}) ⇄ {}({})  {ha:016x} vs {hb:016x}",
                ma.name,
                ma.signature.parameters.len(),
                mb.name,
                mb.signature.parameters.len()
            );
            if let (Some(ba), Some(bb)) = (&ma.body, &mb.body) {
                println!(
                    "      from-il: {} instructions, to-il: {} instructions",
                    ba.instructions.len(),
                    bb.instructions.len()
                );
                let d = similar::TextDiff::from_lines(
                    &format!("{:#?}", ba.instructions),
                    &format!("{:#?}", bb.instructions),
                )
                .unified_diff()
                .context_radius(3)
                .header("from-il", "to-il")
                .to_string();
                println!("{}", d);
            }
        }
    }
}

use dll_diff::dotnetdll::resolved::types::{BaseType, MemberType, TypeSource};
use dll_diff::sig::{field_hash, method_hash};

fn describe_field_type(
    f: &dll_diff::dotnetdll::prelude::Field<'_>,
    res: &Resolution<'_>,
) -> String {
    fn describe_member(m: &MemberType, res: &Resolution<'_>) -> String {
        match m {
            MemberType::Base(b) => describe_base(b, res),
            MemberType::TypeGeneric(n) => format!("T{n}"),
        }
    }
    fn describe_base(b: &BaseType<MemberType>, res: &Resolution<'_>) -> String {
        match b {
            BaseType::Type { source, .. } => match source {
                TypeSource::User(UserType::Definition(idx)) => {
                    let td = &res[*idx];
                    format!("def {}.{}", td.namespace.as_deref().unwrap_or(""), td.name)
                }
                TypeSource::User(UserType::Reference(idx)) => {
                    let r = &res[*idx];
                    format!("ref {}.{}", r.namespace.as_deref().unwrap_or(""), r.name)
                }
                TypeSource::Generic { base, parameters } => {
                    let b = match base {
                        UserType::Definition(idx) => res[*idx].name.to_string(),
                        UserType::Reference(idx) => res[*idx].name.to_string(),
                    };
                    let ps: Vec<_> = parameters.iter().map(|p| describe_member(p, res)).collect();
                    format!("{b}<{}>", ps.join(","))
                }
            },
            BaseType::Vector(_, t) => format!("{}[]", describe_member(t, res)),
            other => format!("{other:?}"),
        }
    }
    describe_member(&f.return_type, res)
}
