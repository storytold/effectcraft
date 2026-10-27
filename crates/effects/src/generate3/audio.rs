use super::raster::{Seg, mask_px, over, poly_segs, raster_segs};
use super::*;

// ---------------------------------------------------------------- Audio Spectrum / Waveform

/// In-place iterative radix-2 FFT (`re`, `im` lengths a power of two).
pub(crate) fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    if n < 2 {
        return;
    }
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f64;
        let (ws, wc) = ang.sin_cos();
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let (a, bb) = (start + k, start + k + len / 2);
                let tr = re[bb] * cr - im[bb] * ci;
                let ti = re[bb] * ci + im[bb] * cr;
                re[bb] = re[a] - tr;
                im[bb] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let ncr = cr * wc - ci * ws;
                ci = cr * ws + ci * wc;
                cr = ncr;
            }
        }
        len <<= 1;
    }
}

const AUDIO_RATE: u32 = 48_000;

/// Fetch mono (or one channel) samples of the audio layer around the current time.
fn audio_window(ctx: &EffectCtx, dur_ms: f64, offset_ms: f64, channel: u32) -> Option<Vec<f64>> {
    let id = ctx.params.get("audioLayer")?.as_layer()?;
    let host = ctx.env.host?;
    let dur = (dur_ms / 1000.0).max(0.001);
    let frames = ((dur * AUDIO_RATE as f64) as usize).clamp(16, 1 << 16);
    let start = ctx.env.comp_time + offset_ms / 1000.0 - dur * 0.5;
    let s = host.audio(id, start, frames, AUDIO_RATE)?;
    Some(
        (0..frames)
            .map(|i| {
                let (l, r) = (*s.get(2 * i).unwrap_or(&0.0) as f64, *s.get(2 * i + 1).unwrap_or(&0.0) as f64);
                match channel {
                    1 => l,
                    2 => r,
                    _ => (l + r) * 0.5,
                }
            })
            .collect(),
    )
}

/// The drawing baseline: a polyline in buffer px (mask path or start→end line), sampled at
/// parameter u∈[0,1] → (point, unit normal).
struct Baseline {
    pts: Vec<[f64; 2]>,
    closed: bool,
    len: f64,
    polar: bool,
    center: [f64; 2],
    radius: f64,
}

impl Baseline {
    fn new(ctx: &EffectCtx, b: &Buf) -> Baseline {
        let pr = ctx.params;
        let polar = pr.b("usePolarPath");
        let (pts, closed) = match mask_px(ctx, b, pr.f("path").round() as usize) {
            Some((p, c, _)) if !polar => (p, c),
            _ => {
                let s = b.to_px(pr.v2("startPoint"));
                let e = b.to_px(pr.v2("endPoint"));
                (vec![[s.0, s.1], [e.0, e.1]], false)
            }
        };
        let len = poly_length(&pts, closed);
        let center = pts[0];
        let radius = if pts.len() > 1 { ((pts[1][0] - pts[0][0]).powi(2) + (pts[1][1] - pts[0][1]).powi(2)).sqrt() } else { 1.0 };
        Baseline { pts, closed, len, polar, center, radius }
    }
    fn at(&self, u: f64) -> ([f64; 2], [f64; 2]) {
        if self.polar {
            let a = u * 2.0 * PI - PI * 0.5;
            let (s, c) = a.sin_cos();
            return ([self.center[0] + c * self.radius * 0.5, self.center[1] + s * self.radius * 0.5], [c, s]);
        }
        let (q, d) = crate::util::poly_point_at(&self.pts, self.closed, u * self.len);
        (q, [d[1], -d[0]])
    }
}

/// Draw line/dot marks with core (inside colour) and soft halo (outside colour).
fn draw_marks(b: &mut Buf, segs_core: &[Seg], thickness: f64, softness: f64, inside: [f32; 4], outside: [f32; 4], composite: bool) {
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    let core = raster_segs(w, h, segs_core, 1.0);
    let halo_segs: Vec<Seg> = segs_core.iter().map(|s| Seg { r: s.r + thickness * softness.max(0.0) + 0.5, ..*s }).collect();
    let halo = if softness > 0.0 { raster_segs(w, h, &halo_segs, 0.0) } else { core.clone() };
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let (c, hl) = (core.data[i], halo.data[i]);
        let a = c.max(hl);
        let base = if composite { *px } else { [0.0; 4] };
        if a <= 0.0 {
            *px = base;
            return;
        }
        let t = if a > 0.0 { c / a } else { 0.0 };
        let col = crate::util::lerp3([outside[0], outside[1], outside[2]], [inside[0], inside[1], inside[2]], t);
        *px = over(base, premul(col, a));
    });
}

/// Audio Spectrum / Audio Waveform, shared with the GPU compositor (effectcraft-gpu `fx_gen2`):
/// the marks (the audio analysis runs on the CPU) and how `draw_marks` colours them.
#[derive(Clone, Debug)]
pub struct MarksPlan {
    pub segs: Vec<Seg>,
    pub thickness: f64,
    pub softness: f64,
    pub inside: [f32; 4],
    pub outside: [f32; 4],
    pub composite: bool,
}

/// [`MarksPlan`] of Audio Spectrum or Audio Waveform on `b`'s geometry (`None` = no audio:
/// the layer passes through).
pub fn marks_plan(id: &str, ctx: &EffectCtx, b: &Buf) -> Option<MarksPlan> {
    match id {
        "ec.generate.audiospectrum" => spectrum_plan(ctx, b),
        "ec.generate.audiowaveform" => waveform_plan(ctx, b),
        _ => None,
    }
}

fn marks(ctx: &EffectCtx, segs: Vec<Seg>, thick: f64, soft: f64) -> MarksPlan {
    let pr = ctx.params;
    MarksPlan {
        segs,
        thickness: thick,
        softness: soft * 2.0,
        inside: pr.color("insideColor"),
        outside: pr.color("outsideColor"),
        composite: pr.b("compositeOnOriginal"),
    }
}

pub(super) fn audio_spectrum(ctx: &EffectCtx, mut b: Buf) -> Buf {
    if let Some(m) = spectrum_plan(ctx, &b) {
        draw_marks(&mut b, &m.segs, m.thickness, m.softness, m.inside, m.outside, m.composite);
    }
    b
}

pub(super) fn audio_waveform(ctx: &EffectCtx, mut b: Buf) -> Buf {
    if let Some(m) = waveform_plan(ctx, &b) {
        draw_marks(&mut b, &m.segs, m.thickness, m.softness, m.inside, m.outside, m.composite);
    }
    b
}

fn compute_fft_mags(samples: &[f64]) -> (usize, f64, Vec<f64>) {
    let m = samples.len();
    let n = m.next_power_of_two().max(64);
    let mut re = vec![0.0; n];
    let mut im = vec![0.0; n];
    let denom = (m.max(2) - 1) as f64;
    for (i, s) in samples.iter().enumerate() {
        let w = 0.5 - 0.5 * (2.0 * PI * i as f64 / denom).cos();
        if let Some(r) = re.get_mut(i) {
            *r = s * w;
        }
    }
    fft(&mut re, &mut im);
    let hz_per_bin = AUDIO_RATE as f64 / n as f64;
    let scale = 4.0 / m.max(1) as f64;
    let num_bins = n / 2 + 1;
    let mut mags = Vec::with_capacity(num_bins);
    for k in 0..num_bins {
        let r = re.get(k).copied().unwrap_or(0.0);
        let im_val = im.get(k).copied().unwrap_or(0.0);
        let mag = (r * r + im_val * im_val).sqrt() * scale;
        mags.push(mag);
    }
    (n, hz_per_bin, mags)
}

fn spectrum_plan(ctx: &EffectCtx, b: &Buf) -> Option<MarksPlan> {
    let pr = ctx.params;
    let samples = audio_window(ctx, pr.f("audioDuration"), pr.f("audioOffset"), 0)?;
    let (_n, hz_per_bin, mags) = compute_fft_mags(&samples);

    let bands = pr.f("frequencyBands").round().clamp(1.0, 4096.0) as usize;
    let f0 = pr.f("startFrequency").max(0.0);
    let f1 = pr.f("endFrequency").max(f0 + 1.0);
    let max_bin = mags.len().saturating_sub(1);

    let get_band_mag = |fc: f64| -> f64 {
        if mags.is_empty() || hz_per_bin <= 0.0 {
            return 0.0;
        }
        let k_float = (fc / hz_per_bin).clamp(0.0, max_bin as f64);
        let k0 = (k_float.floor() as usize).min(max_bin.saturating_sub(1));
        let k1 = (k0 + 1).min(max_bin);
        let frac = (k_float - k0 as f64).clamp(0.0, 1.0);
        let m0 = mags.get(k0).copied().unwrap_or(0.0);
        let m1 = mags.get(k1).copied().unwrap_or(0.0);
        m0 * (1.0 - frac) + m1 * frac
    };

    let max_h = pr.f("maximumHeight") * b.scale;
    let thick = (pr.f("thickness") * b.scale).max(0.1);
    let soft = pr.f("softness") / 100.0;
    let disp = pr.e("displayOptions");
    let side = pr.e("sideOptions");
    let base = Baseline::new(ctx, b);
    let mut tops = Vec::with_capacity(bands);
    let mut segs = Vec::new();
    for i in 0..bands {
        let u = if bands > 1 { i as f64 / (bands - 1) as f64 } else { 0.5 };
        let fc = f0 + (f1 - f0) * u;
        let v = get_band_mag(fc);
        let hgt = if (v * max_h).is_finite() { (v * max_h).clamp(0.0, max_h.max(0.0)) } else { 0.0 };
        let (q, nrm) = base.at(u);
        let (ua, ub) = match side {
            0 => (0.0, hgt),
            1 => (-hgt, 0.0),
            _ => (-hgt, hgt),
        };
        let pa = [q[0] + nrm[0] * ua, q[1] + nrm[1] * ua];
        let pb = [q[0] + nrm[0] * ub, q[1] + nrm[1] * ub];
        match disp {
            0 => segs.push(Seg { a: pa, b: pb, r: thick * 0.5, v: 1.0 }),
            _ => tops.push((pa, pb)),
        }
    }
    if disp == 1 {
        for w2 in tops.windows(2) {
            segs.push(Seg { a: w2[0].1, b: w2[1].1, r: thick * 0.5, v: 1.0 });
            if side == 2 {
                segs.push(Seg { a: w2[0].0, b: w2[1].0, r: thick * 0.5, v: 1.0 });
            }
        }
    } else if disp == 2 {
        for &(pa, pb) in &tops {
            segs.push(Seg { a: pb, b: pb, r: thick * 0.5, v: 1.0 });
            if side == 2 {
                segs.push(Seg { a: pa, b: pa, r: thick * 0.5, v: 1.0 });
            }
        }
    }
    Some(marks(ctx, segs, thick, soft))
}

fn waveform_plan(ctx: &EffectCtx, b: &Buf) -> Option<MarksPlan> {
    let pr = ctx.params;
    let samples = audio_window(ctx, pr.f("audioDuration"), pr.f("audioOffset"), pr.e("waveformOptions"))?;
    let count = pr.f("displayedSamples").round().clamp(2.0, 4096.0) as usize;
    let max_h = pr.f("maximumHeight") * b.scale;
    let thick = (pr.f("thickness") * b.scale).max(0.1);
    let soft = pr.f("softness") / 100.0;
    let disp = pr.e("displayOptions");
    let base = Baseline::new(ctx, b);
    let m = samples.len();
    let mut pts = Vec::with_capacity(count);
    let mut segs = Vec::new();
    for i in 0..count {
        let u = i as f64 / (count - 1) as f64;
        let a = ((u * (m - 1) as f64) as usize).min(m - 1);
        let bnd = (((i + 1) as f64 / count as f64 * (m - 1) as f64) as usize).clamp(a, m - 1);
        // Peak with sign of the window for a stable digital display.
        let (mut lo, mut hi) = (0.0f64, 0.0f64);
        for s in &samples[a..=bnd] {
            lo = lo.min(*s);
            hi = hi.max(*s);
        }
        let (q, nrm) = base.at(u);
        match disp {
            0 => segs.push(Seg {
                a: [q[0] + nrm[0] * lo * max_h, q[1] + nrm[1] * lo * max_h],
                b: [q[0] + nrm[0] * hi * max_h, q[1] + nrm[1] * hi * max_h],
                r: thick * 0.5,
                v: 1.0,
            }),
            _ => {
                let v = samples[a] * max_h;
                pts.push([q[0] + nrm[0] * v, q[1] + nrm[1] * v]);
            }
        }
    }
    if disp == 1 {
        poly_segs(&pts, false, thick * 0.5, 1.0, &mut segs);
    } else if disp == 2 {
        segs.extend(pts.iter().map(|&q| Seg { a: q, b: q, r: thick * 0.5, v: 1.0 }));
    }
    Some(marks(ctx, segs, thick, soft))
}
