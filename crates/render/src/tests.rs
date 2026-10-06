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

// ------------------------------------------------------------------------------ layer cache

fn render_cached(p: &Project, cid: ItemId, t: Tick, cache: Option<&crate::LayerCache>) -> crate::Image {
    let mut r = crate::Renderer::new(p, &crate::NoFootage, crate::RenderOpts::default());
    r.cache = cache;
    r.comp_frame(cid, t)
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

/// Issue #103: the Audio, Lock and Shy switches (and the comp's Hide Shy Layers) don't change a
/// pixel, so they keep every cached layer: nothing re-renders. The disk cache's comp content key
/// stays too, while the Video switch still changes it.
#[test]
fn audio_lock_and_shy_switches_keep_cached_layers() {
    let (p, cid) = cache_scene();
    let t = Tick::from_seconds_f64(0.5);
    let cache = crate::LayerCache::default();
    let orig = render_cached(&p, cid, t, Some(&cache));
    let s0 = cache.stats();
    render_cached(&p, cid, t, Some(&cache));
    let s1 = cache.stats();
    let mut q = p.clone();
    let c = q.comp_mut(cid).unwrap();
    c.hide_shy = true;
    for l in &mut c.layers {
        l.switches.audio = false;
        l.switches.locked = true;
        l.switches.shy = true;
    }
    let again = render_cached(&q, cid, t, Some(&cache));
    let s2 = cache.stats();
    assert_same(&again, &orig, "same pixels");
    assert_eq!((s2.hits - s1.hits, s2.misses - s1.misses), (s1.hits - s0.hits, s1.misses - s0.misses), "nothing re-rendered: {s0:?} {s1:?} {s2:?}");
    assert!(s2.hits > s1.hits);
    let key = |p: &Project| crate::disk_cache::comp_content_key(p, cid);
    assert_eq!(key(&q), key(&p), "the disk cache keeps its frames");
    q.comp_mut(cid).unwrap().layers[2].switches.video = false;
    assert_ne!(key(&q), key(&p), "the Video switch changes the frames");
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
