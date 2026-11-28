//! Edge Tile Repeat (CC RepeTile) repeats a layer's content around itself (#3866).
//!
//! On a shape or text layer the copies must surround the rendered content. Tiling the
//! comp-sized layer rectangle instead puts every copy a comp width or height away, outside
//! the frame, so the effect appeared to do nothing.

use super::*;
use crate::render_frame;

/// A comp `comp` big with a `rect`-sized red rectangle centred at `pos`, as a shape (`shape`) or
/// a solid, with one Edge Tile Repeat per entry in `expands`, each expanding that many px on
/// every side. `extra` adds one more effect after them.
fn scene(comp: (u32, u32), rect: (f64, f64), pos: (f64, f64), shape: bool, expands: &[f64], extra: Option<&str>) -> (Project, ItemId) {
    let mut p = Project::default();
    p.settings.bit_depth = effectcraft_project::BitDepth::Bpc32;
    let c = Comp::new(comp.0, comp.1, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(c.clone().into()));
    let mut l = if shape {
        let mut l = build::layer(&mut p, &c, "Shape Layer 1", LayerSource::Shape, comp, None);
        let mut next = p.next_id;
        {
            let mut ids = Ids(&mut next);
            let r = build::shape_rect(&mut ids, [rect.0, rect.1], [0.0, 0.0], 0.0);
            let fill = build::shape_fill(&mut ids, [1.0, 0.0, 0.0, 1.0]);
            let g = build::shape_group(&mut ids, "Rectangle 1", vec![r, fill]);
            l.props.sub_mut("contents").unwrap().children.push(g.into());
        }
        p.next_id = next;
        l
    } else {
        let sid = p.add_item(
            "Solid",
            Label::Red,
            None,
            ItemKind::Solid(Solid { color: [1.0, 0.0, 0.0], width: rect.0 as u32, height: rect.1 as u32, pixel_aspect: 1.0 }),
        );
        build::layer(&mut p, &c, "Solid", LayerSource::Solid { item: sid }, (rect.0 as u32, rect.1 as u32), None)
    };
    l.props.prop_mut("transform/position").unwrap().value = Value::Vec3([pos.0, pos.1, 0.0]);
    for &expand in expands {
        add_effect(
            &mut p,
            &mut l,
            "ec.stylize.ccrepetile",
            [f64::from(comp.0), f64::from(comp.1)],
            &[
                ("expandRight", Value::Scalar(expand)),
                ("expandLeft", Value::Scalar(expand)),
                ("expandDown", Value::Scalar(expand)),
                ("expandUp", Value::Scalar(expand)),
            ],
        );
    }
    if let Some(id) = extra {
        add_effect(&mut p, &mut l, id, [f64::from(comp.0), f64::from(comp.1)], &[]);
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
    // The copies may reach past the comp, which clips them.
    let (max_x, max_y) = (i64::from(tiled.width) - 1, i64::from(tiled.height) - 1);
    for (got, want, max) in [(a, x0 - expand, max_x), (b, y0 - expand, max_y), (c, x1 + expand, max_x), (d, y1 + expand, max_y)] {
        let want = want.clamp(0, max);
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
    let (p, cid) = scene((1920, 1080), (500.0, 800.0), (960.0, 540.0), true, &[], None);
    let plain = render_frame(&p, cid, Tick::ZERO, 1.0);
    let (q, cid) = scene((1920, 1080), (500.0, 800.0), (960.0, 540.0), true, &[100.0], None);
    let tiled = render_frame(&q, cid, Tick::ZERO, 1.0);
    assert_expanded(&plain, &tiled, 100);
}

/// A sized source already tiled from its own bounds; the content-bounds fix must not change it.
#[test]
fn repetile_still_tiles_a_solid_around_its_source_bounds() {
    let (p, cid) = scene((1920, 1080), (500.0, 800.0), (960.0, 540.0), false, &[], None);
    let plain = render_frame(&p, cid, Tick::ZERO, 1.0);
    let (q, cid) = scene((1920, 1080), (500.0, 800.0), (960.0, 540.0), false, &[100.0], None);
    let tiled = render_frame(&q, cid, Tick::ZERO, 1.0);
    assert_expanded(&plain, &tiled, 100);
}

/// The content may be larger than the comp in both dimensions: the copies then land outside the
/// frame, but the layer keeps its content. The rectangle sits right of the comp's centre, so the
/// comp-sized layer rectangle falls outside the content-sized buffer; measuring the buffer
/// against that rectangle (the old test) left the content unmeasured, so the tiles sampled the
/// wrong part of the buffer and the rectangle moved.
#[test]
fn repetile_keeps_a_shape_larger_than_the_comp_visible() {
    let (p, cid) = scene((400, 300), (500.0, 800.0), (450.0, 150.0), true, &[], None);
    let plain = render_frame(&p, cid, Tick::ZERO, 1.0);
    let (q, cid) = scene((400, 300), (500.0, 800.0), (450.0, 150.0), true, &[100.0], None);
    let tiled = render_frame(&q, cid, Tick::ZERO, 1.0);
    let (x0, y0, x1, y1) = bbox(&plain);
    assert!(x1 > x0 && y1 > y0, "the plain rectangle is painted: {x0}..{x1} / {y0}..{y1}");
    // The rectangle keeps its place: everywhere the plain frame shows it, the tiled frame shows
    // the same pixels.
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (a, b) = (plain.get(x, y), tiled.get(x, y));
            for ch in 0..4 {
                assert!((a[ch] - b[ch]).abs() <= 1e-4, "pixel ({x}, {y}) channel {ch}: plain {}, tiled {}", a[ch], b[ch]);
            }
        }
    }
    // The expansion reaches exactly one expansion left of the content.
    let (a, _, _, _) = bbox(&tiled);
    assert!((a - (x0 - 100)).abs() <= 2, "left edge {a}, the content starts at {x0} and expands 100");
}

/// Two Edge Tile Repeat effects stack: the second repeats the first one's output, so the frame
/// grows by both expansions. Reusing the first effect's content measurement tiles the original
/// content instead and drops the outer ring.
#[test]
fn repetile_stacks_by_repeating_the_previous_output() {
    // A 500×700 rectangle keeps both expansions (300 px in total) inside the 1920×1080 frame.
    let (p, cid) = scene((1920, 1080), (500.0, 700.0), (960.0, 540.0), true, &[], None);
    let plain = render_frame(&p, cid, Tick::ZERO, 1.0);
    let (q, cid) = scene((1920, 1080), (500.0, 700.0), (960.0, 540.0), true, &[100.0, 50.0], None);
    let tiled = render_frame(&q, cid, Tick::ZERO, 1.0);
    assert_expanded(&plain, &tiled, 150);
}

/// A non-skipping effect elsewhere in the stack forces the renderer's cover; Edge Tile Repeat
/// must still repeat the painted content, not the covered comp-sized layer rectangle (#3866).
#[test]
fn repetile_measures_content_when_another_effect_covers_the_layer() {
    let extra = Some("ec.channel.invert");
    let (p, cid) = scene((1920, 1080), (500.0, 800.0), (960.0, 540.0), true, &[], extra);
    let plain = render_frame(&p, cid, Tick::ZERO, 1.0);
    let (q, cid) = scene((1920, 1080), (500.0, 800.0), (960.0, 540.0), true, &[100.0], extra);
    let tiled = render_frame(&q, cid, Tick::ZERO, 1.0);
    assert_expanded(&plain, &tiled, 100);
}
