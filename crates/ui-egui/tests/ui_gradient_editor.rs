//! The Gradient Editor (#612): the Timeline's "Edit Gradient…" opens it on a gradient property
//! (here a shape layer's Gradient Fill ▸ Colors). OK sets the edited gradient with one undo step
//! at the composition time it opened at (offset and stretched layers too); OK with no change, Cancel
//! and Escape change nothing; on an animated gradient only the keyframe at that time changes, with
//! a warning when its stop counts no longer match a neighbour's.

use effectcraft_engine::Session;
use effectcraft_engine::keyframe::{Gradient, Value};
use effectcraft_engine::project::{LayerId, PropGroup};
use effectcraft_engine::time::Tick;
use effectcraft_ui_egui::panels::gradient_editor::Track;
use effectcraft_ui_egui::{Dialog, EffectcraftApp};
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

const WARNING: &str = "These adjacent keyframes cannot interpolate because their stop counts differ.";

/// The uids from the layer's root down to the Gradient Fill group, and its Colors property.
fn gradient_fill(g: &PropGroup) -> Option<(Vec<u64>, u64)> {
    for x in g.groups() {
        if x.match_id == "gfill" {
            return Some((vec![x.uid], x.get("colors")?.uid));
        }
        if let Some((mut path, colors)) = gradient_fill(x) {
            path.insert(0, x.uid);
            return Some((path, colors));
        }
    }
    None
}

/// A 20 s comp with a rectangle shape layer holding a Gradient Fill, its rows open in the
/// Timeline. `timing` runs on the layer first (start, stretch). Returns (harness, layer, Colors uid).
fn setup(timing: &[(&str, serde_json::Value)]) -> (Harness<'static, EffectcraftApp>, u64, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 20})).unwrap();
    let l = s.execute("layer.newShape", json!({"kind": "rect"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.addShapeItem", json!({"layer": l, "kind": "gfill"})).unwrap();
    for (cmd, p) in timing {
        let mut p = p.clone();
        p["layers"] = json!([l]);
        s.execute(cmd, p).unwrap();
    }
    let (groups, colors) = gradient_fill(&s.active_comp().unwrap().layer(LayerId(l)).unwrap().props).expect("a Gradient Fill");
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.state_mut().ui.timeline.open_layers.insert(l);
    h.state_mut().ui.timeline.open_groups.extend(groups);
    h.run_steps(3);
    (h, l, colors)
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> [f32; 4] {
    h.state().auto.find(id).unwrap_or_else(|| panic!("no automation id {id}")).rect
}

fn center(h: &Harness<'_, EffectcraftApp>, id: &str) -> Pos2 {
    let r = rect(h, id);
    pos2(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0)
}

/// The point at `f` (0..1) along the bar, at the height of automation id `row`.
fn along(h: &Harness<'_, EffectcraftApp>, row: &str, f: f32) -> Pos2 {
    let bar = rect(h, "dialog.gradient.bar");
    pos2(bar[0] + bar[2] * f, center(h, row).y)
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

fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for k in 1..=6 {
        let f = k as f32 / 6.0;
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * f));
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn key(h: &mut Harness<'_, EffectcraftApp>, k: Key) {
    h.input_mut().events.push(Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.input_mut().events.push(Event::Key { key: k, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn colors_prop(h: &Harness<'_, EffectcraftApp>, l: u64, uid: u64) -> effectcraft_engine::project::Property {
    h.state().session.active_comp().unwrap().layer(LayerId(l)).unwrap().props.find(uid).unwrap().clone()
}

fn gradient(v: &Value) -> Gradient {
    match v {
        Value::Gradient(g) => g.clone(),
        other => panic!("not a gradient: {other:?}"),
    }
}

fn open_editor(h: &mut Harness<'_, EffectcraftApp>, colors: u64) {
    let id = format!("timeline.prop.{colors}.value");
    assert!(h.state().auto.find(&id).is_some(), "no automation id {id}: the Timeline's gradient value isn't clickable");
    click(h, &id);
    h.run_steps(2);
}

fn set_time(h: &mut Harness<'_, EffectcraftApp>, t: f64) {
    h.state_mut().session.execute("time.set", json!({"time": t})).unwrap();
    h.run_steps(2);
}

fn add_key(h: &mut Harness<'_, EffectcraftApp>, l: u64, colors: u64, comp_t: f64, g: &Gradient) {
    h.state_mut()
        .session
        .execute("prop.addKey", json!({"layer": l, "prop": colors, "time": comp_t, "timeBase": "comp", "value": Value::Gradient(g.clone()).to_json()}))
        .unwrap();
}

fn near(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
}

/// The shared colour picker is showing (its colour field is in the accessibility tree).
fn picker_open(h: &Harness<'_, EffectcraftApp>) -> bool {
    h.query_by_label("Color field").is_some()
}

/// Type `hex` into the open picker's Hex field, over the code it shows.
fn type_hex(h: &mut Harness<'_, EffectcraftApp>, hex: &str) {
    let at = h.get_by_role_and_label(Role::TextInput, "#").rect().center();
    click_at(h, at);
    for pressed in [true, false] {
        h.input_mut().events.push(Event::Key { key: Key::A, physical_key: None, pressed, repeat: false, modifiers: Modifiers::COMMAND });
    }
    h.step();
    h.input_mut().events.push(Event::Text(hex.into()));
    h.run_steps(3);
}

fn stop_color(h: &Harness<'_, EffectcraftApp>, id: u64) -> [f32; 4] {
    h.state().gradient_draft().colors.iter().find(|s| s.id == id).expect("the stop").value
}

/// The numbers the editor's fields show (their accessibility values).
fn shown_numbers(h: &Harness<'_, EffectcraftApp>) -> Vec<f64> {
    h.query_all_by(|n| n.numeric_value().is_some()).filter_map(|n| n.accesskit_node().numeric_value()).collect()
}

fn undo_len(h: &Harness<'_, EffectcraftApp>) -> usize {
    h.state().session.history.undo.len()
}

#[test]
fn stop_edits_ok_sets_the_gradient_in_one_undo_step() {
    let (mut h, l, colors) = setup(&[]);
    let before = colors_prop(&h, l, colors).value.clone();
    let undo = undo_len(&h);
    open_editor(&mut h, colors);
    assert_eq!(h.state().dialog, Some(Dialog::GradientEditor));
    // Add a colour stop a quarter along, recolour it, and drag the right opacity stop to the middle.
    let p = along(&h, "dialog.gradient.addColor", 0.25);
    click_at(&mut h, p);
    let (track, id) = h.state().gradient_draft().selected.expect("the new stop is selected");
    assert_eq!(track, Track::Color);
    let sampled = stop_color(&h, id);
    // The swatch opens the shared colour picker over the modal editor. A click in its colour field
    // reaches it (a modal blocks the layers below it), and so does typing a hex code.
    click(&mut h, "dialog.gradient.color");
    assert!(picker_open(&h), "the colour picker opened");
    let field = h.get_by_label("Color field").rect().center();
    click_at(&mut h, field + egui::vec2(20.0, -20.0));
    assert_ne!(stop_color(&h, id), sampled, "a click in the picker's colour field changed the stop");
    type_hex(&mut h, "336699");
    let picked = stop_color(&h, id);
    assert!(near(picked, [0.2, 0.4, 0.6, sampled[3]]), "the hex code's colour, and the stop kept its alpha: {picked:?}");
    // Clicking outside the picker keeps the colour and closes it; the editor stays.
    let title = h.get_by_label("Gradient Editor").rect().center();
    click_at(&mut h, title);
    assert!(!picker_open(&h), "the picker closed");
    assert_eq!(h.state().dialog, Some(Dialog::GradientEditor));
    assert_eq!(stop_color(&h, id), picked);
    let from = center(&h, "dialog.gradient.opacityStop.1");
    let to = along(&h, "dialog.gradient.opacityStop.1", 0.5);
    drag(&mut h, from, to);
    assert_eq!(undo_len(&h), undo, "nothing is set while editing");
    click(&mut h, "dialog.gradient.ok");
    assert_eq!(h.state().dialog, None);
    let g = gradient(&colors_prop(&h, l, colors).value);
    assert_eq!(g.colors.len(), 3, "{g:?}");
    assert!((g.colors[1].0 - 0.25).abs() < 0.02 && g.colors[1].1 == picked, "{g:?}");
    assert!((g.opacities[1].0 - 0.5).abs() < 0.02, "{g:?}");
    assert_eq!(undo_len(&h), undo + 1, "one undo step");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(colors_prop(&h, l, colors).value, before, "one undo restores it");
}

#[test]
fn cancel_and_escape_change_nothing() {
    for cancel in ["button", "escape"] {
        let (mut h, _, colors) = setup(&[]);
        let project = h.state().session.project.clone();
        let undo = undo_len(&h);
        open_editor(&mut h, colors);
        let p = along(&h, "dialog.gradient.addOpacity", 0.4);
        click_at(&mut h, p);
        assert_eq!(h.state().gradient_draft().opacities.len(), 3);
        if cancel == "button" {
            click(&mut h, "dialog.gradient.cancel");
        } else {
            key(&mut h, Key::Escape);
        }
        assert_eq!(h.state().dialog, None, "{cancel}");
        assert_eq!(h.state().session.project, project, "{cancel} leaves the project as it was");
        assert_eq!(undo_len(&h), undo);
    }
}

/// OK without a change adds no keyframe between two (prop.set would add one).
#[test]
fn ok_without_changes_adds_no_keyframe() {
    let (mut h, l, colors) = setup(&[]);
    add_key(&mut h, l, colors, 1.0, &Gradient::default());
    add_key(&mut h, l, colors, 3.0, &Gradient::default());
    set_time(&mut h, 2.0);
    let undo = undo_len(&h);
    open_editor(&mut h, colors);
    click(&mut h, "dialog.gradient.ok");
    assert_eq!(h.state().dialog, None);
    assert_eq!(colors_prop(&h, l, colors).keys.len(), 2, "still the two keyframes");
    assert_eq!(undo_len(&h), undo, "no undo step");
}

/// On an animated gradient OK sets the keyframe at the edit time (adding one between keys) and no
/// other.
#[test]
fn animated_edits_change_only_the_keyframe_at_the_edit_time() {
    let (mut h, l, colors) = setup(&[]);
    let red = Gradient { colors: vec![(0.0, [1.0, 0.0, 0.0, 1.0]), (1.0, [0.0, 0.0, 0.0, 1.0])], ..Gradient::default() };
    add_key(&mut h, l, colors, 1.0, &Gradient::default());
    add_key(&mut h, l, colors, 3.0, &red);
    let first = colors_prop(&h, l, colors).keys[0].clone();
    for (t, keys) in [(3.0, 2), (2.0, 3)] {
        set_time(&mut h, t);
        open_editor(&mut h, colors);
        let d = &h.state().gradient_draft();
        let id = d.id_at(Track::Color, 1).unwrap();
        h.state_mut().gradient_draft_mut().set_rgb(id, [0.0, 0.0, 1.0]);
        click(&mut h, "dialog.gradient.ok");
        let pr = colors_prop(&h, l, colors);
        assert_eq!(pr.keys.len(), keys, "at {t} s");
        assert_eq!(pr.keys[0], first, "the keyframe at 1 s is untouched");
        let layer = h.state().session.active_comp().unwrap().layer(LayerId(l)).unwrap().clone();
        // The edit time snapped to the nearest frame (29.97 fps), as prop.set snaps it.
        let at = h.state().session.active_comp().unwrap().frame_rate.snap_nearest(Tick::from_seconds_f64(t)).seconds();
        let k = pr.keys.iter().find(|k| (layer.comp_time(k.time).seconds() - at).abs() < 1e-6).expect("a keyframe at the edit time");
        assert_eq!(gradient(&k.value).colors[1].1, [0.0, 0.0, 1.0, 1.0], "at {t} s");
    }
}

/// The key goes at the composition time the editor opened at, on a layer that starts late or is
/// stretched (layer time differs from composition time).
#[test]
fn offset_and_stretched_layers_get_the_keyframe_at_the_comp_time() {
    for timing in
        [vec![("layer.timing", json!({"start": 5.0}))], vec![("layer.timing", json!({"start": 5.0})), ("layer.timeStretch", json!({"percent": 200.0}))]]
    {
        let (mut h, l, colors) = setup(&timing);
        set_time(&mut h, 5.5);
        h.state_mut().session.execute("prop.toggleAnimation", json!({"layer": l, "prop": colors})).unwrap();
        set_time(&mut h, 6.0);
        open_editor(&mut h, colors);
        let p = along(&h, "dialog.gradient.addColor", 0.5);
        click_at(&mut h, p);
        click(&mut h, "dialog.gradient.ok");
        let layer = h.state().session.active_comp().unwrap().layer(LayerId(l)).unwrap().clone();
        let pr = layer.props.find(colors).unwrap();
        let comp_times: Vec<f64> = pr.keys.iter().map(|k| layer.comp_time(k.time).seconds()).collect();
        assert_eq!(pr.keys.len(), 2, "{timing:?}: {comp_times:?}");
        // Composition time 6 s snapped to the nearest frame (29.97 fps: 6.006 s), as prop.set snaps
        // it: the layer's start and stretch don't move it.
        let at = h.state().session.active_comp().unwrap().frame_rate.snap_nearest(Tick::from_seconds_f64(6.0)).seconds();
        let k = pr.keys.iter().find(|k| (layer.comp_time(k.time).seconds() - at).abs() < 1e-6);
        let k = k.unwrap_or_else(|| panic!("{timing:?}: no keyframe at comp time {at} s: {comp_times:?}"));
        assert_eq!(gradient(&k.value).colors.len(), 3, "{timing:?}");
    }
}

/// Adding a stop next to keyframes with fewer stops warns that they can't interpolate; it still
/// saves, the neighbour keeps its stops, and the animation holds until the edited keyframe.
#[test]
fn mismatched_stop_counts_warn_and_neighbours_stay() {
    let (mut h, l, colors) = setup(&[]);
    add_key(&mut h, l, colors, 1.0, &Gradient::default());
    add_key(&mut h, l, colors, 3.0, &Gradient::default());
    set_time(&mut h, 3.0);
    open_editor(&mut h, colors);
    assert!(h.query_by_label(WARNING).is_none(), "same counts: no warning");
    let p = along(&h, "dialog.gradient.addColor", 0.5);
    click_at(&mut h, p);
    assert!(h.query_by_label(WARNING).is_some(), "three colour stops next to a keyframe with two");
    click(&mut h, "dialog.gradient.ok");
    let pr = colors_prop(&h, l, colors);
    assert_eq!(pr.keys.len(), 2);
    assert_eq!(gradient(&pr.keys[0].value), Gradient::default(), "the neighbour keeps its stops");
    assert_eq!(gradient(&pr.keys[1].value).colors.len(), 3);
    let layer = h.state().session.active_comp().unwrap().layer(LayerId(l)).unwrap().clone();
    let mid = pr.value_at(layer.layer_time(Tick::from_seconds_f64(2.0)));
    assert_eq!(gradient(&mid), Gradient::default(), "it holds the first keyframe instead of blending");
}

/// Delete removes the selected stop (never a layer, and never the last stop of a track), but not
/// while typing in a field.
#[test]
fn delete_removes_the_selected_stop_but_not_the_last_or_while_typing() {
    let (mut h, _, colors) = setup(&[]);
    let layers = h.state().session.active_comp().unwrap().layers.len();
    open_editor(&mut h, colors);
    click(&mut h, "dialog.gradient.colorStop.0");
    key(&mut h, Key::Delete);
    assert_eq!(h.state().gradient_draft().colors.len(), 1, "Delete removed the selected stop");
    click(&mut h, "dialog.gradient.colorStop.0");
    key(&mut h, Key::Delete);
    click(&mut h, "dialog.gradient.delete");
    assert_eq!(h.state().gradient_draft().colors.len(), 1, "the last colour stop stays");
    assert_eq!(h.state().session.active_comp().unwrap().layers.len(), layers, "Delete never reaches the layer");
    assert_eq!(h.state().dialog, Some(Dialog::GradientEditor));
    // Typing in Location: Delete edits the text, not the stops.
    click(&mut h, "dialog.gradient.opacityStop.0");
    click(&mut h, "dialog.gradient.location");
    assert!(h.ctx.egui_wants_keyboard_input(), "the Location field takes the keyboard");
    key(&mut h, Key::Delete);
    assert_eq!(h.state().gradient_draft().opacities.len(), 2, "typing doesn't delete a stop");
}

/// One colour stop saves (the typed form; plain JSON needs two) and one undo brings both back.
#[test]
fn a_single_colour_stop_saves_and_undoes() {
    let (mut h, l, colors) = setup(&[]);
    let before = colors_prop(&h, l, colors).value.clone();
    let undo = undo_len(&h);
    open_editor(&mut h, colors);
    click(&mut h, "dialog.gradient.colorStop.1");
    click(&mut h, "dialog.gradient.delete");
    assert_eq!(h.state().gradient_draft().colors.len(), 1);
    click(&mut h, "dialog.gradient.ok");
    let g = gradient(&colors_prop(&h, l, colors).value);
    assert_eq!(g.colors, gradient(&before).colors[..1].to_vec(), "{g:?}");
    assert_eq!(g.opacities, gradient(&before).opacities, "opacity stops unchanged");
    assert_eq!(undo_len(&h), undo + 1);
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(colors_prop(&h, l, colors).value, before);
}

/// Typing a Location that moves a stop past its neighbour keeps the selection on that stop, and
/// in the same frame the Opacity field shows its value, not the neighbour's (now at its old index).
#[test]
fn location_moves_past_a_neighbour_and_the_fields_follow_the_stop() {
    let (mut h, _, colors) = setup(&[]);
    open_editor(&mut h, colors);
    // Opacity 100% at 0 and 40% at 1, plus a stop at 0.5 sampled from them: 70%.
    let right = h.state().gradient_draft().opacities[1].id;
    h.state_mut().gradient_draft_mut().set_opacity(right, 0.4);
    let p = along(&h, "dialog.gradient.addOpacity", 0.5);
    click_at(&mut h, p);
    let middle = h.state().gradient_draft().selected.expect("the new stop").1;
    let i = h.state().gradient_draft().index_of(Track::Opacity, middle).unwrap();
    assert!((h.state().gradient_draft().opacities[i].value - 0.7).abs() < 0.02, "it sampled 70%");
    // Select the 100% stop at 0 and type 80 into Location: it passes the stop at 0.5.
    let first = h.state().gradient_draft().opacities[0].id;
    click(&mut h, "dialog.gradient.opacityStop.0");
    click(&mut h, "dialog.gradient.location");
    assert!(h.ctx.egui_wants_keyboard_input(), "the Location field is being typed in");
    h.input_mut().events.push(Event::Key { key: Key::A, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND });
    h.input_mut().events.push(Event::Text("80".into()));
    h.input_mut().events.push(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.step();
    let d = h.state().gradient_draft();
    assert_eq!(d.selected, Some((Track::Opacity, first)), "the moved stop stays selected");
    assert_eq!(d.index_of(Track::Opacity, first), Some(1), "it is now after the stop at 0.5");
    // The frame that moved it: Opacity already shows this stop's 100%, never the neighbour's 70%
    // that is now at its old index. (Location itself was drawn before the move: 0 until the next.)
    let shown = shown_numbers(&h);
    assert!(shown.contains(&100.0), "this stop's 100%: {shown:?}");
    assert!(!shown.iter().any(|v| (v - 70.0).abs() < 1.0), "never the neighbour's 70%: {shown:?}");
    h.run_steps(2);
    let shown = shown_numbers(&h);
    assert!(shown.contains(&80.0) && shown.contains(&100.0), "Location 80 and Opacity 100%: {shown:?}");
    assert!(!shown.iter().any(|v| (v - 70.0).abs() < 1.0), "{shown:?}");
}

/// A layer deleted while the editor is open: OK reports it, without a crash.
#[test]
fn a_layer_deleted_while_editing_is_reported() {
    let (mut h, l, colors) = setup(&[]);
    open_editor(&mut h, colors);
    let p = along(&h, "dialog.gradient.addColor", 0.5);
    click_at(&mut h, p);
    h.state_mut().session.execute("edit.clear", json!({"layers": [l]})).unwrap();
    h.run_steps(2);
    click(&mut h, "dialog.gradient.ok");
    assert_eq!(h.state().dialog, None);
    assert!(!h.state().ui.status.is_empty(), "the failed set is reported");
}

/// `prop.editGradient` by id (agents, MCP, the control channel) is an error for a target that
/// isn't a gradient property, from the engine and through the UI, and opens nothing.
#[test]
fn edit_gradient_by_id_rejects_bad_targets() {
    let (mut h, l, colors) = setup(&[]);
    let ctx = h.ctx.clone();
    let opacity = h.state().session.active_comp().unwrap().layer(LayerId(l)).unwrap().props.prop("transform/opacity").unwrap().uid;
    for (p, err) in [(json!({"layer": 9999, "prop": colors}), "no layer"), (json!({"layer": l, "prop": opacity}), "not a gradient")] {
        let e = h.state_mut().session.execute_checked("prop.editGradient", p.clone()).expect_err("the engine refuses it");
        assert!(e.to_string().contains(err), "{p}: {e}");
        let e = effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "prop.editGradient", p.clone()).expect_err("so does the UI");
        assert!(e.contains(err), "{p}: {e}");
        h.run_steps(2);
        assert_eq!(h.state().dialog, None, "{p}");
    }
    let r = effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "prop.editGradient", json!({"layer": l, "prop": colors}));
    assert!(r.is_ok(), "{r:?}");
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(Dialog::GradientEditor));
}

/// In the editor, the colour picker's Escape and Cancel go back to the stop's colour and close only
/// the picker: the editor and its other edits stay (dialogs leave Escape to an open picker).
#[test]
fn picker_escape_and_cancel_restore_the_stop_and_keep_the_editor() {
    let (mut h, _, colors) = setup(&[]);
    open_editor(&mut h, colors);
    let added = along(&h, "dialog.gradient.addOpacity", 0.3);
    click_at(&mut h, added);
    click(&mut h, "dialog.gradient.colorStop.0");
    let id = h.state().gradient_draft().selected.expect("a selected stop").1;
    let original = stop_color(&h, id);
    for how in ["escape", "cancel"] {
        click(&mut h, "dialog.gradient.color");
        assert!(picker_open(&h), "{how}: the picker opened");
        type_hex(&mut h, "ff8000");
        assert!(near(stop_color(&h, id), [1.0, 128.0 / 255.0, 0.0, original[3]]), "{how}: the picker previews as it goes");
        if how == "escape" {
            key(&mut h, Key::Escape);
        } else {
            // The picker's Cancel, not the editor's.
            let editor_cancel = rect(&h, "dialog.gradient.cancel");
            let cancel =
                h.query_all_by_label("Cancel").map(|n| n.rect()).find(|r| (r.min.x - editor_cancel[0]).abs() > 1.0 || (r.min.y - editor_cancel[1]).abs() > 1.0);
            click_at(&mut h, cancel.expect("the picker's Cancel").center());
        }
        assert!(!picker_open(&h), "{how}: the picker closed");
        assert_eq!(h.state().dialog, Some(Dialog::GradientEditor), "{how}: the editor stays open");
        assert_eq!(stop_color(&h, id), original, "{how}: the stop's colour is back");
        assert_eq!(h.state().gradient_draft().opacities.len(), 3, "{how}: the editor's other edits stay");
    }
}

/// The Escape that closes the picker is its own: the next Escape, even in the very next pass,
/// cancels the editor (a closed picker doesn't keep the dialog from it).
#[test]
fn the_escape_after_the_picker_closes_cancels_the_editor() {
    let (mut h, _, colors) = setup(&[]);
    let project = h.state().session.project.clone();
    open_editor(&mut h, colors);
    click(&mut h, "dialog.gradient.colorStop.0");
    click(&mut h, "dialog.gradient.color");
    assert!(picker_open(&h), "the picker opened");
    let escape = |h: &mut Harness<'_, EffectcraftApp>| {
        h.input_mut().events.push(Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
        h.input_mut().events.push(Event::Key { key: Key::Escape, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
        h.step();
    };
    escape(&mut h);
    assert_eq!(h.state().dialog, Some(Dialog::GradientEditor), "the first Escape closed only the picker");
    escape(&mut h);
    assert_eq!(h.state().dialog, None, "the next pass's Escape cancels the editor");
    assert_eq!(h.state().session.project, project, "and changes nothing");
}
