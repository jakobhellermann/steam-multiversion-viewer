use rabex_env::handle::SerializedFileHandle;
use rabex_env::rabex::files::serializedfile::ObjectInfo;
use rabex_env::rabex::objects::ClassId;
use rabex_env::rabex::objects::pptr::{PPtr, PathId};
use rabex_env::rabex::typetree::TypeTreeProvider;
use rabex_env::resolver::EnvResolver;
use serde_value::Value;

use crate::structured::NodeStatus;

use super::super::markers::pptr_from_map;
use super::BodyIndex;

pub(crate) fn object_bytes<'a, R: EnvResolver, P>(
    file: &SerializedFileHandle<'a, R, P>,
    obj: &ObjectInfo,
) -> &'a [u8] {
    let start = obj.m_Offset as usize;
    let end = start + obj.m_Size as usize;
    &file.data[start..end]
}

pub(crate) fn matched_status<R: EnvResolver, P: TypeTreeProvider>(
    base_file: &SerializedFileHandle<'_, R, P>,
    target_file: &SerializedFileHandle<'_, R, P>,
    base_bodies: &BodyIndex<'_>,
    target_bodies: &BodyIndex<'_>,
    base_pid: PathId,
    target_pid: PathId,
) -> NodeStatus {
    match (base_bodies.get(&base_pid), target_bodies.get(&target_pid)) {
        (Some(bb), Some(tb)) if bb == tb => NodeStatus::Unchanged,
        (Some(bb), Some(tb))
            if bb.len() == tb.len()
                && objects_equal_modulo_pptr(base_file, base_pid, target_file, target_pid) =>
        {
            NodeStatus::Unchanged
        }
        (Some(_), Some(_))
            if crate::unity::game_specific::objects_equal(
                base_file,
                base_pid,
                target_file,
                target_pid,
            ) == Some(true) =>
        {
            NodeStatus::Unchanged
        }
        _ => NodeStatus::Changed,
    }
}

fn objects_equal_modulo_pptr<R: EnvResolver, P: TypeTreeProvider>(
    base_file: &SerializedFileHandle<'_, R, P>,
    base_pid: PathId,
    target_file: &SerializedFileHandle<'_, R, P>,
    target_pid: PathId,
) -> bool {
    let (Ok(base_handle), Ok(target_handle)) = (
        base_file.object_at::<Value>(base_pid),
        target_file.object_at::<Value>(target_pid),
    ) else {
        return false;
    };
    // HingeJoint2D.m_ConnectedAnchor.y differs by up to ~10 f32 ulps
    // between same-depot manifests with the object otherwise identical.
    let float_tolerance = base_handle.class_id() == ClassId::HingeJoint2D
        && target_handle.class_id() == ClassId::HingeJoint2D;
    let (Ok(base_val), Ok(target_val)) = (base_handle.read(), target_handle.read()) else {
        return false;
    };
    values_equal_modulo_pptr(
        base_file,
        &base_val,
        target_file,
        &target_val,
        float_tolerance,
    )
}

#[allow(clippy::too_many_arguments)]
fn values_equal_modulo_pptr<R: EnvResolver, P: TypeTreeProvider>(
    base_file: &SerializedFileHandle<'_, R, P>,
    base: &Value,
    target_file: &SerializedFileHandle<'_, R, P>,
    target: &Value,
    float_tolerance: bool,
) -> bool {
    if base == target {
        return true;
    }
    match (base, target) {
        (Value::Map(bm), Value::Map(tm)) => {
            if let (Some(bp), Some(tp)) = (pptr_from_map(bm), pptr_from_map(tm)) {
                return pptr_refs_equal(base_file, bp, target_file, tp);
            }
            bm.len() == tm.len()
                && bm.iter().zip(tm).all(|((bk, bv), (tk, tv))| {
                    bk == tk
                        && values_equal_modulo_pptr(base_file, bv, target_file, tv, float_tolerance)
                })
        }
        (Value::Seq(bs), Value::Seq(ts)) => {
            bs.len() == ts.len()
                && bs.iter().zip(ts).all(|(bv, tv)| {
                    values_equal_modulo_pptr(base_file, bv, target_file, tv, float_tolerance)
                })
        }
        (Value::F32(b), Value::F32(t)) if float_tolerance => floats_equal(*b as f64, *t as f64),
        (Value::F64(b), Value::F64(t)) if float_tolerance => floats_equal(*b, *t),
        _ => false,
    }
}

// Absolute floor because the relative term collapses near zero;
// observed anchors at ~0 differ by ~2e-7 between manifests.
fn floats_equal(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-5 * a.abs().max(b.abs()) + 1e-6
}

#[cfg(test)]
mod tests {
    use super::floats_equal;

    #[test]
    fn few_ulp_diffs_are_equal() {
        assert!(floats_equal(1.4599998, 1.4600008));
        assert!(floats_equal(-0.0, 0.0));
        assert!(floats_equal(999999.0, 999_999.5));
        assert!(floats_equal(1.9073486328125e-06, 2.096707703458378e-06));
        assert!(floats_equal(0.0, 1e-7));
    }

    #[test]
    fn real_edits_differ() {
        assert!(!floats_equal(1.46, 1.47));
        assert!(!floats_equal(0.0, 0.002));
        assert!(!floats_equal(-1.46, 1.46));
    }
}

fn pptr_refs_equal<R: EnvResolver, P: TypeTreeProvider>(
    base_file: &SerializedFileHandle<'_, R, P>,
    base: PPtr,
    target_file: &SerializedFileHandle<'_, R, P>,
    target: PPtr,
) -> bool {
    match (base.optional(), target.optional()) {
        (None, None) => true,
        (Some(b), Some(t)) => {
            match (
                pptr_target_identity(base_file, b),
                pptr_target_identity(target_file, t),
            ) {
                (Some(bi), Some(ti)) => bi == ti,
                _ => false,
            }
        }
        _ => false,
    }
}

fn pptr_target_identity<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    pptr: PPtr,
) -> Option<(String, String, ClassId)> {
    let file_key = if pptr.is_local() {
        String::new()
    } else {
        pptr.file_identifier(file.file)?.pathName.clone()
    };
    let obj = file.deref(pptr.typed::<Value>()).ok()?;
    let class = obj.class_id();
    let data = obj.read().ok()?;
    let name = value_m_name(&data)
        .or_else(|| {
            (class == ClassId::Shader)
                .then(|| super::super::shader::parsed_form_name(&data))
                .flatten()
        })
        .or_else(|| gameobject_name(&obj.file, &data))?;
    Some((file_key, name, class))
}

fn gameobject_name<R: EnvResolver, P: TypeTreeProvider>(
    file: &SerializedFileHandle<'_, R, P>,
    component: &Value,
) -> Option<String> {
    let Value::Map(map) = component else {
        return None;
    };
    let Value::Map(go_map) = map.get(&Value::String("m_GameObject".to_string()))? else {
        return None;
    };
    let go_pptr = pptr_from_map(go_map)?;
    let go = file.deref(go_pptr.typed::<Value>()).ok()?.read().ok()?;
    value_m_name(&go)
}

fn value_m_name(value: &Value) -> Option<String> {
    let Value::Map(map) = value else {
        return None;
    };
    match map.get(&Value::String("m_Name".to_string()))? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}
