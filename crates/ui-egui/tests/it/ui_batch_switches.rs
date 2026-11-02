//! A Timeline row control clicked on one of several selected layers applies to the whole
//! selection (#1849), as in After Effects: the A/V Features and Switches columns, the label
//! swatch and the Modes column. The engine commands already took several layers; the row
//! controls used to pass the clicked layer alone.

use effectcraft_engine::Session;
use effectcraft_engine::color::{BlendMode, Label};
use effectcraft_engine::project::{Layer, LayerId, Switches};
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Modifiers, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

/// Comp "Main" with solids A (top) to D (bottom); returns their ids top to bottom.
fn harness() -> (Harness<'static, EffectcraftApp>, Vec<u64>) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "frameRate": 30, "duration": 4})).unwrap();
    let mut ids: Vec<u64> = ["D", "C", "B", "A"]
        .iter()
        .map(|name| s.execute("layer.newSolid", json!({"name": name, "color": "#406080"})).unwrap()["layer"].as_u64().unwrap())
        .collect();
    ids.reverse();
    s.execute("edit.deselectAll", json!({})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    (h, ids)
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}")).clone();
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

/// Press on `from`, move to `to` in `steps`, release there.
fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2, steps: u32) {
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for k in 1..=steps {
        h.event(Event::PointerMoved(from + (to - from) * (k as f32 / steps as f32)));
        h.step();
    }
    h.event(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

/// A click on the Timeline control `id`.
fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let p = rect(h, id).center();
    drag(h, p, p, 1);
}

/// A click at `p` (a menu entry, found by its label).
fn click_at(h: &mut Harness<'_, EffectcraftApp>, p: Pos2) {
    drag(h, p, p, 1);
}

fn alt_click(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let p = rect(h, id).center();
    let modifiers = Modifiers::ALT;
    h.event(Event::PointerMoved(p));
    h.event(Event::ModifiersChanged(modifiers));
    h.step();
    h.event(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers });
    h.step();
    h.event(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers });
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(3);
}

/// Select `ids` in the Timeline, as a Shift range or a Cmd toggle would.
fn select(h: &mut Harness<'_, EffectcraftApp>, ids: &[u64]) {
    h.state_mut().session.execute("layer.select", json!({"layers": ids})).unwrap();
    h.run_steps(2);
}

/// The state of switch `f` on each layer of `ids`, top to bottom.
fn states(h: &Harness<'_, EffectcraftApp>, ids: &[u64], f: fn(&Layer) -> bool) -> Vec<bool> {
    let comp = h.state().session.active_comp().unwrap();
    ids.iter().map(|id| f(comp.layer(LayerId(*id)).unwrap())).collect()
}

fn switches(h: &Harness<'_, EffectcraftApp>, id: u64) -> Switches {
    h.state().session.active_comp().unwrap().layer(LayerId(id)).unwrap().switches.clone()
}

fn locked(l: &Layer) -> bool {
    l.switches.locked
}

fn video(l: &Layer) -> bool {
    l.switches.video
}

fn three_d(l: &Layer) -> bool {
    l.switches.three_d
}

fn solo(l: &Layer) -> bool {
    l.switches.solo
}

fn shy(l: &Layer) -> bool {
    l.switches.shy
}

fn motion_blur(l: &Layer) -> bool {
    l.switches.motion_blur
}

fn preserved(l: &Layer) -> bool {
    l.preserve_transparency
}

/// #1849: a click on the Lock, 3D, Solo, Shy or Motion Blur switch of a selected layer gives the
/// switch to every selected layer, in one undo step.
#[test]
fn switches_on_a_selected_row_set_the_whole_selection() {
    let (mut h, ids) = harness();
    select(&mut h, &[ids[0], ids[2]]);

    let steps = h.state().session.history.undo.len();
    click(&mut h, &format!("timeline.layer.{}.lock", ids[0]));
    assert_eq!(states(&h, &ids, locked), [true, false, true, false]);
    assert_eq!(h.state().session.history.undo.len(), steps + 1, "one undo step");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    h.run_steps(2);
    assert_eq!(states(&h, &ids, locked), [false; 4]);

    click(&mut h, &format!("timeline.layer.{}.switch.threeD", ids[0]));
    assert_eq!(states(&h, &ids, three_d), [true, false, true, false]);
    // The switch of the other selected layer acts the same way.
    click(&mut h, &format!("timeline.layer.{}.solo", ids[2]));
    assert_eq!(states(&h, &ids, solo), [true, false, true, false]);
    click(&mut h, &format!("timeline.layer.{}.switch.shy", ids[2]));
    assert_eq!(states(&h, &ids, shy), [true, false, true, false]);
    click(&mut h, &format!("timeline.layer.{}.switch.motionBlur", ids[0]));
    assert_eq!(states(&h, &ids, motion_blur), [true, false, true, false]);
    // A second click turns both off again, whatever their own state.
    click(&mut h, &format!("timeline.layer.{}.switch.threeD", ids[2]));
    assert_eq!(states(&h, &ids, three_d), [false; 4]);
}

/// A switch on a row that isn't selected still changes that layer alone, and leaves the
/// selection.
#[test]
fn a_switch_on_an_unselected_row_sets_that_layer_only() {
    let (mut h, ids) = harness();
    select(&mut h, &[ids[0], ids[2]]);
    click(&mut h, &format!("timeline.layer.{}.lock", ids[1]));
    assert_eq!(states(&h, &ids, locked), [false, true, false, false]);
    assert_eq!(h.state().session.state.selected_layers, vec![LayerId(ids[0]), LayerId(ids[2])]);
}

/// The targets skip the selected layers a switch doesn't apply to: 3D reaches a null but not a
/// camera (3D by kind), the Effect switch not a layer without effects.
#[test]
fn switch_targets_skip_the_layers_the_switch_does_not_apply_to() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "frameRate": 30, "duration": 4})).unwrap();
    let solid = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
    let null = s.execute("layer.newNull", json!({"name": "Ctrl"})).unwrap()["layer"].as_u64().unwrap();
    let cam = s.execute("layer.newCamera", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("effect.apply", json!({"effect": "ec.blur.gaussian", "layers": [solid]})).unwrap();
    s.execute("edit.deselectAll", json!({})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    select(&mut h, &[solid, null, cam]);

    click(&mut h, &format!("timeline.layer.{solid}.switch.threeD"));
    assert!(switches(&h, solid).three_d && switches(&h, null).three_d, "3D reaches the null");
    assert!(!switches(&h, cam).three_d, "a camera is 3D by kind, its own switch stays off");

    click(&mut h, &format!("timeline.layer.{solid}.switch.fx"));
    assert!(!switches(&h, solid).effects, "the Effect switch reaches the layer that has an effect");
    assert!(switches(&h, null).effects && switches(&h, cam).effects, "no effects, no Effect switch");
}

/// Alt-click on an A/V feature sets the selected layers apart (the others off), and the next
/// Alt-click brings the others back — with several layers, the whole selection stands apart.
#[test]
fn alt_click_sets_the_selected_layers_apart() {
    let (mut h, ids) = harness();
    select(&mut h, &[ids[0], ids[2]]);
    alt_click(&mut h, &format!("timeline.layer.{}.video", ids[0]));
    assert_eq!(states(&h, &ids, video), [true, false, true, false]);
    alt_click(&mut h, &format!("timeline.layer.{}.video", ids[0]));
    assert_eq!(states(&h, &ids, video), [true; 4]);
}

/// The label swatch menu of a selected layer sets the label of every selected layer.
#[test]
fn the_label_menu_sets_the_selections_label() {
    let (mut h, ids) = harness();
    select(&mut h, &[ids[0], ids[2]]);
    let green = h.state().session.prefs.label_name(Label::Green);
    click(&mut h, &format!("timeline.layer.{}.label", ids[0]));
    let entry = h.query_by_label(&green).expect("the label menu is open").rect();
    click_at(&mut h, entry.center());
    let label = |id: u64| h.state().session.active_comp().unwrap().layer(LayerId(id)).unwrap().label;
    assert_eq!([label(ids[0]), label(ids[1]), label(ids[2]), label(ids[3])], [Label::Green, Label::Red, Label::Green, Label::Red]);
}

/// The Modes column: the Blending Mode menu and the Preserve Transparency checkbox of a selected
/// layer set both selected layers, one undo step each.
#[test]
fn the_modes_column_sets_the_whole_selection() {
    let (mut h, ids) = harness();
    h.state_mut().ui.timeline.show_modes = true;
    h.run_steps(3);
    select(&mut h, &[ids[0], ids[2]]);

    click(&mut h, &format!("timeline.layer.{}.mode", ids[0]));
    let entry = h.query_by_label("Multiply").expect("the blend mode menu is open").rect();
    click_at(&mut h, entry.center());
    let mode = |id: u64| h.state().session.active_comp().unwrap().layer(LayerId(id)).unwrap().blend_mode;
    assert_eq!([mode(ids[0]), mode(ids[1]), mode(ids[2]), mode(ids[3])], [BlendMode::Multiply, BlendMode::Normal, BlendMode::Multiply, BlendMode::Normal]);

    click(&mut h, &format!("timeline.layer.{}.preserveTransparency", ids[0]));
    assert_eq!(states(&h, &ids, preserved), [true, false, true, false]);

    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    h.run_steps(2);
    assert_eq!(states(&h, &ids, preserved), [false; 4], "one undo step put both back");
}

/// Preserve Transparency toggles from the row it is clicked on, not from the first selected layer:
/// A on and C off, clicking C's box turns both on.
#[test]
fn preserve_transparency_toggles_from_the_clicked_row() {
    let (mut h, ids) = harness();
    h.state_mut().ui.timeline.show_modes = true;
    h.run_steps(3);
    h.state_mut().session.execute("layer.setSwitch", json!({"layers": [ids[0]], "switch": "preserveTransparency", "value": true})).unwrap();
    h.run_steps(2);
    select(&mut h, &[ids[0], ids[2]]);
    click(&mut h, &format!("timeline.layer.{}.preserveTransparency", ids[2]));
    assert_eq!(states(&h, &ids, preserved), [true, false, true, false]);
    // The next click turns both off again.
    click(&mut h, &format!("timeline.layer.{}.preserveTransparency", ids[2]));
    assert_eq!(states(&h, &ids, preserved), [false; 4]);
}
