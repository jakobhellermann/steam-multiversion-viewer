// TODO(ai-review): review for style and correctness
//! Resolution-aware content hash for a [`TypeDefinition`]. Walks the
//! resolved metadata tree manually, resolving every
//! `TypeIndex`/`TypeRefIndex`/`MethodIndex`/`FieldIndex` to a stable
//! identifier (a name + namespace + scope, never a numeric position)
//! before feeding it into the hasher. The auto-derived
//! `Hash::hash(td)` would otherwise pull in raw table positions and
//! produce different hashes for byte-identical types whenever an
//! unrelated entry shifts the metadata tables — the cascading-index
//! false-positive class.

use std::hash::{Hash, Hasher};

use dotnetdll::prelude::*;
use dotnetdll::resolved::body;
use dotnetdll::resolved::generic::Generic;
use dotnetdll::resolved::il::Instruction;
use dotnetdll::resolved::members::{
    Event, ExternalFieldReference, ExternalMethodReference, Field, FieldReferenceParent,
    FieldSource, GenericMethodInstantiation, Method, MethodReferenceParent, MethodSource, Property,
    UserMethod,
};
use dotnetdll::resolved::signature::{Parameter, ParameterType, ReturnType};
use dotnetdll::resolved::types::{
    BaseType, CustomTypeModifier, ExternalTypeReference, LocalVariable, MemberType, MethodType,
    ResolutionScope, TypeDefinition, TypeSource,
};

/// Hash of `td` that's stable across DLL parses where unrelated
/// types have shifted metadata-table positions.
pub fn hash_type(td: &TypeDefinition<'_>, res: &Resolution<'_>) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    hash_type_inline(td, res, &mut h);
    h.finish()
}

fn hash_type_inline<H: Hasher>(td: &TypeDefinition<'_>, res: &Resolution<'_>, h: &mut H) {
    td.name.hash(h);
    td.namespace.hash(h);
    td.flags.hash(h);
    if let Some(enc) = td.encloser {
        h.write_u8(1);
        hash_type_def_identity(enc, res, h);
    } else {
        h.write_u8(0);
    }
    if let Some(ext) = &td.extends {
        h.write_u8(1);
        hash_type_source_member(ext, res, h);
    } else {
        h.write_u8(0);
    }
    h.write_usize(td.implements.len());
    for (_attrs, t) in &td.implements {
        hash_type_source_member(t, res, h);
    }
    h.write_usize(td.generic_parameters.len());
    for g in &td.generic_parameters {
        hash_generic_param_member(g, res, h);
    }

    let mut fields: Vec<&Field> = td.fields.iter().collect();
    fields.sort_by(|a, b| a.name.cmp(&b.name));
    h.write_usize(fields.len());
    for f in fields {
        hash_field(f, res, h);
    }

    let mut props: Vec<&Property> = td.properties.iter().collect();
    props.sort_by(|a, b| a.name.cmp(&b.name));
    h.write_usize(props.len());
    for p in props {
        hash_property(p, res, h);
    }

    let mut events: Vec<&Event> = td.events.iter().collect();
    events.sort_by(|a, b| a.name.cmp(&b.name));
    h.write_usize(events.len());
    for e in events {
        hash_event(e, res, h);
    }

    let mut methods: Vec<&Method> = td.methods.iter().collect();
    methods.sort_by(|a, b| {
        a.name.cmp(&b.name).then_with(|| {
            a.signature
                .parameters
                .len()
                .cmp(&b.signature.parameters.len())
        })
    });
    h.write_usize(methods.len());
    for m in methods {
        hash_method(m, res, h);
    }

    h.write_usize(td.overrides.len());
    for o in &td.overrides {
        hash_user_method(&o.implementation, res, h);
        hash_user_method(&o.declaration, res, h);
    }

    // Attributes + security skipped in v1.
    let _ = (&td.attributes, &td.security);
}

// ---- identity resolution ---------------------------------------------------

fn hash_type_def_identity<H: Hasher>(idx: TypeIndex, res: &Resolution<'_>, h: &mut H) {
    let td = &res[idx];
    td.namespace.hash(h);
    td.name.hash(h);
    if let Some(outer) = td.encloser {
        h.write_u8(1);
        hash_type_def_identity(outer, res, h);
    } else {
        h.write_u8(0);
    }
}

fn hash_type_ref_identity<H: Hasher>(idx: TypeRefIndex, res: &Resolution<'_>, h: &mut H) {
    hash_external_type_ref(&res[idx], res, h);
}

fn hash_external_type_ref<H: Hasher>(
    tr: &ExternalTypeReference<'_>,
    res: &Resolution<'_>,
    h: &mut H,
) {
    tr.namespace.hash(h);
    tr.name.hash(h);
    match tr.scope {
        ResolutionScope::Nested(enc) => {
            h.write_u8(0);
            hash_type_ref_identity(enc, res, h);
        }
        ResolutionScope::ExternalModule(m) => {
            h.write_u8(1);
            res[m].name.hash(h);
        }
        ResolutionScope::CurrentModule => h.write_u8(2),
        ResolutionScope::Assembly(a) => {
            h.write_u8(3);
            res[a].name.hash(h);
        }
        ResolutionScope::Exported => h.write_u8(4),
    }
}

fn hash_user_type<H: Hasher>(u: &UserType, res: &Resolution<'_>, h: &mut H) {
    match u {
        UserType::Definition(idx) => {
            h.write_u8(0);
            hash_type_def_identity(*idx, res, h);
        }
        UserType::Reference(idx) => {
            h.write_u8(1);
            hash_type_ref_identity(*idx, res, h);
        }
    }
}

// ---- type kinds (member vs method generics differ, so two flavours) --------

fn hash_member_type<H: Hasher>(t: &MemberType, res: &Resolution<'_>, h: &mut H) {
    match t {
        MemberType::Base(b) => {
            h.write_u8(0);
            hash_base_type_member(b, res, h);
        }
        MemberType::TypeGeneric(n) => {
            h.write_u8(1);
            n.hash(h);
        }
    }
}

fn hash_method_type<H: Hasher>(t: &MethodType, res: &Resolution<'_>, h: &mut H) {
    match t {
        MethodType::Base(b) => {
            h.write_u8(0);
            hash_base_type_method(b, res, h);
        }
        MethodType::TypeGeneric(n) => {
            h.write_u8(1);
            n.hash(h);
        }
        MethodType::MethodGeneric(n) => {
            h.write_u8(2);
            n.hash(h);
        }
    }
}

fn hash_base_type_member<H: Hasher>(b: &BaseType<MemberType>, res: &Resolution<'_>, h: &mut H) {
    match b {
        BaseType::Type { value_kind, source } => {
            h.write_u8(0);
            value_kind.hash(h);
            hash_type_source_member(source, res, h);
        }
        BaseType::Vector(mods, t) => {
            h.write_u8(17);
            hash_custom_modifiers(mods, res, h);
            hash_member_type(t, res, h);
        }
        BaseType::Array(t, shape) => {
            h.write_u8(18);
            hash_member_type(t, res, h);
            shape.hash(h);
        }
        BaseType::ValuePointer(mods, t) => {
            h.write_u8(19);
            hash_custom_modifiers(mods, res, h);
            match t {
                Some(t) => {
                    h.write_u8(1);
                    hash_member_type(t, res, h);
                }
                None => h.write_u8(0),
            }
        }
        BaseType::FunctionPointer(sig) => {
            h.write_u8(20);
            sig.instance.hash(h);
            sig.explicit_this.hash(h);
            sig.calling_convention.hash(h);
            hash_return_type_member(&sig.return_type, res, h);
            h.write_usize(sig.parameters.len());
            for p in &sig.parameters {
                hash_parameter_member(p, res, h);
            }
        }
        primitive => primitive_tag(primitive).hash(h),
    }
}

fn hash_return_type_member<H: Hasher>(r: &ReturnType<MemberType>, res: &Resolution<'_>, h: &mut H) {
    hash_custom_modifiers(&r.0, res, h);
    match &r.1 {
        None => h.write_u8(0),
        Some(ParameterType::Value(v)) => {
            h.write_u8(1);
            hash_member_type(v, res, h);
        }
        Some(ParameterType::Ref(r)) => {
            h.write_u8(2);
            hash_member_type(r, res, h);
        }
        Some(ParameterType::TypedReference) => h.write_u8(3),
    }
}

fn hash_base_type_method<H: Hasher>(b: &BaseType<MethodType>, res: &Resolution<'_>, h: &mut H) {
    match b {
        BaseType::Type { value_kind, source } => {
            h.write_u8(0);
            value_kind.hash(h);
            hash_type_source_method(source, res, h);
        }
        BaseType::Vector(mods, t) => {
            h.write_u8(17);
            hash_custom_modifiers(mods, res, h);
            hash_method_type(t, res, h);
        }
        BaseType::Array(t, shape) => {
            h.write_u8(18);
            hash_method_type(t, res, h);
            shape.hash(h);
        }
        BaseType::ValuePointer(mods, t) => {
            h.write_u8(19);
            hash_custom_modifiers(mods, res, h);
            match t {
                Some(t) => {
                    h.write_u8(1);
                    hash_method_type(t, res, h);
                }
                None => h.write_u8(0),
            }
        }
        BaseType::FunctionPointer(sig) => {
            h.write_u8(20);
            sig.instance.hash(h);
            sig.explicit_this.hash(h);
            sig.calling_convention.hash(h);
            hash_return_type_method(&sig.return_type, res, h);
            h.write_usize(sig.parameters.len());
            for p in &sig.parameters {
                hash_parameter_method(p, res, h);
            }
        }
        primitive => primitive_tag(primitive).hash(h),
    }
}

fn primitive_tag<T>(b: &BaseType<T>) -> u8 {
    match b {
        BaseType::Boolean => 1,
        BaseType::Char => 2,
        BaseType::Int8 => 3,
        BaseType::UInt8 => 4,
        BaseType::Int16 => 5,
        BaseType::UInt16 => 6,
        BaseType::Int32 => 7,
        BaseType::UInt32 => 8,
        BaseType::Int64 => 9,
        BaseType::UInt64 => 10,
        BaseType::Float32 => 11,
        BaseType::Float64 => 12,
        BaseType::IntPtr => 13,
        BaseType::UIntPtr => 14,
        BaseType::Object => 15,
        BaseType::String => 16,
        _ => 0,
    }
}

fn hash_type_source_member<H: Hasher>(s: &TypeSource<MemberType>, res: &Resolution<'_>, h: &mut H) {
    match s {
        TypeSource::User(u) => {
            h.write_u8(0);
            hash_user_type(u, res, h);
        }
        TypeSource::Generic { base, parameters } => {
            h.write_u8(1);
            hash_user_type(base, res, h);
            h.write_usize(parameters.len());
            for p in parameters {
                hash_member_type(p, res, h);
            }
        }
    }
}

fn hash_type_source_method<H: Hasher>(s: &TypeSource<MethodType>, res: &Resolution<'_>, h: &mut H) {
    match s {
        TypeSource::User(u) => {
            h.write_u8(0);
            hash_user_type(u, res, h);
        }
        TypeSource::Generic { base, parameters } => {
            h.write_u8(1);
            hash_user_type(base, res, h);
            h.write_usize(parameters.len());
            for p in parameters {
                hash_method_type(p, res, h);
            }
        }
    }
}

fn hash_custom_modifiers<H: Hasher>(mods: &[CustomTypeModifier], res: &Resolution<'_>, h: &mut H) {
    h.write_usize(mods.len());
    for m in mods {
        match m {
            CustomTypeModifier::Required(u) => {
                h.write_u8(0);
                hash_user_type(u, res, h);
            }
            CustomTypeModifier::Optional(u) => {
                h.write_u8(1);
                hash_user_type(u, res, h);
            }
        }
    }
}

// ---- members ---------------------------------------------------------------

fn hash_field<H: Hasher>(f: &Field<'_>, res: &Resolution<'_>, h: &mut H) {
    f.name.hash(h);
    f.accessibility.hash(h);
    f.static_member.hash(h);
    f.init_only.hash(h);
    f.literal.hash(h);
    f.not_serialized.hash(h);
    f.special_name.hash(h);
    f.runtime_special_name.hash(h);
    f.by_ref.hash(h);
    hash_custom_modifiers(&f.type_modifiers, res, h);
    hash_member_type(&f.return_type, res, h);
    f.default.hash(h);
    f.pinvoke.hash(h);
}

fn hash_property<H: Hasher>(p: &Property<'_>, res: &Resolution<'_>, h: &mut H) {
    p.name.hash(h);
    p.static_member.hash(h);
    hash_parameter_member(&p.property_type, res, h);
    match &p.getter {
        Some(m) => {
            h.write_u8(1);
            hash_method(m, res, h);
        }
        None => h.write_u8(0),
    }
    match &p.setter {
        Some(m) => {
            h.write_u8(1);
            hash_method(m, res, h);
        }
        None => h.write_u8(0),
    }
    h.write_usize(p.other.len());
    for m in &p.other {
        hash_method(m, res, h);
    }
}

fn hash_event<H: Hasher>(e: &Event<'_>, res: &Resolution<'_>, h: &mut H) {
    e.name.hash(h);
    hash_member_type(&e.delegate_type, res, h);
    hash_method(&e.add_listener, res, h);
    hash_method(&e.remove_listener, res, h);
    if let Some(raise) = &e.raise_event {
        h.write_u8(1);
        hash_method(raise, res, h);
    } else {
        h.write_u8(0);
    }
    h.write_usize(e.other.len());
    for m in &e.other {
        hash_method(m, res, h);
    }
}

fn hash_method<H: Hasher>(m: &Method<'_>, res: &Resolution<'_>, h: &mut H) {
    m.name.hash(h);
    m.accessibility.hash(h);
    m.sealed.hash(h);
    m.virtual_member.hash(h);
    m.hide_by_sig.hash(h);
    m.vtable_layout.hash(h);
    m.strict.hash(h);
    m.abstract_member.hash(h);
    m.special_name.hash(h);
    m.runtime_special_name.hash(h);
    m.require_sec_object.hash(h);
    m.body_format.hash(h);
    m.body_management.hash(h);
    m.forward_ref.hash(h);
    m.preserve_sig.hash(h);
    m.internal_call.hash(h);
    m.synchronized.hash(h);
    m.no_inlining.hash(h);
    m.no_optimization.hash(h);

    let sig = &m.signature;
    sig.instance.hash(h);
    sig.explicit_this.hash(h);
    sig.calling_convention.hash(h);
    hash_return_type_method(&sig.return_type, res, h);
    h.write_usize(sig.parameters.len());
    for p in &sig.parameters {
        hash_parameter_method(p, res, h);
    }

    h.write_usize(m.generic_parameters.len());
    for g in &m.generic_parameters {
        hash_generic_param_method(g, res, h);
    }

    match &m.body {
        Some(body) => {
            h.write_u8(1);
            hash_method_body(body, res, h);
        }
        None => h.write_u8(0),
    }

    m.pinvoke.hash(h);
}

fn hash_method_body<H: Hasher>(b: &body::Method, res: &Resolution<'_>, h: &mut H) {
    b.header.initialize_locals.hash(h);
    b.header.maximum_stack_size.hash(h);
    h.write_usize(b.header.local_variables.len());
    for lv in &b.header.local_variables {
        match lv {
            LocalVariable::TypedReference => h.write_u8(0),
            LocalVariable::Variable {
                custom_modifiers,
                pinned,
                by_ref,
                var_type,
            } => {
                h.write_u8(1);
                hash_custom_modifiers(custom_modifiers, res, h);
                pinned.hash(h);
                by_ref.hash(h);
                hash_method_type(var_type, res, h);
            }
        }
    }
    h.write_usize(b.instructions.len());
    for ins in &b.instructions {
        hash_instruction(ins, res, h);
    }
    h.write_usize(b.data_sections.len());
    for ds in &b.data_sections {
        match ds {
            body::DataSection::Unrecognized { fat, size } => {
                h.write_u8(0);
                fat.hash(h);
                size.hash(h);
            }
            body::DataSection::ExceptionHandlers(handlers) => {
                h.write_u8(1);
                h.write_usize(handlers.len());
                for ex in handlers {
                    ex.try_offset.hash(h);
                    ex.try_length.hash(h);
                    ex.handler_offset.hash(h);
                    ex.handler_length.hash(h);
                    match &ex.kind {
                        body::ExceptionKind::TypedException(t) => {
                            h.write_u8(0);
                            hash_method_type(t, res, h);
                        }
                        body::ExceptionKind::Filter { offset } => {
                            h.write_u8(1);
                            offset.hash(h);
                        }
                        body::ExceptionKind::Finally => h.write_u8(2),
                        body::ExceptionKind::Fault => h.write_u8(3),
                    }
                }
            }
        }
    }
}

fn hash_parameter_member<H: Hasher>(p: &Parameter<MemberType>, res: &Resolution<'_>, h: &mut H) {
    hash_custom_modifiers(&p.0, res, h);
    match &p.1 {
        ParameterType::Value(v) => {
            h.write_u8(0);
            hash_member_type(v, res, h);
        }
        ParameterType::Ref(r) => {
            h.write_u8(1);
            hash_member_type(r, res, h);
        }
        ParameterType::TypedReference => h.write_u8(2),
    }
}

fn hash_parameter_method<H: Hasher>(p: &Parameter<MethodType>, res: &Resolution<'_>, h: &mut H) {
    hash_custom_modifiers(&p.0, res, h);
    match &p.1 {
        ParameterType::Value(v) => {
            h.write_u8(0);
            hash_method_type(v, res, h);
        }
        ParameterType::Ref(r) => {
            h.write_u8(1);
            hash_method_type(r, res, h);
        }
        ParameterType::TypedReference => h.write_u8(2),
    }
}

fn hash_return_type_method<H: Hasher>(r: &ReturnType<MethodType>, res: &Resolution<'_>, h: &mut H) {
    hash_custom_modifiers(&r.0, res, h);
    match &r.1 {
        None => h.write_u8(0),
        Some(ParameterType::Value(v)) => {
            h.write_u8(1);
            hash_method_type(v, res, h);
        }
        Some(ParameterType::Ref(r)) => {
            h.write_u8(2);
            hash_method_type(r, res, h);
        }
        Some(ParameterType::TypedReference) => h.write_u8(3),
    }
}

fn hash_generic_param_member<H: Hasher>(
    g: &Generic<'_, MemberType>,
    res: &Resolution<'_>,
    h: &mut H,
) {
    g.name.hash(h);
    g.variance.hash(h);
    g.special_constraint.hash(h);
    h.write_usize(g.type_constraints.len());
    for c in &g.type_constraints {
        hash_custom_modifiers(&c.custom_modifiers, res, h);
        hash_member_type(&c.constraint_type, res, h);
    }
}

fn hash_generic_param_method<H: Hasher>(
    g: &Generic<'_, MethodType>,
    res: &Resolution<'_>,
    h: &mut H,
) {
    g.name.hash(h);
    g.variance.hash(h);
    g.special_constraint.hash(h);
    h.write_usize(g.type_constraints.len());
    for c in &g.type_constraints {
        hash_custom_modifiers(&c.custom_modifiers, res, h);
        hash_method_type(&c.constraint_type, res, h);
    }
}

// ---- IL --------------------------------------------------------------------

fn hash_instruction<H: Hasher>(ins: &Instruction, res: &Resolution<'_>, h: &mut H) {
    use Instruction::*;
    match ins {
        Call { tail_call, param0 } => {
            h.write_u8(100);
            tail_call.hash(h);
            hash_method_source(param0, res, h);
        }
        CallConstrained(t, ms) => {
            h.write_u8(101);
            hash_method_type(t, res, h);
            hash_method_source(ms, res, h);
        }
        CallIndirect { tail_call, param0 } => {
            h.write_u8(102);
            tail_call.hash(h);
            param0.instance.hash(h);
            param0.explicit_this.hash(h);
            param0.calling_convention.hash(h);
            hash_return_type_method(&param0.return_type, res, h);
            h.write_usize(param0.parameters.len());
            for p in &param0.parameters {
                hash_parameter_method(p, res, h);
            }
        }
        CallVirtual {
            skip_null_check,
            param0,
        } => {
            h.write_u8(103);
            skip_null_check.hash(h);
            hash_method_source(param0, res, h);
        }
        CallVirtualConstrained(t, ms) => {
            h.write_u8(104);
            hash_method_type(t, res, h);
            hash_method_source(ms, res, h);
        }
        CallVirtualTail(ms) => {
            h.write_u8(105);
            hash_method_source(ms, res, h);
        }
        Jump(ms) => {
            h.write_u8(106);
            hash_method_source(ms, res, h);
        }
        LoadMethodPointer(ms) => {
            h.write_u8(107);
            hash_method_source(ms, res, h);
        }
        LoadVirtualMethodPointer {
            skip_null_check,
            param0,
        } => {
            h.write_u8(108);
            skip_null_check.hash(h);
            hash_method_source(param0, res, h);
        }
        NewObject(um) => {
            h.write_u8(109);
            hash_user_method(um, res, h);
        }

        LoadField {
            unaligned,
            volatile,
            param0,
        } => {
            h.write_u8(120);
            unaligned.hash(h);
            volatile.hash(h);
            hash_field_source(param0, res, h);
        }
        LoadFieldAddress(fs) => {
            h.write_u8(121);
            hash_field_source(fs, res, h);
        }
        LoadFieldSkipNullCheck(fs) => {
            h.write_u8(122);
            hash_field_source(fs, res, h);
        }
        LoadStaticField { volatile, param0 } => {
            h.write_u8(123);
            volatile.hash(h);
            hash_field_source(param0, res, h);
        }
        LoadStaticFieldAddress(fs) => {
            h.write_u8(124);
            hash_field_source(fs, res, h);
        }
        StoreField {
            unaligned,
            volatile,
            param0,
        } => {
            h.write_u8(125);
            unaligned.hash(h);
            volatile.hash(h);
            hash_field_source(param0, res, h);
        }
        StoreFieldSkipNullCheck(fs) => {
            h.write_u8(126);
            hash_field_source(fs, res, h);
        }
        StoreStaticField { volatile, param0 } => {
            h.write_u8(127);
            volatile.hash(h);
            hash_field_source(param0, res, h);
        }

        BoxValue(t) => {
            h.write_u8(140);
            hash_method_type(t, res, h);
        }
        CastClass {
            skip_type_check,
            param0,
        } => {
            h.write_u8(141);
            skip_type_check.hash(h);
            hash_method_type(param0, res, h);
        }
        CopyObject(t) => {
            h.write_u8(142);
            hash_method_type(t, res, h);
        }
        InitializeForObject(t) => {
            h.write_u8(143);
            hash_method_type(t, res, h);
        }
        IsInstance(t) => {
            h.write_u8(144);
            hash_method_type(t, res, h);
        }
        LoadElement {
            skip_range_check,
            skip_null_check,
            param0,
        } => {
            h.write_u8(145);
            skip_range_check.hash(h);
            skip_null_check.hash(h);
            hash_method_type(param0, res, h);
        }
        LoadElementAddress {
            skip_type_check,
            skip_range_check,
            skip_null_check,
            param0,
        } => {
            h.write_u8(146);
            skip_type_check.hash(h);
            skip_range_check.hash(h);
            skip_null_check.hash(h);
            hash_method_type(param0, res, h);
        }
        LoadElementAddressReadonly(t) => {
            h.write_u8(147);
            hash_method_type(t, res, h);
        }
        LoadObject {
            unaligned,
            volatile,
            param0,
        } => {
            h.write_u8(148);
            unaligned.hash(h);
            volatile.hash(h);
            hash_method_type(param0, res, h);
        }
        MakeTypedReference(t) => {
            h.write_u8(149);
            hash_method_type(t, res, h);
        }
        NewArray(t) => {
            h.write_u8(150);
            hash_method_type(t, res, h);
        }
        ReadTypedReferenceValue(t) => {
            h.write_u8(151);
            hash_method_type(t, res, h);
        }
        Sizeof(t) => {
            h.write_u8(152);
            hash_method_type(t, res, h);
        }
        StoreElement {
            skip_type_check,
            skip_range_check,
            skip_null_check,
            param0,
        } => {
            h.write_u8(153);
            skip_type_check.hash(h);
            skip_range_check.hash(h);
            skip_null_check.hash(h);
            hash_method_type(param0, res, h);
        }
        StoreObject {
            unaligned,
            volatile,
            param0,
        } => {
            h.write_u8(154);
            unaligned.hash(h);
            volatile.hash(h);
            hash_method_type(param0, res, h);
        }
        UnboxIntoAddress {
            skip_type_check,
            param0,
        } => {
            h.write_u8(155);
            skip_type_check.hash(h);
            hash_method_type(param0, res, h);
        }
        UnboxIntoValue(t) => {
            h.write_u8(156);
            hash_method_type(t, res, h);
        }

        LoadTokenField(fs) => {
            h.write_u8(160);
            hash_field_source(fs, res, h);
        }
        LoadTokenMethod(ms) => {
            h.write_u8(161);
            hash_method_source(ms, res, h);
        }
        LoadTokenType(t) => {
            h.write_u8(162);
            hash_method_type(t, res, h);
        }

        // Everything else is index-free — Hash-derive is safe.
        other => {
            std::mem::discriminant(other).hash(h);
            other.hash(h);
        }
    }
}

fn hash_method_source<H: Hasher>(ms: &MethodSource, res: &Resolution<'_>, h: &mut H) {
    match ms {
        MethodSource::User(um) => {
            h.write_u8(0);
            hash_user_method(um, res, h);
        }
        MethodSource::Generic(g) => {
            h.write_u8(1);
            hash_generic_method(g, res, h);
        }
    }
}

fn hash_user_method<H: Hasher>(um: &UserMethod, res: &Resolution<'_>, h: &mut H) {
    match um {
        UserMethod::Definition(mi) => {
            h.write_u8(0);
            hash_type_def_identity(mi.parent_type(), res, h);
            let m: &Method<'_> = &res[*mi];
            m.name.hash(h);
            h.write_usize(m.signature.parameters.len());
        }
        UserMethod::Reference(mri) => {
            h.write_u8(1);
            hash_external_method_ref(&res[*mri], res, h);
        }
    }
}

fn hash_external_method_ref<H: Hasher>(
    r: &ExternalMethodReference<'_>,
    res: &Resolution<'_>,
    h: &mut H,
) {
    r.name.hash(h);
    match &r.parent {
        MethodReferenceParent::Type(t) => {
            h.write_u8(0);
            hash_method_type(t, res, h);
        }
        MethodReferenceParent::Module(m) => {
            h.write_u8(1);
            res[*m].name.hash(h);
        }
        MethodReferenceParent::VarargMethod(mi) => {
            h.write_u8(2);
            // VarargMethod points to an internal definition; resolve
            // identity via the same MethodIndex helper used for
            // `UserMethod::Definition`.
            hash_type_def_identity(mi.parent_type(), res, h);
            let m: &Method<'_> = &res[*mi];
            m.name.hash(h);
            h.write_usize(m.signature.parameters.len());
        }
    }
    let sig = &r.signature;
    sig.instance.hash(h);
    sig.explicit_this.hash(h);
    sig.calling_convention.hash(h);
    hash_return_type_method(&sig.return_type, res, h);
    h.write_usize(sig.parameters.len());
    for p in &sig.parameters {
        hash_parameter_method(p, res, h);
    }
}

fn hash_generic_method<H: Hasher>(g: &GenericMethodInstantiation, res: &Resolution<'_>, h: &mut H) {
    hash_user_method(&g.base, res, h);
    h.write_usize(g.parameters.len());
    for p in &g.parameters {
        hash_method_type(p, res, h);
    }
}

fn hash_field_source<H: Hasher>(fs: &FieldSource, res: &Resolution<'_>, h: &mut H) {
    match fs {
        FieldSource::Definition(fi) => {
            h.write_u8(0);
            hash_type_def_identity(fi.parent_type(), res, h);
            let f: &Field<'_> = &res[*fi];
            f.name.hash(h);
        }
        FieldSource::Reference(fri) => {
            h.write_u8(1);
            hash_external_field_ref(&res[*fri], res, h);
        }
    }
}

fn hash_external_field_ref<H: Hasher>(
    r: &ExternalFieldReference<'_>,
    res: &Resolution<'_>,
    h: &mut H,
) {
    r.name.hash(h);
    match &r.parent {
        FieldReferenceParent::Type(t) => {
            h.write_u8(0);
            hash_method_type(t, res, h);
        }
        FieldReferenceParent::Module(m) => {
            h.write_u8(1);
            res[*m].name.hash(h);
        }
    }
    hash_member_type(&r.field_type, res, h);
    hash_custom_modifiers(&r.custom_modifiers, res, h);
}
