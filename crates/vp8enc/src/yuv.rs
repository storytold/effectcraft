//! RGBA to Y′CbCr 4:2:0 (Recommendation BT.601, studio range), padded to whole macroblocks.
//!
//! RFC 9649 §2.5 asks for BT.601 between RGB and the VP8 planes. Chroma is the average of each
//! 2×2 group weighted by alpha, so colour hidden under full transparency does not tint the edge of
//! what shows; a group with no alpha at all averages evenly.

use crate::encode::Plane;

fn luma(r: i32, g: i32, b: i32) -> u8 {
    (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16).clamp(0, 255) as u8
}

fn chroma(r: i32, g: i32, b: i32) -> (u8, u8) {
    let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
    let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
    (u.clamp(0, 255) as u8, v.clamp(0, 255) as u8)
}

/// The planes at the coded size: whole macroblocks, the picture's last row and column repeated
/// into the padding. The caller has checked `rgba` holds `width × height` pixels.
pub(crate) fn rgba_to_yuv420(rgba: &[u8], width: u32, height: u32) -> (Plane, Plane, Plane) {
    let (w, h) = (width as usize, height as usize);
    let (mbw, mbh) = (w.div_ceil(16), h.div_ceil(16));
    let mut y = Plane::new(mbw * 16, mbh * 16);
    let mut u = Plane::new(mbw * 8, mbh * 8);
    let mut v = Plane::new(mbw * 8, mbh * 8);
    let px = |x: usize, yy: usize| -> [i32; 4] {
        let i = (yy.min(h - 1) * w + x.min(w - 1)) * 4;
        match rgba.get(i..i + 4) {
            Some(p) => [i32::from(p[0]), i32::from(p[1]), i32::from(p[2]), i32::from(p[3])],
            None => [0; 4],
        }
    };
    for yy in 0..y.height {
        for x in 0..y.width {
            let [r, g, b, _] = px(x, yy);
            y.set(x, yy, luma(r, g, b));
        }
    }
    for cy in 0..u.height {
        for cx in 0..u.width {
            let group = [px(2 * cx, 2 * cy), px(2 * cx + 1, 2 * cy), px(2 * cx, 2 * cy + 1), px(2 * cx + 1, 2 * cy + 1)];
            let weight: i32 = group.iter().map(|p| p[3]).sum();
            let (mut r, mut g, mut b) = (0, 0, 0);
            for p in &group {
                let k = if weight > 0 { p[3] } else { 1 };
                r += p[0] * k;
                g += p[1] * k;
                b += p[2] * k;
            }
            let total = if weight > 0 { weight } else { 4 };
            let (cu, cv) = chroma((r + total / 2) / total, (g + total / 2) / total, (b + total / 2) / total);
            u.set(cx, cy, cu);
            v.set(cx, cy, cv);
        }
    }
    (y, u, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn studio_range() {
        assert_eq!(luma(0, 0, 0), 16);
        assert_eq!(luma(255, 255, 255), 235);
        assert_eq!(chroma(128, 128, 128), (128, 128));
    }

    #[test]
    fn padding_repeats_the_edge_and_alpha_weights_chroma() {
        // 3×1: red opaque, then two fully transparent black pixels.
        let rgba = [255, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0];
        let (y, u, v) = rgba_to_yuv420(&rgba, 3, 1);
        assert_eq!((y.width, y.height, u.width), (16, 16, 8));
        assert_eq!(y.get(15, 15), y.get(2, 0), "the last pixel repeats");
        // The first chroma sample sees only the red pixel's colour.
        assert_eq!((u.get(0, 0), v.get(0, 0)), chroma(255, 0, 0));
    }
}
