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

/// A locked layer that the panel shows stays as it is: the stroke order's layer-wide Fill & Stroke
/// goes to the other selected text layers, as every other Character change does.
#[test]
fn a_layer_wide_stroke_order_leaves_a_locked_shown_layer_alone() {
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
