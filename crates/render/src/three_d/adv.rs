//! The Advanced 3D renderer (Composition Settings ▸ 3D Renderer ▸ Advanced 3D).
//!
//! A run of 3D layers becomes one [`Scene`] of world-space triangles (models, primitives,
//! extruded text and shapes, and textured cards for every other layer), lit physically
//! ([`shade`]) by the comp's lights, an Environment light's image and shadow maps. The scene
//! is rasterised with a depth buffer on the GPU when an [`crate::Accelerator`] implements
//! [`crate::Accelerator::raster_3d`], otherwise by the software rasteriser ([`raster`]), at 2×2
//! supersampling. The resolve (box filter), depth of field (a gather blur by each pixel's
//! circle of confusion, shaped by the camera's iris: [`dof`]) and the conversion back to the working encoding run on the CPU for
//! both paths, and the result is composited over the layers below the run. An accelerator's
//! compositor runs all of it on the device: [`Prepared`] for whole runs, [`SplitRun`] for runs
//! with layers on the 2D compositing path (`draw_run`'s steps as data).

pub mod dof;
pub mod raster;
pub mod scene;
pub mod shade;

use effectcraft_project::Layer;
use rayon::prelude::*;

pub use dof::depth_of_field;
pub use raster::Target;
pub use scene::{EnvInfo, Light, Material, Scene, ShadowMap, TexInfo, Vertex};

use super::compose::camera_for;
use crate::{EvalCtx, Image, Renderer};

/// Whether a comp draws its 3D layers with the Advanced 3D renderer: its own setting, or for a
/// collapsed precomp's layers, the setting of the outermost comp they collapse into.
pub(crate) fn active(r: &Renderer, ctx: &EvalCtx) -> bool {
    r.collapse3d.map_or(ctx.comp, |c| c.parent.comp).renderer == effectcraft_project::Renderer::Advanced3D
}

/// Box-filter a supersampled target down by `k`: (premultiplied colour, mean depth of the
/// covered subsamples, ∞ where none).
pub fn resolve(t: &Target, k: u32) -> (Image, Vec<f32>) {
    let (w, h) = (t.width / k, t.height / k);
    let mut img = Image::new(w, h);
    let mut depth = vec![f32::INFINITY; (w * h) as usize];
    let n = (k * k) as f32;
    img.data.par_chunks_mut(w as usize).zip(depth.par_chunks_mut(w as usize)).enumerate().for_each(|(y, (row, drow))| {
        for x in 0..w as usize {
            let mut acc = [0.0f32; 4];
            let (mut dz, mut dn) = (0.0f32, 0.0f32);
            for j in 0..k as usize {
                for i in 0..k as usize {
                    let s = (y * k as usize + j) * t.width as usize + x * k as usize + i;
                    let c = t.color[s];
                    for q in 0..4 {
                        acc[q] += c[q];
                    }
                    if t.depth[s].is_finite() {
                        dz += t.depth[s];
                        dn += 1.0;
                    }
                }
            }
            row[x] = acc.map(|v| v / n);
            if dn > 0.0 {
                drow[x] = dz / dn;
            }
        }
    });
    (img, depth)
}

/// Linear → sRGB-encoded premultiplied pixels (the working encoding of non-linear projects).
pub fn encode(img: &mut Image) {
    img.data.par_iter_mut().for_each(|p| {
        let a = p[3];
        if a > 1e-6 {
            for c in 0..3 {
                p[c] = shade::linear_to_srgb(p[c] / a) * a;
            }
        }
    });
}

/// Render a scene on the CPU, resolved: (premultiplied image at output size, depth).
pub fn render_cpu(s: &Scene) -> (Image, Vec<f32>) {
    resolve(&raster::render(s), s.ssaa.max(1))
}

/// Build the scene of a run (for tests and tools).
pub fn scene_of(r: &Renderer, ctx: &EvalCtx, run: &[&Layer], out: (u32, u32)) -> Scene {
    scene::build(r, ctx, run, out)
}

/// Rasterise one scene (GPU when available, else CPU) and resolve it.
fn raster_resolved(r: &Renderer, s: &Scene) -> (Image, Vec<f32>) {
    let target = r.active_accel().and_then(|a| a.raster_3d(s)).filter(|t| t.width == s.width && t.height == s.height).unwrap_or_else(|| raster::render(s));
    resolve(&target, s.ssaa.max(1))
}

/// Number of motion-blur sub-samples for a set of layers (1 = none blurred).
fn mb_samples(r: &Renderer, ctx: &EvalCtx, layers: &[&Layer]) -> usize {
    if !layers.iter().any(|l| scene::motion_blurred(r, ctx, l)) {
        return 1;
    }
    if r.opts.draft { 4 } else { ctx.comp.motion_blur_samples.clamp(2, 64) as usize }
}

/// An Advanced 3D run prepared for an accelerator ([`crate::Accelerator::render_3d`], the GPU
/// compositor's walk): the motion-blur sub-sample scenes (built on demand, one at a time), the
/// camera's depth of field and the output encoding. The accelerator rasterises every
/// sub-sample, resolves the supersampling, averages the sub-samples (nearest depth), applies
/// the depth of field and encodes, as [`render_prepared`] does on the CPU.
pub struct Prepared<'p, 'a> {
    r: &'p Renderer<'a>,
    ctx: &'p EvalCtx<'a>,
    layers: &'p [&'p Layer],
    /// Output size (pixels).
    pub out: (u32, u32),
    /// Motion-blur sub-samples (1 = none).
    pub samples: usize,
    /// Depth of field (the camera at the frame time; `None` = off, Draft 3D or draft renders).
    pub dof: Option<super::camera::Dof>,
    /// Render scale (circles of confusion are in comp pixels).
    pub scale: f64,
    /// The scene is in the working encoding already (linear projects): no sRGB encode.
    pub linear: bool,
}

impl<'p, 'a> Prepared<'p, 'a> {
    pub fn new(r: &'p Renderer<'a>, ctx: &'p EvalCtx<'a>, layers: &'p [&'p Layer], out: (u32, u32)) -> Self {
        let cam = camera_for(r, ctx);
        Prepared {
            r,
            ctx,
            layers,
            out,
            samples: mb_samples(r, ctx, layers),
            dof: cam.dof.filter(|_| !ctx.comp.draft_3d && !r.opts.draft),
            scale: r.opts.scale,
            linear: r.pipe.linear || r.pipe.linear_blend,
        }
    }

    /// The scene of sub-sample `i` (`0..samples`), spread over the comp's shutter (angle and
    /// phase).
    pub fn scene(&self, i: usize) -> Scene {
        let n = self.samples;
        if n <= 1 {
            return scene::build(self.r, self.ctx, self.layers, self.out);
        }
        let ctx = self.ctx;
        let fd = ctx.comp.frame_duration().seconds();
        let (angle, phase) = (ctx.comp.shutter_angle / 360.0, ctx.comp.shutter_phase / 360.0);
        let f = phase + angle * i as f64 / (n - 1) as f64;
        let sub = ctx.at(ctx.time + effectcraft_time::Tick::from_seconds_f64(f * fd));
        scene::build_at(self.r, ctx, Some(&sub), self.layers, self.out)
    }
}

/// What an accelerator returns for a [`Prepared`] run: `None` when nothing was drawn, else the
/// premultiplied image in the canvas's encoding and the camera depth of each pixel (∞ where
/// nothing was drawn; the nearest over the motion-blur sub-samples).
pub type Rendered = Option<(Image, Vec<f32>)>;

/// Render a prepared run on the CPU compositor (each scene rasterised by the accelerator's
/// [`crate::Accelerator::raster_3d`] when there is one): the reference for
/// [`crate::Accelerator::render_3d`].
pub fn render_prepared(p: &Prepared) -> Rendered {
    let (r, n, out) = (p.r, p.samples, p.out);
    let (mut img, depth) = if n <= 1 {
        let s = p.scene(0);
        if s.indices.is_empty() {
            return None;
        }
        raster_resolved(r, &s)
    } else {
        let mut acc = Image::new(out.0, out.1);
        let mut depth = vec![f32::INFINITY; (out.0 * out.1) as usize];
        let mut any = false;
        let k = 1.0 / n as f32;
        for i in 0..n {
            let s = p.scene(i);
            if s.indices.is_empty() {
                continue;
            }
            any = true;
            let (img, d) = raster_resolved(r, &s);
            acc.data.par_iter_mut().zip(img.data.par_iter()).for_each(|(a, p)| {
                for c in 0..4 {
                    a[c] += p[c] * k;
                }
            });
            depth.par_iter_mut().zip(d.par_iter()).for_each(|(a, b)| *a = a.min(*b));
        }
        if !any {
            return None;
        }
        (acc, depth)
    };
    if let Some(dof) = &p.dof {
        img = depth_of_field(&img, &depth, dof, p.scale);
    }
    if !p.linear {
        encode(&mut img);
    }
    Some((img, depth))
}

/// Render `layers` as one Advanced 3D scene: premultiplied pixels in the canvas's encoding and
/// the camera depth of each pixel (∞ where nothing was drawn). With motion blur the scene is
/// rendered at sub-samples spread over the comp's shutter (angle and phase) and averaged; the
/// depth is the nearest over the samples. The whole run goes to the accelerator when it
/// implements [`crate::Accelerator::render_3d`].
pub(crate) fn render_layers(r: &Renderer, ctx: &EvalCtx, layers: &[&Layer], out: (u32, u32)) -> Option<(Image, Vec<f32>)> {
    let p = Prepared::new(r, ctx, layers, out);
    if let Some(a) = r.active_accel()
        && let Some(res) = a.render_3d(&p)
    {
        return res;
    }
    render_prepared(&p)
}

/// The run as one prepared scene for an accelerator's compositor when it can be composited
/// directly over the canvas (Normal-mode layers without track mattes or Preserve Transparency;
/// those go through `draw_run`'s 2D path; environment backgrounds draw their sky first). `None`
/// when the comp isn't Advanced 3D.
pub(crate) fn prepare_run<'p, 'a>(r: &'p Renderer<'a>, ctx: &'p EvalCtx<'a>, run: &'p [&'p Layer], out: (u32, u32)) -> Option<Prepared<'p, 'a>> {
    if !active(r, ctx) || run.iter().any(|l| l.environment_background || needs_2d_composite(ctx, l)) {
        return None;
    }
    Some(Prepared::new(r, ctx, run, out))
}

/// A run of an Advanced 3D comp whose layers need the 2D compositing path (blend modes, track
/// mattes, Preserve Transparency), prepared for an accelerator's compositor: `draw_run`'s
/// steps as data. The plain layers render as one scene ([`SplitRun::main`]) composited over the
/// canvas; then each special layer, from the farthest to the nearest, renders on its own
/// ([`SplitRun::special`]), is hidden where the main scene is nearer, and goes through the 2D
/// path with its track matte (a 3D matte layer drawn through the camera:
/// [`SplitRun::with_matte3d`]).
pub struct SplitRun<'p, 'a> {
    r: &'p Renderer<'a>,
    ctx: &'p EvalCtx<'a>,
    out: (u32, u32),
    plain: Vec<&'p Layer>,
    /// Far to near.
    special: Vec<&'p Layer>,
    /// Per special layer: its 3D track matte drawn solo (and whether it is active now).
    mattes: Vec<Option<(Layer, bool)>>,
}

impl<'p, 'a> SplitRun<'p, 'a> {
    /// The plain layers as one scene.
    pub fn main(&self) -> Prepared<'_, 'a> {
        Prepared::new(self.r, self.ctx, &self.plain, self.out)
    }

    /// Number of special layers.
    pub fn len(&self) -> usize {
        self.special.len()
    }

    pub fn is_empty(&self) -> bool {
        self.special.is_empty()
    }

    /// Special layer `i` (far to near).
    pub fn layer(&self, i: usize) -> &'p Layer {
        self.special[i]
    }

    /// Special layer `i` rendered on its own.
    pub fn special(&self, i: usize) -> Prepared<'_, 'a> {
        Prepared::new(self.r, self.ctx, std::slice::from_ref(&self.special[i]), self.out)
    }

    /// `f` with special layer `i`'s 3D track matte as a prepared run (`None` when the matte
    /// layer isn't active: a transparent matte). `None` when the layer has no 3D matte (its
    /// matte, if any, is placed in 2D as usual).
    pub fn with_matte3d<R>(&self, i: usize, f: impl FnOnce(Option<&Prepared<'_, 'a>>) -> R) -> Option<R> {
        let (solo, active) = self.mattes[i].as_ref()?;
        if !*active {
            return Some(f(None));
        }
        let layers = [solo];
        let p = Prepared::new(self.r, self.ctx, &layers, self.out);
        Some(f(Some(&p)))
    }
}

/// The run split for an accelerator when some layer needs the 2D compositing path (`None` when
/// the comp isn't Advanced 3D, a layer is an environment background, or no layer needs it:
/// `prepare_run` then covers the run).
pub(crate) fn split_run<'p, 'a>(r: &'p Renderer<'a>, ctx: &'p EvalCtx<'a>, run: &'p [&'p Layer], out: (u32, u32)) -> Option<SplitRun<'p, 'a>> {
    if !active(r, ctx) || run.iter().any(|l| l.environment_background) || !run.iter().any(|l| needs_2d_composite(ctx, l)) {
        return None;
    }
    let (special, plain): (Vec<&Layer>, Vec<&Layer>) = run.iter().copied().partition(|l| needs_2d_composite(ctx, l));
    let special = far_to_near(r, ctx, special);
    let mattes = special.iter().map(|l| solo_matte(ctx, l)).collect();
    Some(SplitRun { r, ctx, out, plain, special, mattes })
}

/// Special layers sorted by their anchor's camera depth, farthest first.
fn far_to_near<'p>(r: &Renderer, ctx: &EvalCtx, special: Vec<&'p Layer>) -> Vec<&'p Layer> {
    let cam = camera_for(r, ctx);
    let mut order: Vec<(f64, &Layer)> = special
        .into_iter()
        .map(|l| {
            let anchor = l.transform().map_or([0.0; 3], |tr| ctx.v3(l, tr, "anchor", [0.0; 3]));
            (cam.depth(ctx.world_matrix(l).apply(effectcraft_geom::Vec3::from(anchor))), l)
        })
        .collect();
    order.sort_by(|a, b| b.0.total_cmp(&a.0));
    order.into_iter().map(|(_, l)| l).collect()
}

/// A layer's 3D track matte, drawn through the camera on its own (no matte, Normal mode, no
/// Preserve Transparency, visible), and whether it is active now.
fn solo_matte(ctx: &EvalCtx, l: &Layer) -> Option<(Layer, bool)> {
    let m = l.track_matte.and_then(|tm| ctx.comp.layer(tm.layer).filter(|m| m.id != l.id && m.is_3d()))?;
    let mut solo = m.clone();
    solo.track_matte = None;
    solo.blend_mode = effectcraft_color::BlendMode::Normal;
    solo.preserve_transparency = false;
    solo.switches.video = true;
    Some((solo, m.is_active_at(ctx.time)))
}

/// Layers whose blend mode, track matte or Preserve Transparency needs the 2D compositing path.
fn needs_2d_composite(ctx: &EvalCtx, l: &Layer) -> bool {
    l.blend_mode != effectcraft_color::BlendMode::Normal
        || l.preserve_transparency
        || l.track_matte.is_some_and(|tm| tm.layer != l.id && ctx.comp.layer(tm.layer).is_some())
}

/// Draw a run of 3D layers with the Advanced 3D renderer into `canvas`.
///
/// Layers with a blend mode, a track matte or Preserve Transparency can't blend inside the
/// depth-buffered scene: they are rendered on their own (lit by the same lights, through the
/// same camera), hidden where the rest of the run is nearer, then composited through the 2D
/// path (matte, Preserve Transparency, blend mode) from the farthest to the nearest.
pub(crate) fn draw_run(r: &Renderer, ctx: &EvalCtx, run: &[&Layer], canvas: &mut Image) {
    let out = (canvas.width, canvas.height);
    let (special, plain): (Vec<&Layer>, Vec<&Layer>) = run.iter().copied().partition(|l| needs_2d_composite(ctx, l));
    let main = render_layers(r, ctx, &plain, out);
    if let Some((img, _)) = &main {
        canvas.data.par_iter_mut().zip(img.data.par_iter()).for_each(|(d, s)| {
            let k = 1.0 - s[3];
            *d = [s[0] + d[0] * k, s[1] + d[1] * k, s[2] + d[2] * k, s[3] + d[3] * k];
        });
    }
    if special.is_empty() {
        return;
    }
    for l in far_to_near(r, ctx, special) {
        let Some((mut iso, depth)) = render_layers(r, ctx, &[l], out) else { continue };
        if let Some((m, md)) = &main {
            iso.data.par_iter_mut().zip(depth.par_iter()).zip(m.data.par_iter().zip(md.par_iter())).for_each(|((p, z), (q, mz))| {
                if *mz < *z - 0.01 * z.abs().max(1.0) {
                    let k = 1.0 - q[3].clamp(0.0, 1.0);
                    for c in p.iter_mut() {
                        *c *= k;
                    }
                }
            });
        }
        // A 3D matte layer is drawn through the camera too.
        let matte3d = solo_matte(ctx, l).map(|(solo, active)| {
            if active { render_layers(r, ctx, &[&solo], out).map(|(i, _)| i).unwrap_or_else(|| Image::new(out.0, out.1)) } else { Image::new(out.0, out.1) }
        });
        r.composite_iso_with(ctx, l, iso, canvas, 1.0, matte3d);
    }
}

#[cfg(test)]
mod tests;
