//! Half-float viewer frames (32 bpc projects on the desktop, #2797): the RAM preview keeps the
//! rendered frame's full range, without the HDR step, so viewer exposure, Compand / Tone Map and
//! the display colour run on it in the display pass (`crate::display_gpu`), in that order.

use effectcraft_engine::render::Image;
use effectcraft_engine::viewer::{DisplayPass, FrameEncoding};
use half::f16;
use half::slice::{HalfBitsSliceExt, HalfFloatSliceExt};
use rayon::prelude::*;

/// Values converted per piece: 16 KB of input, so the piece stays in L1 while its non-finite
/// values are fixed up.
const BLOCK: usize = 4096;

/// A premultiplied RGBA frame in half floats, encoded in the output space as rendered (values
/// above 1.0 kept), with how it is encoded. Never NaN or infinite (see [`F16Frame::from_image`]):
/// the display pass relies on it.
pub struct F16Frame {
    pub width: u32,
    pub height: u32,
    /// RGBA, row by row, as IEEE binary16 bits.
    data: Vec<u16>,
    pub encoding: FrameEncoding,
}

/// Bytes per pixel of a half-float frame.
pub const F16_PIXEL_BYTES: usize = 8;

/// Half floats that are NaN become 0, infinities ±65504 (one pass, no branches on the data).
fn finite(bits: &mut [u16]) {
    for b in bits {
        let v = *b;
        let special = v & 0x7c00 == 0x7c00;
        let fixed = if v & 0x03ff != 0 { 0 } else { (v & 0x8000) | 0x7bff };
        *b = if special { fixed } else { v };
    }
}

impl F16Frame {
    /// A rendered frame in half floats. NaN becomes 0 (NaN alpha is transparent, as in
    /// `display_pixel`) and values past the half-float range become ±65504.
    pub fn from_image(img: &Image, encoding: FrameEncoding) -> F16Frame {
        let src = img.data.as_flattened();
        // A zeroed allocation: fresh pages from the OS, no fill pass.
        let mut data = vec![0u16; src.len()];
        data.par_chunks_mut(BLOCK).zip(src.par_chunks(BLOCK)).for_each(|(out, inp)| {
            out.reinterpret_cast_mut::<f16>().convert_from_f32_slice(inp);
            finite(out);
        });
        F16Frame { width: img.width, height: img.height, data, encoding }
    }

    /// A frame from binary16 bits (the disk cache), made finite as [`F16Frame::from_image`] does.
    pub fn from_bits(width: u32, height: u32, mut data: Vec<u16>, encoding: FrameEncoding) -> F16Frame {
        data.resize(width as usize * height as usize * 4, 0);
        data.par_chunks_mut(BLOCK).for_each(finite);
        F16Frame { width, height, data, encoding }
    }

    /// The pixels as binary16 bits (RGBA, row by row).
    pub fn bits(&self) -> &[u16] {
        &self.data
    }

    pub fn size(&self) -> [usize; 2] {
        [self.width as usize, self.height as usize]
    }

    pub fn bytes(&self) -> usize {
        self.data.len() * 2
    }

    /// Pixel `i` (row-major) as `f32`.
    pub fn pixel(&self, i: usize) -> Option<[f32; 4]> {
        let p = self.data.get(i.checked_mul(4)?..i * 4 + 4)?;
        Some([0, 1, 2, 3].map(|k| f16::from_bits(p[k]).to_f32()))
    }

    /// The frame through `pass` on the CPU (no GPU, or a single sample).
    pub fn to_color_image(&self, pass: &DisplayPass) -> egui::ColorImage {
        let px: Vec<egui::Color32> = self
            .data
            .par_chunks(4)
            .map(|p| {
                let [r, g, b, a] = pass.pixel([0, 1, 2, 3].map(|k| f16::from_bits(p[k]).to_f32()));
                egui::Color32::from_rgba_premultiplied(r, g, b, a)
            })
            .collect();
        egui::ColorImage::new(self.size(), px)
    }
}
