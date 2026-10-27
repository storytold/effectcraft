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
    p.comp_mut(cid).unwrap().layers = layers;
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
