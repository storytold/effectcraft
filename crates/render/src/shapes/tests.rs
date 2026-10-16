//! Render-level tests for shape-layer path operators and paint items.

use effectcraft_color::Label;
use effectcraft_keyframe::{Gradient, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Comp, ItemId, ItemKind, LayerSource, Project, PropGroup};
use effectcraft_raster::Image;
use effectcraft_time::{FrameRate, Tick};

use crate::render_frame;

/// A 200×100 comp with one shape layer (origin at the comp centre, 100,50) holding `items` at the
/// top level of its contents.
fn shape_comp(items: impl FnOnce(&mut Ids) -> Vec<PropGroup>) -> (Project, ItemId) {
    shape_comp_sized(200, 100, items)
}

fn shape_comp_sized(w: u32, h: u32, items: impl FnOnce(&mut Ids) -> Vec<PropGroup>) -> (Project, ItemId) {
    let mut p = Project::default();
    let comp = Comp::new(w, h, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let mut l = build::layer(&mut p, &comp, "Shape Layer 1", LayerSource::Shape, (w, h), None);
    let mut next = p.next_id;
    let items = items(&mut Ids(&mut next));
    p.next_id = next;
    let contents = l.props.sub_mut("contents").unwrap();
    for i in items {
        contents.children.push(i.into());
    }
    p.comp_mut(cid).unwrap().layers.push(l);
    (p, cid)
}

fn frame(p: &Project, cid: ItemId) -> Image {
    render_frame(p, cid, Tick::ZERO, 1.0)
}

fn alpha(img: &Image, x: i64, y: i64) -> f32 {
    img.get(x, y)[3]
}

fn coverage(img: &Image) -> f32 {
    img.data.iter().map(|px| px[3]).sum()
}

fn set(g: &mut PropGroup, path: &str, v: Value) {
    g.prop_mut(path).unwrap_or_else(|| panic!("no {path}")).value = v;
}

fn op(ids: &mut Ids, kind: &str, params: &[(&str, Value)]) -> PropGroup {
    let mut g = build::shape_simple_op(ids, kind).unwrap();
    for (k, v) in params {
        set(&mut g, k, v.clone());
    }
    g
}

const RED: [f64; 4] = [1.0, 0.0, 0.0, 1.0];
const GREEN: [f64; 4] = [0.0, 1.0, 0.0, 1.0];

#[test]
fn merge_add_feeds_fill_below_and_drops_paint_above() {
    let (p, cid) = shape_comp(|ids| {
        vec![
            build::shape_rect(ids, [40.0, 40.0], [-20.0, 0.0], 0.0),
            build::shape_rect(ids, [40.0, 40.0], [20.0, 0.0], 0.0),
            build::shape_fill(ids, RED),
            op(ids, "merge", &[("mode", Value::Enum(1))]),
            build::shape_fill(ids, GREEN),
        ]
    });
    let img = frame(&p, cid);
    let c = img.get(100, 50);
    assert!(c[1] > 0.99 && c[0] < 0.01, "merged path painted by the fill below only: {c:?}");
    assert!(img.get(65, 50)[1] > 0.99 && img.get(135, 50)[1] > 0.99);
    assert!((coverage(&img) - 80.0 * 40.0).abs() < 20.0, "{}", coverage(&img));
}

#[test]
fn merge_subtract_and_intersect() {
    let sub = |mode: u32| {
        let (p, cid) = shape_comp(|ids| {
            vec![
                build::shape_rect(ids, [80.0, 60.0], [0.0, 0.0], 0.0),
                build::shape_rect(ids, [20.0, 20.0], [0.0, 0.0], 0.0),
                op(ids, "merge", &[("mode", Value::Enum(mode))]),
                build::shape_fill(ids, RED),
            ]
        });
        frame(&p, cid)
    };
    let s = sub(2);
    assert!(alpha(&s, 100, 50) < 0.01, "hole");
    assert!(alpha(&s, 70, 50) > 0.99);
    assert!((coverage(&s) - (80.0 * 60.0 - 400.0)).abs() < 20.0);
    let i = sub(3);
    assert!(alpha(&i, 100, 50) > 0.99);
    assert!(alpha(&i, 70, 50) < 0.01);
    assert!((coverage(&i) - 400.0).abs() < 10.0);
    let x = sub(4);
    assert!((coverage(&x) - (80.0 * 60.0 - 400.0)).abs() < 20.0);
}

#[test]
fn offset_paths_grow_and_shrink_fill() {
    let run = |amount: f64| {
        let (p, cid) = shape_comp(|ids| {
            vec![build::shape_rect(ids, [40.0, 40.0], [0.0, 0.0], 0.0), op(ids, "offset", &[("amount", Value::Scalar(amount))]), build::shape_fill(ids, RED)]
        });
        frame(&p, cid)
    };
    let g = run(10.0);
    assert!((coverage(&g) - 60.0 * 60.0).abs() < 20.0, "{}", coverage(&g));
    assert!(alpha(&g, 100 + 25, 50) > 0.99);
    let s = run(-10.0);
    assert!((coverage(&s) - 20.0 * 20.0).abs() < 10.0, "{}", coverage(&s));
}

#[test]
fn operators_below_repeater_act_on_copies() {
    let (p, cid) = shape_comp(|ids| {
        let mut rep = build::shape_repeater(ids, 3.0, [30.0, 0.0]);
        set(&mut rep, "transform/position", Value::Vec2([30.0, 0.0]));
        vec![build::shape_rect(ids, [20.0, 20.0], [-30.0, 0.0], 0.0), rep, op(ids, "offset", &[("amount", Value::Scalar(2.0))]), build::shape_fill(ids, RED)]
    });
    let img = frame(&p, cid);
    // Three 24×24 squares (offset by 2) at x = -30, 0, 30.
    for cx in [70, 100, 130] {
        assert!(alpha(&img, cx, 50) > 0.99, "copy at {cx}");
        assert!(alpha(&img, cx + 11, 50) > 0.99, "offset applied to copy at {cx}");
    }
    assert!((coverage(&img) - 3.0 * 24.0 * 24.0).abs() < 30.0, "{}", coverage(&img));
}

#[test]
fn repeater_then_merge_unites_copies() {
    let (p, cid) = shape_comp(|ids| {
        vec![
            build::shape_rect(ids, [20.0, 20.0], [-10.0, 0.0], 0.0),
            build::shape_repeater(ids, 2.0, [10.0, 0.0]),
            op(ids, "merge", &[("mode", Value::Enum(4))]),
            build::shape_fill(ids, RED),
        ]
    });
    let img = frame(&p, cid);
    // Two squares overlapping by 10 px: exclude leaves the two 10×20 ends.
    assert!(alpha(&img, 95, 50) < 0.01);
    assert!(alpha(&img, 85, 50) > 0.99 && alpha(&img, 105, 50) > 0.99);
    assert!((coverage(&img) - 400.0).abs() < 10.0, "{}", coverage(&img));
}

#[test]
fn trim_below_stroke_trims_it() {
    let run = |end: f64| {
        let (p, cid) = shape_comp(|ids| {
            let mut t = build::shape_trim(ids, 0.0, end, 0.0);
            set(&mut t, "end", Value::Scalar(end));
            vec![build::shape_rect(ids, [60.0, 60.0], [0.0, 0.0], 0.0), build::shape_stroke(ids, [1.0; 4], 4.0), t]
        });
        coverage(&frame(&p, cid))
    };
    let full = run(100.0);
    let half = run(50.0);
    assert!(full > 900.0);
    assert!((half / full - 0.5).abs() < 0.05, "{half} / {full}");
}

#[test]
fn trim_individually_vs_simultaneously() {
    let run = |mode: u32| {
        let (p, cid) = shape_comp(|ids| {
            let items = vec![build::shape_rect(ids, [40.0, 40.0], [-40.0, 0.0], 0.0), build::shape_stroke(ids, [1.0; 4], 2.0)];
            let a = build::shape_group(ids, "A", items);
            let items = vec![build::shape_rect(ids, [40.0, 40.0], [40.0, 0.0], 0.0), build::shape_stroke(ids, [1.0; 4], 2.0)];
            let b = build::shape_group(ids, "B", items);
            let mut t = build::shape_trim(ids, 0.0, 50.0, 0.0);
            set(&mut t, "mode", Value::Enum(mode));
            vec![a, b, t]
        });
        let img = frame(&p, cid);
        let left: f32 = (0..100).flat_map(|x| (0..100).map(move |y| (x, y))).map(|(x, y)| alpha(&img, x, y)).sum();
        let right: f32 = (100..200).flat_map(|x| (0..100).map(move |y| (x, y))).map(|(x, y)| alpha(&img, x, y)).sum();
        (left, right)
    };
    let (l, r) = run(0);
    assert!((l - r).abs() < 0.1 * l, "simultaneously: both half drawn {l} {r}");
    let (l, r) = run(1);
    assert!(l > 250.0 && r < 1.0, "individually: first path complete, second not started {l} {r}");
}

#[test]
fn gradient_stroke_paints_gradient_along_stroke() {
    let (p, cid) = shape_comp(|ids| {
        let g = Gradient { colors: vec![(0.0, [0.0, 0.0, 0.0, 1.0]), (1.0, [1.0, 1.0, 1.0, 1.0])], opacities: vec![(0.0, 1.0), (1.0, 1.0)] };
        vec![build::shape_rect(ids, [60.0, 60.0], [0.0, 0.0], 0.0), build::shape_gradient_stroke(ids, false, [-30.0, 0.0], [30.0, 0.0], g, 6.0)]
    });
    let img = frame(&p, cid);
    let l = img.get(70, 50);
    let r = img.get(130, 50);
    assert!(l[3] > 0.99 && r[3] > 0.99, "{l:?} {r:?}");
    assert!(l[0] < 0.1 && r[0] > 0.9, "{l:?} {r:?}");
    assert!(alpha(&img, 100, 50) < 0.01, "stroke only");
}

#[test]
fn fill_blend_mode_multiply() {
    let (p, cid) = shape_comp(|ids| {
        let mut top = build::shape_fill(ids, [0.5, 0.5, 0.5, 1.0]);
        set(
            &mut top,
            "blend",
            Value::Enum(effectcraft_color::BlendMode::ALL.iter().position(|m| *m == effectcraft_color::BlendMode::Multiply).unwrap() as u32),
        );
        vec![build::shape_rect(ids, [40.0, 40.0], [0.0, 0.0], 0.0), top, build::shape_fill(ids, [1.0, 0.0, 0.0, 1.0])]
    });
    let c = frame(&p, cid).get(100, 50);
    assert!((c[0] - 0.5).abs() < 0.01 && c[1] < 0.01, "{c:?}");
}

#[test]
fn reversed_path_direction_cuts_hole_under_non_zero() {
    let (p, cid) = shape_comp(|ids| {
        let mut inner = build::shape_rect(ids, [20.0, 20.0], [0.0, 0.0], 0.0);
        set(&mut inner, "direction", Value::Enum(1));
        vec![build::shape_rect(ids, [60.0, 60.0], [0.0, 0.0], 0.0), inner, op(ids, "merge", &[]), build::shape_fill(ids, RED)]
    });
    let img = frame(&p, cid);
    assert!(alpha(&img, 100, 50) < 0.01);
    assert!(alpha(&img, 75, 50) > 0.99);
}

#[test]
fn round_corners_clip_corner_pixels() {
    let (p, cid) = shape_comp(|ids| {
        vec![build::shape_rect(ids, [40.0, 40.0], [0.0, 0.0], 0.0), op(ids, "round", &[("radius", Value::Scalar(20.0))]), build::shape_fill(ids, RED)]
    });
    let img = frame(&p, cid);
    assert!(alpha(&img, 81, 31) < 0.01, "corner rounded off");
    assert!(alpha(&img, 100, 50) > 0.99);
    assert!((coverage(&img) - std::f32::consts::PI * 400.0).abs() < 15.0, "{}", coverage(&img));
}

#[test]
fn wiggle_paths_vary_over_time_deterministically() {
    let (p, cid) = shape_comp(|ids| {
        vec![build::shape_ellipse(ids, [60.0, 60.0], [0.0, 0.0]), op(ids, "wiggle", &[("size", Value::Scalar(8.0))]), build::shape_fill(ids, RED)]
    });
    let a = render_frame(&p, cid, Tick::from_seconds_f64(0.5), 1.0);
    let b = render_frame(&p, cid, Tick::from_seconds_f64(0.5), 1.0);
    let c = render_frame(&p, cid, Tick::from_seconds_f64(1.0), 1.0);
    assert_eq!(a.data, b.data);
    assert_ne!(a.data, c.data);
}

#[test]
fn zigzag_and_twist_render() {
    let (p, cid) = shape_comp(|ids| {
        vec![
            build::shape_rect(ids, [60.0, 60.0], [0.0, 0.0], 0.0),
            op(ids, "zigzag", &[("size", Value::Scalar(5.0)), ("ridges", Value::Scalar(4.0))]),
            op(ids, "twist", &[("angle", Value::Scalar(45.0))]),
            build::shape_fill(ids, RED),
        ]
    });
    let img = frame(&p, cid);
    assert!(alpha(&img, 100, 50) > 0.99);
    let cov = coverage(&img);
    assert!(cov > 2000.0 && cov < 4600.0, "{cov}");
}

#[test]
fn repeater_draws_match_per_copy_bounds() {
    // Many small copies spread over a large buffer: each must rasterise correctly in its own
    // sub-rectangle.
    let (p, cid) = shape_comp_sized(400, 400, |ids| {
        let mut rep = build::shape_repeater(ids, 12.0, [0.0, 0.0]);
        set(&mut rep, "transform/rotation", Value::Scalar(30.0));
        let items = vec![build::shape_rect(ids, [10.0, 30.0], [0.0, -150.0], 3.0), build::shape_fill(ids, RED)];
        let g = build::shape_group(ids, "Tick", items);
        vec![g, rep]
    });
    let img = frame(&p, cid);
    for k in 0..12 {
        let a = (k as f64 * 30.0).to_radians();
        let (x, y) = (200.0 + 150.0 * a.sin(), 200.0 - 150.0 * a.cos());
        assert!(alpha(&img, x.round() as i64, y.round() as i64) > 0.99, "copy {k}");
    }
    assert!(alpha(&img, 200, 200) < 0.01);
    let expect = 12.0 * (300.0 - (4.0 - std::f32::consts::PI) * 9.0);
    assert!((coverage(&img) - expect).abs() < 40.0, "{}", coverage(&img));
}

/// Performance probe: 60-copy repeater of a small rounded rect at 1920×1080.
/// Run with `cargo test -p effectcraft-render --release -- --ignored --nocapture shape_perf`.
#[test]
#[ignore]
fn shape_perf_repeater_60_copies_1080p() {
    let (p, cid) = shape_comp_sized(1920, 1080, |ids| {
        let mut rep = build::shape_repeater(ids, 60.0, [0.0, 0.0]);
        set(&mut rep, "transform/rotation", Value::Scalar(6.0));
        let items = vec![
            build::shape_rect(ids, [12.0, 60.0], [0.0, -420.0], 4.0),
            build::shape_fill(ids, [1.0, 0.8, 0.2, 1.0]),
            build::shape_stroke(ids, [1.0; 4], 2.0),
        ];
        let g = build::shape_group(ids, "Tick", items);
        vec![g, rep]
    });
    let _ = render_frame(&p, cid, Tick::ZERO, 1.0);
    let n = 10;
    let t0 = std::time::Instant::now();
    for _ in 0..n {
        let _ = render_frame(&p, cid, Tick::ZERO, 1.0);
    }
    let ms = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
    println!("shape_perf: 60-copy repeater at 1920x1080: {ms:.2} ms/frame");
    let ItemKind::Comp(comp) = &p.item(cid).unwrap().kind else { panic!("not a composition") };
    let layer = &comp.layers[0];
    let ctx = crate::eval::EvalCtx::new(&p, cid, comp, Tick::ZERO);
    let contents = layer.props.sub("contents").unwrap();
    let t0 = std::time::Instant::now();
    for _ in 0..n {
        let _ = super::render(&ctx, layer, contents, 1.0);
    }
    let ms = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
    println!("shape_perf: shapes::render alone: {ms:.2} ms");
    let mut arena = Vec::new();
    let t0 = std::time::Instant::now();
    for _ in 0..n {
        arena.clear();
        let _ = super::collect(&ctx, layer, contents, &mut arena);
    }
    let ms = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
    println!("shape_perf: collect alone: {ms:.2} ms");
    let (empty, ecid) = shape_comp_sized(1920, 1080, |_| vec![]);
    let t0 = std::time::Instant::now();
    for _ in 0..n {
        let _ = render_frame(&empty, ecid, Tick::ZERO, 1.0);
    }
    let ms = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
    println!("shape_perf: empty shape layer comp (compositor baseline): {ms:.2} ms");
}

fn line_path(ids: &mut Ids, from: [f64; 2], to: [f64; 2]) -> PropGroup {
    let sp = effectcraft_keyframe::ShapePath {
        vertices: vec![from, to],
        in_tangents: vec![[0.0; 2]; 2],
        out_tangents: vec![[0.0; 2]; 2],
        closed: false,
        feather: Vec::new(),
    };
    build::shape_path(ids, sp)
}

fn column_alpha(img: &Image, x: i64) -> f32 {
    (0..img.height as i64).map(|y| alpha(img, x, y)).sum()
}

#[test]
fn stroke_taper_narrows_the_ends() {
    // A horizontal line from x = 20 to 180 (layer −80…80), 20 px wide, tapered over 40 px at both ends.
    let (p, cid) = shape_comp(|ids| {
        let mut st = build::shape_stroke(ids, RED, 20.0);
        set(&mut st, "taper/startLength", Value::Scalar(40.0));
        set(&mut st, "taper/endLength", Value::Scalar(40.0));
        set(&mut st, "taper/startWidth", Value::Scalar(0.0));
        set(&mut st, "taper/endWidth", Value::Scalar(50.0));
        vec![line_path(ids, [-80.0, 0.0], [80.0, 0.0]), st]
    });
    let img = frame(&p, cid);
    let near_start = column_alpha(&img, 30); // 10 px along → 25 % of 20 px
    let mid = column_alpha(&img, 100);
    let near_end = column_alpha(&img, 169); // 10 px before the end → (50 % + 50 % × 25 %) of 20 px
    assert!((near_start - 5.0).abs() < 1.0, "{near_start}");
    assert!((mid - 20.0).abs() < 0.6, "{mid}");
    assert!((near_end - 12.5).abs() < 1.0, "{near_end}");
}

#[test]
fn stroke_wave_and_multi_dashes() {
    let (p, cid) = shape_comp(|ids| {
        let mut st = build::shape_stroke(ids, RED, 10.0);
        set(&mut st, "wave/amount", Value::Scalar(100.0));
        set(&mut st, "wave/wavelength", Value::Scalar(40.0));
        vec![line_path(ids, [-80.0, 0.0], [80.0, 0.0]), st]
    });
    let img = frame(&p, cid);
    assert!(column_alpha(&img, 40) < 0.5, "trough (20 px along): {}", column_alpha(&img, 40));
    assert!(column_alpha(&img, 60) > 9.0, "crest (40 px along): {}", column_alpha(&img, 60));

    // Dash 20, Gap 10, Dash 2 = 5, Gap 2 = 15: on 0–20, 30–35, 50–70…
    let (p, cid) = shape_comp(|ids| {
        let mut st = build::shape_stroke(ids, RED, 10.0);
        set(&mut st, "dashes/dash", Value::Scalar(20.0));
        set(&mut st, "dashes/gap", Value::Scalar(10.0));
        let d = st.sub_mut("dashes").unwrap();
        d.children.insert(2, ids.prop("gap2", "Gap 2", Value::Scalar(15.0)).into());
        d.children.insert(2, ids.prop("dash2", "Dash 2", Value::Scalar(5.0)).into());
        vec![line_path(ids, [-80.0, 0.0], [80.0, 0.0]), st]
    });
    let img = frame(&p, cid);
    let on = |s: i64| column_alpha(&img, 20 + s) > 9.0;
    assert!(on(10) && !on(25) && on(32) && !on(40) && on(60), "{:?}", (0..80).map(|s| on(s) as u8).collect::<Vec<_>>());
}

#[test]
fn radial_gradient_highlight_moves_the_focal_point() {
    let build_with = |hl: f64| {
        shape_comp_sized(200, 200, move |ids| {
            let mut gf = build::shape_gradient_fill(ids, true, [0.0, 0.0], [80.0, 0.0], Gradient::default());
            set(&mut gf, "highlightLength", Value::Scalar(hl));
            set(&mut gf, "highlightAngle", Value::Scalar(90.0));
            vec![build::shape_rect(ids, [200.0, 200.0], [0.0, 0.0], 0.0), gf]
        })
    };
    let (p0, c0) = build_with(0.0);
    let (p1, c1) = build_with(50.0);
    let (a, b) = (frame(&p0, c0), frame(&p1, c1));
    // Without a highlight the centre is white; a 50 % highlight at 90° (down the +y axis, 90° from
    // the start→end axis) puts the white focus 40 px below the centre and darkens the centre.
    assert!(a.get(100, 100)[0] > 0.98);
    assert!(b.get(100, 140)[0] > 0.97, "focal point: {:?}", b.get(100, 140));
    assert!(b.get(100, 100)[0] < 0.8, "{:?}", b.get(100, 100));
    // The outer circle is unchanged: points just outside it are black in both.
    assert!(a.get(100, 182)[0] < 0.03 && b.get(100, 182)[0] < 0.03 && b.get(100, 17)[0] < 0.03);
}

/// The layer-space buffer `super::render` draws the comp's only shape layer into.
fn shape_buf(p: &Project, cid: ItemId) -> effectcraft_effects::Buf {
    let comp = p.comp(cid).unwrap();
    let ctx = crate::eval::EvalCtx::new(p, cid, comp, Tick::ZERO);
    let layer = &comp.layers[0];
    super::render(&ctx, layer, layer.props.sub("contents").unwrap(), 1.0)
}

#[test]
fn straight_stroke_buffer_is_not_padded_by_the_miter_limit() {
    // A 1080 px wide vertical wipe (a track matte): no joins, so the buffer is the line plus half
    // the width, not 4× (Miter Limit 4) the width on every side (4326×6366, ~440 MB per frame).
    let (p, cid) = shape_comp_sized(1080, 1920, |ids| vec![line_path(ids, [0.0, 1020.0], [0.0, -1020.0]), build::shape_stroke(ids, RED, 1080.0)]);
    let b = shape_buf(&p, cid);
    assert!(b.img.width <= 1090 && b.img.height <= 3130, "{}×{}", b.img.width, b.img.height);
}

#[test]
fn miter_corner_still_fits_its_buffer() {
    // A sharp V with a 20 px miter-joined stroke: the miter tip reaches well past half the width.
    let (p, cid) = shape_comp(|ids| {
        let sp = effectcraft_keyframe::ShapePath {
            vertices: vec![[-40.0, -30.0], [0.0, 20.0], [40.0, -30.0]],
            in_tangents: vec![[0.0; 2]; 3],
            out_tangents: vec![[0.0; 2]; 3],
            closed: false,
            feather: Vec::new(),
        };
        vec![build::shape_path(ids, sp), build::shape_stroke(ids, RED, 20.0)]
    });
    // The miter tip of the V points down from (100, 70): its pixels are drawn, not clipped.
    let img = frame(&p, cid);
    assert!(alpha(&img, 100, 80) > 0.9, "{}", alpha(&img, 100, 80));
    let b = shape_buf(&p, cid);
    assert!(b.img.height as f64 >= 50.0 + 20.0 * 4.0 * 0.5, "{}", b.img.height);
}
