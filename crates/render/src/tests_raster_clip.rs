//! #1388: 3D text and shape layers rasterise only the part of the layer the output shows. Where
//! the whole raster fits its pixel budget the frame is identical to rasterising the whole layer;
//! close to the camera the raster stays small instead of growing to ~16 million pixels.

use effectcraft_color::Label;
use effectcraft_keyframe::{TextDoc, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Comp, ItemId, ItemKind, Layer, LayerSource, Project};
use effectcraft_time::{FrameRate, Tick};

use crate::{NoFootage, RenderOpts, Renderer};

const W: u32 = 400;
const H: u32 = 300;

/// The default camera's distance to the z = 0 plane.
fn zoom() -> f64 {
    W as f64 * 50.0 / 36.0
}

fn setup() -> (Project, ItemId, Comp) {
    let mut p = Project::default();
    p.settings.bit_depth = effectcraft_project::BitDepth::Bpc32;
    let comp = Comp::new(W, H, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    (p, cid, comp)
}

fn set(l: &mut Layer, path: &str, v: Value) {
    l.props.prop_mut(path).unwrap_or_else(|| panic!("no {path}")).value = v;
}

/// A 3D text layer at `pos`, turned `rot_y` degrees about Y.
fn text3(p: &mut Project, comp: &Comp, pos: [f64; 3], rot_y: f64) -> Layer {
    let mut l = build::layer(p, comp, "Text", LayerSource::Text, (W, H), None);
    l.props.prop_mut("text/sourceText").unwrap().value = Value::Text(Box::new(TextDoc { text: "NEAR CAMERA TEXT".into(), size: 60.0, ..Default::default() }));
    l.switches.three_d = true;
    set(&mut l, "transform/position", Value::Vec3(pos));
    set(&mut l, "transform/rotationY", Value::Scalar(rot_y));
    l
}

/// A 3D shape layer (a 300×120 rectangle with a stroke) at `pos`.
fn shape3(p: &mut Project, comp: &Comp, pos: [f64; 3]) -> Layer {
    let mut l = build::layer(p, comp, "Shape", LayerSource::Shape, (W, H), None);
    let mut next = p.next_id;
    {
        let mut ids = Ids(&mut next);
        let r = build::shape_rect(&mut ids, [300.0, 120.0], [0.0, 0.0], 0.0);
        let f = build::shape_fill(&mut ids, [0.9, 0.4, 0.1, 1.0]);
        let s = build::shape_stroke(&mut ids, [1.0, 1.0, 1.0, 1.0], 6.0);
        let g = build::shape_group(&mut ids, "Box", vec![r, f, s]);
        l.props.sub_mut("contents").unwrap().children.push(g.into());
    }
    p.next_id = next;
    l.switches.three_d = true;
    set(&mut l, "transform/position", Value::Vec3(pos));
    l
}

fn scene(l: impl FnOnce(&mut Project, &Comp) -> Layer) -> (Project, ItemId) {
    let (mut p, cid, comp) = setup();
    let layer = l(&mut p, &comp);
    p.comp_mut(cid).unwrap().layers.push(layer);
    (p, cid)
}

fn frame(p: &Project, cid: ItemId, whole: bool) -> effectcraft_raster::Image {
    let mut r = Renderer::new(p, &NoFootage, RenderOpts::default());
    r.whole_raster = whole;
    r.comp_frame(cid, Tick::ZERO)
}

/// Pixels of the layer's processed buffer.
fn raster_px(p: &Project, cid: ItemId, whole: bool) -> u64 {
    let mut r = Renderer::new(p, &NoFootage, RenderOpts::default());
    r.whole_raster = whole;
    let c = p.comp(cid).unwrap();
    let ctx = r.eval_ctx(cid, Tick::ZERO).unwrap();
    let b = r.layer_buf(&ctx, &c.layers[0]).unwrap();
    b.img.width as u64 * b.img.height as u64
}

#[test]
fn the_visible_part_renders_the_same_frame_as_the_whole_layer() {
    let cx = W as f64 / 2.0;
    let cy = H as f64 / 2.0;
    let cases: Vec<(&str, (Project, ItemId))> = vec![
        ("far", scene(|p, c| text3(p, c, [cx, cy, 300.0], 0.0))),
        ("in the frame plane", scene(|p, c| text3(p, c, [cx, cy, 0.0], 0.0))),
        ("past the frame edge", scene(|p, c| text3(p, c, [60.0, cy, -200.0], 0.0))),
        ("turned", scene(|p, c| text3(p, c, [cx, cy, -150.0], 55.0))),
        ("close to the camera", scene(|p, c| text3(p, c, [cx, cy, -zoom() + 40.0], 0.0))),
        ("partly behind the camera", scene(|p, c| text3(p, c, [cx, cy, -zoom() + 30.0], 80.0))),
        ("behind the camera", scene(|p, c| text3(p, c, [cx, cy, -zoom() - 200.0], 0.0))),
        ("shape", scene(|p, c| shape3(p, c, [cx + 150.0, cy, -250.0]))),
    ];
    for (label, (p, cid)) in &cases {
        let clipped = frame(p, *cid, false);
        let whole = frame(p, *cid, true);
        assert_eq!(clipped.data, whole.data, "{label}");
    }
}

/// Close to the camera only part of the text shows: its raster is a fraction of the whole one
/// (millions of pixels), and the frame still shows text.
#[test]
fn text_close_to_the_camera_rasterises_what_shows() {
    let (p, cid) = scene(|p, c| text3(p, c, [W as f64 / 2.0, H as f64 / 2.0, -zoom() + 40.0], 0.0));
    let clipped = raster_px(&p, cid, false);
    let whole = raster_px(&p, cid, true);
    assert!(whole > 5_000_000, "the whole raster is huge ({whole} px)");
    eprintln!("near camera: {clipped} px rasterised, {whole} px whole");
    assert!(clipped * 10 < whole, "{clipped} px of {whole}");
    let img = frame(&p, cid, false);
    let covered = img.data.iter().filter(|px| px[3] > 0.5).count();
    assert!(covered > (W * H / 20) as usize, "text shows ({covered} px)");
    // Behind the camera there is nothing to rasterise.
    let (p, cid) = scene(|p, c| text3(p, c, [W as f64 / 2.0, H as f64 / 2.0, -zoom() - 200.0], 0.0));
    assert!(raster_px(&p, cid, false) <= 1);
}

/// The #1388 case at full size: 200 pt text in a 1920 × 1080 comp, close to the camera. Rasterised
/// whole it fills the ~16 million pixel cap; clipped, it stays within a few frames' worth of pixels
/// (and the text still shows).
#[test]
fn hd_text_close_to_the_camera_stays_within_a_few_frames() {
    let mut p = Project::default();
    let comp = Comp::new(1920, 1080, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let mut l = build::layer(&mut p, &comp, "Text", LayerSource::Text, (1920, 1080), None);
    l.props.prop_mut("text/sourceText").unwrap().value = Value::Text(Box::new(TextDoc { text: "NEAR CAMERA".into(), size: 200.0, ..Default::default() }));
    l.switches.three_d = true;
    set(&mut l, "transform/position", Value::Vec3([960.0, 540.0, -1920.0 * 50.0 / 36.0 + 60.0]));
    p.comp_mut(cid).unwrap().layers.push(l);
    let (clipped, whole) = (raster_px(&p, cid, false), raster_px(&p, cid, true));
    eprintln!("HD near camera: {clipped} px rasterised, {whole} px whole");
    assert!(whole > 12_000_000, "{whole}");
    assert!(clipped <= 4 * 1920 * 1080, "{clipped} px");
    assert!(frame(&p, cid, false).data.iter().any(|px| px[3] > 0.5), "the text shows");
}

/// A slow move of the layer against the camera (a camera move does the same) keeps the part
/// rasterised within the same grid cells for several frames: they reuse the cached raster instead
/// of drawing the text again each frame.
#[test]
fn a_slow_move_reuses_the_clipped_raster() {
    let cache = crate::LayerCache::default();
    let at = |x: f64| {
        let (p, cid) = scene(|p, c| text3(p, c, [x, H as f64 / 2.0, -zoom() + 40.0], 0.0));
        let mut r = Renderer::new(&p, &NoFootage, RenderOpts::default());
        r.cache = Some(&cache);
        r.comp_frame(cid, Tick::ZERO);
    };
    at(W as f64 / 2.0);
    let misses = cache.stats().misses;
    // 16 frames, a quarter of a layer pixel apart (here about three output pixels).
    for i in 1..=16 {
        at(W as f64 / 2.0 + i as f64 * 0.25);
    }
    let rasters = cache.stats().misses - misses;
    assert!(rasters <= 2, "{rasters} new rasters over 16 frames");
}

/// One keyframed motion: (seconds, value) pairs.
type Keys<'a> = &'a [(f64, [f64; 3])];

fn keyed(l: &mut Layer, path: &str, keys: Keys) {
    l.props.prop_mut(path).unwrap_or_else(|| panic!("no {path}")).keys =
        keys.iter().map(|(t, v)| effectcraft_keyframe::Keyframe::new(Tick::from_seconds_f64(*t), Value::Vec3(*v))).collect();
}

/// A camera keyed through `pos` / `poi`, and the text keyed through `text_pos` (and turning about
/// Y through `text_rot`): every frame of a second, rendered with the clipped raster and a layer
/// cache shared across the frames, matches rasterising the whole layer without a cache (or differs
/// by an anti-aliasing sub-sample or two on a few edge pixels) — except where the whole raster is held at
/// its pixel cap (softer than the clipped one, which isn't): there the text shows and the clipped
/// raster stays small.
fn moving(label: &str, pos: Keys, poi: Keys, text_pos: Keys, text_rot: &[(f64, f64)]) {
    let (mut p, cid, comp) = setup();
    let mut text = text3(&mut p, &comp, [W as f64 / 2.0, H as f64 / 2.0, 0.0], 0.0);
    if !text_pos.is_empty() {
        keyed(&mut text, "transform/position", text_pos);
    }
    if !text_rot.is_empty() {
        text.props.prop_mut("transform/rotationY").unwrap().keys =
            text_rot.iter().map(|(t, v)| effectcraft_keyframe::Keyframe::new(Tick::from_seconds_f64(*t), Value::Scalar(*v))).collect();
    }
    let mut cam = build::layer(&mut p, &comp, "Camera", LayerSource::Camera, (W, H), None);
    set(&mut cam, "cameraOptions/zoom", Value::Scalar(zoom()));
    keyed(&mut cam, "transform/position", pos);
    keyed(&mut cam, "transform/poi", poi);
    p.comp_mut(cid).unwrap().layers = vec![cam, text];
    let cache = crate::LayerCache::default();
    let (mut exact, mut near, mut capped) = (0, 0, 0);
    for f in 0..30 {
        let t = Tick::from_seconds_f64(f as f64 / 30.0);
        let mut clipped = Renderer::new(&p, &NoFootage, RenderOpts::default());
        clipped.cache = Some(&cache);
        let mut whole = Renderer::new(&p, &NoFootage, RenderOpts::default());
        whole.whole_raster = true;
        let ctx = whole.eval_ctx(cid, t).unwrap();
        let layer = &p.comp(cid).unwrap().layers[1];
        let a = clipped.comp_frame(cid, t);
        if clipped.raster_scale(&ctx, layer) == whole.raster_scale(&ctx, layer) {
            let b = whole.comp_frame(cid, t);
            if a.data == b.data {
                exact += 1;
            } else {
                // The clipped raster places the glyphs at other coordinates (smaller ones: the whole
                // raster can be thousands of pixels wide), and tiny-skia transforms them in f32: its
                // 4 × 4 anti-aliasing may land a sub-sample or two differently on a few edge pixels.
                let (n, worst) = a.data.iter().zip(&b.data).fold((0usize, 0.0f32), |(n, w), (p, q)| {
                    let d = (0..4).map(|i| (p[i] - q[i]).abs()).fold(0.0f32, f32::max);
                    (n + usize::from(d > 0.0), w.max(d))
                });
                assert!(worst <= 2.0 / 16.0 + 1e-4 && n * 1000 <= (W * H) as usize, "{label}: frame {f}: {n} px differ, by up to {worst}");
                near += 1;
            }
        } else {
            // Held at the cap whole: the clipped raster is sharper, and small.
            let buf = clipped.layer_buf(&ctx, layer).unwrap();
            assert!(buf.img.width as u64 * buf.img.height as u64 <= 16_500_000, "{label}: frame {f}");
            let shows = |img: &effectcraft_raster::Image| img.data.iter().filter(|px| px[3] > 0.5).count();
            let (sa, sb) = (shows(&a), shows(&whole.comp_frame(cid, t)));
            // The same text shows (or none, the camera past it); coverage within a few percent.
            assert!(sa.abs_diff(sb) * 50 <= sb.max(1) + 50, "{label}: frame {f}: {sa} px of text, {sb} rasterised whole");
            capped += 1;
        }
    }
    eprintln!("{label}: {exact} frames identical, {near} within anti-aliasing noise, {capped} sharper than the capped whole raster");
    assert!(exact + near >= 10, "{label}: {exact} identical, {near} close, {capped} at the cap");
}

#[test]
fn a_camera_moving_any_way_renders_every_frame_as_the_whole_layer() {
    let (cx, cy, z) = (W as f64 / 2.0, H as f64 / 2.0, -zoom());
    let still = [(0.0, [cx, cy, 0.0])];
    // In towards the text, through it and past it; and back out.
    moving("dolly in through the text", &[(0.0, [cx, cy, 2.0 * z]), (1.0, [cx, cy, 150.0])], &[(0.0, [cx, cy, 400.0]), (1.0, [cx, cy, 700.0])], &[], &[]);
    moving("dolly out", &[(0.0, [cx, cy, -60.0]), (1.0, [cx, cy, 3.0 * z])], &still, &[], &[]);
    // Sideways and up and down close to the text, so it leaves the frame on both sides.
    moving(
        "pan x",
        &[(0.0, [cx - 500.0, cy, -120.0]), (1.0, [cx + 500.0, cy, -120.0])],
        &[(0.0, [cx - 500.0, cy, 0.0]), (1.0, [cx + 500.0, cy, 0.0])],
        &[],
        &[],
    );
    moving("pan y", &[(0.0, [cx, cy - 300.0, -90.0]), (1.0, [cx, cy + 300.0, -90.0])], &[(0.0, [cx, cy - 300.0, 0.0]), (1.0, [cx, cy + 300.0, 0.0])], &[], &[]);
    // Around the text, close, looking at it.
    let orbit: Vec<(f64, [f64; 3])> = (0..=8)
        .map(|i| {
            let a = i as f64 / 8.0 * std::f64::consts::TAU * 0.4 - 0.8;
            (i as f64 / 8.0, [cx + 150.0 * a.sin(), cy, -150.0 * a.cos()])
        })
        .collect();
    moving("orbit", &orbit, &still, &[], &[]);
    // The text moves instead: towards the camera, past it, and across.
    moving(
        "text towards and past the camera",
        &[(0.0, [cx, cy, z])],
        &still,
        &[(0.0, [cx + 200.0, cy - 80.0, 400.0]), (1.0, [cx - 150.0, cy + 60.0, z - 100.0])],
        &[],
    );
    // Turning close to the camera.
    moving("text turning near the camera", &[(0.0, [cx, cy, z])], &still, &[(0.0, [cx, cy, z + 70.0])], &[(0.0, -70.0), (1.0, 70.0)]);
}

/// Reads of a layer in its own space get the whole layer, not the part the frame shows: an effect
/// taking another layer as a parameter (Set Matte, stretched to fit, from hidden text close to the
/// camera), and the source the motion tracker analyses.
#[test]
fn layer_space_reads_get_the_whole_layer() {
    let (mut p, cid, comp) = setup();
    let mut text = text3(&mut p, &comp, [W as f64 / 2.0, H as f64 / 2.0, -zoom() + 40.0], 0.0);
    text.switches.video = false;
    let text_id = text.id;
    let sid =
        p.add_item("Solid", Label::Red, None, ItemKind::Solid(effectcraft_project::Solid { color: [0.2, 0.6, 1.0], width: W, height: H, pixel_aspect: 1.0 }));
    let mut solid = build::layer(&mut p, &comp, "Solid", LayerSource::Solid { item: sid }, (W, H), None);
    let spec = effectcraft_effects::find("ec.channel.setmatte").unwrap();
    let mut next = p.next_id;
    let mut g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [W as f64, H as f64]);
    p.next_id = next;
    g.prop_mut("takeMatteFromLayer").unwrap().value = Value::Layer(Some(text_id.0));
    solid.props.sub_mut("effects").unwrap().children.push(g.into());
    p.comp_mut(cid).unwrap().layers = vec![text, solid];
    assert_eq!(frame(&p, cid, false).data, frame(&p, cid, true).data, "Set Matte from the text");
    let r = Renderer::new(&p, &NoFootage, RenderOpts::default());
    let ctx = r.eval_ctx(cid, Tick::ZERO).unwrap();
    let layer = &p.comp(cid).unwrap().layers[0];
    let mut whole = Renderer::new(&p, &NoFootage, RenderOpts::default());
    whole.whole_raster = true;
    assert_eq!(r.layer_source(&ctx, layer).unwrap().img, whole.layer_source(&ctx, layer).unwrap().img, "the tracker's source");
}

/// A layer whose pixels outside the frame can matter (an effect here) is rasterised whole.
#[test]
fn layers_with_effects_rasterise_whole() {
    let (mut p, cid) = scene(|p, c| text3(p, c, [W as f64 / 2.0, H as f64 / 2.0, -zoom() + 40.0], 0.0));
    let spec = effectcraft_effects::find("ec.blur.gaussian").unwrap();
    let mut next = p.next_id;
    let g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [W as f64, H as f64]);
    p.next_id = next;
    p.comp_mut(cid).unwrap().layers[0].props.sub_mut("effects").unwrap().children.push(g.into());
    assert_eq!(raster_px(&p, cid, false), raster_px(&p, cid, true));
}
