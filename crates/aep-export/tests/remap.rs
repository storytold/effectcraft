//! Time Remapping: the fixtures with an enabled remap (319 on a precomposition layer, 354 on an
//! image-sequence layer) go through import, export and parse, and the remap record equals the one
//! After Effects wrote; remaps built in code (hold, reverse, bezier, an expression, no keys) are
//! written and imported back; what cannot be remapped is reported; hostile remaps never panic.

mod common;

use common::*;
use effectcraft_aep::{Body, Chunk, parse};
use effectcraft_aep_export::LossCategory;
use effectcraft_aep_import::{ImportOptions, import};
use effectcraft_keyframe::{Ease, Interp, Keyframe, Value};
use effectcraft_project::{Expression, ItemKind, LayerSource, Project, Property};
use effectcraft_time::FrameRate;

fn fixture(name: &str) -> Vec<u8> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures");
    std::fs::read(dir.join(name)).expect(name)
}

fn reimport(bytes: &[u8]) -> Project {
    let mut p = Project::default();
    let none = |_: &str| false;
    let opts = ImportOptions { allow_losses: true, ..ImportOptions::new(&none) };
    import(&mut p, bytes, &opts).expect("import");
    p
}

/// The enabled (`tdsb` 1) `ADBE Time Remapping` records of a file.
fn enabled_remaps(bytes: &[u8]) -> Vec<Chunk<'static>> {
    fn walk(cs: &[Chunk<'_>], out: &mut Vec<Chunk<'static>>) {
        for (i, c) in cs.iter().enumerate() {
            if c.is(b"tdmn")
                && matches!(&c.body, Body::Data(d) if d.starts_with(b"ADBE Time Remapping\0"))
                && let Some(rec) = cs.get(i + 1)
                && rec.children().iter().any(|k| k.is(b"tdsb") && matches!(&k.body, Body::Data(d) if d.as_ref() == [0, 0, 0, 1]))
            {
                out.push(rec.clone().into_owned());
            }
            walk(c.children(), out);
        }
    }
    let f = parse(bytes).expect("parse");
    let mut out = Vec::new();
    walk(&f.chunks, &mut out);
    out
}

fn remap_of(p: &Project) -> Vec<Property> {
    let mut v = Vec::new();
    for item in p.items.values() {
        let ItemKind::Comp(c) = &item.kind else { continue };
        for l in &c.layers {
            if let Some(r) = l.props.prop("timeRemap") {
                v.push(Property { uid: 0, ..r.clone() });
            }
        }
    }
    v
}

#[test]
fn the_remap_fixtures_are_written_as_after_effects_wrote_them() {
    for name in ["319-precomp-timeremap.aep", "354-footage-pngseq-timeremap.aep"] {
        let bytes = fixture(name);
        let p1 = reimport(&bytes);
        assert_eq!(remap_of(&p1).len(), 1, "{name}");
        let r = export_all(&p1);
        assert_eq!(r.report.count(LossCategory::TimeRemapping), 0, "{name}: {:?}", r.report.losses);
        assert_eq!(r.report.exported.time_remaps, 1, "{name}");
        let (a, b) = (enabled_remaps(&bytes), enabled_remaps(&r.bytes));
        assert_eq!(a.len(), 1, "{name}");
        assert_eq!(a, b, "{name}: the remap record differs from After Effects'");
        let p2 = reimport(&r.bytes);
        assert_eq!(remap_of(&p1), remap_of(&p2), "{name}");
    }
}

/// A 25 fps comp `C` showing a 3 s precomposition `P` (a red solid) as layer 0, with `remap`
/// as the layer's `timeRemap` property.
fn remapped(remap: Property) -> Project {
    let mut p = Project::default();
    let pre = add_comp(&mut p, "P", 640, 360, FrameRate::new(25, 1), 3.0, None);
    let s = add_solid(&mut p, "S", 640, 360, [1.0, 0.0, 0.0], None);
    add_layer(&mut p, pre, "S", LayerSource::Solid { item: s }, (640, 360));
    let c = add_comp(&mut p, "C", 1920, 1080, FrameRate::new(25, 1), 5.0, None);
    add_layer(&mut p, c, "P", LayerSource::Comp { item: pre }, (640, 360));
    with_layer(&mut p, c, 0, |l| l.props.children.insert(0, remap.into()));
    p
}

fn remap_prop(keys: Vec<Keyframe>) -> Property {
    let mut r = Property::new(9_000, "timeRemap", "Time Remap", Value::Scalar(0.0));
    r.keys = keys;
    r
}

#[test]
fn remaps_built_in_code_come_back() {
    let k = |t: f64, v: f64| key(t, Value::Scalar(v));
    let hold = Keyframe { out_interp: Interp::Hold, ..k(1.0, 2.0) };
    let ease = Keyframe {
        in_interp: Interp::Bezier,
        out_interp: Interp::Bezier,
        in_ease: vec![Ease { speed: 0.5, influence: 0.4 }],
        out_ease: vec![Ease { speed: 1.5, influence: 0.25 }],
        label: 3,
        ..k(2.0, 1.0)
    };
    let cases: Vec<(&str, Property)> = vec![
        ("reverse", remap_prop(vec![k(0.0, 3.0), k(3.0, 0.0)])),
        ("hold and ease", remap_prop(vec![k(0.0, 0.0), hold, ease, k(4.96, 2.96)])),
        ("expression", {
            let mut r = remap_prop(vec![k(0.0, 0.0), k(3.0, 3.0)]);
            r.expr = Some(Expression { text: "time * 0.5".into(), enabled: true });
            r
        }),
        ("static", Property { value: Value::Scalar(1.2), ..remap_prop(Vec::new()) }),
    ];
    for (name, prop) in cases {
        let p1 = remapped(prop.clone());
        let r = export_all(&p1);
        assert!(r.report.is_lossless(), "{name}: {:?}", r.report.losses);
        assert_eq!(r.report.exported.time_remaps, 1, "{name}");
        let file = parse(&r.bytes).expect("parse");
        assert_eq!(effectcraft_aep::write(&file).expect("write"), r.bytes, "{name}");
        let back = remap_of(&reimport(&r.bytes));
        assert_eq!(back.len(), 1, "{name}");
        let got = &back[0];
        if prop.keys.is_empty() {
            // Written as one key holding the value (a keyless remap reads back off in After
            // Effects, r1).
            assert_eq!(got.keys.len(), 1, "{name}");
            assert_eq!(got.keys[0].value, prop.value, "{name}");
            continue;
        }
        assert_eq!(got.keys.len(), prop.keys.len(), "{name}");
        for (a, b) in got.keys.iter().zip(&prop.keys) {
            assert_eq!((a.time, a.in_interp, a.out_interp, a.label), (b.time, b.in_interp, b.out_interp, b.label), "{name}");
            assert_eq!(a.value, b.value, "{name}");
            if b.in_interp == Interp::Bezier {
                assert_eq!(a.in_ease, b.in_ease, "{name}");
                assert_eq!(a.out_ease, b.out_ease, "{name}");
            }
        }
        assert_eq!(got.expr.as_ref().map(|e| (&e.text, e.enabled)), prop.expr.as_ref().map(|e| (&e.text, e.enabled)), "{name}");
    }
}

#[test]
fn a_remapped_layer_may_run_past_its_source() {
    // A remap holding the last frame: the layer runs 5 s over a 3 s source without a timing loss.
    let mut p = remapped(remap_prop(vec![key(0.0, Value::Scalar(0.0)), key(3.0, Value::Scalar(3.0))]));
    let c = p.comps().find(|(_, c)| c.width == 1920).map(|(id, _)| *id).unwrap();
    with_layer(&mut p, c, 0, |l| {
        l.out_point = secs(5.0);
    });
    let r = export_all(&p);
    assert_eq!(r.report.count(LossCategory::LayerTiming), 0, "{:?}", r.report.losses);
}

#[test]
fn a_remap_where_after_effects_has_none_is_reported() {
    // A solid cannot be remapped in After Effects (319): reported, not written.
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 1920, 1080, FrameRate::new(25, 1), 5.0, None);
    let s = add_solid(&mut p, "S", 100, 100, [1.0, 0.0, 0.0], None);
    add_layer(&mut p, c, "S", LayerSource::Solid { item: s }, (100, 100));
    with_layer(&mut p, c, 0, |l| l.props.children.insert(0, remap_prop(vec![key(0.0, Value::Scalar(0.0))]).into()));
    let r = export_all(&p);
    assert_eq!(r.report.count(LossCategory::TimeRemapping), 1, "{:?}", r.report.losses);
    assert!(enabled_remaps(&r.bytes).is_empty());
}

#[test]
fn hostile_remaps_never_panic() {
    let many: Vec<Keyframe> = (0..20_000).map(|i| key(f64::from(i) * 0.000_2, Value::Scalar(f64::from(i)))).collect();
    let cases = vec![
        remap_prop(vec![key(0.0, Value::Scalar(f64::NAN)), key(1.0, Value::Scalar(f64::INFINITY))]),
        remap_prop(vec![key(-10.0, Value::Scalar(1e300)), key(1e9, Value::Scalar(-1e300))]),
        remap_prop(vec![key(1.0, Value::Vec3([1.0, 2.0, 3.0])), key(1.0, Value::Bool(true))]),
        remap_prop(many),
        Property { value: Value::Scalar(f64::NAN), ..remap_prop(Vec::new()) },
    ];
    for prop in cases {
        let r = export_all(&remapped(prop));
        let file = parse(&r.bytes).expect("parse");
        assert_eq!(effectcraft_aep::write(&file).expect("write"), r.bytes);
        let _ = reimport(&r.bytes);
    }
}
