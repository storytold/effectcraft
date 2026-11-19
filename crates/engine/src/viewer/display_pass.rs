//! The viewer's display pass as plain data: what turns a rendered frame into the pixels the
//! Composition panel shows. The frontend runs it in a shader. The same steps on the CPU
//! ([`DisplayPass::pixel`]) are its fallback without a GPU and the reference the shader is tested
//! against, so both use the same `f32` operations in the same order.
//!
//! Steps, per pixel of a premultiplied frame encoded in the output space:
//! 1. Unpremultiply (NaN alpha counts as 0, NaN colour as 0).
//! 2. When exposure or an HDR step applies: decode with the frame's curve, multiply by 2^stops
//!    in linear light, run the pending HDR step (Project Settings ▸ HDR: Compand / Tone Map, for
//!    frames rendered without it), encode again.
//! 3. Clamp to 0..1: the output's numbers.
//! 4. The display colour conversion (Use Display Color Management, Simulate Output), on RGB and
//!    RGB Straight only.
//! 5. Show Channel, premultiply (without step 2 or a display conversion: the frame's own
//!    premultiplied values, clamped to alpha), and round to 8 bits with `floor(v · 255 + 0.5)`.
//!
//! Exposure therefore acts on the full float range of 32 bpc frames, before tone mapping and
//! before the display conversion. With neither exposure nor an HDR step, step 2 is skipped, so
//! 8-bit frames pass through unchanged.

use std::sync::Arc;

use effectcraft_color::icc::Lut3d;
use effectcraft_color::space;
use effectcraft_project::{HdrMode, ProjectSettings};
use effectcraft_render::color::Pipe;

use super::{Channel, Conv, Curve, DisplayColor};

/// A transfer curve of the display pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PassCurve {
    Linear,
    Srgb,
    /// A pure power curve (BT.1886 2.4, Mac 1.8, custom gammas).
    Gamma(f32),
    Pq,
    Hlg,
}

/// SMPTE ST 2084 and BT.2100 HLG constants (`effectcraft_color::space`), in `f32`.
const PQ_M1: f32 = 2610.0 / 16384.0;
const PQ_M2: f32 = 2523.0 / 4096.0 * 128.0;
const PQ_C1: f32 = 3424.0 / 4096.0;
const PQ_C2: f32 = 2413.0 / 4096.0 * 32.0;
const PQ_C3: f32 = 2392.0 / 4096.0 * 32.0;
/// Linear 1.0 as a share of PQ's 10 000 cd/m² (the 203 cd/m² reference white).
const PQ_WHITE: f32 = 203.0 / 10_000.0;
const HLG_A: f32 = 0.178_832_77;
const HLG_B: f32 = 0.284_668_92;
const HLG_C: f32 = 0.559_910_7;
const HLG_REFERENCE: f32 = 0.264_962_56;

/// The exponents the shader is handed (`vals2.yzw`): 1 / 2.4, 1 / PQ m2, 1 / PQ m1.
pub const SHADER_EXPONENTS: [f32; 3] = [1.0 / 2.4, 1.0 / PQ_M2, 1.0 / PQ_M1];

/// `a^g` for `a ≥ 0`, 0 at 0 (WGSL leaves `pow(0, g)` to the implementation).
fn power(a: f32, g: f32) -> f32 {
    if a <= 0.0 { 0.0 } else { a.powf(g) }
}

impl PassCurve {
    /// The curve a colour space encodes with.
    pub fn of(curve: space::Curve) -> PassCurve {
        match curve {
            space::Curve::Linear => PassCurve::Linear,
            space::Curve::Srgb => PassCurve::Srgb,
            space::Curve::Gamma24 => PassCurve::Gamma(2.4),
            space::Curve::Pq => PassCurve::Pq,
            space::Curve::Hlg => PassCurve::Hlg,
        }
    }

    fn of_viewer(c: Curve) -> PassCurve {
        match c {
            Curve::Srgb => PassCurve::Srgb,
            Curve::Gamma(g) => PassCurve::Gamma(g),
            Curve::Linear => PassCurve::Linear,
            Curve::Space(c) => PassCurve::of(c),
        }
    }

    /// The shader's curve id.
    pub fn id(self) -> u32 {
        match self {
            PassCurve::Linear => 0,
            PassCurve::Srgb => 1,
            PassCurve::Gamma(_) => 2,
            PassCurve::Pq => 3,
            PassCurve::Hlg => 4,
        }
    }

    /// The shader's parameter to decode with: the gamma (1 for other curves).
    pub fn decode_param(self) -> f32 {
        if let PassCurve::Gamma(g) = self { g } else { 1.0 }
    }

    /// The shader's parameter to encode with: 1 / the gamma, as [`PassCurve::encode`] divides.
    pub fn encode_param(self) -> f32 {
        if let PassCurve::Gamma(g) = self { 1.0 / g } else { 1.0 }
    }

    /// Encoded → linear light, mirrored around zero.
    pub fn decode(self, v: f32) -> f32 {
        let a = v.abs();
        let l = match self {
            PassCurve::Linear => a,
            PassCurve::Srgb => {
                if a <= 0.04045 {
                    a / 12.92
                } else {
                    power((a + 0.055) / 1.055, 2.4)
                }
            }
            PassCurve::Gamma(g) => power(a, g),
            PassCurve::Pq => {
                let p = power(a.min(1.0), 1.0 / PQ_M2);
                power((p - PQ_C1).max(0.0) / (PQ_C2 - PQ_C3 * p), 1.0 / PQ_M1) / PQ_WHITE
            }
            PassCurve::Hlg => {
                let e = if a <= 0.5 { a * a / 3.0 } else { (((a - HLG_C) / HLG_A).exp() + HLG_B) / 12.0 };
                e / HLG_REFERENCE
            }
        };
        l.copysign(v)
    }

    /// Linear light → encoded, mirrored around zero.
    pub fn encode(self, v: f32) -> f32 {
        let a = v.abs();
        let e = match self {
            PassCurve::Linear => a,
            PassCurve::Srgb => {
                if a <= 0.003_130_8 {
                    a * 12.92
                } else {
                    1.055 * power(a, 1.0 / 2.4) - 0.055
                }
            }
            PassCurve::Gamma(g) => power(a, 1.0 / g),
            PassCurve::Pq => {
                let y = power(a * PQ_WHITE, PQ_M1);
                power((PQ_C1 + PQ_C2 * y) / (1.0 + PQ_C3 * y), PQ_M2)
            }
            PassCurve::Hlg => {
                let e = a * HLG_REFERENCE;
                if e <= 1.0 / 12.0 { (3.0 * e).sqrt() } else { HLG_A * (12.0 * e - HLG_B).ln() + HLG_C }
            }
        };
        e.copysign(v)
    }
}

/// A conversion of the display colour: decode, an optional linear-light matrix, encode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PassConv {
    pub from: PassCurve,
    /// Rows of the linear RGB matrix (`None`: the same primaries).
    pub matrix: Option<[[f32; 3]; 3]>,
    pub to: PassCurve,
}

impl PassConv {
    fn of(c: &Conv) -> PassConv {
        PassConv { from: PassCurve::of_viewer(c.from), matrix: c.m, to: PassCurve::of_viewer(c.to) }
    }

    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let l = c.map(|v| self.from.decode(v));
        let l = match &self.matrix {
            Some(m) => [0, 1, 2].map(|i| m[i][0] * l[0] + m[i][1] * l[1] + m[i][2] * l[2]),
            None => l,
        };
        l.map(|v| self.to.encode(v))
    }
}

/// The display colour conversion ([`DisplayColor`]) as data for the display pass.
#[derive(Clone, Debug)]
pub enum PassColor {
    /// One conversion; with Simulate Output, the conversion to the simulated device, its 8-bit
    /// numbers (`quantize`) and the conversion from the device to the display.
    Convs { first: PassConv, quantize: bool, second: Option<PassConv> },
    /// A LUT-based My Custom RGB profile: the whole simulation baked into a 3D LUT.
    Lut(Arc<Lut3d>),
}

impl PassColor {
    /// One straight colour (0..1) to the display's numbers (0..1).
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let o = match self {
            PassColor::Lut(l) => lut_eval(l, c),
            PassColor::Convs { first, quantize, second } => {
                let mut o = first.apply(c);
                if *quantize {
                    o = o.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() / 255.0);
                }
                match second {
                    Some(s) => s.apply(o),
                    None => o,
                }
            }
        };
        o.map(|v| v.clamp(0.0, 1.0))
    }
}

/// [`Lut3d::eval`], written as the shader samples the LUT (the same products and sums).
pub fn lut_eval(l: &Lut3d, c: [f32; 3]) -> [f32; 3] {
    let n = l.size.max(2);
    let mut k = [0usize; 3];
    let mut f = [0f32; 3];
    for i in 0..3 {
        let x = c[i].clamp(0.0, 1.0) * (n - 1) as f32;
        k[i] = (x.floor() as usize).min(n - 2);
        f[i] = x - k[i] as f32;
    }
    let at = |r: usize, g: usize, b: usize| l.data.get((r * n + g) * n + b).copied().unwrap_or_default();
    let mut out = [0f32; 3];
    for corner in 0..8 {
        let (dr, dg, db) = (corner >> 2 & 1, corner >> 1 & 1, corner & 1);
        let w = (if dr == 1 { f[0] } else { 1.0 - f[0] }) * (if dg == 1 { f[1] } else { 1.0 - f[1] }) * (if db == 1 { f[2] } else { 1.0 - f[2] });
        let v = at(k[0] + dr, k[1] + dg, k[2] + db);
        for i in 0..3 {
            out[i] += w * v[i];
        }
    }
    out
}

impl DisplayColor {
    /// The conversion as data for the display pass.
    pub fn pass(&self) -> PassColor {
        if let Some(l) = &self.lut {
            return PassColor::Lut(l.clone());
        }
        match (&self.sim, &self.display) {
            (Some((to, show)), _) => PassColor::Convs { first: PassConv::of(to), quantize: true, second: show.as_ref().map(PassConv::of) },
            (None, Some(d)) => PassColor::Convs { first: PassConv::of(d), quantize: false, second: None },
            (None, None) => {
                PassColor::Convs { first: PassConv { from: PassCurve::Linear, matrix: None, to: PassCurve::Linear }, quantize: false, second: None }
            }
        }
    }
}

/// How a rendered frame's pixels are encoded: the curve of its output space, and the HDR step
/// (Project Settings ▸ HDR) it was rendered without and the display pass still runs (`Clip`:
/// none). Kept with the frame, so a later settings change doesn't reinterpret older frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameEncoding {
    pub curve: PassCurve,
    pub hdr: HdrMode,
}

impl FrameEncoding {
    /// A frame of a project with `settings`; `skipped_hdr_step`: rendered with
    /// `RenderOpts::skip_hdr_step`.
    pub fn of(settings: &ProjectSettings, skipped_hdr_step: bool) -> FrameEncoding {
        let pipe = Pipe::of(settings);
        let curve = pipe.output_linear().decode.map_or(PassCurve::Linear, |s| PassCurve::of(s.curve()));
        let hdr = if skipped_hdr_step && pipe.output_hdr().is_some() { settings.hdr } else { HdrMode::Clip };
        FrameEncoding { curve, hdr }
    }

    /// The frame as it would have been rendered with the HDR step: one straight colour.
    pub fn finish(&self, c: [f32; 3]) -> [f32; 3] {
        if self.hdr == HdrMode::Clip {
            return c;
        }
        let l = hdr_step(self.hdr, c.map(|v| self.curve.decode(v)));
        l.map(|v| self.curve.encode(v))
    }
}

/// Project Settings ▸ HDR on linear light (`effectcraft_render::color::output_hdr`).
pub fn hdr_step(mode: HdrMode, l: [f32; 3]) -> [f32; 3] {
    match mode {
        HdrMode::Clip => l,
        HdrMode::Compand => l.map(effectcraft_render::color::compand),
        HdrMode::ToneMap => effectcraft_render::color::tone_map(l),
    }
}

/// The whole display pass of one viewer: Show Channel, exposure, the frame's encoding and the
/// display colour conversion.
#[derive(Clone, Debug)]
pub struct DisplayPass {
    pub channel: Channel,
    pub colorized: bool,
    /// 2^stops of the viewer's exposure.
    pub gain: f32,
    pub encoding: FrameEncoding,
    pub color: Option<PassColor>,
}

impl DisplayPass {
    /// `stops` of exposure on frames encoded as `encoding`.
    pub fn new(channel: Channel, colorized: bool, stops: f32, encoding: FrameEncoding, color: Option<&DisplayColor>) -> DisplayPass {
        let gain = if stops == 0.0 || !stops.is_finite() { 1.0 } else { 2f32.powf(stops) };
        // The display conversion applies to the colour views only (as before).
        let color = color.filter(|_| matches!(channel, Channel::Rgb | Channel::RgbStraight)).map(DisplayColor::pass);
        DisplayPass { channel, colorized, gain, encoding, color }
    }

    /// Step 2 of the module docs runs.
    pub fn changes_light(&self) -> bool {
        self.gain != 1.0 || self.encoding.hdr != HdrMode::Clip
    }

    /// Nothing changes the frame's pixels (beyond clamping and rounding).
    pub fn is_identity(&self) -> bool {
        self.channel == Channel::Rgb && !self.changes_light() && self.color.is_none()
    }

    /// Step 2: exposure and the pending HDR step on straight colour.
    fn light(&self, c: [f32; 3]) -> [f32; 3] {
        if !self.changes_light() {
            return c;
        }
        let curve = self.encoding.curve;
        let l = hdr_step(self.encoding.hdr, c.map(|v| curve.decode(v) * self.gain));
        l.map(|v| curve.encode(v))
    }

    /// One premultiplied pixel (encoded in the output space) → premultiplied RGBA8 on screen.
    pub fn pixel(&self, p: [f32; 4]) -> [u8; 4] {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8;
        let a = if p[3].is_nan() { 0.0 } else { p[3].clamp(0.0, 1.0) };
        let un = |v: f32| if a > 0.0 && !v.is_nan() { v / a } else { 0.0 };
        let c = self.light([un(p[0]), un(p[1]), un(p[2])]).map(|v| v.clamp(0.0, 1.0));
        // A premultiplied channel: without exposure or HDR step straight from the frame (clamped
        // to alpha, as `display_pixel`), so nothing changes it by a division's rounding.
        let premul = |i: usize| if self.changes_light() || p[i].is_nan() { c[i] * a } else { p[i].clamp(0.0, a) };
        match self.channel {
            Channel::Rgb | Channel::RgbStraight => {
                let c = match &self.color {
                    Some(dc) => dc.apply(c),
                    None => c,
                };
                if self.channel == Channel::RgbStraight {
                    [q(c[0]), q(c[1]), q(c[2]), 255]
                } else if self.color.is_none() {
                    [q(premul(0)), q(premul(1)), q(premul(2)), q(a)]
                } else {
                    [q(c[0] * a), q(c[1] * a), q(c[2] * a), q(a)]
                }
            }
            Channel::Red | Channel::Green | Channel::Blue => {
                let i = match self.channel {
                    Channel::Red => 0,
                    Channel::Green => 1,
                    _ => 2,
                };
                let v = q(premul(i));
                if self.colorized {
                    let mut o = [0, 0, 0, 255];
                    o[i] = v;
                    o
                } else {
                    [v, v, v, 255]
                }
            }
            Channel::Alpha => {
                // Exposure lightens or darkens the alpha view as an encoded grey.
                let v = if self.gain != 1.0 { self.encoding.curve.encode(self.encoding.curve.decode(a) * self.gain) } else { a };
                let v = q(v);
                [v, v, v, 255]
            }
        }
    }

    /// The pass on premultiplied 8-bit pixels in place (the CPU path for 8-bit frames). Without
    /// a display conversion or HDR step each channel of an opaque pixel goes through the same
    /// curve on its own, so those pixels (and the Alpha view, which reads alpha only) look the
    /// result up in a table of [`DisplayPass::pixel`] (the browser has no threads to share
    /// the per-pixel work).
    pub fn apply_rgba8(&self, px: &mut [[u8; 4]]) {
        use rayon::prelude::*;
        if self.is_identity() {
            return;
        }
        let f = |v: u8| v as f32 / 255.0;
        if self.channel == Channel::Alpha {
            let t: [[u8; 4]; 256] = std::array::from_fn(|a| self.pixel([0.0, 0.0, 0.0, f(a as u8)]));
            px.par_iter_mut().for_each(|p| *p = t[p[3] as usize]);
            return;
        }
        let table = (self.color.is_none() && self.encoding.hdr == HdrMode::Clip)
            .then(|| -> [[u8; 4]; 256] { std::array::from_fn(|v| self.pixel([f(v as u8), f(v as u8), f(v as u8), 1.0])) });
        px.par_iter_mut().for_each(|p| {
            *p = match &table {
                Some(t) if p[3] == 255 => match self.channel {
                    Channel::Rgb | Channel::RgbStraight => [t[p[0] as usize][0], t[p[1] as usize][1], t[p[2] as usize][2], 255],
                    Channel::Red => t[p[0] as usize],
                    Channel::Green => t[p[1] as usize],
                    _ => t[p[2] as usize],
                },
                _ => self.pixel(p.map(f)),
            }
        });
    }
}
