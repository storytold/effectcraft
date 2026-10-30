use std::time::Instant;

mod deep_nesting;
mod fuzz;

use effectcraft_color::Label;
use effectcraft_keyframe::{Keyframe, ShapePath, TextDoc, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Comp, Expression, ItemId, ItemKind, Layer, LayerId, LayerSource, Marker, MaskMode, Project, PropGroup, Solid};
use effectcraft_render::{EvalCtx, NoFootage, RenderOpts, Renderer};
use effectcraft_time::{FrameRate, Tick};

use crate::{Expressions, check_syntax, eval_property, eval_standalone};

const ROT: &str = "transform/rotation";
const POS: &str = "transform/position";
const OPA: &str = "transform/opacity";

/// A 200×100, 30 fps, 10 s comp named "Main" with:
/// 1. "A": 20×20 red solid at (50, 40), slider effect = 42, rotation keyed 0→90 over 0–1 s
/// 2. "B": 40×40 blue solid at (100, 50)
/// 3. "Title": text layer "Hello"
/// 4. "Shape": shape layer with "Rectangle 1" (60×30 rect path, red fill)
///
/// plus an "Other" comp with one layer "Inner".
struct Fx {
    p: Project,
    cid: ItemId,
    a: LayerId,
    b: LayerId,
    title: LayerId,
    shape: LayerId,
}

fn solid(p: &mut Project, comp: &Comp, name: &str, color: [f32; 3], w: u32, h: u32) -> Layer {
    let sid = p.add_item(name, Label::Red, None, ItemKind::Solid(Solid { color, width: w, height: h, pixel_aspect: 1.0 }));
    build::layer(p, comp, name, LayerSource::Solid { item: sid }, (w, h), None)
}

fn group_push(p: &mut Project, f: impl FnOnce(&mut Ids) -> PropGroup) -> PropGroup {
    let mut next = p.next_id;
    let g = f(&mut Ids(&mut next));
    p.next_id = next;
    g
}

fn fx() -> Fx {
    let mut p = Project::default();
    let comp = Comp::new(200, 100, FrameRate::FPS_30, Tick::from_seconds_f64(10.0));
    let cid = p.add_item("Main", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));

    let mut a = solid(&mut p, &comp, "A", [1.0, 0.0, 0.0], 20, 20);
    a.props.prop_mut(POS).unwrap().value = Value::Vec3([50.0, 40.0, 0.0]);
    a.props.prop_mut(ROT).unwrap().keys = vec![Keyframe::new(Tick::ZERO, Value::Scalar(0.0)), Keyframe::new(secs(1.0), Value::Scalar(90.0))];
    let spec = effectcraft_effects::find("ec.control.slider").unwrap();
    let mut slider = group_push(&mut p, |ids| effectcraft_effects::instantiate(spec, ids, "Slider Control", [20.0, 20.0]));
    slider.prop_mut("slider").unwrap().value = Value::Scalar(42.0);
    a.props.sub_mut("effects").unwrap().children.push(slider.into());
    let mask = group_push(&mut p, |ids| build::mask(ids, "Mask 1", ShapePath::rect([10.0, 10.0], 10.0, 10.0), MaskMode::Add, [255, 0, 0]));
    a.props.sub_mut("masks").unwrap().children.push(mask.into());
    a.markers.push(Marker { time: secs(2.0), comment: "beat".into(), ..Default::default() });
    a.markers.push(Marker { time: secs(4.0), comment: "drop".into(), ..Default::default() });

    let b = solid(&mut p, &comp, "B", [0.0, 0.0, 1.0], 40, 40);

    let mut title = build::layer(&mut p, &comp, "Title", LayerSource::Text, (200, 100), None);
    title.props.prop_mut("text/sourceText").unwrap().value = Value::Text(Box::new(TextDoc { text: "Hello".into(), size: 40.0, ..Default::default() }));
    title.props.prop_mut(POS).unwrap().value = Value::Vec3([20.0, 60.0, 0.0]);

    let mut shape = build::layer(&mut p, &comp, "Shape", LayerSource::Shape, (200, 100), None);
    let g = group_push(&mut p, |ids| {
        let rect = build::shape_rect(ids, [60.0, 30.0], [0.0, 0.0], 0.0);
        let fill = build::shape_fill(ids, [1.0, 0.0, 0.0, 1.0]);
        build::shape_group(ids, "Rectangle 1", vec![rect, fill])
    });
    shape.props.sub_mut("contents").unwrap().children.push(g.into());

    let ids = (a.id, b.id, title.id, shape.id);
    let c = p.comp_mut(cid).unwrap();
    c.layers = vec![a, b, title, shape].into();
    c.markers.push(Marker { time: secs(1.5), comment: "comp marker".into(), ..Default::default() });

    let other = Comp::new(64, 32, FrameRate::FPS_24, Tick::from_seconds_f64(3.0));
    let oid = p.add_item("Other", Label::Sandstone, None, ItemKind::Comp(other.clone().into()));
    let inner = solid(&mut p, &other, "Inner", [0.0, 1.0, 0.0], 64, 32);
    p.comp_mut(oid).unwrap().layers.push(inner);

    Fx { p, cid, a: ids.0, b: ids.1, title: ids.2, shape: ids.3 }
}

fn secs(s: f64) -> Tick {
    Tick::from_seconds_f64(s)
}

impl Fx {
    fn ev(&self, layer: LayerId, path: &str, text: &str, t: f64) -> Result<Value, String> {
        eval_standalone(&self.p, self.cid, layer, path, text, t)
    }
    /// Scalar result on layer A's rotation.
    fn num(&self, text: &str, t: f64) -> f64 {
        match self.ev(self.a, ROT, text, t) {
            Ok(v) => v.as_f64(),
            Err(e) => panic!("{text}: {e}"),
        }
    }
    /// Position result on layer A (3 components).
    fn pos(&self, text: &str, t: f64) -> [f64; 3] {
        match self.ev(self.a, POS, text, t) {
            Ok(v) => v.as_vec3(),
            Err(e) => panic!("{text}: {e}"),
        }
    }
    fn text(&self, text: &str, t: f64) -> String {
        match self.ev(self.title, "text/sourceText", text, t) {
            Ok(Value::Text(d)) => d.text,
            Ok(v) => panic!("{text}: not text: {v:?}"),
            Err(e) => panic!("{text}: {e}"),
        }
    }
    fn err(&self, text: &str) -> String {
        match self.ev(self.a, ROT, text, 0.0) {
            Ok(v) => panic!("{text}: expected an error, got {v:?}"),
            Err(e) => e,
        }
    }
    fn set_expr(&mut self, layer: LayerId, path: &str, text: &str) {
        let l = self.p.comp_mut(self.cid).unwrap().layer_mut(layer).unwrap();
        l.props.prop_mut(path).unwrap().expr = Some(Expression { text: text.into(), enabled: true });
    }
    fn layer_mut(&mut self, layer: LayerId) -> &mut Layer {
        self.p.comp_mut(self.cid).unwrap().layer_mut(layer).unwrap()
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

macro_rules! assert_close {
    ($a:expr, $b:expr) => {{
        let (a, b): (f64, f64) = ($a, $b);
        assert!((a - b).abs() < 1e-6, "{} = {a}, expected {b}", stringify!($a));
    }};
    ($a:expr, $b:expr, $eps:expr) => {{
        let (a, b): (f64, f64) = ($a, $b);
        assert!((a - b).abs() < $eps, "{} = {a}, expected {b} ± {}", stringify!($a), $eps);
    }};
}

// ---- basics -----------------------------------------------------------------------------------

#[test]
fn time_times_360() {
    let f = fx();
    assert_close!(f.num("time * 360", 0.5), 180.0);
    assert_close!(f.num("time*360", 2.0), 720.0);
}

#[test]
fn value_is_the_keyframed_value() {
    let f = fx();
    assert_close!(f.num("value", 0.5), 45.0);
    assert_close!(f.num("value + 10", 1.0), 100.0);
}

#[test]
fn math_object() {
    let f = fx();
    assert_close!(f.num("Math.sin(Math.PI / 2) * 100", 0.0), 100.0);
    assert_close!(f.num("Math.max(3, 7) - Math.floor(2.7)", 0.0), 5.0);
}

#[test]
fn last_statement_is_the_result() {
    let f = fx();
    assert_close!(f.num("var x = 5;\nvar y = x * 2;\ny", 0.0), 10.0);
    assert_close!(f.num("x = 3; x + 1", 0.0), 4.0);
}

#[test]
fn if_else_blocks_end_in_a_value() {
    let f = fx();
    assert_close!(f.num("if (time > 1) { 10 } else { 20 }", 2.0), 10.0);
    assert_close!(f.num("if (time > 1) { 10 } else { 20 }", 0.0), 20.0);
    assert_close!(f.num("if (time > 1) 1; else if (time > 0.5) 2; else 3", 0.75), 2.0);
}

#[test]
fn functions_and_let_are_reusable_across_evaluations() {
    let f = fx();
    let e = "function sq(x) { return x * x }\nlet k = 2;\nconst j = 1;\nsq(time) * k + j";
    assert_close!(f.num(e, 3.0), 19.0);
    assert_close!(f.num(e, 2.0), 9.0);
}

#[test]
fn comments_are_ignored() {
    let f = fx();
    assert_close!(f.num("// spin\n/* fast */ time * 100 // per second", 1.0), 100.0);
}

#[test]
fn ternary_and_comparison() {
    let f = fx();
    assert_close!(f.num("time > 1 ? 1 + 1 : 3 * 3", 2.0), 2.0);
    assert_close!(f.num("time > 1 ? 1 + 1 : 3 * 3", 0.0), 9.0);
}

// ---- array maths ------------------------------------------------------------------------------

#[test]
fn array_plus_array() {
    let f = fx();
    assert_eq!(f.pos("value + [10, 0]", 0.0), [60.0, 40.0, 0.0]);
    assert_eq!(f.pos("[10, 5] + value", 0.0), [60.0, 45.0, 0.0]);
}

#[test]
fn array_times_and_divide_scalar() {
    let f = fx();
    assert_eq!(f.pos("[1, 2] * 2", 0.0), [2.0, 4.0, 0.0]);
    assert_eq!(f.pos("2 * [1, 2]", 0.0), [2.0, 4.0, 0.0]);
    assert_eq!(f.pos("[10, 20] / 2", 0.0), [5.0, 10.0, 0.0]);
    assert_eq!(f.pos("value - [50, 40]", 0.0), [0.0, 0.0, 0.0]);
}

#[test]
fn array_precedence_and_unary_minus() {
    let f = fx();
    assert_eq!(f.pos("[1, 1] + [2, 3] * 2", 0.0), [5.0, 7.0, 0.0]);
    assert_eq!(f.pos("([1, 1] + [2, 3]) * 2", 0.0), [6.0, 8.0, 0.0]);
    assert_eq!(f.pos("-value + [100, 100]", 0.0), [50.0, 60.0, 0.0]);
}

#[test]
fn compound_assignment_on_arrays() {
    let f = fx();
    assert_eq!(f.pos("var p = value;\np += [5, 5];\np", 0.0), [55.0, 45.0, 0.0]);
    assert_eq!(f.pos("var p = [1, 2];\np *= 3;\np", 0.0), [3.0, 6.0, 0.0]);
}

#[test]
fn string_concatenation_still_works() {
    let f = fx();
    assert_eq!(f.text("'Count: ' + 5", 0.0), "Count: 5");
    assert_eq!(f.text("'a' + 1 + 2", 0.0), "a12");
    assert_eq!(f.text("1 + 2 + 'a'", 0.0), "3a");
    assert_eq!(f.text("'pos ' + [1, 2]", 0.0), "pos 1,2");
}

#[test]
fn vector_helpers() {
    let f = fx();
    assert_eq!(f.pos("add([1, 2], [3, 4])", 0.0), [4.0, 6.0, 0.0]);
    assert_eq!(f.pos("sub([5, 5], [1, 2])", 0.0), [4.0, 3.0, 0.0]);
    assert_eq!(f.pos("mul([1, 2], 3)", 0.0), [3.0, 6.0, 0.0]);
    assert_eq!(f.pos("div([3, 6], 3)", 0.0), [1.0, 2.0, 0.0]);
    assert_close!(f.num("dot([1, 2, 3], [4, 5, 6])", 0.0), 32.0);
    assert_eq!(f.pos("cross([1, 0, 0], [0, 1, 0])", 0.0), [0.0, 0.0, 1.0]);
    assert_close!(f.num("length([3, 4])", 0.0), 5.0);
    assert_close!(f.num("length([1, 1], [4, 5])", 0.0), 5.0);
    assert_close!(f.num("normalize([0, 10])[1]", 0.0), 1.0);
}

#[test]
fn clamp_scalar_and_array() {
    let f = fx();
    assert_close!(f.num("clamp(150, 0, 100)", 0.0), 100.0);
    assert_close!(f.num("clamp(-5, 0, 100)", 0.0), 0.0);
    assert_eq!(f.pos("clamp([-10, 500], 0, 100)", 0.0), [0.0, 100.0, 0.0]);
}

#[test]
fn angles_and_look_at() {
    let f = fx();
    assert_close!(f.num("radiansToDegrees(Math.PI)", 0.0), 180.0);
    assert_close!(f.num("degreesToRadians(90)", 0.0), std::f64::consts::FRAC_PI_2);
    let o = f.ev(f.a, "transform/orientation", "lookAt([0, 0, 0], [100, 0, 100])", 0.0).unwrap().as_vec3();
    assert!(close(o[1], 45.0) && close(o[0], 0.0), "{o:?}");
}

// ---- interpolation ----------------------------------------------------------------------------

#[test]
fn linear_five_args_maps_and_clamps() {
    let f = fx();
    assert_close!(f.num("linear(time, 0, 2, 0, 100)", 1.0), 50.0);
    assert_close!(f.num("linear(time, 0, 2, 0, 100)", 5.0), 100.0);
    assert_close!(f.num("linear(time, 1, 2, 0, 100)", 0.0), 0.0);
    assert_close!(f.num("linear(time, 2, 0, 0, 100)", 0.5), 75.0);
}

#[test]
fn linear_three_args() {
    let f = fx();
    assert_close!(f.num("linear(0.25, 10, 20)", 0.0), 12.5);
}

#[test]
fn ease_curves() {
    let f = fx();
    assert_close!(f.num("ease(time, 0, 1, 0, 100)", 0.5), 50.0);
    assert!(f.num("ease(time, 0, 1, 0, 100)", 0.25) < 25.0);
    assert!(f.num("easeIn(time, 0, 1, 0, 100)", 0.25) < 25.0);
    assert!(f.num("easeOut(time, 0, 1, 0, 100)", 0.25) > 25.0);
    assert_close!(f.num("easeIn(time, 0, 1, 0, 100)", 1.0), 100.0);
    assert_close!(f.num("easeOut(0.5, 0, 10)", 0.0) + f.num("easeIn(0.5, 0, 10)", 0.0), 10.0);
}

#[test]
fn interpolation_with_arrays() {
    let f = fx();
    assert_eq!(f.pos("linear(time, 0, 1, [0, 0], [100, 50])", 0.5), [50.0, 25.0, 0.0]);
    assert_eq!(f.pos("ease(time, 0, 1, [0, 0], [100, 50])", 1.0), [100.0, 50.0, 0.0]);
}

#[test]
fn cubic_bezier_curves() {
    let f = fx();
    // ease(t, …) is cubic-bezier(1/3, 0, 2/3, 1): x(u) = u and y(u) = 3u² - 2u³.
    assert_close!(f.num("bezier(0.25, 0, 1, 0, 100, [0.3333333333333333, 0, 0.6666666666666666, 1])", 0.0), f.num("ease(0.25, 0, 1, 0, 100)", 0.0));
    assert_close!(f.num("bezier(0.5, 0, 1, 0, 100, [0.3333333333333333, 0, 0.6666666666666666, 1])", 0.0), 50.0);
    // Symmetric ease-in-out: slower at both ends, faster in the middle.
    let early = f.num("bezier(0.25, 0, 1, 0, 100, [0.42, 0, 0.58, 1])", 0.0);
    let late = f.num("bezier(0.75, 0, 1, 0, 100, [0.42, 0, 0.58, 1])", 0.0);
    assert!(early < 25.0 && late > 75.0, "ease-in-out: {early}, {late}");
    // Control points on the diagonal are linear.
    assert_close!(f.num("bezier(0.3, 0, 1, 0, 100, [0, 0, 1, 1])", 0.0), 30.0);
    // t is clamped to tMin..tMax; tMin == tMax behaves like the other helpers.
    assert_close!(f.num("bezier(-1, 0, 1, 0, 100, [0.42, 0, 0.58, 1])", 0.0), 0.0);
    assert_close!(f.num("bezier(2, 0, 1, 0, 100, [0.42, 0, 0.58, 1])", 0.0), 100.0);
    assert_close!(f.num("bezier(5, 2, 2, 0, 100, [0.42, 0, 0.58, 1])", 0.0), 100.0);
}

#[test]
fn cubic_bezier_short_form_arrays_and_fallbacks() {
    let f = fx();
    // Short form: no tMin / tMax.
    assert_close!(f.num("bezier(0.25, 10, 20, [0, 0, 1, 1])", 0.0), 12.5);
    assert_eq!(f.pos("bezier(time, 0, 1, [0, 0], [100, 50], [0.42, 0, 0.58, 1])", 1.0), [100.0, 50.0, 0.0]);
    // y outside 0..1 overshoots, as CSS allows.
    let over = f.num("bezier(0.5, 0, 1, 0, 100, [0.5, 2, 0.5, 2])", 0.0);
    assert!(over > 100.0, "overshoot: {over}");
    // A curve that is not four finite numbers falls back to linear instead of throwing.
    assert_close!(f.num("bezier(0.5, 0, 1, 0, 100, [1, 0])", 0.0), 50.0);
    assert_close!(f.num("bezier(0.5, 0, 1, 0, 100, 'ease')", 0.0), 50.0);
    // A missing or unusable curve must not consume the last value: read the curve by position.
    assert_close!(f.num("bezier(0.5, 0, 1, 0, 100)", 0.0), 50.0);
    assert_close!(f.num("bezier(0.5, 0, 100)", 0.0), 50.0);
    assert_close!(f.num("bezier(0.5, 0, 100, [1, 0])", 0.0), 50.0);
    let v = f.ev(f.shape, "contents/#1/contents/fill/color", "bezier(0.5, 0, 1, [0, 0, 0, 1], [1, 1, 1, 1])", 0.0).unwrap();
    assert_eq!(v, Value::Color([0.5, 0.5, 0.5, 1.0]));
    // Exactly four values are a curve; extra ones mean the curve is malformed, so linear.
    assert_close!(f.num("bezier(0.5, 0, 1, 0, 100, [0.42, 0, 0.58, 1, 0])", 0.0), 50.0);
    assert_close!(f.num("bezier(0.5, 0, 1, 0, 100, [0.42, 0, 0.58, 1, 2, 3])", 0.0), 50.0);
    // x control points outside 0..1 are clamped, as CSS requires.
    let v = f.num("bezier(0.5, 0, 1, 0, 100, [-1, 0, 2, 1])", 0.0);
    assert!(v.is_finite(), "clamped curve: {v}");
}

#[test]
fn cubic_bezier_is_accurate_near_the_endpoints() {
    /// The value the curve reaches at `x`, by bisection on its monotone x(u).
    fn reference(x1: f64, y1: f64, x2: f64, y2: f64, x: f64) -> f64 {
        let bx = |u: f64| 3.0 * (1.0 - u) * (1.0 - u) * u * x1 + 3.0 * (1.0 - u) * u * u * x2 + u * u * u;
        let by = |u: f64| 3.0 * (1.0 - u) * (1.0 - u) * u * y1 + 3.0 * (1.0 - u) * u * u * y2 + u * u * u;
        let (mut lo, mut hi) = (0.0f64, 1.0f64);
        for _ in 0..100 {
            let mid = 0.5 * (lo + hi);
            if bx(mid) < x {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        by(0.5 * (lo + hi))
    }
    // Steep curves: a residual in x is not a residual in y near the endpoints (dy/dx is in the
    // thousands), so the solver stops on the estimated y error instead.
    let f = fx();
    for (c, x) in [([0.0, 1000.0, 1.0, 1.0], 1e-6), ([0.0, 100.0, 1.0, 0.0], 1e-9), ([0.8, -500.0, 1.0, 1.0], 0.999999)] {
        let got = f.num(&format!("bezier({x}, 0, 1, [{}, {}, {}, {}])", c[0], c[1], c[2], c[3]), 0.0);
        let want = reference(c[0], c[1], c[2], c[3], x);
        assert!((got - want).abs() < 1e-6, "curve {c:?} at {x}: {got} vs {want}");
    }
}

// ---- object model -----------------------------------------------------------------------------
#[test]
fn comp_attributes() {
    let f = fx();
    assert_close!(f.num("thisComp.width + thisComp.height", 0.0), 300.0);
    assert_close!(f.num("thisComp.duration", 0.0), 10.0);
    assert_close!(f.num("thisComp.frameDuration * 30", 0.0), 1.0);
    assert_close!(f.num("thisComp.numLayers", 0.0), 4.0);
    assert_eq!(f.text("thisComp.name", 0.0), "Main");
}

#[test]
fn layer_attributes() {
    let f = fx();
    assert_eq!(f.text("thisLayer.name + ' ' + index", 0.0), "Title 3");
    assert_close!(f.num("index", 0.0), 1.0);
    assert_close!(f.num("inPoint + outPoint + startTime", 0.0), 10.0);
    assert_close!(f.num("width * 1000 + height", 0.0), 20_020.0);
    assert_close!(f.num("thisLayer.hasParent ? 1 : 0", 0.0), 0.0);
}

#[test]
fn layer_by_name_and_index() {
    let f = fx();
    assert_close!(f.num("thisComp.layer(\"B\").transform.position[0]", 0.0), 100.0);
    assert_eq!(f.text("thisComp.layer(2).name", 0.0), "B");
    assert_eq!(f.text("thisComp.layer(thisLayer, 1).name", 0.0), "Shape");
    assert_eq!(f.text("thisComp.layer(thisLayer, -2).name", 0.0), "A");
}

#[test]
fn cross_layer_position_reference() {
    let f = fx();
    assert_eq!(f.pos("thisComp.layer(\"B\").transform.position", 0.0), [100.0, 50.0, 0.0]);
    assert_eq!(f.pos("thisComp.layer(\"B\").position + [0, 10]", 0.0), [100.0, 60.0, 0.0]);
    assert_close!(f.num("thisComp.layer('B').transform.scale[1]", 0.0), 100.0);
    assert_close!(f.num("thisComp.layer('B').transform.opacity", 0.0), 100.0);
}

#[test]
fn layer_shorthand_attributes() {
    let f = fx();
    assert_close!(f.num("position[0] + anchorPoint[0]", 0.0), 60.0);
    assert_close!(f.num("scale[0] + opacity", 0.0), 200.0);
    assert_close!(f.num("transform.anchorPoint[1]", 0.0), 10.0);
}

#[test]
fn reading_a_property_that_has_an_expression() {
    let mut f = fx();
    f.set_expr(f.b, ROT, "time * 10");
    assert_close!(f.num("thisComp.layer('B').transform.rotation + 1", 2.0), 21.0);
    // Chains: B.opacity reads B.rotation (expression), A reads B.opacity.
    f.set_expr(f.b, OPA, "transform.rotation * 2");
    assert_close!(f.num("thisComp.layer('B').transform.opacity", 1.0), 20.0);
}

#[test]
fn own_property_reads_pre_expression_value() {
    let f = fx();
    assert_close!(f.num("transform.rotation + 1", 0.5), 46.0);
    assert_close!(f.num("thisProperty.value * 2", 1.0), 180.0);
}

#[test]
fn circular_references_terminate() {
    let mut f = fx();
    f.set_expr(f.a, OPA, "thisComp.layer('B').transform.opacity");
    f.set_expr(f.b, OPA, "thisComp.layer('A').transform.opacity");
    let start = Instant::now();
    let r = eval_property(&f.p, f.cid, f.a, OPA, 0.0);
    // Only a hang guard (the budgets stop it far sooner); generous for a loaded machine.
    assert!(start.elapsed().as_secs_f64() < 100.0, "{:?}", start.elapsed());
    // The cycle is caught when A needs itself again, and falls back to keyframed values.
    assert!(matches!(r, Ok(Value::Scalar(_))) || r.is_err(), "{r:?}");
}

#[test]
fn an_expression_needing_its_own_value_fails_fast() {
    // Found by the expression fuzzer (#786): rotation's `toWorld` reads the layer's transform,
    // which evaluates rotation's expression again, twice per level (layer-to-comp and world
    // matrices), so the cycle nested to `MAX_DEPTH` 2^16 times and the frame took 20 s.
    let mut f = fx();
    f.set_expr(f.a, ROT, "fromWorld(toWorld(anchorPoint))[0]");
    let start = Instant::now();
    let r = eval_property(&f.p, f.cid, f.a, ROT, 0.5);
    assert!(start.elapsed().as_secs_f64() < 100.0, "{:?}", start.elapsed());
    // The inner read falls back to the keyframed rotation, as in a standalone evaluation.
    assert_eq!(r, f.ev(f.a, ROT, "fromWorld(toWorld(anchorPoint))[0]", 0.5));
}

#[test]
fn fanning_out_reads_stop_at_a_budget() {
    // Two layers reading each other twice at other times: no cycle (the times differ), but
    // 2^16 evaluations at `MAX_DEPTH`; the evaluation stops after `MAX_RUNS` script runs.
    let mut f = fx();
    let text = |other: &str| format!("var r = thisComp.layer('{other}').transform.rotation; r.valueAtTime(time + 0.001) + r.valueAtTime(time + 0.002)");
    f.set_expr(f.a, ROT, &text("B"));
    f.set_expr(f.b, ROT, &text("A"));
    // Take the clock out of the race: on a loaded machine the time budget could otherwise stop
    // the evaluation before the run count does, with a different error.
    crate::runtime::TIME_LIMIT.with(|t| t.set(std::time::Duration::from_secs(3600)));
    let start = Instant::now();
    let r = eval_property(&f.p, f.cid, f.a, ROT, 0.5);
    crate::runtime::TIME_LIMIT.with(|t| t.set(crate::MAX_TIME));
    assert!(start.elapsed().as_secs_f64() < 300.0, "{:?}", start.elapsed());
    assert!(r.unwrap_err().contains("too many"));
    // The budget is per outermost evaluation: the next one starts afresh.
    assert_close!(f.num("time * 2", 1.0), 2.0);
    assert_close!(f.ev(f.b, OPA, "thisComp.layer('A').transform.opacity", 0.0).unwrap().as_f64(), 100.0);
}

#[test]
fn loops_stop_at_a_million_iterations() {
    // Found by the expression fuzzer (#786): `for (…; i < 1e15; …)` ran ten million iterations
    // per run (seconds), once more for every property the script reads after it.
    let f = fx();
    assert_close!(f.num("var n = 0; while (n < 900000) n++; n", 0.0), 900_000.0);
    assert!(f.err("var n = 0; while (n < 2000000) n++; n").contains("iteration"));
    assert!(f.err("for (;;) {}").contains("iteration"));
}

#[test]
fn a_long_running_script_stops_at_the_time_budget() {
    // Found by the expression fuzzer (#786): `smooth(NaN, 1e9)` made a million host reads in
    // one run (tens of seconds), and the loop limit counts per function call, so loops calling
    // functions that loop run for hours. The run is now abandoned after `MAX_TIME`.
    let f = fx();
    let start = Instant::now();
    let e = f.err("function g() { for (var i = 0; i < 900000; i++) {} }\nfor (var k = 0; k < 900000; k++) g()");
    assert!(e.contains("too long"), "{e}");
    assert!(start.elapsed() < crate::MAX_TIME * 12, "{:?}", start.elapsed());
    // The thread's runtime starts afresh.
    assert_close!(f.num("time * 2", 1.0), 2.0);
}

#[test]
fn a_path_with_non_numeric_points_is_an_error() {
    // Found by the expression fuzzer (#786): an array in a vertex became NaN and tripped the
    // rasterizer's clipping assertion when the mask was drawn.
    let mut f = fx();
    let text = "createPath([[0, 0], [[1, 2] * 2, 0], [100, 100]], [], [], true)";
    assert!(f.ev(f.a, "masks/#1/path", text, 0.0).unwrap_err().contains("finite"));
    assert!(f.ev(f.a, "masks/#1/path", "createPath([[0, 0], [1e400, 0], [100, 100]], [], [], true)", 0.0).is_err());
    f.set_expr(f.a, "masks/#1/path", text);
    let host = Expressions;
    let mut r = Renderer::new(&f.p, &NoFootage, RenderOpts { motion_blur: false, ..Default::default() });
    r.expr = Some(&host);
    r.comp_frame(f.cid, Tick::ZERO);
    assert!(f.ev(f.a, "masks/#1/path", "createPath([[0, 0], [10, 0], [10, 10]], [], [], true)", 0.0).is_ok());
}

#[test]
fn errors_and_property_reads_keep_the_runtime_usable() {
    // Errors thrown inside helpers, and the aborts every property read makes until the host
    // answers, must not use up the thread's runtime (it used to fail every expression after a
    // few hundred with "reached the maximum stack size").
    let f = fx();
    for _ in 0..3000 {
        assert!(f.ev(f.b, ROT, "thisComp.layer('Ghost').position", 0.0).is_err());
    }
    for i in 0..1000 {
        let t = i as f64 / 1000.0;
        assert_close!(f.ev(f.b, ROT, "thisComp.layer('A').transform.rotation", t).unwrap().as_f64(), 90.0 * t);
    }
    assert_close!(f.num("time * 2", 1.0), 2.0);
}

#[test]
fn effect_slider_reference() {
    let f = fx();
    assert_close!(f.num("effect(\"Slider Control\")(\"Slider\")", 0.0), 42.0);
    assert_close!(f.num("effect('Slider Control')('Slider') * 2", 0.0), 84.0);
    assert_close!(f.num("thisLayer.effect('Slider Control').param('Slider')", 0.0), 42.0);
    assert_close!(f.num("effect(1).param(1) + effect(1)(1)", 0.0), 84.0);
    assert_close!(f.num("thisComp.layer('A').effect('Slider Control')('Slider').value", 0.0), 42.0);
    assert_close!(f.ev(f.b, ROT, "thisComp.layer('A').effect('Slider Control')('Slider')", 0.0).unwrap().as_f64(), 42.0);
}

#[test]
fn effect_slider_with_expression_drives_another_layer() {
    let mut f = fx();
    f.set_expr(f.a, "effects/#1/slider", "time * 5");
    assert_close!(f.ev(f.b, ROT, "thisComp.layer('A').effect('Slider Control')('Slider')", 2.0).unwrap().as_f64(), 10.0);
}

#[test]
fn missing_effect_is_an_error() {
    let f = fx();
    let e = f.err("effect('Nope')('Slider')");
    assert!(e.contains("Nope"), "{e}");
}

#[test]
fn group_names_and_counts() {
    let f = fx();
    assert_eq!(f.text("thisComp.layer('A').effect(1).name", 0.0), "Slider Control");
    assert_close!(f.ev(f.title, ROT, "thisComp.layer('A').transform.numProperties", 0.0).unwrap().as_f64(), 8.0);
}

#[test]
fn mask_properties() {
    let f = fx();
    assert_close!(f.num("mask('Mask 1').maskOpacity", 0.0), 100.0);
    assert_close!(f.num("mask('Mask 1').maskExpansion + 1", 0.0), 1.0);
}

#[test]
fn shape_content_paths() {
    let f = fx();
    let v = f.ev(f.shape, ROT, "content('Rectangle 1').content('Rectangle Path 1').size[0]", 0.0).unwrap();
    assert_close!(v.as_f64(), 60.0);
    let c = f.ev(f.shape, ROT, "content('Rectangle 1').content('Fill 1').color[0] * 10", 0.0).unwrap();
    assert_close!(c.as_f64(), 10.0);
}

#[test]
fn shape_fill_color_expression() {
    let f = fx();
    let path = "contents/#1/contents/fill/color";
    let v = f.ev(f.shape, path, "[0, 1, 0]", 0.0).unwrap();
    assert_eq!(v, Value::Color([0.0, 1.0, 0.0, 1.0]));
}

#[test]
fn comp_by_name() {
    let f = fx();
    assert_eq!(f.text("comp('Other').layer(1).name", 0.0), "Inner");
    assert_close!(f.num("comp('Other').width", 0.0), 64.0);
    assert!(f.err("comp('Missing').width").contains("Missing"));
}

#[test]
fn parent_attributes() {
    let mut f = fx();
    let b = f.b;
    f.layer_mut(f.a).parent = Some(b);
    assert_eq!(f.text("thisComp.layer('A').parent.name", 0.0), "B");
    assert_close!(f.num("hasParent ? parent.transform.position[0] : -1", 0.0), 100.0);
}

#[test]
fn markers() {
    let f = fx();
    assert_close!(f.num("marker.numKeys", 0.0), 2.0);
    assert_close!(f.num("marker.key(2).time", 0.0), 4.0);
    assert_eq!(f.text("thisComp.layer('A').marker.key(1).comment", 0.0), "beat");
    assert_close!(f.num("marker.key('drop').time", 0.0), 4.0);
    assert_close!(f.num("marker.nearestKey(time).index", 3.5), 2.0);
    assert_close!(f.num("thisComp.marker.key(1).time", 0.0), 1.5);
}

#[test]
fn layer_markers_follow_start_time() {
    let mut f = fx();
    f.layer_mut(f.a).start_time = secs(1.0);
    assert_close!(f.num("marker.key(1).time", 0.0), 3.0);
}

// ---- keyframes, time ---------------------------------------------------------------------------

#[test]
fn value_at_time() {
    let f = fx();
    assert_close!(f.num("valueAtTime(0.5)", 0.0), 45.0);
    assert_close!(f.num("thisProperty.valueAtTime(time - 0.5)", 1.0), 45.0);
    assert_close!(f.ev(f.b, ROT, "thisComp.layer('A').transform.rotation.valueAtTime(0.25)", 0.0).unwrap().as_f64(), 22.5);
}

#[test]
fn value_at_time_respects_layer_start_time() {
    let mut f = fx();
    f.layer_mut(f.a).start_time = secs(1.0);
    // Keys at layer time 0 and 1 → comp time 1 and 2.
    assert_close!(f.num("valueAtTime(1.5)", 0.0), 45.0);
    assert_close!(f.num("key(1).time", 0.0), 1.0);
}

#[test]
fn velocity_and_speed() {
    let f = fx();
    assert_close!(f.num("velocityAtTime(0.5)", 0.0), 90.0, 1e-3);
    assert_close!(f.num("velocity", 0.5), 90.0, 1e-3);
    assert_close!(f.num("speedAtTime(0.5)", 0.0), 90.0, 1e-3);
    assert_close!(f.num("thisComp.layer('B').transform.position.speed", 0.5), 0.0, 1e-6);
}

#[test]
fn keys_and_nearest_key() {
    let f = fx();
    assert_close!(f.num("numKeys", 0.0), 2.0);
    assert_close!(f.num("key(2).time * 1000 + key(2).value", 0.0), 1090.0);
    assert_close!(f.num("nearestKey(0.8).index", 0.0), 2.0);
    assert_close!(f.num("nearestKey(0.2).value", 0.0), 0.0);
    assert!(f.err("key(3).time").contains("out of range"));
}

#[test]
fn loop_out_cycle() {
    let f = fx();
    assert_close!(f.num("loopOut('cycle')", 1.25), 22.5);
    assert_close!(f.num("loopOut()", 2.5), 45.0);
    assert_close!(f.num("loopOut('cycle')", 0.5), 45.0);
}

#[test]
fn loop_out_pingpong() {
    let f = fx();
    assert_close!(f.num("loopOut('pingpong')", 1.25), 67.5);
    assert_close!(f.num("loopOut('pingpong')", 2.25), 22.5);
}

#[test]
fn loop_out_offset() {
    let f = fx();
    assert_close!(f.num("loopOut('offset')", 1.5), 135.0);
    assert_close!(f.num("loopOut('offset')", 3.5), 315.0);
}

#[test]
fn loop_out_continue() {
    let f = fx();
    assert_close!(f.num("loopOut('continue')", 2.0), 180.0, 1e-3);
}

#[test]
fn loop_in_cycle_and_offset() {
    let mut f = fx();
    f.layer_mut(f.a).props.prop_mut(ROT).unwrap().keys = vec![Keyframe::new(secs(1.0), Value::Scalar(0.0)), Keyframe::new(secs(2.0), Value::Scalar(100.0))];
    assert_close!(f.num("loopIn('cycle')", 0.25), 25.0);
    assert_close!(f.num("loopIn('offset')", 0.25), -75.0);
    assert_close!(f.num("loopIn('pingpong')", 0.75), 25.0);
    assert_close!(f.num("loopIn('cycle')", 1.5), 50.0);
}

#[test]
fn loop_out_num_keyframes_and_duration() {
    let mut f = fx();
    let keys =
        vec![Keyframe::new(secs(0.0), Value::Scalar(0.0)), Keyframe::new(secs(1.0), Value::Scalar(100.0)), Keyframe::new(secs(2.0), Value::Scalar(200.0))];
    f.layer_mut(f.a).props.prop_mut(ROT).unwrap().keys = keys;
    // Only the last segment (1 s → 2 s) loops.
    assert_close!(f.num("loopOut('cycle', 1)", 2.5), 150.0);
    assert_close!(f.num("loopOut('cycle', 0)", 2.5), 50.0);
    assert_close!(f.num("loopOutDuration('cycle', 0.5)", 2.25), 175.0);
}

#[test]
fn loop_on_two_dimensional_position() {
    let mut f = fx();
    f.layer_mut(f.a).props.prop_mut(POS).unwrap().keys =
        vec![Keyframe::new(secs(0.0), Value::Vec3([0.0, 0.0, 0.0])), Keyframe::new(secs(1.0), Value::Vec3([100.0, 50.0, 0.0]))];
    let p = f.pos("loopOut('offset')", 1.5);
    assert!(p.iter().zip([150.0, 75.0, 0.0]).all(|(a, b)| (a - b).abs() < 1e-9), "{p:?}");
}

#[test]
fn smooth_on_linear_ramp_is_identity() {
    let f = fx();
    assert_close!(f.num("smooth(0.2, 5)", 0.5), 45.0, 1e-6);
}

#[test]
fn time_conversions() {
    let f = fx();
    assert_close!(f.num("timeToFrames()", 2.0), 60.0);
    assert_close!(f.num("timeToFrames(1, 24)", 0.0), 24.0);
    assert_close!(f.num("framesToTime(45)", 0.0), 1.5);
    assert_eq!(f.text("timeToTimecode()", 61.5), "0:01:01:15");
    assert_eq!(f.text("timeToCurrentFormat(2.5)", 0.0), "0:00:02:15");
}

#[test]
fn posterize_time() {
    let f = fx();
    assert_close!(f.num("posterizeTime(2); time", 0.7), 0.5);
    assert_close!(f.num("posterizeTime(2); value", 0.7), 45.0);
}

// ---- wiggle / random / noise ------------------------------------------------------------------

#[test]
fn wiggle_is_deterministic() {
    let f = fx();
    let a = f.pos("wiggle(3, 25)", 1.234);
    let b = f.pos("wiggle(3, 25)", 1.234);
    assert_eq!(a, b);
    let g = fx();
    assert_eq!(g.pos("wiggle(3, 25)", 1.234), a);
    let other_thread = std::thread::spawn(move || g.pos("wiggle(3, 25)", 1.234)).join().unwrap();
    assert_eq!(other_thread, a);
}

#[test]
fn wiggle_amplitude_bounds_and_motion() {
    let f = fx();
    let mut seen = Vec::new();
    for i in 0..120 {
        let p = f.pos("wiggle(5, 30)", i as f64 / 30.0);
        assert!((p[0] - 50.0).abs() <= 30.0 + 1e-9 && (p[1] - 40.0).abs() <= 30.0 + 1e-9, "{p:?}");
        assert_eq!(p[2], 0.0, "2D layers keep z");
        seen.push(p[0]);
    }
    let spread = seen.iter().cloned().fold(f64::MIN, f64::max) - seen.iter().cloned().fold(f64::MAX, f64::min);
    assert!(spread > 10.0, "wiggle should move: spread {spread}");
}

#[test]
fn wiggle_octaves_bound() {
    let f = fx();
    for i in 0..60 {
        let r = f.num("wiggle(2, 10, 3, 0.5) - value", i as f64 / 10.0);
        assert!(r.abs() <= 10.0 * 1.75 + 1e-9, "{r}");
    }
}

#[test]
fn wiggle_differs_per_layer_and_dimension() {
    let f = fx();
    let a = f.ev(f.a, OPA, "wiggle(2, 20)", 0.7).unwrap().as_f64();
    let b = f.ev(f.b, OPA, "wiggle(2, 20)", 0.7).unwrap().as_f64();
    assert_ne!(a, b);
    let p = f.pos("wiggle(2, 20) - value", 0.7);
    assert_ne!(p[0], p[1]);
}

#[test]
fn wiggle_on_other_layer_property() {
    let f = fx();
    let v = f.num("thisComp.layer('B').transform.rotation.wiggle(1, 5)", 0.3);
    assert!(v.abs() <= 5.0 && v != 0.0, "{v}");
}

#[test]
fn temporal_wiggle_stays_on_the_curve() {
    let f = fx();
    for i in 0..10 {
        let v = f.num("temporalWiggle(2, 0.1)", 0.2 + i as f64 * 0.05);
        assert!((0.0..=90.0).contains(&v), "{v}");
    }
}

#[test]
fn random_is_seeded_and_in_range() {
    let f = fx();
    let a = f.num("random()", 0.5);
    assert!((0.0..1.0).contains(&a));
    assert_eq!(a, f.num("random()", 0.5));
    assert_ne!(a, f.num("random()", 0.6));
    let r = f.num("random(10, 20)", 0.0);
    assert!((10.0..20.0).contains(&r));
    let p = f.pos("random([100, 50])", 0.0);
    assert!((0.0..100.0).contains(&p[0]) && (0.0..50.0).contains(&p[1]));
    assert_ne!(f.num("random() - random()", 0.0), 0.0, "successive calls differ");
}

#[test]
fn seed_random_timeless() {
    let f = fx();
    let a = f.num("seedRandom(5, true); random(100)", 0.1);
    let b = f.num("seedRandom(5, true); random(100)", 3.7);
    assert_eq!(a, b);
    assert_ne!(a, f.num("seedRandom(6, true); random(100)", 0.1));
}

#[test]
fn gauss_random_distribution() {
    let f = fx();
    let e =
        "var s = 0, n = 2000, inside = 0;\nfor (var i = 0; i < n; i++) { var g = gaussRandom(); s += g; if (g >= 0 && g <= 1) inside++; }\n[s / n, inside / n]";
    let r = f.pos(e, 0.0);
    assert!((r[0] - 0.5).abs() < 0.05, "mean {}", r[0]);
    assert!((0.85..0.95).contains(&r[1]), "inside {}", r[1]);
}

#[test]
fn noise_is_smooth_and_bounded() {
    let f = fx();
    let a = f.num("noise(time)", 1.0);
    let b = f.num("noise(time)", 1.001);
    assert!(a.abs() <= 1.0 && (a - b).abs() < 0.01);
    assert!(f.num("noise([1.5, 2.5, 3.5])", 0.0).abs() <= 1.0);
}

// ---- text -------------------------------------------------------------------------------------

#[test]
fn source_text_from_expression_keeps_style() {
    let f = fx();
    let v = f.ev(f.title, "text/sourceText", "'Frame ' + timeToFrames()", 1.0).unwrap();
    let Value::Text(doc) = v else { panic!() };
    assert_eq!(doc.text, "Frame 30");
    assert_eq!(doc.size, 40.0, "style from the keyframed document");
}

#[test]
fn source_text_reading_and_string_methods() {
    let f = fx();
    assert_eq!(f.text("value + '!'", 0.0), "Hello!");
    assert_eq!(f.text("thisComp.layer('Title').text.sourceText + ' world'", 0.0), "Hello world");
    assert_eq!(f.text("text.sourceText.toUpperCase()", 0.0), "HELLO");
    assert_close!(f.num("thisComp.layer('Title').text.sourceText.length", 0.0), 5.0);
}

#[test]
fn source_text_style_api() {
    let mut f = fx();
    let doc = |f: &Fx, e: &str| -> TextDoc {
        match f.ev(f.title, "text/sourceText", e, 0.0) {
            Ok(Value::Text(d)) => *d,
            r => panic!("{e}: {r:?}"),
        }
    };
    // Getters read the first character's style.
    assert_close!(f.num("thisComp.layer('Title').text.sourceText.style.fontSize", 0.0), 40.0);
    assert_close!(f.num("thisComp.layer('Title').text.sourceText.style.isAllCaps ? 1 : 0", 0.0), 0.0);
    assert_close!(f.num("thisComp.layer('Title').text.sourceText.style.autoLeading ? 1 : 0", 0.0), 1.0);
    assert_close!(f.num("thisComp.layer('Title').text.sourceText.style.leading", 0.0), 48.0);
    assert_close!(f.num("thisComp.layer('Title').text.sourceText.style.fillColor[1]", 0.0), 1.0);
    assert_close!(f.num("thisComp.layer('Title').text.sourceText.style.setFontSize(12).fontSize", 0.0), 12.0);
    // Setters chain and return a styled document.
    let d = doc(&f, "text.sourceText.style.setFontSize(80).setFillColor([1, 0, 0]).setAllCaps(true)");
    assert_eq!(d.text, "Hello");
    assert!(d.is_uniform());
    assert_eq!((d.size, d.fill, d.all_caps), (80.0, [1.0, 0.0, 0.0, 1.0], true));
    // Character ranges create runs; setText replaces the text.
    let d = doc(&f, "text.sourceText.style.setText('Hi there').setFontSize(20, 3, 5).setFauxBold(true, 0, 2)");
    assert_eq!(d.text, "Hi there");
    let sizes: Vec<(usize, f64, bool)> = d.runs().iter().map(|r| (r.len, r.style.size, r.style.faux_bold)).collect();
    assert_eq!(sizes, vec![(2, 40.0, true), (1, 40.0, false), (5, 20.0, false)]);
    // getStyleAt reads a styled document per character (and at a time).
    {
        let l = f.layer_mut(f.title);
        let p = l.props.prop_mut("text/sourceText").unwrap();
        let Value::Text(d) = &mut p.value else { panic!() };
        d.apply_style(1..3, |s| s.size = 99.0);
    }
    assert_close!(f.num("thisComp.layer('Title').text.sourceText.getStyleAt(2, 0).fontSize", 0.0), 99.0);
    assert_close!(f.num("thisComp.layer('Title').text.sourceText.getStyleAt(4).fontSize", 0.0), 40.0);
    // Returning the own style keeps its runs; plain strings keep the first character's style.
    let d = doc(&f, "text.sourceText.style");
    assert_eq!(d.runs().len(), 3);
    let d = doc(&f, "'Bye'");
    assert_eq!((d.text.as_str(), d.size), ("Bye", 40.0));
    // Paragraph setters.
    let d = doc(&f, "text.sourceText.style.setJustification('CENTER_JUSTIFY').setStartIndent(12).setSpaceBefore(4).setEveryLineComposer(false)");
    assert_eq!(d.justify, effectcraft_keyframe::Justify::Center);
    assert_eq!((d.indent_left, d.space_before), (12.0, 4.0));
    assert_close!(f.num("thisComp.layer('Title').text.sourceText.style.setStartIndent(7).startIndent", 0.0), 7.0);
    // createStyle sets only what's given and keeps the existing runs.
    let d = doc(&f, "thisComp.layer('Title').text.sourceText.createStyle().setTsume(50).setBaselineOption('superscript')");
    assert_eq!(d.text, "Hello");
    assert!(d.runs().iter().all(|r| r.style.tsume == 50.0 && r.style.baseline == effectcraft_keyframe::BaselineOption::Superscript));
    assert_eq!(d.runs().len(), 3);
    // Non-text properties have no style.
    assert!(f.ev(f.a, ROT, "transform.rotation.style.fontSize", 0.0).is_err());
}

#[test]
fn numbers_to_source_text() {
    let f = fx();
    assert_eq!(f.text("Math.round(time * 10) / 10", 1.25), "1.3");
    assert_eq!(f.text("(time * 2).toFixed(2)", 1.0), "2.00");
    assert_eq!(f.text("12", 0.0), "12");
}

// ---- space conversions ------------------------------------------------------------------------

#[test]
fn to_comp_and_from_comp() {
    let f = fx();
    // A: 20×20 solid, anchor (10, 10), position (50, 40).
    assert_eq!(f.pos("toComp([0, 0])", 0.0), [40.0, 30.0, 0.0]);
    assert_eq!(f.pos("fromComp([50, 40])", 0.0), [10.0, 10.0, 0.0]);
    // Rotation keyed to 90° at 1 s.
    let p = f.pos("toComp([20, 10])", 1.0);
    assert!(close(p[0], 50.0) && close(p[1], 50.0), "{p:?}");
    let q = f.pos("toWorld([10, 10, 0])", 0.0);
    assert!(close(q[0], 50.0) && close(q[1], 40.0), "{q:?}");
    let v = f.pos("toCompVec([1, 0])", 1.0);
    assert!(close(v[0], 0.0) && close(v[1], 1.0), "{v:?}");
}

#[test]
fn source_rect_at_time() {
    let f = fx();
    assert_eq!(f.pos("var r = sourceRectAtTime(); [r.width, r.height]", 0.0), [20.0, 20.0, 0.0]);
    let t = f.ev(f.title, POS, "var r = sourceRectAtTime(); [r.width, r.height]", 0.0).unwrap().as_vec3();
    assert!(t[0] > 40.0 && t[1] > 15.0, "{t:?}");
    let s = f.ev(f.shape, POS, "var r = sourceRectAtTime(); [r.left, r.top, r.width]", 0.0).unwrap().as_vec3();
    assert!((s[0] + 31.0).abs() < 2.0 && (s[1] + 16.0).abs() < 2.0 && (s[2] - 62.0).abs() < 3.0, "{s:?}");
}

#[test]
fn source_dimensions_follow_media_replacement() {
    let mut p = Project::default();
    let comp = Comp::new(200, 100, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let cid = p.add_item("Main", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let footage = |p: &mut Project, name: &str, width, height| {
        p.add_item(
            name,
            Label::SeaFoam,
            None,
            ItemKind::Footage(effectcraft_project::Footage {
                path: name.into(),
                kind: effectcraft_project::FootageKind::Still,
                width,
                height,
                has_video: true,
                ..Default::default()
            }),
        )
    };
    let original = footage(&mut p, "original.png", 100, 80);
    let replacement = footage(&mut p, "replacement.png", 30, 40);
    let mut l = build::layer(&mut p, &comp, "Clip", LayerSource::Footage { item: original }, (100, 80), None);
    l.props.prop_mut(ROT).unwrap().expr =
        Some(Expression { text: "thisLayer.width + thisLayer.height + thisLayer.source.width + sourceRectAtTime().width".into(), enabled: true });
    let lid = l.id;
    p.comp_mut(cid).unwrap().layers.push(l);
    let c = p.comp(cid).unwrap();
    let l = c.layer(lid).unwrap();
    let media = effectcraft_render::MediaOverride::new(lid, replacement);
    let ctx = EvalCtx {
        project: &p,
        comp_id: cid,
        comp: c,
        time: Tick::ZERO,
        expr: Some(&Expressions),
        footage: None,
        property_overrides: &[],
        media_overrides: &[media],
    };
    assert_eq!(ctx.value(l, l.props.prop(ROT).unwrap()), Value::Scalar(130.0));
}

// ---- colour -----------------------------------------------------------------------------------

#[test]
fn colour_conversions() {
    let f = fx();
    let hsl = f.pos("rgbToHsl([1, 0, 0, 1])", 0.0);
    assert_eq!(hsl, [0.0, 1.0, 0.5]);
    let back = f.pos("hslToRgb([2 / 3, 1, 0.5, 1])", 0.0);
    assert!(close(back[0], 0.0) && close(back[1], 0.0) && close(back[2], 1.0), "{back:?}");
    let hex = f.pos("hexToRgb('#FF8000')", 0.0);
    assert!(close(hex[0], 1.0) && close(hex[1], 128.0 / 255.0) && close(hex[2], 0.0), "{hex:?}");
}

#[test]
fn colour_result_keeps_alpha() {
    let f = fx();
    let path = "contents/#1/contents/fill/color";
    let v = f.ev(f.shape, path, "hslToRgb([0, 0, 0.5])", 0.0).unwrap();
    assert_eq!(v, Value::Color([0.5, 0.5, 0.5, 1.0]));
}

// ---- result conversion and errors -------------------------------------------------------------

#[test]
fn checkbox_and_dropdown_results() {
    let mut f = fx();
    let spec = effectcraft_effects::find("ec.control.checkbox").unwrap();
    let cb = group_push(&mut f.p, |ids| effectcraft_effects::instantiate(spec, ids, "Checkbox Control", [20.0, 20.0]));
    let spec = effectcraft_effects::find("ec.control.dropdown").unwrap();
    let dd = group_push(&mut f.p, |ids| effectcraft_effects::instantiate(spec, ids, "Dropdown Menu Control", [20.0, 20.0]));
    let fxg = f.layer_mut(f.b).props.sub_mut("effects").unwrap();
    fxg.children.push(cb.into());
    fxg.children.push(dd.into());
    assert_eq!(f.ev(f.b, "effects/#1/checkbox", "time > 1", 2.0).unwrap(), Value::Bool(true));
    assert_eq!(f.ev(f.b, "effects/#1/checkbox", "0", 2.0).unwrap(), Value::Bool(false));
    assert_eq!(f.ev(f.b, "effects/#2/menu", "3", 0.0).unwrap(), Value::Enum(2));
    assert_close!(f.ev(f.b, ROT, "effect('Dropdown Menu Control')('Menu') * 10", 0.0).unwrap().as_f64(), 10.0);
}

#[test]
fn syntax_error_reports_line() {
    let f = fx();
    let e = f.err("var a = 1;\nvar b = ;\na");
    assert!(e.contains("line 2"), "{e}");
    assert!(check_syntax("var a = (1;").is_err());
    assert!(check_syntax("value + [1, 2]").is_ok());
}

#[test]
fn runtime_error_reports_line() {
    let f = fx();
    let e = f.err("var a = 1;\nfoo.bar + a");
    assert!(e.contains("foo"), "{e}");
    assert!(e.contains("line 2"), "{e}");
}

#[test]
fn thrown_errors_and_missing_layers() {
    let f = fx();
    assert!(f.err("throw new Error('custom failure')").contains("custom failure"));
    assert!(f.err("thisComp.layer('Ghost').position").contains("Ghost"));
    assert!(f.err("thisComp.layer(99).name").contains("99"));
}

#[test]
fn result_kind_errors() {
    let f = fx();
    assert!(f.ev(f.a, POS, "5", 0.0).unwrap_err().contains("dimension"));
    assert!(f.err("'abc'").contains("number"));
    assert!(f.err("var x = 1").contains("undefined"));
    assert!(f.err("0 / 0").contains("finite"));
}

#[test]
fn shadowing_layer_attribute_names_is_local() {
    let f = fx();
    assert_close!(f.num("var width = 3;\nwidth * 2", 0.0), 6.0);
    assert_close!(f.num("width", 0.0), 20.0, 1e-9);
    assert_close!(f.num("function width() { return 4; }\nwidth() * 2", 0.0), 8.0);
    assert_close!(f.num("width", 0.0), 20.0, 1e-9);
}

#[test]
fn try_catch_does_not_swallow_host_reads() {
    let f = fx();
    assert_close!(f.num("try { thisComp.layer('B').transform.position[0] } catch (e) { -1 }", 0.0), 100.0);
    assert_close!(f.num("try { thisComp.layer('Ghost').name; 1 } catch (e) { -1 }", 0.0), -1.0);
}

#[test]
fn loops_over_layers() {
    let f = fx();
    let e = "var s = 0;\nfor (var i = 1; i <= thisComp.numLayers; i++) s += thisComp.layer(i).transform.position[0];\ns";
    assert_close!(f.num(e, 0.0), 50.0 + 100.0 + 20.0 + 100.0);
}

#[test]
fn infinite_loops_are_stopped() {
    let f = fx();
    let e = f.err("while (true) {}");
    assert!(!e.is_empty());
}

#[test]
fn renderer_uses_expressions_through_eval_ctx() {
    let mut f = fx();
    f.set_expr(f.b, ROT, "time * 360");
    let host = Expressions;
    let comp = f.p.comp(f.cid).unwrap();
    let b = comp.layer(f.b).unwrap();
    let prop = b.props.prop(ROT).unwrap();
    let mut ctx = EvalCtx::new(&f.p, f.cid, comp, secs(0.25));
    assert_eq!(ctx.value(b, prop), Value::Scalar(0.0), "no host: keyframed value");
    ctx.expr = Some(&host);
    assert_eq!(ctx.value(b, prop), Value::Scalar(90.0));
}

#[test]
fn disabled_expression_is_ignored() {
    let mut f = fx();
    f.set_expr(f.b, ROT, "time * 360");
    f.layer_mut(f.b).props.prop_mut(ROT).unwrap().expr.as_mut().unwrap().enabled = false;
    assert_eq!(eval_property(&f.p, f.cid, f.b, ROT, 0.25).unwrap(), Value::Scalar(0.0));
}

// ---- render & performance ---------------------------------------------------------------------

#[test]
fn expression_drives_rotation_in_a_rendered_frame() {
    let mut p = Project::default();
    let comp = Comp::new(100, 100, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    // A 80×10 horizontal bar at the centre; rotating 90° makes it vertical.
    let mut bar = solid(&mut p, &comp, "Bar", [1.0, 1.0, 1.0], 80, 10);
    bar.props.prop_mut(ROT).unwrap().expr = Some(Expression { text: "time * 360".into(), enabled: true });
    p.comp_mut(cid).unwrap().layers.push(bar);
    let host = Expressions;
    let mut r = Renderer::new(&p, &NoFootage, RenderOpts { motion_blur: false, ..Default::default() });
    let still = r.comp_frame(cid, Tick::ZERO);
    assert!(still.get(85, 50)[3] > 0.99 && still.get(50, 85)[3] < 0.01, "horizontal at t=0");
    let no_expr = r.comp_frame(cid, secs(0.25));
    assert!(no_expr.get(85, 50)[3] > 0.99, "without a host the bar stays put");
    r.expr = Some(&host);
    let img = r.comp_frame(cid, secs(0.25));
    assert!(img.get(50, 85)[3] > 0.99, "vertical at t=0.25 (90°)");
    assert!(img.get(85, 50)[3] < 0.01);
}

#[test]
fn hundred_wiggles_are_fast() {
    let mut p = Project::default();
    let comp = Comp::new(1920, 1080, FrameRate::FPS_30, Tick::from_seconds_f64(10.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    for i in 0..100 {
        let mut l = solid(&mut p, &comp, &format!("L{i}"), [1.0, 1.0, 1.0], 10, 10);
        l.props.prop_mut(POS).unwrap().expr = Some(Expression { text: "wiggle(2, 50)".into(), enabled: true });
        p.comp_mut(cid).unwrap().layers.push(l);
    }
    let host = Expressions;
    let c = p.comp(cid).unwrap();
    let frame = |t: f64| {
        let mut ctx = EvalCtx::new(&p, cid, c, secs(t));
        ctx.expr = Some(&host);
        c.layers.iter().map(|l| ctx.value(l, l.props.prop(POS).unwrap()).as_vec3()[0]).sum::<f64>()
    };
    frame(0.0); // warm up: runtime + compile
    let n = 20;
    let start = Instant::now();
    let mut acc = 0.0;
    for i in 0..n {
        acc += frame(i as f64 / 30.0);
    }
    let per_frame = start.elapsed().as_secs_f64() * 1000.0 / n as f64;
    println!("100 wiggle properties: {per_frame:.3} ms/frame (checksum {acc})");
    // Generous bound so unoptimised CI builds pass; release is far below.
    assert!(per_frame < 1000.0, "{per_frame} ms/frame");
}

#[test]
fn error_messages_point_at_the_users_line() {
    let f = fx();
    assert_eq!(f.err("var a = 1;\n\nthisComp.layer('Ghost').position"), "Error at line 3: Layer named \"Ghost\" does not exist");
    assert_eq!(f.err("var a = 1;\nfoo.bar + a"), "Error at line 2: ReferenceError: foo is not defined");
    // An unfinished last line is reported on that line, not on the hidden wrapper.
    assert!(f.err("5 +").starts_with("Error at line 1: SyntaxError"));
}

/// Text Expression Selector: Amount evaluated per unit with textIndex / textTotal /
/// selectorValue.
#[test]
fn text_expression_selector_values() {
    let mut p = Project::default();
    let comp = Comp::new(400, 200, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let cid = p.add_item("Main", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let mut l = build::layer(&mut p, &comp, "T", LayerSource::Text, (400, 200), None);
    l.props.prop_mut("text/sourceText").unwrap().value = Value::Text(Box::new(TextDoc { text: "ABCD".into(), size: 40.0, ..Default::default() }));
    let mut next = p.next_id;
    let mut ids = Ids(&mut next);
    let props = build::text_anim_props(&mut ids, "opacity", false);
    let mut a = build::text_animator(&mut ids, "Animator 1", props);
    let sel = build::expression_selector(&mut ids, "Expression Selector 1");
    a.sub_mut("selectors").unwrap().children.push(sel.into());
    p.next_id = next;
    l.props.group_mut("text/animators").unwrap().children.push(a.into());
    let lid = l.id;
    p.comp_mut(cid).unwrap().layers.push(l);
    let sel_of = |p: &Project| {
        let c = p.comp(cid).unwrap();
        let l = c.layer(lid).unwrap();
        let ctx = EvalCtx {
            project: p,
            comp_id: cid,
            comp: c,
            time: Tick::ZERO,
            expr: Some(&Expressions),
            footage: None,
            property_overrides: &[],
            media_overrides: &[],
        };
        let doc = effectcraft_render::text::source_text(&ctx, l).unwrap();
        let lay = effectcraft_text::layout_doc(&doc);
        let anim = l.props.group("text/animators/#1").unwrap();
        effectcraft_render::text::animator_selection(&ctx, l, anim, &lay).into_iter().map(|v| (v[0] * 1000.0).round() / 1000.0).collect::<Vec<_>>()
    };
    // Default: selectorValue * textIndex / textTotal (selectorValue = the full range selector).
    assert_eq!(sel_of(&p), vec![0.25, 0.5, 0.75, 1.0]);
    let set_expr = |p: &mut Project, text: &str| {
        let l = p.comp_mut(cid).unwrap().layer_mut(lid).unwrap();
        l.props.prop_mut("text/animators/#1/selectors/#2/amount").unwrap().expr = Some(Expression { text: text.into(), enabled: true });
    };
    // Range selector covering the first half → selectorValue 0 for the second half.
    p.comp_mut(cid).unwrap().layer_mut(lid).unwrap().props.prop_mut("text/animators/#1/selectors/#1/end").unwrap().value = Value::Scalar(50.0);
    assert_eq!(sel_of(&p), vec![0.25, 0.5, 0.0, 0.0]);
    // Plain numbers, conditionals on textIndex (1-based) and arrays all work.
    p.comp_mut(cid).unwrap().layer_mut(lid).unwrap().props.prop_mut("text/animators/#1/selectors/#1/end").unwrap().value = Value::Scalar(100.0);
    set_expr(&mut p, "textIndex % 2 == 0 ? 100 : 0");
    assert_eq!(sel_of(&p), vec![0.0, 1.0, 0.0, 1.0]);
    set_expr(&mut p, "[50, 0, 0]");
    assert_eq!(sel_of(&p), vec![0.5; 4]);
    // The result replaces the selection so far (which it reads as selectorValue).
    set_expr(&mut p, "selectorValue - textIndex / textTotal * 100");
    assert_eq!(sel_of(&p), vec![0.75, 0.5, 0.25, 0.0]);
    // Errors fall back to the static amount (100%).
    set_expr(&mut p, "nope(");
    assert_eq!(sel_of(&p), vec![1.0; 4]);
    // Outside a selector the expression still evaluates (textIndex = textTotal = 1).
    set_expr(&mut p, build::EXPRESSION_SELECTOR_DEFAULT);
    let v = eval_property(&p, cid, lid, "text/animators/#1/selectors/#2/amount", 0.0).unwrap();
    assert_eq!(v, Value::Vec3([100.0; 3]));
}

/// `sampleImage` reads the layer's rendered pixels (layer space, straight colour) after masks
/// and effects, or the bare source with `postEffect = false`; the radius averages a box.
#[test]
fn sample_image_reads_rendered_pixels() {
    let f = fx();
    let code = |c: &str| format!("var c = {c}; c[0] * 1000 + c[1] * 100 + c[2] * 10 + c[3]");
    // B: 40×40 blue solid.
    assert_close!(f.num(&code("thisComp.layer(\"B\").sampleImage([20, 20])"), 0.0), 11.0);
    // Half the box is outside the layer: alpha 0.5, colour still pure blue (straight).
    assert_close!(f.num(&code("thisComp.layer(\"B\").sampleImage([0, 20], [1, 1])"), 0.0), 10.5);
    // A: red with a mask covering 5..15: outside the mask it's transparent after masks, red before.
    assert_close!(f.num(&code("thisComp.layer(\"A\").sampleImage([2, 2])"), 0.0), 0.0);
    assert_close!(f.num(&code("thisComp.layer(\"A\").sampleImage([2, 2], [0.5, 0.5], false)"), 0.0), 1001.0);
    assert_close!(f.num(&code("thisComp.layer(\"A\").sampleImage([10, 10])"), 0.0), 1001.0);
    // The result has four values; outside the layer it's transparent.
    assert_close!(f.num("thisComp.layer(\"B\").sampleImage([20, 20]).length", 0.0), 4.0);
    assert_close!(f.num(&code("thisComp.layer(\"B\").sampleImage([-50, -50])"), 0.0), 0.0);
}

/// A huge sample point saturates the pixel coordinates; it must not overflow the loop bounds.
#[test]
fn sample_image_huge_point_is_transparent() {
    let f = fx();
    assert_close!(f.num("var c = thisComp.layer(\"B\").sampleImage([1e300, 1e300]); c[3]", 0.0), 0.0);
}

fn data_item(p: &mut Project, name: &str, text: &str) {
    let f = effectcraft_project::Footage {
        path: format!("/data/{name}"),
        kind: effectcraft_project::FootageKind::Data,
        data: Some(text.into()),
        ..Default::default()
    };
    p.add_item(name, Label::Sandstone, None, ItemKind::Footage(f));
}

/// Data-driven animation: `footage(name)` with `sourceData` / `sourceText` / `dataValue` for
/// JSON, CSV and TSV items.
#[test]
fn data_footage_json_csv_tsv() {
    let mut f = fx();
    data_item(&mut f.p, "stats.json", r#"{"title": "Q3", "items": [{"v": 3}, {"v": 7.5}]}"#);
    data_item(&mut f.p, "scores.csv", "name,score\n\"Smith, J\",12\nLee,30.5\n");
    data_item(&mut f.p, "table.tsv", "a\tb\n1\t2\n3\t4\n");
    assert_close!(f.num("footage(\"stats.json\").sourceData.items[1].v", 0.0), 7.5);
    assert_eq!(f.text("footage(\"stats.json\").sourceData.title", 0.0), "Q3");
    assert_close!(f.num("footage(\"stats.json\").dataValue([\"items\", 0, \"v\"])", 0.0), 3.0);
    assert_close!(f.num("footage(\"scores.csv\").sourceData[1].score", 0.0), 30.5);
    assert_eq!(f.text("footage(\"scores.csv\").sourceData[0].name", 0.0), "Smith, J");
    assert_close!(f.num("footage(\"scores.csv\").dataValue([1, 0])", 0.0), 12.0);
    assert_close!(f.num("footage(\"scores.csv\").dataKeyCount", 0.0), 2.0);
    assert_eq!(f.text("footage(\"scores.csv\").dataKeyNames.join('|')", 0.0), "name|score");
    assert_close!(f.num("footage(\"table.tsv\").dataValue([1, 1])", 0.0), 4.0);
    assert_close!(f.num("footage(\"table.tsv\").sourceText.length", 0.0), 12.0);
    assert!(f.err("footage(\"missing.json\").sourceData").contains("does not exist"));
    // Animated from data: the value follows the CSV row picked by time.
    assert_close!(f.num("footage(\"scores.csv\").dataValue([1, Math.floor(time)])", 1.2), 30.5);
}

#[test]
fn project_and_time_globals() {
    let mut f = fx();
    assert_close!(f.num("thisProject.bitsPerChannel", 0.0), 8.0);
    assert_close!(f.num("colorDepth", 0.0), 8.0);
    assert_close!(f.num("thisProject.linearBlending ? 1 : 0", 0.0), 0.0);
    assert_eq!(f.text("timeToFeetAndFrames(2)", 0.0), "3+12");
    assert_close!(f.num("posterizeTime(4); time", 0.6), 0.5);
    // Path API.
    assert_close!(f.num("createPath([[0, 0], [10, 0], [10, 10]], [], [], true).points()[2][1]", 0.0), 10.0);
    assert_close!(f.num("createPath([[0, 0], [10, 0]], [[1, 1], [2, 2]], [], false).inTangents()[1][0]", 0.0), 2.0);
    assert_close!(f.num("mask(\"Mask 1\").maskPath.points().length", 0.0), 4.0);
    assert_close!(f.num("createPath([[0, 0], [10, 0], [10, 10]], [], [], false).pointOnPath(0.75)[1]", 0.0), 5.0);
    assert_close!(f.num("createPath([[0, 0], [10, 0], [10, 10]], [], [], false).pointOnPath(0.75)[0]", 0.0), 10.0);
    assert_close!(f.num("createPath([[0, 0], [10, 0], [10, 10]], [], [], false).tangentOnPath(0.25)[0]", 0.0), 1.0);
    assert_close!(f.num("mask(\"Mask 1\").maskPath.pointOnPath(0)[0] - mask(\"Mask 1\").maskPath.points()[0][0]", 0.0), 0.0);
    assert_close!(f.num("mask(\"Mask 1\").maskPath.isClosed() ? 1 : 0", 0.0), 1.0);
    // Marker keys carry protected regions and cue points.
    {
        let a = f.a;
        let l = f.layer_mut(a);
        l.markers[0].protected = true;
        l.markers[0].cue_point = Some(effectcraft_project::CuePoint { name: "go".into(), navigation: false, params: vec![("k".into(), "v".into())] });
    }
    assert_close!(f.num("thisLayer.marker.key(1).protectedRegion ? 1 : 0", 0.0), 1.0);
    assert_eq!(f.text("thisComp.layer(\"A\").marker.key(1).cuePointName + thisComp.layer(\"A\").marker.key(1).parameters.k", 0.0), "gov");
}

#[test]
fn thousands_of_host_reads_on_one_thread_keep_working() {
    // Every pending host request unwinds through call frames, which leaks boa stack slots; the
    // runtime replaces a context that fills up instead of failing every later expression.
    let f = fx();
    for i in 0..2000 {
        let t = (i % 20) as f64 * 0.05;
        assert_eq!(f.num("effect(\"Slider Control\")(\"Slider\")", t), 42.0, "evaluation {i}");
        assert!(close(f.num("thisComp.layer(\"A\").transform.rotation.valueAtTime(1)", t), 90.0), "evaluation {i}");
        assert_eq!(f.num("function a() { return effect(1)(1); } function b() { return a(); } b() + 1", t), 43.0, "evaluation {i}");
    }
}

#[test]
fn pending_text_style_baseline_setters_are_visible_to_expression_getters() {
    let x = fx();
    assert_eq!(x.num("thisComp.layer('Title').text.sourceText.style.setSuperscript(true).isSuperscript ? 1 : 0", 0.0), 1.0);
    assert_eq!(x.num("thisComp.layer('Title').text.sourceText.style.setSubscript(true).isSubscript ? 1 : 0", 0.0), 1.0);
    assert_eq!(x.num("thisComp.layer('Title').text.sourceText.style.setSuperscript(true).setSubscript(true).isSuperscript ? 1 : 0", 0.0), 0.0);
    assert_eq!(x.num("thisComp.layer('Title').text.sourceText.style.setSuperscript(true).setSubscript(true).isSubscript ? 1 : 0", 0.0), 1.0);
    assert_eq!(x.num("thisComp.layer('Title').text.sourceText.style.setSubscript(true).setSubscript(false).isSubscript ? 1 : 0", 0.0), 0.0);
    assert_eq!(x.num("thisComp.layer('Title').text.sourceText.style.setBaselineOption('superscript').isSuperscript ? 1 : 0", 0.0), 1.0);
    assert_eq!(x.text("var s = text.sourceText.style.setSuperscript(true); s.setText(s.isSuperscript ? 'super' : 'normal')", 0.0), "super");
}

#[test]
fn layer_source_is_the_layers_source_item() {
    let mut f = fx();
    // A solid's source is an item of the solid's size and name (#809).
    assert_close!(f.num("thisComp.layer('A').source.width", 0.0), 20.0);
    assert_close!(f.num("thisComp.layer('A').source.height", 0.0), 20.0);
    assert_close!(f.num("thisComp.layer('A').source.name == 'A' ? 1 : 0", 0.0), 1.0);
    // Text and shape layers have no source item.
    assert_close!(f.num("thisComp.layer('Title').source == null ? 1 : 0", 0.0), 1.0);
    // A precomp layer's source is its composition.
    let main = f.p.comp(f.cid).unwrap().clone();
    let other = f.p.items.values().find(|i| i.name == "Other").unwrap().id;
    let pre = build::layer(&mut f.p, &main, "Pre", LayerSource::Comp { item: other }, (64, 32), None);
    f.p.comp_mut(f.cid).unwrap().layers.push(pre);
    assert_close!(f.num("thisComp.layer('Pre').source.width", 0.0), 64.0);
    assert_close!(f.num("thisComp.layer('Pre').source.numLayers", 0.0), 1.0);
}

#[test]
fn layer_control_property_stands_for_its_layer() {
    let mut f = fx();
    // Layer A gets a Layer Control ("Map") that points at layer B (#809).
    let spec = effectcraft_effects::find("ec.control.layer").unwrap();
    let mut map = group_push(&mut f.p, |ids| effectcraft_effects::instantiate(spec, ids, "Map", [20.0, 20.0]));
    map.prop_mut("layer").unwrap().value = Value::Layer(Some(f.b.0));
    f.p.comp_mut(f.cid).unwrap().layers[0].props.sub_mut("effects").unwrap().children.push(map.into());
    // effect("Map")(1) is the layer itself: its name, index, transform and value.
    assert_close!(f.num("effect('Map')(1).name == 'B' ? 1 : 0", 0.0), 1.0);
    assert_close!(f.num("effect('Map')(1).index", 0.0), 2.0);
    assert_close!(f.num("effect('Map')(1).transform.position[0]", 0.0), 100.0);
    assert_close!(f.num("effect('Map')(1).transform.scale[0]", 0.0), 100.0);
    assert_close!(f.num("effect('Map')(1).value.name == 'B' ? 1 : 0", 0.0), 1.0);
}

#[test]
fn time_to_frames_rounds_durations_away_from_zero() {
    let f = fx();
    // Adobe's public expression reference distinguishes durations from absolute times.
    // Expected integer counts are literal values, independent of the implementation.
    for (expression, expected) in [
        ("timeToFrames(-0.01, 24, true)", -1.0),
        ("timeToFrames(-0.05, 24, true)", -2.0),
        ("timeToFrames(0, 24, true)", 0.0),
        ("timeToFrames(-0.125, 24, true)", -3.0),
        ("timeToFrames(0.125, 24, true)", 3.0),
        ("timeToFrames(0.01, 24, true)", 1.0),
        ("timeToFrames(0.05, 24, true)", 2.0),
    ] {
        assert_eq!(f.num(expression, 0.0), expected, "{expression}");
    }
}

#[test]
fn time_to_frames_rounds_absolute_times_toward_negative_infinity() {
    let f = fx();
    for (expression, expected) in [
        ("timeToFrames(-0.01, 24, false)", -1.0),
        ("timeToFrames(-0.05, 24, false)", -2.0),
        ("timeToFrames(0, 24, false)", 0.0),
        ("timeToFrames(-0.125, 24, false)", -3.0),
        ("timeToFrames(0.125, 24, false)", 3.0),
        ("timeToFrames(0.01, 24, false)", 0.0),
        ("timeToFrames(0.05, 24, false)", 1.0),
    ] {
        assert_eq!(f.num(expression, 0.0), expected, "{expression}");
    }
}

#[test]
fn time_to_frames_preserves_default_arguments() {
    let f = fx();
    // The fixture's composition is 30 fps with displayStartTime = 0.
    for (expression, evaluation_time, expected) in [
        ("timeToFrames(0.01, 24)", 0.0, 0.0),
        ("timeToFrames(-0.01, 24)", 0.0, -1.0),
        ("timeToFrames(0.05)", 0.0, 1.0),
        ("timeToFrames(-0.05)", 0.0, -2.0),
        ("timeToFrames()", 0.0, 0.0),
        ("timeToFrames()", 0.05, 1.0),
        ("timeToFrames()", -0.05, -2.0),
        ("timeToFrames(undefined, 24, true)", 0.05, 2.0),
        ("timeToFrames(0.05, undefined, true)", 0.0, 2.0),
    ] {
        assert_eq!(f.num(expression, evaluation_time), expected, "{expression} at {evaluation_time}s");
    }
}

#[test]
fn time_to_frames_preserves_coercion_and_small_durations() {
    let f = fx();
    // Duration rounding must not erase tiny nonzero values; absolute times keep the existing tolerance.
    for (expression, expected) in [
        ("timeToFrames('0.05', '24', false)", 1.0),
        ("timeToFrames('0.05', '24', true)", 2.0),
        ("timeToFrames('-0.05', '24', true)", -2.0),
        ("timeToFrames(null, 24, true)", 0.0),
        ("timeToFrames(0.05, null, true)", 0.0),
        ("timeToFrames(0.05, 0, true)", 0.0),
        ("timeToFrames(31 / 24 - 5 / 24, 24, true)", 26.0),
        ("timeToFrames(5 / 24 - 31 / 24, 24, true)", -26.0),
        ("timeToFrames(-1e-10, 24, false)", 0.0),
        ("1 / timeToFrames(-0, 24, true) === Infinity ? 1 : 0", 1.0),
        ("isNaN(timeToFrames(NaN, 24, true)) ? 1 : 0", 1.0),
        ("timeToFrames(Infinity, 24, true) === Infinity ? 1 : 0", 1.0),
        ("timeToFrames(-Infinity, 24, true) === -Infinity ? 1 : 0", 1.0),
        ("var calls = 0; var t = {valueOf: function () { calls++; return 0.05; }}; timeToFrames(t, 24, true) + 100 * calls", 102.0),
    ] {
        assert_eq!(f.num(expression, 0.0), expected, "{expression}");
    }
}

/// A layer is a property group too, as in After Effects: `thisLayer("transform")`, another
/// layer's `("effects")("Slider Control")` or `(1)`, as the pick whip writes them with Compact
/// English off (they failed: "not a callable function"). It is still a layer.
#[test]
fn layers_are_callable_property_groups() {
    let f = fx();
    assert_close!(f.num("thisLayer(\"transform\")(\"opacity\")", 0.0), 100.0);
    assert_close!(f.num("thisComp.layer(\"A\")(\"effects\")(\"ec.control.slider\")(\"slider\")", 0.0), 42.0);
    assert_close!(f.num("thisComp.layer(\"A\")(\"effects\")(\"Slider Control\")(1)", 0.0), 42.0);
    assert_close!(f.num("thisComp.layer(1)(\"effects\")(1)(1)", 0.0), 42.0);
    // Its own name, index and methods, and a layer as the result still reads as its index.
    assert_close!(f.num("thisLayer.name == \"A\" ? thisComp.layer(\"B\").index : 0", 0.0), 2.0);
    assert_close!(f.num("thisComp.layer(\"B\")", 0.0), 2.0);
    assert_close!(f.num("thisComp.layer(thisComp.layer(\"B\"), -1).index", 0.0), 1.0);
    assert!(f.err("thisLayer(\"nope\")").contains("no property"), "{}", f.err("thisLayer(\"nope\")"));
}

/// A layer is kept from one evaluation to the next (it is dear to make) but reads the project
/// afresh in each, and an expression's own fields on it don't reach the next one.
#[test]
fn a_kept_layer_reads_the_project_again() {
    let mut f = fx();
    let first = "thisComp.layer(\"B\").index * 10 + thisComp.layer(2).name.length";
    assert_close!(f.num(first, 0.0), 21.0);
    f.layer_mut(f.b).name = "Bee".into();
    f.p.comp_mut(f.cid).unwrap().layers.swap(0, 1);
    assert_close!(f.num("thisComp.layer(\"Bee\").index * 10 + thisComp.layer(2).name.length", 0.0), 11.0);
    assert_close!(f.num("thisLayer.index", 0.0), 2.0);
    let _ = f.ev(f.a, ROT, "thisLayer.mine = 5; 0", 0.0);
    assert_close!(f.num("thisLayer.mine === undefined ? 1 : 0", 0.0), 1.0);
    let _ = f.ev(f.a, ROT, "var L = thisLayer; L.__c = 'x'; L.__l = 99; L.__i = ['Z', 7]; L.__s = __S; 0", 0.0);
    assert_close!(f.num("thisLayer.index * 10 + thisLayer.name.length", 0.0), 21.0);
    let _ = f.ev(f.a, ROT, "var L = thisLayer; delete L.__c; delete L.__l; delete L.__i; delete L.__s; 0", 0.0);
    assert_close!(f.num("thisLayer.index * 10 + thisLayer.name.length", 0.0), 21.0);
    let _ = f.ev(f.a, ROT, "Object.freeze(thisLayer); 0", 0.0);
    f.layer_mut(f.a).name = "Ay".into();
    f.p.comp_mut(f.cid).unwrap().layers.swap(0, 1);
    assert_close!(f.num("thisLayer.index * 10 + thisLayer.name.length", 0.0), 12.0);
}

/// The kept layers are bounded: past the bound a layer isn't kept, an evaluation that makes more
/// layers than are kept still sees one object for each layer it had kept, and the evaluations
/// after it, which start over, read the right layers.
#[test]
fn the_kept_layers_are_bounded() {
    let f = fx();
    let same = "(thisLayer === thisComp.layer(thisLayer.index) ? 1 : 0)";
    let many =
        format!("var c = thisLayer.__c;\nfor (var i = 0; i < 5000; i++) __layer(c, 100000 + i);\n{same} * 10 + (__layer(c, 1e6) === __layer(c, 1e6) ? 0 : 1)");
    assert_close!(f.num(&many, 0.0), 11.0);
    assert_close!(f.num(&format!("{same} * 100 + thisLayer.index * 10 + thisLayer.name.length"), 0.0), 111.0);
    assert_close!(f.num(&format!("{same} * 100 + thisLayer.index * 10 + thisLayer.name.length"), 0.0), 111.0);
}

/// A change to `Object.prototype` doesn't reach the kept layers: a frozen object put there under
/// the comp's id, before any of its layers is kept, doesn't stop them being kept.
#[test]
fn the_kept_layers_dont_go_through_object_prototype() {
    let f = fx();
    let _ = f.ev(f.a, ROT, "try { Object.prototype[thisComp.__c] = Object.freeze({}); } catch (e) {} 0", 0.0);
    assert_close!(f.num("thisLayer === thisComp.layer(thisLayer.index) ? 1 : 0", 0.0), 1.0);
}

/// The prelude is kept from one evaluation to the next, so an expression can't change its object
/// model or internals for the ones after it.
#[test]
fn an_expression_cant_change_the_prelude_for_the_next() {
    let f = fx();
    let read = "[thisComp.layer(1).index, thisLayer.index, thisLayer('transform').property('rotation').value,                 thisLayer.transform.opacity.value, thisProject.bitsPerChannel, comp(thisComp.name).numLayers, width]";
    let before = f.ev(f.a, ROT, read, 0.5).unwrap();
    let attacks = [
        "Comp.prototype.layer = function () { return 0; }",
        "Object.getPrototypeOf(thisLayer).index = 0",
        "Object.getPrototypeOf(thisLayer('transform')).property = function () { return 0; }",
        "Object.getPrototypeOf(thisLayer.transform.opacity).value = 0",
        "Comp = function () {}",
        "Prop = function () {}",
        "thisProject = null",
        "__h = function () { return null; }",
        "__layer = function () { return null; }",
        "__child = function () {}",
        "delete __mkProp",
        "delete thisLayer",
        "delete thisComp",
        "delete width",
        "Object.defineProperty(globalThis, 'thisComp', { get: function () { return 0; } })",
    ];
    for a in attacks {
        let _ = f.ev(f.a, ROT, &format!("try {{ {a} }} catch (e) {{}} 0"), 0.0);
        assert_eq!(f.ev(f.a, ROT, read, 0.5).unwrap(), before, "after `{a}`");
    }
}

mod property_group;
