//! Nested compositions: Preserve frame rate when nested or in render queue, Preserve resolution
//! when nested.

use effectcraft_color::Label;
use effectcraft_keyframe::{Keyframe, Value};
use effectcraft_project::build;
use effectcraft_project::render_queue::RenderSettings;
use effectcraft_project::{Comp, ItemId, ItemKind, LayerSource, Project, Solid};
use effectcraft_time::{FrameRate, Tick};

use crate::render_frame;

fn s(x: f64) -> Tick {
    Tick::from_seconds_f64(x)
}

/// A 30 fps outer comp nesting a 10 fps 100×100 comp whose 10×10 solid moves from x=10 (0 s)
/// to x=90 (2 s). Returns (project, outer, inner).
fn nested(preserve_rate: bool, preserve_res: bool) -> (Project, ItemId, ItemId) {
    let mut p = Project::default();
    let outer = Comp::new(100, 100, FrameRate::FPS_30, s(4.0));
    let mut inner = Comp::new(100, 100, FrameRate::from_f64(10.0), s(4.0));
    inner.preserve_frame_rate = preserve_rate;
    inner.preserve_resolution = preserve_res;
    let sid = p.add_item("S", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 1.0, 1.0], width: 10, height: 10, pixel_aspect: 1.0 }));
    let mut l = build::layer(&mut p, &inner, "S", LayerSource::Solid { item: sid }, (10, 10), None);
    l.props.prop_mut("transform/position").unwrap().keys =
        vec![Keyframe::new(Tick::ZERO, Value::Vec3([10.0, 50.0, 0.0])), Keyframe::new(s(2.0), Value::Vec3([90.0, 50.0, 0.0]))];
    inner.layers.push(l);
    let iid = p.add_item("Inner", Label::Sandstone, None, ItemKind::Comp(inner.into()));
    let cid = p.add_item("Outer", Label::Sandstone, None, ItemKind::Comp(outer.clone().into()));
    let pre = build::layer(&mut p, &outer, "Inner", LayerSource::Comp { item: iid }, (100, 100), None);
    p.comp_mut(cid).unwrap().layers.push(pre);
    (p, cid, iid)
}

#[test]
fn a_comp_preserving_its_frame_rate_shows_only_its_own_frames_when_nested() {
    // 2/30 s falls between the nested comp's frames 0 (0 s) and 1 (0.1 s).
    let between = s(2.0 / 30.0);
    let (p, cid, _) = nested(false, false);
    assert_ne!(render_frame(&p, cid, between, 1.0).data, render_frame(&p, cid, Tick::ZERO, 1.0).data, "follows the outer comp's frames");
    let (p, cid, _) = nested(true, false);
    assert_eq!(render_frame(&p, cid, between, 1.0).data, render_frame(&p, cid, Tick::ZERO, 1.0).data, "holds its own frame 0");
    assert_ne!(render_frame(&p, cid, s(0.1), 1.0).data, render_frame(&p, cid, Tick::ZERO, 1.0).data, "then its frame 1");
}

#[test]
fn the_render_queue_keeps_a_preserved_frame_rate() {
    let (p, _, iid) = nested(true, false);
    let settings = RenderSettings { frame_rate: Some(FrameRate::FPS_30), ..Default::default() };
    assert_eq!(settings.rate(p.comp(iid).unwrap()), FrameRate::from_f64(10.0));
    let (p, _, iid) = nested(false, false);
    assert_eq!(settings.rate(p.comp(iid).unwrap()), FrameRate::FPS_30);
}

#[test]
fn a_comp_preserving_its_resolution_renders_full_size_and_lands_in_place() {
    for preserve in [false, true] {
        let (p, cid, _) = nested(false, preserve);
        let img = render_frame(&p, cid, Tick::ZERO, 0.5);
        assert_eq!((img.width, img.height), (50, 50));
        // The solid covers x 5..15, y 45..55 in the 100×100 comp: 2.5..7.5, 22.5..27.5 at half size.
        assert!(img.get(5, 25)[3] > 0.99, "preserve {preserve}: inside");
        assert!(img.get(20, 25)[3] < 0.01 && img.get(5, 40)[3] < 0.01, "preserve {preserve}: outside");
    }
}

#[test]
fn a_nested_comp_shows_nothing_past_its_end() {
    for collapse in [false, true] {
        let mut p = Project::default();
        let outer = Comp::new(100, 100, FrameRate::FPS_30, s(4.0));
        // A 1 s comp with a solid covering it for 4 s (longer than the comp itself).
        let mut inner = Comp::new(100, 100, FrameRate::FPS_30, s(1.0));
        let sid = p.add_item("S", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 1.0, 1.0], width: 100, height: 100, pixel_aspect: 1.0 }));
        let mut l = build::layer(&mut p, &inner, "S", LayerSource::Solid { item: sid }, (100, 100), None);
        l.out_point = s(4.0);
        inner.layers.push(l);
        let iid = p.add_item("Inner", Label::Sandstone, None, ItemKind::Comp(inner.into()));
        let cid = p.add_item("Outer", Label::Sandstone, None, ItemKind::Comp(outer.clone().into()));
        // The precomp layer is trimmed out to 3 s, past the nested comp's end.
        let mut pre = build::layer(&mut p, &outer, "Inner", LayerSource::Comp { item: iid }, (100, 100), None);
        pre.out_point = s(3.0);
        pre.switches.collapse = collapse;
        p.comp_mut(cid).unwrap().layers.push(pre);
        assert!(render_frame(&p, cid, s(0.5), 1.0).get(50, 50)[3] > 0.99, "collapse {collapse}: inside its span");
        assert!(render_frame(&p, cid, s(2.0), 1.0).get(50, 50)[3] < 0.01, "collapse {collapse}: past its end");
    }
}
