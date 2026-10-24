//! Blend modes beyond the six of batch 1: 906 (13 solids over a test image, each with a mode set
//! by name) and the codes of 907 (each rendered by After Effects and matched against EffectCraft's
//! modes), here written into 906's top layer.

mod common;

use common::{comp, fixture_bytes, load};
use effectcraft_aep_import::{ImportOptions, LossCategory, import};
use effectcraft_color::BlendMode;
use effectcraft_project::Project;

const F906: &str = "906-blendmodes-13-solids-on-steps.aep";

#[test]
fn the_13_modes_of_906_import_lossless() {
    let (p, r) = load(F906);
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    let (_, c) = comp(&p, &r, "C");
    let mode = |name: &str| c.layers.iter().find(|l| l.name == name).unwrap_or_else(|| panic!("{name}")).blend_mode;
    for (name, want) in [
        ("normal", BlendMode::Normal),
        ("add", BlendMode::Add),
        ("multiply", BlendMode::Multiply),
        ("screen", BlendMode::Screen),
        ("overlay", BlendMode::Overlay),
        ("softLight", BlendMode::SoftLight),
        ("hardLight", BlendMode::HardLight),
        ("difference", BlendMode::Difference),
        ("exclusion", BlendMode::Exclusion),
        ("colorDodge", BlendMode::ColorDodge),
        ("colorBurn", BlendMode::ColorBurn),
        ("darken", BlendMode::Darken),
        ("lighten", BlendMode::Lighten),
    ] {
        assert_eq!(mode(name), want, "{name}");
    }
}

/// 906 with the top layer's code (`ldta` byte 99 of the first layer in the file) replaced.
fn with_code(code: u8) -> (Project, effectcraft_aep_import::ImportResult) {
    let mut b = fixture_bytes(F906);
    let at = b.windows(4).position(|w| w == b"ldta").expect("ldta") + 8 + 99;
    assert_eq!(b[at], 11, "the top layer is Lighten");
    b[at] = code;
    let none = |_: &str| false;
    let mut p = Project::default();
    let r = import(&mut p, &b, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).expect("imports");
    (p, r)
}

#[test]
fn the_codes_of_907_map_to_their_modes() {
    for (code, want) in [
        (12, BlendMode::ClassicDifference),
        (13, BlendMode::Hue),
        (14, BlendMode::Saturation),
        (15, BlendMode::Color),
        (16, BlendMode::Luminosity),
        (17, BlendMode::StencilAlpha),
        (18, BlendMode::StencilLuma),
        (19, BlendMode::SilhouetteAlpha),
        (20, BlendMode::SilhouetteLuma),
        (23, BlendMode::ClassicColorDodge),
        (24, BlendMode::ClassicColorBurn),
        (29, BlendMode::LinearDodge),
        (30, BlendMode::LinearBurn),
        (31, BlendMode::LinearLight),
        (32, BlendMode::VividLight),
        (33, BlendMode::PinLight),
        (34, BlendMode::HardMix),
    ] {
        let (p, r) = with_code(code);
        assert!(r.report.is_lossless(), "{code}: {:?}", r.report.losses);
        let (_, c) = comp(&p, &r, "C");
        assert_eq!(c.layers.iter().find(|l| l.name == "lighten").unwrap().blend_mode, want, "{code}");
    }
}

#[test]
fn unsampled_and_ambiguous_codes_stay_reported() {
    for code in [0u8, 1, 3, 21, 22, 35, 200, 255] {
        let (p, r) = with_code(code);
        assert_eq!(r.report.count(LossCategory::BlendModes), 1, "{code}");
        let (_, c) = comp(&p, &r, "C");
        assert_eq!(c.layers.iter().find(|l| l.name == "lighten").unwrap().blend_mode, BlendMode::Normal, "{code}");
    }
}
