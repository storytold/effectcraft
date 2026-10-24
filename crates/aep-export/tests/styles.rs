//! Layer styles: fixture -> import -> export -> import keeps every style value, the written
//! `ADBE Layer Styles` group is After Effects' own (951-979), and styles made in EffectCraft are
//! written with After Effects' defaults in mind.

mod common;

use common::{add_comp, add_layer, add_solid, export_all, key, with_comp, with_layer};
use effectcraft_aep::{PropNode, parse};
use effectcraft_aep_import::{ImportOptions, import};
use effectcraft_keyframe::Value;
use effectcraft_project::build::Ids;
use effectcraft_project::styles::{self as st, GROUP};
use effectcraft_project::{Expression, Layer, LayerSource, Project, PropGroup};
use effectcraft_time::FrameRate;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures").join(name)).expect("fixture")
}

fn import_bytes(bytes: &[u8]) -> (Project, Vec<String>) {
    let mut p = Project::default();
    let none = |_: &str| false;
    let opts = ImportOptions { allow_losses: true, ..ImportOptions::new(&none) };
    let r = import(&mut p, bytes, &opts).expect("import");
    let losses = r.report.losses.iter().flat_map(|l| l.details.clone()).collect();
    (p, losses)
}

fn layer<'p>(p: &'p Project, name: &str) -> &'p Layer {
    p.comps().flat_map(|(_, c)| c.layers.iter()).find(|l| l.name == name).expect("layer")
}

/// Every style parameter as (style, param, value, keys, expression).
fn values(g: &PropGroup) -> Vec<(String, String, Value, usize, Option<Expression>)> {
    let mut out = Vec::new();
    for s in g.groups() {
        for sub in std::iter::once(s).chain(s.groups()) {
            for n in &sub.children {
                if let Some(p) = n.as_prop() {
                    // A keyed property's static value means nothing (the importer takes the first key's).
                    let v = p.keys.first().map_or_else(|| p.value.clone(), |k| k.value.clone());
                    out.push((s.match_id.clone(), p.match_id.clone(), v, p.keys.len(), p.expr.clone()));
                }
            }
        }
    }
    out
}

/// The stored `ADBE Layer Styles` group of the first layer with a style applied.
fn stored_group(bytes: &[u8]) -> Vec<u8> {
    let f = parse(bytes).expect("parse");
    let project = effectcraft_aep::Project::new(&f).expect("project");
    for item in project.all_items() {
        let Some(c) = item.comp() else { continue };
        for l in c.layers() {
            if let Some(PropNode::Group(g)) = l.properties().and_then(|p| p.get("ADBE Layer Styles"))
                && g.flags() == Some(1)
            {
                let mut f2 = effectcraft_aep::AepFile { chunks: vec![g.chunk().clone().into_owned()], trailer: std::borrow::Cow::Owned(vec![]) };
                f2.trailer = std::borrow::Cow::Owned(vec![]);
                return effectcraft_aep::write(&f2).expect("write");
            }
        }
    }
    panic!("no layer with a style");
}

const CASES: &[&str] = &[
    "951-styles-all-nine-defaults-on-B.aep",
    "966-style-drop-shadow-opacity50-distance20.aep",
    "970-style-stroke-size10-color-green.aep",
    "974-style-gradient-overlay-angle45-scale150.aep",
    "975-style-fill-opacity0-with-drop-shadow.aep",
    "976-style-drop-shadow-opacity-keyed-100-20-linear.aep",
    "977-styles-all-params-changed.aep",
    "979-style-inner-glow-source-2.aep",
    "988-style-drop-shadow-switched-off.aep",
];

#[test]
fn fixtures_round_trip_without_losses() {
    for name in CASES {
        let bytes = fixture(name);
        let (p, losses) = import_bytes(&bytes);
        assert!(losses.is_empty(), "{name}: {losses:?}");
        let r = export_all(&p);
        assert!(r.report.losses.is_empty(), "{name}: {:?}", r.report.losses);
        assert!(r.report.exported.layer_styles > 0, "{name}");
        let (q, losses) = import_bytes(&r.bytes);
        assert!(losses.is_empty(), "{name} re-import: {losses:?}");
        let (a, b) = (layer(&p, "B"), layer(&q, "B"));
        assert_eq!(values(a.props.sub(GROUP).expect("styles")), values(b.props.sub(GROUP).expect("styles")), "{name}");
        // The written group is the one After Effects saved.
        assert_eq!(stored_group(&bytes), stored_group(&r.bytes), "{name}: stored group differs");
        let (ca, cb) = (p.comps().next().expect("comp").1, q.comps().next().expect("comp").1);
        assert_eq!(ca.global_light, cb.global_light, "{name}");
    }
}

#[test]
fn a_style_made_here_is_written_with_after_effects_defaults_in_mind() {
    let mut p = Project::default();
    let comp = add_comp(&mut p, "C", 400, 300, FrameRate::FPS_25, 4.0, None);
    let s = add_solid(&mut p, "S", 200, 100, [0.2, 0.4, 0.8], None);
    add_layer(&mut p, comp, "S", LayerSource::Solid { item: s }, (200, 100));
    let gl = p.comp(comp).expect("comp").global_light.clone();
    let mut next = p.next_id;
    with_layer(&mut p, comp, 0, |l| {
        let mut ids = Ids(&mut next);
        st::add_style(l, &mut ids, "dropShadow", &gl, true);
        st::add_style(l, &mut ids, "stroke", &gl, true);
        let g = l.props.sub_mut(GROUP).expect("styles");
        let ds = g.sub_mut("dropShadow").expect("ds");
        ds.get_mut("opacity").expect("opacity").keys = vec![key(0.0, Value::Scalar(100.0)), key(2.0, Value::Scalar(20.0))];
        ds.get_mut("color").expect("color").expr = Some(Expression { text: "[1, 0, 0, 1]".into(), enabled: true });
        let stroke = g.sub_mut("stroke").expect("stroke");
        stroke.get_mut("position").expect("position").value = Value::Enum(2);
        stroke.get_mut("blendMode").expect("mode").value = Value::Enum(23);
    });
    p.next_id = next;
    with_comp(&mut p, comp, |c| st::set_global_light(c, Some(60.0), None, effectcraft_time::Tick::ZERO));
    let r = export_all(&p);
    assert!(r.report.losses.is_empty(), "{:?}", r.report.losses);
    let (q, losses) = import_bytes(&r.bytes);
    assert!(losses.is_empty(), "{losses:?}");
    let (a, b) = (layer(&p, "S"), layer(&q, "S"));
    assert_eq!(values(a.props.sub(GROUP).expect("styles")), values(b.props.sub(GROUP).expect("styles")));
    // EffectCraft's Use Global Light default (on) differs from After Effects' (off): written.
    assert_eq!(b.props.sub(GROUP).and_then(|g| g.sub("dropShadow")).and_then(|g| g.get("useGlobalLight")).map(|p| p.value.clone()), Some(Value::Bool(true)));
    assert_eq!(q.comps().next().expect("comp").1.global_light.angle.value, Value::Scalar(60.0));
}

#[test]
fn what_is_not_written_is_reported() {
    let mut p = Project::default();
    let comp = add_comp(&mut p, "C", 400, 300, FrameRate::FPS_25, 4.0, None);
    let s = add_solid(&mut p, "S", 200, 100, [0.2, 0.4, 0.8], None);
    add_layer(&mut p, comp, "S", LayerSource::Solid { item: s }, (200, 100));
    let gl = p.comp(comp).expect("comp").global_light.clone();
    let mut next = p.next_id;
    with_layer(&mut p, comp, 0, |l| {
        let mut ids = Ids(&mut next);
        st::add_style(l, &mut ids, "satin", &gl, false);
        st::add_style(l, &mut ids, "gradientOverlay", &gl, true);
        let g = l.props.sub_mut(GROUP).expect("styles");
        let go = g.sub_mut("gradientOverlay").expect("go");
        if let Some(Value::Gradient(gr)) = go.get_mut("colors").map(|p| &mut p.value) {
            gr.colors[0].1 = [1.0, 0.0, 0.0, 1.0];
        }
        go.get_mut("style").expect("style").keys = vec![key(0.0, Value::Enum(1)), key(1.0, Value::Enum(2))];
        let adv = g.sub_mut(st::BLENDING).and_then(|b| b.sub_mut("advancedBlending")).expect("adv");
        adv.get_mut("knockout").expect("knockout").value = Value::Enum(1);
    });
    p.next_id = next;
    let r = export_all(&p);
    let lines: Vec<String> = r.report.losses.iter().flat_map(|l| l.details.clone()).collect();
    let has = |s: &str| lines.iter().any(|l| l.contains(s));
    assert!(has("gradientFill/gradient"), "{lines:?}");
    assert!(has("gradientFill/type: keys"), "{lines:?}");
    assert!(has("Knockout"), "{lines:?}");
    let (q, _) = import_bytes(&r.bytes);
    let g = layer(&q, "S").props.sub(GROUP).expect("styles");
    // A switched-off style is written switched off (988).
    assert!(g.sub("satin").is_some_and(|s| !s.enabled) && g.sub("gradientOverlay").is_some_and(|s| s.enabled));
}

#[test]
fn styles_on_every_layer_kind_that_takes_them() {
    let mut p = Project::default();
    let comp = add_comp(&mut p, "C", 400, 300, FrameRate::FPS_25, 4.0, None);
    let s = add_solid(&mut p, "S", 200, 100, [0.2, 0.4, 0.8], None);
    add_layer(&mut p, comp, "S", LayerSource::Solid { item: s }, (200, 100));
    add_layer(&mut p, comp, "Shape", LayerSource::Shape, (400, 300));
    add_layer(&mut p, comp, "Text", LayerSource::Text, (400, 300));
    add_layer(&mut p, comp, "Null", LayerSource::Null, (100, 100));
    let gl = p.comp(comp).expect("comp").global_light.clone();
    let mut next = p.next_id;
    for i in 0..4 {
        with_layer(&mut p, comp, i, |l| {
            st::add_style(l, &mut Ids(&mut next), "stroke", &gl, true);
        });
    }
    p.next_id = next;
    let r = export_all(&p);
    let (q, _) = import_bytes(&r.bytes);
    for name in ["S", "Shape", "Text"] {
        assert!(layer(&q, name).props.sub(GROUP).and_then(|g| g.sub("stroke")).is_some(), "{name}");
    }
}
