//! Effect instances: match names, parameter lists from `parT`/`pard` and stored values, checked
//! against the values the corpus was made with (its MANIFEST).

mod common;

use common::{first_comp, fixture, layer_with_id, open};
use effectcraft_aep::{Effect, ParamKind, ParamValue, Project};

const L: u32 = 0x1b;

/// Runs `f` on the single effect of the solid `L` in a fixture.
fn with_effect<R>(name: &str, f: impl FnOnce(&Effect<'_, '_>) -> R) -> R {
    let bytes = fixture(name);
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let layer = layer_with_id(&first_comp(&project), L);
    let effects = layer.effects();
    assert_eq!(effects.len(), 1, "{name}");
    f(&effects[0])
}

fn kinds(e: &Effect<'_, '_>) -> Vec<(ParamKind, String)> {
    e.params().into_iter().map(|p| (p.kind, p.name)).collect()
}

fn approx(v: Option<ParamValue>, expected: f64) {
    match v {
        Some(ParamValue::Number(n)) => assert!((n - expected).abs() < 1e-9, "{n} != {expected}"),
        other => panic!("{other:?} is not the number {expected}"),
    }
}

#[test]
fn no_effects_without_a_parade_entry() {
    let bytes = fixture("000-base.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    assert!(layer_with_id(&first_comp(&project), L).effects().is_empty());
}

#[test]
fn slider_control_with_a_stored_value() {
    // 071: Slider Control, Slider = 77.
    with_effect("071-fx-slider-value77.aep", |e| {
        assert_eq!((e.match_name(), e.name().as_deref()), ("ADBE Slider Control", Some("Slider Control")));
        assert_eq!(e.declared_param_count(), Some(3));
        assert_eq!(kinds(e), [(ParamKind::Layer, String::new()), (ParamKind::FloatSlider, "Slider".to_string()), (ParamKind::NoData, String::new())]);
        let params = e.params();
        assert_eq!(params[1].match_name, "ADBE Slider Control-0001");
        assert_eq!(params[2].match_name, "ADBE Effect Built In Params");
        assert_eq!(params[1].stored_value(), Some(ParamValue::Number(77.0)));
        assert_eq!(params[1].value(), Some(ParamValue::Number(77.0)));
        // The pard default of the slider is 0 (070 has the untouched control).
        assert_eq!(params[1].pard_default(), Some(ParamValue::Number(0.0)));
        // The input layer parameter has a value record of its own: its `tdpi` holds the id of the layer that owns the effect.
        assert_eq!(params[0].stored_value(), Some(ParamValue::Layer(0x1b)));
        assert_eq!(params[0].pard().len(), 148);
    });
}

#[test]
fn color_control_stores_argb_doubles() {
    // 073: Color -> [0.2, 0.6, 1], stored as [255, 51, 153, 255] (alpha first).
    with_effect("073-fx-color-0.2-0.6-1.aep", |e| {
        assert_eq!((e.match_name(), e.name().as_deref()), ("ADBE Color Control", Some("Color Control")));
        assert_eq!(kinds(e), [(ParamKind::Layer, String::new()), (ParamKind::Color, "Color".to_string()), (ParamKind::NoData, String::new())]);
        let color = e.params()[1].value();
        let Some(ParamValue::Argb([a, r, g, b])) = color else { panic!("{color:?}") };
        for (got, want) in [(a, 1.0), (r, 0.2), (g, 0.6), (b, 1.0)] {
            assert!((got - want).abs() < 1e-6, "{color:?}");
        }
    });
}

#[test]
fn dropdown_menu_items_and_default() {
    // 077: Dropdown Menu Control with the default items.
    with_effect("077-fx-dropdown-default.aep", |e| {
        assert_eq!((e.match_name(), e.name().as_deref()), ("Pseudo/@@S1iPXtJFTHK4QaDbgJvLhw", Some("Dropdown Menu Control")));
        assert_eq!(kinds(e), [(ParamKind::Layer, String::new()), (ParamKind::Popup, "Menu".to_string()), (ParamKind::NoData, String::new())]);
        let menu = &e.params()[1];
        assert_eq!(menu.items.as_deref(), Some(&["Item 1".to_string(), "Item 2".to_string(), "Item 3".to_string()][..]));
        assert_eq!(menu.pard_default(), Some(ParamValue::Index(1)));
        // The choice count is the u16 at byte 60 of pard.
        assert_eq!(u16::from_be_bytes([menu.pard()[60], menu.pard()[61]]), 3);
        // No other parameter has items.
        assert!(e.params().iter().filter(|p| p.index != 1).all(|p| p.items.is_none()));
    });
}

#[test]
fn drop_shadow_defaults_come_from_pard() {
    // 080: Drop Shadow at its defaults (opacity 50 % = 127.5 of 255, direction 135, distance 5, softness 0).
    with_effect("080-fx-dropshadow-default.aep", |e| {
        assert_eq!((e.match_name(), e.name().as_deref()), ("ADBE Drop Shadow", Some("Drop Shadow")));
        let k: Vec<_> = e.params().into_iter().map(|p| (p.kind, p.name)).collect();
        let expected = [
            (ParamKind::Layer, ""),
            (ParamKind::Color, "Shadow Color"),
            (ParamKind::FixedSlider, "Opacity"),
            (ParamKind::Angle, "Direction"),
            (ParamKind::FixedSlider, "Distance"),
            (ParamKind::FixedSlider, "Softness"),
            (ParamKind::Checkbox, "Shadow Only"),
            (ParamKind::NoData, ""),
        ];
        assert_eq!(k, expected.map(|(kind, n)| (kind, n.to_string())));
        let p = e.params();
        approx(p[2].value(), 127.5);
        approx(p[3].value(), 135.0);
        approx(p[4].value(), 5.0);
        approx(p[5].value(), 0.0);
        // A checkbox without a record is off; the shadow colour default (ARGB bytes in pard) is black.
        assert_eq!(p[6].value(), Some(ParamValue::Bool(false)));
        assert_eq!(p[1].value(), Some(ParamValue::Argb([1.0, 0.0, 0.0, 0.0])));
    });
}

// ---------------------------------------------------------------------------- batch 2 (defaults and values)

fn param<'f, 'a>(e: &Effect<'f, 'a>, name: &str) -> effectcraft_aep::EffectParam<'f, 'a> {
    e.params().into_iter().find(|p| p.name == name).unwrap_or_else(|| panic!("no parameter {name}"))
}

#[test]
fn gaussian_blur_default_is_the_pard_default_25() {
    // 214: Gaussian Blur, all parameters default. The file has no value record and a pard default of 25
    // (the manifest says 0; the files only ever contain 25, see docs/aep-format.md).
    with_effect("214-fx-gaussianblur-default.aep", |e| {
        assert_eq!(e.match_name(), "ADBE Gaussian Blur 2");
        let blur = param(e, "Blurriness");
        assert_eq!(blur.kind, ParamKind::FloatSlider);
        assert!(blur.stored().is_none());
        assert_eq!(blur.stored_value(), None);
        assert_eq!(blur.value(), Some(ParamValue::Number(25.0)));
        assert_eq!(f64::from_be_bytes(blur.pard()[56..64].try_into().unwrap()), 25.0);
        assert_eq!(f32::from_be_bytes(blur.pard()[120..124].try_into().unwrap()), 25.0);
        // Dimensions: Horizontal and Vertical (the first item of the popup); Repeat Edge Pixels: on
        // (its pard default is 1; After Effects reads a fresh Gaussian Blur back with Repeat Edge
        // Pixels 1, 417; the corpus manifest's "off" for 214 was wrong).
        let dims = param(e, "Blur Dimensions");
        assert_eq!(dims.kind, ParamKind::Popup);
        assert_eq!(dims.value(), Some(ParamValue::Index(1)));
        assert_eq!(dims.items.as_ref().and_then(|i| i.first()).map(String::as_str), Some("Horizontal and Vertical"));
        // The last parameter is the Repeat Edge Pixels checkbox (its label is not in pard).
        let edge = &e.params()[3];
        assert_eq!(edge.kind, ParamKind::Checkbox);
        assert!(edge.stored().is_none());
        assert_eq!(edge.value(), Some(ParamValue::Bool(true)));
    });
}

#[test]
fn gaussian_blur_40_adds_a_value_record_and_leaves_pard_at_the_default() {
    // 216: Blurriness 40.
    with_effect("216-fx-gaussianblur-40.aep", |e| {
        let blur = param(e, "Blurriness");
        assert_eq!(blur.stored().unwrap().flags(), Some(1));
        assert_eq!(blur.stored_value(), Some(ParamValue::Number(40.0)));
        assert_eq!(blur.value(), Some(ParamValue::Number(40.0)));
        // pard holds the default, not the current value.
        assert_eq!(blur.pard_default(), Some(ParamValue::Number(25.0)));
    });
}

#[test]
fn fill_defaults_come_from_pard() {
    // 215: Fill with the default colour (red) and opacity (100 %); the file has no value records for them.
    with_effect("215-fx-fill-default.aep", |e| {
        assert_eq!(e.match_name(), "ADBE Fill");
        let color = param(e, "Color");
        assert_eq!(color.kind, ParamKind::Color);
        assert!(color.stored().is_none());
        assert_eq!(&color.pard()[56..60], [0xff, 0xff, 0x00, 0x00]);
        assert_eq!(color.value(), Some(ParamValue::Argb([1.0, 1.0, 0.0, 0.0])));
        let opacity = param(e, "Opacity");
        assert_eq!(opacity.kind, ParamKind::FixedSlider);
        assert!(opacity.stored().is_none());
        assert_eq!(opacity.value(), Some(ParamValue::Number(1.0)));
    });
}

#[test]
fn checkbox_point_and_angle_values() {
    // 217: Checkbox 0 -> 1 (stored 1.0).
    with_effect("217-fx-checkbox-on.aep", |e| {
        let p = param(e, "Checkbox");
        assert_eq!(p.kind, ParamKind::Checkbox);
        assert_eq!(p.stored_value(), Some(ParamValue::Bool(true)));
        assert_eq!(p.value(), Some(ParamValue::Bool(true)));
        assert_eq!(p.stored().unwrap().static_value(), Some(vec![1.0]));
    });
    // 218: Point -> [111, 222], stored as a fraction of the layer size (the layer is 1920x1080).
    with_effect("218-fx-point-111-222.aep", |e| {
        let p = param(e, "Point");
        assert_eq!(p.kind, ParamKind::Point);
        let Some(ParamValue::Point([x, y])) = p.value() else { panic!("{:?}", p.value()) };
        assert!((x - 111.0 / 1920.0).abs() < 1e-12 && (y - 222.0 / 1080.0).abs() < 1e-12, "{x} {y}");
        assert!((x * 1920.0 - 111.0).abs() < 1e-9 && (y * 1080.0 - 222.0).abs() < 1e-9);
    });
    // 219: Angle -> 90, stored as plain degrees.
    with_effect("219-fx-angle-90.aep", |e| {
        let p = param(e, "Angle");
        assert_eq!(p.kind, ParamKind::Angle);
        assert_eq!(p.stored_value(), Some(ParamValue::Number(90.0)));
        assert_eq!(p.value(), Some(ParamValue::Number(90.0)));
    });
}

#[test]
fn layer_control_refers_to_a_layer_by_id() {
    // 220: a new Null on top (layer index 1); L (now index 2) gets a Layer Control with Layer = 1: the tdpi
    // next to the value record holds the id of the Null.
    let bytes = fixture("220-fx-layercontrol-to-layer1.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let comp = first_comp(&project);
    let null = comp.layers().next().unwrap();
    let null_id = null.id().unwrap();
    assert_eq!(null_id, 0x22);
    assert!(null.header().unwrap().is_null_object());
    let layer = layer_with_id(&comp, L);
    let e = &layer.effects()[0];
    assert_eq!(e.match_name(), "ADBE Layer Control");
    let p = param(e, "Layer");
    assert_eq!(p.kind, ParamKind::Layer);
    assert_eq!(p.stored_value(), Some(ParamValue::Layer(null_id)));
    // The implicit input layer parameter refers to the layer that owns the effect.
    assert_eq!(e.params()[0].stored_value(), Some(ParamValue::Layer(L)));
}

#[test]
fn curves_and_mesh_warp_data() {
    // 963: Red (0,0) (128,44) (255,255); 964: RGB (0,0) (62,24) (194,230) (255,255). Every stored
    // table is the natural spline through its points.
    for (name, curve, points) in [
        ("963-curves-red-mid-down-on-S.aep", 1usize, vec![(0u8, 0u8), (128, 44), (255, 255)]),
        ("964-curves-rgb-s-curve-on-S.aep", 0, vec![(0, 0), (62, 24), (194, 230), (255, 255)]),
    ] {
        let bytes = fixture(name);
        let file = open(&bytes);
        let project = effectcraft_aep::Project::new(&file).expect("project");
        let comp = first_comp(&project);
        let fx = comp.layers().flat_map(|l| l.effects()).find(|e| e.match_name() == "ADBE CurvesCustom").expect("curves");
        let data = fx.arbitrary_data("ADBE CurvesCustom-0001").expect("data");
        let d = effectcraft_aep::CurvesData::parse(data).expect("parse");
        assert_eq!(d.curves[curve], points, "{name}");
        assert!(d.curves.iter().enumerate().all(|(i, c)| i == curve || c.as_slice() == effectcraft_aep::CURVE_IDENTITY), "{name}");
        // Rebuilt from the points, the data is the stored data byte for byte.
        assert_eq!(d.to_bytes().as_deref(), Some(data), "{name}");
    }
    // 985: a 3 x 3 mesh (4 x 4 vertices), vertex (2, 2) moved by (+132, -2) px on 1920 x 1080 with
    // its handles; the others at their default.
    let bytes = fixture("985-meshwarp-3x3-vertex-moved-on-S.aep");
    let file = open(&bytes);
    let project = effectcraft_aep::Project::new(&file).expect("project");
    let comp = first_comp(&project);
    let fx = comp.layers().flat_map(|l| l.effects()).find(|e| e.match_name() == "ADBE MESH WARP").expect("mesh");
    let data = fx.arbitrary_data("ADBE MESH WARP-0004").expect("data");
    let m = effectcraft_aep::MeshData::parse(data).expect("parse");
    assert_eq!((m.columns, m.rows, m.vertices.len()), (3, 3, 16));
    for (k, v) in m.vertices.iter().enumerate() {
        let (i, j) = (k as u32 % 4, k as u32 / 4);
        let d = effectcraft_aep::MeshData::default_vertex(3, 3, i, j);
        let (dx, dy) = if (i, j) == (2, 2) { (132.0 / 1920.0, -2.0 / 1080.0) } else { (0.0, 0.0) };
        for (a, b) in v.iter().zip(&d) {
            let moved = if *b == [0.0, 0.0] { [0.0, 0.0] } else { [b[0] + dx, b[1] + dy] };
            assert!((a[0] - moved[0]).abs() < 2e-6 && (a[1] - moved[1]).abs() < 2e-6, "vertex {i},{j}: {a:?} vs {moved:?}");
        }
    }
    assert_eq!(m.to_bytes().as_deref(), Some(data));
}
