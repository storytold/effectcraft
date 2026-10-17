//! The shared result memo: identical results to fresh runs (the memo off is the oracle), across
//! edits, cycles and interleaved times; and the frame timing of a controller rig (run
//! `rig_timing` with `--ignored --nocapture`).

use std::time::Instant;

use effectcraft_color::Label;
use effectcraft_keyframe::Value;
use effectcraft_project::build;
use effectcraft_project::{Comp, Expression, ItemId, ItemKind, Layer, LayerSource, Project, Solid};
use effectcraft_render::{EvalCtx, ExprHost};
use effectcraft_time::{FrameRate, Tick};

use crate::{Expressions, memo, script_runs};

const ROT: &str = "transform/rotation";
const OPA: &str = "transform/opacity";

/// "Ctrl" (slider `wiggle(2, 20)`), `n` layers whose rotation reads the slider, and "Sum" whose
/// opacity loops over every layer's rotation.
pub(crate) fn rig(n: usize) -> (Project, ItemId) {
    let mut p = Project::default();
    let comp = Comp::new(200, 100, FrameRate::FPS_30, Tick::from_seconds_f64(10.0));
    let cid = p.add_item("Main", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let solid = |p: &mut Project, name: &str| {
        let sid = p.add_item(name, Label::Red, None, ItemKind::Solid(Solid { color: [1.0; 3], width: 10, height: 10, pixel_aspect: 1.0 }));
        build::layer(p, &comp, name, LayerSource::Solid { item: sid }, (10, 10), None)
    };
    let mut ctrl = solid(&mut p, "Ctrl");
    let spec = effectcraft_effects::find("ec.control.slider").unwrap();
    let mut next = p.next_id;
    let mut slider = effectcraft_effects::instantiate(spec, &mut build::Ids(&mut next), "Slider Control", [5.0, 5.0]);
    p.next_id = next;
    let s = slider.prop_mut("slider").unwrap();
    s.value = Value::Scalar(10.0);
    s.expr = Some(Expression { text: "wiggle(2, 20)".into(), enabled: true });
    ctrl.props.sub_mut("effects").unwrap().children.push(slider.into());
    let mut sum = solid(&mut p, "Sum");
    sum.props.prop_mut(OPA).unwrap().expr = Some(Expression {
        text: "var s = 0; for (var i = 3; i <= thisComp.numLayers; i++) s += thisComp.layer(i).transform.rotation; clamp(s / 100, 0, 100)".into(),
        enabled: true,
    });
    let mut layers = vec![ctrl, sum];
    for i in 0..n {
        let mut l = solid(&mut p, &format!("L{i}"));
        l.props.prop_mut(ROT).unwrap().expr =
            Some(Expression { text: "thisComp.layer(\"Ctrl\").effect(\"Slider Control\")(\"Slider\") * index + time".into(), enabled: true });
        layers.push(l);
    }
    p.comp_mut(cid).unwrap().layers = layers;
    (p, cid)
}

/// Every expression of the rig at comp time `t` (rotations, then the sum), as one frame does.
pub(crate) fn frame(p: &Project, cid: ItemId, t: f64) -> Vec<f64> {
    let host = Expressions;
    let c = p.comp(cid).unwrap();
    let mut ctx = EvalCtx::new(p, cid, c, Tick::from_seconds_f64(t));
    ctx.expr = Some(&host as &dyn ExprHost);
    let mut out = vec![];
    for l in c.layers.iter().skip(2) {
        out.push(ctx.value(l, l.props.prop(ROT).unwrap()).as_f64());
    }
    let sum = c.layers.get(1).unwrap();
    out.push(ctx.value(sum, sum.props.prop(OPA).unwrap()).as_f64());
    out
}

#[test]
#[ignore = "timing readout"]
fn rig_timing() {
    let (p, cid) = rig(100);
    for off in [true, false] {
        memo::set_off(off);
        let _ = frame(&p, cid, 0.0);
        let frames = 150;
        let runs = script_runs();
        let start = Instant::now();
        for f in 1..=frames {
            let _ = frame(&p, cid, f as f64 / 30.0);
        }
        let ms = start.elapsed().as_secs_f64() * 1000.0 / frames as f64;
        let runs = (script_runs() - runs) as f64 / frames as f64;
        println!("rig of 100 layers, memo {}: {ms:.2} ms/frame, {runs:.0} script runs/frame", if off { "off" } else { "on" });
    }
}

/// `f` on a thread with the memo off (the oracle: every evaluation runs its script), leaving
/// this thread's memo as it was.
fn fresh<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|s| {
        s.spawn(|| {
            memo::set_off(true);
            f()
        })
        .join()
        .unwrap()
    })
}

fn bits(v: &[f64]) -> Vec<u64> {
    v.iter().map(|x| x.to_bits()).collect()
}

/// [`frame`] with the memo (kept across calls) equals [`frame`] run fresh, bit for bit.
fn assert_same(p: &Project, cid: ItemId, t: f64, what: &str) {
    let got = frame(p, cid, t);
    let want = fresh(|| frame(p, cid, t));
    assert_eq!(bits(&got), bits(&want), "{what} at {t}: {got:?} vs {want:?}");
}

fn layer<'a>(p: &'a mut Project, cid: ItemId, name: &str) -> &'a mut Layer {
    let c = p.comp_mut(cid).unwrap();
    let id = c.layer_by_name(name).unwrap().id;
    c.layer_mut(id).unwrap()
}

#[test]
fn shared_results_match_fresh_runs() {
    let (p, cid) = rig(12);
    // Repeated and interleaved times, as motion-blur samples and scrubbing ask for them.
    for t in [0.0, 0.5, 0.5 + 1.0 / 120.0, 0.5, 1.0, 0.5 + 1.0 / 120.0, 0.0, 7.25] {
        assert_same(&p, cid, t, "rig");
    }
    // A slider read by every layer runs once per frame, not once per reader.
    let _ = frame(&p, cid, 2.0);
    let before = script_runs();
    let _ = frame(&p, cid, 3.0);
    assert_eq!(script_runs() - before, 12 + 2, "one run per layer, one for the slider, one for the sum");
}

#[test]
fn edits_are_never_answered_from_the_memo() {
    let (mut p, cid) = rig(6);
    let t = 0.4;
    assert_same(&p, cid, t, "before");
    fn slider(p: &mut Project, cid: ItemId) -> &mut effectcraft_project::Property {
        let l = layer(p, cid, "Ctrl");
        l.props.sub_mut("effects").unwrap().children.first_mut().unwrap().as_group_mut().unwrap().prop_mut("slider").unwrap()
    }
    let edits: Vec<(&str, Box<dyn Fn(&mut Project)>)> = vec![
        ("slider value", Box::new(|p| slider(p, cid).value = Value::Scalar(11.0))),
        ("slider expression", Box::new(|p| slider(p, cid).expr = Some(Expression { text: "wiggle(3, 20)".into(), enabled: true }))),
        ("slider expression off", Box::new(|p| slider(p, cid).expr.as_mut().unwrap().enabled = false)),
        ("slider expression on", Box::new(|p| slider(p, cid).expr.as_mut().unwrap().enabled = true)),
        ("rename the controller", Box::new(|p| layer(p, cid, "Ctrl").name = "Boss".into())),
        ("rename it back", Box::new(|p| layer(p, cid, "Boss").name = "Ctrl".into())),
        ("reorder layers", Box::new(|p| p.comp_mut(cid).unwrap().layers.swap(2, 4))),
        ("move a layer in time", Box::new(|p| layer(p, cid, "L1").start_time = Tick::from_seconds_f64(0.25))),
        ("a reader's expression", Box::new(|p| layer(p, cid, "L2").props.prop_mut(ROT).unwrap().expr.as_mut().unwrap().text = "time * 7".into())),
        ("a reader's keyframed value", Box::new(|p| layer(p, cid, "L3").props.prop_mut(ROT).unwrap().value = Value::Scalar(-0.0))),
        ("delete a layer", Box::new(|p| drop(p.comp_mut(cid).unwrap().layers.remove(3)))),
    ];
    for (what, edit) in edits {
        edit(&mut p);
        assert_same(&p, cid, t, what);
        assert_same(&p, cid, t, what);
    }
}

#[test]
fn negative_zero_is_a_different_answer() {
    let (mut p, cid) = rig(2);
    let read = "var x = thisComp.layer(\"Ctrl\").transform.rotation.value; x === 0 && 1 / x < 0 ? 1 : 2";
    layer(&mut p, cid, "L0").props.prop_mut(ROT).unwrap().expr = Some(Expression { text: read.into(), enabled: true });
    assert_eq!(frame(&p, cid, 0.0).first(), Some(&2.0));
    layer(&mut p, cid, "Ctrl").props.prop_mut(ROT).unwrap().value = Value::Scalar(-0.0);
    assert_eq!(frame(&p, cid, 0.0).first(), Some(&1.0));
    assert_same(&p, cid, 0.0, "-0");
}

#[test]
fn cycles_match_fresh_runs() {
    let (mut p, cid) = rig(3);
    let set =
        |p: &mut Project, name: &str, text: &str| layer(p, cid, name).props.prop_mut(ROT).unwrap().expr = Some(Expression { text: text.into(), enabled: true });
    set(&mut p, "L0", "thisComp.layer(\"L1\").transform.rotation + 1");
    set(&mut p, "L1", "thisComp.layer(\"L0\").transform.rotation + 2");
    set(&mut p, "L2", "thisComp.layer(\"L2\").transform.rotation + 3");
    for t in [0.0, 0.0, 1.0, 0.0] {
        assert_same(&p, cid, t, "cycle");
    }
    // Evaluating the other way round first nests differently; the answers still agree.
    p.comp_mut(cid).unwrap().layers.swap(2, 3);
    for t in [0.0, 1.0] {
        assert_same(&p, cid, t, "cycle, reversed");
    }
}

#[test]
fn impure_scripts_are_not_remembered() {
    assert!(!memo::cacheable("Math.random() * 10"));
    assert!(!memo::cacheable("new Date().getSeconds()"));
    assert!(memo::cacheable("random(10)"));
}

/// `text` as the rotation expression of a layer of [`rig`], evaluated at time 0.
fn eval_simple(text: &str) -> f64 {
    let (mut p, cid) = rig(1);
    layer(&mut p, cid, "L0").props.prop_mut(ROT).unwrap().expr = Some(Expression { text: text.into(), enabled: true });
    let host = Expressions;
    let c = p.comp(cid).unwrap();
    let mut ctx = EvalCtx::new(&p, cid, c, Tick::ZERO);
    ctx.expr = Some(&host as &dyn ExprHost);
    let l = c.layer_by_name("L0").unwrap();
    ctx.value(l, l.props.prop(ROT).unwrap()).as_f64()
}

/// Review on #856: scripts that reach `Math.random` or `Date` without spelling them are not
/// remembered either: each evaluation gets a fresh value.
#[test]
fn impure_calls_are_caught_however_they_are_spelled() {
    for (i, text) in [
        r#"Math["random"]() * 1e9"#,
        r#"const M = Math; M["ran" + "dom"]() * 1e9"#,
        r#"globalThis["Da" + "te"]["now"]() + Math["random"]()"#,
        r#"const D = globalThis["Da" + "te"]; new D().getTime() + Math["random"]()"#,
        r#"(new (Object.getPrototypeOf(new globalThis["Da" + "te"]()).constructor)()).getTime() + Math["random"]()"#,
    ]
    .into_iter()
    .enumerate()
    {
        assert!(memo::cacheable(text), "{i}: the text check alone misses it");
        let a = eval_simple(text);
        let b = eval_simple(text);
        assert_ne!(a, b, "{i}: {text} was remembered");
    }
    // Pure scripts are still remembered, and `__impure` isn't reachable.
    assert_eq!(eval_simple("1 + 2"), eval_simple("1 + 2"));
    assert_eq!(eval_simple("typeof __impure === 'undefined' ? 1 : 0"), 1.0);
}

/// Clock-only paths must run again, even when two reads return the same millisecond.
#[test]
fn clock_descriptors_and_aliases_are_never_memoized() {
    memo::set_off(false);
    for text in [
        r#"Object.getOwnPropertyDescriptor(globalThis["Da" + "te"], "now").value()"#,
        r#"Object.getOwnPropertyDescriptors(globalThis["Da" + "te"]).now.value()"#,
        r#"const D = globalThis["Da" + "te"]; D["now"]()"#,
        r#"const D = globalThis["Da" + "te"]; new D().getTime()"#,
    ] {
        assert!(memo::cacheable(text));
        assert!(eval_simple(text) > 0.0);
        let before = script_runs();
        assert!(eval_simple(text) > 0.0);
        assert!(script_runs() > before, "clock expression was reused: {text}");
    }
}
