//! The toolbar's Fill / Stroke swatch with Linear or Radial Gradient (#612, reported on Discord):
//! it opens the Gradient Editor on the selected shape layer's Gradient Fill / Gradient Stroke
//! Colors, as in After Effects, instead of Fill / Stroke Options. The words still open the
//! options, a Solid Color swatch the colour picker, Alt-click still steps through the kinds, and
//! with no such gradient selected the swatch opens the options as before.

use effectcraft_engine::Session;
use effectcraft_engine::keyframe::Value;
use effectcraft_engine::project::{LayerId, PropGroup};
use effectcraft_ui_egui::state::Tool;
use effectcraft_ui_egui::{Dialog, EffectcraftApp};
use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

/// The Colors uid of the first `paint` ("gfill" / "gstroke") group in `g`, through nested groups.
fn colors(g: &PropGroup, paint: &str) -> Option<u64> {
    g.groups().find_map(|x| if x.match_id == paint { x.get("colors").map(|p| p.uid) } else { colors(x, paint) })
}

/// A rectangle shape layer (selected unless `select` is false) with `items` added to its
/// contents, and the toolbar set with `tool` (shape.toolOptions).
fn setup(items: &[&str], tool: serde_json::Value, select: bool) -> (Harness<'static, EffectcraftApp>, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 640, "height": 360, "duration": 4})).unwrap();
    let l = s.execute("layer.newShape", json!({"kind": "rect"})).unwrap()["layer"].as_u64().unwrap();
    for kind in items {
        s.execute("layer.addShapeItem", json!({"layer": l, "kind": kind})).unwrap();
    }
    s.execute("shape.toolOptions", tool.clone()).unwrap();
    // The toolbar shows a selected shape layer's own Fill / Stroke kind (#732), so the layer gets
    // the kind too.
    if select && let Some(o) = tool.as_object() {
        let mut p = o.clone();
        p.insert("layers".into(), json!([l]));
        s.execute("shape.fillStroke", serde_json::Value::Object(p)).unwrap();
    }
    s.state.selected_layers = if select { vec![LayerId(l)] } else { vec![] };
    s.state.selected_props.clear();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    // With no shape layer selected, Fill / Stroke show while a shape tool is active.
    if !select {
        h.state_mut().ui.tool = Tool::Rectangle;
    }
    h.run_steps(4);
    (h, l)
}

fn center(h: &Harness<'_, EffectcraftApp>, id: &str) -> Pos2 {
    let r = h.state().auto.find(id).unwrap_or_else(|| panic!("no automation id {id}")).rect;
    pos2(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0)
}

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str, modifiers: Modifiers) {
    let p = center(h, id);
    h.input_mut().events.push(Event::ModifiersChanged(modifiers));
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers });
    h.step();
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(3);
}

fn layer_colors(h: &Harness<'_, EffectcraftApp>, l: u64, paint: &str) -> u64 {
    colors(&h.state().session.active_comp().unwrap().layer(LayerId(l)).unwrap().props, paint).expect("the gradient")
}

/// The options popup is open (its kind buttons are in the automation tree).
fn options_open(h: &Harness<'_, EffectcraftApp>, key: &str) -> bool {
    h.state().auto.find(&format!("header.{key}Options.solid")).is_some()
}

/// The reported bug: a Linear Gradient Fill swatch opened Fill Options. It opens the Gradient
/// Editor on the selected layer's Gradient Fill Colors, and OK sets them.
#[test]
fn a_gradient_fill_swatch_opens_the_gradient_editor() {
    let (mut h, l) = setup(&["gfill"], json!({"fillType": "linear"}), true);
    let uid = layer_colors(&h, l, "gfill");
    click(&mut h, "header.fill", Modifiers::NONE);
    assert_eq!(h.state().dialog, Some(Dialog::GradientEditor), "the swatch opens the Gradient Editor, not Fill Options");
    assert!(!options_open(&h, "fill"), "and not the options");
    let target = h.state().gradient_draft().target.clone().expect("a target");
    assert_eq!((target.layer, target.prop), (l, uid), "on the layer's Gradient Fill Colors");
    // Edit and save it: the property really changes.
    let bar = h.state().auto.find("dialog.gradient.addColor").unwrap().rect;
    let at = pos2(bar[0] + bar[2] * 0.5, bar[1] + bar[3] / 2.0);
    h.input_mut().events.push(Event::PointerMoved(at));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    click(&mut h, "dialog.gradient.ok", Modifiers::NONE);
    let v = h.state().session.active_comp().unwrap().layer(LayerId(l)).unwrap().props.find(uid).unwrap().value.clone();
    let Value::Gradient(g) = v else { panic!("not a gradient: {v:?}") };
    assert_eq!(g.colors.len(), 3, "the added stop was saved: {g:?}");
}

/// The Stroke swatch with Radial Gradient edits the Gradient Stroke's Colors (not the fill's).
#[test]
fn a_gradient_stroke_swatch_edits_the_gradient_stroke() {
    let (mut h, l) = setup(&["gfill", "gstroke"], json!({"strokeType": "radial"}), true);
    let uid = layer_colors(&h, l, "gstroke");
    click(&mut h, "header.stroke", Modifiers::NONE);
    assert_eq!(h.state().dialog, Some(Dialog::GradientEditor));
    let target = h.state().gradient_draft().target.clone().expect("a target");
    assert_eq!((target.layer, target.prop), (l, uid), "on the Gradient Stroke's Colors");
}

/// With no shape layer selected the swatch opens the options (the kind new shapes get) as before.
/// (A selected shape layer shows its own kind, #732, so a gradient kind always has its gradient.)
#[test]
fn without_a_selected_gradient_the_swatch_opens_the_options() {
    let (mut h, _) = setup(&["gfill"], json!({"fillType": "linear"}), false);
    click(&mut h, "header.fill", Modifiers::NONE);
    assert_eq!(h.state().dialog, None, "no editor");
    assert!(options_open(&h, "fill"), "Fill Options");
}

/// The rest of the toolbar is unchanged: the words open the options, a Solid Color swatch opens the
/// colour picker, and Alt-click steps to the next kind.
#[test]
fn words_solid_swatches_and_alt_click_are_unchanged() {
    let (mut h, _) = setup(&["gfill"], json!({"fillType": "linear"}), true);
    click(&mut h, "header.fillOptions", Modifiers::NONE);
    assert!(options_open(&h, "fill"), "the word opens Fill Options");
    assert_eq!(h.state().dialog, None);
    let (mut h, _) = setup(&["gfill"], json!({"fillType": "solid"}), true);
    click(&mut h, "header.fill", Modifiers::NONE);
    assert!(h.query_by_label("Color field").is_some(), "a Solid Color swatch opens the colour picker");
    assert_eq!(h.state().dialog, None);
    let (mut h, _) = setup(&["gfill"], json!({"fillType": "linear"}), true);
    click(&mut h, "header.fill", Modifiers::ALT);
    assert_eq!(h.state().dialog, None, "Alt-click doesn't open the editor");
    assert_eq!(h.state().session.state.shape_tool.fill.kind.id(), "radial", "it steps Linear → Radial");
}
