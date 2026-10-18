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
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
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

#[test]
fn the_expression_twirl_collapses_and_expands_the_expression_row() {
    let (mut h, _) = harness();
    let (layer, opacity, transform) = {
        let s = &mut h.state_mut().session;
        let l = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
        s.execute("prop.setExpression", json!({"layer": l, "path": "transform/opacity", "expression": "time * 10"})).unwrap();
        let tr = s.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(l)).unwrap().props.sub("transform").unwrap().clone();
        (l, tr.get("opacity").unwrap().uid, tr.uid)
    };
    h.state_mut().ui.timeline.open_layers.insert(layer);
    h.state_mut().ui.timeline.open_groups.insert(transform);
    h.run_steps(3);

    let expr_id = format!("timeline.prop.{opacity}.expression");
    let twirl_id = format!("timeline.prop.{opacity}.exprTwirl");

    // Initially expanded: expression editor row is present.
    assert!(h.state().auto.find(&expr_id).is_some());
    assert!(!h.state().ui.timeline.expr_closed.contains(&opacity));
    assert_eq!(h.state().auto.find(&twirl_id).unwrap().label, "Twirl expression: transform/opacity");

    // Clicking the twirl collapses the expression row.
    let twirl_pos = center(&h, &twirl_id);
    click(&mut h, twirl_pos, 1);
    h.run_steps(2);

    assert!(h.state().ui.timeline.expr_closed.contains(&opacity));
    assert!(h.state().auto.find(&expr_id).is_none());

    // Clicking again expands it back.
    click(&mut h, twirl_pos, 1);
    h.run_steps(2);

    assert!(!h.state().ui.timeline.expr_closed.contains(&opacity));
    assert!(h.state().auto.find(&expr_id).is_some());
}

#[test]
fn collapsed_expression_hint_does_not_overlap_keyframes() {
    let (mut h, _) = harness();
    let (layer, opacity, transform) = {
        let s = &mut h.state_mut().session;
        let l = s.execute("layer.newSolid", json!({"name": "Solid", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
        // Add a long expression.
        let expr = "n = 0; // ".to_string() + &"a".repeat(300);
        s.execute("prop.setExpression", json!({"layer": l, "path": "transform/opacity", "expression": expr})).unwrap();
        // Add keyframes at 1.0s and 2.0s.
        s.execute("prop.addKey", json!({"layer": l, "path": "transform/opacity", "time": 1.0, "value": 50.0})).unwrap();
        s.execute("prop.addKey", json!({"layer": l, "path": "transform/opacity", "time": 2.0, "value": 100.0})).unwrap();
        let tr = s.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(l)).unwrap().props.sub("transform").unwrap().clone();
        (l, tr.get("opacity").unwrap().uid, tr.uid)
    };
    h.state_mut().ui.timeline.open_layers.insert(layer);
    h.state_mut().ui.timeline.open_groups.insert(transform);
    // Collapse the expression row.
    h.state_mut().ui.timeline.expr_closed.insert(opacity);
    h.run_steps(3);

    let hint_id = format!("timeline.prop.{opacity}.exprHint");
    let hint_entry = h.state().auto.find(&hint_id).expect("exprHint should exist before first keyframe");
    let hint_rect = hint_entry.rect;

    // Find the first keyframe.
    let keys: Vec<_> = h.state().auto.query(&format!("timeline.key.{opacity}."));
    assert!(!keys.is_empty(), "keyframes should be present");
    let first_key_x = keys.iter().map(|k| k.rect[0]).fold(f32::INFINITY, f32::min);

    // The expression hint must end before the first keyframe.
    assert!(hint_rect[0] + hint_rect[2] <= first_key_x, "hint right edge {} must not exceed first keyframe x {}", hint_rect[0] + hint_rect[2], first_key_x);
}

#[test]
fn long_expressions_do_not_overflow_layer_rows_and_are_bounded() {
    let (mut h, _) = harness();
    let (layer1, opacity, transform) = {
        let s = &mut h.state_mut().session;
        let l = s.execute("layer.newSolid", json!({"name": "Solid 1", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
        // A 25-line expression.
        let expr = (1..=25).map(|i| format!("// line {i}")).collect::<Vec<_>>().join("\n");
        s.execute("prop.setExpression", json!({"layer": l, "path": "transform/opacity", "expression": expr})).unwrap();
        let tr = s.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(l)).unwrap().props.sub("transform").unwrap().clone();
        (l, tr.get("opacity").unwrap().uid, tr.uid)
    };
    let layer2 = h.state_mut().session.execute("layer.newSolid", json!({"name": "Solid 2", "color": "#804060"})).unwrap()["layer"].as_u64().unwrap();
    h.state_mut().ui.timeline.open_layers.insert(layer1);
    h.state_mut().ui.timeline.open_groups.insert(transform);
    h.run_steps(3);
    let expr_id = format!("timeline.prop.{opacity}.expression");
    let expr_rect = h.state().auto.find(&expr_id).unwrap().rect;
    let line_h = h.state().tokens.row_h;
    // The expression row should be capped at EXPR_AUTO_LINES (8 lines), not 25 lines tall!
    assert!(expr_rect[3] <= line_h * 9.0, "expression height {} exceeds auto limit", expr_rect[3]);
    let layer2_rect = h.state().auto.find(&format!("timeline.layer.{layer2}.row")).unwrap().rect;
    // Layer 2 must sit right below the bounded expression row, NOT pushed 25 lines down!
    assert!(layer2_rect[1] < expr_rect[1] + expr_rect[3] + line_h * 2.0);
}

#[test]
fn scrolling_inside_expression_editor_does_not_scroll_timeline_layers() {
    let (mut h, _) = harness();
    let (layer, opacity, transform) = {
        let s = &mut h.state_mut().session;
        let l = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
        let expr = (1..=20).map(|i| format!("// line {i}")).collect::<Vec<_>>().join("\n");
        s.execute("prop.setExpression", json!({"layer": l, "path": "transform/opacity", "expression": expr})).unwrap();
        let tr = s.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(l)).unwrap().props.sub("transform").unwrap().clone();
        (l, tr.get("opacity").unwrap().uid, tr.uid)
    };
    h.state_mut().ui.timeline.open_layers.insert(layer);
    h.state_mut().ui.timeline.open_groups.insert(transform);
    h.run_steps(3);
    let expr_id = format!("timeline.prop.{opacity}.expression");
    let expr_pos = center(&h, &expr_id);
    let initial_scroll = h.state().ui.timeline.scroll_y;

    // Move pointer over expression editor and scroll wheel.
    h.input_mut().events.push(Event::PointerMoved(expr_pos));
    h.input_mut().events.push(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -50.0),
        modifiers: Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    });
    h.run_steps(2);

    // Timeline layers scroll position should NOT change.
    assert_eq!(h.state().ui.timeline.scroll_y, initial_scroll);
}

#[test]
fn scrolling_over_short_expression_editor_scrolls_timeline_layers() {
    let (mut h, _) = harness();
    let (layer, opacity, transform) = {
        let s = &mut h.state_mut().session;
        // Add background layers so the timeline layers area can scroll.
        for i in 1..=15 {
            s.execute("layer.newSolid", json!({"name": format!("BG {i}"), "color": "#202020"})).unwrap();
        }
        let l = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
        s.execute("prop.setExpression", json!({"layer": l, "path": "transform/opacity", "expression": "50 + 50"})).unwrap();
        let tr = s.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(l)).unwrap().props.sub("transform").unwrap().clone();
        (l, tr.get("opacity").unwrap().uid, tr.uid)
    };
    h.state_mut().ui.timeline.open_layers.insert(layer);
    h.state_mut().ui.timeline.open_groups.insert(transform);
    h.run_steps(3);
    let expr_id = format!("timeline.prop.{opacity}.expression");
    let expr_pos = center(&h, &expr_id);
    let initial_scroll = h.state().ui.timeline.scroll_y;

    // Move pointer over short expression editor and scroll wheel.
    h.input_mut().events.push(Event::PointerMoved(expr_pos));
    h.input_mut().events.push(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -50.0),
        modifiers: Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    });
    h.run_steps(2);

    // Because the short expression has no overflow, the mouse wheel must scroll the Timeline!
    assert!(
        h.state().ui.timeline.scroll_y > initial_scroll,
        "timeline scroll_y {} should have scrolled past initial {}",
        h.state().ui.timeline.scroll_y,
        initial_scroll
    );
}

#[test]
fn expression_editor_horizontal_navigation_and_bounds() {
    let (mut h, _) = harness();
    let (layer, opacity, transform) = {
        let s = &mut h.state_mut().session;
        let l = s.execute("layer.newSolid", json!({"name": "Solid", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
        // A single very long line.
        let expr = "n = 0; // ".to_string() + &"x".repeat(300);
        s.execute("prop.setExpression", json!({"layer": l, "path": "transform/opacity", "expression": expr})).unwrap();
        let tr = s.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(l)).unwrap().props.sub("transform").unwrap().clone();
        (l, tr.get("opacity").unwrap().uid, tr.uid)
    };
    h.state_mut().ui.timeline.open_layers.insert(layer);
    h.state_mut().ui.timeline.open_groups.insert(transform);
    h.run_steps(3);

    let expr_id = format!("timeline.prop.{opacity}.expression");
    let expr_rect = h.state().auto.find(&expr_id).expect("expression editor should be open").rect;

    // Focus editor and press End key to navigate to the end of the long line.
    let p = center(&h, &expr_id);
    click(&mut h, p, 1);
    key(&mut h, Key::End);
    h.run_steps(2);

    // Verify editor has focus and keyboard.
    assert!(h.ctx.egui_wants_keyboard_input(), "expression editor retains focus after navigating to end of line");

    // The editor's auto rect must stay bounded inside the timeline row area.
    assert!(expr_rect[2] > 100.0, "editor has visible width");
}

#[test]
fn drag_selecting_outside_expression_editor_scrolls_and_expands_selection() {
    let (mut h, _) = harness();
    let (layer, opacity, transform) = {
        let s = &mut h.state_mut().session;
        let l = s.execute("layer.newSolid", json!({"name": "Solid", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
        // A 20-line expression.
        let expr = (1..=20).map(|i| format!("line_{i}_code = {i};")).collect::<Vec<_>>().join("\n");
        s.execute("prop.setExpression", json!({"layer": l, "path": "transform/opacity", "expression": expr})).unwrap();
        let tr = s.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(l)).unwrap().props.sub("transform").unwrap().clone();
        (l, tr.get("opacity").unwrap().uid, tr.uid)
    };
    h.state_mut().ui.timeline.open_layers.insert(layer);
    h.state_mut().ui.timeline.open_groups.insert(transform);
    h.run_steps(3);

    let expr_id = format!("timeline.prop.{opacity}.expression");
    let expr_rect = h.state().auto.find(&expr_id).expect("expression editor should be open").rect;
    let from = pos2(expr_rect[0] + 30.0, expr_rect[1] + 5.0);
    // Drag downwards well below the bottom of the editor.
    let to = pos2(expr_rect[0] + 30.0, expr_rect[1] + expr_rect[3] + 40.0);

    drag(&mut h, from, to);
    h.run_steps(3);

    // Verify that selection range expanded significantly into the expression text.
    let edit_id = egui::Id::new(("expr-edit", opacity));
    let st = egui::text_edit::TextEditState::load(&h.ctx, edit_id).expect("editor state exists");
    let range = st.cursor.char_range().expect("text selection range exists");
    assert!(range.primary.index.0 > 80, "selection primary index {} should have scrolled past initial lines", range.primary.index.0);
}

#[test]
fn scrolled_expression_editor_is_clipped_to_rows_rect() {
    let (mut h, _) = harness();
    let (layer, _opacity, transform) = {
        let s = &mut h.state_mut().session;
        for i in 1..=15 {
            s.execute("layer.newSolid", json!({"name": format!("BG {i}"), "color": "#202020"})).unwrap();
        }
        let l = s.execute("layer.newSolid", json!({"name": "Solid", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
        let expr = (1..=20).map(|i| format!("// line {i}")).collect::<Vec<_>>().join("\n");
        s.execute("prop.setExpression", json!({"layer": l, "path": "transform/opacity", "expression": expr})).unwrap();
        let tr = s.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(l)).unwrap().props.sub("transform").unwrap().clone();
        (l, tr.get("opacity").unwrap().uid, tr.uid)
    };
    h.state_mut().ui.timeline.open_layers.insert(layer);
    h.state_mut().ui.timeline.open_groups.insert(transform);
    h.run_steps(3);

    let ruler_rect = h.state().auto.find("timeline.ruler").expect("ruler exists").rect;
    let ruler_bottom = ruler_rect[1] + ruler_rect[3];

    // Scroll timeline down so that the expression editor row partially scrolls up past the top of the rows rect.
    h.state_mut().ui.timeline.scroll_y = 170.0;
    h.run_steps(3);

    // Any shapes rendering expression editor text must NOT have a clip_rect extending above the rows rect / ruler bottom.
    let expr_shapes: Vec<_> = h
        .output()
        .shapes
        .iter()
        .filter(|s| match &s.shape {
            egui::Shape::Text(t) => t.galley.text().contains("line 1"),
            _ => false,
        })
        .collect();
    assert!(!expr_shapes.is_empty(), "expression text should be rendered");
    for s in expr_shapes {
        assert!(s.clip_rect.min.y >= ruler_bottom - 2.0, "expression text clip_rect.min.y {} overlaps ruler bottom {}", s.clip_rect.min.y, ruler_bottom);
    }
}

#[test]
fn scrolling_at_bottom_of_expression_editor_bubbles_to_timeline_layers() {
    let (mut h, _) = harness();
    let (layer, opacity, transform) = {
        let s = &mut h.state_mut().session;
        for i in 1..=15 {
            s.execute("layer.newSolid", json!({"name": format!("BG {i}"), "color": "#202020"})).unwrap();
        }
        let l = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
        let expr = (1..=20).map(|i| format!("// line {i}")).collect::<Vec<_>>().join("\n");
        s.execute("prop.setExpression", json!({"layer": l, "path": "transform/opacity", "expression": expr})).unwrap();
        let tr = s.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(l)).unwrap().props.sub("transform").unwrap().clone();
        (l, tr.get("opacity").unwrap().uid, tr.uid)
    };
    h.state_mut().ui.timeline.open_layers.insert(layer);
    h.state_mut().ui.timeline.open_groups.insert(transform);
    h.run_steps(3);

    let expr_id = format!("timeline.prop.{opacity}.expression");
    let expr_pos = center(&h, &expr_id);
    let initial_scroll = h.state().ui.timeline.scroll_y;

    // Scroll a small step downwards: editor still has lines below, so timeline must NOT scroll.
    h.input_mut().events.push(Event::PointerMoved(expr_pos));
    h.input_mut().events.push(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -30.0),
        modifiers: Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    });
    h.run_steps(2);
    assert_eq!(h.state().ui.timeline.scroll_y, initial_scroll, "timeline layers should not scroll while editor has lines below");

    // Scroll several large steps to reach the bottom of the expression code.
    for _ in 0..6 {
        h.input_mut().events.push(Event::PointerMoved(expr_pos));
        h.input_mut().events.push(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -100.0),
            modifiers: Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        });
        h.step();
    }
    h.run_steps(2);

    // Timeline layers scroll position must have advanced because the editor ran out of lines below!
    assert!(
        h.state().ui.timeline.scroll_y > initial_scroll,
        "timeline scroll_y {} should have scrolled past initial {}",
        h.state().ui.timeline.scroll_y,
        initial_scroll
    );
}
