//! Checkerboard arrives as EffectCraft's Checkerboard. It is built into `synthetic.aep`
//! (`fx_inject`) with the definitions After Effects 26.5 wrote for it (lab fx-tile: valid ranges,
//! defaults, popup items) and that file's values.

use crate::import::fx_inject::{Default, Fx, losses, near, other, popup, pt, run, slider, v};
use effectcraft_aep::ParamKind as K;
use effectcraft_aep::import::LossCategory;
use effectcraft_keyframe::Value;

const SIZE_FROM: &[&str] = &["Corner Point", "Width Slider", "Width & Height Sliders"];
const BLEND: &[&str] = &[
    "None",
    "Normal",
    "Stencil Alpha",
    "(-",
    "Add",
    "Multiply",
    "Screen",
    "Overlay",
    "Soft Light",
    "Hard Light",
    "(-",
    "Color Dodge",
    "Color Burn",
    "(-",
    "Darken",
    "Lighten",
    "Difference",
    "Exclusion",
    "(-",
    "Hue",
    "Saturation",
    "Color",
    "Luminosity",
];

/// Checkerboard with the definitions After Effects 26.5 wrote and the stored `values`.
fn checkerboard(values: Vec<(usize, f64)>) -> Fx {
    checkerboard_with_size_from_slot(values, 2)
}

/// Checkerboard whose Size From definition holds `slot` in its default slot.
fn checkerboard_with_size_from_slot(values: Vec<(usize, f64)>, slot: u32) -> Fx {
    Fx {
        match_name: "ADBE Checkerboard",
        name: "Checkerboard",
        defs: vec![
            other(K::Point, "Anchor", Default::Point(0.5, 0.5)),
            popup("Size From", slot, SIZE_FROM),
            other(K::Point, "Corner", Default::Point(0.58, 0.58)),
            slider(K::FixedSlider, "Width", 16.0, (0.0, 4000.0)),
            slider(K::FixedSlider, "Height", 8.0, (0.0, 4000.0)),
            other(K::GroupStart, "Feather", Default::None),
            slider(K::FixedSlider, "Width", 0.0, (0.0, 400.0)),
            slider(K::FixedSlider, "Height", 0.0, (0.0, 400.0)),
            other(K::GroupEnd, "", Default::None),
            other(K::Color, "Color", Default::Color([255, 255, 255, 255])),
            slider(K::FixedSlider, "Opacity", 100.0, (0.0, 100.0)),
            popup("Blending Mode", 1, BLEND),
        ],
        values,
        data: vec![],
        keys: vec![],
    }
}

/// The lab file's Checkerboard: Width Slider, Width 40, Opacity 35, Normal; no loss.
#[test]
fn checkerboard_values_are_imported() {
    let (g, r) = run(&checkerboard(vec![(2, 2.0), (4, 40.0), (7, 3.0), (11, 35.0), (12, 2.0)]), "ec.generate.checkerboard");
    assert_eq!(losses(&r, LossCategory::Effects, "ADBE Checkerboard"), Vec::<String>::new());
    let g = g.expect("imported");
    assert_eq!(v(&g, "sizeFrom"), Value::Enum(1), "Width Slider");
    near(&g, "width", 40.0);
    near(&g, "feather/featherWidth", 3.0);
    near(&g, "opacity", 35.0);
    assert_eq!(v(&g, "blendingMode"), Value::Enum(1), "Normal");
}

/// The defaults: anchor at the layer's centre (synthetic.aep's layer is 8x8), None.
#[test]
fn checkerboard_defaults() {
    let (g, _) = run(&checkerboard(vec![]), "ec.generate.checkerboard");
    let g = g.expect("imported");
    pt(&g, "anchor", [4.0, 4.0]);
    assert_eq!(v(&g, "sizeFrom"), Value::Enum(1), "Width Slider");
    near(&g, "width", 16.0);
    near(&g, "height", 8.0);
    assert_eq!(v(&g, "blendingMode"), Value::Enum(0), "None");
}

/// Size From without a record and without a usable default slot (0: an instance whose
/// definitions are another copy's, or After Effects' save of a project it opened) is After
/// Effects' fresh default, Width Slider, not Corner Point (lab fx-tile: three Checkerboards set
/// to Width Slider, only one with its own definitions; none has a Size From record).
#[test]
fn size_from_without_record_or_default_is_width_slider() {
    let (g, r) = run(&checkerboard_with_size_from_slot(vec![(4, 40.0)], 0), "ec.generate.checkerboard");
    assert_eq!(losses(&r, LossCategory::Effects, "ADBE Checkerboard"), Vec::<String>::new());
    assert_eq!(v(&g.expect("imported"), "sizeFrom"), Value::Enum(1), "Width Slider");
}
