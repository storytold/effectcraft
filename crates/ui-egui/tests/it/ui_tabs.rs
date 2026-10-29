//! Comp tabs and tab drags (egui_kittest): double-clicking a comp in the Project panel opens it
//! as its own Timeline tab (not a rename, and not in place of the comp shown), the tabs switch and
//! close comps, and a dragged tab lands where its insertion mark shows.

use effectcraft_engine::Session;
use effectcraft_engine::project::ItemId;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::WorkspaceExt as _;
use effectcraft_ui_egui::dock::{Node, PanelKind, SplitSize, Workspace};
use egui::{Event, Pos2, pos2};
use egui_kittest::Harness;
use serde_json::json;

/// "Pre" and "Main" (Main holds Pre); only Main is open.
fn harness() -> (Harness<'static, EffectcraftApp>, u64, u64) {
    let mut s = Session::default();
    let pre = s.execute("comp.new", json!({"name": "Pre", "width": 320, "height": 180, "duration": 4})).unwrap()["comp"].as_u64().unwrap();
    let main = s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4})).unwrap()["comp"].as_u64().unwrap();
    s.execute("layer.addItem", json!({"item": pre})).unwrap();
    s.state.open_comps = vec![ItemId(main)];
    s.state.active_comp = Some(ItemId(main));
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    (h, pre, main)
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> egui::Rect {
    let e = h.state().auto.previous.iter().chain(h.state().auto.elements.iter()).find(|e| e.id == id).cloned().unwrap_or_else(|| {
        let have: Vec<&String> = h.state().auto.elements.iter().map(|e| &e.id).filter(|i| i.starts_with("panel.") || i.starts_with("project.")).collect();
        panic!("no element {id}; have {have:?}")
    });
    egui::Rect::from_min_size(pos2(e.rect[0], e.rect[1]), egui::vec2(e.rect[2], e.rect[3]))
}

fn click_n(h: &mut Harness<'_, EffectcraftApp>, p: Pos2, n: u32) {
    h.event(Event::PointerMoved(p));
    h.step();
    // All in one frame, straight into the input (the harness' event queue spreads presses over
    // frames, longer apart than a double-click).
    for _ in 0..n {
        h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
        h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    }
    h.step();
    h.run_steps(2);
}

fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) {
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    for k in 1..=6 {
        let f = k as f32 / 6.0;
        h.event(Event::PointerMoved(from + (to - from) * f));
        h.step();
    }
    h.event(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.step();
    h.run_steps(2);
}

fn open_comps(h: &Harness<'_, EffectcraftApp>) -> Vec<u64> {
    h.state().session.state.open_comps.iter().map(|c| c.0).collect()
}

#[test]
fn project_double_click_opens_a_comp_as_its_own_timeline_tab() {
    let (mut h, pre, main) = harness();
    // Double-click the precomp's name in the Project panel: it opens (no rename) next to Main.
    let name = rect(&h, &format!("project.item.{pre}.name"));
    click_n(&mut h, name.center(), 2);
    assert_eq!(open_comps(&h), vec![main, pre]);
    assert_eq!(h.state().session.state.active_comp, Some(ItemId(pre)));
    assert_eq!(h.state().session.project.item(ItemId(pre)).map(|i| i.name.clone()), Some("Pre".into()));
    // One comp tab per comp in the Timeline's tab strip, the newest last.
    let (tm, tp) = (rect(&h, &format!("panel.tab.Timeline.{main}")), rect(&h, &format!("panel.tab.Timeline.{pre}")));
    assert!(tp.min.x >= tm.max.x - 0.5, "{tm:?} {tp:?}");
    let strip = rect(&h, "panel.tab.Timeline");
    assert!(strip.y_range().contains(tp.center().y) && tp.min.x > strip.max.x, "{strip:?} {tp:?}");
    // Clicking Main's tab shows Main again; both stay open.
    click_n(&mut h, tm.center(), 1);
    assert_eq!(h.state().session.state.active_comp, Some(ItemId(main)));
    assert_eq!(open_comps(&h), vec![main, pre]);
    // × closes the shown comp's Timeline only: the panel stays with Pre.
    let close = rect(&h, "panel.tab.Timeline.close");
    click_n(&mut h, close.center(), 1);
    assert_eq!(open_comps(&h), vec![pre]);
    assert_eq!(h.state().session.state.active_comp, Some(ItemId(pre)));
    assert!(h.state().ui.dock.is_docked(PanelKind::Timeline));
    // Double-clicking an open comp again adds no second tab.
    let name = rect(&h, &format!("project.item.{pre}.name"));
    click_n(&mut h, name.center(), 2);
    assert_eq!(open_comps(&h), vec![pre]);
}

#[test]
fn dragged_tabs_land_where_the_insertion_mark_is() {
    let (mut h, _, _) = harness();
    let dock = |h: &Harness<'_, EffectcraftApp>| h.state().ui.dock.clone();
    assert_eq!(group_of(&dock(&h), PanelKind::Timeline), Some(vec![PanelKind::Timeline, PanelKind::RenderQueue]));
    // Reorder within the group: Render Queue dropped on the left half of the Timeline tab goes
    // before it.
    let tl = rect(&h, "panel.tab.Timeline");
    let rq = rect(&h, "panel.tab.RenderQueue");
    drag(&mut h, rq.center(), pos2(tl.min.x + tl.width() * 0.2, tl.center().y));
    assert_eq!(group_of(&dock(&h), PanelKind::Timeline), Some(vec![PanelKind::RenderQueue, PanelKind::Timeline]));
    // Another group's tab dropped between them lands between them.
    let (rq, tl) = (rect(&h, "panel.tab.RenderQueue"), rect(&h, "panel.tab.Timeline"));
    let fx = rect(&h, "panel.tab.EffectControls");
    drag(&mut h, fx.center(), pos2((rq.max.x + tl.min.x) / 2.0 + 2.0, tl.center().y));
    assert_eq!(group_of(&dock(&h), PanelKind::Timeline), Some(vec![PanelKind::RenderQueue, PanelKind::EffectControls, PanelKind::Timeline]));
    // Past the last tab: last.
    let (ec, tl) = (rect(&h, "panel.tab.EffectControls"), rect(&h, "panel.tab.Timeline"));
    drag(&mut h, ec.center(), pos2(tl.max.x + 30.0, tl.center().y));
    assert_eq!(group_of(&dock(&h), PanelKind::Timeline), Some(vec![PanelKind::RenderQueue, PanelKind::Timeline, PanelKind::EffectControls]));
}

/// `cargo test -p effectcraft-ui-egui --test it ui_tabs:: -- --ignored`: the insertion mark mid-drag
/// (`target/test-out/tab-drag.png`).
#[test]
#[ignore]
fn tab_drag_snapshot() {
    let (mut h, _, _) = harness();
    let (tl, rq) = (rect(&h, "panel.tab.Timeline"), rect(&h, "panel.tab.RenderQueue"));
    let fx = rect(&h, "panel.tab.EffectControls");
    let to = pos2((tl.max.x + rq.min.x) / 2.0, tl.center().y);
    h.event(Event::PointerMoved(fx.center()));
    h.step();
    h.event(Event::PointerButton { pos: fx.center(), button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    for k in 1..=6 {
        h.event(Event::PointerMoved(fx.center() + (to - fx.center()) * (k as f32 / 6.0)));
        h.step();
    }
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/test-out");
    std::fs::create_dir_all(dir).unwrap();
    h.render().expect("render").save(format!("{dir}/tab-drag.png")).unwrap();
}

/// A precomp layer's bar shows its comp's markers; a double-click opens that comp at the marker.
#[test]
fn nested_comp_markers_show_on_the_precomp_bar() {
    let (mut h, pre, main) = harness();
    {
        let s = &mut h.state_mut().session;
        s.execute("comp.open", json!({"comp": pre})).unwrap();
        s.execute("markers.set", json!({"new": true, "time": 1.0, "comment": "beat"})).unwrap();
        s.execute("comp.open", json!({"comp": main})).unwrap();
    }
    h.run_steps(4);
    let l = h.state().session.project.comp(ItemId(main)).unwrap().layers[0].id.0;
    let id = format!("timeline.layer.{l}.nestedMarker.0");
    let m = rect(&h, &id);
    let e = h.state().auto.find(&id).unwrap().clone();
    assert_eq!(e.label, "beat (marker in Pre)");
    click_n(&mut h, m.center(), 2);
    assert_eq!(h.state().session.active_comp_id(), Some(ItemId(pre)));
    // (29.97 fps: the marker's frame is at 1.001 s)
    assert!((h.state().session.time().seconds() - 1.0).abs() < 0.034, "{}", h.state().session.time().seconds());
}

/// #637: double-clicking a precomp layer's bar opens its comp, as double-clicking its name does.
#[test]
fn double_clicking_a_precomp_bar_opens_its_comp() {
    let (mut h, pre, main) = harness();
    h.state_mut().session.execute("comp.open", json!({"comp": main})).unwrap();
    h.run_steps(4);
    let l = h.state().session.project.comp(ItemId(main)).unwrap().layers[0].id.0;
    let bar = rect(&h, &format!("timeline.layer.{l}.bar"));
    click_n(&mut h, bar.center(), 2);
    assert_eq!(h.state().session.active_comp_id(), Some(ItemId(pre)));
}

/// Issue #46: a panel dragged onto the left edge of another group docks there as its own group;
/// dragging the gap between panels resizes them.
#[test]
fn panels_dock_beside_others_and_gutters_resize_them() {
    let (mut h, _, _) = harness();
    let dock = |h: &Harness<'_, EffectcraftApp>| h.state().ui.dock.clone();
    let tab = rect(&h, "panel.tab.EffectControls");
    let tl = rect(&h, "panel.Timeline");
    // Inside the Timeline body's left drop zone.
    let depth = craft_ui::docking::SIDE_EDGE_ZONE_DEPTH.min(tl.width().min(tl.height()) / 4.0);
    drag(&mut h, tab.center(), pos2(tl.min.x + depth / 2.0, tl.center().y));
    assert_eq!(group_of(&dock(&h), PanelKind::EffectControls), Some(vec![PanelKind::EffectControls]));
    assert!(!group_of(&dock(&h), PanelKind::Timeline).unwrap().contains(&PanelKind::EffectControls));
    let (fx, tl) = (rect(&h, "panel.EffectControls"), rect(&h, "panel.Timeline"));
    assert!(fx.max.x <= tl.min.x + 1.0 && (fx.center().y - tl.center().y).abs() < tl.height(), "{fx:?} {tl:?}");
    // The gutter on the Project panel's right edge: dragging it 60 px widens the panel by 60.
    let project = rect(&h, "panel.Project");
    let gutter = h
        .state()
        .auto
        .previous
        .iter()
        .filter(|e| e.id.starts_with("dock.gutter.") && e.rect[3] > e.rect[2])
        .map(|e| egui::Rect::from_min_size(pos2(e.rect[0], e.rect[1]), egui::vec2(e.rect[2], e.rect[3])))
        .filter(|g| g.y_range().contains(project.center().y))
        .min_by(|a, b| (a.center().x - project.max.x).abs().total_cmp(&(b.center().x - project.max.x).abs()))
        .expect("a gutter beside the Project panel");
    drag(&mut h, gutter.center(), gutter.center() + egui::vec2(60.0, 0.0));
    let wider = rect(&h, "panel.Project");
    assert!((wider.width() - project.width() - 60.0).abs() < 2.0, "{} → {}", project.width(), wider.width());
}

/// The divider below the Preview frame: the horizontal gutter between it and the Properties
/// frame.
fn preview_divider(h: &Harness<'_, EffectcraftApp>) -> egui::Rect {
    let (preview, props) = (rect(h, "panel.Preview"), rect(h, "panel.Properties"));
    h.state()
        .auto
        .previous
        .iter()
        .filter(|e| e.id.starts_with("dock.gutter.") && e.rect[2] > e.rect[3])
        .map(|e| egui::Rect::from_min_size(pos2(e.rect[0], e.rect[1]), egui::vec2(e.rect[2], e.rect[3])))
        .find(|g| g.x_range().contains(preview.center().x) && g.center().y >= preview.max.y - 1.0 && g.center().y <= props.min.y + 1.0)
        .expect("a gutter between Preview and Properties")
}

/// Default's right column: a short Preview frame (46 points of body) and the Properties frame
/// below it taking the rest of the column, at any window height.
#[test]
fn the_default_preview_frame_is_short_and_properties_takes_the_rest() {
    let (mut h, _, _) = harness();
    let body = |h: &Harness<'_, EffectcraftApp>| rect(h, "panel.Preview").height();
    assert!((body(&h) - 46.0).abs() < 1.5, "Preview {}", body(&h));
    let props = rect(&h, "panel.Properties");
    assert!(props.height() > 300.0, "Properties {props:?}");
    // A taller window gives the room to Properties.
    let size = h.ctx.content_rect().size();
    h.set_size(size + egui::vec2(0.0, 200.0));
    h.run_steps(3);
    assert!((body(&h) - 46.0).abs() < 1.5, "Preview {}", body(&h));
    assert!((rect(&h, "panel.Properties").height() - props.height() - 200.0).abs() < 2.0);
}

/// Issue #171: the divider below the Preview frame drags to resize it (Preview was stuck at its
/// default height), not below a minimum; the new height stays in the layout.
#[test]
fn the_preview_frame_divider_resizes_it() {
    let (mut h, _, _) = harness();
    fn preview_size(w: &Workspace) -> Option<SplitSize> {
        fn rec(n: &Node<PanelKind>) -> Option<SplitSize> {
            match n {
                Node::Split { first, second, size, .. } => match first.as_ref() {
                    Node::Tabs { panels, .. } if panels.contains(&PanelKind::Preview) => Some(*size),
                    _ => rec(first).or_else(|| rec(second)),
                },
                _ => None,
            }
        }
        rec(w.layout.root.as_ref()?)
    }
    let (preview, props) = (rect(&h, "panel.Preview"), rect(&h, "panel.Properties"));
    let gap = preview_divider(&h);
    drag(&mut h, gap.center(), gap.center() + egui::vec2(0.0, 150.0));
    let (p2, q2) = (rect(&h, "panel.Preview"), rect(&h, "panel.Properties"));
    assert!((p2.height() - preview.height() - 150.0).abs() < 2.0, "Preview {} → {}", preview.height(), p2.height());
    assert!((props.height() - q2.height() - 150.0).abs() < 2.0, "Properties {} → {}", props.height(), q2.height());
    assert!((q2.max.y - props.max.y).abs() < 1.0, "the column stays filled");
    // The frame keeps a fixed height, 150 points more.
    let Some(SplitSize::FixedFirst(size)) = preview_size(&h.state().ui.dock) else { panic!("{:?}", preview_size(&h.state().ui.dock)) };
    assert!((size - effectcraft_ui_egui::dock::PREVIEW_FRAME - 150.0).abs() < 2.0, "{size}");
    // Dragged up to Preview's top, it stops at its minimum.
    let gap = preview_divider(&h);
    let top = rect(&h, "panel.Preview").min.y;
    drag(&mut h, gap.center(), pos2(gap.center().x, top));
    assert!((rect(&h, "panel.Preview").height() - 40.0).abs() < 1.0, "{}", rect(&h, "panel.Preview").height());
}

/// Maximize acts on one frame: the Preview frame fills the dock area, Properties hides.
#[test]
fn maximizing_preview_maximizes_its_frame_alone() {
    let (mut h, _, _) = harness();
    let preview = rect(&h, "panel.Preview");
    h.state_mut().ui.maximized = Some(PanelKind::Preview);
    h.run_steps(3);
    let big = rect(&h, "panel.Preview");
    assert!(big.height() > preview.height() + 300.0 && big.width() > preview.width() + 300.0, "{preview:?} → {big:?}");
    assert!(h.state().auto.find("panel.Properties").is_none(), "the Properties frame is not shown");
}

/// Issue #45: the Project panel's details (name, size, duration) stay inside the panel with a
/// margin, cut short with "…" when they don't fit; so does the hint shown with nothing selected.
#[test]
fn project_details_stay_inside_the_panel() {
    let (mut h, pre, _) = harness();
    let long = "A composition with a very long name that could never fit beside the thumbnail".to_string();
    h.state_mut().session.execute("project.rename", json!({"item": pre, "name": long})).unwrap();
    h.state_mut().session.state.project_selection = vec![ItemId(pre)];
    h.run_steps(3);
    let panel = rect(&h, "panel.Project");
    for id in ["project.details.name", "project.details.0", "project.details.1"] {
        let r = rect(&h, id);
        assert!(r.max.x <= panel.max.x - 9.0 && r.width() > 10.0, "{id}: {r:?} in {panel:?}");
    }
    h.state_mut().session.state.project_selection.clear();
    h.run_steps(3);
    let r = rect(&h, "project.details.hint");
    assert!(r.min.x >= panel.min.x + 9.0 && r.max.x <= panel.max.x - 9.0, "{r:?} in {panel:?}");
}

/// A file dropped on the window (what the windowing layer hands egui).
#[derive(Debug)]
struct Dropped(std::path::PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

/// Issue #44: double-clicking the Project panel's empty area asks for files to import (as After
/// Effects' File ▸ Import ▸ File... does), and files dropped on the window are imported. (Tests
/// have no media decoders: what is checked is that the import runs with those files.)
#[test]
fn project_empty_area_double_click_and_dropped_files_import() {
    let (mut h, _, _) = harness();
    let png = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/app-icon/hicolor/16x16/apps/ai.storyteller.effectcraft.png");
    let png = std::fs::canonicalize(png).unwrap().to_string_lossy().to_string();
    let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (a, p) = (asked.clone(), png.clone());
    h.state_mut().hooks.pick_files = Some(Box::new(move |_| {
        a.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        vec![p.clone()]
    }));
    let imports = |h: &Harness<'_, EffectcraftApp>| h.state().session.journal.iter().filter(|(c, p)| c == "file.import" && p["paths"] == json!([png])).count();
    // Below the last row.
    let last = rect(&h, "project.item.2");
    let empty = rect(&h, "project.empty");
    click_n(&mut h, pos2(empty.center().x, (last.max.y + empty.max.y) / 2.0), 2);
    assert_eq!(asked.load(std::sync::atomic::Ordering::Relaxed), 1, "the file dialog opened");
    assert_eq!(imports(&h), 1, "the picked file was imported");
    // A double-click on a row opens that item instead.
    let row = rect(&h, "project.item.2.name");
    click_n(&mut h, row.center(), 2);
    assert_eq!(asked.load(std::sync::atomic::Ordering::Relaxed), 1);
    // Dropping a file on the window imports it.
    h.input_mut().dropped_files.push(std::sync::Arc::new(Dropped(png.clone().into())));
    h.run_steps(2);
    assert_eq!(imports(&h), 2, "the dropped file was imported");
}

/// Issue #306: dragging the viewer/Timeline divider far up made a nested split shorter than its
/// two 20 pt minimums, and `f32::clamp` panicked with crossed bounds, closing the app.
#[test]
fn dragging_the_viewer_timeline_divider_far_up_does_not_crash() {
    let (mut h, _, _) = harness();
    // The horizontal gutter right above the Timeline.
    let tl = rect(&h, "panel.Timeline");
    let gutter = |h: &Harness<'_, EffectcraftApp>| {
        h.state()
            .auto
            .previous
            .iter()
            .filter(|e| e.id.starts_with("dock.gutter.") && e.rect[2] > e.rect[3])
            .map(|e| (e.id.clone(), egui::Rect::from_min_size(pos2(e.rect[0], e.rect[1]), egui::vec2(e.rect[2], e.rect[3]))))
            .filter(|(_, g)| g.x_range().contains(tl.center().x) && g.max.y <= tl.min.y + 1.0)
            .min_by(|a, b| (tl.min.y - a.1.max.y).total_cmp(&(tl.min.y - b.1.max.y)))
            .expect("the gutter above the Timeline")
    };
    let (id, g) = gutter(&h);
    drag(&mut h, g.center(), g.center() - egui::vec2(0.0, 2000.0));
    let g2 = rect(&h, &id);
    assert!(g2.center().y < g.center().y - 100.0, "the divider moved up: {g:?} → {g2:?}");
    // And back down past the bottom.
    drag(&mut h, g2.center(), g2.center() + egui::vec2(0.0, 3000.0));
    h.run_steps(2);
}

/// Issue #301: a stacked panel's header bar opens it with a click anywhere on it, and its −/+
/// button at the bar's right end collapses and expands it.
#[test]
fn a_stacked_panels_whole_header_bar_toggles_it() {
    let (mut h, _, _) = harness();
    use effectcraft_ui_egui::dock;
    let stack = dock::stack(&[(PanelKind::Preview, true, Some(46.0)), (PanelKind::Properties, true, None)]);
    h.state_mut().ui.dock = dock::docked(dock::hsplit(SplitSize::FixedSecond(300.0), dock::tabs(&[PanelKind::Composition], 0), stack));
    h.run_steps(3);
    let open = |h: &Harness<'_, EffectcraftApp>| h.state().ui.dock.is_visible(PanelKind::Preview);
    assert!(open(&h));
    let toggle = rect(&h, "panel.header.Preview");
    let bar = rect(&h, "panel.tab.Preview");
    assert!(toggle.max.x <= bar.max.x + 1.0 && toggle.min.x > bar.center().x, "{toggle:?} {bar:?}");
    click_n(&mut h, toggle.center(), 1);
    assert!(!open(&h), "the button collapses Preview");
    let toggle = rect(&h, "panel.header.Preview");
    click_n(&mut h, toggle.center(), 1);
    assert!(open(&h), "and expands it again");
    let toggle = rect(&h, "panel.header.Preview");
    click_n(&mut h, toggle.center(), 1);
    assert!(!open(&h));
    click_n(&mut h, pos2(bar.min.x + bar.width() * 0.25, bar.center().y), 1);
    assert!(open(&h), "a click on the bar away from the button opens it");
}

/// #302: a panel dropped onto the Properties frame becomes a tab in that frame; its tabs switch
/// the frame's panel, and the Preview frame above keeps showing.
#[test]
fn a_tab_dropped_on_a_frame_joins_it() {
    let (mut h, _, _) = harness();
    let dock = |h: &Harness<'_, EffectcraftApp>| h.state().ui.dock.clone();
    let fx = rect(&h, "panel.tab.EffectControls");
    let props = rect(&h, "panel.Properties");
    drag(&mut h, fx.center(), props.center());
    let group = group_of(&dock(&h), PanelKind::Properties).unwrap();
    assert!(group.starts_with(&[PanelKind::Properties]) && group.contains(&PanelKind::EffectControls), "{group:?}");
    assert!(dock(&h).is_visible(PanelKind::EffectControls) && !dock(&h).is_visible(PanelKind::Properties));
    assert!(dock(&h).is_visible(PanelKind::Preview));
    // The frame's other tab shows Properties again, the frame stays open.
    let tab = rect(&h, "panel.tab.Properties");
    click_n(&mut h, tab.center(), 1);
    assert!(dock(&h).is_visible(PanelKind::Properties) && !dock(&h).is_visible(PanelKind::EffectControls));
    // The layout (saved workspaces) round-trips.
    let s = serde_json::to_string(&dock(&h)).unwrap();
    assert_eq!(serde_json::from_str::<Workspace>(&s).unwrap(), dock(&h));
}

/// The tabs of the group holding `p`, in order.
fn group_of(w: &Workspace, p: PanelKind) -> Option<Vec<PanelKind>> {
    Some(w.group_of(p)).filter(|g| !g.is_empty())
}

fn has(h: &Harness<'_, EffectcraftApp>, id: &str) -> bool {
    h.state().auto.previous.iter().chain(h.state().auto.elements.iter()).any(|e| e.id == id)
}

#[test]
fn open_comps_are_tabs_in_the_timeline_row_and_the_rest_are_in_its_overflow_menu() {
    let mut s = Session::default();
    let names = ["Opening Titles", "Lower Third Left", "Lower Third Right", "Product Shot", "End Card Wide", "Logo Sting Short"];
    let comps: Vec<u64> = names
        .iter()
        .map(|name| s.execute("comp.new", json!({"name": name, "width": 320, "height": 180, "duration": 4})).unwrap()["comp"].as_u64().unwrap())
        .collect();
    s.state.open_comps = vec![ItemId(comps[0])];
    s.state.active_comp = Some(ItemId(comps[0]));
    let mut h = Harness::builder().with_size(egui::vec2(1000.0, 800.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    let alone = rect(&h, "panel.tab.RenderQueue");
    h.state_mut().session.state.open_comps = comps.iter().map(|c| ItemId(*c)).collect();
    h.run_steps(3);
    // The panel tabs keep their width; the comp tabs follow them in the same row, in order.
    let (tl, rq) = (rect(&h, "panel.tab.Timeline"), rect(&h, "panel.tab.RenderQueue"));
    assert!((rq.width() - alone.width()).abs() < 0.5, "Render Queue {rq:?} was {alone:?}");
    let shown: Vec<u64> = comps.iter().copied().filter(|c| has(&h, &format!("panel.tab.Timeline.{c}"))).collect();
    assert!(shown.contains(&comps[0]) && shown.len() < comps.len(), "{shown:?}");
    let mut x = rq.max.x;
    for c in &shown {
        let tab = rect(&h, &format!("panel.tab.Timeline.{c}"));
        assert!(tl.y_range().contains(tab.center().y) && tab.min.x >= x - 0.5, "comp {c} at {tab:?} after x {x}");
        x = tab.max.x;
    }
    // A comp with no room is in the » menu; picking it shows it, and its tab joins the row.
    let hidden = comps.iter().copied().find(|c| !shown.contains(c)).unwrap();
    let more = rect(&h, "panel.tab.Timeline.more");
    assert!(more.min.x >= x - 0.5 && tl.y_range().contains(more.center().y), "{more:?}");
    click_n(&mut h, more.center(), 1);
    let item = rect(&h, &format!("panel.tab.Timeline.more.{hidden}"));
    click_n(&mut h, item.center(), 1);
    assert_eq!(h.state().session.state.active_comp, Some(ItemId(hidden)));
    assert_eq!(open_comps(&h), comps);
    assert!(has(&h, &format!("panel.tab.Timeline.{hidden}")));
}

/// Whether `id` was registered in the last frame (not the one before).
fn drawn(h: &Harness<'_, EffectcraftApp>, id: &str) -> bool {
    h.state().auto.elements.iter().any(|e| e.id == id)
}

#[test]
fn an_inactive_comp_tab_shows_its_close_button_on_hover_and_closes_without_showing_its_comp() {
    let (mut h, pre, main) = harness();
    h.state_mut().session.state.open_comps = vec![ItemId(main), ItemId(pre)];
    h.run_steps(3);
    let (tm, tp) = (rect(&h, &format!("panel.tab.Timeline.{main}")), rect(&h, &format!("panel.tab.Timeline.{pre}")));
    // The shown comp's × is always there; Pre's only while the pointer is over its tab.
    h.event(Event::PointerMoved(pos2(800.0, 900.0)));
    h.run_steps(2);
    assert!(drawn(&h, "panel.tab.Timeline.close") && drawn(&h, &format!("panel.tab.Timeline.{main}.close")));
    assert!(!drawn(&h, &format!("panel.tab.Timeline.{pre}.close")), "no × on an inactive tab the pointer is not over");
    h.event(Event::PointerMoved(tp.center()));
    h.run_steps(2);
    let close = rect(&h, &format!("panel.tab.Timeline.{pre}.close"));
    assert!(tp.contains_rect(close), "{close:?} inside {tp:?}");
    // Every comp tab keeps the ×'s room: hovering changes no width.
    assert_eq!((rect(&h, &format!("panel.tab.Timeline.{main}")), rect(&h, &format!("panel.tab.Timeline.{pre}"))), (tm, tp));
    // Its × closes Pre's Timeline; Main stays shown.
    click_n(&mut h, close.center(), 1);
    assert_eq!(open_comps(&h), vec![main]);
    assert_eq!(h.state().session.state.active_comp, Some(ItemId(main)));
}
