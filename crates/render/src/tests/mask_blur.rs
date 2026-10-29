//! Mask motion blur follows the renderer's motion blur gate: the render options (Render
//! Settings ▸ Motion Blur, previews), the comp's switch and the precomp layers above.

use effectcraft_color::Label;
use effectcraft_keyframe::{Keyframe, ShapePath, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Comp, GroupKind, ItemId, ItemKind, LayerSource, MaskMode, MaskMotionBlur, Node, Project, Solid};
use effectcraft_time::{FrameRate, Tick};

use crate::{NoFootage, RenderOpts, Renderer};

/// A white 100×100 solid whose rect mask jumps from x 10 (frame 0) to x 50 (frame 1), Mask
/// Motion Blur On, the layer's own switch off. Rendered half way, unblurred the mask covers x
/// 30..50; blurred over the shutter it reaches down to x ≈ 20.
fn project(comp_blur: bool) -> (Project, ItemId) {
    let mut p = Project::default();
    let mut comp = Comp::new(100, 100, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
    comp.enable_motion_blur = comp_blur;
    let sid = p.add_item("S", Label::Red, None, ItemKind::Solid(Solid { color: [1.0; 3], width: 100, height: 100, pixel_aspect: 1.0 }));
    let mut l = build::layer(&mut p, &comp, "S", LayerSource::Solid { item: sid }, (100, 100), None);
    let mut next = p.next_id;
    let mut m = build::mask(&mut Ids(&mut next), "Mask 1", ShapePath::rect([20.0, 50.0], 20.0, 100.0), MaskMode::Add, [255, 0, 0]);
    p.next_id = next;
    if let GroupKind::Mask { motion_blur, .. } = &mut m.kind {
        *motion_blur = MaskMotionBlur::On;
    }
    let path = m.get_mut("path").unwrap();
    path.keys = vec![
        Keyframe::new(Tick::ZERO, Value::Path(ShapePath::rect([20.0, 50.0], 20.0, 100.0))),
        Keyframe::new(FrameRate::FPS_30.tick_of(1), Value::Path(ShapePath::rect([60.0, 50.0], 20.0, 100.0))),
    ];
    l.props.sub_mut("masks").unwrap().children.push(Node::Group(m));
    l.switches.motion_blur = false;
    let cid = p.add_item("C", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    p.comp_mut(cid).unwrap().layers.push(l);
    (p, cid)
}

fn alpha_at_25(p: &Project, cid: ItemId, motion_blur: bool) -> f32 {
    let half = Tick::from_seconds_f64(0.5 / 30.0);
    Renderer::new(p, &NoFootage, RenderOpts { motion_blur, ..Default::default() }).comp_frame(cid, half).get(25, 50)[3]
}

/// A mask that holds from frame 0 to 2, then moves: frames 1 and 2 have the same mask value, but at
/// frame 2 the shutter already sees it move. The layer cache must not serve frame 1's layer there.
#[test]
fn mask_motion_blur_keys_the_layer_cache_by_time() {
    let (mut p, cid) = project(true);
    let rect = |x: f64| Value::Path(ShapePath::rect([x, 50.0], 20.0, 100.0));
    let l = &mut p.comp_mut(cid).unwrap().layers[0];
    let Some(Node::Group(m)) = l.props.sub_mut("masks").unwrap().children.first_mut() else { panic!("mask") };
    m.get_mut("path").unwrap().keys = vec![
        Keyframe::new(Tick::ZERO, rect(20.0)),
        Keyframe::new(FrameRate::FPS_30.tick_of(2), rect(20.0)),
        Keyframe::new(FrameRate::FPS_30.tick_of(3), rect(60.0)),
    ];
    let cache = crate::LayerCache::default();
    let render = |t: Tick, cached: bool| {
        let mut r = Renderer::new(&p, &NoFootage, RenderOpts { motion_blur: true, ..Default::default() });
        r.cache = cached.then_some(&cache);
        r.comp_frame(cid, t)
    };
    let (f1, f2) = (FrameRate::FPS_30.tick_of(1), FrameRate::FPS_30.tick_of(2));
    assert_ne!(render(f1, false), render(f2, false), "the shutter at frame 2 sees the move");
    render(f1, true);
    assert_eq!(render(f2, true), render(f2, false));
}

#[test]
fn mask_motion_blur_follows_the_render_and_the_comp() {
    let (p, cid) = project(true);
    let a = alpha_at_25(&p, cid, true);
    assert!(a > 0.05 && a < 0.95, "blurred: {a}");
    assert!(alpha_at_25(&p, cid, false) < 0.01, "Render Settings ▸ Motion Blur off");
    let (p, cid) = project(false);
    assert!(alpha_at_25(&p, cid, true) < 0.01, "the comp's switch off");
}
