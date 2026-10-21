//! Mask Expansion grows or shrinks the mask outline by its distance, however large: past the
//! shape's curvature radius it must still cover everything within the distance of the path,
//! never leave a ring around an uncovered middle.

use effectcraft_color::Label;
use effectcraft_keyframe::{ShapePath, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Comp, ItemId, ItemKind, LayerSource, MaskMode, Node, Project, Solid};
use effectcraft_time::{FrameRate, Tick};

use crate::{NoFootage, RenderOpts, Renderer};

const W: u32 = 400;
const H: u32 = 300;

/// A white 400×300 solid with one mask of `shape` in `mode` expanded by `expansion` pixels.
fn project(shape: ShapePath, mode: MaskMode, expansion: f64) -> (Project, ItemId) {
    let mut p = Project::default();
    let comp = Comp::new(W, H, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
    let sid = p.add_item("S", Label::Red, None, ItemKind::Solid(Solid { color: [1.0; 3], width: W, height: H, pixel_aspect: 1.0 }));
    let mut l = build::layer(&mut p, &comp, "S", LayerSource::Solid { item: sid }, (W, H), None);
    let mut next = p.next_id;
    let mut m = build::mask(&mut Ids(&mut next), "Mask 1", shape, mode, [255, 0, 0]);
    p.next_id = next;
    m.get_mut("expansion").unwrap().value = Value::Scalar(expansion);
    l.props.sub_mut("masks").unwrap().children.push(Node::Group(m));
    let cid = p.add_item("C", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    p.comp_mut(cid).unwrap().layers.push(l);
    (p, cid)
}

/// Alpha at each of `points` for a Subtract mask on a 40 px circle at the centre (the hole).
fn hole(expansion: f64, points: &[(i64, i64)]) -> Vec<f32> {
    let (p, cid) = project(ShapePath::ellipse([200.0, 150.0], 40.0, 40.0), MaskMode::Subtract, expansion);
    let img = Renderer::new(&p, &NoFootage, RenderOpts::default()).comp_frame(cid, Tick::ZERO);
    points.iter().map(|&(x, y)| img.get(x, y)[3]).collect()
}

#[test]
fn no_expansion_cuts_the_circle() {
    let a = hole(0.0, &[(200, 150), (215, 150), (225, 150)]);
    assert!(a[0] < 0.01 && a[1] < 0.01, "inside the 20 px radius is cut: {a:?}");
    assert!(a[2] > 0.99, "outside it is kept: {a:?}");
}

#[test]
fn expansion_past_the_radius_grows_a_solid_hole() {
    // +60 on a 20 px radius: a solid hole of radius 80. The fill + stroke blend left a ring
    // from 40 to 80 px with the layer showing at 21–39 px.
    let a = hole(60.0, &[(200, 150), (230, 150), (200, 120), (275, 150), (285, 150)]);
    assert!(a[..4].iter().all(|&v| v < 0.01), "everything within 80 px is cut: {a:?}");
    assert!(a[4] > 0.99, "beyond 80 px is kept: {a:?}");
}

#[test]
fn huge_expansion_cuts_the_whole_frame() {
    // +600 reaches every pixel; the blend left only the 20 px circle cut.
    let a = hole(600.0, &[(200, 150), (230, 150), (0, 0), (399, 299), (399, 0)]);
    assert!(a.iter().all(|&v| v < 0.01), "{a:?}");
}

#[test]
fn negative_expansion_shrinks_then_removes_the_hole() {
    let a = hole(-15.0, &[(200, 150), (210, 150)]);
    assert!(a[0] < 0.01, "a 5 px hole remains: {a:?}");
    assert!(a[1] > 0.99, "shrunk away from 10 px out: {a:?}");
    let a = hole(-25.0, &[(200, 150), (190, 150)]);
    assert!(a.iter().all(|&v| v > 0.99), "shrunk past its radius, nothing is cut: {a:?}");
}

#[test]
fn expanded_rect_add_mask_has_round_corners() {
    // A 100 px square mask (Add) grown by 30: the sides move out 30 px, the corners round off.
    let (p, cid) = project(ShapePath::rect([200.0, 150.0], 100.0, 100.0), MaskMode::Add, 30.0);
    let img = Renderer::new(&p, &NoFootage, RenderOpts::default()).comp_frame(cid, Tick::ZERO);
    assert!(img.get(275, 150)[3] > 0.99, "25 px past the right side is inside");
    assert!(img.get(285, 150)[3] < 0.01, "35 px past it is outside");
    // 25 px diagonally out from the corner (250,100) is within 30 px: inside.
    assert!(img.get(267, 82)[3] > 0.99);
    // The square's bounding corner (280,70) is 42 px from the corner: a miter would cover it.
    assert!(img.get(277, 73)[3] < 0.01);
}
