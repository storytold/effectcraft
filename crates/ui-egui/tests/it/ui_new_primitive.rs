//! Layer ▸ New ▸ 3D Primitive… (#1734): the dialog picks a type, name and dimensions and runs
//! `layer.newPrimitive`, which switches a Classic 3D comp to Advanced 3D in the same step.

use effectcraft_engine::Session;
use effectcraft_engine::project::{Layer, LayerSource, PrimitiveKind, Renderer};
use effectcraft_ui_egui::{Dialog, EffectcraftApp};
use egui::{Event, Key, Modifiers, PointerButton};
use egui_kittest::Harness;
use serde_json::json;

fn harness() -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Scene", "width": 640, "height": 360, "duration": 2})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.step();
    h
}

/// An automation element's rect in the last frame drawn (a dialog settles its place in its
/// first frames).
fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> Option<[f32; 4]> {
    let auto = &h.state().auto;
    auto.elements.iter().chain(auto.previous.iter()).find(|e| e.id == id).map(|e| e.rect)
}

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let [x, y, w, hh] = rect(h, id).unwrap_or_else(|| panic!("{id} is registered"));
    let p = egui::pos2(x + w / 2.0, y + hh / 2.0);
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn key(h: &mut Harness<'_, EffectcraftApp>, key: Key, modifiers: Modifiers) {
    for pressed in [true, false] {
        h.input_mut().events.push(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
    }
    h.run_steps(2);
}

/// Types `text` over a field's contents and leaves it with Tab.
fn type_into(h: &mut Harness<'_, EffectcraftApp>, id: &str, text: &str) {
    click(h, id);
    assert!(h.ctx.egui_wants_keyboard_input(), "{id} has focus");
    key(h, Key::A, Modifiers::COMMAND);
    h.input_mut().events.push(Event::Text(text.into()));
    h.step();
    key(h, Key::Tab, Modifiers::NONE);
}

/// Layer ▸ New ▸ 3D Primitive…, as the menu runs it.
fn open(h: &mut Harness<'_, EffectcraftApp>) {
    let (path, entry) =
        effectcraft_engine::menus::entries().into_iter().find(|(_, e)| e.label == "3D Primitive...").expect("Layer ▸ New lists 3D Primitive...");
    assert_eq!(path, ["Layer", "New"]);
    assert_eq!(entry.command, "layer.newPrimitive");
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, &entry.command, entry.params_or_empty()).unwrap();
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(Dialog::NewPrimitive));
}

fn choose(h: &mut Harness<'_, EffectcraftApp>, kind: PrimitiveKind) {
    click(h, "dialog.primitive.type");
    let i = PrimitiveKind::ALL.iter().position(|k| *k == kind).unwrap();
    click(h, &format!("dialog.primitive.typeItem.{i}"));
}

fn layers(h: &Harness<'_, EffectcraftApp>) -> Vec<Layer> {
    h.state().session.active_comp().unwrap().layers.clone()
}

fn renderer(h: &Harness<'_, EffectcraftApp>) -> Renderer {
    h.state().session.active_comp().unwrap().renderer
}

fn geometry(l: &Layer, id: &str) -> f64 {
    l.props.prop(&format!("geometryOptions/{id}")).unwrap_or_else(|| panic!("{id}")).value.as_f64()
}

/// A torus named "Ring" with a 250 px radius, the rest left at their defaults.
fn create_ring(h: &mut Harness<'_, EffectcraftApp>) {
    open(h);
    choose(h, PrimitiveKind::Torus);
    type_into(h, "dialog.primitive.name", "Ring");
    type_into(h, "dialog.primitive.radius", "250");
    click(h, "dialog.primitive.ok");
    assert_eq!(h.state().dialog, None);
}

fn assert_ring(l: &Layer) {
    assert_eq!(l.source, LayerSource::Primitive { kind: PrimitiveKind::Torus });
    assert_eq!(l.name, "Ring");
    assert_eq!(geometry(l, "radius"), 250.0);
    assert_eq!(geometry(l, "tubeRadius"), 50.0);
}

#[test]
fn creates_a_primitive_in_a_classic_3d_comp_in_one_undoable_step() {
    let mut h = harness();
    assert_eq!(renderer(&h), Renderer::Classic3D);
    let steps = h.state().session.history.undo.len();
    create_ring(&mut h);
    let ls = layers(&h);
    assert_eq!(ls.len(), 1);
    assert_ring(&ls[0]);
    assert_eq!(renderer(&h), Renderer::Advanced3D, "the comp switches to Advanced 3D");
    assert_eq!(h.state().session.history.undo.len(), steps + 1);
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert!(layers(&h).is_empty());
    assert_eq!(renderer(&h), Renderer::Classic3D, "undo restores the renderer too");
    h.state_mut().session.execute("edit.redo", json!({})).unwrap();
    assert_ring(&layers(&h)[0]);
    assert_eq!(renderer(&h), Renderer::Advanced3D);
}

#[test]
fn cancel_creates_nothing() {
    let mut h = harness();
    let steps = h.state().session.history.undo.len();
    open(&mut h);
    choose(&mut h, PrimitiveKind::Sphere);
    type_into(&mut h, "dialog.primitive.radius", "80");
    click(&mut h, "dialog.primitive.cancel");
    assert_eq!(h.state().dialog, None);
    assert!(layers(&h).is_empty());
    assert_eq!(renderer(&h), Renderer::Classic3D);
    assert_eq!(h.state().session.history.undo.len(), steps);
}

#[test]
fn each_type_offers_the_dimensions_its_layer_has() {
    let mut h = harness();
    for kind in PrimitiveKind::ALL {
        open(&mut h);
        choose(&mut h, kind);
        click(&mut h, "dialog.primitive.ok");
        let l = layers(&h).into_iter().find(|l| l.source == LayerSource::Primitive { kind }).unwrap();
        assert_eq!(l.name, kind.label(), "the name follows the type until typed");
        let group = l.props.group("geometryOptions").unwrap();
        let ids: Vec<&str> = group.props().map(|p| p.match_id.as_str()).collect();
        open(&mut h);
        choose(&mut h, kind);
        let controls = ["type", "name", "ok", "cancel"];
        let shown: Vec<&str> = (h.state().auto.elements.iter())
            .filter_map(|e| e.id.strip_prefix("dialog.primitive."))
            .filter(|id| !controls.contains(id) && !id.starts_with("typeItem."))
            .collect();
        assert_eq!(shown, ids, "{} dimensions", kind.label());
        click(&mut h, "dialog.primitive.cancel");
    }
}

#[test]
fn a_created_primitive_survives_save_and_reopen() {
    let mut h = harness();
    create_ring(&mut h);
    let path = std::env::temp_dir().join(format!("ec-ui-new-primitive-{}.ecproj", std::process::id()));
    h.state_mut().session.execute("file.saveAs", json!({"path": path.to_string_lossy()})).unwrap();
    let mut o = Session::default();
    o.execute("file.open", json!({"path": path.to_string_lossy()})).unwrap();
    let _ = std::fs::remove_file(&path);
    let (_, comp) = o.project.comps().next().unwrap();
    assert_eq!(comp.renderer, Renderer::Advanced3D);
    assert_ring(&comp.layers[0]);
}

#[test]
fn untouched_dimensions_take_each_types_defaults_and_typed_ones_carry_over() {
    let mut h = harness();
    open(&mut h);
    choose(&mut h, PrimitiveKind::Plane);
    click(&mut h, "dialog.primitive.ok");
    let plane = layers(&h).remove(0);
    assert_eq!(geometry(&plane, "width"), 400.0, "a plane's default, not the cube's 300");
    open(&mut h);
    type_into(&mut h, "dialog.primitive.width", "120");
    choose(&mut h, PrimitiveKind::Plane);
    click(&mut h, "dialog.primitive.ok");
    let plane = layers(&h).into_iter().find(|l| l.id != plane.id).unwrap();
    assert_eq!(geometry(&plane, "width"), 120.0, "the typed width carries over");
    assert_eq!(geometry(&plane, "height"), 400.0);
}

/// A dialog left open while something else changes the editing context closes without creating
/// anything rather than adding the primitive where it was not opened.
fn closes_when(change: impl FnOnce(&mut Session)) {
    let mut h = harness();
    let a = h.state().session.active_comp_id().unwrap();
    open(&mut h);
    choose(&mut h, PrimitiveKind::Torus);
    change(&mut h.state_mut().session);
    let b = h.state().session.active_comp_id().unwrap();
    let steps = h.state().session.history.undo.len();
    h.run_steps(2);
    assert_eq!(h.state().dialog, None, "the dialog closes");
    assert!(rect(&h, "dialog.primitive.ok").is_none());
    let s = &h.state().session;
    for c in [a, b] {
        if let Some(c) = s.project.comp(c) {
            assert!(c.layers.is_empty(), "nothing is created");
            assert_eq!(c.renderer, Renderer::Classic3D);
        }
    }
    assert_eq!(s.history.undo.len(), steps);
    assert!(h.state().ui.status.contains("changed"), "{}", h.state().ui.status);
}

#[test]
fn another_comp_made_active_closes_the_dialog() {
    closes_when(|s| {
        s.execute("comp.new", json!({"name": "Other", "width": 640, "height": 360, "duration": 2})).unwrap();
    });
}

#[test]
fn a_replaced_project_closes_the_dialog_even_when_the_comp_id_is_reused() {
    closes_when(|s| {
        let before = s.active_comp_id().unwrap();
        s.execute("file.newProject", json!({})).unwrap();
        s.execute("comp.new", json!({"name": "Scene", "width": 640, "height": 360, "duration": 2})).unwrap();
        assert_eq!(s.active_comp_id(), Some(before), "the new project reuses the comp's id");
    });
}
