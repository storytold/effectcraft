//! Learn: interactive tutorials on the Home screen (our own content, licensed with the repo).
//!
//! A tutorial is a list of steps. Each step names the UI element to highlight (an automation id,
//! see `docs/control-protocol.md`), the commands that complete it when the user performs them,
//! and the commands "Show me" runs to perform it for the user. Progress lives on the
//! [`Session`] (not the project), so opening the demo project in a step keeps the tutorial going.
//!
//! Commands: `learn.list`, `learn.start {id}`, `learn.step {action: next|back|showMe|goto,
//! index?}`, `learn.stop`, `learn.state`.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, str_p};
use crate::{Result, Session, cmd, query};

/// One step of a tutorial.
#[derive(Clone, Debug, Serialize)]
pub struct Step {
    pub title: String,
    pub text: String,
    /// Automation id of the element to highlight. `*` matches any run of characters and
    /// `{layer}` stands for the first selected layer's id (see [`resolve_target`]).
    pub target: Option<String>,
    /// Commands that complete the step when they run (empty: a reading step, advanced with Next).
    pub accepts: Vec<String>,
    /// Parameters the completing command must carry (a subset match; `null` = any).
    pub when: Value,
    /// What "Show me" runs: (command, params), in order.
    pub show: Vec<(String, Value)>,
}

/// An interactive tutorial.
#[derive(Clone, Debug, Serialize)]
pub struct Tutorial {
    pub id: String,
    pub title: String,
    pub summary: String,
    /// Rough length in minutes.
    pub minutes: u32,
    pub steps: Vec<Step>,
}

/// Where the user is in a tutorial.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    pub tutorial: String,
    /// Index of the current step (`== steps.len()` once finished).
    pub step: usize,
    pub done: bool,
}

struct S<'a> {
    title: &'a str,
    text: &'a str,
    target: Option<&'a str>,
    accepts: &'a [&'a str],
    when: Value,
    show: Vec<(&'a str, Value)>,
}

fn step(s: S) -> Step {
    Step {
        title: s.title.into(),
        text: s.text.into(),
        target: s.target.map(str::to_string),
        accepts: s.accepts.iter().map(|a| a.to_string()).collect(),
        when: s.when,
        show: s.show.into_iter().map(|(c, p)| (c.to_string(), p)).collect(),
    }
}

/// The built-in tutorials.
pub fn tutorials() -> Vec<Tutorial> {
    vec![
        Tutorial {
            id: "animate-title".into(),
            title: "Animate a title".into(),
            summary: "Make a composition, type a title and fly it in with two position keyframes and an easy ease.".into(),
            minutes: 4,
            steps: vec![
                step(S {
                    title: "Make a composition",
                    text: "Everything you animate lives in a composition. Choose Composition ▸ New Composition (Cmd+N) and keep the HD 1920×1080 settings.",
                    target: Some("menu.Composition"),
                    accepts: &["comp.new"],
                    when: Value::Null,
                    show: vec![("comp.new", json!({"name": "Title Animation", "width": 1920, "height": 1080, "duration": 5}))],
                }),
                step(S {
                    title: "Type a title",
                    text: "Choose Layer ▸ New ▸ Text (or the Type tool) and type a short title. A new text layer appears at the top of the Timeline.",
                    target: Some("menu.Layer"),
                    accepts: &["layer.newText"],
                    when: Value::Null,
                    show: vec![("layer.newText", json!({"text": "Hello, motion", "size": 120}))],
                }),
                step(S {
                    title: "Start the animation",
                    text: "Twirl the text layer open, then Transform. Click the stopwatch beside Position: it turns blue and records a keyframe at the current time.",
                    target: Some("timeline.layer.{layer}.twirl"),
                    accepts: &["prop.toggleAnimation", "prop.addKey", "prop.toggleKey", "anim.addKeyframe"],
                    when: Value::Null,
                    show: vec![
                        ("prop.set", json!({"path": "transform/position", "value": [-400.0, 540.0, 0.0]})),
                        ("prop.toggleAnimation", json!({"path": "transform/position", "value": true})),
                    ],
                }),
                step(S {
                    title: "Move in time",
                    text: "Drag the blue current-time indicator to about 0:00:01:00, or click in the time ruler above the layer bars.",
                    target: Some("timeline.ruler"),
                    accepts: &["time.set", "time.step", "time.go"],
                    when: Value::Null,
                    show: vec![("time.set", json!({"time": 1.0}))],
                }),
                step(S {
                    title: "Set the end position",
                    text: "Drag the title to the middle of the frame in the Composition viewer, or scrub the Position value. Because the stopwatch is on, a second keyframe appears by itself.",
                    target: Some("viewer.comp"),
                    accepts: &["prop.set"],
                    when: json!({"path": "transform/position"}),
                    show: vec![("prop.set", json!({"path": "transform/position", "value": [960.0, 540.0, 0.0]}))],
                }),
                step(S {
                    title: "Smooth it out",
                    text: "Select both keyframes (drag a box around them, or click the Position name) and press F9 for Easy Ease. The title now glides in and settles.",
                    target: Some("timeline.ruler"),
                    accepts: &["keys.easyEase"],
                    when: Value::Null,
                    show: vec![("keys.selectAll", json!({})), ("keys.easyEase", json!({}))],
                }),
                step(S {
                    title: "Watch it",
                    text: "Press the Spacebar (or Play in the Preview panel) to play your animation. Press it again to stop. That's it: you made your first animation.",
                    target: Some("panel.Preview"),
                    accepts: &["playback.toggle"],
                    when: Value::Null,
                    show: vec![("time.set", json!({"time": 0.0})), ("playback.toggle", json!({}))],
                }),
            ],
        },
        Tutorial {
            id: "track-attach".into(),
            title: "Track and attach".into(),
            summary: "Follow a moving element with the point tracker, then hang a null on the track and parent a layer to it.".into(),
            minutes: 5,
            steps: vec![
                step(S {
                    title: "Open the demo",
                    text: "We'll track the moving lower third in the demo project. Open it from Home ▸ Open Demo Project (or File ▸ Open Demo Project).",
                    target: Some("menu.File"),
                    accepts: &["file.openDemoProject"],
                    when: Value::Null,
                    show: vec![("file.openDemoProject", json!({}))],
                }),
                step(S {
                    title: "Pick what to track",
                    text: "Click the Lower Third layer in the Timeline, then press I to jump to where it starts (0:00:05:00). The tracker follows the pixels of the selected layer's source.",
                    target: Some("timeline.layer.*.row"),
                    accepts: &["layer.select"],
                    when: Value::Null,
                    show: vec![("layer.select", json!({"layers": ["Lower Third"]})), ("time.set", json!({"time": 5.5}))],
                }),
                step(S {
                    title: "Add a track point",
                    text: "Open Window ▸ Tracker and click Track Motion. A track point appears: the inner box is the feature to follow, the outer box is where to look for it on the next frame.",
                    target: Some("tracker.trackMotion"),
                    accepts: &["track.motion", "track.new"],
                    when: Value::Null,
                    show: vec![("track.motion", json!({}))],
                }),
                step(S {
                    title: "Analyze",
                    text: "Click Analyze forward ▶ in the Tracker panel. The point follows the feature frame by frame and leaves a path of keyframes behind.",
                    target: Some("tracker.analyze.forward"),
                    accepts: &["track.analyze"],
                    when: Value::Null,
                    show: vec![("track.analyze", json!({"direction": "forward", "end": 6.0, "wait": true}))],
                }),
                step(S {
                    title: "Make a null to carry the motion",
                    text: "Choose Layer ▸ New ▸ Null Object. A null is invisible; it only holds a transform, which makes it a handy anchor for tracked motion.",
                    target: Some("menu.Layer"),
                    accepts: &["layer.newNull"],
                    when: Value::Null,
                    show: vec![("layer.newNull", json!({"name": "Track Null"}))],
                }),
                step(S {
                    title: "Point the track at the null",
                    text: "Select Lower Third again, click Edit Target… in the Tracker panel and choose the null as the motion target.",
                    target: Some("tracker.editTarget"),
                    accepts: &["track.setTarget"],
                    when: Value::Null,
                    show: vec![("track.setTarget", json!({"layer": "Lower Third", "target": "Track Null"}))],
                }),
                step(S {
                    title: "Apply",
                    text: "Click Apply and keep X and Y. The null's Position now follows the track.",
                    target: Some("tracker.apply"),
                    accepts: &["track.apply"],
                    when: Value::Null,
                    show: vec![("track.apply", json!({"layer": "Lower Third", "dimensions": "xy"}))],
                }),
                step(S {
                    title: "Attach a layer",
                    text: "Drag a layer's pick whip (the spiral in Parent & Link) onto the null, or pick it from the menu. The child now rides along with the track.",
                    target: Some("timeline.layer.*.pickWhip"),
                    accepts: &["layer.setParent"],
                    when: Value::Null,
                    show: vec![("layer.setParent", json!({"layers": ["Tagline"], "parent": "Track Null"}))],
                }),
            ],
        },
        Tutorial {
            id: "3d-scene".into(),
            title: "Make a 3D scene".into(),
            summary: "Turn flat layers into 3D, light them, add a camera and look at the scene from outside the frame.".into(),
            minutes: 5,
            steps: vec![
                step(S {
                    title: "Make a composition",
                    text: "Choose Composition ▸ New Composition. Any size works; 1920×1080 is a good start.",
                    target: Some("menu.Composition"),
                    accepts: &["comp.new"],
                    when: Value::Null,
                    show: vec![("comp.new", json!({"name": "3D Scene", "width": 1920, "height": 1080, "duration": 5}))],
                }),
                step(S {
                    title: "Add a floor",
                    text: "Choose Layer ▸ New ▸ Solid and make a large solid in a calm colour. It will become the floor.",
                    target: Some("menu.Layer"),
                    accepts: &["layer.newSolid"],
                    when: Value::Null,
                    show: vec![("layer.newSolid", json!({"name": "Floor", "color": "#3c5a82", "width": 2400, "height": 2400}))],
                }),
                step(S {
                    title: "Make it 3D",
                    text: "Click the cube switch of the solid in the Timeline. The layer gains a Z position and X/Y/Z rotation.",
                    target: Some("timeline.layer.{layer}.switch.threeD"),
                    accepts: &["layer.setSwitch"],
                    when: json!({"switch": "threeD"}),
                    show: vec![("layer.setSwitch", json!({"switch": "threeD", "value": true}))],
                }),
                step(S {
                    title: "Lay it down",
                    text: "Set X Rotation to -80° (twirl open Transform and scrub the value) so the solid lies like a floor, and push its Position down a little.",
                    target: Some("timeline.layer.{layer}.twirl"),
                    accepts: &["prop.set"],
                    when: Value::Null,
                    show: vec![
                        ("prop.set", json!({"path": "transform/rotationX", "value": -80.0})),
                        ("prop.set", json!({"path": "transform/position", "value": [960.0, 760.0, 0.0]})),
                    ],
                }),
                step(S {
                    title: "Light it",
                    text: "Choose Layer ▸ New ▸ Light and pick Spot. 3D layers now catch light and fall-off.",
                    target: Some("menu.Layer"),
                    accepts: &["layer.newLight"],
                    when: Value::Null,
                    show: vec![("layer.newLight", json!({"kind": "Spot", "name": "Key Light"}))],
                }),
                step(S {
                    title: "Add a camera",
                    text: "Choose Layer ▸ New ▸ Camera. The comp is now seen through this camera; orbit it with the camera tools (press C).",
                    target: Some("menu.Layer"),
                    accepts: &["layer.newCamera"],
                    when: Value::Null,
                    show: vec![("layer.newCamera", json!({"name": "Camera"}))],
                }),
                step(S {
                    title: "Step outside the camera",
                    text: "In the 3D View popup under the viewer choose Custom View 1. You see the scene, the camera and the light from a working view.",
                    target: Some("viewer.view3d"),
                    accepts: &[
                        "view.set3DView",
                        "view.3d.custom1",
                        "view.3d.custom2",
                        "view.3d.custom3",
                        "view.3d.front",
                        "view.3d.top",
                        "view.3d.left",
                        "view.3d.right",
                        "view.3d.back",
                        "view.3d.bottom",
                    ],
                    when: Value::Null,
                    show: vec![("view.3d.custom1", json!({}))],
                }),
                step(S {
                    title: "See past the frame",
                    text: "Click Extended Viewer beside the 3D View popup. The viewer now shows the layers beyond the comp's edges, with the frame outlined.",
                    target: Some("viewer.extendedViewer"),
                    accepts: &["view.extendedViewer"],
                    when: Value::Null,
                    show: vec![("view.extendedViewer", json!({"value": true}))],
                }),
            ],
        },
    ]
}

/// The tutorial with this id.
pub fn find(id: &str) -> Option<Tutorial> {
    tutorials().into_iter().find(|t| t.id == id)
}

/// The automation id pattern of a step's target with `{layer}` filled in (`*` stays a wildcard).
pub fn resolve_target(target: &str, layer: Option<u64>) -> String {
    match layer {
        Some(l) => target.replace("{layer}", &l.to_string()),
        None => target.replace("{layer}", "*"),
    }
}

/// Whether an automation id matches a target pattern (`*` = any run of characters).
pub fn target_matches(pattern: &str, id: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == id;
    }
    let mut rest = id;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            let Some(r) = rest.strip_prefix(part) else { return false };
            rest = r;
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            let Some(at) = rest.find(part) else { return false };
            rest = &rest[at + part.len()..];
        }
    }
    true
}

/// `want`'s keys all equal in `got` (`null` matches anything).
fn params_match(want: &Value, got: &Value) -> bool {
    match want {
        Value::Object(m) => m.iter().all(|(k, v)| got.get(k) == Some(v)),
        _ => true,
    }
}

impl Session {
    /// The current tutorial and step, if one is running.
    pub fn learn_current(&self) -> Option<(Tutorial, usize)> {
        let p = self.learn.as_ref()?;
        Some((find(&p.tutorial)?, p.step))
    }

    /// A command ran: complete the current step when it is one the step accepts.
    pub(crate) fn learn_observe(&mut self, id: &str, params: &Value) {
        if id.starts_with("learn.") {
            return;
        }
        let Some((t, i)) = self.learn_current() else { return };
        let Some(st) = t.steps.get(i) else { return };
        if st.accepts.iter().any(|a| a == id) && params_match(&st.when, params) {
            self.learn_goto(&t, i + 1);
        }
    }

    fn learn_goto(&mut self, t: &Tutorial, i: usize) {
        let n = t.steps.len();
        let i = i.min(n);
        if let Some(p) = &mut self.learn {
            p.step = i;
            p.done = i >= n;
        }
        if i >= n {
            self.toast(format!("Tutorial complete: {}", t.title));
        }
    }
}

fn state_json(s: &Session) -> Value {
    match s.learn_current() {
        Some((t, i)) => json!({
            "tutorial": t.id,
            "title": t.title,
            "step": i,
            "steps": t.steps.len(),
            "done": i >= t.steps.len(),
            "current": t.steps.get(i).map(|st| json!({
                "title": st.title,
                "text": st.text,
                "target": st.target.as_deref().map(|x| resolve_target(x, s.state.selected_layers.first().map(|l| l.0))),
                "accepts": st.accepts,
            })),
        }),
        None => Value::Null,
    }
}

fn list(_: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!(
        tutorials()
            .iter()
            .map(|t| json!({"id": t.id, "title": t.title, "summary": t.summary, "minutes": t.minutes, "steps": t.steps.iter().map(|s| s.title.clone()).collect::<Vec<_>>()}))
            .collect::<Vec<_>>()
    ))
}

fn start(s: &mut Session, p: &Value) -> Result<Value> {
    let id = str_p(p, "id").or_else(|| str_p(p, "tutorial")).ok_or_else(|| bad("learn.start", "missing `id`"))?;
    let t = find(id).ok_or_else(|| bad("learn.start", format!("unknown tutorial `{id}` (see learn.list)")))?;
    s.learn = Some(Progress { tutorial: t.id.clone(), step: 0, done: false });
    Ok(state_json(s))
}

fn step_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let (t, i) = s.learn_current().ok_or_else(|| bad("learn.step", "no tutorial is running (learn.start)"))?;
    match str_p(p, "action").unwrap_or("next") {
        "next" => s.learn_goto(&t, i + 1),
        "back" | "prev" | "previous" => s.learn_goto(&t, i.saturating_sub(1)),
        "goto" => {
            let n = p.get("index").and_then(Value::as_u64).ok_or_else(|| bad("learn.step", "goto needs `index`"))?;
            s.learn_goto(&t, n as usize);
        }
        "showMe" | "show" => {
            let st = t.steps.get(i).ok_or_else(|| bad("learn.step", "the tutorial is finished"))?;
            for (c, params) in &st.show {
                s.execute(c, params.clone()).map_err(|e| bad("learn.step", format!("Show me: {c}: {e}")))?;
            }
            // Steps whose commands don't complete them (or reading steps) still move on.
            if s.learn.as_ref().is_some_and(|p| p.step == i) {
                s.learn_goto(&t, i + 1);
            }
        }
        a => return Err(bad("learn.step", format!("action must be next|back|showMe|goto, not `{a}`"))),
    }
    Ok(state_json(s))
}

fn stop(s: &mut Session, _: &Value) -> Result<Value> {
    s.learn = None;
    Ok(Value::Null)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        query!("learn.list", "Learn Tutorials", "{} → [{id, title, summary, minutes, steps}]", list),
        query!("learn.state", "Tutorial State", "{} → {tutorial, step, steps, done, current: {title, text, target, accepts}} | null", |s, _| Ok(state_json(
            s
        ))),
        cmd!("learn.start", "Start Tutorial", [], None, "{id}", always, start),
        cmd!("learn.step", "Tutorial Step", [], None, "{action?: next|back|showMe|goto, index?}", always, step_cmd),
        cmd!("learn.stop", "Close Tutorial", [], None, "{}", always, stop),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_match_with_wildcards() {
        assert!(target_matches("timeline.layer.*.row", "timeline.layer.12.row"));
        assert!(!target_matches("timeline.layer.*.row", "timeline.layer.12.bar"));
        assert!(target_matches("menu.File", "menu.File"));
        assert!(!target_matches("menu.File", "menu.File.2"));
        assert_eq!(resolve_target("timeline.layer.{layer}.twirl", Some(7)), "timeline.layer.7.twirl");
        assert_eq!(resolve_target("timeline.layer.{layer}.twirl", None), "timeline.layer.*.twirl");
    }

    #[test]
    fn every_step_names_real_commands() {
        for t in tutorials() {
            assert!(!t.steps.is_empty(), "{}", t.id);
            for st in &t.steps {
                for c in st.accepts.iter().chain(st.show.iter().map(|(c, _)| c)) {
                    assert!(crate::commands::find(c).is_some(), "{}: unknown command {c}", t.id);
                }
                assert!(!st.show.is_empty(), "{}: `{}` has no Show me", t.id, st.title);
            }
        }
    }

    #[test]
    fn tutorials_run_end_to_end_with_show_me() {
        for id in ["animate-title", "3d-scene", "track-attach"] {
            let mut s = Session::default();
            s.execute("learn.start", json!({"id": id})).unwrap();
            let n = find(id).unwrap().steps.len();
            for i in 0..n {
                let st = s.execute("learn.step", json!({"action": "showMe"})).unwrap_or_else(|e| panic!("{id} step {i}: {e}"));
                assert_eq!(st["step"], json!(i + 1), "{id}: {st}");
            }
            assert_eq!(s.execute("learn.state", json!({})).unwrap()["done"], json!(true));
        }
    }

    #[test]
    fn performing_the_command_advances() {
        let mut s = Session::default();
        let list = s.execute("learn.list", json!({})).unwrap();
        assert_eq!(list.as_array().unwrap().len(), 3);
        s.execute("learn.start", json!({"id": "animate-title"})).unwrap();
        // Unrelated commands don't advance.
        s.execute("file.openDemoProject", json!({})).unwrap();
        assert_eq!(s.learn.as_ref().unwrap().step, 0);
        s.execute("comp.new", json!({"name": "Mine"})).unwrap();
        assert_eq!(s.learn.as_ref().unwrap().step, 1);
        s.execute("layer.newText", json!({"text": "Hi"})).unwrap();
        assert_eq!(s.learn.as_ref().unwrap().step, 2);
        let cur = s.execute("learn.state", json!({})).unwrap();
        let lid = s.state.selected_layers[0].0;
        assert_eq!(cur["current"]["target"], json!(format!("timeline.layer.{lid}.twirl")));
        // Back and Next.
        s.execute("learn.step", json!({"action": "back"})).unwrap();
        assert_eq!(s.learn.as_ref().unwrap().step, 1);
        s.execute("learn.step", json!({})).unwrap();
        assert_eq!(s.learn.as_ref().unwrap().step, 2);
        // A matching command with the wrong parameters doesn't complete a `when` step.
        s.execute("learn.start", json!({"id": "3d-scene"})).unwrap();
        s.execute("learn.step", json!({"action": "goto", "index": 2})).unwrap();
        s.execute("layer.setSwitch", json!({"switch": "shy", "value": true})).ok();
        assert_eq!(s.learn.as_ref().unwrap().step, 2);
        s.execute("learn.stop", json!({})).unwrap();
        assert!(s.learn.is_none());
        assert!(s.execute("learn.step", json!({})).is_err());
    }
}
