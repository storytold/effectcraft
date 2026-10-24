//! The layer-style table shared by the importer (`effectcraft-aep-import`) and the exporter
//! (`effectcraft-aep-export`): the After Effects layer styles (`ADBE Layer Styles`, docs
//! `aep-format.md` 3.14) and their parameters, the EffectCraft style and parameter each maps to,
//! the unit conversion and After Effects' default as it would be stored.
//!
//! Layout (950-980): every layer's root group holds `ADBE Layer Styles`; under it `ADBE Blend
//! Options Group` and one switch group per style (`dropShadow/enabled`, ...), whose `tdsb` is 1
//! while the style is applied and 2 while it is not. A parameter has a record only when it
//! differs from its default, in the order of [`StyleMap::params`] (After Effects' property
//! order). Scalars are stored in the units shown (opacity in percent, pixels, degrees), colours
//! as ARGB 0..255, popups 1-based, checkboxes 1.0 when on.

/// The match name of the layer-styles group in a layer's root group.
pub const GROUP: &str = "ADBE Layer Styles";
/// The Blending Options group.
pub const BLEND_OPTIONS: &str = "ADBE Blend Options Group";
/// The Advanced Blending group inside Blending Options.
pub const ADV_BLEND: &str = "ADBE Adv Blend Group";
/// Global Light Angle in Blending Options (degrees; 977).
pub const GLOBAL_ANGLE: &str = "ADBE Global Angle2";
/// Global Light Altitude in Blending Options (degrees; 977).
pub const GLOBAL_ALTITUDE: &str = "ADBE Global Altitude2";
/// The group `tdsb` of an applied style (and of Layer Styles and Blending Options once a style is
/// applied, 951-960).
pub const FLAGS_ON: u32 = 1;
/// The group `tdsb` of a style that is applied but switched off (its eye; 988: not drawn). Bit 0
/// is the switch, as on shape items (986, 987).
pub const FLAGS_OFF: u32 = 0;
/// The group `tdsb` of a style that is not applied (950).
pub const FLAGS_UNUSED: u32 = 2;
/// The `tdsb` of Layer Styles and Blending Options while no style is applied (950).
pub const FLAGS_GROUP_UNUSED: u32 = 3;
/// The switch group of Pattern Overlay, which has no EffectCraft counterpart (and no menu
/// command in After Effects 2026).
pub const PATTERN_OVERLAY: &str = "patternFill";

/// How a stored style value becomes an EffectCraft value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StyleConv {
    /// A number used as stored (percent, pixels, degrees; 966-974, 977).
    Number,
    /// A colour: ARGB 0..255 (967, 970, 977).
    Color,
    /// A style blend mode: After Effects' 1-based item of [`STYLE_MODES`] (978).
    Mode,
    /// Another popup: `map[stored - 1]` is the EffectCraft item (0-based).
    Popup(&'static [u32]),
    /// A checkbox, stored 1.0 when on (977).
    Checkbox,
    /// A two-component value used as stored (Gradient Overlay Offset; 977).
    Vec2,
    /// A gradient (`GCst`), not decoded: reported on import, never written.
    Gradient,
}

/// One parameter: its match name without the style prefix, the EffectCraft parameter id, the
/// conversion and After Effects' default as stored (MANIFEST 951 read-back).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StyleParam {
    pub ae: &'static str,
    pub ec: &'static str,
    pub conv: StyleConv,
    pub default: &'static [f64],
}

/// One style: its switch-group prefix, the EffectCraft style id and the parameters in After
/// Effects' property order (the order records are written in, 970).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StyleMap {
    pub ae: &'static str,
    pub ec: &'static str,
    pub params: &'static [StyleParam],
}

impl StyleMap {
    /// The switch group's match name (`dropShadow/enabled`).
    pub fn switch_name(&self) -> String {
        format!("{}/enabled", self.ae)
    }
    /// A parameter's full match name (`dropShadow/opacity`).
    pub fn param_name(&self, p: &StyleParam) -> String {
        format!("{}/{}", self.ae, p.ae)
    }
}

/// The style blend modes by their stored value (1-based): EffectCraft's mode name, `None` for a
/// separator of the menu (renders Normal). Sweep 978 (16 bpc, each value rendered over a known
/// colour): Normal and Dissolve (at 100 % opacity) render alike, as do Lighter Color and Normal
/// for that colour pair (E); the others matched one formula within 1/255.
pub const STYLE_MODES: &[Option<&str>] = &[
    Some("Normal"),
    Some("Dissolve"),
    None,
    Some("Darken"),
    Some("Multiply"),
    Some("Color Burn"),
    Some("Linear Burn"),
    Some("Darker Color"),
    None,
    Some("Lighten"),
    Some("Screen"),
    Some("Color Dodge"),
    Some("Linear Dodge (Add)"),
    Some("Lighter Color"),
    None,
    Some("Overlay"),
    Some("Soft Light"),
    Some("Hard Light"),
    Some("Vivid Light"),
    Some("Linear Light"),
    Some("Pin Light"),
    Some("Hard Mix"),
    None,
    Some("Difference"),
    Some("Exclusion"),
    None,
    Some("Hue"),
    Some("Saturation"),
    Some("Color"),
    Some("Luminosity"),
    None,
    Some("Subtract"),
    Some("Divide"),
];

/// The stored value (1-based) of a style blend mode by EffectCraft's name.
pub fn mode_code(name: &str) -> Option<u32> {
    STYLE_MODES.iter().position(|m| *m == Some(name)).and_then(|i| u32::try_from(i + 1).ok())
}

/// EffectCraft's name of a stored style blend mode; `None` for a separator or out of range.
pub fn mode_name(code: f64) -> Option<&'static str> {
    if !code.is_finite() || code < 1.0 {
        return None;
    }
    let i = code.round() as usize;
    *STYLE_MODES.get(i.checked_sub(1)?)?
}

/// A popup's stored value for an EffectCraft item (the inverse of `map`).
pub fn popup_code(map: &[u32], item: u32) -> Option<u32> {
    map.iter().position(|m| *m == item).and_then(|i| u32::try_from(i + 1).ok())
}

/// The EffectCraft item of a stored popup value.
pub fn popup_item(map: &[u32], code: f64) -> Option<u32> {
    if !code.is_finite() || code < 1.0 {
        return None;
    }
    map.get((code.round() as usize).checked_sub(1)?).copied()
}

use StyleConv as C;

const fn p(ae: &'static str, ec: &'static str, conv: StyleConv, default: &'static [f64]) -> StyleParam {
    StyleParam { ae, ec, conv, default }
}

/// Popups whose items are in the same order in both programs (1-based there, 0-based here).
const SAME2: &[u32] = &[0, 1];
const SAME3: &[u32] = &[0, 1, 2];
const SAME5: &[u32] = &[0, 1, 2, 3, 4];
/// Inner Glow Source: After Effects stores Edge as 1 and Center as 2 (955, 979); EffectCraft
/// lists Center, Edge.
const GLOW_SOURCE: &[u32] = &[1, 0];

const BLACK: &[f64] = &[255.0, 0.0, 0.0, 0.0];
const WHITE: &[f64] = &[255.0, 255.0, 255.0, 255.0];
const RED: &[f64] = &[255.0, 255.0, 0.0, 0.0];
const GLOW: &[f64] = &[255.0, 255.0, 255.0, 190.0];

/// The nine styles with a counterpart, in menu order. Defaults: the read-back of 951 (MANIFEST);
/// popup items: 977-980 and the read-back (E where only the stored number is known: Technique
/// Precise, Bevel Technique and Direction, Gradient Style beyond Linear).
pub const STYLES: &[StyleMap] = &[
    StyleMap {
        ae: "dropShadow",
        ec: "dropShadow",
        params: &[
            p("mode2", "blendMode", C::Mode, &[5.0]),
            p("color", "color", C::Color, BLACK),
            p("opacity", "opacity", C::Number, &[75.0]),
            p("useGlobalAngle", "useGlobalLight", C::Checkbox, &[0.0]),
            p("localLightingAngle", "angle", C::Number, &[120.0]),
            p("distance", "distance", C::Number, &[5.0]),
            p("chokeMatte", "spread", C::Number, &[0.0]),
            p("blur", "size", C::Number, &[5.0]),
            p("noise", "noise", C::Number, &[0.0]),
            p("layerConceals", "knocksOut", C::Checkbox, &[1.0]),
        ],
    },
    StyleMap {
        ae: "innerShadow",
        ec: "innerShadow",
        params: &[
            p("mode2", "blendMode", C::Mode, &[5.0]),
            p("color", "color", C::Color, BLACK),
            p("opacity", "opacity", C::Number, &[75.0]),
            p("useGlobalAngle", "useGlobalLight", C::Checkbox, &[0.0]),
            p("localLightingAngle", "angle", C::Number, &[120.0]),
            p("distance", "distance", C::Number, &[5.0]),
            p("chokeMatte", "choke", C::Number, &[0.0]),
            p("blur", "size", C::Number, &[5.0]),
            p("noise", "noise", C::Number, &[0.0]),
        ],
    },
    StyleMap {
        ae: "outerGlow",
        ec: "outerGlow",
        params: &[
            p("mode2", "blendMode", C::Mode, &[11.0]),
            p("opacity", "opacity", C::Number, &[75.0]),
            p("noise", "noise", C::Number, &[0.0]),
            p("AEColorChoice", "colorType", C::Popup(SAME2), &[1.0]),
            p("color", "color", C::Color, GLOW),
            p("gradient", "colors", C::Gradient, &[]),
            p("gradientSmoothness", "gradientSmoothness", C::Number, &[100.0]),
            p("glowTechnique", "technique", C::Popup(SAME2), &[1.0]),
            p("chokeMatte", "spread", C::Number, &[0.0]),
            p("blur", "size", C::Number, &[5.0]),
            p("inputRange", "range", C::Number, &[50.0]),
            p("shadingNoise", "jitter", C::Number, &[0.0]),
        ],
    },
    StyleMap {
        ae: "innerGlow",
        ec: "innerGlow",
        params: &[
            p("mode2", "blendMode", C::Mode, &[11.0]),
            p("opacity", "opacity", C::Number, &[75.0]),
            p("noise", "noise", C::Number, &[0.0]),
            p("AEColorChoice", "colorType", C::Popup(SAME2), &[1.0]),
            p("color", "color", C::Color, GLOW),
            p("gradient", "colors", C::Gradient, &[]),
            p("gradientSmoothness", "gradientSmoothness", C::Number, &[100.0]),
            p("glowTechnique", "technique", C::Popup(SAME2), &[1.0]),
            p("innerGlowSource", "source", C::Popup(GLOW_SOURCE), &[1.0]),
            p("chokeMatte", "choke", C::Number, &[0.0]),
            p("blur", "size", C::Number, &[5.0]),
            p("inputRange", "range", C::Number, &[50.0]),
            p("shadingNoise", "jitter", C::Number, &[0.0]),
        ],
    },
    StyleMap {
        ae: "bevelEmboss",
        ec: "bevelEmboss",
        params: &[
            p("bevelStyle", "style", C::Popup(SAME5), &[2.0]),
            p("bevelTechnique", "technique", C::Popup(SAME3), &[1.0]),
            p("strengthRatio", "depth", C::Number, &[100.0]),
            p("bevelDirection", "direction", C::Popup(SAME2), &[1.0]),
            p("blur", "size", C::Number, &[5.0]),
            p("softness", "soften", C::Number, &[0.0]),
            p("useGlobalAngle", "useGlobalLight", C::Checkbox, &[0.0]),
            p("localLightingAngle", "angle", C::Number, &[120.0]),
            p("localLightingAltitude", "altitude", C::Number, &[30.0]),
            p("highlightMode", "highlightMode", C::Mode, &[11.0]),
            p("highlightColor", "highlightColor", C::Color, WHITE),
            p("highlightOpacity", "highlightOpacity", C::Number, &[75.0]),
            p("shadowMode", "shadowMode", C::Mode, &[5.0]),
            p("shadowColor", "shadowColor", C::Color, BLACK),
            p("shadowOpacity", "shadowOpacity", C::Number, &[75.0]),
        ],
    },
    StyleMap {
        ae: "chromeFX",
        ec: "satin",
        params: &[
            p("mode2", "blendMode", C::Mode, &[5.0]),
            p("color", "color", C::Color, BLACK),
            p("opacity", "opacity", C::Number, &[50.0]),
            p("localLightingAngle", "angle", C::Number, &[19.0]),
            p("distance", "distance", C::Number, &[11.0]),
            p("blur", "size", C::Number, &[14.0]),
            p("invert", "invert", C::Checkbox, &[1.0]),
        ],
    },
    StyleMap {
        ae: "solidFill",
        ec: "colorOverlay",
        params: &[p("mode2", "blendMode", C::Mode, &[1.0]), p("color", "color", C::Color, RED), p("opacity", "opacity", C::Number, &[100.0])],
    },
    StyleMap {
        ae: "gradientFill",
        ec: "gradientOverlay",
        params: &[
            p("mode2", "blendMode", C::Mode, &[1.0]),
            p("opacity", "opacity", C::Number, &[100.0]),
            p("gradient", "colors", C::Gradient, &[]),
            p("gradientSmoothness", "gradientSmoothness", C::Number, &[100.0]),
            p("angle", "angle", C::Number, &[90.0]),
            p("type", "style", C::Popup(SAME5), &[1.0]),
            p("reverse", "reverse", C::Checkbox, &[0.0]),
            p("align", "alignWithLayer", C::Checkbox, &[1.0]),
            p("scale", "scale", C::Number, &[100.0]),
            p("offset", "offset", C::Vec2, &[0.0, 0.0]),
        ],
    },
    StyleMap {
        ae: "frameFX",
        ec: "stroke",
        params: &[
            p("mode2", "blendMode", C::Mode, &[1.0]),
            p("color", "color", C::Color, RED),
            p("size", "size", C::Number, &[3.0]),
            p("opacity", "opacity", C::Number, &[100.0]),
            p("style", "position", C::Popup(SAME3), &[1.0]),
        ],
    },
];

/// Advanced Blending (in Blending Options): full match name, EffectCraft id in
/// `blendingOptions/advancedBlending`, conversion and default (977; Fill Opacity 975). After
/// Effects has no Knockout.
pub const ADVANCED_BLENDING: &[StyleParam] = &[
    p("ADBE Layer Fill Opacity2", "fillOpacity", C::Number, &[100.0]),
    p("ADBE R Channel Blend", "red", C::Checkbox, &[1.0]),
    p("ADBE G Channel Blend", "green", C::Checkbox, &[1.0]),
    p("ADBE B Channel Blend", "blue", C::Checkbox, &[1.0]),
    p("ADBE Blend Interior", "blendInteriorAsGroup", C::Checkbox, &[0.0]),
    p("ADBE Blend Ranges", "useBlendRanges", C::Checkbox, &[1.0]),
];

/// The style with this switch-group prefix.
pub fn by_ae(prefix: &str) -> Option<&'static StyleMap> {
    STYLES.iter().find(|s| s.ae == prefix)
}

/// The style with this EffectCraft id.
pub fn by_ec(id: &str) -> Option<&'static StyleMap> {
    STYLES.iter().find(|s| s.ec == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_round_trip_and_defaults_are_the_photoshop_ones() {
        assert_eq!(mode_name(5.0), Some("Multiply"));
        assert_eq!(mode_name(11.0), Some("Screen"));
        assert_eq!(mode_name(1.0), Some("Normal"));
        assert_eq!(mode_name(3.0), None);
        assert_eq!(mode_name(34.0), None);
        assert_eq!(mode_name(0.0), None);
        assert_eq!(mode_name(f64::NAN), None);
        for (i, m) in STYLE_MODES.iter().enumerate() {
            if let Some(m) = m {
                assert_eq!(mode_code(m), Some(i as u32 + 1));
            }
        }
        assert_eq!(STYLE_MODES.iter().flatten().count(), 27);
    }

    #[test]
    fn popups_invert() {
        assert_eq!(popup_item(GLOW_SOURCE, 1.0), Some(1));
        assert_eq!(popup_item(GLOW_SOURCE, 2.0), Some(0));
        assert_eq!(popup_code(GLOW_SOURCE, 0), Some(2));
        assert_eq!(popup_item(SAME3, 4.0), None);
        assert_eq!(popup_item(SAME3, -1.0), None);
    }

    #[test]
    fn every_param_has_a_default_of_its_size() {
        for s in STYLES {
            for prm in s.params {
                let n = match prm.conv {
                    C::Color => 4,
                    C::Vec2 => 2,
                    C::Gradient => 0,
                    _ => 1,
                };
                assert_eq!(prm.default.len(), n, "{}/{}", s.ae, prm.ae);
            }
        }
    }
}
