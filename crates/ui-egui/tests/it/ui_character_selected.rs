//! Character and Paragraph changes reach every selected text layer (#2862).
use effectcraft_engine::{
    Session,
    commands::text_edit::layer_doc,
    keyframe::{Justify, TextDoc},
    project::LayerId,
};
use effectcraft_ui_egui::{EffectcraftApp, dock::PanelKind};
use egui::{Event, Modifiers, PointerButton, Pos2, accesskit::Role, pos2, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;

fn center(h: &Harness<'_, EffectcraftApp>, id: &str) -> Pos2 {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("missing {id}"));
    pos2(e.rect[0] + e.rect[2] * 0.5, e.rect[1] + e.rect[3] * 0.5)
}

fn click(h: &mut Harness<'_, EffectcraftApp>, pos: Pos2) {
    h.event(Event::PointerMoved(pos));
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
}

fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) {
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for k in 1..=4 {
        h.event(Event::PointerMoved(from + (to - from) * (k as f32 / 4.0)));
        h.step();
    }
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

fn doc(h: &Harness<'_, EffectcraftApp>, layer: u64) -> TextDoc {
    layer_doc(&h.state().session, LayerId(layer)).unwrap()
}

/// A Character panel harness with two text layers, both selected.
fn two_text_layers(edit: bool) -> (Harness<'static, EffectcraftApp>, u64, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 320, "height": 180, "duration": 2})).unwrap();
    let a = s.execute("layer.newText", json!({"text": "One", "size": 60, "justify": "left"})).unwrap()["layer"].as_u64().unwrap();
    let b = s.execute("layer.newText", json!({"text": "Two", "size": 60, "justify": "left"})).unwrap()["layer"].as_u64().unwrap();
    if edit {
        s.execute("text.edit", json!({"layer": a})).unwrap();
    }
    s.state.selected_layers = vec![LayerId(a), LayerId(b)];
    let mut app = EffectcraftApp::new(s);
    app.set_workspace("Text");
    let mut h = Harness::builder().with_size(vec2(1600.0, 1400.0)).build_eframe(|_| app);
    h.state_mut().show_panel(PanelKind::Character);
    h.run_steps(4);
    (h, a, b)
}

#[test]
fn character_and_paragraph_changes_reach_every_selected_text_layer() {
    let (mut h, a, b) = two_text_layers(false);

    // Scrub the font size: both layers follow, in one undo step.
    let steps = h.state().session.history.undo.len();
    let from = center(&h, "character.size");
    drag(&mut h, from, from + vec2(40.0, 0.0));
    let size = doc(&h, a).size;
    assert!(size > 60.0, "the scrub changed the size");
    assert_eq!(doc(&h, b).size, size);
    assert_eq!(h.state().session.history.undo.len(), steps + 1);
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!((doc(&h, a).size, doc(&h, b).size), (60.0, 60.0));
    h.run_steps(2);

    // A Character toggle.
    let bold = center(&h, "character.fauxBold");
    click(&mut h, bold);
    assert!(doc(&h, a).faux_bold && doc(&h, b).faux_bold);

    // The stroke order menu's layer-wide orders set More Options > Fill & Stroke on both.
    let order = center(&h, "character.strokeOverFill");
    click(&mut h, order);
    h.get_by_role_and_label(Role::Button, "All Fills Over All Strokes").click();
    h.run_steps(3);
    for l in [a, b] {
        assert_eq!(h.state_mut().session.execute("prop.get", json!({"layer": l, "path": "text/moreOptions/fillStroke"})).unwrap()["value"], json!(1));
    }

    // The Paragraph panel.
    h.state_mut().show_panel(PanelKind::Paragraph);
    h.run_steps(4);
    let right = center(&h, "paragraph.right");
    click(&mut h, right);
    assert_eq!((doc(&h, a).justify, doc(&h, b).justify), (Justify::Right, Justify::Right));
}

/// A locked selected layer stays as it is: the stroke order's layer-wide Fill & Stroke goes to the
/// other selected text layers, as every other Character change does.
#[test]
fn a_layer_wide_stroke_order_leaves_a_locked_selected_layer_alone() {
    let (mut h, a, b) = two_text_layers(false);
    h.state_mut().session.execute("layer.setSwitch", json!({"layers": [a], "switch": "lock", "value": true})).unwrap();
    h.run_steps(2);
    let order = center(&h, "character.strokeOverFill");
    click(&mut h, order);
    h.get_by_role_and_label(Role::Button, "All Fills Over All Strokes").click();
    h.run_steps(3);
    let fill_stroke = |h: &mut Harness<'_, EffectcraftApp>, l: u64| {
        h.state_mut().session.execute("prop.get", json!({"layer": l, "path": "text/moreOptions/fillStroke"})).unwrap()["value"].clone()
    };
    assert_eq!(fill_stroke(&mut h, b), json!(1));
    assert_eq!(fill_stroke(&mut h, a), json!(0), "the locked layer keeps its order");
}

/// The panels show the first layer their changes reach, not a locked one above it: a toggle sets
/// what it looks like it sets, and a scrub starts from the value it changes.
#[test]
fn the_panels_show_the_first_unlocked_selected_layer() {
    let (mut h, a, b) = two_text_layers(false);
    let s = &mut h.state_mut().session;
    s.execute("layer.setSwitch", json!({"layers": [a], "switch": "lock", "value": true})).unwrap();
    s.execute("layer.setText", json!({"layer": b, "fauxBold": true, "size": 90})).unwrap();
    h.run_steps(2);
    let bold = center(&h, "character.fauxBold");
    click(&mut h, bold);
    assert!(!doc(&h, b).faux_bold, "the lit toggle turns Faux Bold off");
    assert!(!doc(&h, a).faux_bold);

    h.state_mut().show_panel(PanelKind::Properties);
    h.run_steps(4);
    let from = center(&h, "properties.text.size");
    drag(&mut h, from, from + vec2(40.0, 0.0));
    assert!(doc(&h, b).size > 90.0, "the scrub starts from the unlocked layer's size");
    assert_eq!(doc(&h, a).size, 60.0, "the locked layer keeps its size");
}

/// A lone locked text layer stays as it is, and no error shows: nothing qualifies, so the panel's
/// changes, the stroke order's `prop.set` among them, do nothing.
#[test]
fn character_changes_leave_a_lone_locked_layer_alone_quietly() {
    let (mut h, a, b) = two_text_layers(false);
    h.state_mut().session.execute("layer.select", json!({"layers": [a]})).unwrap();
    h.state_mut().session.execute("layer.setSwitch", json!({"layers": [a], "switch": "lock", "value": true})).unwrap();
    h.run_steps(2);
    let steps = h.state().session.history.undo.len();
    let bold = center(&h, "character.fauxBold");
    click(&mut h, bold);
    let order = center(&h, "character.strokeOverFill");
    click(&mut h, order);
    h.get_by_role_and_label(Role::Button, "All Fills Over All Strokes").click();
    h.run_steps(3);
    assert!(!doc(&h, a).faux_bold && !doc(&h, b).faux_bold);
    for l in [a, b] {
        assert_eq!(h.state_mut().session.execute("prop.get", json!({"layer": l, "path": "text/moreOptions/fillStroke"})).unwrap()["value"], json!(0));
    }
    assert_eq!(h.state().session.history.undo.len(), steps, "no undo step");
    assert!(h.state().ui.status.is_empty(), "{}", h.state().ui.status);
}

/// A per-character stroke order puts every selected text layer back on the Per Character Palette,
/// though the layer the panel shows already is on it.
#[test]
fn a_per_character_stroke_order_resets_the_other_selected_layers() {
    let (mut h, a, b) = two_text_layers(false);
    h.state_mut().session.execute("prop.set", json!({"layer": b, "path": "text/moreOptions/fillStroke", "value": 2})).unwrap();
    h.run_steps(2);
    let steps = h.state().session.history.undo.len();
    let order = center(&h, "character.strokeOverFill");
    click(&mut h, order);
    h.get_by_role_and_label(Role::Button, "Stroke Over Fill").click();
    h.run_steps(3);
    for l in [a, b] {
        assert_eq!(h.state_mut().session.execute("prop.get", json!({"layer": l, "path": "text/moreOptions/fillStroke"})).unwrap()["value"], json!(0));
        assert!(doc(&h, l).stroke_over_fill);
    }
    assert_eq!(h.state().session.history.undo.len(), steps + 1, "one undo step");
}

/// While one layer's text is edited, the panel changes its selected characters only.
#[test]
fn character_changes_stay_on_the_edited_layer_while_editing() {
    let (mut h, a, b) = two_text_layers(true);
    assert!(h.state().session.state.text_edit.is_some());
    let bold = center(&h, "character.fauxBold");
    click(&mut h, bold);
    assert!(doc(&h, a).style_at(0).faux_bold);
    assert!(!doc(&h, b).style_at(0).faux_bold);
}

/// While a layer below the first selected one is edited, Properties' text rows change that
/// layer's selected characters, as the Character panel does, not the first layer's whole text.
#[test]
fn properties_text_changes_reach_the_edited_layer_below_the_first_selected() {
    let (mut h, a, b) = two_text_layers(false);
    let s = &mut h.state_mut().session;
    s.execute("text.edit", json!({"layer": b, "select": [0, 1]})).unwrap();
    s.execute("layer.select", json!({"layers": [a, b]})).unwrap();
    h.state_mut().show_panel(PanelKind::Properties);
    h.run_steps(4);
    assert!(h.state().session.state.text_edit.as_ref().is_some_and(|e| e.layer == LayerId(b)));
    let from = center(&h, "properties.text.size");
    drag(&mut h, from, from + vec2(40.0, 0.0));
    assert!(doc(&h, b).style_at(0).size > 60.0, "the edited character changes");
    assert_eq!(doc(&h, b).style_at(1).size, 60.0, "the rest of the edited layer stays");
    assert_eq!(doc(&h, a).size, 60.0, "the first selected layer stays");
}
