//! #1190: the shared drag gesture also reaches real dialog controls.
use effectcraft_engine::Session;
use effectcraft_ui_egui::{EffectcraftApp, menus};
use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::{Harness, kittest::Queryable as _};
use serde_json::json;

fn pointer(h: &mut Harness<'_, EffectcraftApp>, at: Pos2, pressed: Option<bool>) {
    h.event(Event::PointerMoved(at));
    h.step();
    if let Some(pressed) = pressed {
        h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
}

#[test]
fn view_options_drag_checks_and_unchecks_neighbours() {
    let mut session = Session::default();
    session.prefs.disk.disk_cache_enabled = false;
    session.execute("comp.new", json!({"width":32,"height":32,"duration":1})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1200.0, 900.0)).build_eframe(|_| EffectcraftApp::new(session));
    h.state_mut().ui.viewer.rulers = false;
    h.state_mut().ui.viewer.guides = true;
    h.state_mut().ui.viewer.grid = false;
    h.run_steps(3);
    let ctx = h.ctx.clone();
    menus::invoke(h.state_mut(), &ctx, "view.options", json!({})).unwrap();
    h.run_steps(3);
    let rulers = h.get_by_label("Rulers").rect().center();
    let guides = h.get_by_label("Guides").rect().center();
    let grid = h.get_by_label("Grid").rect().center();
    pointer(&mut h, rulers, Some(true));
    assert!(h.state().ui.viewer.rulers);
    pointer(&mut h, guides, None);
    assert!(h.state().ui.viewer.guides, "an already-checked neighbour stays checked");
    pointer(&mut h, grid, None);
    assert!(h.state().ui.viewer.grid);
    pointer(&mut h, rulers, None);
    pointer(&mut h, rulers, Some(false));
    assert!(h.state().ui.viewer.rulers, "revisiting and releasing does not undo the change");
    if let Ok(path) = std::env::var("CHECKBOX_REVIEW_PNG") {
        h.render().unwrap().save(path).unwrap();
    }
    pointer(&mut h, grid, Some(true));
    pointer(&mut h, guides, None);
    pointer(&mut h, rulers, None);
    pointer(&mut h, rulers, Some(false));
    let viewer = &h.state().ui.viewer;
    assert!(!viewer.rulers && !viewer.guides && !viewer.grid);
    pointer(&mut h, guides, None);
    assert!(!h.state().ui.viewer.guides, "hover after release does not change a checkbox");
}
