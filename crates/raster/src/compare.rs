//! Frame comparison for agents and golden tests: error metrics (max / mean absolute difference,
//! PSNR, SSIM, changed-pixel share) and an amplified difference heatmap for two 8-bit RGBA images,
//! plus the area resampler that brings two frames to a common size.
//!
//! Images are straight (unpremultiplied) RGBA8. Colour is compared **premultiplied** (each image
//! composited over black), so the colour under fully transparent pixels never counts; alpha is
//! compared as its own channel. All differences are in 8-bit levels (0–255).
//!
//! Pure math, no I/O: callers decode and encode PNGs. Every size is checked against the buffer
//! it describes, so a hostile width/height can't index out of bounds.

/// A borrowed straight-alpha RGBA8 image (`data.len() == width * height * 4`).
#[derive(Clone, Copy, Debug)]
pub struct Rgba8<'a> {
    pub width: u32,
    pub height: u32,
    pub data: &'a [u8],
}

impl<'a> Rgba8<'a> {
    pub fn new(width: u32, height: u32, data: &'a [u8]) -> Result<Self, CompareError> {
        let img = Rgba8 { width, height, data };
        img.check()?;
        Ok(img)
    }

    fn pixels(&self) -> Result<usize, CompareError> {
        (self.width as usize).checked_mul(self.height as usize).ok_or(CompareError::BadBuffer { width: self.width, height: self.height, len: self.data.len() })
    }

    fn check(&self) -> Result<(), CompareError> {
        let n = self.pixels()?;
        if self.width == 0 || self.height == 0 {
            return Err(CompareError::Empty);
        }
        if n.checked_mul(4) != Some(self.data.len()) {
            return Err(CompareError::BadBuffer { width: self.width, height: self.height, len: self.data.len() });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CompareError {
    /// The two images differ in size (resample one first, see [`resample`]).
    SizeMismatch { a: (u32, u32), b: (u32, u32) },
    /// `data` doesn't hold `width * height` RGBA pixels.
    BadBuffer { width: u32, height: u32, len: usize },
    /// A zero-sized image.
    Empty,
    /// A crop rectangle that is empty or reaches outside the image.
    BadRegion { region: [u32; 4], width: u32, height: u32 },
}

impl std::fmt::Display for CompareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompareError::SizeMismatch { a, b } => write!(f, "size mismatch: a is {}x{}, b is {}x{}", a.0, a.1, b.0, b.1),
            CompareError::BadBuffer { width, height, len } => {
                write!(f, "{width}x{height} RGBA image needs {} bytes, got {len}", u128::from(*width) * u128::from(*height) * 4)
            }
            CompareError::Empty => write!(f, "empty image"),
            CompareError::BadRegion { region: [x, y, w, h], width, height } => {
                write!(f, "region [{x}, {y}, {w}, {h}] is empty or outside the {width}x{height} image")
            }
        }
    }
}

impl std::error::Error for CompareError {}

/// Comparison settings.
#[derive(Clone, Copy, Debug)]
pub struct CompareOpts {
    /// Per-channel tolerance in 8-bit levels: a pixel is *changed* when any channel (premultiplied
    /// R, G, B or alpha) differs by more than this.
    pub threshold: f64,
    /// Heatmap gain: a difference of `255 / amplify` levels or more shows as white.
    pub amplify: f64,
}

impl Default for CompareOpts {
    fn default() -> Self {
        CompareOpts { threshold: 1.0, amplify: 8.0 }
    }
}

/// What [`compare`] measured. Colour metrics are over premultiplied R, G, B (alpha separately).
#[derive(Clone, Debug, PartialEq)]
pub struct CompareStats {
    pub width: u32,
    pub height: u32,
    /// Largest colour-channel difference (levels).
    pub max_abs: f64,
    /// Mean colour-channel difference (levels).
    pub mean_abs: f64,
    /// Largest alpha difference (levels).
    pub alpha_max_abs: f64,
    /// Mean squared colour-channel difference (levels²).
    pub mse: f64,
    /// Peak signal-to-noise ratio in dB (`f64::INFINITY` when the colour is identical).
    pub psnr_db: f64,
    /// Mean structural similarity of luma over 8×8 windows (stride 4); 1 = identical.
    pub ssim: f64,
    /// Pixels where some channel differs by more than the threshold.
    pub changed_pixels: u64,
    /// `changed_pixels` as a percentage of all pixels.
    pub changed_pct: f64,
    /// Bounding box `[x, y, w, h]` of the changed pixels (`None` when none changed).
    pub diff_bounds: Option<[u32; 4]>,
}

impl CompareStats {
    /// The default verdict: no pixel beyond the threshold. With `min_psnr`, the colour PSNR
    /// criterion replaces it (for references that are close but never bit-exact, such as another
    /// app's render).
    pub fn pass(&self, min_psnr: Option<f64>) -> bool {
        match min_psnr {
            Some(db) => self.psnr_db >= db,
            None => self.changed_pixels == 0,
        }
    }
}

/// Premultiplied (R, G, B, A) of pixel `i` in levels; zero outside the buffer.
fn premul(d: &[u8], i: usize) -> [f64; 4] {
    let Some(p) = i.checked_mul(4).and_then(|o| d.get(o..o + 4)) else { return [0.0; 4] };
    let a = p[3] as f64;
    let k = a / 255.0;
    [p[0] as f64 * k, p[1] as f64 * k, p[2] as f64 * k, a]
}

/// Rec. 709 luma of a premultiplied pixel.
fn luma(p: [f64; 4]) -> f64 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

fn same_size(a: &Rgba8, b: &Rgba8) -> Result<usize, CompareError> {
    a.check()?;
    b.check()?;
    if (a.width, a.height) != (b.width, b.height) {
        return Err(CompareError::SizeMismatch { a: (a.width, a.height), b: (b.width, b.height) });
    }
    a.pixels()
}

/// Compare two same-sized images.
pub fn compare(a: &Rgba8, b: &Rgba8, opts: &CompareOpts) -> Result<CompareStats, CompareError> {
    let n = same_size(a, b)?;
    let threshold = if opts.threshold.is_finite() { opts.threshold.max(0.0) } else { 0.0 };
    let (mut max_abs, mut sum_abs, mut sum_sq, mut alpha_max, mut changed) = (0f64, 0f64, 0f64, 0f64, 0u64);
    let (mut la, mut lb) = (Vec::with_capacity(n), Vec::with_capacity(n));
    let w = a.width as usize;
    // Changed-pixel box as (min x, min y, max x, max y), inclusive.
    let mut bounds: Option<(usize, usize, usize, usize)> = None;
    for i in 0..n {
        let (pa, pb) = (premul(a.data, i), premul(b.data, i));
        let mut px_max = 0f64;
        for c in 0..3 {
            let d = (pa[c] - pb[c]).abs();
            sum_abs += d;
            sum_sq += d * d;
            px_max = px_max.max(d);
        }
        max_abs = max_abs.max(px_max);
        let da = (pa[3] - pb[3]).abs();
        alpha_max = alpha_max.max(da);
        if px_max.max(da) > threshold {
            changed += 1;
            let (x, y) = (i % w, i / w);
            bounds = Some(match bounds {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
        }
        la.push(luma(pa) as f32);
        lb.push(luma(pb) as f32);
    }
    let samples = (n as f64) * 3.0;
    let mse = sum_sq / samples;
    let psnr_db = if mse <= 0.0 { f64::INFINITY } else { 10.0 * (255.0 * 255.0 / mse).log10() };
    let ssim = ssim(&la, &lb, a.width as usize, a.height as usize);
    Ok(CompareStats {
        width: a.width,
        height: a.height,
        max_abs,
        mean_abs: sum_abs / samples,
        alpha_max_abs: alpha_max,
        mse,
        psnr_db,
        ssim,
        changed_pixels: changed,
        changed_pct: changed as f64 * 100.0 / n as f64,
        // Coordinates are below the u32 width/height they came from.
        diff_bounds: bounds.map(|(x0, y0, x1, y1)| [x0 as u32, y0 as u32, (x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32]),
    })
}

/// Mean SSIM over `WIN`×`WIN` windows stepped by `WIN / 2` (one window when the image is smaller).
fn ssim(a: &[f32], b: &[f32], w: usize, h: usize) -> f64 {
    const WIN: usize = 8;
    const C1: f64 = (0.01 * 255.0) * (0.01 * 255.0);
    const C2: f64 = (0.03 * 255.0) * (0.03 * 255.0);
    let (ww, wh) = (WIN.min(w), WIN.min(h));
    let starts = |len: usize, win: usize| -> Vec<usize> {
        let mut v: Vec<usize> = (0..=len.saturating_sub(win)).step_by((win / 2).max(1)).collect();
        // Cover the last row/column too.
        if v.last().is_some_and(|&l| l + win < len) {
            v.push(len - win);
        }
        v
    };
    let (xs, ys) = (starts(w, ww), starts(h, wh));
    let (mut total, mut count) = (0f64, 0u64);
    let m = (ww * wh) as f64;
    for &y0 in &ys {
        for &x0 in &xs {
            let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0f64, 0f64, 0f64, 0f64, 0f64);
            for y in y0..y0 + wh {
                let row = y * w;
                for x in x0..x0 + ww {
                    let (va, vb) = (a.get(row + x).copied().unwrap_or(0.0) as f64, b.get(row + x).copied().unwrap_or(0.0) as f64);
                    sa += va;
                    sb += vb;
                    saa += va * va;
                    sbb += vb * vb;
                    sab += va * vb;
                }
            }
            let (ma, mb) = (sa / m, sb / m);
            let (va, vb, cov) = ((saa / m - ma * ma).max(0.0), (sbb / m - mb * mb).max(0.0), sab / m - ma * mb);
            total += ((2.0 * ma * mb + C1) * (2.0 * cov + C2)) / ((ma * ma + mb * mb + C1) * (va + vb + C2));
            count += 1;
        }
    }
    if count == 0 { 1.0 } else { total / count as f64 }
}

/// An opaque RGBA8 heatmap of where `a` and `b` differ: unchanged pixels show `a`'s luma dimmed
/// to a dark grey, differences within the threshold dark blue, and changed pixels a red → yellow →
/// white ramp by size (`amplify` times the difference; even a one-level change is bright red).
pub fn diff_heatmap(a: &Rgba8, b: &Rgba8, opts: &CompareOpts) -> Result<Vec<u8>, CompareError> {
    let n = same_size(a, b)?;
    let threshold = if opts.threshold.is_finite() { opts.threshold.max(0.0) } else { 0.0 };
    let gain = if opts.amplify.is_finite() && opts.amplify > 0.0 { opts.amplify } else { 1.0 };
    let mut out = Vec::with_capacity(n.saturating_mul(4));
    for i in 0..n {
        let (pa, pb) = (premul(a.data, i), premul(b.data, i));
        let d = (0..4).map(|c| (pa[c] - pb[c]).abs()).fold(0f64, f64::max);
        let px = if d > threshold {
            let t = (d * gain / 255.0).clamp(0.0, 1.0);
            if t < 0.5 { [255, (t * 510.0) as u8, 0] } else { [255, 255, ((t - 0.5) * 510.0).min(255.0) as u8] }
        } else {
            let g = (luma(pa) * 0.3) as u8;
            if d > 0.0 { [g / 2, g / 2, 140] } else { [g, g, g] }
        };
        out.extend_from_slice(&[px[0], px[1], px[2], 255]);
    }
    Ok(out)
}

/// The `[x, y, w, h]` rectangle of `src` as a new straight RGBA8 buffer. The rectangle must be
/// non-empty and lie inside the image.
pub fn crop(src: &Rgba8, region: [u32; 4]) -> Result<Vec<u8>, CompareError> {
    src.check()?;
    let [x, y, w, h] = region;
    let bad = CompareError::BadRegion { region, width: src.width, height: src.height };
    let inside = |o: u32, len: u32, side: u32| len > 0 && o.checked_add(len).is_some_and(|end| end <= side);
    if !inside(x, w, src.width) || !inside(y, h, src.height) {
        return Err(bad);
    }
    if (x, y, w, h) == (0, 0, src.width, src.height) {
        return Ok(src.data.to_vec());
    }
    let (sw, x, w) = (src.width as usize, x as usize, w as usize);
    let mut out = Vec::with_capacity(w.saturating_mul(h as usize).saturating_mul(4));
    for row in y as usize..(y as usize + h as usize) {
        let start = (row * sw + x) * 4;
        out.extend_from_slice(src.data.get(start..start + w * 4).ok_or_else(|| bad.clone())?);
    }
    Ok(out)
}

/// The size of a `width`×`height` image scaled so its longest side is at most `max_side`
/// (unchanged when it already fits or `max_side` is 0).
pub fn fit_max_side(width: u32, height: u32, max_side: u32) -> (u32, u32) {
    let long = width.max(height);
    if max_side == 0 || long <= max_side {
        return (width, height);
    }
    let s = max_side as f64 / long as f64;
    (((width as f64 * s).round() as u32).clamp(1, max_side), ((height as f64 * s).round() as u32).clamp(1, max_side))
}

/// Whether two sizes have the same aspect ratio, up to rounding one side to whole pixels (or 1%).
pub fn same_aspect(a: (u32, u32), b: (u32, u32)) -> bool {
    if a.0 == 0 || a.1 == 0 || b.0 == 0 || b.1 == 0 {
        return false;
    }
    let (ra, rb) = (a.0 as f64 / a.1 as f64, b.0 as f64 / b.1 as f64);
    // Rounding the smaller image's sides moves its ratio by at most about 1/side.
    let slack = (1.0 / a.0.min(a.1).min(b.0).min(b.1) as f64).max(0.01);
    (ra / rb - 1.0).abs() <= slack
}

/// Per output index along one axis: the (source index, weight) taps, weights summing to 1.
/// Downscaling averages the covered area (box filter); upscaling interpolates linearly.
fn axis_taps(src: usize, dst: usize) -> Vec<Vec<(usize, f64)>> {
    let scale = src as f64 / dst as f64;
    (0..dst)
        .map(|i| {
            if scale <= 1.0 {
                let c = ((i as f64 + 0.5) * scale - 0.5).clamp(0.0, (src - 1) as f64);
                let (i0, f) = (c.floor() as usize, c.fract());
                let i1 = (i0 + 1).min(src - 1);
                return if f > 0.0 && i1 != i0 { vec![(i0, 1.0 - f), (i1, f)] } else { vec![(i0, 1.0)] };
            }
            let (lo, hi) = (i as f64 * scale, ((i + 1) as f64 * scale).min(src as f64));
            let mut taps = vec![];
            let mut j = lo.floor() as usize;
            while (j as f64) < hi && j < src {
                let w = (hi.min((j + 1) as f64) - lo.max(j as f64)).max(0.0);
                if w > 0.0 {
                    taps.push((j, w / (hi - lo)));
                }
                j += 1;
            }
            taps
        })
        .collect()
}

/// Resample `src` to `width`×`height` (area average down, bilinear up, in premultiplied space so
/// transparent pixels don't bleed their colour). Returns straight RGBA8.
pub fn resample(src: &Rgba8, width: u32, height: u32) -> Result<Vec<u8>, CompareError> {
    src.check()?;
    if width == 0 || height == 0 {
        return Err(CompareError::Empty);
    }
    let (sw, sh, dw, dh) = (src.width as usize, src.height as usize, width as usize, height as usize);
    let out_len = dw.checked_mul(dh).and_then(|n| n.checked_mul(4)).ok_or(CompareError::BadBuffer { width, height, len: 0 })?;
    if (sw, sh) == (dw, dh) {
        return Ok(src.data.to_vec());
    }
    let (tx, ty) = (axis_taps(sw, dw), axis_taps(sh, dh));
    // Horizontal pass: sh rows of dw premultiplied pixels.
    let mut mid = vec![[0f64; 4]; dw.saturating_mul(sh)];
    for y in 0..sh {
        for (x, taps) in tx.iter().enumerate() {
            let mut acc = [0f64; 4];
            for &(j, w) in taps {
                let p = premul(src.data, y * sw + j);
                for c in 0..4 {
                    acc[c] += p[c] * w;
                }
            }
            if let Some(m) = mid.get_mut(y * dw + x) {
                *m = acc;
            }
        }
    }
    let mut out = Vec::with_capacity(out_len);
    for taps in &ty {
        for x in 0..dw {
            let mut acc = [0f64; 4];
            for &(j, w) in taps {
                let p = mid.get(j * dw + x).copied().unwrap_or([0.0; 4]);
                for c in 0..4 {
                    acc[c] += p[c] * w;
                }
            }
            let a = acc[3].clamp(0.0, 255.0);
            let un = |v: f64| if a > 0.0 { (v * 255.0 / a).round().clamp(0.0, 255.0) as u8 } else { 0 };
            out.extend_from_slice(&[un(acc[0]), un(acc[1]), un(acc[2]), a.round() as u8]);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `w`×`h` opaque image: white where `x >= edge`, black elsewhere.
    fn edge(w: u32, h: u32, edge: u32) -> Vec<u8> {
        (0..w * h).flat_map(|i| if i % w >= edge { [255, 255, 255, 255] } else { [0, 0, 0, 255] }).collect()
    }

    #[test]
    fn identical_frames_pass_with_infinite_psnr() {
        let d = edge(16, 8, 5);
        let a = Rgba8::new(16, 8, &d).unwrap();
        let s = compare(&a, &a, &CompareOpts::default()).unwrap();
        assert_eq!((s.max_abs, s.mean_abs, s.changed_pixels), (0.0, 0.0, 0));
        assert!(s.psnr_db.is_infinite());
        assert!((s.ssim - 1.0).abs() < 1e-9);
        assert!(s.pass(None) && s.pass(Some(60.0)));
        assert_eq!(s.diff_bounds, None);
    }

    /// A `w`×`h` black image with an opaque white `size`×`size` square at (`x`, `y`).
    fn square(w: u32, h: u32, x: u32, y: u32, size: u32) -> Vec<u8> {
        (0..w * h)
            .flat_map(|i| {
                let (px, py) = (i % w, i / w);
                if (x..x + size).contains(&px) && (y..y + size).contains(&py) { [255, 255, 255, 255] } else { [0, 0, 0, 255] }
            })
            .collect()
    }

    #[test]
    fn diff_bounds_box_the_changed_pixels() {
        // A 6×6 square moved 10 px right: the box spans where it left and where it arrived.
        let (da, db) = (square(40, 20, 3, 5, 6), square(40, 20, 13, 5, 6));
        let (a, b) = (Rgba8::new(40, 20, &da).unwrap(), Rgba8::new(40, 20, &db).unwrap());
        let s = compare(&a, &b, &CompareOpts::default()).unwrap();
        assert_eq!((s.changed_pixels, s.diff_bounds), (72, Some([3, 5, 16, 6])));
        // One changed pixel is a 1×1 box; differences within the threshold don't count.
        let (dc, mut dd) = (vec![9u8; 4 * 4 * 4], vec![9u8; 4 * 4 * 4]);
        dd[(2 * 4 + 3) * 4] = 200;
        dd[0] = 10;
        let (c, d) = (Rgba8::new(4, 4, &dc).unwrap(), Rgba8::new(4, 4, &dd).unwrap());
        assert_eq!(compare(&c, &d, &CompareOpts::default()).unwrap().diff_bounds, Some([3, 2, 1, 1]));
    }

    #[test]
    fn crop_takes_the_rectangle_and_rejects_bad_ones() {
        let d = square(40, 20, 3, 5, 6);
        let a = Rgba8::new(40, 20, &d).unwrap();
        let c = crop(&a, [2, 4, 3, 2]).unwrap();
        assert_eq!(c.len(), 3 * 2 * 4);
        // Row 4 is above the square; row 5 starts it at x = 3.
        assert_eq!(c.chunks(4).map(|p| p[0]).collect::<Vec<_>>(), vec![0, 0, 0, 0, 255, 255]);
        assert_eq!(crop(&a, [0, 0, 40, 20]).unwrap(), d);
        for r in [[0, 0, 0, 5], [0, 0, 5, 0], [38, 0, 3, 1], [0, 19, 1, 2], [u32::MAX, 0, 2, 2], [0, 0, u32::MAX, 1]] {
            assert!(matches!(crop(&a, r), Err(CompareError::BadRegion { .. })), "{r:?}");
        }
        // Compare on crops: the metrics see only the rectangle.
        let e = square(40, 20, 13, 5, 6);
        let b = Rgba8::new(40, 20, &e).unwrap();
        let (ca, cb) = (crop(&a, [0, 0, 10, 20]).unwrap(), crop(&b, [0, 0, 10, 20]).unwrap());
        let s = compare(&Rgba8::new(10, 20, &ca).unwrap(), &Rgba8::new(10, 20, &cb).unwrap(), &CompareOpts::default()).unwrap();
        assert_eq!((s.changed_pixels, s.diff_bounds), (36, Some([3, 5, 6, 6])));
        assert_eq!(diff_heatmap(&Rgba8::new(10, 20, &ca).unwrap(), &Rgba8::new(10, 20, &cb).unwrap(), &CompareOpts::default()).unwrap().len(), 10 * 20 * 4);
    }

    #[test]
    fn one_pixel_shift_gives_known_numbers() {
        // 16×8, the edge moves from x=5 to x=6: one column (8 pixels) flips black ↔ white.
        let (da, db) = (edge(16, 8, 5), edge(16, 8, 6));
        let (a, b) = (Rgba8::new(16, 8, &da).unwrap(), Rgba8::new(16, 8, &db).unwrap());
        let s = compare(&a, &b, &CompareOpts::default()).unwrap();
        assert_eq!(s.max_abs, 255.0);
        assert_eq!(s.changed_pixels, 8);
        assert!((s.changed_pct - 100.0 / 16.0).abs() < 1e-9);
        // 8 of 128 pixels differ by 255 in all three channels.
        assert!((s.mean_abs - 255.0 / 16.0).abs() < 1e-9);
        assert!((s.mse - 255.0 * 255.0 / 16.0).abs() < 1e-6);
        assert!((s.psnr_db - 10.0 * 16f64.log10()).abs() < 1e-9, "{}", s.psnr_db); // ≈ 12.04 dB
        assert!(s.ssim < 0.9);
        assert!(!s.pass(None));
        assert!(s.pass(Some(12.0)) && !s.pass(Some(13.0)));
        // The heatmap marks exactly that column white (255-level change × 8 → saturated).
        let h = diff_heatmap(&a, &b, &CompareOpts::default()).unwrap();
        let px = |x: usize, y: usize| &h[(y * 16 + x) * 4..(y * 16 + x) * 4 + 4];
        assert_eq!(px(5, 3), [255, 255, 255, 255]);
        assert_eq!(px(4, 3), [0, 0, 0, 255]);
        assert_eq!(px(9, 3), [76, 76, 76, 255]);
    }

    #[test]
    fn threshold_ignores_small_differences() {
        let da = vec![100u8, 100, 100, 255];
        let db = vec![102u8, 100, 100, 255];
        let (a, b) = (Rgba8::new(1, 1, &da).unwrap(), Rgba8::new(1, 1, &db).unwrap());
        assert_eq!(compare(&a, &b, &CompareOpts { threshold: 1.0, ..Default::default() }).unwrap().changed_pixels, 1);
        let s = compare(&a, &b, &CompareOpts { threshold: 2.0, ..Default::default() }).unwrap();
        assert_eq!((s.changed_pixels, s.max_abs), (0, 2.0));
        assert!(s.pass(None));
    }

    #[test]
    fn colour_under_full_transparency_is_ignored_alpha_is_not() {
        let (da, db) = (vec![255u8, 0, 0, 0], vec![0u8, 255, 0, 0]);
        let (a, b) = (Rgba8::new(1, 1, &da).unwrap(), Rgba8::new(1, 1, &db).unwrap());
        assert!(compare(&a, &b, &CompareOpts::default()).unwrap().pass(None));
        let dc = vec![255u8, 0, 0, 255];
        let s = compare(&a, &Rgba8::new(1, 1, &dc).unwrap(), &CompareOpts::default()).unwrap();
        assert_eq!((s.alpha_max_abs, s.changed_pixels), (255.0, 1));
    }

    #[test]
    fn size_mismatch_and_bad_buffers_are_errors() {
        let (da, db) = (edge(4, 4, 2), edge(4, 2, 2));
        let (a, b) = (Rgba8::new(4, 4, &da).unwrap(), Rgba8::new(4, 2, &db).unwrap());
        assert_eq!(compare(&a, &b, &CompareOpts::default()), Err(CompareError::SizeMismatch { a: (4, 4), b: (4, 2) }));
        assert!(diff_heatmap(&a, &b, &CompareOpts::default()).is_err());
        assert!(matches!(Rgba8::new(4, 4, &db), Err(CompareError::BadBuffer { .. })));
        assert_eq!(Rgba8::new(0, 4, &[]).unwrap_err(), CompareError::Empty);
        assert!(Rgba8::new(u32::MAX, u32::MAX, &[]).is_err());
        // A hand-built Rgba8 with a lying size is caught too.
        let lying = Rgba8 { width: 100, height: 100, data: &da };
        assert!(compare(&lying, &lying, &CompareOpts::default()).is_err());
        assert!(resample(&lying, 2, 2).is_err());
    }

    #[test]
    fn resample_halves_by_area_and_keeps_flat_colour() {
        // 4×2 → 2×1: each output pixel averages a 2×2 block.
        let d: Vec<u8> = [[0u8, 0, 0, 255], [255, 255, 255, 255], [10, 20, 30, 255], [10, 20, 30, 255]].iter().cycle().take(8).flatten().copied().collect();
        let src = Rgba8::new(4, 2, &d).unwrap();
        let out = resample(&src, 2, 1).unwrap();
        assert_eq!(out, vec![128, 128, 128, 255, 10, 20, 30, 255]);
        // Upscaling a flat image stays flat; transparent neighbours don't bleed colour.
        let flat = [[200u8, 100, 50, 255], [0, 255, 0, 0]].concat();
        let up = resample(&Rgba8::new(2, 1, &flat).unwrap(), 8, 3).unwrap();
        assert_eq!(&up[0..4], &[200, 100, 50, 255]);
        assert!(up.chunks(4).all(|p| p[3] == 0 || (p[0], p[1], p[2]) == (200, 100, 50)));
        assert_eq!(fit_max_side(1920, 1080, 960), (960, 540));
        assert_eq!(fit_max_side(100, 50, 0), (100, 50));
        assert!(same_aspect((1920, 1080), (960, 540)) && same_aspect((1920, 1080), (853, 480)) && !same_aspect((1920, 1080), (800, 800)));
    }
}

#[cfg(test)]
mod hostile_error_tests {
    use super::*;
    #[test]
    fn extreme_dimensions_can_be_reported_without_overflow() {
        let error = Rgba8::new(u32::MAX, u32::MAX, &[]).unwrap_err();
        let bytes = u128::from(u32::MAX) * u128::from(u32::MAX) * 4;
        assert!(error.to_string().contains(&bytes.to_string()));
    }
}
