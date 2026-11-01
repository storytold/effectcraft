//! Noise & Grain effects: film grain, median-family filters, denoising and procedural noise.

use effectcraft_color::{hsl_to_rgb, luminance, rgb_to_hsl};
use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::{Image, Px};
use rayon::prelude::*;

use crate::generate::value_noise;
use crate::util::{Plane, gauss_plane, guided_filter, hash1, join, lerp, premul, smoothstep, split, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Noise & Grain", params, render, gpu: false, float: true }
}

fn pct() -> ParamUi {
    slider(0.0, 100.0, 0.0, 100.0, 1)
}

/// Fractional-octave fBm of value noise in 0..1 (octave amplitude 0.5, frequency 2).
pub(crate) fn fbm(u: f32, v: f32, z: f32, seed: u32, octaves: f32) -> f32 {
    let octaves = octaves.clamp(1.0, 20.0);
    let n = octaves.ceil() as usize;
    let frac = octaves - octaves.floor();
    let (mut sum, mut norm, mut amp, mut f) = (0.0, 0.0, 1.0, 1.0);
    for o in 0..n {
        let w = if o + 1 == n && frac > 0.0 { frac } else { 1.0 };
        sum += value_noise(u * f, v * f, z + o as f32 * 7.31, seed.wrapping_add(o as u32)) * amp * w;
        norm += amp * w;
        amp *= 0.5;
        f *= 2.0;
    }
    sum / norm.max(1e-6)
}

// ---- Add Grain / Match Grain shared look ----

/// Blending Mode options of Add Grain and Match Grain.
pub(crate) const GRAIN_BLEND_MODES: [&str; 5] = ["Film", "Multiply", "Add", "Screen", "Overlay"];

/// The Tweaking / Color / Application / Animation controls Add Grain and Match Grain share
/// (twirl-down groups with the same ids in both effects).
pub struct GrainLook {
    pub intensity: f32,
    pub channel_intensity: [f32; 3],
    pub size: f32,
    pub channel_size: [f32; 3],
    pub softness: f64,
    pub aspect: f32,
    pub mono: bool,
    pub saturation: f32,
    pub tint_amount: f32,
    pub tint: [f32; 3],
    pub mode: u32,
    /// Shadows, Midtones, Highlights weights and the tonal Midpoint.
    pub tones: [f32; 3],
    pub midpoint: f32,
    /// Grain time (frames of grain at the animation speed) and whether it interpolates.
    pub frame: f32,
    pub seed: u32,
    /// Color ▸ Red / Green / Blue Balance.
    pub balance: [f32; 3],
}

impl GrainLook {
    pub fn from_params(ctx: &EffectCtx) -> GrainLook {
        let pr = ctx.params;
        let ch = |pre: &str, a: &str, b: &str, c: &str| [a, b, c].map(|k| pr.get(&format!("{pre}{k}")).map(Value::as_f64).unwrap_or(1.0) as f32);
        let ft = ctx.time * 24.0 * pr.f("animation/animationSpeed");
        let smooth = pr.get("animation/animateSmoothly").is_none_or(Value::as_bool);
        let tint = pr.color("color/tintColor");
        GrainLook {
            intensity: pr.f("tweaking/intensity") as f32,
            channel_intensity: ch("tweaking/channelIntensities/", "redIntensity", "greenIntensity", "blueIntensity"),
            size: pr.f("tweaking/size").max(0.05) as f32,
            channel_size: ch("tweaking/channelSize/", "redSize", "greenSize", "blueSize"),
            softness: pr.f("tweaking/softness"),
            aspect: pr.f("tweaking/aspectRatio").max(0.05) as f32,
            mono: pr.b("color/monochromatic"),
            saturation: pr.f("color/saturation") as f32,
            tint_amount: (pr.f("color/tintAmount") as f32).clamp(0.0, 1.0),
            tint: [tint[0], tint[1], tint[2]],
            mode: pr.e("application/blendingMode"),
            tones: [pr.f("application/shadows") as f32, pr.f("application/midtones") as f32, pr.f("application/highlights") as f32],
            midpoint: pr.get("application/midpoint").map(Value::as_f64).unwrap_or(0.5).clamp(0.01, 0.99) as f32,
            frame: if smooth && ft.fract() != 0.0 { ft as f32 } else { ft.floor() as f32 },
            seed: (pr.f("animation/randomSeed") as i64 as u32) ^ ctx.seed.wrapping_mul(0x9e37),
            balance: ch("color/", "redBalance", "greenBalance", "blueBalance"),
        }
        .with_preset(pr.e("preset"))
    }

    /// Apply Add Grain's Preset (a film-stock look on top of the controls).
    fn with_preset(mut self, preset: u32) -> GrainLook {
        // (intensity ×, size ×, softness +, saturation ×, monochrome)
        let (ki, ks, soft, ksat, mono) = match preset {
            1 => (0.6, 0.7, 0.2, 0.6, false),
            2 => (1.3, 1.8, 0.4, 0.8, false),
            3 => (1.8, 2.5, 0.8, 0.5, false),
            4 => (1.0, 0.6, 0.0, 1.0, false),
            5 => (1.6, 1.4, 0.3, 1.0, true),
            _ => return self,
        };
        self.intensity *= ki;
        self.size *= ks;
        self.softness += soft;
        self.saturation *= ksat;
        self.mono |= mono;
        self
    }

    /// Zero-mean noise planes (one, or one per channel) of grain `size` pixels (times each
    /// channel's size), softened by Softness, stretched by Aspect Ratio.
    pub(crate) fn planes(&self, w: usize, h: usize, size: f32, scale: f64, salt: u32) -> Vec<Plane> {
        let nch = if self.mono { 1 } else { 3 };
        let mut planes: Vec<Plane> = (0..nch)
            .map(|k| {
                let s = (size * if self.mono { 1.0 } else { self.channel_size[k].max(0.05) }).max(0.05);
                let mut pl = Plane::new(w, h);
                pl.data.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
                    for (x, v) in row.iter_mut().enumerate() {
                        *v = value_noise(x as f32 / (s * self.aspect), y as f32 / s, self.frame, self.seed.wrapping_add(k as u32 * salt)) - 0.5;
                    }
                });
                pl
            })
            .collect();
        let soft = self.softness * scale;
        if soft > 0.05 {
            planes = planes.iter().map(|pl| gauss_plane(pl, soft * self.aspect as f64, soft)).collect();
        }
        planes
    }

    /// Tonal weight of the grain at straight colour `c` (Shadows / Midtones / Highlights around
    /// the Midpoint).
    pub(crate) fn weight(&self, c: [f32; 3]) -> f32 {
        let l = luminance(c[0], c[1], c[2]).clamp(0.0, 1.0);
        let s = 1.0 - smoothstep(0.0, self.midpoint, l);
        let hi = smoothstep(self.midpoint, 1.0, l);
        self.tones[0] * s + self.tones[1] * (1.0 - s - hi) + self.tones[2] * hi
    }

    /// Per-channel grain from the raw plane values `g` (Saturation, Tint).
    pub(crate) fn color(&self, mut g: [f32; 3]) -> [f32; 3] {
        if !self.mono {
            let gm = (g[0] + g[1] + g[2]) / 3.0;
            g = g.map(|v| gm + (v - gm) * self.saturation);
        }
        if self.tint_amount > 0.0 {
            let l = luminance(self.tint[0], self.tint[1], self.tint[2]).max(1e-3);
            g = [0, 1, 2].map(|k| g[k] * ((1.0 - self.tint_amount) + self.tint_amount * self.tint[k] / l));
        }
        [g[0] * self.balance[0], g[1] * self.balance[1], g[2] * self.balance[2]]
    }

    /// Combine source channel `c` with signed grain `n` by the Blending Mode.
    pub(crate) fn blend(&self, c: f32, n: f32) -> f32 {
        match self.mode {
            // Multiply: scales the source (the grain is signed, so it can darken or lighten).
            1 => c * (1.0 + n),
            // Add.
            2 => c + n,
            // Screen: always lighter.
            3 => 1.0 - (1.0 - c) * (1.0 - n.abs()),
            // Overlay: full grain in the midtones, less in shadows and highlights.
            4 => c + n * 4.0 * c * (1.0 - c),
            // Film: embedded in the image, stronger in the darker tones.
            _ => c + n * (1.0 - 0.5 * c),
        }
        .max(0.0)
    }
}

/// Grain control parameters shared by Add Grain and Match Grain, in Effect Controls order:
/// (Tweaking, Color, Application, Animation).
pub(crate) fn grain_params() -> (Vec<crate::ParamSpec>, Vec<crate::ParamSpec>, Vec<crate::ParamSpec>, Vec<crate::ParamSpec>) {
    let mult = || slider(0.0, 10.0, 0.0, 2.0, 2);
    let tweaking = vec![
        p("tweaking/intensity", "Intensity", num(1.0), slider(0.0, 100.0, 0.0, 10.0, 2)),
        p("tweaking/channelIntensities/redIntensity", "Red Intensity", num(1.0), mult()),
        p("tweaking/channelIntensities/greenIntensity", "Green Intensity", num(1.0), mult()),
        p("tweaking/channelIntensities/blueIntensity", "Blue Intensity", num(1.0), mult()),
        p("tweaking/size", "Size", num(1.0), slider(0.05, 100.0, 0.1, 10.0, 2)),
        p("tweaking/channelSize/redSize", "Red Size", num(1.0), mult()),
        p("tweaking/channelSize/greenSize", "Green Size", num(1.0), mult()),
        p("tweaking/channelSize/blueSize", "Blue Size", num(1.0), mult()),
        p("tweaking/softness", "Softness", num(0.0), slider(0.0, 100.0, 0.0, 5.0, 2)),
        p("tweaking/aspectRatio", "Aspect Ratio", num(1.0), slider(0.05, 20.0, 0.25, 4.0, 2)),
    ];
    let color = vec![
        p("color/monochromatic", "Monochromatic", Value::Bool(false), ParamUi::Checkbox),
        p("color/saturation", "Saturation", num(1.0), mult()),
        p("color/tintAmount", "Tint Amount", num(0.0), slider(0.0, 1.0, 0.0, 1.0, 2)),
        p("color/tintColor", "Tint Color", crate::col(1.0, 1.0, 1.0), ParamUi::Color),
        p("color/redBalance", "Red Balance", num(1.0), mult()),
        p("color/greenBalance", "Green Balance", num(1.0), mult()),
        p("color/blueBalance", "Blue Balance", num(1.0), mult()),
    ];
    let application = vec![
        p("application/blendingMode", "Blending Mode", Value::Enum(0), popup(&GRAIN_BLEND_MODES)),
        p("application/shadows", "Shadows", num(1.0), mult()),
        p("application/midtones", "Midtones", num(1.0), mult()),
        p("application/highlights", "Highlights", num(1.0), mult()),
        p("application/midpoint", "Midpoint", num(0.5), slider(0.0, 1.0, 0.0, 1.0, 2)),
    ];
    let animation = vec![
        p("animation/animationSpeed", "Animation Speed", num(1.0), slider(0.0, 100.0, 0.0, 5.0, 2)),
        p("animation/animateSmoothly", "Animate Smoothly", Value::Bool(true), ParamUi::Checkbox),
        p("animation/randomSeed", "Random Seed", num(0.0), slider(0.0, 100000.0, 0.0, 1000.0, 0)),
    ];
    (tweaking, color, application, animation)
}

/// Add Grain's Viewing Mode options.
pub(crate) const ADD_GRAIN_VIEWS: [&str; 3] = ["Preview", "Blending Matte", "Final Output"];
/// Add Grain's Preset options (our own film-stock looks).
pub(crate) const GRAIN_PRESETS: [&str; 6] = ["None", "Fine 35mm", "Coarse 16mm", "Vintage 8mm", "Video Noise", "Monochrome High Speed"];

/// The Preview Region controls of the grain effects (centre, size, box display).
pub(crate) fn preview_params() -> Vec<crate::ParamSpec> {
    vec![
        p("previewRegion/center", "Center", Value::Vec2([0.5, 0.5]), ParamUi::Point),
        p("previewRegion/width", "Width", num(200.0), slider(1.0, 10000.0, 1.0, 1000.0, 0)),
        p("previewRegion/height", "Height", num(200.0), slider(1.0, 10000.0, 1.0, 1000.0, 0)),
        p("previewRegion/showBox", "Show Box", Value::Bool(true), ParamUi::Checkbox),
        p("previewRegion/boxColor", "Box Color", crate::col(1.0, 1.0, 1.0), ParamUi::Color),
    ]
}

/// Preview viewing mode: the processed pixels inside the Preview Region, the original
/// outside, and (Show Box) the region's outline.
pub(crate) fn preview_compose(ctx: &EffectCtx, b: &mut Buf, orig: &Image) {
    let c = b.to_px(ctx.params.v2("previewRegion/center"));
    let (hw, hh) = (ctx.params.f("previewRegion/width") * b.scale * 0.5, ctx.params.f("previewRegion/height") * b.scale * 0.5);
    let (x0, x1, y0, y1) = (c.0 - hw, c.0 + hw, c.1 - hh, c.1 + hh);
    let show = ctx.params.b("previewRegion/showBox");
    let col = ctx.params.color("previewRegion/boxColor");
    let w = b.img.width as usize;
    b.img.data.par_iter_mut().zip(orig.data.par_iter()).enumerate().for_each(|(i, (px, o))| {
        let (x, y) = ((i % w) as f64 + 0.5, (i / w) as f64 + 0.5);
        let inside = x >= x0 && x < x1 && y >= y0 && y < y1;
        if !inside {
            *px = *o;
        }
        if show {
            let near = |v: f64, e: f64| (v - e).abs() < 0.75;
            let on = ((near(x, x0) || near(x, x1)) && y >= y0 - 0.75 && y < y1 + 0.75) || ((near(y, y0) || near(y, y1)) && x >= x0 - 0.75 && x < x1 + 0.75);
            if on {
                *px = [col[0], col[1], col[2], 1.0];
            }
        }
    });
}

/// Boxes (x, y, size in buffer px) where grain is sampled: the flattest of a grid of
/// candidate boxes (real grain shows best where the picture has no detail).
pub(crate) fn sample_boxes(img: &Image, count: usize, size: usize) -> Vec<(usize, usize, usize)> {
    let (w, h) = (img.width as usize, img.height as usize);
    let size = size.clamp(3, w.min(h).max(3));
    if w < size || h < size || count == 0 {
        return vec![];
    }
    let low = Plane::luma(img);
    let step = (size / 2).max(1);
    // Candidate rows in parallel; the sort below is a total order (variance, y, x), so the
    // result does not depend on the collection order.
    let rows: Vec<usize> = (0..=h - size).step_by(step).collect();
    let mut cands: Vec<(f32, usize, usize)> = rows
        .par_iter()
        .flat_map_iter(|&y| {
            let data = low.data.as_slice();
            let alpha = img.data.as_slice();
            (0..=w - size).step_by(step).filter_map(move |x| {
                // Variance of the picture inside the box (the flattest boxes hold the least detail).
                let (mut s, mut sq, mut n, mut opaque) = (0.0f32, 0.0f32, 0.0f32, true);
                for yy in y..y + size {
                    for xx in x..x + size {
                        let v = data[yy * w + xx];
                        s += v;
                        sq += v * v;
                        n += 1.0;
                        opaque &= alpha[yy * w + xx][3] > 0.5;
                    }
                }
                opaque.then(|| (sq / n - (s / n).powi(2), x, y))
            })
        })
        .collect();
    cands.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)).then(a.1.cmp(&b.1)));
    cands.into_iter().take(count).map(|(_, x, y)| (x, y, size)).collect()
}

/// Draw sample boxes' outlines (Noise Samples viewing mode).
pub(crate) fn draw_boxes(img: &mut Image, boxes: &[(usize, usize, usize)], c: [f32; 4]) {
    for &(x, y, s) in boxes {
        for k in 0..s {
            for (px, py) in [(x + k, y), (x + k, y + s - 1), (x, y + k), (x + s - 1, y + k)] {
                img.set(px as u32, py as u32, [c[0], c[1], c[2], 1.0]);
            }
        }
    }
}

// ---- Add Grain ----

fn add_grain(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let look = GrainLook::from_params(ctx);
    // Viewing Mode (Final Output for projects saved before it existed).
    let view = ctx.params.get("viewingMode").map(|v| v.as_enum()).unwrap_or(2);
    if look.intensity <= 0.0 && view != 1 {
        return b;
    }
    let orig = b.img.clone();
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    let planes = look.planes(w, h, look.size * b.scale as f32, b.scale, 101);
    let amp = look.intensity * 0.25;
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let (c, a) = unpremul(*px);
        let weight = look.weight(c);
        if view == 1 {
            // Blending Matte: where (and how strongly) the grain applies.
            let v = weight.clamp(0.0, 1.0);
            *px = [v, v, v, 1.0];
            return;
        }
        if a <= 0.0 {
            return;
        }
        let raw = if look.mono { [planes[0].data[i]; 3] } else { [planes[0].data[i], planes[1].data[i], planes[2].data[i]] };
        let g = look.color(raw);
        let o = [0, 1, 2].map(|k| look.blend(c[k], g[k] * amp * look.channel_intensity[k] * weight));
        *px = premul(o, a);
    });
    if view == 0 {
        preview_compose(ctx, &mut b, &orig);
    }
    b
}

// ---- Median (Huang sliding histogram) ----

const NB: usize = 512;

#[inline]
fn quant(v: f32) -> u16 {
    (v.clamp(0.0, 1.0) * (NB - 1) as f32).round() as u16
}

/// Per-channel median over a (2r+1)² window of the premultiplied channels (values quantised to
/// 512 levels in 0..1, edges repeated).
pub(crate) fn median_image(img: &Image, r: usize) -> Image {
    let (w, h) = (img.width as usize, img.height as usize);
    if r == 0 || w == 0 || h == 0 {
        return img.clone();
    }
    let q: Vec<[u16; 4]> = img.data.par_iter().map(|p| [quant(p[0]), quant(p[1]), quant(p[2]), quant(p[3])]).collect();
    let n = (2 * r + 1) * (2 * r + 1);
    let half = n / 2;
    let ri = r as i64;
    let mut out = Image::new(img.width, img.height);
    out.rows_mut().for_each(|(y, row)| {
        let mut hist = vec![0u32; NB * 4];
        let mut med = [0usize; 4];
        let mut lt = [0usize; 4];
        let at = |x: i64, y: i64| q[y.clamp(0, h as i64 - 1) as usize * w + x.clamp(0, w as i64 - 1) as usize];
        let col = |x: i64, f: &mut dyn FnMut([u16; 4])| {
            for dy in -ri..=ri {
                f(at(x, y as i64 + dy));
            }
        };
        for dx in -ri..=ri {
            col(dx, &mut |v| {
                for c in 0..4 {
                    hist[c * NB + v[c] as usize] += 1;
                }
            });
        }
        let rebalance = |hist: &[u32], med: &mut [usize; 4], lt: &mut [usize; 4]| {
            for c in 0..4 {
                let hc = &hist[c * NB..(c + 1) * NB];
                while lt[c] > half {
                    med[c] -= 1;
                    lt[c] -= hc[med[c]] as usize;
                }
                while lt[c] + hc[med[c]] as usize <= half {
                    lt[c] += hc[med[c]] as usize;
                    med[c] += 1;
                }
            }
        };
        rebalance(&hist, &mut med, &mut lt);
        for x in 0..w {
            row[x] = med.map(|m| m as f32 / (NB - 1) as f32);
            if x + 1 == w {
                break;
            }
            col(x as i64 - ri, &mut |v| {
                for c in 0..4 {
                    hist[c * NB + v[c] as usize] -= 1;
                    if (v[c] as usize) < med[c] {
                        lt[c] -= 1;
                    }
                }
            });
            col(x as i64 + ri + 1, &mut |v| {
                for c in 0..4 {
                    hist[c * NB + v[c] as usize] += 1;
                    if (v[c] as usize) < med[c] {
                        lt[c] += 1;
                    }
                }
            });
            rebalance(&hist, &mut med, &mut lt);
        }
    });
    out
}

/// Median result honouring "operate on alpha": otherwise keep the original alpha and take the
/// median's straight colour.
fn median_px(orig: Px, m: Px, on_alpha: bool) -> Px {
    if on_alpha {
        let a = m[3].clamp(0.0, 1.0);
        return [m[0].min(a), m[1].min(a), m[2].min(a), a];
    }
    let a = orig[3];
    if m[3] <= 1e-4 {
        return orig;
    }
    premul([m[0] / m[3], m[1] / m[3], m[2] / m[3]].map(|v| v.min(1.0)), a)
}

fn median(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let r = (ctx.params.f("radius") * b.scale).round().max(0.0) as usize;
    if r == 0 {
        return b;
    }
    let on_alpha = ctx.params.b("operateOnAlpha");
    let m = median_image(&b.img, r);
    b.img.data.par_iter_mut().zip(m.data.par_iter()).for_each(|(o, &mv)| *o = median_px(*o, mv, on_alpha));
    b
}

fn dust_scratches(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let r = (ctx.params.f("radius") * b.scale).round().max(0.0) as usize;
    if r == 0 {
        return b;
    }
    let thr = ctx.params.f("threshold") as f32 / 255.0;
    let on_alpha = ctx.params.b("operateOnAlpha");
    let m = median_image(&b.img, r);
    b.img.data.par_iter_mut().zip(m.data.par_iter()).for_each(|(o, &mv)| {
        let cand = median_px(*o, mv, on_alpha);
        let diff = (0..4).map(|c| (cand[c] - o[c]).abs()).fold(0.0f32, f32::max);
        if diff > thr {
            *o = cand;
        }
    });
    b
}

/// Remove Grain's Viewing Mode options.
pub(crate) const REMOVE_GRAIN_VIEWS: [&str; 4] = ["Preview", "Noise Samples", "Blending Matte", "Final Output"];

/// Grain sampling controls (Match Grain, Remove Grain): how many flat boxes of what size the
/// grain is measured in.
pub(crate) fn sampling_params() -> Vec<crate::ParamSpec> {
    vec![
        p("sampling/samplingProcess", "Sampling Process", Value::Enum(0), popup(&["Automatic"])),
        p("sampling/numberOfSamples", "Number of Samples", num(80.0), slider(1.0, 500.0, 1.0, 200.0, 0)),
        p("sampling/sampleSize", "Sample Size", num(9.0), slider(3.0, 64.0, 3.0, 32.0, 0)),
        p("sampling/sampleBoxColor", "Sample Box Color", crate::col(1.0, 1.0, 0.0), ParamUi::Color),
    ]
}

/// The sample boxes for the effect's Sampling settings.
pub(crate) fn sampling_boxes(ctx: &EffectCtx, img: &Image, scale: f64) -> Vec<(usize, usize, usize)> {
    let n = ctx.params.get("sampling/numberOfSamples").map(Value::as_f64).unwrap_or(80.0).clamp(1.0, 500.0) as usize;
    let s = (ctx.params.get("sampling/sampleSize").map(Value::as_f64).unwrap_or(9.0) * scale).round().max(3.0) as usize;
    sample_boxes(img, n, s)
}

/// The grain level (0.5..2) measured in the sample boxes of `img`.
fn grain_level(img: &Image, boxes: &[(usize, usize, usize)]) -> f32 {
    let st = crate::noise2::grain_stats_boxes(img, boxes);
    let s = (st.std[0] + st.std[1] + st.std[2]) / 3.0;
    if boxes.is_empty() { 1.0 } else { (s / 0.02).clamp(0.5, 2.0) }
}

/// Remove Grain's measured grain level for buffer `b` without Temporal Filtering (the GPU
/// kernels' spatial pass, effectcraft-gpu `fx_noise`).
pub fn remove_grain_level(ctx: &EffectCtx, b: &Buf) -> f32 {
    grain_level(&b.img, &sampling_boxes(ctx, &b.img, b.scale))
}

fn remove_grain(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pr = ctx.params;
    let amt = pr.f("noiseReductionSettings/noiseReduction") as f32;
    let um_amount = pr.f("unsharpMask/amount") as f32 / 100.0;
    let view = pr.get("viewingMode").map(|v| v.as_enum()).unwrap_or(3);
    let boxes = sampling_boxes(ctx, &b.img, b.scale);
    if view == 1 {
        let c = pr.get("sampling/sampleBoxColor").map(|v| v.as_color()).unwrap_or([1.0, 1.0, 0.0, 1.0]);
        draw_boxes(&mut b.img, &boxes, c);
        return b;
    }
    if view == 2 {
        b.img.data.iter_mut().for_each(|p| *p = [1.0, 1.0, 1.0, 1.0]);
        return b;
    }
    let temporal = pr.b("temporalFiltering/enabled");
    if amt <= 0.0 && um_amount <= 0.0 && !temporal {
        return b;
    }
    let before = b.img.clone();
    // Temporal Filtering: steady pixels average with the neighbouring frames (grain changes
    // every frame, the picture does not); Motion Sensitivity keeps moving areas out of it.
    if temporal {
        let fps = ctx.fps();
        let host = ctx.env.host;
        let at = |t: f64| host.and_then(|h| h.self_at(t, ctx.env.effect_index)).filter(|o| o.img.width == b.img.width && o.img.height == b.img.height);
        let (prev, next) = (at(ctx.time - 1.0 / fps), at(ctx.time + 1.0 / fps));
        let k = (pr.get("temporalFiltering/amount").map(Value::as_f64).unwrap_or(100.0) / 100.0).clamp(0.0, 1.0) as f32;
        let sens = pr.get("temporalFiltering/motionSensitivity").map(Value::as_f64).unwrap_or(0.5).clamp(0.0, 1.0) as f32;
        let thr = 0.02 + (1.0 - sens) * 0.2;
        if prev.is_some() || next.is_some() {
            b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
                let mut acc = *px;
                let mut n = 1.0;
                for o in [&prev, &next].into_iter().flatten() {
                    let q = o.img.data[i];
                    let d = (0..3).map(|c| (q[c] - px[c]).abs()).fold(0.0f32, f32::max);
                    if d < thr {
                        let wgt = 1.0 - d / thr;
                        for c in 0..4 {
                            acc[c] += q[c] * wgt;
                        }
                        n += wgt;
                    }
                }
                for c in 0..4 {
                    px[c] += (acc[c] / n - px[c]) * k;
                }
            });
        }
        if amt <= 0.0 && um_amount <= 0.0 {
            if view == 0 {
                preview_compose(ctx, &mut b, &before);
            }
            return b;
        }
    }
    // The grain level measured in the sample boxes scales the smoothing.
    let level = grain_level(&b.img, &boxes);
    let passes = pr.f("noiseReductionSettings/passes").round().clamp(1.0, 8.0) as usize;
    let multichannel = pr.e("noiseReductionSettings/mode") == 0;
    let texture = (pr.f("fineTuning/texture") as f32).clamp(0.0, 1.0);
    let r = (2.0 * b.scale).round().max(1.0) as usize;
    let eps = (0.02 * amt * level).powi(2);
    let orig = split(&b.img);
    let mut ch = orig.clone();
    if amt > 0.0 {
        // Multichannel: all channels are guided by the luminance (correlated noise model);
        // Single Channel: each channel guides itself.
        for _ in 0..passes {
            let guide = multichannel.then(|| {
                let (w, h) = (ch[0].w, ch[0].h);
                Plane { w, h, data: (0..w * h).map(|i| luminance(ch[0].data[i], ch[1].data[i], ch[2].data[i])).collect() }
            });
            for c in ch.iter_mut().take(3) {
                *c = match &guide {
                    Some(g) => guided_filter(g, c, r, eps),
                    None => guided_filter(c, c, r, eps),
                };
            }
        }
        // Texture: let some of the removed fine detail back through.
        if texture > 0.0 {
            for k in 0..3 {
                ch[k] = ch[k].zip_map(&orig[k], |f, o| f + (o - f) * texture);
            }
        }
    }
    // Unsharp Mask: restore edge contrast lost to the degraining.
    if um_amount > 0.0 {
        let radius = (pr.f("unsharpMask/radius") * b.scale).max(0.1);
        let thr = pr.f("unsharpMask/threshold") as f32 / 255.0;
        for c in ch.iter_mut().take(3) {
            let low = gauss_plane(c, radius, radius);
            *c = c.zip_map(&low, |v, l| if (v - l).abs() > thr { v + (v - l) * um_amount } else { v });
        }
    }
    let alpha = orig[3].clone();
    let mut out = join(&ch);
    out.data.par_iter_mut().zip(alpha.data.par_iter()).for_each(|(p, &a)| {
        for c in 0..3 {
            p[c] = p[c].clamp(0.0, a.max(0.0));
        }
        p[3] = a;
    });
    b.img = out;
    if view == 0 {
        preview_compose(ctx, &mut b, &before);
    }
    b
}

// ---- Noise Alpha / Noise HLS ----

/// Per-pixel noise in 0..1 that changes smoothly with `phase` (whole units = new pattern).
#[inline]
fn phased(x: u32, y: u32, seed: u32, phase: f32) -> f32 {
    phased_cycle(x, y, seed, phase, 0)
}

/// [`phased`] whose patterns repeat every `cycle` whole units (0 = never): Cycle Noise.
#[inline]
fn phased_cycle(x: u32, y: u32, seed: u32, phase: f32, cycle: i64) -> f32 {
    let k = phase.floor();
    let t = phase - k;
    let k = k as i64;
    let wrap = |k: i64| if cycle > 0 { k.rem_euclid(cycle) } else { k } as u32;
    let a = hash1(x, y, seed.wrapping_add(wrap(k).wrapping_mul(0x632b)));
    let b = hash1(x, y, seed.wrapping_add(wrap(k + 1).wrapping_mul(0x632b)));
    lerp(a, b, t * t * (3.0 - 2.0 * t))
}

fn noise_alpha(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let k = ctx.params.f("amount") as f32 / 100.0;
    if k <= 0.0 {
        return b;
    }
    let kind = ctx.params.e("noise");
    let orig = ctx.params.e("originalAlpha");
    let ov = ctx.params.e("overflow");
    let seed = (ctx.params.f("randomSeed") as i64 as u32) ^ ctx.seed.wrapping_mul(0x2f1);
    let phase = ctx.params.f("noisePhase") as f32 / 360.0 + if kind >= 2 { ctx.time as f32 } else { 0.0 };
    let cycle = if ctx.params.b("noiseOptions/cycleNoise") { ctx.params.f("noiseOptions/cycle").round().max(1.0) as i64 } else { 0 };
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let ph = if kind >= 2 { phase } else { phase.floor() };
            let mut n = phased_cycle(x as u32, y as u32, seed, ph, cycle);
            if kind % 2 == 1 {
                n *= n;
            }
            let (c, a) = unpremul(*px);
            let na = match orig {
                0 => a + n * k,
                1 => {
                    if a > 0.0 {
                        a + n * k
                    } else {
                        0.0
                    }
                }
                2 => a * (1.0 + (2.0 * n - 1.0) * k),
                _ => a + (2.0 * n - 1.0) * k * 4.0 * a * (1.0 - a),
            };
            let na = match ov {
                1 => {
                    let t = na.rem_euclid(2.0);
                    if t > 1.0 { 2.0 - t } else { t }
                }
                2 => {
                    if !(0.0..=1.0).contains(&na) {
                        na.rem_euclid(1.0)
                    } else {
                        na
                    }
                }
                _ => na,
            }
            .clamp(0.0, 1.0);
            *px = premul(c, na);
        }
    });
    b
}

fn noise_hls_core(ctx: &EffectCtx, mut b: Buf, phase: f32) -> Buf {
    let hue = ctx.params.f("hue") as f32 / 100.0;
    let light = ctx.params.f("lightness") as f32 / 100.0;
    let satv = ctx.params.f("saturation") as f32 / 100.0;
    if hue == 0.0 && light == 0.0 && satv == 0.0 {
        return b;
    }
    let kind = ctx.params.e("noise");
    let gs = (ctx.params.f("grainSize") * b.scale).max(0.1) as f32;
    let seed = ctx.seed.wrapping_mul(0x51f3);
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (c, a) = unpremul(*px);
            if a <= 0.0 {
                continue;
            }
            let n = |k: u32| -> f32 {
                let s = seed.wrapping_add(k * 7919);
                match kind {
                    2 => ((value_noise(x as f32 / gs, y as f32 / gs, phase, s) - 0.5) * 3.0).clamp(-1.0, 1.0),
                    1 => {
                        let v = phased(x as u32, y as u32, s, phase) * 2.0 - 1.0;
                        v * v.abs()
                    }
                    _ => phased(x as u32, y as u32, s, phase) * 2.0 - 1.0,
                }
            };
            let (mut h, mut s, mut l) = rgb_to_hsl(c[0], c[1], c[2]);
            if hue != 0.0 {
                h = (h + n(0) * hue * 0.5).rem_euclid(1.0);
            }
            if light != 0.0 {
                l = (l + n(1) * light).clamp(0.0, 1.0);
            }
            if satv != 0.0 {
                s = (s + n(2) * satv).clamp(0.0, 1.0);
            }
            let (r, g, bl) = hsl_to_rgb(h, s, l);
            *px = premul([r, g, bl], a);
        }
    });
    b
}

fn noise_hls(ctx: &EffectCtx, b: Buf) -> Buf {
    let phase = ctx.params.f("noisePhase") as f32 / 360.0;
    noise_hls_core(ctx, b, phase)
}

fn noise_hls_auto(ctx: &EffectCtx, b: Buf) -> Buf {
    let phase = ctx.time as f32 * ctx.params.f("noiseAnimationSpeed") as f32;
    noise_hls_core(ctx, b, phase)
}

pub fn specs() -> Vec<EffectSpec> {
    let hls_common = || {
        vec![
            p("noise", "Noise", Value::Enum(0), popup(&["Uniform", "Squared", "Grain"])),
            p("hue", "Hue", num(0.0), pct()),
            p("lightness", "Lightness", num(0.0), pct()),
            p("saturation", "Saturation", num(0.0), pct()),
            p("grainSize", "Grain Size", num(1.0), slider(0.1, 100.0, 0.5, 10.0, 2)),
        ]
    };
    let mut hls = hls_common();
    hls.push(p("noisePhase", "Noise Phase", num(0.0), ParamUi::Angle));
    let mut hls_auto = hls_common();
    hls_auto.push(p("noiseAnimationSpeed", "Noise Animation Speed", num(10.0), slider(0.0, 1000.0, 0.0, 30.0, 1)));
    vec![
        {
            let (tweaking, color, application, animation) = grain_params();
            let head =
                vec![p("viewingMode", "Viewing Mode", Value::Enum(2), popup(&ADD_GRAIN_VIEWS)), p("preset", "Preset", Value::Enum(0), popup(&GRAIN_PRESETS))];
            spec("ec.noise.addgrain", "Add Grain", [head, preview_params(), tweaking, color, application, animation].into_iter().flatten().collect(), add_grain)
        },
        spec(
            "ec.noise.median",
            "Median",
            vec![
                p("radius", "Radius", num(0.0), slider(0.0, 255.0, 0.0, 50.0, 0)),
                p("operateOnAlpha", "Operate On Alpha Channel", Value::Bool(false), ParamUi::Checkbox),
            ],
            median,
        ),
        spec(
            "ec.noise.dustscratches",
            "Dust & Scratches",
            vec![
                p("radius", "Radius", num(1.0), slider(0.0, 255.0, 0.0, 50.0, 0)),
                p("threshold", "Threshold", num(0.0), slider(0.0, 255.0, 0.0, 255.0, 0)),
                p("operateOnAlpha", "Operate On Alpha Channel", Value::Bool(false), ParamUi::Checkbox),
            ],
            dust_scratches,
        ),
        spec(
            "ec.noise.removegrain",
            "Remove Grain",
            [
                vec![p("viewingMode", "Viewing Mode", Value::Enum(3), popup(&REMOVE_GRAIN_VIEWS))],
                preview_params(),
                vec![
                    p("noiseReductionSettings/noiseReduction", "Noise Reduction", num(1.0), slider(0.0, 5.0, 0.0, 5.0, 2)),
                    p("noiseReductionSettings/passes", "Passes", num(1.0), slider(1.0, 8.0, 1.0, 4.0, 0)),
                    p("noiseReductionSettings/mode", "Mode", Value::Enum(0), popup(&["Multichannel", "Single Channel"])),
                    p("fineTuning/texture", "Texture", num(0.0), slider(0.0, 1.0, 0.0, 1.0, 2)),
                    p("unsharpMask/amount", "Amount", num(0.0), slider(0.0, 500.0, 0.0, 200.0, 1)),
                    p("unsharpMask/radius", "Radius", num(1.0), slider(0.1, 100.0, 0.1, 10.0, 1)),
                    p("unsharpMask/threshold", "Threshold", num(0.0), slider(0.0, 255.0, 0.0, 255.0, 0)),
                    p("temporalFiltering/enabled", "Enabled", Value::Bool(false), ParamUi::Checkbox),
                    p("temporalFiltering/amount", "Amount", num(100.0), pct()),
                    p("temporalFiltering/motionSensitivity", "Motion Sensitivity", num(0.5), slider(0.0, 1.0, 0.0, 1.0, 2)),
                ],
                sampling_params(),
            ]
            .into_iter()
            .flatten()
            .collect(),
            remove_grain,
        ),
        spec(
            "ec.noise.noisealpha",
            "Noise Alpha",
            vec![
                p("noise", "Noise", Value::Enum(0), popup(&["Uniform Random", "Squared Random", "Uniform Animation", "Squared Animation"])),
                p("amount", "Amount", num(0.0), pct()),
                p("originalAlpha", "Original Alpha", Value::Enum(1), popup(&["Add", "Clamp", "Scale", "Edges"])),
                p("overflow", "Overflow", Value::Enum(0), popup(&["Clip", "Wrap Back", "Wrap"])),
                p("randomSeed", "Random Seed", num(0.0), slider(0.0, 100000.0, 0.0, 1000.0, 0)),
                p("noisePhase", "Noise Phase", num(0.0), ParamUi::Angle),
                p("noiseOptions/cycleNoise", "Cycle Noise", Value::Bool(false), ParamUi::Checkbox),
                p("noiseOptions/cycle", "Cycle", num(1.0), slider(1.0, 100.0, 1.0, 30.0, 0)),
            ],
            noise_alpha,
        ),
        spec("ec.noise.noisehls", "Noise HLS", hls, noise_hls),
        spec("ec.noise.noisehlsauto", "Noise HLS Auto", hls_auto, noise_hls_auto),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Params;

    fn run(id: &str, vals: &[(&str, Value)], img: Image, time: f64) -> Buf {
        let s = crate::find(id).unwrap();
        let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
        for (k, v) in vals {
            params.values.insert(k.to_string(), v.clone());
        }
        let ctx = EffectCtx { params: &params, time, layer_size: [img.width as f64, img.height as f64], seed: 3, adjustment: false, env: Default::default() };
        crate::apply(s, &ctx, Buf { img, offset: [0.0, 0.0], scale: 1.0 })
    }

    fn gray(w: u32, h: u32, v: f32) -> Image {
        Image::filled(w, h, [v, v, v, 1.0])
    }

    #[test]
    fn median_removes_salt_noise() {
        let mut img = gray(16, 16, 0.5);
        for (x, y) in [(3, 3), (8, 2), (12, 12), (5, 10)] {
            img.set(x, y, [1.0, 1.0, 1.0, 1.0]);
        }
        let out = run("ec.noise.median", &[("radius", num(1.0))], img, 0.0);
        for p in &out.img.data {
            assert!((p[0] - 0.5).abs() < 2.0 / 511.0, "{p:?}");
        }
    }

    #[test]
    fn median_matches_brute_force() {
        let (w, h) = (9u32, 7u32);
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = |k: u32| ((x * 37 + y * 101 + k * 13) % 511) as f32 / 511.0;
                img.set(x, y, [v(0), v(1), v(2), v(3)]);
            }
        }
        for r in 1..3usize {
            let m = median_image(&img, r);
            for y in 0..h as i64 {
                for x in 0..w as i64 {
                    for c in 0..4 {
                        let mut vals: Vec<f32> = vec![];
                        for dy in -(r as i64)..=r as i64 {
                            for dx in -(r as i64)..=r as i64 {
                                vals.push(img.get_clamped(x + dx, y + dy)[c]);
                            }
                        }
                        vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
                        let want = vals[vals.len() / 2];
                        let got = m.data[y as usize * w as usize + x as usize][c];
                        assert!((want - got).abs() < 1e-4, "r={r} ({x},{y}) c={c}: {want} vs {got}");
                    }
                }
            }
        }
    }

    #[test]
    fn dust_threshold_keeps_detail() {
        let mut img = gray(12, 12, 0.5);
        img.set(6, 6, [0.56, 0.56, 0.56, 1.0]);
        let keep = run("ec.noise.dustscratches", &[("radius", num(1.0)), ("threshold", num(40.0))], img.clone(), 0.0);
        assert!((keep.img.get(6, 6)[0] - 0.56).abs() < 1e-6);
        let rm = run("ec.noise.dustscratches", &[("radius", num(1.0)), ("threshold", num(5.0))], img, 0.0);
        assert!((rm.img.get(6, 6)[0] - 0.5).abs() < 2.0 / 511.0);
    }

    #[test]
    fn turbulent_noise_seeded() {
        let a = run("ec.noise.turbulent", &[("evolutionOptions/randomSeed", num(4.0))], gray(24, 16, 0.0), 0.0);
        let b = run("ec.noise.turbulent", &[("evolutionOptions/randomSeed", num(4.0))], gray(24, 16, 0.0), 0.0);
        let c = run("ec.noise.turbulent", &[("evolutionOptions/randomSeed", num(5.0))], gray(24, 16, 0.0), 0.0);
        assert_eq!(a.img, b.img);
        assert_ne!(a.img, c.img);
        assert!(a.img.data.iter().all(|p| (0.0..=1.0).contains(&p[0])));
    }

    #[test]
    fn noise_alpha_in_range() {
        for orig in 0..4 {
            for ov in 0..3 {
                let mut img = gray(10, 10, 0.5);
                img.data[3] = [0.0; 4];
                img.data[4] = [0.1, 0.1, 0.1, 0.2];
                let out = run("ec.noise.noisealpha", &[("amount", num(80.0)), ("originalAlpha", Value::Enum(orig)), ("overflow", Value::Enum(ov))], img, 0.3);
                assert!(out.img.data.iter().all(|p| (0.0..=1.0).contains(&p[3]) && p[0] <= p[3] + 1e-6));
            }
        }
    }

    #[test]
    fn noise_hls_changes_and_animates() {
        let mut img = Image::new(8, 8);
        for (i, p) in img.data.iter_mut().enumerate() {
            *p = premul([0.6, 0.3, (i % 7) as f32 / 7.0], 1.0);
        }
        let a = run("ec.noise.noisehlsauto", &[("lightness", num(30.0))], img.clone(), 0.0);
        let b = run("ec.noise.noisehlsauto", &[("lightness", num(30.0))], img.clone(), 0.55);
        assert_ne!(a.img, img);
        assert_ne!(a.img, b.img);
        let still = run("ec.noise.noisehls", &[], img.clone(), 0.0);
        assert_eq!(still.img, img);
    }

    #[test]
    fn add_grain_keeps_flat_mean() {
        let out = run("ec.noise.addgrain", &[("tweaking/intensity", num(2.0)), ("application/blendingMode", Value::Enum(2))], gray(64, 64, 0.5), 0.0);
        let mean: f32 = out.img.data.iter().map(|p| p[0]).sum::<f32>() / out.img.data.len() as f32;
        assert!((mean - 0.5).abs() < 0.05, "{mean}");
        assert!(out.img.data.iter().any(|p| (p[0] - 0.5).abs() > 0.01));
    }

    #[test]
    fn remove_grain_smooths_noise() {
        let mut img = gray(32, 32, 0.5);
        for (i, p) in img.data.iter_mut().enumerate() {
            let n = hash1(i as u32, 0, 9) * 0.1 - 0.05;
            *p = [0.5 + n, 0.5 + n, 0.5 + n, 1.0];
        }
        let var = |im: &Image| im.data.iter().map(|p| (p[0] - 0.5).powi(2)).sum::<f32>();
        let out = run("ec.noise.removegrain", &[("noiseReductionSettings/noiseReduction", num(3.0))], img.clone(), 0.0);
        assert!(var(&out.img) < var(&img) * 0.5);
        // Single Channel also smooths; Texture lets detail back; Unsharp Mask restores edges.
        let single = run(
            "ec.noise.removegrain",
            &[("noiseReductionSettings/noiseReduction", num(3.0)), ("noiseReductionSettings/mode", Value::Enum(1))],
            img.clone(),
            0.0,
        );
        assert!(var(&single.img) < var(&img) * 0.5);
        let tex = run("ec.noise.removegrain", &[("noiseReductionSettings/noiseReduction", num(3.0)), ("fineTuning/texture", num(1.0))], img.clone(), 0.0);
        assert!((var(&tex.img) - var(&img)).abs() < 1e-3);
        let mut edge = gray(16, 8, 0.2);
        for y in 0..8 {
            for x in 8..16 {
                edge.set(x, y, [0.8, 0.8, 0.8, 1.0]);
            }
        }
        let sharp = run(
            "ec.noise.removegrain",
            &[("noiseReductionSettings/noiseReduction", num(0.0)), ("unsharpMask/amount", num(100.0)), ("unsharpMask/radius", num(2.0))],
            edge.clone(),
            0.0,
        );
        assert!(sharp.img.get(8, 4)[0] > 0.8 && sharp.img.get(7, 4)[0] < 0.2);
    }

    #[test]
    fn add_grain_blending_modes_tint_and_channel_controls() {
        let base = gray(48, 48, 0.4);
        let a = run("ec.noise.addgrain", &[("tweaking/intensity", num(3.0))], base.clone(), 0.0);
        let screen = run("ec.noise.addgrain", &[("tweaking/intensity", num(3.0)), ("application/blendingMode", Value::Enum(3))], base.clone(), 0.0);
        // Screen only ever lightens.
        assert!(screen.img.data.iter().all(|p| p[0] >= 0.4 - 1e-6));
        assert_ne!(a.img, screen.img);
        // Zero red intensity leaves red untouched.
        let nored = run("ec.noise.addgrain", &[("tweaking/intensity", num(3.0)), ("tweaking/channelIntensities/redIntensity", num(0.0))], base.clone(), 0.0);
        assert!(nored.img.data.iter().all(|p| (p[0] - 0.4).abs() < 1e-6));
        assert!(nored.img.data.iter().any(|p| (p[1] - 0.4).abs() > 1e-3));
        // A full red tint on monochromatic grain puts the grain in red only... mostly.
        let tinted = run(
            "ec.noise.addgrain",
            &[
                ("tweaking/intensity", num(3.0)),
                ("color/monochromatic", Value::Bool(true)),
                ("color/tintAmount", num(1.0)),
                ("color/tintColor", crate::col(1.0, 0.0, 0.0)),
            ],
            base,
            0.0,
        );
        assert!(tinted.img.data.iter().all(|p| (p[1] - 0.4).abs() < 1e-6));
        assert!(tinted.img.data.iter().any(|p| (p[0] - 0.4).abs() > 1e-3));
    }

    #[test]
    fn grain_preview_presets_balance_and_views() {
        let base = gray(64, 64, 0.5);
        let grain = [("tweaking/intensity", num(3.0))];
        let with = |extra: &[(&str, Value)]| run("ec.noise.addgrain", &[grain.as_slice(), extra].concat(), base.clone(), 0.0).img;
        // Preview: grain only inside the region; its box outline drawn.
        let prev = with(&[
            ("viewingMode", Value::Enum(0)),
            ("previewRegion/center", Value::Vec2([32.0, 32.0])),
            ("previewRegion/width", num(20.0)),
            ("previewRegion/height", num(20.0)),
        ]);
        assert_eq!(prev.get(2, 2), base.get(2, 2));
        assert_ne!(prev.get(32, 32), base.get(32, 32));
        assert_eq!(prev.get(22, 32), [1.0, 1.0, 1.0, 1.0], "box outline");
        // Blending Matte shows the tonal weight (1 at mid grey with the defaults).
        assert_eq!(with(&[("viewingMode", Value::Enum(1))]).get(5, 5), [1.0, 1.0, 1.0, 1.0]);
        // Presets and channel balance change the grain.
        let plain = with(&[]);
        assert_ne!(with(&[("preset", Value::Enum(3))]), plain);
        let no_red = with(&[("color/redBalance", num(0.0))]);
        assert!(no_red.data.iter().all(|p| (p[0] - 0.5).abs() < 1e-6));
        // Sample boxes sit in the flat part of a picture.
        let mut half = gray(64, 64, 0.5);
        for y in 0..64 {
            for x in 32..64 {
                half.set(x, y, if (x + y) % 2 == 0 { [1.0; 4] } else { [0.0, 0.0, 0.0, 1.0] });
            }
        }
        let boxes = sample_boxes(&half, 4, 8);
        assert_eq!(boxes.len(), 4);
        assert!(boxes.iter().all(|(x, _, s)| x + s <= 32), "{boxes:?}");
        // Remove Grain's Noise Samples view draws them.
        let shown = run("ec.noise.removegrain", &[("viewingMode", Value::Enum(1))], gray(64, 64, 0.5), 0.0).img;
        assert!(shown.data.iter().any(|p| p == &[1.0, 1.0, 0.0, 1.0]));
    }

    #[test]
    fn remove_grain_temporal_filtering_averages_steady_frames() {
        /// Flat grey with per-frame grain.
        struct Grainy;
        impl crate::EffectHost for Grainy {
            fn layer(&self, _: u64, _: bool) -> Option<crate::LayerPixels> {
                None
            }
            fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
                None
            }
            fn self_at(&self, t: f64, _: usize) -> Option<Buf> {
                Some(Buf { img: frame((t * 10.0).round() as u32), offset: [0.0; 2], scale: 1.0 })
            }
        }
        fn frame(f: u32) -> Image {
            crate::util::gen_image(32, 32, |x, y| {
                let n = (hash1(x as u32, y as u32, f.wrapping_mul(977)) - 0.5) * 0.06;
                [0.5 + n, 0.5 + n, 0.5 + n, 1.0]
            })
        }
        let env = crate::EffectEnv { host: Some(&Grainy), frame_rate: 10.0, ..Default::default() };
        let vals = [("noiseReductionSettings/noiseReduction", num(0.0)), ("temporalFiltering/enabled", Value::Bool(true))];
        let out = crate::run_fx("ec.noise.removegrain", &vals, frame(5), 0.5, env).img;
        let dev = |i: &Image| i.data.iter().map(|p| (p[0] - 0.5).abs()).sum::<f32>();
        assert!(dev(&out) < dev(&frame(5)) * 0.75, "{} vs {}", dev(&out), dev(&frame(5)));
    }

    #[test]
    fn noise_alpha_cycle_repeats() {
        let at = |phase: f64| {
            run(
                "ec.noise.noisealpha",
                &[
                    ("amount", num(60.0)),
                    ("noise", Value::Enum(2)),
                    ("noisePhase", num(phase)),
                    ("noiseOptions/cycleNoise", Value::Bool(true)),
                    ("noiseOptions/cycle", num(3.0)),
                ],
                Image::filled(12, 12, [0.25, 0.25, 0.25, 0.5]),
                0.0,
            )
            .img
        };
        assert_eq!(at(90.0), at(90.0 + 3.0 * 360.0));
        assert_ne!(at(90.0), at(90.0 + 360.0));
    }
}
