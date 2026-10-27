use super::*;

// ------------------------------------------------------------------ Caustics

/// Sample with repeat mode: 0 Once (transparent outside), 1 Tiled, 2 Reflected.
fn sample_repeat(img: &Image, x: f64, y: f64, mode: u32) -> Px {
    let (w, hh) = (img.width as f64, img.height as f64);
    match mode {
        1 => img.sample_bilinear_clamped(x.rem_euclid(w), y.rem_euclid(hh)),
        2 => {
            let m = |v: f64, n: f64| {
                let r = v.rem_euclid(2.0 * n);
                if r > n { 2.0 * n - r } else { r }
            };
            img.sample_bilinear_clamped(m(x, w), m(y, hh))
        }
        _ => img.sample_bilinear(x, y),
    }
}

/// Everything Caustics' per-pixel pass needs, prepared on the CPU (the other layers it reads
/// and the parameter conversions); the layer's own pixels are not read.
pub struct CausticsSetup {
    /// The Bottom layer fitted to the buffer, `None` = the layer itself.
    pub bottom: Option<Image>,
    /// Gaussian σ (buffer px) blurring the bottom (transparent edges), 0 = none.
    pub bottom_sigma: f64,
    /// The water surface's height field (smoothed luminance), `None` = flat.
    pub height: Option<Plane>,
    /// The Sky layer fitted to the buffer.
    pub sky: Option<Image>,
    pub scaling: f64,
    pub repeat: u32,
    pub wave_h: f64,
    /// Displacement per unit of height gradient.
    pub k: f64,
    /// Layer centre (buffer px).
    pub cx: f64,
    pub cy: f64,
    /// Layer width (buffer px).
    pub lw: f64,
    pub surf: [f32; 4],
    pub surf_op: f32,
    pub cstr: f32,
    pub lc: [f32; 4],
    pub li: f32,
    /// Light position (buffer px) and height (layer widths); `point` = from the position to
    /// each pixel, else one direction from above the layer centre.
    pub lx: f64,
    pub ly: f64,
    pub lheight: f64,
    pub point: bool,
    pub ambient: f32,
    pub diffuse: f32,
    pub specular: f32,
    pub sharp: f32,
    pub sky_scaling: f64,
    pub sky_repeat: u32,
    pub sky_int: f32,
    pub convergence: f64,
}

/// Caustics' setup for layer geometry `b` (see [`CausticsSetup`]).
pub fn caustics_setup(ctx: &EffectCtx, b: &Buf) -> CausticsSetup {
    let pr = ctx.params;
    let s = b.scale;
    let bottom = ctx.layer_param("bottom/bottom", true).map(|o| crate::util::fit_layer(ctx, b, &o, pr.e("bottom/bottomSizeDiffers") == 1));
    let blur = pr.f("bottom/blur") * s;
    let wave_h = pr.f("water/waveHeight");
    let smoothing = pr.f("water/smoothing") * s;
    let depth = pr.f("water/waterDepth");
    let ior = pr.f("water/refractiveIndex").max(1.0);
    let lpos = pr.v2("lighting/lightPosition");
    // Height field of the water surface (luminance of the chosen layer; none = flat).
    let height = ctx.layer_param("water/waterSurface", true).map(|o| {
        let img = crate::util::fit_layer(ctx, b, &o, true);
        let pl = Plane::luma(&img);
        if smoothing > 0.05 { gauss_plane(&pl, smoothing * 0.5, smoothing * 0.5) } else { pl }
    });
    let lw = ctx.layer_size[0] * s;
    let k = wave_h * depth * (1.0 - 1.0 / ior) * lw * 2.0;
    let (cx, cy) = (b.offset[0] + ctx.layer_size[0] * 0.5 * s, b.offset[1] + ctx.layer_size[1] * 0.5 * s);
    let (lx, ly) = b.to_px(lpos);
    let (lc, li, lheight) = (pr.color("lighting/lightColor"), pr.f("lighting/lightIntensity") as f32, pr.f("lighting/lightHeight").max(0.01));
    // Light Type: Distant Source (one direction, from the light position above the layer
    // centre), Point Source (from the light position to each pixel) or First Comp Light.
    let light_type = pr.e("lighting/lightType");
    let comp_light = if light_type == 2 { ctx.env.host.and_then(|h| h.comp_scene()).and_then(|sc| sc.light) } else { None };
    let (lc, li, lx, ly, lheight, point) = match (light_type, comp_light) {
        (2, Some(l)) => {
            let c = [l.color[0], l.color[1], l.color[2], 1.0];
            // Comp space: z away from the viewer; the light above the water is at −z.
            let (px, py) = (l.pos[0] * s + b.offset[0], l.pos[1] * s + b.offset[1]);
            (c, li, px, py, (-l.pos[2] / ctx.layer_size[0].max(1.0)).max(0.01), l.kind == 1)
        }
        (2, None) => (lc, 0.0, lx, ly, lheight, false),
        (1, _) => (lc, li, lx, ly, lheight, true),
        _ => (lc, li, lx, ly, lheight, false),
    };
    // Sky: the layer the water surface reflects (Surface Opacity 1 = a mirror of it).
    let sky = ctx.layer_param("sky/sky", true).map(|o| crate::util::fit_layer(ctx, b, &o, pr.e("sky/skySizeDiffers") == 1));
    CausticsSetup {
        bottom,
        bottom_sigma: if blur > 0.05 { blur * 0.5 } else { 0.0 },
        height,
        sky,
        scaling: pr.f("bottom/scaling").max(0.01),
        repeat: pr.e("bottom/repeatMode"),
        wave_h,
        k,
        cx,
        cy,
        lw,
        surf: pr.color("water/surfaceColor"),
        surf_op: pr.f("water/surfaceOpacity") as f32,
        cstr: pr.f("water/causticsStrength") as f32,
        lc,
        li,
        lx,
        ly,
        lheight,
        point,
        ambient: pr.f("lighting/ambientLight") as f32,
        diffuse: pr.f("material/diffuse") as f32,
        specular: pr.f("material/specular") as f32,
        sharp: pr.f("material/highlightSharpness").max(1.0) as f32,
        sky_scaling: pr.get("sky/scaling").map(Value::as_f64).unwrap_or(1.0).max(0.01),
        sky_repeat: pr.e("sky/repeatMode"),
        sky_int: pr.get("sky/intensity").map(Value::as_f64).unwrap_or(0.3) as f32,
        convergence: pr.get("sky/convergence").map(Value::as_f64).unwrap_or(0.5).clamp(0.0, 1.0),
    }
}

pub(super) fn caustics(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let st = caustics_setup(ctx, &b);
    let mut bottom = st.bottom.clone().unwrap_or_else(|| b.img.clone());
    if st.bottom_sigma > 0.0 {
        bottom = effectcraft_raster::gaussian_blur(&bottom, st.bottom_sigma, st.bottom_sigma, false);
    }
    let CausticsSetup {
        scaling,
        repeat,
        wave_h,
        k,
        cx,
        cy,
        lw,
        surf,
        surf_op,
        cstr,
        lc,
        li,
        lx,
        ly,
        lheight,
        point,
        ambient,
        diffuse,
        specular,
        sharp,
        sky_scaling,
        sky_repeat,
        sky_int,
        convergence,
        ..
    } = st;
    let (height, sky) = (&st.height, &st.sky);
    let (w, hh) = (b.img.width as usize, b.img.height as usize);
    let norm3 = |v: [f64; 3]| {
        let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
        [(v[0] / n) as f32, (v[1] / n) as f32, (v[2] / n) as f32]
    };
    let lv_at = |x: f64, y: f64| {
        if point {
            norm3([(lx - x) / lw.max(1.0), (ly - y) / lw.max(1.0), lheight])
        } else {
            norm3([(lx - cx) / lw.max(1.0), (ly - cy) / lw.max(1.0), lheight])
        }
    };
    let grad = |x: usize, y: usize| -> (f64, f64) {
        match height {
            Some(pl) => {
                let gx = (pl.get_clamped(x as i64 + 1, y as i64) - pl.get_clamped(x as i64 - 1, y as i64)) as f64 * 0.5;
                let gy = (pl.get_clamped(x as i64, y as i64 + 1) - pl.get_clamped(x as i64, y as i64 - 1)) as f64 * 0.5;
                (gx, gy)
            }
            None => (0.0, 0.0),
        }
    };
    // Displacement and its Jacobian (caustic focusing) per pixel.
    let disp: Vec<(f64, f64)> = (0..w * hh)
        .into_par_iter()
        .map(|i| {
            let (gx, gy) = grad(i % w, i / w);
            (-gx * k, -gy * k)
        })
        .collect();
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let (dx, dy) = disp[i];
            let d = |xx: usize, yy: usize| disp[yy.min(hh - 1) * w + xx.min(w - 1)];
            let (dxr, _) = d(x + 1, y);
            let (_, dyd) = d(x, y + 1);
            let (dxl, _) = d(x.saturating_sub(1), y);
            let (_, dyu) = d(x, y.saturating_sub(1));
            let jac = (1.0 + (dxr - dxl) * 0.5) * (1.0 + (dyd - dyu) * 0.5);
            let focus = ((1.0 / jac.abs().max(0.2)) as f32 - 1.0).clamp(-1.0, 4.0);
            let sx = cx + (x as f64 + 0.5 + dx - cx) / scaling;
            let sy = cy + (y as f64 + 0.5 + dy - cy) / scaling;
            let p = sample_repeat(&bottom, sx, sy, repeat);
            let (c, a) = unpremul(p);
            let (gx, gy) = grad(x, y);
            let n = {
                let v = [-gx * wave_h * 20.0, -gy * wave_h * 20.0, 1.0];
                let m = (v[0] * v[0] + v[1] * v[1] + 1.0).sqrt();
                [(v[0] / m) as f32, (v[1] / m) as f32, (v[2] / m) as f32]
            };
            let lv = lv_at(x as f64 + 0.5, y as f64 + 0.5);
            let nl = (n[0] * lv[0] + n[1] * lv[1] + n[2] * lv[2]).max(0.0);
            let hv = {
                let v = [lv[0], lv[1], lv[2] + 1.0];
                let m = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                [v[0] / m, v[1] / m, v[2] / m]
            };
            let spec = (n[0] * hv[0] + n[1] * hv[1] + n[2] * hv[2]).max(0.0).powf(sharp) * specular * li;
            let light = ambient + diffuse * li * nl + cstr * focus * li;
            // The surface: its colour lit like the water, or the sky it reflects (sampled along
            // the surface normal; Convergence pulls the reflection towards the pixel itself).
            let surface: [f32; 3] = match &sky {
                Some(sk) => {
                    let reach = (1.0 - convergence) * lw * 0.5;
                    let (rx, ry) = (x as f64 + 0.5 + n[0] as f64 * reach, y as f64 + 0.5 + n[1] as f64 * reach);
                    let (rx, ry) = (cx + (rx - cx) / sky_scaling, cy + (ry - cy) / sky_scaling);
                    let (sc, _) = unpremul(sample_repeat(sk, rx, ry, sky_repeat));
                    std::array::from_fn(|q| sc[q] * sky_int * 3.0 * surf[q])
                }
                None => std::array::from_fn(|q| surf[q] * (ambient + diffuse * li * nl)),
            };
            let cc: [f32; 3] = std::array::from_fn(|q| {
                let base = c[q] * light * lc[q];
                base * (1.0 - surf_op) + surface[q] * surf_op + spec * lc[q]
            });
            let a = (a + surf_op * (1.0 - a)).min(1.0);
            *px = [cc[0] * a, cc[1] * a, cc[2] * a, a];
        }
    });
    b
}

// ------------------------------------------------------------------ Wave World

/// Wave World's simulated surface: an `nx` × `ny` grid of heights `u` and velocities `v`.
#[derive(Clone, Debug)]
pub struct Waves {
    pub nx: usize,
    pub ny: usize,
    pub u: Vec<f32>,
    pub v: Vec<f32>,
}

struct Producer {
    ring: bool,
    pos: [f32; 2],
    len: f32,
    width: f32,
    angle: f32,
    amp: f32,
    freq: f32,
    phase: f32,
}

impl Producer {
    /// Weight of a grid point (layer-fraction coordinates, x in widths, y in widths).
    fn weight(&self, x: f32, y: f32) -> f32 {
        let (dx, dy) = (x - self.pos[0], y - self.pos[1]);
        let (sa, ca) = self.angle.sin_cos();
        let (u, v) = (dx * ca + dy * sa, -dx * sa + dy * ca);
        let d = if self.ring {
            let (a, bb) = ((self.len * 0.5).max(1e-3), (self.width * 0.5).max(1e-3));
            ((u / a).powi(2) + (v / bb).powi(2)).sqrt()
        } else {
            let l = (self.len * 0.5).max(1e-3);
            let wd = (self.width * 0.5).max(1e-3);
            (u.abs() / l).max(v.abs() / wd)
        };
        (1.0 - d).clamp(0.0, 1.0).min(1.0)
    }
}

static WAVE_CACHE: SimCache<Waves> = SimCache::new(4);

/// What Wave World draws: the Height Map view of the simulated surface (grid heights and,
/// with a ground layer, water depths per grid point), or the wireframe preview's lines.
pub enum WavePlan {
    Height { st: std::sync::Arc<Waves>, depth: Option<Vec<f32>> },
    Wire(crate::sim::SpritePlan),
}

pub(super) fn wave_world(ctx: &EffectCtx, b: Buf) -> Buf {
    match wave_world_impl(ctx, &b, false) {
        Ok(img) => Buf { img, ..b },
        Err(WavePlan::Wire(plan)) => plan.finish(b),
        Err(WavePlan::Height { .. }) => b,
    }
}

/// Wave World's simulation state for the frame, as a [`WavePlan`] (its pixels are not read).
pub fn wave_world_plan(ctx: &EffectCtx, b: &Buf) -> Option<WavePlan> {
    wave_world_impl(ctx, b, true).err()
}

fn wave_world_impl(ctx: &EffectCtx, b: &Buf, plan_only: bool) -> Result<Image, WavePlan> {
    let pr = ctx.params;
    let [lw, lh] = ctx.layer_size;
    let res = pr.f("simulation/gridResolution").clamp(1.0, 400.0) as usize;
    let res = if pr.b("simulation/gridResDownsamples") { ((res as f64) * b.scale).round().max(1.0) as usize } else { res };
    let nx = res.max(2) + 1;
    let ny = ((res as f64 * lh / lw.max(1.0)).round() as usize).max(2) + 1;
    let speed = pr.f("simulation/waveSpeed").max(0.0) as f32;
    let damping = pr.f("simulation/damping").max(0.0) as f32;
    let reflect = pr.e("simulation/reflectEdges");
    let preroll = pr.f("simulation/preRoll").max(0.0);
    let prods: Vec<Producer> = [1, 2]
        .iter()
        .map(|k| {
            let pos = pr.v2(&format!("producer{k}/producer{k}Position"));
            Producer {
                ring: pr.e(&format!("producer{k}/producer{k}Type")) == 0,
                pos: [(pos[0] / lw.max(1.0)) as f32, (pos[1] / lw.max(1.0)) as f32],
                len: pr.f(&format!("producer{k}/producer{k}Length")) as f32,
                width: pr.f(&format!("producer{k}/producer{k}Width")) as f32,
                angle: (pr.f(&format!("producer{k}/producer{k}Angle")) as f32).to_radians(),
                amp: pr.f(&format!("producer{k}/producer{k}Amplitude")) as f32,
                freq: pr.f(&format!("producer{k}/producer{k}Frequency")) as f32,
                phase: (pr.f(&format!("producer{k}/producer{k}Phase")) as f32).to_radians(),
            }
        })
        .collect();
    let sps = crate::sim::SPS as f32;
    // Wave speed in layer widths per second → cells per second; sub-steps keep it stable.
    let cps = speed * (nx - 1) as f32;
    let sub = ((cps * 2.0 / sps).ceil() as usize).clamp(1, 64);
    let dt = 1.0 / (sps * sub as f32);
    let cell = 1.0 / (nx - 1) as f32;
    let weights: Vec<Vec<f32>> = prods.iter().map(|p| (0..nx * ny).map(|i| p.weight((i % nx) as f32 * cell, (i / nx) as f32 * cell)).collect()).collect();
    // Ground: the ground layer's first frame, its luminance an elevation (× Steepness). Where
    // it rises above Height the grid is dry (a wall the waves reflect from); shallow water
    // slows the waves by Wave Strength.
    let ground = pr.get("ground/ground").and_then(Value::as_layer).and_then(|id| {
        let host = ctx.env.host?;
        host.layer_at(id, ctx.env.comp_time - ctx.time, true)
    });
    let steep = pr.get("ground/steepness").map(Value::as_f64).unwrap_or(0.25) as f32;
    let level = pr.get("ground/height").map(Value::as_f64).unwrap_or(0.25) as f32;
    let wave_strength = pr.f("ground/waveStrength").clamp(0.0, 1.0) as f32;
    // Water depth per grid point (None without ground: deep everywhere).
    let depth: Option<Vec<f32>> = ground.as_ref().map(|lp| {
        let (gw, gh) = (lp.size[0].max(1.0), lp.size[1].max(1.0));
        (0..nx * ny)
            .map(|i| {
                let (fx, fy) = ((i % nx) as f64 / (nx - 1) as f64, (i / nx) as f64 / (ny - 1) as f64);
                let (c, a) = unpremul(lp.buf.img.sample_bilinear_clamped(fx * gw * lp.buf.scale + lp.buf.offset[0], fy * gh * lp.buf.scale + lp.buf.offset[1]));
                level - steep * effectcraft_color::luminance(c[0], c[1], c[2]) * a
            })
            .collect()
    });
    let speed_k: Vec<f32> = match &depth {
        Some(d) => d.iter().map(|&v| if v <= 0.0 { 0.0 } else { 1.0 - wave_strength * (1.0 - (v / level.max(1e-3)).clamp(0.0, 1.0).sqrt()) }).collect(),
        None => vec![1.0; nx * ny],
    };
    let mut key =
        params_key(ctx, &Buf { img: Image::new(0, 0), offset: [0.0; 2], scale: if pr.b("simulation/gridResDownsamples") { b.scale } else { 1.0 } }, 4);
    if let Some(d) = &depth {
        key ^= d.iter().fold(0u64, |a, v| a.rotate_left(3) ^ v.to_bits() as u64);
    }
    let steps = crate::sim::steps_at(ctx.time + preroll);
    let st = WAVE_CACHE.run(
        key,
        steps,
        || Waves { nx, ny, u: vec![0.0; nx * ny], v: vec![0.0; nx * ny] },
        |w, step| {
            for k in 0..sub {
                let t = (step as f32 + k as f32 / sub as f32) / sps - preroll as f32;
                let c2 = cps * cps;
                let mut acc = vec![0.0f32; nx * ny];
                for y in 0..ny {
                    for x in 0..nx {
                        let i = y * nx + x;
                        let g = |xx: usize, yy: usize| w.u[yy * nx + xx];
                        let l = g(x.saturating_sub(1), y) + g((x + 1).min(nx - 1), y) + g(x, y.saturating_sub(1)) + g(x, (y + 1).min(ny - 1)) - 4.0 * w.u[i];
                        acc[i] = c2 * speed_k[i] * speed_k[i] * l - damping * 4.0 * w.v[i];
                    }
                }
                for i in 0..nx * ny {
                    w.v[i] += acc[i] * dt;
                    w.u[i] += w.v[i] * dt;
                }
                // Producers drive the surface.
                for (p, wt) in prods.iter().zip(&weights) {
                    if p.amp == 0.0 {
                        continue;
                    }
                    let target = p.amp * (std::f32::consts::TAU * p.freq * t + p.phase).sin();
                    for i in 0..nx * ny {
                        if wt[i] > 0.0 {
                            w.u[i] += (target - w.u[i]) * wt[i];
                            w.v[i] *= 1.0 - wt[i];
                        }
                    }
                }
                // Dry ground holds the surface flat.
                for i in 0..nx * ny {
                    if speed_k[i] <= 0.0 {
                        w.u[i] = 0.0;
                        w.v[i] = 0.0;
                    }
                }
                // Edges: reflective ones mirror; the others absorb in a thin sponge layer.
                let refl = |side: u32| reflect == 5 || reflect == side;
                let sponge = 4usize.min(nx / 4).max(1);
                for y in 0..ny {
                    for x in 0..nx {
                        let i = y * nx + x;
                        let mut f = 1.0f32;
                        if !refl(1) && x < sponge {
                            f = f.min(x as f32 / sponge as f32);
                        }
                        if !refl(3) && nx - 1 - x < sponge {
                            f = f.min((nx - 1 - x) as f32 / sponge as f32);
                        }
                        if !refl(2) && y < sponge {
                            f = f.min(y as f32 / sponge as f32);
                        }
                        if !refl(4) && ny - 1 - y < sponge {
                            f = f.min((ny - 1 - y) as f32 / sponge as f32);
                        }
                        if f < 1.0 {
                            let k = 0.85 + 0.15 * f;
                            w.u[i] *= k;
                            w.v[i] *= k;
                        }
                    }
                }
            }
        },
    );
    let s = b.scale;
    let (bw, bh) = (b.img.width, b.img.height);
    let sample = |x: f64, y: f64| -> f32 {
        let gx = (x / lw.max(1.0) * (st.nx - 1) as f64).clamp(0.0, (st.nx - 1) as f64);
        let gy = (y / lw.max(1.0) * (st.nx - 1) as f64).clamp(0.0, (st.ny - 1) as f64);
        let (x0, y0) = (gx.floor() as usize, gy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(st.nx - 1), (y0 + 1).min(st.ny - 1));
        let (tx, ty) = ((gx - x0 as f64) as f32, (gy - y0 as f64) as f32);
        let g = |xx: usize, yy: usize| st.u[yy * st.nx + xx];
        let a = g(x0, y0) + (g(x1, y0) - g(x0, y0)) * tx;
        let c = g(x0, y1) + (g(x1, y1) - g(x0, y1)) * tx;
        a + (c - a) * ty
    };
    if plan_only && pr.e("view") == 1 {
        return Err(WavePlan::Height { st, depth });
    }
    if pr.e("view") == 1 {
        let mut img = Image::new(bw, bh);
        let bright = pr.f("heightMapControls/brightness") as f32;
        let contrast = pr.f("heightMapControls/contrast") as f32;
        let gamma = pr.f("heightMapControls/gamma").max(0.01) as f32;
        let alpha = 1.0 - pr.f("heightMapControls/transparency") as f32;
        let dry_transparent = pr.e("heightMapControls/renderDryAreasAs") == 1;
        let dry_at = |x: f64, y: f64| -> bool {
            let Some(d) = &depth else { return false };
            let gx = (x / lw.max(1.0) * (st.nx - 1) as f64).round().clamp(0.0, (st.nx - 1) as f64) as usize;
            let gy = (y / lw.max(1.0) * (st.nx - 1) as f64).round().clamp(0.0, (st.ny - 1) as f64) as usize;
            d[gy * st.nx + gx] <= 0.0
        };
        img.rows_mut().for_each(|(y, row)| {
            for (x, px) in row.iter_mut().enumerate() {
                let lx = (x as f64 + 0.5 - b.offset[0]) / s;
                let ly = (y as f64 + 0.5 - b.offset[1]) / s;
                let v = (bright + contrast * sample(lx, ly)).clamp(0.0, 1.0).powf(1.0 / gamma);
                let al = if dry_transparent && dry_at(lx, ly) { 0.0 } else { alpha };
                *px = [v * al, v * al, v * al, al];
            }
        });
        Ok(img)
    } else {
        // Wireframe preview: the grid turned by Horizontal Rotation about the vertical axis,
        // tilted by Vertical Rotation, heights scaled by Vertical Scale.
        let hr = pr.f("wireframeControls/horizontalRotation").to_radians();
        let vr = pr.get("wireframeControls/verticalRotation").map(Value::as_f64).unwrap_or(53.0).to_radians();
        let vs = pr.get("wireframeControls/verticalScale").map(Value::as_f64).unwrap_or(0.5);
        let (sh, ch) = hr.sin_cos();
        let (sv, cv) = vr.sin_cos();
        let to = |x: usize, y: usize| -> [f32; 2] {
            // Grid point relative to the layer centre (layer px), height in layer px.
            let gx = x as f64 / (st.nx - 1) as f64 * lw - lw * 0.5;
            let gy = y as f64 / (st.nx - 1) as f64 * lw - (st.ny - 1) as f64 / (st.nx - 1) as f64 * lw * 0.5;
            let hgt = st.u[y * st.nx + x] as f64 * vs * lh * 0.5;
            let (rx, ry) = (gx * ch - gy * sh, gx * sh + gy * ch);
            let (px, py) = b.to_px([lw * 0.5 + rx, lh * 0.5 + ry * cv - hgt * sv]);
            [px as f32, py as f32]
        };
        let mut lines = Vec::new();
        for y in 0..st.ny {
            for x in 0..st.nx {
                let a = to(x, y);
                if x + 1 < st.nx {
                    let c = to(x + 1, y);
                    lines.push(Sprite::new(a[0], a[1], 0.4, [0.4, 0.9, 0.4, 1.0], Shape::Line { dx: c[0] - a[0], dy: c[1] - a[1] }));
                }
                if y + 1 < st.ny {
                    let c = to(x, y + 1);
                    lines.push(Sprite::new(a[0], a[1], 0.4, [0.4, 0.9, 0.4, 1.0], Shape::Line { dx: c[0] - a[0], dy: c[1] - a[1] }));
                }
            }
        }
        let plan = crate::sim::SpritePlan { sprites: lines, tints: vec![], acc: Acc::Over, post: crate::sim::Post::OverBlack };
        if plan_only {
            return Err(WavePlan::Wire(plan));
        }
        Ok(plan.finish(Buf { img: Image::new(bw, bh), offset: b.offset, scale: b.scale }).img)
    }
}
