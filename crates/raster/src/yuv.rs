//! 8-bit BT.709 limited-range 4:2:0, the H.264 export's input. Shared by the export and by the
//! GPU compositor's tests, which hold its kernel to this function.

use rayon::prelude::*;

/// `v / 255` for every 8-bit `v` (the same values as dividing).
pub const UNIT8: [f32; 256] = {
    let mut t = [0.0; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = i as f32 / 255.0;
        i += 1;
    }
    t
};

/// `v.round()` for `0 <= v < 2^23`, the range the colour conversions round in: truncation and
/// the fraction are exact there, so this is `f32::round` without its libm call.
#[inline(always)]
pub fn round_pos(v: f32) -> f32 {
    let t = v as u32 as f32;
    if v - t >= 0.5 { t + 1.0 } else { t }
}

/// BT.709 limited-range 8-bit 4:2:0 from RGBA8 (2×2 chroma average; alpha ignored).
pub fn rgba_to_yuv420(rgba: &[u8], w: usize, h: usize, y: &mut Vec<u8>, u: &mut Vec<u8>, v: &mut Vec<u8>) {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    y.resize(w * h, 0);
    u.resize(cw * ch, 0);
    v.resize(cw * ch, 0);
    y.par_chunks_mut(w * 2).zip(u.par_chunks_mut(cw).zip(v.par_chunks_mut(cw))).enumerate().for_each(|(cy, (yr, (ur, vr)))| {
        let mut acc = vec![(0f32, 0f32, 0f32); cw];
        for (dy, yrow) in yr.chunks_exact_mut(w).enumerate() {
            let src = &rgba[(cy * 2 + dy) * w * 4..][..w * 4];
            for (x, (s, yo)) in src.as_chunks::<4>().0.iter().zip(yrow.iter_mut()).enumerate() {
                let (r, g, b) = (UNIT8[s[0] as usize], UNIT8[s[1] as usize], UNIT8[s[2] as usize]);
                let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                *yo = round_pos(16.0 + 219.0 * l).clamp(1.0, 254.0) as u8;
                let a = &mut acc[x / 2];
                a.0 += (b - l) / 1.8556;
                a.1 += (r - l) / 1.5748;
                a.2 += 1.0;
            }
        }
        for (cx, a) in acc.iter().enumerate() {
            ur[cx] = round_pos(128.0 + 224.0 * a.0 / a.2).clamp(1.0, 254.0) as u8;
            vr[cx] = round_pos(128.0 + 224.0 * a.1 / a.2).clamp(1.0, 254.0) as u8;
        }
    });
}

/// The H.264 export's planes of `img` over `bg` (opaque): Y, then U and V, as
/// [`crate::Image::to_rgba8_over`] + [`rgba_to_yuv420`] produce them.
pub fn yuv420_over(img: &crate::Image, bg: [f32; 3]) -> Vec<u8> {
    let (mut y, mut u, mut v) = (vec![], vec![], vec![]);
    rgba_to_yuv420(&img.to_rgba8_over(bg), img.width as usize, img.height as usize, &mut y, &mut u, &mut v);
    y.extend_from_slice(&u);
    y.extend_from_slice(&v);
    y
}
