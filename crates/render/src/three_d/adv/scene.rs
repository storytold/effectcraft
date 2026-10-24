//! The Advanced 3D scene: everything a rasteriser needs, flattened into world-space triangles
//! and plain arrays (identical input for the CPU rasteriser and the GPU pipeline).
//!
//! - Model and primitive layers: their meshes (glTF/OBJ models are +Y up in model units and
//!   are mapped into the layer by `Model Scale` with Y and Z flipped, a 180° turn about X).
//! - Extruded text and shape layers (Geometry Options ▸ Extrusion Depth > 0): bevelled meshes.
//! - Every other 3D layer (footage, solids, comps, flat text and shapes): a textured card of
//!   its processed buffer, lit with its Material Options.
//! - Lights from the comp's light layers, the Environment light's image (prefiltered for image
//!   based lighting), and shadow maps of the shadow-casting lights.
//!
//! Colours are linear light; textures and layer buffers are linearised from the sRGB curve
//! unless the project already blends in linear light.

use std::collections::HashMap;
use std::sync::Arc;

use effectcraft_effects::Buf;
use effectcraft_geom::{Mat4, Vec3, vec3};
use effectcraft_model::{self as model, Primitive};
use effectcraft_project::{ItemKind, Layer, LayerSource, LightKind, PrimitiveKind};

use super::shade::srgb_to_linear;
use crate::three_d::camera::{CameraState, NEAR};
use crate::three_d::compose::camera_for;
use crate::three_d::light::{LightState, Material as ClassicMaterial};
use crate::{EvalCtx, Renderer};

/// One vertex in world space.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub tangent: [f32; 4],
    pub material: u32,
}

/// A material, resolved for shading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Material {
    /// Linear RGBA factor.
    pub base: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: [f32; 3],
    pub normal_scale: f32,
    pub occlusion_strength: f32,
    /// 0 opaque, 1 mask, 2 blend.
    pub alpha_mode: u32,
    pub alpha_cutoff: f32,
    /// Layer opacity (multiplies alpha).
    pub opacity: f32,
    /// Texture indices (−1 = none).
    pub tex_base: i32,
    pub tex_mr: i32,
    pub tex_normal: i32,
    pub tex_occlusion: i32,
    pub tex_emissive: i32,
    pub double_sided: bool,
    pub unlit: bool,
    pub accepts_lights: bool,
    pub receives_shadows: bool,
    /// "Accepts Shadows: Only": draws just the shadows it receives, as black × their amount.
    pub shadows_only: bool,
    pub casts_shadows: bool,
    /// Drawn (false for "Casts Shadows: Only").
    pub visible: bool,
    /// Material Options weights (cards: Diffuse, Specular Intensity / 50%, Ambient).
    pub diffuse_k: f32,
    pub specular_k: f32,
    pub ambient_k: f32,
}

impl Default for Material {
    fn default() -> Self {
        Material {
            base: [1.0; 4],
            metallic: 0.0,
            roughness: 0.5,
            emissive: [0.0; 3],
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            alpha_mode: 0,
            alpha_cutoff: 0.5,
            opacity: 1.0,
            tex_base: -1,
            tex_mr: -1,
            tex_normal: -1,
            tex_occlusion: -1,
            tex_emissive: -1,
            double_sided: false,
            unlit: false,
            accepts_lights: true,
            receives_shadows: true,
            shadows_only: false,
            casts_shadows: false,
            visible: true,
            diffuse_k: 1.0,
            specular_k: 1.0,
            ambient_k: 1.0,
        }
    }
}

impl Material {
    /// Drawn in the transparent (sorted, blended) pass.
    pub fn transparent(&self) -> bool {
        self.alpha_mode == 2 || self.opacity < 0.999 || self.shadows_only
    }
}

/// A texture in [`Scene::texels`]: linear straight RGBA.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TexInfo {
    pub offset: u32,
    pub width: u32,
    pub height: u32,
    /// 0 repeat, 1 clamp, 2 mirror.
    pub wrap_u: u32,
    pub wrap_v: u32,
}

/// A light (ambient lights are summed into [`Scene::ambient`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    /// 0 parallel, 1 spot, 2 point.
    pub kind: u32,
    pub pos: [f32; 3],
    /// Direction the light travels.
    pub dir: [f32; 3],
    /// Linear colour × intensity.
    pub color: [f32; 3],
    pub cos_inner: f32,
    pub cos_outer: f32,
    /// 0 none, 1 smooth, 2 inverse square clamped.
    pub falloff: u32,
    pub radius: f32,
    pub falloff_distance: f32,
    /// First shadow map (point lights use six), −1 = none.
    pub shadow: i32,
    pub shadow_darkness: f32,
    /// Shadow Diffusion (pixels).
    pub shadow_diffusion: f32,
}

/// A depth map seen from a light: `view` maps world → light space (+z forward); pixels
/// `u = focal·x/z + size/2` (perspective) or `focal·x + size/2` (orthographic); texels store
/// the light-space depth of the nearest caster.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowMap {
    pub view: [[f32; 4]; 3],
    pub focal: f32,
    pub ortho: bool,
    pub size: u32,
    pub offset: u32,
    /// Depth bias in texels (scaled by the texel footprint at the receiver).
    pub bias: f32,
    /// Penumbra width (output pixels) requested by Shadow Diffusion; the shader
    /// converts it to a tap reach via the receiver's texel footprint.
    pub radius: f32,
}

/// The environment image: radiance mip levels `radiance..radiance + mips` and an irradiance
/// map, in [`Scene::textures`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvInfo {
    pub radiance: u32,
    pub mips: u32,
    pub irradiance: u32,
    pub intensity: f32,
    /// Radians about the vertical axis.
    pub rotation: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Scene {
    /// Raster size (output × `ssaa`).
    pub width: u32,
    pub height: u32,
    pub ssaa: u32,
    /// World → clip (row-major; reversed depth: ndc z = near / camera z).
    pub clip: [[f32; 4]; 4],
    /// World → camera (row-major; camera z = depth for depth of field).
    pub view: [[f32; 4]; 4],
    pub eye: [f32; 3],
    pub cam_fwd: [f32; 3],
    pub ortho: bool,
    pub vertices: Vec<Vertex>,
    /// Triangles: the first `opaque_count` indices are opaque, the rest transparent, sorted
    /// back to front.
    pub indices: Vec<u32>,
    pub opaque_count: u32,
    pub materials: Vec<Material>,
    pub textures: Vec<TexInfo>,
    pub texels: Vec<[f32; 4]>,
    pub lights: Vec<Light>,
    pub shadows: Vec<ShadowMap>,
    pub shadow_texels: Vec<f32>,
    pub env: Option<EnvInfo>,
    pub ambient: [f32; 3],
    /// Shading inputs and outputs are linear (no sRGB conversion needed).
    pub linear_io: bool,
}

/// Row-major f32 copy of a matrix.
pub fn m32(m: &Mat4) -> [[f32; 4]; 4] {
    m.0.map(|r| r.map(|v| v as f32))
}

/// Builds a [`Scene`]: texture uploads deduplicated by source.
pub(crate) struct Builder {
    pub s: Scene,
    tex_keys: HashMap<(usize, bool), i32>,
    /// Per triangle: transparent and its sort depth.
    tri_depth: Vec<(bool, f32)>,
    cam: CameraState,
    /// World of the collapsed precomp layers the current layers are drawn through (identity at
    /// the top level).
    outer: Mat4,
}

impl Builder {
    fn texture_from(&mut self, w: u32, h: u32, wrap: [u32; 2], data: impl Iterator<Item = [f32; 4]>) -> i32 {
        let offset = self.s.texels.len() as u32;
        self.s.texels.extend(data);
        self.s.textures.push(TexInfo { offset, width: w, height: h, wrap_u: wrap[0], wrap_v: wrap[1] });
        (self.s.textures.len() - 1) as i32
    }

    /// Upload a model texture (`srgb`: colour data to linearise).
    fn model_texture(&mut self, t: &Arc<model::Texture>, srgb: bool) -> i32 {
        let key = (Arc::as_ptr(t) as usize, srgb);
        if let Some(&i) = self.tex_keys.get(&key) {
            return i;
        }
        let lin = srgb && !self.s.linear_io;
        let wrap = t.wrap.map(|w| match w {
            model::Wrap::Repeat => 0,
            model::Wrap::Clamp => 1,
            model::Wrap::Mirror => 2,
        });
        let i = self.texture_from(
            t.width,
            t.height,
            wrap,
            t.data.iter().map(|p| if lin { [srgb_to_linear(p[0]), srgb_to_linear(p[1]), srgb_to_linear(p[2]), p[3]] } else { *p }),
        );
        self.tex_keys.insert(key, i);
        i
    }

    /// Upload a premultiplied layer buffer as a linear straight-alpha texture (clamped edges).
    fn buf_texture(&mut self, b: &Buf) -> (i32, bool) {
        let lin = !self.s.linear_io;
        let mut opaque = true;
        let data: Vec<[f32; 4]> = b
            .img
            .data
            .iter()
            .map(|p| {
                let a = p[3];
                if a < 0.999 {
                    opaque = false;
                }
                if a <= 1e-6 {
                    return [0.0; 4];
                }
                let s = [p[0] / a, p[1] / a, p[2] / a];
                if lin { [srgb_to_linear(s[0]), srgb_to_linear(s[1]), srgb_to_linear(s[2]), a] } else { [s[0], s[1], s[2], a] }
            })
            .collect();
        (self.texture_from(b.img.width, b.img.height, [1, 1], data.into_iter()), opaque)
    }

    fn material(&mut self, m: Material) -> u32 {
        self.s.materials.push(m);
        (self.s.materials.len() - 1) as u32
    }

    /// Append a primitive with per-vertex matrices (model/layer space → world).
    fn emit(&mut self, p: &Primitive, mat: u32, xf: &dyn Fn(usize) -> Mat4) {
        if p.positions.is_empty() || p.indices.len() < 3 {
            return;
        }
        let base = self.s.vertices.len() as u32;
        let transparent = self.s.materials[mat as usize].transparent();
        let uniform = xf(usize::MAX);
        let normal_of = |m: &Mat4| -> (Mat4, f64) {
            let a = &m.0;
            let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
                + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
            let inv = Mat4([[a[0][0], a[0][1], a[0][2], 0.0], [a[1][0], a[1][1], a[1][2], 0.0], [a[2][0], a[2][1], a[2][2], 0.0], [0.0, 0.0, 0.0, 1.0]])
                .inverse()
                .unwrap_or(Mat4::IDENTITY);
            (inv.transpose(), det)
        };
        let (un, udet) = normal_of(&uniform);
        let mut flip_count = 0usize;
        for k in 0..p.positions.len() {
            let (m, n, det) = if p.joints.is_empty() {
                (uniform, un, udet)
            } else {
                let m = xf(k);
                let (n, d) = normal_of(&m);
                (m, n, d)
            };
            if det < 0.0 {
                flip_count += 1;
            }
            let q = p.positions[k];
            let w = m.apply(vec3(q[0] as f64, q[1] as f64, q[2] as f64));
            let nn = p.normals.get(k).copied().unwrap_or([0.0, 0.0, 1.0]);
            let wn = n.apply_vec(vec3(nn[0] as f64, nn[1] as f64, nn[2] as f64)).normalize();
            let t = p.tangents.get(k).copied().unwrap_or([1.0, 0.0, 0.0, 1.0]);
            let wt = m.apply_vec(vec3(t[0] as f64, t[1] as f64, t[2] as f64)).normalize();
            // Degenerate normals and tangents (zero length: NaN once normalised) get the shader's
            // fallbacks here, so every rasteriser shades them alike (GPU compilers may not keep
            // NaN comparisons).
            let finite = |v: Vec3, or: [f32; 3]| if v.x.is_finite() && v.y.is_finite() && v.z.is_finite() { [v.x as f32, v.y as f32, v.z as f32] } else { or };
            let [tx, ty, tz] = finite(wt, [1.0, 0.0, 0.0]);
            self.s.vertices.push(Vertex {
                pos: [w.x as f32, w.y as f32, w.z as f32],
                normal: finite(wn, [0.0, 0.0, -1.0]),
                uv: p.uvs.get(k).copied().unwrap_or([0.0, 0.0]),
                tangent: [tx, ty, tz, if det < 0.0 { -t[3] } else { t[3] }],
                material: mat,
            });
        }
        // Mirroring transforms reverse the winding.
        let flip = flip_count * 2 > p.positions.len();
        for t in p.indices.as_chunks::<3>().0 {
            let (a, b, c) = (t[0] + base, t[1] + base, t[2] + base);
            if flip {
                self.s.indices.extend([a, c, b])
            } else {
                self.s.indices.extend([a, b, c])
            }
            let z = [a, b, c].iter().map(|&i| self.cam.depth(Vec3::from(self.s.vertices[i as usize].pos.map(|v| v as f64)))).sum::<f64>() / 3.0;
            self.tri_depth.push((transparent, z as f32));
        }
    }

    /// Opaque triangles first, then transparent ones back to front.
    fn sort_triangles(&mut self) {
        let tris: Vec<[u32; 3]> = self.s.indices.as_chunks::<3>().0.iter().map(|t| [t[0], t[1], t[2]]).collect();
        let mut order: Vec<usize> = (0..tris.len()).collect();
        order.sort_by(|&a, &b| {
            let (ta, za) = self.tri_depth[a];
            let (tb, zb) = self.tri_depth[b];
            ta.cmp(&tb).then_with(|| if ta { zb.total_cmp(&za) } else { std::cmp::Ordering::Equal })
        });
        self.s.opaque_count = 3 * self.tri_depth.iter().filter(|t| !t.0).count() as u32;
        self.s.indices = order.into_iter().flat_map(|i| tris[i]).collect();
    }
}

/// The Material Options of a card (classic properties).
fn card_material(ctx: &EvalCtx, layer: &Layer) -> Material {
    let c = ClassicMaterial::of(ctx, layer);
    Material {
        roughness: (1.0 - c.shininess).clamp(0.05, 1.0),
        double_sided: true,
        accepts_lights: c.accepts_lights,
        receives_shadows: c.accepts_shadows != 0,
        shadows_only: c.accepts_shadows == 2,
        casts_shadows: c.casts_shadows != 0,
        visible: c.casts_shadows != 2,
        diffuse_k: c.diffuse,
        specular_k: c.specular * 2.0,
        ambient_k: c.ambient,
        ..Material::default()
    }
}

/// Shadow/light switches and the primitive material of a model or primitive layer.
fn model_options(ctx: &EvalCtx, layer: &Layer, lin: bool) -> (Material, Option<model::Material>) {
    let g = layer.props.sub("materialOptions");
    let e = |m: &str, d: u32| g.and_then(|g| g.get(m)).map_or(d, |p| ctx.value(layer, p).as_enum());
    let b = |m: &str| g.and_then(|g| g.get(m)).is_none_or(|p| ctx.value(layer, p).as_bool());
    let casts = e("castsShadows", 0);
    let accepts = e("acceptsShadows", 1);
    let opts = Material {
        accepts_lights: b("acceptsLights"),
        receives_shadows: accepts != 0,
        shadows_only: accepts == 2,
        casts_shadows: casts != 0,
        visible: casts != 2,
        ..Material::default()
    };
    let prim = match (layer.source.clone(), g) {
        (LayerSource::Primitive { .. }, Some(g)) => {
            let c = ctx.color(layer, g, "baseColor");
            let em = ctx.color(layer, g, "emissive");
            let l = |v: f32| if lin { v } else { srgb_to_linear(v) };
            let mut m = model::Material::plain(
                [l(c[0]), l(c[1]), l(c[2]), 1.0],
                (ctx.f(layer, g, "metallic", 0.0) / 100.0).clamp(0.0, 1.0) as f32,
                (ctx.f(layer, g, "roughness", 50.0) / 100.0).clamp(0.0, 1.0) as f32,
            );
            m.emissive = [l(em[0]), l(em[1]), l(em[2])];
            Some(m)
        }
        _ => None,
    };
    (opts, prim)
}

fn prim_kind(k: PrimitiveKind) -> model::prim::PrimitiveKind {
    use model::prim::PrimitiveKind as M;
    match k {
        PrimitiveKind::Cube => M::Cube,
        PrimitiveKind::Sphere => M::Sphere,
        PrimitiveKind::Plane => M::Plane,
        PrimitiveKind::Torus => M::Torus,
        PrimitiveKind::Cone => M::Cone,
        PrimitiveKind::Cylinder => M::Cylinder,
    }
}

/// The primitive model of a primitive layer at the context time.
pub fn primitive_model(ctx: &EvalCtx, layer: &Layer, kind: PrimitiveKind, mat: model::Material) -> model::Model {
    let g = layer.props.sub("geometryOptions");
    let f = |m: &str, d: f64| g.map_or(d, |g| ctx.f(layer, g, m, d));
    let pr = model::prim::PrimParams {
        size: [f("width", 300.0) as f32, f("height", 300.0) as f32, f("depth", 300.0) as f32],
        radius: f("radius", 150.0) as f32,
        tube: f("tubeRadius", 50.0) as f32,
        height: f("height", 300.0) as f32,
        segments: f("segments", 48.0).round().clamp(3.0, 512.0) as u32,
        rings: f("rings", 24.0).round().clamp(2.0, 512.0) as u32,
    };
    model::prim::model(prim_kind(kind), &pr, mat)
}

/// Model space → layer space: model units to pixels, Y up → Y down (a 180° turn about X).
pub fn model_to_layer(unit_scale: f64) -> Mat4 {
    Mat4::scale(vec3(unit_scale, -unit_scale, -unit_scale))
}

fn model_layer(r: &Renderer, ctx: &EvalCtx, layer: &Layer, b: &mut Builder) {
    let lin = b.s.linear_io;
    let (opts, prim_mat) = model_options(ctx, layer, lin);
    let opacity = (ctx.opacity(layer) as f32 * r.opacity_mul()).clamp(0.0, 1.0);
    let geo = layer.props.sub("geometryOptions");
    let (mdl, unit, clip, t): (Arc<model::Model>, f64, Option<usize>, f64) = match &layer.source {
        LayerSource::Primitive { kind } => (Arc::new(primitive_model(ctx, layer, *kind, prim_mat.unwrap_or_default())), 1.0, None, 0.0),
        LayerSource::Model { item } => {
            let Some(it) = r.project.item(*item) else { return };
            let ItemKind::Footage(f) = &it.kind else { return };
            let Some(m) = r.footage.model(*item, f) else { return };
            let unit = geo.map_or(1.0, |g| ctx.f(layer, g, "unitScale", 1.0));
            let clip = geo.map_or(0, |g| ctx.e(layer, g, "animation")) as usize;
            let clip = clip.checked_sub(1).filter(|c| *c < m.animations.len());
            let speed = geo.map_or(100.0, |g| ctx.f(layer, g, "animationSpeed", 100.0)) / 100.0;
            let mut t = layer.layer_time(ctx.time).seconds() * speed;
            if let Some(c) = clip {
                let d = m.animations[c].duration();
                let looping = geo.is_none_or(|g| ctx.b(layer, g, "loopAnimation"));
                if looping && d > 0.0 {
                    t = t.rem_euclid(d);
                }
            }
            (m, unit, clip, t)
        }
        _ => return,
    };
    let world = b.outer * ctx.world_matrix(layer) * model_to_layer(unit);
    // Materials of the model (one default when it has none).
    let mut mats: Vec<u32> = vec![];
    let defaults = [model::Material::plain([0.8, 0.8, 0.8, 1.0], 0.0, 0.5)];
    let src: &[model::Material] = if mdl.materials.is_empty() { &defaults } else { &mdl.materials };
    for mm in src {
        let tex = |b: &mut Builder, t: Option<model::TexRef>, srgb: bool| -> i32 {
            t.and_then(|t| mdl.textures.get(t.texture)).map_or(-1, |t| b.model_texture(t, srgb))
        };
        let m = Material {
            base: mm.base_color,
            metallic: mm.metallic,
            roughness: mm.roughness,
            emissive: mm.emissive,
            normal_scale: mm.normal_scale,
            occlusion_strength: mm.occlusion_strength,
            alpha_mode: match mm.alpha_mode {
                model::AlphaMode::Opaque => 0,
                model::AlphaMode::Mask => 1,
                model::AlphaMode::Blend => 2,
            },
            alpha_cutoff: mm.alpha_cutoff,
            opacity,
            tex_base: tex(b, mm.base_color_tex, true),
            tex_mr: tex(b, mm.metallic_roughness_tex, false),
            tex_normal: tex(b, mm.normal_tex, false),
            tex_occlusion: tex(b, mm.occlusion_tex, false),
            tex_emissive: tex(b, mm.emissive_tex, true),
            double_sided: mm.double_sided,
            unlit: mm.unlit,
            ..opts
        };
        mats.push(b.material(m));
    }
    for inst in mdl.instances(clip, t) {
        for p in &inst.mesh.primitives {
            let mat = mats[p.material.unwrap_or(0).min(mats.len() - 1)];
            b.emit(p, mat, &|k| if k == usize::MAX { world * inst.world } else { world * model::vertex_matrix(&inst, p, k) });
        }
    }
}

/// Closed polylines of a path (flattened to `tol` pixels).
pub fn contours(path: &kurbo::BezPath, tol: f64) -> Vec<Vec<[f64; 2]>> {
    let mut out: Vec<Vec<[f64; 2]>> = vec![];
    let mut cur: Vec<[f64; 2]> = vec![];
    kurbo::flatten(path, tol, |el| match el {
        kurbo::PathEl::MoveTo(p) => {
            if cur.len() >= 3 {
                out.push(std::mem::take(&mut cur));
            }
            cur = vec![[p.x, p.y]];
        }
        kurbo::PathEl::LineTo(p) => cur.push([p.x, p.y]),
        kurbo::PathEl::ClosePath => {
            if cur.len() >= 3 {
                out.push(std::mem::take(&mut cur));
            }
            cur.clear();
        }
        _ => {}
    });
    if cur.len() >= 3 {
        out.push(cur);
    }
    out
}

/// Geometry Options of a text/shape layer when it is extruded.
pub fn extrusion_params(ctx: &EvalCtx, layer: &Layer) -> Option<model::extrude::ExtrudeParams> {
    let g = layer.props.sub("geometryOptions")?;
    let depth = ctx.f(layer, g, "extrusionDepth", 0.0);
    (depth > 0.0).then(|| model::extrude::ExtrudeParams {
        bevel: model::extrude::BevelStyle::from_index(ctx.e(layer, g, "bevelStyle")),
        bevel_depth: ctx.f(layer, g, "bevelDepth", 2.0).max(0.0),
        hole_bevel: (ctx.f(layer, g, "holeBevelDepth", 100.0) / 100.0).clamp(0.0, 1.0),
        depth,
    })
}

/// Extruded meshes of a text or shape layer: (mesh, straight colour, mesh space → layer
/// space). Fills and strokes are both extruded (a stroke as the region it covers, bevelled like
/// the fill), stacked in the layer's paint order (see [`model::extrude::extrude_stacked`]).
/// Text extrudes each character in its own space, so per-character 3D animation (position,
/// rotation, scale in Z) carries over. A paint whose Front / Bevel / Side / Back are coloured
/// (shape surface items, text animators) comes as one mesh per surface, in that order.
pub fn extruded_meshes(ctx: &EvalCtx, layer: &Layer, p: &model::extrude::ExtrudeParams) -> Vec<(Primitive, [f32; 4], Mat4)> {
    // (paths, colour, mesh → layer, stacking level, surface colours)
    let mut groups: Vec<(Vec<kurbo::BezPath>, [f32; 4], Mat4, usize, crate::shapes::SurfaceColors)> = vec![];
    match layer.source {
        LayerSource::Text => {
            if let Some(tg) = crate::text::text_geom(ctx, layer) {
                // Fill & Stroke order: all strokes over all fills (2), all fills over all strokes
                // (1), else the character style's Stroke Over Fill.
                let stroke_on_top = match tg.fill_stroke {
                    1 => false,
                    2 => true,
                    _ => tg.doc.stroke_over_fill,
                };
                for g in &tg.glyphs {
                    let op = (g.xf.opacity / 100.0).clamp(0.0, 1.0) as f32;
                    if g.apply_fill && g.fill[3] * op > 0.0 {
                        let c = [g.fill[0], g.fill[1], g.fill[2], g.fill[3] * op];
                        groups.push((vec![g.local.clone()], c, g.m, usize::from(!stroke_on_top), g.xf.surface_colors(c)));
                    }
                    if g.stroke_width > 0.0 && g.stroke[3] * op > 0.0 {
                        let st = effectcraft_path::StrokeStyle { width: g.stroke_width, join: effectcraft_path::Join::Round, ..Default::default() };
                        if let Some(region) = crate::shapes::stroke_region(std::slice::from_ref(&g.local), &st) {
                            let c = [g.stroke[0], g.stroke[1], g.stroke[2], g.stroke[3] * op];
                            groups.push((vec![region], c, g.m, usize::from(stroke_on_top), g.xf.surface_colors(c)));
                        }
                    }
                }
            }
        }
        LayerSource::Shape => {
            if let Some(c) = layer.props.sub("contents") {
                groups =
                    crate::shapes::extrusion_outlines(ctx, layer, c).into_iter().enumerate().map(|(k, (ps, c, s))| (ps, c, Mat4::IDENTITY, k, s)).collect();
            }
        }
        _ => {}
    }
    let mut out = vec![];
    for (paths, color, m, level, surfaces) in groups {
        let cs: Vec<Vec<[f64; 2]>> = paths.iter().flat_map(|bp| contours(bp, 0.25)).collect();
        let outlines = model::extrude::group_contours(cs);
        if outlines.is_empty() {
            continue;
        }
        if surfaces.iter().all(Option::is_none) {
            out.push((model::extrude::extrude_stacked(&outlines, p, level), color, m));
            continue;
        }
        for (mesh, s) in model::extrude::extrude_surfaces_stacked(&outlines, p, level).into_iter().zip(surfaces) {
            if mesh.indices.is_empty() {
                continue;
            }
            let c = s.map_or(color, |s| [s[0], s[1], s[2], color[3]]);
            out.push((mesh, c, m));
        }
    }
    out
}

fn extruded_layer(r: &Renderer, ctx: &EvalCtx, layer: &Layer, p: &model::extrude::ExtrudeParams, b: &mut Builder) {
    let base = card_material(ctx, layer);
    let opacity = (ctx.opacity(layer) as f32 * r.opacity_mul()).clamp(0.0, 1.0);
    let world = b.outer * ctx.world_matrix(layer);
    let lin = b.s.linear_io;
    for (mesh, c, local) in extruded_meshes(ctx, layer, p) {
        let l = |v: f32| if lin { v } else { srgb_to_linear(v) };
        let m = b.material(Material {
            base: [l(c[0]), l(c[1]), l(c[2]), c[3]],
            alpha_mode: if c[3] < 0.999 { 2 } else { 0 },
            opacity,
            double_sided: false,
            ..base
        });
        let xf = world * local;
        b.emit(&mesh, m, &|_| xf);
    }
}

fn card(b: &mut Builder, buf: &Buf, world: Mat4, mut mat: Material) {
    if buf.img.width == 0 || buf.img.height == 0 {
        return;
    }
    let (tex, opaque) = b.buf_texture(buf);
    mat.tex_base = tex;
    mat.alpha_mode = if opaque { 0 } else { 2 };
    let m = b.material(mat);
    let s = buf.scale.max(1e-9);
    let (x0, y0) = (-buf.offset[0] / s, -buf.offset[1] / s);
    let (x1, y1) = ((buf.img.width as f64 - buf.offset[0]) / s, (buf.img.height as f64 - buf.offset[1]) / s);
    // Faces the camera (−z) in layer space; uv (0,0) at the buffer's top-left texel corner.
    let p = Primitive {
        positions: vec![[x0 as f32, y0 as f32, 0.0], [x1 as f32, y0 as f32, 0.0], [x1 as f32, y1 as f32, 0.0], [x0 as f32, y1 as f32, 0.0]],
        normals: vec![[0.0, 0.0, -1.0]; 4],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        tangents: vec![[1.0, 0.0, 0.0, 1.0]; 4],
        indices: vec![0, 2, 1, 0, 3, 2],
        material: Some(0),
        ..Default::default()
    };
    b.emit(&p, m, &|_| world);
}

fn card_layer(r: &Renderer, ctx: &EvalCtx, layer: &Layer, b: &mut Builder) {
    let mut mat = card_material(ctx, layer);
    mat.opacity = (ctx.opacity(layer) as f32 * r.opacity_mul()).clamp(0.0, 1.0);
    let world = b.outer * ctx.world_matrix(layer);
    if crate::text::per_char_3d(ctx, layer) {
        for (buf, m) in crate::text::per_char_planes(ctx, layer, r.opts.scale) {
            let buf = r.to_blend(Arc::new(buf), None);
            card(b, &buf, world * m, mat);
        }
        return;
    }
    if let Some(buf) = r.blend_layer_buf(ctx, layer) {
        card(b, &buf, world, mat);
    }
}

/// Evaluated lights: (scene lights, ambient sum, environment light (layer, intensity × colour,
/// rotation)).
fn lights(ctx: &EvalCtx, lin: bool) -> (Vec<Light>, [f32; 3], Option<(Option<u64>, [f32; 3], f32)>) {
    let mut out = vec![];
    let mut ambient = [0.0f32; 3];
    let mut env = None;
    let l = |v: f32| if lin { v } else { srgb_to_linear(v) };
    for layer in ctx.comp.layers.iter().filter(|l| l.is_light() && l.switches.video && l.is_active_at(ctx.time)) {
        let LayerSource::Light { kind } = layer.source else { continue };
        let Some(g) = layer.props.sub("lightOptions") else { continue };
        let k = (ctx.f(layer, g, "intensity", 100.0) / 100.0) as f32;
        let c = ctx.color(layer, g, "color");
        let color = [l(c[0]) * k, l(c[1]) * k, l(c[2]) * k];
        match kind {
            LightKind::Ambient => {
                for i in 0..3 {
                    ambient[i] += color[i];
                }
            }
            LightKind::Environment => {
                if env.is_none() {
                    let src = g.get("source").and_then(|p| ctx.value(layer, p).as_layer());
                    let rot = ctx.f(layer, g, "rotation", 0.0).to_radians() as f32;
                    env = Some((src, color, rot));
                }
            }
            _ => {
                let Some(st) = LightState::from_layer(ctx, layer) else { continue };
                let f = |v: Vec3| [v.x as f32, v.y as f32, v.z as f32];
                out.push(Light {
                    kind: match kind {
                        LightKind::Parallel => 0,
                        LightKind::Spot => 1,
                        _ => 2,
                    },
                    pos: f(st.pos),
                    dir: f(st.dir.normalize()),
                    color,
                    cos_inner: (st.cone_half * (1.0 - st.feather)).cos() as f32,
                    cos_outer: st.cone_half.cos() as f32,
                    falloff: st.falloff,
                    radius: st.radius as f32,
                    falloff_distance: st.falloff_distance as f32,
                    // Marked; `raster::shadow_maps` assigns the maps.
                    shadow: if st.casts_shadows { 0 } else { -1 },
                    shadow_darkness: st.shadow_darkness as f32,
                    shadow_diffusion: st.shadow_diffusion as f32,
                });
            }
        }
    }
    (out, ambient, env)
}

/// Box-downsample a straight RGBA image by 2 (odd sizes round up).
fn half(w: u32, h: u32, d: &[[f32; 4]]) -> (u32, u32, Vec<[f32; 4]>) {
    let (nw, nh) = (w.div_ceil(2).max(1), h.div_ceil(2).max(1));
    let mut out = vec![[0.0f32; 4]; (nw * nh) as usize];
    for y in 0..nh {
        for x in 0..nw {
            let mut acc = [0.0f32; 4];
            let mut n = 0.0;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (sx, sy) = (2 * x + dx, 2 * y + dy);
                if sx < w && sy < h {
                    let p = d[(sy * w + sx) as usize];
                    for k in 0..4 {
                        acc[k] += p[k];
                    }
                    n += 1.0;
                }
            }
            out[(y * nw + x) as usize] = acc.map(|v| v / n);
        }
    }
    (nw, nh, out)
}

/// Cosine-weighted irradiance (`w`×`h` equirectangular) of an equirectangular radiance image:
/// each texel is the cosine-weighted mean radiance over the hemisphere around its direction
/// (a constant environment gives the same constant).
pub fn irradiance_map(sw: u32, sh: u32, src: &[[f32; 4]], w: u32, h: u32) -> Vec<[f32; 4]> {
    use super::shade::{dot, equirect_dir};
    // Source directions and solid-angle weights (∝ sin θ).
    let dirs: Vec<([f32; 3], f32, [f32; 3])> = (0..sh)
        .flat_map(|y| (0..sw).map(move |x| (x, y)))
        .map(|(x, y)| {
            let (u, v) = ((x as f32 + 0.5) / sw as f32, (y as f32 + 0.5) / sh as f32);
            let p = src[(y * sw + x) as usize];
            (equirect_dir(u, v, 0.0), (v * std::f32::consts::PI).sin(), [p[0], p[1], p[2]])
        })
        .collect();
    let mut out = vec![[0.0f32; 4]; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let n = equirect_dir((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32, 0.0);
            let (mut acc, mut wsum) = ([0.0f32; 3], 0.0f32);
            for (d, sa, c) in &dirs {
                let k = dot(n, *d).max(0.0) * sa;
                if k > 0.0 {
                    for i in 0..3 {
                        acc[i] += c[i] * k;
                    }
                    wsum += k;
                }
            }
            let inv = if wsum > 0.0 { 1.0 / wsum } else { 0.0 };
            out[(y * w + x) as usize] = [acc[0] * inv, acc[1] * inv, acc[2] * inv, 1.0];
        }
    }
    out
}

/// Prefilter an environment image into the scene's textures.
pub(crate) fn add_environment(b: &mut Builder, w: u32, h: u32, data: Vec<[f32; 4]>, intensity: [f32; 3], rotation: f32) {
    // Radiance level 0 at most 1024 wide.
    let (mut w, mut h, mut d) = (w, h, data);
    while w > 1024 {
        (w, h, d) = half(w, h, &d);
    }
    let first = b.s.textures.len() as u32;
    let mut levels = 0;
    let mut small: Option<(u32, u32, Vec<[f32; 4]>)> = None;
    loop {
        b.texture_from(w, h, [0, 1], d.iter().copied());
        levels += 1;
        if w <= 64 && small.is_none() {
            small = Some((w, h, d.clone()));
        }
        if w <= 8 || h <= 4 {
            break;
        }
        (w, h, d) = half(w, h, &d);
    }
    let (sw, sh, sd) = small.unwrap_or((w, h, d));
    let irr = irradiance_map(sw, sh, &sd, 32, 16);
    let ii = b.texture_from(32, 16, [0, 1], irr.into_iter()) as u32;
    // Intensity × colour folded into the radiance (one scalar on the GPU): tint the texels.
    let k = (intensity[0] + intensity[1] + intensity[2]) / 3.0;
    if k > 0.0 && (intensity[0] != intensity[1] || intensity[1] != intensity[2]) {
        let tint = intensity.map(|c| c / k);
        for t in first..=ii {
            let ti = b.s.textures[t as usize];
            for p in &mut b.s.texels[ti.offset as usize..(ti.offset + ti.width * ti.height) as usize] {
                for c in 0..3 {
                    p[c] *= tint[c];
                }
            }
        }
    }
    b.s.env = Some(EnvInfo { radiance: first, mips: levels, irradiance: ii, intensity: k, rotation });
}

/// The environment layer's pixels as linear straight RGBA.
fn env_pixels(r: &Renderer, ctx: &EvalCtx, src: Option<u64>, lin: bool) -> Option<(u32, u32, Vec<[f32; 4]>)> {
    let layer = match src {
        Some(id) => ctx.comp.layer(effectcraft_project::LayerId(id))?,
        None => ctx.comp.layers.iter().find(|l| l.environment && l.is_active_at(ctx.time))?,
    };
    let buf = r.blend_layer_buf(ctx, layer)?;
    let img = &buf.img;
    let data = img
        .data
        .iter()
        .map(|p| {
            let a = p[3];
            if a <= 1e-6 {
                return [0.0, 0.0, 0.0, 1.0];
            }
            let s = [p[0] / a, p[1] / a, p[2] / a];
            if lin { [s[0], s[1], s[2], 1.0] } else { [srgb_to_linear(s[0]), srgb_to_linear(s[1]), srgb_to_linear(s[2]), 1.0] }
        })
        .collect();
    Some((img.width, img.height, data))
}

/// Supersampling factor: 2×2 unless draft or very large.
fn ssaa_for(r: &Renderer, out: (u32, u32)) -> u32 {
    if r.opts.draft || (out.0 as u64 * out.1 as u64) > 8_000_000 { 1 } else { 2 }
}

/// Add one layer of a run: a model or primitive mesh, an extruded text/shape layer, the nested
/// layers of a collapsed precomp, or a textured card.
fn emit_layer<'a>(r: &Renderer<'a>, ctx: &EvalCtx<'a>, layer: &Layer, b: &mut Builder) {
    if layer.environment || layer.switches.adjustment || ctx.opacity(layer) <= 0.0 || !layer.has_video() {
        return;
    }
    if let Some(item) = r.collapsed(ctx, layer) {
        collapsed_layer(r, ctx, layer, item, b);
    } else if layer.source.is_model() {
        model_layer(r, ctx, layer, b);
    } else if matches!(layer.source, LayerSource::Text | LayerSource::Shape)
        && let Some(p) = extrusion_params(ctx, layer)
    {
        extruded_layer(r, ctx, layer, &p, b);
    } else {
        card_layer(r, ctx, layer, b);
    }
}

/// A collapsed precomp layer: its nested layers join this scene as real Advanced 3D geometry
/// (meshes, extrusions, cards) placed by the precomp layer's world transform, so they
/// intersect, occlude and shadow the parent's layers. Nested 2D layers lie on the precomp
/// layer's plane; nested lights and cameras are ignored (the parent's light the scene).
fn collapsed_layer<'a>(r: &Renderer<'a>, ctx: &EvalCtx<'a>, layer: &Layer, item: effectcraft_project::ItemId, b: &mut Builder) {
    let op = ctx.opacity(layer) as f32 * r.opacity_mul();
    let Some((sub, nctx)) = r.collapse_into(ctx, layer, item, op) else { return };
    let saved = b.outer;
    b.outer = saved * ctx.world_matrix(layer);
    let t = nctx.time;
    let any_solo = nctx.comp.layers.iter().any(|l| l.switches.solo && l.source.is_av());
    for l in nctx.comp.layers.iter().rev() {
        if !l.is_active_at(t)
            || l.is_light()
            || matches!(l.source, LayerSource::Camera)
            || (any_solo && !l.switches.solo)
            || (!sub.opts.guides && l.switches.guide)
        {
            continue;
        }
        emit_layer(&sub, &nctx, l, b);
    }
    b.outer = saved;
}

/// Build the scene of a run of 3D layers.
pub(crate) fn build(r: &Renderer, ctx: &EvalCtx, run: &[&Layer], out: (u32, u32)) -> Scene {
    build_at(r, ctx, None, run, out)
}

/// Whether a layer's motion is blurred (the renderer's gate: Motion Blur switch, comp setting,
/// render option and the precomp layers above).
pub(crate) fn motion_blurred(r: &Renderer, ctx: &EvalCtx, layer: &Layer) -> bool {
    r.mb_on(ctx, layer)
}

/// [`build`] at a motion-blur sub-sample: layers with Motion Blur on, the camera and the lights
/// are evaluated at `sub`'s time; the other layers stay at `ctx`'s.
pub(crate) fn build_at(r: &Renderer, ctx: &EvalCtx, sub: Option<&EvalCtx>, run: &[&Layer], out: (u32, u32)) -> Scene {
    let base = ctx;
    let at = sub.unwrap_or(ctx);
    let ctx = at;
    let cam = camera_for(r, ctx);
    let lin = r.pipe.linear || r.pipe.linear_blend;
    let ssaa = ssaa_for(r, out);
    let mut b = Builder {
        s: Scene { width: out.0 * ssaa, height: out.1 * ssaa, ssaa, linear_io: lin, ..Scene::default() },
        tex_keys: HashMap::new(),
        tri_depth: vec![],
        cam,
        outer: r.collapse3d.map_or(Mat4::IDENTITY, |c| c.world),
    };
    for layer in run {
        let ctx = if sub.is_some() && motion_blurred(r, base, layer) { at } else { base };
        emit_layer(r, ctx, layer, &mut b);
    }
    b.sort_triangles();
    // Lights, environment, shadows (Draft 3D: unlit, like After Effects). A collapsed
    // precomp's layers are lit by the outermost comp's lights.
    let parent = r.collapse3d.map(|c| c.parent);
    let ctx = parent.as_ref().unwrap_or(ctx);
    if !ctx.comp.draft_3d {
        let (ls, amb, env) = lights(ctx, lin);
        b.s.lights = ls;
        b.s.ambient = amb;
        if let Some((src, k, rot)) = env
            && let Some((w, h, d)) = env_pixels(r, ctx, src, lin)
        {
            add_environment(&mut b, w, h, d, k, rot);
        }
        if r.opts.shadows() {
            super::raster::shadow_maps(&mut b.s, r.opts.draft);
        } else {
            // Settings ▸ 3D ▸ Realtime Shadows in Draft off: draft renders skip shadows.
            for l in &mut b.s.lights {
                l.shadow = -1;
            }
        }
    }
    // Camera matrices.
    let s = r.opts.scale;
    let (cw, ch) = crate::three_d::compose::view_size(r, ctx);
    let (ow, oh) = (out.0 as f64, out.1 as f64);
    let (a, bb) = (2.0 * s * cam.zoom / ow, s * cw / ow - 1.0);
    let (c, d) = (2.0 * s * cam.zoom / oh, s * ch / oh - 1.0);
    let proj = if cam.ortho {
        let (mut zmin, mut zmax) = (f64::INFINITY, f64::NEG_INFINITY);
        for v in &b.s.vertices {
            let z = cam.depth(Vec3::from(v.pos.map(|x| x as f64)));
            zmin = zmin.min(z);
            zmax = zmax.max(z);
        }
        if !zmin.is_finite() {
            (zmin, zmax) = (-1.0, 1.0);
        }
        let pad = (zmax - zmin).abs() * 0.01 + 1.0;
        let (zmin, zmax) = (zmin - pad, zmax + pad);
        let zr = zmax - zmin;
        Mat4([[a, 0.0, 0.0, bb], [0.0, -c, 0.0, -d], [0.0, 0.0, -1.0 / zr, zmax / zr], [0.0, 0.0, 0.0, 1.0]])
    } else {
        Mat4([[a, 0.0, bb, 0.0], [0.0, -c, -d, 0.0], [0.0, 0.0, 0.0, NEAR], [0.0, 0.0, 1.0, 0.0]])
    };
    b.s.clip = m32(&(proj * cam.view));
    b.s.view = m32(&cam.view);
    let f = cam.forward();
    b.s.cam_fwd = [f.x as f32, f.y as f32, f.z as f32];
    b.s.eye = [cam.eye.x as f32, cam.eye.y as f32, cam.eye.z as f32];
    b.s.ortho = cam.ortho;
    b.s
}
