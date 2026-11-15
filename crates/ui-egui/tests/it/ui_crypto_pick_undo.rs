//! Global undo / redo with the Cryptomatte eyedropper (#473): every pick is one undo step that
//! never merges with a neighbouring edit, and Ctrl+Z / Ctrl+Shift+Z (the app's bindings; Ctrl+Y
//! is New Solid, as in After Effects) walk a model of the expected states over long sequences of
//! picks mixed with ordinary edits, at Full and Third, in two window sizes. After every step the
//! Selection, the other properties, the Edit menu's Undo / Redo entries, the History panel, the
//! viewer's chip and highlight, and the eyedropper's armed state are checked. Undo while the IDs
//! are being read and with clicks waiting, across Open / Revert / New, and after Esc.

use crate::ui_crypto_pick::{Held, ReadLog, at, chip, crypto_exr, settle, wait_held_id_read, wait_probe};
use effectcraft_engine::Session;
use effectcraft_engine::project::{LayerId, Resolution};
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::{self, PanelKind, SplitSize};
use effectcraft_ui_egui::panels::viewer::comp_to_screen;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

const READING: &str = "Reading Cryptomatte IDs…";
const HINT: &str = "Click: replace · Shift: add · Alt or Ctrl: remove";
const CRYPTO: &str = "ec.3d.cryptomatte";
/// Settings ▸ General ▸ Undo Levels in these tests (the most allowed).
const LEVELS: u32 = 99;
/// The app's Cmd (Ctrl+Z / Ctrl+Shift+Z) as winit reports it: Ctrl off macOS (`command` is Ctrl
/// there), Cmd on macOS.
const MAC: bool = cfg!(target_os = "macos");
const CTRL: Modifiers = Modifiers { alt: false, ctrl: !MAC, shift: false, mac_cmd: MAC, command: true };
const CTRL_SHIFT: Modifiers = Modifiers { alt: false, ctrl: !MAC, shift: true, mac_cmd: MAC, command: true };

// ---------------------------------------------------------------------------------------------
// Fixture: session / populate / harness (shared EXR helpers from ui_crypto_pick.rs).

struct Fix {
    comp: u64,
    /// The footage layer (Cryptomatte EXR) and the precomp layer of a 3D comp, each with a
    /// Cryptomatte effect, and a plain solid.
    plate: u64,
    pre: u64,
    other: u64,
    inner: u64,
    dir: std::path::PathBuf,
}

/// A `w` × `h` comp: the plate (optionally behind a heavy blur, so its probe takes a while),
/// with a precomp of two 3D solids (`PA` full size, `PB` a 60 px strip at the left) on top, and a
/// solid. Each Cryptomatte layer has the effect. The history starts empty.
fn session(tag: &str, w: usize, h: usize, blur: bool, precomp: bool) -> (Session, Fix) {
    let dir = std::env::temp_dir().join(format!("ec-ui-crypick-undo-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = crypto_exr(&dir, "plate.exr", w, h, 1);
    let mut s = effectcraft_host::session();
    let f = populate(&mut s, &path, w, h, blur, precomp, dir);
    (s, f)
}

/// [`session`]'s project, built in `s`'s current project from the EXR at `path`.
fn populate(s: &mut Session, path: &str, w: usize, h: usize, blur: bool, precomp: bool, dir: std::path::PathBuf) -> Fix {
    let r = s.execute("file.import", json!({"paths": [path]})).unwrap();
    let footage = r["items"][0].as_u64().unwrap();
    let comp = s.execute("comp.new", json!({"name": "Crypto", "width": w, "height": h, "frameRate": 24, "duration": 2})).unwrap();
    let comp = comp.get("comp").and_then(Value::as_u64).unwrap_or_else(|| s.active_comp_id().unwrap().0);
    let plate = s.execute("layer.addItem", json!({"item": footage})).unwrap()["layer"].as_u64().unwrap();
    s.state.selected_layers = vec![LayerId(plate)];
    if blur {
        s.execute_checked("effect.apply", json!({"effect": "Gaussian Blur"})).unwrap();
        s.execute_checked("prop.set", json!({"path": "effects/#1/blurriness", "value": 40.0})).unwrap();
    }
    s.execute_checked("effect.apply", json!({"effect": "Cryptomatte"})).unwrap();
    let (mut pre, mut inner) = (0, 0);
    if precomp {
        let a = s.execute("layer.newSolid", json!({"name": "PA", "width": w, "height": h})).unwrap()["layer"].clone();
        let b = s.execute("layer.newSolid", json!({"name": "PB", "width": 60, "height": h})).unwrap()["layer"].clone();
        s.execute_checked("prop.set", json!({"layer": b, "path": "transform/position", "value": [30.0, h as f64 / 2.0, -10.0]})).unwrap();
        s.execute("layer.setSwitch", json!({"layers": [a, b], "switch": "threeD", "value": true})).unwrap();
        s.execute("layer.precompose", json!({"layers": [a, b], "name": "Inner"})).unwrap();
        inner = s.project.items.iter().find(|(_, i)| i.name == "Inner").unwrap().0.0;
        pre = s.active_comp().unwrap().layers.iter().find(|l| l.name == "Inner").unwrap().id.0;
        s.execute_checked("effect.apply", json!({"layers": [pre], "effect": "Cryptomatte"})).unwrap();
    }
    let other = s.execute("layer.newSolid", json!({"name": "Other", "width": 64, "height": 64})).unwrap()["layer"].as_u64().unwrap();
    s.state.selected_layers = vec![LayerId(plate)];
    s.history = Default::default();
    s.prefs.general.undo_levels = LEVELS;
    Fix { comp, plate, pre, other, inner, dir }
}

/// Composition viewer and Effect Controls side by side, the Timeline and the History panel below.
fn harness(s: Session, size: egui::Vec2) -> Harness<'static, EffectcraftApp> {
    let mut app = EffectcraftApp::new(s);
    app.ui.viewer.show_layer_controls = false;
    app.ui.dock = dock::docked(dock::vsplit(
        SplitSize::FixedSecond(200.0),
        dock::hsplit(SplitSize::FixedSecond(420.0), dock::tabs(&[PanelKind::Composition], 0), dock::tabs(&[PanelKind::EffectControls], 0)),
        dock::hsplit(SplitSize::FixedSecond(300.0), dock::tabs(&[PanelKind::Timeline], 0), dock::tabs(&[PanelKind::History], 0)),
    ));
    let mut h = Harness::builder().with_size(size).with_pixels_per_point(1.0).with_step_dt(0.25).build_eframe(|_| app);
    settle(&mut h);
    h
}

fn highlight(h: &Harness<'_, EffectcraftApp>) -> bool {
    h.state().auto.find("viewer.cryptoPick.highlight").is_some()
}

/// Like `ui_crypto_pick::hover`, plus an extra step so the chip label is current after the probe.
fn hover(h: &mut Harness<'_, EffectcraftApp>, pos: Pos2) -> String {
    h.event(Event::PointerMoved(pos));
    h.step();
    wait_probe(h);
    h.step();
    chip(h).unwrap_or_default()
}

/// Press and release at `pos` with `modifiers`, without waiting for the probe.
fn press(h: &mut Harness<'_, EffectcraftApp>, pos: Pos2, modifiers: Modifiers) {
    h.event(Event::PointerMoved(pos));
    h.event(Event::ModifiersChanged(modifiers));
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers });
        h.step();
    }
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
}

fn click(h: &mut Harness<'_, EffectcraftApp>, pos: Pos2, modifiers: Modifiers) {
    press(h, pos, modifiers);
    wait_probe(h);
    h.run_steps(2);
}

fn key(h: &mut Harness<'_, EffectcraftApp>, key: Key, modifiers: Modifiers) {
    h.event(Event::ModifiersChanged(modifiers));
    h.event(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
    h.event(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
}

/// The Selection property uid of the layer's (first) Cryptomatte effect.
fn sel_uid(s: &Session, comp: u64, layer: u64) -> Option<u64> {
    let l = s.project.comp(effectcraft_engine::project::ItemId(comp))?.layer(LayerId(layer))?;
    l.effects()?.groups().find(|g| g.match_id == CRYPTO)?.get("selection").map(|p| p.uid)
}

/// Select `layer` (Effect Controls shows it) and arm its Cryptomatte eyedropper.
fn arm(h: &mut Harness<'_, EffectcraftApp>, f: &Fix, layer: u64) -> u64 {
    h.state_mut().session.state.selected_layers = vec![LayerId(layer)];
    h.run_steps(3);
    let uid = sel_uid(&h.state().session, f.comp, layer).expect("a Cryptomatte effect");
    let eye = at(h, &format!("effectControls.prop.{uid}.eyedropper"));
    click(h, eye, Modifiers::NONE);
    assert!(h.state().ui.fx_pick.as_ref().is_some_and(|p| p.kind == "crypto" && p.prop == uid && p.layer == layer), "armed on {layer}");
    uid
}

/// Type `text` into the Selection field of `layer` (shown in Effect Controls), committed with Enter.
fn type_selection(h: &mut Harness<'_, EffectcraftApp>, f: &Fix, layer: u64, text: &str, commit: bool) {
    h.state_mut().session.state.selected_layers = vec![LayerId(layer)];
    h.run_steps(3);
    let uid = sel_uid(&h.state().session, f.comp, layer).unwrap();
    let field = at(h, &format!("effectControls.prop.{uid}.value"));
    click(h, field, Modifiers::NONE);
    h.key_press_modifiers(CTRL, Key::A);
    h.step();
    h.event(Event::Text(text.into()));
    h.step();
    if commit {
        key(h, Key::Enter, Modifiers::NONE);
    }
}

// ---------------------------------------------------------------------------------------------
// Observation and model.

#[derive(Clone, Debug, PartialEq)]
struct LayerObs {
    id: u64,
    name: String,
    opacity: Value,
    opacity_keys: Vec<(f64, Value)>,
    position: Value,
    /// (match id, Selection uid and value for Cryptomatte) per effect.
    effects: Vec<(String, Option<(u64, String)>)>,
}

impl LayerObs {
    fn crypto(&self) -> Option<&(u64, String)> {
        self.effects.iter().find_map(|e| e.1.as_ref())
    }
    fn crypto_mut(&mut self) -> &mut (u64, String) {
        self.effects.iter_mut().find_map(|e| e.1.as_mut()).expect("a Cryptomatte effect")
    }
}

type Obs = Vec<LayerObs>;

fn observe(s: &Session, comp: u64) -> Obs {
    let c = s.project.comp(effectcraft_engine::project::ItemId(comp)).unwrap();
    c.layers
        .iter()
        .map(|l| {
            let tr = l.transform().unwrap();
            let op = tr.get("opacity").unwrap();
            let effects = l
                .effects()
                .map(|fx| {
                    fx.groups()
                        .map(|g| {
                            let sel = (g.match_id == CRYPTO).then(|| {
                                let p = g.get("selection").unwrap();
                                (p.uid, p.value.to_json().as_str().unwrap_or_default().to_string())
                            });
                            (g.match_id.clone(), sel)
                        })
                        .collect()
                })
                .unwrap_or_default();
            LayerObs {
                id: l.id.0,
                name: l.name.clone(),
                opacity: op.value.to_json(),
                opacity_keys: op.keys.iter().map(|k| (k.time.seconds(), k.value.to_json())).collect(),
                position: tr.get("position").unwrap().value.to_json(),
                effects,
            }
        })
        .collect()
}

fn sel_of(o: &Obs, id: u64) -> String {
    o.iter().find(|l| l.id == id).and_then(|l| l.crypto()).map(|c| c.1.clone()).unwrap_or_default()
}

fn layer_mut(o: &mut Obs, id: u64) -> &mut LayerObs {
    o.iter_mut().find(|l| l.id == id).unwrap_or_else(|| panic!("no layer {id}"))
}

/// The Selection after a pick of token `t` (as `edit_selection` writes it: `, ` separated).
fn model_pick(sel: &str, t: &str, mode: &str) -> String {
    let mut v: Vec<String> = sel.split(", ").filter(|s| !s.is_empty()).map(str::to_string).collect();
    match mode {
        "replace" => v = vec![t.to_string()],
        "add" => {
            if !v.iter().any(|x| x == t) {
                v.push(t.to_string())
            }
        }
        _ => v.retain(|x| x != t),
    }
    v.join(", ")
}

fn mode_of(m: Modifiers) -> &'static str {
    if m.alt || m.ctrl || m.command {
        "remove"
    } else if m.shift {
        "add"
    } else {
        "replace"
    }
}

struct Model {
    comp: u64,
    /// The working line: (label of the step that made it, expected state); `cur` is current.
    line: Vec<(String, Obs)>,
    cur: usize,
    time: f64,
    /// (layer, Selection uid) the eyedropper is armed on.
    armed: Option<(u64, u64)>,
    branches: usize,
    /// States per branch (the history bounds them by the undo levels).
    branch_states: Vec<usize>,
    /// Every distinct undo label seen, with the step that made it.
    labels: Vec<(String, String)>,
    /// Undo / redo steps after which the probe had to be built again, of those checked.
    rebuilds: (usize, usize),
    checks: usize,
}

impl Model {
    fn new(h: &Harness<'_, EffectcraftApp>, comp: u64) -> Model {
        let s = &h.state().session;
        Model {
            comp,
            line: vec![("Original".into(), observe(s, comp))],
            cur: 0,
            time: s.time_of(effectcraft_engine::project::ItemId(comp)).seconds(),
            armed: None,
            branches: 0,
            branch_states: vec![],
            labels: vec![],
            rebuilds: (0, 0),
            checks: 0,
        }
    }
    fn state(&self) -> &Obs {
        &self.line[self.cur].1
    }
    /// A step that records: the redo line becomes a branch.
    fn record(&mut self, label: &str, o: Obs, what: &str) {
        if self.cur + 1 < self.line.len() {
            self.branches += 1;
            self.branch_states.push(self.line.len() - 1 - self.cur);
            // The oldest branches go beyond the undo levels (no branch here grows from another).
            while self.branch_states.iter().sum::<usize>() > LEVELS as usize {
                self.branch_states.remove(0);
                self.branches -= 1;
            }
        }
        self.line.truncate(self.cur + 1);
        self.line.push((label.to_string(), o));
        self.cur += 1;
        assert!(self.cur <= LEVELS as usize, "the model doesn't trim the working line");
        if !self.labels.iter().any(|(l, _)| l == label) {
            self.labels.push((label.to_string(), what.to_string()));
        }
    }
    /// Undo or redo moved to another state: the pick ends when its effect is gone there.
    fn moved(&mut self) {
        if let Some((l, uid)) = self.armed
            && !self.state().iter().any(|x| x.id == l && x.crypto().is_some_and(|c| c.0 == uid))
        {
            self.armed = None;
        }
    }
}

/// The comp point of layer point `p` (no scale, rotation or anchor change in these tests).
fn comp_point(o: &Obs, layer: u64, p: [f64; 2], size: [f64; 2]) -> [f32; 2] {
    let l = o.iter().find(|l| l.id == layer).unwrap();
    let pos = &l.position;
    let (dx, dy) = (pos[0].as_f64().unwrap() - size[0] / 2.0, pos[1].as_f64().unwrap() - size[1] / 2.0);
    [(p[0] + dx) as f32, (p[1] + dy) as f32]
}

/// Where the checks hover on each Cryptomatte layer (layer point) and the ID they must name.
fn probe_point(f: &Fix, layer: u64) -> ([f64; 2], &'static str) {
    if layer == f.plate { ([55.5, 43.5], "c1_1") } else { ([20.0, 90.0], "PB") }
}

/// Every check after a step: state, Edit menu, History panel, time, armed state, chip, highlight.
fn verify(h: &mut Harness<'_, EffectcraftApp>, f: &Fix, m: &mut Model, size: [f64; 2], what: &str) {
    m.checks += 1;
    // (Two passes: the panels draw what the last edit left, and their rows are current.)
    h.run_steps(2);
    let got = observe(&h.state().session, m.comp);
    assert_eq!(got, *m.state(), "{what}: project state");
    // Edit ▸ Undo / Redo: labels and availability.
    let items = effectcraft_ui_egui::menus::menu_items(h.state());
    let item = |id: &str| items.iter().find(|i| i.id == id).map(|i| (i.label.clone(), i.enabled)).unwrap();
    let want_undo = if m.cur > 0 { (format!("Undo {}", m.line[m.cur].0), true) } else { ("Can't Undo".to_string(), false) };
    let want_redo = if m.cur + 1 < m.line.len() { (format!("Redo {}", m.line[m.cur + 1].0), true) } else { ("Can't Redo".to_string(), false) };
    assert_eq!((item("edit.undo"), item("edit.redo")), (want_undo, want_redo), "{what}: Edit menu");
    let s = &h.state().session;
    assert_eq!((s.history.undo.len(), s.history.redo.len()), (m.cur, m.line.len() - 1 - m.cur), "{what}: history depth");
    // The History panel: the working line's labels, the current state highlighted and in view.
    let tree = s.history_tree();
    let line: Vec<&str> = tree.iter().filter(|n| n.line).map(|n| n.label.as_str()).collect();
    let want: Vec<&str> = m.line.iter().map(|l| l.0.as_str()).collect();
    assert_eq!(line, want, "{what}: History panel line");
    let cur = tree.iter().find(|n| n.current).unwrap();
    assert_eq!(cur.label, m.line[m.cur].0, "{what}: History current");
    assert_eq!(s.history.branches.len(), m.branches, "{what}: History branches");
    let row = h.state().auto.find(&format!("history.state.{}", cur.index)).map(|e| e.label.clone());
    assert_eq!(row.as_deref(), Some(m.line[m.cur].0.as_str()), "{what}: History panel row of the current state");
    // The time is not part of the history.
    let t = s.time_of(effectcraft_engine::project::ItemId(m.comp)).seconds();
    assert!((t - m.time).abs() < 1e-9, "{what}: time {t} != {}", m.time);
    // The eyedropper: armed where the model says, the chip names what is under the pointer
    // (selected or not as the Selection says), and the hovered ID is highlighted.
    let pick = h.state().ui.fx_pick.as_ref().map(|p| (p.layer, p.prop));
    assert_eq!(pick, m.armed, "{what}: armed");
    match m.armed {
        Some((layer, _)) => {
            let (lp, name) = probe_point(f, layer);
            let p = comp_point(m.state(), layer, lp, size);
            let sp = comp_to_screen(&h.ctx, p).unwrap();
            let sel = m.state().iter().find(|l| l.id == layer).unwrap().crypto().unwrap().1.clone();
            let selected = sel.split(", ").any(|t| t == name);
            let want = if selected { format!("{name} (selected)") } else { name.to_string() };
            assert_eq!(hover(h, sp), want, "{what}: chip");
            assert!(highlight(h), "{what}: the hovered ID is highlighted");
        }
        None => {
            h.step();
            assert_eq!(chip(h), None, "{what}: no chip when disarmed");
            assert!(!highlight(h), "{what}: no highlight when disarmed");
        }
    }
}

/// Undo (`redo` false) or redo with the keyboard, then check against the model. Counts whether
/// the probe was built again for the step.
fn step_history(h: &mut Harness<'_, EffectcraftApp>, f: &Fix, m: &mut Model, size: [f64; 2], redo: bool, what: &str) {
    let can = if redo { m.cur + 1 < m.line.len() } else { m.cur > 0 };
    let builds = h.state().session.crypto_probe_stats().1;
    // A step that changed only Selections (a pick or a typed Selection).
    let only_sel = can && {
        let other = if redo { m.cur + 1 } else { m.cur - 1 };
        let strip = |o: &Obs| {
            o.iter()
                .map(|l| LayerObs { effects: l.effects.iter().map(|e| (e.0.clone(), e.1.as_ref().map(|c| (c.0, String::new())))).collect(), ..l.clone() })
                .collect::<Vec<_>>()
        };
        strip(&m.line[other].1) == strip(m.state())
    };
    if redo {
        key(h, Key::Z, CTRL_SHIFT);
    } else {
        key(h, Key::Z, CTRL);
    }
    if can {
        if redo {
            m.cur += 1;
        } else {
            m.cur -= 1;
        }
    }
    m.moved();
    let armed = m.armed.is_some();
    verify(h, f, m, size, what);
    if armed && only_sel {
        m.rebuilds.1 += 1;
        if h.state().session.crypto_probe_stats().1 > builds {
            m.rebuilds.0 += 1;
        }
    }
}

/// One forward step of the long sequence.
enum Step {
    /// Click at a layer point of the armed layer: the token it names and whether it is an edit.
    Pick([f64; 2], Modifiers, &'static str),
    /// Click where the armed layer has no ID: no step.
    PickEmpty,
    Arm(u64),
    Esc,
    Opacity(u64, f64, Option<&'static str>),
    Position(u64, [f64; 2]),
    Type(u64, &'static str),
    OpacityKey(u64, f64, f64),
    Time(f64),
    RemoveCrypto(u64),
    ApplyCrypto(u64),
    DeleteLayer(u64),
    AddPrecompLayer,
    /// The plate's blur (before the Cryptomatte effect): drops the probe, slow to rebuild.
    Blur(u64, f64),
}

/// Performs one model step (a pick, an edit, undo or redo, applying or removing the effect) and
/// records the state the model expects after it.
fn run_step(h: &mut Harness<'_, EffectcraftApp>, f: &Fix, m: &mut Model, size: [f64; 2], step: &Step, n: usize) {
    let mut o = m.state().clone();
    let before = h.state().session.history.undo.len();
    let (what, records): (String, bool) = match step {
        Step::Pick(lp, mods, token) => {
            let (layer, _) = m.armed.expect("armed");
            let p = comp_point(&o, layer, *lp, size);
            click(h, comp_to_screen(&h.ctx, p).unwrap(), *mods);
            let c = layer_mut(&mut o, layer).crypto_mut();
            let next = model_pick(&c.1, token, mode_of(*mods));
            let changed = next != c.1;
            c.1 = next;
            (format!("#{n} pick {token} {}", mode_of(*mods)), changed)
        }
        Step::PickEmpty => {
            let (layer, _) = m.armed.expect("armed");
            // Just off the layer's left or right edge, inside the viewer.
            let area = h.state().auto.find("viewer.fxPick").unwrap().rect;
            let inside = |p: Pos2| p.x > area[0] + 2.0 && p.x < area[0] + area[2] - 2.0 && p.y > area[1] + 40.0 && p.y < area[1] + area[3] - 2.0;
            let sp = [[-6.0, 120.0], [size[0] + 6.0, 120.0], [100.0, size[1] + 6.0]]
                .into_iter()
                .map(|lp| comp_to_screen(&h.ctx, comp_point(&o, layer, lp, size)).unwrap())
                .find(|p| inside(*p))
                .expect("an empty point in the viewer");
            assert_eq!(hover(h, sp), HINT);
            click(h, sp, Modifiers::NONE);
            (format!("#{n} click off the IDs"), false)
        }
        Step::Arm(layer) => {
            let uid = arm(h, f, *layer);
            m.armed = Some((*layer, uid));
            (format!("#{n} arm {layer}"), false)
        }
        Step::Esc => {
            key(h, Key::Escape, Modifiers::NONE);
            m.armed = None;
            (format!("#{n} Esc"), false)
        }
        Step::Opacity(layer, v, merge) => {
            let mut q = json!({"comp": f.comp, "layer": layer, "path": "transform/opacity", "value": v});
            if let Some(k) = merge {
                q["merge"] = json!(k);
            }
            h.state_mut().session.execute_checked("prop.set", q).unwrap();
            layer_mut(&mut o, *layer).opacity = json!(v);
            (format!("#{n} opacity {layer} {v} merge {merge:?}"), true)
        }
        Step::Position(layer, v) => {
            let l = layer_mut(&mut o, *layer);
            let mut p = l.position.clone();
            (p[0], p[1]) = (json!(v[0]), json!(v[1]));
            h.state_mut().session.execute_checked("prop.set", json!({"comp": f.comp, "layer": layer, "path": "transform/position", "value": p})).unwrap();
            l.position = p;
            (format!("#{n} position {layer} {v:?}"), true)
        }
        Step::Type(layer, text) => {
            type_selection(h, f, *layer, text, true);
            layer_mut(&mut o, *layer).crypto_mut().1 = text.to_string();
            // Typing selects the layer; the pick (if any) stays where it was.
            (format!("#{n} type {text:?}"), true)
        }
        Step::OpacityKey(layer, t, v) => {
            h.state_mut()
                .session
                .execute_checked("prop.addKey", json!({"comp": f.comp, "layer": layer, "path": "transform/opacity", "time": t, "value": v}))
                .unwrap();
            layer_mut(&mut o, *layer).opacity_keys.push((*t, json!(v)));
            (format!("#{n} opacity key {layer} {t}s {v}"), true)
        }
        Step::Time(t) => {
            h.state_mut().session.set_time(effectcraft_engine::time::Tick::from_seconds_f64(*t));
            m.time = *t;
            settle(h);
            (format!("#{n} time {t}"), false)
        }
        Step::RemoveCrypto(layer) => {
            let uid = o.iter().find(|l| l.id == *layer).unwrap().effects.iter().position(|e| e.0 == CRYPTO).unwrap();
            h.state_mut().session.execute_checked("effect.remove", json!({"comp": f.comp, "layer": layer, "effect": uid + 1})).unwrap();
            layer_mut(&mut o, *layer).effects.retain(|e| e.0 != CRYPTO);
            if m.armed.is_some_and(|a| a.0 == *layer) {
                m.armed = None;
            }
            (format!("#{n} remove Cryptomatte from {layer}"), true)
        }
        Step::ApplyCrypto(layer) => {
            h.state_mut().session.execute_checked("effect.apply", json!({"comp": f.comp, "layers": [layer], "effect": "Cryptomatte"})).unwrap();
            // Not left selected: the model covers the armed eyedropper alone (#2839).
            h.state_mut().session.state.selected_props.clear();
            let uid = sel_uid(&h.state().session, f.comp, *layer).unwrap();
            layer_mut(&mut o, *layer).effects.push((CRYPTO.into(), Some((uid, String::new()))));
            (format!("#{n} apply Cryptomatte to {layer}"), true)
        }
        Step::DeleteLayer(layer) => {
            h.state_mut().session.execute_checked("edit.clear", json!({"comp": f.comp, "layers": [layer]})).unwrap();
            o.retain(|l| l.id != *layer);
            (format!("#{n} delete layer {layer}"), true)
        }
        Step::Blur(layer, v) => {
            h.state_mut().session.execute_checked("prop.set", json!({"comp": f.comp, "layer": layer, "path": "effects/#1/blurriness", "value": v})).unwrap();
            (format!("#{n} blur {v}"), true)
        }
        Step::AddPrecompLayer => {
            let r = h.state_mut().session.execute_checked("layer.addItem", json!({"comp": f.comp, "item": f.inner})).unwrap();
            let id = r["layer"].as_u64().unwrap();
            let now = observe(&h.state().session, f.comp);
            let new = now.iter().find(|l| l.id == id).unwrap().clone();
            assert!(new.name.starts_with("Inner") && new.opacity == json!(100.0) && new.effects.is_empty(), "a fresh precomp layer: {new:?}");
            let at = now.iter().position(|l| l.id == id).unwrap();
            o.insert(at, new);
            (format!("#{n} add precomp layer {id}"), true)
        }
    };
    h.run_steps(2);
    let after = h.state().session.history.undo.len();
    if records {
        assert_eq!(after, before + 1, "{what}: exactly one undo step");
        let label = h.state().session.history.undo.last().unwrap().0.clone();
        m.record(&label, o, &what);
    } else {
        assert_eq!(after, before, "{what}: no undo step");
        assert_eq!(m.state(), &o, "{what}: no change");
    }
    verify(h, f, m, size, &what);
}

fn long_sequence(tag: &str, size: egui::Vec2, res: Resolution) {
    let (w, hh) = (320usize, 180usize);
    let dims = [w as f64, hh as f64];
    let (mut s, f) = session(tag, w, hh, false, true);
    let cid = effectcraft_engine::project::ItemId(f.comp);
    s.set_resolution(cid, res).unwrap();
    let mut h = harness(s, size);
    let mut m = Model::new(&h, f.comp);
    verify(&mut h, &f, &mut m, dims, "start");
    let ctr = |cx: usize, cy: usize| [cx as f64 * 37.0 + 18.5, cy as f64 * 29.0 + 14.5];
    let (pl, pre, other) = (f.plate, f.pre, f.other);
    use Step::*;
    let steps = vec![
        Arm(pl),
        Pick(ctr(1, 1), Modifiers::NONE, "c1_1"),
        Opacity(pl, 80.0, Some("scrub-a")),
        Pick(ctr(3, 3), Modifiers::SHIFT, "c3_3"),
        Opacity(pl, 70.0, Some("scrub-a")),
        Position(pl, [170.0, 95.0]),
        Pick(ctr(1, 1), Modifiers::ALT, "c1_1"),
        Opacity(other, 50.0, None),
        Pick(ctr(3, 3), Modifiers::COMMAND, "c3_3"),
        PickEmpty,
        Pick(ctr(5, 2), Modifiers::NONE, "c5_2"),
        Pick(ctr(5, 2), Modifiers::SHIFT, "c5_2"),
        Pick(ctr(0, 0), Modifiers::ALT, "c0_0"),
        Type(pl, "c2_2, c5_2"),
        Pick(ctr(4, 4), Modifiers::SHIFT, "c4_4"),
        Time(1.0),
        OpacityKey(pl, 1.0, 60.0),
        Pick(ctr(6, 1), Modifiers::SHIFT, "c6_1"),
        Time(0.5),
        RemoveCrypto(pl),
        Opacity(other, 40.0, None),
        ApplyCrypto(pl),
        Arm(pl),
        Pick(ctr(7, 3), Modifiers::NONE, "c7_3"),
        Arm(pre),
        Pick([20.0, 90.0], Modifiers::NONE, "PB"),
        Pick([200.0, 90.0], Modifiers::SHIFT, "PA"),
        Opacity(pre, 30.0, None),
        Pick([20.0, 90.0], Modifiers::ALT, "PB"),
        DeleteLayer(other),
        Pick([20.0, 90.0], Modifiers::NONE, "PB"),
        AddPrecompLayer,
        Arm(pl),
        Pick(ctr(8, 4), Modifiers::SHIFT, "c8_4"),
    ];
    for (n, st) in steps.iter().enumerate() {
        run_step(&mut h, &f, &mut m, dims, st, n);
    }
    let total = m.cur;
    assert!(total >= 20, "{total} undo steps");
    // Undo all the way with Ctrl+Z, and once more (nothing left).
    for k in 0..=total {
        step_history(&mut h, &f, &mut m, dims, false, &format!("undo {k}"));
    }
    assert_eq!(m.cur, 0);
    // Redo all the way with Ctrl+Shift+Z, and once more.
    for k in 0..=total {
        step_history(&mut h, &f, &mut m, dims, true, &format!("redo {k}"));
    }
    assert_eq!(m.cur, total);
    // Ctrl+Y is New Solid (After Effects' binding), not Redo: it opens the Solid Settings dialog
    // and changes nothing; Esc closes the dialog.
    for _ in 0..3 {
        step_history(&mut h, &f, &mut m, dims, false, "undo before Ctrl+Y");
    }
    if m.armed.is_none() {
        let l = if m.state().iter().find(|l| l.id == pl).and_then(|l| l.crypto()).is_some() { pl } else { pre };
        run_step(&mut h, &f, &mut m, dims, &Arm(l), 99);
    }
    key(&mut h, Key::Y, CTRL);
    assert!(h.state().dialog.is_some(), "Ctrl+Y opens the New Solid dialog");
    assert_eq!(h.state().session.history.redo.len(), 3, "Ctrl+Y doesn't redo");
    key(&mut h, Key::Escape, Modifiers::NONE);
    assert!(h.state().dialog.is_none());
    assert!(h.state().ui.fx_pick.is_some(), "Esc that closes the New Solid dialog keeps the eyedropper armed");
    key(&mut h, Key::Escape, Modifiers::NONE);
    assert!(h.state().ui.fx_pick.is_none(), "a second Esc disarms");
    m.armed = None;
    verify(&mut h, &f, &mut m, dims, "after Ctrl+Y and Esc");
    // Undo partway, then a new pick: no redo left, the undone steps kept as a branch.
    for k in 0..4 {
        step_history(&mut h, &f, &mut m, dims, false, &format!("partway {k}"));
    }
    if m.armed.is_none() {
        let l = if m.state().iter().find(|l| l.id == pl).and_then(|l| l.crypto()).is_some() { pl } else { pre };
        run_step(&mut h, &f, &mut m, dims, &Arm(l), 100);
    }
    let (layer, _) = m.armed.unwrap();
    let (lp, name) = probe_point(&f, layer);
    let sel = m.state().iter().find(|l| l.id == layer).unwrap().crypto().unwrap().1.clone();
    let mods = if sel.split(", ").any(|t| t == name) { Modifiers::ALT } else { Modifiers::SHIFT };
    let branches = m.branches;
    run_step(&mut h, &f, &mut m, dims, &Pick(lp, mods, name), 101);
    assert_eq!((m.branches, m.cur + 1 == m.line.len()), (branches + 1, true), "the redo line became a branch");
    // Random undo / redo (fixed seed), with an occasional new pick or re-arm.
    let mut seed: u64 = 0x5eed_c0ff_ee12_3457;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for i in 0..400 {
        let r = rnd() % 20;
        if r < 9 {
            step_history(&mut h, &f, &mut m, dims, false, &format!("random {i} undo"));
        } else if r < 18 {
            step_history(&mut h, &f, &mut m, dims, true, &format!("random {i} redo"));
        } else {
            match m.armed {
                None => {
                    let l = [pl, pre].into_iter().find(|l| m.state().iter().find(|x| x.id == *l).is_some_and(|x| x.crypto().is_some()));
                    if let Some(l) = l {
                        run_step(&mut h, &f, &mut m, dims, &Arm(l), 200 + i);
                    }
                }
                Some((layer, _)) => {
                    let (lp, name) = probe_point(&f, layer);
                    let sel = m.state().iter().find(|l| l.id == layer).unwrap().crypto().unwrap().1.clone();
                    let mods = if sel.split(", ").any(|t| t == name) { Modifiers::ALT } else { Modifiers::SHIFT };
                    run_step(&mut h, &f, &mut m, dims, &Pick(lp, mods, name), 200 + i);
                }
            }
        }
    }
    eprintln!(
        "{tag}: {} checks; {} history states on the line, {} branches; probe rebuilt after {} of {} armed undo/redo steps that changed only a Selection",
        m.checks,
        m.line.len(),
        m.branches,
        m.rebuilds.0,
        m.rebuilds.1
    );
    for (l, w) in &m.labels {
        eprintln!("{tag}: LABEL {l:?} (first from {w})");
    }
    let _ = std::fs::remove_dir_all(&f.dir);
}

/// Scenario 2 at Full and Third, in a large and a small window.
#[test]
fn long_sequence_full_large_window() {
    long_sequence("full-l", vec2(1400.0, 860.0), Resolution::Full);
}

#[test]
fn long_sequence_third_large_window() {
    long_sequence("third-l", vec2(1400.0, 860.0), Resolution::Third);
}

#[test]
fn long_sequence_full_small_window() {
    long_sequence("full-s", vec2(1024.0, 700.0), Resolution::Full);
}

#[test]
fn long_sequence_third_small_window() {
    long_sequence("third-s", vec2(1024.0, 700.0), Resolution::Third);
}

// ---------------------------------------------------------------------------------------------
// Scenario 1: each pick kind is one step; clicks off the IDs and picks that change nothing are
// none; clicks queued while reading are one step each, undone in order.

#[test]
fn each_pick_is_one_step_and_clicks_that_change_nothing_are_none() {
    let (s, f) = session("kinds", 320, 180, false, false);
    let dims = [320.0, 180.0];
    let mut h = harness(s, vec2(1400.0, 860.0));
    let mut m = Model::new(&h, f.comp);
    let ctr = |cx: usize, cy: usize| [cx as f64 * 37.0 + 18.5, cy as f64 * 29.0 + 14.5];
    let pl = f.plate;
    let ctrl_only = Modifiers { ctrl: true, ..Modifiers::NONE };
    use Step::*;
    let steps = [
        Arm(pl),
        Pick(ctr(1, 1), Modifiers::NONE, "c1_1"),    // replace
        Pick(ctr(3, 3), Modifiers::SHIFT, "c3_3"),   // add
        Pick(ctr(3, 3), Modifiers::SHIFT, "c3_3"),   // add, already selected: no step
        Pick(ctr(1, 1), Modifiers::ALT, "c1_1"),     // remove
        Pick(ctr(1, 1), Modifiers::ALT, "c1_1"),     // remove, not selected: no step
        Pick(ctr(4, 2), Modifiers::SHIFT, "c4_2"),   // add
        Pick(ctr(4, 2), Modifiers::COMMAND, "c4_2"), // Ctrl (as `command`) removes
        Pick(ctr(5, 5), Modifiers::SHIFT, "c5_5"),   // add
        Pick(ctr(5, 5), ctrl_only, "c5_5"),          // Ctrl alone removes too
        Pick(ctr(3, 3), Modifiers::NONE, "c3_3"),    // replace with what it already is: no step
        PickEmpty,                                   // off the layer: no step
        Pick(ctr(2, 1), Modifiers::NONE, "c2_1"),    // replace
    ];
    for (n, st) in steps.iter().enumerate() {
        run_step(&mut h, &f, &mut m, dims, st, n);
    }
    assert_eq!(m.cur, 8, "eight picks changed the Selection");
    for k in 0..9 {
        step_history(&mut h, &f, &mut m, dims, false, &format!("undo {k}"));
    }
    for k in 0..9 {
        step_history(&mut h, &f, &mut m, dims, true, &format!("redo {k}"));
    }
    let _ = std::fs::remove_dir_all(&f.dir);
}

/// Clicks made while the IDs are read (a 4K plate behind a blur; the build is held until the
/// clicks are checked) are one undo step each, in the order clicked; Ctrl+Z takes them back one
/// at a time and Ctrl+Shift+Z makes them again.
#[test]
fn clicks_queued_while_reading_are_one_step_each_and_undo_in_order() {
    let (mut s, f) = session("queue", 3840, 2160, true, false);
    let dims = [3840.0, 2160.0];
    let hold = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let log = ReadLog::default();
    s.footage = std::sync::Arc::new(Held(s.footage.clone(), hold.clone(), log.clone()));
    let mut h = harness(s, vec2(1280.0, 860.0));
    let mut m = Model::new(&h, f.comp);
    let pl = f.plate;
    run_step(&mut h, &f, &mut m, dims, &Step::Arm(pl), 0);
    run_step(&mut h, &f, &mut m, dims, &Step::Blur(pl, 41.0), 1);
    let (a, b) = ([1017.5, 594.5], [1868.5, 1174.5]);
    let sp = |h: &Harness<'_, EffectcraftApp>, m: &Model, p: [f64; 2]| comp_to_screen(&h.ctx, comp_point(m.state(), pl, p, dims)).unwrap();
    // The verify above built the probe; drop it again.
    run_step(&mut h, &f, &mut m, dims, &Step::Blur(pl, 42.0), 2);
    let pa = sp(&h, &m, a);
    hold.store(true, Ordering::Release);
    log.lock().unwrap().clear();
    h.state_mut().session.execute_checked("prop.set", json!({"comp": f.comp, "layer": pl, "path": "effects/#1/blurriness", "value": 43.0})).unwrap();
    let label = h.state().session.history.undo.last().unwrap().0.clone();
    let o = m.state().clone();
    m.record(&label, o, "blur 43");
    h.event(Event::PointerMoved(pa));
    h.run_steps(2);
    assert_eq!(chip(&h).as_deref(), Some(READING));
    let before = h.state().session.history.undo.len();
    let clicks = [(a, Modifiers::NONE, "c27_20"), (b, Modifiers::SHIFT, "c50_40"), (a, Modifiers::ALT, "c27_20"), (a, Modifiers::SHIFT, "c27_20")];
    for (p, mods, _) in clicks {
        let q = sp(&h, &m, p);
        press(&mut h, q, mods);
    }
    assert_eq!(chip(&h).as_deref(), Some(READING), "all four made while reading");
    assert_eq!(h.state().session.history.undo.len(), before, "none made yet");
    wait_held_id_read(&mut h, &log);
    for _ in 0..10 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(chip(&h).as_deref(), Some(READING), "still reading while the build is held");
    assert_eq!(h.state().session.history.undo.len(), before, "none made yet while held");
    hold.store(false, Ordering::Release);
    wait_probe(&mut h);
    h.run_steps(3);
    assert_eq!(h.state().session.history.undo.len(), before + 4, "one step per click");
    // The model: each click on the state the previous one left.
    for (k, (_, mods, t)) in clicks.iter().enumerate() {
        let mut o = m.state().clone();
        let c = layer_mut(&mut o, pl).crypto_mut();
        c.1 = model_pick(&c.1, t, mode_of(*mods));
        let label = h.state().session.history.undo[before + k].0.clone();
        m.record(&label, o, &format!("queued click {k}"));
    }
    verify(&mut h, &f, &mut m, dims, "after the queued clicks");
    for k in 0..5 {
        step_history(&mut h, &f, &mut m, dims, false, &format!("undo {k}"));
    }
    for k in 0..5 {
        step_history(&mut h, &f, &mut m, dims, true, &format!("redo {k}"));
    }
    let _ = std::fs::remove_dir_all(&f.dir);
}

// ---------------------------------------------------------------------------------------------
// Scenario 3: undo / redo while armed, while reading, and with clicks waiting.

/// Ctrl+Z or Ctrl+Shift+Z without waiting for anything; the model follows.
fn undo_now(h: &mut Harness<'_, EffectcraftApp>, m: &mut Model, redo: bool) {
    if redo {
        key(h, Key::Z, CTRL_SHIFT);
        m.cur += 1;
    } else {
        key(h, Key::Z, CTRL);
        m.cur -= 1;
    }
    m.moved();
}

fn blur_value(h: &Harness<'_, EffectcraftApp>, f: &Fix) -> Value {
    let s = &h.state().session;
    let l = s.project.comp(effectcraft_engine::project::ItemId(f.comp)).unwrap().layer(LayerId(f.plate)).unwrap();
    l.effects().unwrap().groups().next().unwrap().get("blurriness").unwrap().value.to_json()
}

#[test]
fn undo_and_redo_while_reading_and_with_clicks_waiting() {
    let (mut s, f) = session("reading", 3840, 2160, true, false);
    let dims = [3840.0, 2160.0];
    let hold = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let log = ReadLog::default();
    s.footage = std::sync::Arc::new(Held(s.footage.clone(), hold.clone(), log.clone()));
    let mut h = harness(s, vec2(1280.0, 860.0));
    let mut m = Model::new(&h, f.comp);
    let pl = f.plate;
    let (a, b) = ([1017.5, 594.5], [1868.5, 1174.5]);
    let sp = |h: &Harness<'_, EffectcraftApp>, m: &Model, p: [f64; 2]| comp_to_screen(&h.ctx, comp_point(m.state(), pl, p, dims)).unwrap();
    use Step::*;
    for (n, st) in [Arm(pl), Pick(a, Modifiers::NONE, "c27_20"), Pick(b, Modifiers::SHIFT, "c50_40")].iter().enumerate() {
        run_step(&mut h, &f, &mut m, dims, st, n);
    }
    let blur = |h: &mut Harness<'_, EffectcraftApp>, m: &mut Model, v: f64| {
        h.state_mut().session.execute_checked("prop.set", json!({"comp": f.comp, "layer": pl, "path": "effects/#1/blurriness", "value": v})).unwrap();
        let label = h.state().session.history.undo.last().unwrap().0.clone();
        let o = m.state().clone();
        m.record(&label, o, &format!("blur {v}"));
    };

    // (a) Undo a pick while armed: the probe goes with the edit, the chip says it is reading,
    // then names the right IDs with the right "(selected)" for the state undone to.
    let pb = sp(&h, &m, b);
    assert_eq!(hover(&mut h, pb), "c50_40 (selected)");
    let builds = h.state().session.crypto_probe_stats().1;
    undo_now(&mut h, &mut m, false);
    let right_after = chip(&h);
    eprintln!("chip right after undoing a pick: {right_after:?}");
    assert_eq!(hover(&mut h, pb), "c50_40", "after undo: not selected any more");
    let pa = sp(&h, &m, a);
    assert_eq!(hover(&mut h, pa), "c27_20 (selected)");
    assert!(highlight(&h));
    let rebuilt = h.state().session.crypto_probe_stats().1 - builds;
    eprintln!("probes built for undoing one pick: {rebuilt}");
    verify(&mut h, &f, &mut m, dims, "(a) undo a pick while armed");

    // (b) Ctrl+Z while the IDs are being read (a build in flight for the state undone).
    blur(&mut h, &mut m, 41.0);
    hold.store(true, Ordering::Release);
    log.lock().unwrap().clear();
    h.event(Event::PointerMoved(pa));
    h.run_steps(2);
    wait_held_id_read(&mut h, &log);
    assert_eq!(chip(&h).as_deref(), Some(READING));
    undo_now(&mut h, &mut m, false);
    hold.store(false, Ordering::Release);
    assert_eq!(blur_value(&h, &f), json!(40.0));
    verify(&mut h, &f, &mut m, dims, "(b) undo while reading");
    assert_eq!(hover(&mut h, pb), "c50_40", "the probe answers for the state undone to");
    // ... and Ctrl+Shift+Z while reading.
    hold.store(true, Ordering::Release);
    log.lock().unwrap().clear();
    h.state_mut().session.drop_crypto_probe();
    h.event(Event::PointerMoved(pa));
    h.run_steps(2);
    wait_held_id_read(&mut h, &log);
    assert_eq!(chip(&h).as_deref(), Some(READING));
    undo_now(&mut h, &mut m, true);
    hold.store(false, Ordering::Release);
    assert_eq!(blur_value(&h, &f), json!(41.0));
    verify(&mut h, &f, &mut m, dims, "(b) redo while reading");

    // (c) A click waiting for the probe, then Ctrl+Z: the undo goes first and the waiting click
    // is not made on the state undone to (it was aimed at the state before the undo).
    blur(&mut h, &mut m, 42.0);
    hold.store(true, Ordering::Release);
    log.lock().unwrap().clear();
    h.event(Event::PointerMoved(pb));
    h.run_steps(2);
    wait_held_id_read(&mut h, &log);
    assert_eq!(chip(&h).as_deref(), Some(READING));
    press(&mut h, pb, Modifiers::SHIFT);
    assert_eq!(chip(&h).as_deref(), Some(READING), "the click waits");
    h.state_mut().ui.status.clear();
    undo_now(&mut h, &mut m, false);
    // (Asked at once: a status message goes after 5 s, which is 20 steps here.)
    assert!(h.state().ui.status.starts_with("Pick not made"), "the dropped click says so: {:?}", h.state().ui.status);
    hold.store(false, Ordering::Release);
    wait_probe(&mut h);
    h.run_steps(3);
    verify(&mut h, &f, &mut m, dims, "(c) waiting click, then Ctrl+Z: the click is not made");

    // (d) Ctrl+Z, then a click while the probe is rebuilt: the click is made on the state undone
    // to, as one step, and the redo line becomes a branch.
    undo_now(&mut h, &mut m, false); // the blur 41
    undo_now(&mut h, &mut m, false); // the pick of c27_20: the Selection is now empty
    assert_eq!(sel_of(m.state(), pl), "", "{:?}", m.state());
    let pb = sp(&h, &m, b);
    h.event(Event::PointerMoved(pb));
    h.run_steps(2);
    // (The blur's render is in the layer cache: the rebuild can be done already.)
    press(&mut h, pb, Modifiers::SHIFT);
    wait_probe(&mut h);
    h.run_steps(3);
    let mut o = m.state().clone();
    layer_mut(&mut o, pl).crypto_mut().1 = "c50_40".into();
    let label = h.state().session.history.undo.last().unwrap().0.clone();
    m.record(&label, o, "(d) click after undo");
    verify(&mut h, &f, &mut m, dims, "(d) Ctrl+Z, then a click while reading");

    // (e) Ctrl+Z, a click waiting, then Ctrl+Shift+Z: the click is not made.
    blur(&mut h, &mut m, 43.0);
    verify(&mut h, &f, &mut m, dims, "(e) blur 43");
    // (Undo goes back to a render the layer cache keeps: forget it, so the probe takes a while.)
    // Hold before undo_now: it steps the UI and would otherwise start (and finish) a rebuild.
    h.state().session.layer_cache.clear();
    hold.store(true, Ordering::Release);
    log.lock().unwrap().clear();
    undo_now(&mut h, &mut m, false);
    let pa = sp(&h, &m, a);
    h.event(Event::PointerMoved(pa));
    h.run_steps(2);
    wait_held_id_read(&mut h, &log);
    assert_eq!(chip(&h).as_deref(), Some(READING));
    press(&mut h, pa, Modifiers::SHIFT);
    undo_now(&mut h, &mut m, true);
    hold.store(false, Ordering::Release);
    wait_probe(&mut h);
    h.run_steps(3);
    verify(&mut h, &f, &mut m, dims, "(e) waiting click, then Ctrl+Shift+Z: the click is not made");

    // (f) A click waiting, Ctrl+Z, another click: only the second is made, on the undone state.
    hold.store(true, Ordering::Release);
    log.lock().unwrap().clear();
    blur(&mut h, &mut m, 44.0);
    h.event(Event::PointerMoved(pa));
    h.run_steps(2);
    wait_held_id_read(&mut h, &log);
    assert_eq!(chip(&h).as_deref(), Some(READING));
    press(&mut h, pa, Modifiers::ALT);
    undo_now(&mut h, &mut m, false);
    press(&mut h, pa, Modifiers::SHIFT);
    assert_eq!(chip(&h).as_deref(), Some(READING), "both clicks made while the build is held");
    hold.store(false, Ordering::Release);
    wait_probe(&mut h);
    h.run_steps(3);
    let mut o = m.state().clone();
    let c = layer_mut(&mut o, pl).crypto_mut();
    c.1 = model_pick(&c.1, "c27_20", "add");
    let label = h.state().session.history.undo.last().unwrap().0.clone();
    m.record(&label, o, "(f) second click");
    verify(&mut h, &f, &mut m, dims, "(f) click, Ctrl+Z, click: only the second is made");
    for k in 0..m.cur {
        step_history(&mut h, &f, &mut m, dims, false, &format!("undo all {k}"));
    }
    let _ = std::fs::remove_dir_all(&f.dir);
}

// ---------------------------------------------------------------------------------------------
// Scenario 4: Esc mid-sequence; undo across Revert, Open and New.

#[test]
fn esc_mid_sequence_and_undo_across_revert_open_and_new() {
    let (s, f) = session("reload", 320, 180, false, false);
    let dims = [320.0, 180.0];
    let mut h = harness(s, vec2(1400.0, 860.0));
    let mut m = Model::new(&h, f.comp);
    let ctr = |cx: usize, cy: usize| [cx as f64 * 37.0 + 18.5, cy as f64 * 29.0 + 14.5];
    let pl = f.plate;
    use Step::*;
    let steps = [Arm(pl), Pick(ctr(1, 1), Modifiers::NONE, "c1_1"), Opacity(f.other, 50.0, None), Pick(ctr(3, 3), Modifiers::SHIFT, "c3_3"), Esc];
    for (n, st) in steps.iter().enumerate() {
        run_step(&mut h, &f, &mut m, dims, st, n);
    }
    // Disarmed: undo and redo still walk the picks, and don't arm it again.
    for k in 0..3 {
        step_history(&mut h, &f, &mut m, dims, false, &format!("undo after Esc {k}"));
    }
    for k in 0..3 {
        step_history(&mut h, &f, &mut m, dims, true, &format!("redo after Esc {k}"));
    }
    // Esc while armed with a pick just made: one Esc, no step, no undo.
    run_step(&mut h, &f, &mut m, dims, &Arm(pl), 10);
    run_step(&mut h, &f, &mut m, dims, &Pick(ctr(2, 2), Modifiers::SHIFT, "c2_2"), 11);
    run_step(&mut h, &f, &mut m, dims, &Esc, 12);
    // Saved, then one more pick, then Revert: the saved state, no history, still a sane pick.
    let path = f.dir.join("saved.ecproj");
    h.state_mut().session.execute("file.save", json!({"path": path.to_string_lossy()})).unwrap();
    let saved = m.state().clone();
    run_step(&mut h, &f, &mut m, dims, &Arm(pl), 13);
    run_step(&mut h, &f, &mut m, dims, &Pick(ctr(4, 4), Modifiers::SHIFT, "c4_4"), 14);
    let pick = h.state().ui.fx_pick.clone();
    h.state_mut().session.execute("file.revert", json!({})).unwrap();
    h.run_steps(3);
    let armed_after_revert = h.state().ui.fx_pick.clone();
    eprintln!("armed before Revert {pick:?}, after {armed_after_revert:?}; chip {:?}", chip(&h));
    m.line = vec![("Original".into(), saved.clone())];
    (m.cur, m.branches) = (0, 0);
    m.branch_states.clear();
    m.armed = armed_after_revert.as_ref().map(|p| (p.layer, p.prop));
    verify(&mut h, &f, &mut m, dims, "after Revert");
    step_history(&mut h, &f, &mut m, dims, false, "Ctrl+Z after Revert: nothing");
    if m.armed.is_none() {
        run_step(&mut h, &f, &mut m, dims, &Arm(pl), 15);
    }
    run_step(&mut h, &f, &mut m, dims, &Pick(ctr(5, 1), Modifiers::SHIFT, "c5_1"), 16);
    step_history(&mut h, &f, &mut m, dims, false, "undo the pick after Revert");
    step_history(&mut h, &f, &mut m, dims, true, "redo the pick after Revert");
    // New Project: the pick ends, nothing to undo.
    h.state_mut().session.execute("file.newProject", json!({})).unwrap();
    h.run_steps(3);
    eprintln!("after New Project: armed {:?}, chip {:?}, status {:?}", h.state().ui.fx_pick, chip(&h), h.state().ui.status);
    assert!(chip(&h).is_none(), "no viewer, no chip");
    let items = effectcraft_ui_egui::menus::menu_items(h.state());
    let undo = items.iter().find(|i| i.id == "edit.undo").unwrap();
    assert_eq!((undo.label.as_str(), undo.enabled), ("Can't Undo", false));
    // Open the saved project: its state, no history; arm and pick as usual.
    h.state_mut().session.execute("file.open", json!({"path": path.to_string_lossy()})).unwrap();
    settle(&mut h);
    m.line = vec![("Original".into(), saved)];
    (m.cur, m.branches, m.armed) = (0, 0, None);
    // (Still armed from before New, on the same ids: see `opening_a_project_ends_the_pick`.)
    if let Some(p) = h.state().ui.fx_pick.as_ref() {
        eprintln!("after Open: still armed {p:?}, chip {:?}", chip(&h));
        m.armed = Some((p.layer, p.prop));
    }
    m.branch_states.clear();
    m.time = h.state().session.time_of(effectcraft_engine::project::ItemId(f.comp)).seconds();
    verify(&mut h, &f, &mut m, dims, "after Open");
    if m.armed.is_none() {
        run_step(&mut h, &f, &mut m, dims, &Arm(pl), 20);
    }
    run_step(&mut h, &f, &mut m, dims, &Pick(ctr(6, 2), Modifiers::NONE, "c6_2"), 21);
    step_history(&mut h, &f, &mut m, dims, false, "undo after Open");
    step_history(&mut h, &f, &mut m, dims, false, "undo past Open: nothing");
    step_history(&mut h, &f, &mut m, dims, true, "redo after Open");
    let _ = std::fs::remove_dir_all(&f.dir);
}

// ---------------------------------------------------------------------------------------------
// Scenario 5: grouping and labels.

#[test]
fn picks_never_merge_with_neighbouring_edits() {
    let (s, f) = session("group", 320, 180, false, false);
    let dims = [320.0, 180.0];
    let mut h = harness(s, vec2(1024.0, 700.0));
    let mut m = Model::new(&h, f.comp);
    let ctr = |cx: usize, cy: usize| [cx as f64 * 37.0 + 18.5, cy as f64 * 29.0 + 14.5];
    let pl = f.plate;
    use Step::*;
    run_step(&mut h, &f, &mut m, dims, &Arm(pl), 0);
    // A scrub of the plate's opacity (one merged step), a pick, the same scrub going on: three.
    run_step(&mut h, &f, &mut m, dims, &Opacity(pl, 90.0, Some("scrub-1")), 1);
    let n = h.state().session.history.undo.len();
    h.state_mut()
        .session
        .execute_checked("prop.set", json!({"comp": f.comp, "layer": pl, "path": "transform/opacity", "value": 85.0, "merge": "scrub-1"}))
        .unwrap();
    assert_eq!(h.state().session.history.undo.len(), n, "a scrub merges");
    layer_mut(&mut m.line.last_mut().unwrap().1, pl).opacity = json!(85.0);
    run_step(&mut h, &f, &mut m, dims, &Pick(ctr(1, 1), Modifiers::NONE, "c1_1"), 2);
    run_step(&mut h, &f, &mut m, dims, &Opacity(pl, 80.0, Some("scrub-1")), 3);
    // A pick next to a typed Selection.
    run_step(&mut h, &f, &mut m, dims, &Type(pl, "c2_2"), 4);
    run_step(&mut h, &f, &mut m, dims, &Pick(ctr(3, 3), Modifiers::SHIFT, "c3_3"), 5);
    run_step(&mut h, &f, &mut m, dims, &Type(pl, "c2_2, c3_3, c4_4"), 6);
    // Two picks in a row.
    run_step(&mut h, &f, &mut m, dims, &Pick(ctr(4, 4), Modifiers::ALT, "c4_4"), 7);
    run_step(&mut h, &f, &mut m, dims, &Pick(ctr(5, 5), Modifiers::SHIFT, "c5_5"), 8);
    for k in 0..m.cur {
        step_history(&mut h, &f, &mut m, dims, false, &format!("undo {k}"));
    }
    for (l, w) in &m.labels {
        eprintln!("LABEL {l:?} (first from {w})");
    }
    let _ = std::fs::remove_dir_all(&f.dir);
}

/// FINDING 4. Typing in the Selection field (not committed yet) and then clicking an ID in the
/// viewer: the pick is made first, on the Selection before the typing, and then the field loses
/// the focus and commits the typed text over it. The pick is lost, yet it left an undo step (to a
/// state never shown). Expected: the typed text, then the pick on top, one step each.
#[test]
fn a_click_while_the_selection_field_is_being_edited() {
    let (s, f) = session("typing", 320, 180, false, false);
    let mut h = harness(s, vec2(1400.0, 860.0));
    let pl = f.plate;
    arm(&mut h, &f, pl);
    let a = comp_to_screen(&h.ctx, [37.0 + 18.5, 29.0 + 14.5]).unwrap();
    assert_eq!(hover(&mut h, a), "c1_1");
    let n = h.state().session.history.undo.len();
    type_selection(&mut h, &f, pl, "c2_2", false);
    assert!(h.ctx.memory(|m| m.focused().is_some()), "the field is being edited");
    // Ctrl+Z in the field is the field's own: no global undo.
    key(&mut h, Key::Z, CTRL);
    assert_eq!(h.state().session.history.undo.len(), n, "Ctrl+Z while typing doesn't undo the project");
    type_selection(&mut h, &f, pl, "c2_2", false);
    click(&mut h, a, Modifiers::SHIFT);
    h.run_steps(3);
    let sel = sel_of(&observe(&h.state().session, f.comp), pl);
    let steps: Vec<String> = h.state().session.history.undo[n..].iter().map(|u| u.0.clone()).collect();
    // The state between the two steps (what one Ctrl+Z goes back to).
    let mid = h.state().session.history.undo.last().map(|u| {
        let c = u.1.comp(effectcraft_engine::project::ItemId(f.comp)).unwrap().layer(LayerId(pl)).unwrap();
        c.effects().unwrap().groups().next().unwrap().get("selection").unwrap().value.to_json()
    });
    eprintln!("the state one Ctrl+Z goes back to: Selection {mid:?}");
    eprintln!("typed c2_2 (uncommitted) + Shift-click c1_1: Selection {sel:?}, steps {steps:?}");
    assert_eq!((sel.as_str(), steps.len()), ("c2_2, c1_1", 2), "the typed text, then the pick on top, one step each");
    key(&mut h, Key::Z, CTRL);
    assert_eq!(sel_of(&observe(&h.state().session, f.comp), pl), "c2_2", "undo takes back the pick");
    key(&mut h, Key::Z, CTRL);
    assert_eq!(sel_of(&observe(&h.state().session, f.comp), pl), "", "then the typed text");
    let _ = std::fs::remove_dir_all(&f.dir);
}

/// On an animated Selection a pick sets (or adds) the key at the current time: one step, and
/// undo takes the key back.
#[test]
fn a_pick_on_an_animated_selection_is_one_step() {
    let (s, f) = session("anim", 320, 180, false, false);
    let mut h = harness(s, vec2(1400.0, 860.0));
    let pl = f.plate;
    let uid = arm(&mut h, &f, pl);
    let keys = |h: &Harness<'_, EffectcraftApp>| {
        let s = &h.state().session;
        let l = s.project.comp(effectcraft_engine::project::ItemId(f.comp)).unwrap().layer(LayerId(pl)).unwrap();
        l.props.find(uid).unwrap().keys.iter().map(|k| (k.time.seconds(), k.value.to_json())).collect::<Vec<_>>()
    };
    h.state_mut().session.execute_checked("prop.toggleAnimation", json!({"comp": f.comp, "layer": pl, "prop": uid, "value": true})).unwrap();
    h.state_mut().session.set_time(effectcraft_engine::time::Tick::from_seconds_f64(1.0));
    settle(&mut h);
    let n = h.state().session.history.undo.len();
    let before = keys(&h);
    let a = comp_to_screen(&h.ctx, [37.0 + 18.5, 29.0 + 14.5]).unwrap();
    assert_eq!(hover(&mut h, a), "c1_1");
    click(&mut h, a, Modifiers::NONE);
    assert_eq!(h.state().session.history.undo.len(), n + 1);
    assert_eq!(keys(&h), [before.clone(), vec![(1.0, json!("c1_1"))]].concat(), "a key at 1 s");
    key(&mut h, Key::Z, CTRL);
    assert_eq!(keys(&h), before, "undo takes the key back");
    assert_eq!(hover(&mut h, a), "c1_1");
    key(&mut h, Key::Z, CTRL_SHIFT);
    assert_eq!(hover(&mut h, a), "c1_1 (selected)");
    let _ = std::fs::remove_dir_all(&f.dir);
}

// ---------------------------------------------------------------------------------------------
// Findings: minimal failing tests (fixes proposed in research/crypick-undo-fixes.patch).

/// FINDING 1. Undoing (or redoing) a pick changes only the Selection, which the probe doesn't
/// read, yet the probe is dropped and built again: on a 3D precomp that is ~0.6 s of "Reading
/// Cryptomatte IDs…" after every Ctrl+Z of a pick (the pick itself keeps it).
#[test]
fn undoing_or_redoing_a_pick_keeps_the_probe() {
    let (mut s, f) = session("keep", 320, 180, false, false);
    let q = json!({"comp": f.comp, "layer": f.plate, "effect": 1, "x": 55.5, "y": 43.5, "probeOnly": true});
    s.crypto_probe_job(&q).unwrap().expect("nothing built yet")();
    let pick = json!({"comp": f.comp, "layer": f.plate, "effect": 1, "x": 55.5, "y": 43.5});
    s.execute_checked("effect.pickCryptomatte", pick).unwrap();
    assert!(s.crypto_probe_job(&q).unwrap().is_none(), "the pick keeps the probe");
    s.execute_checked("edit.undo", json!({})).unwrap();
    assert!(s.crypto_probe_job(&q).unwrap().is_none(), "so does its undo");
    assert_eq!(s.execute("effect.cryptomatteAt", q.clone()).unwrap()["selected"], false);
    s.execute_checked("edit.redo", json!({})).unwrap();
    assert!(s.crypto_probe_job(&q).unwrap().is_none(), "and its redo");
    assert_eq!(s.execute("effect.cryptomatteAt", q.clone()).unwrap()["selected"], true);
    // Any other edit still drops it.
    s.execute_checked("prop.set", json!({"comp": f.comp, "layer": f.plate, "path": "transform/opacity", "value": 50})).unwrap();
    s.execute_checked("edit.undo", json!({})).unwrap();
    assert!(s.crypto_probe_job(&q).unwrap().is_some(), "an undone opacity change is a new probe");
    let _ = std::fs::remove_dir_all(&f.dir);
}

/// FINDING 2. The eyedropper stays armed through New Project, Open and Revert: the pick is kept
/// by layer id and property uid only, and a new project reuses them. Rebuilt the same way, the
/// new project's Cryptomatte is picked into by a plain viewer click the user never armed.
#[test]
fn a_new_or_opened_project_ends_the_pick() {
    let (s, f) = session("stale", 320, 180, false, false);
    let mut h = harness(s, vec2(1400.0, 860.0));
    let path = f.dir.join("plate.exr").to_string_lossy().to_string();
    let saved = f.dir.join("saved.ecproj").to_string_lossy().to_string();
    h.state_mut().session.execute("file.save", json!({"path": saved})).unwrap();
    // Open (the same file, as Revert does).
    arm(&mut h, &f, f.plate);
    h.state_mut().session.execute("file.open", json!({"path": saved})).unwrap();
    settle(&mut h);
    assert!(h.state().ui.fx_pick.is_none(), "Open ends the pick: {:?}", h.state().ui.fx_pick);
    // New Project, then the same setup built in it.
    h.state_mut().ui.fx_pick = None;
    h.run_steps(2);
    arm(&mut h, &f, f.plate);
    h.state_mut().session.execute("file.newProject", json!({})).unwrap();
    h.run_steps(3);
    let g = populate(&mut h.state_mut().session, &path, 320, 180, false, false, f.dir.clone());
    settle(&mut h);
    let stale = h.state().ui.fx_pick.clone();
    let a = comp_to_screen(&h.ctx, [55.5, 43.5]).unwrap();
    click(&mut h, a, Modifiers::NONE);
    let sel = sel_of(&observe(&h.state().session, g.comp), g.plate);
    assert!(stale.is_none() && sel.is_empty(), "New ends the pick: armed {stale:?}; a plain click picked {sel:?} into the new project");
    let _ = std::fs::remove_dir_all(&f.dir);
}

/// FINDING 3 (wording). A pick's undo step is "Change Property", the same as an opacity change
/// or a typed Selection: Edit ▸ Undo and the History panel can't tell them apart. The command
/// is "Pick Cryptomatte"; its step could say so.
#[test]
fn a_pick_is_labelled_as_a_pick() {
    let (s, f) = session("label", 320, 180, false, false);
    let mut h = harness(s, vec2(1400.0, 860.0));
    arm(&mut h, &f, f.plate);
    let a = comp_to_screen(&h.ctx, [55.5, 43.5]).unwrap();
    click(&mut h, a, Modifiers::NONE);
    let items = effectcraft_ui_egui::menus::menu_items(h.state());
    let undo = items.iter().find(|i| i.id == "edit.undo").unwrap();
    assert_eq!(undo.label, "Undo Pick Cryptomatte");
    let _ = std::fs::remove_dir_all(&f.dir);
}

/// FINDING 5 (from review). A middle-button drag pans the viewer, armed or not: egui hands the
/// press to the eyedropper's overlay (it sensed drags the pass before), so armed it didn't pan.
#[test]
fn a_middle_drag_pans_while_armed() {
    let (s, f) = session("pan", 320, 180, false, false);
    let mut h = harness(s, vec2(1400.0, 860.0));
    let a = comp_to_screen(&h.ctx, [160.0, 90.0]).unwrap();
    let drag = |h: &mut Harness<'_, EffectcraftApp>| {
        let before = h.state().ui.viewer.pan;
        h.event(Event::PointerMoved(a));
        h.step();
        h.event(Event::PointerButton { pos: a, button: PointerButton::Middle, pressed: true, modifiers: Modifiers::NONE });
        h.step();
        for k in 1..=5 {
            h.event(Event::PointerMoved(a + vec2(10.0 * k as f32, 0.0)));
            h.step();
        }
        h.event(Event::PointerButton { pos: a + vec2(50.0, 0.0), button: PointerButton::Middle, pressed: false, modifiers: Modifiers::NONE });
        h.run_steps(2);
        let after = h.state().ui.viewer.pan;
        h.state_mut().ui.viewer.pan = before;
        h.run_steps(2);
        after[0] - before[0]
    };
    let unarmed = drag(&mut h);
    let n = h.state().session.history.undo.len();
    arm(&mut h, &f, f.plate);
    let armed = drag(&mut h);
    assert_eq!(h.state().session.history.undo.len(), n, "no pick");
    assert!(h.state().ui.fx_pick.is_some(), "still armed");
    assert!((unarmed - 50.0).abs() < 0.01 && (armed - 50.0).abs() < 0.01, "a 50 px middle drag pans 50 px: unarmed {unarmed}, armed {armed}");
    let _ = std::fs::remove_dir_all(&f.dir);
}
