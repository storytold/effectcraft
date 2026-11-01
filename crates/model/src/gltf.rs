//! glTF 2.0 import, written from the Khronos glTF 2.0 specification.
//!
//! Supports `.gltf` (JSON with external `.bin`/image files or `data:` URIs) and `.glb` (binary
//! container: a JSON chunk and a BIN chunk). Imports the default scene's node hierarchy (TRS or
//! matrix), triangle primitives (`TRIANGLES`, `TRIANGLE_STRIP`, `TRIANGLE_FAN`) with POSITION,
//! NORMAL, TANGENT, TEXCOORD_0, JOINTS_0 and WEIGHTS_0, PBR metallic-roughness materials
//! (base colour, metallic-roughness, normal, occlusion and emissive textures; alpha modes;
//! double-sided; `KHR_materials_emissive_strength`, `KHR_materials_unlit`), embedded or
//! external PNG/JPEG images with sampler wrap modes, skins and animations (step, linear and
//! cubic-spline translation/rotation/scale), perspective and orthographic cameras, and
//! `KHR_lights_punctual` lights (directional, point, spot). Sparse accessors and morph targets
//! are ignored.

use std::{cell::Cell, sync::Arc};

use serde_json::Value;

use crate::{
    AlphaMode, Animation, Channel, ChannelPath, Interp, Mat4, Material, Mesh, Model, ModelError, Node, Primitive, Result, Skin, TexRef, Texture, Wrap, perr,
};

const GLB_MAGIC: u32 = 0x4654_6C67;
const CHUNK_JSON: u32 = 0x4E4F_534A;
const CHUNK_BIN: u32 = 0x004E_4942;

/// Split a GLB container into its JSON text and BIN chunk.
pub fn split_glb(b: &[u8]) -> Result<(&[u8], Option<&[u8]>)> {
    let u32_at = |o: usize| -> Option<u32> { b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])) };
    if u32_at(0) != Some(GLB_MAGIC) {
        return Err(perr("not a GLB file"));
    }
    if u32_at(4) != Some(2) {
        return Err(ModelError::Unsupported(format!("GLB version {}", u32_at(4).unwrap_or(0))));
    }
    let total = (u32_at(8).unwrap_or(0) as usize).min(b.len());
    let mut o = 12;
    let mut json = None;
    let mut bin = None;
    while o + 8 <= total {
        let len = u32_at(o).unwrap_or(0) as usize;
        let ty = u32_at(o + 4).unwrap_or(0);
        let past_end = || perr("GLB chunk runs past the end of the file");
        let end = (o + 8).checked_add(len).ok_or_else(past_end)?;
        let data = b.get(o + 8..end).ok_or_else(past_end)?;
        match ty {
            CHUNK_JSON if json.is_none() => json = Some(data),
            CHUNK_BIN if bin.is_none() => bin = Some(data),
            _ => {}
        }
        o = end;
    }
    Ok((json.ok_or_else(|| perr("GLB without a JSON chunk"))?, bin))
}

/// Decode standard base64 (with or without padding; whitespace ignored).
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Percent-decode a relative URI.
pub fn uri_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && b[i + 1].is_ascii_hexdigit()
            && b[i + 2].is_ascii_hexdigit()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Bytes of a `data:` URI or an external resource.
fn load_uri(uri: &str, resolve: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<Vec<u8>> {
    if let Some(rest) = uri.strip_prefix("data:") {
        let (meta, data) = rest.split_once(',').ok_or_else(|| perr("malformed data URI"))?;
        return if meta.ends_with(";base64") { base64_decode(data).ok_or_else(|| perr("bad base64 in data URI")) } else { Ok(uri_decode(data).into_bytes()) };
    }
    let name = uri_decode(uri);
    resolve(&name).ok_or(ModelError::Missing(name))
}

struct Doc<'a> {
    j: &'a Value,
    buffers: Vec<Vec<u8>>,
    /// Zero components that buffer-less accessors may still allocate, across the whole file:
    /// nothing backs them, so many references to one capped accessor must not multiply (#2139).
    zero_left: Cell<usize>,
    /// Components every accessor read may still allocate, across the whole file: each primitive
    /// that references an accessor keeps its own copy, so references must not multiply memory.
    read_left: Cell<usize>,
    /// The budget `read_left` started from, for the error message.
    read_cap: usize,
}

fn arr<'v>(v: &'v Value, k: &str) -> &'v [Value] {
    v.get(k).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}
/// A non-negative integer field; one too large for `usize` counts as absent (#1235).
fn idx(v: &Value, k: &str) -> Option<usize> {
    v.get(k).and_then(Value::as_u64).and_then(|x| usize::try_from(x).ok())
}
/// Node indices in an array field (out-of-range ones are dropped).
fn indices(v: &Value, k: &str) -> Vec<usize> {
    arr(v, k).iter().filter_map(Value::as_u64).filter_map(|c| usize::try_from(c).ok()).collect()
}

/// The most components accessors without a buffer view (all zeros) may have, together: nothing
/// in the file backs their counts, so they are capped instead of checked against a buffer
/// (#1235, #2139).
const MAX_ZERO_COMPONENTS: usize = 1 << 24;

/// The most components all accessor reads in one file may allocate together (about 1 GiB as
/// `f32`/`u32`). Each primitive keeps its own copy of what it reads, so an accessor referenced
/// N times costs N times; this bounds that by a fixed amount rather than by buffer size, because
/// real assets legitimately share one vertex buffer across many primitives. A 10M-triangle scan
/// (5M vertices with position, normal, UV and tangent, 30M indices) reads about 90M.
const MAX_READ_COMPONENTS: usize = 1 << 28;

/// Where an accessor's elements lie in its buffer: `count` elements of `elem` bytes, `stride`
/// apart from `base`, all checked to be inside `buf`.
struct Span<'b> {
    buf: &'b [u8],
    base: usize,
    stride: usize,
}
fn num(v: &Value, k: &str, d: f64) -> f64 {
    v.get(k).and_then(Value::as_f64).unwrap_or(d)
}
fn nums<const N: usize>(v: &Value, k: &str, d: [f64; N]) -> [f64; N] {
    let mut out = d;
    if let Some(a) = v.get(k).and_then(Value::as_array) {
        for (i, x) in a.iter().take(N).enumerate() {
            out[i] = x.as_f64().unwrap_or(d[i]);
        }
    }
    out
}

/// Bytes of component `c` of element `i`, with checked offsets (the span was validated, but
/// nothing derived from the file is indexed unchecked).
fn component(buf: &[u8], base: usize, stride: usize, i: usize, c: usize, csize: usize) -> Result<&[u8]> {
    let o = i.checked_mul(stride).and_then(|o| o.checked_add(base)).and_then(|o| o.checked_add(c.checked_mul(csize)?));
    o.and_then(|o| buf.get(o..o.checked_add(csize)?)).ok_or_else(|| perr("accessor reads past its buffer"))
}

impl Doc<'_> {
    /// Charge `total` components to the file's read budget before allocating them.
    fn charge(&self, total: usize) -> Result<()> {
        let left = self.read_left.get();
        let Some(rest) = left.checked_sub(total) else {
            let cap = self.read_cap;
            return Err(perr(format!("accessor reads need more than {cap} components in total ({total} requested, {left} left)")));
        };
        self.read_left.set(rest);
        Ok(())
    }

    /// Charge `count` items of `per` components each.
    fn charge_each(&self, count: usize, per: usize) -> Result<()> {
        self.charge(count.checked_mul(per).ok_or_else(|| perr("primitive is too large"))?)
    }

    /// The accessor's element count and total components (`count * n`), checked; and its span in
    /// the buffer, validated before anything is allocated from the count. `None` for an
    /// accessor without a buffer view, whose components are all zero.
    fn layout(&self, acc: &Value, n: usize, csize: usize) -> Result<(usize, usize, Option<Span<'_>>)> {
        let count = match acc.get("count") {
            None => 0,
            Some(c) => c.as_u64().and_then(|c| usize::try_from(c).ok()).ok_or_else(|| perr("accessor count is not a valid size"))?,
        };
        let total = count.checked_mul(n).ok_or_else(|| perr("accessor count is too large"))?;
        let Some(bv) = idx(acc, "bufferView") else {
            let left = self.zero_left.get();
            let Some(rest) = left.checked_sub(total) else {
                return Err(perr(format!("accessors without a bufferView have more than {MAX_ZERO_COMPONENTS} components ({total} requested, {left} left)")));
            };
            self.zero_left.set(rest);
            self.charge(total)?;
            return Ok((count, total, None));
        };
        let view = arr(self.j, "bufferViews").get(bv).ok_or_else(|| perr(format!("no bufferView {bv}")))?;
        let buf = self.buffers.get(idx(view, "buffer").unwrap_or(0)).ok_or_else(|| perr("bufferView buffer out of range"))?;
        let past = || perr("accessor reads past its buffer");
        let base = idx(view, "byteOffset").unwrap_or(0).checked_add(idx(acc, "byteOffset").unwrap_or(0)).ok_or_else(past)?;
        let elem = csize.checked_mul(n).ok_or_else(past)?;
        let stride = idx(view, "byteStride").filter(|s| *s > 0).unwrap_or(elem);
        if stride < elem {
            return Err(perr(format!("bufferView byteStride {stride} is smaller than its {elem}-byte elements")));
        }
        if count > 0 {
            let end = (count - 1).checked_mul(stride).and_then(|o| o.checked_add(base)).and_then(|o| o.checked_add(elem)).ok_or_else(past)?;
            if end > buf.len() {
                return Err(past());
            }
        }
        self.charge(total)?;
        Ok((count, total, Some(Span { buf, base, stride })))
    }

    /// An accessor as `f32` rows of `n` components (normalised integers mapped to 0..1 / -1..1).
    fn floats(&self, a: usize) -> Result<(Vec<f32>, usize)> {
        let acc = arr(self.j, "accessors").get(a).ok_or_else(|| perr(format!("no accessor {a}")))?;
        let n = match acc.get("type").and_then(Value::as_str).unwrap_or("") {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" => 4,
            "MAT2" => 4,
            "MAT3" => 9,
            "MAT4" => 16,
            t => return Err(perr(format!("accessor type `{t}`"))),
        };
        let ct = idx(acc, "componentType").unwrap_or(5126);
        let norm = acc.get("normalized").and_then(Value::as_bool).unwrap_or(false);
        let csize = match ct {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            5125 | 5126 => 4,
            _ => return Err(perr(format!("componentType {ct}"))),
        };
        let (count, total, span) = self.layout(acc, n, csize)?;
        let mut out = vec![0.0f32; total];
        let Some(Span { buf, base, stride }) = span else {
            // No buffer view: all zeros (sparse substitution is not supported).
            return Ok((out, n));
        };
        // Matrices of 1/2-byte components pad columns to 4 bytes; ignored (rare).
        for i in 0..count {
            for c in 0..n {
                let s = component(buf, base, stride, i, c, csize)?;
                out[i * n + c] = match ct {
                    5126 => f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
                    5125 => u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32,
                    5123 => {
                        let v = u16::from_le_bytes([s[0], s[1]]) as f32;
                        if norm { v / 65535.0 } else { v }
                    }
                    5122 => {
                        let v = i16::from_le_bytes([s[0], s[1]]) as f32;
                        if norm { (v / 32767.0).max(-1.0) } else { v }
                    }
                    5121 => {
                        let v = s[0] as f32;
                        if norm { v / 255.0 } else { v }
                    }
                    _ => {
                        let v = s[0] as i8 as f32;
                        if norm { (v / 127.0).max(-1.0) } else { v }
                    }
                };
            }
        }
        Ok((out, n))
    }

    /// Integer accessor (indices, joints) read exactly.
    fn ints(&self, a: usize) -> Result<Vec<u32>> {
        let acc = arr(self.j, "accessors").get(a).ok_or_else(|| perr(format!("no accessor {a}")))?;
        let ct = idx(acc, "componentType").unwrap_or(5125);
        if ct == 5126 {
            return Ok(self.floats(a)?.0.into_iter().map(|v| v as u32).collect());
        }
        let n = match acc.get("type").and_then(Value::as_str).unwrap_or("SCALAR") {
            "VEC4" => 4,
            "VEC3" => 3,
            "VEC2" => 2,
            _ => 1,
        };
        let csize = match ct {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            _ => 4,
        };
        let (count, total, span) = self.layout(acc, n, csize)?;
        let Some(Span { buf, base, stride }) = span else { return Ok(vec![0; total]) };
        let mut out = Vec::with_capacity(total);
        for i in 0..count {
            for c in 0..n {
                let s = component(buf, base, stride, i, c, csize)?;
                out.push(match csize {
                    1 => s[0] as u32,
                    2 => u16::from_le_bytes([s[0], s[1]]) as u32,
                    _ => u32::from_le_bytes([s[0], s[1], s[2], s[3]]),
                });
            }
        }
        Ok(out)
    }
}

/// Parse a `.gltf` (JSON) or `.glb` file.
pub fn parse(bytes: &[u8], resolve: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<Model> {
    parse_with_budget(bytes, resolve, MAX_READ_COMPONENTS)
}

/// [`parse`] with a read budget of `read_budget` accessor components for the whole file.
pub(crate) fn parse_with_budget(bytes: &[u8], resolve: &dyn Fn(&str) -> Option<Vec<u8>>, read_budget: usize) -> Result<Model> {
    let (json, bin) = if bytes.starts_with(b"glTF") { split_glb(bytes)? } else { (bytes, None) };
    let j: Value = serde_json::from_slice(json).map_err(|e| perr(format!("glTF JSON: {e}")))?;
    let ver = j.get("asset").and_then(|a| a.get("version")).and_then(Value::as_str).unwrap_or("");
    if !ver.starts_with('2') {
        return Err(ModelError::Unsupported(format!("glTF version `{ver}` (2.0 required)")));
    }
    for ext in arr(&j, "extensionsRequired") {
        let e = ext.as_str().unwrap_or("");
        if !matches!(e, "KHR_materials_unlit" | "KHR_materials_emissive_strength") {
            return Err(ModelError::Unsupported(format!("required extension {e}")));
        }
    }
    let mut buffers = vec![];
    for (i, b) in arr(&j, "buffers").iter().enumerate() {
        let data = match b.get("uri").and_then(Value::as_str) {
            Some(u) => load_uri(u, resolve)?,
            None if i == 0 => bin.map(<[u8]>::to_vec).ok_or_else(|| perr("buffer 0 has no uri and the file has no BIN chunk"))?,
            None => vec![],
        };
        buffers.push(data);
    }
    let d = Doc { j: &j, buffers, zero_left: Cell::new(MAX_ZERO_COMPONENTS), read_left: Cell::new(read_budget), read_cap: read_budget };

    // Images → textures (decoded once per image; samplers set the wrap modes).
    let mut images: Vec<Option<Arc<Texture>>> = vec![];
    for im in arr(&j, "images") {
        let data = if let Some(u) = im.get("uri").and_then(Value::as_str) {
            load_uri(u, resolve).ok()
        } else if let Some(bv) = idx(im, "bufferView") {
            arr(&j, "bufferViews").get(bv).and_then(|v| {
                let b = d.buffers.get(idx(v, "buffer").unwrap_or(0))?;
                let o = idx(v, "byteOffset").unwrap_or(0);
                b.get(o..o.checked_add(idx(v, "byteLength").unwrap_or(0))?).map(<[u8]>::to_vec)
            })
        } else {
            None
        };
        images.push(data.and_then(|b| Texture::decode(&b).ok()).map(Arc::new));
    }
    let wrap = |v: u64| match v {
        33071 => Wrap::Clamp,
        33648 => Wrap::Mirror,
        _ => Wrap::Repeat,
    };
    let mut textures: Vec<Arc<Texture>> = vec![];
    let mut tex_map: Vec<Option<usize>> = vec![];
    for t in arr(&j, "textures") {
        let Some(img) = idx(t, "source").and_then(|s| images.get(s).cloned().flatten()) else {
            tex_map.push(None);
            continue;
        };
        let samp = idx(t, "sampler").and_then(|s| arr(&j, "samplers").get(s));
        let ws = samp.and_then(|s| s.get("wrapS")).and_then(Value::as_u64).unwrap_or(10497);
        let wt = samp.and_then(|s| s.get("wrapT")).and_then(Value::as_u64).unwrap_or(10497);
        let w = [wrap(ws), wrap(wt)];
        let tex = if img.wrap == w { img } else { Arc::new(Texture { wrap: w, ..(*img).clone() }) };
        tex_map.push(Some(textures.len()));
        textures.push(tex);
    }
    let tref = |v: Option<&Value>| -> Option<TexRef> {
        let v = v?;
        let t = tex_map.get(idx(v, "index")?).copied().flatten()?;
        Some(TexRef { texture: t, uv_set: idx(v, "texCoord").unwrap_or(0) as u32 })
    };

    let mut materials = vec![];
    for m in arr(&j, "materials") {
        let pbr = m.get("pbrMetallicRoughness").cloned().unwrap_or(Value::Null);
        let bc = nums(&pbr, "baseColorFactor", [1.0; 4]);
        let ext = m.get("extensions");
        let strength = ext.and_then(|e| e.get("KHR_materials_emissive_strength")).map_or(1.0, |e| num(e, "emissiveStrength", 1.0));
        let em = nums(m, "emissiveFactor", [0.0; 3]);
        materials.push(Material {
            name: m.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
            base_color: bc.map(|v| v as f32),
            base_color_tex: tref(pbr.get("baseColorTexture")),
            metallic: num(&pbr, "metallicFactor", 1.0) as f32,
            roughness: num(&pbr, "roughnessFactor", 1.0) as f32,
            metallic_roughness_tex: tref(pbr.get("metallicRoughnessTexture")),
            normal_tex: tref(m.get("normalTexture")),
            normal_scale: m.get("normalTexture").map_or(1.0, |n| num(n, "scale", 1.0)) as f32,
            occlusion_tex: tref(m.get("occlusionTexture")),
            occlusion_strength: m.get("occlusionTexture").map_or(1.0, |n| num(n, "strength", 1.0)) as f32,
            emissive: [(em[0] * strength) as f32, (em[1] * strength) as f32, (em[2] * strength) as f32],
            emissive_tex: tref(m.get("emissiveTexture")),
            alpha_mode: match m.get("alphaMode").and_then(Value::as_str) {
                Some("MASK") => AlphaMode::Mask,
                Some("BLEND") => AlphaMode::Blend,
                _ => AlphaMode::Opaque,
            },
            alpha_cutoff: num(m, "alphaCutoff", 0.5) as f32,
            double_sided: m.get("doubleSided").and_then(Value::as_bool).unwrap_or(false),
            unlit: ext.and_then(|e| e.get("KHR_materials_unlit")).is_some(),
        });
    }

    let mut meshes = vec![];
    for m in arr(&j, "meshes") {
        let mut mesh = Mesh { name: m.get("name").and_then(Value::as_str).unwrap_or("").to_string(), primitives: vec![] };
        for p in arr(m, "primitives") {
            if let Some(prim) = primitive(&d, p, &materials)? {
                mesh.primitives.push(prim);
            }
        }
        meshes.push(mesh);
    }

    let mut nodes = vec![];
    for n in arr(&j, "nodes") {
        let matrix = n.get("matrix").and_then(Value::as_array).filter(|a| a.len() == 16).map(|a| {
            // Column-major in the file.
            let v: Vec<f64> = a.iter().map(|x| x.as_f64().unwrap_or(0.0)).collect();
            let mut m = [[0.0; 4]; 4];
            for c in 0..4 {
                for r in 0..4 {
                    m[r][c] = v[c * 4 + r];
                }
            }
            Mat4(m)
        });
        nodes.push(Node {
            name: n.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
            translation: nums(n, "translation", [0.0; 3]),
            rotation: nums(n, "rotation", [0.0, 0.0, 0.0, 1.0]),
            scale: nums(n, "scale", [1.0; 3]),
            matrix,
            mesh: idx(n, "mesh").filter(|&m| m < meshes.len()),
            skin: idx(n, "skin"),
            camera: idx(n, "camera"),
            light: n.get("extensions").and_then(|e| e.get("KHR_lights_punctual")).and_then(|e| idx(e, "light")),
            children: indices(n, "children"),
        });
    }

    let scene = idx(&j, "scene").unwrap_or(0);
    let roots: Vec<usize> = match arr(&j, "scenes").get(scene) {
        Some(s) => indices(s, "nodes"),
        None => {
            // No scene: every node that is nobody's child.
            let mut child = vec![false; nodes.len()];
            for n in &nodes {
                for &c in &n.children {
                    if c < child.len() {
                        child[c] = true;
                    }
                }
            }
            (0..nodes.len()).filter(|&i| !child[i]).collect()
        }
    };

    let mut skins = vec![];
    for s in arr(&j, "skins") {
        let joints: Vec<usize> = indices(s, "joints");
        let inverse_bind = match idx(s, "inverseBindMatrices") {
            Some(a) => {
                let (f, _) = d.floats(a)?;
                f.as_chunks::<16>()
                    .0
                    .iter()
                    .map(|v| {
                        let mut m = [[0.0; 4]; 4];
                        for c in 0..4 {
                            for r in 0..4 {
                                m[r][c] = v[c * 4 + r] as f64;
                            }
                        }
                        Mat4(m)
                    })
                    .collect()
            }
            None => vec![Mat4::IDENTITY; joints.len()],
        };
        skins.push(Skin { joints, inverse_bind });
    }

    let mut animations = vec![];
    for a in arr(&j, "animations") {
        let samplers = arr(a, "samplers");
        let mut anim = Animation { name: a.get("name").and_then(Value::as_str).unwrap_or("").to_string(), channels: vec![] };
        for c in arr(a, "channels") {
            let Some(t) = c.get("target") else { continue };
            let Some(node) = idx(t, "node") else { continue };
            let path = match t.get("path").and_then(Value::as_str) {
                Some("translation") => ChannelPath::Translation,
                Some("rotation") => ChannelPath::Rotation,
                Some("scale") => ChannelPath::Scale,
                _ => continue,
            };
            let Some(s) = idx(c, "sampler").and_then(|s| samplers.get(s)) else { continue };
            let interp = match s.get("interpolation").and_then(Value::as_str) {
                Some("STEP") => Interp::Step,
                Some("CUBICSPLINE") => Interp::CubicSpline,
                _ => Interp::Linear,
            };
            let (Some(input), Some(output)) = (idx(s, "input"), idx(s, "output")) else { continue };
            let (times, _) = d.floats(input)?;
            let (values, _) = d.floats(output)?;
            anim.channels.push(Channel { node, path, interp, times, values });
        }
        animations.push(anim);
    }

    let cameras = arr(&j, "cameras").iter().enumerate().map(|(i, c)| camera(c, i)).collect();
    let lights = j
        .get("extensions")
        .and_then(|e| e.get("KHR_lights_punctual"))
        .map(|e| arr(e, "lights").iter().enumerate().map(|(i, l)| light(l, i)).collect())
        .unwrap_or_default();
    Ok(Model { meshes, nodes, roots, materials, textures, skins, animations, cameras, lights })
}

fn name_or(v: &Value, d: String) -> String {
    v.get("name").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string).unwrap_or(d)
}

/// A glTF camera (glTF 2.0 §5.12): `perspective` {yfov, aspectRatio?, znear} or
/// `orthographic` {xmag, ymag}.
fn camera(c: &Value, i: usize) -> crate::ModelCamera {
    let projection = match (c.get("type").and_then(Value::as_str), c.get("orthographic")) {
        (Some("orthographic"), Some(o)) => crate::CameraProjection::Orthographic { xmag: num(o, "xmag", 1.0), ymag: num(o, "ymag", 1.0) },
        _ => {
            let p = c.get("perspective").cloned().unwrap_or(Value::Null);
            crate::CameraProjection::Perspective {
                yfov: num(&p, "yfov", 0.8).clamp(1e-3, 3.1),
                aspect: p.get("aspectRatio").and_then(Value::as_f64),
                znear: num(&p, "znear", 0.01),
            }
        }
    };
    crate::ModelCamera { name: name_or(c, format!("Camera {}", i + 1)), projection }
}

/// A `KHR_lights_punctual` light: type, color, intensity, range, spot cone angles.
fn light(l: &Value, i: usize) -> crate::ModelLight {
    let kind = match l.get("type").and_then(Value::as_str) {
        Some("directional") => crate::ModelLightKind::Directional,
        Some("spot") => {
            let s = l.get("spot").cloned().unwrap_or(Value::Null);
            let outer = num(&s, "outerConeAngle", std::f64::consts::FRAC_PI_4).clamp(1e-3, std::f64::consts::FRAC_PI_2);
            crate::ModelLightKind::Spot { inner: num(&s, "innerConeAngle", 0.0).clamp(0.0, outer), outer }
        }
        _ => crate::ModelLightKind::Point,
    };
    crate::ModelLight {
        name: name_or(l, format!("Light {}", i + 1)),
        kind,
        color: nums(l, "color", [1.0; 3]),
        intensity: num(l, "intensity", 1.0).max(0.0),
        range: l.get("range").and_then(Value::as_f64),
    }
}

fn primitive(d: &Doc, p: &Value, materials: &[Material]) -> Result<Option<Primitive>> {
    let mode = idx(p, "mode").unwrap_or(4);
    if !matches!(mode, 4..=6) {
        return Ok(None);
    }
    let attrs = p.get("attributes").ok_or_else(|| perr("primitive without attributes"))?;
    let Some(pa) = idx(attrs, "POSITION") else { return Ok(None) };
    let (pos, _) = d.floats(pa)?;
    let positions: Vec<[f32; 3]> = pos.as_chunks::<3>().0.iter().map(|c| [c[0], c[1], c[2]]).collect();
    let n = positions.len();
    let vec3s = |a: Option<usize>| -> Result<Vec<[f32; 3]>> {
        Ok(match a {
            Some(a) => d.floats(a)?.0.as_chunks::<3>().0.iter().map(|c| [c[0], c[1], c[2]]).collect(),
            None => vec![],
        })
    };
    let mut normals = vec3s(idx(attrs, "NORMAL"))?;
    let uvs: Vec<[f32; 2]> = match idx(attrs, "TEXCOORD_0") {
        Some(a) => d.floats(a)?.0.as_chunks::<2>().0.iter().map(|c| [c[0], c[1]]).collect(),
        None => vec![],
    };
    let tangents: Vec<[f32; 4]> = match idx(attrs, "TANGENT") {
        Some(a) => d.floats(a)?.0.as_chunks::<4>().0.iter().map(|c| [c[0], c[1], c[2], c[3]]).collect(),
        None => vec![],
    };
    let joints: Vec<[u16; 4]> = match idx(attrs, "JOINTS_0") {
        Some(a) => d.ints(a)?.as_chunks::<4>().0.iter().map(|c| [c[0] as u16, c[1] as u16, c[2] as u16, c[3] as u16]).collect(),
        None => vec![],
    };
    let weights: Vec<[f32; 4]> = match idx(attrs, "WEIGHTS_0") {
        Some(a) => d.floats(a)?.0.as_chunks::<4>().0.iter().map(|c| [c[0], c[1], c[2], c[3]]).collect(),
        None => vec![],
    };
    let raw: Vec<u32> = match idx(p, "indices") {
        Some(a) => d.ints(a)?,
        None => {
            // Generated indices are allocated like a read, once per referencing primitive.
            d.charge(n)?;
            (0..n as u32).collect()
        }
    };
    if raw.iter().any(|&i| i as usize >= n) {
        return Err(perr("index out of range"));
    }
    let mut indices = Vec::with_capacity(raw.len());
    match mode {
        5 => {
            for i in 2..raw.len() {
                let (a, b, c) = (raw[i - 2], raw[i - 1], raw[i]);
                if i % 2 == 0 { indices.extend([a, b, c]) } else { indices.extend([b, a, c]) }
            }
        }
        6 => {
            for i in 2..raw.len() {
                indices.extend([raw[0], raw[i - 1], raw[i]]);
            }
        }
        _ => indices.extend(raw.as_chunks::<3>().0.iter().flatten()),
    }
    // The triangle list is kept besides the raw indices (a strip or fan of k indices becomes
    // 3(k-2)), so it is charged like a read.
    d.charge(indices.len())?;
    if normals.len() != n {
        normals.clear();
    }
    let material = idx(p, "material").filter(|&m| m < materials.len());
    let skinned = joints.len() == n && weights.len() == n;
    if normals.is_empty() {
        // Flat normals unshare every vertex: position and normal, plus UVs and joints/weights.
        let per = 6 + if uvs.len() == n { 2 } else { 0 } + if skinned { 8 } else { 0 };
        d.charge_each(indices.len(), per)?;
    }
    let mut prim = Primitive {
        positions,
        normals,
        uvs: if uvs.len() == n { uvs } else { vec![] },
        tangents: if tangents.len() == n { tangents } else { vec![] },
        joints: if skinned { joints } else { vec![] },
        weights: if skinned { weights } else { vec![] },
        indices,
        material,
    };
    if prim.normals.is_empty() {
        prim.flat_normals();
        prim.tangents.clear();
    }
    let needs_tangents = material.is_some_and(|m| materials[m].normal_tex.is_some());
    if needs_tangents && prim.tangents.is_empty() {
        d.charge_each(prim.positions.len(), 4)?;
        prim.compute_tangents();
    }
    Ok(Some(prim))
}

#[cfg(test)]
mod tests {
    use super::uri_decode;

    #[test]
    fn uri_decode_rejects_signed_hex_pair() {
        assert_eq!(uri_decode("a%+1b"), "a%+1b");
        assert_eq!(uri_decode("a%-1b"), "a%-1b");
        assert_eq!(uri_decode("a%41b"), "aAb");
    }
}
