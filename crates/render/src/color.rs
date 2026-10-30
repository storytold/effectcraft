//! Project colour settings in the compositor: bit depth, working space, linear blending.
//!
//! * **Bit depth.** Pixels are `f32` throughout, but 8 and 16 bpc projects behave like integer
//!   pipelines: each layer's pixels are clamped to 0..1 and quantised to the depth after its
//!   source and masks and after every effect, and the comp is clamped and quantised after every
//!   layer is blended. Over-range values (Add/Screen of bright layers, Exposure…) therefore only
//!   survive in 32 bpc. 16 bpc uses After Effects' 0..32768 range.
//! * **Working space** (colour management). Footage is converted from its colour profile (its
//!   metadata, else sRGB) into the working space; the top-level comp output is converted from
//!   the working space to the sRGB display. Without a working space nothing is converted.
//! * **Linearize Working Space**: the working space is linear light, so sources (footage and
//!   the colours of solids, text and shapes, which are authored in the working space's encoding)
//!   are linearised before masks and effects, and everything runs in linear.
//! * **Blend Colors Using 1.0 Gamma**: only blending is linear. Each layer's finished pixels
//!   are linearised just before they are transformed and blended, the comp accumulates in linear
//!   and is encoded back to the working space when the comp is done (so precomps hand encoded
//!   pixels to their parent). Without a working space the Working Gamma's curve is used (sRGB
//!   at 2.2, a pure 2.4 power curve at 2.4); so is it for Interpret As Linear Light footage
//!   and for float footage (OpenEXR, float TIFF), whose linear values are encoded once
//!   ([`Pipe::float_curve`]).

use effectcraft_color::{ColorSpace, Conversion};
use effectcraft_project::{Footage, HdrMode, ProjectSettings};
use effectcraft_raster::{Image, quantize_px};
use rayon::prelude::*;

/// The colour pipeline of a project (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pipe {
    /// Quantisation levels (255, 32768) or `None` for 32 bpc float.
    pub levels: Option<f32>,
    /// Working space (`None` = unmanaged).
    pub space: Option<ColorSpace>,
    /// The working space is linear.
    pub linear: bool,
    /// Blend in linear light (and the working space is not already linear).
    pub linear_blend: bool,
    /// The top-level output's space (sRGB unless Project Settings choose another).
    pub out: ColorSpace,
    /// HDR handling for standard-dynamic-range output.
    pub hdr: HdrMode,
    /// Unmanaged with Working Gamma 2.4: the pure 2.4 power curve instead of sRGB's.
    pub gamma24: bool,
}

impl Pipe {
    pub fn of(s: &ProjectSettings) -> Pipe {
        // ACES working spaces are linear light by definition.
        let linear = s.working_space.is_some_and(|w| s.linearize || w.is_linear());
        Pipe {
            levels: s.bit_depth.levels(),
            space: s.working_space,
            linear,
            linear_blend: s.blend_linear && !linear,
            out: s.output_space.unwrap_or(ColorSpace::Srgb),
            hdr: s.hdr,
            gamma24: s.working_space.is_none() && s.working_gamma_24(),
        }
    }

    /// The space whose curve encodes working-space pixels (unmanaged: the Working Gamma's, sRGB
    /// at 2.2 and Rec. 709's pure 2.4 power curve at 2.4).
    fn curve(&self) -> ColorSpace {
        self.space.unwrap_or(if self.gamma24 { ColorSpace::Rec709 } else { ColorSpace::Srgb })
    }

    /// The space Interpret As Linear Light encodes footage `f` in: its colour profile, else sRGB
    /// (managed; the working space converts from there) or the Working Gamma's curve (unmanaged).
    pub fn linear_footage_space(&self, f: &Footage) -> ColorSpace {
        f.color_profile.unwrap_or(if self.space.is_some() { ColorSpace::Srgb } else { self.curve() })
    }

    /// How float footage `f` (OpenEXR, float TIFF; linear light) is decoded: as stored when
    /// Interpret As Linear Light encodes it afterwards (once, with
    /// [`Pipe::linear_footage_space`]), else with the Working Gamma's curve when unmanaged and
    /// the sRGB curve otherwise (the working space converts from there).
    pub fn float_curve(&self, f: &Footage) -> crate::FloatCurve {
        if f.linear_light_applies() { crate::FloatCurve::Linear } else { self.float_encoding() }
    }

    /// The curve float footage is encoded with when nothing interprets it afterwards: the
    /// Working Gamma's when unmanaged (sRGB at 2.2, a pure 2.4 power curve at 2.4), else sRGB.
    pub fn float_encoding(&self) -> crate::FloatCurve {
        if self.space.is_none() && self.gamma24 { crate::FloatCurve::Gamma24 } else { crate::FloatCurve::Srgb }
    }

    /// Footage `f` from its colour profile (`None` = sRGB) → working space; nothing for
    /// Preserve RGB, whose values enter the working space as the file stores them.
    pub fn media_in(&self, f: &Footage) -> Option<Conversion> {
        let ws = self.space.filter(|_| !f.preserve_rgb)?;
        Conversion::new(f.color_profile.unwrap_or(ColorSpace::Srgb), false, ws, self.linear)
    }

    /// Authored colours (solids, text, shapes) → working space: linearised in a linear working
    /// space.
    pub fn authored_in(&self) -> Option<Conversion> {
        self.linear.then(|| Conversion::linearize(self.curve()))
    }

    /// Working space → blending space (linear when blending with 1.0 gamma).
    pub fn to_blend(&self) -> Option<Conversion> {
        self.linear_blend.then(|| Conversion::linearize(self.curve()))
    }

    /// Blending space → working space.
    pub fn from_blend(&self) -> Option<Conversion> {
        self.linear_blend.then(|| Conversion::delinearize(self.curve()))
    }

    /// Working space → the output space (top-level comp output; sRGB by default). `None` when
    /// unmanaged or when [`Pipe::output_hdr`] applies.
    pub fn output(&self) -> Option<Conversion> {
        let ws = self.space?;
        if self.output_hdr().is_some() {
            return None;
        }
        Conversion::new(ws, self.linear, self.out, false)
    }

    /// Output with HDR compand / tone mapping (standard-dynamic-range outputs only): working
    /// space → linear output primaries, the HDR curve, then the output's encoding.
    pub fn output_hdr(&self) -> Option<(Option<Conversion>, HdrMode, Conversion)> {
        let ws = self.space?;
        (self.hdr != HdrMode::Clip && !self.out.is_hdr())
            .then(|| (Conversion::new(ws, self.linear, self.out, true), self.hdr, Conversion::delinearize(self.out)))
    }

    /// One pixel clamped and quantised to the bit depth.
    pub fn quantize_px(&self, mut p: [f32; 4]) -> [f32; 4] {
        if let Some(l) = self.levels {
            quantize_px(&mut p, l, 1.0 / l);
        }
        p
    }

    /// Clamp and quantise to the bit depth (no-op in 32 bpc).
    pub fn quantize(&self, img: &mut Image) {
        self.quantize_region(img, Region::Full);
    }

    /// Clamp and quantise the pixels of `region` (no-op in 32 bpc). Pixels outside it must
    /// already be quantised (checked in debug builds).
    pub fn quantize_region(&self, img: &mut Image, region: Region) {
        let Some(l) = self.levels else { return };
        let r = match region {
            Region::Empty => None,
            Region::Full => Some(PxRect { x0: 0, y0: 0, x1: img.width, y1: img.height }),
            Region::Rect(r) => Some(r.clamp_to(img.width, img.height)),
        };
        if let Some(r) = r {
            quantize_rect(img, r, l);
        }
        #[cfg(debug_assertions)]
        if img.data.len() <= 1 << 20 {
            let inv = 1.0 / l;
            let bad = img.data.iter().position(|p| {
                let q = |v: f32, hi: f32| (v.clamp(0.0, hi) * l).round() * inv;
                let a = q(p[3], 1.0);
                (p[3] - a).abs() > 1e-5 || (0..3).any(|c| (p[c] - q(p[c], a)).abs() > 1e-5)
            });
            if let Some(i) = bad {
                debug_assert!(
                    false,
                    "pixel ({}, {}) outside the quantised region {region:?} is not quantised: {:?}",
                    i as u32 % img.width,
                    i as u32 / img.width,
                    img.data[i]
                );
            }
        }
    }

    /// Hash of everything that changes pixels (for cache keys).
    pub fn key(&self) -> u64 {
        let mut k = self.levels.map_or(0, |l| l as u64);
        k = k.wrapping_mul(31).wrapping_add(self.space.map_or(7, |s| s as u64 + 11));
        k = k.wrapping_mul(31).wrapping_add(self.linear as u64 * 2 + self.linear_blend as u64);
        k = k.wrapping_mul(31).wrapping_add(self.out as u64 * 4 + self.hdr as u64);
        k = k.wrapping_mul(31).wrapping_add(self.gamma24 as u64);
        k
    }
}

/// A pixel rectangle `[x0, x1) × [y0, y1)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PxRect {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl PxRect {
    /// The pixels covering the real-valued box `[x0, x1] × [y0, y1]` grown by `margin`.
    pub fn covering(x0: f64, y0: f64, x1: f64, y1: f64, margin: f64) -> Option<PxRect> {
        if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
            return None;
        }
        let c = |v: f64| v.clamp(0.0, u32::MAX as f64) as u32;
        Some(PxRect { x0: c((x0 - margin).floor()), y0: c((y0 - margin).floor()), x1: c((x1 + margin).ceil()), y1: c((y1 + margin).ceil()) })
    }
    fn clamp_to(self, w: u32, h: u32) -> PxRect {
        let (x1, y1) = (self.x1.min(w), self.y1.min(h));
        PxRect { x0: self.x0.min(x1), y0: self.y0.min(y1), x1, y1 }
    }
}

/// The part of a canvas that a drawing step may have changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Empty,
    Rect(PxRect),
    Full,
}

/// Clamp premultiplied pixels to 0..1 (colour ≤ alpha) and round to `levels` steps.
pub fn quantize(img: &mut Image, levels: f32) {
    quantize_rect(img, PxRect { x0: 0, y0: 0, x1: img.width, y1: img.height }, levels);
}

/// [`quantize`] restricted to a rectangle (already clamped to the image), row-parallel.
fn quantize_rect(img: &mut Image, r: PxRect, levels: f32) {
    if r.x0 >= r.x1 || r.y0 >= r.y1 {
        return;
    }
    let inv = 1.0 / levels;
    let w = img.width as usize;
    let (x0, x1) = (r.x0 as usize, r.x1 as usize);
    img.data[r.y0 as usize * w..r.y1 as usize * w].par_chunks_mut(w).for_each(|row| {
        // Runs of all-zero pixels (+0.0 in every channel) quantise to themselves: skip them a
        // cache line at a time (sparse shape, precomp and glow buffers are mostly empty).
        let (blocks, rest) = row[x0..x1].as_chunks_mut::<4>();
        for b in blocks {
            if b.as_flattened().iter().fold(0, |a, v| a | v.to_bits()) != 0 {
                for p in b {
                    quantize_px(p, levels, inv);
                }
            }
        }
        for p in rest {
            quantize_px(p, levels, inv);
        }
    });
}

/// Convert premultiplied pixels (on straight colour).
pub fn convert(img: &mut Image, c: &Conversion) {
    img.data.par_iter_mut().for_each(|p| {
        let a = p[3];
        if a <= 0.0 {
            return;
        }
        let o = c.apply([p[0] / a, p[1] / a, p[2] / a]);
        *p = [o[0] * a, o[1] * a, o[2] * a, a];
    });
}

/// Compand: an exponential knee from 80%, approaching 1.0 (linear light, per channel).
pub fn compand(v: f32) -> f32 {
    const K: f32 = 0.8;
    if v <= K { v } else { K + (1.0 - K) * (1.0 - (-(v - K) / (1.0 - K)).exp()) }
}

/// Extended Reinhard tone mapping of luminance with white at `W` (linear light).
pub fn tone_map(c: [f32; 3]) -> [f32; 3] {
    const W: f32 = 4.0;
    let l = effectcraft_color::luminance(c[0], c[1], c[2]);
    if l <= 0.0 {
        return c;
    }
    let lm = l * (1.0 + l / (W * W)) / (1.0 + l);
    let k = lm / l;
    c.map(|v| v * k)
}

/// The HDR step of [`Pipe::output_hdr`] on premultiplied pixels.
pub fn output_hdr(img: &mut Image, to_linear: Option<Conversion>, mode: HdrMode, encode: Conversion) {
    img.data.par_iter_mut().for_each(|p| {
        let a = p[3];
        if a <= 0.0 {
            return;
        }
        let mut c = [p[0] / a, p[1] / a, p[2] / a];
        if let Some(t) = &to_linear {
            c = t.apply(c);
        }
        c = match mode {
            HdrMode::Clip => c,
            HdrMode::Compand => c.map(compand),
            HdrMode::ToneMap => tone_map(c),
        };
        let o = encode.apply(c);
        *p = [o[0] * a, o[1] * a, o[2] * a, a];
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Skipping all-zero runs leaves every pixel as quantising it one by one does, bit for bit
    /// (negative zeros and lone pixels in empty runs included).
    #[test]
    fn quantize_skips_only_zero_runs() {
        let mut img = Image::new(23, 7);
        for (i, p) in img.data.iter_mut().enumerate() {
            *p = match i % 11 {
                0 => [0.3, -0.2, 0.71, 0.9],
                3 => [-0.0, 0.0, 0.0, 0.0],
                5 => [0.0, 0.0, 0.0, -0.0],
                7 => [1.7, 0.4, f32::NAN, 0.5],
                _ => [0.0; 4],
            };
        }
        for levels in [255.0f32, 32_768.0] {
            let mut want = img.clone();
            for p in &mut want.data {
                quantize_px(p, levels, 1.0 / levels);
            }
            for r in [PxRect { x0: 0, y0: 0, x1: 23, y1: 7 }, PxRect { x0: 3, y0: 1, x1: 20, y1: 6 }] {
                let mut got = img.clone();
                quantize_rect(&mut got, r, levels);
                for (i, (g, p)) in got.data.iter().zip(&img.data).enumerate() {
                    let (x, y) = (i as u32 % 23, i as u32 / 23);
                    let w = if x >= r.x0 && x < r.x1 && y >= r.y0 && y < r.y1 { want.data[i] } else { *p };
                    assert_eq!(g.map(f32::to_bits), w.map(f32::to_bits), "{levels} {r:?} px ({x}, {y})");
                }
            }
        }
    }

    /// Both forms `round_half_up` picks between agree on everything quantising feeds it:
    /// 0..=levels, exact halves included.
    #[test]
    fn rounding_forms_agree() {
        for levels in [255.0f32, 32_768.0] {
            for i in 0..=(levels as u32 * 16) {
                let v = i as f32 / 16.0;
                assert_eq!((v + 0.5).floor(), (v + 0.5) as u32 as f32, "{v}");
            }
            for i in 0..=100_000 {
                let v = (i as f32 / 100_000.0) * levels;
                assert_eq!((v + 0.5).floor(), (v + 0.5) as u32 as f32, "{v}");
            }
        }
    }
}
