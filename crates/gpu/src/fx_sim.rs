//! GPU effects, simulation render passes (kernels in `shaders/fx_sim.wgsl`, every entry point
//! prefixed `fxm_`): CC Rainfall, CC Snowfall, CC Star Burst, CC Bubbles, CC Drizzle, CC Hair,
//! CC Mr. Mercury, Caustics, Wave World, Foam, Shatter, Card Dance and Card Wipe.
//!
//! The simulation (or closed-form layout) of every frame stays on the CPU, shared with the CPU
//! effect: sprite plans (`effects::sprite_plan`: rain drops, snow flakes, star-burst discs, hair
//! strands, Foam's bubbles, Wave World's wireframe), piece plans (`effects::piece_plan`: Card
//! Dance's cards, Shatter's shards, Card Wipe's cards, sorted far to near), CC Bubbles' and CC
//! Mr. Mercury's lists, CC Drizzle's drops, Wave World's height grid and Caustics' prepared
//! layers. The raster / render pass runs here: items are binned into 16 × 16 tiles on the CPU
//! and `fxm_raster` composites each pixel's items in the CPU's order with the CPU's shading
//! (colours that depend on the layer's own pixels — refracting rain, star-burst and hair-root
//! colours, bubble refraction — are sampled per pixel from the layer texture). Piece plans read
//! the layer's pixels on the CPU (gradient maps and textures default to the layer), so those
//! effects read the frame back once. Shatter's wireframe views draw their lines as sprites
//! (`effects::pixel_plan`); Foam's User Defined texture, Environment Map and flow-map preview
//! are drawn by `fx_particles` over the bubble sprites (`effects::foam_full_plan`). Sprite
//! layers and back-less piece plans too large for one item table are drawn in several passes
//! (`fx_particles::Chunked`).

use effectcraft_effects::{Acc, EffectCtx, PieceTex, Post, Shape, SpritePlan, Tint};
use effectcraft_raster::Image;

use crate::context::{Enc, GpuImage, Params};
use crate::effects::{GBuf, gaussian_blur};

/// Compute entry points in `fx_sim.wgsl`.
pub(crate) const KERNELS: &[&str] = &["fxm_raster", "fxm_post", "fxm_mercury", "fxm_drizzle", "fxm_wave", "fxm_caustics"];

/// Effect ids implemented here.
pub(crate) const IDS: &[&str] = &[
    "ec.sim.ccrainfall",
    "ec.sim.ccsnowfall",
    "ec.sim.ccstarburst",
    "ec.sim.ccbubbles",
    "ec.sim.ccdrizzle",
    "ec.sim.cchair",
    "ec.sim.ccmrmercury",
    "ec.sim.caustics",
    "ec.sim.waveworld",
    "ec.sim.foam",
    "ec.sim.shatter",
    "ec.sim.carddance",
    "ec.transition.cardwipe",
];

/// Largest item table (floats) the raster kernel takes; bigger plans render on the CPU.
const MAX_FLOATS: usize = 1 << 24;
const TILE: i32 = 16;

/// Run effect `id` (one of [`IDS`]); `None` = this parameter combination runs on the CPU.
pub(crate) fn apply(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let geo = effectcraft_effects::Buf { img: Image::new(0, 0), offset: b.offset, scale: b.scale };
    match id {
        "ec.sim.ccbubbles" => bubbles(e, ctx, b, &geo),
        "ec.sim.ccdrizzle" => drizzle(e, ctx, b, &geo),
        "ec.sim.ccmrmercury" => mercury(e, ctx, b, &geo),
        "ec.sim.caustics" => caustics(e, ctx, b, &geo),
        "ec.sim.waveworld" => match effectcraft_effects::wave_world_plan(ctx, &geo)? {
            effectcraft_effects::WavePlan::Wire(plan) => sprites(e, b, &plan),
            effectcraft_effects::WavePlan::Height { st, depth } => wave_height(e, ctx, b, &st, depth.as_deref()),
        },
        "ec.sim.shatter" | "ec.sim.carddance" | "ec.transition.cardwipe" => pieces(e, id, ctx, b),
        "ec.sim.foam" => crate::fx_particles::foam(e, ctx, b),
        "ec.sim.cchair" => match effectcraft_effects::sprite_plan(id, ctx, &geo) {
            Some(plan) => sprites(e, b, &plan),
            None => Some(b),
        },
        _ => {
            let plan = effectcraft_effects::sprite_plan(id, ctx, &geo)?;
            sprites(e, b, &plan)
        }
    }
}

// ---------------------------------------------------------------- tiled item raster

/// Items with their integer pixel bounds, binned into tiles (sim::raster's bands, finer).
pub(crate) struct Items {
    stride: usize,
    recs: Vec<f32>,
    w: i32,
    h: i32,
    tiles: Vec<Vec<u32>>,
    pub(crate) tx: i32,
    /// Item indices in all tiles.
    n_idx: usize,
}

impl Items {
    pub(crate) fn new(stride: usize, w: u32, h: u32) -> Items {
        let (w, h) = (w as i32, h as i32);
        let tx = (w + TILE - 1) / TILE;
        let ty = (h + TILE - 1) / TILE;
        Items { stride, recs: vec![], w, h, tiles: vec![vec![]; (tx * ty).max(0) as usize], tx, n_idx: 0 }
    }

    /// The integer pixel bounds of float bounds `bb` (`None` = skipped, as sim::raster).
    fn bounds(&self, bb: Option<[f32; 4]>) -> Option<[i32; 4]> {
        let b = bb?;
        if !(b[0].is_finite() && b[1].is_finite() && b[2].is_finite() && b[3].is_finite()) {
            return None;
        }
        let x0 = b[0].floor().max(0.0) as i32;
        let y0 = b[1].floor().max(0.0) as i32;
        let x1 = (b[2].ceil() as i32).min(self.w - 1);
        let y1 = (b[3].ceil() as i32).min(self.h - 1);
        if x1 < x0 || y1 < y0 || b[2] < 0.0 || b[3] < 0.0 {
            return None;
        }
        Some([x0, y0, x1, y1])
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.recs.is_empty()
    }

    /// Whether adding an item covering `bb` keeps the kernel data within [`MAX_FLOATS`].
    pub(crate) fn fits(&self, bb: Option<[f32; 4]>) -> bool {
        let Some([x0, y0, x1, y1]) = self.bounds(bb) else { return true };
        let tiles = ((x1 / TILE - x0 / TILE + 1) * (y1 / TILE - y0 / TILE + 1)) as usize;
        self.tiles.len() + 1 + self.n_idx + tiles + self.recs.len() + self.stride <= MAX_FLOATS
    }

    /// Add an item (`rec` = its fields, without the bounds) covering float bounds `bb`
    /// (`None` or empty = skipped, as sim::raster).
    pub(crate) fn push(&mut self, bb: Option<[f32; 4]>, rec: &[f32]) {
        let Some([x0, y0, x1, y1]) = self.bounds(bb) else { return };
        let i = (self.recs.len() / self.stride) as u32;
        let start = self.recs.len();
        self.recs.extend_from_slice(rec);
        self.recs.resize(start + self.stride - 4, 0.0);
        self.recs.extend([x0 as f32, y0 as f32, x1 as f32, y1 as f32]);
        for ty in y0 / TILE..=y1 / TILE {
            for tx in x0 / TILE..=x1 / TILE {
                self.tiles[(ty * self.tx + tx) as usize].push(i);
                self.n_idx += 1;
            }
        }
    }

    /// Kernel data: tile offsets (tiles + 1), item indices, items. `None` when too large.
    pub(crate) fn data(self) -> Option<(Vec<f32>, u32, u32)> {
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
        Some((d, items_at, self.stride as u32))
    }
}

/// Rasterise items over a transparent frame. `kind`: 0 sprites, 1 CC Bubbles, 2 Mr. Mercury
/// field, 3 pieces; `src` / `aux` = the textures the items sample.
#[allow(clippy::too_many_arguments)]
fn raster(e: &mut Enc, items: Items, kind: u32, add: bool, src: &GpuImage, aux: Option<&GpuImage>, mut p: Params, w: u32, h: u32) -> Option<GpuImage> {
    let tx = items.tx as u32;
    let (data, items_at, stride) = items.data()?;
    let buf = e.data(&data);
    p.u[0] = [kind, add as u32, items_at, stride];
    p.u[1][0] = tx;
    let out = e.scratch(w, h);
    e.pixels("fxm_raster", &p, src, aux, &out, Some(&buf));
    Some(out)
}

/// sim::sprite_bbox.
fn sprite_bbox(x: f32, y: f32, r: f32, c3: f32, shape: Shape) -> Option<[f32; 4]> {
    if c3 <= 1e-5 || r <= 0.0 {
        return None;
    }
    let r = r.max(0.5) + 1.0;
    Some(match shape {
        Shape::Line { dx, dy } => [x.min(x + dx) - r, y.min(y + dy) - r, x.max(x + dx) + r, y.max(y + dy) + r],
        Shape::Star => [x - r * 2.0, y - r * 2.0, x + r * 2.0, y + r * 2.0],
        _ => [x - r, y - r, x + r, y + r],
    })
}

/// Draw a sprite plan and meet the layer `b` (sim::SpritePlan::finish).
pub(crate) fn sprites(e: &mut Enc, b: GBuf, plan: &SpritePlan) -> Option<GBuf> {
    let (w, h) = (b.img.width, b.img.height);
    let fx = sprite_layer(e, &b.img, plan)?;
    let mut p = Params::default();
    match plan.post {
        Post::Combine(m) => p.u[0] = [0, m, 0, 0],
        Post::Replace => return Some(GBuf { img: fx, ..b }),
        Post::Lerp(t) => {
            p.u[0][0] = 1;
            p.f[0][0] = t;
        }
        Post::OverBlack => p.u[0][0] = 2,
    }
    let out = e.scratch(w, h);
    e.pixels("fxm_post", &p, &b.img, Some(&fx), &out, None);
    Some(GBuf { img: out, ..b })
}

/// sim::splat of a sprite plan over a transparent frame of `src`'s size (tints sample `src`),
/// in as many passes as the item tables need.
pub(crate) fn sprite_layer(e: &mut Enc, src: &GpuImage, plan: &SpritePlan) -> Option<GpuImage> {
    let (w, h) = (src.width, src.height);
    // Without tints the record stops after the (zero) tint kind.
    let stride = if plan.tints.is_empty() { 16 } else { 24 };
    let mut ch = crate::fx_particles::Chunked::new(0, (plan.acc == Acc::Add) as u32, stride, w, h);
    for (i, s) in plan.sprites.iter().enumerate() {
        let (kind, dx, dy) = match s.shape {
            Shape::Soft => (0.0, 0.0, 0.0),
            Shape::Disc => (1.0, 0.0, 0.0),
            Shape::Sphere => (2.0, 0.0, 0.0),
            Shape::Faded => (3.0, 0.0, 0.0),
            Shape::Bubble => (4.0, 0.0, 0.0),
            Shape::Star => (5.0, 0.0, 0.0),
            Shape::Square => (6.0, 0.0, 0.0),
            Shape::Tri => (7.0, 0.0, 0.0),
            Shape::Line { dx, dy } => (8.0, dx, dy),
        };
        let mut rec = [kind, s.x, s.y, s.r, s.c[0], s.c[1], s.c[2], s.c[3], s.rot, dx, dy, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        match plan.tints.get(i).copied().unwrap_or(Tint::Fixed) {
            Tint::Fixed => {}
            Tint::Refract { x, y } => rec[11..14].copy_from_slice(&[1.0, x as f32, y as f32]),
            Tint::Source { x, y } => rec[11..14].copy_from_slice(&[2.0, x as f32, y as f32]),
            Tint::Root { x, y, hair, inherit, shade, spec } => {
                rec[11..20].copy_from_slice(&[3.0, x as f32, y as f32, hair[0], hair[1], hair[2], inherit, shade, spec]);
            }
        }
        ch.push(e, src, sprite_bbox(s.x, s.y, s.r, s.c[3], s.shape), &rec[..stride - 4]);
    }
    ch.finish(e, src)
}

// ---------------------------------------------------------------- CC Bubbles / Mr. Mercury

fn bubbles(e: &mut Enc, ctx: &EffectCtx, b: GBuf, geo: &effectcraft_effects::Buf) -> Option<GBuf> {
    let (w, h) = (b.img.width, b.img.height);
    let mut items = Items::new(12, w, h);
    for q in effectcraft_effects::bubble_list(ctx, geo) {
        items.push(Some([q.x - q.r - 1.0, q.y - q.r - 1.0, q.x + q.r + 1.0, q.y + q.r + 1.0]), &[9.0, q.x, q.y, q.r, q.sx, q.sy]);
    }
    let mut p = Params::default();
    p.u[1] = [0, (ctx.params.e("reflectionType") == 1) as u32, ctx.params.e("shadingType"), 0];
    p.f[0] = effectcraft_effects::bubble_sample_bounds(ctx, b.offset, b.scale);
    let img = raster(e, items, 1, false, &b.img, None, p, w, h)?;
    Some(GBuf { img, ..b })
}

fn mercury(e: &mut Enc, ctx: &EffectCtx, b: GBuf, geo: &effectcraft_effects::Buf) -> Option<GBuf> {
    let pr = ctx.params;
    let (w, h) = (b.img.width, b.img.height);
    let infl = 1.0 + pr.f("blobInfluence") as f32 / 100.0;
    let mut items = Items::new(8, w, h);
    for q in effectcraft_effects::mercury_blobs(ctx, geo) {
        let ex = q.r * 2.0 * infl;
        items.push(Some([q.x - ex, q.y - ex, q.x + ex, q.y + ex]), &[10.0, q.x, q.y, q.r]);
    }
    let mut p = Params::default();
    p.f[0][0] = infl;
    let field = raster(e, items, 2, true, &b.img, None, p, w, h)?;
    let ldir = (pr.f("light/lightDirection") as f32).to_radians();
    let lheight = pr.f("light/lightHeight") as f32 / 100.0;
    let l = norm3([ldir.sin(), -ldir.cos(), lheight.max(0.05)]);
    let lc = pr.color("light/lightColor");
    let mut p = Params::default();
    p.f[0] = [
        pr.f("shading/ambient") as f32 / 100.0,
        pr.f("shading/diffuse") as f32 / 100.0,
        pr.f("shading/specular") as f32 / 100.0,
        pr.f("shading/roughness").max(0.001) as f32,
    ];
    p.f[1] = [pr.f("shading/metal") as f32 / 100.0, pr.f("light/lightIntensity") as f32 / 100.0, b.scale as f32, 0.0];
    p.f[2] = [l[0], l[1], l[2], 0.0];
    p.f[3] = [lc[0], lc[1], lc[2], 0.0];
    let out = e.scratch(w, h);
    e.pixels("fxm_mercury", &p, &b.img, Some(&field), &out, None);
    Some(GBuf { img: out, ..b })
}

/// sim::norm3.
fn norm3(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-9 { [v[0] / l, v[1] / l, v[2] / l] } else { [0.0, -1.0, 0.0] }
}

// ---------------------------------------------------------------- CC Drizzle

fn drizzle(e: &mut Enc, ctx: &EffectCtx, b: GBuf, geo: &effectcraft_effects::Buf) -> Option<GBuf> {
    let pr = ctx.params;
    let Some(drops) = effectcraft_effects::drizzle_drops(ctx, geo) else { return Some(b) };
    let rippling = (pr.f("rippling") / 360.0).max(0.05) as f32;
    let lh = ctx.layer_size[1] as f32;
    let s = b.scale as f32;
    let wavelen = (lh * 0.04 * s).max(2.0) / rippling.clamp(0.25, 4.0);
    let width = wavelen * (1.0 + rippling * 2.0);
    let ldir = (pr.f("light/lightDirection") as f32).to_radians();
    let lheight = pr.f("light/lightHeight") as f32 / 100.0;
    let (ls, lcs) = ldir.sin_cos();
    let l = {
        let v = [lcs, ls, lheight.max(0.05)];
        let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / n, v[1] / n, v[2] / n]
    };
    let hz = (l[2] + 1.0) * 0.5;
    let hv = {
        let v = [l[0] * 0.5, l[1] * 0.5, hz];
        let m = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / m, v[1] / m, v[2] / m]
    };
    let lc = pr.color("light/lightColor");
    let d: Vec<f32> = drops.iter().flatten().copied().collect();
    let buf = e.data(&d);
    let mut p = Params::default();
    p.u[0][0] = drops.len() as u32;
    p.f[0] = [wavelen, width, pr.f("displacement") as f32 * s * 4.0, pr.f("shading/roughness").max(0.001) as f32];
    p.f[1] = [l[0], l[1], l[2], pr.f("light/lightIntensity") as f32 / 100.0];
    p.f[2] = [hv[0], hv[1], hv[2], pr.f("shading/specular") as f32 / 100.0];
    p.f[3] = [lc[0], lc[1], lc[2], 0.0];
    p.f[4] = [pr.f("shading/ambient") as f32 / 100.0, pr.f("shading/diffuse") as f32 / 100.0, 0.0, 0.0];
    let out = e.scratch(b.img.width, b.img.height);
    e.pixels("fxm_drizzle", &p, &b.img, None, &out, Some(&buf));
    Some(GBuf { img: out, ..b })
}

// ---------------------------------------------------------------- Wave World (Height Map)

fn wave_height(e: &mut Enc, ctx: &EffectCtx, b: GBuf, st: &effectcraft_effects::Waves, depth: Option<&[f32]>) -> Option<GBuf> {
    let pr = ctx.params;
    let mut d = st.u.clone();
    if let Some(dp) = depth {
        d.extend_from_slice(dp);
    }
    let buf = e.data(&d);
    let mut p = Params::default();
    let dry_transparent = pr.e("heightMapControls/renderDryAreasAs") == 1 && depth.is_some();
    p.u[0] = [st.nx as u32, st.ny as u32, dry_transparent as u32, 0];
    p.f[0] = [
        pr.f("heightMapControls/brightness") as f32,
        pr.f("heightMapControls/contrast") as f32,
        pr.f("heightMapControls/gamma").max(0.01) as f32,
        1.0 - pr.f("heightMapControls/transparency") as f32,
    ];
    p.f[1] = [b.offset[0] as f32, b.offset[1] as f32, b.scale as f32, ctx.layer_size[0].max(1.0) as f32];
    let out = e.scratch(b.img.width, b.img.height);
    e.pixels("fxm_wave", &p, &b.img, None, &out, Some(&buf));
    Some(GBuf { img: out, ..b })
}

// ---------------------------------------------------------------- pieces

fn pieces(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    // Planning reads the layer (gradient maps and textures default to it).
    let img = e.download(&b.img)?;
    let cpu = effectcraft_effects::Buf { img, offset: b.offset, scale: b.scale };
    if id == "ec.sim.shatter" {
        // The rendered pieces or the wireframe views' lines.
        return match effectcraft_effects::pixel_plan(id, ctx, &cpu)? {
            effectcraft_effects::PixelPlan::Pieces(plan) => piece_pass(e, b, &plan),
            effectcraft_effects::PixelPlan::Sprites(plan) => sprites(e, b, &plan),
        };
    }
    let Some(plan) = effectcraft_effects::piece_plan(id, ctx, &cpu) else {
        // Card Wipe before anything moves: the layer as is.
        return (id == "ec.transition.cardwipe").then_some(b);
    };
    piece_pass(e, b, &plan)
}

/// Draw a piece plan over a transparent frame of `b`'s size (sim2::PiecePlan::finish).
pub(crate) fn piece_pass(e: &mut Enc, b: GBuf, plan: &effectcraft_effects::PiecePlan) -> Option<GBuf> {
    let (w, h) = (b.img.width, b.img.height);
    let tex = |t: &PieceTex| -> Option<GpuImage> {
        match t {
            PieceTex::Layer => Some(b.img.clone()),
            PieceTex::Image(i) => e.g.upload_image(i),
        }
    };
    let front = tex(&plan.front)?;
    let back = match &plan.back {
        Some(t) => Some(tex(t)?),
        None => None,
    };
    let mut items = Items::new(48, w, h);
    // Without a back texture the pieces can be drawn in passes over a base (CC Pixel Polly's
    // fine grids).
    let mut ch = back.is_none().then(|| crate::fx_particles::Chunked::new(4, 0, 48, w, h));
    for q in &plan.pieces {
        let m = q.inv;
        let mut rec = vec![11.0];
        for r in m {
            rec.extend(r.map(|v| v as f32));
        }
        rec.extend(q.c0);
        for v in q.poly {
            rec.extend(v);
        }
        rec.extend([q.n as f32, q.shade]);
        rec.extend(q.tint);
        rec.extend(q.spec);
        rec.push(q.alpha);
        match q.flat {
            Some(c) => rec.extend([1.0, c[0], c[1], c[2]]),
            None => rec.extend([0.0; 4]),
        }
        rec.extend([q.back as u32 as f32, q.back_mirror as f32]);
        match &mut ch {
            Some(ch) => ch.push(e, &front, Some(q.bbox), &rec),
            None => items.push(Some(q.bbox), &rec),
        }
    }
    if let Some(ch) = ch {
        return Some(GBuf { img: ch.finish(e, &front)?, ..b });
    }
    let mut p = Params::default();
    p.u[1][1] = back.is_some() as u32;
    let img = raster(e, items, 3, false, &front, back.as_ref(), p, w, h)?;
    Some(GBuf { img, ..b })
}

// ---------------------------------------------------------------- Caustics

fn caustics(e: &mut Enc, ctx: &EffectCtx, b: GBuf, geo: &effectcraft_effects::Buf) -> Option<GBuf> {
    let st = effectcraft_effects::caustics_setup(ctx, geo);
    let (w, h) = (b.img.width, b.img.height);
    let mut bottom = match &st.bottom {
        Some(i) => e.g.upload_image(i)?,
        None => b.img.clone(),
    };
    if st.bottom_sigma > 0.0 {
        bottom = gaussian_blur(e, &bottom, st.bottom_sigma, st.bottom_sigma, false);
    }
    let height = match &st.height {
        Some(pl) => {
            let mut img = Image::new(pl.w as u32, pl.h as u32);
            for (px, v) in img.data.iter_mut().zip(&pl.data) {
                *px = [*v, 0.0, 0.0, 0.0];
            }
            Some(e.g.upload_image(&img)?)
        }
        None => None,
    };
    let sky = match &st.sky {
        Some(i) => {
            let g = e.g.upload_image(i)?;
            Some((e.image_rows(&g), g.width, g.height))
        }
        None => None,
    };
    let distant = {
        let lw = st.lw.max(1.0);
        let v = [(st.lx - st.cx) / lw, (st.ly - st.cy) / lw, st.lheight];
        let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
        [(v[0] / n) as f32, (v[1] / n) as f32, (v[2] / n) as f32]
    };
    let mut p = Params::default();
    p.u[0] = [height.is_some() as u32, st.repeat, st.point as u32, st.sky_repeat];
    p.u[1] = [sky.is_some() as u32, sky.as_ref().map_or(0, |s| s.0.1), sky.as_ref().map_or(0, |s| s.1), sky.as_ref().map_or(0, |s| s.2)];
    p.f[0] = [st.scaling as f32, st.wave_h as f32, st.k as f32, st.lw as f32];
    p.f[1] = [st.cx as f32, st.cy as f32, st.lx as f32, st.ly as f32];
    p.f[2] = [st.surf[0], st.surf[1], st.surf[2], st.surf_op];
    p.f[3] = [st.lc[0], st.lc[1], st.lc[2], st.li];
    p.f[4] = [st.lheight as f32, st.cstr, st.ambient, st.diffuse];
    p.f[5] = [st.specular, st.sharp, st.sky_scaling as f32, st.sky_int];
    p.f[6] = [distant[0], distant[1], distant[2], st.convergence as f32];
    let out = e.scratch(w, h);
    e.pixels("fxm_caustics", &p, &bottom, height.as_ref(), &out, sky.as_ref().map(|s| &s.0.0));
    Some(GBuf { img: out, ..b })
}
