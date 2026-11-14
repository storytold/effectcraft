//! Plug-in effects that set other parameters when the user changes one (#3663): the answer is
//! applied with the user's change as one undo step, is not reported again, is checked, and a
//! plug-in that fails leaves the user's change alone.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use effectcraft_effects::plugin::{
    EffectPlugin, ParamChange, PluginFrame, PluginManifest, PluginParam, PluginParamKind, PluginParams, instance_key_for, register_plugin,
};
use effectcraft_keyframe::Value as KV;
use effectcraft_project::{Layer, LayerId, PropGroup};
use serde_json::{Value, json};

use crate::Session;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// Preset 1 sets amount 75 and a tint; it also sets `link`, which is reported too.
    Preset,
    /// Fails (as a helper that timed out).
    Fail,
    /// Answers values that don't fit.
    Invalid,
    /// Takes 150 ms, then fails.
    Slow,
    /// Takes all the time it is given, then answers nothing.
    Spend,
}

struct Notifier {
    m: PluginManifest,
    notify: Vec<String>,
    mode: Mode,
    calls: AtomicUsize,
    last: Mutex<Option<ParamChange>>,
    /// The budget of every change it was told about.
    budgets: Mutex<Vec<Option<std::time::Duration>>>,
}

impl EffectPlugin for Notifier {
    fn manifest(&self) -> &PluginManifest {
        &self.m
    }
    fn render(&self, _: &mut PluginFrame, _: &PluginParams, _: f64) -> Result<(), String> {
        Ok(())
    }
    fn notify_params(&self) -> &[String] {
        &self.notify
    }
    fn param_changed(&self, change: &ParamChange) -> Result<Vec<(String, Value)>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.last.lock().unwrap() = Some(change.clone());
        self.budgets.lock().unwrap().push(change.budget);
        match self.mode {
            Mode::Slow => {
                std::thread::sleep(std::time::Duration::from_millis(150));
                Err("the helper did not answer in time".into())
            }
            Mode::Spend => {
                std::thread::sleep(change.budget.unwrap_or_default());
                Ok(vec![])
            }
            Mode::Fail => Err("the helper did not answer within 2 s".into()),
            Mode::Preset => Ok(if change.values["preset"] == json!(1) {
                vec![
                    ("amount".into(), json!(75.0)),
                    ("tint".into(), json!([1.0, 0.5, 0.0, 1.0])),
                    ("link".into(), json!(true)),
                    ("label".into(), json!("warm")),
                    ("secret".into(), json!("lut data")),
                ]
            } else {
                vec![]
            }),
            Mode::Invalid => Ok(vec![
                ("nope".into(), json!(1)),
                ("amount".into(), json!("lots")),
                ("preset".into(), json!(7)),
                ("tint".into(), json!([1.0, 2.0])),
                ("other".into(), json!(99)),
                ("label".into(), json!(3)),
                ("mirror".into(), json!(1e9)),
            ]),
        }
    }
}

fn param(id: &str, kind: PluginParamKind) -> PluginParam {
    PluginParam { id: id.into(), name: id.into(), group: String::new(), kind }
}

fn slider(default: f64) -> PluginParamKind {
    PluginParamKind::Slider { default, min: 0.0, max: 100.0, slider_min: None, slider_max: None, decimals: 1, integer: false }
}

/// Register a notifying plug-in `org.test.notify.<tag>` (ids are global: one per test).
fn notifier(tag: &str, mode: Mode) -> Arc<Notifier> {
    let m = PluginManifest {
        api: 2,
        id: format!("org.test.notify.{tag}"),
        name: format!("Notify {tag}"),
        category: "Test".into(),
        version: String::new(),
        author: String::new(),
        description: String::new(),
        params: vec![
            param("preset", PluginParamKind::Popup { options: vec!["Custom".into(), "Warm".into(), "Cold".into()], default: 0 }),
            param("amount", slider(10.0)),
            param("tint", PluginParamKind::Color { default: [1.0; 4] }),
            param("link", PluginParamKind::Checkbox { default: false }),
            param("mirror", slider(0.0)),
            param("other", PluginParamKind::Layer),
            param("label", PluginParamKind::Text { default: "x".into() }),
            param("speed", slider(0.0)),
            param("secret", PluginParamKind::Hidden { default: String::new() }),
        ],
    };
    let n = Arc::new(Notifier {
        m,
        notify: vec!["preset".into(), "link".into(), "speed".into()],
        mode,
        calls: AtomicUsize::new(0),
        last: Mutex::new(None),
        budgets: Mutex::new(vec![]),
    });
    register_plugin(n.clone()).unwrap();
    n
}

/// A comp with a solid carrying the plug-in effect: (session, layer, effect uid).
fn setup(effect: &str) -> (Session, u64, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 640, "height": 360, "frameRate": 30, "duration": 10})).unwrap();
    let l = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 200, "height": 100})).unwrap()["layer"].as_u64().unwrap();
    let fx = s.execute("effect.apply", json!({"layers": [l], "effect": effect})).unwrap()["effects"][0].as_u64().unwrap();
    (s, l, fx)
}

fn layer(s: &Session, id: u64) -> Layer {
    s.active_comp().unwrap().layer(LayerId(id)).unwrap().clone()
}

fn group(s: &Session, l: u64, fx: u64) -> PropGroup {
    layer(s, l).effects().unwrap().groups().find(|g| g.uid == fx).unwrap().clone()
}

fn value(s: &Session, l: u64, fx: u64, id: &str) -> KV {
    group(s, l, fx).get(id).unwrap().value.clone()
}

fn set(s: &mut Session, l: u64, fx: u64, id: &str, v: Value) -> Result<Value, crate::EngineError> {
    let prop = group(s, l, fx).get(id).unwrap().uid;
    s.execute("prop.set", json!({"layer": l, "prop": prop, "value": v}))
}

#[test]
fn a_change_applies_the_answer_in_the_same_undo_step() {
    let n = notifier("preset", Mode::Preset);
    let (mut s, l, fx) = setup("org.test.notify.preset");
    let undo_before = s.history.undo.len();
    set(&mut s, l, fx, "preset", json!(1)).unwrap();
    assert_eq!(value(&s, l, fx, "preset"), KV::Enum(1));
    assert_eq!(value(&s, l, fx, "amount"), KV::Scalar(75.0));
    assert_eq!(value(&s, l, fx, "tint"), KV::Color([1.0, 0.5, 0.0, 1.0]));
    // `link` is a notifying parameter too, but set by the answer: not reported again.
    assert_eq!(value(&s, l, fx, "link"), KV::Bool(true));
    // Text and hidden parameters take the strings of the answer.
    assert_eq!(value(&s, l, fx, "label"), KV::Str("warm".into()));
    assert_eq!(value(&s, l, fx, "secret"), KV::Str("lut data".into()));
    assert_eq!(n.calls.load(Ordering::SeqCst), 1, "the answer started another round");
    assert_eq!(s.history.undo.len(), undo_before + 1, "one undo step");
    // What the plug-in was told: the parameter, the instance its renders have, the values.
    let c = n.last.lock().unwrap().clone().unwrap();
    assert_eq!(c.param, "preset");
    assert_eq!(c.instance_key, instance_key_for(fx));
    assert_eq!(c.values["amount"], json!(10.0));
    assert_eq!(c.values["preset"], json!(1));
    assert_eq!(c.values["tint"], json!([1.0, 1.0, 1.0, 1.0]));
    assert_eq!(c.values["other"], Value::Null);
    assert!(!c.values.contains_key("label"));
    assert_eq!(c.texts, vec!["x".to_string(), String::new()]);
    assert!(c.layer_size[0] > 0.0 && c.layer_size[1] > 0.0, "{:?}", c.layer_size);
    // One undo takes back the user's change and the plug-in's.
    assert!(s.undo());
    assert_eq!(value(&s, l, fx, "preset"), KV::Enum(0));
    assert_eq!(value(&s, l, fx, "amount"), KV::Scalar(10.0));
    assert_eq!(value(&s, l, fx, "link"), KV::Bool(false));
    // Undo and redo report nothing.
    assert!(s.redo());
    assert_eq!(value(&s, l, fx, "amount"), KV::Scalar(75.0));
    assert_eq!(n.calls.load(Ordering::SeqCst), 1);
    // Other parameters don't report; an unchanged value doesn't either.
    set(&mut s, l, fx, "mirror", json!(5.0)).unwrap();
    set(&mut s, l, fx, "preset", json!(1)).unwrap();
    assert_eq!(n.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn an_animated_parameter_gets_a_key_where_the_user_changed() {
    let n = notifier("keys", Mode::Preset);
    let (mut s, l, fx) = setup("org.test.notify.keys");
    let amount = group(&s, l, fx).get("amount").unwrap().uid;
    s.execute("prop.addKey", json!({"layer": l, "prop": amount, "time": 0.0, "timeBase": "comp", "value": 20.0})).unwrap();
    s.execute("time.set", json!({"time": 2.0})).unwrap();
    set(&mut s, l, fx, "preset", json!(1)).unwrap();
    let pr = group(&s, l, fx).get("amount").unwrap().clone();
    assert_eq!(pr.keys.len(), 2, "{:?}", pr.keys);
    assert_eq!(pr.keys[1].value, KV::Scalar(75.0));
    assert!((pr.keys[1].time.seconds() - 2.0).abs() < 1e-9, "{:?}", pr.keys[1].time);
    assert_eq!(n.last.lock().unwrap().as_ref().map(|c| c.time), Some(2.0));
}

#[test]
fn a_failing_plugin_keeps_the_users_change() {
    let n = notifier("fail", Mode::Fail);
    let (mut s, l, fx) = setup("org.test.notify.fail");
    set(&mut s, l, fx, "preset", json!(2)).unwrap();
    assert_eq!(n.calls.load(Ordering::SeqCst), 1);
    assert_eq!(value(&s, l, fx, "preset"), KV::Enum(2));
    assert_eq!(value(&s, l, fx, "amount"), KV::Scalar(10.0));
    assert!(s.undo());
    assert_eq!(value(&s, l, fx, "preset"), KV::Enum(0));
}

#[test]
fn values_that_dont_fit_are_ignored() {
    let n = notifier("invalid", Mode::Invalid);
    let (mut s, l, fx) = setup("org.test.notify.invalid");
    set(&mut s, l, fx, "preset", json!(1)).unwrap();
    assert_eq!(n.calls.load(Ordering::SeqCst), 1);
    assert_eq!(value(&s, l, fx, "preset"), KV::Enum(1), "the user's value, not the out-of-range 7");
    assert_eq!(value(&s, l, fx, "amount"), KV::Scalar(10.0));
    assert_eq!(value(&s, l, fx, "tint"), KV::Color([1.0; 4]));
    assert_eq!(value(&s, l, fx, "other"), KV::Layer(None), "no such layer");
    assert_eq!(value(&s, l, fx, "label"), KV::Str("x".into()));
    assert_eq!(value(&s, l, fx, "mirror"), KV::Scalar(100.0), "clamped to the slider");
}

fn key_count(s: &Session, l: u64, fx: u64, id: &str) -> usize {
    group(s, l, fx).get(id).unwrap().keys.len()
}

/// Only a changed value is reported: turning the stopwatch on, or changing a keyframe's
/// interpolation or ease, changes no value.
#[test]
fn only_value_changes_are_reported() {
    let n = notifier("values", Mode::Preset);
    let (mut s, l, fx) = setup("org.test.notify.values");
    let speed = group(&s, l, fx).get("speed").unwrap().uid;
    // The stopwatch on (a key with the value it has): nothing changes.
    s.execute("prop.addKey", json!({"layer": l, "prop": speed, "time": 0.0, "timeBase": "comp", "value": 0.0})).unwrap();
    assert_eq!(key_count(&s, l, fx, "speed"), 1);
    assert_eq!(n.calls.load(Ordering::SeqCst), 0, "stopwatch on");
    // A key with another value at 1 s: reported, at that time.
    s.execute("prop.addKey", json!({"layer": l, "prop": speed, "time": 1.0, "timeBase": "comp", "value": 50.0})).unwrap();
    assert_eq!(n.calls.load(Ordering::SeqCst), 1, "a new value");
    assert_eq!(n.last.lock().unwrap().as_ref().map(|c| c.time), Some(1.0));
    // Interpolation and ease of the keys: no value changes.
    let cid = s.active_comp_id().unwrap();
    s.edit("Interpolation", None, |proj, _| {
        let layer = &mut proj.comp_mut(cid).unwrap().layers[0];
        let pr = layer.props.find_group_mut(fx).unwrap().get_mut("speed").unwrap();
        for k in &mut pr.keys {
            k.in_interp = effectcraft_keyframe::Interp::Bezier;
            k.out_interp = effectcraft_keyframe::Interp::Bezier;
            k.in_ease = vec![effectcraft_keyframe::Ease { speed: 0.0, influence: 60.0 }];
            k.out_ease = vec![effectcraft_keyframe::Ease { speed: 0.0, influence: 60.0 }];
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(n.calls.load(Ordering::SeqCst), 1, "interpolation and ease");
    // A changed value at the current time: reported.
    let speed_path = json!({"layer": l, "prop": speed, "value": 20.0});
    s.execute("prop.set", speed_path).unwrap();
    assert_eq!(n.calls.load(Ordering::SeqCst), 2, "a changed value");
}

/// A project can hold a slider whose range is upside down (min > max, from a file): setting it,
/// by hand or from a plug-in's answer, takes the value instead of failing on the clamp.
#[test]
fn a_slider_with_an_upside_down_range_takes_the_value() {
    notifier("range", Mode::Preset);
    let (mut s, l, fx) = setup("org.test.notify.range");
    let cid = s.active_comp_id().unwrap();
    s.edit("Range", None, |proj, _| {
        let layer = &mut proj.comp_mut(cid).unwrap().layers[0];
        let g = layer.props.find_group_mut(fx).unwrap();
        for id in ["amount", "mirror"] {
            g.get_mut(id).unwrap().ui = effectcraft_project::ParamUi::Slider { min: 100.0, max: 0.0, slider_min: 0.0, slider_max: 100.0, decimals: 1 };
        }
        Ok(())
    })
    .unwrap();
    // prop.set (the user's edit) and the plug-in's answer (amount 75).
    set(&mut s, l, fx, "mirror", json!(5.0)).unwrap();
    assert_eq!(value(&s, l, fx, "mirror"), KV::Scalar(5.0));
    set(&mut s, l, fx, "preset", json!(1)).unwrap();
    assert_eq!(value(&s, l, fx, "amount"), KV::Scalar(75.0));
}

/// One command waits at most the edit timeout for all its answers: after a plug-in failed, the
/// rest of a batch reports nothing; and what is left of the wait is passed on.
#[test]
fn a_batch_waits_for_plugins_once() {
    effectcraft_effects::plugin::set_param_change_timeout(std::time::Duration::from_millis(400));
    let slow = notifier("slow", Mode::Slow);
    let (mut s, l, fx) = setup("org.test.notify.slow");
    let preset = group(&s, l, fx).get("preset").unwrap().uid;
    let steps: Vec<Value> = [1, 2, 1, 2].iter().map(|v| json!({"command": "prop.set", "params": {"layer": l, "prop": preset, "value": v}})).collect();
    let t0 = std::time::Instant::now();
    s.execute("engine.batch", json!({"steps": steps})).unwrap();
    assert_eq!(slow.calls.load(Ordering::SeqCst), 1, "after a failure the batch reports nothing more");
    assert_eq!(value(&s, l, fx, "preset"), KV::Enum(2), "every step still applies");
    assert!(t0.elapsed() < std::time::Duration::from_millis(1500), "{:?}", t0.elapsed());
    // Each budget is what is left: a plug-in that uses it all leaves nothing for the next step.
    let spend = notifier("spend", Mode::Spend);
    let (mut s, l, fx) = setup("org.test.notify.spend");
    let preset = group(&s, l, fx).get("preset").unwrap().uid;
    let steps: Vec<Value> = [1, 2, 1].iter().map(|v| json!({"command": "prop.set", "params": {"layer": l, "prop": preset, "value": v}})).collect();
    let t0 = std::time::Instant::now();
    s.execute("engine.batch", json!({"steps": steps})).unwrap();
    let budgets = spend.budgets.lock().unwrap().clone();
    assert_eq!(budgets.len(), 1, "{budgets:?}");
    assert!(budgets[0].is_some_and(|b| b <= std::time::Duration::from_millis(400)), "{budgets:?}");
    assert!(t0.elapsed() < std::time::Duration::from_millis(1500), "{:?}", t0.elapsed());
    // A new command has its own wait again.
    set(&mut s, l, fx, "preset", json!(2)).unwrap();
    assert_eq!(spend.budgets.lock().unwrap().len(), 2);
}
