//! A `craft-mesh` scene (glTF, OBJ, STL or FBX, read there) as a [`Model`]: one primitive per
//! part with one index per vertex, decoded textures, and the normals and tangents the renderer
//! needs where the file has none.

use std::collections::HashMap;
use std::sync::Arc;

use craft_mesh as cm;

use crate::{
    AlphaMode, Animation, CameraProjection, Channel, ChannelPath, Interp, Mat4, Material, Mesh, Model, ModelCamera, ModelError, ModelLight, ModelLightKind,
    Node, Primitive, Result, Skin, TexRef, Texture, Wrap,
};

/// The bounds every reader keeps: `craft-mesh`'s defaults, with up to 2 GiB of buffers,
/// arrays and embedded images in one import.
pub(crate) fn limits() -> cm::Limits {
    cm::Limits { max_data_bytes: 2 << 30, ..cm::Limits::default() }
}

pub(crate) fn error(e: cm::Error) -> ModelError {
    match e {
        cm::Error::Missing(name) => ModelError::Missing(name),
        cm::Error::Unsupported(m) => ModelError::Unsupported(m),
        cm::Error::Malformed(m) | cm::Error::TooLarge(m) => ModelError::Parse(m),
    }
}

/// What a primitive without normals gets.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MissingNormals {
    /// Every triangle its own vertices and face normal (glTF 2.0 §3.7.2.1).
    Flat,
    /// Area-weighted vertex normals (FBX).
    Smooth,
    /// Smooth within an OBJ smoothing group, flat where smoothing is off.
    BySmoothing,
}

/// A column-major `craft-mesh` matrix as a [`Mat4`].
pub(crate) fn mat4(m: &cm::Mat4) -> Mat4 {
    let mut out = [[0.0; 4]; 4];
    for (c, col) in m.as_chunks::<4>().0.iter().enumerate() {
        for (r, v) in col.iter().enumerate() {
            out[r][c] = *v;
        }
    }
    Mat4(out)
}

fn wrap(w: cm::Wrap) -> Wrap {
    match w {
        cm::Wrap::Repeat => Wrap::Repeat,
        cm::Wrap::Clamp => Wrap::Clamp,
        cm::Wrap::Mirror => Wrap::Mirror,
    }
}

/// The scene as a model. `image` gives the bytes of an image the scene names but does not hold
/// (OBJ, which only names its textures); an image that cannot be read or decoded leaves its
/// material untextured, with a log line.
pub(crate) fn to_model(scene: cm::Scene, missing: MissingNormals, image: &dyn Fn(&cm::Image) -> Option<Vec<u8>>) -> Result<Model> {
    for w in &scene.warnings {
        log::warn!("model: {w}");
    }
    // Images decode once; each (image, wrap) a material uses is one texture.
    let decoded: Vec<Option<Arc<Texture>>> = scene
        .images
        .iter()
        .map(|im| {
            let bytes = im.data.clone().or_else(|| image(im));
            match bytes.map(|b| Texture::decode(&b)) {
                Some(Ok(t)) => Some(Arc::new(t)),
                Some(Err(e)) => {
                    log::warn!("model: cannot decode texture `{}`: {e}", im.uri.as_deref().unwrap_or(&im.name));
                    None
                }
                None => {
                    log::warn!("model: texture `{}` was not found next to the model", im.uri.as_deref().unwrap_or(&im.name));
                    None
                }
            }
        })
        .collect();
    let mut textures: Vec<Arc<Texture>> = vec![];
    let mut texture_of: HashMap<(usize, [Wrap; 2]), usize> = HashMap::new();
    let mut tref = |t: Option<cm::TextureRef>| -> Option<TexRef> {
        let t = t?;
        let img = decoded.get(t.image).cloned().flatten()?;
        let w = t.wrap.map(wrap);
        let texture = *texture_of.entry((t.image, w)).or_insert_with(|| {
            textures.push(if img.wrap == w { img } else { Arc::new(Texture { wrap: w, ..(*img).clone() }) });
            textures.len() - 1
        });
        Some(TexRef { texture, uv_set: t.uv_set })
    };
    let materials: Vec<Material> = scene
        .materials
        .iter()
        .map(|m| Material {
            name: m.name.clone(),
            base_color: m.base_color,
            base_color_tex: tref(m.base_color_texture),
            metallic: m.metallic,
            roughness: m.roughness,
            metallic_roughness_tex: tref(m.metallic_roughness_texture),
            normal_tex: tref(m.normal_texture),
            normal_scale: m.normal_scale,
            occlusion_tex: tref(m.occlusion_texture),
            occlusion_strength: m.occlusion_strength,
            emissive: m.emissive,
            emissive_tex: tref(m.emissive_texture),
            alpha_mode: match m.alpha_mode {
                cm::AlphaMode::Opaque => AlphaMode::Opaque,
                cm::AlphaMode::Mask(_) => AlphaMode::Mask,
                cm::AlphaMode::Blend => AlphaMode::Blend,
            },
            alpha_cutoff: if let cm::AlphaMode::Mask(c) = m.alpha_mode { c } else { 0.5 },
            double_sided: m.double_sided,
            unlit: m.unlit,
        })
        .collect();
    let mut adapter_bytes = 0u64;
    let meshes = scene.meshes.iter().map(|m| mesh(m, &materials, missing, &mut adapter_bytes)).collect::<Result<Vec<_>>>()?;
    let nodes = scene
        .nodes
        .iter()
        .map(|n| {
            let (translation, rotation, scale, matrix) = match n.trs {
                Some(t) => (t.translation, t.rotation, t.scale, None),
                None => ([0.0; 3], [0.0, 0.0, 0.0, 1.0], [1.0; 3], Some(mat4(&n.transform))),
            };
            Node {
                name: n.name.clone(),
                translation,
                rotation,
                scale,
                matrix,
                mesh: n.mesh.filter(|&m| m < scene.meshes.len()),
                skin: n.skin,
                camera: n.camera,
                light: n.light,
                children: n.children.clone(),
            }
        })
        .collect();
    let skins = scene
        .skins
        .iter()
        .map(|s| Skin {
            joints: s.joints.clone(),
            inverse_bind: if s.inverse_bind.is_empty() { vec![Mat4::IDENTITY; s.joints.len()] } else { s.inverse_bind.iter().map(mat4).collect() },
        })
        .collect();
    let animations = scene
        .animations
        .iter()
        .map(|a| Animation {
            name: a.name.clone(),
            channels: a
                .channels
                .iter()
                .map(|c| Channel {
                    node: c.node,
                    path: match c.path {
                        cm::ChannelPath::Translation => ChannelPath::Translation,
                        cm::ChannelPath::Rotation => ChannelPath::Rotation,
                        cm::ChannelPath::Scale => ChannelPath::Scale,
                    },
                    interp: match c.interpolation {
                        cm::Interpolation::Step => Interp::Step,
                        cm::Interpolation::Linear => Interp::Linear,
                        cm::Interpolation::CubicSpline => Interp::CubicSpline,
                    },
                    times: c.times.clone(),
                    values: c.values.clone(),
                })
                .collect(),
        })
        .collect();
    let name_or = |n: &str, d: String| if n.is_empty() { d } else { n.to_string() };
    let cameras = scene
        .cameras
        .iter()
        .enumerate()
        .map(|(i, c)| ModelCamera {
            name: name_or(&c.name, format!("Camera {}", i + 1)),
            projection: match c.projection {
                cm::Projection::Perspective { yfov, aspect, znear, .. } => CameraProjection::Perspective { yfov: yfov.clamp(1e-3, 3.1), aspect, znear },
                cm::Projection::Orthographic { xmag, ymag, .. } => CameraProjection::Orthographic { xmag, ymag },
            },
        })
        .collect();
    let lights = scene
        .lights
        .iter()
        .enumerate()
        .map(|(i, l)| ModelLight {
            name: name_or(&l.name, format!("Light {}", i + 1)),
            kind: match l.kind {
                cm::LightKind::Directional => ModelLightKind::Directional,
                cm::LightKind::Point => ModelLightKind::Point,
                cm::LightKind::Spot { inner, outer } => {
                    let outer = outer.clamp(1e-3, std::f64::consts::FRAC_PI_2);
                    ModelLightKind::Spot { inner: inner.clamp(0.0, outer), outer }
                }
            },
            color: l.color,
            intensity: l.intensity.max(0.0),
            range: l.range,
        })
        .collect();
    Ok(Model { meshes, nodes, roots: scene.roots, materials, textures, skins, animations, cameras, lights })
}

/// One primitive per part, each with one vertex per distinct corner of its triangles. Normals
/// and UVs are kept for a part when every corner of it has them; a triangle naming a vertex
/// that is not there is left out.
fn mesh(m: &cm::Mesh, materials: &[Material], missing: MissingNormals, adapter_bytes: &mut u64) -> Result<Mesh> {
    let skinned = !m.joints.is_empty() && m.joints.len() == m.positions.len() && m.weights.len() == m.positions.len();
    let uv_set = m.uv_sets.first();
    let smooth = smooth_normals(m, missing);
    // Include temporary hash keys and the peak duplicate produced by flat-normal expansion.
    // Account against the whole scene before building any primitive arrays.
    let projected = m
        .parts
        .iter()
        .try_fold(0u64, |sum, part| {
            let has_normals = part.corners.iter().any(|c| (c.normal as usize) < m.normals.len());
            let vertices =
                if missing == MissingNormals::Flat && !has_normals { (part.triangle_count() as u64).checked_mul(3)? } else { part.corners.len() as u64 };
            sum.checked_add(vertices.checked_mul(64)?)
        })
        .ok_or_else(|| ModelError::Parse("expanded mesh size overflow".into()))?;
    *adapter_bytes = adapter_bytes.checked_add(projected).ok_or_else(|| ModelError::Parse("expanded mesh size overflow".into()))?;
    if *adapter_bytes > (2u64 << 30) {
        return Err(ModelError::Parse("expanded mesh data exceeds the 2 GiB import limit".into()));
    }
    let mut primitives = vec![];
    for part in &m.parts {
        let normals = part.corners.iter().any(|c| (c.normal as usize) < m.normals.len());
        let tangents = part.has_tangents && m.tangents.len() == m.positions.len();
        let uvs = uv_set.filter(|set| part.corners.iter().any(|c| (c.uv as usize) < set.len()));
        let mut authored_normals = vec![];
        let mut ids: HashMap<cm::Corner, u32> = HashMap::new();
        let mut p = Primitive { material: part.material.filter(|&i| i < materials.len()), ..Primitive::default() };
        for t in part.triangles() {
            if t.iter().any(|c| c.position as usize >= m.positions.len()) {
                continue;
            }
            for c in t {
                let has_normal = (c.normal as usize) < m.normals.len();
                let has_uv = uvs.is_some_and(|set| (c.uv as usize) < set.len());
                let key = cm::Corner { position: c.position, normal: if has_normal { c.normal } else { cm::NONE }, uv: if has_uv { c.uv } else { cm::NONE } };
                let next = p.positions.len() as u32;
                let id = *ids.entry(key).or_insert_with(|| {
                    let at = c.position as usize;
                    p.positions.push(m.positions.get(at).copied().unwrap_or_default().map(|x| x as f32));
                    if normals {
                        let authored = m.normals.get(c.normal as usize).copied().map(|n| n.map(|x| x as f32));
                        let fallback = smooth.get(&(c.position, part.material, smooth_group(part, missing))).copied();
                        authored_normals.push(authored.or(fallback));
                        p.normals.push(authored.or(fallback).unwrap_or([0.0, 0.0, 1.0]));
                    } else if let Some(n) = smooth.get(&(c.position, part.material, smooth_group(part, missing))) {
                        p.normals.push(*n);
                    }
                    if tangents {
                        p.tangents.push(m.tangents.get(at).copied().unwrap_or_default().map(|x| x as f32));
                    }
                    if let Some(set) = uvs {
                        p.uvs.push(set.get(c.uv as usize).copied().unwrap_or_default());
                    }
                    if skinned {
                        p.joints.push(m.joints.get(at).copied().unwrap_or_default());
                        p.weights.push(m.weights.get(at).copied().unwrap_or_default());
                    }
                    next
                });
                p.indices.push(id);
            }
        }
        if p.indices.is_empty() {
            continue;
        }
        if normals && authored_normals.iter().any(Option::is_none) {
            // Keep authored corner normals, and fill only absent ones from the surrounding
            // triangles. The generated normals are smooth in a mixed-authorship part.
            if authored_normals.iter().any(Option::is_none) {
                p.compute_normals();
                for (n, authored) in p.normals.iter_mut().zip(authored_normals) {
                    if let Some(authored) = authored {
                        *n = authored;
                    }
                }
            }
        } else if p.normals.is_empty() {
            match missing {
                MissingNormals::Flat => p.flat_normals(),
                MissingNormals::BySmoothing if part.smoothing == 0 => p.flat_normals(),
                MissingNormals::Smooth | MissingNormals::BySmoothing => p.compute_normals(),
            }
        }
        if p.tangents.is_empty() && p.material.is_some_and(|i| materials.get(i).is_some_and(|mat| mat.normal_tex.is_some())) {
            p.compute_tangents();
        }
        primitives.push(p);
    }
    Ok(Mesh { name: m.name.clone(), primitives })
}

fn smooth_group(part: &cm::Part, missing: MissingNormals) -> u32 {
    match missing {
        MissingNormals::Smooth => 1,
        MissingNormals::BySmoothing => part.smoothing,
        MissingNormals::Flat => 0,
    }
}

/// Area weighted generated normals across parts with the same material and smoothing state.
fn smooth_normals(m: &cm::Mesh, missing: MissingNormals) -> HashMap<(u32, Option<usize>, u32), [f32; 3]> {
    let mut sums: HashMap<(u32, Option<usize>, u32), [f64; 3]> = HashMap::new();
    if missing == MissingNormals::Flat {
        return HashMap::new();
    }
    for part in &m.parts {
        let group = smooth_group(part, missing);
        if group == 0 {
            continue;
        }
        for tri in part.triangles() {
            let Some(a) = m.positions.get(tri[0].position as usize) else { continue };
            let Some(b) = m.positions.get(tri[1].position as usize) else { continue };
            let Some(c) = m.positions.get(tri[2].position as usize) else { continue };
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let face = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
            for corner in tri {
                if (corner.normal as usize) < m.normals.len() {
                    continue;
                }
                let n = sums.entry((corner.position, part.material, group)).or_default();
                for i in 0..3 {
                    n[i] += face[i];
                }
            }
        }
    }
    sums.into_iter()
        .map(|(key, n)| {
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            let n = if len > 0.0 { n.map(|x| (x / len) as f32) } else { [0.0, 0.0, 1.0] };
            (key, n)
        })
        .collect()
}
