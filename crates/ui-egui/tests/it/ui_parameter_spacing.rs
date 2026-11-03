use effectcraft_engine::Session;
use effectcraft_ui_egui::{EffectcraftApp, dock::PanelKind};
use egui::{Event, Modifiers, PointerButton, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

fn setup() -> (EffectcraftApp, u64, u64, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width":640,"height":360,"duration":4})).unwrap();
    let lid = s.execute("layer.newSolid", json!({"name":"Plate","color":"#406080"})).unwrap()["layer"].as_u64().unwrap();
    let checkbox = s.execute("effect.apply", json!({"effect":"Checkbox Control"})).unwrap()["effects"][0].as_u64().unwrap();
    let point = s.execute("effect.apply", json!({"effect":"Point Control"})).unwrap()["effects"][0].as_u64().unwrap();
    let fx = s.active_comp().unwrap().layers[0].effects().unwrap();
    let param = |uid, id| fx.groups().find(|g| g.uid == uid).unwrap().get(id).unwrap().uid;
    let (checkbox, point) = (param(checkbox, "checkbox"), param(point, "point"));
    let mut a = EffectcraftApp::new(s);
    a.show_panel(PanelKind::EffectControls);
    a.ui.timeline.search = "Checkbox".into();
    (a, lid, checkbox, point)
}

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let r = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}")).rect;
    let p = pos2(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    h.event(Event::PointerMoved(p));
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(3);
}

#[test]
fn fixed_compact_rows_preserve_checkbox_and_point_hit_targets() {
    let (a, lid, checkbox, point) = setup();
    let mut h = Harness::builder().with_size(vec2(1500.0, 1000.0)).build_eframe(|_| a);
    h.run_steps(4);
    let fx = h.state().auto.find(&format!("effectControls.row.{checkbox}")).unwrap().rect;
    let tl = h.state().auto.find(&format!("timeline.prop.{checkbox}.row")).unwrap().rect;
    assert_eq!(fx[3], 20.0);
    assert_eq!(tl[3], 19.0);
    let value = |h: &Harness<'_, EffectcraftApp>| h.state().session.active_comp().unwrap().layers[0].props.find(checkbox).unwrap().value.clone();
    let before = value(&h);
    click(&mut h, &format!("effectControls.prop.{checkbox}.value"));
    assert_ne!(value(&h), before);
    click(&mut h, &format!("effectControls.prop.{point}.crosshair"));
    assert_eq!(h.state().ui.fx_pick.as_ref().map(|p| (p.layer, p.prop)), Some((lid, point)));
    click(&mut h, &format!("effectControls.prop.{point}.crosshair"));
    assert!(h.state().ui.fx_pick.is_none());
}

#[test]
fn nested_groups_keep_indentation_value_alignment_and_effect_gaps_at_fixed_compact_spacing() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width":64,"height":36,"duration":1})).unwrap();
    s.execute("layer.newSolid", json!({"name":"Plate","color":"#406080"})).unwrap();
    let fx = s.execute("effect.apply", json!({"effect":"Lumetri Color"})).unwrap()["effects"][0].as_u64().unwrap();
    let next = s.execute("effect.apply", json!({"effect":"Checkbox Control"})).unwrap()["effects"][0].as_u64().unwrap();
    let g = s.active_comp().unwrap().layers[0].props.find_group(fx).unwrap();
    let basic = g.sub("basicCorrection").unwrap();
    let white = basic.sub("whiteBalance").unwrap();
    let (root, basic_id, white_id) = (g.get("highDynamicRange").unwrap().uid, basic.uid, white.uid);
    let (temp, tint, sat) = (white.get("temperature").unwrap().uid, white.get("tint").unwrap().uid, basic.get("saturation").unwrap().uid);
    let closed: Vec<_> = g.groups().filter(|g| g.uid != basic_id).chain(basic.groups().filter(|g| g.uid != white_id)).map(|g| g.uid).collect();
    let mut a = EffectcraftApp::new(s);
    a.show_panel(PanelKind::EffectControls);
    a.ui.fx_closed.extend(closed);
    a.ui.timeline.search = "Temperature".into();
    let mut h = Harness::builder().with_size(vec2(1500.0, 1800.0)).build_eframe(|_| a);
    h.run_steps(4);
    let rect = |h: &Harness<'_, EffectcraftApp>, id: String| h.state().auto.find(&id).unwrap_or_else(|| panic!("no {id}")).rect;
    let name_x = |kind: &str, uid| rect(&h, format!("effectControls.{kind}.{uid}.name"))[0];
    assert!(name_x("effect", fx) < name_x("group", basic_id));
    assert_eq!(name_x("prop", root), name_x("group", basic_id), "siblings align even when their icons differ");
    assert!(name_x("group", basic_id) < name_x("group", white_id));
    assert!(name_x("group", white_id) < name_x("prop", temp));
    assert_eq!(name_x("prop", temp), name_x("prop", tint));
    assert_eq!(rect(&h, format!("effectControls.prop.{temp}.value"))[0], rect(&h, format!("effectControls.prop.{sat}.value"))[0]);
    let row = rect(&h, format!("effectControls.row.{temp}"));
    assert_eq!(rect(&h, format!("effectControls.row.{tint}"))[1], row[1] + 20.0);
    let top = rect(&h, format!("effectControls.effect.{next}"))[1];
    let last = h
        .state()
        .auto
        .elements
        .iter()
        .filter(|e| e.id.starts_with("effectControls.row.") && e.rect[1] < top)
        .map(|e| e.rect[1] + e.rect[3])
        .fold(0.0, f32::max);
    assert_eq!(top - last, 4.0, "the gap between effects is preserved");
    click(&mut h, &format!("effectControls.prop.{temp}.twirl"));
    assert_eq!(rect(&h, format!("effectControls.row.{tint}"))[1], row[1] + 20.0 + 34.0);
    click(&mut h, &format!("effectControls.prop.{temp}.twirl"));
    click(&mut h, &format!("effectControls.group.{white_id}.twirl"));
    assert!(h.state().auto.find(&format!("effectControls.row.{temp}")).is_none());
    assert_eq!(rect(&h, format!("effectControls.effect.{next}"))[1], top - 2.0 * 20.0);
    click(&mut h, &format!("effectControls.group.{white_id}.twirl"));
    assert_eq!(rect(&h, format!("effectControls.row.{temp}")), row);
    // Timeline search reveals the same nesting, with each child farther in than its parent.
    let tl_x = |kind: &str, uid| rect(&h, format!("timeline.{kind}.{uid}.name"))[0];
    assert!(tl_x("group", fx) < tl_x("group", basic_id));
    assert!(tl_x("group", basic_id) < tl_x("group", white_id));
    assert!(tl_x("group", white_id) < tl_x("prop", temp));
}
