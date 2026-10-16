//! Composition Mini-Flowchart (Tab), as in After Effects: Tab toggles it from the Timeline and the
//! viewer, the arrow keys move between comps and expand a branch, Enter opens the selected comp.

use std::sync::Arc;

use effectcraft_engine::Session;
use effectcraft_engine::project::{ItemId, LayerId, LayerSource};
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use effectcraft_ui_egui::panels::precomp::{MINI_MAX_COLUMN, MINI_MAX_DEPTH, MiniNode, mini_columns};
use egui::{Event, Key, Modifiers, vec2};
use egui_kittest::Harness;
use serde_json::json;

struct Comps {
    top: ItemId,
    main: ItemId,
    mid: ItemId,
    inner: ItemId,
}

/// Top ⊃ Main ⊃ Mid ⊃ Inner, with Mid active.
fn nested() -> (Session, Comps) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 160, "height": 90, "frameRate": 25, "duration": 1})).unwrap();
    let main = s.active_comp_id().unwrap();
    let l = s.execute("layer.newSolid", json!({"name": "Red", "color": "#ff0000"})).unwrap()["layer"].as_u64().unwrap();
    let mid = ItemId(s.execute("layer.precompose", json!({"layers": [l], "name": "Mid", "open": true})).unwrap()["comp"].as_u64().unwrap());
    let l = s.project.comp(mid).unwrap().layers[0].id.0;
    let inner = ItemId(s.execute("layer.precompose", json!({"layers": [l], "name": "Inner", "open": false})).unwrap()["comp"].as_u64().unwrap());
    s.execute("comp.new", json!({"name": "Top", "width": 160, "height": 90, "frameRate": 25, "duration": 1})).unwrap();
    let top = s.active_comp_id().unwrap();
    s.execute("layer.addItem", json!({"item": main.0})).unwrap();
    s.open_comp(mid);
    (s, Comps { top, main, mid, inner })
}

fn harness(s: Session, focused: PanelKind) -> Harness<'static, EffectcraftApp> {
    let mut app = EffectcraftApp::new(s);
    app.ui.start_screen = false;
    app.ui.show_menu_bar = false;
    app.ui.focused = focused;
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(move |_| app);
    h.run_steps(4);
    h
}

fn key(h: &mut Harness<'_, EffectcraftApp>, key: Key, modifiers: Modifiers) {
    h.event(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
    h.step();
    h.event(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
    h.step();
    h.run_steps(2);
}

fn tab(h: &mut Harness<'_, EffectcraftApp>) {
    key(h, Key::Tab, Modifiers::NONE);
}

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let [x, y, w, hh] = h.state().auto.find(id).unwrap_or_else(|| panic!("{id} is registered")).rect;
    let p = egui::pos2(x + w / 2.0, y + hh / 2.0);
    h.input_mut().events.push(Event::PointerMoved(p));
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    // Let the Mini-Flowchart's camera ease (0.12 s) settle on the new selection.
    h.run_steps(20);
}

fn open(h: &Harness<'_, EffectcraftApp>) -> bool {
    h.state().ui.mini_flowchart.is_some()
}

fn shown(h: &Harness<'_, EffectcraftApp>, id: &str) -> bool {
    h.state().auto.elements.iter().any(|e| e.id == id)
}

/// The user's case: a layer clicked in the Timeline, then Tab. egui's focus traversal took the
/// press (it focused the toolbar), so the Mini-Flowchart never opened.
#[test]
fn tab_toggles_the_mini_flowchart_from_the_timeline_after_a_layer_click() {
    let (s, c) = nested();
    let layer = s.project.comp(c.mid).unwrap().layers[0].id.0;
    let project = s.project.clone();
    let mut h = harness(s, PanelKind::Composition);
    click(&mut h, &format!("timeline.layer.{layer}.row"));
    assert_eq!(h.state().ui.focused, PanelKind::Timeline);
    assert_eq!(h.state().session.state.selected_layers, vec![LayerId(layer)]);
    tab(&mut h);
    assert!(open(&h), "Tab opens the Mini-Flowchart with the Timeline active");
    assert_eq!(h.ctx.memory(|m| m.focused()), None, "Tab did not also move keyboard focus to the toolbar");
    for id in [c.mid, c.main, c.inner] {
        assert!(shown(&h, &format!("flowchart.mini.{}", id.0)), "{id:?} is a node");
    }
    assert!(shown(&h, &format!("flowchart.mini.up.{}.expand", c.main.0)), "Main is nested in Top: it has an expand arrow");
    assert!(!shown(&h, &format!("flowchart.mini.{}", c.top.0)), "Top is one more level up, collapsed");
    assert!(!shown(&h, &format!("flowchart.mini.down.{}.expand", c.inner.0)), "Inner nests nothing");
    tab(&mut h);
    assert!(!open(&h), "Tab again closes it");
    tab(&mut h);
    assert!(open(&h));
    key(&mut h, Key::Escape, Modifiers::NONE);
    assert!(!open(&h), "Escape closes it");
    // The viewer: a click in the Composition panel, then Tab.
    click(&mut h, "panel.Composition");
    assert_eq!(h.state().ui.focused, PanelKind::Composition);
    tab(&mut h);
    assert!(open(&h), "Tab opens it with the Composition panel active");
    click(&mut h, "panel.Composition");
    assert!(!open(&h), "a click outside closes it");
    assert!(Arc::ptr_eq(&h.state().session.project, &project), "the Mini-Flowchart does not edit the project");
}

#[test]
fn tab_in_a_text_field_or_during_keyboard_traversal_keeps_its_widget_meaning() {
    let (s, _) = nested();
    let mut h = harness(s, PanelKind::Timeline);
    click(&mut h, "timeline.search");
    assert!(h.ctx.text_edit_focused(), "the Timeline's search field has the keyboard");
    tab(&mut h);
    assert!(!open(&h), "Tab in a text field is the field's");
    // Shift+Tab starts keyboard traversal; Tab then keeps moving between widgets.
    click(&mut h, "panel.Timeline");
    assert_eq!(h.ctx.memory(|m| m.focused()), None);
    key(&mut h, Key::Tab, Modifiers::SHIFT);
    let first = h.ctx.memory(|m| m.focused());
    assert!(first.is_some(), "Shift+Tab focuses a widget");
    for _ in 0..3 {
        tab(&mut h);
        assert!(!open(&h), "Tab during keyboard traversal moves focus, not the Mini-Flowchart");
    }
    assert_ne!(h.ctx.memory(|m| m.focused()), first);
    // In other panels Tab is widget traversal, as before.
    let mut h = harness(nested().0, PanelKind::Project);
    tab(&mut h);
    assert!(!open(&h));
    assert!(h.ctx.memory(|m| m.focused()).is_some());
}

#[test]
fn arrow_keys_move_the_focus_with_the_camera_on_it_and_enter_opens_it() {
    let (mut s, c) = nested();
    // Main also nests Side: Main branches into Mid and Side.
    s.execute("comp.new", json!({"name": "Side", "width": 160, "height": 90, "frameRate": 25, "duration": 1})).unwrap();
    let side = s.active_comp_id().unwrap();
    nest(&mut s, c.main, side, 9100);
    s.open_comp(c.mid);
    let mut h = harness(s, PanelKind::Timeline);
    tab(&mut h);
    assert!(open(&h));
    let focus = |h: &Harness<'_, EffectcraftApp>| h.state().ui.mini_flow.focus.map(ItemId);
    assert_eq!(focus(&h), None, "the active comp is the focus");
    let camera = |h: &Harness<'_, EffectcraftApp>| {
        let [x, y, w, hh] = h.state().auto.find("flowchart.mini.focus").expect("the focus is drawn").rect;
        egui::pos2(x + w / 2.0, y + hh / 2.0)
    };
    let settle = |h: &mut Harness<'_, EffectcraftApp>| h.run_steps(20);
    let start = camera(&h);
    // Right: Inner becomes the focus, in the camera's place.
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    settle(&mut h);
    assert_eq!(focus(&h), Some(c.inner));
    assert!((camera(&h) - start).length() < 1.0, "the camera stays on the focus");
    // Left goes back to Mid, then on to Main, which shows all its branches at once (Mid and Side)
    // and the comp that nests it (Top), without reopening the popup.
    key(&mut h, Key::ArrowLeft, Modifiers::NONE);
    assert_eq!(focus(&h), Some(c.mid));
    key(&mut h, Key::ArrowLeft, Modifiers::NONE);
    settle(&mut h);
    assert_eq!(focus(&h), Some(c.main));
    assert!((camera(&h) - start).length() < 1.0, "the camera stays on the focus");
    for id in [c.mid, side, c.top] {
        assert!(shown(&h, &format!("flowchart.mini.{}", id.0)), "{id:?} is shown around Main");
    }
    // Right goes back to Mid (where it came from), Down to its sibling Side, Up back to Mid.
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    assert_eq!(focus(&h), Some(c.mid));
    assert!(shown(&h, &format!("flowchart.mini.sibling.{}", side.0)), "Side is Mid's sibling, in its column");
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(focus(&h), Some(side));
    key(&mut h, Key::ArrowUp, Modifiers::NONE);
    assert_eq!(focus(&h), Some(c.mid));
    // Up to Main and Top, the end of the chain: Left there stays put.
    key(&mut h, Key::ArrowLeft, Modifiers::NONE);
    key(&mut h, Key::ArrowLeft, Modifiers::NONE);
    assert_eq!(focus(&h), Some(c.top));
    key(&mut h, Key::ArrowLeft, Modifiers::NONE);
    assert_eq!(focus(&h), Some(c.top));
    assert_eq!(h.state().session.active_comp_id(), Some(c.mid), "arrows in the popup don't open comps");
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert!(!open(&h));
    assert_eq!(h.state().session.active_comp_id(), Some(c.top), "Enter opens the focused comp");
}

#[test]
fn clicking_an_expand_arrow_shows_the_next_level_and_a_node_opens_its_comp() {
    let (s, c) = nested();
    let mut h = harness(s, PanelKind::Composition);
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &egui::Context::default(), "comp.miniFlowchart", json!({})).unwrap();
    h.run_steps(3);
    assert!(open(&h));
    let expand = format!("flowchart.mini.up.{}.expand", c.main.0);
    click(&mut h, &expand);
    assert!(open(&h), "the expand arrow keeps the popup open");
    assert!(shown(&h, &format!("flowchart.mini.{}", c.top.0)));
    click(&mut h, &expand);
    assert!(!shown(&h, &format!("flowchart.mini.{}", c.top.0)), "clicking it again collapses the branch");
    click(&mut h, &format!("flowchart.mini.{}", c.inner.0));
    assert!(!open(&h));
    assert_eq!(h.state().session.active_comp_id(), Some(c.inner));
}

/// Add a layer using `item` to `comp` directly (the commands refuse to nest a comp in itself, but a
/// project file can).
fn nest(s: &mut Session, comp: ItemId, item: ItemId, layer: u64) {
    let p = Arc::make_mut(&mut s.project);
    let template = p.comps().find_map(|(_, c)| c.layers.first().cloned()).unwrap();
    let c = p.comp_mut(comp).unwrap();
    let mut l = template;
    l.id = LayerId(layer);
    l.source = LayerSource::Comp { item };
    c.layers.push(l);
}

#[test]
fn cyclic_and_deep_nesting_stays_bounded() {
    let (mut s, c) = nested();
    // Inner nests Mid: Mid ⊃ Inner ⊃ Mid. Top nests Inner: another way round.
    nest(&mut s, c.inner, c.mid, 9001);
    nest(&mut s, c.top, c.inner, 9002);
    nest(&mut s, c.inner, c.inner, 9003);
    // A chain deeper than the Mini-Flowchart expands.
    let mut prev = c.inner;
    for i in 0..(MINI_MAX_DEPTH + 8) {
        s.execute("comp.new", json!({"name": format!("Deep {i}"), "width": 16, "height": 16, "frameRate": 25, "duration": 1})).unwrap();
        let deep = s.active_comp_id().unwrap();
        nest(&mut s, prev, deep, 9100 + i as u64);
        prev = deep;
    }
    // Wide: more comps under one node than a column shows.
    for i in 0..(MINI_MAX_COLUMN + 5) {
        s.execute("comp.new", json!({"name": format!("Wide {i}"), "width": 16, "height": 16, "frameRate": 25, "duration": 1})).unwrap();
        let wide = s.active_comp_id().unwrap();
        nest(&mut s, c.inner, wide, 9500 + i as u64);
    }
    s.open_comp(c.mid);
    // Expand every node on both sides until nothing changes.
    let mut expanded: Vec<MiniNode> = vec![];
    for _ in 0..(MINI_MAX_DEPTH + 4) {
        for up in [true, false] {
            for col in mini_columns(&s.project, c.mid, up, &expanded) {
                for e in col {
                    if e.more && !expanded.contains(&e.key) {
                        expanded.push(e.key);
                    }
                }
            }
        }
    }
    for up in [true, false] {
        let cols = mini_columns(&s.project, c.mid, up, &expanded);
        assert!(cols.len() <= MINI_MAX_DEPTH);
        for col in &cols {
            assert!(col.len() <= MINI_MAX_COLUMN);
            for e in col {
                assert!(!e.key.path.contains(&c.mid.0), "the active comp is not repeated");
                let mut seen = e.key.path.clone();
                seen.sort_unstable();
                seen.dedup();
                assert_eq!(seen.len(), e.key.path.len(), "a branch never repeats a comp: {:?}", e.key.path);
            }
        }
    }
    let downs = mini_columns(&s.project, c.mid, false, &expanded);
    assert_eq!(downs.len(), MINI_MAX_DEPTH, "the deep chain stops at the depth limit");
    // The popup draws and navigates it without hanging.
    let mut h = harness(s, PanelKind::Timeline);
    tab(&mut h);
    assert!(open(&h));
    h.state_mut().ui.mini_flow.expanded = expanded;
    h.run_steps(2);
    for k in [Key::ArrowRight, Key::ArrowRight, Key::ArrowRight, Key::ArrowDown, Key::ArrowRight, Key::ArrowLeft, Key::ArrowUp, Key::ArrowLeft, Key::ArrowLeft]
    {
        key(&mut h, k, Modifiers::NONE);
    }
    assert!(open(&h));
    tab(&mut h);
    assert!(!open(&h));
}
