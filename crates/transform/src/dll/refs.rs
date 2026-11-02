// TODO(ai-review): review for style and correctness
//! Every type a decompiled type's C# source can name: base type,
//! interfaces, attributes, member signatures, locals, the IL operands of
//! its methods and the signatures of what they call, including those of
//! its nested types (ilspy renders them into the outer's source).

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, LazyLock, Mutex};

use crate::TransformError;
use dll_diff::dotnetdll::prelude::*;
use dll_diff::dotnetdll::resolved::attribute::Attribute;
use dll_diff::dotnetdll::resolved::body;
use dll_diff::dotnetdll::resolved::generic::Generic;
use dll_diff::dotnetdll::resolved::il::Instruction;
use dll_diff::dotnetdll::resolved::members::{
    BodyFormat, FieldReferenceParent, FieldSource, Method, MethodReferenceParent, MethodSource,
    UserMethod,
};
use dll_diff::dotnetdll::resolved::signature::{
    MethodSignature, Parameter, ParameterType, ReturnType,
};
use dll_diff::dotnetdll::resolved::types::{
    BaseType, CustomTypeModifier, LocalVariable, MemberType, MethodType, ResolutionScope,
    TypeImplementation, TypeSource,
};

/// A type named in a decompiled type's source.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TypeRef {
    /// `Namespace.Outer.Inner`, without generic arity suffixes.
    pub fqn: String,
    /// Number of generic parameters the type itself declares.
    pub arity: usize,
    /// Simple name of the defining assembly, `None` for the assembly itself.
    pub assembly: Option<String>,
}

/// The types named by the source of `type_name` (an ilspy-shaped FQN) and
/// its nested types, `type_name` itself excluded.
pub fn type_refs(dll_bytes: &[u8], type_name: &str) -> Result<Vec<TypeRef>, TransformError> {
    let res = parse(dll_bytes)?;
    let root = res
        .enumerate_type_definitions()
        .map(|(idx, _)| idx)
        .find(|&idx| def_fqn(idx, &res) == type_name)
        .ok_or_else(|| TransformError::Other(format!("type not in assembly: {type_name}")))?;

    let mut c = Collector {
        res: &res,
        out: BTreeSet::new(),
    };
    let mut pending = vec![root];
    while let Some(idx) = pending.pop() {
        c.type_def(idx)?;
        pending.extend(
            res.enumerate_type_definitions()
                .filter(|(_, td)| td.encloser == Some(idx))
                .map(|(nested, _)| nested),
        );
    }
    c.out
        .retain(|r| !(r.fqn == type_name && r.assembly.is_none()));
    Ok(c.out.into_iter().collect())
}

/// Type forwarders of an assembly (a facade like `netstandard`):
/// FQN → simple name of the assembly that now defines the type.
fn forwarded_types(dll_bytes: &[u8]) -> Result<HashMap<String, String>, TransformError> {
    let res = parse(dll_bytes)?;
    let mut out = HashMap::new();
    for (idx, _) in res.enumerate_exported_types() {
        if let Some((fqn, assembly)) = forwarded(idx, &res) {
            out.insert(fqn, assembly);
        }
    }
    Ok(out)
}

type Forwarders = Arc<HashMap<String, String>>;

static FORWARDERS: LazyLock<Mutex<HashMap<[u8; 20], Forwarders>>> = LazyLock::new(Default::default);

/// [`forwarded_types`] of the DLL with sha `dll_sha`, if already computed.
pub fn cached_forwarded_types(dll_sha: &[u8; 20]) -> Option<Forwarders> {
    FORWARDERS
        .lock()
        .expect("forwarders poisoned")
        .get(dll_sha)
        .cloned()
}

/// [`forwarded_types`], memoized by `dll_sha` for the process lifetime.
pub fn forwarded_types_memoized(
    dll_sha: [u8; 20],
    dll_bytes: &[u8],
) -> Result<Forwarders, TransformError> {
    if let Some(cached) = cached_forwarded_types(&dll_sha) {
        return Ok(cached);
    }
    let forwarders = Arc::new(forwarded_types(dll_bytes)?);
    FORWARDERS
        .lock()
        .expect("forwarders poisoned")
        .insert(dll_sha, Arc::clone(&forwarders));
    Ok(forwarders)
}

/// Walks a nested exported type up to the top-level one that carries
/// the forwarder.
fn forwarded(idx: ExportedTypeIndex, res: &Resolution<'_>) -> Option<(String, String)> {
    let et = &res[idx];
    match et.implementation {
        TypeImplementation::TypeForwarder(a) => Some((
            qualify(et.namespace.as_deref(), &et.name),
            res[a].name.to_string(),
        )),
        TypeImplementation::Nested(outer) => {
            let (outer_fqn, assembly) = forwarded(outer, res)?;
            Some((format!("{outer_fqn}.{}", strip_arity(&et.name)), assembly))
        }
        TypeImplementation::ModuleFile { .. } => None,
    }
}

/// Bodies and signatures decode on first access, so a walk pays only for
/// the members it visits.
fn parse(dll_bytes: &[u8]) -> Result<Resolution<'_>, TransformError> {
    let opts = ReadOptions {
        lazy_method_bodies: true,
        lazy_method_signatures: true,
        lazy_property_signatures: true,
        ..ReadOptions::default()
    };
    Resolution::parse(dll_bytes, opts).map_err(dll_err)
}

fn dll_err(e: impl std::fmt::Display) -> TransformError {
    TransformError::Other(format!("dotnetdll: {e}"))
}

/// FQNs follow `ilspycmd -l`: `Outer.Inner` for nested types, no generic
/// arity suffix (`` List`1 `` → `List`), empty namespaces count as none.
fn def_fqn(idx: TypeIndex, res: &Resolution<'_>) -> String {
    let td = &res[idx];
    match td.encloser {
        Some(outer) => format!("{}.{}", def_fqn(outer, res), strip_arity(&td.name)),
        None => qualify(td.namespace.as_deref(), &td.name),
    }
}

fn qualify(namespace: Option<&str>, name: &str) -> String {
    let name = strip_arity(name);
    match namespace {
        Some(ns) if !ns.is_empty() => format!("{ns}.{name}"),
        _ => name.to_string(),
    }
}

fn strip_arity(name: &str) -> &str {
    name.split_once('`').map_or(name, |(name, _)| name)
}

fn arity(name: &str) -> usize {
    name.split_once('`')
        .and_then(|(_, n)| n.parse().ok())
        .unwrap_or(0)
}

/// `None` for an exported-type scope, which doesn't name the target assembly.
fn ref_type(idx: TypeRefIndex, res: &Resolution<'_>) -> Option<TypeRef> {
    let tr = &res[idx];
    let top_level = |assembly| TypeRef {
        fqn: qualify(tr.namespace.as_deref(), &tr.name),
        arity: arity(&tr.name),
        assembly,
    };
    match tr.scope {
        ResolutionScope::Nested(outer) => {
            let outer = ref_type(outer, res)?;
            Some(TypeRef {
                fqn: format!("{}.{}", outer.fqn, strip_arity(&tr.name)),
                arity: arity(&tr.name),
                assembly: outer.assembly,
            })
        }
        ResolutionScope::Assembly(a) => Some(top_level(Some(res[a].name.to_string()))),
        ResolutionScope::CurrentModule | ResolutionScope::ExternalModule(_) => {
            Some(top_level(None))
        }
        ResolutionScope::Exported => None,
    }
}

/// Whether the method has an IL body (non-zero RVA); `method_body`
/// reports a missing one as an error.
fn has_body(m: &Method<'_>) -> bool {
    !m.abstract_member && m.body_format == BodyFormat::IL && !m.internal_call && m.pinvoke.is_none()
}

struct Collector<'r, 'a> {
    res: &'r Resolution<'a>,
    out: BTreeSet<TypeRef>,
}

impl Collector<'_, '_> {
    /// Also adds the types enclosing a nested one: the source qualifies
    /// `Outer.Inner` even where only `Inner` is referenced.
    fn add(&mut self, v: &impl Visit) {
        let res = self.res;
        let out = &mut self.out;
        v.visit(&mut |u| match *u {
            UserType::Definition(idx) => {
                let mut next = Some(idx);
                while let Some(idx) = next {
                    out.insert(TypeRef {
                        fqn: def_fqn(idx, res),
                        arity: arity(&res[idx].name),
                        assembly: None,
                    });
                    next = res[idx].encloser;
                }
            }
            UserType::Reference(idx) => {
                let mut next = Some(idx);
                while let Some(idx) = next {
                    out.extend(ref_type(idx, res));
                    next = match res[idx].scope {
                        ResolutionScope::Nested(outer) => Some(outer),
                        _ => None,
                    };
                }
            }
        });
    }

    fn type_def(&mut self, idx: TypeIndex) -> Result<(), TransformError> {
        let res = self.res;
        let td = &res[idx];
        self.attributes(&td.attributes)?;
        if let Some(ext) = &td.extends {
            self.add(ext);
        }
        for (attrs, t) in &td.implements {
            self.attributes(attrs)?;
            self.add(t);
        }
        for g in &td.generic_parameters {
            self.add(g);
        }
        for (_, f) in res.enumerate_fields(idx) {
            self.attributes(&f.attributes)?;
            self.add(&f.type_modifiers);
            self.add(&f.return_type);
        }
        for (pidx, p) in res.enumerate_properties(idx) {
            self.attributes(&p.attributes)?;
            let (_, ty, params) = res.property_signature(pidx).map_err(dll_err)?;
            self.add(ty);
            for param in params {
                self.add(param);
            }
            let others = (0..p.other.len()).map(|i| res.property_other_index(pidx, i));
            let accessors = [
                res.property_getter_index(pidx),
                res.property_setter_index(pidx),
            ];
            for midx in accessors.into_iter().chain(others).flatten() {
                self.method(midx)?;
            }
        }
        for (eidx, e) in res.enumerate_events(idx) {
            self.attributes(&e.attributes)?;
            self.add(&e.delegate_type);
            self.method(res.event_add_index(eidx))?;
            self.method(res.event_remove_index(eidx))?;
        }
        for (midx, _) in res.enumerate_methods(idx) {
            self.method(midx)?;
        }
        Ok(())
    }

    fn method(&mut self, idx: MethodIndex) -> Result<(), TransformError> {
        let res = self.res;
        let m = &res[idx];
        self.attributes(&m.attributes)?;
        self.add(res.method_signature(idx).map_err(dll_err)?);
        for g in &m.generic_parameters {
            self.add(g);
        }
        if has_body(m) {
            self.body(res.method_body(idx).map_err(dll_err)?)?;
        }
        Ok(())
    }

    fn body(&mut self, b: &body::Method) -> Result<(), TransformError> {
        for lv in &b.header.local_variables {
            if let LocalVariable::Variable {
                custom_modifiers,
                var_type,
                ..
            } = lv
            {
                self.add(custom_modifiers);
                self.add(var_type);
            }
        }
        for ds in &b.data_sections {
            if let body::DataSection::ExceptionHandlers(handlers) = ds {
                for ex in handlers {
                    if let body::ExceptionKind::TypedException(t) = &ex.kind {
                        self.add(t);
                    }
                }
            }
        }
        for ins in &b.instructions {
            self.instruction(ins)?;
        }
        Ok(())
    }

    fn instruction(&mut self, ins: &Instruction) -> Result<(), TransformError> {
        use Instruction::*;
        match ins {
            Call { param0: ms, .. }
            | CallVirtual { param0: ms, .. }
            | CallVirtualTail(ms)
            | Jump(ms)
            | LoadMethodPointer(ms)
            | LoadVirtualMethodPointer { param0: ms, .. }
            | LoadTokenMethod(ms) => self.method_source(ms)?,
            CallConstrained(t, ms) | CallVirtualConstrained(t, ms) => {
                self.add(t);
                self.method_source(ms)?;
            }
            CallIndirect { param0, .. } => self.add(param0),
            NewObject(um) => self.user_method(um)?,
            LoadField { param0: fs, .. }
            | LoadFieldAddress(fs)
            | LoadFieldSkipNullCheck(fs)
            | LoadStaticField { param0: fs, .. }
            | LoadStaticFieldAddress(fs)
            | StoreField { param0: fs, .. }
            | StoreFieldSkipNullCheck(fs)
            | StoreStaticField { param0: fs, .. }
            | LoadTokenField(fs) => self.field_source(fs),
            BoxValue(t)
            | CastClass { param0: t, .. }
            | CopyObject(t)
            | InitializeForObject(t)
            | IsInstance(t)
            | LoadElement { param0: t, .. }
            | LoadElementAddress { param0: t, .. }
            | LoadElementAddressReadonly(t)
            | LoadObject { param0: t, .. }
            | MakeTypedReference(t)
            | NewArray(t)
            | ReadTypedReferenceValue(t)
            | Sizeof(t)
            | StoreElement { param0: t, .. }
            | StoreObject { param0: t, .. }
            | UnboxIntoAddress { param0: t, .. }
            | UnboxIntoValue(t)
            | LoadTokenType(t) => self.add(t),
            _ => {}
        }
        Ok(())
    }

    fn method_source(&mut self, ms: &MethodSource) -> Result<(), TransformError> {
        match ms {
            MethodSource::User(um) => self.user_method(um),
            MethodSource::Generic(g) => {
                for p in &g.parameters {
                    self.add(p);
                }
                self.user_method(&g.base)
            }
        }
    }

    /// The callee's declaring type and signature: ilspy names enum
    /// constants and the types of elided locals after the callee's
    /// parameter and return types.
    fn user_method(&mut self, um: &UserMethod) -> Result<(), TransformError> {
        let res = self.res;
        match um {
            UserMethod::Definition(mi) => {
                self.add(&UserType::Definition(mi.parent_type()));
                self.add(res.method_signature(*mi).map_err(dll_err)?);
            }
            UserMethod::Reference(mri) => {
                match &res[*mri].parent {
                    MethodReferenceParent::Type(t) => self.add(t),
                    MethodReferenceParent::VarargMethod(mi) => {
                        self.add(&UserType::Definition(mi.parent_type()))
                    }
                    MethodReferenceParent::Module(_) => {}
                }
                self.add(res.method_ref_signature(*mri).map_err(dll_err)?);
            }
        }
        Ok(())
    }

    fn field_source(&mut self, fs: &FieldSource) {
        let res = self.res;
        match fs {
            FieldSource::Definition(fi) => {
                self.add(&UserType::Definition(fi.parent_type()));
                self.add(&res[*fi].return_type);
            }
            FieldSource::Reference(fri) => {
                let field = &res[*fri];
                if let FieldReferenceParent::Type(t) = &field.parent {
                    self.add(t);
                }
                self.add(&field.field_type);
            }
        }
    }

    fn attributes(&mut self, attrs: &[Attribute<'_>]) -> Result<(), TransformError> {
        for a in attrs {
            self.user_method(&a.constructor)?;
        }
        Ok(())
    }
}

/// Reports every [`UserType`] a signature fragment names.
trait Visit {
    fn visit(&self, f: &mut dyn FnMut(&UserType));
}

impl Visit for UserType {
    fn visit(&self, f: &mut dyn FnMut(&UserType)) {
        f(self);
    }
}

impl Visit for MemberType {
    fn visit(&self, f: &mut dyn FnMut(&UserType)) {
        if let MemberType::Base(b) = self {
            b.visit(f);
        }
    }
}

impl Visit for MethodType {
    fn visit(&self, f: &mut dyn FnMut(&UserType)) {
        if let MethodType::Base(b) = self {
            b.visit(f);
        }
    }
}

impl<T: Visit> Visit for BaseType<T> {
    fn visit(&self, f: &mut dyn FnMut(&UserType)) {
        match self {
            BaseType::Type { source, .. } => source.visit(f),
            BaseType::Vector(mods, t) => {
                mods.visit(f);
                t.visit(f);
            }
            BaseType::Array(t, _) => t.visit(f),
            BaseType::ValuePointer(mods, t) => {
                mods.visit(f);
                if let Some(t) = t {
                    t.visit(f);
                }
            }
            BaseType::FunctionPointer(sig) => sig.visit(f),
            _ => {}
        }
    }
}

impl<T: Visit> Visit for TypeSource<T> {
    fn visit(&self, f: &mut dyn FnMut(&UserType)) {
        match self {
            TypeSource::User(u) => f(u),
            TypeSource::Generic { base, parameters } => {
                f(base);
                for p in parameters {
                    p.visit(f);
                }
            }
        }
    }
}

impl<C, T: Visit> Visit for MethodSignature<C, T> {
    fn visit(&self, f: &mut dyn FnMut(&UserType)) {
        self.return_type.visit(f);
        for p in &self.parameters {
            p.visit(f);
        }
    }
}

impl<T: Visit> Visit for ReturnType<T> {
    fn visit(&self, f: &mut dyn FnMut(&UserType)) {
        self.0.visit(f);
        if let Some(t) = &self.1 {
            t.visit(f);
        }
    }
}

impl<T: Visit> Visit for Parameter<T> {
    fn visit(&self, f: &mut dyn FnMut(&UserType)) {
        self.0.visit(f);
        self.1.visit(f);
    }
}

impl<T: Visit> Visit for ParameterType<T> {
    fn visit(&self, f: &mut dyn FnMut(&UserType)) {
        if let ParameterType::Value(t) | ParameterType::Ref(t) = self {
            t.visit(f);
        }
    }
}

impl<T: Visit> Visit for Generic<'_, T> {
    fn visit(&self, f: &mut dyn FnMut(&UserType)) {
        for c in &self.type_constraints {
            c.custom_modifiers.visit(f);
            c.constraint_type.visit(f);
        }
    }
}

impl Visit for Vec<CustomTypeModifier> {
    fn visit(&self, f: &mut dyn FnMut(&UserType)) {
        for m in self {
            match m {
                CustomTypeModifier::Required(u) | CustomTypeModifier::Optional(u) => f(u),
            }
        }
    }
}
