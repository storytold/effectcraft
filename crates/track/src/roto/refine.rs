//! From a frame's binary segmentation to the final soft matte: the Roto Brush Matte and Refine
//! Edge Matte adjustments, edge matting inside the Refine Edge band (colour guided filter),
//! Reduce Chatter (temporal averaging with the neighbouring frames), matte motion blur and edge
//! colour decontamination.

use super::FrameSeg;
use super::matting::{self, contrast, gauss, guided_filter, shift_soft, signed_distance};

/// The effect's matte parameters (After Effects' units: pixels and percent).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MatteParams {
    // Roto Brush Matte.
    pub rb_feather: f64,
    pub rb_contrast: f64,
    pub rb_shift: f64,
    pub rb_chatter: f64,
    // Refine Edge Matte.
    pub smooth: f64,
    pub feather: f64,
    pub contrast: f64,
    pub shift: f64,
    pub chatter: f64,
    pub motion_blur: bool,
    pub mb_samples: usize,
    pub shutter_angle: f64,
    /// Higher Quality: closed-form matting after the guided-filter estimate.
    pub higher_quality: bool,
    pub decontaminate: bool,
    pub decontamination: f64,
    pub extend_where_smoothed: bool,
    pub increase_radius: f64,
}

impl Default for MatteParams {
    fn default() -> Self {
        MatteParams {
            rb_feather: 0.0,
            rb_contrast: 0.0,
            rb_shift: 0.0,
            rb_chatter: 0.0,
            smooth: 1.0,
            feather: 0.0,
            contrast: 0.0,
            shift: 0.0,
            chatter: 0.0,
            motion_blur: false,
            mb_samples: 11,
            shutter_angle: 180.0,
            higher_quality: true,
            decontaminate: true,
            decontamination: 100.0,
            extend_where_smoothed: true,
            increase_radius: 0.0,
        }
    }
}

fn shifted(seg: &FrameSeg, px: f64) -> Vec<f32> {
    if px.abs() < 0.25 {
        return seg.matte.iter().map(|v| *v as f32).collect();
    }
    let sd = signed_distance(&seg.matte, seg.w, seg.h);
    sd.iter().map(|d| ((*d as f64) > -px) as u8 as f32).collect()
}

/// The soft matte of `seg` (all planes `seg.w × seg.h`), with `rgb` the frame's straight colour.
/// `prev` / `next`: the neighbouring frames' segmentations (Reduce Chatter, motion blur).
/// `band_radius`: the Refine Edge brush radius in these pixels; `scale`: pixels per layer pixel.
pub fn matte_alpha(
    rgb: &[[f32; 3]],
    seg: &FrameSeg,
    prev: Option<&FrameSeg>,
    next: Option<&FrameSeg>,
    p: &MatteParams,
    band_radius: f64,
    scale: f64,
) -> Vec<f32> {
    let (w, h) = (seg.w, seg.h);
    let n = w * h;
    let same = |s: &&FrameSeg| s.w == w && s.h == h;
    let (prev, next) = (prev.filter(same), next.filter(same));
    // Roto Brush Matte: Shift Edge (100 % = 10 px), Reduce Chatter, Feather, Contrast.
    let shift_px = p.rb_shift / 100.0 * 10.0 * scale;
    let mut a = shifted(seg, shift_px);
    let nb: Vec<Vec<f32>> = [prev, next].into_iter().flatten().map(|s| shifted(s, shift_px)).collect();
    if p.rb_chatter > 0.0 && !nb.is_empty() {
        let k = (p.rb_chatter / 100.0).clamp(0.0, 1.0) as f32 * (nb.len() as f32 / (nb.len() as f32 + 1.0));
        for i in 0..n {
            let m = nb.iter().map(|v| v[i]).sum::<f32>() / nb.len() as f32;
            a[i] += (m - a[i]) * k;
        }
    }
    if p.rb_feather > 0.0 {
        a = gauss(&a, w, h, p.rb_feather * scale / 2.0);
    }
    contrast(&mut a, p.rb_contrast);
    // Refine Edge band: guided-filter matting.
    if seg.refine.iter().any(|v| *v != 0) && seg.refine.len() == n {
        let r = (band_radius * 1.0).round().max(2.0) as usize;
        let eps = (2e-4 * (1.0 + p.smooth.max(0.0))) as f32;
        // Work on the band's bounding box (plus the filter's reach).
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        for (i, v) in seg.refine.iter().enumerate() {
            if *v != 0 {
                let (x, y) = (i % w, i / w);
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
        let pad = 2 * r + 2;
        let (x0, y0, x1, y1) = (x0.saturating_sub(pad), y0.saturating_sub(pad), (x1 + pad).min(w), (y1 + pad).min(h));
        let (cw, ch) = (x1 - x0, y1 - y0);
        let crop = |src: &dyn Fn(usize) -> f32| -> Vec<f32> { (0..cw * ch).map(|j| src((y0 + j / cw) * w + x0 + j % cw)).collect() };
        let cimg: Vec<[f32; 3]> = (0..cw * ch).map(|j| rgb[(y0 + j / cw) * w + x0 + j % cw]).collect();
        let cp = crop(&|i| a[i]);
        let band: Vec<bool> = crop(&|i| seg.refine[i] as f32).iter().map(|v| *v > 0.0).collect();
        let outside: Vec<bool> = band.iter().map(|b| !b).collect();
        // Guided filter estimate, then closed-form matting with everything outside the band known.
        let mut q = guided_filter(&cimg, &cp, cw, ch, r, eps);
        for j in 0..cw * ch {
            q[j] = if band[j] { q[j].clamp(0.0, 1.0) } else { cp[j] };
        }
        if p.higher_quality {
            q = matting::closed_form(&cimg, &q, &outside, (cw, ch), 1, 1e-5 * (1.0 + p.smooth.max(0.0)) as f32, 50);
        }
        // Refine Edge Matte adjustments.
        if p.smooth > 1.0 {
            q = gauss(&q, cw, ch, (p.smooth - 1.0) * 0.5 * scale);
        }
        if p.feather > 0.0 {
            q = gauss(&q, cw, ch, p.feather * scale / 2.0);
        }
        contrast(&mut q, p.contrast);
        shift_soft(&mut q, p.shift);
        // Blend in, fading over the band's last two pixels.
        let din = matting::distance(&outside, cw, ch);
        for j in 0..cw * ch {
            if !band[j] {
                continue;
            }
            let wgt = (din[j] / 2.0).clamp(0.0, 1.0);
            let i = (y0 + j / cw) * w + x0 + j % cw;
            a[i] += (q[j] - a[i]) * wgt;
        }
        // Refine-stage Reduce Chatter: towards the neighbours' mattes inside the band.
        if p.chatter > 0.0 && !nb.is_empty() {
            let k = (p.chatter / 100.0).clamp(0.0, 1.0) as f32 * 0.5;
            let soft: Vec<Vec<f32>> = nb.iter().map(|v| gauss(v, w, h, (r as f64 / 2.0).max(1.0))).collect();
            for i in 0..n {
                if seg.refine[i] != 0 {
                    let m = soft.iter().map(|v| v[i]).sum::<f32>() / soft.len() as f32;
                    a[i] += (m - a[i]) * k;
                }
            }
        }
    }
    // Motion blur: the matte's motion between the neighbouring frames.
    if p.motion_blur {
        let c = seg.centroid();
        let v = match (prev.and_then(FrameSeg::centroid), next.and_then(FrameSeg::centroid), c) {
            (Some(a), Some(b), _) => Some([(b[0] - a[0]) / 2.0, (b[1] - a[1]) / 2.0]),
            (Some(a), None, Some(c)) => Some([c[0] - a[0], c[1] - a[1]]),
            (None, Some(b), Some(c)) => Some([b[0] - c[0], b[1] - c[1]]),
            _ => None,
        };
        if let Some(v) = v {
            let s = p.shutter_angle / 360.0;
            a = matting::motion_blur(&a, w, h, [v[0] * s, v[1] * s], p.mb_samples);
        }
    }
    a.iter_mut().for_each(|v| *v = v.clamp(0.0, 1.0));
    a
}

/// Decontaminate edge colours of `rgb` in place for the matte `alpha` (see
/// [`matting::decontaminate`]); returns the decontamination map. `band`: the Refine Edge band
/// (pixels where decontamination always applies).
pub fn decontaminate(rgb: &mut [[f32; 3]], alpha: &[f32], band: &[u8], size: (usize, usize), p: &MatteParams, band_radius: f64, scale: f64) -> Vec<f32> {
    let (w, h) = size;
    if !p.decontaminate {
        return vec![0.0; w * h];
    }
    let grow = p.increase_radius * scale;
    let in_band: Vec<bool> = if band.len() == w * h && band.iter().any(|v| *v != 0) {
        if grow > 0.5 {
            let d = matting::distance(&band.iter().map(|v| *v != 0).collect::<Vec<_>>(), w, h);
            d.iter().map(|v| (*v as f64) <= grow).collect()
        } else {
            band.iter().map(|v| *v != 0).collect()
        }
    } else {
        vec![false; w * h]
    };
    let region: Vec<bool> = (0..w * h).map(|i| alpha[i] > 0.004 && alpha[i] < 0.996 && (in_band[i] || p.extend_where_smoothed)).collect();
    let radius = (band_radius + grow).max(4.0 * scale).round() as usize;
    matting::decontaminate(rgb, alpha, &region, w, h, radius, (p.decontamination / 100.0).clamp(0.0, 1.0) as f32)
}
