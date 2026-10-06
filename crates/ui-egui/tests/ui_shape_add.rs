//! Human access to shape Contents operators, including nested and empty groups.
use effectcraft_engine::{Session, project::LayerId};
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Modifiers, PointerButton, Rect, pos2, vec2};
use egui_kittest::{Harness, kittest::Queryable as _};
use serde_json::json;

fn harness(nested: bool) -> (Harness<'static, EffectcraftApp>, LayerId, u64) {
    let mut session = Session::default();
    session.execute("comp.new", json!({"width": 64, "height": 64, "duration": 1})).unwrap();
    let layer = LayerId(session.execute("layer.newShape", json!({})).unwrap()["layer"].as_u64().unwrap());
    let root = session.active_comp().unwrap().layer(layer).unwrap().props.sub("contents").unwrap().uid;
    let target = if nested {
        let group = session.execute("layer.addShapeItem", json!({"layer": layer.0, "group": root, "kind": "group"})).unwrap()["uid"].as_u64().unwrap();
        session.active_comp().unwrap().layer(layer).unwrap().props.find_group(group).unwrap().sub("contents").unwrap().uid
    } else {
        root
    };
    // A different selected layer must not redirect the clicked Contents menu.
    session.execute("layer.newSolid", json!({"width": 8, "height": 8})).unwrap();
    let mut app = EffectcraftApp::new(session);
    app.ui.timeline.open_layers.insert(layer.0);
    app.ui.timeline.open_groups.insert(root);
    if nested {
        let group = app.session.active_comp().unwrap().layer(layer).unwrap().props.sub("contents").unwrap().groups().next().unwrap().uid;
        app.ui.timeline.open_groups.insert(group);
        app.ui.timeline.open_groups.insert(target);
    }
    let mut h = Harness::builder().with_size(vec2(1600.0, 1200.0)).build_eframe(|_| app);
    h.run_steps(3);
    (h, layer, target)
}

fn click_add(h: &mut Harness<'_, EffectcraftApp>, uid: u64) {
    let e = h.state().auto.find(&format!("timeline.group.{uid}.shapeAdd")).expect("Contents has an Add button");
    let r = Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]));
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerMoved(r.center()));
        h.input_mut().events.push(Event::PointerButton { pos: r.center(), button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
}

#[test]
fn empty_shape_contents_adds_repeater_with_one_undo_and_redo() {
    let (mut h, layer, target) = harness(false);
    click_add(&mut h, target);
    h.get_by_label("Repeater").click();
    h.run_steps(3);
    let contents = h.state().session.active_comp().unwrap().layer(layer).unwrap().props.find_group(target).unwrap();
    assert_eq!(contents.children.len(), 1);
    let repeater = contents.groups().next().unwrap();
    assert_eq!(repeater.match_id, "repeater");
    assert_eq!(h.state().session.state.selected_props, vec![(layer, repeater.uid)]);
    assert!(h.state().ui.timeline.open_groups.contains(&repeater.uid));
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert!(h.state().session.active_comp().unwrap().layer(layer).unwrap().props.find_group(target).unwrap().children.is_empty());
    h.state_mut().session.execute("edit.redo", json!({})).unwrap();
    assert_eq!(h.state().session.active_comp().unwrap().layer(layer).unwrap().props.find_group(target).unwrap().children.len(), 1);
}

#[test]
fn nested_contents_adds_trim_without_changing_root_siblings() {
    let (mut h, layer, target) = harness(true);
    click_add(&mut h, target);
    h.get_by_label("Trim Paths").click();
    h.run_steps(3);
    let props = &h.state().session.active_comp().unwrap().layer(layer).unwrap().props;
    assert_eq!(props.sub("contents").unwrap().children.len(), 1);
    let nested = props.find_group(target).unwrap();
    assert_eq!(nested.children.len(), 1);
    assert_eq!(nested.groups().next().unwrap().match_id, "trim");
}

#[test]
fn adding_a_path_places_it_before_existing_paints() {
    let (mut h, layer, target) = harness(false);
    h.state_mut().session.execute("layer.addShapeItem", json!({"layer": layer.0, "group": target, "kind": "fill"})).unwrap();
    h.run_steps(3);
    click_add(&mut h, target);
    h.get_by_label("Rectangle").click();
    h.run_steps(3);
    let contents = h.state().session.active_comp().unwrap().layer(layer).unwrap().props.find_group(target).unwrap();
    assert_eq!(contents.groups().map(|g| g.match_id.as_str()).collect::<Vec<_>>(), vec!["rect", "fill"], "{}", h.state().ui.status);
}

#[test]
fn alt_add_appends_a_path_after_existing_paints() {
    let (mut h, layer, target) = harness(false);
    h.state_mut().session.execute("layer.addShapeItem", json!({"layer": layer.0, "group": target, "kind": "fill"})).unwrap();
    h.run_steps(3);
    click_add(&mut h, target);
    let e = h.state().auto.find(&format!("timeline.group.{target}.shapeAdd.rect")).unwrap();
    let r = Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]));
    for pressed in [true, false] {
        h.input_mut().events.push(Event::ModifiersChanged(Modifiers::ALT));
        h.input_mut().events.push(Event::PointerMoved(r.center()));
        h.input_mut().events.push(Event::PointerButton { pos: r.center(), button: PointerButton::Primary, pressed, modifiers: Modifiers::ALT });
        h.step();
    }
    h.run_steps(3);
    let contents = h.state().session.active_comp().unwrap().layer(layer).unwrap().props.find_group(target).unwrap();
    assert_eq!(contents.groups().map(|g| g.match_id.as_str()).collect::<Vec<_>>(), vec!["fill", "rect"], "{}", h.state().ui.status);
}

#[test]
fn operators_follow_existing_operators_and_paints_precede_existing_paints() {
    for (existing, label, expected) in [
        (vec!["rect", "fill", "repeater"], "Trim Paths", vec!["rect", "fill", "repeater", "trim"]),
        (vec!["rect", "trim", "fill"], "Offset Paths", vec!["rect", "trim", "offset", "fill"]),
        (vec!["rect", "trim", "fill"], "Stroke", vec!["rect", "trim", "stroke", "fill"]),
        (vec!["rect", "trim", "fill"], "Repeater", vec!["rect", "trim", "fill", "repeater"]),
    ] {
        let (mut h, layer, target) = harness(false);
        for kind in existing {
            h.state_mut().session.execute("layer.addShapeItem", json!({"layer": layer.0, "group": target, "kind": kind})).unwrap();
        }
        h.run_steps(3);
        click_add(&mut h, target);
        h.get_by_label(label).click();
        h.run_steps(3);
        let contents = h.state().session.active_comp().unwrap().layer(layer).unwrap().props.find_group(target).unwrap();
        assert_eq!(contents.groups().map(|g| g.match_id.as_str()).collect::<Vec<_>>(), expected, "Add {label}");
    }
}

#[test]
fn locking_a_shape_after_opening_add_prevents_a_pending_menu_edit() {
    let (mut h, layer, target) = harness(false);
    click_add(&mut h, target);
    h.state_mut().session.execute("layer.setSwitch", json!({"layers": [layer.0], "switch": "lock", "value": true})).unwrap();
    h.run_steps(3);
    h.get_by_label("Repeater").click();
    h.run_steps(3);
    assert!(h.state().session.active_comp().unwrap().layer(layer).unwrap().props.find_group(target).unwrap().children.is_empty());
}

#[test]
fn clipped_contents_add_button_cannot_receive_pointer_clicks() {
    let mut session = Session::default();
    session.execute("comp.new", json!({"width": 64, "height": 64})).unwrap();
    let layer = LayerId(session.execute("layer.newShape", json!({})).unwrap()["layer"].as_u64().unwrap());
    let target = session.active_comp().unwrap().layer(layer).unwrap().props.sub("contents").unwrap().uid;
    let mut app = EffectcraftApp::new(session);
    app.ui.timeline.open_layers.insert(layer.0);
    app.ui.timeline.open_groups.insert(target);
    let ctx = egui::Context::default();
    effectcraft_ui_egui::theme::install(&ctx, &app.tokens);
    let pane = Rect::from_min_size(pos2(0.0, 0.0), vec2(1600.0, 600.0));
    let draw = |app: &mut EffectcraftApp, clip: Rect, events: Vec<Event>| {
        ctx.run_ui(egui::RawInput { screen_rect: Some(pane), events, ..Default::default() }, |ui| {
            app.auto.begin_frame();
            ui.set_clip_rect(clip);
            effectcraft_ui_egui::panels::timeline::show(app, ui, pane);
        })
        .textures_delta
        .clear();
    };
    for _ in 0..2 {
        draw(&mut app, pane, vec![]);
    }
    let e = app.auto.find(&format!("timeline.group.{target}.shapeAdd")).unwrap();
    let button = Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]));
    let clip = Rect::from_min_max(pane.min, pos2(button.left() - 1.0, pane.bottom()));
    assert!(!clip.contains(button.center()));
    let revision = app.session.revision;
    let selection = app.session.state.selected_props.clone();
    let selected_layers = app.session.state.selected_layers.clone();
    draw(&mut app, clip, vec![Event::PointerMoved(button.center())]);
    let clipped = app.auto.find(&format!("timeline.group.{target}.shapeAdd")).unwrap();
    assert_eq!(clipped.rect, [button.min.x, button.min.y, button.width(), button.height()]);
    for pressed in [true, false] {
        draw(&mut app, clip, vec![Event::PointerButton { pos: button.center(), button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }]);
    }
    let popup = egui::Id::new(("tl-shape-add-popup", target));
    assert!(!ctx.data(|d| d.get_temp::<bool>(popup.with("open")).unwrap_or(false)));
    assert_eq!(app.session.revision, revision);
    assert_eq!(app.session.state.selected_props, selection);
    assert_eq!(app.session.state.selected_layers, selected_layers);
    assert!(app.session.active_comp().unwrap().layer(layer).unwrap().props.find_group(target).unwrap().children.is_empty());
}
