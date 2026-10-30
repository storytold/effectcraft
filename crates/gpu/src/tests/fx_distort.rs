//! Distort / stylize GPU effects vs the CPU effects (the oracle): see `fx_distort.rs`.

use effectcraft_keyframe::Value;
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{BitDepth, LayerSource};
use effectcraft_render::RenderOpts;
use effectcraft_time::Tick;

use crate::tests::{Scene, check, compare_at, effect_case, n, opts, set};

fn e(v: u32) -> Value {
    Value::Enum(v)
}

fn on() -> Value {
    Value::Bool(true)
}

#[test]
fn twirl_bulge_ripple_wave_warp() {
    effect_case("ec.distort.twirl", &[("angle", n(120.0)), ("radius", n(40.0))]);
    effect_case("ec.distort.twirl", &[("angle", n(-400.0)), ("radius", n(70.0)), ("center", Value::Vec2([25.0, 18.0]))]);
    effect_case("ec.distort.bulge", &[("hradius", n(30.0)), ("vradius", n(20.0)), ("height", n(1.8))]);
    effect_case("ec.distort.bulge", &[("height", n(-2.5)), ("taperRadius", n(60.0)), ("pinAllEdges", on()), ("center", Value::Vec2([30.0, 25.0]))]);
    effect_case("ec.distort.ripple", &[("radius", n(80.0)), ("waveWidth", n(9.0)), ("waveHeight", n(4.0))]);
    effect_case("ec.distort.ripple", &[("conversion", e(1)), ("phase", n(70.0)), ("speed", n(3.0)), ("center", Value::Vec2([20.0, 30.0]))]);
    effect_case("ec.distort.wavewarp", &[("height", n(6.0)), ("width", n(25.0)), ("direction", n(30.0))]);
    effect_case("ec.distort.wavewarp", &[("waveType", e(2)), ("pinning", e(1)), ("phase", n(400.0)), ("speed", n(-2.0))]);
    effect_case("ec.distort.wavewarp", &[("waveType", e(8)), ("pinning", e(2)), ("height", n(-5.0)), ("direction", n(200.0))]);
    effect_case("ec.distort.wavewarp", &[("waveType", e(4)), ("pinning", e(7)), ("width", n(33.0))]);
    // #1181: the single-edge pinning paths, with the default vertical displacement direction.
    effect_case("ec.distort.wavewarp", &[("direction", n(90.0)), ("height", n(7.0)), ("width", n(25.0)), ("pinning", e(4))]);
    effect_case("ec.distort.wavewarp", &[("direction", n(90.0)), ("height", n(7.0)), ("width", n(19.0)), ("pinning", e(6)), ("waveType", e(1))]);
    effect_case("ec.distort.wavewarp", &[("direction", n(45.0)), ("height", n(-6.0)), ("pinning", e(4)), ("speed", n(2.0))]);
    effect_case("ec.distort.wavewarp", &[("direction", n(45.0)), ("height", n(-6.0)), ("pinning", e(6)), ("speed", n(2.0))]);
}

#[test]
fn turbulent_displace_and_cc_lens() {
    effect_case("ec.distort.turbulentdisplace", &[("size", n(30.0)), ("amount", n(20.0))]);
    effect_case(
        "ec.distort.turbulentdisplace",
        &[("displacement", e(1)), ("complexity", n(3.5)), ("evolution", n(90.0)), ("size", n(25.0)), ("pinning", e(2)), ("resizeLayer", on())],
    );
    effect_case(
        "ec.distort.turbulentdisplace",
        &[
            ("displacement", e(5)),
            ("evolutionOptions/cycleEvolution", on()),
            ("evolutionOptions/cycle", n(2.0)),
            ("evolution", n(500.0)),
            ("evolutionOptions/randomSeed", n(7.0)),
            ("pinning", e(0)),
            ("amount", n(-30.0)),
        ],
    );
    effect_case("ec.distort.turbulentdisplace", &[("displacement", e(8)), ("size", n(15.0)), ("pinning", e(6))]);
    // #943: Evolution moves the pattern at a constant rate on both paths, including steps of a
    // few degrees and angles past a whole revolution, where an eased evolution axis diverges.
    for evolution in [10.0, 37.0, 400.0, 725.5] {
        effect_case("ec.distort.turbulentdisplace", &[("evolution", n(evolution)), ("size", n(12.0)), ("amount", n(30.0))]);
    }
    effect_case("ec.distort.cclens", &[("size", n(70.0))]);
    effect_case("ec.distort.cclens", &[("size", n(45.0)), ("convergence", n(60.0)), ("center", Value::Vec2([30.0, 20.0]))]);
    effect_case("ec.distort.cclens", &[("convergence", n(-70.0))]);
}

/// Effect `id` on a shape layer, whose comp-sized bounds are centred on its origin and whose
/// effects run in effect space from the bounds' top-left (#227), at full and half resolution
/// (the 97 × 61 comp puts that corner between pixels).
fn shape_case(id: &str, vals: &[(&str, Value)]) {
    let mut s = Scene::new(BitDepth::Bpc32);
    let mut l = build::layer(&mut s.p, &s.comp, "Shape", LayerSource::Shape, (97, 61), None);
    let mut next = s.p.next_id;
    let mut ids = Ids(&mut next);
    let rect = build::shape_rect(&mut ids, [90.0, 54.0], [0.0, 0.0], 0.0);
    let fill = build::shape_fill(&mut ids, [0.9, 0.3, 0.1, 1.0]);
    let g = build::shape_group(&mut ids, "Rectangle 1", vec![rect, fill]);
    s.p.next_id = next;
    l.props.sub_mut("contents").unwrap().children.push(g.into());
    s.effect(&mut l, id, vals);
    s.push(l);
    check(&format!("{id} on a shape layer"), compare_at(&s, opts(), Tick::ZERO), 0.0);
    check(&format!("{id} on a shape layer, half"), compare_at(&s, RenderOpts { scale: 0.5, ..opts() }, Tick::ZERO), 0.0);
}

/// Edge pinning on a shape layer (#227).
#[test]
fn edge_pinning_on_a_shape_layer() {
    shape_case("ec.distort.turbulentdisplace", &[("size", n(30.0)), ("amount", n(20.0))]);
    shape_case("ec.distort.wavewarp", &[("pinning", e(1)), ("height", n(6.0)), ("width", n(25.0))]);
    // #1181: single-edge pinning over the layer rect, not just All Edges.
    shape_case("ec.distort.wavewarp", &[("pinning", e(4)), ("direction", n(90.0)), ("height", n(6.0)), ("width", n(25.0))]);
    shape_case("ec.distort.wavewarp", &[("pinning", e(6)), ("direction", n(90.0)), ("height", n(6.0)), ("width", n(25.0))]);
    shape_case(
        "ec.distort.bulge",
        &[("pinAllEdges", on()), ("height", n(2.0)), ("hradius", n(40.0)), ("vradius", n(30.0)), ("center", Value::Vec2([28.5, 20.5]))],
    );
}

/// Effect points on a shape layer at their defaults (the bounds' centre) and set (#227).
#[test]
fn effect_points_on_a_shape_layer() {
    shape_case("ec.distort.twirl", &[("angle", n(120.0))]);
    shape_case("ec.distort.bulge", &[("hradius", n(30.0)), ("vradius", n(20.0)), ("height", n(1.8))]);
    shape_case("ec.distort.ripple", &[("radius", n(80.0)), ("waveWidth", n(9.0)), ("waveHeight", n(4.0))]);
    shape_case("ec.distort.cclens", &[("size", n(45.0)), ("convergence", n(60.0)), ("center", Value::Vec2([30.0, 20.0]))]);
    shape_case("ec.distort.turbulentdisplace", &[("size", n(25.0)), ("amount", n(30.0)), ("offset", Value::Vec2([10.0, 40.0]))]);
}

#[test]
fn mesh_warp() {
    // 2 rows × 3 columns: 12 vertices.
    let mesh = "0,0 0,0 0,0 0,0  0,0 6,-4 -5,3 0,0  0,0 0,0 0,0 0,0";
    effect_case("ec.distort.meshwarp", &[("rows", n(2.0)), ("columns", n(3.0)), ("quality", n(3.0)), ("mesh", Value::Str(mesh.into()))]);
    let mesh = "-4,-3 2,0 8,5 0,-2 9,9 1,0 -3,4 2,2 4,-6";
    effect_case("ec.distort.meshwarp", &[("rows", n(2.0)), ("columns", n(2.0)), ("quality", n(5.0)), ("mesh", Value::Str(mesh.into()))]);
}

#[test]
fn mosaic_find_edges_emboss() {
    effect_case("ec.stylize.mosaic", &[]);
    effect_case("ec.stylize.mosaic", &[("horizontal", n(7.0)), ("vertical", n(3.0))]);
    effect_case("ec.stylize.mosaic", &[("horizontal", n(13.0)), ("vertical", n(9.0)), ("sharpColors", on())]);
    effect_case("ec.stylize.findedges", &[]);
    effect_case("ec.stylize.findedges", &[("invert", on()), ("blend", n(30.0))]);
    effect_case("ec.stylize.emboss", &[]);
    effect_case("ec.stylize.emboss", &[("direction", n(160.0)), ("relief", n(4.0)), ("contrast", n(250.0)), ("blend", n(20.0))]);
}

#[test]
fn displacement_map_self() {
    effect_case("ec.distort.displacementmap", &[("maxHorizontal", n(8.0)), ("maxVertical", n(-5.0))]);
    effect_case("ec.distort.displacementmap", &[("useForHorizontal", e(4)), ("useForVertical", e(3)), ("wrapPixelsAround", on())]);
    effect_case(
        "ec.distort.displacementmap",
        &[("useForHorizontal", e(10)), ("useForVertical", e(6)), ("maxVertical", n(12.0)), ("expandOutput", Value::Bool(false))],
    );
}

/// Displacement Map reading another layer (Center / Stretch / Tile), composited.
#[test]
fn displacement_map_layer() {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        for (behavior, wrap) in [(0, false), (1, false), (2, true), (2, false)] {
            let mut s = Scene::new(depth);
            let bg = s.solid([0.15, 0.1, 0.2], 97, 61);
            s.push(bg);
            let mut map = s.footage(50, 36);
            map.switches.video = false;
            let mid = s.push(map);
            let mut l = s.footage(70, 44);
            s.effect(
                &mut l,
                "ec.distort.displacementmap",
                &[
                    ("displacementMapLayer", Value::Layer(Some(mid.0))),
                    ("useForHorizontal", e(0)),
                    ("useForVertical", e(4)),
                    ("maxHorizontal", n(9.0)),
                    ("maxVertical", n(6.0)),
                    ("displacementMapBehavior", e(behavior)),
                    ("wrapPixelsAround", Value::Bool(wrap)),
                ],
            );
            set(&mut l, "transform/rotation", Value::Scalar(8.0));
            s.push(l);
            let label = format!("displacement map {behavior} {wrap} {depth:?}");
            check(&label, compare_at(&s, opts(), Tick::ZERO), 0.0);
            check(&format!("{label} half"), compare_at(&s, RenderOpts { scale: 0.5, ..opts() }, Tick::ZERO), 0.0);
        }
    }
}
