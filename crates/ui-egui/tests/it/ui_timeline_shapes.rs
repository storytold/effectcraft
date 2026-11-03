//! Timeline rows for shape layers, masks and keyframing: the Contents "Add:" menu and the Classic
//! 3D "Change Renderer…" row (#206), Alt+Shift+P revealing the key it adds and clicks below the
//! layers deselecting them (#205), Mask Feather's linked values (#203) and shape Size's chain link
//! (#2489), copying shape items between shape layers (#227), and Ctrl+V of SVG markup making a shape
//! layer (#738).

use effectcraft_engine::Session;
use effectcraft_engine::project::LayerId;
use effectcraft_ui_egui::{Dialog, EffectcraftApp};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

fn harness(s: Session) -> Harness<'static, EffectcraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    h
}

fn session() -> Session {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4})).unwrap();
    s
}

fn center(h: &Harness<'_, EffectcraftApp>, id: &str) -> Pos2 {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0)
}

/// Step until the viewer's frame has rendered (frames render on background threads).
fn settle(h: &mut Harness<'_, EffectcraftApp>) {
    for _ in 0..600 {
        h.step();
        if h.state().frames.inflight() == 0 && h.state().frames.last_ms.lock().map(|v| *v > 0.0).unwrap_or(false) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    h.run_steps(2);
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
    let p = center(h, id);
    click_at(h, p);
}

fn open_layer(h: &mut Harness<'_, EffectcraftApp>, id: u64) {
    h.state_mut().ui.timeline.open_layers.insert(id);
    h.run_steps(2);
}

#[test]
fn shape_contents_add_menu_adds_path_operations() {
    let mut s = session();
    let l = s.execute("layer.newShape", json!({"kind": "ellipse"})).unwrap()["layer"].as_u64().unwrap();
    let contents = s.active_comp().unwrap().layer(LayerId(l)).unwrap().props.sub("contents").unwrap().uid;
    let mut h = harness(s);
    open_layer(&mut h, l);
    click(&mut h, &format!("timeline.group.{contents}.add"));
    let entry = h.get_by_label("Trim Paths").rect();
    assert!(entry.max.y <= 1000.0, "the whole menu fits in the window: {entry:?}");
    click_at(&mut h, entry.center());
    let layer = h.state().session.active_comp().unwrap().layer(LayerId(l)).unwrap().clone();
    let items: Vec<&str> = layer.props.sub("contents").unwrap().groups().map(|g| g.match_id.as_str()).collect();
    assert_eq!(items, ["group", "trim"]);
}

#[test]
fn shape_contents_add_menu_adds_wiggle_transform() {
    let mut s = session();
    let l = s.execute("layer.newShape", json!({"kind": "ellipse"})).unwrap()["layer"].as_u64().unwrap();
    let contents = s.active_comp().unwrap().layer(LayerId(l)).unwrap().props.sub("contents").unwrap().uid;
    let mut h = harness(s);
    open_layer(&mut h, l);
    click(&mut h, &format!("timeline.group.{contents}.add"));
    assert!(h.get_by_label("Zig Zag").rect().max.y <= 1000.0, "the whole menu fits in the window");
    let entry = h.get_by_label("Wiggle Transform").rect();
    click_at(&mut h, entry.center());
    let layer = h.state().session.active_comp().unwrap().layer(LayerId(l)).unwrap().clone();
    let items: Vec<&str> = layer.props.sub("contents").unwrap().groups().map(|g| g.match_id.as_str()).collect();
    assert_eq!(items, ["group", "wiggler"]);
}

/// A shape group's inner Contents is elided in After Effects: opening the group lists its items
/// (here the path) directly, with no "Contents" row between them.
#[test]
fn shape_group_contents_has_no_inner_contents_row() {
    let mut s = session();
    let l = s.execute("layer.newShape", json!({"kind": "rect"})).unwrap()["layer"].as_u64().unwrap();
    let layer = s.active_comp().unwrap().layer(LayerId(l)).unwrap().clone();
    let contents = layer.props.sub("contents").unwrap().clone();
    let group = contents.groups().next().unwrap().clone();
    let inner = group.sub("contents").unwrap().clone();
    let item = inner.groups().next().unwrap().clone();
    let mut h = harness(s);
    h.state_mut().ui.timeline.open_layers.insert(l);
    h.state_mut().ui.timeline.open_groups.insert(contents.uid);
    h.state_mut().ui.timeline.open_groups.insert(group.uid);
    h.run_steps(2);
    let name = |uid: u64| format!("timeline.group.{uid}.name");
    assert!(h.state().auto.find(&name(contents.uid)).is_some(), "the layer's Contents row");
    assert!(h.state().auto.find(&name(group.uid)).is_some(), "the shape group row");
    assert!(h.state().auto.find(&name(inner.uid)).is_none(), "no inner Contents row");
    assert!(h.state().auto.find(&name(item.uid)).is_some(), "the path lists directly under the group");
}

#[test]
fn classic_3d_shape_layers_offer_change_renderer() {
    let mut s = session();
    let l = s.execute("layer.newShape", json!({"kind": "rect"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layers": [l], "switch": "threeD", "value": true})).unwrap();
    let mut h = harness(s);
    open_layer(&mut h, l);
    click(&mut h, &format!("timeline.layer.{l}.changeRenderer"));
    assert_eq!(h.state().dialog, Some(Dialog::CompSettings));
    assert!(h.state().auto.find("dialog.comp.renderer").is_some(), "on the 3D Renderer tab");
    // Advanced 3D shows the extrusion options instead.
    h.state_mut().dialog = None;
    h.state_mut().session.execute("comp.renderer", json!({"renderer": "advanced3d"})).unwrap();
    h.run_steps(3);
    assert!(h.state().auto.find(&format!("timeline.layer.{l}.changeRenderer")).is_none());
}

#[test]
fn add_position_key_reveals_position() {
    let mut s = session();
    let l = s.execute("layer.newSolid", json!({"color": "#808080"})).unwrap()["layer"].as_u64().unwrap();
    let mut h = harness(s);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "timeline.keyAt.position", json!({})).unwrap();
    h.run_steps(2);
    let app = h.state();
    let position = app.session.active_comp().unwrap().layer(LayerId(l)).unwrap().props.prop("transform/position").unwrap().clone();
    assert_eq!(position.keys.len(), 1);
    assert_eq!(app.ui.timeline.layer_reveal.get(&l), Some(&vec!["position".to_string()]));
    assert!(app.auto.find(&format!("timeline.prop.{}.value.0", position.uid)).is_some(), "the Position row shows");
}

#[test]
fn clicking_below_the_layers_deselects_them() {
    let mut s = session();
    let l = s.execute("layer.newSolid", json!({"color": "#808080"})).unwrap()["layer"].as_u64().unwrap();
    let mut h = harness(s);
    assert_eq!(h.state().session.state.selected_layers, vec![LayerId(l)]);
    // In the time graph, below the only layer.
    let bar = center(&h, &format!("timeline.layer.{l}.bar"));
    click_at(&mut h, bar + egui::vec2(0.0, 80.0));
    assert!(h.state().session.state.selected_layers.is_empty());
}

#[test]
fn mask_feather_values_are_linked_and_not_negative() {
    let mut s = session();
    let l = s.execute("layer.newSolid", json!({"color": "#808080"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.addMask", json!({"layer": l, "shape": "rect"})).unwrap();
    let feather = |h: &Harness<'_, EffectcraftApp>| {
        let p = h.state().session.active_comp().unwrap().layer(LayerId(l)).unwrap().props.prop("masks/#1/feather").unwrap().clone();
        (p.uid, p.value.components())
    };
    let mut h = harness(s);
    h.state_mut().ui.timeline.open_layers.insert(l);
    h.state_mut().ui.timeline.layer_reveal.insert(l, vec!["feather".into()]);
    h.run_steps(2);
    let (uid, _) = feather(&h);
    let type_value = |h: &mut Harness<'_, EffectcraftApp>, d: usize, v: &str| {
        click(h, &format!("timeline.prop.{uid}.value.{d}"));
        h.input_mut().events.push(Event::Text(v.into()));
        h.step();
        h.input_mut().events.push(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
        h.run_steps(3);
    };
    type_value(&mut h, 0, "12");
    assert_eq!(feather(&h).1, vec![12.0, 12.0], "linked by default");
    type_value(&mut h, 1, "-5");
    assert_eq!(feather(&h).1, vec![0.0, 0.0], "never negative");
    click(&mut h, &format!("timeline.prop.{uid}.link"));
    type_value(&mut h, 1, "7");
    assert_eq!(feather(&h).1, vec![0.0, 7.0], "unlinked");
}

/// #2489: Rectangle and Ellipse Path Size get the Constrain Proportions chain, off by default:
/// unlinked, W moves alone; linked, H follows W in proportion. Scale on the same layer still starts
/// linked.
#[test]
fn shape_size_chain_starts_unlinked_and_keeps_the_ratio_when_linked() {
    let mut s = session();
    let l = s.execute("layer.newShape", json!({"kind": "rect", "size": [100, 50]})).unwrap()["layer"].as_u64().unwrap();
    let o = s.execute("layer.newShape", json!({"kind": "ellipse", "size": [60, 30]})).unwrap()["layer"].as_u64().unwrap();
    let uid = |s: &Session, l: u64, path: &str| s.active_comp().unwrap().layer(LayerId(l)).unwrap().props.prop(path).unwrap().uid;
    let (size, oval, scale) =
        (uid(&s, l, "contents/group/contents/rect/size"), uid(&s, o, "contents/group/contents/ellipse/size"), uid(&s, l, "transform/scale"));
    let mut h = harness(s);
    for (layer, props) in [(l, vec![size, scale]), (o, vec![oval])] {
        h.state_mut().ui.timeline.open_layers.insert(layer);
        h.state_mut().ui.timeline.layer_reveal.insert(layer, vec!["props".into()]);
        h.state_mut().ui.timeline.reveal_props.extend(props);
    }
    h.run_steps(2);
    let value = |h: &Harness<'_, EffectcraftApp>, l: u64, uid: u64| {
        h.state().session.active_comp().unwrap().layer(LayerId(l)).unwrap().props.prop(&format!("@{uid}")).unwrap().value.components()
    };
    let type_value = |h: &mut Harness<'_, EffectcraftApp>, uid: u64, d: usize, v: &str| {
        click(h, &format!("timeline.prop.{uid}.value.{d}"));
        h.input_mut().events.push(Event::Text(v.into()));
        h.step();
        h.input_mut().events.push(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
        h.run_steps(3);
    };
    type_value(&mut h, size, 0, "80");
    assert_eq!(value(&h, l, size), vec![80.0, 50.0], "a new rectangle's Size starts unlinked");
    type_value(&mut h, oval, 1, "40");
    assert_eq!(value(&h, o, oval), vec![60.0, 40.0], "a new ellipse's Size starts unlinked");
    click(&mut h, &format!("timeline.prop.{size}.link"));
    type_value(&mut h, size, 0, "160");
    assert_eq!(value(&h, l, size), vec![160.0, 100.0], "linked, H follows W in proportion");
    click(&mut h, &format!("timeline.prop.{size}.link"));
    type_value(&mut h, size, 0, "40");
    assert_eq!(value(&h, l, size), vec![40.0, 100.0], "unlinked again, H stays");
    // The ellipse's Size links the same way: 60x40 linked, W to 90 makes H 60.
    click(&mut h, &format!("timeline.prop.{oval}.link"));
    type_value(&mut h, oval, 0, "90");
    assert_eq!(value(&h, o, oval), vec![90.0, 60.0], "linked, the ellipse keeps its ratio too");
    type_value(&mut h, scale, 0, "50");
    assert_eq!(value(&h, l, scale)[..2], [50.0, 50.0], "Scale still starts linked");
}

/// Two shape layers, "Rect" (Rectangle 1) and "Oval" (Ellipse 1, selected): (harness with Rect's
/// Contents open, Rect, Oval, Rectangle 1's uid).
fn two_shape_layers() -> (Harness<'static, EffectcraftApp>, u64, u64, u64) {
    let mut s = session();
    let a = s.execute("layer.newShape", json!({"kind": "rect", "name": "Rect"})).unwrap()["layer"].as_u64().unwrap();
    let b = s.execute("layer.newShape", json!({"kind": "ellipse", "name": "Oval"})).unwrap()["layer"].as_u64().unwrap();
    let contents = s.active_comp().unwrap().layer(LayerId(a)).unwrap().props.sub("contents").unwrap().clone();
    let rect = contents.groups().next().unwrap().uid;
    let mut h = harness(s);
    h.state_mut().ui.timeline.open_groups.insert(contents.uid);
    open_layer(&mut h, a);
    (h, a, b, rect)
}

fn contents_names(h: &Harness<'_, EffectcraftApp>, l: u64) -> Vec<String> {
    h.state().session.active_comp().unwrap().layer(LayerId(l)).unwrap().props.sub("contents").unwrap().groups().map(|g| g.name.clone()).collect()
}

/// #959: every item in a shape layer's Contents has an eye switch, as in After Effects; it turns
/// the item off (the renderer skips it), undoably. The group's Transform has none.
#[test]
fn shape_contents_items_have_eye_switches() {
    let (mut h, a, _, rect) = two_shape_layers();
    let enabled = |h: &Harness<'_, EffectcraftApp>| {
        h.state().session.active_comp().unwrap().layer(LayerId(a)).unwrap().props.sub("contents").unwrap().find_group(rect).unwrap().enabled
    };
    let eye = format!("timeline.group.{rect}.eye");
    assert!(enabled(&h));
    click(&mut h, &eye);
    assert!(!enabled(&h), "the eye turns Rectangle 1 off");
    click(&mut h, &eye);
    assert!(enabled(&h), "and on again");
    click(&mut h, &eye);
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert!(enabled(&h), "one undo step");
    // Inside the group: its path and fill have eyes, its Transform doesn't.
    h.state_mut().ui.timeline.open_groups.insert(rect);
    h.run_steps(3);
    let group = h.state().session.active_comp().unwrap().layer(LayerId(a)).unwrap().props.find_group(rect).unwrap().clone();
    let transform = group.sub("transform").expect("a shape group has a Transform").uid;
    assert!(h.state().auto.find(&format!("timeline.group.{transform}.eye")).is_none(), "no eye on Transform");
    let items = group.sub("contents").expect("the group's items").groups().map(|g| g.uid).collect::<Vec<_>>();
    assert!(!items.is_empty() && items.iter().all(|u| h.state().auto.find(&format!("timeline.group.{u}.eye")).is_some()), "every item has an eye");
}

#[test]
fn ctrl_c_and_ctrl_v_copy_a_shape_group_into_another_shape_layer() {
    let (mut h, a, b, rect) = two_shape_layers();
    click(&mut h, &format!("timeline.group.{rect}.name"));
    assert_eq!(h.state().session.state.selected_props, vec![(LayerId(a), rect)]);
    h.input_mut().events.push(Event::Copy);
    h.step();
    click(&mut h, &format!("timeline.layer.{b}.row"));
    assert_eq!(h.state().session.state.selected_layers, vec![LayerId(b)]);
    h.input_mut().events.push(Event::Paste("EffectCraft: 1 shape item".into()));
    h.run_steps(3);
    assert_eq!(h.state().session.active_comp().unwrap().layers.len(), 2, "no new layer");
    assert_eq!(contents_names(&h, b), ["Rectangle 1", "Ellipse 1"]);
    assert_eq!(contents_names(&h, a), ["Rectangle 1"]);
}

#[test]
fn ctrl_d_duplicates_the_selected_shape_group_in_its_layer() {
    let (mut h, a, _, rect) = two_shape_layers();
    click(&mut h, &format!("timeline.group.{rect}.name"));
    h.input_mut().events.push(Event::Key { key: Key::D, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND });
    h.run_steps(3);
    assert_eq!(h.state().session.active_comp().unwrap().layers.len(), 2, "no new layer");
    assert_eq!(contents_names(&h, a), ["Rectangle 2", "Rectangle 1"]);
}

#[test]
fn toolbar_stroke_edits_the_selected_shape_layer() {
    let mut s = session();
    let l = s.execute("layer.newShape", json!({"kind": "rect", "fill": "#ff0000"})).unwrap()["layer"].as_u64().unwrap();
    let mut h = harness(s);
    // The Selection tool is active: the options show for the selected shape layer.
    click(&mut h, "header.strokeWidth");
    h.input_mut().events.push(Event::Text("4".into()));
    h.step();
    h.input_mut().events.push(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    let layer = h.state().session.active_comp().unwrap().layer(LayerId(l)).unwrap().clone();
    let width = layer.props.prop("contents/group/contents/stroke/width").map(|p| p.value.as_f64());
    assert_eq!(width, Some(4.0), "a stroke was added with the toolbar's width");
    assert_eq!(h.state().session.state.shape_tool.stroke_width, 4.0);
}

/// Issue #633: a colour property in the Timeline (a shape's Fill Color) has an eyedropper after
/// its swatch, as in After Effects: click it, then click in the Composition panel to sample.
#[test]
fn timeline_colour_eyedropper_samples_the_comp() {
    let mut s = session();
    s.execute("layer.newSolid", json!({"name": "Back", "color": "#20c040"})).unwrap();
    let l = s.execute("layer.newShape", json!({"kind": "rect", "fill": [1.0, 0.0, 0.0], "size": [100.0, 60.0]})).unwrap()["layer"].as_u64().unwrap();
    let layer = s.active_comp().unwrap().layer(LayerId(l)).unwrap().clone();
    let mut fill = None;
    layer.props.walk("", &mut |_, p| {
        if fill.is_none() && matches!(p.value, effectcraft_engine::keyframe::Value::Color(_)) {
            fill = Some(p.uid);
        }
    });
    let fill = fill.expect("a fill colour");
    fn groups(g: &effectcraft_engine::project::PropGroup, out: &mut Vec<u64>) {
        for c in g.groups() {
            // The rectangle's path rows would push Fill below the rows the Timeline fits; the
            // eyedropper only needs Fill, so leave that group collapsed.
            if c.name == "Rectangle Path 1" {
                continue;
            }
            out.push(c.uid);
            groups(c, out);
        }
    }
    let mut open = vec![];
    groups(&layer.props, &mut open);
    let mut h = harness(s);
    h.state_mut().ui.timeline.open_groups.extend(open);
    open_layer(&mut h, l);
    settle(&mut h);
    click(&mut h, &format!("timeline.prop.{fill}.eyedropper"));
    assert_eq!(h.state().ui.fx_pick.as_ref().map(|p| (p.prop, p.kind.as_str())), Some((fill, "color")), "the eyedropper waits for a viewer click");
    let pos = effectcraft_ui_egui::panels::viewer::comp_to_screen(&h.ctx, [6.0, 6.0]).unwrap();
    click_at(&mut h, pos);
    assert!(h.state().ui.fx_pick.is_none(), "the pick is used up");
    let layer = h.state().session.active_comp().unwrap().layer(LayerId(l)).unwrap().clone();
    let mut got = None;
    layer.props.walk("", &mut |_, p| {
        if p.uid == fill {
            got = Some(p.value.clone());
        }
    });
    let Some(effectcraft_engine::keyframe::Value::Color(c)) = got else { panic!("{got:?}") };
    let want = [0x20 as f64 / 255.0, 0xc0 as f64 / 255.0, 0x40 as f64 / 255.0];
    for k in 0..3 {
        assert!((c[k] - want[k]).abs() < 0.02, "{c:?} vs {want:?}");
    }
}

/// Ctrl+V with SVG markup on the system clipboard pastes it as a shape layer (After Effects
/// 26.3), even with layers on the app's clipboard: copying in the app replaces the system
/// clipboard, so the SVG was copied later (#738).
#[test]
fn ctrl_v_pastes_svg_markup_as_a_shape_layer() {
    let (mut h, a, b, _) = two_shape_layers();
    click(&mut h, &format!("timeline.layer.{b}.row"));
    h.input_mut().events.push(Event::Copy);
    h.step();
    assert_eq!(h.state().session.state.clipboard.len(), 1, "a layer on the app's clipboard");
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><circle cx="10" cy="10" r="8" fill="#00ff00"/><rect x="20" width="20" height="20"/></svg>"##;
    h.input_mut().events.push(Event::Paste(svg.into()));
    h.run_steps(3);
    let c = h.state().session.active_comp().unwrap().clone();
    assert_eq!(c.layers.len(), 3, "{:?}", h.state().ui.status);
    let top = &c.layers[0];
    assert!(top.id != LayerId(a) && top.id != LayerId(b));
    assert_eq!(top.name, "Shape Layer 3");
    assert_eq!(top.props.sub("contents").unwrap().groups().count(), 2);
    assert_eq!(h.state().session.state.selected_layers, vec![top.id]);
    // The app's own clipboard note pastes the copied layer as before.
    h.input_mut().events.push(Event::Paste("EffectCraft: 1 layer".into()));
    h.run_steps(3);
    let c = h.state().session.active_comp().unwrap();
    assert_eq!(c.layers.len(), 4);
    assert_eq!(c.layers[0].name, "Oval 2");
}

fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2, cancel: bool) {
    h.input_mut().events.push(Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for k in 1..=8 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (k as f32 / 8.0)));
        h.step();
    }
    if cancel {
        h.input_mut().events.push(Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

/// #2210: an effect's row drags to a new place in the layer's effect stack, as in After Effects
/// (not a marquee); Escape cancels the drag.
#[test]
fn effects_drag_to_a_new_place_in_the_timeline() {
    let mut s = session();
    let l = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
    for fx in ["Gaussian Blur", "Invert", "Tint"] {
        s.execute("effect.apply", json!({"layers": [l], "effect": fx})).unwrap();
    }
    let effects = s.active_comp().unwrap().layer(LayerId(l)).unwrap().props.sub("effects").unwrap().clone();
    let uids: Vec<u64> = effects.groups().map(|g| g.uid).collect();
    let mut h = harness(s);
    h.state_mut().ui.timeline.open_groups.insert(effects.uid);
    open_layer(&mut h, l);
    let names = |h: &Harness<'_, EffectcraftApp>| -> Vec<String> {
        h.state().session.active_comp().unwrap().layer(LayerId(l)).unwrap().props.sub("effects").unwrap().groups().map(|g| g.name.clone()).collect()
    };
    let row = |h: &Harness<'_, EffectcraftApp>, uid: u64| {
        let e = h.state().auto.find(&format!("timeline.group.{uid}.name")).unwrap_or_else(|| panic!("no row for {uid}"));
        egui::Rect::from_min_size(pos2(e.rect[0], e.rect[1]), egui::vec2(e.rect[2], e.rect[3]))
    };
    // Tint above Gaussian Blur.
    let (tint, blur) = (row(&h, uids[2]), row(&h, uids[0]));
    drag(&mut h, tint.center(), pos2(blur.center().x, blur.min.y + 2.0), true);
    assert_eq!(names(&h), ["Gaussian Blur", "Invert", "Tint"], "Escape cancels");
    drag(&mut h, tint.center(), pos2(blur.center().x, blur.min.y + 2.0), false);
    assert_eq!(names(&h), ["Tint", "Gaussian Blur", "Invert"]);
    // Gaussian Blur (now second) below Invert, the last.
    let (blur, invert) = (row(&h, uids[0]), row(&h, uids[1]));
    drag(&mut h, blur.center(), pos2(invert.center().x, invert.max.y - 2.0), false);
    assert_eq!(names(&h), ["Tint", "Invert", "Gaussian Blur"]);
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(names(&h), ["Tint", "Gaussian Blur", "Invert"], "one undo step per drop");
}

/// #2552: Shift-clicking effect rows in the Timeline adds them to the selection (and a second
/// Shift-click takes one out), so one animation preset can hold several effects.
#[test]
fn shift_click_selects_several_effects_for_a_preset() {
    let mut s = session();
    let l = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
    for fx in ["Gaussian Blur", "Invert"] {
        s.execute("effect.apply", json!({"layers": [l], "effect": fx})).unwrap();
    }
    let effects = s.active_comp().unwrap().layer(LayerId(l)).unwrap().props.sub("effects").unwrap().clone();
    let uids: Vec<u64> = effects.groups().map(|g| g.uid).collect();
    let mut h = harness(s);
    h.state_mut().ui.timeline.open_groups.insert(effects.uid);
    open_layer(&mut h, l);
    let shift_click = |h: &mut Harness<'_, EffectcraftApp>, uid: u64| {
        let p = center(h, &format!("timeline.group.{uid}.name"));
        h.input_mut().events.push(Event::ModifiersChanged(Modifiers::SHIFT));
        h.input_mut().events.push(Event::PointerMoved(p));
        h.step();
        for pressed in [true, false] {
            h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::SHIFT });
            h.step();
        }
        h.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
        h.run_steps(2);
    };
    let selected = |h: &Harness<'_, EffectcraftApp>| h.state().session.state.selected_props.iter().map(|(_, u)| *u).collect::<Vec<_>>();
    click(&mut h, &format!("timeline.group.{}.name", uids[0]));
    assert_eq!(selected(&h), [uids[0]]);
    shift_click(&mut h, uids[1]);
    assert_eq!(selected(&h), [uids[0], uids[1]], "Shift adds the second effect");
    let path = std::env::temp_dir().join(format!("ec-2552-{}.ffx", std::process::id()));
    let r = h.state_mut().session.execute("anim.savePreset", json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!(r["effects"], 2, "{r}");
    let _ = std::fs::remove_file(&path);
    shift_click(&mut h, uids[1]);
    assert_eq!(selected(&h), [uids[0]], "a second Shift-click takes it out");
}

/// #2513: the Text group's Animate menu adds Animator 1 with the chosen property (and a Range
/// Selector), and the animator's Add menu adds another property to it.
#[test]
fn text_animate_menu_adds_an_animator_with_the_property() {
    let mut s = session();
    let l = s.execute("layer.newText", json!({"text": "Hello"})).unwrap()["layer"].as_u64().unwrap();
    let text = s.active_comp().unwrap().layer(LayerId(l)).unwrap().props.sub("text").unwrap().uid;
    let mut h = harness(s);
    h.state_mut().ui.timeline.open_groups.insert(text);
    open_layer(&mut h, l);
    // The Properties panel's Position row has the same accessible name: take the menu's new entry.
    let shown: Vec<_> = h.query_all_by_label("Position").map(|n| n.rect()).collect();
    click(&mut h, &format!("timeline.group.{text}.animate"));
    let entry = h.query_all_by_label("Position").map(|n| n.rect()).find(|r| !shown.contains(r)).expect("the Animate menu's Position entry");
    click_at(&mut h, entry.center());
    let animators = |h: &Harness<'_, EffectcraftApp>| {
        h.state().session.active_comp().unwrap().layer(LayerId(l)).unwrap().props.sub("text").unwrap().sub("animators").cloned()
    };
    let anims = animators(&h).expect("an Animators group");
    let a = anims.groups().next().expect("Animator 1");
    let has = |a: &effectcraft_engine::project::PropGroup, m: &str| a.sub("properties").is_some_and(|p| p.children.iter().any(|c| c.match_id() == m));
    assert!(has(a, "position"), "Position was added: {:?}", a.sub("properties"));
    assert!(a.sub("selectors").is_some_and(|s| !s.children.is_empty()), "with a Range Selector");
    // The animator's Add menu adds a property to it.
    h.state_mut().ui.timeline.open_groups.insert(anims.uid);
    h.state_mut().ui.timeline.open_groups.insert(a.uid);
    h.run_steps(2);
    click(&mut h, &format!("timeline.group.{}.add", a.uid));
    let entry = h.get_by_label("Property: Opacity").rect();
    click_at(&mut h, entry.center());
    let a = animators(&h).unwrap().groups().next().unwrap().clone();
    assert!(has(&a, "opacity"), "Opacity was added: {:?}", a.sub("properties"));
}
