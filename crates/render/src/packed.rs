//! Lossless compact storage of quantised layer buffers (the [`crate::LayerCache`]).
//!
//! In 8 and 16 bpc projects each layer buffer is clamped and quantised to the bit depth
//! ([`crate::color::Pipe::quantize`]): every premultiplied channel is `k as f32 * (1.0 / levels)`
//! for an integer `k` in `0..=levels` (colour ≤ alpha). Such a buffer can be kept as those
//! integers — `u8` for 255 levels, `u16` for 32768 — a quarter or half of its `f32` size, and
//! unpacks to the identical floats. [`PackedBuf::pack`] checks that every value round-trips bit
//! for bit and declines otherwise (32 bpc, linear-blending copies, unquantised style passes), so
//! packing never changes a pixel.

use effectcraft_effects::Buf;
use effectcraft_raster::{Image, Px};
use rayon::prelude::*;

/// A [`Buf`] whose pixels sit on an integer grid, stored as the grid's integers.
#[derive(Clone, Debug)]
pub(crate) struct PackedBuf {
    width: u32,
    height: u32,
    offset: [f64; 2],
    scale: f64,
    data: Packed,
}

#[derive(Clone, Debug)]
enum Packed {
    /// 8 bpc: 255 levels.
    U8(Vec<[u8; 4]>),
    /// 16 bpc: After Effects' 32768 levels.
    U16(Vec<[u16; 4]>),
}

const L8: f32 = 255.0;
const L16: f32 = 32768.0;

/// `v` as an integer level `k` with `k as f32 * inv` bitwise equal to `v` (`None` off the grid,
/// out of range, negative zero or NaN).
#[inline(always)]
fn level(v: f32, levels: f32, inv: f32) -> Option<u32> {
    if !(0.0..=1.0).contains(&v) {
        return None;
    }
    // Same rounding as `color::quantize_px` (non-negative, so the cast rounds half up).
    let k = (v * levels + 0.5) as u32;
    ((k as f32 * inv).to_bits() == v.to_bits()).then_some(k)
}

fn pack_with<T: Send>(data: &[Px], levels: f32, conv: impl Fn(u32) -> T + Sync + Send) -> Option<Vec<[T; 4]>> {
    let inv = 1.0 / levels;
    data.par_iter()
        .map(|p| {
            let k = [level(p[0], levels, inv)?, level(p[1], levels, inv)?, level(p[2], levels, inv)?, level(p[3], levels, inv)?];
            Some(k.map(&conv))
        })
        .collect()
}

impl PackedBuf {
    /// `buf` packed losslessly, or `None` when a value is not on the 8 or 16 bpc grid.
    pub(crate) fn pack(buf: &Buf) -> Option<PackedBuf> {
        let px = &buf.img.data;
        let data = match pack_with(px, L8, |k| k as u8) {
            Some(d) => Packed::U8(d),
            None => Packed::U16(pack_with(px, L16, |k| k as u16)?),
        };
        Some(PackedBuf { width: buf.img.width, height: buf.img.height, offset: buf.offset, scale: buf.scale, data })
    }

    /// The buffer again (bit-identical to the one packed).
    pub(crate) fn unpack(&self) -> Buf {
        fn floats<T: Copy + Into<u32> + Sync>(d: &[[T; 4]], levels: f32) -> Vec<Px> {
            let inv = 1.0 / levels;
            d.par_iter().map(|p| p.map(|k| k.into() as f32 * inv)).collect()
        }
        let data = match &self.data {
            Packed::U8(d) => floats(d, L8),
            Packed::U16(d) => floats(d, L16),
        };
        Buf { img: Image { width: self.width, height: self.height, data }, offset: self.offset, scale: self.scale }
    }

    /// Bytes of pixel storage.
    pub(crate) fn bytes(&self) -> usize {
        match &self.data {
            Packed::U8(d) => d.len() * 4,
            Packed::U16(d) => d.len() * 8,
        }
    }
}
