//! 8-bit limited-range 4:2:0, the H.264 export's input. Shared by the export and by the
//! GPU compositor's tests, which hold its kernel to this function.

use rayon::prelude::*;

/// The Y′CbCr matrix a frame is converted with, and so the one its file has to name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Matrix {
    Bt709,
    /// BT.2020 non-constant luminance, for Rec. 2100 PQ and HLG.
    Bt2020Ncl,
}

impl Matrix {
    /// ITU-T H.273 MatrixCoefficients.
    pub const fn code(self) -> u8 {
        match self {
            Self::Bt709 => 1,
            Self::Bt2020Ncl => 9,
        }
    }

    /// Kr, Kg, Kb, then the Cb and Cr divisors 2(1 − Kb) and 2(1 − Kr), from Rec. ITU-R BT.709-6
    /// and BT.2020-2 Table 4.
    pub const fn coefficients(self) -> [f32; 5] {
        match self {
            Self::Bt709 => [0.2126, 0.7152, 0.0722, 1.8556, 1.5748],
            Self::Bt2020Ncl => [0.2627, 0.6780, 0.0593, 1.8814, 1.4746],
        }
    }
}

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

/// Limited-range 8-bit 4:2:0 from RGBA8 (2×2 chroma average; alpha ignored).
pub fn rgba_to_yuv420(rgba: &[u8], w: usize, h: usize, matrix: Matrix, y: &mut Vec<u8>, u: &mut Vec<u8>, v: &mut Vec<u8>) {
    let [kr, kg, kb, cb_div, cr_div] = matrix.coefficients();
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
                let l = kr * r + kg * g + kb * b;
                *yo = round_pos(16.0 + 219.0 * l).clamp(1.0, 254.0) as u8;
                let a = &mut acc[x / 2];
                a.0 += (b - l) / cb_div;
                a.1 += (r - l) / cr_div;
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
pub fn yuv420_over(img: &crate::Image, bg: [f32; 3], matrix: Matrix) -> Vec<u8> {
    let (mut y, mut u, mut v) = (vec![], vec![], vec![]);
    rgba_to_yuv420(&img.to_rgba8_over(bg), img.width as usize, img.height as usize, matrix, &mut y, &mut u, &mut v);
    y.extend_from_slice(&u);
    y.extend_from_slice(&v);
    y
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bt2020_ncl_code_values() {
        for (rgb, want) in [([255, 0, 0], [74, 97, 240]), ([255, 255, 255], [235, 128, 128]), ([0, 0, 0], [16, 128, 128])] {
            let rgba = [rgb[0], rgb[1], rgb[2], 255].repeat(4);
            let (mut y, mut u, mut v) = (vec![], vec![], vec![]);
            rgba_to_yuv420(&rgba, 2, 2, Matrix::Bt2020Ncl, &mut y, &mut u, &mut v);
            assert_eq!(y, vec![want[0]; 4]);
            assert_eq!((u, v), (vec![want[1]], vec![want[2]]));
        }
        assert_eq!(Matrix::Bt709.code(), 1);
        assert_eq!(Matrix::Bt2020Ncl.code(), 9);
    }
}
