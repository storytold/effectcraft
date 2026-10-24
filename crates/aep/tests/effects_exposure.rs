//! Batch 4, session S7 (cases 389-397): Exposure (`ADBE Exposure2`): the popup, the four
//! twirl-down groups (start / end markers that hold no value), float sliders with their valid and
//! slider ranges, the Bypass checkbox and a keyed slider. Every expected value is the one the case
//! was made with (its MANIFEST row).

mod common;

use common::{assert_near, first_comp, fixture, layer_with_id, open};
use effectcraft_aep::{DefinitionSource, EffectParam, Interpolation, KeyframeKind, ParamKind, ParamValue, Project};

const L: u32 = 0x1b;
/// Key time 2 s in the property timescale 25600.
const T2: i32 = 51200;
/// Float sliders hold the float32 value widened to f64 (0.1 is 0.10000000149).
const F32: f64 = 1e-6;

/// Runs `f` on the parameters of the single effect of `L`.
fn with_params<R>(name: &str, f: impl FnOnce(&[EffectParam<'_, '_>]) -> R) -> R {
    let bytes = fixture(name);
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let layer = layer_with_id(&first_comp(&project), L);
    let effects = layer.effects();
    assert_eq!(effects.len(), 1, "{name}");
    assert_eq!(effects[0].match_name(), "ADBE Exposure2", "{name}");
    assert_eq!(project.definition_source(&effects[0]), DefinitionSource::Instance, "{name}");
    let params = project.effect_params(&effects[0]);
    assert_eq!(params.len(), 24, "{name}");
    f(&params)
}

fn number(v: Option<ParamValue>) -> f64 {
    match v {
        Some(ParamValue::Number(n)) => n,
        other => panic!("{other:?} is not a number"),
    }
}

/// The `parT` indices of the Exposure / Offset / Gamma Correction sliders of Master, Red, Green
/// and Blue.
const GROUPS: [(usize, &str); 4] = [(2, "Master"), (7, "Red"), (12, "Green"), (17, "Blue")];

#[test]
fn exposure_parameters_and_defaults() {
    // 389: Exposure with every parameter at its default.
    let bytes = fixture("389-fx-exposure-default.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let fx = layer_with_id(&first_comp(&project), L).effects();
    assert_eq!((fx[0].match_name(), fx[0].name().as_deref()), ("ADBE Exposure2", Some("Exposure")));
    assert_eq!(fx[0].declared_param_count(), Some(24));
    with_params("389-fx-exposure-default.aep", |p| {
        assert_eq!((p[0].kind, p[1].kind, p[22].kind, p[23].kind), (ParamKind::Layer, ParamKind::Popup, ParamKind::Checkbox, ParamKind::NoData));
        assert_eq!(p[1].name, "Channels:");
        assert_eq!(p[1].items.as_deref(), Some(&["Master".to_string(), "Individual Channels".to_string()][..]));
        assert_eq!((p[1].value(), p[1].popup_item_count(), p[1].popup_selected_item()), (Some(ParamValue::Index(1)), Some(2), Some("Master")));
        for (start, group) in GROUPS {
            let prefix = if group == "Master" { String::new() } else { format!("{group} ") };
            // The group markers: always a record, no value, the end repeats the group's name.
            for (i, kind) in [(start, ParamKind::GroupStart), (start + 4, ParamKind::GroupEnd)] {
                assert_eq!((p[i].kind, p[i].name.as_str()), (kind, group));
                assert!(p[i].stored().is_some(), "{group} {kind:?}");
                assert_eq!((p[i].stored_value(), p[i].pard_default(), p[i].value(), p[i].is_default()), (None, None, None, None));
            }
            let names = [format!("{prefix}Exposure"), format!("{prefix}Offset"), format!("{prefix}Gamma Correction")];
            // Valid range, slider range and default per slider (the same for all four groups).
            let want = [((-100.0, 100.0), (-4.0, 4.0), 0.0), ((-2.0, 2.0), (-0.5, 0.5), 0.0), ((0.1, 10.0), (0.1, 10.0), 1.0)];
            for (k, (valid, slider, default)) in want.into_iter().enumerate() {
                let s = &p[start + 1 + k];
                assert_eq!((s.kind, s.name.as_str()), (ParamKind::FloatSlider, names[k].as_str()));
                assert_eq!(s.match_name, format!("ADBE Exposure2-{:04}", start + 1 + k));
                assert!(s.stored().is_none(), "{}", s.name);
                assert_eq!(s.value(), Some(ParamValue::Number(default)), "{}", s.name);
                assert_eq!(s.is_default(), Some(true));
                assert_near(&[s.valid_min().unwrap(), s.valid_max().unwrap()], &[valid.0, valid.1], F32);
                let (lo, hi) = s.slider_range().unwrap();
                assert_near(&[lo, hi], &[slider.0, slider.1], F32);
            }
        }
        // Bypass Linear Light Conversion: an unnamed checkbox labelled in pdnm, off.
        assert_eq!(p[22].name, "");
        assert_eq!(p[22].items.as_deref(), Some(&["Bypass Linear Light Conversion".to_string()][..]));
        assert!(p[22].stored().is_none());
        assert_eq!(p[22].value(), Some(ParamValue::Bool(false)));
        // Ranges exist only on float sliders.
        assert_eq!((p[1].valid_min(), p[1].slider_range(), p[22].slider_range(), p[2].valid_max()), (None, None, None, None));
        assert!(p.iter().all(|p| !p.borrowed_definition && !p.is_animated()));
    });
}

#[test]
fn exposure_values() {
    // 390: Channels -> Individual Channels (2).
    with_params("390-fx-exposure-channels-individual.aep", |p| {
        assert_eq!(p[1].stored_value(), Some(ParamValue::Index(2)));
        assert_eq!(p[1].popup_selected_item(), Some("Individual Channels"));
        assert_eq!(p[1].is_default(), Some(false));
    });
    // 391: Exposure 1.5.
    with_params("391-fx-exposure-master-1.5.aep", |p| {
        assert_eq!(p[3].stored_value(), Some(ParamValue::Number(1.5)));
        assert_eq!(p[3].is_default(), Some(false));
        assert_eq!(p[4].value(), Some(ParamValue::Number(0.0)));
    });
    // 393: Offset 0.1 (float32: 0.10000000149).
    with_params("393-fx-exposure-offset-0.1.aep", |p| {
        assert_near(&[number(p[4].stored_value())], &[0.1], F32);
        assert_eq!(p[3].value(), Some(ParamValue::Number(0.0)));
    });
    // 394: Gamma Correction 2.2 (float32: 2.2000000477).
    with_params("394-fx-exposure-gamma-2.2.aep", |p| {
        assert_near(&[number(p[5].stored_value())], &[2.2], F32);
        assert_eq!(p[5].is_default(), Some(false));
    });
    // 395: Channels 2 and Red Exposure 1: two ordinary records.
    with_params("395-fx-exposure-individual-red-1.aep", |p| {
        assert_eq!(p[1].stored_value(), Some(ParamValue::Index(2)));
        assert_eq!(p[8].stored_value(), Some(ParamValue::Number(1.0)));
        assert_eq!(p[3].value(), Some(ParamValue::Number(0.0)));
    });
    // 396: Bypass on: a record with 1.0.
    with_params("396-fx-exposure-bypass-on.aep", |p| {
        assert_eq!(p[22].stored_value(), Some(ParamValue::Bool(true)));
        assert_eq!(p[22].is_default(), Some(false));
    });
}

#[test]
fn exposure_keyframes() {
    // 397: Exposure keyed -1 at 0 s, 2 at 2 s, linear: plain 48-byte records.
    with_params("397-fx-exposure-master-2key.aep", |p| {
        let e = &p[3];
        assert!(e.is_animated());
        assert_eq!(e.stored_value(), None);
        let keys: Vec<_> = e.keys().unwrap().keyframes().collect();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys.iter().map(|k| k.time_units().unwrap()).collect::<Vec<_>>(), [0, T2]);
        assert_eq!(keys.iter().map(|k| k.scalar_value().unwrap()).collect::<Vec<_>>(), [-1.0, 2.0]);
        assert!(keys.iter().all(|k| k.kind() == KeyframeKind::Plain { components: 1 } && k.raw().len() == 48));
        assert!(keys.iter().all(|k| k.interpolation_in() == Some(Interpolation::Linear) && k.interpolation_out() == Some(Interpolation::Linear)));
    });
}
