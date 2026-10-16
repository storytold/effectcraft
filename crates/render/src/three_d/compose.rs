//! The Classic 3D compositor for a run of consecutive 3D layers.
//!
//! Each layer is a textured plane: its processed buffer (source → masks → effects, DOF-blurred)
//! mapped through `projection · world`. Rendering is per output pixel (rows in parallel): every
//! plane covering the pixel is inverse-mapped through its homography (exact perspective-correct
//! texture coordinates, bilinear/bicubic sampling), its camera depth taken from the plane
//! equation, and the fragment shaded by the comp's lights. The pixel's fragments are then sorted
//! far→near (coplanar fragments keep layer-stack order) and blended with each layer's mode and
//! opacity. Sorting per pixel handles intersecting layers exactly, and blend modes and partial
//! transparency still compose in depth order, as in AE's Classic 3D renderer.
//!
//! Shadows are ray-cast against the shadow-casting planes (exact for planar layers, including
//! alpha-shaped and coloured shadows through Light Transmission); Shadow Diffusion softens them
//! with a penumbra that grows with the distance between caster and receiver.

use std::sync::Arc;

use effectcraft_color::{BlendMode, blend_pixel};
use effectcraft_effects::Buf;
use effectcraft_geom::{Mat3, Mat4, Vec3, vec3};
use effectcraft_project::{Layer, LightKind, MatteKind};
use effectcraft_raster::{Image, Px, hash_noise};
use effectcraft_time::Tick;
use rayon::prelude::*;

use super::camera::{CameraState, NEAR, active_camera};
use super::light::{LightState, Material, lights_at, shade};
use crate::{EvalCtx, Renderer};

/// One position of a plane (several with motion blur).
#[derive(Clone, Copy, Debug)]
pub struct Geo {
    /// Output pixel → homogeneous layer coordinates.
    pub hinv: Mat3,
    /// Camera-space depth as a function of layer (u, v): `d0·u + d1·v + d2`.
    pub depth: [f64; 3],
    /// Layer → world.
    pub world: Mat4,
    /// Unit world normal.
    pub normal: Vec3,
    /// Output pixel bounds [x0, y0, x1, y1) covered by the plane.
    pub bbox: [i64; 4],
    pub cam: CameraState,
}

/// A prepared 3D layer.
pub(crate) struct Item<'a> {
    pub layer: &'a Layer,
    /// Position in the layer stack (higher = above).
    order: usize,
    buf: Arc<Buf>,
    bicubic: bool,
    geos: Vec<Geo>,
    mat: Material,
    opacity: f32,
    /// Drawn (false for "Casts Shadows: Only" and shadow-only casters outside the run).
    draw: bool,
    /// Screen-space track matte factor (output size).
    matte: Option<Arc<Vec<f32>>>,
    preserve: bool,
    /// World → layer (for shadow rays) and the buffer's layer-space bounds.
    winv: Mat4,
    bounds: [f64; 4],
    bbox: [i64; 4],
    /// Depth of field left to an accelerator (`buf` is then unblurred and unpadded).
    dof: Option<PlaneDof>,
}

/// Depth of field of a plane for an accelerator to apply ([`Plane3d::dof`]): pad the buffer by
/// `pad` transparent pixels, boost its highlights, then blur it at each of `levels` with
/// [`super::bokeh::kernel_spans`] and blend the levels by each pixel's `radius` (padded buffer
/// pixels, row-major) with [`super::bokeh::level_weight`] — what the CPU's depth of field does.
#[derive(Clone, Debug)]
pub struct PlaneDof {
    pub pad: u32,
    pub iris: super::bokeh::Iris,
    pub highlight: super::bokeh::Highlight,
    pub radius: Arc<Vec<f32>>,
    pub levels: Vec<f32>,
}

impl Item<'_> {
    #[inline]
    fn texel(&self, u: f64, v: f64) -> Px {
        let x = u * self.buf.scale + self.buf.offset[0];
        let y = v * self.buf.scale + self.buf.offset[1];
        if self.bicubic { self.buf.img.sample_bicubic(x, y) } else { self.buf.img.sample_bilinear(x, y) }
    }
}

/// Build the plane geometry of a layer for a camera.
fn geo(cam: &CameraState, world: Mat4, b: [f64; 4], comp: (f64, f64), scale: f64, out: (u32, u32), off: (f64, f64)) -> Option<Geo> {
    // The output canvas may start at `off` (scaled comp pixels): region of interest / Extended
    // Viewer frames.
    let proj = Mat4::translate(vec3(-off.0, -off.1, 0.0)) * Mat4::scale(vec3(scale, scale, 1.0)) * cam.projection(comp.0, comp.1);
    let full = proj * world;
    let h = full.plane_to_mat3();
    // Edge-on planes project to a line (singular homography): nothing to draw.
    let col = |j: usize| (h.0[0][j].powi(2) + h.0[1][j].powi(2) + h.0[2][j].powi(2)).sqrt();
    if h.determinant().abs() <= 1e-10 * col(0) * col(1) * col(2) {
        return None;
    }
    let hinv = h.inverse()?;
    let cv = cam.view * world;
    let depth = [cv.0[2][0], cv.0[2][1], cv.0[2][3]];
    let normal = world.apply_vec(vec3(1.0, 0.0, 0.0)).cross(world.apply_vec(vec3(0.0, 1.0, 0.0)));
    if normal.length() < 1e-12 {
        return None;
    }
    let normal = normal.normalize();
    let corners = [(b[0], b[1]), (b[2], b[1]), (b[2], b[3]), (b[0], b[3])];
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut behind = 0;
    let mut full_frame = false;
    for (u, v) in corners {
        let z = depth[0] * u + depth[1] * v + depth[2];
        if !cam.ortho && z < NEAR {
            behind += 1;
            full_frame = true;
            continue;
        }
        let p = full.apply(vec3(u, v, 0.0));
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    if behind == 4 {
        return None;
    }
    let (ow, oh) = (out.0 as i64, out.1 as i64);
    let bbox = if full_frame {
        [0, 0, ow, oh]
    } else {
        [((x0.floor() as i64) - 2).max(0), ((y0.floor() as i64) - 2).max(0), ((x1.ceil() as i64) + 2).min(ow), ((y1.ceil() as i64) + 2).min(oh)]
    };
    if bbox[0] >= bbox[2] || bbox[1] >= bbox[3] {
        return None;
    }
    Some(Geo { hinv, depth, world, normal, bbox, cam: *cam })
}

/// Layer-space rectangle covered by a buffer.
fn buf_bounds(buf: &Buf) -> [f64; 4] {
    let s = buf.scale.max(1e-9);
    [-buf.offset[0] / s, -buf.offset[1] / s, (buf.img.width as f64 - buf.offset[0]) / s, (buf.img.height as f64 - buf.offset[1]) / s]
}

/// Inverse-map an output pixel centre onto a plane: (u, v, camera depth).
#[inline]
fn unproject(g: &Geo, sx: f64, sy: f64) -> Option<(f64, f64, f64)> {
    let h = &g.hinv.0;
    let qz = h[2][0] * sx + h[2][1] * sy + h[2][2];
    if !g.cam.ortho && qz <= 0.0 {
        return None;
    }
    if qz.abs() < 1e-300 {
        return None;
    }
    let u = (h[0][0] * sx + h[0][1] * sy + h[0][2]) / qz;
    let v = (h[1][0] * sx + h[1][1] * sy + h[1][2]) / qz;
    let z = g.depth[0] * u + g.depth[1] * v + g.depth[2];
    if !g.cam.ortho && z < NEAR {
        return None;
    }
    Some((u, v, z))
}

/// The camera a renderer uses at a context time (view override only for the top-level comp).
pub(crate) fn camera_for(r: &Renderer, ctx: &EvalCtx) -> CameraState {
    // Collapsed precomps see through the parent's camera.
    if let Some(c) = r.collapse3d {
        return c.cam;
    }
    match r.opts.view {
        Some(v) if r.depth == 0 => v,
        _ => active_camera(ctx),
    }
}

/// The size of the comp the camera looks through: a collapsed precomp's layers are projected
/// into the outermost comp they collapse into (its centre and size), not their own comp's.
pub(crate) fn view_size(r: &Renderer, ctx: &EvalCtx) -> (f64, f64) {
    let c = r.collapse3d.map_or(ctx.comp, |c| c.parent.comp);
    (c.width as f64, c.height as f64)
}

/// Number of motion-blur sub-samples for a layer.
fn mb_samples(r: &Renderer, ctx: &EvalCtx, layer: &Layer) -> usize {
    if r.opts.motion_blur && ctx.comp.enable_motion_blur && r.layer_motion_blur(layer) {
        if r.opts.draft { 4 } else { ctx.comp.motion_blur_samples.clamp(2, 64) as usize }
    } else {
        1
    }
}

/// Prepare a layer (buffer, geometry, material). `None` when it shows nothing.
pub(crate) fn prepare<'a>(r: &Renderer, ctx: &EvalCtx<'a>, layer: &'a Layer, order: usize, out: (u32, u32), in_run: bool) -> Option<Item<'a>> {
    let buf = r.blend_layer_buf(ctx, layer)?;
    prepare_with(r, ctx, layer, order, out, in_run, buf, Mat4::IDENTITY)
}

/// Prepare a layer as planes: one per character for per-character 3D text, else one.
pub(crate) fn prepare_all<'a>(r: &Renderer, ctx: &EvalCtx<'a>, layer: &'a Layer, order: usize, out: (u32, u32), in_run: bool) -> Vec<Item<'a>> {
    if crate::text::per_char_3d(ctx, layer) {
        return crate::text::per_char_planes(ctx, layer, r.opts.scale)
            .into_iter()
            .filter_map(|(buf, m)| prepare_with(r, ctx, layer, order, out, in_run, r.to_blend(Arc::new(buf), None), m))
            .collect();
    }
    prepare(r, ctx, layer, order, out, in_run).into_iter().collect()
}

/// Prepare a plane showing `buf`, whose space maps into the layer through `local`.
#[allow(clippy::too_many_arguments)]
fn prepare_with<'a>(
    r: &Renderer,
    ctx: &EvalCtx<'a>,
    layer: &'a Layer,
    order: usize,
    out: (u32, u32),
    in_run: bool,
    mut buf: Arc<Buf>,
    local: Mat4,
) -> Option<Item<'a>> {
    if buf.img.is_empty() {
        return None;
    }
    let comp = view_size(r, ctx);
    let s = r.opts.scale;
    let cam = camera_for(r, ctx);
    let outer = r.collapse3d.map_or(Mat4::IDENTITY, |c| c.world);
    let world = outer * ctx.world_matrix(layer) * local;
    // Depth of field: a bokeh blur by each buffer pixel's circle of confusion (constant for a
    // layer facing the camera, progressive for a tilted one).
    let mut plane_dof = None;
    let mut bounds = buf_bounds(&buf);
    if let Some(dof) = cam.dof.filter(|_| !ctx.comp.draft_3d && !r.opts.draft) {
        if r.defer_dof {
            // An accelerator blurs: hand it the radius map of the padded buffer.
            if let Some((pd, padded_bounds)) = dof_plan(&cam, &dof, world, &buf) {
                plane_dof = Some(pd);
                bounds = padded_bounds;
            }
        } else {
            buf = dof_blur(&cam, &dof, world, buf);
            bounds = buf_bounds(&buf);
        }
    }
    let n = if in_run { mb_samples(r, ctx, layer) } else { 1 };
    let geos: Vec<Geo> = if n <= 1 {
        geo(&cam, world, bounds, comp, s, out, r.out_offset()).into_iter().collect()
    } else {
        let fd = ctx.comp.frame_duration().seconds();
        let angle = ctx.comp.shutter_angle / 360.0;
        let phase = ctx.comp.shutter_phase / 360.0;
        (0..n)
            .filter_map(|i| {
                let f = phase + angle * i as f64 / (n - 1) as f64;
                let sub = ctx.at(ctx.time + Tick::from_seconds_f64(f * fd));
                let c = camera_for(r, &sub);
                geo(&c, outer * sub.world_matrix(layer) * local, bounds, comp, s, out, r.out_offset())
            })
            .collect()
    };
    if geos.is_empty() && in_run {
        return None;
    }
    let mut bbox = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
    for g in &geos {
        bbox = [bbox[0].min(g.bbox[0]), bbox[1].min(g.bbox[1]), bbox[2].max(g.bbox[2]), bbox[3].max(g.bbox[3])];
    }
    let mat = Material::of(ctx, layer);
    Some(Item {
        layer,
        order,
        bicubic: matches!(r.sampling(layer), effectcraft_raster::Sampling::Bicubic),
        buf,
        geos,
        opacity: ctx.opacity(layer) as f32 * r.opacity_mul(),
        draw: in_run && mat.casts_shadows != 2,
        mat,
        matte: None,
        preserve: layer.preserve_transparency,
        winv: world.inverse().unwrap_or(Mat4::IDENTITY),
        bounds,
        bbox,
        dof: plane_dof,
    })
}

/// Blur radius (buffer pixels) of every buffer pixel of a plane at `world` (layer → world), from
/// its camera depth: the circle of confusion over the on-screen size of one layer pixel there.
fn dof_radii(cam: &CameraState, dof: &super::camera::Dof, world: Mat4, buf: &Buf) -> Vec<f32> {
    dof_radii_at(cam, dof, world, (buf.img.width as usize, buf.img.height as usize), buf.offset, buf.scale)
}

fn dof_radii_at(cam: &CameraState, dof: &super::camera::Dof, world: Mat4, (w, h): (usize, usize), offset: [f64; 2], scale: f64) -> Vec<f32> {
    let s = scale.max(1e-9);
    let sx = world.apply_vec(vec3(1.0, 0.0, 0.0)).length().max(1e-9);
    let at = |x: f64, y: f64| cam.depth(world.apply(vec3((x - offset[0]) / s, (y - offset[1]) / s, 0.0)));
    // Camera depth is affine across a plane.
    let z0 = at(0.5, 0.5);
    let (zx, zy) = (at(1.5, 0.5) - z0, at(0.5, 1.5) - z0);
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let z = z0 + zx * x as f64 + zy * y as f64;
            if z <= super::camera::NEAR {
                continue;
            }
            // coc · ½ / (zoom / z · sx) · scale
            *o = (dof.coc(z) * 0.5 * z / (cam.zoom * sx) * s) as f32;
        }
    });
    out
}

/// The padding [`dof_blur`] gives a buffer whose largest blur radius is `rmax`.
fn dof_pad(dof: &super::camera::Dof, rmax: f64) -> u32 {
    let a = dof.iris.aspect.clamp(0.01, 100.0).sqrt().max(1.0 / dof.iris.aspect.clamp(0.01, 100.0).sqrt());
    (rmax * a).ceil() as u32 + 2
}

/// [`dof_blur`] planned for an accelerator: the padding, radius map and levels, and the padded
/// buffer's layer-space bounds. `None` when nothing blurs.
fn dof_plan(cam: &CameraState, dof: &super::camera::Dof, world: Mat4, buf: &Buf) -> Option<(PlaneDof, [f64; 4])> {
    let radii = dof_radii(cam, dof, world, buf);
    let rmax = radii.iter().copied().fold(0.0f32, f32::max) as f64;
    if rmax <= 0.3 {
        return None;
    }
    let pad = dof_pad(dof, rmax);
    let (w, h) = (buf.img.width as usize + 2 * pad as usize, buf.img.height as usize + 2 * pad as usize);
    let offset = [buf.offset[0] + pad as f64, buf.offset[1] + pad as f64];
    let radius = dof_radii_at(cam, dof, world, (w, h), offset, buf.scale);
    let levels = super::bokeh::blur_levels(&radius)?;
    let s = buf.scale.max(1e-9);
    let bounds = [-offset[0] / s, -offset[1] / s, (w as f64 - offset[0]) / s, (h as f64 - offset[1]) / s];
    Some((PlaneDof { pad, iris: dof.iris, highlight: dof.highlight, radius: Arc::new(radius), levels }, bounds))
}

/// Depth of field for one plane: highlights boosted, then a bokeh-shaped blur by depth.
fn dof_blur(cam: &CameraState, dof: &super::camera::Dof, world: Mat4, buf: Arc<Buf>) -> Arc<Buf> {
    let radii = dof_radii(cam, dof, world, &buf);
    let rmax = radii.iter().copied().fold(0.0f32, f32::max) as f64;
    if rmax <= 0.3 {
        return buf;
    }
    let pad = dof_pad(dof, rmax);
    let mut padded = buf.img.padded(pad);
    super::bokeh::boost_highlights(&mut padded, &dof.highlight);
    let pbuf = Buf { img: padded, offset: [buf.offset[0] + pad as f64, buf.offset[1] + pad as f64], scale: buf.scale };
    let pr = dof_radii(cam, dof, world, &pbuf);
    // The (cached, shared) layer buffer stays untouched; the blurred copy is per frame.
    Arc::new(Buf { img: super::bokeh::progressive_blur(&pbuf.img, &dof.iris, &pr), offset: pbuf.offset, scale: pbuf.scale })
}

/// Screen-space track matte factor for a layer, or None.
fn matte_for(r: &Renderer, ctx: &EvalCtx, layer: &Layer, out: (u32, u32)) -> Option<Arc<Vec<f32>>> {
    let tm = layer.track_matte?;
    let m = ctx.comp.layer(tm.layer).filter(|m| m.id != layer.id)?;
    let mut img = Image::new(out.0, out.1);
    if m.is_active_at(ctx.time) {
        if m.is_3d() {
            // Render the matte plane alone (its own matte is ignored).
            let mut solo = m.clone();
            solo.track_matte = None;
            solo.preserve_transparency = false;
            solo.blend_mode = BlendMode::Normal;
            let its = prepare_all(r, ctx, &solo, 0, out, true);
            if !its.is_empty() {
                let lights = scene_lights(ctx);
                composite(&mut img, &its, &lights, &[], m.id.0 as u32);
            }
        } else if let Some(mb) = r.layer_buf(ctx, m) {
            r.place(ctx, m, &mb, &mut img, BlendMode::Normal, ctx.opacity(m) as f32);
        }
    }
    Some(Arc::new(
        img.data
            .par_iter()
            .map(|q| {
                match tm.kind {
                    MatteKind::Alpha => q[3],
                    MatteKind::AlphaInverted => 1.0 - q[3],
                    MatteKind::Luma => effectcraft_color::luminance(q[0], q[1], q[2]),
                    MatteKind::LumaInverted => 1.0 - effectcraft_color::luminance(q[0], q[1], q[2]),
                }
                .clamp(0.0, 1.0)
            })
            .collect(),
    ))
}

/// Lights used for shading: none in Draft 3D (which also turns off shadows and depth of field).
fn scene_lights(ctx: &EvalCtx) -> Vec<LightState> {
    if ctx.comp.draft_3d { vec![] } else { lights_at(ctx) }
}

/// Draw a run of consecutive 3D layers (bottom-to-top order) into `canvas`.
pub(crate) fn draw_run(r: &Renderer, ctx: &EvalCtx, run: &[&Layer], canvas: &mut Image) {
    // Environment background layers: an infinitely distant sky behind the rest of the run.
    if run.iter().any(|l| l.environment_background) {
        for l in run.iter().filter(|l| l.environment_background) {
            draw_sky(r, ctx, l, canvas);
        }
        let rest: Vec<&Layer> = run.iter().copied().filter(|l| !l.environment_background).collect();
        draw_run(r, ctx, &rest, canvas);
        return;
    }
    // 3D adjustment layers act on everything below them in the stack (like 2D ones): split.
    if let Some(k) = run.iter().position(|l| l.switches.adjustment) {
        draw_run(r, ctx, &run[..k], canvas);
        r.draw_layer(ctx, run[k], canvas, false);
        draw_run(r, ctx, &run[k + 1..], canvas);
        return;
    }
    if run.is_empty() {
        return;
    }
    // Advanced 3D comps: meshes, physically based lights, shadow maps (a collapsed precomp's
    // layers use the renderer of the comp they collapse into).
    if super::adv::active(r, ctx) {
        super::adv::draw_run(r, ctx, run, canvas);
        return;
    }
    let out = (canvas.width, canvas.height);
    let g = gather(r, ctx, run, out);
    let casters: Vec<&Item> = g.items.iter().chain(g.extra.iter()).filter(|i| r.opts.shadows() && i.mat.casts_shadows != 0 && !i.geos.is_empty()).collect();
    composite(canvas, &g.items, &g.lights, &casters, run[0].id.0 as u32);
    for l in run.iter().filter(|l| r.quality(l) == effectcraft_project::Quality::Wireframe) {
        r.draw_layer(ctx, l, canvas, false);
    }
}

/// The prepared planes of a run: drawn items (stack order), shadow casters outside the run, and
/// the lights.
struct Gathered<'a> {
    items: Vec<Item<'a>>,
    extra: Vec<Item<'a>>,
    lights: Vec<LightState>,
}

fn gather<'a>(r: &Renderer<'a>, ctx: &EvalCtx<'a>, run: &[&'a Layer], out: (u32, u32)) -> Gathered<'a> {
    // A collapsed precomp's 3D layers are lit by the outer comp's lights.
    let lights = scene_lights(&r.collapse3d.map_or(*ctx, |c| c.parent));
    let mut items: Vec<Item> = run
        .par_iter()
        .enumerate()
        .filter(|(_, l)| ctx.opacity(l) > 0.0 && r.quality(l) != effectcraft_project::Quality::Wireframe)
        .flat_map_iter(|(i, l)| {
            // Stack order with room for a collapsed precomp's layers in between.
            let i = i * 1024;
            if let Some(item) = r.collapsed(ctx, l) {
                return collapsed_items(r, ctx, l, item, i, out);
            }
            let mut its = prepare_all(r, ctx, l, i, out, true);
            if !its.is_empty() {
                let m = matte_for(r, ctx, l, out);
                for it in &mut its {
                    it.matte = m.clone();
                }
            }
            its
        })
        .collect();
    // Shadow casters outside this run (other 3D layers of the comp).
    let mut extra: Vec<Item> = vec![];
    if lights.iter().any(|l| l.casts_shadows && l.kind != LightKind::Ambient) {
        let in_run: Vec<_> = run.iter().map(|l| l.id).collect();
        extra = ctx
            .comp
            .layers
            .par_iter()
            .filter(|l| l.switches.three_d && l.has_video() && l.is_active_at(ctx.time) && !in_run.contains(&l.id) && !l.switches.adjustment)
            .filter(|l| Material::of(ctx, l).casts_shadows != 0)
            .flat_map_iter(|l| prepare_all(r, ctx, l, 0, out, false))
            .collect();
    }
    items.sort_by_key(|i| i.order);
    Gathered { items, extra, lights }
}

/// A collapsed 3D precomp layer inside a 3D run: its nested layers become planes in this comp's
/// 3D space (world = precomp layer's world × nested world), depth-sorted with the run, so they
/// intersect the parent's 3D layers; nested 2D layers lie on the precomp layer's plane in stack
/// order. (Nested adjustment layers are skipped here.)
fn collapsed_items<'a>(
    r: &Renderer<'a>,
    ctx: &EvalCtx<'a>,
    layer: &'a Layer,
    item: effectcraft_project::ItemId,
    order: usize,
    out: (u32, u32),
) -> Vec<Item<'a>> {
    let op = ctx.opacity(layer) as f32 * r.opacity_mul();
    let Some((sub, nctx)) = r.collapse_into(ctx, layer, item, op) else { return vec![] };
    let t = nctx.time;
    let any_solo = nctx.comp.layers.iter().any(|l| l.switches.solo && l.source.is_av());
    let nested: Vec<&'a Layer> = nctx
        .comp
        .layers
        .iter()
        .rev()
        .filter(|l| l.is_active_at(t) && l.has_video() && !l.switches.adjustment && (!any_solo || l.switches.solo) && (r.opts.guides || !l.switches.guide))
        .collect();
    let mut items = Vec::new();
    for (k, l) in nested.into_iter().enumerate() {
        if nctx.opacity(l) <= 0.0 {
            continue;
        }
        let mut its = prepare_all(&sub, &nctx, l, order + 1 + k.min(1022), out, true);
        if !its.is_empty() {
            let m = matte_for(&sub, &nctx, l, out);
            for it in &mut its {
                it.matte = m.clone();
            }
        }
        items.extend(its);
    }
    items
}

/// Per-pixel depth-sorted compositing of prepared planes.
fn composite(canvas: &mut Image, items: &[Item], lights: &[LightState], casters: &[&Item], seed: u32) {
    let w = canvas.width as usize;
    canvas.rows_mut().for_each(|(y, row)| {
        let yi = y as i64;
        let active: Vec<&Item> = items.iter().filter(|it| it.draw && yi >= it.bbox[1] && yi < it.bbox[3]).collect();
        if active.is_empty() {
            return;
        }
        let mut frags: Vec<(f64, usize, Px, usize)> = Vec::with_capacity(active.len());
        for x in 0..w {
            let xi = x as i64;
            frags.clear();
            for (k, it) in active.iter().enumerate() {
                if xi < it.bbox[0] || xi >= it.bbox[2] {
                    continue;
                }
                if let Some((z, px)) = fragment(it, x as f64 + 0.5, y as f64 + 0.5, lights, casters) {
                    let px = match &it.matte {
                        Some(m) => {
                            let f = m[y * w + x];
                            [px[0] * f, px[1] * f, px[2] * f, px[3] * f]
                        }
                        None => px,
                    };
                    if px[3] > 0.0 || px[0] != 0.0 || px[1] != 0.0 || px[2] != 0.0 {
                        frags.push((z, it.order, px, k));
                    }
                }
            }
            if frags.is_empty() {
                continue;
            }
            // Far → near; (near-)coplanar fragments in stack order (lower layer first).
            for i in 1..frags.len() {
                let mut j = i;
                while j > 0 && before(&frags[j], &frags[j - 1]) {
                    frags.swap(j, j - 1);
                    j -= 1;
                }
            }
            let mut d = row[x];
            for (_, _, px, k) in &frags {
                let it = active[*k];
                let mut s = *px;
                let op = it.opacity * if it.preserve { d[3] } else { 1.0 };
                for c in s.iter_mut() {
                    *c *= op;
                }
                let mode = it.layer.blend_mode;
                let n = if matches!(mode, BlendMode::Dissolve | BlendMode::DancingDissolve) {
                    hash_noise(x as u32, y as u32, seed ^ it.layer.id.0 as u32)
                } else {
                    0.5
                };
                d = blend_pixel(mode, d, s, n);
            }
            row[x] = d;
        }
    });
}

#[inline]
fn before(a: &(f64, usize, Px, usize), b: &(f64, usize, Px, usize)) -> bool {
    let tol = 1e-6 * a.0.abs().max(b.0.abs()).max(1.0);
    if (a.0 - b.0).abs() <= tol { a.1 < b.1 } else { a.0 > b.0 }
}

/// Shaded premultiplied fragment of a plane at an output pixel centre, with its depth.
fn fragment(it: &Item, sx: f64, sy: f64, lights: &[LightState], casters: &[&Item]) -> Option<(f64, Px)> {
    let n = it.geos.len();
    let mut acc = [0.0f32; 4];
    let mut depth = None;
    for g in &it.geos {
        let Some((u, v, z)) = unproject(g, sx, sy) else { continue };
        let t = it.texel(u, v);
        if t[3] <= 1e-6 {
            continue;
        }
        depth.get_or_insert(z);
        let px = shade_texel(it, g, u, v, t, lights, casters);
        for c in 0..4 {
            acc[c] += px[c];
        }
    }
    let depth = depth?;
    if n > 1 {
        let k = 1.0 / n as f32;
        for c in acc.iter_mut() {
            *c *= k;
        }
    }
    Some((depth, acc))
}

fn shade_texel(it: &Item, g: &Geo, u: f64, v: f64, t: Px, lights: &[LightState], casters: &[&Item]) -> Px {
    if lights.is_empty() || !it.mat.accepts_lights {
        if it.mat.accepts_shadows == 2 {
            return [0.0; 4];
        }
        return t;
    }
    let a = t[3];
    let rgb = [t[0] / a, t[1] / a, t[2] / a];
    let p = g.world.apply(vec3(u, v, 0.0));
    let shadow = |li: usize, p: Vec3| -> [f32; 3] {
        let l = &lights[li];
        let mut tr = [1.0f32; 3];
        for c in casters {
            if std::ptr::eq(*c, it) || c.layer.id == it.layer.id {
                continue;
            }
            let o = occlusion(c, p, l);
            for k in 0..3 {
                tr[k] *= o[k];
            }
        }
        tr
    };
    let (lit, shadow_amt) = shade(rgb, p, g.normal, g.cam.to_viewer(p), &it.mat, lights, &shadow);
    if it.mat.accepts_shadows == 2 {
        return [0.0, 0.0, 0.0, a * shadow_amt];
    }
    [lit[0] * a, lit[1] * a, lit[2] * a, a]
}

/// RGB transmittance of light `l` reaching `p` past caster `c` (1 = unoccluded).
fn occlusion(c: &Item, p: Vec3, l: &LightState) -> [f32; 3] {
    const FAR: f64 = 1.0e6;
    let a = c.winv.apply(p);
    let target = if l.kind == LightKind::Parallel { p - l.dir * FAR } else { l.pos };
    let b = c.winv.apply(target);
    if a.z == b.z || a.z.signum() == b.z.signum() {
        return [1.0; 3];
    }
    let t = a.z / (a.z - b.z);
    if t <= 1e-6 || t >= 1.0 - 1e-9 {
        return [1.0; 3];
    }
    let hit = a + (b - a) * t;
    // Penumbra radius (layer pixels) from Shadow Diffusion: grows with receiver–caster distance.
    let seg = (target - p).length();
    let d_pc = seg * t;
    let d_cl = if l.kind == LightKind::Parallel { 1000.0 } else { (seg * (1.0 - t)).max(1.0) };
    let r = l.shadow_diffusion * (d_pc / d_cl).min(4.0);
    let bd = &c.bounds;
    if hit.x < bd[0] - r || hit.y < bd[1] - r || hit.x > bd[2] + r || hit.y > bd[3] + r {
        return [1.0; 3];
    }
    let mut px = [0.0f32; 4];
    if r * c.buf.scale < 0.5 {
        px = c.texel(hit.x, hit.y);
    } else {
        for j in -1..=1 {
            for i in -1..=1 {
                let q = c.texel(hit.x + i as f64 * r * 0.66, hit.y + j as f64 * r * 0.66);
                for k in 0..4 {
                    px[k] += q[k] / 9.0;
                }
            }
        }
    }
    let alpha = (px[3] * c.opacity).clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return [1.0; 3];
    }
    let tr = c.mat.light_transmission;
    let dark = l.shadow_darkness as f32;
    let mut out = [1.0f32; 3];
    for k in 0..3 {
        let straight = if px[3] > 0.0 { (px[k] / px[3]).clamp(0.0, 1.0) } else { 0.0 };
        let pass = (1.0 - alpha) + alpha * tr * straight;
        out[k] = 1.0 - dark * (1.0 - pass);
    }
    out
}

// ------------------------------------------------------------------ accelerator export

/// A Classic 3D run prepared for an accelerator ([`Renderer::prepare_3d_run`]): every plane
/// with its geometry, material and buffer, ready for the per-pixel depth sort, lighting and
/// ray-cast shadows of [`draw_run`] on another device.
#[derive(Clone, Debug)]
pub struct Run3d {
    /// Drawn planes in stack order, then shadow casters from outside the run (`draw` false).
    pub planes: Vec<Plane3d>,
    /// Indices (into `planes`) of the shadow casters, in the CPU's order.
    pub casters: Vec<usize>,
    pub lights: Vec<LightState>,
    /// Dissolve seed of the run.
    pub seed: u32,
}

/// One plane of a [`Run3d`].
#[derive(Clone, Debug)]
pub struct Plane3d {
    /// The layer buffer (blending space; depth of field already applied unless `dof` is set).
    pub buf: Arc<Buf>,
    /// Depth of field for the accelerator to apply to `buf` (`bounds` are then the padded
    /// buffer's).
    pub dof: Option<PlaneDof>,
    pub bicubic: bool,
    /// One per motion-blur sub-sample (equal weights).
    pub geos: Vec<Geo>,
    pub mat: Material,
    pub opacity: f32,
    pub draw: bool,
    /// Screen-space track matte factor (output size, row-major).
    pub matte: Option<Arc<Vec<f32>>>,
    pub preserve: bool,
    /// World → layer, and the (blurred) buffer's layer-space bounds.
    pub winv: Mat4,
    pub bounds: [f64; 4],
    /// Output pixel bounds of all geometries.
    pub bbox: [i64; 4],
    /// Stack order (coplanar fragments draw lower orders first).
    pub order: usize,
    pub layer_id: u32,
    pub mode: BlendMode,
}

/// Prepare a run of 3D layers for an accelerator. `None` when the run must be drawn on the CPU
/// (adjustment, wireframe or environment background layers, Advanced 3D).
pub(crate) fn gpu_run(r: &Renderer, ctx: &EvalCtx, run: &[&Layer], out: (u32, u32)) -> Option<Run3d> {
    if run.is_empty() || run.iter().any(|l| l.switches.adjustment || l.environment_background || r.quality(l) == effectcraft_project::Quality::Wireframe) {
        return None;
    }
    if super::adv::active(r, ctx) {
        return None;
    }
    // Depth of field is left to the accelerator (its blur runs where the planes are drawn).
    let rd = Renderer { defer_dof: true, ..*r };
    let g = gather(&rd, ctx, run, out);
    let planes: Vec<Plane3d> = g
        .items
        .into_iter()
        .chain(g.extra)
        .map(|it| Plane3d {
            layer_id: it.layer.id.0 as u32,
            mode: it.layer.blend_mode,
            buf: it.buf,
            dof: it.dof,
            bicubic: it.bicubic,
            geos: it.geos,
            mat: it.mat,
            opacity: it.opacity,
            draw: it.draw,
            matte: it.matte,
            preserve: it.preserve,
            winv: it.winv,
            bounds: it.bounds,
            bbox: it.bbox,
            order: it.order,
        })
        .collect();
    let casters = planes.iter().enumerate().filter(|(_, p)| r.opts.shadows() && p.mat.casts_shadows != 0 && !p.geos.is_empty()).map(|(i, _)| i).collect();
    Some(Run3d { planes, casters, lights: g.lights, seed: run[0].id.0 as u32 })
}

// ------------------------------------------------------------------ auxiliary render pass

/// One surface seen through an output pixel (aux pass).
#[derive(Clone, Copy)]
struct AuxFrag {
    z: f32,
    a: f32,
    obj: f32,
    mat: f32,
    n: [f32; 3],
    uv: [f32; 2],
    obj_hash: u32,
    mat_hash: u32,
}

/// The comp's auxiliary channels at the context time, at output resolution: camera-space depth
/// (`Z`), `ObjectID` (layer number), `MaterialID` (source item id), `Coverage`, camera-space
/// normals (`N.X/Y/Z`), texture coordinates (`UV.U/V`) and Cryptomatte `CryptoObject` (layer
/// names) / `CryptoMaterial` (source names) with two ranks each, plus their manifests.
///
/// Layers are gathered in stack order (top first); a run of 3D layers contributes its planes
/// sorted near → far at each pixel, a 2D layer its coverage on the plane of the comp. The
/// front-most surface with ≥ 50 % opacity (else the strongest one) provides depth, IDs, normal
/// and UV; Cryptomatte coverage is each surface's visible share (`α · Π(1 − α_front)`).
pub(crate) fn aux_pass(r: &Renderer, ctx: &EvalCtx) -> effectcraft_raster::AuxChannels {
    use effectcraft_raster::channels3d::{BACKGROUND_DEPTH, crypto_float, crypto_hash};
    let s = r.opts.scale;
    let (w, h) = (((ctx.comp.width as f64 * s).round() as u32).max(1), ((ctx.comp.height as f64 * s).round() as u32).max(1));
    let out = (w, h);
    let (wu, n) = (w as usize, w as usize * h as usize);
    let cam = camera_for(r, ctx);
    let visible: Vec<&Layer> = r.visible_layers(ctx).into_iter().filter(|l| !l.switches.adjustment).collect();
    let number = |l: &Layer| ctx.comp.layers.iter().position(|x| x.id == l.id).map_or(0.0, |i| i as f32 + 1.0);
    let source_name = |l: &Layer| l.source.item().and_then(|i| r.project.item(i)).map(|it| it.name.clone()).unwrap_or_else(|| l.name.clone());
    let source_id = |l: &Layer| l.source.item().map_or(0.0, |i| i.0 as f32);
    let mut manifest_obj: Vec<(String, u32)> = vec![];
    let mut manifest_mat: Vec<(String, u32)> = vec![];
    for l in &visible {
        let (on, mn) = (l.name.clone(), source_name(l));
        if !manifest_obj.iter().any(|(x, _)| *x == on) {
            manifest_obj.push((on.clone(), crypto_hash(&on)));
        }
        if !manifest_mat.iter().any(|(x, _)| *x == mn) {
            manifest_mat.push((mn.clone(), crypto_hash(&mn)));
        }
    }
    // Groups in stack order: runs of 3D layers, single 2D layers.
    let mut groups: Vec<&[&Layer]> = vec![];
    let mut i = 0;
    while i < visible.len() {
        let mut j = i + 1;
        if visible[i].is_3d() {
            while j < visible.len() && visible[j].is_3d() {
                j += 1;
            }
        }
        groups.push(&visible[i..j]);
        i = j;
    }
    let mut frags: Vec<Vec<AuxFrag>> = vec![Vec::new(); n];
    let plane_z = cam.depth(vec3(ctx.comp.width as f64 * 0.5, ctx.comp.height as f64 * 0.5, 0.0)) as f32;
    for g in groups.iter().rev() {
        if g[0].is_3d() {
            let items: Vec<Item> = g.iter().enumerate().flat_map(|(k, l)| prepare_all(r, ctx, l, k, out, true)).collect();
            let meta: Vec<(f32, f32, u32, u32, [f32; 3], [f64; 2])> = items
                .iter()
                .map(|it| {
                    let nrm = it.geos.first().map_or(vec3(0.0, 0.0, -1.0), |gm| cam.view.apply_vec(gm.normal));
                    let mut nv = [nrm.x as f32, nrm.y as f32, nrm.z as f32];
                    if nv[2] > 0.0 {
                        nv = nv.map(|v| -v);
                    }
                    let (sw, sh) = crate::source_size(r.project, it.layer);
                    let size = if sw == 0 { [ctx.comp.width as f64, ctx.comp.height as f64] } else { [sw as f64, sh as f64] };
                    (number(it.layer), source_id(it.layer), crypto_hash(&it.layer.name), crypto_hash(&source_name(it.layer)), nv, size)
                })
                .collect();
            frags.par_chunks_mut(wu).enumerate().for_each(|(y, row)| {
                let yi = y as i64;
                let mut local: Vec<AuxFrag> = Vec::new();
                for (x, cell) in row.iter_mut().enumerate() {
                    let xi = x as i64;
                    local.clear();
                    for (it, m) in items.iter().zip(&meta) {
                        if xi < it.bbox[0] || xi >= it.bbox[2] || yi < it.bbox[1] || yi >= it.bbox[3] {
                            continue;
                        }
                        let Some(gm) = it.geos.first() else { continue };
                        let Some((u, v, z)) = unproject(gm, x as f64 + 0.5, y as f64 + 0.5) else { continue };
                        let a = it.texel(u, v)[3] * it.opacity;
                        if a > 1e-4 {
                            let uv = [(u / m.5[0]) as f32, (v / m.5[1]) as f32];
                            local.push(AuxFrag { z: z as f32, a: a.min(1.0), obj: m.0, mat: m.1, n: m.4, uv, obj_hash: m.2, mat_hash: m.3 });
                        }
                    }
                    local.sort_by(|p, q| p.z.total_cmp(&q.z));
                    cell.extend_from_slice(&local);
                }
            });
        } else {
            let l = g[0];
            let mut img = Image::new(w, h);
            if let Some(b) = r.layer_buf(ctx, l) {
                r.place(ctx, l, &b, &mut img, BlendMode::Normal, ctx.opacity(l) as f32);
            }
            let (obj, mat, oh, mh) = (number(l), source_id(l), crypto_hash(&l.name), crypto_hash(&source_name(l)));
            frags.par_chunks_mut(wu).zip(img.data.par_chunks(wu)).enumerate().for_each(|(y, (row, px))| {
                for (x, (cell, p)) in row.iter_mut().zip(px).enumerate() {
                    if p[3] > 1e-4 {
                        let uv = [(x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32];
                        cell.push(AuxFrag { z: plane_z, a: p[3].min(1.0), obj, mat, n: [0.0, 0.0, -1.0], uv, obj_hash: oh, mat_hash: mh });
                    }
                }
            });
        }
    }
    // Resolve per pixel.
    const NAMES: [&str; 9] = ["Z", "ObjectID", "MaterialID", "Coverage", "N.X", "N.Y", "N.Z", "UV.U", "UV.V"];
    const CRYPTO: usize = 16;
    let resolved: Vec<([f32; 9], [f32; CRYPTO])> = frags
        .par_iter()
        .map(|fs| {
            let mut o = [BACKGROUND_DEPTH, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
            let mut crypto = [0.0f32; CRYPTO];
            if fs.is_empty() {
                return (o, crypto);
            }
            let mut t = 1.0f32;
            let mut obj: Vec<(u32, f32)> = vec![];
            let mut mat: Vec<(u32, f32)> = vec![];
            let mut best: Option<(usize, f32)> = None;
            let mut front: Option<usize> = None;
            for (k, f) in fs.iter().enumerate() {
                let c = f.a * t;
                if front.is_none() && f.a >= 0.5 {
                    front = Some(k);
                }
                if best.is_none_or(|(_, bc)| c > bc) {
                    best = Some((k, c));
                }
                for (list, hsh) in [(&mut obj, f.obj_hash), (&mut mat, f.mat_hash)] {
                    match list.iter_mut().find(|(x, _)| *x == hsh) {
                        Some(e) => e.1 += c,
                        None => list.push((hsh, c)),
                    }
                }
                t *= 1.0 - f.a;
                if t <= 1e-5 {
                    break;
                }
            }
            let f = fs[front.or(best.map(|b| b.0)).unwrap_or(0)];
            o = [f.z, f.obj, f.mat, 1.0 - t, f.n[0], f.n[1], f.n[2], f.uv[0], f.uv[1]];
            for (base, list) in [(0usize, &mut obj), (8usize, &mut mat)] {
                list.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
                for (rank, (hsh, c)) in list.iter().take(4).enumerate() {
                    crypto[base + rank * 2] = crypto_float(*hsh);
                    crypto[base + rank * 2 + 1] = *c;
                }
            }
            (o, crypto)
        })
        .collect();
    let mut aux = effectcraft_raster::AuxChannels::new(w, h, s);
    for (k, name) in NAMES.iter().enumerate() {
        aux.channels.push((name.to_string(), resolved.iter().map(|r| r.0[k]).collect()));
    }
    for (base, layer) in [(0usize, "CryptoObject"), (8usize, "CryptoMaterial")] {
        for rank_pair in 0..2 {
            for (c, ch) in ["R", "G", "B", "A"].iter().enumerate() {
                let k = base + rank_pair * 4 + c;
                aux.channels.push((format!("{layer}{rank_pair:02}.{ch}"), resolved.iter().map(|r| r.1[k]).collect()));
            }
        }
    }
    aux.manifests = vec![("CryptoObject".into(), manifest_obj), ("CryptoMaterial".into(), manifest_mat)];
    aux
}

/// Rotation (radians) of the comp's first Environment light, which also turns its backdrop.
pub(crate) fn environment_rotation(ctx: &EvalCtx) -> f32 {
    ctx.comp
        .layers
        .iter()
        .find(|l| matches!(l.source, effectcraft_project::LayerSource::Light { kind: LightKind::Environment }) && l.is_active_at(ctx.time))
        .and_then(|l| l.props.sub("lightOptions").map(|g| ctx.f(l, g, "rotation", 0.0)))
        .unwrap_or(0.0)
        .to_radians() as f32
}

/// An Environment Light Background layer ready to draw (see [`draw_sky`]): the layer's
/// buffer in the blending space, its opacity and the camera's view rays. Also handed to an
/// accelerator's compositor (`Renderer::sky_draw`).
#[derive(Clone)]
pub struct SkyDraw {
    pub buf: Arc<Buf>,
    pub opacity: f32,
    /// Camera basis: forward, right, down.
    pub fwd: [f64; 3],
    pub right: [f64; 3],
    pub down: [f64; 3],
    pub zoom: f64,
    pub ortho: bool,
    /// View size (comp pixels), region-of-interest offset (output pixels), output scale.
    pub view: (f64, f64),
    pub roi: (f64, f64),
    pub scale: f64,
    /// The Environment light's rotation (radians).
    pub rotation: f32,
}

pub(crate) fn sky_draw(r: &Renderer, ctx: &EvalCtx, layer: &Layer) -> Option<SkyDraw> {
    let buf = r.blend_layer_buf(ctx, layer)?;
    if buf.img.width == 0 || buf.img.height == 0 {
        return None;
    }
    let op = (ctx.opacity(layer) as f32 * r.opacity_mul()).clamp(0.0, 1.0);
    if op <= 0.0 {
        return None;
    }
    let cam = camera_for(r, ctx);
    let v3 = |v: Vec3| [v.x, v.y, v.z];
    Some(SkyDraw {
        buf,
        opacity: op,
        fwd: v3(cam.forward()),
        right: v3(cam.right()),
        down: v3(cam.down()),
        zoom: cam.zoom,
        ortho: cam.ortho,
        view: view_size(r, ctx),
        roi: r.roi_offset().map_or((0.0, 0.0), |(x, y, _, _)| (x, y)),
        scale: r.opts.scale.max(1e-9),
        rotation: environment_rotation(ctx),
    })
}

/// Draw an Environment Light Background layer: its (equirectangular) image looked up by each
/// pixel's view direction through the camera, as an infinitely distant backdrop.
pub(crate) fn draw_sky(r: &Renderer, ctx: &EvalCtx, layer: &Layer, canvas: &mut Image) {
    let Some(sky) = sky_draw(r, ctx, layer) else { return };
    let img = &sky.buf.img;
    let op = sky.opacity;
    let v3 = |v: [f64; 3]| vec3(v[0], v[1], v[2]);
    let (fwd, right, down) = (v3(sky.fwd), v3(sky.right), v3(sky.down));
    let s = sky.scale;
    let (cw, ch) = sky.view;
    let (ox, oy) = sky.roi;
    let rot = sky.rotation;
    let (iw, ih) = (img.width as f64, img.height as f64);
    canvas.rows_mut().for_each(|(y, row)| {
        let cy = (y as f64 + 0.5 + oy) / s;
        for (x, d) in row.iter_mut().enumerate() {
            let cx = (x as f64 + 0.5 + ox) / s;
            let dir = if sky.ortho { fwd } else { (fwd * sky.zoom + right * (cx - cw / 2.0) + down * (cy - ch / 2.0)).normalize() };
            let (u, v) = super::adv::shade::equirect_uv([dir.x as f32, dir.y as f32, dir.z as f32], rot);
            // Wrap horizontally across the seam.
            let sx = (u as f64 * iw).clamp(0.5, iw - 0.5);
            let sy = (v as f64 * ih).clamp(0.5, ih - 0.5);
            let p = img.sample_bilinear(sx, sy);
            let k = 1.0 - p[3] * op;
            *d = [p[0] * op + d[0] * k, p[1] * op + d[1] * k, p[2] * op + d[2] * k, p[3] * op + d[3] * k];
        }
    });
}
