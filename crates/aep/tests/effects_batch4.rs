//! Batch 4 (cases 356-383): Tint and Gradient Ramp parameters, their defaults, values, keys and
//! scales, the effect stack order, and the second instance of an effect type whose `parT` is
//! empty. Every expected value is the one the case was made with (its MANIFEST row).

mod common;

use common::{assert_near, first_comp, fixture, layer_with_id, open};
use effectcraft_aep::{DefinitionSource, EffectParam, Interpolation, KeyframeKind, ParamKind, ParamValue, Project};

const L: u32 = 0x1b;
/// Key time 2 s in the property timescale 25600.
const T2: i32 = 51200;

/// Runs `f` on the project view and the parameters of the single effect of `L`.
fn with_params<R>(name: &str, f: impl FnOnce(&Project<'_, '_>, &[EffectParam<'_, '_>]) -> R) -> R {
    let bytes = fixture(name);
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let layer = layer_with_id(&first_comp(&project), L);
    let effects = layer.effects();
    assert_eq!(effects.len(), 1, "{name}");
    assert_eq!(project.definition_source(&effects[0]), DefinitionSource::Instance, "{name}");
    let params = project.effect_params(&effects[0]);
    f(&project, &params)
}

fn argb(v: Option<ParamValue>) -> [f64; 4] {
    match v {
        Some(ParamValue::Argb(c)) => c,
        other => panic!("{other:?} is not a colour"),
    }
}

fn point(v: Option<ParamValue>) -> [f64; 2] {
    match v {
        Some(ParamValue::Point(p)) => p,
        other => panic!("{other:?} is not a point"),
    }
}

fn number(v: Option<ParamValue>) -> f64 {
    match v {
        Some(ParamValue::Number(n)) => n,
        other => panic!("{other:?} is not a number"),
    }
}

/// The colour as the case set it: float32 precision (0.2 becomes 0.20000000298).
const F32: f64 = 1e-6;

// ---------------------------------------------------------------------------- Tint

#[test]
fn tint_parameters_and_defaults() {
    // 356: Tint with every parameter at its default.
    let bytes = fixture("356-fx-tint-default.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let fx = layer_with_id(&first_comp(&project), L).effects();
    let e = &fx[0];
    assert_eq!((e.match_name(), e.name().as_deref()), ("ADBE Tint", Some("Tint")));
    assert_eq!(e.declared_param_count(), Some(6));
    let p = project.effect_params(e);
    let kinds: Vec<_> = p.iter().map(|p| (p.kind, p.name.as_str())).collect();
    assert_eq!(
        kinds,
        [
            (ParamKind::Layer, ""),
            (ParamKind::Color, "Map Black To"),
            (ParamKind::Color, "Map White To"),
            (ParamKind::FixedSlider, "Amount to Tint"),
            (ParamKind::Button, ""),
            (ParamKind::NoData, ""),
        ]
    );
    assert_eq!(p[4].match_name, "ADBE Tint-0004");
    // The defaults have alpha 0 (what After Effects reads back: [0,0,0,0] / [1,1,1,0]).
    assert_eq!(p[1].value(), Some(ParamValue::Argb([0.0, 0.0, 0.0, 0.0])));
    assert_eq!(p[2].value(), Some(ParamValue::Argb([0.0, 1.0, 1.0, 1.0])));
    assert!(p[1].stored().is_none() && p[2].stored().is_none() && p[3].stored().is_none());
    // Amount to Tint: 100 percent, stored as a plain percent (valid max 100).
    assert_eq!(p[3].value(), Some(ParamValue::Number(100.0)));
    assert_eq!(p[3].valid_max(), Some(100.0));
    // Swap Colors: a button whose record always exists and holds no value; its label is in pdnm.
    assert!(p[4].stored().is_some());
    assert_eq!(p[4].value(), None);
    assert_eq!(p[4].is_default(), None);
    assert_eq!(p[4].items.as_deref(), Some(&["Swap Colors".to_string()][..]));
    assert!(p.iter().all(|p| !p.borrowed_definition && !p.is_animated()));
}

#[test]
fn tint_colour_and_amount_values() {
    // 357: Map Black To [0.2, 0.4, 0.6], stored with alpha 255.
    with_params("357-fx-tint-black-0.2-0.4-0.6.aep", |_, p| {
        assert_near(&argb(p[1].stored_value()), &[1.0, 0.2, 0.4, 0.6], F32);
        assert_eq!(p[1].is_default(), Some(false));
        assert_eq!(p[2].value(), Some(ParamValue::Argb([0.0, 1.0, 1.0, 1.0])));
    });
    // 361: Amount to Tint 0 writes a record with 0.
    with_params("361-fx-tint-amount-0.aep", |_, p| {
        assert_eq!(p[3].stored_value(), Some(ParamValue::Number(0.0)));
        assert_eq!(p[3].is_default(), Some(false));
    });
}

#[test]
fn tint_keyframes() {
    // 363: Amount to Tint keyed 20 at 0 s, 80 at 2 s, linear: plain 48-byte records.
    with_params("363-fx-tint-amount-2key.aep", |_, p| {
        let amount = &p[3];
        assert!(amount.is_animated());
        assert_eq!(amount.stored_value(), None);
        let keys: Vec<_> = amount.keys().unwrap().keyframes().collect();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys.iter().map(|k| k.time_units().unwrap()).collect::<Vec<_>>(), [0, T2]);
        assert_eq!(keys.iter().map(|k| k.scalar_value().unwrap()).collect::<Vec<_>>(), [20.0, 80.0]);
        assert!(keys.iter().all(|k| k.kind() == KeyframeKind::Plain { components: 1 } && k.interpolation_out() == Some(Interpolation::Linear)));
    });
    // 364: Map White To keyed [1,1,0] -> [0,0,1]: 152-byte colour records, ARGB 0..255.
    with_params("364-fx-tint-white-2key.aep", |_, p| {
        let white = &p[2];
        assert!(white.is_animated());
        let keys: Vec<_> = white.keys().unwrap().keyframes().collect();
        assert_eq!(keys.len(), 2);
        assert!(keys.iter().all(|k| k.kind() == KeyframeKind::Spatial { components: 4 } && k.raw().len() == 152));
        assert_eq!(keys.iter().map(|k| k.time_units().unwrap()).collect::<Vec<_>>(), [0, T2]);
        assert_near(&keys[0].value().unwrap(), &[255.0, 255.0, 255.0, 0.0], 1e-9);
        assert_near(&keys[1].value().unwrap(), &[255.0, 0.0, 0.0, 255.0], 1e-9);
    });
}

#[test]
fn effect_stack_order_is_the_parade_order() {
    // 366: Gaussian Blur moved above the Tint.
    let bytes = fixture("366-fx-blur-then-tint.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let fx = layer_with_id(&first_comp(&project), L).effects();
    assert_eq!(fx.iter().map(|e| e.match_name()).collect::<Vec<_>>(), ["ADBE Gaussian Blur 2", "ADBE Tint"]);
    // Both are the only instance of their type and keep their own definitions; the project copy
    // follows the stack order.
    assert!(fx.iter().all(|e| project.definition_source(e) == DefinitionSource::Instance));
    let defs: Vec<String> = project.effect_definitions().into_iter().map(|(m, _)| m).collect();
    assert_eq!(defs, ["ADBE Gaussian Blur 2", "ADBE Tint"]);
}

// ---------------------------------------------------------------------------- Gradient Ramp

#[test]
fn ramp_parameters_and_defaults() {
    // 368: Gradient Ramp with every parameter at its default.
    with_params("368-fx-ramp-default.aep", |_, p| {
        let kinds: Vec<_> = p.iter().map(|p| (p.kind, p.name.as_str())).collect();
        assert_eq!(
            kinds,
            [
                (ParamKind::Layer, ""),
                (ParamKind::Point, "Start of Ramp"),
                (ParamKind::Color, "Start Color"),
                (ParamKind::Point, "End of Ramp"),
                (ParamKind::Color, "End Color"),
                (ParamKind::Popup, "Ramp Shape"),
                (ParamKind::FixedSlider, "Ramp Scatter"),
                (ParamKind::FixedSlider, "Blend With Original"),
                (ParamKind::Button, ""),
                (ParamKind::NoData, ""),
            ]
        );
        // Start [960,0] and End [960,1080] on the 1920x1080 layer: fractions of the layer size.
        assert_eq!(p[1].value(), Some(ParamValue::Point([0.5, 0.0])));
        assert_eq!(p[3].value(), Some(ParamValue::Point([0.5, 1.0])));
        assert_eq!(p[1].point_px(1920.0, 1080.0), Some([960.0, 0.0]));
        assert_eq!(p[3].point_px(1920.0, 1080.0), Some([960.0, 1080.0]));
        // Black to white with alpha 255.
        assert_eq!(p[2].value(), Some(ParamValue::Argb([1.0, 0.0, 0.0, 0.0])));
        assert_eq!(p[4].value(), Some(ParamValue::Argb([1.0, 1.0, 1.0, 1.0])));
        // Linear Ramp is item 1 of 2.
        assert_eq!(p[5].value(), Some(ParamValue::Index(1)));
        assert_eq!(p[5].popup_item_count(), Some(2));
        assert_eq!(p[5].popup_selected_item(), Some("Linear Ramp"));
        // Scatter 0 (pixels, valid max 512); Blend 0 (a 0..1 fraction, valid max 1).
        assert_eq!((p[6].value(), p[6].valid_max()), (Some(ParamValue::Number(0.0)), Some(512.0)));
        assert_eq!((p[7].value(), p[7].valid_max()), (Some(ParamValue::Number(0.0)), Some(1.0)));
        assert!(p[8].stored().is_some() && p[8].value().is_none());
        assert!((1..=7).all(|i| p[i].stored().is_none()));
    });
}

#[test]
fn ramp_values() {
    // 369: Start of Ramp [100, 200] -> [100/1920, 200/1080].
    with_params("369-fx-ramp-start-100-200.aep", |_, p| {
        assert_near(&point(p[1].stored_value()), &[100.0 / 1920.0, 200.0 / 1080.0], 1e-12);
        assert_near(&p[1].point_px(1920.0, 1080.0).unwrap(), &[100.0, 200.0], 1e-9);
    });
    // 371: Start Color red.
    with_params("371-fx-ramp-startcolor-red.aep", |_, p| {
        assert_eq!(p[2].stored_value(), Some(ParamValue::Argb([1.0, 1.0, 0.0, 0.0])));
    });
    // 374: End of Ramp [2000, -100], outside the layer.
    with_params("374-fx-ramp-end-2000-m100.aep", |_, p| {
        assert_near(&p[3].point_px(1920.0, 1080.0).unwrap(), &[2000.0, -100.0], 1e-9);
    });
    // 377: Ramp Shape 2 = Radial Ramp.
    with_params("377-fx-ramp-shape-radial.aep", |_, p| {
        assert_eq!(p[5].stored_value(), Some(ParamValue::Index(2)));
        assert_eq!(p[5].popup_selected_item(), Some("Radial Ramp"));
    });
    // 378: Ramp Scatter 12 (plain pixels).
    with_params("378-fx-ramp-scatter-12.aep", |_, p| assert_eq!(number(p[6].stored_value()), 12.0));
    // 380: Blend With Original 25 % is stored as the fraction 0.25.
    with_params("380-fx-ramp-blend-25pct.aep", |_, p| {
        assert_eq!(number(p[7].stored_value()), 0.25);
        assert_eq!(p[7].valid_max(), Some(1.0));
    });
}

#[test]
fn ramp_point_keyframes() {
    // 382: Start of Ramp keyed [100,100] at 0 s, [1800,900] at 2 s, linear: 104-byte spatial records.
    with_params("382-fx-ramp-start-2key.aep", |_, p| {
        let start = &p[1];
        assert!(start.is_animated());
        let keys: Vec<_> = start.keys().unwrap().keyframes().collect();
        assert_eq!(keys.len(), 2);
        assert!(keys.iter().all(|k| k.kind() == KeyframeKind::Spatial { components: 2 } && k.raw().len() == 104));
        assert_eq!(keys.iter().map(|k| k.time_units().unwrap()).collect::<Vec<_>>(), [0, T2]);
        assert_near(&keys[0].value().unwrap(), &[100.0 / 1920.0, 100.0 / 1080.0], 1e-12);
        assert_near(&keys[1].value().unwrap(), &[1800.0 / 1920.0, 900.0 / 1080.0], 1e-12);
        assert!(keys.iter().all(|k| k.interpolation_in() == Some(Interpolation::Linear)));
        let s = keys[0].spatial().unwrap();
        assert_eq!((s.auto_bezier_flag, s.in_tangent.clone(), s.out_tangent.clone()), (1, vec![0.0; 2], vec![0.0; 2]));
    });
}

#[test]
fn second_instance_takes_its_definitions_from_the_project_and_its_values_from_itself() {
    // 383: L keeps its default Gradient Ramp; the new 100x100 solid S100 above it has a Ramp with
    // Start of Ramp [10, 20]. One of the two instances has an empty parT.
    let bytes = fixture("383-fx-ramp-on-100x100-start-10-20.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let comp = first_comp(&project);
    let layers: Vec<_> = comp.layers().collect();
    assert_eq!(layers.len(), 2);
    let small = layers.iter().find(|l| l.id() != Some(L)).unwrap();
    let l = layer_with_id(&comp, L);

    let small_fx = small.effects();
    let l_fx = l.effects();
    assert_eq!((small_fx.len(), l_fx.len()), (1, 1));
    assert_eq!(project.definition_source(&small_fx[0]), DefinitionSource::Instance);
    assert_eq!(project.definition_source(&l_fx[0]), DefinitionSource::Project);
    assert!(l_fx[0].params().is_empty());
    assert_eq!(l_fx[0].declared_param_count(), None);

    // S100: [10,20] px on its own 100x100 size; End of Ramp default [50,100].
    let sp = project.effect_params(&small_fx[0]);
    assert_near(&point(sp[1].stored_value()), &[0.1, 0.2], 1e-12);
    assert_near(&sp[1].point_px(100.0, 100.0).unwrap(), &[10.0, 20.0], 1e-9);
    assert_eq!(sp[3].point_px(100.0, 100.0), Some([50.0, 100.0]));

    // L: the 368 defaults, not the values of the project copy (which is S100's instance).
    let lp = project.effect_params(&l_fx[0]);
    assert_eq!(lp.len(), 10);
    assert!(lp.iter().all(|p| p.borrowed_definition));
    assert_eq!(lp[1].name, "Start of Ramp");
    assert!(lp[1].stored().is_none());
    assert_eq!(lp[1].value(), Some(ParamValue::Point([0.5, 0.0])));
    assert_eq!(lp[3].value(), Some(ParamValue::Point([0.5, 1.0])));
    assert_eq!(lp[2].value(), Some(ParamValue::Argb([1.0, 0.0, 0.0, 0.0])));
    assert_eq!(lp[4].value(), Some(ParamValue::Argb([1.0, 1.0, 1.0, 1.0])));
    assert_eq!((number(lp[6].value()), number(lp[7].value())), (0.0, 0.0));
    // A borrowed popup has no reliable pard default (a selection writes into that slot).
    assert_eq!(lp[5].pard_default(), None);
    assert_eq!(lp[5].items.as_deref().map(<[String]>::len), Some(2));
    // L's own records: the input layer (its own id) and the button.
    assert_eq!(lp[0].stored_value(), Some(ParamValue::Layer(L)));
    assert!(lp[8].stored().is_some());

    // One definition for the type, although there are two instances.
    let defs = project.effect_definitions();
    assert_eq!(defs.iter().map(|(m, _)| m.as_str()).collect::<Vec<_>>(), ["ADBE Ramp"]);
}

#[test]
fn files_without_effects_have_no_definitions() {
    let bytes = fixture("000-base.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    assert!(project.effect_definitions().is_empty());
}
