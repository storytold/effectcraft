use super::raster::over;
use super::{Buf, EffectCtx, IndexedParallelIterator, ParallelIterator, ParallelSliceMut, Px, hsl_to_rgb};

// ---------------------------------------------------------------- Fractal

#[derive(Clone, Copy)]
struct FracSet {
    /// Mandelbrot iterations start at the Julia centre (the "Over Julia" set choices).
    over_julia: bool,
    julia: bool,
    inverse: bool,
}

/// Smooth escape count, or `None` inside the set.
fn escape(z0: (f64, f64), c: (f64, f64), power: u32, limit: f64, max_iter: u32) -> Option<f64> {
    let (mut x, mut y) = z0;
    let lim2 = limit * limit;
    for i in 0..max_iter {
        let (nx, ny) = match power {
            3 => (x * x * x - 3.0 * x * y * y, 3.0 * x * x * y - y * y * y),
            4 => {
                let (a, b) = (x * x - y * y, 2.0 * x * y);
                (a * a - b * b, 2.0 * a * b)
            }
            _ => (x * x - y * y, 2.0 * x * y),
        };
        x = nx + c.0;
        y = ny + c.1;
        let m2 = x * x + y * y;
        if m2 > lim2 {
            let lz = m2.ln() * 0.5;
            let nu = (lz / limit.max(1.0001).ln()).max(1e-9).ln() / (power as f64).ln();
            return Some((i as f64 + 1.0 - nu).max(0.0));
        }
    }
    None
}

/// Fractal Set Choice options.
pub(crate) const FRACTAL_SETS: [&str; 6] =
    ["Mandelbrot", "Mandelbrot Inverse", "Mandelbrot Over Julia", "Mandelbrot Inverse Over Julia", "Julia", "Julia Inverse"];

/// Fractal Palette options.
pub(crate) const FRACTAL_PALETTES: [&str; 4] = ["Lightness Gradient", "Hue Wheel", "Black And White", "Solid Color"];

/// Fractal Oversample Method options.
pub(crate) const FRACTAL_OVERSAMPLE: [&str; 2] = ["Edge Detect-Fast-May Miss Pixels", "Brute Force-Slow-Every Pixel"];

pub(super) fn fractal(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pr = ctx.params;
    let set_choice = pr.e("setChoice");
    let power = 2 + pr.e("equation").min(2);
    let mc = (pr.f("mandelbrot/mandelbrotX"), pr.f("mandelbrot/mandelbrotY"));
    let mmag = pr.f("mandelbrot/mandelbrotMagnification");
    let mlim = pr.f("mandelbrot/mandelbrotEscapeLimit").max(1.01);
    let jc = (pr.f("julia/juliaX"), pr.f("julia/juliaY"));
    let jmag = pr.f("julia/juliaMagnification");
    let jlim = pr.f("julia/juliaEscapeLimit").max(1.01);
    let post = (pr.f("postInversionOffset/postInversionX"), pr.f("postInversionOffset/postInversionY"));
    let overlay = pr.b("fractalColor/overlay");
    let transparency = pr.b("fractalColor/transparency");
    let transparent_inside = transparency || overlay;
    let palette = pr.e("fractalColor/palette");
    let hue = (pr.f("fractalColor/hue") / 360.0) as f32;
    let steps = pr.f("fractalColor/cycleSteps").max(1.0);
    let cyc_off = pr.f("fractalColor/cycleOffset") / 360.0;
    let edge = pr.b("fractalColor/edgeHighlight");
    let every = pr.e("highQualitySettings/samplingMethod") == 1;
    let factor = pr.f("highQualitySettings/samplingFactor").round().clamp(1.0, 9.0) as usize;
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    let lw = ctx.layer_size[0].max(1.0);
    let lh = ctx.layer_size[1].max(1.0);
    let inv = 1.0 / b.scale.max(1e-9);
    let (off_x, off_y) = (b.offset[0], b.offset[1]);
    // FRACTAL_SETS order.
    let set = |julia, inverse, over_julia| FracSet { over_julia, julia, inverse };
    let sets: (FracSet, Option<FracSet>) = match set_choice {
        1 => (set(false, true, false), None),
        2 => (set(false, false, true), None),
        3 => (set(false, true, true), None),
        4 => (set(true, false, false), None),
        5 => (set(true, true, false), None),
        _ => (set(false, false, false), None),
    };
    let iter_for = |mag: f64| (96.0 + 48.0 * mag.max(0.0)).clamp(64.0, 3000.0) as u32;
    let eval_set = |s: FracSet, lx: f64, ly: f64| -> Option<f64> {
        let (center, mag, lim) = if s.julia { (jc, jmag, jlim) } else { (mc, mmag, mlim) };
        let span = 3.5 / 2f64.powf(mag);
        let mut u = (lx - lw * 0.5) / lw * span + center.0;
        let mut v = (ly - lh * 0.5) / lw * span + center.1;
        if s.inverse {
            let d = (u * u + v * v).max(1e-12);
            u = u / d + post.0;
            v = -v / d + post.1;
        }
        if s.julia {
            escape((u, v), mc, power, lim, iter_for(mag))
        } else {
            let z0 = if s.over_julia { jc } else { (0.0, 0.0) };
            escape(z0, (u, v), power, lim, iter_for(mag))
        }
    };
    let eval = |lx: f64, ly: f64| -> Option<f64> {
        let a = eval_set(sets.0, lx, ly);
        match (a, sets.1) {
            (None, Some(s2)) => eval_set(s2, lx, ly),
            _ => a,
        }
    };
    let shade = |n: Option<f64>| -> Px {
        // Solid Color: only the inside of the set is drawn (Transparency flips the sides).
        if palette == 3 {
            let (r, g, bl) = hsl_to_rgb(hue, 1.0, 0.5);
            return if n.is_none() != transparency { [r, g, bl, 1.0] } else { [0.0; 4] };
        }
        let Some(n) = n else {
            return if transparent_inside { [0.0; 4] } else { [0.0, 0.0, 0.0, 1.0] };
        };
        let u = n / steps + cyc_off;
        let band = (u.rem_euclid(1.0)) as f32;
        let (r, g, bl) = match palette {
            // Hue Wheel: the full hue circle at full saturation and brightness.
            1 => hsl_to_rgb((hue + band).rem_euclid(1.0), 1.0, 0.5),
            // Black And White: alternating bands.
            2 => {
                let v = if (u.floor() as i64).rem_euclid(2) == 0 { 0.0 } else { 1.0 };
                (v, v, v)
            }
            // Lightness Gradient: black → hue → white, the hue moving 45° each cycle.
            _ => hsl_to_rgb((hue + u.floor() as f32 * 0.125).rem_euclid(1.0), 0.85, band.clamp(0.0, 1.0)),
        };
        let mut c = [r, g, bl];
        if edge && n.fract() < 0.12 {
            c = [c[0] * 0.4 + 0.6, c[1] * 0.4 + 0.6, c[2] * 0.4 + 0.6];
        }
        [c[0], c[1], c[2], 1.0]
    };
    // First pass: one sample per pixel.
    let mut first: Vec<Option<f64>> = vec![None; w * h];
    first.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            *v = eval((x as f64 + 0.5 - off_x) * inv, (y as f64 + 0.5 - off_y) * inv);
        }
    });
    let src = if overlay { Some(b.img.clone()) } else { None };
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let here = first[y * w + x];
            let needs_ss = factor > 1
                && (every || {
                    let mut diff = false;
                    for (dx, dy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                        let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                        if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                            continue;
                        }
                        let o = first[ny as usize * w + nx as usize];
                        diff |= match (here, o) {
                            (Some(a), Some(c)) => (a - c).abs() > steps * 0.25,
                            (None, None) => false,
                            _ => true,
                        };
                    }
                    diff
                });
            let c = if needs_ss {
                let mut acc = [0.0f32; 4];
                for sy in 0..factor {
                    for sx in 0..factor {
                        let fx = x as f64 + (sx as f64 + 0.5) / factor as f64;
                        let fy = y as f64 + (sy as f64 + 0.5) / factor as f64;
                        let s = shade(eval((fx - off_x) * inv, (fy - off_y) * inv));
                        let a = s[3];
                        for k in 0..3 {
                            acc[k] += s[k] * a;
                        }
                        acc[3] += a;
                    }
                }
                let n = (factor * factor) as f32;
                [acc[0] / n, acc[1] / n, acc[2] / n, acc[3] / n]
            } else {
                let s = shade(here);
                [s[0] * s[3], s[1] * s[3], s[2] * s[3], s[3]]
            };
            *px = match &src {
                Some(o) => over(o.data[y * w + x], c),
                None => c,
            };
        }
    });
    b
}
