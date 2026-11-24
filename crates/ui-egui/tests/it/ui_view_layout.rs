//! #3391: the Composition viewer's control row has a "Views: 1 | 2 | 4" switch (egui_kittest).

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use egui_kittest::Harness;
use serde_json::json;

fn harness(comp: Option<&str>) -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    if let Some(comp) = comp {
        s.execute("comp.open", json!({"comp": comp})).unwrap();
    }
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_context_setup(effectcraft_ui_egui::framework::install_headless).build_eframe(|_| EffectcraftApp::new(s))
}

/// Click the centre of the automation element `id`.
fn click_id(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    h.run_steps(3);
    let [x, y, w, hh] = h.state().auto.find(id).unwrap_or_else(|| panic!("{id} is registered")).rect;
    let p = egui::pos2(x + w / 2.0, y + hh / 2.0);
    h.input_mut().events.push(egui::Event::PointerMoved(p));
    h.input_mut().events.push(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.step();
    h.input_mut().events.push(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(3);
}

#[test]
fn the_layout_buttons_switch_between_one_two_and_four_views_in_a_3d_comp() {
    let mut h = harness(Some("3D Showcase"));
    h.run_steps(3);
    assert!(h.state().auto.find("viewer.view3d").is_some(), "this is a 3D comp");
    for (id, want) in [("viewer.layout.4", 4), ("viewer.layout.2", 2), ("viewer.layout.1", 1)] {
        click_id(&mut h, id);
        assert_eq!(h.state().session.state.view_layout, want, "{id}");
    }
}

#[test]
fn the_layout_buttons_are_there_in_a_2d_comp_too() {
    let mut h = harness(None);
    h.run_steps(3);
    assert!(h.state().auto.find("viewer.view3d").is_none(), "this is a 2D comp");
    for id in ["viewer.layout.1", "viewer.layout.2", "viewer.layout.4"] {
        assert!(h.state().auto.find(id).is_some(), "{id} is registered");
    }
    click_id(&mut h, "viewer.layout.4");
    assert_eq!(h.state().session.state.view_layout, 4);
}

#[test]
fn the_layout_buttons_are_named_in_full_and_expose_the_current_layout() {
    use egui::accesskit::{Role, Toggled};
    use egui_kittest::kittest::{NodeT, Queryable};
    let mut h = harness(None);
    h.run_steps(3);
    let state = |h: &Harness<'_, EffectcraftApp>, name: &str| {
        let node = h.query_by_label(name).unwrap_or_else(|| panic!("a button named {name:?}"));
        let node = node.accesskit_node();
        assert_eq!(node.role(), Role::Button, "{name}");
        node.toggled()
    };
    assert_eq!(state(&h, "1 View"), Some(Toggled::True));
    assert_eq!(state(&h, "2 Views"), Some(Toggled::False));
    assert_eq!(state(&h, "4 Views"), Some(Toggled::False));
    click_id(&mut h, "viewer.layout.2");
    assert_eq!(state(&h, "1 View"), Some(Toggled::False));
    assert_eq!(state(&h, "2 Views"), Some(Toggled::True));
}
