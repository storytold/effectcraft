//! Motion Tile, Shift Channels and Emboss arrive as EffectCraft's effects of the same name. Each
//! is built into `synthetic.aep` (`fx_inject`) with the definitions After Effects 2026 wrote for it
//! in its effect sweep (valid ranges, defaults, popup items).

use crate::import::fx_inject::{Default, Fx, losses, near, other, popup, run, slider, v};
use effectcraft_aep::ParamKind as K;
use effectcraft_aep::import::LossCategory;
use effectcraft_keyframe::Value;

const SHIFT: &[&str] = &["Alpha", "Red", "Green", "Blue", "Luminance", "Hue", "Lightness", "Saturation", "Full On", "Full Off"];

fn motion_tile(values: Vec<(usize, f64)>) -> Fx {
    Fx {
        match_name: "ADBE Tile",
        name: "Motion Tile",
        defs: vec![
            other(K::Point, "Tile Center", Default::Point(0.5, 0.5)),
            slider(K::FixedSlider, "Tile Width", 100.0, (0.0, 100.0)),
            slider(K::FixedSlider, "Tile Height", 100.0, (0.0, 100.0)),
            slider(K::FixedSlider, "Output Width", 100.0, (0.0, 30000.0)),
            slider(K::FixedSlider, "Output Height", 100.0, (0.0, 30000.0)),
            other(K::Checkbox, " ", Default::Number(0.0)),
            other(K::Angle, "Phase", Default::Number(0.0)),
            other(K::Checkbox, " ", Default::Number(0.0)),
        ],
        values,
        data: vec![],
        keys: vec![],
    }
}

/// A zoomed-out transition: Output 550 % with Mirror Edges, so the edges show mirrored copies
/// (the effect was dropped and the layer showed a black border around the scaled frame).
#[test]
fn motion_tile_output_size_and_mirror_edges() {
    let (g, r) = run(&motion_tile(vec![(2, 50.0), (4, 550.0), (5, 550.0), (6, 1.0), (7, 90.0), (8, 1.0)]), "ec.stylize.motiontile");
    assert_eq!(losses(&r, LossCategory::Effects, "ADBE Tile"), Vec::<String>::new());
    let g = g.expect("imported");
    near(&g, "tileWidth", 50.0);
    near(&g, "tileHeight", 100.0);
    near(&g, "outputWidth", 550.0);
    near(&g, "outputHeight", 550.0);
    assert_eq!(v(&g, "mirrorEdges"), Value::Bool(true));
    near(&g, "phase", 90.0);
    assert_eq!(v(&g, "horizontalPhaseShift"), Value::Bool(true));
    let (g, _) = run(&motion_tile(vec![]), "ec.stylize.motiontile");
    let g = g.expect("imported");
    near(&g, "outputWidth", 100.0);
    assert_eq!(v(&g, "mirrorEdges"), Value::Bool(false));
    let Value::Vec2(c) = v(&g, "tileCenter") else { panic!("tileCenter") };
    assert!(c.iter().all(|x| x.is_finite()));
}

/// The four menus, mapped by item name; the defaults are Alpha, Red, Green and Blue.
#[test]
fn shift_channels_menus() {
    let fx = |values| Fx {
        match_name: "ADBE Shift Channels",
        name: "Shift Channels",
        defs: vec![popup("Take Alpha From", 1, SHIFT), popup("Take Red From", 2, SHIFT), popup("Take Green From", 3, SHIFT), popup("Take Blue From", 4, SHIFT)],
        values,
        data: vec![],
        keys: vec![],
    };
    let (g, r) = run(&fx(vec![(2, 3.0), (4, 10.0)]), "ec.channel.shiftchannels");
    assert_eq!(losses(&r, LossCategory::Effects, "ADBE Shift Channels"), Vec::<String>::new());
    let g = g.expect("imported");
    assert_eq!(v(&g, "takeRedFrom"), Value::Enum(2), "Green");
    assert_eq!(v(&g, "takeBlueFrom"), Value::Enum(9), "Full Off");
    assert_eq!((v(&g, "takeAlphaFrom"), v(&g, "takeGreenFrom")), (Value::Enum(0), Value::Enum(2)));
}

/// Emboss's four parameters, set and default (After Effects' Relief default is 1).
#[test]
fn emboss_parameters() {
    let fx = |values| Fx {
        match_name: "ADBE Emboss",
        name: "Emboss",
        defs: vec![
            other(K::Angle, "Direction", Default::Number(45.0)),
            slider(K::FixedSlider, "Relief", 1.0, (0.0, 1000.0)),
            slider(K::IntSlider, "Contrast", 100.0, (0.0, 32767.0)),
            slider(K::FixedSlider, "Blend With Original", 0.0, (0.0, 100.0)),
        ],
        values,
        data: vec![],
        keys: vec![],
    };
    let (g, r) = run(&fx(vec![(1, 120.0), (2, 4.5), (3, 250.0), (4, 30.0)]), "ec.stylize.emboss");
    assert_eq!(losses(&r, LossCategory::Effects, "ADBE Emboss"), Vec::<String>::new());
    let g = g.expect("imported");
    near(&g, "direction", 120.0);
    near(&g, "relief", 4.5);
    near(&g, "contrast", 250.0);
    near(&g, "blend", 30.0);
    let (g, _) = run(&fx(vec![]), "ec.stylize.emboss");
    let g = g.expect("imported");
    near(&g, "relief", 1.0);
    near(&g, "direction", 45.0);
}
