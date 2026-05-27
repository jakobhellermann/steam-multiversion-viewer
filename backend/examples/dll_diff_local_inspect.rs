// TODO(ai-review): review for style and correctness
//! Diagnostic for a fixture pair on disk (no Steam fetch). Pick two
//! DLLs + an FQN, prints which fields/methods of that type hash
//! differently across the pair using the same per-piece helpers the
//! production walker exposes.

use std::path::PathBuf;

use anyhow::Result;
use dll_diff::dotnetdll::prelude::*;
use dll_diff::dotnetdll::resolved::types::{BaseType, MemberType, TypeDefinition, TypeSource};
use dll_diff::sig::{field_hash, method_hash};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        eprintln!("usage: dll_diff_local_inspect FROM.dll TO.dll FQN");
        std::process::exit(2);
    }
    let from = std::fs::read(PathBuf::from(&args[0]))?;
    let to = std::fs::read(PathBuf::from(&args[1]))?;
    let fqn = &args[2];

    let from_res = Resolution::parse(&from, ReadOptions::default())?;
    let to_res = Resolution::parse(&to, ReadOptions::default())?;
    compare(&from_res, &to_res, fqn);
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

fn compare(from_res: &Resolution<'_>, to_res: &Resolution<'_>, fqn: &str) {
    let from = find(from_res, fqn);
    let to = find(to_res, fqn);
    println!("=== {fqn} ===");

    println!("\nfields:");
    let mut from_fields: Vec<_> = from.fields.iter().collect();
    let mut to_fields: Vec<_> = to.fields.iter().collect();
    from_fields.sort_by(|a, b| a.name.cmp(&b.name));
    to_fields.sort_by(|a, b| a.name.cmp(&b.name));
    for (fa, fb) in from_fields.iter().zip(to_fields.iter()) {
        let ha = field_hash(fa, from_res);
        let hb = field_hash(fb, to_res);
        if ha != hb {
            println!(
                "  ≠≠ {} ⇄ {}  {ha:016x} vs {hb:016x}\n        from: {}\n        to:   {}",
                fa.name,
                fb.name,
                describe_member(&fa.return_type, from_res),
                describe_member(&fb.return_type, to_res)
            );
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
                let from_str = format!("{:#?}", ba.instructions);
                let to_str = format!("{:#?}", bb.instructions);
                let d = similar::TextDiff::from_lines(&from_str, &to_str)
                    .unified_diff()
                    .context_radius(3)
                    .header("from-il", "to-il")
                    .to_string();
                println!("{}", d);
            }
        }
    }
}

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
            TypeSource::Generic { base, .. } => match base {
                UserType::Definition(idx) => format!("def-gen {}", res[*idx].name),
                UserType::Reference(idx) => format!("ref-gen {}", res[*idx].name),
            },
        },
        BaseType::Vector(_, t) => format!("{}[]", describe_member(t, res)),
        other => format!("{other:?}"),
    }
}
