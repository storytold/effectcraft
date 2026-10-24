//! Effects of batch 5 (cases 409-450, 511-516): integer sliders, checkbox defaults, the stored
//! scale of each slider (its valid maximum), popup items, and the arbitrary data of Hue/Saturation
//! and Levels.

mod common;

use common::{first_comp, fixture, open};
use effectcraft_aep::{EffectParam, HueSaturationData, LevelsData, ParamKind, ParamValue, Project};

/// Runs `f` on the parameters of the first effect of layer L (the only layer).
fn with_params<R>(name: &str, f: impl FnOnce(&[EffectParam<'_, '_>], &effectcraft_aep::Effect<'_, '_>) -> R) -> R {
    let bytes = fixture(name);
    let file = open(&bytes);
    let project = Project::new(&file).expect("project");
    let comp = first_comp(&project);
    let layer = comp.layers().next().expect("L");
    let fx = layer.effects().into_iter().next().expect("an effect");
    let params = project.effect_params(&fx);
    f(&params, &fx)
}

fn by_name<'p, 'f, 'a>(params: &'p [EffectParam<'f, 'a>], name: &str) -> &'p EffectParam<'f, 'a> {
    params.iter().find(|p| p.name == name || p.items.as_ref().is_some_and(|i| i.first().is_some_and(|f| f == name))).unwrap_or_else(|| panic!("no {name}"))
}

#[test]
fn invert_channel_items_and_blend() {
    with_params("410-fx-invert-channel-2.aep", |p, fx| {
        assert_eq!(fx.match_name(), "ADBE Invert");
        let ch = by_name(p, "Channel");
        assert_eq!(ch.kind, ParamKind::Popup);
        // The items include separators "(-" (one index each).
        assert_eq!(ch.items.as_ref().map(|i| (i.len(), i[1].as_str(), i[4].as_str(), i[15].as_str())), Some((16, "Red", "(-", "Alpha")));
        assert_eq!(ch.stored_value(), Some(ParamValue::Index(2)));
        assert_eq!(by_name(p, "Blend With Original").valid_max(), Some(100.0));
    });
    with_params("411-fx-invert-blend-50.aep", |p, _| assert_eq!(by_name(p, "Blend With Original").stored_value(), Some(ParamValue::Number(50.0))));
}

#[test]
fn fast_box_blur_integer_slider_and_checkbox_default_on() {
    with_params("413-fx-fastbox-radius-20.aep", |p, _| {
        let r = by_name(p, "Blur Radius");
        assert_eq!((r.kind, r.valid_max(), r.stored_value()), (ParamKind::FixedSlider, Some(30000.0), Some(ParamValue::Number(20.0))));
        let it = by_name(p, "Iterations");
        assert_eq!((it.kind, it.pard_default(), it.valid_max(), it.stored_value()), (ParamKind::IntSlider, Some(ParamValue::Number(3.0)), Some(50.0), None));
        // Repeat Edge Pixels is on by default (AE read-back 1): the pard default, no record.
        let rep = by_name(p, "Repeat Edge Pixels");
        assert_eq!((rep.kind, rep.pard_default(), rep.value()), (ParamKind::Checkbox, Some(ParamValue::Bool(true)), Some(ParamValue::Bool(true))));
    });
    with_params("414-fx-fastbox-iterations-1.aep", |p, _| assert_eq!(by_name(p, "Iterations").stored_value(), Some(ParamValue::Number(1.0))));
    with_params("416-fx-fastbox-repeatedge-off.aep", |p, _| {
        let rep = by_name(p, "Repeat Edge Pixels");
        assert_eq!((rep.stored_value(), rep.value(), rep.is_default()), (Some(ParamValue::Bool(false)), Some(ParamValue::Bool(false)), Some(false)));
    });
}

#[test]
fn gaussian_blur_dimensions_and_repeat_edge() {
    with_params("418-fx-gaussian-dimensions-3.aep", |p, _| {
        assert_eq!(by_name(p, "Blur Dimensions").stored_value(), Some(ParamValue::Index(3)));
        assert_eq!(by_name(p, "Blurriness").pard_default(), Some(ParamValue::Number(25.0)));
        assert_eq!(by_name(p, "Repeat Edge Pixels").value(), Some(ParamValue::Bool(true)));
    });
    with_params("419-fx-gaussian-repeatedge-off.aep", |p, _| assert_eq!(by_name(p, "Repeat Edge Pixels").value(), Some(ParamValue::Bool(false))));
}

#[test]
fn drop_shadow_opacity_is_stored_on_0_to_255() {
    with_params("422-fx-dropshadow-opacity-204.aep", |p, _| {
        let o = by_name(p, "Opacity");
        assert_eq!((o.valid_max(), o.pard_default(), o.stored_value()), (Some(255.0), Some(ParamValue::Number(127.5)), Some(ParamValue::Number(204.0))));
        assert_eq!(by_name(p, "Distance").valid_max(), Some(4000.0));
        assert_eq!(by_name(p, "Shadow Only").pard_default(), Some(ParamValue::Bool(false)));
    });
    with_params("426-fx-dropshadow-shadowonly-on.aep", |p, _| assert_eq!(by_name(p, "Shadow Only").stored_value(), Some(ParamValue::Bool(true))));
}

#[test]
fn glow_defaults_items_and_scales() {
    with_params("427-fx-glow-default.aep", |p, _| {
        let popup = |n: &str| (by_name(p, n).pard_default(), by_name(p, n).items.as_ref().map(|i| i.len()));
        assert_eq!(popup("Glow Based On"), (Some(ParamValue::Index(2)), Some(2)));
        assert_eq!(popup("Composite Original"), (Some(ParamValue::Index(2)), Some(3)));
        assert_eq!(popup("Glow Operation"), (Some(ParamValue::Index(3)), Some(25)));
        assert_eq!(popup("Color Looping"), (Some(ParamValue::Index(3)), Some(4)));
        let t = by_name(p, "Glow Threshold");
        assert_eq!((t.valid_max(), t.pard_default()), (Some(255.0), Some(ParamValue::Number(153.0))));
        let m = by_name(p, "A & B Midpoint");
        assert_eq!(m.pard_default(), Some(ParamValue::Number(0.5)));
        assert!(m.valid_max().is_some_and(|v| (v - 0.99).abs() < 0.001));
    });
    with_params("429-fx-glow-threshold-76.5.aep", |p, _| assert_eq!(by_name(p, "Glow Threshold").stored_value(), Some(ParamValue::Number(76.5))));
    with_params("438-fx-glow-midpoint-0.25.aep", |p, _| assert_eq!(by_name(p, "A & B Midpoint").stored_value(), Some(ParamValue::Number(0.25))));
}

#[test]
fn hue_saturation_master_values_are_arbitrary_data() {
    for (name, master) in [("444-fx-huesat-hue-90.aep", [90, 0, 0]), ("446-fx-huesat-light-m30.aep", [0, 0, -30])] {
        with_params(name, |p, fx| {
            // The visible Master parameters keep no record.
            assert!(by_name(p, "Master Hue").stored().is_none(), "{name}");
            let range = by_name(p, "Channel Range");
            assert_eq!(range.kind, ParamKind::ArbitraryData);
            let data = fx.arbitrary_data(&range.match_name).expect("data");
            let d = HueSaturationData::parse(data).expect("180 bytes");
            assert_eq!(d.master, master, "{name}");
            assert!(d.ranges_are_default(), "{name}");
        });
    }
    with_params("448-fx-huesat-colorizehue-120.aep", |p, fx| {
        assert_eq!(by_name(p, "Colorize Hue").stored_value(), Some(ParamValue::Number(120.0)));
        assert_eq!(by_name(p, "Colorize Saturation").pard_default(), Some(ParamValue::Number(25.0)));
        let d = HueSaturationData::parse(fx.arbitrary_data(&by_name(p, "Channel Range").match_name).expect("data")).expect("parses");
        assert_eq!(d.master, [0, 0, 0]);
    });
}

#[test]
fn levels_values_are_arbitrary_data() {
    let f32r = |x: f64| f32::from_bits((x as f32).to_bits());
    for (name, rgb) in [
        ("512-fx-levels-inblack-0.1.aep", [f32r(0.1), 1.0, 1.0, 0.0, 1.0]),
        ("514-fx-levels-gamma-2.aep", [0.0, 1.0, 2.0, 0.0, 1.0]),
        ("516-fx-levels-outwhite-0.9.aep", [0.0, 1.0, 1.0, 0.0, f32r(0.9)]),
    ] {
        with_params(name, |p, fx| {
            assert!(by_name(p, "Input Black").stored().is_none(), "{name}: the visible parameter keeps no record");
            let h = by_name(p, "Histogram");
            assert_eq!(h.kind, ParamKind::ArbitraryData);
            let d = LevelsData::parse(fx.arbitrary_data(&h.match_name).expect("data")).expect("108 bytes");
            assert_eq!(d.header, [0x000f_10a7, 1], "{name}");
            assert_eq!(d.channels[0], rgb, "{name}");
            for c in &d.channels[1..] {
                assert_eq!(*c, LevelsData::DEFAULT_CHANNEL, "{name}");
            }
            let clip = by_name(p, "Clip To Output Black");
            assert_eq!(clip.items.as_ref().map(|i| i.join("|")), Some("On|Off|Off for 32 bpc Color".to_string()));
            assert_eq!(clip.pard_default(), Some(ParamValue::Index(3)));
        });
    }
}

#[test]
fn hostile_arbitrary_data_is_none() {
    assert_eq!(HueSaturationData::parse(&[0; 179]), None);
    assert_eq!(LevelsData::parse(&[0; 200]), None);
    assert!(HueSaturationData::parse(&[0xff; 180]).is_some());
}
