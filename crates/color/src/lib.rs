//! Colour for EffectCraft: RGBA values, sRGB transfer, HSL/HSV, luminance, the label palette and
//! the 38 After-Effects-compatible blend modes.
//!
//! Blend-mode formulas come from the W3C *Compositing and Blending Level 1* specification and the
//! standard definitions of the "classic" photographic modes; they are applied to un-premultiplied
//! colour and composited with Porter-Duff source-over on premultiplied pixels.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod blend;
pub mod icc;
pub mod space;

pub use blend::{BlendMode, blend_pixel};
use serde::{Deserialize, Serialize};
pub use space::{ColorSpace, Conversion};

/// Straight (un-premultiplied) RGBA, 0..1 nominal (32 bpc may exceed).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Rgba {
    Rgba { r, g, b, a }
}

impl Rgba {
    pub const BLACK: Rgba = rgba(0.0, 0.0, 0.0, 1.0);
    pub const WHITE: Rgba = rgba(1.0, 1.0, 1.0, 1.0);
    pub const TRANSPARENT: Rgba = rgba(0.0, 0.0, 0.0, 0.0);

    pub fn from_u8(r: u8, g: u8, b: u8) -> Rgba {
        rgba(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
    }
    /// `#rrggbb` or `#rrggbbaa`.
    pub fn from_hex(s: &str) -> Option<Rgba> {
        let s = s.trim().trim_start_matches('#');
        let p = |i: usize| {
            let pair = s.get(i..i + 2)?;
            if !pair.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            u8::from_str_radix(pair, 16).ok()
        };
        match s.len() {
            6 => Some(Rgba::from_u8(p(0)?, p(2)?, p(4)?)),
            8 => Some(Rgba { a: p(6)? as f32 / 255.0, ..Rgba::from_u8(p(0)?, p(2)?, p(4)?) }),
            _ => None,
        }
    }
    pub fn to_hex(&self) -> String {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02X}{:02X}{:02X}", c(self.r), c(self.g), c(self.b))
    }
    pub fn premultiplied(&self) -> [f32; 4] {
        [self.r * self.a, self.g * self.a, self.b * self.a, self.a]
    }
    pub fn to_array(&self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }
    pub fn from_array(a: [f32; 4]) -> Rgba {
        rgba(a[0], a[1], a[2], a[3])
    }
    pub fn lerp(&self, o: &Rgba, t: f32) -> Rgba {
        rgba(self.r + (o.r - self.r) * t, self.g + (o.g - self.g) * t, self.b + (o.b - self.b) * t, self.a + (o.a - self.a) * t)
    }
    pub fn to_u8(&self) -> [u8; 4] {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        [c(self.r), c(self.g), c(self.b), c(self.a)]
    }
}

/// sRGB electro-optical transfer: encoded → linear.
pub fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}
/// Linear → sRGB encoded.
pub fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.max(0.0).powf(1.0 / 2.4) - 0.055 }
}

/// Rec. 709 luma weights (AE's luminance for luma mattes/keys).
pub fn luminance(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// RGB (0..1) → HSL with h in 0..1.
pub fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) * 0.5;
    if (max - min).abs() < 1e-7 {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (((g - b) / d + if g < b { 6.0 } else { 0.0 }) / 6.0).rem_euclid(1.0)
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    (h, s, l)
}

pub fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    if s <= 0.0 {
        return (l, l, l);
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |mut t: f32| {
        t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    (f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0))
}

pub fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d < 1e-7 {
        0.0
    } else if max == r {
        (((g - b) / d).rem_euclid(6.0) / 6.0).rem_euclid(1.0)
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    let s = if max > 0.0 { d / max } else { 0.0 };
    (h, s, max)
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let mut h6 = h.rem_euclid(1.0) * 6.0;
    // In f32, a tiny negative hue wraps to exactly 1.0; treat it as 0.
    if h6 >= 6.0 {
        h6 = 0.0;
    }
    let i = h6.floor();
    let f = h6 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i as i32 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    }
}

/// Item/layer label colours (16 slots + None), defaults as in the public Labels preferences.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Label {
    None,
    #[default]
    Red,
    Yellow,
    Aqua,
    Pink,
    Lavender,
    Peach,
    SeaFoam,
    Blue,
    Green,
    Purple,
    Orange,
    Brown,
    Fuchsia,
    Cyan,
    Sandstone,
    DarkGreen,
}

impl Label {
    pub const ALL: [Label; 17] = [
        Label::None,
        Label::Red,
        Label::Yellow,
        Label::Aqua,
        Label::Pink,
        Label::Lavender,
        Label::Peach,
        Label::SeaFoam,
        Label::Blue,
        Label::Green,
        Label::Purple,
        Label::Orange,
        Label::Brown,
        Label::Fuchsia,
        Label::Cyan,
        Label::Sandstone,
        Label::DarkGreen,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Label::None => "None",
            Label::Red => "Red",
            Label::Yellow => "Yellow",
            Label::Aqua => "Aqua",
            Label::Pink => "Pink",
            Label::Lavender => "Lavender",
            Label::Peach => "Peach",
            Label::SeaFoam => "Sea Foam",
            Label::Blue => "Blue",
            Label::Green => "Green",
            Label::Purple => "Purple",
            Label::Orange => "Orange",
            Label::Brown => "Brown",
            Label::Fuchsia => "Fuchsia",
            Label::Cyan => "Cyan",
            Label::Sandstone => "Sandstone",
            Label::DarkGreen => "Dark Green",
        }
    }
    pub fn from_name(s: &str) -> Option<Label> {
        Label::ALL.into_iter().find(|l| l.name().eq_ignore_ascii_case(s) || format!("{l:?}").eq_ignore_ascii_case(s))
    }
    /// sRGB 8-bit colour.
    pub fn rgb(self) -> [u8; 3] {
        match self {
            Label::None => [0x50, 0x50, 0x50],
            Label::Red => [0xb5, 0x38, 0x38],
            Label::Yellow => [0xe4, 0xd8, 0x4c],
            Label::Aqua => [0xa9, 0xcb, 0xc7],
            Label::Pink => [0xe5, 0xbc, 0xc9],
            Label::Lavender => [0xa9, 0xa9, 0xca],
            Label::Peach => [0xe7, 0xc1, 0x9e],
            Label::SeaFoam => [0xb3, 0xc7, 0xb3],
            Label::Blue => [0x67, 0x7d, 0xe0],
            Label::Green => [0x4a, 0xa4, 0x4c],
            Label::Purple => [0x8e, 0x2c, 0x9a],
            Label::Orange => [0xe8, 0x92, 0x0d],
            Label::Brown => [0x7f, 0x45, 0x2a],
            Label::Fuchsia => [0xf4, 0x6d, 0xd6],
            Label::Cyan => [0x3d, 0xa2, 0xa5],
            Label::Sandstone => [0xa8, 0x96, 0x77],
            Label::DarkGreen => [0x1e, 0x40, 0x1e],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_to_rgb_tiny_negative_hue_is_red() {
        let (r, g, b) = hsv_to_rgb(-1e-9, 1.0, 1.0);
        assert!(r > 0.99 && g < 0.01 && b < 0.01, "{r} {g} {b}");
    }

    #[test]
    fn hue_stays_below_one_for_tiny_blue() {
        let (h, _, _) = rgb_to_hsv(1.0, 0.0, 1e-9);
        assert!((0.0..1.0).contains(&h), "rgb_to_hsv hue {h} not in 0..1");
        let (h, _, _) = rgb_to_hsl(1.0, 0.0, 1e-9);
        assert!((0.0..1.0).contains(&h), "rgb_to_hsl hue {h} not in 0..1");
    }

    #[test]
    fn rejects_signed_hex_pairs() {
        assert!(Rgba::from_hex("#+1+2+3").is_none());
        assert!(Rgba::from_hex("#1+2+3+").is_none());
        assert!(Rgba::from_hex("#ff00+0ff").is_none());
        assert!(Rgba::from_hex("#a1B2c3").is_some());
    }

    #[test]
    fn srgb_roundtrip() {
        for i in 0..=100 {
            let v = i as f32 / 100.0;
            assert!((linear_to_srgb(srgb_to_linear(v)) - v).abs() < 1e-5);
        }
    }

    #[test]
    fn hsl_roundtrip() {
        for (r, g, b) in [(0.2, 0.4, 0.9), (1.0, 0.0, 0.0), (0.5, 0.5, 0.5), (0.9, 0.8, 0.1)] {
            let (h, s, l) = rgb_to_hsl(r, g, b);
            let (r2, g2, b2) = hsl_to_rgb(h, s, l);
            assert!((r - r2).abs() < 1e-5 && (g - g2).abs() < 1e-5 && (b - b2).abs() < 1e-5);
            let (h, s, v) = rgb_to_hsv(r, g, b);
            let (r3, g3, b3) = hsv_to_rgb(h, s, v);
            assert!((r - r3).abs() < 1e-5 && (g - g3).abs() < 1e-5 && (b - b3).abs() < 1e-5);
        }
    }

    #[test]
    fn hex() {
        let c = Rgba::from_hex("#2D8CEB").unwrap();
        assert_eq!(c.to_hex(), "#2D8CEB");
        assert!(Rgba::from_hex("zz").is_none());
    }
}
