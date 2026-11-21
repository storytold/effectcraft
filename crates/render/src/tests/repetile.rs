//! Edge Tile Repeat (CC RepeTile) repeats a layer's content around itself (#3866).
//!
//! On a shape or text layer the copies must surround the rendered content. Tiling the
//! comp-sized layer rectangle instead puts every copy a comp width or height away, outside
//! the frame, so the effect appeared to do nothing.

use super::*;
use crate::render_frame;

const W: u32 = 1920;
const H: u32 = 1080;

/// A 500×800 red rectangle centred in the comp, as a shape (`shape`) or a solid, with Edge
/// Tile Repeat expanding 100 px on every side when `effect`.
fn scene(shape: bool, effect: bool) -> (Project, ItemId) {
    let mut p = Project::default();
    p.settings.bit_depth = effectcraft_project::BitDepth::Bpc32;
    let comp = Comp::new(W, H, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let mut l = if shape {
        let mut l = build::layer(&mut p, &comp, "Shape Layer 1", LayerSource::Shape, (W, H), None);
        let mut next = p.next_id;
        {
            let mut ids = Ids(&mut next);
            let rect = build::shape_rect(&mut ids, [500.0, 800.0], [0.0, 0.0], 0.0);
            let fill = build::shape_fill(&mut ids, [1.0, 0.0, 0.0, 1.0]);
            let g = build::shape_group(&mut ids, "Rectangle 1", vec![rect, fill]);
            l.props.sub_mut("contents").unwrap().children.push(g.into());
        }
        p.next_id = next;
        l
    } else {
        let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 0.0, 0.0], width: 500, height: 800, pixel_aspect: 1.0 }));
        build::layer(&mut p, &comp, "Solid", LayerSource::Solid { item: sid }, (500, 800), None)
    };
    l.props.prop_mut("transform/position").unwrap().value = Value::Vec3([960.0, 540.0, 0.0]);
    if effect {
        add_effect(
            &mut p,
            &mut l,
            "ec.stylize.ccrepetile",
            [f64::from(W), f64::from(H)],
            &[
                ("expandRight", Value::Scalar(100.0)),
                ("expandLeft", Value::Scalar(100.0)),
                ("expandDown", Value::Scalar(100.0)),
                ("expandUp", Value::Scalar(100.0)),
            ],
        );
    }
    p.comp_mut(cid).unwrap().layers.push(l);
    (p, cid)
}

/// Bounds of the painted pixels (`alpha > 0.5`).
fn bbox(img: &crate::Image) -> (i64, i64, i64, i64) {
    let (mut x0, mut y0, mut x1, mut y1) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
    for y in 0..i64::from(img.height) {
        for x in 0..i64::from(img.width) {
            if img.get(x, y)[3] > 0.5 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    (x0, y0, x1, y1)
}

/// The expanded frame: the plain bounds grown by the expansion on every side, with the new
/// ring filled by copies of the content.
fn assert_expanded(plain: &crate::Image, tiled: &crate::Image, expand: i64) {
    let (x0, y0, x1, y1) = bbox(plain);
    assert!(x1 > x0 && y1 > y0, "the rectangle is painted: {x0}..{x1} / {y0}..{y1}");
    let (a, b, c, d) = bbox(tiled);
    for (got, want) in [(a, x0 - expand), (b, y0 - expand), (c, x1 + expand), (d, y1 + expand)] {
        assert!((got - want).abs() <= 2, "bounds {a}..{c} / {b}..{d}, plain {x0}..{x1} / {y0}..{y1}");
    }
    // The middle of each new ring is a copy of the opaque content.
    let (cx, cy) = ((a + c) / 2, (b + d) / 2);
    for (x, y) in [(a + expand / 2, cy), (c - expand / 2, cy), (cx, b + expand / 2), (cx, d - expand / 2)] {
        assert!(tiled.get(x, y)[3] > 0.99, "copy at ({x}, {y}) is transparent: {:?}", tiled.get(x, y));
    }
}

#[test]
fn repetile_repeats_a_shape_layers_content_around_itself() {
    let (p, cid) = scene(true, false);
    let plain = render_frame(&p, cid, Tick::ZERO, 1.0);
    let (q, cid) = scene(true, true);
    let tiled = render_frame(&q, cid, Tick::ZERO, 1.0);
    assert_expanded(&plain, &tiled, 100);
}

/// A sized source already tiled from its own bounds; the content-bounds fix must not change it.
#[test]
fn repetile_still_tiles_a_solid_around_its_source_bounds() {
    let (p, cid) = scene(false, false);
    let plain = render_frame(&p, cid, Tick::ZERO, 1.0);
    let (q, cid) = scene(false, true);
    let tiled = render_frame(&q, cid, Tick::ZERO, 1.0);
    assert_expanded(&plain, &tiled, 100);
}
