//! Wavefront OBJ geometry and MTL materials.
//!
//! OBJ: `v` (optional vertex colours ignored), `vt`, `vn`, `f` with `v`, `v/vt`, `v//vn`,
//! `v/vt/vn` references (negative = relative), polygons fan-triangulated, `o`/`g` groups, `s`
//! smoothing (normals computed per smoothing state when a face has none), `usemtl`, `mtllib`.
//!
//! MTL materials map onto metallic-roughness: `Kd` → base colour, `d`/`Tr` → alpha, `Ke` →
//! emissive, `Pr`/`Pm` (the PBR extension) → roughness/metallic, otherwise roughness from the
//! Phong exponent `Ns` (`sqrt(2 / (Ns + 2))`) and metallic 0; `map_Kd`, `map_Ke`,
//! `map_Bump`/`bump`/`norm` load base colour, emissive and normal textures.

use std::collections::HashMap;
use std::sync::Arc;

use crate::{AlphaMode, Material, Mesh, Model, Primitive, Result, TexRef, Texture, perr};

/// The last component of a referenced path, treating `\` as a separator too: a path from a
/// machine of another kind (`C:\textures\checker.png`) still finds `checker.png` next to the
/// model (#1274). `None` when the name carries no path at all.
fn sibling_name(name: &str) -> Option<&str> {
    let file = name.rsplit(['/', '\\']).next()?;
    (!file.is_empty() && file.len() < name.len()).then_some(file)
}

/// A material reference resolved against the model's folder: the name as written (a name
/// containing spaces is one name), then its file name (an absolute path from another machine,
/// #1274), then the final token (the options convention, `-bm 1 file.png`).
fn resolve_reference(resolve: &dyn Fn(&str) -> Option<Vec<u8>>, name: &str) -> Option<Vec<u8>> {
    let mut tried: Vec<&str> = vec![];
    for candidate in [name, sibling_name(name).unwrap_or(name), name.split_whitespace().last().unwrap_or(name)] {
        if tried.contains(&candidate) {
            continue;
        }
        tried.push(candidate);
        if let Some(b) = resolve(candidate) {
            return Some(b);
        }
    }
    None
}

/// Parse MTL text into named materials (textures appended to `textures`).
pub fn parse_mtl(text: &str, resolve: &dyn Fn(&str) -> Option<Vec<u8>>, textures: &mut Vec<Arc<Texture>>) -> Vec<Material> {
    let mut out: Vec<Material> = vec![];
    let mut loaded: HashMap<String, Option<usize>> = HashMap::new();
    let mut tex = |name: &str, textures: &mut Vec<Arc<Texture>>| -> Option<TexRef> {
        let key = name.trim().to_string();
        let t = *loaded.entry(key.clone()).or_insert_with(|| {
            let Some(b) = resolve_reference(resolve, &key) else {
                log::warn!("model: texture `{key}` was not found next to the model");
                return None;
            };
            match Texture::decode(&b) {
                Ok(t) => {
                    textures.push(Arc::new(t));
                    Some(textures.len() - 1)
                }
                Err(e) => {
                    log::warn!("model: cannot decode texture `{key}`: {e}");
                    None
                }
            }
        });
        t.map(|texture| TexRef { texture, uv_set: 0 })
    };
    let mut ns_seen = false;
    let mut pr_seen = false;
    for line in text.lines() {
        let line = line.trim();
        let (key, rest) = line.split_once(char::is_whitespace).map(|(k, r)| (k, r.trim())).unwrap_or((line, ""));
        let f3 = || -> [f32; 3] {
            let v: Vec<f32> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            match v.len() {
                0 => [0.0; 3],
                1 | 2 => [v[0]; 3],
                _ => [v[0], v[1], v[2]],
            }
        };
        let f1 = || rest.split_whitespace().next().and_then(|x| x.parse::<f32>().ok());
        if key == "newmtl" {
            if let Some(m) = out.last_mut() {
                finish(m, ns_seen, pr_seen);
            }
            ns_seen = false;
            pr_seen = false;
            out.push(Material { name: rest.to_string(), metallic: 0.0, roughness: 0.5, ..Material::default() });
            continue;
        }
        let Some(m) = out.last_mut() else { continue };
        match key {
            "Kd" => {
                let c = f3();
                m.base_color = [srgb_to_linear(c[0]), srgb_to_linear(c[1]), srgb_to_linear(c[2]), m.base_color[3]];
            }
            "Ke" => m.emissive = f3().map(srgb_to_linear),
            "d" => m.base_color[3] = f1().unwrap_or(1.0),
            "Tr" => m.base_color[3] = 1.0 - f1().unwrap_or(0.0),
            "Ns" => {
                let ns = f1().unwrap_or(0.0).max(0.0);
                if !pr_seen {
                    m.roughness = (2.0 / (ns + 2.0)).sqrt();
                }
                ns_seen = true;
            }
            "Pr" => {
                m.roughness = f1().unwrap_or(0.5).clamp(0.0, 1.0);
                pr_seen = true;
            }
            "Pm" => m.metallic = f1().unwrap_or(0.0).clamp(0.0, 1.0),
            "map_Kd" => m.base_color_tex = tex(rest, textures),
            "map_Ke" => {
                m.emissive_tex = tex(rest, textures);
                if m.emissive == [0.0; 3] {
                    m.emissive = [1.0; 3];
                }
            }
            "map_Bump" | "map_bump" | "bump" | "norm" => m.normal_tex = tex(rest, textures),
            _ => {}
        }
    }
    if let Some(m) = out.last_mut() {
        finish(m, ns_seen, pr_seen);
    }
    out
}

fn finish(m: &mut Material, _ns: bool, _pr: bool) {
    if m.base_color[3] < 0.999 {
        m.alpha_mode = AlphaMode::Blend;
    }
}

/// sRGB-encoded component → linear (MTL colours are display colours).
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    v: u32,
    t: u32,
    n: u32,
    /// Smoothing state (faces without normals get computed normals per state).
    s: u32,
}

/// Parse OBJ text (and its MTL libraries through `resolve`).
pub fn parse(bytes: &[u8], resolve: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<Model> {
    let text = String::from_utf8_lossy(bytes);
    let mut v: Vec<[f32; 3]> = vec![];
    let mut vt: Vec<[f32; 2]> = vec![];
    let mut vn: Vec<[f32; 3]> = vec![];
    let mut materials: Vec<Material> = vec![];
    let mut textures: Vec<Arc<Texture>> = vec![];
    // One primitive per material, in order of first use.
    let mut prims: Vec<(Option<usize>, Primitive, HashMap<Key, u32>, Vec<bool>)> = vec![];
    let mut current: Option<usize> = None;
    let mut smooth = 0u32;
    let mut flat_id = 1u32 << 31;
    let mut faces = 0usize;
    // Join `\` line continuations.
    let joined = text.replace("\\\r\n", " ").replace("\\\n", " ");
    for (ln, line) in joined.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split_whitespace();
        let key = it.next().unwrap_or("");
        let rest: Vec<&str> = it.collect();
        let fl = |i: usize| rest.get(i).and_then(|x| x.parse::<f32>().ok());
        match key {
            "v" => v.push([fl(0).unwrap_or(0.0), fl(1).unwrap_or(0.0), fl(2).unwrap_or(0.0)]),
            "vt" => vt.push([fl(0).unwrap_or(0.0), fl(1).unwrap_or(0.0)]),
            "vn" => vn.push([fl(0).unwrap_or(0.0), fl(1).unwrap_or(0.0), fl(2).unwrap_or(1.0)]),
            "s" => {
                smooth = match rest.first().copied() {
                    None | Some("off") | Some("0") => 0,
                    Some(x) => x.parse().unwrap_or(1).max(1),
                }
            }
            "mtllib" => {
                let name = line["mtllib".len()..].trim();
                let mut found = false;
                for file in std::iter::once(name).chain(name.split_whitespace()).chain(sibling_name(name)) {
                    if let Some(b) = resolve(file) {
                        let mut ms = parse_mtl(&String::from_utf8_lossy(&b), resolve, &mut textures);
                        materials.append(&mut ms);
                        found = true;
                        break;
                    }
                }
                if !found && !name.is_empty() {
                    log::warn!("model: material library `{name}` was not found next to the model");
                }
            }
            "usemtl" => {
                let name = line["usemtl".len()..].trim();
                current = materials.iter().position(|m| m.name == name);
                if current.is_none() {
                    // Unknown material: a named grey default.
                    materials.push(Material { name: name.to_string(), metallic: 0.0, roughness: 0.5, base_color: [0.8, 0.8, 0.8, 1.0], ..Material::default() });
                    current = Some(materials.len() - 1);
                }
            }
            "f" => {
                if rest.len() < 3 {
                    continue;
                }
                faces += 1;
                let resolve_i = |s: &str, len: usize| -> Option<u32> {
                    if s.is_empty() {
                        return None;
                    }
                    let i: i64 = s.parse().ok()?;
                    let k = if i < 0 { len as i64 + i } else { i - 1 };
                    (k >= 0 && (k as usize) < len).then_some(k as u32)
                };
                let mut refs = vec![];
                for r in &rest {
                    let mut parts = r.split('/');
                    let vi = resolve_i(parts.next().unwrap_or(""), v.len()).ok_or_else(|| perr(format!("line {}: bad vertex index `{r}`", ln + 1)))?;
                    let ti = parts.next().and_then(|s| resolve_i(s, vt.len()));
                    let ni = parts.next().and_then(|s| resolve_i(s, vn.len()));
                    refs.push((vi, ti, ni));
                }
                let has_n = refs.iter().all(|r| r.2.is_some());
                // Faces without normals: shared within a smoothing group, unique otherwise.
                let s = if has_n {
                    0
                } else if smooth == 0 {
                    flat_id += 1;
                    flat_id
                } else {
                    smooth
                };
                let slot = match prims.iter().position(|p| p.0 == current) {
                    Some(i) => i,
                    None => {
                        prims.push((current, Primitive { material: current, ..Default::default() }, HashMap::new(), vec![]));
                        prims.len() - 1
                    }
                };
                let (_, prim, map, need) = &mut prims[slot];
                let mut ids = vec![];
                for (vi, ti, ni) in refs {
                    let k = Key { v: vi, t: ti.unwrap_or(u32::MAX), n: ni.unwrap_or(u32::MAX), s };
                    let id = *map.entry(k).or_insert_with(|| {
                        prim.positions.push(v[vi as usize]);
                        prim.uvs.push(ti.map_or([0.0, 0.0], |t| vt[t as usize]));
                        prim.normals.push(ni.map_or([0.0; 3], |n| vn[n as usize]));
                        need.push(ni.is_none());
                        (prim.positions.len() - 1) as u32
                    });
                    ids.push(id);
                }
                for i in 2..ids.len() {
                    prim.indices.extend([ids[0], ids[i - 1], ids[i]]);
                }
            }
            _ => {}
        }
    }
    if faces == 0 {
        return Err(perr("OBJ file without faces"));
    }
    let any_uv = !vt.is_empty();
    let mut mesh = Mesh { name: String::new(), primitives: vec![] };
    for (_, mut p, _, need) in prims {
        if need.iter().any(|&b| b) {
            // Fill missing normals from the triangles (area weighted), keep given ones.
            let given = p.normals.clone();
            p.compute_normals();
            for (i, n) in p.normals.iter_mut().enumerate() {
                if !need[i] {
                    *n = given[i];
                }
            }
        }
        if !any_uv {
            p.uvs.clear();
        } else {
            // OBJ's v axis points up the image; textures are stored top row first.
            for uv in p.uvs.iter_mut() {
                uv[1] = 1.0 - uv[1];
            }
        }
        if p.material.is_some_and(|m| materials[m].normal_tex.is_some()) {
            p.compute_tangents();
        }
        mesh.primitives.push(p);
    }
    Ok(Model::single(mesh, materials).with_textures(textures))
}

impl Model {
    fn with_textures(mut self, t: Vec<Arc<Texture>>) -> Model {
        self.textures = t;
        self
    }
}
