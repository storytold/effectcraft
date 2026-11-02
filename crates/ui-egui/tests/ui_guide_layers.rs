//! Guide Layer in the layer context menus (Timeline rows and the Composition viewer), a plain item
//! (no check mark) that toggles the switch on the selection, and the guide mark by a guide layer's
//! name.
use effectcraft_engine::Session;
use effectcraft_engine::project::LayerId;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use serde_json::json;

/// A 640×360 comp with two 200×200 solids, "Left" at (160, 180) and "Right" at (480, 180).
fn harness() -> (Harness<'static, EffectcraftApp>, LayerId, LayerId) {
    let mut session = Session::default();
    session.execute("comp.new", json!({"name": "Guides", "width": 640, "height": 360, "duration": 4})).unwrap();
    let mut solid = |name: &str, x: f64| {
        let id = session.execute("layer.newSolid", json!({"name": name, "color": "#406080", "width": 200, "height": 200})).unwrap()["layer"].as_u64().unwrap();
        session.execute("prop.set", json!({"layer": id, "path": "transform/position", "value": [x, 180.0]})).unwrap();
        LayerId(id)
    };
    let left = solid("Left", 160.0);
    let right = solid("Right", 480.0);
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(session));
    h.run_steps(3);
    (h, left, right)
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("missing {id}"));
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

fn click(h: &mut Harness<'_, EffectcraftApp>, p: Pos2, button: PointerButton) {
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerMoved(p));
        h.input_mut().events.push(Event::PointerButton { pos: p, button, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
}

fn guide(h: &Harness<'_, EffectcraftApp>, id: LayerId) -> bool {
    h.state().session.active_comp().unwrap().layer(id).unwrap().switches.guide
}

fn snapshot(h: &mut Harness<'_, EffectcraftApp>, name: &str) {
    if let Ok(dir) = std::env::var("GUIDE_LAYER_SNAPSHOTS") {
        h.render().unwrap().save(format!("{dir}/{name}.png")).unwrap();
    }
}

/// The open menu's Guide Layer item carries no check mark (a plain toggle).
fn plain(h: &Harness<'_, EffectcraftApp>) -> bool {
    !h.get_by_label_contains("Guide Layer").accesskit_node().label().unwrap_or_default().contains('✔')
}

fn row(h: &Harness<'_, EffectcraftApp>, id: LayerId) -> Pos2 {
    let r = rect(h, &format!("timeline.layer.{}.row", id.0));
    pos2(r.min.x + 210.0, r.center().y)
}

/// The Guide Layer item of the open menu `prefix`.
fn guide_item(h: &Harness<'_, EffectcraftApp>, prefix: &str) -> Rect {
    rect(h, &format!("{prefix}.switch.guide"))
}

#[test]
fn timeline_layer_menu_toggles_guide_layer() {
    let (mut h, left, right) = harness();
    assert!(h.state().auto.find(&format!("timeline.layer.{}.guide", left.0)).is_none(), "no guide mark yet");
    let p = row(&h, left);
    click(&mut h, p, PointerButton::Secondary);
    assert_eq!(h.state().session.state.selected_layers, vec![left], "right-click selects the row");
    assert!(plain(&h), "no check mark");
    let item = guide_item(&h, "timeline.layerMenu");
    click(&mut h, item.center(), PointerButton::Primary);
    assert!(guide(&h, left), "Guide Layer turned on");
    assert!(!guide(&h, right), "only the target layer");
    // The guide mark shows by the name.
    h.run_steps(2);
    assert!(h.state().auto.find(&format!("timeline.layer.{}.guide", left.0)).is_some(), "guide mark by the name");
    // Again: off. Undo brings it back.
    let p = row(&h, left);
    click(&mut h, p, PointerButton::Secondary);
    assert!(plain(&h), "no check mark on a guide layer either");
    snapshot(&mut h, "timeline-menu");
    let item = guide_item(&h, "timeline.layerMenu");
    click(&mut h, item.center(), PointerButton::Primary);
    assert!(!guide(&h, left), "Guide Layer turned off");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert!(guide(&h, left));
}

#[test]
fn timeline_layer_menu_guide_layer_acts_on_the_selection() {
    let (mut h, left, right) = harness();
    h.state_mut().session.execute("layer.select", json!({"layers": [left.0, right.0]})).unwrap();
    h.run_steps(3);
    let p = row(&h, right);
    click(&mut h, p, PointerButton::Secondary);
    let item = guide_item(&h, "timeline.layerMenu");
    click(&mut h, item.center(), PointerButton::Primary);
    assert!(guide(&h, left) && guide(&h, right), "both selected layers become guide layers");
    let p = row(&h, left);
    click(&mut h, p, PointerButton::Secondary);
    let item = guide_item(&h, "timeline.layerMenu");
    click(&mut h, item.center(), PointerButton::Primary);
    assert!(!guide(&h, left) && !guide(&h, right), "and back together");
}

#[test]
fn viewer_layer_menu_toggles_guide_layer() {
    let (mut h, left, right) = harness();
    let comp = rect(&h, "viewer.comp");
    let k = comp.width() / 640.0;
    let at = |x: f32| comp.min + vec2(x, 180.0) * k;
    click(&mut h, at(480.0), PointerButton::Secondary);
    assert_eq!(h.state().session.state.selected_layers, vec![right], "right-click picks the layer under the pointer");
    let item = guide_item(&h, "viewer.layerMenu");
    click(&mut h, item.center(), PointerButton::Primary);
    assert!(guide(&h, right) && !guide(&h, left));
    // The Timeline and viewer menus share the item: the viewer menu turns it off again.
    click(&mut h, at(480.0), PointerButton::Secondary);
    assert!(plain(&h), "no check mark");
    snapshot(&mut h, "viewer-menu");
    let item = guide_item(&h, "viewer.layerMenu");
    click(&mut h, item.center(), PointerButton::Primary);
    assert!(!guide(&h, right));
}
