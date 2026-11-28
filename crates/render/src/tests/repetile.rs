//! CC RepeTile (Edge Tile Repeat) on adjustment layers: the buffer grows around the comp
//! below so a following effect can sample tiles from beyond the frame (#3923).

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

/// RepeTile (Expand `expand` on every side, Tiling Repeat) then Transform (Scale `scale` %).
fn repetile_then_scale(p: &mut Project, l: &mut Layer, expand: f64, scale: f64) {
    for (id, vals) in [
        (
            "ec.stylize.ccrepetile",
            vec![
                ("expandRight", Value::Scalar(expand)),
                ("expandLeft", Value::Scalar(expand)),
                ("expandDown", Value::Scalar(expand)),
                ("expandUp", Value::Scalar(expand)),
            ],
        ),
        ("ec.distort.transform", vec![("scaleHeight", Value::Scalar(scale)), ("scaleWidth", Value::Scalar(scale))]),
    ] {
        add_effect(p, l, id, vals);
    }
}

fn add_effect(p: &mut Project, l: &mut Layer, id: &str, vals: Vec<(&str, Value)>) {
    let spec = effectcraft_effects::find(id).unwrap();
    let mut next = p.next_id;
    let mut g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [W as f64, H as f64]);
    p.next_id = next;
    for (k, v) in vals {
        g.prop_mut(k).unwrap().value = v;
    }
    l.props.sub_mut("effects").unwrap().children.push(g.into());
}

fn comp() -> (Project, ItemId, Comp) {
    let mut p = Project::default();
    p.settings.bit_depth = effectcraft_project::BitDepth::Bpc32;
    let comp = Comp::new(W, H, FrameRate::FPS_30, Tick::from_seconds_f64(3.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    (p, cid, comp)
}

/// The picture under an adjustment layer carrying the two effects.
fn adjustment_scene(expand: f64, scale: f64) -> (Project, ItemId) {
    let (mut p, cid, comp) = comp();
    let mut adj = solid(&mut p, &comp, [1.0, 1.0, 1.0], W, H, [100.0, 50.0]);
    adj.switches.adjustment = true;
    repetile_then_scale(&mut p, &mut adj, expand, scale);
    let mut layers = vec![adj];
    layers.extend(picture(&mut p, &comp));
    p.comp_mut(cid).unwrap().layers = layers.into();
    (p, cid)
}

/// The picture as a full-frame precomp layer carrying the two effects.
fn layer_scene(expand: f64, scale: f64) -> (Project, ItemId) {
    let (mut p, cid, comp) = comp();
    let inner = Comp::new(W, H, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
    let iid = p.add_item("Picture", Label::Sandstone, None, ItemKind::Comp(inner.clone().into()));
    let layers = picture(&mut p, &inner);
    p.comp_mut(iid).unwrap().layers = layers.into();
    let mut pre = build::layer(&mut p, &comp, "Picture", LayerSource::Comp { item: iid }, (W, H), None);
    repetile_then_scale(&mut p, &mut pre, expand, scale);
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

/// Expand 100 px then Scale 50 % about the frame centre: comp pixel (x, y) shows the tiled
/// frame at (100 + 2(x − 100), 50 + 2(y − 50)), so the red square repeats every 200 × 100
/// pixels and the frame is covered edge to edge, the tiles from beyond the frame included.
fn assert_tiled(img: &Image, what: &str) {
    for (x, y) in [(55, 30), (155, 30), (55, 80), (155, 80)] {
        is(img, x, y, RED, what);
    }
    for (x, y) in [(5, 5), (195, 5), (5, 95), (195, 95), (100, 50), (100, 5)] {
        is(img, x, y, WHITE, what);
    }
}

#[test]
fn adjustment_repetile_then_transform_spreads_tiles_past_the_frame() {
    let (p, cid) = adjustment_scene(100.0, 50.0);
    assert_tiled(&render_frame(&p, cid, Tick::ZERO, 1.0), "adjustment RepeTile Expand 100 then Scale 50 %");
}

#[test]
fn layer_repetile_then_transform_spreads_tiles_past_the_frame() {
    let (p, cid) = layer_scene(100.0, 50.0);
    assert_tiled(&render_frame(&p, cid, Tick::ZERO, 1.0), "layer RepeTile Expand 100 then Scale 50 %");
}

/// The recipe in #3923: a white solid under an adjustment layer carrying RepeTile (Expand
/// 100, Repeat) then Wave Warp (Triangle, Height 80, Direction 45°, Speed 0.2). The warp
/// displaces up to 80 px, inside the 100 px margin, so the frame stays filled to the edges
/// instead of going transparent where it samples past the comp (#3923).
#[test]
fn adjustment_repetile_feeds_wave_warp_with_tiles() {
    let (mut p, cid, comp) = comp();
    let bg = solid(&mut p, &comp, [1.0, 1.0, 1.0], W, H, [100.0, 50.0]);
    let mut adj = solid(&mut p, &comp, [1.0, 1.0, 1.0], W, H, [100.0, 50.0]);
    adj.switches.adjustment = true;
    add_effect(
        &mut p,
        &mut adj,
        "ec.stylize.ccrepetile",
        vec![
            ("expandRight", Value::Scalar(100.0)),
            ("expandLeft", Value::Scalar(100.0)),
            ("expandDown", Value::Scalar(100.0)),
            ("expandUp", Value::Scalar(100.0)),
        ],
    );
    add_effect(
        &mut p,
        &mut adj,
        "ec.distort.wavewarp",
        vec![
            ("waveType", Value::Enum(2)),
            ("height", Value::Scalar(80.0)),
            ("width", Value::Scalar(800.0)),
            ("direction", Value::Scalar(45.0)),
            ("speed", Value::Scalar(0.2)),
        ],
    );
    p.comp_mut(cid).unwrap().layers = vec![adj, bg].into();
    let img = render_frame(&p, cid, Tick::from_seconds_f64(2.0), 1.0);
    // Where the warp samples past the comp it finds tiles of the white solid, not transparency.
    for (x, y) in [(0, 0), (1, 1), (198, 1), (199, 99), (1, 98), (50, 0), (150, 99), (100, 0)] {
        is(&img, x, y, WHITE, "edge pixel");
    }
}

/// Without the expansion the same stack leaves the sampled edges transparent, which is the
/// failure in #3923. Kept as the negative control for the test above.
#[test]
fn wave_warp_without_repetile_leaves_edges_clear() {
    let (mut p, cid, comp) = comp();
    let bg = solid(&mut p, &comp, [1.0, 1.0, 1.0], W, H, [100.0, 50.0]);
    let mut adj = solid(&mut p, &comp, [1.0, 1.0, 1.0], W, H, [100.0, 50.0]);
    adj.switches.adjustment = true;
    add_effect(
        &mut p,
        &mut adj,
        "ec.distort.wavewarp",
        vec![
            ("waveType", Value::Enum(2)),
            ("height", Value::Scalar(80.0)),
            ("width", Value::Scalar(800.0)),
            ("direction", Value::Scalar(45.0)),
            ("speed", Value::Scalar(0.2)),
        ],
    );
    p.comp_mut(cid).unwrap().layers = vec![adj, bg].into();
    let img = render_frame(&p, cid, Tick::from_seconds_f64(2.0), 1.0);
    let clear = [(0i64, 0i64), (199, 0), (0, 99), (199, 99)].iter().filter(|&&(x, y)| img.get(x, y) == CLEAR).count();
    assert!(clear > 0, "the warp should leave at least one corner transparent without RepeTile");
}
