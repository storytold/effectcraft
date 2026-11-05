//! Inline text fields (a name, the current time) open with their whole text selected so typing
//! replaces it, commit on Enter and cancel on Escape (#170).

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

fn harness() -> (Harness<'static, EffectcraftApp>, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4, "fps": 24})).unwrap();
    let comp = s.active_comp_id().unwrap().0;
    // Arabic, Hebrew, Devanagari and Thai names typed below need installed system faces: draw
    // tofu on a host without them rather than failing the missing-glyph gate.
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).allow_missing_glyphs().build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(4);
    (h, comp)
}

fn center(h: &Harness<'_, EffectcraftApp>, id: &str) -> Pos2 {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0)
}

fn click(h: &mut Harness<'_, EffectcraftApp>, p: Pos2, times: usize) {
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    // All in one frame (the harness' frames are longer apart than a double-click).
    for _ in 0..times {
        h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    }
    h.run_steps(3);
}

/// A primary-button drag from `from` to `to`, in steps.
fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for k in 1..=8 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (k as f32 / 8.0)));
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn key(h: &mut Harness<'_, EffectcraftApp>, key: Key) {
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

fn type_text(h: &mut Harness<'_, EffectcraftApp>, text: &str) {
    assert!(h.ctx.egui_wants_keyboard_input(), "the field has the keyboard");
    h.input_mut().events.push(Event::Text(text.into()));
    h.step();
}

#[test]
fn project_rename_replaces_the_name() {
    let (mut h, comp) = harness();
    let name = |h: &Harness<'_, EffectcraftApp>| h.state().session.project.items.values().find(|i| i.id.0 == comp).unwrap().name.clone();
    let p = center(&h, &format!("project.item.{comp}.name"));
    click(&mut h, p, 1);
    key(&mut h, Key::Enter);
    type_text(&mut h, "Hero");
    key(&mut h, Key::Enter);
    assert_eq!(name(&h), "Hero");
    assert!(!h.ctx.egui_wants_keyboard_input(), "Enter releases the keyboard");
}

/// #881: Tab in the Project panel's rename field commits the name like Enter and gives the keyboard
/// back to the panel; it doesn't leave keyboard focus on another widget (the row's label swatch),
/// where Enter would no longer rename and the arrow keys would move between widgets.
#[test]
fn tab_in_the_project_rename_field_commits_and_keeps_enter_renaming() {
    let (mut h, comp) = harness();
    let name = |h: &Harness<'_, EffectcraftApp>| h.state().session.project.items.values().find(|i| i.id.0 == comp).unwrap().name.clone();
    let edit = format!("project.item.{comp}.nameEdit");
    let editing = |h: &Harness<'_, EffectcraftApp>| h.state().auto.elements.iter().any(|e| e.id == edit);
    let p = center(&h, &format!("project.item.{comp}.name"));
    click(&mut h, p, 1);
    key(&mut h, Key::Enter);
    type_text(&mut h, "Hero");
    key(&mut h, Key::Tab);
    assert_eq!(name(&h), "Hero", "Tab commits the name");
    assert!(!editing(&h), "Tab closes the rename field");
    assert_eq!(h.ctx.memory(|m| m.focused()), None, "no other widget takes the keyboard");
    key(&mut h, Key::Enter);
    assert!(editing(&h), "Enter renames again");
    type_text(&mut h, "Main");
    key(&mut h, Key::Enter);
    assert_eq!(name(&h), "Main");
}

#[test]
fn timecode_field_replaces_commits_and_cancels() {
    let (mut h, _) = harness();
    let frame = |h: &Harness<'_, EffectcraftApp>| h.state().session.active_comp().unwrap().frame_rate.frame_at(h.state().session.time());
    let p = center(&h, "timeline.timecode");
    click(&mut h, p, 2);
    type_text(&mut h, "0:00:01:00");
    key(&mut h, Key::Enter);
    assert_eq!(frame(&h), 24, "Enter commits the typed time");
    assert!(!h.ctx.egui_wants_keyboard_input(), "Enter releases the keyboard");
    click(&mut h, p, 2);
    type_text(&mut h, "0:00:02:00");
    key(&mut h, Key::Escape);
    assert_eq!(frame(&h), 24, "Escape cancels");
    assert!(!h.ctx.egui_wants_keyboard_input(), "Escape releases the keyboard");
}

/// #284: dragging the expression pick whip while the expression is being edited puts the
/// reference at the cursor (After Effects), and the editing goes on after it; without an edit
/// in progress it replaces the expression.
#[test]
fn the_expression_pick_whip_inserts_at_the_cursor_while_editing() {
    let (mut h, _) = harness();
    let (layer, opacity, rotation, transform) = {
        let s = &mut h.state_mut().session;
        let l = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
        s.execute("prop.setExpression", json!({"layer": l, "path": "transform/opacity", "expression": "50 + 0"})).unwrap();
        let tr = s.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(l)).unwrap().props.sub("transform").unwrap().clone();
        (l, tr.get("opacity").unwrap().uid, tr.get("rotation").unwrap().uid, tr.uid)
    };
    h.state_mut().ui.timeline.open_layers.insert(layer);
    h.state_mut().ui.timeline.open_groups.insert(transform);
    h.run_steps(3);
    let expr = |h: &Harness<'_, EffectcraftApp>| {
        let comp = h.state().session.active_comp().unwrap();
        comp.layer(effectcraft_engine::project::LayerId(layer)).unwrap().props.find(opacity).unwrap().expr.as_ref().unwrap().text.clone()
    };
    let whip = |h: &mut Harness<'_, EffectcraftApp>| {
        let (from, to) = (center(h, &format!("timeline.prop.{opacity}.pickWhip")), center(h, &format!("timeline.prop.{rotation}.name")));
        drag(h, from, to);
    };
    // Editing: the cursor at the end after typing " * ".
    let p = center(&h, &format!("timeline.prop.{opacity}.expression"));
    click(&mut h, p, 1);
    key(&mut h, Key::End);
    type_text(&mut h, " * ");
    whip(&mut h);
    assert_eq!(expr(&h), "50 + 0 * transform.rotation");
    // The editor has the keyboard again, its cursor after the reference.
    type_text(&mut h, " + 1");
    // Ctrl/Cmd+Enter commits.
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::COMMAND));
    h.input_mut().events.push(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND });
    h.step();
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(3);
    assert_eq!(expr(&h), "50 + 0 * transform.rotation + 1");
    // Not editing: the reference replaces the expression.
    whip(&mut h);
    assert_eq!(expr(&h), "transform.rotation");
}

/// #363: dropped on one of a property's values the expression pick whip picks that dimension
/// (`position[1]`), as in After Effects; on its name, the whole property.
#[test]
fn the_expression_pick_whip_picks_the_dimension_it_is_dropped_on() {
    let (mut h, _) = harness();
    let (layer, opacity, position, transform) = {
        let s = &mut h.state_mut().session;
        let l = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
        s.execute("prop.setExpression", json!({"layer": l, "path": "transform/opacity", "expression": "50"})).unwrap();
        let tr = s.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(l)).unwrap().props.sub("transform").unwrap().clone();
        (l, tr.get("opacity").unwrap().uid, tr.get("position").unwrap().uid, tr.uid)
    };
    h.state_mut().ui.timeline.open_layers.insert(layer);
    h.state_mut().ui.timeline.open_groups.insert(transform);
    h.run_steps(3);
    let expr = |h: &Harness<'_, EffectcraftApp>| {
        let comp = h.state().session.active_comp().unwrap();
        comp.layer(effectcraft_engine::project::LayerId(layer)).unwrap().props.find(opacity).unwrap().expr.as_ref().unwrap().text.clone()
    };
    let whip_to = |h: &mut Harness<'_, EffectcraftApp>, target: &str| {
        let (from, to) = (center(h, &format!("timeline.prop.{opacity}.pickWhip")), center(h, target));
        drag(h, from, to);
    };
    whip_to(&mut h, &format!("timeline.prop.{position}.value.1"));
    assert_eq!(expr(&h), "transform.position[1]");
    whip_to(&mut h, &format!("timeline.prop.{position}.name"));
    assert_eq!(expr(&h), "transform.position[0]", "the whole Position into one value: its first, as before");
}

/// #363: every property has a pick whip in the Parent & Link column (After Effects CC 2018 and
/// later). Dropped on another property's name or one of its values, in the Timeline or in Effect
/// Controls, it gives the property an expression referencing it; one it already has is replaced
/// by the reference, and a disabled one is enabled.
#[test]
fn the_property_pick_whip_links_a_property_to_another() {
    let (mut h, _) = harness();
    let (layer, opacity, rotation, position, transform) = {
        let s = &mut h.state_mut().session;
        let l = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
        s.execute("layer.select", json!({"layers": [l]})).unwrap();
        for fx in ["Slider Control", "Point Control"] {
            s.execute("effect.apply", json!({"layers": [l], "effect": fx})).unwrap();
        }
        let tr = s.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(l)).unwrap().props.sub("transform").unwrap().clone();
        (l, tr.get("opacity").unwrap().uid, tr.get("rotation").unwrap().uid, tr.get("position").unwrap().uid, tr.uid)
    };
    h.state_mut().ui.timeline.open_layers.insert(layer);
    h.state_mut().ui.timeline.open_groups.insert(transform);
    h.state_mut().show_panel(effectcraft_ui_egui::dock::PanelKind::EffectControls);
    h.run_steps(3);
    let props =
        |h: &Harness<'_, EffectcraftApp>| h.state().session.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(layer)).unwrap().props.clone();
    let expr = |h: &Harness<'_, EffectcraftApp>| props(h).find(opacity).unwrap().expr.clone().map(|e| (e.text, e.enabled));
    let whip_to = |h: &mut Harness<'_, EffectcraftApp>, target: &str| {
        let (from, to) = (center(h, &format!("timeline.prop.{opacity}.propertyPickWhip")), center(h, target));
        drag(h, from, to);
    };
    assert_eq!(expr(&h), None);
    // Onto another property's name: no expression yet, one is added.
    whip_to(&mut h, &format!("timeline.prop.{rotation}.name"));
    assert_eq!(expr(&h), Some(("transform.rotation".into(), true)));
    // Onto one of a property's values: that dimension.
    whip_to(&mut h, &format!("timeline.prop.{position}.value.1"));
    assert_eq!(expr(&h), Some(("transform.position[1]".into(), true)));
    // A disabled expression is replaced by the reference, and enabled.
    h.state_mut().session.execute("prop.setExpression", json!({"layer": layer, "prop": opacity, "expression": "50 + 1", "enabled": false})).unwrap();
    h.run_steps(3);
    whip_to(&mut h, &format!("timeline.prop.{rotation}.name"));
    assert_eq!(expr(&h), Some(("transform.rotation".into(), true)));
    // Onto an effect's property in Effect Controls: its value, then one of a point's values.
    let (slider, point) = (props(&h).prop("effects/#1/slider").unwrap().uid, props(&h).prop("effects/#2/point").unwrap().uid);
    whip_to(&mut h, &format!("effectControls.prop.{slider}.value"));
    assert_eq!(expr(&h), Some(("effect(\"Slider Control\")(\"Slider\")".into(), true)));
    whip_to(&mut h, &format!("effectControls.prop.{point}.value.1"));
    assert_eq!(expr(&h), Some(("effect(\"Point Control\")(\"Point\")[1]".into(), true)));
    // Onto itself: nothing changes.
    whip_to(&mut h, &format!("timeline.prop.{opacity}.name"));
    assert_eq!(expr(&h), Some(("effect(\"Point Control\")(\"Point\")[1]".into(), true)));
}

/// #362: an expression pick whip held near the bottom or top of the Timeline's layers scrolls
/// them (After Effects), so a property out of view can be picked.
#[test]
fn the_pick_whip_scrolls_the_timeline_at_its_edges() {
    let (mut h, _) = harness();
    let (top, opacity, transform) = {
        let s = &mut h.state_mut().session;
        for i in 0..40 {
            s.execute("layer.newSolid", json!({"name": format!("Layer {i}"), "color": "#406080"})).unwrap();
        }
        let l = s.execute("layer.newSolid", json!({"name": "Top", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
        s.execute("prop.setExpression", json!({"layer": l, "path": "transform/opacity", "expression": "50"})).unwrap();
        let tr = s.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(l)).unwrap().props.sub("transform").unwrap().clone();
        (l, tr.get("opacity").unwrap().uid, tr.uid)
    };
    h.state_mut().ui.timeline.open_layers.insert(top);
    h.state_mut().ui.timeline.open_groups.insert(transform);
    h.run_steps(3);
    let scroll = |h: &Harness<'_, EffectcraftApp>| h.state().ui.timeline.scroll_y;
    let rows = h.state().auto.find("timeline.scroll").unwrap().rect;
    let from = center(&h, &format!("timeline.prop.{opacity}.pickWhip"));
    let (bottom, upper) = (pos2(from.x, rows[1] + rows[3] - 2.0), pos2(from.x, rows[1] + 2.0));
    h.input_mut().events.push(Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for k in 1..=8 {
        h.input_mut().events.push(Event::PointerMoved(from + (bottom - from) * (k as f32 / 8.0)));
        h.step();
    }
    h.run_steps(20);
    let down = scroll(&h);
    assert!(down > 50.0, "held at the bottom edge the layers scroll up: {down}");
    h.input_mut().events.push(Event::PointerMoved(upper));
    h.run_steps(10);
    assert!(scroll(&h) < down, "and at the top edge back down: {} after {down}", scroll(&h));
    h.input_mut().events.push(Event::PointerMoved(pos2(from.x, rows[1] + rows[3] / 2.0)));
    h.run_steps(2);
    let still = scroll(&h);
    h.run_steps(10);
    assert_eq!(scroll(&h), still, "away from the edges it stays");
    h.input_mut().events.push(Event::PointerButton { pos: upper, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

/// #371: the inline expression field grows with the text being typed (it used to keep the
/// saved text's height until committed), and its bottom edge drags to any height in lines.
#[test]
fn the_expression_field_grows_while_editing_and_drags_taller() {
    let (mut h, _) = harness();
    let (layer, opacity, transform) = {
        let s = &mut h.state_mut().session;
        let l = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
        s.execute("prop.setExpression", json!({"layer": l, "path": "transform/opacity", "expression": "50"})).unwrap();
        let tr = s.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(l)).unwrap().props.sub("transform").unwrap().clone();
        (l, tr.get("opacity").unwrap().uid, tr.uid)
    };
    h.state_mut().ui.timeline.open_layers.insert(layer);
    h.state_mut().ui.timeline.open_groups.insert(transform);
    h.run_steps(3);
    let id = format!("timeline.prop.{opacity}.expression");
    let height = |h: &Harness<'_, EffectcraftApp>| h.state().auto.find(&id).unwrap().rect[3];
    let line = h.state().tokens.row_h;
    let one = height(&h);
    let p = center(&h, &id);
    click(&mut h, p, 1);
    key(&mut h, Key::End);
    for more in ["+ 1", "+ 2"] {
        key(&mut h, Key::Enter);
        type_text(&mut h, more);
    }
    h.run_steps(2);
    assert!((height(&h) - one - 2.0 * line).abs() < 0.5, "three lines while editing: {} from {one}", height(&h));
    // Its bottom edge: dragged down 4 lines, then up past the top (one line).
    let edge_id = format!("timeline.prop.{opacity}.expressionHeight");
    let drag_edge = |h: &mut Harness<'_, EffectcraftApp>, dy: f32| {
        let edge = center(h, &edge_id);
        let to = edge + egui::vec2(0.0, dy);
        h.input_mut().events.push(Event::PointerMoved(edge));
        h.step();
        h.input_mut().events.push(Event::PointerButton { pos: edge, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.step();
        for k in 1..=6 {
            h.input_mut().events.push(Event::PointerMoved(edge + (to - edge) * (k as f32 / 6.0)));
            h.step();
        }
        h.input_mut().events.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.run_steps(3);
    };
    drag_edge(&mut h, 4.0 * line);
    assert_eq!(h.state().ui.timeline.expr_lines.get(&opacity), Some(&7));
    assert!((height(&h) - one - 6.0 * line).abs() < 0.5, "{}", height(&h));
    drag_edge(&mut h, -20.0 * line);
    assert_eq!(h.state().ui.timeline.expr_lines.get(&opacity), Some(&1));
}

fn key_with(h: &mut Harness<'_, EffectcraftApp>, key: Key, modifiers: Modifiers) {
    h.input_mut().events.push(Event::ModifiersChanged(modifiers));
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
    h.step();
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
}

/// #361: Tab in a value being typed commits it and types in the next value (the next dimension,
/// then the next property's), Shift+Tab the previous, in the Timeline and in Effect Controls,
/// as in After Effects. It used to drop the keyboard instead.
#[test]
fn tab_commits_a_value_and_moves_to_the_next() {
    let (mut h, _) = harness();
    let (layer, transform) = {
        let s = &mut h.state_mut().session;
        let l = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
        s.execute("layer.select", json!({"layers": [l]})).unwrap();
        for fx in ["Point Control", "Slider Control"] {
            s.execute("effect.apply", json!({"layers": [l], "effect": fx})).unwrap();
        }
        let tr = s.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(l)).unwrap().props.sub("transform").unwrap().uid;
        (l, tr)
    };
    h.state_mut().ui.timeline.open_layers.insert(layer);
    h.state_mut().ui.timeline.open_groups.insert(transform);
    h.state_mut().show_panel(effectcraft_ui_egui::dock::PanelKind::EffectControls);
    h.run_steps(3);
    let props =
        |h: &Harness<'_, EffectcraftApp>| h.state().session.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(layer)).unwrap().props.clone();
    let value = |h: &Harness<'_, EffectcraftApp>, path: &str| props(h).prop(path).unwrap().value.components();
    let uid = |h: &Harness<'_, EffectcraftApp>, path: &str| props(h).prop(path).unwrap().uid;
    let (position, scale) = (uid(&h, "transform/position"), uid(&h, "transform/scale"));
    let none = Modifiers::NONE;
    // Timeline: Position X, Tab, Position Y, Tab, Scale (linked).
    let p = center(&h, &format!("timeline.prop.{position}.value.0"));
    click(&mut h, p, 1);
    type_text(&mut h, "100");
    key_with(&mut h, Key::Tab, none);
    type_text(&mut h, "50");
    key_with(&mut h, Key::Tab, none);
    type_text(&mut h, "200");
    key(&mut h, Key::Enter);
    assert_eq!(value(&h, "transform/position")[..2], [100.0, 50.0]);
    assert_eq!(value(&h, "transform/scale")[..2], [200.0, 200.0]);
    assert!(!h.ctx.egui_wants_keyboard_input(), "Enter ends the typing");
    // Shift+Tab: from Scale X back to Position Y.
    let p = center(&h, &format!("timeline.prop.{scale}.value.0"));
    click(&mut h, p, 1);
    type_text(&mut h, "120");
    key_with(&mut h, Key::Tab, Modifiers::SHIFT);
    type_text(&mut h, "60");
    key(&mut h, Key::Enter);
    assert_eq!(value(&h, "transform/position")[..2], [100.0, 60.0]);
    assert_eq!(value(&h, "transform/scale")[..2], [120.0, 120.0]);
    // Effect Controls: Point Control's Y, Tab, Slider Control's Slider (the next effect).
    let point = uid(&h, "effects/#1/point");
    let p = center(&h, &format!("effectControls.prop.{point}.value.1"));
    click(&mut h, p, 1);
    type_text(&mut h, "7");
    key_with(&mut h, Key::Tab, none);
    type_text(&mut h, "42");
    key(&mut h, Key::Enter);
    assert_eq!(value(&h, "effects/#1/point")[1], 7.0);
    assert_eq!(value(&h, "effects/#2/slider"), [42.0]);
}

/// A click at `p` with `modifiers` held.
fn click_with(h: &mut Harness<'_, EffectcraftApp>, p: Pos2, modifiers: Modifiers) {
    h.input_mut().events.push(Event::PointerMoved(p));
    h.input_mut().events.push(Event::ModifiersChanged(modifiers));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers });
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers });
    h.step();
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(3);
}

/// #370: Ctrl/Cmd-click on the current-time display (the Timeline's or the Composition
/// panel's) toggles the Time Display Style between Timecode and Frames, one undo step each, as
/// in After Effects; under the Timeline's display the other style shows.
#[test]
fn ctrl_click_on_the_time_display_toggles_timecode_and_frames() {
    use effectcraft_engine::project::TimeDisplayStyle;
    let (mut h, _) = harness();
    h.state_mut().session.execute("time.set", json!({"frame": 30})).unwrap();
    h.run_steps(2);
    let shown = |h: &Harness<'_, EffectcraftApp>, id: &str| h.state().auto.find(id).unwrap().label.clone();
    let style = |h: &Harness<'_, EffectcraftApp>| h.state().session.project.settings.time_display;
    let undo = h.state().session.history.undo.len();
    let at = center(&h, "timeline.timecode");
    click_with(&mut h, at, Modifiers::COMMAND);
    assert_eq!(style(&h), TimeDisplayStyle::Frames);
    assert_eq!(shown(&h, "timeline.timecode"), "00030");
    assert_eq!(h.state().session.history.undo.len(), undo + 1, "one undo step");
    assert!(!h.ctx.egui_wants_keyboard_input(), "no time entry opened");
    let at = center(&h, "viewer.timecode");
    click_with(&mut h, at, Modifiers::COMMAND);
    assert_eq!(style(&h), TimeDisplayStyle::Timecode);
    assert_eq!(shown(&h, "timeline.timecode"), "0:00:01:06");
    // A plain click changes nothing.
    click_with(&mut h, at, Modifiers::NONE);
    let at = center(&h, "timeline.timecode");
    click_with(&mut h, at, Modifiers::NONE);
    assert_eq!(style(&h), TimeDisplayStyle::Timecode);
    // Feet + Frames goes to Timecode.
    h.state_mut().session.execute("file.projectSettings", json!({"timeDisplay": "feet35"})).unwrap();
    h.run_steps(2);
    click_with(&mut h, at, Modifiers::COMMAND);
    assert_eq!(style(&h), TimeDisplayStyle::Timecode);
}

/// Hold Alt and Shift and let go of them, as Windows' keyboard-layout hotkey does; `shift_first`
/// releases Shift before Alt.
fn switch_layout(h: &mut Harness<'_, EffectcraftApp>, shift_first: bool) {
    let held = if shift_first { Modifiers::ALT } else { Modifiers::SHIFT };
    for m in [Modifiers::ALT, Modifiers::ALT | Modifiers::SHIFT, held, Modifiers::NONE] {
        h.input_mut().events.push(Event::ModifiersChanged(m));
        h.step();
    }
    h.run_steps(2);
}

/// #394: switching the keyboard layout to Arabic with Alt+Shift while a name is being typed
/// leaves the field with the keyboard: releasing Shift before Alt left Alt held alone, which
/// counted as an Alt tap, so the menu bar took the keyboard, the rename ended and the Arabic
/// letters went nowhere. Layer and Project names keep what is typed in any script.
#[test]
fn a_layout_switch_keeps_the_name_field_and_arabic_names_are_kept() {
    let (mut h, comp) = harness();
    let layer = h.state_mut().session.execute("layer.newNull", json!({})).unwrap()["layer"].as_u64().unwrap();
    h.run_steps(3);
    let layer_name =
        |h: &Harness<'_, EffectcraftApp>| h.state().session.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(layer)).unwrap().name.clone();
    let p = center(&h, &format!("timeline.layer.{layer}.row"));
    click(&mut h, p, 2);
    assert!(h.state().auto.find(&format!("timeline.layer.{layer}.nameEdit")).is_some(), "double-click opened the rename field");
    for shift_first in [true, false] {
        switch_layout(&mut h, shift_first);
        assert!(h.state().auto.find(&format!("timeline.layer.{layer}.nameEdit")).is_some(), "the rename field stays open");
        type_text(&mut h, if shift_first { "طبقة " } else { "مرحبا" });
    }
    key(&mut h, Key::Enter);
    assert_eq!(layer_name(&h), "طبقة مرحبا");
    // The Project panel's rename field, with Hebrew, Devanagari and Thai.
    let p = center(&h, &format!("project.item.{comp}.name"));
    click(&mut h, p, 1);
    key(&mut h, Key::Enter);
    switch_layout(&mut h, true);
    type_text(&mut h, "שכבה नाम ชื่อ");
    key(&mut h, Key::Enter);
    assert_eq!(h.state().session.project.items.values().find(|i| i.id.0 == comp).unwrap().name, "שכבה नाम ชื่อ");
    // A plain Alt tap still takes the keyboard to the menu bar (#279).
    if !cfg!(target_os = "macos") {
        h.input_mut().events.push(Event::ModifiersChanged(Modifiers::ALT));
        h.step();
        h.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
        h.run_steps(2);
        let focused = h.query_by(|n| n.is_focused()).and_then(|n| n.accesskit_node().label()).unwrap_or_default();
        assert_eq!(focused.trim(), "File", "Alt focused the menu bar");
    }
}

/// #488: once a rename field is open, the arrow keys move its caret (the whole name is selected
/// only as it opens), so part of a name can be edited.
#[test]
fn arrow_keys_move_the_caret_in_rename_fields() {
    let (mut h, comp) = harness();
    let name = |h: &Harness<'_, EffectcraftApp>| h.state().session.project.items.values().find(|i| i.id.0 == comp).unwrap().name.clone();
    let p = center(&h, &format!("project.item.{comp}.name"));
    click(&mut h, p, 1);
    key(&mut h, Key::Enter);
    // The arrows keep the field (Up / Down too); in "Main", two to the left of its end.
    key(&mut h, Key::ArrowRight);
    key(&mut h, Key::ArrowLeft);
    key(&mut h, Key::ArrowUp);
    key(&mut h, Key::ArrowDown);
    key(&mut h, Key::End);
    key(&mut h, Key::ArrowLeft);
    key(&mut h, Key::ArrowLeft);
    type_text(&mut h, "X");
    key(&mut h, Key::Backspace);
    key(&mut h, Key::Backspace);
    type_text(&mut h, "Y");
    key(&mut h, Key::Enter);
    assert_eq!(name(&h), "MYin");
    // The Timeline's layer rename.
    let layer = h.state_mut().session.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
    h.state_mut().session.execute("layer.select", json!({"layers": [layer]})).unwrap();
    h.run_steps(3);
    let p = center(&h, &format!("timeline.layer.{layer}.row"));
    click(&mut h, p, 1);
    key(&mut h, Key::Enter);
    key(&mut h, Key::ArrowRight);
    key(&mut h, Key::ArrowUp);
    key(&mut h, Key::End);
    key(&mut h, Key::ArrowLeft);
    key(&mut h, Key::ArrowLeft);
    type_text(&mut h, "-");
    key(&mut h, Key::Enter);
    let layer_name = h.state().session.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(layer)).unwrap().name.clone();
    assert_eq!(layer_name, "Pla-te");
}

/// A solid named `name` with its Transform group open in the Timeline; returns the layer, its
/// Opacity uid and its Rotation uid.
fn open_transform(h: &mut Harness<'_, EffectcraftApp>, name: &str) -> (u64, u64, u64) {
    let (layer, opacity, rotation, transform) = {
        let s = &mut h.state_mut().session;
        let l = s.execute("layer.newSolid", json!({"name": name, "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
        let tr = s.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(l)).unwrap().props.sub("transform").unwrap().clone();
        (l, tr.get("opacity").unwrap().uid, tr.get("rotation").unwrap().uid, tr.uid)
    };
    h.state_mut().ui.timeline.open_layers.insert(layer);
    h.state_mut().ui.timeline.open_groups.insert(transform);
    h.run_steps(3);
    (layer, opacity, rotation)
}

fn expression(h: &Harness<'_, EffectcraftApp>, layer: u64, prop: u64) -> Option<String> {
    let comp = h.state().session.active_comp().unwrap();
    comp.layer(effectcraft_engine::project::LayerId(layer)).unwrap().props.find(prop).unwrap().expr.as_ref().map(|e| e.text.clone())
}

fn commit_expression(h: &mut Harness<'_, EffectcraftApp>) {
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::COMMAND));
    h.input_mut().events.push(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND });
    h.step();
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(3);
}

/// #1072: Alt-clicking a stopwatch adds an expression and its editor takes the keyboard at once,
/// the new expression selected, so typing replaces it (it took another click). That happens
/// once: a field clicked next keeps the keyboard, and removing the expression with another
/// Alt-click leaves nothing to take it.
#[test]
fn alt_clicking_a_stopwatch_focuses_the_new_expression() {
    let (mut h, _) = harness();
    let (layer, opacity, rotation) = open_transform(&mut h, "Plate");
    let alt = Modifiers { alt: true, ..Modifiers::NONE };
    let stopwatch = |h: &Harness<'_, EffectcraftApp>, prop: u64| center(h, &format!("timeline.prop.{prop}.stopwatch"));
    let at = stopwatch(&h, opacity);
    click_with(&mut h, at, alt);
    assert!(expression(&h, layer, opacity).is_some(), "an expression was added");
    type_text(&mut h, "50");
    commit_expression(&mut h);
    assert_eq!(expression(&h, layer, opacity).as_deref(), Some("50"), "typing replaced the new expression");
    // A field clicked right after an Alt-click keeps the keyboard.
    let at = stopwatch(&h, rotation);
    click_with(&mut h, at, alt);
    let added = expression(&h, layer, rotation).expect("an expression on Rotation");
    let p = center(&h, "timeline.timecode");
    click(&mut h, p, 2);
    type_text(&mut h, "0:00:01:00");
    key(&mut h, Key::Enter);
    let frame = h.state().session.active_comp().unwrap().frame_rate.frame_at(h.state().session.time());
    assert_eq!((frame, expression(&h, layer, rotation)), (24, Some(added)), "the time took the typing, not the expression");
    // Alt-click again removes the expression; nothing takes the keyboard after it.
    let at = stopwatch(&h, opacity);
    click_with(&mut h, at, alt);
    h.run_steps(5);
    assert_eq!(expression(&h, layer, opacity), None);
    assert!(!h.ctx.egui_wants_keyboard_input(), "nothing has the keyboard");
}

/// #1072: an expression added at the bottom of the Timeline brings its editor into view to take
/// the keyboard (its row is added below the last one shown).
#[test]
fn an_expression_added_at_the_bottom_of_the_timeline_comes_into_view() {
    let (mut h, _) = harness();
    let (layer, opacity, _) = open_transform(&mut h, "Bottom");
    for i in 0..40 {
        h.state_mut().session.execute("layer.newSolid", json!({"name": format!("Layer {i}"), "color": "#406080"})).unwrap();
    }
    h.run_steps(3);
    // Scroll to the end (the layer is at the bottom of the stack), then until Opacity is the
    // last row shown.
    h.state_mut().ui.timeline.scroll_y = 1.0e6;
    h.run_steps(3);
    let rows = h.state().auto.find("timeline.scroll").unwrap().rect;
    let bottom = rows[1] + rows[3];
    let row = h.state().auto.find(&format!("timeline.prop.{opacity}.row")).unwrap().rect;
    h.state_mut().ui.timeline.scroll_y += row[1] + row[3] - bottom;
    h.run_steps(3);
    let row = h.state().auto.find(&format!("timeline.prop.{opacity}.row")).unwrap().rect;
    assert!((row[1] + row[3] - bottom).abs() < 1.0, "Opacity is at the bottom: {row:?}");
    let at = center(&h, &format!("timeline.prop.{opacity}.stopwatch"));
    click_with(&mut h, at, Modifiers { alt: true, ..Modifiers::NONE });
    let editor = h.state().auto.find(&format!("timeline.prop.{opacity}.expression")).expect("the editor is drawn").rect;
    assert!(editor[1] >= rows[1] - 0.5 && editor[1] + editor[3] <= bottom + 0.5, "the editor is in view: {editor:?} in {rows:?}");
    type_text(&mut h, "25");
    commit_expression(&mut h);
    assert_eq!(expression(&h, layer, opacity).as_deref(), Some("25"));
}

/// A primary-button drag from `from` to `to` with `modifiers` held from the press to the release.
fn drag_with(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2, modifiers: Modifiers) {
    h.input_mut().events.push(Event::PointerMoved(from));
    h.input_mut().events.push(Event::ModifiersChanged(modifiers));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers });
    h.step();
    for k in 1..=8 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (k as f32 / 8.0)));
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers });
    h.step();
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(3);
}

/// #1072: with Alt held as it drops, a pick whip refers to an effect's property by its index,
/// `effect("Slider Control")(1)`, as in After Effects; without Alt, by its name. Onto Effect
/// Controls or the Timeline, and into the expression being edited (with a dimension).
#[test]
fn an_alt_pick_whip_refers_to_an_effect_property_by_index() {
    let (mut h, _) = harness();
    let (layer, opacity, transform, effects) = {
        let s = &mut h.state_mut().session;
        let l = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
        s.execute("layer.select", json!({"layers": [l]})).unwrap();
        for fx in ["Slider Control", "Point Control"] {
            s.execute("effect.apply", json!({"layers": [l], "effect": fx})).unwrap();
        }
        let props = s.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(l)).unwrap().props.clone();
        let effects: Vec<u64> = std::iter::once(props.sub("effects").unwrap().uid).chain(props.sub("effects").unwrap().groups().map(|g| g.uid)).collect();
        (l, props.sub("transform").unwrap().get("opacity").unwrap().uid, props.sub("transform").unwrap().uid, effects)
    };
    h.state_mut().ui.timeline.open_layers.insert(layer);
    h.state_mut().ui.timeline.open_groups.insert(transform);
    h.state_mut().ui.timeline.open_groups.extend(effects);
    h.state_mut().show_panel(effectcraft_ui_egui::dock::PanelKind::EffectControls);
    h.run_steps(3);
    let props =
        |h: &Harness<'_, EffectcraftApp>| h.state().session.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(layer)).unwrap().props.clone();
    let (slider, point) = (props(&h).prop("effects/#1/slider").unwrap().uid, props(&h).prop("effects/#2/point").unwrap().uid);
    let expr = |h: &Harness<'_, EffectcraftApp>| props(h).find(opacity).unwrap().expr.clone().map(|e| e.text);
    let alt = Modifiers { alt: true, ..Modifiers::NONE };
    let whip = |h: &mut Harness<'_, EffectcraftApp>, target: &str, modifiers: Modifiers| {
        let (from, to) = (center(h, &format!("timeline.prop.{opacity}.propertyPickWhip")), center(h, target));
        drag_with(h, from, to, modifiers);
    };
    whip(&mut h, &format!("effectControls.prop.{slider}.value"), alt);
    assert_eq!(expr(&h).as_deref(), Some("effect(\"Slider Control\")(1)"), "Alt onto Effect Controls");
    let undo = h.state().session.history.undo.len();
    whip(&mut h, &format!("effectControls.prop.{slider}.value"), Modifiers::NONE);
    assert_eq!(expr(&h).as_deref(), Some("effect(\"Slider Control\")(\"Slider\")"), "without Alt, by name");
    assert_eq!(h.state().session.history.undo.len(), undo + 1, "one undo step");
    whip(&mut h, &format!("timeline.prop.{slider}.name"), alt);
    assert_eq!(expr(&h).as_deref(), Some("effect(\"Slider Control\")(1)"), "Alt onto the Timeline");
    // Into the expression being edited, at the cursor, with the value it was dropped on.
    let p = center(&h, &format!("timeline.prop.{opacity}.expression"));
    click(&mut h, p, 1);
    key(&mut h, Key::End);
    type_text(&mut h, " + ");
    let (from, to) = (center(&h, &format!("timeline.prop.{opacity}.pickWhip")), center(&h, &format!("effectControls.prop.{point}.value.1")));
    drag_with(&mut h, from, to, alt);
    assert_eq!(expr(&h).as_deref(), Some("effect(\"Slider Control\")(1) + effect(\"Point Control\")(1)[1]"));
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(expr(&h).as_deref(), Some("effect(\"Slider Control\")(1)"), "undone in one step");
}

/// #1434: with several 3D layers selected, typing a Z Position sets Z on each of them; their X
/// and Y stay their own. Every selected layer used to take the typed layer's whole Position, so
/// they stacked on top of it and seemed to disappear.
#[test]
fn typing_one_component_with_several_layers_selected_keeps_their_other_components() {
    let (mut h, _) = harness();
    let (a, b, transform) = {
        let s = &mut h.state_mut().session;
        let mut ids = vec![];
        for (name, xy) in [("A", [40.0, 50.0]), ("B", [200.0, 120.0])] {
            let l = s.execute("layer.newSolid", json!({"name": name, "color": "#406080", "width": 40, "height": 40})).unwrap()["layer"].as_u64().unwrap();
            s.execute("layer.setSwitch", json!({"layers": [l], "switch": "threeD", "value": true})).unwrap();
            s.execute("prop.set", json!({"layer": l, "path": "transform/position", "value": [xy[0], xy[1], 0.0]})).unwrap();
            ids.push(l);
        }
        s.execute("layer.select", json!({"layers": ids})).unwrap();
        let tr = s.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(ids[0])).unwrap().props.sub("transform").unwrap().uid;
        (ids[0], ids[1], tr)
    };
    h.state_mut().ui.timeline.open_layers.insert(a);
    h.state_mut().ui.timeline.open_groups.insert(transform);
    h.run_steps(3);
    let pos = |h: &Harness<'_, EffectcraftApp>, l: u64| {
        h.state()
            .session
            .active_comp()
            .unwrap()
            .layer(effectcraft_engine::project::LayerId(l))
            .unwrap()
            .props
            .prop("transform/position")
            .unwrap()
            .value
            .components()
    };
    let position =
        h.state().session.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(a)).unwrap().props.prop("transform/position").unwrap().uid;
    let p = center(&h, &format!("timeline.prop.{position}.value.2"));
    click(&mut h, p, 1);
    type_text(&mut h, "-300");
    key(&mut h, Key::Enter);
    assert_eq!(pos(&h, a), [40.0, 50.0, -300.0]);
    assert_eq!(pos(&h, b), [200.0, 120.0, -300.0], "B keeps its own X and Y");
}
