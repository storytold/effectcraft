//! Collapse Transformations, Continuously Rasterize and the Quality switch.

use effectcraft_color::{BlendMode, Label};
use effectcraft_keyframe::{TextDoc, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{BitDepth, Comp, FrameBlend, ItemId, ItemKind, Layer, LayerSource, Project, Quality, Solid};
use effectcraft_time::{FrameRate, Tick};

use crate::{EvalCtx, Image, NoFootage, RenderOpts, Renderer, render_frame};

fn project() -> Project {
    let mut p = Project::default();
    p.settings.bit_depth = BitDepth::Bpc32;
    p
}

fn solid(p: &mut Project, comp: &Comp, color: [f32; 3], size: (u32, u32), pos: [f64; 3]) -> Layer {
    let sid = p.add_item("S", Label::Red, None, ItemKind::Solid(Solid { color, width: size.0, height: size.1, pixel_aspect: 1.0 }));
    let mut l = build::layer(p, comp, "S", LayerSource::Solid { item: sid }, size, None);
    l.props.prop_mut("transform/position").unwrap().value = Value::Vec3(pos);
    l
}

fn set(l: &mut Layer, path: &str, v: Value) {
    l.props.prop_mut(path).unwrap().value = v;
}

fn max_diff(a: &Image, b: &Image) -> f32 {
    a.data.iter().zip(&b.data).flat_map(|(p, q)| (0..4).map(move |c| (p[c] - q[c]).abs())).fold(0.0, f32::max)
}

/// Outer 200×100 comp: a grey background, and either a precomp (scaled 150%, rotated) of two
/// solids — the top one in Add mode — or the same two solids parented to a null that carries the
/// precomp layer's transform (the manual flattening).
fn scene(collapse: bool, flatten: bool) -> (Project, ItemId) {
    let mut p = project();
    let outer = Comp::new(200, 100, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let inner = Comp::new(100, 100, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let mut a = solid(&mut p, &inner, [0.6, 0.1, 0.1], (40, 40), [40.0, 50.0, 0.0]);
    let mut b = solid(&mut p, &inner, [0.1, 0.3, 0.6], (40, 40), [60.0, 50.0, 0.0]);
    b.blend_mode = BlendMode::Add;
    let bg = solid(&mut p, &outer, [0.3, 0.3, 0.3], (200, 100), [100.0, 50.0, 0.0]);
    let xf = |l: &mut Layer| {
        set(l, "transform/anchor", Value::Vec3([50.0, 50.0, 0.0]));
        set(l, "transform/position", Value::Vec3([100.0, 50.0, 0.0]));
        set(l, "transform/scale", Value::Vec3([150.0, 150.0, 100.0]));
        set(l, "transform/rotation", Value::Scalar(20.0));
    };

    let cid = if flatten {
        let mut null = build::layer(&mut p, &outer, "Null", LayerSource::Null, (100, 100), None);
        xf(&mut null);
        a.parent = Some(null.id);
        b.parent = Some(null.id);
        let mut c = outer.clone();
        c.layers = vec![null, b, a, bg].into();
        p.add_item("Outer", Label::Sandstone, None, ItemKind::Comp(c.into()))
    } else {
        let mut inner = inner;
        inner.layers = vec![b, a].into();
        let iid = p.add_item("Inner", Label::Sandstone, None, ItemKind::Comp(inner.into()));
        let mut pre = build::layer(&mut p, &outer, "Inner", LayerSource::Comp { item: iid }, (100, 100), None);
        xf(&mut pre);
        pre.switches.collapse = collapse;
        let mut c = outer.clone();
        c.layers = vec![pre, bg].into();
        p.add_item("Outer", Label::Sandstone, None, ItemKind::Comp(c.into()))
    };
    (p, cid)
}

#[test]
fn collapsed_precomp_equals_manual_flattening() {
    let (p, cid) = scene(false, true);
    let flat = render_frame(&p, cid, Tick::ZERO, 1.0);
    let (p, cid) = scene(true, false);
    let collapsed = render_frame(&p, cid, Tick::ZERO, 1.0);
    assert!(max_diff(&flat, &collapsed) < 1e-4, "collapsed differs from flattening by {}", max_diff(&flat, &collapsed));
    // Without collapsing, Add only sees the precomp's own layers (not the grey below), and the
    // precomp is resampled twice.
    let (p, cid) = scene(false, false);
    let nested = render_frame(&p, cid, Tick::ZERO, 1.0);
    assert!(max_diff(&flat, &nested) > 0.2);
    // Opacity of the collapsed layer fades every nested layer.
    let (mut p, cid) = scene(true, false);
    set(&mut p.comp_mut(cid).unwrap().layers[0], "transform/opacity", Value::Scalar(50.0));
    let half = render_frame(&p, cid, Tick::ZERO, 1.0);
    let (x, y) = (100, 50);
    let (bg, full) = (0.3, collapsed.get(x, y)[0]);
    assert!((half.get(x, y)[0] - (bg + (full - bg) * 0.5)).abs() < 0.03, "{:?} vs {full}", half.get(x, y));
}

#[test]
fn masks_on_a_collapsed_layer_force_a_flattened_render() {
    let (mut p, cid) = scene(true, false);
    let (p2, cid2) = scene(false, false);
    // Add a mask covering everything: the layer renders like an uncollapsed precomp.
    let mut next = p.next_id;
    let mask = build::mask(
        &mut Ids(&mut next),
        "Mask 1",
        effectcraft_keyframe::ShapePath::rect([50.0, 50.0], 4000.0, 4000.0),
        effectcraft_project::MaskMode::Add,
        [255, 255, 0],
    );
    p.next_id = next;
    p.comp_mut(cid).unwrap().layers[0].props.sub_mut("masks").unwrap().children.push(mask.into());
    let a = render_frame(&p, cid, Tick::ZERO, 1.0);
    let b = render_frame(&p2, cid2, Tick::ZERO, 1.0);
    assert!(max_diff(&a, &b) < 1e-4, "{}", max_diff(&a, &b));
}

/// #1747: the flattened path honours a composition proxy and frame blending; a collapsed layer
/// renders the nested comp directly and ignores both, so such a layer must not collapse.
#[test]
fn proxy_and_frame_blending_keep_a_precomp_on_the_flattened_path() {
    let (mut p, cid) = scene(true, false);
    let id = p.comp(cid).unwrap().layers[0].id;
    let collapses = |p: &Project| {
        let r = Renderer::new(p, &NoFootage, RenderOpts::default());
        let ctx = EvalCtx::new(p, cid, p.comp(cid).unwrap(), Tick::ZERO);
        r.collapsed(&ctx, ctx.comp.layer(id).unwrap()).is_some()
    };
    assert!(collapses(&p), "a plain precomp collapses");
    // Frame blending needs both the comp's switch and the layer's.
    p.comp_mut(cid).unwrap().enable_frame_blending = true;
    p.comp_mut(cid).unwrap().layers[0].switches.frame_blend = FrameBlend::FrameMix;
    assert!(!collapses(&p), "frame blending keeps the flattened path");
    p.comp_mut(cid).unwrap().enable_frame_blending = false;
    p.comp_mut(cid).unwrap().layers[0].switches.frame_blend = FrameBlend::Off;
    // A composition proxy stands in for the precomp's pixels.
    let LayerSource::Comp { item } = p.comp(cid).unwrap().layers[0].source else { panic!("a precomp layer") };
    let proxy =
        effectcraft_project::Proxy { footage: effectcraft_project::Footage { has_video: true, width: 32, height: 32, ..Default::default() }, enabled: true };
    p.item_mut(item).unwrap().proxy = Some(Box::new(proxy));
    assert!(!collapses(&p), "a proxy keeps the flattened path");
}

/// #1747: an effect on B that reads A must not reuse its cached buffer when two collapsed
/// instances give A different static overrides (`hash_values` only sees animated values).
#[test]
fn referenced_layer_overrides_change_the_owner_cache_key() {
    let mut p = project();
    let comp = Comp::new(100, 100, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let a = solid(&mut p, &comp, [1.0, 1.0, 1.0], (40, 40), [30.0, 50.0, 0.0]);
    let aid = a.id;
    let mut b = solid(&mut p, &comp, [1.0, 1.0, 1.0], (100, 100), [50.0, 50.0, 0.0]);
    let bid = b.id;
    let mut next = p.next_id;
    let mut ids = Ids(&mut next);
    let spec = effectcraft_effects::find("ec.channel.setmatte").unwrap();
    let mut fx = effectcraft_effects::instantiate(spec, &mut ids, spec.name, [100.0, 100.0]);
    fx.prop_mut("takeMatteFromLayer").unwrap().value = Value::Layer(Some(a.id.0));
    p.next_id = next;
    b.props.sub_mut("effects").unwrap().children.push(fx.into());
    let mut c = comp;
    c.layers = vec![b, a].into();
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(c.into()));
    let comp = p.comp(cid).unwrap();
    let b = comp.layer(bid).unwrap();
    let opacity = comp.layer(aid).unwrap().props.prop("transform/opacity").unwrap().uid;
    let key = |value: f64| {
        let overrides = [crate::PropertyOverride { layer: aid, prop: opacity, value: Value::Scalar(value), font: false }];
        let ctx =
            EvalCtx { project: &p, comp_id: cid, comp, time: Tick::ZERO, expr: None, footage: None, property_overrides: &overrides, media_overrides: &[] };
        crate::cache::layer_key(&ctx, b, 1.0, false, false)
    };
    let (opaque, half) = (key(100.0), key(50.0));
    assert!(opaque.is_some(), "the owner is cacheable");
    assert_ne!(opaque, half, "A's static override changes B's key");
}

#[test]
fn collapsed_3d_layers_join_the_parent_scene() {
    // Parent: a red 3D solid at z = 0 above (in the stack) a collapsed 3D precomp holding a
    // blue 3D solid at z = −200 (nearer the camera). Collapsed, depth sorting puts blue in front;
    // uncollapsed, the precomp is one plane at z = 0 drawn below the red.
    let run = |collapse: bool| {
        let mut p = project();
        let outer = Comp::new(200, 200, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
        let inner = Comp::new(200, 200, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
        let mut blue = solid(&mut p, &inner, [0.0, 0.0, 1.0], (60, 60), [100.0, 100.0, -200.0]);
        blue.switches.three_d = true;
        let mut inner = inner;
        inner.layers = vec![blue].into();
        let iid = p.add_item("Inner", Label::Sandstone, None, ItemKind::Comp(inner.into()));
        let mut pre = build::layer(&mut p, &outer, "Inner", LayerSource::Comp { item: iid }, (200, 200), None);
        pre.switches.three_d = true;
        pre.switches.collapse = collapse;
        let mut red = solid(&mut p, &outer, [1.0, 0.0, 0.0], (100, 100), [100.0, 100.0, 0.0]);
        red.switches.three_d = true;
        let mut c = outer;
        c.layers = vec![red, pre].into();
        let cid = p.add_item("Outer", Label::Sandstone, None, ItemKind::Comp(c.into()));
        render_frame(&p, cid, Tick::ZERO, 1.0).get(100, 100)
    };
    let c = run(true);
    assert!(c[2] > 0.99 && c[0] < 0.01, "collapsed: blue in front {c:?}");
    let n = run(false);
    assert!(n[0] > 0.99 && n[2] < 0.01, "uncollapsed: red on top {n:?}");
}

/// Pixels along row `y` from `x0` to `x1` with partial coverage (an edge's width).
fn edge_width(img: &Image, y: i64, x0: i64, x1: i64) -> usize {
    (x0..x1).filter(|&x| (0.02..0.98).contains(&img.get(x, y)[3])).count()
}

/// A 10 px white square in a 200×200 comp, scaled 800% either by its own Scale or by a parent
/// null's, with the Continuously Rasterize switch `cr`.
fn scaled_square(cr: bool, via_parent: bool) -> Image {
    let mut p = project();
    let comp = Comp::new(200, 200, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let mut l = build::layer(&mut p, &comp, "Shape", LayerSource::Shape, (200, 200), None);
    let mut next = p.next_id;
    let mut ids = Ids(&mut next);
    let rect = build::shape_rect(&mut ids, [10.0, 10.0], [0.0, 0.0], 0.0);
    let fill = build::shape_fill(&mut ids, [1.0, 1.0, 1.0, 1.0]);
    let g = build::shape_group(&mut ids, "Rectangle 1", vec![rect, fill]);
    p.next_id = next;
    l.props.sub_mut("contents").unwrap().children.push(g.into());
    set(&mut l, "transform/anchor", Value::Vec3([0.0, 0.0, 0.0]));
    l.switches.collapse = cr;
    let mut c = comp.clone();
    if via_parent {
        let mut null = build::layer(&mut p, &comp, "Null", LayerSource::Null, (100, 100), None);
        set(&mut null, "transform/anchor", Value::Vec3([0.0, 0.0, 0.0]));
        set(&mut null, "transform/position", Value::Vec3([100.3, 100.3, 0.0]));
        set(&mut null, "transform/scale", Value::Vec3([800.0, 800.0, 100.0]));
        set(&mut l, "transform/position", Value::Vec3([0.0, 0.0, 0.0]));
        l.parent = Some(null.id);
        c.layers = vec![l, null].into();
    } else {
        set(&mut l, "transform/position", Value::Vec3([100.3, 100.3, 0.0]));
        set(&mut l, "transform/scale", Value::Vec3([800.0, 800.0, 100.0]));
        c.layers = vec![l].into();
    }
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(c.into()));
    render_frame(&p, cid, Tick::ZERO, 1.0)
}

/// Shape layers are always continuously rasterised, as in After Effects (#751): a shape scaled up
/// by its own transform or a parent's is drawn at its on-screen size, with a 1 px anti-aliased
/// edge, whether or not the Continuously Rasterize switch is on.
#[test]
fn shape_scaled_up_stays_sharp_without_the_switch() {
    // The 10 px square scaled 800% spans 60.3..140.3.
    for cr in [false, true] {
        for via_parent in [false, true] {
            let img = scaled_square(cr, via_parent);
            let w = edge_width(&img, 100, 50, 75);
            assert!(w <= 1, "switch {cr}, scaled by parent {via_parent}: edge {w} px wide");
            assert!(img.get(100, 100)[3] > 0.99 && img.get(55, 100)[3] < 0.01);
        }
    }
}

#[test]
fn wireframe_quality_draws_the_layer_bounds() {
    let mut p = project();
    let comp = Comp::new(100, 100, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let mut l = solid(&mut p, &comp, [1.0, 0.0, 0.0], (40, 40), [50.0, 50.0, 0.0]);
    l.switches.quality = Quality::Wireframe;
    let mut c = comp;
    c.layers = vec![l].into();
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(c.into()));
    let img = render_frame(&p, cid, Tick::ZERO, 1.0);
    assert_eq!(img.get(50, 50)[3], 0.0, "no fill");
    assert_eq!(img.get(30, 50), [1.0, 1.0, 1.0, 1.0], "outline");
    // Draft quality samples nearest-neighbour: no partial pixels at a half-pixel offset.
    let mut p = project();
    let comp = Comp::new(100, 100, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let mut l = solid(&mut p, &comp, [1.0, 1.0, 1.0], (40, 40), [50.5, 50.0, 0.0]);
    l.switches.quality = Quality::Draft;
    let mut c = comp;
    c.layers = vec![l].into();
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(c.into()));
    let img = render_frame(&p, cid, Tick::ZERO, 1.0);
    assert!(img.data.iter().all(|q| q[3] == 0.0 || q[3] == 1.0));
}

#[test]
fn collapsed_precomp_of_another_size_renders_through_the_parent_camera() {
    // A 1000×600 scene comp with a blue 3D solid at its centre, collapsed into a 300×200 comp
    // whose camera looks at the precomp layer (at the parent's centre). The nested layers must
    // be projected with the parent comp's view (its centre and size), not the scene comp's.
    let run = |three_d_precomp: bool, camera: bool| {
        let mut p = project();
        let outer = Comp::new(300, 200, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
        let inner = Comp::new(1000, 600, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
        let mut blue = solid(&mut p, &inner, [0.0, 0.0, 1.0], (40, 40), [500.0, 300.0, 0.0]);
        blue.switches.three_d = true;
        let mut inner = inner;
        inner.layers = vec![blue].into();
        let iid = p.add_item("Scene", Label::Sandstone, None, ItemKind::Comp(inner.into()));
        let mut pre = build::layer(&mut p, &outer, "Scene", LayerSource::Comp { item: iid }, (1000, 600), None);
        pre.switches.three_d = three_d_precomp;
        pre.switches.collapse = true;
        let mut c = outer.clone();
        c.layers = vec![pre].into();
        if camera {
            let cam = build::layer(&mut p, &outer, "Camera", LayerSource::Camera, (300, 200), None);
            c.layers.insert(0, cam);
        }
        let cid = p.add_item("Face", Label::Sandstone, None, ItemKind::Comp(c.into()));
        render_frame(&p, cid, Tick::ZERO, 1.0)
    };
    for (three_d, camera) in [(true, true), (true, false), (false, true)] {
        let img = run(three_d, camera);
        let c = img.get(150, 100);
        assert!(c[2] > 0.99 && c[3] > 0.99, "3D precomp {three_d}, camera {camera}: centre {c:?}");
        assert!(img.get(100, 100)[3] < 0.01, "only the 40 px solid is drawn");
    }
}

/// Text layers are always continuously rasterised, as text is vector in After Effects (#390, #194):
/// text scaled up with the layer's Scale is drawn at its on-screen size, as sharp as text set at
/// that font size, without the switch.
#[test]
fn text_scaled_up_stays_sharp_without_the_switch() {
    let run = |size: f64, scale: f64| {
        let mut p = project();
        let comp = Comp::new(240, 160, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
        let mut l = build::layer(&mut p, &comp, "Text", LayerSource::Text, (240, 160), None);
        l.props.prop_mut("text/sourceText").unwrap().value = Value::Text(Box::new(TextDoc { text: "HE".into(), size, ..Default::default() }));
        set(&mut l, "transform/anchor", Value::Vec3([0.0, 0.0, 0.0]));
        set(&mut l, "transform/position", Value::Vec3([40.0, 120.0, 0.0]));
        set(&mut l, "transform/scale", Value::Vec3([scale, scale, 100.0]));
        assert!(!l.switches.collapse);
        let mut c = comp;
        c.layers = vec![l].into();
        let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(c.into()));
        render_frame(&p, cid, Tick::ZERO, 1.0)
    };
    let partial = |img: &Image| img.data.iter().filter(|px| (0.02..0.98).contains(&px[3])).count();
    let scaled = run(10.0, 800.0);
    let native = run(80.0, 100.0);
    let (ps, pn) = (partial(&scaled), partial(&native));
    let d = max_diff(&scaled, &native);
    eprintln!("partially covered pixels: scaled {ps}, native {pn}; max difference {d}");
    assert!(native.data.iter().filter(|px| px[3] > 0.98).count() > 1000, "the text renders");
    assert!(ps <= pn + pn / 2, "scaled text is blurred: {ps} soft pixels against {pn}");
    assert!(d < 0.05, "scaled text differs from text set at that size by {d}");
}

/// Unscaled text is drawn at the output scale wherever it sits: the lengths a position that isn't a
/// whole number maps the unit vectors to, a rounding error over 1, don't raise its raster scale a
/// quarter octave (#390: that softened it, and drew it differently at nearby times).
#[test]
fn unscaled_text_rasterises_at_the_output_scale() {
    let mut p = project();
    let mut comp = Comp::new(240, 160, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let mut l = build::layer(&mut p, &comp, "Text", LayerSource::Text, (240, 160), None);
    l.props.prop_mut("text/sourceText").unwrap().value = Value::Text(Box::new(TextDoc { text: "HE".into(), size: 30.0, ..Default::default() }));
    comp.layers = vec![l].into();
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    for i in 0..500 {
        let pos = [3.1 + i as f64 * 0.37, 7.3 + i as f64 * 0.29, 0.0];
        set(&mut p.comp_mut(cid).unwrap().layers[0], "transform/position", Value::Vec3(pos));
        let r = Renderer::new(&p, &NoFootage, RenderOpts::default());
        let c = p.comp(cid).unwrap();
        let k = r.raster_scale(&EvalCtx::new(&p, cid, c, Tick::ZERO), &c.layers[0]);
        assert_eq!(k, 1.0, "text at {pos:?}");
    }
}

/// Effects on a shape layer run on its continuously rasterised pixels, still in layer space as
/// with the switch off in After Effects: a 10 px square scaled 800% with a 2 px blur looks like
/// an 80 px square with a 16 px blur, not a blurred bitmap enlarged afterwards (#751).
#[test]
fn effects_on_a_scaled_shape_follow_its_scale() {
    let run = |size: f64, scale: f64, blur: f64| {
        let mut p = project();
        let comp = Comp::new(200, 200, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
        let mut l = build::layer(&mut p, &comp, "Shape", LayerSource::Shape, (200, 200), None);
        let mut next = p.next_id;
        let mut ids = Ids(&mut next);
        let rect = build::shape_rect(&mut ids, [size, size], [0.0, 0.0], 0.0);
        let fill = build::shape_fill(&mut ids, [1.0, 1.0, 1.0, 1.0]);
        let g = build::shape_group(&mut ids, "Rectangle 1", vec![rect, fill]);
        let spec = effectcraft_effects::find("ec.blur.gaussian").unwrap();
        let mut fx = effectcraft_effects::instantiate(spec, &mut ids, spec.name, [200.0, 200.0]);
        p.next_id = next;
        fx.prop_mut("blurriness").unwrap().value = Value::Scalar(blur);
        l.props.sub_mut("contents").unwrap().children.push(g.into());
        l.props.sub_mut("effects").unwrap().children.push(fx.into());
        set(&mut l, "transform/anchor", Value::Vec3([0.0, 0.0, 0.0]));
        set(&mut l, "transform/position", Value::Vec3([100.0, 100.0, 0.0]));
        set(&mut l, "transform/scale", Value::Vec3([scale, scale, 100.0]));
        let mut c = comp;
        c.layers = vec![l].into();
        let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(c.into()));
        render_frame(&p, cid, Tick::ZERO, 1.0)
    };
    let scaled = run(10.0, 800.0, 2.0);
    let native = run(80.0, 100.0, 16.0);
    let d = max_diff(&scaled, &native);
    assert!(scaled.get(100, 100)[3] > 0.99 && scaled.get(70, 100)[3] > 0.05, "the blur spreads the edge");
    assert!(d < 0.05, "scaled blurred shape differs from the native one by {d}");
}
