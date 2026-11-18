//! Layer blend modes.

use serde::{Deserialize, Serialize};

use crate::luminance;

/// The blend modes of the Layer ▸ Blending Mode menu, in menu order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlendMode {
    #[default]
    Normal,
    Dissolve,
    DancingDissolve,
    Darken,
    Multiply,
    ColorBurn,
    ClassicColorBurn,
    LinearBurn,
    DarkerColor,
    Add,
    Lighten,
    Screen,
    ColorDodge,
    ClassicColorDodge,
    LinearDodge,
    LighterColor,
    Overlay,
    SoftLight,
    HardLight,
    LinearLight,
    VividLight,
    PinLight,
    HardMix,
    Difference,
    ClassicDifference,
    Exclusion,
    Subtract,
    Divide,
    Hue,
    Saturation,
    Color,
    Luminosity,
    StencilAlpha,
    StencilLuma,
    SilhouetteAlpha,
    SilhouetteLuma,
    AlphaAdd,
    LuminescentPremul,
}

impl BlendMode {
    pub const ALL: [BlendMode; 38] = [
        BlendMode::Normal,
        BlendMode::Dissolve,
        BlendMode::DancingDissolve,
        BlendMode::Darken,
        BlendMode::Multiply,
        BlendMode::ColorBurn,
        BlendMode::ClassicColorBurn,
        BlendMode::LinearBurn,
        BlendMode::DarkerColor,
        BlendMode::Add,
        BlendMode::Lighten,
        BlendMode::Screen,
        BlendMode::ColorDodge,
        BlendMode::ClassicColorDodge,
        BlendMode::LinearDodge,
        BlendMode::LighterColor,
        BlendMode::Overlay,
        BlendMode::SoftLight,
        BlendMode::HardLight,
        BlendMode::LinearLight,
        BlendMode::VividLight,
        BlendMode::PinLight,
        BlendMode::HardMix,
        BlendMode::Difference,
        BlendMode::ClassicDifference,
        BlendMode::Exclusion,
        BlendMode::Subtract,
        BlendMode::Divide,
        BlendMode::Hue,
        BlendMode::Saturation,
        BlendMode::Color,
        BlendMode::Luminosity,
        BlendMode::StencilAlpha,
        BlendMode::StencilLuma,
        BlendMode::SilhouetteAlpha,
        BlendMode::SilhouetteLuma,
        BlendMode::AlphaAdd,
        BlendMode::LuminescentPremul,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BlendMode::Normal => "Normal",
            BlendMode::Dissolve => "Dissolve",
            BlendMode::DancingDissolve => "Dancing Dissolve",
            BlendMode::Darken => "Darken",
            BlendMode::Multiply => "Multiply",
            BlendMode::ColorBurn => "Color Burn",
            BlendMode::ClassicColorBurn => "Classic Color Burn",
            BlendMode::LinearBurn => "Linear Burn",
            BlendMode::DarkerColor => "Darker Color",
            BlendMode::Add => "Add",
            BlendMode::Lighten => "Lighten",
            BlendMode::Screen => "Screen",
            BlendMode::ColorDodge => "Color Dodge",
            BlendMode::ClassicColorDodge => "Classic Color Dodge",
            BlendMode::LinearDodge => "Linear Dodge",
            BlendMode::LighterColor => "Lighter Color",
            BlendMode::Overlay => "Overlay",
            BlendMode::SoftLight => "Soft Light",
            BlendMode::HardLight => "Hard Light",
            BlendMode::LinearLight => "Linear Light",
            BlendMode::VividLight => "Vivid Light",
            BlendMode::PinLight => "Pin Light",
            BlendMode::HardMix => "Hard Mix",
            BlendMode::Difference => "Difference",
            BlendMode::ClassicDifference => "Classic Difference",
            BlendMode::Exclusion => "Exclusion",
            BlendMode::Subtract => "Subtract",
            BlendMode::Divide => "Divide",
            BlendMode::Hue => "Hue",
            BlendMode::Saturation => "Saturation",
            BlendMode::Color => "Color",
            BlendMode::Luminosity => "Luminosity",
            BlendMode::StencilAlpha => "Stencil Alpha",
            BlendMode::StencilLuma => "Stencil Luma",
            BlendMode::SilhouetteAlpha => "Silhouette Alpha",
            BlendMode::SilhouetteLuma => "Silhouette Luma",
            BlendMode::AlphaAdd => "Alpha Add",
            BlendMode::LuminescentPremul => "Luminescent Premul",
        }
    }

    pub fn from_name(s: &str) -> Option<BlendMode> {
        let k: String = s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_ascii_lowercase();
        BlendMode::ALL.into_iter().find(|m| m.label().chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_ascii_lowercase() == k)
    }

    /// Menu groups (a separator follows the last mode of each group).
    pub fn ends_group(self) -> bool {
        matches!(
            self,
            BlendMode::DancingDissolve
                | BlendMode::DarkerColor
                | BlendMode::LighterColor
                | BlendMode::HardMix
                | BlendMode::Divide
                | BlendMode::Luminosity
                | BlendMode::SilhouetteLuma
        )
    }

    /// Modes that act on everything below (the accumulated comp) rather than compositing on top.
    pub fn is_stencil(self) -> bool {
        matches!(self, BlendMode::StencilAlpha | BlendMode::StencilLuma | BlendMode::SilhouetteAlpha | BlendMode::SilhouetteLuma)
    }

    /// Modes whose formulas are defined for over-range (HDR, 32 bpc) values. The others clamp
    /// their inputs to 0..1 before blending, as After Effects documents for 32 bpc projects
    /// (in 8/16 bpc everything is already in range). Overlay, Exclusion, Luminosity and the
    /// Color Burns keep their plain formula above 1 in After Effects (#494).
    pub fn supports_hdr(self) -> bool {
        matches!(
            self,
            BlendMode::Normal
                | BlendMode::Dissolve
                | BlendMode::DancingDissolve
                | BlendMode::Darken
                | BlendMode::Multiply
                | BlendMode::ColorBurn
                | BlendMode::ClassicColorBurn
                | BlendMode::LinearBurn
                | BlendMode::DarkerColor
                | BlendMode::Add
                | BlendMode::Lighten
                | BlendMode::Screen
                | BlendMode::LinearDodge
                | BlendMode::LighterColor
                | BlendMode::Difference
                | BlendMode::ClassicDifference
                | BlendMode::Overlay
                | BlendMode::Exclusion
                | BlendMode::Luminosity
                | BlendMode::Subtract
                | BlendMode::Divide
                | BlendMode::StencilAlpha
                | BlendMode::StencilLuma
                | BlendMode::SilhouetteAlpha
                | BlendMode::SilhouetteLuma
                | BlendMode::AlphaAdd
                | BlendMode::LuminescentPremul
        )
    }

    /// The source colour a classic mode leaves the backdrop unchanged with. After Effects fades
    /// the source toward it by the layer's opacity before applying the classic formula at full
    /// strength, rather than mixing the full-strength result by opacity (#494).
    fn classic_neutral(self) -> Option<f32> {
        match self {
            BlendMode::ClassicColorDodge | BlendMode::ClassicDifference => Some(0.0),
            BlendMode::ClassicColorBurn => Some(1.0),
            _ => None,
        }
    }

    pub fn next(self) -> BlendMode {
        let i = BlendMode::ALL.iter().position(|m| *m == self).unwrap_or(0);
        BlendMode::ALL[(i + 1) % BlendMode::ALL.len()]
    }
    pub fn previous(self) -> BlendMode {
        let i = BlendMode::ALL.iter().position(|m| *m == self).unwrap_or(0);
        BlendMode::ALL[(i + BlendMode::ALL.len() - 1) % BlendMode::ALL.len()]
    }
}

fn burn(cb: f32, cs: f32) -> f32 {
    if cb >= 1.0 {
        1.0
    } else if cs <= 0.0 {
        0.0
    } else {
        1.0 - ((1.0 - cb) / cs).min(1.0)
    }
}
fn dodge(cb: f32, cs: f32) -> f32 {
    if cb <= 0.0 {
        0.0
    } else if cs >= 1.0 {
        1.0
    } else {
        (cb / (1.0 - cs)).min(1.0)
    }
}
fn hard_light(cb: f32, cs: f32) -> f32 {
    if cs <= 0.5 {
        cb * 2.0 * cs
    } else {
        let s = 2.0 * cs - 1.0;
        cb + s - cb * s
    }
}
fn soft_light(cb: f32, cs: f32) -> f32 {
    if cs <= 0.5 {
        cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb)
    } else {
        let d = if cb <= 0.25 { ((16.0 * cb - 12.0) * cb + 4.0) * cb } else { cb.max(0.0).sqrt() };
        cb + (2.0 * cs - 1.0) * (d - cb)
    }
}
fn vivid(cb: f32, cs: f32) -> f32 {
    if cs <= 0.5 { burn(cb, 2.0 * cs) } else { dodge(cb, 2.0 * (cs - 0.5)) }
}

fn separable(mode: BlendMode, cb: f32, cs: f32) -> f32 {
    match mode {
        BlendMode::Darken => cb.min(cs),
        BlendMode::Multiply => cb * cs,
        BlendMode::ColorBurn => burn(cb, cs),
        BlendMode::ClassicColorBurn => {
            // (negative results are kept, as in After Effects)
            if cs <= 0.0 { 0.0 } else { 1.0 - (1.0 - cb) / cs }
        }
        BlendMode::LinearBurn => (cb + cs - 1.0).max(0.0),
        BlendMode::Add | BlendMode::LinearDodge => cb + cs,
        BlendMode::Lighten => cb.max(cs),
        BlendMode::Screen => cb + cs - cb * cs,
        BlendMode::ColorDodge => dodge(cb, cs),
        BlendMode::ClassicColorDodge => {
            if cs >= 1.0 {
                1.0
            } else {
                (cb / (1.0 - cs)).min(1.0)
            }
        }
        BlendMode::Overlay => hard_light(cs, cb),
        BlendMode::SoftLight => soft_light(cb, cs),
        BlendMode::HardLight => hard_light(cb, cs),
        BlendMode::LinearLight => (cb + 2.0 * cs - 1.0).clamp(0.0, 1.0),
        BlendMode::VividLight => vivid(cb, cs),
        BlendMode::PinLight => {
            if cs <= 0.5 {
                cb.min(2.0 * cs)
            } else {
                cb.max(2.0 * cs - 1.0)
            }
        }
        BlendMode::HardMix => {
            if cb + cs >= 1.0 {
                1.0
            } else {
                0.0
            }
        }
        BlendMode::Difference | BlendMode::ClassicDifference => (cb - cs).abs(),
        BlendMode::Exclusion => cb + cs - 2.0 * cb * cs,
        BlendMode::Subtract => (cb - cs).max(0.0),
        BlendMode::Divide => {
            if cs <= 0.0 {
                if cb > 0.0 { 1.0 } else { 0.0 }
            } else {
                cb / cs
            }
        }
        _ => cs,
    }
}

fn lum(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}
fn clip_color(c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut o = c;
    if n < 0.0 {
        for v in &mut o {
            *v = l + (*v - l) * l / (l - n).max(1e-9);
        }
    }
    if x > 1.0 {
        for v in &mut o {
            *v = l + (*v - l) * (1.0 - l) / (x - l).max(1e-9);
        }
    }
    o
}
fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    clip_color([c[0] + d, c[1] + d, c[2] + d])
}
fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}
fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    let mx = c[0].max(c[1]).max(c[2]);
    let mn = c[0].min(c[1]).min(c[2]);
    if mx - mn <= 1e-9 {
        return [0.0; 3];
    }
    let f = |v: f32| (v - mn) * s / (mx - mn);
    [f(c[0]), f(c[1]), f(c[2])]
}

fn non_separable(mode: BlendMode, cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
    match mode {
        BlendMode::Hue => set_lum(set_sat(cs, sat(cb)), lum(cb)),
        BlendMode::Saturation => set_lum(set_sat(cb, sat(cs)), lum(cb)),
        BlendMode::Color => set_lum(cs, lum(cb)),
        BlendMode::Luminosity => set_lum(cb, lum(cs)),
        BlendMode::DarkerColor => {
            if luminance(cs[0], cs[1], cs[2]) < luminance(cb[0], cb[1], cb[2]) {
                cs
            } else {
                cb
            }
        }
        BlendMode::LighterColor => {
            if luminance(cs[0], cs[1], cs[2]) > luminance(cb[0], cb[1], cb[2]) {
                cs
            } else {
                cb
            }
        }
        _ => [separable(mode, cb[0], cs[0]), separable(mode, cb[1], cs[1]), separable(mode, cb[2], cs[2])],
    }
}

/// Composite `src` onto `dst` with Preserve Underlying Transparency: the layer is blended as
/// usual and the result keeps its colour at the alpha below. After Effects 2026 lab renders: an
/// opaque layer over a 30 % background comes out in the layer's colour at 30 %; a white layer at
/// 50 % over (0.2, 0.4, 0.6) at 30 % gives (208, 220, 232) at alpha 77 (the plain composite's
/// colour). Stencil and silhouette modes still cut the alpha below as without the switch (lab
/// `pt_stencil_iso`: Stencil Alpha at 50 % over 30 % leaves 15 %; lab `pt_stencil`: it clears
/// where the layer has no pixels). Where nothing lies below, or the blend leaves nothing, `dst`
/// stays as it is. `src` must already include layer opacity, as for [`blend_pixel`].
#[inline]
pub fn blend_pixel_preserve(mode: BlendMode, dst: [f32; 4], src: [f32; 4], noise: f32) -> [f32; 4] {
    let a = dst[3];
    if a.is_nan() || a <= 0.0 {
        return dst;
    }
    let r = blend_pixel(mode, dst, src, noise);
    if mode.is_stencil() {
        return r;
    }
    let ra = r[3];
    if ra.is_nan() || ra <= 0.0 {
        return dst;
    }
    let k = a / ra;
    [r[0] * k, r[1] * k, r[2] * k, a]
}

/// Composite one premultiplied source pixel over a premultiplied destination pixel.
///
/// `src` must already include layer opacity. `noise` (0..1) is a per-pixel random value used by
/// Dissolve modes. Stencil/Silhouette modes return the destination modified by the source.
#[inline]
pub fn blend_pixel(mode: BlendMode, dst: [f32; 4], src: [f32; 4], noise: f32) -> [f32; 4] {
    let sa = src[3];
    let da = dst[3];
    match mode {
        BlendMode::Normal => {
            let k = 1.0 - sa;
            return [src[0] + dst[0] * k, src[1] + dst[1] * k, src[2] + dst[2] * k, sa + da * k];
        }
        BlendMode::Dissolve | BlendMode::DancingDissolve => {
            if sa <= 0.0 || noise >= sa {
                return dst;
            }
            let inv = 1.0 / sa;
            return [src[0] * inv, src[1] * inv, src[2] * inv, 1.0];
        }
        BlendMode::StencilAlpha => return dst.map(|v| v * sa),
        BlendMode::SilhouetteAlpha => return dst.map(|v| v * (1.0 - sa)),
        BlendMode::StencilLuma | BlendMode::SilhouetteLuma => {
            let l = luminance(src[0], src[1], src[2]);
            let k = if mode == BlendMode::StencilLuma { l } else { 1.0 - l };
            return dst.map(|v| v * k);
        }
        BlendMode::AlphaAdd => {
            return [src[0] + dst[0] * (1.0 - sa), src[1] + dst[1] * (1.0 - sa), src[2] + dst[2] * (1.0 - sa), (sa + da).min(1.0)];
        }
        BlendMode::LuminescentPremul => {
            return [src[0] + dst[0] * (1.0 - sa), src[1] + dst[1] * (1.0 - sa), src[2] + dst[2] * (1.0 - sa), sa + da - sa * da];
        }
        _ => {}
    }
    if sa <= 0.0 {
        return dst;
    }
    if da <= 0.0 {
        return src;
    }
    let mut cs = [src[0] / sa, src[1] / sa, src[2] / sa];
    let mut cb = [dst[0] / da, dst[1] / da, dst[2] / da];
    if !mode.supports_hdr() {
        cs = cs.map(|v| v.clamp(0.0, 1.0));
        cb = cb.map(|v| v.clamp(0.0, 1.0));
    }
    let ao = sa + da - sa * da;
    let mut o = [0.0f32; 4];
    if let Some(neutral) = mode.classic_neutral() {
        // Fade the source toward the neutral colour by its coverage, then apply the formula.
        let b = non_separable(mode, cb, cs.map(|v| neutral + sa * (v - neutral)));
        for i in 0..3 {
            o[i] = (1.0 - da) * src[i] + da * b[i];
        }
    } else {
        let b = non_separable(mode, cb, cs);
        for i in 0..3 {
            o[i] = (1.0 - da) * src[i] + (1.0 - sa) * dst[i] + sa * da * b[i];
        }
    }
    o[3] = ao;
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opaque(v: f32) -> [f32; 4] {
        [v, v, v, 1.0]
    }

    #[test]
    fn opaque_formulas() {
        let d = opaque(0.6);
        let s = opaque(0.3);
        let r = |m| blend_pixel(m, d, s, 0.5)[0];
        assert!((r(BlendMode::Normal) - 0.3).abs() < 1e-6);
        assert!((r(BlendMode::Multiply) - 0.18).abs() < 1e-6);
        assert!((r(BlendMode::Screen) - (0.6 + 0.3 - 0.18)).abs() < 1e-6);
        assert!((r(BlendMode::Add) - 0.9).abs() < 1e-6);
        assert!((r(BlendMode::Difference) - 0.3).abs() < 1e-6);
        assert!((r(BlendMode::Darken) - 0.3).abs() < 1e-6);
        assert!((r(BlendMode::Lighten) - 0.6).abs() < 1e-6);
        assert!((r(BlendMode::Subtract) - 0.3).abs() < 1e-6);
        assert!((r(BlendMode::Divide) - 1.0).abs() < 1e-6 || r(BlendMode::Divide) > 1.0);
        assert!((r(BlendMode::LinearBurn) - 0.0).abs() < 1e-6);
        assert!((r(BlendMode::Exclusion) - (0.9 - 0.36)).abs() < 1e-6);
        // Overlay with base 0.6 (>0.5): screen(0.3, 2*0.6-1)
        assert!((r(BlendMode::Overlay) - (0.3 + 0.2 - 0.06)).abs() < 1e-6);
    }

    #[test]
    fn transparent_source_keeps_destination() {
        let d = [0.2, 0.3, 0.4, 0.8];
        for m in BlendMode::ALL {
            if m.is_stencil() {
                continue;
            }
            let o = blend_pixel(m, d, [0.0; 4], 0.5);
            assert!((0..4).all(|i| (o[i] - d[i]).abs() < 1e-6), "{m:?} {o:?}");
        }
    }

    #[test]
    fn source_over_transparent_destination() {
        let s = [0.25, 0.1, 0.05, 0.5];
        for m in [BlendMode::Normal, BlendMode::Multiply, BlendMode::Screen, BlendMode::Hue, BlendMode::Overlay] {
            let o = blend_pixel(m, [0.0; 4], s, 0.9);
            assert!((0..4).all(|i| (o[i] - s[i]).abs() < 1e-6), "{m:?}");
        }
    }

    #[test]
    fn stencil() {
        let d = [0.5, 0.5, 0.5, 1.0];
        assert_eq!(blend_pixel(BlendMode::StencilAlpha, d, [0.0, 0.0, 0.0, 0.0], 0.0), [0.0; 4]);
        assert_eq!(blend_pixel(BlendMode::SilhouetteAlpha, d, [0.0, 0.0, 0.0, 0.0], 0.0), d);
    }

    #[test]
    fn hdr_modes() {
        // Add and Screen keep over-range values (32 bpc); Soft Light clamps its inputs.
        let d = [2.0, 2.0, 2.0, 1.0];
        let s = [0.5, 0.5, 0.5, 1.0];
        assert!((blend_pixel(BlendMode::Add, d, s, 0.5)[0] - 2.5).abs() < 1e-6);
        assert!((blend_pixel(BlendMode::Screen, d, s, 0.5)[0] - 1.5).abs() < 1e-6);
        assert!(blend_pixel(BlendMode::SoftLight, d, s, 0.5)[0] <= 1.0);
    }

    /// #494: After Effects' 32 bpc formulas, measured by the reporter. b = bottom, o = top,
    /// k = opacity (premultiplied into the source, as the compositor passes it).
    #[test]
    fn after_effects_32bpc_formulas() {
        let at = |m, b: f32, o: f32, k: f32| blend_pixel(m, opaque(b), [o * k, o * k, o * k, k], 0.5)[0];
        let close = |a: f32, e: f32| (a - e).abs() < 1e-5;
        // Classic modes fade the source toward their neutral colour first.
        for (b, o) in [(0.4, 0.5), (0.8, 0.9), (0.1, 0.3)] {
            let k = 0.15;
            assert!(close(at(BlendMode::ClassicColorDodge, b, o, k), (b / (1.0 - k * o)).min(1.0)));
            assert!(close(at(BlendMode::ClassicDifference, b, o, k), (b - k * o).abs()));
            assert!(close(at(BlendMode::ClassicColorBurn, b, o, k), 1.0 - (1.0 - b) / (1.0 - k * (1.0 - o))));
        }
        // At full opacity the classic formulas are unchanged.
        assert!(close(at(BlendMode::ClassicDifference, 0.6, 0.3, 1.0), 0.3));
        // Over-range top layers keep the plain formulas.
        let (b, o) = (0.7, 3.0);
        assert!(close(at(BlendMode::Exclusion, b, o, 1.0), b + o - 2.0 * b * o));
        let s = 2.0 * b - 1.0;
        assert!(close(at(BlendMode::Overlay, b, o, 1.0), o + s - o * s));
        assert!(close(at(BlendMode::ColorBurn, 0.0, 1.35, 0.15), 0.15 * (1.0 - 1.0 / 1.35)));
        assert!(close(at(BlendMode::ClassicColorBurn, 0.0, 0.1, 1.0), -9.0), "negative results are kept");
        // Luminosity: SetLum + ClipColor on the unclamped top layer.
        let l = blend_pixel(BlendMode::Luminosity, [0.2, 0.4, 0.6, 1.0], [2.0, 2.0, 2.0, 1.0], 0.5);
        let e = set_lum([0.2, 0.4, 0.6], 2.0);
        assert!((0..3).all(|i| close(l[i], e[i])), "{l:?} {e:?}");
        assert!((l[0] - 1.0).abs() > 0.1, "not the white of a top layer clamped to 1: {l:?}");
    }

    #[test]
    fn names_roundtrip() {
        for m in BlendMode::ALL {
            assert_eq!(BlendMode::from_name(m.label()), Some(m));
        }
        assert_eq!(BlendMode::from_name("soft-light"), Some(BlendMode::SoftLight));
    }
}

#[cfg(test)]
mod preserve_tests {
    use super::*;

    fn near(got: [f32; 4], want: [f32; 4]) -> bool {
        got.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-4)
    }

    #[test]
    fn preserve_keeps_the_alpha_below() {
        // Opaque red over blue at 30 %: red at 30 %.
        let r = blend_pixel_preserve(BlendMode::Normal, [0.0, 0.0, 0.3, 0.3], [1.0, 0.0, 0.0, 1.0], 0.5);
        assert!(near(r, [0.3, 0.0, 0.0, 0.3]), "{r:?}");
        // Nothing below: unchanged.
        assert_eq!(blend_pixel_preserve(BlendMode::Normal, [0.0; 4], [1.0, 0.0, 0.0, 1.0], 0.5), [0.0; 4]);
        // Opaque multiply onto white at 50 %: the layer's colour at 50 %.
        let r = blend_pixel_preserve(BlendMode::Multiply, [0.5, 0.5, 0.5, 0.5], [0.2, 0.4, 0.6, 1.0], 0.5);
        assert!(near(r, [0.1, 0.2, 0.3, 0.5]), "{r:?}");
    }

    #[test]
    fn preserve_keeps_the_colour_of_the_plain_composite() {
        // After Effects 2026 (lab `pt_stencil_iso`, the Normal control): white at 50 % over
        // (0.2, 0.4, 0.6) at 30 % gives (208, 220, 232) at alpha 77, the colour of the plain
        // composite at the alpha below. Mixing by the layer opacity alone gave (153, 178, 204).
        let dst = [0.2 * 0.3, 0.4 * 0.3, 0.6 * 0.3, 0.3];
        let r = blend_pixel_preserve(BlendMode::Normal, dst, [0.5, 0.5, 0.5, 0.5], 0.5);
        // 207.9, 219.7, 231.5: within one level of the 8-bit render.
        let straight = [r[0] / r[3], r[1] / r[3], r[2] / r[3]].map(|v| v * 255.0);
        assert!(straight.iter().zip([208.0, 220.0, 232.0]).all(|(a, b)| (a - b).abs() <= 1.0), "{straight:?}");
        assert!((r[3] - 0.3).abs() < 1e-6, "{r:?}");
    }

    #[test]
    fn preserve_leaves_stencil_and_silhouette_modes_cutting_the_alpha_below() {
        // After Effects 2026 (lab `pt_stencil_iso`): a white layer at 50 % in Stencil Alpha,
        // Silhouette Alpha or Stencil Luma mode with the switch on leaves 39, 38 and 39 of 77
        // (about 30 % × 50 %); lab `pt_stencil`: a stencil clears where it has no pixels. The
        // switch changes nothing for these modes.
        let dst = [0.06, 0.12, 0.18, 0.3];
        for mode in [BlendMode::StencilAlpha, BlendMode::SilhouetteAlpha, BlendMode::StencilLuma, BlendMode::SilhouetteLuma] {
            for src in [[0.0; 4], [0.5, 0.5, 0.5, 0.5], [1.0; 4]] {
                let (got, want) = (blend_pixel_preserve(mode, dst, src, 0.5), blend_pixel(mode, dst, src, 0.5));
                assert!(near(got, want), "{mode:?} {src:?}: {got:?}, want {want:?}");
            }
            let a = blend_pixel_preserve(mode, dst, [0.5, 0.5, 0.5, 0.5], 0.5)[3];
            assert!((a - 0.15).abs() < 1e-6, "{mode:?}: {a}");
        }
        assert_eq!(blend_pixel_preserve(BlendMode::StencilAlpha, dst, [0.0; 4], 0.5)[3], 0.0, "cut where the layer has no pixels");
    }
}
