//! Round-trip tests: build comps feature by feature, export → import, compare property values
//! and rendered frames; plus easing math and document-level checks.

use effectcraft_color::{BlendMode, Label};
use effectcraft_keyframe::{Ease, Gradient, Interp, Keyframe, ShapePath, TextDoc, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{
    AlphaMode, Comp, Expression, Footage, FootageKind, GroupKind, ItemId, ItemKind, Layer, LayerId, LayerSource, Marker, MaskMode, MatteKind, Node, Project,
    PropGroup, Solid, TrackMatte,
};
use effectcraft_render::render_frame;
use effectcraft_time::{FrameRate, Tick};
use serde_json::{Value as Json, json};

use crate::{ExportOptions, export_comp, import};

fn s(x: f64) -> Tick {
    Tick::from_seconds_f64(x)
}

fn setup() -> (Project, ItemId) {
    let mut p = Project::default();
    let comp = Comp::new(200, 120, FrameRate::FPS_30, s(2.0));
    let cid = p.add_item("Main", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    (p, cid)
}

fn comp(p: &Project, cid: ItemId) -> Comp {
    p.comp(cid).unwrap().clone()
}

fn push(p: &mut Project, cid: ItemId, l: Layer) -> LayerId {
    let id = l.id;
    p.comp_mut(cid).unwrap().layers.push(l);
    id
}

fn solid_layer(p: &mut Project, cid: ItemId, color: [f32; 3], w: u32, h: u32) -> Layer {
    let c = comp(p, cid);
    let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color, width: w, height: h, pixel_aspect: 1.0 }));
    build::layer(p, &c, "Solid", LayerSource::Solid { item: sid }, (w, h), None)
}

fn shape_layer(p: &mut Project, cid: ItemId, items: impl FnOnce(&mut Ids) -> Vec<PropGroup>) -> Layer {
    let c = comp(p, cid);
    let mut l = build::layer(p, &c, "Shape Layer 1", LayerSource::Shape, (c.width, c.height), None);
    let mut next = p.next_id;
    let groups = items(&mut Ids(&mut next));
    p.next_id = next;
    l.props.sub_mut("contents").unwrap().children.extend(groups.into_iter().map(Node::Group));
    l
}

fn no_read(_: &str) -> Option<Vec<u8>> {
    None
}

/// Export comp `cid`, serialise, import into a fresh project.
fn roundtrip(p: &Project, cid: ItemId, opts: &ExportOptions) -> (Project, ItemId, Vec<String>, Json) {
    let res = export_comp(p, cid, opts, &no_read).unwrap();
    let text = res.to_string_compact();
    let mut q = Project::default();
    let imp = import(&mut q, text.as_bytes(), "Imported", "", &mut |_| None).unwrap();
    (q, imp.comp, res.warnings, res.json)
}

fn close(a: &Value, b: &Value, tol: f64) -> bool {
    let near = |x: f64, y: f64| (x - y).abs() <= tol * (1.0 + x.abs().max(y.abs()) * 1e-3);
    match (a, b) {
        (Value::Path(p), Value::Path(q)) => {
            let pts = |u: &[[f64; 2]], v: &[[f64; 2]]| u.len() == v.len() && u.iter().zip(v).all(|(m, n)| near(m[0], n[0]) && near(m[1], n[1]));
            p.closed == q.closed && pts(&p.vertices, &q.vertices) && pts(&p.in_tangents, &q.in_tangents) && pts(&p.out_tangents, &q.out_tangents)
        }
        (Value::Gradient(g), Value::Gradient(h)) => {
            g.colors.len() == h.colors.len()
                && g.colors.iter().zip(&h.colors).all(|(x, y)| near(x.0, y.0) && (0..3).all(|i| near(x.1[i] as f64, y.1[i] as f64)))
                && g.opacities.iter().zip(&h.opacities).all(|(x, y)| near(x.0, y.0) && near(x.1 as f64, y.1 as f64))
        }
        (Value::Text(x), Value::Text(y)) => x == y,
        _ => {
            let (ca, cb) = (a.components(), b.components());
            ca.len() == cb.len() && ca.iter().zip(&cb).all(|(x, y)| near(*x, *y))
        }
    }
}

/// Compare every property of `a` with the same-path property of `b` at the sample times.
/// Returns the number of properties compared.
fn compare_tree(a: &PropGroup, b: &PropGroup, times: &[Tick], path: &str) -> usize {
    let mut n = 0;
    for (i, c) in a.children.iter().enumerate() {
        let occ = a.children[..i].iter().filter(|o| o.match_id() == c.match_id()).count();
        let Some(other) = b.children.iter().filter(|o| o.match_id() == c.match_id()).nth(occ) else {
            continue;
        };
        let here = format!("{path}/{}", c.match_id());
        match (c, other) {
            (Node::Prop(x), Node::Prop(y)) => {
                for t in times {
                    let (va, vb) = (x.value_at(*t), y.value_at(*t));
                    assert!(close(&va, &vb, 1e-3), "{here} at {:.3}s: {va:?} != {vb:?}", t.seconds());
                }
                assert_eq!(x.expr.as_ref().map(|e| &e.text), y.expr.as_ref().map(|e| &e.text), "{here}: expression");
                n += 1;
            }
            (Node::Group(x), Node::Group(y)) => {
                if let (GroupKind::Mask { mode: m1, inverted: i1, .. }, GroupKind::Mask { mode: m2, inverted: i2, .. }) = (&x.kind, &y.kind) {
                    assert_eq!((m1, i1), (m2, i2), "{here}: mask mode");
                }
                assert_eq!(x.enabled, y.enabled, "{here}: enabled");
                n += compare_tree(x, y, times, &here);
            }
            _ => panic!("{here}: node kind differs"),
        }
    }
    n
}

fn sample_times(c: &Comp) -> Vec<Tick> {
    (0..=12).map(|i| Tick(c.duration.0 * i / 12)).collect()
}

/// Compare layers (timing, switches, mattes, parents, properties).
fn compare_comps(p: &Project, a: ItemId, q: &Project, b: ItemId) -> usize {
    let (ca, cb) = (p.comp(a).unwrap(), q.comp(b).unwrap());
    assert_eq!((ca.width, ca.height, ca.frame_rate, ca.duration), (cb.width, cb.height, cb.frame_rate, cb.duration));
    assert_eq!(ca.layers.len(), cb.layers.len(), "layer count");
    let times = sample_times(ca);
    let mut n = 0;
    for (la, lb) in ca.layers.iter().zip(&cb.layers) {
        assert_eq!(la.name, lb.name);
        assert_eq!(la.source.type_name(), lb.source.type_name(), "{}", la.name);
        assert_eq!((la.in_point, la.out_point, la.start_time), (lb.in_point, lb.out_point, lb.start_time), "{}: timing", la.name);
        assert!((la.stretch - lb.stretch).abs() < 1e-9, "{}: stretch", la.name);
        assert_eq!(la.blend_mode, lb.blend_mode, "{}: blend", la.name);
        assert_eq!(la.parent.and_then(|x| ca.index_of(x)), lb.parent.and_then(|x| cb.index_of(x)), "{}: parent", la.name);
        assert_eq!(la.track_matte.map(|m| (ca.index_of(m.layer), m.kind)), lb.track_matte.map(|m| (cb.index_of(m.layer), m.kind)), "{}: matte", la.name);
        n += compare_tree(&la.props, &lb.props, &times, &la.name);
    }
    n
}

/// Rendered frames must agree within a small tolerance.
fn compare_renders(p: &Project, a: ItemId, q: &Project, b: ItemId, tol: f32) {
    let c = p.comp(a).unwrap();
    assert!(sample_times(c).into_iter().any(|t| not_blank(p, a, t)), "nothing rendered");
    for t in sample_times(c).into_iter().step_by(3) {
        let (x, y) = (render_frame(p, a, t, 0.5), render_frame(q, b, t, 0.5));
        assert_eq!((x.width, x.height), (y.width, y.height));
        let mut worst = 0.0f32;
        let mut bad = 0usize;
        for (u, v) in x.data.iter().zip(&y.data) {
            let d = (0..4).map(|i| (u[i] - v[i]).abs()).fold(0.0, f32::max);
            worst = worst.max(d);
            if d > tol {
                bad += 1;
            }
        }
        assert!(bad * 200 <= x.data.len(), "at {:.2}s: {bad} pixels differ by more than {tol} (worst {worst})", t.seconds());
    }
}

fn not_blank(p: &Project, cid: ItemId, t: Tick) -> bool {
    render_frame(p, cid, t, 0.5).data.iter().any(|px| px[3] > 0.5)
}

// ---------------------------------------------------------------- easing math

#[test]
fn easing_converts_exactly_both_ways() {
    let (mut p, cid) = setup();
    let mut l = solid_layer(&mut p, cid, [1.0, 0.5, 0.0], 40, 40);
    let tr = l.props.sub_mut("transform").unwrap();
    // Custom speed / influence (non-spatial), easy ease, hold, auto-bezier and linear sides.
    let rot = tr.get_mut("rotation").unwrap();
    let mut a = Keyframe::new(s(0.0), Value::Scalar(0.0));
    a.out_interp = Interp::Bezier;
    a.out_ease = vec![Ease { speed: 300.0, influence: 0.6 }];
    let mut b = Keyframe::new(s(0.8), Value::Scalar(180.0));
    b.in_interp = Interp::Bezier;
    b.in_ease = vec![Ease { speed: -50.0, influence: 0.2 }];
    b.out_interp = Interp::Hold;
    let c = Keyframe::new(s(1.2), Value::Scalar(90.0)).eased();
    let mut d = Keyframe::new(s(1.6), Value::Scalar(45.0));
    d.auto_bezier = true;
    d.in_interp = Interp::Bezier;
    d.out_interp = Interp::Bezier;
    let e = Keyframe::new(s(1.9), Value::Scalar(0.0));
    rot.keys = vec![a, b, c, d, e];
    let sc = tr.get_mut("scale").unwrap();
    sc.keys = vec![Keyframe::new(s(0.0), Value::Vec3([100.0, 50.0, 100.0])).eased(), Keyframe::new(s(1.0), Value::Vec3([20.0, 150.0, 100.0])).eased()];
    sc.keys[0].out_ease = vec![Ease { speed: 10.0, influence: 0.9 }, Ease { speed: 0.0, influence: 0.1 }, Ease::EASY];
    push(&mut p, cid, l);
    let (q, nc, _, json) = roundtrip(&p, cid, &ExportOptions::default());
    // The Lottie tangents of the first rotation segment: o = (0.6, 300·0.6·0.8/180).
    let k0 = &json["layers"][0]["ks"]["r"]["k"][0];
    assert!((k0["o"]["x"][0].as_f64().unwrap() - 0.6).abs() < 1e-9);
    assert!((k0["o"]["y"][0].as_f64().unwrap() - 300.0 * 0.6 * 0.8 / 180.0).abs() < 1e-5);
    assert_eq!(json["layers"][0]["ks"]["r"]["k"][1]["h"], json!(1));
    let scale = &json["layers"][0]["ks"]["s"]["k"][0];
    assert!((scale["o"]["x"][0].as_f64().unwrap() - 0.9).abs() < 1e-9);
    assert!((scale["i"]["x"][0].as_f64().unwrap() - 2.0 / 3.0).abs() < 1e-6);
    assert!((scale["o"]["y"][0].as_f64().unwrap() + 0.1125).abs() < 1e-9);
    let rq = &q.comp(nc).unwrap().layers[0];
    let rk = &rq.props.prop("transform/rotation").unwrap().keys;
    assert!((rk[0].out_ease[0].speed - 300.0).abs() < 1e-3 && (rk[0].out_ease[0].influence - 0.6).abs() < 1e-6, "{:?}", rk[0].out_ease);
    assert!((rk[1].in_ease[0].speed + 50.0).abs() < 1e-3);
    assert_eq!(rk[1].out_interp, Interp::Hold);
    assert!(compare_comps(&p, cid, &q, nc) > 5);
}

#[test]
fn spatial_tangents_and_separated_dimensions() {
    let (mut p, cid) = setup();
    let mut l = solid_layer(&mut p, cid, [0.0, 1.0, 0.0], 20, 20);
    let pos = l.props.prop_mut("transform/position").unwrap();
    let mut k1 = Keyframe::new(s(0.0), Value::Vec3([20.0, 20.0, 0.0])).eased();
    k1.spatial_auto = false;
    k1.spatial_out = [80.0, 0.0, 0.0];
    let k2 = Keyframe::new(s(1.0), Value::Vec3([180.0, 100.0, 0.0])).eased();
    let k3 = Keyframe::new(s(1.5), Value::Vec3([100.0, 60.0, 0.0]));
    pos.keys = vec![k1, k2, k3];
    push(&mut p, cid, l);
    // A second layer with separated X/Y position.
    let mut l2 = solid_layer(&mut p, cid, [0.0, 0.0, 1.0], 20, 20);
    let mut next = p.next_id;
    let tr = l2.props.sub_mut("transform").unwrap();
    for (m, name, keys) in [
        ("positionY", "Y Position", vec![Keyframe::new(s(0.0), Value::Scalar(10.0)).eased(), Keyframe::new(s(2.0), Value::Scalar(110.0))]),
        ("positionX", "X Position", vec![Keyframe::new(s(0.5), Value::Scalar(30.0)), Keyframe::new(s(1.5), Value::Scalar(170.0)).eased()]),
    ] {
        let mut pr = effectcraft_project::Property::new(next, m, name, Value::Scalar(0.0));
        next += 1;
        pr.keys = keys;
        tr.children.insert(2, Node::Prop(pr));
    }
    let mut pz = effectcraft_project::Property::new(next, "positionZ", "Z Position", Value::Scalar(0.0));
    pz.three_d_only = true;
    next += 1;
    tr.children.insert(4, Node::Prop(pz));
    p.next_id = next;
    push(&mut p, cid, l2);
    let (q, nc, _, json) = roundtrip(&p, cid, &ExportOptions::default());
    assert_eq!(json["layers"][1]["ks"]["p"]["s"], json!(true));
    let to = &json["layers"][0]["ks"]["p"]["k"][0]["to"];
    assert_eq!(to[0].as_f64(), Some(80.0));
    assert!(compare_comps(&p, cid, &q, nc) > 10);
    compare_renders(&p, cid, &q, nc, 2.0 / 255.0);
}

// ---------------------------------------------------------------- shapes

#[test]
fn shape_contents_roundtrip() {
    let (mut p, cid) = setup();
    let l = shape_layer(&mut p, cid, |ids| {
        let mut rect = build::shape_rect(ids, [60.0, 40.0], [0.0, 0.0], 6.0);
        rect.get_mut("size").unwrap().keys = vec![Keyframe::new(s(0.0), Value::Vec2([60.0, 40.0])).eased(), Keyframe::new(s(1.5), Value::Vec2([90.0, 20.0]))];
        let ell = build::shape_ellipse(ids, [30.0, 30.0], [20.0, 10.0]);
        let mut stroke = build::shape_stroke(ids, [0.0, 0.0, 1.0, 1.0], 3.0);
        {
            let d = stroke.sub_mut("dashes").unwrap();
            d.get_mut("dash").unwrap().value = Value::Scalar(6.0);
            d.get_mut("gap").unwrap().value = Value::Scalar(3.0);
        }
        stroke.get_mut("cap").unwrap().value = Value::Enum(1);
        stroke.get_mut("join").unwrap().value = Value::Enum(2);
        let mut fill = build::shape_fill(ids, [1.0, 0.2, 0.2, 1.0]);
        fill.get_mut("color").unwrap().keys =
            vec![Keyframe::new(s(0.0), Value::Color([1.0, 0.2, 0.2, 1.0])).hold(), Keyframe::new(s(1.0), Value::Color([0.2, 1.0, 0.2, 1.0])).hold()];
        fill.get_mut("rule").unwrap().value = Value::Enum(1);
        let mut g = build::shape_group(ids, "Group A", vec![rect, ell, stroke, fill]);
        let tr = g.sub_mut("transform").unwrap();
        tr.get_mut("position").unwrap().keys = vec![Keyframe::new(s(0.0), Value::Vec2([60.0, 60.0])), Keyframe::new(s(2.0), Value::Vec2([90.0, 50.0]))];
        tr.get_mut("rotation").unwrap().value = Value::Scalar(15.0);
        tr.get_mut("skew").unwrap().value = Value::Scalar(10.0);
        g.get_mut("blend").unwrap().value = Value::Enum(BlendMode::ALL.iter().position(|b| *b == BlendMode::Screen).unwrap() as u32);
        let star = build::shape_star(ids, true, 5.0, [150.0, 40.0], 30.0, 12.0);
        let poly = build::shape_star(ids, false, 6.0, [150.0, 95.0], 20.0, 0.0);
        let path = build::shape_path(
            ids,
            ShapePath {
                vertices: vec![[10.0, 110.0], [40.0, 80.0], [70.0, 110.0]],
                in_tangents: vec![[0.0, 0.0], [-10.0, 0.0], [0.0, 0.0]],
                out_tangents: vec![[0.0, 0.0], [10.0, 0.0], [0.0, 0.0]],
                closed: true,
                feather: Vec::new(),
            },
        );
        let grad = Gradient {
            colors: vec![(0.0, [1.0, 1.0, 0.0, 1.0]), (0.5, [0.0, 1.0, 1.0, 1.0]), (1.0, [1.0, 0.0, 1.0, 1.0])],
            opacities: vec![(0.0, 1.0), (1.0, 0.5)],
        };
        let gfill = build::shape_gradient_fill(ids, true, [150.0, 40.0], [180.0, 40.0], grad.clone());
        let gstroke = build::shape_gradient_stroke(ids, false, [130.0, 95.0], [170.0, 95.0], grad, 4.0);
        let mut g2 = build::shape_group(ids, "Group B", vec![star, gfill, poly, gstroke]);
        g2.enabled = true;
        let mut fill2 = build::shape_fill(ids, [0.9, 0.9, 0.1, 1.0]);
        fill2.get_mut("opacity").unwrap().value = Value::Scalar(70.0);
        let g3 = build::shape_group(ids, "Group C", vec![path, fill2]);
        vec![g, g2, g3]
    });
    push(&mut p, cid, l);
    let (q, nc, warnings, _) = roundtrip(&p, cid, &ExportOptions::default());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(compare_comps(&p, cid, &q, nc) > 60);
    compare_renders(&p, cid, &q, nc, 2.0 / 255.0);
}

#[test]
fn path_operators_roundtrip() {
    let (mut p, cid) = setup();
    let l = shape_layer(&mut p, cid, |ids| {
        let mut out = vec![];
        for (k, (x, y)) in ["round", "offset", "pucker", "twist", "zigzag", "merge"].iter().zip([
            (30.0, 30.0),
            (90.0, 30.0),
            (150.0, 30.0),
            (30.0, 90.0),
            (90.0, 90.0),
            (150.0, 90.0),
        ]) {
            let rect = build::shape_rect(ids, [36.0, 30.0], [x, y], 0.0);
            let ell = build::shape_ellipse(ids, [20.0, 20.0], [x + 10.0, y]);
            let mut op = build::shape_simple_op(ids, k).unwrap();
            match *k {
                "pucker" => op.get_mut("amount").unwrap().value = Value::Scalar(-30.0),
                "twist" => op.get_mut("angle").unwrap().value = Value::Scalar(40.0),
                "zigzag" => op.get_mut("points").unwrap().value = Value::Enum(1),
                "merge" => op.get_mut("mode").unwrap().value = Value::Enum(2),
                "offset" => op.get_mut("amount").unwrap().value = Value::Scalar(-4.0),
                _ => {}
            }
            let fill = build::shape_fill(ids, [0.2, 0.6, 1.0, 1.0]);
            out.push(build::shape_group(ids, k, vec![rect, ell, op, fill]));
        }
        // Trim paths (animated) and a repeater.
        let ell = build::shape_ellipse(ids, [50.0, 50.0], [100.0, 60.0]);
        let mut trim = build::shape_trim(ids, 0.0, 100.0, 0.0);
        trim.get_mut("end").unwrap().keys = vec![Keyframe::new(s(0.0), Value::Scalar(0.0)).eased(), Keyframe::new(s(1.5), Value::Scalar(100.0)).eased()];
        trim.get_mut("mode").unwrap().value = Value::Enum(1);
        let st = build::shape_stroke(ids, [1.0, 1.0, 1.0, 1.0], 4.0);
        out.push(build::shape_group(ids, "Trim", vec![ell, trim, st]));
        let r = build::shape_rect(ids, [8.0, 8.0], [10.0, 10.0], 0.0);
        let f = build::shape_fill(ids, [1.0, 0.0, 0.0, 1.0]);
        let mut rep = build::shape_repeater(ids, 4.0, [12.0, 0.0]);
        rep.get_mut("composite").unwrap().value = Value::Enum(1);
        let rt = rep.sub_mut("transform").unwrap();
        rt.get_mut("rotation").unwrap().value = Value::Scalar(10.0);
        rt.get_mut("endOpacity").unwrap().value = Value::Scalar(30.0);
        out.push(build::shape_group(ids, "Repeat", vec![r, f, rep]));
        out
    });
    push(&mut p, cid, l);
    let (q, nc, warnings, _) = roundtrip(&p, cid, &ExportOptions::default());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(compare_comps(&p, cid, &q, nc) > 50);
    compare_renders(&p, cid, &q, nc, 2.0 / 255.0);
}

#[test]
fn offset_paths_miter_uses_animatable_lottie_value() {
    let (mut p, cid) = setup();
    let l = shape_layer(&mut p, cid, |ids| {
        let mut animated = build::shape_simple_op(ids, "offset").unwrap();
        animated.get_mut("miter").unwrap().keys = vec![Keyframe::new(s(0.0), Value::Scalar(5.0)), Keyframe::new(s(1.0), Value::Scalar(9.0))];
        let mut constant = build::shape_simple_op(ids, "offset").unwrap();
        constant.get_mut("miter").unwrap().value = Value::Scalar(6.0);
        let mut stroke = build::shape_stroke(ids, [1.0, 1.0, 1.0, 1.0], 2.0);
        stroke.get_mut("miter").unwrap().value = Value::Scalar(7.0);
        vec![animated, constant, stroke]
    });
    push(&mut p, cid, l);

    let (q, nc, warnings, json) = roundtrip(&p, cid, &ExportOptions::default());
    assert!(warnings.is_empty(), "{warnings:?}");
    let shapes = &json["layers"][0]["shapes"];
    assert_eq!(shapes[0]["ty"], "op");
    assert_eq!(shapes[0]["ml"]["a"], json!(1));
    assert_eq!(shapes[0]["ml"]["k"][0]["s"], json!([5.0]));
    assert_eq!(shapes[0]["ml"]["k"][1]["s"], json!([9.0]));
    assert_eq!(shapes[1]["ml"], json!({"a": 0, "k": 6.0}));
    assert_eq!(shapes[2]["ty"], "st");
    assert_eq!(shapes[2]["ml"], json!(7.0));
    assert!(compare_comps(&p, cid, &q, nc) > 10);
    let restored = &q.comp(nc).unwrap().layers[0];
    assert_eq!(restored.props.group("contents/offset").unwrap().get("miter").unwrap().keys.len(), 2);

    // Previously exported Offset Paths have a scalar ml; they remain importable.
    let mut old = json;
    old["layers"][0]["shapes"][1]["ml"] = json!(6.0);
    let mut legacy = Project::default();
    let bytes = serde_json::to_vec(&old).unwrap();
    let imported = import(&mut legacy, &bytes, "Legacy", "", &mut |_| None).unwrap();
    let constant = &legacy.comp(imported.comp).unwrap().layers[0].props;
    let second = constant
        .sub("contents")
        .unwrap()
        .children
        .iter()
        .filter_map(|n| match n {
            Node::Group(g) if g.match_id == "offset" => Some(g),
            _ => None,
        })
        .nth(1)
        .unwrap();
    assert_eq!(second.get("miter").unwrap().value, Value::Scalar(6.0));
}

// ---------------------------------------------------------------- layers

#[test]
fn layers_parenting_timing_mattes_masks_and_blend_modes() {
    let (mut p, cid) = setup();
    let c = comp(&p, cid);
    // Null parent with rotation; child solid.
    let mut null = build::layer(&mut p, &c, "Null 1", LayerSource::Null, (100, 100), None);
    null.props.prop_mut("transform/rotation").unwrap().keys = vec![Keyframe::new(s(0.0), Value::Scalar(0.0)), Keyframe::new(s(2.0), Value::Scalar(90.0))];
    let null_id = null.id;
    let mut child = solid_layer(&mut p, cid, [1.0, 1.0, 0.0], 40, 20);
    child.parent = Some(null_id);
    child.props.prop_mut("transform/anchor").unwrap().value = Value::Vec3([0.0, 0.0, 0.0]);
    child.props.prop_mut("transform/scale").unwrap().value = Value::Vec3([150.0, 80.0, 100.0]);
    child.in_point = s(0.2);
    child.out_point = s(1.8);
    child.start_time = s(0.1);
    child.stretch = 200.0;
    child.props.prop_mut("transform/opacity").unwrap().keys = vec![Keyframe::new(s(0.0), Value::Scalar(20.0)), Keyframe::new(s(0.8), Value::Scalar(100.0))];
    // Matte (ellipse shape) over a solid with masks and a blend mode.
    let matte = shape_layer(&mut p, cid, |ids| {
        let e = build::shape_ellipse(ids, [120.0, 80.0], [100.0, 60.0]);
        let f = build::shape_fill(ids, [1.0, 1.0, 1.0, 1.0]);
        vec![build::shape_group(ids, "Ellipse", vec![e, f])]
    });
    let mut matte = matte;
    matte.name = "Matte".into();
    matte.switches.video = false;
    let matte_id = matte.id;
    let mut masked = solid_layer(&mut p, cid, [0.2, 0.4, 1.0], 200, 120);
    masked.name = "Masked".into();
    masked.blend_mode = BlendMode::Multiply;
    masked.track_matte = Some(TrackMatte { layer: matte_id, kind: MatteKind::Alpha });
    let mut next = p.next_id;
    {
        let mut ids = Ids(&mut next);
        let mut m1 = build::mask(&mut ids, "Mask 1", ShapePath::rect([100.0, 60.0], 150.0, 90.0), MaskMode::Add, [255, 0, 0]);
        m1.get_mut("expansion").unwrap().value = Value::Scalar(-5.0);
        m1.get_mut("path").unwrap().keys = vec![
            Keyframe::new(s(0.0), Value::Path(ShapePath::rect([100.0, 60.0], 150.0, 90.0))),
            Keyframe::new(s(2.0), Value::Path(ShapePath::rect([90.0, 60.0], 120.0, 80.0))),
        ];
        let mut m2 = build::mask(&mut ids, "Mask 2", ShapePath::ellipse([100.0, 60.0], 40.0, 40.0), MaskMode::Subtract, [0, 255, 0]);
        m2.get_mut("opacity").unwrap().value = Value::Scalar(60.0);
        let mut m3 = build::mask(&mut ids, "Mask 3", ShapePath::rect([30.0, 30.0], 20.0, 20.0), MaskMode::Add, [0, 0, 255]);
        if let GroupKind::Mask { inverted, .. } = &mut m3.kind {
            *inverted = true;
        }
        m3.get_mut("feather").unwrap().value = Value::Vec2([4.0, 4.0]);
        let masks = masked.props.sub_mut("masks").unwrap();
        masks.children.extend([m1, m2, m3].map(Node::Group));
    }
    p.next_id = next;
    // A luma-inverted matte pair below.
    let mut luma = solid_layer(&mut p, cid, [0.5, 0.5, 0.5], 200, 120);
    luma.name = "Luma".into();
    luma.switches.video = false;
    let luma_id = luma.id;
    let mut bg = solid_layer(&mut p, cid, [0.9, 0.1, 0.1], 200, 120);
    bg.name = "Bg".into();
    bg.track_matte = Some(TrackMatte { layer: luma_id, kind: MatteKind::LumaInverted });
    for l in [null, child, matte, masked, luma, bg] {
        push(&mut p, cid, l);
    }
    p.comp_mut(cid).unwrap().markers.push(Marker { time: s(1.0), duration: s(0.5), comment: "beat".into(), ..Default::default() });
    let (q, nc, warnings, json) = roundtrip(&p, cid, &ExportOptions::default());
    assert!(warnings.iter().any(|w| w.contains("feather")), "{warnings:?}");
    assert_eq!(json["layers"][2]["td"], json!(1));
    assert_eq!(json["layers"][3]["tt"], json!(1));
    assert_eq!(json["layers"][3]["masksProperties"][1]["mode"], json!("s"));
    assert_eq!(json["layers"][5]["tt"], json!(4));
    assert_eq!(json["markers"][0]["cm"], json!("beat"));
    assert_eq!(q.comp(nc).unwrap().markers[0].time, s(1.0));
    assert!(compare_comps(&p, cid, &q, nc) > 20);
    compare_renders(&p, cid, &q, nc, 2.0 / 255.0);
}

#[test]
fn precomp_with_time_remap() {
    let (mut p, cid) = setup();
    let inner = p.add_item("Inner", Label::Sandstone, None, ItemKind::Comp(Comp::new(100, 100, FrameRate::FPS_30, s(2.0)).into()));
    let sl = shape_layer(&mut p, inner, |ids| {
        let mut r = build::shape_rect(ids, [40.0, 40.0], [0.0, 0.0], 0.0);
        r.get_mut("position").unwrap().keys = vec![Keyframe::new(s(0.0), Value::Vec2([-20.0, 0.0])), Keyframe::new(s(2.0), Value::Vec2([60.0, 0.0]))];
        let f = build::shape_fill(ids, [0.0, 1.0, 0.5, 1.0]);
        vec![build::shape_group(ids, "Box", vec![r, f])]
    });
    push(&mut p, inner, sl);
    let c = comp(&p, cid);
    let mut pre = build::layer(&mut p, &c, "Inner", LayerSource::Comp { item: inner }, (100, 100), None);
    let mut next = p.next_id;
    let mut tr = Ids(&mut next).prop("timeRemap", "Time Remap", Value::Scalar(0.0));
    tr.keys = vec![Keyframe::new(s(0.0), Value::Scalar(1.5)), Keyframe::new(s(2.0), Value::Scalar(0.0))];
    p.next_id = next;
    pre.props.children.insert(0, Node::Prop(tr));
    push(&mut p, cid, pre);
    let (q, nc, _, json) = roundtrip(&p, cid, &ExportOptions::default());
    assert_eq!(json["assets"].as_array().unwrap().len(), 1);
    assert!(json["layers"][0]["tm"].is_object());
    assert!(compare_comps(&p, cid, &q, nc) > 5);
    let qi = match &q.comp(nc).unwrap().layers[0].source {
        LayerSource::Comp { item } => *item,
        _ => panic!("not a precomp"),
    };
    assert!(compare_comps(&p, inner, &q, qi) > 3);
    compare_renders(&p, cid, &q, nc, 2.0 / 255.0);
}

#[test]
fn text_layers_roundtrip_as_text_and_as_shapes() {
    let (mut p, cid) = setup();
    let c = comp(&p, cid);
    let mut l = build::layer(&mut p, &c, "Title", LayerSource::Text, (c.width, c.height), None);
    let doc = TextDoc { text: "Hi\nthere".into(), size: 30.0, fill: [1.0, 0.8, 0.1, 1.0], tracking: 20.0, ..Default::default() };
    let doc2 = TextDoc { text: "Bye".into(), ..doc.clone() };
    let st = l.props.prop_mut("text/sourceText").unwrap();
    st.value = Value::Text(Box::new(doc.clone()));
    st.keys = vec![Keyframe::new(s(0.0), Value::Text(Box::new(doc))).hold(), Keyframe::new(s(1.5), Value::Text(Box::new(doc2))).hold()];
    l.props.prop_mut("transform/position").unwrap().value = Value::Vec3([30.0, 50.0, 0.0]);
    let mut next = p.next_id;
    {
        let mut ids = Ids(&mut next);
        let mut props = build::text_anim_props(&mut ids, "opacity", false);
        props.extend(build::text_anim_props(&mut ids, "position", false));
        let mut a = build::text_animator(&mut ids, "Animator 1", props);
        let pr = a.sub_mut("properties").unwrap();
        pr.get_mut("opacity").unwrap().value = Value::Scalar(0.0);
        pr.get_mut("position").unwrap().value = Value::Vec3([0.0, 20.0, 0.0]);
        let sel = a.sub_mut("selectors").unwrap().children[0].as_group_mut().unwrap();
        sel.get_mut("start").unwrap().keys = vec![Keyframe::new(s(0.0), Value::Scalar(0.0)), Keyframe::new(s(1.0), Value::Scalar(100.0))];
        sel.sub_mut("advanced").unwrap().get_mut("shape").unwrap().value = Value::Enum(1);
        l.props.group_mut("text/animators").unwrap().children.push(Node::Group(a));
    }
    p.next_id = next;
    push(&mut p, cid, l);
    let (q, nc, _, json) = roundtrip(&p, cid, &ExportOptions::default());
    assert_eq!(json["layers"][0]["ty"], json!(5));
    assert_eq!(json["layers"][0]["t"]["d"]["k"][0]["s"]["t"], json!("Hi\rthere"));
    assert_eq!(json["fonts"]["list"][0]["fFamily"], json!("Inter"));
    assert!(compare_comps(&p, cid, &q, nc) > 10);
    compare_renders(&p, cid, &q, nc, 2.0 / 255.0);
    // As glyph shapes: the static look at time 0 survives (animators baked out).
    p.comp_mut(cid).unwrap().layers[0].props.group_mut("text/animators").unwrap().children.clear();
    let opts = ExportOptions { text_as_shapes: true, ..Default::default() };
    let res = export_comp(&p, cid, &opts, &no_read).unwrap();
    assert_eq!(res.json["layers"][0]["ty"], json!(4));
    let mut q2 = Project::default();
    let imp = import(&mut q2, res.to_string_compact().as_bytes(), "x", "", &mut |_| None).unwrap();
    let (a, b) = (render_frame(&p, cid, s(0.5), 1.0), render_frame(&q2, imp.comp, s(0.5), 1.0));
    let (ca, cb) = (a.data.iter().map(|px| px[3] as f64).sum::<f64>(), b.data.iter().map(|px| px[3] as f64).sum::<f64>());
    assert!(ca > 50.0 && (ca - cb).abs() / ca < 0.05, "glyph coverage {ca} vs {cb}");
}

#[test]
fn effects_expressions_and_warnings() {
    let (mut p, cid) = setup();
    let mut l = solid_layer(&mut p, cid, [1.0, 1.0, 1.0], 80, 60);
    let mut next = p.next_id;
    {
        let mut ids = Ids(&mut next);
        let fx = l.props.sub_mut("effects").unwrap();
        for id in ["ec.blur.gaussian", "ec.color.tint", "ec.perspective.dropshadow", "ec.control.slider", "ec.distort.twirl"] {
            if let Some(spec) = effectcraft_effects::find(id) {
                let mut g = effectcraft_effects::instantiate(spec, &mut ids, spec.name, [80.0, 60.0]);
                if let Some(b) = g.get_mut("blurriness") {
                    b.keys = vec![Keyframe::new(s(0.0), Value::Scalar(0.0)), Keyframe::new(s(1.0), Value::Scalar(8.0))];
                }
                if let Some(o) = g.get_mut("opacity") {
                    o.value = Value::Scalar(40.0);
                }
                if let Some(a) = g.get_mut("amount") {
                    a.value = Value::Scalar(60.0);
                }
                if let Some(sl) = g.get_mut("slider") {
                    sl.value = Value::Scalar(12.5);
                }
                fx.children.push(Node::Group(g));
            }
        }
    }
    p.next_id = next;
    l.props.prop_mut("transform/rotation").unwrap().expr = Some(Expression { text: "time * 90".into(), enabled: true });
    push(&mut p, cid, l);
    let c = comp(&p, cid);
    let cam = build::layer(&mut p, &c, "Camera 1", LayerSource::Camera, (c.width, c.height), None);
    push(&mut p, cid, cam);
    // Without the expression flag: warned and dropped.
    let res = export_comp(&p, cid, &ExportOptions::default(), &no_read).unwrap();
    assert!(res.json["layers"][0]["ks"]["r"].get("x").is_none());
    let w = res.warnings.join("\n");
    assert!(w.contains("expression not exported"), "{w}");
    assert!(w.contains("effect not supported by Lottie"), "{w}");
    assert!(w.contains("cameras and lights"), "{w}");
    // With it: `x` strings, and supported effects survive.
    let opts = ExportOptions { include_expressions: true, ..Default::default() };
    let res = export_comp(&p, cid, &opts, &no_read).unwrap();
    assert_eq!(res.json["layers"][0]["ks"]["r"]["x"], json!("time * 90"));
    let ef = res.json["layers"][0]["ef"].as_array().unwrap();
    assert_eq!(ef.iter().map(|e| e["ty"].as_u64().unwrap()).collect::<Vec<_>>(), vec![29, 20, 25, 5]);
    assert!((ef[2]["ef"][1]["v"]["k"].as_f64().unwrap() - 40.0 * 2.55).abs() < 1e-9);
    let mut q = Project::default();
    let imp = import(&mut q, res.to_string_compact().as_bytes(), "x", "", &mut |_| None).unwrap();
    let lq = &q.comp(imp.comp).unwrap().layers[0];
    let fxq: Vec<&PropGroup> = lq.effects().unwrap().groups().collect();
    assert_eq!(fxq.len(), 4);
    assert!((fxq[2].get("opacity").unwrap().value.as_f64() - 40.0).abs() < 1e-9);
    assert_eq!(fxq[3].get("slider").unwrap().value, Value::Scalar(12.5));
    assert_eq!(lq.props.prop("transform/rotation").unwrap().expr.as_ref().unwrap().text, "time * 90");
    let lp = &p.comp(cid).unwrap().layers[0];
    let times = sample_times(p.comp(cid).unwrap());
    compare_tree(lp.effects().unwrap().groups().next().unwrap(), fxq[0], &times, "blur");
}

#[test]
fn images_embed_as_base64_and_come_back() {
    let (mut p, cid) = setup();
    let png: Vec<u8> = vec![0x89, b'P', b'N', b'G', 1, 2, 3, 4, 5];
    let fid = p.add_item(
        "pic.png",
        Label::Lavender,
        None,
        ItemKind::Footage(Footage {
            path: "/virtual/pic.png".into(),
            kind: FootageKind::Still,
            width: 64,
            height: 32,
            pixel_aspect: 1.0,
            frame_rate: FrameRate::FPS_30,
            native_rate: None,
            duration: Tick::ZERO,
            has_video: true,
            has_audio: false,
            alpha: AlphaMode::Straight,
            premul_color: [0.0; 3],
            loop_count: 1,
            codec: String::new(),
            missing: false,
            sequence: vec![],
            color_profile: None,
            ..Default::default()
        }),
    );
    let c = comp(&p, cid);
    let l = build::layer(&mut p, &c, "pic.png", LayerSource::Footage { item: fid }, (64, 32), None);
    push(&mut p, cid, l);
    let bytes = png.clone();
    let read = move |path: &str| (path == "/virtual/pic.png").then(|| bytes.clone());
    let res = export_comp(&p, cid, &ExportOptions::default(), &read).unwrap();
    let asset = &res.json["assets"][0];
    assert!(asset["p"].as_str().unwrap().starts_with("data:image/png;base64,"));
    assert_eq!(res.json["layers"][0]["ty"], json!(2));
    let mut q = Project::default();
    let mut stored = vec![];
    let imp = import(&mut q, res.to_string_compact().as_bytes(), "x", "", &mut |img| {
        stored.push(img.bytes.clone());
        Some(format!("/tmp/{}.{}", img.id, img.ext))
    })
    .unwrap();
    assert_eq!(stored, vec![png]);
    let l = &q.comp(imp.comp).unwrap().layers[0];
    let LayerSource::Footage { item } = l.source else { panic!("not footage") };
    let ItemKind::Footage(f) = &q.item(item).unwrap().kind else { panic!() };
    assert_eq!((f.width, f.height), (64, 32));
    assert!(f.path.starts_with("/tmp/image_") && f.path.ends_with(".png"), "{}", f.path);
}

#[test]
fn dotlottie_archive_roundtrip() {
    let (mut p, cid) = setup();
    let l = solid_layer(&mut p, cid, [1.0, 0.0, 0.0], 50, 50);
    push(&mut p, cid, l);
    let res = export_comp(&p, cid, &ExportOptions::default(), &no_read).unwrap();
    let zip = crate::to_dotlottie(&res, "Main");
    assert!(zip.starts_with(b"PK"));
    let mut q = Project::default();
    let imp = import(&mut q, &zip, "x", "", &mut |_| None).unwrap();
    assert_eq!(q.comp(imp.comp).unwrap().layers.len(), 1);
}

// ---------------------------------------------------------------- hand-written fixtures

fn fixture(name: &str) -> (Project, ItemId, Vec<String>) {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(&path).unwrap();
    let mut q = Project::default();
    let imp = import(&mut q, &bytes, name, "", &mut |_| None).unwrap_or_else(|e| panic!("{name}: {e}"));
    (q, imp.comp, imp.warnings)
}

#[test]
fn fixture_bouncing_ball() {
    let (q, c, w) = fixture("bouncing-ball.json");
    assert!(w.is_empty(), "{w:?}");
    let comp = q.comp(c).unwrap();
    assert_eq!((comp.width, comp.height, comp.frame_rate), (200, 200, FrameRate::FPS_30));
    let l = &comp.layers[0];
    let pos = l.props.prop("transform/position").unwrap();
    assert_eq!(pos.keys.len(), 3);
    // Eased: slow at the top, the key values are hit exactly.
    assert_eq!(pos.value_at(s(0.5)).as_vec3()[1], 160.0);
    assert!(not_blank(&q, c, s(0.25)));
}

#[test]
fn fixture_shapes_and_gradients() {
    let (q, c, w) = fixture("shapes-gradient.json");
    assert!(w.is_empty(), "{w:?}");
    let l = &q.comp(c).unwrap().layers[0];
    let contents = l.props.sub("contents").unwrap();
    let g = contents.groups().next().unwrap();
    let kinds: Vec<&str> = g.sub("contents").unwrap().groups().map(|x| x.match_id.as_str()).collect();
    assert_eq!(kinds, ["star", "rect", "gfill", "stroke", "trim"]);
    assert!(not_blank(&q, c, s(0.5)));
}

#[test]
fn fixture_precomp_matte_text() {
    let (q, c, w) = fixture("precomp-matte-text.json");
    assert!(w.is_empty(), "{w:?}");
    let comp = q.comp(c).unwrap();
    assert_eq!(comp.layers.len(), 4);
    let matte = &comp.layers[0];
    assert!(!matte.switches.video);
    assert_eq!(comp.layers[1].track_matte.map(|m| (m.layer, m.kind)), Some((matte.id, MatteKind::Alpha)));
    assert!(matches!(comp.layers[1].source, LayerSource::Comp { .. }));
    assert_eq!(comp.layers[2].parent, Some(comp.layers[3].id));
    let Value::Text(doc) = &comp.layers[2].props.prop("text/sourceText").unwrap().value else { panic!() };
    assert_eq!(doc.text, "Hello\nLottie");
    assert_eq!(doc.justify, effectcraft_keyframe::Justify::Center);
    assert!(not_blank(&q, c, s(0.5)));
}

#[test]
fn fixture_legacy_keyframes_and_masks() {
    let (q, c, w) = fixture("legacy-masks.json");
    assert!(w.iter().all(|w| w.contains("not supported")), "{w:?}");
    let comp = q.comp(c).unwrap();
    let l = &comp.layers[0];
    // Legacy `e` end values fill the last key.
    let o = l.props.prop("transform/opacity").unwrap();
    assert_eq!(o.keys.len(), 2);
    assert_eq!(o.keys[1].value, Value::Scalar(100.0));
    let masks: Vec<_> = l.masks().unwrap().groups().collect();
    assert_eq!(masks.len(), 2);
    assert!(matches!(masks[1].kind, GroupKind::Mask { mode: MaskMode::Subtract, inverted: true, .. }));
    assert!(not_blank(&q, c, s(0.9)));
}

#[test]
fn text_style_runs_export_as_range_animators() {
    let (mut p, cid) = setup();
    let c = comp(&p, cid);
    let mut l = build::layer(&mut p, &c, "Runs", LayerSource::Text, (c.width, c.height), None);
    let mut doc = TextDoc { text: "Hello".into(), size: 30.0, fill: [1.0, 1.0, 1.0, 1.0], ..Default::default() };
    doc.apply_style(1..3, |st| {
        st.fill = [1.0, 0.0, 0.0, 1.0];
        st.baseline_shift = 6.0;
    });
    l.props.prop_mut("text/sourceText").unwrap().value = Value::Text(Box::new(doc));
    l.props.prop_mut("transform/position").unwrap().value = Value::Vec3([30.0, 60.0, 0.0]);
    push(&mut p, cid, l);
    let (q, nc, _, json) = roundtrip(&p, cid, &ExportOptions::default());
    let a = &json["layers"][0]["t"]["a"][0];
    assert_eq!(a["s"]["r"], json!(2), "{a}");
    assert_eq!(a["s"]["s"]["k"], json!(1));
    assert_eq!(a["s"]["e"]["k"], json!(3));
    assert_eq!(a["a"]["fc"]["k"], json!([1.0, 0.0, 0.0, 1.0]));
    assert_eq!(a["a"]["p"]["k"], json!([0.0, -6.0, 0.0]));
    // Played back as an animator, it looks the same.
    compare_renders(&p, cid, &q, nc, 2.0 / 255.0);
}

#[test]
fn extreme_enum_numbers_do_not_overflow() {
    // Line cap / join / merge mode / text-selector enums are 1-based; a huge negative number
    // (-1e308 rounds to i64::MIN) overflowed the `- 1`.
    let path = format!("{}/tests/fixtures/shapes-gradient.json", env!("CARGO_MANIFEST_DIR"));
    let mut doc: Json = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let mut hits = 0;
    fn patch(j: &mut Json, hits: &mut usize) {
        match j {
            Json::Object(o) => {
                for k in ["lc", "lj", "mm"] {
                    if o.contains_key(k) {
                        o.insert(k.into(), json!(-1e308));
                        *hits += 1;
                    }
                }
                o.values_mut().for_each(|v| patch(v, hits));
            }
            Json::Array(a) => a.iter_mut().for_each(|v| patch(v, hits)),
            _ => {}
        }
    }
    patch(&mut doc, &mut hits);
    assert!(hits > 0);
    let mut q = Project::default();
    import(&mut q, doc.to_string().as_bytes(), "x", "", &mut |_| None).unwrap();
}

/// External image assets are taken from the Lottie file's folder; one on another computer is not
/// imported.
#[test]
fn external_images_are_local_or_skipped() {
    let base = std::env::temp_dir().join("ec-lottie-anim");
    let mut assets = vec![
        serde_json::json!({"id": "near", "u": "images/", "p": "a.png", "w": 1, "h": 1}),
        serde_json::json!({"id": "unc", "u": "", "p": r"\\192.0.2.1\share\x.png", "w": 1, "h": 1}),
        serde_json::json!({"id": "url", "u": "file://192.0.2.1/s/", "p": "x.png", "w": 1, "h": 1}),
    ];
    // `u` + `p` is `//tmp/…/a.png`, which Linux reads as a local file.
    if cfg!(unix) {
        assets.push(serde_json::json!({"id": "slashes", "u": "/", "p": base.join("a.png").to_string_lossy(), "w": 1, "h": 1}));
    }
    let doc = serde_json::json!({"w": 10, "h": 10, "fr": 30, "ip": 0, "op": 30, "assets": assets, "layers": []});
    let mut q = Project::default();
    let r = import(&mut q, doc.to_string().as_bytes(), "x", &base.to_string_lossy(), &mut |_| None).unwrap();
    let paths: Vec<String> = q.items.values().filter_map(|i| if let ItemKind::Footage(f) = &i.kind { Some(f.path.clone()) } else { None }).collect();
    assert_eq!(paths.len(), 1, "{paths:?}");
    assert_eq!(std::path::Path::new(&paths[0]), base.join("images").join("a.png"));
    assert_eq!(r.warnings.iter().filter(|w| w.contains("is not imported")).count(), assets.len() - 1, "{:?}", r.warnings);
}
