//! GPU effects, generators of part C (kernels in `shaders/fx_gen2.wgsl`, every entry point
//! prefixed `fg2_`): Lightning, Advanced Lightning, Beam, Fractal, Lens Flare, Radio Waves,
//! Vegas, Stroke, Scribble, Write-on, Paint Bucket, Eyedropper Fill, CC Glue Gun, CC Threads,
//! Audio Spectrum, Audio Waveform, Basic Text and Path Text.
//!
//! Geometry stays on the CPU and is shared with the CPU effect as a per-frame plan: the bolts
//! (`effects::bolt_plan`, `effects::lightning_segments`), mask strokes, scribbles and contour
//! segments (`effects::paint_plan`), audio marks (`effects::marks_plan`: the FFT and peak
//! analysis), glyph strokes (`effects::text_passes`), live waves and their contour
//! (`effects::radio_plan`), Paint Bucket's flood-filled region and Eyedropper Fill's sampled
//! colour (both over the read-back layer). The plans' segments and polygons are binned into
//! 16 × 16 tiles on the CPU and rasterised here with the CPU's coverage functions
//! (`raster_segs`' smooth round caps, `raster_convex`'s 4 × 4 samples); the colouring, glows
//! (util::gauss_plane) and compositing run here too. Beam, Lens Flare, CC Glue Gun and CC
//! Threads are closed-form kernels.
//!
//! Fractal stays on the CPU: its escape iteration needs f64 (WGSL has none, and Metal's fast
//! math defeats double-f32 emulation); in f32, orbits near the set escape at other iterations
//! on 0.3–1.5 % of the pixels even at low magnification.

use effectcraft_effects::{Buf, Coverage, EffectCtx};
use effectcraft_raster::Image;

use crate::context::{Enc, GpuImage, Params};
use crate::effects::GBuf;
use crate::fx_pixel2::gauss_plane;

/// Compute entry points in `fx_gen2.wgsl`.
pub(crate) const KERNELS: &[&str] = &[
    "fg2_segs",
    "fg2_convex",
    "fg2_paint",
    "fg2_marks",
    "fg2_pair",
    "fg2_bolt",
    "fg2_adv_bolt",
    "fg2_text",
    "fg2_radio",
    "fg2_beam",
    "fg2_flare",
    "fg2_glue",
    "fg2_threads",
    "fg2_bucket",
    "fg2_bucket_cov",
    "fg2_eyedropper",
];

/// Effect ids implemented here.
pub(crate) const IDS: &[&str] = &[
    "ec.obsolete.lightning",
    "ec.generate.advancedlightning",
    "ec.generate.beam",
    "ec.generate.lensflare",
    "ec.generate.radiowaves",
    "ec.generate.vegas",
    "ec.generate.stroke",
    "ec.generate.scribble",
    "ec.generate.writeon",
    "ec.generate.paintbucket",
    "ec.generate.eyedropperfill",
    "ec.generate.ccgluegun",
    "ec.generate.ccthreads",
    "ec.generate.audiospectrum",
    "ec.generate.audiowaveform",
    "ec.obsolete.basictext",
    "ec.obsolete.pathtext",
];

/// Largest item table (floats) the raster kernels take; bigger plans render on the CPU.
const MAX_FLOATS: usize = 1 << 24;
const TILE: i32 = 16;
/// "No minimum radius" for a segment variant.
const NO_MIN: f32 = -3.0e38;

/// Run effect `id` (one of [`IDS`]); `None` = this parameter combination runs on the CPU.
pub(crate) fn apply(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    match id {
        "ec.obsolete.lightning" => lightning(e, ctx, b),
        "ec.generate.advancedlightning" => advanced_lightning(e, ctx, b),
        "ec.generate.beam" => beam(e, ctx, b),
        "ec.generate.lensflare" => lens_flare(e, ctx, b),
        "ec.generate.radiowaves" => radio_waves(e, ctx, b),
        "ec.generate.vegas" | "ec.generate.stroke" | "ec.generate.scribble" | "ec.generate.writeon" => paint(e, id, ctx, b),
        "ec.generate.paintbucket" => paint_bucket(e, ctx, b),
        "ec.generate.eyedropperfill" => eyedropper(e, ctx, b),
        "ec.generate.ccgluegun" => glue_gun(e, ctx, b),
        "ec.generate.ccthreads" => threads(e, ctx, b),
        "ec.generate.audiospectrum" | "ec.generate.audiowaveform" => marks(e, id, ctx, b),
        "ec.obsolete.basictext" | "ec.obsolete.pathtext" => text(e, id, ctx, b),
        _ => None,
    }
}

/// A same-size per-pixel kernel over the buffer.
fn run(e: &mut Enc, entry: &str, p: &Params, mut b: GBuf, aux: Option<&GpuImage>, data: Option<&wgpu::Buffer>) -> Option<GBuf> {
    let out = e.scratch(b.img.width, b.img.height);
    e.pixels(entry, p, &b.img, aux, &out, data);
    b.img = out;
    Some(b)
}

/// The buffer on the CPU for building a plan: its pixels when `pixels` (read back), else only
/// its geometry.
fn cpu_buf(e: &mut Enc, b: &GBuf, pixels: bool) -> Option<Buf> {
    let img = if pixels { e.download(&b.img)? } else { Image { width: b.img.width, height: b.img.height, data: vec![] } };
    Some(Buf { img, offset: b.offset, scale: b.scale })
}

fn rgb(c: [f32; 4]) -> [f32; 4] {
    [c[0], c[1], c[2], 0.0]
}

// ---------------------------------------------------------------- tiled item raster

/// Items with their integer pixel bounds, binned into 16 × 16 tiles.
struct Items {
    stride: usize,
    recs: Vec<f32>,
    w: i32,
    h: i32,
    tiles: Vec<Vec<u32>>,
    tx: i32,
}

impl Items {
    fn new(stride: usize, w: u32, h: u32) -> Items {
        let (w, h) = (w as i32, h as i32);
        let tx = (w + TILE - 1) / TILE;
        let ty = (h + TILE - 1) / TILE;
        Items { stride, recs: vec![], w, h, tiles: vec![vec![]; (tx * ty).max(0) as usize], tx }
    }

    /// Add an item (`rec` = its fields, without the bounds) covering float bounds `bb`
    /// ([x0, y0, x1, y1]).
    fn push(&mut self, bb: [f64; 4], rec: &[f32]) {
        if !bb.iter().all(|v| v.is_finite()) {
            return;
        }
        let x0 = bb[0].floor().max(0.0) as i32;
        let y0 = bb[1].floor().max(0.0) as i32;
        let x1 = (bb[2].ceil().min(self.w as f64 - 1.0)) as i32;
        let y1 = (bb[3].ceil().min(self.h as f64 - 1.0)) as i32;
        if x1 < x0 || y1 < y0 || bb[2] < 0.0 || bb[3] < 0.0 {
            return;
        }
        let i = (self.recs.len() / self.stride) as u32;
        let start = self.recs.len();
        self.recs.extend_from_slice(rec);
        self.recs.resize(start + self.stride - 4, 0.0);
        self.recs.extend([x0 as f32, y0 as f32, x1 as f32, y1 as f32]);
        for ty in y0 / TILE..=y1 / TILE {
            for tx in x0 / TILE..=x1 / TILE {
                self.tiles[(ty * self.tx + tx) as usize].push(i);
            }
        }
    }

    /// Kernel data (tile offsets, item indices, items), the items' offset and the tile count
    /// across. `None` when too large.
    fn data(self) -> Option<(Vec<f32>, u32, u32)> {
        let n_tiles = self.tiles.len();
        let n_idx: usize = self.tiles.iter().map(Vec::len).sum();
        let total = n_tiles + 1 + n_idx + self.recs.len();
        if total > MAX_FLOATS {
            return None;
        }
        let mut d = Vec::with_capacity(total);
        let base = (n_tiles + 1) as f32;
        let mut at = 0usize;
        for t in &self.tiles {
            d.push(base + at as f32);
            at += t.len();
        }
        d.push(base + at as f32);
        for t in &self.tiles {
            d.extend(t.iter().map(|&i| i as f32));
        }
        let items_at = d.len() as u32;
        d.extend(self.recs);
        Some((d, items_at, self.tx.max(0) as u32))
    }
}

/// A coverage of the segment raster: radius max(r · `mul` + `add`, `min`) and hardness
/// (generate3::raster_segs), or Advanced Lightning's linear core.
#[derive(Clone, Copy)]
struct Variant {
    mul: f64,
    add: f64,
    min: f64,
    hard: f64,
    linear: bool,
}

impl Variant {
    fn plain(hard: f64) -> Variant {
        Variant { mul: 1.0, add: 0.0, min: NO_MIN as f64, hard, linear: false }
    }

    fn radius(&self, r: f64) -> f64 {
        (r * self.mul + self.add).max(self.min)
    }
}

/// Rasterise segments (a, b, radius, value) into up to four coverages (x, y, z, w), one per
/// variant, max-combined as raster_segs.
fn raster_segs(e: &mut Enc, w: u32, h: u32, segs: impl Iterator<Item = ([f64; 2], [f64; 2], f64, f32)>, variants: &[Variant]) -> Option<GpuImage> {
    let mut items = Items::new(10, w, h);
    for (a, z, r, v) in segs {
        let pad = variants.iter().map(|k| k.radius(r)).fold(f64::NEG_INFINITY, f64::max) + 1.0;
        let bb = [a[0].min(z[0]) - pad, a[1].min(z[1]) - pad, a[0].max(z[0]) + pad, a[1].max(z[1]) + pad];
        items.push(bb, &[a[0] as f32, a[1] as f32, z[0] as f32, z[1] as f32, r as f32, v]);
    }
    let (data, items_at, tx) = items.data()?;
    let buf = e.data(&data);
    let mut p = Params::default();
    p.u[0] = [items_at, 10, tx, variants.len() as u32];
    for (k, v) in variants.iter().enumerate() {
        p.f[k] = [v.mul as f32, v.add as f32, v.min as f32, v.hard as f32];
        p.u[1][0] |= (v.linear as u32) << k;
    }
    let out = e.scratch(w, h);
    let dummy = e.zeros(1, 1);
    e.pixels("fg2_segs", &p, &dummy, None, &out, Some(&buf));
    Some(out)
}

/// A plan's coverage (x).
fn coverage(e: &mut Enc, w: u32, h: u32, cov: &Coverage) -> Option<GpuImage> {
    match cov {
        Coverage::Segs { segs, hardness } => raster_segs(e, w, h, segs.iter().map(|s| (s.a, s.b, s.r, s.v)), &[Variant::plain(*hardness)]),
        Coverage::Convex(polys) => {
            const STRIDE: usize = 1 + 32 + 4;
            let mut items = Items::new(STRIDE, w, h);
            for poly in polys {
                if poly.len() < 3 {
                    continue;
                }
                if poly.len() > 16 {
                    return None;
                }
                let mut rec = vec![poly.len() as f32];
                let mut bb = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
                for q in poly {
                    rec.extend([q[0] as f32, q[1] as f32]);
                    bb = [bb[0].min(q[0]), bb[1].min(q[1]), bb[2].max(q[0]), bb[3].max(q[1])];
                }
                items.push(bb, &rec);
            }
            let (data, items_at, tx) = items.data()?;
            let buf = e.data(&data);
            let mut p = Params::default();
            p.u[0] = [items_at, STRIDE as u32, tx, 0];
            let out = e.scratch(w, h);
            let dummy = e.zeros(1, 1);
            e.pixels("fg2_convex", &p, &dummy, None, &out, Some(&buf));
            Some(out)
        }
    }
}

// ---------------------------------------------------------------- Stroke, Scribble, Vegas, Write-on

fn paint(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let cpu = cpu_buf(e, &b, effectcraft_effects::plan_reads_pixels(id, ctx))?;
    let Some(plan) = effectcraft_effects::paint_plan(id, ctx, &cpu) else {
        // Scribble without a usable mask: cleared On Transparent, else unchanged.
        if id == "ec.generate.scribble" && effectcraft_effects::scribble_style(ctx) == 1 {
            let img = e.image(b.img.width, b.img.height);
            return Some(GBuf { img, ..b });
        }
        return Some(b);
    };
    let cov = coverage(e, b.img.width, b.img.height, &plan.cov)?;
    let mut p = Params::default();
    p.u[0][0] = plan.style;
    p.f[0] = plan.color;
    p.f[1][0] = plan.opacity;
    run(e, "fg2_paint", &p, b, Some(&cov), None)
}

// ---------------------------------------------------------------- Audio Spectrum / Waveform

fn marks(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let cpu = cpu_buf(e, &b, false)?;
    let Some(m) = effectcraft_effects::marks_plan(id, ctx, &cpu) else { return Some(b) };
    let core = Variant::plain(1.0);
    let halo = if m.softness > 0.0 { Variant { add: m.thickness * m.softness.max(0.0) + 0.5, ..Variant::plain(0.0) } } else { core };
    let cov = raster_segs(e, b.img.width, b.img.height, m.segs.iter().map(|s| (s.a, s.b, s.r, s.v)), &[core, halo])?;
    let mut p = Params::default();
    p.u[0][0] = m.composite as u32;
    p.f[0] = m.inside;
    p.f[1] = m.outside;
    run(e, "fg2_marks", &p, b, Some(&cov), None)
}

// ---------------------------------------------------------------- Lightning / Advanced Lightning

fn lightning(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let cpu = cpu_buf(e, &b, false)?;
    let plan = effectcraft_effects::bolt_plan(ctx, &cpu);
    let (w, h) = (b.img.width, b.img.height);
    let core = Variant { mul: plan.core, min: 0.3, ..Variant::plain(0.6) };
    let cov = raster_segs(e, w, h, plan.segs.iter().map(|s| (s.a, s.b, s.r, s.v)), &[Variant::plain(0.0), core])?;
    let glow = gauss_plane(e, &cov, plan.width * 0.5, plan.width * 0.5)?;
    let m = e.scratch(w, h);
    e.pixels("fg2_pair", &Params::default(), &cov, Some(&glow), &m, None);
    let mut p = Params::default();
    p.u[0][0] = plan.mode;
    p.f[0] = rgb(plan.outside);
    p.f[1] = rgb(plan.inside);
    run(e, "fg2_bolt", &p, b, Some(&m), None)
}

fn advanced_lightning(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let cpu = cpu_buf(e, &b, effectcraft_effects::plan_reads_pixels("ec.generate.advancedlightning", ctx))?;
    let core_r = (pr.f("coreSettings/coreRadius") * b.scale).max(0.3);
    let glow_r = pr.f("glowSettings/glowRadius") * b.scale;
    let segs = effectcraft_effects::lightning_segments(ctx, &cpu);
    let (w, h) = (b.img.width, b.img.height);
    let linear = Variant { linear: true, ..Variant::plain(0.0) };
    let core = raster_segs(e, w, h, segs.iter().map(|(a, z, i)| ([a.0, a.1], [z.0, z.1], core_r, *i)), &[linear])?;
    let glow = glow_r > 0.5;
    let blurred = if glow { gauss_plane(e, &core, glow_r / 3.0, glow_r / 3.0)? } else { core.clone() };
    let m = e.scratch(w, h);
    e.pixels("fg2_pair", &Params::default(), &core, Some(&blurred), &m, None);
    let mut p = Params::default();
    p.u[0] = [pr.b("compositeOnOriginal") as u32, glow as u32, 0, 0];
    p.f[0] = [(pr.f("coreSettings/coreOpacity") / 100.0) as f32, (pr.f("glowSettings/glowOpacity") / 100.0) as f32, 0.0, 0.0];
    p.f[1] = rgb(pr.color("coreSettings/coreColor"));
    p.f[2] = rgb(pr.color("glowSettings/glowColor"));
    run(e, "fg2_adv_bolt", &p, b, Some(&m), None)
}

// ---------------------------------------------------------------- Basic Text / Path Text

fn text(e: &mut Enc, id: &str, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let cpu = cpu_buf(e, &b, false)?;
    let passes = effectcraft_effects::text_passes(id, ctx, &cpu)?;
    let (w, h) = (b.img.width, b.img.height);
    for pass in passes {
        let look = pass.look;
        let mut segs = Vec::new();
        for pl in &pass.polys {
            effectcraft_effects::poly_segs(pl, false, pass.r, 1.0, &mut segs);
        }
        let sw = look.stroke_w.max(0.0);
        let at = |add: f64| Variant { add, min: 0.05, ..Variant::plain(1.0) };
        let cov = raster_segs(e, w, h, segs.iter().map(|s| (s.a, s.b, s.r, s.v)), &[at(0.0), at(sw * 0.5), at(-sw * 0.5)])?;
        let mut p = Params::default();
        p.u[0] = [look.display, look.on_original as u32, (look.display != 0 && sw > 0.0) as u32, (pass.r - sw * 0.5 > 0.05) as u32];
        p.f[0] = look.fill;
        p.f[1] = look.stroke;
        p.f[2][0] = look.opacity;
        b = run(e, "fg2_text", &p, b, Some(&cov), None)?;
    }
    Some(b)
}

// ---------------------------------------------------------------- Radio Waves

fn radio_waves(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let cpu = cpu_buf(e, &b, false)?;
    let Some(plan) = effectcraft_effects::radio_plan(ctx, &cpu) else { return Some(b) };
    let n = plan.waves.len();
    let mut data = Vec::with_capacity(n * 13);
    let mut outlines = Vec::new();
    for wv in &plan.waves {
        let (off, cnt) = match &wv.outline {
            Some(o) => {
                let off = n * 13 + outlines.len();
                outlines.extend(o.iter().flat_map(|q| [q.0 as f32, q.1 as f32]));
                (off, o.len())
            }
            None => (0, 0),
        };
        data.extend([
            wv.centre.0 as f32,
            wv.centre.1 as f32,
            wv.radius as f32,
            wv.half_width as f32,
            wv.rotation as f32,
            wv.fade,
            wv.color[0],
            wv.color[1],
            wv.color[2],
            wv.color[3],
            wv.opacity,
            off as f32,
            cnt as f32,
        ]);
    }
    data.extend(outlines);
    if data.len() > MAX_FLOATS {
        return None;
    }
    let mut p = Params::default();
    p.u[0] = [n as u32, plan.contour.is_some() as u32, plan.profile, 0];
    p.f[0][0] = plan.aa as f32;
    let sdf = match &plan.contour {
        Some(c) => {
            let mut img = Image::new(c.sdf.w as u32, c.sdf.h as u32);
            for (px, &v) in img.data.iter_mut().zip(&c.sdf.data) {
                px[0] = v;
            }
            p.f[0][1] = c.anchor.0 as f32;
            p.f[0][2] = c.anchor.1 as f32;
            Some(e.g.upload_image(&img)?)
        }
        None => None,
    };
    let buf = e.data(&data);
    run(e, "fg2_radio", &p, b, sdf.as_ref(), Some(&buf))
}

// ---------------------------------------------------------------- Beam / Lens Flare

fn beam(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let s = b.to_px(pr.v2("startPoint"));
    let en = b.to_px(pr.v2("endPoint"));
    let len = (pr.f("length") / 100.0).clamp(0.0, 1.0);
    let time = (pr.f("time") / 100.0).clamp(0.0, 1.0);
    let t0 = time * (1.0 - len);
    let (dx, dy) = (en.0 - s.0, en.1 - s.1);
    let mut p = Params::default();
    p.u[0] = [pr.b("perspective3d") as u32, pr.b("compositeOnOriginal") as u32, (len > 0.0) as u32, 0];
    p.f[0] = [s.0 as f32, s.1 as f32, dx as f32, dy as f32];
    p.f[1] = [t0 as f32, (t0 + len) as f32, (pr.f("startThickness") * b.scale) as f32, (pr.f("endThickness") * b.scale) as f32];
    p.f[2] = [(pr.f("softness") / 100.0).clamp(0.0, 1.0) as f32, (dx * dx + dy * dy).max(1e-9) as f32, 0.0, 0.0];
    p.f[3] = rgb(pr.color("insideColor"));
    p.f[4] = rgb(pr.color("outsideColor"));
    run(e, "fg2_beam", &p, b, None, None)
}

fn lens_flare(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let f = b.to_px(pr.v2("flareCenter"));
    let lens = pr.e("lensType");
    let (w, h) = (ctx.layer_size[0] * b.scale, ctx.layer_size[1] * b.scale);
    let c = (b.offset[0] + w * 0.5, b.offset[1] + h * 0.5);
    let diag = (w * w + h * h).sqrt().max(1.0);
    let (core_k, rays, halo_r) = match lens {
        1 => (0.035, 6.0, 0.18),
        2 => (0.02, 12.0, 0.3),
        _ => (0.03, 8.0, 0.25),
    };
    let ghosts = effectcraft_effects::flare_ghosts(lens);
    let data: Vec<f32> = ghosts.iter().flat_map(|g| [g.t as f32, g.r as f32, g.c[0], g.c[1], g.c[2], g.k, g.ring as u32 as f32]).collect();
    let mut p = Params::default();
    p.u[0][0] = ghosts.len() as u32;
    p.f[0] = [f.0 as f32, f.1 as f32, c.0 as f32, c.1 as f32];
    p.f[1] = [diag as f32, core_k, rays, halo_r];
    p.f[2] = [(pr.f("flareBrightness") / 100.0) as f32, (pr.f("blendWithOriginal") / 100.0) as f32, 0.0, 0.0];
    let buf = e.data(&data);
    run(e, "fg2_flare", &p, b, None, Some(&buf))
}

// ---------------------------------------------------------------- CC Glue Gun / CC Threads

fn glue_gun(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let q = b.to_px(pr.v2("brushPosition"));
    let rad = (pr.f("strokeWidth") * b.scale * 0.5).max(0.5);
    let refl = pr.f("reflection").to_radians();
    let strength = (pr.f("strength") / 100.0) as f32;
    let beads = (pr.f("density").max(0.0).round() as usize).clamp(1, 32);
    let hash = |i: usize, k: u32| effectcraft_raster::hash_noise(i as u32, k, ctx.seed) as f64;
    // generate3::glue_gun's beads.
    let mut data = Vec::with_capacity(beads * 3);
    for i in 0..beads {
        let a = hash(i, 1) * 2.0 * std::f64::consts::PI;
        let d = if i == 0 { 0.0 } else { hash(i, 2) * rad * 0.35 };
        data.extend([(q.0 + a.cos() * d) as f32, (q.1 + a.sin() * d) as f32, (rad * (0.75 + 0.25 * hash(i, 3))) as f32]);
    }
    let mut p = Params::default();
    p.u[0] = [beads as u32, (pr.e("style") == 1) as u32, 0, 0];
    p.f[0] = [(refl.cos() * 0.6) as f32, (-refl.sin() * 0.6) as f32, 0.53, strength];
    p.f[1][0] = strength.clamp(0.0, 1.0).max(0.3);
    let buf = e.data(&data);
    run(e, "fg2_glue", &p, b, None, Some(&buf))
}

fn threads(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let sc = b.scale;
    let (sn, cs) = pr.f("direction").to_radians().sin_cos();
    let c = b.to_px(pr.v2("center"));
    let mut p = Params::default();
    p.u[0][0] = pr.f("overlaps").round().clamp(1.0, 20.0) as u32;
    p.f[0] = [c.0 as f32, c.1 as f32, sn as f32, cs as f32];
    p.f[1] = [
        (pr.f("width") * sc).max(1.0) as f32,
        (pr.f("height") * sc).max(1.0) as f32,
        (pr.f("coverage") / 100.0).clamp(0.0, 1.0) as f32,
        (pr.f("shadowing") / 100.0) as f32,
    ];
    p.f[2][0] = (pr.f("texture") / 100.0) as f32;
    run(e, "fg2_threads", &p, b, None, None)
}

// ---------------------------------------------------------------- Paint Bucket / Eyedropper Fill

fn paint_bucket(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let cpu = cpu_buf(e, &b, true)?;
    let region = effectcraft_effects::paint_bucket_region(ctx, &cpu);
    let (w, h) = (b.img.width, b.img.height);
    let mut img = Image::new(w, h);
    for (px, &v) in img.data.iter_mut().zip(&region.data) {
        px[0] = v;
    }
    let region = e.g.upload_image(&img)?;
    let cov_kernel = |e: &mut Enc, mode: u32, a: &GpuImage, x: &GpuImage| {
        let mut p = Params::default();
        p.u[0][0] = mode;
        let out = e.scratch(w, h);
        e.pixels("fg2_bucket_cov", &p, a, Some(x), &out, None);
        out
    };
    if pr.b("viewThreshold") {
        let img = cov_kernel(e, 1, &region, &region);
        return Some(GBuf { img, ..b });
    }
    let sc = b.scale;
    use crate::fx_color::{DILATE_X, ERODE_X, morph_frac};
    let cov = match pr.e("stroke") {
        1 => {
            let s = pr.f("featherSoftness") * sc * 0.5;
            if s > 0.05 { gauss_plane(e, &region, s, s)? } else { region }
        }
        2 => morph_frac(e, &region, pr.f("spreadRadius") * sc, DILATE_X),
        3 => morph_frac(e, &region, pr.f("spreadRadius") * sc, ERODE_X),
        4 => {
            let r = pr.f("strokeWidth") * sc;
            let out = morph_frac(e, &region, r * 0.5, DILATE_X);
            let inn = morph_frac(e, &region, r * 0.5, ERODE_X);
            cov_kernel(e, 2, &out, &inn)
        }
        _ => {
            let blurred = gauss_plane(e, &region, 0.5, 0.5)?;
            cov_kernel(e, 0, &region, &blurred)
        }
    };
    let mut p = Params::default();
    p.u[0][0] = pr.e("blendingMode");
    p.f[0] = pr.color("color");
    p.f[1][0] = (pr.f("opacity") / 100.0) as f32;
    run(e, "fg2_bucket", &p, b, Some(&cov), None)
}

fn eyedropper(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let cpu = cpu_buf(e, &b, true)?;
    let fill = effectcraft_effects::eyedropper_color(ctx, &cpu);
    let mut p = Params::default();
    p.u[0][0] = ctx.params.b("maintainOriginalAlpha") as u32;
    p.f[0] = fill;
    p.f[1][0] = (ctx.params.f("blendWithOriginal") / 100.0) as f32;
    run(e, "fg2_eyedropper", &p, b, None, None)
}
