// TODO(ai-review): review for style and correctness
//! Decode a `Shader`'s `compressedBlob` for the structured view:
//! [`program_groups`] enumerates the per-platform sub-programs from
//! `m_ParsedForm` (no decompression, for the tree), and
//! [`decode_one_program`] decompresses and decodes a single program's
//! source on demand (USCSandbox layout).
//!
//! Per platform the blob holds LZ4 segments that concatenate into a
//! `ShaderSubProgramBlob` (`count` + entry index). Entries are
//! heterogeneous; `m_ParsedForm` says which `m_BlobIndex` is a
//! sub-program for which platform (via `m_GpuProgramType`). A
//! sub-program's code is GLSL text, a `0xF00DCAFE` MSL container (Metal),
//! or bytecode (DXBC / SPIR-V).

use std::collections::BTreeMap;

use serde_value::Value;

type Version = (u16, u16, u16);

/// Metal program code wraps its MSL in a `0xF00DCAFE`-tagged container.
const METAL_MAGIC: [u8; 4] = [0xfe, 0xca, 0x0d, 0xf0];

/// `(m_ParsedForm field, label)` for the six pass program stages.
const STAGES: [(&str, &str); 6] = [
    ("progVertex", "vertex"),
    ("progFragment", "fragment"),
    ("progGeometry", "geometry"),
    ("progHull", "hull"),
    ("progDomain", "domain"),
    ("progRayTracing", "raytracing"),
];

pub(crate) struct PlatformPrograms {
    pub platform: u32,
    pub passes: Vec<PassPrograms>,
}

pub(crate) struct PassPrograms {
    pub label: String,
    pub programs: Vec<ProgramRef>,
}

pub(crate) struct ProgramRef {
    pub blob_index: u32,
    pub type_name: &'static str,
    pub stage: &'static str,
    pub keywords: Vec<String>,
}

/// Enumerate the sub-programs straight from `m_ParsedForm` (no
/// decompression), grouped platform → pass. Each variant's
/// `m_BlobIndex` is assigned to the platform its `m_GpuProgramType`
/// belongs to and labelled with its stage + keyword set; the pass label
/// disambiguates otherwise-identical variants across passes.
pub(crate) fn program_groups(value: &Value) -> Vec<PlatformPrograms> {
    let parsed = get(Some(value), "m_ParsedForm");
    let keyword_names: Vec<&str> = seq(get(parsed, "m_KeywordNames"))
        .iter()
        .map(|v| match v {
            Value::String(s) => s.as_str(),
            _ => "",
        })
        .collect();

    let subshaders = seq(get(parsed, "m_SubShaders"));
    let multi_subshader = subshaders.len() > 1;

    let mut platforms: BTreeMap<u32, Vec<PassPrograms>> = BTreeMap::new();
    for (si, subshader) in subshaders.iter().enumerate() {
        for (pi, pass) in seq(get(Some(subshader), "m_Passes")).iter().enumerate() {
            let label = pass_label(pass, si, pi, multi_subshader);
            let mut per_platform: BTreeMap<u32, Vec<ProgramRef>> = BTreeMap::new();
            for (field, stage) in STAGES {
                let prog = get(Some(pass), field);
                for tier in seq(get(prog, "m_PlayerSubPrograms")) {
                    for variant in seq(Some(tier)) {
                        let gpu = as_i64(get(Some(variant), "m_GpuProgramType")).unwrap_or(-1);
                        let (Some(platform), Some(blob_index)) =
                            (gpu_platform(gpu), as_u32(get(Some(variant), "m_BlobIndex")))
                        else {
                            continue;
                        };
                        let keywords = seq(get(Some(variant), "m_KeywordIndices"))
                            .iter()
                            .filter_map(|k| as_u32(Some(k)))
                            .filter_map(|i| keyword_names.get(i as usize).copied())
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                            .collect();
                        per_platform.entry(platform).or_default().push(ProgramRef {
                            blob_index,
                            type_name: gpu_type_name(gpu as i32),
                            stage,
                            keywords,
                        });
                    }
                }
            }
            for (platform, mut programs) in per_platform {
                programs.sort_by_key(|p| p.blob_index);
                programs.dedup_by_key(|p| p.blob_index);
                platforms.entry(platform).or_default().push(PassPrograms {
                    label: label.clone(),
                    programs,
                });
            }
        }
    }

    platforms
        .into_iter()
        .map(|(platform, passes)| PlatformPrograms { platform, passes })
        .collect()
}

fn pass_label(pass: &Value, subshader: usize, pass_index: usize, multi_subshader: bool) -> String {
    let base = if multi_subshader {
        format!("SubShader {subshader} Pass {pass_index}")
    } else {
        format!("Pass {pass_index}")
    };
    let name = ["m_Name", "m_UseName"]
        .into_iter()
        .find_map(|k| match get(Some(pass), k) {
            Some(Value::String(s)) if !s.is_empty() => Some(s.as_str()),
            _ => None,
        });
    match name {
        Some(name) => format!("{base} ({name})"),
        None => base,
    }
}

/// Decompress and decode a single sub-program's source — the lazy
/// per-program node content. `(mime, source)`; `None` on missing fields.
pub(crate) fn decode_one_program(
    value: &Value,
    platform: u32,
    blob_index: u32,
    version: Version,
) -> Option<(&'static str, String)> {
    let platforms = as_u32_seq(get(Some(value), "platforms")?)?;
    let p = platforms.iter().position(|&x| x == platform)?;
    let blob = as_bytes(get(Some(value), "compressedBlob")?)?;
    let offsets = nested(get(Some(value), "offsets")?)?;
    let comp_lengths = nested(get(Some(value), "compressedLengths")?)?;
    let decomp_lengths = nested(get(Some(value), "decompressedLengths")?)?;

    let decompressed = decompress_platform(
        &blob,
        offsets.get(p)?,
        comp_lengths.get(p)?,
        decomp_lengths.get(p)?,
    )?;
    let entries = blob_entries(&decompressed, version)?;
    let sub = parse_subprogram(entries.get(blob_index as usize)?, version)?;
    Some(render_source(sub.program_type, sub.program_data))
}

fn render_source(program_type: i32, data: &[u8]) -> (&'static str, String) {
    match program_type {
        1..=8 => ("text/x-glsl", String::from_utf8_lossy(data).into_owned()),
        23 | 24 => match unwrap_metal(data) {
            Some(msl) => ("text/x-metal", String::from_utf8_lossy(msl).into_owned()),
            None => (
                "text/plain",
                format!("// Metal container, {} bytes", data.len()),
            ),
        },
        _ => (
            "text/plain",
            format!(
                "// TODO: {} bytecode, {} bytes (disassembly not implemented)",
                bytecode_format(program_type),
                data.len()
            ),
        ),
    }
}

fn decompress_platform(blob: &[u8], offs: &[u32], clens: &[u32], dlens: &[u32]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    for ((&off, &clen), &dlen) in offs.iter().zip(clens).zip(dlens) {
        let (off, clen, dlen) = (off as usize, clen as usize, dlen as usize);
        let slice = blob.get(off..off + clen)?;
        let chunk = lz4_flex::block::decompress(slice, dlen).ok()?;
        if chunk.len() != dlen {
            return None;
        }
        out.extend(chunk);
    }
    Some(out)
}

/// https://github.com/nesrak1/USCSandbox/blob/main/USCSandbox/Processor/BlobManager.cs
fn blob_entries(blob: &[u8], version: Version) -> Option<Vec<&[u8]>> {
    let mut r = Reader::new(blob);
    let count = r.i32_as_usize()?;
    let mut spans = Vec::with_capacity(count);
    for _ in 0..count {
        let off = r.i32_as_usize()?;
        let len = r.i32_as_usize()?;
        if version >= (2019, 3, 0) {
            r.i32()?; // segment
        }
        spans.push((off, len));
    }
    spans
        .into_iter()
        .map(|(off, len)| blob.get(off..off + len))
        .collect()
}

struct SubProgram<'a> {
    program_type: i32,
    program_data: &'a [u8],
}

/// Parse the `ShaderSubProgram` header up to its length-prefixed program
/// code (channels / params follow, unused here).
/// https://github.com/nesrak1/USCSandbox/blob/main/USCSandbox/Processor/ShaderSubProgram.cs
fn parse_subprogram(sub: &[u8], version: Version) -> Option<SubProgram<'_>> {
    let mut r = Reader::new(sub);
    r.i32()?; // blob version
    let program_type = r.i32()?;
    r.i32()?; // 3 stats (ALU, TEX, flow)
    r.i32()?;
    r.i32()?;
    if version >= (5, 5, 0) {
        r.i32()?; // stats: temp registers
    }

    let global_keywords = r.i32_as_usize()?;
    for _ in 0..global_keywords {
        r.skip_string()?;
    }
    if (2019, 1, 0) <= version && version < (2021, 2, 0) {
        let local_keywords = r.i32_as_usize()?;
        for _ in 0..local_keywords {
            r.skip_string()?;
        }
    }

    let program_data = r.byte_array()?;
    Some(SubProgram {
        program_type,
        program_data,
    })
}

/// `0xF00DCAFE` container → MSL: an `i32` offset reaches a
/// null-terminated entry-point name, the rest is UTF-8 source.
fn unwrap_metal(data: &[u8]) -> Option<&[u8]> {
    if !data.starts_with(&METAL_MAGIC) {
        return None;
    }
    let offset = u32::from_le_bytes(data.get(4..8)?.try_into().ok()?) as usize;
    let body = data.get(offset..)?;
    let entry_end = body.iter().position(|&b| b == 0)?;
    body.get(entry_end + 1..)
}

/// `ShaderGpuProgramType` → its `GPUPlatform`/`ShaderCompilerPlatform`
/// value (as in `platforms`); `None` for console / ray-tracing types.
/// https://github.com/nesrak1/USCSandbox/blob/main/USCSandbox/Processor/ShaderGpuProgramType.cs
/// https://github.com/nesrak1/USCSandbox/blob/main/USCSandbox/GPUPlatform.cs
fn gpu_platform(t: i64) -> Option<u32> {
    Some(match t {
        1 => 0,          // GLLegacy → openGL
        5 => 5,          // GLES → gles
        2 | 3 | 4 => 9,  // GLES31AEP / GLES31 / GLES3 → gles3
        6 | 7 | 8 => 15, // GLCore32/41/43 → glcore
        9..=12 => 1,     // DX9* → d3d9
        13 | 14 => 8,    // DX10Level9* → d3d11_9x
        15..=22 => 4,    // DX11* → d3d11
        23 | 24 => 14,   // MetalVS / MetalFS → metal
        25 => 18,        // SPIRV → vulkan
        _ => return None,
    })
}

/// https://github.com/nesrak1/USCSandbox/blob/main/USCSandbox/Processor/ShaderGpuProgramType.cs
fn gpu_type_name(t: i32) -> &'static str {
    match t {
        1 => "GLLegacy",
        2 => "GLES31AEP",
        3 => "GLES31",
        4 => "GLES3",
        5 => "GLES",
        6 => "GLCore32",
        7 => "GLCore41",
        8 => "GLCore43",
        9 => "DX9VertexSM20",
        10 => "DX9VertexSM30",
        11 => "DX9PixelSM20",
        12 => "DX9PixelSM30",
        13 => "DX10Level9Vertex",
        14 => "DX10Level9Pixel",
        15 => "DX11VertexSM40",
        16 => "DX11VertexSM50",
        17 => "DX11PixelSM40",
        18 => "DX11PixelSM50",
        19 => "DX11GeometrySM40",
        20 => "DX11GeometrySM50",
        21 => "DX11HullSM50",
        22 => "DX11DomainSM50",
        23 => "MetalVS",
        24 => "MetalFS",
        25 => "SPIRV",
        26 => "ConsoleVS",
        27 => "ConsoleFS",
        28 => "ConsoleHS",
        29 => "ConsoleDS",
        30 => "ConsoleGS",
        31 => "RayTracing",
        32 => "PS5NGGC",
        _ => "Unknown",
    }
}

fn bytecode_format(program_type: i32) -> &'static str {
    match program_type {
        9..=22 => "DXBC",
        25 => "SPIR-V",
        _ => "unknown",
    }
}

pub(crate) fn platform_name(platform: u32) -> &'static str {
    match platform {
        4 => "D3D11",
        14 => "Metal",
        15 => "OpenGLCore",
        18 => "Vulkan",
        _ => "Unknown",
    }
}

fn get<'a>(v: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    match v? {
        Value::Map(m) => m.get(&Value::String(key.to_string())),
        _ => None,
    }
}

fn seq(v: Option<&Value>) -> &[Value] {
    match v {
        Some(Value::Seq(items)) => items,
        _ => &[],
    }
}

fn as_bytes(v: &Value) -> Option<Vec<u8>> {
    match v {
        Value::Bytes(b) => Some(b.clone()),
        Value::Seq(items) => items
            .iter()
            .map(|x| u8::try_from(as_i64(Some(x))?).ok())
            .collect(),
        _ => None,
    }
}

fn as_u32(v: Option<&Value>) -> Option<u32> {
    u32::try_from(as_i64(v)?).ok()
}

fn as_i64(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::U8(n) => Some(i64::from(*n)),
        Value::U16(n) => Some(i64::from(*n)),
        Value::U32(n) => Some(i64::from(*n)),
        Value::U64(n) => i64::try_from(*n).ok(),
        Value::I8(n) => Some(i64::from(*n)),
        Value::I16(n) => Some(i64::from(*n)),
        Value::I32(n) => Some(i64::from(*n)),
        Value::I64(n) => Some(*n),
        _ => None,
    }
}

fn as_u32_seq(v: &Value) -> Option<Vec<u32>> {
    match v {
        Value::Seq(items) => items.iter().map(|x| as_u32(Some(x))).collect(),
        _ => None,
    }
}

/// `Vec<u32>` or `Vec<Vec<u32>>` → per-platform segment lists.
fn nested(v: &Value) -> Option<Vec<Vec<u32>>> {
    let Value::Seq(items) = v else { return None };
    items
        .iter()
        .map(|item| match item {
            Value::Seq(_) => as_u32_seq(item),
            scalar => as_u32(Some(scalar)).map(|n| vec![n]),
        })
        .collect()
}

/// Little-endian reader; aligns to 4 bytes (relative to the slice start)
/// after each variable-length field.
struct Reader<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Self {
        Reader { b, p: 0 }
    }
    fn i32(&mut self) -> Option<i32> {
        let s = self.b.get(self.p..self.p + 4)?;
        self.p += 4;
        Some(i32::from_le_bytes(s.try_into().unwrap()))
    }
    fn i32_as_usize(&mut self) -> Option<usize> {
        usize::try_from(self.i32()?).ok()
    }
    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.p..self.p + n)?;
        self.p += n;
        Some(s)
    }
    fn align(&mut self) {
        self.p = self.p.next_multiple_of(4);
    }
    fn skip_string(&mut self) -> Option<()> {
        let n = self.i32_as_usize()?;
        self.bytes(n)?;
        self.align();
        Some(())
    }
    fn byte_array(&mut self) -> Option<&'a [u8]> {
        let n = self.i32_as_usize()?;
        let v = self.bytes(n)?;
        self.align();
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Unity 2022.3: tempReg present (>= 5.5), local keywords absent
    // (outside 2019.1–2021.2) — matches the games we target.
    const V: Version = (2022, 3, 0);

    fn i32le(buf: &mut Vec<u8>, v: i32) {
        buf.extend_from_slice(&v.to_le_bytes());
    }

    /// A `ShaderSubProgram` for version `V`: header, zero keywords, then
    /// the length-prefixed (4-aligned) program code.
    fn make_subprogram(program_type: i32, code: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        i32le(&mut b, 0x0C0A_75BA); // blob version
        i32le(&mut b, program_type);
        i32le(&mut b, 0); // stats: ALU
        i32le(&mut b, 0); // stats: TEX
        i32le(&mut b, 0); // stats: flow
        i32le(&mut b, 0); // stats: temp registers (>= 5.5)
        i32le(&mut b, 0); // global keyword count
        i32le(&mut b, code.len() as i32);
        b.extend_from_slice(code);
        while b.len() % 4 != 0 {
            b.push(0);
        }
        b
    }

    /// A `ShaderSubProgramBlob` holding a single entry.
    fn make_blob(sub: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        i32le(&mut b, 1); // count
        i32le(&mut b, 16); // offset = 4 (count) + 12 (one entry)
        i32le(&mut b, sub.len() as i32); // length
        i32le(&mut b, 0); // segment
        b.extend_from_slice(sub);
        b
    }

    fn smap(pairs: &[(&str, Value)]) -> Value {
        Value::Map(
            pairs
                .iter()
                .map(|(k, v)| (Value::String(k.to_string()), v.clone()))
                .collect(),
        )
    }

    #[test]
    fn gpu_platform_and_type_names() {
        assert_eq!(gpu_platform(6), Some(15)); // GLCore32 → OpenGLCore
        assert_eq!(gpu_platform(15), Some(4)); // DX11VertexSM40 → D3D11
        assert_eq!(gpu_platform(24), Some(14)); // MetalFS → Metal
        assert_eq!(gpu_platform(25), Some(18)); // SPIRV → Vulkan
        assert_eq!(gpu_platform(31), None); // RayTracing → unsupported
        assert_eq!(gpu_type_name(6), "GLCore32");
        assert_eq!(gpu_type_name(25), "SPIRV");
    }

    #[test]
    fn unwrap_metal_extracts_msl() {
        let mut data = Vec::new();
        data.extend_from_slice(&METAL_MAGIC);
        i32le(&mut data, 8); // offset past the 8-byte header
        data.extend_from_slice(b"xlatMtlMain\0");
        data.extend_from_slice(b"#include <metal_stdlib>");
        assert_eq!(unwrap_metal(&data), Some(&b"#include <metal_stdlib>"[..]));
        assert_eq!(unwrap_metal(b"not metal"), None);
    }

    #[test]
    fn parse_subprogram_reaches_program_code() {
        let sub = make_subprogram(6, b"#version 150\nvoid main(){}");
        let parsed = parse_subprogram(&sub, V).unwrap();
        assert_eq!(parsed.program_type, 6);
        assert_eq!(parsed.program_data, b"#version 150\nvoid main(){}");
    }

    #[test]
    fn blob_entries_splits_by_index() {
        let sub = make_subprogram(6, b"abc");
        let blob = make_blob(&sub);
        let entries = blob_entries(&blob, V).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], sub.as_slice());
    }

    #[test]
    fn decode_one_program_roundtrips_glsl() {
        let sub = make_subprogram(6, b"#version 150\nvoid main(){}");
        let decompressed = make_blob(&sub);
        let compressed = lz4_flex::block::compress(&decompressed);
        let value = smap(&[
            ("platforms", Value::Seq(vec![Value::U32(15)])),
            ("compressedBlob", Value::Bytes(compressed.clone())),
            ("offsets", Value::Seq(vec![Value::Seq(vec![Value::U32(0)])])),
            (
                "compressedLengths",
                Value::Seq(vec![Value::Seq(vec![Value::U32(compressed.len() as u32)])]),
            ),
            (
                "decompressedLengths",
                Value::Seq(vec![Value::Seq(vec![
                    Value::U32(decompressed.len() as u32),
                ])]),
            ),
        ]);
        let (mime, source) = decode_one_program(&value, 15, 0, V).unwrap();
        assert_eq!(mime, "text/x-glsl");
        assert!(source.contains("#version 150"));
    }

    #[test]
    fn program_groups_labels_stage_and_keywords() {
        let variant = smap(&[
            ("m_GpuProgramType", Value::I8(6)), // GLCore32 → platform 15
            ("m_BlobIndex", Value::U32(0)),
            ("m_KeywordIndices", Value::Seq(vec![Value::U16(0)])),
        ]);
        let pass = smap(&[
            ("m_Name", Value::String(String::new())),
            (
                "progVertex",
                smap(&[(
                    "m_PlayerSubPrograms",
                    Value::Seq(vec![Value::Seq(vec![variant])]),
                )]),
            ),
        ]);
        let parsed_form = smap(&[
            (
                "m_KeywordNames",
                Value::Seq(vec![Value::String("USE_MASK".into())]),
            ),
            (
                "m_SubShaders",
                Value::Seq(vec![smap(&[("m_Passes", Value::Seq(vec![pass]))])]),
            ),
        ]);
        let value = smap(&[("m_ParsedForm", parsed_form)]);

        let groups = program_groups(&value);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].platform, 15);
        assert_eq!(groups[0].passes.len(), 1);
        assert_eq!(groups[0].passes[0].label, "Pass 0");
        let programs = &groups[0].passes[0].programs;
        assert_eq!(programs.len(), 1);
        assert_eq!(programs[0].blob_index, 0);
        assert_eq!(programs[0].stage, "vertex");
        assert_eq!(programs[0].type_name, "GLCore32");
        assert_eq!(programs[0].keywords, ["USE_MASK"]);
    }
}
