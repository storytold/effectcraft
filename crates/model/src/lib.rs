//! 3D models for the Advanced 3D renderer.
//!
//! - [`gltf`]: glTF 2.0 import (`.gltf` with external or embedded buffers, `.glb`), written from
//!   the Khronos glTF 2.0 specification: meshes, PBR metallic-roughness materials (base colour,
//!   metallic-roughness, normal, occlusion and emissive maps), the node hierarchy, skins and
//!   animations.
//! - [`obj`]: Wavefront OBJ geometry with MTL materials (mapped onto the metallic-roughness model).
//! - [`stl`]: STL meshes (binary and ASCII, with the binary per-facet and part colours).
//! - [`fbx`]: FBX 7.x scenes, binary and ASCII: meshes, the Model hierarchy, Lambert/Phong
//!   materials and their textures.
//! - [`prim`]: parametric primitives (cube, sphere, plane, torus, cone, cylinder).
//! - [`extrude`]: extruded, bevelled meshes from 2D outlines (text and shape layers), with
//!   [`triangulate`] for the caps.
//!
//! Models are in their own units with +Y up (glTF's convention); the renderer maps them into
//! After Effects' Y-down pixel space.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod extrude;
pub mod fbx;
pub mod gltf;
pub mod obj;
pub mod prim;
pub mod stl;
pub mod triangulate;

use std::sync::Arc;

pub use effectcraft_geom::{Mat4, Vec3, vec3};

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("{0}")]
    Parse(String),
    #[error("missing resource `{0}`")]
    Missing(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, ModelError>;

pub(crate) fn perr(s: impl Into<String>) -> ModelError {
    ModelError::Parse(s.into())
}

/// Texture coordinate wrapping (glTF sampler wrap modes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Wrap {
    #[default]
    Repeat,
    Clamp,
    Mirror,
}

/// A decoded texture: straight RGBA, 0..1, as stored in the file (colour textures are still
/// sRGB-encoded; the renderer linearises them).
#[derive(Clone, Debug, PartialEq)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub data: Vec<[f32; 4]>,
    pub wrap: [Wrap; 2],
}

impl Texture {
    pub fn solid(c: [f32; 4]) -> Texture {
        Texture { width: 1, height: 1, data: vec![c], wrap: [Wrap::Repeat; 2] }
    }
    /// Decode a PNG or JPEG.
    pub fn decode(bytes: &[u8]) -> Result<Texture> {
        let img = image::load_from_memory(bytes).map_err(|e| perr(format!("image: {e}")))?.to_rgba32f();
        let (w, h) = (img.width(), img.height());
        let data = img.pixels().map(|p| p.0).collect();
        Ok(Texture { width: w, height: h, data, wrap: [Wrap::Repeat; 2] })
    }
}

/// How a material's alpha is used (glTF `alphaMode`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AlphaMode {
    #[default]
    Opaque,
    Mask,
    Blend,
}

/// A texture reference of a material.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TexRef {
    pub texture: usize,
    /// TEXCOORD set (only set 0 is imported; others fall back to set 0).
    pub uv_set: u32,
}

/// A metallic-roughness PBR material (glTF 2.0 core material model).
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    pub name: String,
    /// Linear RGBA factor.
    pub base_color: [f32; 4],
    pub base_color_tex: Option<TexRef>,
    pub metallic: f32,
    pub roughness: f32,
    /// Roughness in G, metalness in B.
    pub metallic_roughness_tex: Option<TexRef>,
    pub normal_tex: Option<TexRef>,
    pub normal_scale: f32,
    /// Occlusion in R.
    pub occlusion_tex: Option<TexRef>,
    pub occlusion_strength: f32,
    /// Linear emissive factor (× KHR_materials_emissive_strength).
    pub emissive: [f32; 3],
    pub emissive_tex: Option<TexRef>,
    pub alpha_mode: AlphaMode,
    pub alpha_cutoff: f32,
    pub double_sided: bool,
    /// KHR_materials_unlit.
    pub unlit: bool,
}

impl Default for Material {
    fn default() -> Self {
        Material {
            name: String::new(),
            base_color: [1.0; 4],
            base_color_tex: None,
            metallic: 1.0,
            roughness: 1.0,
            metallic_roughness_tex: None,
            normal_tex: None,
            normal_scale: 1.0,
            occlusion_tex: None,
            occlusion_strength: 1.0,
            emissive: [0.0; 3],
            emissive_tex: None,
            alpha_mode: AlphaMode::Opaque,
            alpha_cutoff: 0.5,
            double_sided: false,
            unlit: false,
        }
    }
}

impl Material {
    /// A plain dielectric material of a colour (primitives, OBJ without MTL).
    pub fn plain(base: [f32; 4], metallic: f32, roughness: f32) -> Material {
        Material { base_color: base, metallic, roughness, ..Material::default() }
    }
}

/// Triangles with one material.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Primitive {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// TEXCOORD_0 (empty when the primitive has none).
    pub uvs: Vec<[f32; 2]>,
    /// Tangents (xyz, w = bitangent sign); empty when not needed.
    pub tangents: Vec<[f32; 4]>,
    /// JOINTS_0 / WEIGHTS_0 (skinned meshes; empty otherwise).
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    /// Triangle list.
    pub indices: Vec<u32>,
    pub material: Option<usize>,
}

impl Primitive {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
    /// Smooth vertex normals from the triangles (area weighted), for meshes without normals.
    pub fn compute_normals(&mut self) {
        let mut n = vec![[0.0f32; 3]; self.positions.len()];
        for t in self.indices.as_chunks::<3>().0 {
            let (a, b, c) = (self.positions[t[0] as usize], self.positions[t[1] as usize], self.positions[t[2] as usize]);
            let f = cross(sub(b, a), sub(c, a));
            for &i in t {
                let v = &mut n[i as usize];
                v[0] += f[0];
                v[1] += f[1];
                v[2] += f[2];
            }
        }
        self.normals = n.into_iter().map(|v| normalize(v).unwrap_or([0.0, 0.0, 1.0])).collect();
    }
    /// Split every triangle into its own vertices with a face normal (glTF: flat normals when a
    /// primitive has none).
    pub fn flat_normals(&mut self) {
        let mut p = Primitive { material: self.material, ..Default::default() };
        for t in self.indices.as_chunks::<3>().0 {
            let (a, b, c) = (self.positions[t[0] as usize], self.positions[t[1] as usize], self.positions[t[2] as usize]);
            let f = normalize(cross(sub(b, a), sub(c, a))).unwrap_or([0.0, 0.0, 1.0]);
            for &i in t {
                let i = i as usize;
                p.indices.push(p.positions.len() as u32);
                p.positions.push(self.positions[i]);
                p.normals.push(f);
                if !self.uvs.is_empty() {
                    p.uvs.push(self.uvs[i]);
                }
                if !self.joints.is_empty() {
                    p.joints.push(self.joints[i]);
                    p.weights.push(self.weights[i]);
                }
            }
        }
        *self = p;
    }
    /// Per-vertex tangents from the UV layout (accumulated per triangle, Gram-Schmidt
    /// orthogonalised against the normal, handedness in w).
    pub fn compute_tangents(&mut self) {
        let n = self.positions.len();
        if self.uvs.len() != n || self.normals.len() != n {
            return;
        }
        let mut tan = vec![[0.0f32; 3]; n];
        let mut bit = vec![[0.0f32; 3]; n];
        for t in self.indices.as_chunks::<3>().0 {
            let (i0, i1, i2) = (t[0] as usize, t[1] as usize, t[2] as usize);
            let (p0, p1, p2) = (self.positions[i0], self.positions[i1], self.positions[i2]);
            let (w0, w1, w2) = (self.uvs[i0], self.uvs[i1], self.uvs[i2]);
            let e1 = sub(p1, p0);
            let e2 = sub(p2, p0);
            let (du1, dv1, du2, dv2) = (w1[0] - w0[0], w1[1] - w0[1], w2[0] - w0[0], w2[1] - w0[1]);
            let det = du1 * dv2 - du2 * dv1;
            if det.abs() < 1e-12 {
                continue;
            }
            let r = 1.0 / det;
            let sdir = [(e1[0] * dv2 - e2[0] * dv1) * r, (e1[1] * dv2 - e2[1] * dv1) * r, (e1[2] * dv2 - e2[2] * dv1) * r];
            let tdir = [(e2[0] * du1 - e1[0] * du2) * r, (e2[1] * du1 - e1[1] * du2) * r, (e2[2] * du1 - e1[2] * du2) * r];
            for i in [i0, i1, i2] {
                for k in 0..3 {
                    tan[i][k] += sdir[k];
                    bit[i][k] += tdir[k];
                }
            }
        }
        self.tangents = (0..n)
            .map(|i| {
                let nn = self.normals[i];
                let t = tan[i];
                let d = dot(nn, t);
                let o = [t[0] - nn[0] * d, t[1] - nn[1] * d, t[2] - nn[2] * d];
                let o = normalize(o).unwrap_or_else(|| any_perpendicular(nn));
                let w = if dot(cross(nn, o), bit[i]) < 0.0 { -1.0 } else { 1.0 };
                [o[0], o[1], o[2], w]
            })
            .collect();
    }
    /// Axis-aligned bounds of the positions.
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let mut it = self.positions.iter();
        let f = *it.next()?;
        let (mut lo, mut hi) = (f, f);
        for p in it {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        Some((lo, hi))
    }
}

pub(crate) fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub(crate) fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub(crate) fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub(crate) fn normalize(a: [f32; 3]) -> Option<[f32; 3]> {
    let l = dot(a, a).sqrt();
    (l > 1e-20 && l.is_finite()).then(|| [a[0] / l, a[1] / l, a[2] / l])
}
fn any_perpendicular(n: [f32; 3]) -> [f32; 3] {
    let a = if n[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    normalize(cross(n, a)).unwrap_or([1.0, 0.0, 0.0])
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub name: String,
    pub primitives: Vec<Primitive>,
}

/// A node of the scene hierarchy: local transform (TRS or a matrix), an optional mesh and skin.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub name: String,
    pub translation: [f64; 3],
    /// Unit quaternion (x, y, z, w).
    pub rotation: [f64; 4],
    pub scale: [f64; 3],
    /// A fixed local matrix (nodes given by `matrix` are not animated).
    pub matrix: Option<Mat4>,
    pub mesh: Option<usize>,
    pub skin: Option<usize>,
    /// A camera of [`Model::cameras`] at this node (looking down the node's −Z, +Y up).
    pub camera: Option<usize>,
    /// A light of [`Model::lights`] at this node (`KHR_lights_punctual`, shining down −Z).
    pub light: Option<usize>,
    pub children: Vec<usize>,
}

impl Default for Node {
    fn default() -> Self {
        Node {
            name: String::new(),
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            matrix: None,
            mesh: None,
            skin: None,
            camera: None,
            light: None,
            children: vec![],
        }
    }
}

impl Node {
    pub fn local(&self) -> Mat4 {
        self.matrix.unwrap_or_else(|| trs(self.translation, self.rotation, self.scale))
    }
}

/// Translation × rotation (quaternion x, y, z, w) × scale.
pub fn trs(t: [f64; 3], q: [f64; 4], s: [f64; 3]) -> Mat4 {
    let r = quat_matrix(q);
    let mut m = r;
    for i in 0..3 {
        for j in 0..3 {
            m.0[i][j] *= s[j];
        }
        m.0[i][3] = t[i];
    }
    m
}

/// Rotation matrix of a quaternion (x, y, z, w); normalised first.
pub fn quat_matrix(q: [f64; 4]) -> Mat4 {
    let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    let [x, y, z, w] = if l > 1e-12 { [q[0] / l, q[1] / l, q[2] / l, q[3] / l] } else { [0.0, 0.0, 0.0, 1.0] };
    Mat4([
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w), 0.0],
        [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w), 0.0],
        [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y), 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ])
}

/// Spherical linear interpolation of unit quaternions (shortest path).
pub fn slerp(a: [f64; 4], b: [f64; 4], t: f64) -> [f64; 4] {
    let mut d = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let mut b = b;
    if d < 0.0 {
        d = -d;
        b = [-b[0], -b[1], -b[2], -b[3]];
    }
    let (ka, kb) = if d > 0.9995 {
        (1.0 - t, t)
    } else {
        let th = d.clamp(-1.0, 1.0).acos();
        let s = th.sin();
        (((1.0 - t) * th).sin() / s, (t * th).sin() / s)
    };
    let q = [a[0] * ka + b[0] * kb, a[1] * ka + b[1] * kb, a[2] * ka + b[2] * kb, a[3] * ka + b[3] * kb];
    let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt().max(1e-12);
    [q[0] / l, q[1] / l, q[2] / l, q[3] / l]
}

/// A skin: joint nodes and their inverse bind matrices.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Skin {
    pub joints: Vec<usize>,
    pub inverse_bind: Vec<Mat4>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelPath {
    Translation,
    Rotation,
    Scale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interp {
    Step,
    Linear,
    CubicSpline,
}

/// One animated node property.
#[derive(Clone, Debug, PartialEq)]
pub struct Channel {
    pub node: usize,
    pub path: ChannelPath,
    pub interp: Interp,
    /// Key times (seconds).
    pub times: Vec<f32>,
    /// Values, `width` floats per key (cubic spline: in-tangent, value, out-tangent per key).
    pub values: Vec<f32>,
}

impl Channel {
    fn width(&self) -> usize {
        match self.path {
            ChannelPath::Rotation => 4,
            _ => 3,
        }
    }
    /// The value at time `t` (seconds), clamped to the key range.
    pub fn sample(&self, t: f64) -> Option<[f64; 4]> {
        let n = self.times.len();
        let w = self.width();
        let stride = if self.interp == Interp::CubicSpline { 3 * w } else { w };
        let off = if self.interp == Interp::CubicSpline { w } else { 0 };
        if n == 0 || self.values.len() < n * stride {
            return None;
        }
        let get = |k: usize, o: usize| -> [f64; 4] {
            let mut v = [0.0; 4];
            for i in 0..w {
                v[i] = self.values[k * stride + o + i] as f64;
            }
            v
        };
        let t0 = self.times[0] as f64;
        if t <= t0 || n == 1 {
            return Some(get(0, off));
        }
        if t >= self.times[n - 1] as f64 {
            return Some(get(n - 1, off));
        }
        let k = self.times.partition_point(|&x| (x as f64) <= t).saturating_sub(1).min(n - 2);
        let (ta, tb) = (self.times[k] as f64, self.times[k + 1] as f64);
        let dt = (tb - ta).max(1e-9);
        let u = ((t - ta) / dt).clamp(0.0, 1.0);
        let (a, b) = (get(k, off), get(k + 1, off));
        let v = match self.interp {
            Interp::Step => a,
            Interp::Linear => {
                if self.path == ChannelPath::Rotation {
                    slerp(a, b, u)
                } else {
                    let mut v = [0.0; 4];
                    for i in 0..w {
                        v[i] = a[i] + (b[i] - a[i]) * u;
                    }
                    v
                }
            }
            Interp::CubicSpline => {
                // Hermite spline with tangents scaled by the key interval (glTF Appendix C).
                let out_a = get(k, 2 * w);
                let in_b = get(k + 1, 0);
                let (u2, u3) = (u * u, u * u * u);
                let h00 = 2.0 * u3 - 3.0 * u2 + 1.0;
                let h10 = u3 - 2.0 * u2 + u;
                let h01 = -2.0 * u3 + 3.0 * u2;
                let h11 = u3 - u2;
                let mut v = [0.0; 4];
                for i in 0..w {
                    v[i] = h00 * a[i] + h10 * dt * out_a[i] + h01 * b[i] + h11 * dt * in_b[i];
                }
                if self.path == ChannelPath::Rotation {
                    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2] + v[3] * v[3]).sqrt().max(1e-12);
                    for c in v.iter_mut() {
                        *c /= l;
                    }
                }
                v
            }
        };
        Some(v)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Animation {
    pub name: String,
    pub channels: Vec<Channel>,
}

impl Animation {
    /// Length in seconds (the last key time of any channel).
    pub fn duration(&self) -> f64 {
        self.channels.iter().filter_map(|c| c.times.last()).fold(0.0f64, |a, &b| a.max(b as f64))
    }
}

/// An imported model (one glTF scene, one OBJ file, or a primitive).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    pub meshes: Vec<Mesh>,
    pub nodes: Vec<Node>,
    /// Root nodes of the displayed scene.
    pub roots: Vec<usize>,
    pub materials: Vec<Material>,
    pub textures: Vec<Arc<Texture>>,
    pub skins: Vec<Skin>,
    pub animations: Vec<Animation>,
    /// Cameras embedded in the file (glTF `cameras`).
    pub cameras: Vec<ModelCamera>,
    /// Punctual lights embedded in the file (glTF `KHR_lights_punctual`).
    pub lights: Vec<ModelLight>,
}

/// A camera embedded in a model file.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelCamera {
    pub name: String,
    pub projection: CameraProjection,
}

/// glTF camera projections.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CameraProjection {
    /// Vertical field of view (radians), optional aspect ratio, near plane.
    Perspective { yfov: f64, aspect: Option<f64>, znear: f64 },
    /// Half the orthographic view's width and height (model units).
    Orthographic { xmag: f64, ymag: f64 },
}

/// Kinds of `KHR_lights_punctual` lights.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ModelLightKind {
    Directional,
    Point,
    /// Inner and outer cone angles (radians, from the axis).
    Spot {
        inner: f64,
        outer: f64,
    },
}

/// A punctual light embedded in a model file.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelLight {
    pub name: String,
    pub kind: ModelLightKind,
    /// Linear RGB.
    pub color: [f64; 3],
    /// Candela (point, spot) or lux (directional).
    pub intensity: f64,
    pub range: Option<f64>,
}

/// A camera or light placed in model space by [`Model::placed`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    /// Index into [`Model::cameras`] or [`Model::lights`].
    pub index: usize,
    pub node: usize,
    /// Node → model space.
    pub world: Mat4,
}

/// One mesh placed in model space by [`Model::instances`].
#[derive(Clone, Debug)]
pub struct Instance<'m> {
    pub mesh: &'m Mesh,
    /// Node → model space.
    pub world: Mat4,
    /// Skinned meshes: joint matrices (model space, `joint_world × inverse_bind`).
    pub joints: Option<Vec<Mat4>>,
}

impl Model {
    /// A model of one mesh at the root.
    pub fn single(mesh: Mesh, materials: Vec<Material>) -> Model {
        Model { meshes: vec![mesh], nodes: vec![Node { mesh: Some(0), ..Node::default() }], roots: vec![0], materials, ..Model::default() }
    }

    /// Local node matrices at animation `clip` time `t` (seconds); `None` = rest pose.
    pub fn local_pose(&self, clip: Option<usize>, t: f64) -> Vec<Mat4> {
        let mut trs_v: Vec<([f64; 3], [f64; 4], [f64; 3])> = self.nodes.iter().map(|n| (n.translation, n.rotation, n.scale)).collect();
        if let Some(a) = clip.and_then(|c| self.animations.get(c)) {
            for ch in &a.channels {
                let (Some(v), Some(e)) = (ch.sample(t), trs_v.get_mut(ch.node)) else { continue };
                match ch.path {
                    ChannelPath::Translation => e.0 = [v[0], v[1], v[2]],
                    ChannelPath::Rotation => e.1 = v,
                    ChannelPath::Scale => e.2 = [v[0], v[1], v[2]],
                }
            }
        }
        self.nodes.iter().zip(trs_v).map(|(n, (t, r, s))| n.matrix.unwrap_or_else(|| trs(t, r, s))).collect()
    }

    /// Model-space matrices of every node (hierarchy applied) for a local pose.
    pub fn world_pose(&self, local: &[Mat4]) -> Vec<Mat4> {
        let mut world = vec![Mat4::IDENTITY; self.nodes.len()];
        let mut seen = vec![false; self.nodes.len()];
        let mut stack: Vec<(usize, Mat4)> = self.roots.iter().map(|&r| (r, Mat4::IDENTITY)).collect();
        while let Some((i, parent)) = stack.pop() {
            if i >= self.nodes.len() || seen[i] {
                continue;
            }
            seen[i] = true;
            let w = parent * local[i];
            world[i] = w;
            for &c in &self.nodes[i].children {
                stack.push((c, w));
            }
        }
        world
    }

    /// The meshes to draw at animation `clip` time `t`.
    pub fn instances(&self, clip: Option<usize>, t: f64) -> Vec<Instance<'_>> {
        let local = self.local_pose(clip, t);
        let world = self.world_pose(&local);
        let mut seen = vec![false; self.nodes.len()];
        let mut stack: Vec<usize> = self.roots.clone();
        let mut out = vec![];
        while let Some(i) = stack.pop() {
            if i >= self.nodes.len() || seen[i] {
                continue;
            }
            seen[i] = true;
            let n = &self.nodes[i];
            if let Some(m) = n.mesh.and_then(|m| self.meshes.get(m)) {
                let joints = n.skin.and_then(|s| self.skins.get(s)).map(|s| {
                    s.joints
                        .iter()
                        .enumerate()
                        .map(|(k, &j)| world.get(j).copied().unwrap_or(Mat4::IDENTITY) * s.inverse_bind.get(k).copied().unwrap_or(Mat4::IDENTITY))
                        .collect()
                });
                // Skinned vertices are placed by their joints alone (glTF: the node transform of
                // a skinned mesh is ignored).
                let w = if joints.is_some() { Mat4::IDENTITY } else { world[i] };
                out.push(Instance { mesh: m, world: w, joints });
            }
            stack.extend(n.children.iter().rev());
        }
        out
    }

    /// The scene's cameras (`lights = false`) or lights (`true`) placed at animation `clip`
    /// time `t`, in node order.
    pub fn placed(&self, lights: bool, clip: Option<usize>, t: f64) -> Vec<Placed> {
        let world = self.world_pose(&self.local_pose(clip, t));
        let mut reach = vec![false; self.nodes.len()];
        let mut stack: Vec<usize> = self.roots.clone();
        while let Some(i) = stack.pop() {
            if i >= self.nodes.len() || reach[i] {
                continue;
            }
            reach[i] = true;
            stack.extend(self.nodes[i].children.iter().copied());
        }
        let count = if lights { self.lights.len() } else { self.cameras.len() };
        self.nodes
            .iter()
            .enumerate()
            .filter(|(i, _)| reach[*i])
            .filter_map(|(i, n)| {
                let index = if lights { n.light } else { n.camera }?;
                (index < count).then_some(Placed { index, node: i, world: world[i] })
            })
            .collect()
    }

    /// Model-space bounds of the rest pose (None for an empty model).
    pub fn bounds(&self) -> Option<([f64; 3], [f64; 3])> {
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for inst in self.instances(None, 0.0) {
            for p in &inst.mesh.primitives {
                for (k, v) in p.positions.iter().enumerate() {
                    let q = skin_point(&inst, p, k, *v);
                    for c in 0..3 {
                        lo[c] = lo[c].min(q[c]);
                        hi[c] = hi[c].max(q[c]);
                    }
                }
            }
        }
        lo[0].is_finite().then_some((lo, hi))
    }

    pub fn vertex_count(&self) -> usize {
        self.meshes.iter().flat_map(|m| &m.primitives).map(|p| p.positions.len()).sum()
    }
    pub fn triangle_count(&self) -> usize {
        self.meshes.iter().flat_map(|m| &m.primitives).map(|p| p.triangle_count()).sum()
    }
}

/// The skinning (or node) matrix of vertex `k` of an instance's primitive.
pub fn vertex_matrix(inst: &Instance, p: &Primitive, k: usize) -> Mat4 {
    let Some(j) = &inst.joints else { return inst.world };
    let (Some(ji), Some(wi)) = (p.joints.get(k), p.weights.get(k)) else { return inst.world };
    let mut m = [[0.0; 4]; 4];
    let mut total = 0.0;
    for c in 0..4 {
        let w = wi[c] as f64;
        if w == 0.0 {
            continue;
        }
        let Some(jm) = j.get(ji[c] as usize) else { continue };
        total += w;
        for r in 0..4 {
            for q in 0..4 {
                m[r][q] += jm.0[r][q] * w;
            }
        }
    }
    if total <= 1e-9 { inst.world } else { Mat4(m) }
}

/// A vertex position of an instance in model space.
pub fn skin_point(inst: &Instance, p: &Primitive, k: usize, v: [f32; 3]) -> [f64; 3] {
    let q = vertex_matrix(inst, p, k).apply(vec3(v[0] as f64, v[1] as f64, v[2] as f64));
    [q.x, q.y, q.z]
}

/// Which loader handles a file name (`gltf`, `glb`, `obj`, `stl`, `fbx`), by extension.
pub fn format_of(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    match ext.as_str() {
        "gltf" => Some("gltf"),
        "glb" => Some("glb"),
        "obj" => Some("obj"),
        "stl" => Some("stl"),
        "fbx" => Some("fbx"),
        _ => None,
    }
}

/// Load a model from its main file's bytes; `resolve(uri)` reads sibling resources (buffers,
/// textures, MTL files) by their relative URI.
pub fn load(path: &str, bytes: &[u8], resolve: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<Model> {
    match format_of(path) {
        Some("obj") => obj::parse(bytes, resolve),
        Some("stl") => stl::parse(bytes),
        Some("fbx") => fbx::parse(bytes, resolve),
        Some(_) => gltf::parse(bytes, resolve),
        None if bytes.starts_with(b"glTF") => gltf::parse(bytes, resolve),
        None if bytes.starts_with(fbx::BINARY_MAGIC) => fbx::parse(bytes, resolve),
        None => Err(ModelError::Unsupported(format!("{path}: not a glTF, OBJ, STL or FBX file"))),
    }
}

/// Summary for footage metadata (Project panel, Interpret Footage).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Summary {
    pub meshes: usize,
    pub vertices: usize,
    pub triangles: usize,
    pub materials: usize,
    pub textures: usize,
    pub animations: Vec<(String, f64)>,
    pub skinned: bool,
    /// Rest-pose bounds (model units).
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Model {
    pub fn summary(&self) -> Summary {
        let (min, max) = self.bounds().unwrap_or(([0.0; 3], [0.0; 3]));
        Summary {
            meshes: self.meshes.len(),
            vertices: self.vertex_count(),
            triangles: self.triangle_count(),
            materials: self.materials.len(),
            textures: self.textures.len(),
            animations: self.animations.iter().map(|a| (a.name.clone(), a.duration())).collect(),
            skinned: !self.skins.is_empty(),
            min,
            max,
        }
    }
}

#[cfg(test)]
mod tests;
