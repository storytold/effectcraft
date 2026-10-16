use effectcraft_color::{BlendMode, Label};
use effectcraft_keyframe::{Keyframe, ShapePath, TextDoc, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Comp, ItemId, ItemKind, LayerSource, MaskMode, MatteKind, Project, Solid, TrackMatte};
use effectcraft_time::{FrameRate, Tick};

use crate::render_frame;

fn setup() -> (Project, ItemId, Comp) {
    let mut p = Project::default();
    // Exact float maths; 8/16 bpc quantisation has its own tests (tests_color).
    p.settings.bit_depth = effectcraft_project::BitDepth::Bpc32;
    let comp = Comp::new(200, 100, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    (p, cid, comp)
}

fn solid(p: &mut Project, comp: &Comp, color: [f32; 3], w: u32, h: u32) -> effectcraft_project::Layer {
    let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color, width: w, height: h, pixel_aspect: 1.0 }));
    build::layer(p, comp, "Solid", LayerSource::Solid { item: sid }, (w, h), None)
}

#[test]
fn solid_fills_comp() {
    let (mut p, cid, comp) = setup();
    let l = solid(&mut p, &comp, [1.0, 0.0, 0.0], 200, 100);
    p.comp_mut(cid).unwrap().layers.push(l);
    let img = render_frame(&p, cid, Tick::ZERO, 1.0);
    assert_eq!(img.get(10, 10), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(img.get(199, 99), [1.0, 0.0, 0.0, 1.0]);
}

#[test]
fn position_keyframes_move_layer() {
    let (mut p, cid, comp) = setup();
    let mut l = solid(&mut p, &comp, [0.0, 1.0, 0.0], 20, 20);
    let pos = l.props.prop_mut("transform/position").unwrap();
    pos.keys = vec![Keyframe::new(Tick::ZERO, Value::Vec3([20.0, 50.0, 0.0])), Keyframe::new(Tick::from_seconds_f64(1.0), Value::Vec3([180.0, 50.0, 0.0]))];
    p.comp_mut(cid).unwrap().layers.push(l);
    let a = render_frame(&p, cid, Tick::ZERO, 1.0);
    assert!(a.get(20, 50)[3] > 0.99 && a.get(100, 50)[3] < 0.01);
    let b = render_frame(&p, cid, Tick::from_seconds_f64(0.5), 1.0);
    assert!(b.get(100, 50)[3] > 0.99 && b.get(20, 50)[3] < 0.01);
}

#[test]
fn opacity_and_blend_modes() {
    let (mut p, cid, comp) = setup();
    let bottom = solid(&mut p, &comp, [0.5, 0.5, 0.5], 200, 100);
    let mut top = solid(&mut p, &comp, [0.5, 0.5, 0.5], 200, 100);
    top.blend_mode = BlendMode::Multiply;
    let c = p.comp_mut(cid).unwrap();
    c.layers.push(top);
    c.layers.push(bottom);
    let img = render_frame(&p, cid, Tick::ZERO, 1.0);
    assert!((img.get(50, 50)[0] - 0.25).abs() < 1e-4);
}

#[test]
fn half_resolution() {
    let (mut p, cid, comp) = setup();
    let l = solid(&mut p, &comp, [1.0, 1.0, 1.0], 200, 100);
    p.comp_mut(cid).unwrap().layers.push(l);
    let img = render_frame(&p, cid, Tick::ZERO, 0.5);
    assert_eq!((img.width, img.height), (100, 50));
    assert!((img.get(50, 25)[3] - 1.0).abs() < 1e-4);
}

#[test]
fn mask_cuts_layer() {
    let (mut p, cid, comp) = setup();
    let mut l = solid(&mut p, &comp, [1.0, 1.0, 1.0], 200, 100);
    let mut next = p.next_id;
    let m = build::mask(&mut Ids(&mut next), "Mask 1", ShapePath::rect([50.0, 50.0], 40.0, 40.0), MaskMode::Add, [255, 255, 0]);
    p.next_id = next;
    l.props.sub_mut("masks").unwrap().children.push(m.into());
    p.comp_mut(cid).unwrap().layers.push(l);
    let img = render_frame(&p, cid, Tick::ZERO, 1.0);
    assert!(img.get(50, 50)[3] > 0.99);
    assert!(img.get(150, 50)[3] < 0.01);
}

#[test]
fn first_intersect_and_darken_keep_their_rectangle() {
    let render = |modes: &[MaskMode], centers: &[[f64; 2]]| {
        let (mut p, cid, comp) = setup();
        let mut l = solid(&mut p, &comp, [1.0, 1.0, 1.0], 200, 100);
        for (i, (mode, center)) in modes.iter().zip(centers).enumerate() {
            let mut next = p.next_id;
            let m = build::mask(&mut Ids(&mut next), &format!("Mask {}", i + 1), ShapePath::rect(*center, 40.0, 40.0), *mode, [255, 255, 0]);
            p.next_id = next;
            l.props.sub_mut("masks").unwrap().children.push(m.into());
        }
        p.comp_mut(cid).unwrap().layers.push(l);
        render_frame(&p, cid, Tick::ZERO, 1.0)
    };
    // A first Intersect or Darken has nothing to combine with, so it covers like Add.
    // Lighten and Difference already do; Subtract still starts from the whole layer.
    for mode in [MaskMode::Intersect, MaskMode::Darken, MaskMode::Add, MaskMode::Lighten, MaskMode::Difference] {
        let img = render(&[mode], &[[50.0, 50.0]]);
        assert!(img.get(50, 50)[3] > 0.99, "{mode:?} keeps the rectangle");
        assert!(img.get(150, 50)[3] < 0.01, "{mode:?} stays outside it");
    }
    let hole = render(&[MaskMode::Subtract], &[[50.0, 50.0]]);
    assert!(hole.get(50, 50)[3] < 0.01, "subtract still cuts its rectangle");
    assert!(hole.get(150, 50)[3] > 0.99, "subtract leaves the rest of the layer");

    // Add, then Intersect with a shifted rectangle: only the overlap remains.
    let overlap = render(&[MaskMode::Add, MaskMode::Intersect], &[[50.0, 50.0], [70.0, 50.0]]);
    assert!(overlap.get(50, 50)[3] > 0.99, "overlap stays");
    assert!(overlap.get(35, 50)[3] < 0.01, "the first rectangle's exclusive side is cut");
    assert!(overlap.get(85, 50)[3] < 0.01, "the second rectangle's exclusive side is cut");
    let darkened = render(&[MaskMode::Add, MaskMode::Darken], &[[50.0, 50.0], [70.0, 50.0]]);
    assert!(darkened.get(50, 50)[3] > 0.99 && darkened.get(35, 50)[3] < 0.01 && darkened.get(85, 50)[3] < 0.01);

    // A later disjoint Add still contributes after a first Intersect.
    let both = render(&[MaskMode::Intersect, MaskMode::Add], &[[50.0, 50.0], [150.0, 50.0]]);
    assert!(both.get(50, 50)[3] > 0.99 && both.get(150, 50)[3] > 0.99);
    assert!(both.get(100, 50)[3] < 0.01);
}

#[test]
fn track_matte_alpha() {
    let (mut p, cid, comp) = setup();
    let mut matte = solid(&mut p, &comp, [1.0, 1.0, 1.0], 50, 50);
    matte.props.prop_mut("transform/position").unwrap().value = Value::Vec3([50.0, 50.0, 0.0]);
    matte.switches.video = false;
    let mut fill = solid(&mut p, &comp, [0.0, 0.0, 1.0], 200, 100);
    fill.track_matte = Some(TrackMatte { layer: matte.id, kind: MatteKind::Alpha });
    let c = p.comp_mut(cid).unwrap();
    c.layers.push(matte);
    c.layers.push(fill);
    let img = render_frame(&p, cid, Tick::ZERO, 1.0);
    assert!(img.get(50, 50)[2] > 0.99);
    assert!(img.get(150, 50)[3] < 0.01);
}

/// Every matte kind with an opaque white 50×50 matte at the left of a 200×100 comp: the plain
/// kinds keep the fill only inside the matte, the inverted kinds only outside it (#864).
#[test]
fn track_matte_kinds_keep_and_invert() {
    for (kind, inside) in [(MatteKind::Alpha, true), (MatteKind::AlphaInverted, false), (MatteKind::Luma, true), (MatteKind::LumaInverted, false)] {
        let (mut p, cid, comp) = setup();
        let mut matte = solid(&mut p, &comp, [1.0, 1.0, 1.0], 50, 50);
        matte.props.prop_mut("transform/position").unwrap().value = Value::Vec3([50.0, 50.0, 0.0]);
        matte.switches.video = false;
        let mut fill = solid(&mut p, &comp, [0.0, 0.0, 1.0], 200, 100);
        fill.track_matte = Some(TrackMatte { layer: matte.id, kind });
        let c = p.comp_mut(cid).unwrap();
        c.layers.push(matte);
        c.layers.push(fill);
        let img = render_frame(&p, cid, Tick::ZERO, 1.0);
        let (a_in, a_out) = (img.get(50, 50)[3], img.get(150, 50)[3]);
        if inside {
            assert!(a_in > 0.99 && a_out < 0.01, "{kind:?}: {a_in} inside, {a_out} outside");
            assert!(img.get(50, 50)[2] > 0.99, "{kind:?} keeps the fill's colour");
        } else {
            assert!(a_in < 0.01 && a_out > 0.99, "{kind:?}: {a_in} inside, {a_out} outside");
            assert!(img.get(150, 50)[2] > 0.99, "{kind:?} keeps the fill's colour");
        }
    }
}

/// An adjustment layer with a track matte applies its effects only where the matte allows: an
/// Invert over a 0.25 grey with an opaque white 50×50 matte at the left of the comp (#1201).
#[test]
fn track_matte_limits_an_adjustment_layer() {
    for (kind, inside) in [(MatteKind::Alpha, true), (MatteKind::AlphaInverted, false), (MatteKind::Luma, true), (MatteKind::LumaInverted, false)] {
        let (mut p, cid, comp) = setup();
        let mut matte = solid(&mut p, &comp, [1.0, 1.0, 1.0], 50, 50);
        matte.props.prop_mut("transform/position").unwrap().value = Value::Vec3([50.0, 50.0, 0.0]);
        matte.switches.video = false;
        let mut adj = solid(&mut p, &comp, [1.0, 1.0, 1.0], 200, 100);
        adj.switches.adjustment = true;
        add_effect_200(&mut p, &mut adj, "ec.channel.invert", &[]);
        adj.track_matte = Some(TrackMatte { layer: matte.id, kind });
        let bg = solid(&mut p, &comp, [0.25, 0.25, 0.25], 200, 100);
        let c = p.comp_mut(cid).unwrap();
        c.layers.push(matte);
        c.layers.push(adj);
        c.layers.push(bg);
        let img = render_frame(&p, cid, Tick::ZERO, 1.0);
        let (g_in, g_out) = (img.get(50, 50)[0], img.get(150, 50)[0]);
        let (want_in, want_out) = if inside { (0.75, 0.25) } else { (0.25, 0.75) };
        assert!((g_in - want_in).abs() < 1e-4 && (g_out - want_out).abs() < 1e-4, "{kind:?}: {g_in} inside, {g_out} outside");
        assert_eq!(img.get(150, 50)[3], 1.0, "{kind:?} keeps the comp opaque");
    }
}

#[test]
fn shape_layer_draws_fill_and_stroke() {
    let (mut p, cid, comp) = setup();
    let mut l = build::layer(&mut p, &comp, "Shape Layer 1", LayerSource::Shape, (200, 100), None);
    let mut next = p.next_id;
    let mut ids = Ids(&mut next);
    let rect = build::shape_rect(&mut ids, [40.0, 40.0], [0.0, 0.0], 0.0);
    let fill = build::shape_fill(&mut ids, [1.0, 0.0, 0.0, 1.0]);
    let stroke = build::shape_stroke(&mut ids, [1.0, 1.0, 1.0, 1.0], 4.0);
    let g = build::shape_group(&mut ids, "Rectangle 1", vec![rect, stroke, fill]);
    p.next_id = next;
    l.props.sub_mut("contents").unwrap().children.push(g.into());
    p.comp_mut(cid).unwrap().layers.push(l);
    let img = render_frame(&p, cid, Tick::ZERO, 1.0);
    let centre = img.get(100, 50);
    assert!(centre[0] > 0.99 && centre[1] < 0.01, "{centre:?}");
    let edge = img.get(80, 50);
    assert!(edge[1] > 0.5, "stroke on top {edge:?}");
}

#[test]
fn text_layer_renders_glyphs() {
    let (mut p, cid, comp) = setup();
    let mut l = build::layer(&mut p, &comp, "Text", LayerSource::Text, (200, 100), None);
    l.props.prop_mut("text/sourceText").unwrap().value = Value::Text(Box::new(TextDoc { text: "HI".into(), size: 60.0, ..Default::default() }));
    l.props.prop_mut("transform/position").unwrap().value = Value::Vec3([60.0, 80.0, 0.0]);
    p.comp_mut(cid).unwrap().layers.push(l);
    let img = render_frame(&p, cid, Tick::ZERO, 1.0);
    let covered = img.data.iter().filter(|px| px[3] > 0.5).count();
    assert!(covered > 300, "{covered}");
}

#[test]
fn precomp_and_3d_render() {
    let (mut p, cid, comp) = setup();
    let inner = Comp::new(100, 100, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let iid = p.add_item("Inner", Label::Sandstone, None, ItemKind::Comp(inner.clone().into()));
    let s = solid(&mut p, &inner, [1.0, 0.5, 0.0], 100, 100);
    p.comp_mut(iid).unwrap().layers.push(s);
    let mut pre = build::layer(&mut p, &comp, "Inner", LayerSource::Comp { item: iid }, (100, 100), None);
    pre.switches.three_d = true;
    p.comp_mut(cid).unwrap().layers.push(pre);
    let img = render_frame(&p, cid, Tick::ZERO, 1.0);
    // A 3D layer at z = 0 with the default camera looks exactly like 2D.
    let c = img.get(100, 50);
    assert!((c[0] - 1.0).abs() < 1e-3 && (c[1] - 0.5).abs() < 1e-3, "{c:?}");
    assert!(img.get(10, 50)[3] < 0.01);
}

#[test]
fn effects_run_in_pipeline() {
    let (mut p, cid, comp) = setup();
    let mut l = solid(&mut p, &comp, [1.0, 1.0, 1.0], 50, 50);
    let spec = effectcraft_effects::find("ec.color.tint").unwrap();
    let mut next = p.next_id;
    let mut g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), "Tint", [50.0, 50.0]);
    p.next_id = next;
    if let Some(pr) = g.prop_mut("white") {
        pr.value = Value::Color([0.0, 1.0, 0.0, 1.0]);
    }
    l.props.sub_mut("effects").unwrap().children.push(g.into());
    p.comp_mut(cid).unwrap().layers.push(l);
    let img = render_frame(&p, cid, Tick::ZERO, 1.0);
    let c = img.get(100, 50);
    assert!(c[1] > 0.99 && c[0] < 0.01, "{c:?}");
}

/// Card Dance's Comp Camera through the default comp camera reproduces a 2D layer in place.
#[test]
fn card_dance_comp_camera_matches_the_default_view() {
    let (mut p, cid, comp) = setup();
    let mut l = solid(&mut p, &comp, [0.2, 0.6, 1.0], 50, 50);
    add_effect(&mut p, &mut l, "ec.sim.carddance", [50.0, 50.0], &[("cameraSystem", Value::Enum(2))]);
    p.comp_mut(cid).unwrap().layers.push(l);
    let img = render_frame(&p, cid, Tick::ZERO, 1.0);
    let c = img.get(100, 50);
    assert!((c[0] - 0.2).abs() < 0.02 && (c[2] - 1.0).abs() < 0.02 && c[3] > 0.99, "{c:?}");
    assert!(img.get(10, 50)[3] < 0.01);
}

/// A black 2D solid with CC Particle World, under a camera layer panned right by `pan` px.
fn particle_world_scene(pan: f64) -> (Project, ItemId) {
    let (mut p, cid, comp) = setup();
    let mut l = solid(&mut p, &comp, [0.0, 0.0, 0.0], 200, 100);
    let vals = [("birthRate", Value::Scalar(2.0)), ("particle/particleType", Value::Enum(2)), ("particle/birthSize", Value::Scalar(0.1))];
    add_effect(&mut p, &mut l, "ec.sim.ccparticleworld", [200.0, 100.0], &vals);
    let mut cam = build::layer(&mut p, &comp, "Camera", LayerSource::Camera, (200, 100), None);
    for path in ["transform/position", "transform/poi"] {
        let pr = cam.props.prop_mut(path).unwrap();
        let Value::Vec3(v) = pr.value else { panic!("{path}: {:?}", pr.value) };
        pr.value = Value::Vec3([v[0] + pan, v[1], v[2]]);
    }
    p.comp_mut(cid).unwrap().layers = vec![cam, l];
    (p, cid)
}

/// The red-weighted centre of the particles (x).
fn particles_x(img: &crate::Image) -> f32 {
    let (mut m, mut sx) = (0.0, 0.0);
    for y in 0..img.height {
        for x in 0..img.width {
            let r = img.get(x as i64, y as i64)[0];
            m += r;
            sx += r * x as f32;
        }
    }
    assert!(m > 1.0, "particles drawn");
    sx / m
}

/// #397: CC Particle World on a 2D layer is seen through the comp's camera layer, and the layer
/// cache notices the camera moving.
#[test]
fn particle_world_follows_the_comp_camera() {
    let t = Tick::from_seconds_f64(1.0);
    let (p, cid) = particle_world_scene(0.0);
    let (q, _) = particle_world_scene(40.0);
    let cache = crate::LayerCache::default();
    let here = render_cached(&p, cid, t, Some(&cache));
    let panned = render_cached(&q, cid, t, Some(&cache));
    assert_same(&panned, &render_cached(&q, cid, t, None), "cached render after the camera moved");
    let (a, b) = (particles_x(&here), particles_x(&panned));
    assert!(b < a - 20.0, "panning the camera right moves the particles left: {a} -> {b}");
}

// ------------------------------------------------------------------------------ layer cache

fn render_cached(p: &Project, cid: ItemId, t: Tick, cache: Option<&crate::LayerCache>) -> crate::Image {
    let mut r = crate::Renderer::new(p, &crate::NoFootage, crate::RenderOpts::default());
    r.cache = cache;
    r.comp_frame(cid, t)
}

/// #329: a render whose footage reads were called off keeps nothing in the layer cache: a layer
/// made from a frame that came back empty (an effect's layer parameter) would be reused by the
/// next render of that time.
#[test]
fn a_called_off_render_keeps_nothing_in_the_layer_cache() {
    let (p, cid) = cache_scene();
    let t = Tick::from_seconds_f64(0.5);
    let cache = crate::LayerCache::default();
    let off = crate::CancellableFootage { inner: &crate::NoFootage, cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)) };
    let mut r = crate::Renderer::new(&p, &off, crate::RenderOpts::default());
    r.cache = Some(&cache);
    r.comp_frame(cid, t);
    assert_eq!(cache.stats().entries, 0, "a called-off render kept layers");
    render_cached(&p, cid, t, Some(&cache));
    assert!(cache.stats().entries > 0, "a render that runs to the end keeps its layers");
}

/// [`add_effect`] for a 200×100 layer.
fn add_effect_200(p: &mut Project, l: &mut effectcraft_project::Layer, id: &str, vals: &[(&str, Value)]) {
    add_effect(p, l, id, [200.0, 100.0], vals);
}

fn add_effect(p: &mut Project, l: &mut effectcraft_project::Layer, id: &str, size: [f64; 2], vals: &[(&str, Value)]) {
    let spec = effectcraft_effects::find(id).unwrap();
    let mut next = p.next_id;
    let mut g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, size);
    p.next_id = next;
    for (k, v) in vals {
        g.prop_mut(k).unwrap().value = v.clone();
    }
    l.props.sub_mut("effects").unwrap().children.push(g.into());
}

/// A comp with a static effected solid, an animated-trim shape, a moving text layer with a blur.
fn cache_scene() -> (Project, ItemId) {
    let (mut p, cid, comp) = setup();
    let mut bg = solid(&mut p, &comp, [0.2, 0.3, 0.4], 200, 100);
    add_effect_200(&mut p, &mut bg, "ec.color.tint", &[("white", Value::Color([1.0, 0.5, 0.0, 1.0]))]);
    let mut shape = build::layer(&mut p, &comp, "Shape", LayerSource::Shape, (200, 100), None);
    let mut next = p.next_id;
    {
        let mut ids = Ids(&mut next);
        let e = build::shape_ellipse(&mut ids, [60.0, 60.0], [0.0, 0.0]);
        let tr = build::shape_trim(&mut ids, 0.0, 0.0, 0.0);
        let s = build::shape_stroke(&mut ids, [1.0, 1.0, 1.0, 1.0], 4.0);
        let g = build::shape_group(&mut ids, "Ring", vec![e, tr, s]);
        shape.props.sub_mut("contents").unwrap().children.push(g.into());
    }
    p.next_id = next;
    shape.props.prop_mut("contents/group#1/contents/trim/end").unwrap().keys =
        vec![Keyframe::new(Tick::ZERO, Value::Scalar(10.0)), Keyframe::new(Tick::from_seconds_f64(1.0), Value::Scalar(100.0))];
    let mut text = build::layer(&mut p, &comp, "Text", LayerSource::Text, (200, 100), None);
    text.props.prop_mut("text/sourceText").unwrap().value = Value::Text(Box::new(TextDoc { text: "AB".into(), size: 40.0, ..Default::default() }));
    text.props.prop_mut("transform/position").unwrap().keys =
        vec![Keyframe::new(Tick::ZERO, Value::Vec3([20.0, 70.0, 0.0])), Keyframe::new(Tick::from_seconds_f64(1.0), Value::Vec3([120.0, 70.0, 0.0]))];
    add_effect_200(&mut p, &mut text, "ec.blur.gaussian", &[("blurriness", Value::Scalar(6.0))]);
    let c = p.comp_mut(cid).unwrap();
    c.layers = vec![text, shape, bg];
    (p, cid)
}

fn assert_same(a: &crate::Image, b: &crate::Image, what: &str) {
    assert_eq!((a.width, a.height), (b.width, b.height), "{what}");
    for (i, (p, q)) in a.data.iter().zip(&b.data).enumerate() {
        for c in 0..4 {
            assert!((p[c] - q[c]).abs() < 1e-6, "{what}: pixel {i}: {p:?} vs {q:?}");
        }
    }
}

#[test]
fn cached_frames_match_uncached() {
    let (p, cid) = cache_scene();
    let cache = crate::LayerCache::default();
    for f in [0.0, 0.25, 0.5, 0.5, 1.2, 1.5, 0.25] {
        let t = Tick::from_seconds_f64(f);
        assert_same(&render_cached(&p, cid, t, Some(&cache)), &render_cached(&p, cid, t, None), &format!("t={f}"));
    }
    // bg is static, text only moves (transform), the shape is static after 1 s: lots of reuse.
    let st = cache.stats();
    assert!(st.hits >= 10, "{st:?}");
}

#[test]
fn static_and_transform_only_layers_are_reused() {
    let (p, cid) = cache_scene();
    let cache = crate::LayerCache::default();
    render_cached(&p, cid, Tick::from_seconds_f64(0.2), Some(&cache));
    let before = cache.stats();
    render_cached(&p, cid, Tick::from_seconds_f64(0.4), Some(&cache));
    let after = cache.stats();
    // bg (static) + text (only its position animates) hit; the shape's trim animates: miss.
    assert_eq!(after.hits - before.hits, 2, "{before:?} {after:?}");
    assert_eq!(after.misses - before.misses, 1, "{before:?} {after:?}");
}

type Edit = Box<dyn Fn(&mut Project, ItemId)>;

/// Every kind of edit re-renders the affected layer: the cached render of the edited project
/// equals a fresh uncached render, and differs from the unedited frame.
#[test]
fn edits_invalidate_cached_layers() {
    let (p, cid) = cache_scene();
    let t = Tick::from_seconds_f64(0.5);
    let edits: Vec<(&str, Edit)> = vec![
        (
            "effect param value",
            Box::new(|p, cid| p.comp_mut(cid).unwrap().layers[2].props.prop_mut("effects/#1/white").unwrap().value = Value::Color([0.0, 1.0, 0.0, 1.0])),
        ),
        (
            "effect param keyframes",
            Box::new(|p, cid| {
                p.comp_mut(cid).unwrap().layers[0].props.prop_mut("effects/#1/blurriness").unwrap().keys =
                    vec![Keyframe::new(Tick::ZERO, Value::Scalar(0.0)), Keyframe::new(Tick::from_seconds_f64(1.0), Value::Scalar(30.0))]
            }),
        ),
        ("effect disabled", Box::new(|p, cid| p.comp_mut(cid).unwrap().layers[2].props.group_mut("effects/#1").unwrap().enabled = false)),
        ("effects switch off", Box::new(|p, cid| p.comp_mut(cid).unwrap().layers[0].switches.effects = false)),
        (
            "effect added",
            Box::new(|p, cid| {
                let mut l = p.comp_mut(cid).unwrap().layers[2].clone();
                add_effect_200(p, &mut l, "ec.channel.invert", &[]);
                p.comp_mut(cid).unwrap().layers[2] = l;
            }),
        ),
        (
            "shape keyframe moved",
            Box::new(|p, cid| {
                p.comp_mut(cid).unwrap().layers[1].props.prop_mut("contents/group#1/contents/trim/end").unwrap().keys[1].time = Tick::from_seconds_f64(2.0)
            }),
        ),
        (
            "text changed",
            Box::new(|p, cid| {
                p.comp_mut(cid).unwrap().layers[0].props.prop_mut("text/sourceText").unwrap().value =
                    Value::Text(Box::new(TextDoc { text: "XYZ".into(), size: 40.0, ..Default::default() }))
            }),
        ),
        (
            "solid colour",
            Box::new(|p, cid| {
                let LayerSource::Solid { item } = p.comp(cid).unwrap().layers[2].source else { panic!("not a solid layer") };
                if let ItemKind::Solid(s) = &mut p.item_mut(item).unwrap().kind {
                    s.color = [0.9, 0.1, 0.1];
                }
                p.comp_mut(cid).unwrap().layers[2].props.group_mut("effects/#1").unwrap().enabled = false;
            }),
        ),
        (
            "mask added",
            Box::new(|p, cid| {
                let mut next = p.next_id;
                let m = build::mask(&mut Ids(&mut next), "Mask 1", ShapePath::rect([50.0, 50.0], 40.0, 40.0), MaskMode::Add, [255, 255, 0]);
                p.next_id = next;
                p.comp_mut(cid).unwrap().layers[2].props.sub_mut("masks").unwrap().children.push(m.into());
            }),
        ),
        ("layer time shifted", Box::new(|p, cid| p.comp_mut(cid).unwrap().layers[1].start_time = Tick::from_seconds_f64(0.3))),
    ];
    for (what, edit) in edits {
        let cache = crate::LayerCache::default();
        let orig = render_cached(&p, cid, t, Some(&cache));
        let mut q = p.clone();
        edit(&mut q, cid);
        let cached = render_cached(&q, cid, t, Some(&cache));
        let fresh = render_cached(&q, cid, t, None);
        assert_same(&cached, &fresh, what);
        assert!(orig.data.iter().zip(&fresh.data).any(|(a, b)| (a[0] - b[0]).abs() + (a[3] - b[3]).abs() > 1e-3), "{what}: edit had no visible effect");
    }
}

#[test]
fn time_dependent_effects_rerender_every_frame() {
    let (mut p, cid, comp) = setup();
    let mut l = solid(&mut p, &comp, [0.5, 0.5, 0.5], 200, 100);
    add_effect_200(&mut p, &mut l, "ec.noise.noise", &[("amount", Value::Scalar(50.0))]);
    p.comp_mut(cid).unwrap().layers.push(l);
    let cache = crate::LayerCache::default();
    let a = render_cached(&p, cid, Tick::from_seconds_f64(0.1), Some(&cache));
    let b = render_cached(&p, cid, Tick::from_seconds_f64(0.2), Some(&cache));
    assert_same(&b, &render_cached(&p, cid, Tick::from_seconds_f64(0.2), None), "noise at 0.2");
    assert!(a != b, "noise must animate");
}

#[test]
fn effects_see_layer_masks() {
    let (mut p, cid, comp) = setup();
    let mut l = solid(&mut p, &comp, [0.0, 0.0, 0.0], 200, 100);
    let mut next = p.next_id;
    let m = build::mask(&mut Ids(&mut next), "Mask 1", ShapePath::rect([50.0, 50.0], 40.0, 40.0), MaskMode::None, [255, 255, 0]);
    p.next_id = next;
    l.props.sub_mut("masks").unwrap().children.push(m.into());
    add_effect(&mut p, &mut l, "ec.generate.stroke", [200.0, 100.0], &[("brushSize", Value::Scalar(6.0))]);
    p.comp_mut(cid).unwrap().layers.push(l);
    let img = render_frame(&p, cid, Tick::ZERO, 1.0);
    assert!(img.get(30, 50)[0] > 0.5, "stroke on the mask edge: {:?}", img.get(30, 50));
    assert!(img.get(50, 50)[0] < 0.01, "inside untouched");
    assert!(img.get(150, 50)[3] > 0.99, "mode None mask does not cut the layer");
}

/// An open mask (the Pen still drawing it) leaves the layer whole; closed, it masks (#290).
#[test]
fn open_masks_make_no_transparency() {
    let render = |closed: bool| {
        let (mut p, cid, comp) = setup();
        let mut l = solid(&mut p, &comp, [1.0, 0.0, 0.0], 200, 100);
        let path = ShapePath { closed, ..ShapePath::rect([50.0, 50.0], 40.0, 40.0) };
        let mut next = p.next_id;
        let m = build::mask(&mut Ids(&mut next), "Mask 1", path, MaskMode::Add, [255, 255, 0]);
        p.next_id = next;
        l.props.sub_mut("masks").unwrap().children.push(m.into());
        p.comp_mut(cid).unwrap().layers.push(l);
        render_frame(&p, cid, Tick::ZERO, 1.0)
    };
    let open = render(false);
    assert!(open.get(50, 50)[3] > 0.99 && open.get(150, 50)[3] > 0.99, "the open mask cut the layer");
    let closed = render(true);
    assert!(closed.get(50, 50)[3] > 0.99 && closed.get(150, 50)[3] < 0.01, "the closed mask masks");
}

#[test]
fn effects_read_layer_params() {
    let (mut p, cid, comp) = setup();
    let mut src = solid(&mut p, &comp, [0.0, 0.0, 1.0], 200, 100);
    src.switches.video = false;
    let src_id = src.id.0;
    let mut top = solid(&mut p, &comp, [1.0, 0.0, 0.0], 200, 100);
    add_effect(&mut p, &mut top, "ec.channel.blend", [200.0, 100.0], &[("blendWithLayer", Value::Layer(Some(src_id)))]);
    let c = p.comp_mut(cid).unwrap();
    c.layers.push(top);
    c.layers.push(src);
    let img = render_frame(&p, cid, Tick::ZERO, 1.0);
    let px = img.get(100, 50);
    assert!(px[2] > 0.99 && px[0] < 0.01, "{px:?}");
}

/// Records whether the GPU compositor was asked to draw a frame.
struct ProbeAccel(std::sync::atomic::AtomicBool);
impl crate::Accelerator for ProbeAccel {
    fn name(&self) -> String {
        "probe".into()
    }
    fn comp_frame(&self, _r: &crate::Renderer, _comp: ItemId, _t: Tick) -> Option<crate::Image> {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        None // fall back to the CPU result
    }
    fn supports_effect(&self, _id: &str) -> bool {
        false
    }
    fn effects(&self, _chain: &[crate::FxStep], _buf: &crate::Buf, _levels: Option<f32>) -> Option<crate::Buf> {
        None
    }
}

#[test]
fn auto_backend_sends_2d_and_3d_comps_to_the_gpu() {
    let (mut p, cid, comp) = setup();
    p.settings.gpu_acceleration = true;
    let l = solid(&mut p, &comp, [1.0, 0.0, 0.0], 50, 50);
    p.comp_mut(cid).unwrap().layers.push(l);
    let asked = |p: &Project, backend: crate::Backend| {
        let probe = ProbeAccel(Default::default());
        let mut r = crate::Renderer::new(p, &crate::NoFootage, crate::RenderOpts { backend, ..Default::default() });
        r.accel = Some(&probe);
        let _ = r.comp_frame(cid, Tick::ZERO);
        probe.0.load(std::sync::atomic::Ordering::SeqCst)
    };
    // 2D comp: Auto uses the GPU compositor.
    assert!(asked(&p, crate::Backend::Auto));
    // A 3D layer: Classic 3D runs on the GPU too (M12.7), so Auto still asks it.
    p.comp_mut(cid).unwrap().layers[0].switches.three_d = true;
    assert!(asked(&p, crate::Backend::Auto));
    assert!(asked(&p, crate::Backend::Gpu));
    // Software Only: never.
    p.settings.gpu_acceleration = false;
    assert!(!asked(&p, crate::Backend::Auto));
}

/// #227: a shape layer's content surrounds its origin, so edge pinning (Turbulent Displace's
/// default Pin All, Wave Warp's All Edges) must measure from comp-sized bounds centred on it;
/// bounds starting at the origin pinned all but the bottom-right quadrant.
#[test]
fn edge_pinning_displaces_all_of_a_centred_shape_layer() {
    for (id, vals) in [("ec.distort.turbulentdisplace", vec![]), ("ec.distort.wavewarp", vec![("pinning", Value::Enum(1))])] {
        let (mut p, cid, comp) = setup();
        let mut l = build::layer(&mut p, &comp, "Shape Layer 1", LayerSource::Shape, (200, 100), None);
        let mut next = p.next_id;
        let mut ids = Ids(&mut next);
        let rect = build::shape_rect(&mut ids, [180.0, 80.0], [0.0, 0.0], 0.0);
        let fill = build::shape_fill(&mut ids, [1.0, 0.0, 0.0, 1.0]);
        let g = build::shape_group(&mut ids, "Rectangle 1", vec![rect, fill]);
        p.next_id = next;
        l.props.sub_mut("contents").unwrap().children.push(g.into());
        p.comp_mut(cid).unwrap().layers.push(l.clone());
        let plain = render_frame(&p, cid, Tick::ZERO, 1.0);
        add_effect_200(&mut p, &mut l, id, &vals);
        p.comp_mut(cid).unwrap().layers = vec![l];
        let warped = render_frame(&p, cid, Tick::ZERO, 1.0);
        // Alpha change per comp quadrant (TL, TR, BL, BR): the rectangle's edges move in each.
        let mut q = [0.0f32; 4];
        for y in 0..100 {
            for x in 0..200 {
                q[(x >= 100) as usize + 2 * (y >= 50) as usize] += (plain.get(x, y)[3] - warped.get(x, y)[3]).abs();
            }
        }
        assert!(q.iter().all(|&d| d > 20.0), "{id}: per-quadrant change {q:?}");
    }
}

/// Effect points on a shape layer are measured from the top-left of its comp-sized bounds
/// (#227), so the default "layer centre" is where the content's origin is. Measured from the
/// origin they sat at the comp's bottom-right corner: Twirl changed nothing and Bulge only
/// the bottom-right quadrant.
#[test]
fn default_effect_points_sit_at_the_centre_of_a_shape_layer() {
    let cases: [(&str, Vec<(&str, Value)>); 2] =
        [("ec.distort.twirl", vec![("angle", Value::Scalar(90.0))]), ("ec.distort.bulge", vec![("height", Value::Scalar(2.0))])];
    for (id, vals) in cases {
        let (mut p, cid, comp) = setup();
        let mut l = build::layer(&mut p, &comp, "Shape Layer 1", LayerSource::Shape, (200, 100), None);
        let mut next = p.next_id;
        let mut ids = Ids(&mut next);
        let rect = build::shape_rect(&mut ids, [180.0, 80.0], [0.0, 0.0], 0.0);
        let fill = build::shape_fill(&mut ids, [1.0, 0.0, 0.0, 1.0]);
        let g = build::shape_group(&mut ids, "Rectangle 1", vec![rect, fill]);
        p.next_id = next;
        l.props.sub_mut("contents").unwrap().children.push(g.into());
        p.comp_mut(cid).unwrap().layers.push(l.clone());
        let plain = render_frame(&p, cid, Tick::ZERO, 1.0);
        add_effect_200(&mut p, &mut l, id, &vals);
        p.comp_mut(cid).unwrap().layers = vec![l];
        let warped = render_frame(&p, cid, Tick::ZERO, 1.0);
        // Alpha change per comp quadrant (TL, TR, BL, BR): the top and bottom edges move in each.
        let mut q = [0.0f32; 4];
        for y in 0..100 {
            for x in 0..200 {
                q[(x >= 100) as usize + 2 * (y >= 50) as usize] += (plain.get(x, y)[3] - warped.get(x, y)[3]).abs();
            }
        }
        assert!(q.iter().all(|&d| d > 5.0), "{id}: per-quadrant change {q:?}");
        // Symmetric about the centre: opposite quadrants change alike.
        assert!((q[0] - q[3]).abs() < 0.05 * q[0].max(q[3]) && (q[1] - q[2]).abs() < 0.05 * q[1].max(q[2]), "{id}: {q:?}");
    }
}

/// #659: a tiling effect on a text layer tiles across the layer's whole region. The glyphs
/// were cut off at their own bounding box: the effect computed over the comp-sized layer rect
/// but wrote into the painted (glyph-bounds) buffer alone. Tiling shrinks each copy, so the
/// probe is the x-span of the painted pixels, not their count.
#[test]
fn effects_on_text_reach_past_the_text_bounds() {
    let (mut p, cid, comp) = setup();
    let mut l = build::layer(&mut p, &comp, "Text", LayerSource::Text, (200, 100), None);
    l.props.prop_mut("text/sourceText").unwrap().value = Value::Text(Box::new(TextDoc { text: "HI".into(), size: 24.0, ..Default::default() }));
    l.props.prop_mut("transform/position").unwrap().value = Value::Vec3([50.0, 60.0, 0.0]);
    p.comp_mut(cid).unwrap().layers.push(l.clone());
    let plain = render_frame(&p, cid, Tick::ZERO, 1.0);
    let painted = plain.data.iter().filter(|px| px[3] > 0.5).count();
    add_effect_200(&mut p, &mut l, "ec.distort.cctiler", &[("scale", Value::Scalar(25.0))]);
    p.comp_mut(cid).unwrap().layers = vec![l];
    let tiled = render_frame(&p, cid, Tick::ZERO, 1.0);
    let (mut lo, mut hi) = (u32::MAX, 0u32);
    for y in 0..100 {
        for x in 0..200 {
            if tiled.get(x as i64, y as i64)[3] > 0.5 {
                lo = lo.min(x);
                hi = hi.max(x);
            }
        }
    }
    assert!(
        painted > 50 && hi.saturating_sub(lo) > 100,
        "CC Tiler tiles the glyphs across the layer region, not just its text bounds: {painted} painted, x-span {lo}..{hi}"
    );
}

/// #658: CC Smear drags text glyphs beyond the text layer's own bounds. Pixels smeared past
/// the painted rectangle used to fall off its edge, so the liquid trail never appeared.
#[test]
fn cc_smear_trails_text_past_its_bounds() {
    let (mut p, cid, comp) = setup();
    let mut l = build::layer(&mut p, &comp, "Text", LayerSource::Text, (200, 100), None);
    l.props.prop_mut("text/sourceText").unwrap().value = Value::Text(Box::new(TextDoc { text: "HI".into(), size: 24.0, ..Default::default() }));
    l.props.prop_mut("transform/position").unwrap().value = Value::Vec3([60.0, 60.0, 0.0]);
    p.comp_mut(cid).unwrap().layers.push(l.clone());
    let right = |img: &effectcraft_raster::Image| (0..100).map(|y| (0..200).filter(|x| img.get(*x, y)[3] > 0.5).max().unwrap_or(0)).max().unwrap_or(0);
    let plain = right(&render_frame(&p, cid, Tick::ZERO, 1.0));
    add_effect_200(
        &mut p,
        &mut l,
        "ec.distort.ccsmear",
        &[("from", Value::Vec2([110.0, 45.0])), ("to", Value::Vec2([185.0, 45.0])), ("reach", Value::Scalar(100.0)), ("radius", Value::Scalar(40.0))],
    );
    p.comp_mut(cid).unwrap().layers = vec![l];
    let smeared = right(&render_frame(&p, cid, Tick::ZERO, 1.0));
    assert!(plain > 40 && smeared > plain + 25, "CC Smear trails the glyphs past the text bounds: right edge {plain} → {smeared}");
}

/// A layer parameter pointing at a shape layer sees all of it (in its effect space), and the
/// effect's own masks are in effect space too: Set Matte with a centred shape layer as the
/// matte keeps the centre of a full-comp solid (#227).
#[test]
fn layer_parameters_see_a_whole_shape_layer() {
    let (mut p, cid, comp) = setup();
    let mut shape = build::layer(&mut p, &comp, "Matte", LayerSource::Shape, (200, 100), None);
    let mut next = p.next_id;
    let mut ids = Ids(&mut next);
    let rect = build::shape_rect(&mut ids, [100.0, 50.0], [0.0, 0.0], 0.0);
    let fill = build::shape_fill(&mut ids, [1.0, 1.0, 1.0, 1.0]);
    let g = build::shape_group(&mut ids, "Rectangle 1", vec![rect, fill]);
    p.next_id = next;
    shape.props.sub_mut("contents").unwrap().children.push(g.into());
    shape.switches.video = false;
    let matte_id = shape.id;
    let mut s = solid(&mut p, &comp, [0.0, 0.0, 1.0], 200, 100);
    add_effect_200(&mut p, &mut s, "ec.channel.setmatte", &[("takeMatteFromLayer", Value::Layer(Some(matte_id.0)))]);
    p.comp_mut(cid).unwrap().layers = vec![s, shape];
    let f = render_frame(&p, cid, Tick::ZERO, 1.0);
    // The matte covers x 50..150, y 25..75: kept inside (all four quadrants), gone outside.
    for (x, y) in [(60, 30), (140, 30), (60, 70), (140, 70)] {
        assert!(f.get(x, y)[3] > 0.99, "inside at ({x}, {y}): {:?}", f.get(x, y));
    }
    for (x, y) in [(20, 10), (180, 90), (190, 50)] {
        assert!(f.get(x, y)[3] < 0.01, "outside at ({x}, {y}): {:?}", f.get(x, y));
    }
}

#[test]
fn set_matte_self_source_and_masks_precede_effects() {
    for source in [0, 1] {
        let (mut p, cid, comp) = setup();
        let mut l = solid(&mut p, &comp, [1.0, 0.0, 0.0], 200, 100);
        let mut next = p.next_id;
        let mask = build::mask(&mut Ids(&mut next), "Left", ShapePath::rect([50.0, 50.0], 80.0, 80.0), MaskMode::Add, [255, 255, 0]);
        p.next_id = next;
        l.props.sub_mut("masks").unwrap().children.push(mask.into());
        let id = l.id.0;
        add_effect_200(&mut p, &mut l, "ec.channel.invert", &[("channel", Value::Enum(effectcraft_effects::INVERT_ALPHA))]);
        add_effect_200(
            &mut p,
            &mut l,
            "ec.channel.setmatte",
            &[
                ("takeMatteFromLayer", Value::Layer(Some(id))),
                ("takeMatteFromLayerSource", Value::Enum(source)),
                ("compositeMatteWithOriginal", Value::Bool(false)),
            ],
        );
        p.comp_mut(cid).unwrap().layers.push(l);
        let img = render_frame(&p, cid, Tick::ZERO, 1.0);
        assert!(img.get(50, 50)[3] > 0.99, "original alpha must survive preceding alpha inversion");
        assert!((img.get(150, 50)[3] - if source == 0 { 1.0 } else { 0.0 }).abs() < 0.01, "Source and Masks must remain distinct");
    }
}

#[test]
fn cc_composite_restores_stack_input_on_layers_and_adjustments() {
    for adjustment in [false, true] {
        for mode in [0, 1] {
            let (mut p, cid, comp) = setup();
            let mut l = solid(&mut p, &comp, [1.0, 0.0, 0.0], 200, 100);
            l.switches.adjustment = adjustment;
            add_effect_200(&mut p, &mut l, "ec.channel.invert", &[]);
            add_effect_200(&mut p, &mut l, "ec.channel.cccomposite", &[("compositeOriginal", Value::Enum(mode))]);
            p.comp_mut(cid).unwrap().layers.push(l);
            if adjustment {
                let bg = solid(&mut p, &comp, [1.0, 0.0, 0.0], 200, 100);
                p.comp_mut(cid).unwrap().layers.push(bg);
            }
            let img = render_frame(&p, cid, Tick::ZERO, 1.0);
            let want = if mode == 0 { [1.0, 0.0, 0.0, 1.0] } else { [0.0, 1.0, 1.0, 1.0] };
            assert_eq!(img.get(50, 50), want, "adjustment={adjustment} mode={mode}");
        }
    }
}

#[test]
fn self_inputs_share_source_semantics_across_effects_and_times() {
    use crate::{EffectHost, FxHost, NoFootage, RenderOpts, Renderer};
    let (mut p, cid, comp) = setup();
    let mut l = solid(&mut p, &comp, [1.0, 0.0, 0.0], 200, 100);
    let mut next = p.next_id;
    let mask = build::mask(&mut Ids(&mut next), "Left", ShapePath::rect([50.0, 50.0], 80.0, 80.0), MaskMode::Add, [255, 255, 0]);
    p.next_id = next;
    l.props.sub_mut("masks").unwrap().children.push(mask.into());
    p.comp_mut(cid).unwrap().layers.push(l.clone());
    let c = p.comp(cid).unwrap();
    let ctx = crate::eval::EvalCtx::new(&p, cid, c, Tick::ZERO);
    let renderer = Renderer::new(&p, &NoFootage, RenderOpts::default());
    let host = FxHost { r: &renderer, ctx: &ctx, layer: &l, origin: [0.0; 2], index: Default::default(), original: None };
    assert_eq!(host.layer(l.id.0, false).unwrap().buf.img.get(150, 50)[3], 1.0);
    assert_eq!(host.layer_masks(l.id.0).unwrap().buf.img.get(150, 50)[3], 0.0);
    assert_eq!(host.layer_at(l.id.0, 0.5, false).unwrap().buf.img.get(150, 50)[3], 1.0);
    assert!(host.layer(l.id.0, true).is_none(), "full-stack self references must not recurse");
    assert!(host.layer_at(l.id.0, 0.5, true).is_none());
    assert!(host.layer_at(l.id.0, f64::NAN, false).is_none());
    // A second built-in using the shared selector: Invert(red) then Blend(original red).
    l.props.sub_mut("masks").unwrap().children.clear();
    add_effect_200(&mut p, &mut l, "ec.channel.invert", &[]);
    let id = l.id.0;
    add_effect_200(
        &mut p,
        &mut l,
        "ec.channel.blend",
        &[("blendWithLayer", Value::Layer(Some(id))), ("blendWithLayerSource", Value::Enum(0)), ("blendWithOriginal", Value::Scalar(0.0))],
    );
    p.comp_mut(cid).unwrap().layers = vec![l];
    let c = p.comp(cid).unwrap();
    let ctx = crate::eval::EvalCtx::new(&p, cid, c, Tick::ZERO);
    assert!(crate::cache::input_key(&ctx, &c.layers[0], 1.0, false, false, 1).is_some());
    assert!(crate::cache::input_key(&ctx, &c.layers[0], 1.0, false, false, 2).is_some());
    assert_eq!(render_frame(&p, cid, Tick::ZERO, 1.0).get(50, 50), [1.0, 0.0, 0.0, 1.0]);
}

#[test]
fn timed_source_reads_preserve_other_layer_masks() {
    use effectcraft_effects::EffectHost;
    let (mut p, cid, comp) = setup();
    let owner = solid(&mut p, &comp, [1.0, 0.0, 0.0], 200, 100);
    let mut other = solid(&mut p, &comp, [0.0, 1.0, 0.0], 200, 100);
    let mut next = p.next_id;
    let mask = build::mask(&mut Ids(&mut next), "Left", ShapePath::rect([50.0, 50.0], 80.0, 80.0), MaskMode::Add, [255, 255, 0]);
    p.next_id = next;
    other.props.sub_mut("masks").unwrap().children.push(mask.into());
    p.comp_mut(cid).unwrap().layers = vec![owner.clone(), other.clone()];
    let ctx = crate::EvalCtx::new(&p, cid, p.comp(cid).unwrap(), Tick::ZERO);
    let renderer = crate::Renderer::new(&p, &crate::NoFootage, crate::RenderOpts::default());
    let host = crate::FxHost { r: &renderer, ctx: &ctx, layer: &owner, origin: [0.0; 2], index: Default::default(), original: None };
    // Current Source and timed Source had different contracts before this fix. Keep
    // the existing timed read (used by Clone/Time effects) masked for other layers.
    assert_eq!(host.layer(other.id.0, false).unwrap().buf.img.get(150, 50)[3], 1.0);
    let timed = host.layer_at(other.id.0, 0.5, false).unwrap();
    assert_eq!(timed.buf.img.get(50, 50)[3], 1.0);
    assert_eq!(timed.buf.img.get(150, 50)[3], 0.0);
}

#[test]
fn layer_dependencies_remain_cacheable_and_invalidate_only_related_edits() {
    let (mut p, cid, comp) = setup();
    let mut owner = solid(&mut p, &comp, [1.0, 0.0, 0.0], 200, 100);
    let mut source = solid(&mut p, &comp, [0.0, 1.0, 0.0], 200, 100);
    let third = solid(&mut p, &comp, [0.0, 0.0, 1.0], 200, 100);
    let unrelated = solid(&mut p, &comp, [0.2, 0.3, 0.4], 200, 100);
    add_effect_200(&mut p, &mut source, "ec.channel.blend", &[("blendWithLayer", Value::Layer(Some(third.id.0))), ("blendWithOriginal", Value::Scalar(0.0))]);
    add_effect_200(&mut p, &mut owner, "ec.channel.blend", &[("blendWithLayer", Value::Layer(Some(source.id.0))), ("blendWithOriginal", Value::Scalar(0.0))]);
    p.comp_mut(cid).unwrap().layers = vec![owner, source, third, unrelated];
    let key = |p: &Project, time: f64, limit| {
        let comp = p.comp(cid).unwrap();
        let ctx = crate::EvalCtx::new(p, cid, comp, Tick::from_seconds_f64(time));
        crate::cache::input_key(&ctx, &comp.layers[0], 1.0, false, false, limit).unwrap()
    };
    let initial = key(&p, 0.0, 1);
    let input = key(&p, 0.0, 0);
    assert_eq!(initial, key(&p, 0.5, 1), "static dependencies reuse across frames");
    let cache = crate::LayerCache::default();
    let before = render_cached(&p, cid, Tick::ZERO, Some(&cache));
    render_cached(&p, cid, Tick::ZERO, Some(&cache));
    assert!(cache.stats().hits > 0, "layer parameters must not disable caching");
    let LayerSource::Solid { item: unrelated } = p.comp(cid).unwrap().layers[3].source else { panic!() };
    let ItemKind::Solid(s) = &mut p.item_mut(unrelated).unwrap().kind else { panic!() };
    s.color = [0.9, 0.8, 0.7];
    assert_eq!(initial, key(&p, 0.0, 1), "unrelated edits preserve the owner key");
    let LayerSource::Solid { item: third } = p.comp(cid).unwrap().layers[2].source else { panic!() };
    let ItemKind::Solid(s) = &mut p.item_mut(third).unwrap().kind else { panic!() };
    s.color = [1.0, 0.0, 1.0];
    assert_ne!(initial, key(&p, 0.0, 1), "transitive source edits invalidate the owner");
    assert_eq!(input, key(&p, 0.0, 0), "effects after an input prefix do not affect its key");
    let cached = render_cached(&p, cid, Tick::ZERO, Some(&cache));
    let fresh = render_cached(&p, cid, Tick::ZERO, None);
    assert_same(&cached, &fresh, "transitive dependency edit");
    assert_ne!(before.get(50, 50), fresh.get(50, 50));
    let owner_id = p.comp(cid).unwrap().layers[0].id;
    p.comp_mut(cid).unwrap().layers[1].props.prop_mut("effects/#1/blendWithLayer").unwrap().value = Value::Layer(Some(owner_id.0));
    let comp = p.comp(cid).unwrap();
    let ctx = crate::EvalCtx::new(&p, cid, comp, Tick::ZERO);
    assert!(crate::cache::layer_key(&ctx, &comp.layers[0], 1.0, false, false).is_none(), "cycles decline caching without recursion");
}

#[test]
fn precomp_dependencies_hash_nested_sources_and_evaluated_animation() {
    let (mut p, cid, comp) = setup();
    let mut child = solid(&mut p, &comp, [0.0, 0.0, 1.0], 200, 100);
    let LayerSource::Solid { item: source } = child.source else { panic!() };
    child.props.prop_mut("transform/opacity").unwrap().keys =
        vec![Keyframe::new(Tick::ZERO, Value::Scalar(100.0)), Keyframe::new(Tick::from_seconds_f64(1.0), Value::Scalar(50.0))];
    let mut nested = comp.clone();
    nested.layers = vec![child];
    let nested_id = p.add_item("Nested", Label::Sandstone, None, ItemKind::Comp(nested.into()));
    let pre = build::layer(&mut p, &comp, "Precomp", LayerSource::Comp { item: nested_id }, (200, 100), None);
    let mut owner = solid(&mut p, &comp, [1.0, 0.0, 0.0], 200, 100);
    add_effect_200(&mut p, &mut owner, "ec.channel.blend", &[("blendWithLayer", Value::Layer(Some(pre.id.0))), ("blendWithOriginal", Value::Scalar(0.0))]);
    p.comp_mut(cid).unwrap().layers = vec![owner, pre];
    let key = |p: &Project, time| {
        let c = p.comp(cid).unwrap();
        crate::cache::layer_key(&crate::EvalCtx::new(p, cid, c, time), &c.layers[0], 1.0, false, false).unwrap()
    };
    let cache = crate::LayerCache::default();
    let before = render_cached(&p, cid, Tick::ZERO, Some(&cache));
    let initial = key(&p, Tick::ZERO);
    let later = Tick::from_seconds_f64(0.5);
    assert_ne!(initial, key(&p, later));
    assert_same(&render_cached(&p, cid, later, Some(&cache)), &render_cached(&p, cid, later, None), "animated precomp dependency");
    let ItemKind::Solid(s) = &mut p.item_mut(source).unwrap().kind else { panic!() };
    s.color = [0.0, 1.0, 0.0];
    assert_ne!(initial, key(&p, Tick::ZERO), "items used inside the precomp invalidate the owner");
    let fresh = render_cached(&p, cid, Tick::ZERO, None);
    assert_same(&render_cached(&p, cid, Tick::ZERO, Some(&cache)), &fresh, "precomp source edit");
    assert_ne!(before.get(50, 50), fresh.get(50, 50));
}

/// #352: a shape layer's bounds (snapping, Align) are its visual edges: a 100×100 rectangle is
/// exactly ±50, and a 10 px stroke adds half its width, with no anti-aliasing padding.
#[test]
fn shape_content_bounds_are_the_visual_edges() {
    for (stroke, half) in [(None, 50.0), (Some(10.0), 55.0)] {
        let (mut p, cid, comp) = setup();
        let mut l = build::layer(&mut p, &comp, "Shape Layer 1", LayerSource::Shape, (200, 100), None);
        let mut next = p.next_id;
        let mut ids = Ids(&mut next);
        let mut items = vec![build::shape_rect(&mut ids, [100.0, 100.0], [0.0, 0.0], 0.0), build::shape_fill(&mut ids, [1.0, 0.0, 0.0, 1.0])];
        if let Some(w) = stroke {
            items.push(build::shape_stroke(&mut ids, [1.0, 1.0, 1.0, 1.0], w));
        }
        let g = build::shape_group(&mut ids, "Rectangle 1", items);
        p.next_id = next;
        l.props.sub_mut("contents").unwrap().children.push(g.into());
        p.comp_mut(cid).unwrap().layers.push(l.clone());
        let ctx = crate::EvalCtx::new(&p, cid, p.comp(cid).unwrap(), Tick::ZERO);
        let b = crate::content_bounds(&ctx, &l).unwrap();
        assert!(b.iter().zip([-half, -half, half, half]).all(|(a, e)| (a - e).abs() < 1e-6), "{stroke:?}: {b:?}");
    }
}

/// Converts synchronously: answers `planes` (or declines).
struct SyncYuv(Option<Vec<u8>>);
impl crate::Accelerator for SyncYuv {
    fn name(&self) -> String {
        "sync-yuv".into()
    }
    fn comp_frame(&self, _r: &crate::Renderer, _comp: ItemId, _t: Tick) -> Option<crate::Image> {
        None
    }
    fn comp_frame_yuv420(&self, _r: &crate::Renderer, _comp: ItemId, _t: Tick, _bg: [f32; 3]) -> Option<Vec<u8>> {
        self.0.clone()
    }
    fn supports_effect(&self, _id: &str) -> bool {
        false
    }
    fn effects(&self, _chain: &[crate::FxStep], _buf: &crate::Buf, _levels: Option<f32>) -> Option<crate::Buf> {
        None
    }
}

/// An accelerator without asynchronous readbacks still serves the asynchronous call: the
/// callback runs once, right away, or comes back untouched when the frame is declined.
#[test]
fn async_yuv_defaults_to_the_synchronous_conversion() {
    use crate::Accelerator;
    let (p, cid, _) = setup();
    let r = crate::Renderer::new(&p, &crate::NoFootage, Default::default());
    let got = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let sink = |got: &std::sync::Arc<std::sync::Mutex<Vec<Option<Vec<u8>>>>>| -> crate::YuvDone {
        let got = got.clone();
        Box::new(move |b| got.lock().unwrap().push(b))
    };
    let a = SyncYuv(Some(vec![1, 2, 3]));
    assert!(a.comp_frame_yuv420_async(&r, cid, Tick::ZERO, [0.0; 3], sink(&got)).is_ok());
    assert_eq!(*got.lock().unwrap(), vec![Some(vec![1, 2, 3])]);
    assert!(a.poll_readbacks(Some(std::time::Duration::from_millis(1))));
    let back = SyncYuv(None).comp_frame_yuv420_async(&r, cid, Tick::ZERO, [0.0; 3], sink(&got)).expect_err("declined");
    assert_eq!(got.lock().unwrap().len(), 1);
    back(None);
    assert_eq!(got.lock().unwrap().len(), 2);
}
