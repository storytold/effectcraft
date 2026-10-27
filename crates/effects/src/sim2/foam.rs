use super::*;

// ------------------------------------------------------------------ Foam

#[derive(Clone, Debug)]
struct FoamBubble {
    p: [f32; 2],
    v: [f32; 2],
    age: f32,
    size: f32,
    /// Spin (radians) for Physical Orientation.
    spin: f32,
    id: u32,
}

#[derive(Clone, Debug, Default)]
struct FoamState {
    b: Vec<FoamBubble>,
    carry: f32,
    next: u32,
}

static FOAM_CACHE: SimCache<FoamState> = SimCache::new(4);

/// Foam steps once per frame at 30 frames per second.
const FOAM_FPS: f64 = 30.0;

/// Foam's Bubble Texture options (the last uses Bubble Texture Layer).
pub(super) const FOAM_TEXTURES: [&str; 6] = ["Default Bubble", "Spit", "Bubblegum", "Dishwater", "Milky", "User Defined"];

/// Age (in simulation steps) at which bubble `id` pops: its lifespan, shortened at random for
/// frail bubbles (`frailty` 0 = every bubble reaches `lifespan`, 1 = uniformly early).
pub(super) fn foam_pop_age(id: u32, lifespan: f32, frailty: f32, seed: u32) -> f32 {
    lifespan * (1.0 - frailty * (hs(id, 11, seed) * 0.5 + 0.5))
}

/// Foam: bubbles born in the producer rectangle grow to their size, drift with the wind,
/// turbulence and the Flow Map (bubbles run downhill on its luminance), repel and stick to each
/// other, wobble, and pop at the end of their (Strength-shortened) lifespan, pushing their
/// neighbours away at Pop Velocity. Rendered bubbles use a built-in texture or the Bubble
/// Texture Layer, oriented by Bubble Orientation, and reflect the Environment Map.
pub(super) fn foam(ctx: &EffectCtx, b: Buf) -> Buf {
    foam_full_plan(ctx, &b).finish(b)
}

/// Foam's bubbles as sprites, `None` when it draws more than sprites (a User Defined bubble
/// texture, an Environment Map reflection or the Draft + Flow Map view's flow map).
pub(crate) fn foam_plan(ctx: &EffectCtx, b: &Buf) -> Option<crate::sim::SpritePlan> {
    let plan = foam_full_plan(ctx, b);
    (plan.user.is_none() && plan.env.is_none() && plan.flow.is_none()).then_some(plan.sprites)
}

/// A bubble disc filled by Foam's User Defined texture (buffer px; `rot` radians).
#[derive(Clone, Copy, Debug)]
pub struct FoamDisc {
    pub x: f64,
    pub y: f64,
    pub r: f32,
    pub rot: f32,
    pub fade: f32,
}

/// Foam's Environment Map reflection: the map fitted to the buffer, Reflection Convergence and
/// Strength, and the bubble discs reflecting it (`rot` and `fade` unused).
pub struct FoamEnv {
    pub img: Image,
    pub conv: f64,
    pub strength: f32,
    pub discs: Vec<FoamDisc>,
}

/// What Foam draws this frame over a transparent frame of the layer's size: the bubble sprites,
/// then the User Defined texture's discs, the Environment Map's reflections and (Draft + Flow
/// Map) the flow map under it all, each `foam_disc`-composited in bubble order.
pub struct FoamPlan {
    pub sprites: crate::sim::SpritePlan,
    pub user: Option<(crate::LayerPixels, Vec<FoamDisc>)>,
    pub env: Option<FoamEnv>,
    /// The flow map fitted to the buffer.
    pub flow: Option<Image>,
}

impl FoamPlan {
    pub fn finish(&self, b: Buf) -> Buf {
        let mut out = splat(b.img.width, b.img.height, &self.sprites.sprites, self.sprites.acc);
        // User Defined: the texture layer fills each bubble's disc.
        if let Some((tex, discs)) = &self.user {
            for d in discs {
                let (rot, fade) = (d.rot, d.fade);
                foam_disc(&mut out, d.x, d.y, d.r as f64, |u, v| {
                    // u, v in −1..1 across the bubble, rotated into the texture.
                    let (sr, cr) = rot.sin_cos();
                    let (tu, tv) = (u as f32 * cr + v as f32 * sr, -u as f32 * sr + v as f32 * cr);
                    let (x, y) = ((tu as f64 * 0.5 + 0.5) * tex.size[0], (tv as f64 * 0.5 + 0.5) * tex.size[1]);
                    let px = tex.buf.img.sample_bilinear(x * tex.buf.scale + tex.buf.offset[0], y * tex.buf.scale + tex.buf.offset[1]);
                    px.map(|c| c * fade)
                });
            }
        }
        // Environment Map: each bubble reflects the map along its sphere normal (Reflection
        // Convergence pulls the reflection towards the bubble's own spot of the map).
        if let Some(env) = &self.env {
            let (conv, strength) = (env.conv, env.strength);
            let (w, h) = (b.img.width as f64, b.img.height as f64);
            for d in &env.discs {
                let (bx, by) = (d.x, d.y);
                foam_disc(&mut out, bx, by, d.r as f64, |u, v| {
                    // Sphere normal; the reflection looks across the whole map, or (convergence 1)
                    // just behind the bubble.
                    let (ex, ey) =
                        if conv >= 1.0 { (bx, by) } else { (bx * conv + (u * 0.5 + 0.5) * w * (1.0 - conv), by * conv + (v * 0.5 + 0.5) * h * (1.0 - conv)) };
                    let e = env.img.sample_bilinear(ex, ey);
                    let rim = ((u * u + v * v) as f32).min(1.0);
                    let k = strength * (0.3 + 0.7 * rim);
                    [e[0] * k, e[1] * k, e[2] * k, 0.0]
                });
            }
        }
        // Draft + Flow Map shows the flow map under the bubbles.
        if let Some(fm) = &self.flow {
            out.data.par_iter_mut().zip(fm.data.par_iter()).for_each(|(o, f)| {
                let k = 1.0 - o[3];
                for c in 0..4 {
                    o[c] += f[c] * 0.5 * k;
                }
            });
        }
        Buf { img: out, ..b }
    }
}

/// Foam's [`FoamPlan`] for layer geometry `b` (its size; its pixels are not read).
pub fn foam_full_plan(ctx: &EffectCtx, b: &Buf) -> FoamPlan {
    let pr = ctx.params;
    let (lw, lh) = (ctx.layer_size[0] as f32, ctx.layer_size[1] as f32);
    let unit = lw * 0.01;
    let prod = pr.v2("producer/producerPoint");
    let (psx, psy) = (pr.f("producer/producerXSize") as f32 * lw, pr.f("producer/producerYSize") as f32 * lh);
    let porient = (pr.f("producer/producerOrientation") as f32).to_radians();
    let rate = pr.f("producer/productionRate").max(0.0) as f32;
    let size = pr.f("bubbles/size").max(0.0) as f32;
    let size_var = pr.f("bubbles/sizeVariance") as f32;
    let lifespan = pr.f("bubbles/lifespan").max(1.0) as f32;
    let growth = pr.f("bubbles/growthSpeed").max(0.0001) as f32;
    // Strength: weaker bubbles tend to pop before their lifespan (at 20 or more every bubble
    // lives the full lifespan); each bubble gets its own pop age.
    let frailty = 1.0 - (pr.f("bubbles/strength") as f32 / 20.0).clamp(0.0, 1.0);
    let init_speed = pr.f("physics/initialSpeed") as f32;
    let init_dir = (pr.f("physics/initialDirection") as f32).to_radians();
    let wind = pr.f("physics/windSpeed") as f32;
    let wind_dir = (pr.f("physics/windDirection") as f32).to_radians();
    let turb = pr.f("physics/turbulence") as f32;
    let wobble = pr.f("physics/wobbleAmount").max(0.0) as f32;
    let repulsion = pr.f("physics/repulsion") as f32;
    let pop_velocity = pr.f("physics/popVelocity") as f32;
    let viscosity = pr.f("physics/viscosity").clamp(0.0, 1.0) as f32;
    let sticky = pr.f("physics/stickiness").clamp(0.0, 1.0) as f32;
    let universe = pr.f("universeSize").max(0.01) as f32;
    let seed = (pr.f("randomSeed") as u32).wrapping_mul(0x68e31da4) ^ ctx.seed;
    let wvec = [wind_dir.sin() * wind * unit, -wind_dir.cos() * wind * unit];
    // Flow Map: the map layer's first frame, its luminance a height field (Fits the layer or
    // the whole universe).
    let flow = pr.get("flowMap/flowMap").and_then(Value::as_layer).and_then(|id| {
        let host = ctx.env.host?;
        let me = pr.get(&crate::layer_source_id("flowMap/flowMap")).is_none_or(|v| v.as_enum() != 0);
        host.layer_at(id, ctx.env.comp_time - ctx.time, me)
    });
    let steep = pr.f("flowMap/flowMapSteepness") as f32;
    let fits_universe = pr.e("flowMap/flowMapFits") == 1;
    let substeps = match pr.e("flowMap/simulationQuality") {
        1 => 2,
        2 => 4,
        _ => 1,
    };
    let height = flow.as_ref().map(|lp| {
        let img = &lp.buf.img;
        let pl = Plane::from_image(img, |px| {
            let (c, a) = unpremul(px);
            effectcraft_color::luminance(c[0], c[1], c[2]) * a
        });
        (pl, lp.buf.scale, lp.buf.offset, lp.size)
    });
    let (ux0, uy0) = (-(universe - 1.0) * 0.5 * lw, -(universe - 1.0) * 0.5 * lh);
    let (ux1, uy1) = (lw - ux0, lh - uy0);
    // Height at a layer point (0..1).
    let h_at = |x: f32, y: f32| -> f32 {
        let Some((pl, s, off, size)) = &height else { return 0.0 };
        let (fx, fy) = if fits_universe { ((x - ux0) / (ux1 - ux0), (y - uy0) / (uy1 - uy0)) } else { (x / lw, y / lh) };
        pl.sample(fx as f64 * size[0] * s + off[0], fy as f64 * size[1] * s + off[1])
    };
    let mut key = params_key(ctx, &Buf { img: Image::new(0, 0), offset: [0.0; 2], scale: 1.0 }, 5);
    if let Some((pl, ..)) = &height {
        key ^= pl.data.iter().step_by(5).fold(0u64, |a, v| a.rotate_left(3) ^ v.to_bits() as u64);
    }
    let steps = if ctx.time <= 0.0 { 0 } else { (ctx.time * FOAM_FPS + 1e-6).floor() as u64 };
    let rad = |q: &FoamBubble| q.size * unit * 3.0 * (q.age * growth).min(1.0);
    let st = FOAM_CACHE.run(key, steps, FoamState::default, |st, step| {
        let t = step as f32 / FOAM_FPS as f32;
        // Births.
        st.carry += rate;
        let n = st.carry.floor() as u32;
        st.carry -= n as f32;
        for _ in 0..n {
            if st.b.len() >= 3000 {
                break;
            }
            let id = st.next;
            st.next = st.next.wrapping_add(1);
            let (ux, uy) = (hs(id, 1, seed) * 0.5 * psx, hs(id, 2, seed) * 0.5 * psy);
            let (so, co) = porient.sin_cos();
            let p = [prod[0] as f32 + ux * co - uy * so, prod[1] as f32 + ux * so + uy * co];
            let v = [init_dir.sin() * init_speed * unit, -init_dir.cos() * init_speed * unit];
            let sz = (size * (1.0 + size_var * hs(id, 3, seed))).max(0.01);
            st.b.push(FoamBubble { p, v, age: 0.0, size: sz, spin: hs(id, 12, seed) * std::f32::consts::PI, id });
        }
        for _ in 0..substeps {
            let k = 1.0 / substeps as f32;
            // Forces: drift towards the wind, turbulence, viscosity, the flow map's slope.
            for q in st.b.iter_mut() {
                let n1 = value_noise(q.p[0] / (lw * 0.15), q.p[1] / (lw * 0.15), t * 0.5, seed) - 0.5;
                let n2 = value_noise(q.p[0] / (lw * 0.15) + 19.0, q.p[1] / (lw * 0.15), t * 0.5, seed) - 0.5;
                for c in 0..2 {
                    q.v[c] += (wvec[c] - q.v[c]) * 0.1 * k;
                    q.v[c] *= 1.0 - viscosity * 0.5 * k;
                }
                q.v[0] += n1 * turb * unit * k;
                q.v[1] += n2 * turb * unit * k;
                if height.is_some() && steep != 0.0 {
                    let e = unit.max(1.0);
                    let gx = (h_at(q.p[0] + e, q.p[1]) - h_at(q.p[0] - e, q.p[1])) / (2.0 * e);
                    let gy = (h_at(q.p[0], q.p[1] + e) - h_at(q.p[0], q.p[1] - e)) / (2.0 * e);
                    // Downhill: from light (high) to dark (low).
                    q.v[0] -= gx * steep * lw * 0.5 * k;
                    q.v[1] -= gy * steep * lw * 0.5 * k;
                }
                // Physical Orientation: bubbles roll with their sideways motion.
                q.spin += q.v[0] / (rad(q).max(0.5)) * k;
            }
            // Repulsion between overlapping bubbles (grid hashed, deterministic order).
            if repulsion > 0.0 && st.b.len() > 1 {
                let maxr = st.b.iter().map(rad).fold(0.0f32, f32::max).max(1.0);
                let cs = maxr * 2.0;
                let mut grid: std::collections::HashMap<(i32, i32), Vec<usize>> = std::collections::HashMap::new();
                for (i, q) in st.b.iter().enumerate() {
                    grid.entry(((q.p[0] / cs).floor() as i32, (q.p[1] / cs).floor() as i32)).or_default().push(i);
                }
                let mut push = vec![[0.0f32; 2]; st.b.len()];
                for (i, q) in st.b.iter().enumerate() {
                    let (gx, gy) = ((q.p[0] / cs).floor() as i32, (q.p[1] / cs).floor() as i32);
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            let Some(list) = grid.get(&(gx + dx, gy + dy)) else { continue };
                            for &j in list {
                                if j <= i {
                                    continue;
                                }
                                let o = &st.b[j];
                                let (ddx, ddy) = (o.p[0] - q.p[0], o.p[1] - q.p[1]);
                                let d = (ddx * ddx + ddy * ddy).sqrt().max(1e-4);
                                let overlap = rad(q) + rad(o) - d;
                                if overlap > 0.0 {
                                    let f = overlap * 0.5 * repulsion.min(4.0) * (1.0 - sticky * 0.5) / d * k;
                                    push[i][0] -= ddx * f;
                                    push[i][1] -= ddy * f;
                                    push[j][0] += ddx * f;
                                    push[j][1] += ddy * f;
                                }
                            }
                        }
                    }
                }
                for (q, d) in st.b.iter_mut().zip(&push) {
                    q.p[0] += d[0].clamp(-maxr, maxr);
                    q.p[1] += d[1].clamp(-maxr, maxr);
                }
            }
            for q in st.b.iter_mut() {
                q.p[0] += q.v[0] * k;
                q.p[1] += q.v[1] * k;
            }
        }
        for q in st.b.iter_mut() {
            q.age += 1.0;
        }
        // Pops: bubbles past their pop age burst, pushing their neighbours away.
        let popped: Vec<([f32; 2], f32)> = st.b.iter().filter(|q| q.age >= foam_pop_age(q.id, lifespan, frailty, seed)).map(|q| (q.p, rad(q))).collect();
        if pop_velocity != 0.0 {
            for (pp, pr_) in &popped {
                let reach = (pr_ * 3.0).max(unit);
                for q in st.b.iter_mut() {
                    let (dx, dy) = (q.p[0] - pp[0], q.p[1] - pp[1]);
                    let d = (dx * dx + dy * dy).sqrt();
                    if d > 1e-4 && d < reach {
                        let f = pop_velocity * unit * (1.0 - d / reach) / d;
                        q.v[0] += dx * f;
                        q.v[1] += dy * f;
                    }
                }
            }
        }
        st.b.retain(|q| q.age < foam_pop_age(q.id, lifespan, frailty, seed) && q.p[0] > ux0 && q.p[0] < ux1 && q.p[1] > uy0 && q.p[1] < uy1);
    });
    let view = pr.e("view");
    let zoom = pr.f("zoom").max(0.01) as f32;
    let zoom_producer = pr.b("producer/zoomProducerPoint");
    let texture = pr.e("rendering/bubbleTexture");
    let orient = pr.e("rendering/bubbleOrientation");
    let blend = pr.e("rendering/blendMode");
    let s = b.scale as f32;
    // Zoom centres on the layer, or on the producer with Zoom Producer Point.
    let (cx, cy) = if zoom_producer { (prod[0] as f32, prod[1] as f32) } else { (lw * 0.5, lh * 0.5) };
    let mut order: Vec<&FoamBubble> = st.b.iter().collect();
    if blend == 2 {
        // Solid New on Top: oldest first.
        order.sort_by(|a, c| c.age.total_cmp(&a.age).then(a.id.cmp(&c.id)));
    } else if blend == 1 {
        order.sort_by(|a, c| a.age.total_cmp(&c.age).then(a.id.cmp(&c.id)));
    }
    // Where a bubble is drawn: buffer centre, radius, rotation (radians).
    let place = |q: &FoamBubble| {
        let wob = if wobble > 0.0 { 1.0 + wobble * 0.15 * ((q.age * 0.35) + hs(q.id, 13, seed) * 6.0).sin() } else { 1.0 };
        let r = rad(q) * wob * zoom * s;
        let (bx, by) = b.to_px([(cx + (q.p[0] - cx) * zoom) as f64, (cy + (q.p[1] - cy) * zoom) as f64]);
        let rot = match orient {
            1 => q.spin,
            2 => q.v[1].atan2(q.v[0]),
            _ => 0.0,
        };
        (bx, by, r, rot)
    };
    let user_tex = (view == 2 && texture == 5).then(|| ctx.layer_param("rendering/bubbleTextureLayer", true)).flatten();
    let sprites: Vec<Sprite> = order
        .iter()
        .filter(|_| user_tex.is_none())
        .map(|q| {
            let (bx, by, r, rot) = place(q);
            let fade = ((foam_pop_age(q.id, lifespan, frailty, seed) - q.age) / 5.0).clamp(0.0, 1.0);
            // Draft and Draft + Flow Map show the wireframe-style bubbles.
            let mut sp = if view != 2 {
                Sprite::new(bx as f32, by as f32, r, [0.55, 0.75, 1.0, fade], Shape::Bubble)
            } else {
                let (c, shape) = match texture {
                    1 => ([0.95, 0.95, 0.92], Shape::Faded),
                    2 => ([1.0, 0.6, 0.8], Shape::Bubble),
                    3 => ([0.8, 0.85, 0.75], Shape::Bubble),
                    4 => ([0.97, 0.97, 0.95], Shape::Sphere),
                    _ => ([1.0, 1.0, 1.0], Shape::Bubble),
                };
                Sprite::new(bx as f32, by as f32, r, [c[0], c[1], c[2], fade], shape)
            };
            sp.rot = rot;
            sp
        })
        .collect();
    let acc = if blend == 0 && view == 2 { Acc::Add } else { Acc::Over };
    let disc = |q: &&FoamBubble| {
        let (x, y, r, rot) = place(q);
        let fade = ((foam_pop_age(q.id, lifespan, frailty, seed) - q.age) / 5.0).clamp(0.0, 1.0);
        FoamDisc { x, y, r, rot, fade }
    };
    let user = user_tex.map(|tex| (tex, order.iter().map(disc).collect()));
    let strength = pr.f("rendering/reflectionStrength").clamp(0.0, 1.0) as f32;
    let env = if view == 2
        && strength > 0.0
        && let Some(env) = ctx.layer_param("rendering/environmentMap", true)
    {
        Some(FoamEnv {
            img: crate::util::fit_layer(ctx, b, &env, true),
            conv: pr.f("rendering/reflectionConvergence").clamp(0.0, 1.0),
            strength,
            discs: order.iter().map(disc).collect(),
        })
    } else {
        None
    };
    let flow = if view == 1
        && let Some(lp) = &flow
    {
        Some(crate::util::fit_layer(ctx, b, lp, true))
    } else {
        None
    };
    FoamPlan { sprites: crate::sim::SpritePlan { sprites, tints: vec![], acc, post: crate::sim::Post::Replace }, user, env, flow }
}

/// Add `f(u, v)` (premultiplied; u, v ∈ −1..1 across the disc) over the disc of radius `r` at
/// (`x`, `y`) with anti-aliased edges ("over" for opaque texels, additive for alpha 0).
fn foam_disc(out: &mut Image, x: f64, y: f64, r: f64, f: impl Fn(f64, f64) -> Px) {
    if r <= 0.0 {
        return;
    }
    let (x0, x1) = ((x - r - 1.0).floor().max(0.0) as i64, (x + r + 1.0).ceil().min(out.width as f64) as i64);
    let (y0, y1) = ((y - r - 1.0).floor().max(0.0) as i64, (y + r + 1.0).ceil().min(out.height as f64) as i64);
    for py in y0..y1 {
        for px in x0..x1 {
            let (u, v) = ((px as f64 + 0.5 - x) / r, (py as f64 + 0.5 - y) / r);
            let d = (u * u + v * v).sqrt();
            let cov = ((1.0 - d) * r + 0.5).clamp(0.0, 1.0) as f32;
            if cov <= 0.0 {
                continue;
            }
            let s = f(u.clamp(-1.0, 1.0), v.clamp(-1.0, 1.0)).map(|c| c * cov);
            let o = out.get(px, py);
            let k = 1.0 - s[3];
            out.set(px as u32, py as u32, [s[0] + o[0] * k, s[1] + o[1] * k, s[2] + o[2] * k, (s[3] + o[3] * k).min(1.0)]);
        }
    }
}
