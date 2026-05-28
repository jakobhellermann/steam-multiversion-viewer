// TODO(ai-review): review for style and correctness
//! Extract the AES key from a game's `SecPlayerPrefs.SecurePlayerPrefs`
//! static initializer and decrypt the resulting blobs.
//!
//! Used by the TextAsset renderer: many Unity games ship localised
//! strings as base64(AES-256-ECB(xml)) blobs in TextAssets, keyed off
//! a hardcoded `byte[] keyArray` in `Assembly-CSharp.dll`. The IL
//! pattern we match is the one Unity's C# compiler emits for
//!
//! ```csharp
//! private static byte[] keyArray =
//!     Encoding.UTF8.GetBytes("UKu52ePUBwetZ9wNX88o54dnfKRu0T1l");
//! ```
//!
//! which compiles to `ldstr <KEY>; call UTF8.GetBytes; stsfld
//! keyArray` inside the type's static constructor. We don't try to
//! recognise other shapes — games that diverge from this pattern just
//! fall through and the renderer skips decryption.

use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use dll_diff::dotnetdll::prelude::{ReadOptions, Resolution};
use dll_diff::dotnetdll::resolved::il::Instruction;
use dll_diff::dotnetdll::resolved::members::{
    FieldSource, MethodReferenceParent, MethodSource, UserMethod,
};
use dll_diff::dotnetdll::resolved::types::{BaseType, MethodType, TypeSource};

/// Try to find a game's SecurePlayerPrefs AES key inside the given
/// `Assembly-CSharp.dll` bytes. `None` means "no such type, or the
/// initializer doesn't match the expected shape" — the caller should
/// silently skip decryption rather than fail the request.
#[tracing::instrument(skip_all, fields(bytes = dll_bytes.len()))]
pub fn extract_key(dll_bytes: &[u8]) -> Option<Vec<u8>> {
    let res = Resolution::parse(dll_bytes, ReadOptions::default()).ok()?;

    // The type lives in the `SecPlayerPrefs` namespace in every game
    // we've seen, but match on the simple name as a fallback in case a
    // game inlines the helper into a different namespace.
    let (ty_idx, ty) = res.enumerate_type_definitions().find(|(_, td)| {
        td.name == "SecurePlayerPrefs" && td.namespace.as_deref() == Some("SecPlayerPrefs")
    })?;

    // `keyArray` is a static field on the same type. Resolving by name
    // beats walking IL operands twice — the stsfld target has to point
    // here for us to accept the match.
    let key_field_idx = ty.fields.iter().position(|f| f.name == "keyArray")?;
    let key_field = res.field_index(ty_idx, key_field_idx)?;

    // `.cctor` is the static initializer the compiler synthesises for
    // field initializers. It's marked `runtime_special_name = true`
    // and named literally `.cctor`.
    let cctor = ty.methods.iter().find(|m| m.name == ".cctor")?;
    let body = cctor.body.as_ref()?;

    // Pattern: `ldstr <KEY>` followed (after any number of nops) by a
    // call to `Encoding.UTF8.GetBytes(string)` followed by `stsfld
    // keyArray`. We allow a small gap between the three because Mono's
    // compiler occasionally inserts a `volatile.`/`nop` prefix.
    let insns = &body.instructions;
    for (i, ins) in insns.iter().enumerate() {
        let Instruction::LoadString(utf16) = ins else {
            continue;
        };
        let Ok(key_str) = String::from_utf16(utf16) else {
            continue;
        };
        // Scan forward up to 4 instructions for the GetBytes call.
        let mut j = i + 1;
        let mut saw_getbytes = false;
        while j < insns.len() && j <= i + 4 {
            if is_utf8_getbytes_call(&insns[j], &res) {
                saw_getbytes = true;
                break;
            }
            j += 1;
        }
        if !saw_getbytes {
            continue;
        }
        // Then the stsfld pointing at our keyArray field.
        let mut k = j + 1;
        while k < insns.len() && k <= j + 4 {
            if let Instruction::StoreStaticField {
                param0: FieldSource::Definition(fi),
                ..
            } = &insns[k]
                && *fi == key_field
            {
                return Some(key_str.into_bytes());
            }
            k += 1;
        }
    }

    None
}

fn is_utf8_getbytes_call(ins: &Instruction, res: &Resolution<'_>) -> bool {
    // Roslyn emits `callvirt` for instance calls on `Encoding.UTF8` (a
    // virtual property getter that returns a non-sealed Encoding); the
    // legacy Mono compiler emits a plain `call`. Accept both — the
    // name + parent check below is what makes the match specific.
    let um = match ins {
        Instruction::Call {
            param0: MethodSource::User(um),
            ..
        } => um,
        Instruction::CallVirtual {
            param0: MethodSource::User(um),
            ..
        } => um,
        _ => return false,
    };
    // We accept both definition and reference variants — `mscorlib`'s
    // `Encoding.UTF8.GetBytes` is always a Reference in user assemblies,
    // but a defensive match on both keeps the helper sane against
    // synthetic test DLLs that inline the call.
    let (parent_name, method_name) = match um {
        UserMethod::Reference(mri) => {
            let r = &res[*mri];
            let parent = match &r.parent {
                MethodReferenceParent::Type(MethodType::Base(b)) => match &**b {
                    // `Encoding` lives in mscorlib so the C# compiler
                    // always emits the GetBytes target as a plain
                    // `BaseType::Type` over a user type-ref. Anything
                    // generic / vector / pointer-shaped is by
                    // construction not `System.Text.Encoding`.
                    BaseType::Type {
                        source: TypeSource::User(ut),
                        ..
                    } => ut.type_name(res),
                    _ => return false,
                },
                _ => return false,
            };
            (parent, r.name.as_ref())
        }
        UserMethod::Definition(mi) => {
            let m = &res[*mi];
            let ty_idx = mi.parent_type();
            let td = &res[ty_idx];
            let parent = match &td.namespace {
                Some(ns) if !ns.is_empty() => format!("{}.{}", ns, td.name),
                _ => td.name.to_string(),
            };
            (parent, m.name.as_ref())
        }
    };
    method_name == "GetBytes" && parent_name.ends_with("Encoding")
}

/// Decrypt a `base64(AES-256-ECB(plaintext))` blob the way
/// SecurePlayerPrefs does, returning the UTF-8 plaintext. Returns
/// `None` if decoding, decryption, or UTF-8 validation fails — all of
/// which mean "this TextAsset wasn't encoded with our key", not an
/// error worth surfacing.
pub fn decrypt(key: &[u8], blob: &str) -> Option<String> {
    use aes::cipher::block_padding::Pkcs7;
    use aes::cipher::{BlockDecryptMut, KeyInit};

    // The cipher rejects bad key sizes via `new_from_slice`. Anything
    // other than the canonical 32-byte key just yields `None`.
    let cipher = ecb::Decryptor::<aes::Aes256>::new_from_slice(key).ok()?;

    let ct = BASE64_STANDARD.decode(blob.trim()).ok()?;
    let mut buf = vec![0u8; ct.len()];
    let pt = cipher.decrypt_padded_b2b_mut::<Pkcs7>(&ct, &mut buf).ok()?;
    std::str::from_utf8(pt).ok().map(|s| s.to_owned())
}
