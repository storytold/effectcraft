//! A text animator's "Add:" menu lists the layer font's variable axes and puts the chosen axis on
//! that animator (#3964), as After Effects' Add ▸ Property does.

use effectcraft_engine::Session;
use effectcraft_engine::project::LayerId;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Modifiers, PointerButton, Pos2, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

fn harness(s: Session) -> Harness<'static, EffectcraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    h
}

fn click_at(h: &mut Harness<'_, EffectcraftApp>, p: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    click_at(h, pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0));
}

/// The Add menu of an existing animator offers every axis of the layer's font and adds the picked
/// one to that animator, not to a new one. Needs a variable font; the bundled Inter is static.
#[test]
fn animator_add_menu_lists_and_adds_the_font_variable_axes() {
    let Some((face, axes)) = effectcraft_engine::text::variable::find_variable_face() else {
        eprintln!("no variable font installed: skipped");
        return;
    };
    let info = effectcraft_engine::text::fonts::face(face).info.clone();
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4})).unwrap();
    let l = s.execute("layer.newText", json!({"text": "HH"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setText", json!({"layer": l, "font": info.family, "style": info.style})).unwrap();
    // Only a face the command can resolve by family/style says anything about the menu.
    let listed = s.execute("text.animatorFontAxes", json!({"layer": l})).unwrap();
    if listed["axes"].as_array().is_none_or(|a| a.is_empty()) {
        eprintln!("the variable face isn't reachable by family/style: skipped");
        return;
    }
    let axis = axes.iter().find(|a| a.max > a.min).expect("a variable axis").clone();
    s.execute("layer.addTextAnimator", json!({"layer": l, "property": "opacity"})).unwrap();
    let layer = s.active_comp().unwrap().layer(LayerId(l)).unwrap().clone();
    let text = layer.props.sub("text").unwrap().uid;
    let anims = layer.props.sub("text").unwrap().sub("animators").unwrap().uid;
    let anim = layer.props.sub("text").unwrap().sub("animators").unwrap().groups().next().unwrap().uid;

    let mut h = harness(s);
    h.state_mut().ui.timeline.open_layers.insert(l);
    h.state_mut().ui.timeline.open_groups.insert(text);
    h.state_mut().ui.timeline.open_groups.insert(anims);
    h.state_mut().ui.timeline.open_groups.insert(anim);
    h.run_steps(2);
    click(&mut h, &format!("timeline.group.{anim}.add"));
    let label = format!("Variable Font Axes: {}", axis.name);
    let entry = h.query_all_by_label(label.as_str()).map(|n| n.rect()).next().expect("the axis entry");
    click_at(&mut h, entry.center());

    let layer = h.state().session.active_comp().unwrap().layer(LayerId(l)).unwrap().clone();
    let props = layer.props.find_group(anim).unwrap().sub("properties").unwrap();
    let want = format!("axis_{}", axis.tag);
    assert!(
        props.children.iter().any(|c| c.match_id() == want.as_str()),
        "the axis joined the animator: {:?}",
        props.children.iter().map(|c| c.match_id()).collect::<Vec<_>>()
    );
}
