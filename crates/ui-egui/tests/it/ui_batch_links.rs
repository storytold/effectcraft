//! Parent and Track Matte for several layers at once (#1260): on a selected layer's row, the
//! Timeline's Parent and Track Matte menus and pick whips set every selected layer; on an
//! unselected row, that layer alone. One undo step each; each layer keeps its own matte mode.

use effectcraft_engine::Session;
use effectcraft_engine::project::{LayerId, MatteKind};
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use serde_json::json;

/// Solids A, B, C, P (moved and scaled, a parent) and the mattes X and Y; stacked Y, X, P, C, B, A
/// from the top. The Track Matte column is shown.
fn harness() -> (Harness<'static, EffectcraftApp>, [u64; 6]) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Links", "width": 320, "height": 180, "duration": 4})).unwrap();
    let ids = ["A", "B", "C", "P", "X", "Y"]
        .map(|n| s.execute("layer.newSolid", json!({"name": n, "color": "#406080", "width": 40, "height": 40})).unwrap()["layer"].as_u64().unwrap());
    s.execute("prop.set", json!({"layer": ids[3], "path": "transform/position", "value": [60, 40]})).unwrap();
    s.execute("prop.set", json!({"layer": ids[3], "path": "transform/scale", "value": [50, 50]})).unwrap();
    s.history.undo.clear();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.state_mut().ui.timeline.show_modes = true;
    h.state_mut().ui.timeline.columns_w = 900.0;
    h.run_steps(3);
    (h, ids)
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

fn press(h: &mut Harness<'_, EffectcraftApp>, at: Pos2, pressed: bool) {
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
}

fn click(h: &mut Harness<'_, EffectcraftApp>, at: Pos2) {
    h.event(Event::PointerMoved(at));
    h.step();
    press(h, at, true);
    h.step();
    press(h, at, false);
    h.run_steps(3);
}

/// Open the menu at automation id `dropdown` and choose `item` in it.
fn choose(h: &mut Harness<'_, EffectcraftApp>, dropdown: &str, item: &str) {
    let menu = rect(h, dropdown);
    // Other rows' menus show their choice as well ("No Track Matte"): the entry is a node the
    // opened menu added.
    let shown: Vec<Rect> = h.query_all_by_label(item).map(|n| n.rect()).collect();
    click(h, menu.center());
    // (The menu may open above it to fit the window: the entry nearest to it.)
    let off = |r: &Rect| (r.center().y - menu.center().y).abs();
    let at = h.query_all_by_label(item).map(|n| n.rect()).filter(|r| !shown.contains(r)).min_by(|a, b| off(a).total_cmp(&off(b)));
    let at = at.unwrap_or_else(|| panic!("no {item} in the {dropdown} menu")).center();
    click(h, at);
}

/// Drag the pick whip `whip` onto layer `onto`'s row.
fn whip(h: &mut Harness<'_, EffectcraftApp>, whip: &str, onto: u64) {
    let (from, to) = (rect(h, whip).center(), rect(h, &format!("timeline.layer.{onto}.row")).center());
    h.event(Event::PointerMoved(from));
    h.step();
    press(h, from, true);
    h.step();
    for i in 1..=10 {
        h.event(Event::PointerMoved(from + (to - from) * (i as f32 / 10.0)));
        h.step();
    }
    press(h, to, false);
    h.run_steps(3);
}

fn select(h: &mut Harness<'_, EffectcraftApp>, ids: &[u64]) {
    h.state_mut().session.execute("layer.select", json!({"layers": ids})).unwrap();
    h.run_steps(2);
}

fn parent(h: &Harness<'_, EffectcraftApp>, id: u64) -> Option<u64> {
    h.state().session.active_comp().unwrap().layer(LayerId(id)).unwrap().parent.map(|p| p.0)
}

fn position(h: &Harness<'_, EffectcraftApp>, id: u64) -> [f64; 3] {
    h.state().session.active_comp().unwrap().layer(LayerId(id)).unwrap().props.prop("transform/position").unwrap().value.as_vec3()
}

fn matte(h: &Harness<'_, EffectcraftApp>, id: u64) -> Option<(u64, MatteKind)> {
    h.state().session.active_comp().unwrap().layer(LayerId(id)).unwrap().track_matte.map(|m| (m.layer.0, m.kind))
}

/// The project and its undo steps as they are now, to compare with after an undo or a refusal.
fn mark(h: &Harness<'_, EffectcraftApp>) -> (std::sync::Arc<effectcraft_engine::project::Project>, usize) {
    (h.state().session.project.clone(), h.state().session.history.undo.len())
}

fn unchanged(h: &Harness<'_, EffectcraftApp>, m: &(std::sync::Arc<effectcraft_engine::project::Project>, usize)) -> bool {
    h.state().session.project.same_content(&m.0) && h.state().session.history.undo.len() == m.1
}

#[test]
fn parent_menu_and_pick_whip_parent_the_selected_layers() {
    let (mut h, [a, b, c, p, ..]) = harness();
    let start = position(&h, a);
    select(&mut h, &[a, b]);
    let before = mark(&h);
    // The Parent menu on A's (selected) row parents A and B, kept where they are.
    choose(&mut h, &format!("timeline.layer.{a}.parent"), "3. P");
    assert_eq!([parent(&h, a), parent(&h, b), parent(&h, c)], [Some(p), Some(p), None]);
    assert!(position(&h, a) != start && position(&h, a) == position(&h, b), "both re-expressed in P's space");
    assert_eq!(h.state().session.history.undo.len(), before.1 + 1, "one undo step");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    h.run_steps(2);
    assert!(unchanged(&h, &before), "one undo puts both back");
    // The pick whip of an unselected row parents that layer alone.
    whip(&mut h, &format!("timeline.layer.{c}.pickWhip"), p);
    assert_eq!([parent(&h, a), parent(&h, b), parent(&h, c)], [None, None, Some(p)]);
    // A selected row's whip parents the selection, less the parent itself if selected.
    select(&mut h, &[a, b, p]);
    whip(&mut h, &format!("timeline.layer.{a}.pickWhip"), p);
    assert_eq!([parent(&h, a), parent(&h, b), parent(&h, p)], [Some(p), Some(p), None]);
    // A cycle (P under A, while A is under P) is refused as a whole.
    select(&mut h, &[p, c]);
    let before = mark(&h);
    choose(&mut h, &format!("timeline.layer.{p}.parent"), "6. A");
    assert!(unchanged(&h, &before), "the refused parenting changed nothing");
    assert_eq!([parent(&h, p), parent(&h, c)], [None, Some(p)]);
}

#[test]
fn track_matte_menu_and_pick_whip_set_the_selected_layers_mattes() {
    let (mut h, [a, b, c, _, x, y]) = harness();
    h.state_mut().session.execute("layer.setTrackMatte", json!({"layer": a, "matte": x, "kind": "luma"})).unwrap();
    h.state_mut().session.execute("layer.setTrackMatte", json!({"layer": b, "matte": x, "kind": "alpha"})).unwrap();
    select(&mut h, &[a, b]);
    let before = mark(&h);
    // A new matte layer for both; each keeps its mode; C (not selected) keeps none.
    choose(&mut h, &format!("timeline.layer.{a}.trackMatte"), "1. Y");
    assert_eq!([matte(&h, a), matte(&h, b), matte(&h, c)], [Some((y, MatteKind::Luma)), Some((y, MatteKind::Alpha)), None]);
    assert!(!h.state().session.active_comp().unwrap().layer(LayerId(y)).unwrap().switches.video, "Y is hidden as a matte");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    h.run_steps(2);
    assert!(unchanged(&h, &before), "one undo puts both mattes and Y's visibility back");
    // A mode for both, then No Track Matte for both.
    choose(&mut h, &format!("timeline.layer.{b}.trackMatte"), "Luma Inverted Matte");
    assert_eq!([matte(&h, a), matte(&h, b)], [Some((x, MatteKind::LumaInverted)), Some((x, MatteKind::LumaInverted))]);
    choose(&mut h, &format!("timeline.layer.{a}.trackMatte"), "No Track Matte");
    assert_eq!([matte(&h, a), matte(&h, b)], [None, None]);
    // The track matte pick whip: a selected row's sets the selection's, an unselected row's
    // its own.
    whip(&mut h, &format!("timeline.layer.{a}.trackMattePickWhip"), y);
    assert_eq!([matte(&h, a), matte(&h, b), matte(&h, c)], [Some((y, MatteKind::Alpha)), Some((y, MatteKind::Alpha)), None]);
    whip(&mut h, &format!("timeline.layer.{c}.trackMattePickWhip"), x);
    assert_eq!([matte(&h, a), matte(&h, c)], [Some((y, MatteKind::Alpha)), Some((x, MatteKind::Alpha))]);
}

fn video(h: &Harness<'_, EffectcraftApp>, id: u64) -> bool {
    h.state().session.active_comp().unwrap().layer(LayerId(id)).unwrap().switches.video
}

/// A locked layer in the selection stays selected, and keeps its parent, its transform and its
/// matte while another selected row's menus and pick whips change the rest.
#[test]
fn locked_selected_layers_keep_their_parent_and_matte() {
    let (mut h, [a, b, c, _, x, _]) = harness();
    select(&mut h, &[a, b]);
    // B alone (a click on its Lock switch locks the whole selection).
    h.state_mut().session.execute("layer.setSwitch", json!({"layers": [b], "switch": "lock", "value": true})).unwrap();
    h.run_steps(2);
    let selected: Vec<u64> = h.state().session.state.selected_layers.iter().map(|l| l.0).collect();
    assert!(h.state().session.active_comp().unwrap().layer(LayerId(b)).unwrap().switches.locked && selected == vec![a, b], "{selected:?}");
    let b_position = position(&h, b);
    choose(&mut h, &format!("timeline.layer.{a}.parent"), "3. P");
    whip(&mut h, &format!("timeline.layer.{a}.pickWhip"), c);
    choose(&mut h, &format!("timeline.layer.{a}.trackMatte"), "1. Y");
    whip(&mut h, &format!("timeline.layer.{a}.trackMattePickWhip"), x);
    assert_eq!((parent(&h, a), matte(&h, a)), (Some(c), Some((x, MatteKind::Alpha))), "A changed each time");
    assert_eq!((parent(&h, b), matte(&h, b), position(&h, b)), (None, None, b_position), "locked B didn't");
}

/// Selected nulls (no Track Matte on their rows) get no matte from another row's menu or pick
/// whip: at the top of the comp one used to refuse the whole mode change (nothing above it to
/// take as a matte), at the bottom it took the layer above as its matte and hid it.
#[test]
fn selected_nulls_get_no_matte() {
    let (mut h, [a, b, _, _, x, y]) = harness();
    h.state_mut().session.execute("layer.setTrackMatte", json!({"layers": [a, b], "matte": x, "kind": "alpha"})).unwrap();
    let n = h.state_mut().session.execute("layer.newNull", json!({"name": "N"})).unwrap()["layer"].as_u64().unwrap();
    select(&mut h, &[a, b, n]);
    choose(&mut h, &format!("timeline.layer.{a}.trackMatte"), "Luma Matte");
    assert_eq!([matte(&h, a), matte(&h, b), matte(&h, n)], [Some((x, MatteKind::Luma)), Some((x, MatteKind::Luma)), None], "a null on top");
    h.state_mut().session.execute("layer.arrange", json!({"layers": [n], "to": "back"})).unwrap();
    select(&mut h, &[a, b, n]);
    choose(&mut h, &format!("timeline.layer.{a}.trackMatte"), "Alpha Inverted Matte");
    assert_eq!(
        [matte(&h, a), matte(&h, b), matte(&h, n)],
        [Some((x, MatteKind::AlphaInverted)), Some((x, MatteKind::AlphaInverted)), None],
        "a null at the bottom"
    );
    assert!(video(&h, a), "A wasn't hidden as the null's matte");
    whip(&mut h, &format!("timeline.layer.{a}.trackMattePickWhip"), y);
    assert_eq!(
        [matte(&h, a), matte(&h, b), matte(&h, n)],
        [Some((y, MatteKind::AlphaInverted)), Some((y, MatteKind::AlphaInverted)), None],
        "the pick whip too"
    );
}
