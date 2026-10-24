//! Masks: projects built in code are exported, parsed back with `effectcraft-aep` (every written
//! field compared) and imported again; the mask fixtures go through import -> export -> import.

mod common;

use common::{add_comp, add_layer, add_solid, export_all, key, with_layer};
use effectcraft_aep::{Interpolation, KeyframeKind, MaskMode as AepMode, PropNode, ValueSource, parse, write};
use effectcraft_aep_export::{ExportOptions, LossCategory, export};
use effectcraft_aep_import::{ImportOptions, import};
use effectcraft_keyframe::{Ease, Interp, ShapePath, Value};
use effectcraft_project::{Expression, FeatherFalloff, GroupKind, Layer, LayerSource, MaskMode, Project, PropGroup, build};
use effectcraft_time::FrameRate;

fn add_mask(l: &mut Layer, next: &mut u64, name: &str, path: ShapePath, mode: MaskMode, f: impl FnOnce(&mut PropGroup)) {
    let i = l.masks().map_or(0, |m| m.children.len());
    let mut g = build::mask(&mut build::Ids(next), name, path, mode, build::MASK_COLORS[i % 8]);
    f(&mut g);
    l.props.sub_mut("masks").expect("masks group").children.push(g.into());
}

fn set(g: &mut PropGroup, id: &str, v: Value) {
    g.get_mut(id).expect(id).value = v;
}

fn invert(g: &mut PropGroup) {
    if let GroupKind::Mask { inverted, .. } = &mut g.kind {
        *inverted = true;
    }
}

/// A five-vertex open path with tangents.
fn open_path() -> ShapePath {
    ShapePath {
        vertices: vec![[200.0, 200.0], [600.0, 150.0], [900.0, 500.0], [700.0, 900.0], [300.0, 800.0]],
        in_tangents: vec![[0.0, 0.0], [-100.0, -20.0], [0.0, -150.0], [120.0, 0.0], [40.0, 60.0]],
        out_tangents: vec![[50.0, -30.0], [100.0, 20.0], [0.0, 150.0], [-120.0, 0.0], [0.0, 0.0]],
        closed: false,
        feather: Vec::new(),
    }
}

/// A six-vertex closed polygon with tangents.
fn hexagon() -> ShapePath {
    let mut p = ShapePath::polygon(&[[960.0, 200.0], [1260.0, 370.0], [1260.0, 710.0], [960.0, 880.0], [660.0, 710.0], [660.0, 370.0]], true);
    p.in_tangents[1] = [-40.0, -40.0];
    p.out_tangents[1] = [40.0, 40.0];
    p
}

/// Composition C (1920x1080, 25 fps, 5 s) with solid layer L and the masks `f` adds.
fn project(f: impl FnOnce(&mut Layer, &mut u64)) -> Project {
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 1920, 1080, FrameRate::new(25, 1), 5.0, None);
    let s = add_solid(&mut p, "L", 1920, 1080, [1.0, 0.0, 0.0], None);
    add_layer(&mut p, c, "L", LayerSource::Solid { item: s }, (1920, 1080));
    let mut next = p.next_id;
    with_layer(&mut p, c, 0, |l| f(l, &mut next));
    p.next_id = next;
    p
}

fn all_modes() -> Project {
    project(|l, n| {
        add_mask(l, n, "Rect", ShapePath::rect([960.0, 540.0], 800.0, 400.0), MaskMode::Add, |_| {});
        add_mask(l, n, "Ellipse", ShapePath::ellipse([960.0, 540.0], 600.0, 300.0), MaskMode::Subtract, |g| {
            invert(g);
            set(g, "feather", Value::Vec2([20.0, 10.0]));
            set(g, "opacity", Value::Scalar(50.0));
            set(g, "expansion", Value::Scalar(5.0));
        });
        add_mask(l, n, "Open", open_path(), MaskMode::Intersect, |_| {});
        add_mask(l, n, "Hex", hexagon(), MaskMode::Difference, |g| set(g, "expansion", Value::Scalar(-12.5)));
        add_mask(l, n, "Lighten", ShapePath::rect([300.0, 300.0], 200.0, 100.0), MaskMode::Lighten, |_| {});
        add_mask(l, n, "Darken", ShapePath::rect([1600.0, 300.0], 200.0, 100.0), MaskMode::Darken, |_| {});
        add_mask(l, n, "None", ShapePath::ellipse([300.0, 900.0], 100.0, 100.0), MaskMode::None, |_| {});
    })
}

fn animated() -> Project {
    project(|l, n| {
        add_mask(l, n, "Keyed", ShapePath::rect([960.0, 540.0], 400.0, 200.0), MaskMode::Add, |g| {
            let path = g.get_mut("path").expect("path");
            let mut k1 = key(1.0, Value::Path(ShapePath::rect([960.0, 540.0], 800.0, 400.0)));
            k1.out_interp = Interp::Hold;
            k1.label = 3;
            path.keys = vec![
                key(0.0, Value::Path(ShapePath::rect([960.0, 540.0], 400.0, 200.0))),
                k1,
                key(2.0, Value::Path(ShapePath::rect([900.0, 500.0], 600.0, 300.0))),
            ];
            let feather = g.get_mut("feather").expect("feather");
            let mut a = key(0.0, Value::Vec2([0.0, 0.0]));
            a.out_interp = Interp::Bezier;
            a.out_ease = vec![Ease { speed: 15.0, influence: 0.66 }, Ease { speed: 5.0, influence: 0.5 }];
            let mut b = key(2.0, Value::Vec2([50.0, 30.0]));
            b.in_interp = Interp::Bezier;
            b.in_ease = vec![Ease { speed: 8.0, influence: 0.66 }, Ease { speed: 8.0, influence: 0.66 }];
            feather.keys = vec![a, b];
        });
    })
}

fn masks_of(p: &Project) -> Vec<PropGroup> {
    let c = p.items.values().find_map(|i| match &i.kind {
        effectcraft_project::ItemKind::Comp(c) if i.name == "C" => Some(c.clone()),
        _ => None,
    });
    let c = c.expect("comp C");
    c.layers.iter().find(|l| l.name == "L").and_then(|l| l.masks()).map(|m| m.groups().cloned().collect()).unwrap_or_default()
}

fn reimport(bytes: &[u8]) -> Project {
    let mut p = Project::default();
    let none = |_: &str| false;
    import(&mut p, bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).expect("import");
    p
}

fn path_of(v: &Value) -> &ShapePath {
    match v {
        Value::Path(p) => p,
        other => panic!("{other:?}"),
    }
}

fn same_path(a: &ShapePath, b: &ShapePath, what: &str) {
    assert_eq!((a.len(), a.closed), (b.len(), b.closed), "{what}");
    let near = |x: [f64; 2], y: [f64; 2]| (x[0] - y[0]).abs() < 0.01 && (x[1] - y[1]).abs() < 0.01;
    for i in 0..a.len() {
        assert!(near(a.vertices[i], b.vertices[i]), "{what} vertex {i}: {:?} {:?}", a.vertices[i], b.vertices[i]);
        assert!(near(a.in_tangents[i], b.in_tangents[i]), "{what} in {i}: {:?} {:?}", a.in_tangents[i], b.in_tangents[i]);
        assert!(near(a.out_tangents[i], b.out_tangents[i]), "{what} out {i}: {:?} {:?}", a.out_tangents[i], b.out_tangents[i]);
    }
}

/// Two projects' masks are equal for every written field.
fn same_masks(a: &[PropGroup], b: &[PropGroup]) {
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(b) {
        let w = x.name.as_str();
        assert_eq!(x.name, y.name);
        let k = |g: &PropGroup| match g.kind {
            GroupKind::Mask { mode, inverted, .. } => (mode, inverted),
            _ => panic!("not a mask"),
        };
        assert_eq!(k(x), k(y), "{w}");
        for id in ["path", "feather", "opacity", "expansion"] {
            let (px, py) = (x.get(id).unwrap(), y.get(id).unwrap());
            assert_eq!(px.keys.len(), py.keys.len(), "{w} {id} keys");
            let pairs = std::iter::once((&px.value, &py.value)).chain(px.keys.iter().map(|k| &k.value).zip(py.keys.iter().map(|k| &k.value)));
            for (vx, vy) in pairs {
                if id == "path" {
                    same_path(path_of(vx), path_of(vy), w);
                } else {
                    assert!(vx.components().iter().zip(vy.components()).all(|(p, q)| (p - q).abs() < 1e-9), "{w} {id} {vx:?} {vy:?}");
                }
            }
            for (kx, ky) in px.keys.iter().zip(&py.keys) {
                assert_eq!((kx.time, kx.in_interp, kx.out_interp, kx.label), (ky.time, ky.in_interp, ky.out_interp, ky.label), "{w} {id}");
                if id == "feather" {
                    let near = |e: &[Ease], f: &[Ease]| {
                        e.len() == f.len() && e.iter().zip(f).all(|(a, b)| (a.speed - b.speed).abs() < 1e-9 && (a.influence - b.influence).abs() < 1e-9)
                    };
                    // The first key's in ease and the last key's out ease are not carried.
                    if kx.in_interp == Interp::Bezier && !kx.in_ease.is_empty() && !ky.in_ease.is_empty() {
                        assert!(near(&kx.in_ease, &ky.in_ease), "{w} in ease {:?} {:?}", kx.in_ease, ky.in_ease);
                    }
                    if kx.out_interp == Interp::Bezier && !kx.out_ease.is_empty() && !ky.out_ease.is_empty() {
                        assert!(near(&kx.out_ease, &ky.out_ease), "{w} out ease {:?} {:?}", kx.out_ease, ky.out_ease);
                    }
                }
            }
        }
    }
}

#[test]
fn every_written_field_parses_back() {
    let p = all_modes();
    let res = export_all(&p);
    assert!(res.report.is_lossless(), "{:?}", res.report.losses);
    assert_eq!(res.report.exported.masks, 7);
    // The container is consistent: it writes back byte for byte.
    let file = parse(&res.bytes).expect("parse");
    assert_eq!(write(&file).expect("write"), res.bytes);
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let comp = view.all_items().into_iter().find_map(|i| i.comp()).expect("comp");
    let layer = comp.layers().next().expect("layer");
    let masks = layer.masks();
    let names: Vec<_> = masks.iter().map(|m| m.name.clone().unwrap_or_default()).collect();
    assert_eq!(names, ["Rect", "Ellipse", "Open", "Hex", "Lighten", "Darken", "None"]);
    let modes: Vec<_> = masks.iter().map(|m| m.mode).collect();
    assert_eq!(modes, [AepMode::Add, AepMode::Subtract, AepMode::Intersect, AepMode::Difference, AepMode::Lighten, AepMode::Darken, AepMode::None].map(Some));
    assert_eq!(masks.iter().map(|m| m.inverted).collect::<Vec<_>>(), [false, true, false, false, false, false, false].map(Some));
    // The running counter of the layer: 1 to n.
    assert_eq!(masks.iter().map(|m| m.counter).collect::<Vec<_>>(), (1..=7).map(Some).collect::<Vec<_>>());
    // Feather, opacity, expansion: stored only when not at the default.
    let e = &masks[1];
    assert_eq!((e.feather.value, e.feather.source), ([20.0, 10.0], ValueSource::Stored));
    assert_eq!((e.opacity_percent.value, e.opacity_percent.source), (50.0, ValueSource::Stored));
    assert_eq!((e.expansion.value, e.expansion.source), (5.0, ValueSource::Stored));
    assert_eq!(masks[3].expansion.value, -12.5);
    for m in [&masks[0], &masks[2], &masks[4]] {
        assert_eq!((m.feather.source, m.opacity_percent.source, m.expansion.source), (ValueSource::Default, ValueSource::Default, ValueSource::Default));
    }
    // Paths: vertices in layer pixels, open bit.
    let px = view.mask_vertices_px(&layer, &masks[2]).expect("vertices");
    let open = open_path();
    assert!(masks[2].shapes[0].open && !masks[0].shapes[0].open);
    for (v, (want, (ti, to))) in px[0].iter().zip(open.vertices.iter().zip(open.in_tangents.iter().zip(&open.out_tangents))) {
        let near = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 0.01 && (a[1] - b[1]).abs() < 0.01;
        assert!(near(v.position, *want) && near(v.in_tangent(), *ti) && near(v.out_tangent(), *to), "{v:?}");
    }
    // The point list header: count, vertex capacity and four times it (as 090 and 233).
    let atoms = layer.mask_atoms();
    let hex = &atoms[3].mask.shapes[0];
    assert_eq!((hex.vertex_count(), hex.points.len()), (6, 18));
    // The whole mask set imports back as it was exported.
    same_masks(&masks_of(&p), &masks_of(&reimport(&res.bytes)));
}

#[test]
fn path_and_feather_keys_parse_back() {
    let p = animated();
    let res = export_all(&p);
    assert!(res.report.is_lossless(), "{:?}", res.report.losses);
    let file = parse(&res.bytes).expect("parse");
    assert_eq!(write(&file).expect("write"), res.bytes);
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let comp = view.all_items().into_iter().find_map(|i| i.comp()).expect("comp");
    let layer = comp.layers().next().expect("layer");
    let atoms = layer.mask_atoms();
    let m = &atoms[0];
    assert!(m.mask.path_animated);
    assert_eq!(m.mask.path_key_seconds, [0.0, 1.0, 2.0]);
    assert_eq!(m.path_shapes.len(), 3);
    let path = m.path.expect("path property");
    let keys: Vec<_> = path.keys().expect("keys").keyframes().collect();
    assert!(keys.iter().all(|k| k.kind() == KeyframeKind::MaskPath));
    assert_eq!((keys[1].interpolation_in(), keys[1].interpolation_out(), keys[1].label()), (Some(Interpolation::Linear), Some(Interpolation::Hold), Some(3)));
    let Some(PropNode::Property(feather)) = m.atom.get("ADBE Mask Feather") else { panic!("feather") };
    let fk: Vec<_> = feather.keys().expect("feather keys").keyframes().collect();
    assert_eq!(fk.len(), 2);
    assert_eq!(fk[1].value(), Some(vec![50.0, 30.0]));
    let e = fk[0].ease_components().expect("ease");
    assert_eq!((e[0].out_speed, e[0].out_influence, e[1].out_speed, e[1].out_influence), (15.0, 0.66, 5.0, 0.5));
    same_masks(&masks_of(&p), &masks_of(&reimport(&res.bytes)));
}

#[test]
fn a_mask_on_a_small_solid_is_normalised_to_the_layer_size() {
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 1280, 720, FrameRate::new(25, 1), 2.0, None);
    let s = add_solid(&mut p, "L", 400, 300, [0.0, 1.0, 0.0], None);
    add_layer(&mut p, c, "L", LayerSource::Solid { item: s }, (400, 300));
    let mut next = p.next_id;
    with_layer(&mut p, c, 0, |l| add_mask(l, &mut next, "Mask 1", ShapePath::rect([200.0, 150.0], 200.0, 100.0), MaskMode::Add, |_| {}));
    p.next_id = next;
    let res = export_all(&p);
    let file = parse(&res.bytes).expect("parse");
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let comp = view.all_items().into_iter().find_map(|i| i.comp()).expect("comp");
    let layer = comp.layers().next().expect("layer");
    let m = &layer.masks()[0];
    // The box is a fraction of the 400x300 solid: 100..300 / 400, 100..200 / 300.
    let b = m.shapes[0].bbox.map(f64::from);
    for (got, want) in b.iter().zip([0.25, 1.0 / 3.0, 0.75, 2.0 / 3.0]) {
        assert!((got - want).abs() < 1e-6, "{b:?}");
    }
    same_masks(&masks_of(&p), &masks_of(&reimport(&res.bytes)));
}

#[test]
fn what_cannot_be_written_is_reported() {
    let p = project(|l, n| {
        add_mask(l, n, "Off", ShapePath::rect([960.0, 540.0], 100.0, 100.0), MaskMode::Add, |g| g.enabled = false);
        add_mask(l, n, "Empty", ShapePath::default(), MaskMode::Add, |_| {});
        add_mask(l, n, "Keyed opacity", ShapePath::rect([960.0, 540.0], 100.0, 100.0), MaskMode::Add, |g| {
            g.get_mut("opacity").unwrap().keys = vec![key(0.0, Value::Scalar(10.0)), key(1.0, Value::Scalar(90.0))];
            g.get_mut("expansion").unwrap().expr = Some(Expression { text: "time".into(), enabled: true });
            if let GroupKind::Mask { locked, feather_falloff, roto_bezier, .. } = &mut g.kind {
                *locked = true;
                *feather_falloff = FeatherFalloff::Linear;
                *roto_bezier = true;
            }
        });
        add_mask(l, n, "Bezier path", ShapePath::rect([960.0, 540.0], 100.0, 100.0), MaskMode::Add, |g| {
            let mut a = key(0.0, Value::Path(ShapePath::rect([960.0, 540.0], 100.0, 100.0)));
            a.out_interp = Interp::Bezier;
            g.get_mut("path").unwrap().keys = vec![a, key(1.0, Value::Path(ShapePath::rect([960.0, 540.0], 200.0, 100.0)))];
        });
        add_mask(l, n, "NaN", ShapePath::polygon(&[[f64::NAN, 0.0], [10.0, 10.0]], true), MaskMode::Add, |g| {
            set(g, "feather", Value::Vec2([f64::INFINITY, -1.0]));
            set(g, "opacity", Value::Scalar(400.0));
        });
    });
    let err = export(&p, &ExportOptions { allow_losses: false }).unwrap_err();
    assert!(err.to_string().contains("masks"), "{err}");
    let res = export_all(&p);
    let r = &res.report;
    // Off, Empty and NaN are not written; Keyed opacity (5 losses: keys, expression, lock,
    // falloff, RotoBezier) and Bezier path (its ease is written, 599 / 700) are.
    assert_eq!(r.exported.masks, 2, "{:?}", r.losses);
    assert_eq!(r.count(LossCategory::Masks), 3 + 5, "{:?}", r.losses);
    assert_eq!(r.count(LossCategory::ApproximatedKeyframes), 0, "{:?}", r.losses);
    let back = reimport(&res.bytes);
    let m = masks_of(&back);
    assert_eq!(m.len(), 2);
    assert_eq!(m[1].get("path").unwrap().keys[0].out_interp, Interp::Bezier);
}

#[test]
fn hostile_masks_never_panic() {
    let huge = ShapePath::polygon(&vec![[1.0, 2.0]; 70_000], true);
    let line = ShapePath::polygon(&[[5.0, 5.0], [5.0, 900.0], [5.0, 30.0]], false);
    let point = ShapePath::polygon(&[[5.0, 5.0]], true);
    let lopsided = ShapePath { vertices: vec![[0.0, 0.0], [10.0, 0.0]], in_tangents: vec![], out_tangents: vec![[1.0, 1.0]], closed: true, feather: vec![] };
    let p = project(|l, n| {
        for (i, path) in [huge, line, point, lopsided, ShapePath::polygon(&[[1e300, 0.0], [0.0, 0.0]], true)].into_iter().enumerate() {
            add_mask(l, n, &format!("M{i}"), path, MaskMode::Add, |_| {});
        }
        add_mask(l, n, &"x".repeat(1000), ShapePath::rect([1.0, 1.0], 1.0, 1.0), MaskMode::Add, |_| {});
    });
    let res = export_all(&p);
    let file = parse(&res.bytes).expect("parse");
    assert_eq!(write(&file).expect("write"), res.bytes);
    // The line (zero width box), the point (zero box), the lopsided path and the long name are
    // written; the huge and the out-of-range paths are not.
    assert_eq!(res.report.exported.masks, 4, "{:?}", res.report.losses);
    let back = masks_of(&reimport(&res.bytes));
    assert_eq!(back.len(), 4);
    let line = path_of(&back[0].get("path").unwrap().value);
    assert!(line.vertices.iter().all(|v| (v[0] - 5.0).abs() < 1e-3));
    assert!((line.vertices[1][1] - 900.0).abs() < 1e-2);
    assert!(back[3].name.len() <= 255);
}

#[test]
fn mask_fixtures_round_trip() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures");
    for name in [
        "090-mask-rect.aep",
        "091-mask-rect-props.aep",
        "093-mask-path-2keys.aep",
        "208-key-mask-feather-2key-linear.aep",
        "232-mask-ellipse.aep",
        "233-mask-open-path.aep",
        "234-mask-mode-intersect.aep",
        "235-mask-mode-lighten.aep",
        "236-mask-mode-darken.aep",
        "237-mask-mode-difference.aep",
        "306-key-maskfeather-bezier-ease.aep",
        "309-solid-100x100-mask-rect.aep",
        "328-mask-two-ellipse-rect.aep",
        "329-mask-ellipse-mode-none.aep",
    ] {
        let a = reimport(&std::fs::read(dir.join(name)).expect("fixture"));
        let res = export_all(&a);
        assert_eq!(res.report.count(LossCategory::Masks), 0, "{name}: {:?}", res.report.losses);
        let b = reimport(&res.bytes);
        // 309 has its masked layer on a 100x100 solid in a second composition layer list; compare
        // every layer with masks.
        let all = |p: &Project| -> Vec<Vec<PropGroup>> {
            let mut v: Vec<(String, Vec<PropGroup>)> = Vec::new();
            for item in p.items.values() {
                if let effectcraft_project::ItemKind::Comp(c) = &item.kind {
                    for l in &c.layers {
                        if let Some(m) = l.masks().filter(|m| !m.children.is_empty()) {
                            v.push((format!("{}/{}", item.name, l.name), m.groups().cloned().collect()));
                        }
                    }
                }
            }
            v.sort_by(|x, y| x.0.cmp(&y.0));
            v.into_iter().map(|(_, m)| m).collect()
        };
        let (ma, mb) = (all(&a), all(&b));
        assert_eq!(ma.len(), mb.len(), "{name}");
        assert!(!ma.is_empty(), "{name}");
        for (x, y) in ma.iter().zip(&mb) {
            same_masks(x, y);
        }
    }
}
