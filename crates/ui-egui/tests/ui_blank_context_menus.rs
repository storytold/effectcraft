//! Right-click actions in the blank Project and Timeline areas, and on Timeline property names.
use effectcraft_engine::Session;
use effectcraft_ui_egui::{Dialog, EffectcraftApp, dock::PanelKind};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use egui_kittest::{Harness, kittest::Queryable as _};
use serde_json::json;

fn harness() -> Harness<'static, EffectcraftApp> {
    let mut session = Session::default();
    session.execute("comp.new", json!({"name":"Empty", "width":640,"height":360,"duration":4})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(session));
    h.run_steps(3);
    h
}
fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("missing {id}"));
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}
fn snapshot(h: &mut Harness<'_, EffectcraftApp>, name: &str) {
    if let Ok(dir) = std::env::var("BLANK_CONTEXT_MENU_SNAPSHOTS") {
        h.render().unwrap().save(format!("{dir}/{name}.png")).unwrap();
    }
}
fn right_click(h: &mut Harness<'_, EffectcraftApp>, p: Pos2) {
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerMoved(p));
        h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
}

#[test]
fn project_blank_menu_creates_a_folder_and_undo_restores_the_project() {
    let mut h = harness();
    let before = h.state().session.project.items.len();
    let p = rect(&h, "project.empty").center();
    right_click(&mut h, p);
    snapshot(&mut h, "project");
    h.get_by_label("New Folder").click();
    h.run_steps(3);
    assert_eq!(h.state().session.project.items.len(), before + 1);
    assert!(h.state().session.project.items.values().any(|i| i.is_folder()));
    assert!(h.query_by_label("New Folder").is_none());
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(h.state().session.project.items.len(), before);
}

#[test]
fn project_blank_menu_opens_composition_settings_and_offers_import() {
    let mut h = harness();
    let p = rect(&h, "project.empty").center();
    right_click(&mut h, p);
    assert!(h.query_by_label("Import File…").is_some());
    h.get_by_label("New Composition…").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::NewComp));
}

#[test]
fn timeline_blank_outline_and_time_graph_create_layers() {
    for graph in [false, true] {
        let mut h = harness();
        let area = rect(&h, "timeline.layerMarquee");
        let p = if graph { pos2(area.max.x + 150.0, area.center().y) } else { area.center() };
        right_click(&mut h, p);
        h.get_by_label("New ⏵").click();
        h.run_steps(3);
        snapshot(&mut h, if graph { "timeline-graph" } else { "timeline-outline" });
        h.get_by_label("Null Object").click();
        h.run_steps(3);
        assert_eq!(h.state().session.active_comp().unwrap().layers.len(), 1);
        h.state_mut().session.execute("edit.undo", json!({})).unwrap();
        assert!(h.state().session.active_comp().unwrap().layers.is_empty());
    }
}

#[test]
fn timeline_blank_menu_opens_solid_settings() {
    let mut h = harness();
    let p = rect(&h, "timeline.layerMarquee").center();
    right_click(&mut h, p);
    h.get_by_label("New ⏵").click();
    h.run_steps(3);
    h.get_by_label("Solid…").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::SolidSettings));
}

#[test]
fn project_item_and_timeline_layer_keep_their_own_context_menus() {
    let mut h = harness();
    let comp = h.state().session.active_comp_id().unwrap();
    let p = rect(&h, &format!("project.item.{}.name", comp.0)).center();
    right_click(&mut h, p);
    assert!(h.query_by_label("Open Composition").is_some());
    assert!(h.query_by_label("New Folder").is_none());
    egui::Popup::close_all(&h.ctx);
    h.state_mut().session.execute("layer.newNull", json!({})).unwrap();
    h.run_steps(3);
    let id = h.state().session.active_comp().unwrap().layers[0].id;
    let r = rect(&h, &format!("timeline.layer.{}.row", id.0));
    right_click(&mut h, pos2(r.min.x + 210.0, r.center().y));
    assert!(h.query_by_label("Duplicate").is_some());
    assert!(h.query_by_label("Composition Settings…").is_none());
}

#[test]
fn project_blank_import_invokes_the_file_picker() {
    let mut h = harness();
    let called = std::rc::Rc::new(std::cell::Cell::new(false));
    let picked = called.clone();
    h.state_mut().hooks.pick_files = Some(Box::new(move |_| {
        picked.set(true);
        Vec::new()
    }));
    let p = rect(&h, "project.empty").center();
    right_click(&mut h, p);
    h.get_by_label("Import File…").click();
    h.run_steps(3);
    assert!(called.get());
    assert!(h.query_by_label("Import File…").is_none());
}

#[test]
fn timeline_blank_menu_opens_composition_settings() {
    let mut h = harness();
    let p = rect(&h, "timeline.layerMarquee").center();
    right_click(&mut h, p);
    h.get_by_label("Composition Settings…").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::CompSettings));
}

#[test]
fn timeline_blank_menu_opens_current_comp_in_essential_graphics() {
    for graph in [false, true] {
        let mut h = harness();
        let comp = h.state().session.active_comp_id().unwrap();
        let area = rect(&h, "timeline.layerMarquee");
        let p = if graph { pos2(area.max.x + 150.0, area.center().y) } else { area.center() };
        right_click(&mut h, p);
        let target = if graph { "timeline.context.graph.essentialGraphics" } else { "timeline.context.outline.essentialGraphics" };
        assert!(h.state().auto.find(target).is_some());
        h.get_by_label("Open in Essential Graphics").click();
        h.run_steps(3);
        assert_eq!(h.state().session.state.essential_primary, Some(comp));
        assert!(h.state().ui.dock.contains(PanelKind::EssentialGraphics));
    }
}

/// #407: right-clicking a property's name in the Timeline opens After Effects' property menu:
/// Reset, Edit Value…, Separate Dimensions (Position only, checked once separated), Add or
/// Remove Expression and Add Property to Essential Graphics.
#[test]
fn timeline_property_names_have_the_property_menu() {
    let mut h = harness();
    let layer = h.state_mut().session.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
    h.state_mut().session.execute("prop.set", json!({"layer": layer, "path": "transform/rotation", "value": 30})).unwrap();
    let props =
        |h: &Harness<'_, EffectcraftApp>| h.state().session.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(layer)).unwrap().props.clone();
    let uid = |h: &Harness<'_, EffectcraftApp>, path: &str| props(h).prop(path).unwrap().uid;
    h.state_mut().ui.timeline.open_layers.insert(layer);
    let transform = props(&h).sub("transform").unwrap().uid;
    h.state_mut().ui.timeline.open_groups.insert(transform);
    h.run_steps(3);
    let menu = |h: &mut Harness<'_, EffectcraftApp>, uid: u64| {
        let p = rect(h, &format!("timeline.prop.{uid}.name")).center();
        right_click(h, p);
    };
    let choose = |h: &mut Harness<'_, EffectcraftApp>, uid: u64, entry: &str| {
        let p = rect(h, &format!("timeline.prop.{uid}.menu.{entry}")).center();
        for pressed in [true, false] {
            h.input_mut().events.push(Event::PointerMoved(p));
            h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
            h.step();
        }
        h.run_steps(3);
    };
    // Reset: Rotation's default.
    let rotation = uid(&h, "transform/rotation");
    menu(&mut h, rotation);
    assert!(h.query_by_label("Edit Value…").is_some());
    assert!(h.query_by_label("Separate Dimensions").is_none(), "only Position separates");
    choose(&mut h, rotation, "reset");
    assert_eq!(props(&h).prop("transform/rotation").unwrap().value.as_f64(), 0.0);
    // Separate Dimensions on Position; X Position's menu has it too, and joins them again.
    let position = uid(&h, "transform/position");
    menu(&mut h, position);
    choose(&mut h, position, "separateDimensions");
    assert!(props(&h).prop("transform/positionX").is_some(), "separated");
    let x = uid(&h, "transform/positionX");
    menu(&mut h, x);
    choose(&mut h, x, "separateDimensions");
    assert!(props(&h).prop("transform/positionX").is_none(), "joined");
    // Add Expression, then Remove Expression.
    let opacity = uid(&h, "transform/opacity");
    menu(&mut h, opacity);
    choose(&mut h, opacity, "addExpression");
    assert_eq!(props(&h).prop("transform/opacity").unwrap().expr.as_ref().map(|e| e.text.clone()).as_deref(), Some("transform.opacity"));
    menu(&mut h, opacity);
    choose(&mut h, opacity, "removeExpression");
    assert!(props(&h).prop("transform/opacity").unwrap().expr.is_none());
    // Add Property to Essential Graphics.
    menu(&mut h, opacity);
    choose(&mut h, opacity, "essentialGraphics");
    let comp = h.state().session.active_comp_id().unwrap();
    assert_eq!(h.state().session.project.comp(comp).unwrap().essential.as_ref().map_or(0, |e| e.controls.len()), 1);
    // Edit Value… opens the value dialog.
    let scale = uid(&h, "transform/scale");
    menu(&mut h, scale);
    choose(&mut h, scale, "editValue");
    assert_eq!(h.state().dialog, Some(Dialog::Form));
}

/// The rows of a context menu touch: a gap between two entries highlighted neither and
/// swallowed a click there.
#[test]
fn context_menu_rows_leave_no_gap_between_them() {
    let mut h = harness();
    let p = rect(&h, "project.empty").center();
    right_click(&mut h, p);
    let rows = ["newComp", "newFolder", "import"].map(|key| rect(&h, &format!("project.context.{key}")));
    for pair in rows.windows(2) {
        assert!((pair[1].top() - pair[0].bottom()).abs() < 0.5, "gap between {:?} and {:?}", pair[0], pair[1]);
    }
    snapshot(&mut h, "project-rows");
}

/// The layer context menu offers the whole effect palette as an "Effects" submenu (synced with
/// the Effect menu), between Blending Mode and Arrange; picking an effect applies it.
#[test]
fn timeline_layer_context_menu_has_the_effects_submenu_and_applies_an_effect() {
    let mut h = harness();
    let layer = h.state_mut().session.execute("layer.newNull", json!({"name": "Null"})).unwrap()["layer"].as_u64().unwrap();
    h.run_steps(3);
    let r = rect(&h, &format!("timeline.layer.{layer}.row"));
    right_click(&mut h, pos2(r.min.x + 210.0, r.center().y));
    // The palette sits between Blending Mode and Arrange, as After Effects' layer menu does.
    let blending = h.get_by_label("Blending Mode ⏵").rect();
    let effects = h.get_by_label("Effects ⏵").rect();
    let arrange = h.get_by_label("Arrange ⏵").rect();
    assert!(blending.bottom() <= effects.top() && effects.bottom() <= arrange.top(), "Effects between Blending Mode and Arrange");
    h.get_by_label("Effects ⏵").click();
    h.run_steps(3);
    h.get_by_label("Blur & Sharpen ⏵").click();
    h.run_steps(3);
    h.get_by_label("Gaussian Blur").click();
    h.run_steps(3);
    let names: Vec<String> = h
        .state()
        .session
        .active_comp()
        .unwrap()
        .layer(effectcraft_engine::project::LayerId(layer))
        .unwrap()
        .effects()
        .map(|f| f.groups().map(|g| g.name.clone()).collect())
        .unwrap_or_default();
    assert_eq!(names, ["Gaussian Blur"]);
}

/// #976: a layer's context menu has After Effects' Markers ▸ Add Marker, which marks the layer that
/// was right-clicked (not the one selected before, and not the composition) at the current time.
#[test]
fn layer_context_menu_adds_a_layer_marker() {
    let mut h = harness();
    let solid = |h: &mut Harness<'_, EffectcraftApp>, name: &str| -> u64 {
        h.state_mut().session.execute("layer.newSolid", json!({"name": name, "color": "#406080"})).unwrap()["layer"].as_u64().unwrap()
    };
    let (below, top) = (solid(&mut h, "Below"), solid(&mut h, "Top"));
    h.state_mut().session.execute("layer.select", json!({"layers": [below]})).unwrap();
    h.state_mut().session.execute("time.set", json!({"time": 1.5})).unwrap();
    h.run_steps(3);
    let r = rect(&h, &format!("timeline.layer.{top}.row"));
    right_click(&mut h, pos2(r.min.x + 210.0, r.center().y));
    h.get_by_label("Markers ⏵").click();
    h.run_steps(3);
    snapshot(&mut h, "layer-markers");
    h.get_by_label("Add Marker").click();
    h.run_steps(3);
    let now = h.state().session.time().seconds();
    let comp = h.state().session.active_comp().unwrap();
    let markers = |id: u64| -> Vec<f64> { comp.layer(effectcraft_engine::project::LayerId(id)).unwrap().markers.iter().map(|m| m.time.seconds()).collect() };
    assert_eq!(markers(top), vec![now]);
    assert!((now - 1.5).abs() < 0.01, "at the current time: {now}");
    assert!(markers(below).is_empty(), "{:?}", markers(below));
    assert!(comp.markers.is_empty(), "no composition marker");
    assert_eq!(h.state().session.history.undo.last().map(|u| u.0.as_str()), Some("Add Marker"));
}
