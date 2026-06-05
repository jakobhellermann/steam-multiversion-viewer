// TODO(ai-review): review for style and correctness
//! Decode a `Shader`'s `compressedBlob` into per-platform program source
//! for the JSON dump (USCSandbox).
//!
//! Per platform the blob holds LZ4 segments that concatenate into a
//! `ShaderSubProgramBlob` (`count` + entry index). Entries are
//! heterogeneous; `m_ParsedForm` says which `m_BlobIndex` is a
//! sub-program for which platform (via `m_GpuProgramType`). A
//! sub-program's code is GLSL text, a `0xF00DCAFE` MSL container (Metal),
//! or bytecode (DXBC / SPIR-V).

use std::collections::{BTreeMap, BTreeSet};

use serde_value::Value;

type Version = (u16, u16, u16);

/// Metal program code wraps its MSL in a `0xF00DCAFE`-tagged container.
const METAL_MAGIC: [u8; 4] = [0xfe, 0xca, 0x0d, 0xf0];

/// `None` when the blob fields are absent (pre-5.5 shader, …); the
/// caller then falls back to the plain JSON dump.
pub(crate) fn decode_shader(value: &Value, version: Version) -> Option<Value> {
    let parsed = get(Some(value), "m_ParsedForm")?;
    let blob = as_bytes(get(Some(value), "compressedBlob")?)?;
    let platforms = as_u32_seq(get(Some(value), "platforms")?)?;
    let offsets = nested(get(Some(value), "offsets")?)?;
    let comp_lengths = nested(get(Some(value), "compressedLengths")?)?;
    let decomp_lengths = nested(get(Some(value), "decompressedLengths")?)?;

    let mut out = Vec::with_capacity(platforms.len());
    for (p, &platform) in platforms.iter().enumerate() {
        let decompressed = decompress_platform(
            &blob,
            offsets.get(p)?,
            comp_lengths.get(p)?,
            decomp_lengths.get(p)?,
        )?;
        let entries = blob_entries(&decompressed, version)?;

        let indices = subprogram_indices(parsed, platform);
        let mut programs = Vec::with_capacity(indices.len());
        for index in indices {
            let bytes = entries.get(index as usize)?;
            let sub = parse_subprogram(bytes, version)?;
            programs.push(program_value(index, sub.program_type, sub.program_data));
        }

        let mut entry = BTreeMap::new();
        entry.insert(svalue_str("platform"), Value::U32(platform));
        entry.insert(
            svalue_str("platformName"),
            svalue_str(platform_name(platform)),
        );
        entry.insert(svalue_str("programs"), Value::Seq(programs));
        out.push(Value::Map(entry));
    }
    Some(Value::Seq(out))
}

fn program_value(index: u32, program_type: i32, data: &[u8]) -> Value {
    let mut map = BTreeMap::new();
    map.insert(svalue_str("blobIndex"), Value::U32(index));
    map.insert(svalue_str("type"), svalue_str(gpu_type_name(program_type)));

    let source = match program_type {
        1..=8 => Some(String::from_utf8_lossy(data).into_owned()),
        23 | 24 => unwrap_metal(data).map(|msl| String::from_utf8_lossy(msl).into_owned()),
        _ => None,
    };
    match source {
        Some(source) => {
            map.insert(svalue_str("source"), svalue_str(source));
        }
        None => {
            map.insert(
                svalue_str("note"),
                svalue_str(format!("TODO: {} bytecode", bytecode_format(program_type))),
            );
            map.insert(svalue_str("bytes"), Value::U64(data.len() as u64));
        }
    }
    Value::Map(map)
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

/// Blob indices that are sub-programs for `platform`, filtered via each
/// variant's `m_GpuProgramType`.
fn subprogram_indices(parsed: &Value, platform: u32) -> BTreeSet<u32> {
    const STAGES: [&str; 6] = [
        "progVertex",
        "progFragment",
        "progGeometry",
        "progHull",
        "progDomain",
        "progRayTracing",
    ];
    let mut indices = BTreeSet::new();
    for subshader in seq(get(Some(parsed), "m_SubShaders")) {
        for pass in seq(get(Some(subshader), "m_Passes")) {
            for stage in STAGES {
                let prog = get(Some(pass), stage);
                for tier in seq(get(prog, "m_PlayerSubPrograms")) {
                    for variant in seq(Some(tier)) {
                        let gpu = as_i64(get(Some(variant), "m_GpuProgramType")).unwrap_or(-1);
                        if gpu_platform(gpu) == Some(platform) {
                            indices.extend(as_u32(get(Some(variant), "m_BlobIndex")));
                        }
                    }
                }
            }
        }
    }
    indices
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

fn platform_name(platform: u32) -> &'static str {
    match platform {
        4 => "D3D11",
        14 => "Metal",
        15 => "OpenGLCore",
        18 => "Vulkan",
        _ => "Unknown",
    }
}

fn svalue_str(s: impl Into<String>) -> Value {
    Value::String(s.into())
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
