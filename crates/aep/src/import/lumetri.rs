//! Lumetri Color (`ADBE Lumetri`) to EffectCraft's Lumetri Color (`ec.color.lumetri`).
//!
//! The parameters are found by their match name (`ADBE Lumetri-0011` is Exposure), not by their
//! place in `parT`: After Effects has added parameters in the middle of the list over the
//! versions (`-0127` ... `-0130` sit between older ones), and several sliders share a valid range
//! (Contrast, Highlights, Shadows, Whites and Blacks are all -150..150), so a shifted index
//! would map silently to the wrong control. Evidence: the 1200 samples (After Effects 2026,
//! `docs/aep-format.md` 3.7).
//!
//! - **Values in any record state.** Lumetri writes the values of its hidden controls (High
//!   Dynamic Range, HDR Specular, the second Look popup, ...) in records in the default state
//!   (`tdsb` 3); After Effects reads them back as set, so every record counts here
//!   ([`EffectParam::recorded_value`]).
//! - **Section switches.** Each section (Basic Correction, Creative, Curves, RGB Curves, the five
//!   hue / saturation curves, Color Wheels, HSL Secondary, Vignette) has an on / off checkbox in
//!   After Effects; EffectCraft's Lumetri has none. A section that is off does nothing in After
//!   Effects (1200 `lumetri-03-sec-basic-correction`, `-17-sec-vignette` render as `-00-defaults`),
//!   so its values are not imported (the effect looks the same) and are listed in the report.
//! - **Arbitrary data** (`ParamKind::ArbitraryData`, every instance stores it). Curves are a
//!   curve type, a point count and points as `f64` x, y ([`curve_points`]).
//!   - The RGB curve (`-0039`) holds four curves; the first is the master (white) curve (1200
//!     `lumetri-60-preset-curves-rgb`), imported. The other three are reported when moved: no
//!     sample tells which is which channel.
//!   - Hue vs Sat (`-0042`): y is an offset from neutral, 1 doubling the saturation (`-61`), so
//!     EffectCraft's y is `0.5 + y / 2`. The other four hue / saturation curves have no sample
//!     of their scale and are reported when moved.
//!   - Color Wheels (`-0047`): Shadows, Midtones, Highlights and a fourth wheel, each hue,
//!     brightness slider, distance ([`import_wheels`], `-62`).
//!   - The HSL Secondary key ([`HslKey`]) is imported; Split Toning and the correction wheels are
//!     reported when they are not the default (not sampled).
//! - **LUTs and looks.** The Input LUT and the Look are files After Effects names (the Look's name
//!   is UTF-16 text in `-0096`, the popups only index a list of the installed ones); they are not
//!   in the project, so a set one is reported by name and EffectCraft's popup stays None.
//! - **Eyedropper colours** (White Balance, the hue curve selectors, the key's Set / Add /
//!   Remove colours) only set other controls in After Effects; they are reported when set.
//! - The `Blob` (`-0001`, an XML copy of the grade After Effects writes when a Look changes and does
//!   not keep current: 1200 `-01-set-all` holds the defaults of controls set after the Look) and
//!   the Auto Tone analysis (`-0121`, `-0122`) are derived data and not read.

use crate::effect_map::{Conv, ParamMap};
use crate::{Effect as AepEffect, EffectParam, ParamKind, ParamValue};
use effectcraft_keyframe::Value;

use crate::import::effects::{LayerCtx, ParamOut, carries_nothing, import_mapped, param_label};
use crate::import::report::LossCategory;
use ParamKind as K;

/// After Effects' Show Mask styles (`-0075`, `pdnm`), EffectCraft's in the same order.
pub const MASK_STYLE: &[&str] = &["Color/Gray", "Color/Black", "White/Black"];

/// A part of the effect with its own on / off switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    /// No switch (High Dynamic Range and the controls outside the sections).
    Always,
    BasicCorrection,
    Creative,
    RgbCurves,
    /// The five hue / saturation curves (0 Hue vs Sat ... 4 Sat vs Sat).
    HueSat(u8),
    ColorWheels,
    HslSecondary,
    Vignette,
    SpeedGradeCustom,
}

impl Section {
    fn name(self) -> &'static str {
        match self {
            Section::Always => "",
            Section::BasicCorrection => "Basic Correction",
            Section::Creative => "Creative",
            Section::RgbCurves => "RGB Curves",
            Section::HueSat(0) => "Hue vs Sat",
            Section::HueSat(1) => "Hue vs Hue",
            Section::HueSat(2) => "Hue vs Luma",
            Section::HueSat(3) => "Luma vs Sat",
            Section::HueSat(_) => "Sat vs Sat",
            Section::ColorWheels => "Color Wheels",
            Section::HslSecondary => "HSL Secondary",
            Section::Vignette => "Vignette",
            Section::SpeedGradeCustom => "SpeedGrade Custom",
        }
    }
}

/// Arbitrary data the importer reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Data {
    /// The four RGB curves (4 x 520 bytes).
    RgbCurves,
    /// One hue / saturation curve (520 bytes); the EffectCraft parameter when the scale of its y
    /// is known (Hue vs Sat).
    HueSatCurve(Option<&'static str>),
    /// Color Wheels (4 x 3 `f64`, each wheel 0, 0.5, 0 by default).
    Wheels,
    /// HSL Secondary's correction wheels (8 x 3 `f64`, the same default).
    CorrectionWheels,
    /// Split Toning (32 zero bytes by default).
    SplitToning,
    /// HSL Secondary's key ([`HslKey`]).
    HslKey,
    /// Data without a counterpart, empty by default (`fe fe` or zero bytes).
    Other,
}

/// What a Lumetri parameter is.
#[derive(Clone, Copy, Debug)]
enum Role {
    /// An EffectCraft parameter and the conversion.
    To(&'static str, Conv),
    /// The on / off switch of a section.
    Switch,
    /// No counterpart: reported when not at the default.
    Missing,
    /// A checkbox without counterpart whose `pard` default is not reliable: reported when a
    /// record says on. Legacy Tone And Saturation (`-0097`) is off in a fresh instance, but its
    /// `pard` default reads off in 1200 as saved by the script and on after the project was
    /// opened and saved again (`versions/1200-lumetri-26`, `-25x`, `-24x`).
    MissingWhenOn,
    /// An eyedropper colour: reported when set.
    Eyedropper,
    /// The Input LUT (`lut` true) or the Look: a popup, a name or an asset.
    Lut {
        lut: bool,
    },
    /// The Look's colour space (its popup and the space's id): goes with the Look.
    LookSpace,
    Data(Data),
    /// Derived data, not read.
    Derived,
}

struct Row {
    suffix: u16,
    section: Section,
    kind: Option<ParamKind>,
    role: Role,
}

const fn row(suffix: u16, section: Section, kind: Option<ParamKind>, role: Role) -> Row {
    Row { suffix, section, kind, role }
}

const fn to(suffix: u16, section: Section, kind: ParamKind, target: &'static str, conv: Conv) -> Row {
    row(suffix, section, Some(kind), Role::To(target, conv))
}

const fn plain(valid_max: f64) -> Conv {
    Conv::Plain { valid_max }
}

use Section as S;

/// Every Lumetri parameter by match-name suffix (1200: `dump.props` and the `pard` of
/// `lumetri-00-defaults`). The valid maxima are After Effects 2026's; the stored numbers are the
/// values After Effects shows (read back in every variant).
const ROWS: &[Row] = &[
    to(62, S::Always, K::Checkbox, "highDynamicRange", Conv::Checkbox),
    // Basic Correction: 3 the switch; 4 / 95 the Input LUT's name data, 5 / 127 its popups.
    row(3, S::BasicCorrection, Some(K::Checkbox), Role::Switch),
    row(4, S::BasicCorrection, Some(K::ArbitraryData), Role::Lut { lut: true }),
    row(95, S::BasicCorrection, Some(K::ArbitraryData), Role::Lut { lut: true }),
    row(5, S::BasicCorrection, Some(K::Popup), Role::Lut { lut: true }),
    row(127, S::BasicCorrection, Some(K::Popup), Role::Lut { lut: true }),
    to(63, S::BasicCorrection, K::FloatSlider, "basicCorrection/tone/hdrWhite", plain(1000.0)),
    // The Auto tone's Intensity (next to the Auto and Reset buttons 120, 119).
    row(123, S::BasicCorrection, Some(K::FloatSlider), Role::Missing),
    row(94, S::BasicCorrection, Some(K::Color), Role::Eyedropper),
    to(7, S::BasicCorrection, K::FloatSlider, "basicCorrection/whiteBalance/temperature", plain(300.0)),
    to(8, S::BasicCorrection, K::FloatSlider, "basicCorrection/whiteBalance/tint", plain(300.0)),
    to(20, S::BasicCorrection, K::FloatSlider, "basicCorrection/saturation", plain(300.0)),
    to(11, S::BasicCorrection, K::FloatSlider, "basicCorrection/tone/exposure", plain(7.0)),
    to(12, S::BasicCorrection, K::FloatSlider, "basicCorrection/tone/contrast", plain(150.0)),
    to(13, S::BasicCorrection, K::FloatSlider, "basicCorrection/tone/highlights", plain(150.0)),
    to(14, S::BasicCorrection, K::FloatSlider, "basicCorrection/tone/shadows", plain(150.0)),
    to(15, S::BasicCorrection, K::FloatSlider, "basicCorrection/tone/whites", plain(150.0)),
    to(16, S::BasicCorrection, K::FloatSlider, "basicCorrection/tone/blacks", plain(150.0)),
    to(64, S::BasicCorrection, K::FloatSlider, "basicCorrection/tone/hdrSpecular", plain(150.0)),
    // Creative: 23 the switch; 24 / 96 the Look's name, 25 / 128 its popups, 129 / 130 its colour
    // space.
    row(23, S::Creative, Some(K::Checkbox), Role::Switch),
    row(24, S::Creative, Some(K::ArbitraryData), Role::Lut { lut: false }),
    row(96, S::Creative, Some(K::ArbitraryData), Role::Lut { lut: false }),
    row(25, S::Creative, Some(K::Popup), Role::Lut { lut: false }),
    row(128, S::Creative, Some(K::Popup), Role::Lut { lut: false }),
    row(129, S::Creative, Some(K::ArbitraryData), Role::LookSpace),
    row(130, S::Creative, Some(K::Popup), Role::LookSpace),
    to(26, S::Creative, K::FloatSlider, "creative/lookIntensity", plain(200.0)),
    to(28, S::Creative, K::FloatSlider, "creative/adjustments/fadedFilm", plain(150.0)),
    to(29, S::Creative, K::FloatSlider, "creative/adjustments/sharpen", plain(100.0)),
    to(30, S::Creative, K::FloatSlider, "creative/adjustments/vibrance", plain(100.0)),
    to(31, S::Creative, K::FloatSlider, "creative/adjustments/creativeSaturation", plain(300.0)),
    row(32, S::Creative, Some(K::ArbitraryData), Role::Data(Data::SplitToning)),
    to(33, S::Creative, K::FloatSlider, "creative/adjustments/tintBalance", plain(150.0)),
    // Curves: 37 switches all curves, 118 the RGB curves, 104 ... 111 each hue / saturation curve.
    row(37, S::RgbCurves, Some(K::Checkbox), Role::Switch),
    row(118, S::RgbCurves, Some(K::Checkbox), Role::Switch),
    row(65, S::RgbCurves, Some(K::FloatSlider), Role::Missing),
    row(39, S::RgbCurves, Some(K::ArbitraryData), Role::Data(Data::RgbCurves)),
    row(104, S::HueSat(0), Some(K::Checkbox), Role::Switch),
    row(113, S::HueSat(0), Some(K::Color), Role::Eyedropper),
    row(42, S::HueSat(0), Some(K::ArbitraryData), Role::Data(Data::HueSatCurve(Some("curves/hueSaturationCurves/hueVsSat")))),
    row(105, S::HueSat(1), Some(K::Checkbox), Role::Switch),
    row(114, S::HueSat(1), Some(K::Color), Role::Eyedropper),
    row(106, S::HueSat(1), Some(K::ArbitraryData), Role::Data(Data::HueSatCurve(None))),
    row(107, S::HueSat(2), Some(K::Checkbox), Role::Switch),
    row(115, S::HueSat(2), Some(K::Color), Role::Eyedropper),
    row(108, S::HueSat(2), Some(K::ArbitraryData), Role::Data(Data::HueSatCurve(None))),
    row(109, S::HueSat(3), Some(K::Checkbox), Role::Switch),
    row(116, S::HueSat(3), Some(K::Color), Role::Eyedropper),
    row(110, S::HueSat(3), Some(K::ArbitraryData), Role::Data(Data::HueSatCurve(None))),
    row(111, S::HueSat(4), Some(K::Checkbox), Role::Switch),
    row(117, S::HueSat(4), Some(K::Color), Role::Eyedropper),
    row(112, S::HueSat(4), Some(K::ArbitraryData), Role::Data(Data::HueSatCurve(None))),
    // Color Wheels: 46 the switch, 102 Face Detection (on by default), 66 HDR White, 47 the wheels.
    row(46, S::ColorWheels, Some(K::Checkbox), Role::Switch),
    row(102, S::ColorWheels, Some(K::Checkbox), Role::Missing),
    row(66, S::ColorWheels, Some(K::FloatSlider), Role::Missing),
    row(47, S::ColorWheels, Some(K::ArbitraryData), Role::Data(Data::Wheels)),
    // HSL Secondary: 68 the switch; the key (70-76), Refine (81, 82), Correction (85-90).
    row(68, S::HslSecondary, Some(K::Checkbox), Role::Switch),
    row(70, S::HslSecondary, Some(K::Color), Role::Eyedropper),
    row(71, S::HslSecondary, Some(K::Color), Role::Eyedropper),
    row(72, S::HslSecondary, Some(K::Color), Role::Eyedropper),
    row(73, S::HslSecondary, Some(K::ArbitraryData), Role::Data(Data::HslKey)),
    to(74, S::HslSecondary, K::Checkbox, "hslSecondary/key/showMask", Conv::Checkbox),
    to(75, S::HslSecondary, K::Popup, "hslSecondary/key/maskStyle", Conv::Popup { default: 1, items: MASK_STYLE }),
    to(76, S::HslSecondary, K::Checkbox, "hslSecondary/key/invertMask", Conv::Checkbox),
    to(81, S::HslSecondary, K::FloatSlider, "hslSecondary/refine/denoise", plain(100.0)),
    to(82, S::HslSecondary, K::FloatSlider, "hslSecondary/refine/blur", plain(1000.0)),
    row(85, S::HslSecondary, Some(K::ArbitraryData), Role::Data(Data::CorrectionWheels)),
    // The correction wheel's mode (4 zero bytes by default).
    row(93, S::HslSecondary, Some(K::ArbitraryData), Role::Data(Data::Other)),
    to(86, S::HslSecondary, K::FloatSlider, "hslSecondary/correction/temperature", plain(300.0)),
    to(87, S::HslSecondary, K::FloatSlider, "hslSecondary/correction/tint", plain(300.0)),
    to(88, S::HslSecondary, K::FloatSlider, "hslSecondary/correction/contrast", plain(150.0)),
    to(89, S::HslSecondary, K::FloatSlider, "hslSecondary/correction/sharpen", plain(100.0)),
    to(90, S::HslSecondary, K::FloatSlider, "hslSecondary/correction/saturation", plain(300.0)),
    // Vignette: 50 the switch.
    row(50, S::Vignette, Some(K::Checkbox), Role::Switch),
    to(51, S::Vignette, K::FloatSlider, "vignette/vignetteAmount", plain(5.0)),
    to(52, S::Vignette, K::FloatSlider, "vignette/vignetteMidpoint", plain(100.0)),
    to(53, S::Vignette, K::FloatSlider, "vignette/vignetteRoundness", plain(100.0)),
    to(54, S::Vignette, K::FloatSlider, "vignette/vignetteFeather", plain(100.0)),
    // SpeedGrade Custom: 57 Custom Layer (on by default), 60 its data.
    row(57, S::SpeedGradeCustom, Some(K::Checkbox), Role::Switch),
    row(60, S::SpeedGradeCustom, Some(K::ArbitraryData), Role::Data(Data::Other)),
    // Outside the sections: Legacy Tone And Saturation, the embedded LUTs, Auto Tone, Auto Color,
    // the LUT and Look assets.
    row(97, S::Always, Some(K::Checkbox), Role::MissingWhenOn),
    row(98, S::Always, Some(K::ArbitraryData), Role::Data(Data::Other)),
    row(99, S::Always, Some(K::ArbitraryData), Role::Data(Data::Other)),
    row(100, S::Always, Some(K::ArbitraryData), Role::Data(Data::Other)),
    row(124, S::Always, Some(K::Checkbox), Role::Missing),
    row(125, S::Always, Some(K::ArbitraryData), Role::Lut { lut: true }),
    row(126, S::Always, Some(K::ArbitraryData), Role::Lut { lut: false }),
    row(1, S::Always, Some(K::ArbitraryData), Role::Derived),
    row(121, S::Always, Some(K::FloatSlider), Role::Derived),
    row(122, S::Always, Some(K::ArbitraryData), Role::Derived),
];

/// The names After Effects shows for the parameters whose `pard` name is blank (`dump.props`).
fn blank_name(suffix: u16) -> Option<&'static str> {
    Some(match suffix {
        62 => "High Dynamic Range",
        3 | 23 | 37 | 118 | 104 | 105 | 107 | 109 | 111 | 46 | 68 | 50 => "Active",
        4 | 95 => "Input LUT data",
        24 | 96 => "Look name",
        102 => "Face Detection",
        74 => "Show Mask",
        75 => "Mask style",
        76 => "Invert Mask",
        97 => "Legacy Tone And Saturation",
        124 => "Auto Color Active",
        _ => return None,
    })
}

/// The parameter's name for the report: its own, else the one After Effects shows.
fn name_of(p: &EffectParam<'_, '_>) -> String {
    match (p.name.trim(), suffix(&p.match_name).and_then(blank_name)) {
        ("", Some(n)) => n.to_string(),
        ("", None) => p.match_name.clone(),
        (n, _) => n.to_string(),
    }
}

/// The match-name suffix of a Lumetri parameter (`ADBE Lumetri-0011` is 11).
fn suffix(match_name: &str) -> Option<u16> {
    match_name.strip_prefix("ADBE Lumetri-")?.parse().ok()
}

/// The parameter's value: the recorded one, else the `pard` default.
fn value_of(p: &EffectParam<'_, '_>) -> Option<ParamValue> {
    p.recorded_value().or_else(|| p.pard_default())
}

/// True when the parameter has keyframes, an expression, or a recorded value other than its
/// default (a record without a readable default counts as set).
fn is_set(p: &EffectParam<'_, '_>) -> bool {
    if p.is_animated() || p.stored().is_some_and(|s| s.expression().is_some()) {
        return true;
    }
    match (p.recorded_value(), p.pard_default()) {
        (Some(v), Some(d)) => !same(v, d),
        (Some(_), None) => true,
        (None, _) => false,
    }
}

/// Equal within the float noise of a stored `f64` / `f32`.
fn same(a: ParamValue, b: ParamValue) -> bool {
    let close = |x: f64, y: f64| (x - y).abs() <= 1e-6 * x.abs().max(y.abs()).max(1.0);
    match (a, b) {
        (ParamValue::Number(x), ParamValue::Number(y)) => close(x, y),
        (ParamValue::Argb(x), ParamValue::Argb(y)) => x.iter().zip(y).skip(1).all(|(x, y)| close(*x, y)),
        _ => a == b,
    }
}

/// A value as the report shows it.
fn shown_value(v: Option<ParamValue>) -> String {
    match v {
        Some(ParamValue::Number(n)) => {
            let s = format!("{n:.3}");
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        }
        Some(ParamValue::Bool(b)) => if b { "on" } else { "off" }.to_string(),
        Some(ParamValue::Index(i)) => format!("item {i}"),
        Some(ParamValue::Argb([_, r, g, b])) => format!("RGB {:.3} {:.3} {:.3}", r, g, b),
        Some(other) => format!("{other:?}"),
        None => "set".to_string(),
    }
}

/// Empty data: absent, `fe fe` (no text, no asset) or only zero bytes.
fn is_empty_data(d: Option<&[u8]>) -> bool {
    match d {
        None => true,
        Some(d) => d == [0xfe, 0xfe] || d.iter().all(|b| *b == 0),
    }
}

fn le_u32(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(*d.get(at..at.checked_add(4)?)?.first_chunk::<4>()?))
}

fn le_f32(d: &[u8], at: usize) -> Option<f32> {
    Some(f32::from_le_bytes(*d.get(at..at.checked_add(4)?)?.first_chunk::<4>()?))
}

fn le_f64(d: &[u8], at: usize) -> Option<f64> {
    Some(f64::from_le_bytes(*d.get(at..at.checked_add(8)?)?.first_chunk::<8>()?))
}

/// A UTF-16LE text up to its NUL (the Look's name, `-0096`: `SL BLUE ICE` in 1200 `-01-set-all`);
/// `None` for empty data.
fn utf16_text(d: &[u8]) -> Option<String> {
    if is_empty_data(Some(d)) {
        return None;
    }
    let units: Vec<u16> = d.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).take_while(|u| *u != 0).collect();
    let s = String::from_utf16_lossy(&units);
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// A Lumetri curve (520 bytes): `i32` curve type at 0 (3 for the hue curves, which wrap; 2 for
/// Luma vs Sat and Sat vs Sat; 0 for an RGB curve), the `u32` point count at 4, then the points
/// as little-endian `f64` pairs x, y from byte 8 (at most 32), sorted by x. Bytes after the
/// counted points are stale (a fresh hue curve has count 0 and old points behind it). An RGB
/// curve's x and y are 0..1 (2 points 0,0 and 1,1 by default); a hue / saturation curve's x is
/// the hue (0 red, 1/3 green) or the luma / saturation, 0..1, and y its offset from neutral.
/// `None` when the count does not fit.
fn curve_points(d: &[u8]) -> Option<Vec<(f64, f64)>> {
    let n = usize::try_from(le_u32(d, 4)?).ok()?;
    if n > d.len().saturating_sub(8) / 16 {
        return None;
    }
    (0..n).map(|i| Some((le_f64(d, 8 + 16 * i)?, le_f64(d, 16 + 16 * i)?))).collect()
}

/// HSL Secondary's key (`-0073`, 40 bytes): a flag byte (1 once a key colour was picked, 0 for a
/// fresh effect), then hue, saturation and lightness, each three little-endian `f32` (centre,
/// range, tolerance; fractions of the full hue circle / of 0..1) and an enabled byte. A fresh effect
/// stores flag 0 and centre 0.5, range 0, tolerance 0, enabled for each (1200 `-00-defaults`);
/// After Effects' own copy in `-0001` names the same fields (`middleHLS`, `rangeHLS`,
/// `toleranceHLS`, `enableHLS`, hue in degrees: centre 180). Picking Set color RGB 0.02 0.55 0.85
/// gives hue centre 0.5605 (that colour's hue, 201.8 degrees) and lightness centre 0.435 (its
/// HSL lightness) with range 0.05 and tolerance 0.1 (`-01-set-all`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HslKey {
    pub picked: bool,
    /// Hue, saturation, lightness: centre, range, tolerance, enabled.
    pub channels: [(f32, f32, f32, bool); 3],
}

impl HslKey {
    pub fn parse(d: &[u8]) -> Option<HslKey> {
        if d.len() != 40 {
            return None;
        }
        let picked = match d.first()? {
            0 => false,
            1 => true,
            _ => return None,
        };
        let ch = |at: usize| -> Option<(f32, f32, f32, bool)> {
            let (c, r, t) = (le_f32(d, at)?, le_f32(d, at + 4)?, le_f32(d, at + 8)?);
            let on = match d.get(at + 12)? {
                0 => false,
                1 => true,
                _ => return None,
            };
            (c.is_finite() && r.is_finite() && t.is_finite()).then_some((c, r, t, on))
        };
        Some(HslKey { picked, channels: [ch(1)?, ch(14)?, ch(27)?] })
    }

    fn is_default(&self) -> bool {
        !self.picked && self.channels.iter().all(|c| *c == (0.5, 0.0, 0.0, true))
    }
}

/// The colour wheel data: triples of `f64`, each wheel `0, 0.5, 0` by default.
fn wheels_default(d: &[u8], wheels: usize) -> bool {
    d.len() == wheels * 24 && (0..wheels).all(|w| le_f64(d, w * 24) == Some(0.0) && le_f64(d, w * 24 + 8) == Some(0.5) && le_f64(d, w * 24 + 16) == Some(0.0))
}

/// Is the data of `data` at After Effects' default (as far as it can be told)?
fn data_default(data: Data, d: Option<&[u8]>) -> bool {
    let Some(d) = d else { return true };
    match data {
        Data::RgbCurves => {
            d.len() == 4 * 520 && d.as_chunks::<520>().0.iter().all(|c| curve_points(c).is_some_and(|pts| pts.iter().all(|(x, y)| (x - y).abs() < 1e-9)))
        }
        Data::HueSatCurve(_) => curve_points(d).is_some_and(|pts| pts.iter().all(|(_, y)| *y == 0.0)),
        Data::Wheels => wheels_default(d, 4),
        Data::CorrectionWheels => wheels_default(d, 8),
        Data::SplitToning | Data::Other => is_empty_data(Some(d)),
        Data::HslKey => HslKey::parse(d).is_some_and(|k| k.is_default()),
    }
}

/// What the Input LUT or the Look holds: names, a popup item, an asset.
#[derive(Default)]
struct LutState {
    names: Vec<String>,
    item: Option<String>,
    asset: bool,
}

/// How a set parameter of a section that does nothing is listed in the report; `None` when it is
/// at its default.
fn off_text(p: &EffectParam<'_, '_>, role: Role, data: Option<&[u8]>) -> Option<String> {
    match role {
        Role::Data(d) => (!data_default(d, data)).then(|| name_of(p)),
        // A LUT by its name (the popups index the same choice).
        Role::Lut { lut } if p.kind == K::ArbitraryData => {
            let what = if lut { "Input LUT" } else { "Look" };
            data.and_then(utf16_text).map(|n| format!("{what} '{n}'")).or_else(|| (!is_empty_data(data)).then(|| what.to_string()))
        }
        // The Look's colour space goes with the Look.
        Role::Lut { .. } | Role::LookSpace | Role::Derived => None,
        _ => is_set(p).then(|| format!("{} {}", name_of(p), shown_value(p.recorded_value()))),
    }
}

/// Notes what an Input LUT or Look parameter holds: a popup item, an asset or a name.
fn note_lut(p: &EffectParam<'_, '_>, suffix: u16, data: Option<&[u8]>, st: &mut LutState) {
    if p.kind == K::Popup {
        // The popup indexes the LUTs installed with After Effects; item 1 is None.
        if let Some(ParamValue::Index(i)) = p.recorded_value()
            && i > 1
        {
            st.item = usize::try_from(i).ok().and_then(|i| p.items.as_ref()?.get(i - 1).cloned()).or(Some(format!("item {i}")));
        }
    } else if suffix == 125 || suffix == 126 {
        st.asset |= !is_empty_data(data);
    } else if let Some(name) = data.and_then(utf16_text)
        && !st.names.contains(&name)
    {
        st.names.push(name);
    }
}

/// Imports a Lumetri Color instance's parameters into `out.group` and reports what does not arrive.
pub(crate) fn import_lumetri(fx: &AepEffect<'_, '_>, params: &[EffectParam<'_, '_>], cx: &LayerCtx<'_>, shown: &str, out: ParamOut<'_>) {
    let ParamOut { group, refs, col } = out;
    let here = format!("{}: '{}' effect '{shown}' ({})", cx.comp, cx.layer, fx.match_name());
    let find = |s: u16| params.iter().find(|p| suffix(&p.match_name) == Some(s));
    // The switches: on unless a record says off. Keys or an expression on a switch are not
    // imported (the static value counts).
    let switch_on = |s: u16| -> bool {
        match find(s) {
            Some(p) => !matches!(value_of(p), Some(ParamValue::Bool(false))),
            None => true,
        }
    };
    // HSL Secondary without a picked key colour selects nothing: After Effects applies none of
    // its correction (1200 `-16-sec-correction`, `-15-sec-refine` render as `-00-defaults`);
    // EffectCraft's default key is the whole image, so the values are left out too.
    // Unreadable or missing key data does not count as unpicked (it is reported or older).
    let key_picked = find(73).and_then(|p| fx.arbitrary_data(&p.match_name)).and_then(HslKey::parse).is_none_or(|k| k.picked);
    let section_on = |sec: Section| -> bool {
        match sec {
            Section::Always => true,
            Section::BasicCorrection => switch_on(3),
            Section::Creative => switch_on(23),
            Section::RgbCurves => switch_on(37) && switch_on(118),
            Section::HueSat(i) => switch_on(37) && switch_on([104, 105, 107, 109, 111].get(usize::from(i)).copied().unwrap_or(104)),
            Section::ColorWheels => switch_on(46),
            Section::HslSecondary => switch_on(68) && key_picked,
            Section::Vignette => switch_on(50),
            // Custom Layer off is itself reported (a switch without counterpart).
            Section::SpeedGradeCustom => true,
        }
    };
    let mut off: Vec<(Section, Vec<String>)> = Vec::new();
    let mut eyedroppers: Vec<String> = Vec::new();
    let mut luts = [LutState::default(), LutState::default()];
    for p in params {
        if carries_nothing(p) || p.match_name == "ADBE Effect Built In Params" {
            continue;
        }
        let label = if p.name.trim().is_empty() { format!("{} ({})", name_of(p), p.match_name) } else { param_label(p) };
        let Some(row) = suffix(&p.match_name).and_then(|s| ROWS.iter().find(|r| r.suffix == s)) else {
            // A parameter this table does not know (a newer version): lost when set.
            if p.stored().is_some()
                && (p.kind != K::ArbitraryData && is_set(p) || p.kind == K::ArbitraryData && !is_empty_data(fx.arbitrary_data(&p.match_name)))
            {
                col.lose(LossCategory::Effects, format!("{here}: parameter {label} not imported"));
            }
            continue;
        };
        if row.kind.is_some_and(|k| k != p.kind) {
            if p.stored().is_some() {
                col.lose(LossCategory::Effects, format!("{here}: parameter {label} has type {:?}, expected {:?}; not imported", p.kind, row.kind));
            }
            continue;
        }
        let data = || fx.arbitrary_data(&p.match_name);
        // A switched-off section: its set values are listed, not imported.
        if !matches!(row.role, Role::Switch) && !section_on(row.section) {
            let text = off_text(p, row.role, data());
            if let Some(text) = text {
                match off.iter_mut().find(|(s, _)| *s == row.section) {
                    Some((_, list)) => {
                        if !list.contains(&text) {
                            list.push(text);
                        }
                    }
                    None => off.push((row.section, vec![text])),
                }
            }
            continue;
        }
        match row.role {
            Role::To(target, conv) => {
                let m = ParamMap { index: p.index, kind: p.kind, target, conv, limit: None };
                import_mapped(p, &m, cx, (&here, &label), ParamOut { group: &mut *group, refs: &mut *refs, col: &mut *col });
            }
            Role::Switch => {
                if p.is_animated() || p.stored().is_some_and(|s| s.expression().is_some()) {
                    col.lose(LossCategory::Effects, format!("{here}: keyframes or an expression on the switch {label} are not imported"));
                }
                if row.section == Section::SpeedGradeCustom && !switch_on(row.suffix) {
                    col.lose(LossCategory::Effects, format!("{here}: parameter {label} is off and has no counterpart"));
                }
            }
            Role::MissingWhenOn => {
                if p.is_animated() || matches!(p.recorded_value(), Some(ParamValue::Bool(true))) {
                    col.lose(LossCategory::Effects, format!("{here}: parameter {label} is on and has no counterpart, not imported"));
                }
            }
            Role::Missing => {
                if is_set(p) {
                    col.lose(
                        LossCategory::Effects,
                        format!("{here}: parameter {label} is {} and has no counterpart, not imported", shown_value(p.recorded_value())),
                    );
                }
            }
            Role::Eyedropper => {
                if is_set(p) {
                    eyedroppers.push(name_of(p));
                }
            }
            Role::Lut { lut } => note_lut(p, row.suffix, data(), &mut luts[usize::from(!lut)]),
            // The Look's colour space only applies to a Look, which is reported.
            Role::LookSpace => {}
            Role::Data(_) if p.stored().is_some() && data().is_none() => {
                // The value record is there but its aRbs/aRbp payload is not: damaged, not the
                // default (a parameter without a record is the default).
                col.lose(LossCategory::DamagedData, format!("{here}: the data of {label} is missing, not imported (the default used)"));
            }
            Role::Data(Data::HslKey) => import_hsl_key(data(), &here, &mut *group, &mut *col),
            Role::Data(d) => {
                if p.is_animated() || p.stored().is_some_and(|s| s.expression().is_some()) {
                    col.lose(LossCategory::Effects, format!("{here}: keyframes or an expression on {label} are not imported"));
                }
                if !data_default(d, data()) {
                    let why = match d {
                        Data::RgbCurves => {
                            import_rgb_curves(data(), (&here, &label), &mut *group, &mut *col);
                            continue;
                        }
                        Data::HueSatCurve(Some(target)) => {
                            import_hue_sat_curve(data(), target, (&here, &label), &mut *group, &mut *col);
                            continue;
                        }
                        Data::Wheels => {
                            import_wheels(data(), (&here, &label), &mut *group, &mut *col);
                            continue;
                        }
                        Data::HueSatCurve(None) => "the curve's points are moved; no sample tells their scale",
                        Data::CorrectionWheels | Data::SplitToning => "the data is not decoded (not sampled)",
                        Data::HslKey | Data::Other => "no counterpart",
                    };
                    col.lose(LossCategory::Effects, format!("{here}: parameter {label} is not imported ({why})"));
                }
            }
            Role::Derived => {}
        }
    }
    for (sec, list) in off {
        let why = if sec == Section::HslSecondary && switch_on(68) {
            "has no key colour picked (After Effects applies it to nothing)"
        } else {
            "is switched off (EffectCraft's Lumetri has no section switch)"
        };
        col.lose(LossCategory::Effects, format!("{here}: section {} {why}, its values are not imported: {}", sec.name(), list.join(", ")));
    }
    if !eyedroppers.is_empty() {
        col.lose(LossCategory::Effects, format!("{here}: the eyedropper colours {} are not imported (they only set other controls)", eyedroppers.join(", ")));
    }
    for (st, what) in luts.iter().zip(["Input LUT", "Look"]) {
        let name = st.names.first().cloned().or_else(|| st.item.clone());
        if name.is_some() || st.asset {
            let name = name.map(|n| format!(" '{n}'")).unwrap_or_default();
            col.lose(
                LossCategory::Effects,
                format!("{here}: the {what}{name} is not imported (the LUT file is not in the project; EffectCraft's {what} stays None)"),
            );
        }
    }
}

/// HSL Secondary's key as EffectCraft's hue centre / range / softness (degrees) and saturation and
/// lightness bands (percent): range is taken as the half width, tolerance as the softness. A key
/// that was never picked keeps EffectCraft's default (the whole image).
fn import_hsl_key(d: Option<&[u8]>, here: &str, group: &mut effectcraft_project::PropGroup, col: &mut crate::import::report::Collector) {
    let Some(d) = d else { return };
    let Some(key) = HslKey::parse(d) else {
        col.lose(LossCategory::DamagedData, format!("{here}: the HSL Secondary key data is unreadable, the default key used"));
        return;
    };
    if !key.picked {
        if !key.is_default() {
            col.lose(LossCategory::Effects, format!("{here}: the HSL Secondary key holds values without a picked colour, not imported"));
        }
        return;
    }
    let mut set = |id: &str, v: Value| {
        if let Some(prop) = group.prop_mut(&format!("hslSecondary/key/{id}")) {
            prop.value = v;
        }
    };
    let [h, s, l] = key.channels.map(|(c, r, t, on)| (f64::from(c), f64::from(r), f64::from(t), on));
    set("hueEnabled", Value::Bool(h.3));
    set("hueCenter", Value::Scalar((h.0 * 360.0).rem_euclid(360.0)));
    set("hueRange", Value::Scalar((h.1 * 360.0).clamp(0.0, 180.0)));
    set("hueSoftness", Value::Scalar((h.2 * 360.0).clamp(0.0, 180.0)));
    for ((c, r, t, on), id) in [(s.0, s.1, s.2, s.3), (l.0, l.1, l.2, l.3)].into_iter().zip(["saturation", "lightness"]) {
        set(&format!("{id}Enabled"), Value::Bool(on));
        set(&format!("{id}Low"), Value::Scalar(((c - r) * 100.0).clamp(0.0, 100.0)));
        set(&format!("{id}High"), Value::Scalar(((c + r) * 100.0).clamp(0.0, 100.0)));
        set(&format!("{id}Softness"), Value::Scalar((t * 100.0).clamp(0.0, 100.0)));
    }
}

/// A parameter of `group` set to `v` (ids the spec does not have are skipped).
fn set_prop(group: &mut effectcraft_project::PropGroup, id: &str, v: Value) {
    if let Some(prop) = group.prop_mut(id) {
        prop.value = v;
    }
}

/// Curve points as EffectCraft's `x,y x,y ...` text, x and y clamped to 0..1; `None` when a value
/// is not finite.
fn points_text(pts: impl IntoIterator<Item = (f64, f64)>) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    for (x, y) in pts {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        out.push(format!("{},{}", x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)));
    }
    Some(out.join(" "))
}

/// The RGB curve (`-0039`, four curves of 520 bytes). The first is the master curve (1200
/// `lumetri-60-preset-curves-rgb`: its midpoint raised to 0.787 lifts R, G and B alike) and
/// becomes `curves/rgbCurves/master`. The other three are the channel curves, but no sample moves
/// one, so which is red, green or blue is not known: a moved one is reported.
fn import_rgb_curves(d: Option<&[u8]>, (here, label): (&str, &str), group: &mut effectcraft_project::PropGroup, col: &mut crate::import::report::Collector) {
    let Some(d) = d else { return };
    let curves: Option<Vec<Vec<(f64, f64)>>> = if d.len() == 4 * 520 { d.as_chunks::<520>().0.iter().map(|c| curve_points(c)).collect() } else { None };
    let Some(curves) = curves else {
        col.lose(LossCategory::DamagedData, format!("{here}: the data of {label} is unreadable, the curves are not imported"));
        return;
    };
    let identity = |pts: &[(f64, f64)]| pts.iter().all(|(x, y)| (x - y).abs() < 1e-9);
    if let Some(master) = curves.first().filter(|c| !identity(c)) {
        match points_text(master.iter().copied()) {
            Some(text) => set_prop(group, "curves/rgbCurves/master", Value::Str(text)),
            None => col.lose(LossCategory::DamagedData, format!("{here}: the master curve of {label} has unreadable points, not imported")),
        }
    }
    let moved = curves.iter().skip(1).filter(|c| !identity(c)).count();
    if moved > 0 {
        col.lose(
            LossCategory::Effects,
            format!("{here}: {moved} channel curve(s) of {label} are not imported (no sample tells which curve is red, green or blue)"),
        );
    }
}

/// Hue vs Sat (`-0042`): After Effects' y is an offset from neutral where 1 doubles the
/// saturation (1200 `lumetri-61-preset-curves-huesat`: a point at hue 0.0155, y 0.97 scales pure
/// red's saturation by about 1.96 in linear light; pins at -0.03 and -0.02 scale green, blue and
/// cyan by 0.97). EffectCraft's y = 1 doubles it and 0.5 is neutral, so y becomes `0.5 + y / 2`.
/// x is the hue in both (0 red, 1/3 green).
fn import_hue_sat_curve(
    d: Option<&[u8]>,
    target: &str,
    (here, label): (&str, &str),
    group: &mut effectcraft_project::PropGroup,
    col: &mut crate::import::report::Collector,
) {
    let Some(d) = d else { return };
    match curve_points(d).and_then(|pts| points_text(pts.into_iter().map(|(x, y)| (x, 0.5 + y / 2.0)))) {
        Some(text) => set_prop(group, target, Value::Str(text)),
        None => col.lose(LossCategory::DamagedData, format!("{here}: the data of {label} is unreadable, the curve is not imported")),
    }
}

/// The colour EffectCraft's wheel takes for After Effects' hue (degrees, 0 red, 120 green) and
/// distance from the centre (0..1): the HSL colour of that hue with saturation = distance and
/// lightness 0.5 (grey 0.5 at distance 0). EffectCraft's wheel adds that colour's offset from its
/// grey, so the direction is After Effects' and the strength grows with the distance.
fn wheel_colour(hue: f64, distance: f64) -> [f64; 4] {
    let c = distance.clamp(0.0, 1.0);
    let h = hue.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match h {
        h if h < 1.0 => (c, x, 0.0),
        h if h < 2.0 => (x, c, 0.0),
        h if h < 3.0 => (0.0, c, x),
        h if h < 4.0 => (0.0, x, c),
        h if h < 5.0 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = 0.5 - c / 2.0;
    [r + m, g + m, b + m, 1.0]
}

/// Color Wheels (`-0047`, 96 bytes): four wheels of three little-endian `f64`, the hue in degrees,
/// the brightness slider (0..1, 0.5 neutral) and the distance from the centre (0..1); `0, 0.5, 0`
/// each by default. The first three are Shadows, Midtones and Highlights (1200
/// `lumetri-62-preset-wheels`: Shadows 229 degrees (blue), slider 0.37, distance 0.55; Midtones
/// 0.1 degrees (red), 0.5, 0.39; Highlights 93 degrees (yellow-green), 0.73, 0.32); the fourth is
/// not known. Hue and distance become EffectCraft's wheel colour ([`wheel_colour`]); the brightness
/// sliders and a set fourth wheel have no counterpart and are reported.
fn import_wheels(d: Option<&[u8]>, (here, label): (&str, &str), group: &mut effectcraft_project::PropGroup, col: &mut crate::import::report::Collector) {
    let Some(d) = d else { return };
    let wheels: Option<Vec<[f64; 3]>> = if d.len() == 4 * 24 {
        (0..4).map(|w| Some([le_f64(d, w * 24)?, le_f64(d, w * 24 + 8)?, le_f64(d, w * 24 + 16)?]).filter(|v| v.iter().all(|x| x.is_finite()))).collect()
    } else {
        None
    };
    let Some(wheels) = wheels else {
        col.lose(LossCategory::DamagedData, format!("{here}: the data of {label} is unreadable, the colour wheels are not imported"));
        return;
    };
    let mut lost: Vec<String> = Vec::new();
    for (w, (id, name)) in wheels.iter().zip([("shadowsWheel", "Shadows"), ("midtonesWheel", "Midtones"), ("highlightsWheel", "Highlights")]) {
        let [hue, brightness, distance] = *w;
        if distance != 0.0 {
            set_prop(group, &format!("colorWheels/{id}"), Value::Color(wheel_colour(hue, distance)));
        }
        if (brightness - 0.5).abs() > 1e-9 {
            lost.push(format!("{name} brightness {brightness:.3} (0.5 is neutral)"));
        }
    }
    if wheels.get(3).is_some_and(|w| *w != [0.0, 0.5, 0.0]) {
        lost.push("the fourth wheel (its role is not known)".to_string());
    }
    if !lost.is_empty() {
        col.lose(LossCategory::Effects, format!("{here}: {label}: {} not imported (no counterpart)", lost.join(", ")));
    }
}
