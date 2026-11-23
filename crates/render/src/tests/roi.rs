//! Region of interest: a ROI render equals the matching crop of the full render.

use effectcraft_color::{BlendMode, Label};
use effectcraft_keyframe::Value;
use effectcraft_project::build;
use effectcraft_project::{Comp, ItemKind, LayerSource, Project, Solid};
use effectcraft_time::{FrameRate, Tick};

use crate::{Image, NoFootage, RenderOpts, Renderer};

fn project(three_d: bool) -> (Project, effectcraft_project::ItemId) {
    let mut p = Project::default();
    let comp = Comp::new(160, 120, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let mut layers = vec![];
    for (i, (c, w, h, pos, rot)) in
        [([1.0, 0.2, 0.1], 90, 50, [60.0, 50.0], 17.0), ([0.1, 0.4, 1.0], 70, 70, [100.0, 75.0], -30.0), ([0.9, 0.9, 0.2], 40, 30, [30.0, 95.0], 0.0)]
            .into_iter()
            .enumerate()
    {
        let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color: c, width: w, height: h, pixel_aspect: 1.0 }));
        let mut l = build::layer(&mut p, &comp, "Solid", LayerSource::Solid { item: sid }, (w, h), None);
        l.props.prop_mut("transform/position").unwrap().value = Value::Vec3([pos[0], pos[1], 0.0]);
        l.props.prop_mut("transform/rotation").unwrap().value = Value::Scalar(rot);
        l.props.prop_mut("transform/opacity").unwrap().value = Value::Scalar(80.0);
        if i == 1 {
            l.blend_mode = BlendMode::Screen;
            l.switches.three_d = three_d;
        }
        layers.push(l);
    }
    p.comp_mut(cid).unwrap().layers = layers.into();
    (p, cid)
}

fn crop(img: &Image, x: u32, y: u32, w: u32, h: u32) -> Image {
    let mut out = Image::new(w, h);
    for yy in 0..h {
        for xx in 0..w {
            out.data[(yy * w + xx) as usize] = img.data[((yy + y) * img.width + xx + x) as usize];
        }
    }
    out
}

fn assert_same(a: &Image, b: &Image) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    for (i, (p, q)) in a.data.iter().zip(&b.data).enumerate() {
        for c in 0..4 {
            assert!((p[c] - q[c]).abs() < 1e-4, "pixel {i} channel {c}: {} vs {}", p[c], q[c]);
        }
    }
}

#[test]
fn roi_render_equals_cropped_full_render() {
    for three_d in [false, true] {
        let (p, cid) = project(three_d);
        let full = Renderer::new(&p, &NoFootage, RenderOpts::default()).comp_frame(cid, Tick::ZERO);
        let roi = Renderer::new(&p, &NoFootage, RenderOpts { roi: Some([30.0, 20.0, 80.0, 60.0]), ..Default::default() }).comp_frame(cid, Tick::ZERO);
        assert_eq!((roi.width, roi.height), (80, 60));
        assert_same(&roi, &crop(&full, 30, 20, 80, 60));
    }
}

#[test]
fn roi_at_half_resolution() {
    let (p, cid) = project(false);
    let full = Renderer::new(&p, &NoFootage, RenderOpts { scale: 0.5, ..Default::default() }).comp_frame(cid, Tick::ZERO);
    let roi = Renderer::new(&p, &NoFootage, RenderOpts { scale: 0.5, roi: Some([40.0, 20.0, 60.0, 40.0]), ..Default::default() }).comp_frame(cid, Tick::ZERO);
    assert_eq!((roi.width, roi.height), (30, 20));
    assert_same(&roi, &crop(&full, 20, 10, 30, 20));
}

/// Extended Viewer: a region reaching past the comp frame shows the 3D layer's pixels on the
/// pasteboard, and its inner part equals the normal frame.
#[test]
fn extended_region_renders_3d_layers_beyond_the_frame() {
    let (mut p, cid) = project(true);
    // Push the 3D (blue) layer half out of the frame on the right.
    let comp = p.comp_mut(cid).unwrap();
    comp.layers[1].props.prop_mut("transform/position").unwrap().value = Value::Vec3([150.0, 60.0, 0.0]);
    comp.layers[1].props.prop_mut("transform/rotation").unwrap().value = Value::Scalar(0.0);
    let full = Renderer::new(&p, &NoFootage, RenderOpts::default()).comp_frame(cid, Tick::ZERO);
    let region = crate::extended_region(160, 120, [-100.0, -100.0, 260.0, 220.0], 0.5).unwrap();
    assert_eq!(region, [-80.0, -64.0, 320.0, 248.0]);
    let ext = Renderer::new(&p, &NoFootage, RenderOpts { roi: Some(region), ..Default::default() }).comp_frame(cid, Tick::ZERO);
    assert_eq!((ext.width, ext.height), (320, 248));
    // Comp x 175 (past the 160 px frame) at y 60: the layer covers x 115..185.
    let out = ext.get(175 + 80, 60 + 64);
    assert!(out[3] > 0.5 && out[2] > 0.5, "pasteboard pixel {out:?}");
    // Nothing is drawn far outside every layer.
    assert_eq!(ext.get(5, 5)[3], 0.0);
    // The frame part is the normal frame.
    assert_same(&crop(&ext, 80, 64, 160, 120), &full);
}

#[test]
fn extended_region_is_none_inside_the_frame() {
    assert_eq!(crate::extended_region(160, 120, [10.0, 10.0, 150.0, 110.0], 1.0), None);
    // Clamped to the margin.
    assert_eq!(crate::extended_region(100, 100, [-1000.0, 0.0, 50.0, 100.0], 1.0), Some([-112.0, 0.0, 212.0, 100.0]));
}

/// ROI effect-stack cropping on a full-frame layer matches cropped full-frame rendering
/// pixel-for-pixel, and uses a distinct cache key so it never pollutes the full-layer cache.
#[test]
fn roi_effect_crop_matches_full_render_and_isolates_cache() {
    let mut p = Project::default();
    let comp = Comp::new(640, 480, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color: [0.85, 0.35, 0.15], width: 640, height: 480, pixel_aspect: 1.0 }));
    let mut l = build::layer(&mut p, &comp, "HeavyFx", LayerSource::Solid { item: sid }, (640, 480), None);
    let mut ids = build::Ids(&mut p.next_id);
    let mask =
        build::mask(&mut ids, "Mask 1", effectcraft_keyframe::ShapePath::rect([320.0, 240.0], 280.0, 200.0), effectcraft_project::MaskMode::Add, [255, 255, 0]);
    l.props.sub_mut("masks").unwrap().children.push(effectcraft_project::Node::Group(mask));
    let mut blur = effectcraft_effects::instantiate(effectcraft_effects::find("ec.blur.gaussian").unwrap(), &mut ids, "Gaussian Blur", [640.0, 480.0]);
    blur.prop_mut("blurriness").unwrap().value = Value::Scalar(18.0);
    let mut glow = effectcraft_effects::instantiate(effectcraft_effects::find("ec.stylize.glow").unwrap(), &mut ids, "Glow", [640.0, 480.0]);
    glow.prop_mut("radius").unwrap().value = Value::Scalar(14.0);
    let mut levels = effectcraft_effects::instantiate(effectcraft_effects::find("ec.color.levels").unwrap(), &mut ids, "Levels", [640.0, 480.0]);
    levels.prop_mut("gamma").unwrap().value = Value::Scalar(1.3);
    let fx = l.props.sub_mut("effects").unwrap();
    fx.children.push(effectcraft_project::Node::Group(blur));
    fx.children.push(effectcraft_project::Node::Group(glow));
    fx.children.push(effectcraft_project::Node::Group(levels));
    p.comp_mut(cid).unwrap().layers = vec![l].into();

    let roi_box = [200.0, 160.0, 160.0, 120.0];
    let cache = crate::LayerCache::default();
    let mut r_roi = Renderer::new(&p, &NoFootage, RenderOpts { roi: Some(roi_box), ..Default::default() });
    r_roi.cache = Some(&cache);
    let roi = r_roi.comp_frame(cid, Tick::ZERO);

    let mut r_full = Renderer::new(&p, &NoFootage, RenderOpts::default());
    r_full.cache = Some(&cache);
    let full = r_full.comp_frame(cid, Tick::ZERO);

    assert_eq!((roi.width, roi.height), (160, 120));
    assert_same(&roi, &crop(&full, 200, 160, 160, 120));
    // Verify both the ROI-cropped entry and the full-layer entry exist separately in cache.
    assert!(cache.stats().entries >= 2, "expected distinct ROI and full cache entries, got {:?}", cache.stats());
}

/// Off-ROI 2D layers and adjustment layers are culled safely, while stencil layers outside the
/// ROI are NOT culled because they clear pixels outside their own bounds.
#[test]
fn roi_layer_culling_respects_stencil_and_matches_full_render() {
    let mut p = Project::default();
    let comp = Comp::new(320, 240, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let s_bg = p.add_item("BG", Label::Red, None, ItemKind::Solid(Solid { color: [0.2, 0.7, 0.4], width: 320, height: 240, pixel_aspect: 1.0 }));
    let s_off = p.add_item("Off", Label::Blue, None, ItemKind::Solid(Solid { color: [0.9, 0.1, 0.1], width: 60, height: 60, pixel_aspect: 1.0 }));
    let s_in = p.add_item("In", Label::Yellow, None, ItemKind::Solid(Solid { color: [0.9, 0.9, 0.1], width: 80, height: 80, pixel_aspect: 1.0 }));

    let bg = build::layer(&mut p, &comp, "BG", LayerSource::Solid { item: s_bg }, (320, 240), None);
    let mut off = build::layer(&mut p, &comp, "OffROI", LayerSource::Solid { item: s_off }, (60, 60), None);
    off.props.prop_mut("transform/position").unwrap().value = Value::Vec3([30.0, 30.0, 0.0]);
    let mut adj_off = build::layer(&mut p, &comp, "AdjOffROI", LayerSource::Solid { item: s_off }, (60, 60), None);
    adj_off.switches.adjustment = true;
    adj_off.props.prop_mut("transform/position").unwrap().value = Value::Vec3([30.0, 30.0, 0.0]);
    let mut inside = build::layer(&mut p, &comp, "InROI", LayerSource::Solid { item: s_in }, (80, 80), None);
    inside.props.prop_mut("transform/position").unwrap().value = Value::Vec3([240.0, 180.0, 0.0]);

    p.comp_mut(cid).unwrap().layers = vec![inside, adj_off, off, bg].into();
    let roi_box = [180.0, 120.0, 120.0, 100.0];
    let r_roi = Renderer::new(&p, &NoFootage, RenderOpts { roi: Some(roi_box), ..Default::default() });
    let ctx = r_roi.eval_ctx(cid, Tick::ZERO).unwrap();
    assert!(r_roi.layer_outside_roi(&ctx, &ctx.comp.layers[2]), "OffROI layer should be culled");
    assert!(r_roi.layer_outside_roi(&ctx, &ctx.comp.layers[1]), "OffROI adjustment layer should be culled");
    assert!(!r_roi.layer_outside_roi(&ctx, &ctx.comp.layers[0]), "InROI layer must not be culled");

    let full = Renderer::new(&p, &NoFootage, RenderOpts::default()).comp_frame(cid, Tick::ZERO);
    let roi = r_roi.comp_frame(cid, Tick::ZERO);
    assert_same(&roi, &crop(&full, 180, 120, 120, 100));

    // Stencil Alpha outside ROI MUST NOT be culled (it clears the ROI to transparent).
    p.comp_mut(cid).unwrap().layers[2].blend_mode = BlendMode::StencilAlpha;
    let r_roi2 = Renderer::new(&p, &NoFootage, RenderOpts { roi: Some(roi_box), ..Default::default() });
    let ctx2 = r_roi2.eval_ctx(cid, Tick::ZERO).unwrap();
    assert!(!r_roi2.layer_outside_roi(&ctx2, &ctx2.comp.layers[2]), "Stencil layer outside ROI must not be culled");
    let full2 = Renderer::new(&p, &NoFootage, RenderOpts::default()).comp_frame(cid, Tick::ZERO);
    let roi2 = r_roi2.comp_frame(cid, Tick::ZERO);
    assert_same(&roi2, &crop(&full2, 180, 120, 120, 100));
}
