//! Half-float viewer frames (32 bpc projects on the desktop, #2797): the RAM preview keeps the
//! rendered frame's full range, without the HDR step, so viewer exposure, Compand / Tone Map and
//! the display colour run on it in the display pass (`crate::display_gpu`), in that order.

use effectcraft_engine::render::Image;
use effectcraft_engine::viewer::{DisplayPass, FrameEncoding};
use half::f16;
use half::slice::HalfFloatSliceExt;
use rayon::prelude::*;

/// Rows converted per parallel piece (about 120 KB of output per piece for 4K).
const ROWS: usize = 8;

/// A premultiplied RGBA frame in half floats, encoded in the output space as rendered (values
/// above 1.0 kept), with how it is encoded.
pub struct F16Frame {
    pub width: u32,
    pub height: u32,
    /// RGBA, row by row.
    pub data: Vec<f16>,
    pub encoding: FrameEncoding,
}

/// Bytes per pixel of a half-float frame.
pub const F16_PIXEL_BYTES: usize = 8;

impl F16Frame {
    /// A rendered frame in half floats. NaN becomes 0 (NaN alpha is transparent, as in
    /// `display_pixel`) and values past the half-float range become ±65504.
    pub fn from_image(img: &Image, encoding: FrameEncoding) -> F16Frame {
        let w = img.width as usize;
        let src = img.data.as_flattened();
        let mut data = vec![f16::ZERO; src.len()];
        let chunk = (w * 4 * ROWS).max(4);
        data.par_chunks_mut(chunk).zip(src.par_chunks(chunk)).for_each(|(out, inp)| {
            out.convert_from_f32_slice(inp);
            for v in out.iter_mut() {
                if !v.is_finite() {
                    *v = if v.is_nan() { f16::ZERO } else { f16::MAX.copysign(*v) };
                }
            }
        });
        F16Frame { width: img.width, height: img.height, data, encoding }
    }

    pub fn size(&self) -> [usize; 2] {
        [self.width as usize, self.height as usize]
    }

    pub fn bytes(&self) -> usize {
        self.data.len() * 2
    }

    /// Pixel `i` (row-major) as `f32`.
    pub fn pixel(&self, i: usize) -> Option<[f32; 4]> {
        let p = self.data.get(i * 4..i * 4 + 4)?;
        Some([p[0].to_f32(), p[1].to_f32(), p[2].to_f32(), p[3].to_f32()])
    }

    /// The frame through `pass` on the CPU (no GPU, or a single sample).
    pub fn to_color_image(&self, pass: &DisplayPass) -> egui::ColorImage {
        let px: Vec<egui::Color32> = self
            .data
            .par_chunks(4)
            .map(|p| {
                let [r, g, b, a] = pass.pixel([p[0].to_f32(), p[1].to_f32(), p[2].to_f32(), p[3].to_f32()]);
                egui::Color32::from_rgba_premultiplied(r, g, b, a)
            })
            .collect();
        egui::ColorImage::new(self.size(), px)
    }

    /// Averaged down by `k` (rounded up at the edges), as `frames::average_down` does for 8-bit
    /// frames: for frames wider or taller than the GPU's texture limit (#201).
    pub fn averaged_down(&self, k: usize) -> F16Frame {
        let [w, h] = self.size();
        let k = k.max(1);
        let (fw, fh) = (w.div_ceil(k), h.div_ceil(k));
        let mut data = vec![f16::ZERO; fw * fh * 4];
        data.par_chunks_mut(4).enumerate().for_each(|(i, o)| {
            let (x0, y0) = ((i % fw) * k, (i / fw) * k);
            let (mut sum, mut n) = ([0f32; 4], 0f32);
            for y in y0..(y0 + k).min(h) {
                for x in x0..(x0 + k).min(w) {
                    if let Some(p) = self.pixel(y * w + x) {
                        for c in 0..4 {
                            sum[c] += p[c];
                        }
                        n += 1.0;
                    }
                }
            }
            for c in 0..4 {
                o[c] = f16::from_f32(sum[c] / n.max(1.0));
            }
        });
        F16Frame { width: fw as u32, height: fh as u32, data, encoding: self.encoding }
    }
}
