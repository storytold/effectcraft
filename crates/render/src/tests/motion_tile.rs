//! Motion Tile's Output Width / Height on adjustment layers and on full-frame layers, with a
//! Transform after it that scales the tiled result into view (#1727, #1991).

use effectcraft_color::Label;
use effectcraft_keyframe::Value;
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Comp, ItemId, ItemKind, Layer, LayerSource, Project, Solid};
use effectcraft_time::{FrameRate, Tick};

use crate::{Image, render_frame};

const W: u32 = 200;
const H: u32 = 100;

fn solid(p: &mut Project, comp: &Comp, color: [f32; 3], w: u32, h: u32, pos: [f64; 2]) -> Layer {
    let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color, width: w, height: h, pixel_aspect: 1.0 }));
    let mut l = build::layer(p, comp, "Solid", LayerSource::Solid { item: sid }, (w, h), None);
    l.props.prop_mut("transform/position").unwrap().value = Value::Vec3([pos[0], pos[1], 0.0]);
    l
}

/// A white frame with a red 20×20 square in its top-left corner.
fn picture(p: &mut Project, comp: &Comp) -> Vec<Layer> {
    vec![solid(p, comp, [1.0, 0.0, 0.0], 20, 20, [10.0, 10.0]), solid(p, comp, [1.0, 1.0, 1.0], W, H, [100.0, 50.0])]
}

/// Motion Tile (Output `out` %) then Transform (Scale `scale` %), on `l`.
fn tile_then_scale(p: &mut Project, l: &mut Layer, out: f64, scale: f64) {
    let size = [W as f64, H as f64];
    for (id, vals) in [
        ("ec.stylize.motiontile", vec![("outputWidth", Value::Scalar(out)), ("outputHeight", Value::Scalar(out))]),
        ("ec.distort.transform", vec![("scaleHeight", Value::Scalar(scale)), ("scaleWidth", Value::Scalar(scale))]),
    ] {
        let spec = effectcraft_effects::find(id).unwrap();
        let mut next = p.next_id;
        let mut g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, size);
        p.next_id = next;
        for (k, v) in vals {
            g.prop_mut(k).unwrap().value = v;
        }
        l.props.sub_mut("effects").unwrap().children.push(g.into());
    }
}

fn comp() -> (Project, ItemId, Comp) {
    let mut p = Project::default();
    p.settings.bit_depth = effectcraft_project::BitDepth::Bpc32;
    let comp = Comp::new(W, H, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    (p, cid, comp)
}

/// The picture under an adjustment layer carrying the two effects.
fn adjustment_scene(out: f64, scale: f64) -> (Project, ItemId) {
    let (mut p, cid, comp) = comp();
    let mut adj = solid(&mut p, &comp, [1.0, 1.0, 1.0], W, H, [100.0, 50.0]);
    adj.switches.adjustment = true;
    tile_then_scale(&mut p, &mut adj, out, scale);
    let mut layers = vec![adj];
    layers.extend(picture(&mut p, &comp));
    p.comp_mut(cid).unwrap().layers = layers.into();
    (p, cid)
}

/// The picture as a full-frame precomp layer carrying the two effects.
fn layer_scene(out: f64, scale: f64) -> (Project, ItemId) {
    let (mut p, cid, comp) = comp();
    let inner = Comp::new(W, H, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
    let iid = p.add_item("Picture", Label::Sandstone, None, ItemKind::Comp(inner.clone().into()));
    let layers = picture(&mut p, &inner);
    p.comp_mut(iid).unwrap().layers = layers.into();
    let mut pre = build::layer(&mut p, &comp, "Picture", LayerSource::Comp { item: iid }, (W, H), None);
    tile_then_scale(&mut p, &mut pre, out, scale);
    p.comp_mut(cid).unwrap().layers.push(pre);
    (p, cid)
}

fn is(img: &Image, x: i64, y: i64, want: [f32; 4], what: &str) {
    let got = img.get(x, y);
    assert!(got.iter().zip(want).all(|(a, b)| (a - b).abs() < 0.02), "{what}: ({x}, {y}) is {got:?}, want {want:?}");
}

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const WHITE: [f32; 4] = [1.0; 4];
const CLEAR: [f32; 4] = [0.0; 4];

/// Output 300 % then Scale 50 % about the frame centre: comp pixel (x, y) shows the tiled frame
/// at (100 + 2(x − 100), 50 + 2(y − 50)), so the red square repeats every 100 × 50 pixels and
/// the frame is covered edge to edge, the tiles from beyond the frame included.
fn assert_tiled(img: &Image, what: &str) {
    for (x, y) in [(55, 30), (155, 30), (55, 80), (155, 80)] {
        is(img, x, y, RED, what);
    }
    for (x, y) in [(5, 5), (195, 5), (5, 95), (195, 95), (100, 50), (100, 5)] {
        is(img, x, y, WHITE, what);
    }
}

#[test]
fn adjustment_motion_tile_output_spreads_tiles_past_the_frame() {
    let (p, cid) = adjustment_scene(300.0, 50.0);
    assert_tiled(&render_frame(&p, cid, Tick::ZERO, 1.0), "adjustment, Output 300 %");
}

#[test]
fn layer_motion_tile_output_spreads_tiles_past_the_frame() {
    let (p, cid) = layer_scene(300.0, 50.0);
    assert_tiled(&render_frame(&p, cid, Tick::ZERO, 1.0), "full-frame layer, Output 300 %");
}

/// Output under 100 % crops: only the centre 50 % of the frame is left, the rest is transparent
/// (After Effects shows the comp background there).
#[test]
fn adjustment_motion_tile_output_under_100_crops_the_frame() {
    let (p, cid) = adjustment_scene(50.0, 100.0);
    let img = render_frame(&p, cid, Tick::ZERO, 1.0);
    is(&img, 100, 50, WHITE, "inside the kept centre");
    is(&img, 60, 30, WHITE, "inside the kept centre");
    for (x, y) in [(5, 5), (10, 10), (195, 95), (45, 50), (100, 20)] {
        is(&img, x, y, CLEAR, "outside the kept centre");
    }
}

/// The After Effects 2026 frames in #1727: a 1920×1080 red solid under an adjustment layer with
/// Motion Tile (Output 910 × 340 %, then 90 × 340 %) and Transform (Scale 12 %, Position 1361, 540).
/// The tiled frame, 9.1 × 3.4 comps scaled to 12 %, is a 2097 × 441 band centred on (1361, 540):
/// x 313–1920 (cut by the frame), y 320–760; at 90 % a 207-pixel column, x 1257–1465.
#[test]
fn adjustment_motion_tile_matches_the_after_effects_frames() {
    for (out_w, x0, x1) in [(910.0, 313.0, 1920.0), (90.0, 1257.0, 1465.0)] {
        let mut p = Project::default();
        p.settings.bit_depth = effectcraft_project::BitDepth::Bpc32;
        let comp = Comp::new(1920, 1080, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
        let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
        let red = solid(&mut p, &comp, [1.0, 0.0, 0.0], 1920, 1080, [960.0, 540.0]);
        let mut adj = solid(&mut p, &comp, [1.0, 1.0, 1.0], 1920, 1080, [960.0, 540.0]);
        adj.switches.adjustment = true;
        for (id, vals) in [
            ("ec.stylize.motiontile", vec![("outputWidth", Value::Scalar(out_w)), ("outputHeight", Value::Scalar(340.0))]),
            (
                "ec.distort.transform",
                vec![("scaleHeight", Value::Scalar(12.0)), ("scaleWidth", Value::Scalar(12.0)), ("position", Value::Vec2([1361.0, 540.0]))],
            ),
        ] {
            let spec = effectcraft_effects::find(id).unwrap();
            let mut next = p.next_id;
            let mut g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [1920.0, 1080.0]);
            p.next_id = next;
            for (k, v) in vals {
                g.prop_mut(k).unwrap().value = v;
            }
            adj.props.sub_mut("effects").unwrap().children.push(g.into());
        }
        p.comp_mut(cid).unwrap().layers = vec![adj, red].into();
        // Quarter resolution: comp pixel (x, y) is output pixel (x / 4, y / 4).
        let img = render_frame(&p, cid, Tick::ZERO, 0.25);
        let at = |x: f64, y: f64| img.get((x / 4.0) as i64, (y / 4.0) as i64);
        let what = format!("Output {out_w} %");
        for (x, y) in [(x0 + 8.0, 540.0), (x1 - 8.0, 540.0), (1361.0, 328.0), (1361.0, 752.0)] {
            assert_eq!(at(x, y), RED, "{what}: ({x}, {y}) is red");
        }
        for (x, y) in [(x0 - 8.0, 540.0), (1361.0, 312.0), (1361.0, 768.0), (100.0, 100.0)] {
            assert_eq!(at(x, y), CLEAR, "{what}: ({x}, {y}) is empty");
        }
        if x1 < 1920.0 {
            assert_eq!(at(x1 + 8.0, 540.0), CLEAR, "{what}: right of the column");
        }
    }
}
