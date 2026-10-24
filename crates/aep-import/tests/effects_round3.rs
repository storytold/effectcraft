//! Effects of session I3 (905: the 24 effects of 904, one per layer of one composition) import
//! with the values they were made with, After Effects' defaults where nothing was set, and the
//! layer parameters naming the layer they were set to. Layer Control: 220, 320, 321.

mod common;

use common::{comp, load};
use effectcraft_keyframe::Value;
use effectcraft_project::{Comp, PropGroup};

const F905: &str = "905-fx-24-effects-set-one-comp.aep";

fn fx_of(c: &Comp, layer: &str) -> PropGroup {
    let l = c.layers.iter().find(|l| l.name == layer).unwrap_or_else(|| panic!("no {layer}"));
    let fx = l.props.sub("effects").expect("effects");
    let g: Vec<&PropGroup> = fx.children.iter().filter_map(|n| n.as_group()).collect();
    assert_eq!(g.len(), 1, "{layer}");
    g[0].clone()
}

fn v(g: &PropGroup, id: &str) -> Value {
    g.prop(id).unwrap_or_else(|| panic!("no {id} in {}", g.match_id)).value.clone()
}

fn num(g: &PropGroup, id: &str) -> f64 {
    match v(g, id) {
        Value::Scalar(x) => x,
        other => panic!("{id}: {other:?}"),
    }
}

fn near(g: &PropGroup, id: &str, want: f64) {
    let got = num(g, id);
    assert!((got - want).abs() < 1e-4, "{} {id}: {got} != {want}", g.match_id);
}

fn pt(g: &PropGroup, id: &str, want: [f64; 2]) {
    let Value::Vec2(a) = v(g, id) else { panic!("{id}") };
    assert!((a[0] - want[0]).abs() < 1e-3 && (a[1] - want[1]).abs() < 1e-3, "{} {id}: {a:?} != {want:?}", g.match_id);
}

fn col(g: &PropGroup, id: &str, want: [f64; 3]) {
    let Value::Color(a) = v(g, id) else { panic!("{id}") };
    assert!(a.iter().zip(want).all(|(x, y)| (x - y).abs() < 1e-3) && a[3] == 1.0, "{} {id}: {a:?} != {want:?}", g.match_id);
}

#[test]
fn the_24_effects_import_with_their_values() {
    let (p, r) = load(F905);
    // The one loss: CC Bend It's default Render Prestart "Bend" has no EffectCraft item.
    assert_eq!(r.report.total_losses(), 1, "{:?}", r.report.losses);
    assert!(format!("{:?}", r.report.losses).contains("'Bend' has no counterpart"));
    let (_, c) = comp(&p, &r, "C");
    let e = |n: &str| fx_of(c, n);

    let g = e("E01");
    assert_eq!(g.match_id, "ec.color.brightnesscontrast");
    near(&g, "brightness", 20.0);
    near(&g, "contrast", 30.0);
    assert_eq!(v(&g, "useLegacy"), Value::Bool(false));

    let g = e("E02");
    near(&g, "amount", 80.0);
    near(&g, "size", 150.0);
    near(&g, "complexity", 2.0);
    near(&g, "evolution", 45.0);
    near(&g, "evolutionOptions/randomSeed", 3.0);
    // After Effects' defaults: Turbulent, Pin All.
    assert_eq!((v(&g, "displacement"), v(&g, "pinning")), (Value::Enum(0), Value::Enum(1)));

    let g = e("E03");
    near(&g, "contrast", 150.0);
    near(&g, "brightness", -10.0);
    near(&g, "complexity", 4.0);
    near(&g, "evolution", 90.0);
    near(&g, "evolutionOptions/seed", 5.0);
    near(&g, "opacity", 80.0);
    // Defaults: Basic, Soft Linear, Allow HDR Results, Normal, Sub Influence 70.
    assert_eq!(
        (v(&g, "fractalType"), v(&g, "noiseType"), v(&g, "overflow"), v(&g, "blendingMode")),
        (Value::Enum(0), Value::Enum(2), Value::Enum(3), Value::Enum(1))
    );
    near(&g, "subSettings/subInfluence", 70.0);

    // Beam: Length / Time / Softness are fractions in the file, percentages here; After Effects'
    // defaults where nothing was set (3D Perspective on, Composite On Original off).
    let g = e("E04");
    pt(&g, "startPoint", [200.0, 300.0]);
    pt(&g, "endPoint", [1700.0, 800.0]);
    near(&g, "length", 60.0);
    near(&g, "time", 40.0);
    near(&g, "startThickness", 20.0);
    near(&g, "endThickness", 40.0);
    near(&g, "softness", 30.0);
    col(&g, "insideColor", [1.0, 1.0, 0.0]);
    col(&g, "outsideColor", [0.0, 0.0, 1.0]);
    assert_eq!((v(&g, "perspective3d"), v(&g, "compositeOnOriginal")), (Value::Bool(true), Value::Bool(false)));

    let g = e("E05");
    pt(&g, "center", [700.0, 400.0]);
    near(&g, "amount", 70.0);
    assert_eq!(v(&g, "zoom"), Value::Enum(1));

    near(&e("E06"), "amount", 30.0);

    let g = e("E07");
    for (id, want) in [
        ("rgb/rgbInBlack", 0.1),
        ("rgb/rgbInWhite", 0.9),
        ("rgb/rgbGamma", 1.5),
        ("rgb/rgbOutBlack", 0.05),
        ("rgb/rgbOutWhite", 0.95),
        ("red/redGamma", 0.8),
        ("green/greenInWhite", 0.8),
        ("blue/blueOutBlack", 0.2),
        ("alpha/alphaOutWhite", 1.0),
    ] {
        near(&g, id, want);
    }
    // After Effects' Off for 32 bpc Color.
    assert_eq!(v(&g, "clipToOutputBlack"), Value::Enum(2));

    let g = e("E08");
    pt(&g, "ul", [100.0, 50.0]);
    pt(&g, "lr", [1850.0, 1050.0]);

    let g = e("E09");
    pt(&g, "anchor", [100.0, 100.0]);
    near(&g, "width", 100.0);
    near(&g, "height", 80.0);
    near(&g, "border", 4.0);
    col(&g, "color", [0.0, 1.0, 0.0]);
    near(&g, "opacity", 70.0);
    // After Effects' Size From default is Corner Point.
    assert_eq!(v(&g, "sizeFrom"), Value::Enum(0));

    near(&e("E10"), "amount", 50.0);

    let g = e("E11");
    col(&g, "keyColor", [0.0, 0.0, 1.0]);
    near(&g, "tolerance", 20.0);
    near(&g, "softness", 15.0);

    let g = e("E12");
    near(&g, "completion", 40.0);
    near(&g, "startAngle", 30.0);
    pt(&g, "center", [800.0, 500.0]);
    assert_eq!(v(&g, "wipe"), Value::Enum(1));
    near(&g, "feather", 20.0);

    col(&e("E13"), "backgroundColor", [1.0, 1.0, 1.0]);

    let g = e("E14");
    near(&g, "bend", 30.0);
    pt(&g, "start", [960.0, 200.0]);

    let g = e("E15");
    pt(&g, "topRight", [1700.0, 50.0]);
    pt(&g, "bottomRight", [1800.0, 950.0]);
    near(&g, "perspective", 100.0);

    let g = e("E16");
    pt(&g, "positionsColors/point2", [1600.0, 250.0]);
    col(&g, "positionsColors/color1", [1.0, 0.0, 0.0]);
    col(&g, "positionsColors/color4", [1.0, 1.0, 0.0]);
    near(&g, "blend", 50.0);
    near(&g, "opacity", 80.0);

    let g = e("E17");
    near(&g, "hueTransform", 90.0);
    col(&g, "colorToChange", [0.0, 0.0, 1.0]);
    near(&g, "matchingTolerance", 30.0);
    // After Effects' Match Colors default is Using RGB (EffectCraft's own: Using Hue).
    assert_eq!(v(&g, "matchColors"), Value::Enum(0));

    let g = e("E18");
    near(&g, "blackPoint", 30.0);
    near(&g, "whitePoint", 220.0);
    near(&g, "blackSoftness", 10.0);

    let g = e("E19");
    near(&g, "hue", 20.0);
    near(&g, "lightness", 10.0);
    near(&g, "saturation", 30.0);

    near(&e("E20"), "amount", 20.0);

    let g = e("E21");
    pt(&g, "center", [800.0, 500.0]);
    near(&g, "direction", 20.0);
    near(&g, "width", 80.0);
    near(&g, "sweepIntensity", 60.0);
    near(&g, "edgeIntensity", 100.0);
    near(&g, "edgeThickness", 4.0);
    col(&g, "lightColor", [1.0, 0.5, 0.0]);
    // After Effects' Shape default is Sharp.
    assert_eq!(v(&g, "shape"), Value::Enum(2));
}

/// The layer parameters of 905 name `R` (below them), with its source only (After Effects hands
/// the named layer's source to the effect, 904 E24).
#[test]
fn layer_parameters_name_the_layer_they_were_set_to() {
    let (p, r) = load(F905);
    let (_, c) = comp(&p, &r, "C");
    let rid = c.layers.iter().find(|l| l.name == "R").unwrap().id.0;
    for (layer, id) in [("E22", "blurLayer"), ("E23", "displacementMapLayer"), ("E24", "takeMatteFromLayer")] {
        let g = fx_of(c, layer);
        assert_eq!(v(&g, id), Value::Layer(Some(rid)), "{layer}");
        assert_eq!(v(&g, &format!("{id}Source")), Value::Enum(0), "{layer}");
    }
    let g = fx_of(c, "E24");
    // Luminance, by name: "Luminance" here, after "Red Channel" ... "Alpha Channel" -> "Red" ...
    assert_eq!(v(&g, "takeMatteFrom"), Value::Enum(4));
    near(&fx_of(c, "E22"), "maximumBlur", 10.0);
    near(&fx_of(c, "E23"), "maxHorizontal", 20.0);
    near(&fx_of(c, "E23"), "maxVertical", 10.0);
}

/// Layer Control: another layer (220: the Null above), the owning layer (320, also the default),
/// None (321).
#[test]
fn layer_control_names_another_layer_itself_or_none() {
    let (p, r) = load("220-fx-layercontrol-to-layer1.aep");
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    let (_, c) = comp(&p, &r, "C");
    let null = c.layers.iter().find(|l| l.name != "L").unwrap().id.0;
    assert_eq!(v(&fx_of(c, "L"), "layer"), Value::Layer(Some(null)));

    let (p, r) = load("320-fx-layercontrol-self-explicit.aep");
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    let (_, c) = comp(&p, &r, "C");
    let own = c.layers.iter().find(|l| l.name == "L").unwrap().id.0;
    assert_eq!(v(&fx_of(c, "L"), "layer"), Value::Layer(Some(own)));

    let (p, r) = load("321-fx-layercontrol-none.aep");
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    let (_, c) = comp(&p, &r, "C");
    assert_eq!(v(&fx_of(c, "L"), "layer"), Value::Layer(None));
}

/// Hostile layer ids in valid structure: a layer that does not exist is reported and left unset,
/// the owning layer's own id is "this layer", 0 is the explicit None (`NO_LAYER`); nothing panics.
#[test]
fn hostile_layer_parameter_ids_are_reported_or_neutral() {
    use effectcraft_aep_import::{ImportOptions, LossCategory, import};
    let bytes = common::fixture_bytes(F905);
    // The value records (the layer's and the project list's copy): the match name followed by its
    // `LIST tdbs`, not the definition in `parT`.
    let records: Vec<usize> = (0..bytes.len().saturating_sub(64))
        .filter(|&i| bytes[i..].starts_with(b"ADBE Set Matte3-0001\0") && bytes[i..i + 64].windows(4).any(|w| w == b"tdbs"))
        .map(|at| at + bytes[at..].windows(4).position(|w| w == b"tdpi").expect("tdpi") + 8)
        .collect();
    assert!(!records.is_empty());
    let none = |_: &str| false;
    let opts = ImportOptions { allow_losses: true, ..ImportOptions::new(&none) };
    let none_value = Value::Layer(Some(effectcraft_project::NO_LAYER));
    for (id, expected) in [(u32::MAX, Value::Layer(None)), (0, none_value), (12345, Value::Layer(None))] {
        let mut b = bytes.clone();
        for &tdpi in &records {
            b[tdpi..tdpi + 4].copy_from_slice(&id.to_be_bytes());
        }
        let mut p = effectcraft_project::Project::default();
        let r = import(&mut p, &b, &opts).expect("imports");
        let (_, c) = comp(&p, &r, "C");
        let g = fx_of(c, "E24");
        assert_eq!(v(&g, "takeMatteFromLayer"), expected, "{id}");
        let named_missing = id != 0;
        assert_eq!(r.report.count(LossCategory::Effects), 1 + usize::from(named_missing), "{id}: {:?}", r.report.losses);
    }
}

/// Older files store Glow's A & B Midpoint maximum as 0.98999 (16.16 64 880) where After Effects
/// 2026 writes 0.990005 (64 881): the same scale, imported.
#[test]
fn a_valid_maximum_off_by_one_16_16_step_is_the_same_scale() {
    use effectcraft_aep_import::{ImportOptions, LossCategory, import};
    let mut b = common::fixture_bytes("438-fx-glow-midpoint-0.25.aep");
    let mut n = 0;
    let mut i = 0;
    while let Some(off) = b[i..].windows(14).position(|w| w == b"A & B Midpoint") {
        let at = i + off;
        // In `pard` the name is at byte 16 and the valid maximum at 128.
        let max = at - 16 + 128;
        if b[max..max + 4] == 64_881u32.to_be_bytes() {
            b[max..max + 4].copy_from_slice(&64_880u32.to_be_bytes());
            n += 1;
        }
        i = at + 14;
    }
    assert!(n >= 1);
    let none = |_: &str| false;
    let mut p = effectcraft_project::Project::default();
    let r = import(&mut p, &b, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).expect("imports");
    assert_eq!(r.report.count(LossCategory::Effects), 0, "{:?}", r.report.losses);
}

#[test]
fn curves_and_mesh_warp_import() {
    use effectcraft_aep_import::{ImportOptions, import};
    let none = |_: &str| false;
    let opts = ImportOptions { allow_losses: false, ..ImportOptions::new(&none) };
    let fx = |name: &str| -> effectcraft_project::PropGroup {
        let mut p = effectcraft_project::Project::default();
        let r = import(&mut p, &common::fixture_bytes(name), &opts).unwrap_or_else(|e| panic!("{name}: {e}"));
        let (_, c) = comp(&p, &r, "C");
        c.layers.iter().filter_map(|l| l.props.sub("effects")).flat_map(|e| e.children.iter()).find_map(|n| n.as_group().cloned()).expect("effect")
    };
    let g = fx("963-curves-red-mid-down-on-S.aep");
    assert_eq!(v(&g, "red"), Value::Str("0,0 0.501961,0.172549 1,1".into()));
    assert_eq!(v(&g, "rgb"), Value::Str("0,0 1,1".into()));
    let g = fx("964-curves-rgb-s-curve-on-S.aep");
    assert_eq!(v(&g, "rgb"), Value::Str("0,0 0.243137,0.094118 0.760784,0.901961 1,1".into()));
    let g = fx("985-meshwarp-3x3-vertex-moved-on-S.aep");
    assert_eq!((v(&g, "rows"), v(&g, "columns"), v(&g, "quality")), (Value::Scalar(3.0), Value::Scalar(3.0), Value::Scalar(8.0)));
    let Value::Str(mesh) = v(&g, "mesh") else { panic!("mesh") };
    let offs: Vec<&str> = mesh.split(' ').collect();
    assert_eq!(offs.len(), 16);
    for (k, o) in offs.iter().enumerate() {
        let (x, y) = o.split_once(',').expect("pair");
        let (x, y): (f64, f64) = (x.parse().expect("x"), y.parse().expect("y"));
        let want = if k == 10 { (132.0, -2.0) } else { (0.0, 0.0) };
        assert!((x - want.0).abs() < 0.01 && (y - want.1).abs() < 0.01, "{k}: {o}");
    }
}
