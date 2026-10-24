//! Panels that come up on their own (egui_kittest), as in After Effects: opening a comp brings up
//! its Composition panel even when it was closed (#90); adding an effect, or double-clicking one
//! in the Timeline, brings up Effect Controls (#87).

use effectcraft_engine::Session;
use effectcraft_engine::project::ItemId;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::{DockNode, PanelKind};
use egui::{Event, Pos2, pos2};
use egui_kittest::Harness;
use serde_json::json;

/// Comp "Main" (open) holding a solid, and comp "Other" (not open).
fn harness() -> (Harness<'static, EffectcraftApp>, u64, u64) {
    let mut s = Session::default();
    let other = s.execute("comp.new", json!({"name": "Other", "width": 320, "height": 180, "duration": 4})).unwrap()["comp"].as_u64().unwrap();
    let main = s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4})).unwrap()["comp"].as_u64().unwrap();
    s.execute("layer.newSolid", json!({"name": "Red", "color": [1.0, 0.0, 0.0, 1.0]})).unwrap();
    s.state.open_comps = vec![ItemId(main)];
    s.state.active_comp = Some(ItemId(main));
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    (h, other, main)
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> egui::Rect {
    let e = h.state().auto.previous.iter().chain(h.state().auto.elements.iter()).find(|e| e.id == id).cloned().unwrap_or_else(|| {
        let have: Vec<&String> = h.state().auto.elements.iter().map(|e| &e.id).collect();
        panic!("no element {id}; have {have:?}")
    });
    egui::Rect::from_min_size(pos2(e.rect[0], e.rect[1]), egui::vec2(e.rect[2], e.rect[3]))
}

fn click_n(h: &mut Harness<'_, EffectcraftApp>, p: Pos2, n: u32) {
    h.event(Event::PointerMoved(p));
    h.step();
    for _ in 0..n {
        h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
        h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    }
    h.step();
    h.run_steps(2);
}

/// The tabs of the docked group holding `p`, in order.
fn group_of(n: &DockNode, p: PanelKind) -> Option<Vec<PanelKind>> {
    match n {
        DockNode::Split { a, b, .. } => group_of(a, p).or_else(|| group_of(b, p)),
        DockNode::Tabs { panels, .. } => panels.contains(&p).then(|| panels.clone()),
        DockNode::Stack { entries } => entries.iter().find(|e| e.contains(p)).map(|e| e.panels.clone()),
    }
}

fn invoke(h: &mut Harness<'_, EffectcraftApp>, id: &str, params: serde_json::Value) {
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, id, params).unwrap();
    h.run_steps(2);
}

/// Issue #90: with the Composition panel closed (the centre shows the Layer panel), opening a
/// comp (double-click in the Project panel, or Open Composition) brings the Composition panel
/// back, in front, where it normally lives (with the Layer panel), and so does Window ▸
/// Composition.
#[test]
fn opening_a_comp_brings_up_a_closed_composition_panel() {
    let (mut h, other, main) = harness();
    assert_eq!(group_of(&h.state().ui.dock, PanelKind::Composition), Some(vec![PanelKind::Composition, PanelKind::Layer]));
    invoke(&mut h, "window.closePanel", json!({"panel": "Composition"}));
    assert!(!h.state().ui.dock.contains(PanelKind::Composition));
    assert!(h.state().ui.dock.is_visible(PanelKind::Layer), "the centre shows the Layer panel");

    // Double-click "Other" in the Project panel.
    let name = rect(&h, &format!("project.item.{other}.name"));
    click_n(&mut h, name.center(), 2);
    assert_eq!(h.state().session.active_comp_id(), Some(ItemId(other)));
    let dock = &h.state().ui.dock;
    assert!(dock.is_visible(PanelKind::Composition), "the Composition panel is back and in front");
    assert_eq!(group_of(dock, PanelKind::Composition), Some(vec![PanelKind::Layer, PanelKind::Composition]), "back in the centre");
    assert!(h.state().auto.elements.iter().any(|e| e.id == "panel.Composition"), "the Composition panel is drawn");

    // Open Composition (the Project panel's context menu) with the panel closed again.
    invoke(&mut h, "window.closePanel", json!({"panel": "Composition"}));
    invoke(&mut h, "comp.open", json!({"comp": main}));
    assert_eq!(h.state().session.active_comp_id(), Some(ItemId(main)));
    assert!(h.state().ui.dock.is_visible(PanelKind::Composition));
    assert_eq!(group_of(&h.state().ui.dock, PanelKind::Composition), Some(vec![PanelKind::Layer, PanelKind::Composition]));

    // Window ▸ Composition reopens it in the centre too (not beside Effects & Presets).
    invoke(&mut h, "window.closePanel", json!({"panel": "Composition"}));
    invoke(&mut h, "window.panel", json!({"panel": "Composition"}));
    assert_eq!(group_of(&h.state().ui.dock, PanelKind::Composition), Some(vec![PanelKind::Layer, PanelKind::Composition]));
    assert!(h.state().ui.dock.is_visible(PanelKind::Composition));
}

/// Issue #90 in a workspace without a Layer panel: closing Composition leaves only the Timeline,
/// and opening a comp brings the viewer back above it.
#[test]
fn opening_a_comp_in_the_minimal_workspace_docks_the_viewer_above_the_timeline() {
    let (mut h, other, _) = harness();
    invoke(&mut h, "window.workspace", json!({"name": "Minimal"}));
    invoke(&mut h, "window.closePanel", json!({"panel": "Composition"}));
    assert!(!h.state().ui.dock.contains(PanelKind::Composition));
    invoke(&mut h, "comp.open", json!({"comp": other}));
    match &h.state().ui.dock {
        DockNode::Split { vertical: true, a, b, .. } => {
            assert_eq!(group_of(a, PanelKind::Composition), Some(vec![PanelKind::Composition]), "the viewer is on top");
            assert!(b.contains(PanelKind::Timeline));
        }
        other => panic!("expected the viewer above the Timeline, got {other:?}"),
    }
}

/// The id of layer `name` in the active comp.
fn layer_id(h: &Harness<'_, EffectcraftApp>, name: &str) -> u64 {
    h.state().session.active_comp().unwrap().layers.iter().find(|l| l.name == name).unwrap().id.0
}

fn try_invoke(h: &mut Harness<'_, EffectcraftApp>, id: &str, params: serde_json::Value) -> serde_json::Value {
    let ctx = h.ctx.clone();
    let r = effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, id, params).unwrap();
    h.run_steps(2);
    r
}

fn ec_up(h: &Harness<'_, EffectcraftApp>) -> bool {
    h.state().ui.dock.is_visible(PanelKind::EffectControls) && h.state().auto.elements.iter().any(|e| e.id == "panel.EffectControls")
}

/// Issue #87: adding an effect to a layer from the Effect menu (and Last Effect), or dropping one
/// on a layer in the Timeline (`effect.apply` with its `layers`), brings up Effect Controls, even
/// when it was closed, on the layer the effect went to; the focus stays where it was.
#[test]
fn applying_an_effect_brings_up_effect_controls_on_that_layer() {
    let (mut h, _, _) = harness();
    invoke(&mut h, "layer.newSolid", json!({"name": "Blue", "color": [0.0, 0.0, 1.0, 1.0]}));
    let (red, blue) = (layer_id(&h, "Red"), layer_id(&h, "Blue"));
    // Effect Controls shares its group with the Project panel, which is in front.
    invoke(&mut h, "window.panel", json!({"panel": "Project"}));
    assert!(!h.state().ui.dock.is_visible(PanelKind::EffectControls));

    // Effect ▸ Blur & Sharpen ▸ Gaussian Blur on the selected layer.
    invoke(&mut h, "layer.select", json!({"layers": [red]}));
    h.state_mut().ui.focused = PanelKind::Timeline;
    try_invoke(&mut h, "effect.apply", json!({"effect": "ec.blur.gaussian"}));
    assert!(ec_up(&h), "Effect Controls came to the front");
    assert_eq!(h.state().ui.focused, PanelKind::Timeline, "the focus stays");

    // Closed: Effect ▸ Last Effect reopens it.
    invoke(&mut h, "window.closePanel", json!({"panel": "EffectControls"}));
    assert!(!h.state().ui.dock.contains(PanelKind::EffectControls));
    try_invoke(&mut h, "effect.applyLast", json!({}));
    assert!(ec_up(&h), "Effect Controls reopened");

    // Dropped on the other (unselected) layer in the Timeline: Effect Controls shows that layer,
    // with the new effect selected.
    invoke(&mut h, "window.closePanel", json!({"panel": "EffectControls"}));
    let r = try_invoke(&mut h, "effect.apply", json!({"effect": "ec.blur.gaussian", "layers": [blue]}));
    let uid = r["effects"][0].as_u64().unwrap();
    assert!(ec_up(&h));
    let st = &h.state().session.state;
    assert_eq!(st.selected_layers.iter().map(|l| l.0).collect::<Vec<_>>(), vec![blue], "Effect Controls shows the layer it went to");
    assert_eq!(st.selected_props.iter().map(|(l, u)| (l.0, *u)).collect::<Vec<_>>(), vec![(blue, uid)], "the new effect is selected");
}

/// Issue #87: double-clicking an effect's name under its layer in the Timeline opens it in
/// Effect Controls (instead of twirling it).
#[test]
fn double_clicking_an_effect_in_the_timeline_opens_effect_controls() {
    let (mut h, _, _) = harness();
    invoke(&mut h, "layer.newSolid", json!({"name": "Blue", "color": [0.0, 0.0, 1.0, 1.0]}));
    let red = layer_id(&h, "Red");
    let r = try_invoke(&mut h, "effect.apply", json!({"effect": "ec.blur.gaussian", "layers": [red]}));
    let uid = r["effects"][0].as_u64().unwrap();
    // Reveal the layer's effects (E), then close Effect Controls and select the other layer.
    invoke(&mut h, "layer.select", json!({"layers": [red]}));
    invoke(&mut h, "timeline.revealAdd.effects", json!({}));
    invoke(&mut h, "window.closePanel", json!({"panel": "EffectControls"}));
    let blue = layer_id(&h, "Blue");
    invoke(&mut h, "layer.select", json!({"layers": [blue]}));
    assert!(!h.state().ui.dock.contains(PanelKind::EffectControls));

    let name = rect(&h, &format!("timeline.group.{uid}.name"));
    click_n(&mut h, name.center(), 2);
    assert!(ec_up(&h), "Effect Controls opened");
    let st = &h.state().session.state;
    assert_eq!(st.selected_layers.iter().map(|l| l.0).collect::<Vec<_>>(), vec![red], "on the effect's layer");
    assert_eq!(st.selected_props.iter().map(|(l, u)| (l.0, *u)).collect::<Vec<_>>(), vec![(red, uid)], "with the effect selected");
    assert!(!h.state().ui.timeline.open_groups.contains(&uid), "the effect did not twirl open");
}
