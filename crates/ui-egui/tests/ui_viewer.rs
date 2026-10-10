//! Headless checks for the Composition viewer's interaction parity: bottom bar order, region of
//! interest drawing, rulers and guides, snapping, the shape Pen and motion-path key drags
//! (egui_kittest, UI logic only).

use effectcraft_engine::Session;
use effectcraft_engine::commands::shape_tool::PaintKind;
use effectcraft_engine::project::LayerId;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::state::Tool;
use egui::{Event, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

fn app() -> EffectcraftApp {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "View", "width": 640, "height": 360, "duration": 4})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080", "width": 640, "height": 360})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Box", "color": "#e04020", "width": 80, "height": 80})).unwrap();
    s.execute("edit.deselectAll", json!({})).unwrap();
    EffectcraftApp::new(s)
}

fn harness() -> Harness<'static, EffectcraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| app());
    h.run_steps(3);
    h
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

fn click(h: &mut Harness<'_, EffectcraftApp>, p: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
}

fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(from));
    h.input_mut().events.push(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    for i in 1..=8 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (i as f32 / 8.0)));
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
}

/// Comp pixel → screen point.
fn screen(h: &Harness<'_, EffectcraftApp>, p: [f32; 2]) -> Pos2 {
    let c = rect(h, "viewer.comp");
    let z = c.width() / 640.0;
    c.min + vec2(p[0] * z, p[1] * z)
}

fn selection_harness() -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Selection", "width": 640, "height": 360, "duration": 4})).unwrap();
    for name in ["Back", "Front"] {
        s.execute("layer.newSolid", json!({"name": name, "color": "#406080", "width": 80, "height": 80})).unwrap();
    }
    s.execute("edit.deselectAll", json!({})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(move |_| EffectcraftApp::new(s));
    h.run_steps(3);
    h
}

#[test]
fn viewer_click_skips_hidden_and_unsoloed_layers() {
    for switch in ["video", "solo"] {
        let mut h = selection_harness();
        let comp = h.state().session.active_comp().unwrap();
        let (front, back) = (comp.layers[0].id, comp.layers[1].id);
        let at = screen(&h, [320.0, 180.0]);
        click(&mut h, at);
        assert_eq!(h.state().session.state.selected_layers, vec![front]);
        let s = &mut h.state_mut().session;
        let (target, value) = if switch == "video" { (front, false) } else { (back, true) };
        s.execute("layer.setSwitch", json!({"layers": [target.0], "switch": switch, "value": value})).unwrap();
        s.execute("edit.deselectAll", json!({})).unwrap();
        h.run_steps(40); // Start a new click rather than a double-click.
        click(&mut h, at);
        assert_eq!(h.state().session.state.selected_layers, vec![back], "{switch}");
    }
}

#[test]
fn viewer_marquee_skips_hidden_and_unsoloed_layers() {
    for switch in ["video", "solo"] {
        let mut h = selection_harness();
        let comp = h.state().session.active_comp().unwrap();
        let (front, back) = (comp.layers[0].id, comp.layers[1].id);
        let (target, value) = if switch == "video" { (front, false) } else { (back, true) };
        h.state_mut().session.execute("layer.setSwitch", json!({"layers": [target.0], "switch": switch, "value": value})).unwrap();
        h.run_steps(3);
        let (from, to) = (screen(&h, [250.0, 110.0]), screen(&h, [390.0, 250.0]));
        drag(&mut h, from, to);
        assert_eq!(h.state().session.state.selected_layers, vec![back], "{switch}");
    }
}

#[test]
fn viewer_click_does_not_pick_through_hidden_or_locked_solo_layers() {
    for switch in ["video", "lock"] {
        let mut h = selection_harness();
        let front = h.state().session.active_comp().unwrap().layers[0].id;
        let s = &mut h.state_mut().session;
        s.execute("layer.setSwitch", json!({"layers": [front.0], "switch": "solo", "value": true})).unwrap();
        s.execute("layer.setSwitch", json!({"layers": [front.0], "switch": switch, "value": switch == "lock"})).unwrap();
        h.run_steps(3);
        let at = screen(&h, [320.0, 180.0]);
        click(&mut h, at);
        assert!(h.state().session.state.selected_layers.is_empty(), "{switch}");
    }
}

#[test]
fn viewer_click_ignores_inactive_solo_layers() {
    let mut h = selection_harness();
    let comp = h.state().session.active_comp().unwrap();
    let (front, back) = (comp.layers[0].id, comp.layers[1].id);
    let s = &mut h.state_mut().session;
    s.execute("layer.setSwitch", json!({"layers": [front.0], "switch": "solo", "value": true})).unwrap();
    let cid = s.active_comp_id().unwrap();
    std::sync::Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(front).unwrap().in_point = effectcraft_engine::time::Tick::from_seconds_f64(1.0);
    h.run_steps(3);
    let at = screen(&h, [320.0, 180.0]);
    click(&mut h, at);
    assert_eq!(h.state().session.state.selected_layers, vec![back]);
}

/// A selected layer under the pointer takes the press before the layers in front of it, as in
/// After Effects: it stays selected and a drag moves it. With none selected there, the topmost
/// layer does (#230).
#[test]
fn viewer_press_prefers_a_selected_layer_under_the_pointer() {
    let mut h = selection_harness();
    let comp = h.state().session.active_comp().unwrap();
    let (front, back) = (comp.layers[0].id, comp.layers[1].id);
    h.state_mut().session.execute("layer.select", json!({"layers": [back.0]})).unwrap();
    h.run_steps(3);
    let at = screen(&h, [320.0, 180.0]);
    click(&mut h, at);
    assert_eq!(h.state().session.state.selected_layers, vec![back], "a click keeps the selected layer behind");
    h.run_steps(40);
    let (front0, back0) = (position_and_anchor(&h, front).0, position_and_anchor(&h, back).0);
    drag(&mut h, at, at + vec2(60.0, 0.0));
    assert_eq!(h.state().session.state.selected_layers, vec![back]);
    assert_eq!(position_and_anchor(&h, front).0, front0, "the layer in front stays");
    assert!(position_and_anchor(&h, back).0[0] > back0[0] + 10.0, "the selected layer behind moves");
    h.state_mut().session.execute("edit.deselectAll", json!({})).unwrap();
    h.run_steps(40);
    let at = screen(&h, [300.0, 180.0]);
    click(&mut h, at);
    assert_eq!(h.state().session.state.selected_layers, vec![front], "nothing selected: the topmost layer");
}

#[test]
fn bottom_bar_in_after_effects_order() {
    let h = harness();
    let order = [
        "viewer.magnification",
        "viewer.resolution",
        "viewer.transparency",
        "viewer.masks",
        "viewer.roi",
        "viewer.grid",
        "viewer.channel",
        "viewer.resetExposure",
        "viewer.exposure",
        "viewer.snapshot",
        "viewer.showSnapshot",
        "viewer.fastPreviews",
        "viewer.timecode",
    ];
    let xs: Vec<f32> = order.iter().map(|id| rect(&h, id).min.x).collect();
    assert!(xs.windows(2).all(|w| w[0] < w[1]), "{xs:?}");
    let mag = &h.state().auto.find("viewer.magnification").unwrap().label;
    assert_eq!(mag, "Magnification");
}

#[test]
fn region_of_interest_is_drawn_in_the_viewer() {
    let mut h = harness();
    let at = rect(&h, "viewer.roi").center();
    click(&mut h, at);
    assert!(h.state().ui.viewer.roi_draw);
    let (a, b) = (screen(&h, [100.0, 50.0]), screen(&h, [300.0, 250.0]));
    drag(&mut h, a, b);
    let r = h.state().session.state.region_of_interest.expect("roi");
    assert!((r[0] - 100.0).abs() <= 2.0 && (r[1] - 50.0).abs() <= 2.0 && (r[2] - 200.0).abs() <= 3.0 && (r[3] - 200.0).abs() <= 3.0, "{r:?}");
    assert!(!h.state().ui.viewer.roi_draw);
    h.run_steps(3);
    assert!(h.state().auto.find("viewer.regionOfInterest").is_some());
    // The button clears it again.
    let at = rect(&h, "viewer.roi").center();
    click(&mut h, at);
    assert!(h.state().session.state.region_of_interest.is_none());
}

#[test]
fn rulers_make_guides_and_guides_move() {
    let mut h = harness();
    h.state_mut().ui.viewer.rulers = true;
    h.run_steps(3);
    let top = rect(&h, "viewer.ruler.top");
    let to = screen(&h, [0.0, 120.0]);
    drag(&mut h, pos2(to.x + 200.0, top.center().y), pos2(to.x + 200.0, to.y));
    let g = h.state().session.active_comp().unwrap().guides.clone();
    assert_eq!(g.len(), 1);
    assert!(!g[0].vertical && (g[0].position - 120.0).abs() <= 2.0, "{g:?}");
    // Drag the guide down with the Selection tool.
    let from = pos2(screen(&h, [500.0, 0.0]).x, screen(&h, [0.0, g[0].position as f32]).y);
    let to2 = pos2(from.x, screen(&h, [0.0, 200.0]).y);
    drag(&mut h, from, to2);
    let g = h.state().session.active_comp().unwrap().guides.clone();
    assert!((g[0].position - 200.0).abs() <= 2.0, "{g:?}");
}

#[test]
fn layer_drag_snaps_to_comp_centre_and_ctrl_disables() {
    let mut h = harness();
    let box_id = h.state().session.active_comp().unwrap().layers[0].id;
    h.state_mut().session.execute("prop.set", json!({"layer": box_id.0, "path": "transform/position", "value": [100, 100, 0]})).unwrap();
    h.run_steps(2);
    // Grab the box at its centre and drop it 3 px from the comp centre: it snaps there.
    let from = screen(&h, [100.0, 100.0]);
    let to = screen(&h, [323.0, 182.0]);
    drag(&mut h, from, to);
    let pos = |h: &Harness<'_, EffectcraftApp>| {
        let l = h.state().session.active_comp().unwrap().layer(box_id).unwrap().clone();
        l.props.prop("transform/position").unwrap().value.as_vec3()
    };
    let p = pos(&h);
    assert!((p[0] - 320.0).abs() < 0.01 && (p[1] - 180.0).abs() < 0.01, "{p:?}");
    // Snapping off: the same drag lands where the pointer is.
    h.state_mut().session.execute("view.snapping", json!({"value": false})).unwrap();
    h.state_mut().session.execute("prop.set", json!({"layer": box_id.0, "path": "transform/position", "value": [100, 100, 0]})).unwrap();
    h.run_steps(2);
    drag(&mut h, from, to);
    let p = pos(&h);
    assert!((p[0] - 320.0).abs() > 1.0, "{p:?}");
}

/// #253: a dragged layer's feature nearest the pointer snaps to other layers' edges; Cmd/Ctrl
/// held during the drag turns snapping on while the Snapping checkbox is off (and off while it
/// is on), and the Tools bar's Snapping options turn Snap Edges Extended off, so an edge only
/// snaps along the layer.
#[test]
fn layer_drag_snaps_to_other_layers_edges_ctrl_toggles_and_options_apply() {
    let mut h = harness();
    let box_id = h.state().session.active_comp().unwrap().layers[0].id;
    // A 100×100 target at x 450–550, y 50–150.
    let target = h.state_mut().session.execute("layer.newSolid", json!({"name": "Target", "color": "#20e040", "width": 100, "height": 100})).unwrap()["layer"]
        .as_u64()
        .unwrap();
    let place = |h: &mut Harness<'_, EffectcraftApp>| {
        let s = &mut h.state_mut().session;
        s.execute("prop.set", json!({"layer": target, "path": "transform/position", "value": [500, 100, 0]})).unwrap();
        s.execute("prop.set", json!({"layer": box_id.0, "path": "transform/position", "value": [100, 250, 0]})).unwrap();
        s.execute("edit.deselectAll", json!({})).unwrap();
        h.run_steps(2);
    };
    let pos = |h: &Harness<'_, EffectcraftApp>| {
        let l = h.state().session.active_comp().unwrap().layer(box_id).unwrap().clone();
        l.props.prop("transform/position").unwrap().value.as_vec3()
    };
    // Grab the box by its top right corner (140, 210) and drop it 3 px left of the target's
    // left edge, well below the target: the corner lands on the edge's line (x 450).
    let ctrl = egui::Modifiers { ctrl: true, command: true, ..Default::default() };
    let drop = |h: &mut Harness<'_, EffectcraftApp>, mods: egui::Modifiers| {
        place(h);
        let (from, to) = (screen(h, [138.0, 212.0]), screen(h, [445.0, 232.0]));
        hold_drag(h, from, to, mods);
        pos(h)
    };
    let snapped = |p: [f64; 3]| (p[0] - 410.0).abs() < 0.01;
    let p = drop(&mut h, Default::default());
    assert!(snapped(p), "snapping on: {p:?}");
    let p = drop(&mut h, ctrl);
    assert!(!snapped(p), "Ctrl turns it off: {p:?}");
    h.state_mut().session.execute("view.snapping", json!({"value": false})).unwrap();
    let p = drop(&mut h, Default::default());
    assert!(!snapped(p), "snapping off: {p:?}");
    let p = drop(&mut h, ctrl);
    assert!(snapped(p), "Ctrl turns it on: {p:?}");
    h.state_mut().session.execute("view.snapping", json!({"value": true})).unwrap();
    // Snapping options ▸ Snap Edges Extended off: below the target its edge no longer snaps.
    let menu = rect(&h, "header.snappingOptions").center();
    click(&mut h, menu);
    // After Effects' two options, without per-feature toggles.
    assert!(h.query_by_label("✓ Snap to Features in Collapsed Compositions and Text Layers").is_some());
    for gone in ["✓ Corners", "✓ Anchor Points", "✓ Mask and Shape Path Points"] {
        assert!(h.query_by_label(gone).is_none(), "{gone}");
    }
    let item = h.query_by_label("✓ Snap Edges Extended").expect("the Snapping options menu").rect().center();
    click(&mut h, item);
    assert!(!h.state().session.state.snap_features.edges_extended);
    let p = drop(&mut h, Default::default());
    assert!(!snapped(p), "edges not extended: {p:?}");
}

/// With a shape layer selected the Rectangle tool draws a new group into its Contents (After
/// Effects' behaviour); with nothing selected it draws a new shape layer (#227).
#[test]
fn shape_tool_draws_into_the_selected_shape_layer() {
    let mut h = harness();
    let l = h.state_mut().session.execute("layer.newShape", json!({"kind": "rect", "size": [100, 100], "position": [320, 180]})).unwrap()["layer"]
        .as_u64()
        .unwrap();
    h.state_mut().ui.tool = Tool::Rectangle;
    h.run_steps(2);
    let n0 = h.state().session.active_comp().unwrap().layers.len();
    let (a, b) = (screen(&h, [100.0, 100.0]), screen(&h, [200.0, 160.0]));
    drag(&mut h, a, b);
    let comp = h.state().session.active_comp().unwrap().clone();
    assert_eq!(comp.layers.len(), n0, "no new layer");
    let contents = comp.layer(LayerId(l)).unwrap().props.sub("contents").unwrap().clone();
    let names: Vec<&str> = contents.groups().map(|g| g.name.as_str()).collect();
    assert_eq!(names, ["Rectangle 2", "Rectangle 1"]);
    // Placed where it was drawn (centred near comp (150, 130)), in the layer's space: the layer
    // sits at the comp centre.
    let g = contents.groups().next().unwrap();
    let at = g.sub("transform").unwrap().get("position").unwrap().value.components();
    assert!((at[0] + 170.0).abs() < 8.0 && (at[1] + 50.0).abs() < 8.0, "{at:?}");
    // Nothing selected: a new shape layer.
    h.state_mut().session.execute("edit.deselectAll", json!({})).unwrap();
    let (a, b) = (screen(&h, [400.0, 250.0]), screen(&h, [500.0, 300.0]));
    drag(&mut h, a, b);
    let comp = h.state().session.active_comp().unwrap();
    assert_eq!(comp.layers.len(), n0 + 1);
    assert!(matches!(comp.layers[0].source, effectcraft_engine::project::LayerSource::Shape));
    assert_ne!(comp.layers[0].id, LayerId(l));
}

/// With a shape layer selected and a shape tool active, the Tools bar's Tool Creates Mask makes
/// the tool draw a mask on the layer (and hides Fill and Stroke); Tool Creates Shape draws
/// shapes again (#227).
#[test]
fn tool_creates_mask_draws_a_mask_on_the_selected_shape_layer() {
    let mut h = harness();
    let l = h.state_mut().session.execute("layer.newShape", json!({"kind": "rect", "size": [100, 100], "position": [320, 180]})).unwrap()["layer"]
        .as_u64()
        .unwrap();
    h.state_mut().ui.tool = Tool::Star;
    h.run_steps(2);
    assert!(h.state().auto.find("header.fill").is_some());
    let at = rect(&h, "header.createsMask").center();
    click(&mut h, at);
    assert!(h.state().session.state.shape_tool.creates_mask);
    assert!(h.state().auto.find("header.fill").is_none(), "no Fill or Stroke for masks");
    let (a, b) = (screen(&h, [100.0, 100.0]), screen(&h, [200.0, 200.0]));
    drag(&mut h, a, b);
    let layer = h.state().session.active_comp().unwrap().layer(LayerId(l)).unwrap().clone();
    let masks = layer.props.sub("masks").unwrap();
    assert_eq!(masks.groups().count(), 1, "a mask");
    assert_eq!(masks.groups().next().unwrap().get("path").unwrap().value.as_path().unwrap().vertices.len(), 10, "a star");
    assert_eq!(layer.props.sub("contents").unwrap().groups().count(), 1, "no new shape");
    let at = rect(&h, "header.createsShape").center();
    click(&mut h, at);
    assert!(!h.state().session.state.shape_tool.creates_mask);
}

// ---- the shape tools' drag ghost

/// Run frames until the viewer's background frame has landed, so the comp is painted (and not
/// still black) before the drag begins: the ghost is then the only thing that changes.
fn settle_viewer(h: &mut Harness<'_, EffectcraftApp>) {
    for _ in 0..600 {
        h.step();
        if h.state().frames.inflight() == 0 && h.state().frames.last_ms.lock().map(|v| *v > 0.0).unwrap_or(false) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    h.run_steps(4);
}

/// Press at `from`, drag to `to` with `mods` held and return the frame drawn with the button still
/// down — what the user sees mid-drag, before anything is committed. egui only takes modifiers
/// from [`egui::Event::ModifiersChanged`], so they are set before the press and stay set.
fn mid_drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2, mods: egui::Modifiers) -> image::RgbaImage {
    h.input_mut().events.push(Event::ModifiersChanged(mods));
    h.input_mut().events.push(Event::PointerMoved(from));
    h.input_mut().events.push(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: mods });
    h.step();
    for i in 1..=8 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (i as f32 / 8.0)));
        h.step();
    }
    h.render().expect("the mid-drag frame")
}

/// Release the button where the drag left it.
fn release(h: &mut Harness<'_, EffectcraftApp>, at: Pos2, mods: egui::Modifiers) {
    h.input_mut().events.push(Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: mods });
    h.run_steps(2);
}

/// The pixel at a comp point of a rendered frame.
fn px_at(img: &image::RgbaImage, h: &Harness<'_, EffectcraftApp>, p: [f32; 2]) -> [u8; 3] {
    let s = screen(h, p);
    let p = img.get_pixel(s.x.round() as u32, s.y.round() as u32).0;
    [p[0], p[1], p[2]]
}

/// How different two pixels are, summed over the channels.
fn delta(a: [u8; 3], b: [u8; 3]) -> u32 {
    (0..3).map(|i| (a[i] as i32 - b[i] as i32).unsigned_abs()).sum()
}

/// The Tools-bar Fill as bytes, the colour a filled ghost carries.
fn fill_rgb(h: &Harness<'_, EffectcraftApp>) -> [u8; 3] {
    let c = h.state().session.state.shape_tool.fill.color;
    std::array::from_fn(|i| (c[i].clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// The shape tools show the shape they will draw under the cursor while the drag is still going:
/// filled with the Tools bar's Fill, so it reads at any zoom where the 1 px outline it replaces was
/// invisible, and in the tool's own form — only the Rectangle fills the corner of the box.
#[test]
fn shape_drag_ghost_shows_the_tools_own_shape_while_dragging() {
    let (from, to) = ([140.0, 100.0], [400.0, 300.0]);
    let centre = [270.0, 200.0];
    // 6 comp px inside the box's top-left corner: inside a rectangle, outside every other kind.
    let corner = [146.0, 106.0];
    for (tool, fills_corner) in [(Tool::Rectangle, true), (Tool::RoundedRect, false), (Tool::Ellipse, false), (Tool::Polygon, false), (Tool::Star, false)] {
        let mut h = harness();
        h.state_mut().ui.tool = tool;
        settle_viewer(&mut h);
        let (a, b) = (screen(&h, from), screen(&h, to));
        let before = h.render().expect("the frame before the drag");
        let mid = mid_drag(&mut h, a, b, egui::Modifiers::default());
        let fill = fill_rgb(&h);

        // Drawn while the button is down, and painted with the Fill rather than left to a hairline.
        let was = px_at(&before, &h, centre);
        let now = px_at(&mid, &h, centre);
        assert!(delta(was, now) > 30, "{tool:?}: no ghost drawn mid-drag ({was:?} → {now:?})");
        assert!(delta(now, fill) < delta(was, fill), "{tool:?}: the ghost is not the Fill colour ({now:?} vs {was:?})");

        // The tool's own form: only the Rectangle reaches the corner of its box.
        let was_c = px_at(&before, &h, corner);
        let now_c = px_at(&mid, &h, corner);
        assert_eq!(delta(was_c, now_c) > 30, fills_corner, "{tool:?}: the ghost is not drawn in its own shape");

        release(&mut h, b, egui::Modifiers::default());
    }
}

/// The ghost and the shape the release commits share one normalisation, so nothing it shows is a
/// lie: Shift squares the box and Ctrl/Cmd draws it from the press point (as in After Effects), both live.
#[test]
fn shape_drag_ghost_normalises_the_same_way_as_the_release() {
    let from = [140.0f64, 100.0];
    let to = [400.0f64, 260.0];
    // (mods, centre, size): Shift squares the box from its top-left, Ctrl/Cmd centres it on the press.
    for (mods, centre, size) in [
        (egui::Modifiers::default(), [270.0, 180.0], [260.0, 160.0]),
        (egui::Modifiers::SHIFT, [270.0, 230.0], [260.0, 260.0]),
        (egui::Modifiers::COMMAND, [140.0, 100.0], [520.0, 320.0]),
        (egui::Modifiers { shift: true, ..egui::Modifiers::COMMAND }, [140.0, 100.0], [520.0, 520.0]),
    ] {
        let mut h = harness();
        h.state_mut().ui.tool = Tool::Ellipse;
        settle_viewer(&mut h);
        let (a, b) = (screen(&h, from.map(|v| v as f32)), screen(&h, to.map(|v| v as f32)));
        let before = h.render().expect("the frame before the drag");
        // Measured with the button down, so this is the ghost and not the committed shape.
        let mid = mid_drag(&mut h, a, b, mods);
        let on = [centre[0] as f32 + size[0] as f32 / 4.0, centre[1] as f32 + size[1] as f32 / 4.0];
        assert!(delta(px_at(&before, &h, on), px_at(&mid, &h, on)) > 30, "{mods:?}: the ghost is not where it will land");
        release(&mut h, b, mods);

        // And the shape it commits is that box: a new shape layer carries the position, its
        // group sits at its own centre's origin.
        let comp = h.state().session.active_comp().unwrap().clone();
        let layer = comp.layers.first().expect("a new shape layer").clone();
        assert!(matches!(layer.source, effectcraft_engine::project::LayerSource::Shape), "{mods:?}: no shape layer");
        let at = layer.transform().unwrap().get("position").unwrap().value.components();
        assert!((at[0] - centre[0]).abs() < 1.0 && (at[1] - centre[1]).abs() < 1.0, "{mods:?}: at {at:?}, not {centre:?}");
        let group = layer.props.sub("contents").unwrap().groups().next().unwrap().clone();
        let path = group.sub("contents").unwrap().sub("ellipse").unwrap();
        let s = path.get("size").unwrap().value.components();
        assert!((s[0] - size[0]).abs() < 1.0 && (s[1] - size[1]).abs() < 1.0, "{mods:?}: sized {s:?}, not {size:?}");
    }
}

/// The ghost says where the shape lands: a mask on the selected layer is drawn in the mask colour
/// and not painted with the Fill, where a shape drawn into the selected shape layer is.
#[test]
fn shape_drag_ghost_shows_where_the_shape_will_land() {
    let (from, to) = ([140.0, 100.0], [400.0, 300.0]);
    let centre = [270.0, 200.0];

    // Nothing selected: the shape becomes a new layer, painted with the Fill.
    let mut h = harness();
    h.state_mut().ui.tool = Tool::Ellipse;
    settle_viewer(&mut h);
    let (a, b) = (screen(&h, from), screen(&h, to));
    let shape_mid = mid_drag(&mut h, a, b, egui::Modifiers::default());
    let shape_px = px_at(&shape_mid, &h, centre);
    let fill = fill_rgb(&h);
    assert!(delta(shape_px, fill) < delta(shape_px, [0xff, 0xc0, 0x00]), "a shape ghost is painted with the Fill, not the mask colour");
    release(&mut h, b, egui::Modifiers::default());

    // A solid selected: the same drag is a mask on it, in the mask colour.
    let mut h = harness();
    let plate = h.state().session.active_comp().unwrap().layers.iter().find(|l| l.name == "Box").unwrap().id;
    h.state_mut().session.execute("layer.select", json!({"layers": [plate.0]})).unwrap();
    h.state_mut().ui.tool = Tool::Ellipse;
    settle_viewer(&mut h);
    let (a, b) = (screen(&h, from), screen(&h, to));
    let mask_mid = mid_drag(&mut h, a, b, egui::Modifiers::default());
    let mask_px = px_at(&mask_mid, &h, centre);
    assert!(delta(mask_px, shape_px) > 20, "a mask ghost must not look like the shape ghost ({mask_px:?} vs {shape_px:?})");
    release(&mut h, b, egui::Modifiers::default());
    let comp = h.state().session.active_comp().unwrap();
    assert_eq!(comp.layer(plate).unwrap().props.sub("masks").unwrap().groups().count(), 1, "the drag drew a mask");
}

/// Dragging a layer moves its picture while the button is still down. Every pointer move replaces
/// the comp's content identity, so the exact frame on screen is re-rendered at every step: waiting
/// for it would freeze the viewer on the last completed frame for the whole drag.
#[test]
fn a_layer_drag_moves_the_picture_while_the_button_is_down() {
    let mut h = harness();
    let box_ = h.state().session.active_comp().unwrap().layers.iter().find(|l| l.name == "Box").unwrap().id;
    h.state_mut().session.execute("layer.select", json!({"layers": [box_.0]})).unwrap();
    settle_viewer(&mut h);
    let from = screen(&h, [320.0, 180.0]);
    let to = screen(&h, [520.0, 180.0]);
    h.input_mut().events.push(Event::PointerMoved(from));
    h.input_mut().events.push(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    // Where the layer is not (yet): the plate behind it.
    let plate = px_at(&h.render().unwrap(), &h, [440.0, 200.0]);

    // Drag in small steps, still holding the button, and give the renders a chance to land.
    let mut moved = false;
    for i in 1..=40 {
        let p = from + (to - from) * (i as f32 / 40.0);
        h.input_mut().events.push(Event::PointerMoved(p));
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(15));
        // 30 comp px behind the pointer: inside the layer, well away from its outline, its anchor
        // icon and where it started.
        let at = px_at(&h.render().expect("mid-drag frame"), &h, [320.0 + 200.0 * (i as f32 / 40.0) - 30.0, 200.0]);
        if i >= 30 && delta(at, plate) > 30 {
            moved = true;
            break;
        }
    }
    assert!(moved, "the layer's pixels did not follow the pointer while the button was down");
    h.input_mut().events.push(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
}

/// Clicking the word "Fill" opens Fill Options: a radial gradient in Multiply at 40% paints the
/// next shape drawn (#227).
#[test]
fn fill_options_paint_the_next_shape() {
    let mut h = harness();
    h.state_mut().ui.tool = Tool::Ellipse;
    h.run_steps(2);
    let at = rect(&h, "header.fillOptions").center();
    click(&mut h, at);
    let at = rect(&h, "header.fillOptions.radial").center();
    click(&mut h, at);
    let at = rect(&h, "header.fillOptions.blend").center();
    click(&mut h, at);
    let at = h.get_by_label("Multiply").rect().center();
    click(&mut h, at);
    let at = rect(&h, "header.fillOptions.opacity").center();
    click(&mut h, at);
    h.input_mut().events.push(Event::Text("40".into()));
    h.step();
    h.input_mut().events.push(Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
    h.run_steps(2);
    let fill = h.state().session.state.shape_tool.fill.clone();
    assert_eq!((fill.kind, fill.blend, fill.opacity), (PaintKind::Radial, effectcraft_engine::color::BlendMode::Multiply, 40.0));
    h.input_mut().events.push(Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
    h.run_steps(3);
    assert!(h.state().auto.find("header.fillOptions.radial").is_none(), "closed");
    let (a, b) = (screen(&h, [100.0, 100.0]), screen(&h, [220.0, 160.0]));
    drag(&mut h, a, b);
    let layer = h.state().session.active_comp().unwrap().layers[0].clone();
    let g = layer.props.sub("contents").unwrap().groups().next().unwrap().sub("contents").unwrap().clone();
    assert_eq!(g.groups().map(|x| x.match_id.as_str()).collect::<Vec<_>>(), ["ellipse", "gfill"]);
    let gfill = g.groups().find(|x| x.match_id == "gfill").unwrap();
    let multiply = effectcraft_engine::color::BlendMode::ALL.iter().position(|m| *m == effectcraft_engine::color::BlendMode::Multiply).unwrap() as u32;
    assert_eq!(gfill.get("type").unwrap().value, effectcraft_keyframe::Value::Enum(1));
    assert_eq!(gfill.get("blend").unwrap().value, effectcraft_keyframe::Value::Enum(multiply));
    assert_eq!(gfill.get("opacity").unwrap().value.as_f64(), 40.0);
}

#[test]
fn shape_pen_draws_a_closed_shape_layer() {
    let mut h = harness();
    h.state_mut().ui.tool = Tool::Pen;
    h.run_steps(2);
    let n0 = h.state().session.active_comp().unwrap().layers.len();
    for p in [[200.0, 100.0], [400.0, 120.0], [300.0, 260.0], [200.0, 100.0]] {
        let at = screen(&h, p);
        click(&mut h, at);
    }
    let comp = h.state().session.active_comp().unwrap().clone();
    assert_eq!(comp.layers.len(), n0 + 1);
    let l = &comp.layers[0];
    assert!(matches!(l.source, effectcraft_engine::project::LayerSource::Shape));
    let g = l.props.sub("contents").unwrap().groups().next().unwrap();
    assert_eq!(g.name, "Shape 1");
    let path = g.sub("contents").unwrap().groups().next().unwrap();
    let sp = path.get("path").unwrap().value.as_path().unwrap().clone();
    assert_eq!(sp.vertices.len(), 3);
    assert!(sp.closed);
    assert!((sp.vertices[0][0] - (200.0 - 320.0)).abs() < 1.5, "{:?}", sp.vertices);
    // G cycles the Pen slot: Add Vertex, Delete Vertex, Convert Vertex, Mask Feather, Pen.
    let ctx = h.ctx.clone();
    let mut seen = vec![];
    for _ in 0..5 {
        effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "tool.pen", json!({})).unwrap();
        seen.push(h.state().ui.tool);
    }
    assert_eq!(seen, vec![Tool::PenAdd, Tool::PenDelete, Tool::PenConvert, Tool::MaskFeather, Tool::Pen]);
    // Add Vertex on the first segment.
    h.state_mut().ui.tool = Tool::PenAdd;
    h.run_steps(2);
    let at = screen(&h, [300.0, 110.0]);
    click(&mut h, at);
    let comp = h.state().session.active_comp().unwrap().clone();
    let sp = comp.layers[0]
        .props
        .sub("contents")
        .unwrap()
        .groups()
        .next()
        .unwrap()
        .sub("contents")
        .unwrap()
        .groups()
        .next()
        .unwrap()
        .get("path")
        .unwrap()
        .value
        .as_path()
        .unwrap()
        .clone();
    assert_eq!(sp.vertices.len(), 4);
}

#[test]
fn motion_path_key_drag_edits_that_key() {
    let mut h = harness();
    let box_id: LayerId = h.state().session.active_comp().unwrap().layers[0].id;
    let s = &mut h.state_mut().session;
    s.execute("layer.select", json!({"layers": [box_id.0]})).unwrap();
    s.execute("prop.addKey", json!({"layer": box_id.0, "path": "transform/position", "time": 0.0, "value": [100, 100, 0]})).unwrap();
    s.execute("prop.addKey", json!({"layer": box_id.0, "path": "transform/position", "time": 2.0, "value": [500, 100, 0]})).unwrap();
    s.execute("view.snapping", json!({"value": false})).unwrap();
    s.set_time(effectcraft_engine::time::Tick::from_seconds_f64(1.0));
    h.run_steps(3);
    assert!(h.state().auto.find(&format!("viewer.motionPath.{}.1", box_id.0)).is_some());
    let from = screen(&h, [500.0, 100.0]);
    let to2 = screen(&h, [500.0, 300.0]);
    drag(&mut h, from, to2);
    let l = h.state().session.active_comp().unwrap().layer(box_id).unwrap().clone();
    let k = &l.props.prop("transform/position").unwrap().keys;
    let v1 = k[1].value.as_vec3();
    assert!((v1[1] - 300.0).abs() < 2.0 && (v1[0] - 500.0).abs() < 2.0, "{v1:?}");
    assert_eq!(k[0].value.as_vec3(), [100.0, 100.0, 0.0]);
}

/// The first mask path of layer `name`.
fn mask_path(h: &Harness<'_, EffectcraftApp>, name: &str) -> effectcraft_engine::keyframe::ShapePath {
    let l = h.state().session.active_comp().unwrap().layers.iter().find(|l| l.name == name).unwrap().clone();
    l.masks().unwrap().groups().next().unwrap().get("path").unwrap().value.as_path().unwrap().clone()
}

/// Drawing a mask with the Pen, moving a point and undoing take one action back per undo; a
/// double-click on a point selects all of them in a free-transform box, and dragging inside it
/// (or, once the box is gone, dragging a point) moves the whole mask (#290).
#[test]
fn pen_mask_undoes_step_by_step_and_double_click_moves_the_whole_mask() {
    let mut h = harness();
    let plate = layer_id(&h, "Plate");
    h.state_mut().session.execute("layer.select", json!({"layers": [plate]})).unwrap();
    h.state_mut().session.execute("view.snapping", json!({"value": false})).unwrap();
    h.state_mut().ui.tool = Tool::Pen;
    h.run_steps(2);
    let corners = [[100.0, 60.0], [500.0, 60.0], [500.0, 300.0], [100.0, 300.0]];
    for p in corners.iter().chain([&corners[0]]) {
        let at = screen(&h, [p[0] as f32, p[1] as f32]);
        click(&mut h, at);
    }
    let drawn = mask_path(&h, "Plate");
    let near = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 1.0 && (a[1] - b[1]).abs() < 1.0;
    assert!(drawn.closed && drawn.vertices.len() == 4 && drawn.vertices.iter().zip(&corners).all(|(v, c)| near(*v, *c)), "{drawn:?}");
    h.state_mut().ui.tool = Tool::Selection;
    h.run_steps(2);
    // Move one point: one undo step puts it back, the next reopens the path, the next removes the
    // last point.
    let (from, to) = (screen(&h, [500.0, 60.0]), screen(&h, [520.0, 80.0]));
    drag(&mut h, from, to);
    let moved = mask_path(&h, "Plate");
    assert!(near(moved.vertices[1], [520.0, 80.0]) && moved.vertices[0] == drawn.vertices[0], "{moved:?}");
    let undo = |h: &mut Harness<'_, EffectcraftApp>| {
        h.state_mut().session.execute("edit.undo", json!({})).unwrap();
        h.run_steps(2);
        mask_path(h, "Plate")
    };
    assert_eq!(undo(&mut h), drawn);
    assert!(!undo(&mut h).closed);
    assert_eq!(undo(&mut h).vertices.len(), 3);
    for _ in 0..3 {
        h.state_mut().session.execute("edit.redo", json!({})).unwrap();
    }
    h.run_steps(2);
    // Double-click a point: all of them selected, in a free-transform box.
    let at = screen(&h, [100.0, 300.0]);
    h.input_mut().events.push(Event::PointerMoved(at));
    h.step();
    for pressed in [true, false, true, false] {
        h.input_mut().events.push(Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
    }
    h.run_steps(3);
    assert_eq!(h.state().session.state.selected_vertices.len(), 4);
    assert!(h.state().auto.find("viewer.freeTransform.handle.0").is_some(), "the free-transform box is up");
    // Dragging inside the box moves the whole mask.
    let (from, to) = (screen(&h, [300.0, 120.0]), screen(&h, [310.0, 130.0]));
    drag(&mut h, from, to);
    let path = mask_path(&h, "Plate");
    assert!(path.vertices.iter().zip(&moved.vertices).all(|(v, c)| near(*v, [c[0] + 10.0, c[1] + 10.0])), "{path:?}");
    // A click away ends the box; the points stay selected and dragging one moves them all.
    let away = screen(&h, [600.0, 340.0]);
    click(&mut h, away);
    h.run_steps(2);
    assert!(h.state().auto.find("viewer.freeTransform.handle.0").is_none());
    let (from, to) = (screen(&h, [110.0, 310.0]), screen(&h, [100.0, 300.0]));
    drag(&mut h, from, to);
    let back = mask_path(&h, "Plate");
    assert!(back.vertices.iter().zip(&moved.vertices).all(|(v, c)| near(*v, *c)), "{back:?}");
}

/// #513: clicking away from a mask's path, even on its own layer, deselects the mask's points;
/// the layer stays selected.
#[test]
fn clicking_off_a_mask_deselects_its_points() {
    let mut h = harness();
    let plate = layer_id(&h, "Plate");
    let s = &mut h.state_mut().session;
    s.execute("layer.select", json!({"layers": [plate]})).unwrap();
    s.execute("mask.new", json!({"layer": plate, "vertices": [[100.0, 60.0], [500.0, 60.0], [500.0, 300.0], [100.0, 300.0]], "closed": true})).unwrap();
    h.state_mut().ui.tool = Tool::Selection;
    h.run_steps(2);
    let at = screen(&h, [500.0, 60.0]);
    click(&mut h, at);
    assert_eq!(h.state().session.state.selected_vertices.len(), 1, "a click on a point selects it");
    let away = screen(&h, [200.0, 250.0]);
    click(&mut h, away);
    assert!(h.state().session.state.selected_vertices.is_empty(), "a click off the path deselects its points");
    assert_eq!(h.state().session.state.selected_layers, vec![LayerId(plate)], "the layer stays selected");
}

/// A straight two-key motion path shows Bezier handles at both keys, and dragging one pulls the
/// path into a curve (#290).
#[test]
fn a_straight_motion_path_has_handles_to_curve_it() {
    let mut h = harness();
    let box_id: LayerId = h.state().session.active_comp().unwrap().layers[0].id;
    let s = &mut h.state_mut().session;
    s.execute("layer.select", json!({"layers": [box_id.0]})).unwrap();
    s.execute("prop.addKey", json!({"layer": box_id.0, "path": "transform/position", "time": 0.0, "value": [100, 100, 0]})).unwrap();
    s.execute("prop.addKey", json!({"layer": box_id.0, "path": "transform/position", "time": 2.0, "value": [400, 100, 0]})).unwrap();
    s.execute("view.snapping", json!({"value": false})).unwrap();
    s.set_time(effectcraft_engine::time::Tick::from_seconds_f64(1.0));
    h.run_steps(3);
    let (out0, in1) = (format!("viewer.motionPath.{}.0.out", box_id.0), format!("viewer.motionPath.{}.1.in", box_id.0));
    assert!(h.state().auto.find(&in1).is_some(), "the second key has a handle");
    // The first key's auto-Bezier handle lies a sixth of the way along the path, as After Effects
    // puts it: drag it down 150 px.
    let from = rect(&h, &out0).center();
    assert!(from.distance(screen(&h, [150.0, 100.0])) < 2.0, "{from:?}");
    let to = screen(&h, [150.0, 250.0]);
    drag(&mut h, from, to);
    let p = h.state().session.active_comp().unwrap().layer(box_id).unwrap().props.prop("transform/position").unwrap().clone();
    assert!(!p.keys[0].spatial_auto);
    let t = p.keys[0].spatial_out;
    assert!((t[0] - 50.0).abs() < 2.0 && (t[1] - 150.0).abs() < 2.0, "{t:?}");
    let mid = p.value_at(effectcraft_engine::time::Tick::from_seconds_f64(1.0)).as_vec3();
    assert!(mid[1] > 130.0, "the path curves: {mid:?}");
}

#[test]
fn graph_editor_transform_box_scales_selected_keys_in_time() {
    let mut h = harness();
    let box_id: LayerId = h.state().session.active_comp().unwrap().layers[0].id;
    let s = &mut h.state_mut().session;
    s.execute("layer.select", json!({"layers": [box_id.0]})).unwrap();
    for (t, v) in [(0.0, 0.0), (1.0, 50.0), (2.0, 100.0)] {
        s.execute("prop.addKey", json!({"layer": box_id.0, "path": "transform/opacity", "time": t, "value": v})).unwrap();
    }
    let uid = s.active_comp().unwrap().layer(box_id).unwrap().props.prop("transform/opacity").unwrap().uid;
    s.execute("prop.select", json!({"layer": box_id.0, "prop": uid})).unwrap();
    let keys: Vec<_> = [0.0, 1.0, 2.0].iter().map(|t| json!({"layer": box_id.0, "prop": uid, "time": t})).collect();
    s.execute("keys.select", json!({"keys": keys})).unwrap();
    h.state_mut().ui.timeline.graph_editor = true;
    h.run_steps(4);
    for id in ["timeline.graph.snap", "timeline.graph.reference", "timeline.graph.transformBox", "timeline.graph.transformBox.5"] {
        assert!(h.state().auto.find(id).is_some(), "missing {id}");
    }
    // Drag the right edge handle to the right: the keys spread out in time, the first stays.
    let r = rect(&h, "timeline.graph.transformBox.5");
    let k0 = rect(&h, &format!("timeline.graph.key.{uid}.0.0")).center();
    let k2 = rect(&h, &format!("timeline.graph.key.{uid}.0.2")).center();
    let to = r.center() + vec2((k2.x - k0.x) * 0.5, 0.0);
    drag(&mut h, r.center(), to);
    let p = h.state().session.active_comp().unwrap().layer(box_id).unwrap().props.prop("transform/opacity").unwrap().clone();
    let ts: Vec<f64> = p.keys.iter().map(|k| k.time.seconds()).collect();
    assert_eq!(ts[0], 0.0);
    assert!(ts[2] > 2.5 && ts[2] < 3.5, "{ts:?}");
    assert_eq!(p.keys.iter().map(|k| k.value.as_f64()).collect::<Vec<_>>(), vec![0.0, 50.0, 100.0]);
    let undo = h.state().session.history.undo.iter().filter(|(l, _)| l == "Transform Keyframes").count();
    assert_eq!(undo, 1, "one undo step per drag");
}

#[test]
fn timeline_alt_drag_scales_a_key_group_in_time() {
    let mut h = harness();
    let box_id: LayerId = h.state().session.active_comp().unwrap().layers[0].id;
    let s = &mut h.state_mut().session;
    s.execute("layer.select", json!({"layers": [box_id.0]})).unwrap();
    for (t, v) in [(0.0, 0.0), (1.0, 50.0), (2.0, 100.0)] {
        s.execute("prop.addKey", json!({"layer": box_id.0, "path": "transform/opacity", "time": t, "value": v})).unwrap();
    }
    let uid = s.active_comp().unwrap().layer(box_id).unwrap().props.prop("transform/opacity").unwrap().uid;
    s.execute("keys.selectAll", json!({"layers": [box_id.0]})).unwrap();
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "timeline.reveal.opacity", json!({})).unwrap();
    h.run_steps(4);
    let mut ks: Vec<Pos2> =
        h.state().auto.query(&format!("timeline.key.{uid}.")).iter().map(|e| pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0)).collect();
    ks.sort_by(|a, b| a.x.total_cmp(&b.x));
    assert_eq!(ks.len(), 3, "{ks:?}");
    let (k0, k2) = (ks[0], ks[2]);
    // Alt-drag the last key to where 3 s would be: the group scales by 1.5 about the first key.
    let to = pos2(k0.x + (k2.x - k0.x) * 1.5, k2.y);
    let alt = egui::Modifiers { alt: true, ..Default::default() };
    h.input_mut().events.push(Event::PointerMoved(k2));
    h.step();
    h.input_mut().events.push(Event::ModifiersChanged(alt));
    h.input_mut().events.push(Event::PointerButton { pos: k2, button: egui::PointerButton::Primary, pressed: true, modifiers: alt });
    h.step();
    for i in 1..=8 {
        h.input_mut().events.push(Event::PointerMoved(k2 + (to - k2) * (i as f32 / 8.0)));
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: alt });
    h.run_steps(2);
    let p = h.state().session.active_comp().unwrap().layer(box_id).unwrap().props.prop("transform/opacity").unwrap().clone();
    let ts: Vec<f64> = p.keys.iter().map(|k| k.time.seconds()).collect();
    assert_eq!(ts.len(), 3);
    assert_eq!(ts[0], 0.0);
    assert!((ts[2] - 3.0).abs() < 0.1 && (ts[1] - 1.5).abs() < 0.1, "{ts:?}");
}

#[test]
fn mask_feather_tool_adds_and_drags_feather_points() {
    let mut h = harness();
    // A mask on the Box (80×80 at the comp centre, comp 280…360 × 140…220): layer 10…70.
    let (box_id, mask) = {
        let s = &mut h.state_mut().session;
        let b = s.active_comp().unwrap().layers.iter().find(|l| l.name == "Box").unwrap().id;
        let m = s.execute("mask.new", json!({"layer": b.0, "vertices": [[10, 10], [70, 10], [70, 70], [10, 70]], "closed": true})).unwrap()["mask"]
            .as_u64()
            .unwrap();
        s.execute("layer.select", json!({"layers": [b.0]})).unwrap();
        (b, m)
    };
    h.state_mut().ui.tool = Tool::MaskFeather;
    h.run_steps(3);
    // Press on the top edge (comp 320,150) and drag 12 px up: an outer feather point of ≈12 px.
    let from = screen(&h, [320.0, 150.0]);
    let z = rect(&h, "viewer.comp").width() / 640.0;
    drag(&mut h, from, from - vec2(0.0, 12.0 * z));
    let pts = h.state_mut().session.execute("mask.featherPoint.list", json!({"layer": box_id.0, "mask": mask})).unwrap();
    let r = pts["points"][0]["radius"].as_f64().unwrap();
    assert!((r - 12.0).abs() < 1.5, "{pts}");
    assert_eq!(pts["points"][0]["segment"], json!(0));
    // The handle is registered; dragging it inwards makes it an inner feather point.
    h.run_steps(2);
    let handle = rect(&h, &format!("viewer.mask.{mask}.feather.0")).center();
    let to = screen(&h, [320.0, 160.0]);
    drag(&mut h, handle, to);
    let pts = h.state_mut().session.execute("mask.featherPoint.list", json!({"layer": box_id.0, "mask": mask})).unwrap();
    let r = pts["points"][0]["radius"].as_f64().unwrap();
    assert!((r + 10.0).abs() < 1.5, "{pts}");
    assert_eq!(pts["points"].as_array().unwrap().len(), 1);
    // One drag = one undo step each.
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    let pts = h.state_mut().session.execute("mask.featherPoint.list", json!({"layer": box_id.0, "mask": mask})).unwrap();
    assert!(pts["points"][0]["radius"].as_f64().unwrap() > 0.0);
}

#[test]
fn region_of_interest_resizes_by_its_handles() {
    let mut h = harness();
    h.state_mut().session.execute("view.setRegionOfInterest", json!({"rect": [100, 50, 200, 200]})).unwrap();
    h.run_steps(3);
    // Bottom-right corner (handle 2) to (400, 300): the top-left stays.
    let c = rect(&h, "viewer.regionOfInterest.handle.2").center();
    let to = screen(&h, [400.0, 300.0]);
    drag(&mut h, c, to);
    let r = h.state().session.state.region_of_interest.expect("roi");
    assert!((r[0] - 100.0).abs() <= 1.0 && (r[1] - 50.0).abs() <= 1.0 && (r[2] - 300.0).abs() <= 2.0 && (r[3] - 250.0).abs() <= 2.0, "{r:?}");
    // The left edge (handle 7) only moves x.
    h.run_steps(2);
    let c = rect(&h, "viewer.regionOfInterest.handle.7").center();
    let to = pos2(screen(&h, [150.0, 0.0]).x, c.y + 40.0);
    drag(&mut h, c, to);
    let r = h.state().session.state.region_of_interest.expect("roi");
    assert!((r[0] - 150.0).abs() <= 1.0 && (r[1] - 50.0).abs() <= 1.0 && (r[2] - 250.0).abs() <= 2.0 && (r[3] - 250.0).abs() <= 2.0, "{r:?}");
}

#[test]
fn pan_behind_snaps_the_anchor_point() {
    let mut h = harness();
    let box_id = h.state().session.active_comp().unwrap().layers[0].id;
    h.state_mut().session.execute("prop.set", json!({"layer": box_id.0, "path": "transform/position", "value": [100, 100, 0]})).unwrap();
    h.state_mut().session.execute("layer.select", json!({"layers": [box_id.0]})).unwrap();
    h.state_mut().ui.tool = Tool::PanBehind;
    h.run_steps(2);
    // Drag the anchor (at the box centre) to 3 px from the comp centre: it snaps there, and the
    // position follows so the box doesn't move.
    let (from, to) = (screen(&h, [100.0, 100.0]), screen(&h, [323.0, 182.0]));
    hold_drag(&mut h, from, to, Default::default());
    let (p, a) = position_and_anchor(&h, box_id);
    assert!((p[0] - 320.0).abs() < 0.01 && (p[1] - 180.0).abs() < 0.01, "anchor point snapped to the comp centre: {p:?}");
    assert!((a[0] - 260.0).abs() < 0.01 && (a[1] - 120.0).abs() < 0.01, "{a:?}");
}

/// Issue #147: Pan Behind snaps the anchor to its own layer's corners, edge midpoints and centre
/// (only the anchor itself is left out: the box stays put while the anchor moves).
#[test]
fn pan_behind_snaps_the_anchor_to_its_own_layer() {
    let mut h = harness();
    let box_id = h.state().session.active_comp().unwrap().layers[0].id;
    h.state_mut().session.execute("prop.set", json!({"layer": box_id.0, "path": "transform/position", "value": [100, 100, 0]})).unwrap();
    h.state_mut().session.execute("layer.select", json!({"layers": [box_id.0]})).unwrap();
    h.state_mut().ui.tool = Tool::PanBehind;
    h.run_steps(2);
    // The 80×80 box spans (60, 60)–(140, 140). Its anchor, dragged to 3 px from the top-left
    // corner, lands on it.
    let (from, to) = (screen(&h, [100.0, 100.0]), screen(&h, [63.0, 62.0]));
    hold_drag(&mut h, from, to, Default::default());
    let (p, a) = position_and_anchor(&h, box_id);
    assert!(a[0].abs() < 0.01 && a[1].abs() < 0.01, "anchor point snapped to the corner: {a:?}");
    assert!((p[0] - 60.0).abs() < 0.01 && (p[1] - 60.0).abs() < 0.01, "the box stays put: {p:?}");
    // And back to near the centre: it snaps there.
    let (from, to) = (screen(&h, [61.0, 61.0]), screen(&h, [103.0, 98.0]));
    hold_drag(&mut h, from, to, Default::default());
    let (p, a) = position_and_anchor(&h, box_id);
    assert!((a[0] - 40.0).abs() < 0.01 && (a[1] - 40.0).abs() < 0.01, "anchor point snapped to the centre: {a:?}");
    assert!((p[0] - 100.0).abs() < 0.01 && (p[1] - 100.0).abs() < 0.01, "{p:?}");
}

/// Issue #162: with Pan Behind, Alt-drag moves the anchor point alone (the layer shifts, its
/// position stays), Shift keeps the move to one axis, and Ctrl+double-clicking the tool's button
/// centres the anchor point in the layer content.
#[test]
fn pan_behind_alt_moves_the_anchor_alone_shift_constrains_and_the_tool_button_centres_it() {
    let mut h = harness();
    let box_id = h.state().session.active_comp().unwrap().layers[0].id;
    h.state_mut().session.execute("prop.set", json!({"layer": box_id.0, "path": "transform/position", "value": [100, 100, 0]})).unwrap();
    h.state_mut().session.execute("layer.select", json!({"layers": [box_id.0]})).unwrap();
    h.state_mut().session.execute("view.snapping", json!({"value": false})).unwrap();
    h.state_mut().ui.tool = Tool::PanBehind;
    h.run_steps(2);
    let close = |a: [f64; 3], b: [f64; 2]| (a[0] - b[0]).abs() < 0.01 && (a[1] - b[1]).abs() < 0.01;
    // Alt: the anchor point moves by the drag, Position stays.
    let (from, to) = (screen(&h, [100.0, 100.0]), screen(&h, [120.0, 105.0]));
    hold_drag(&mut h, from, to, egui::Modifiers::ALT);
    let (p, a) = position_and_anchor(&h, box_id);
    assert!(close(a, [60.0, 45.0]) && close(p, [100.0, 100.0]), "Alt moves the anchor point alone: {a:?} {p:?}");
    // Shift: the move keeps to the axis dragged along most (Position compensates).
    h.state_mut().session.execute("prop.set", json!({"layer": box_id.0, "path": "transform/anchor", "value": [40, 40, 0]})).unwrap();
    h.run_steps(2);
    let (from, to) = (screen(&h, [100.0, 100.0]), screen(&h, [130.0, 110.0]));
    hold_drag(&mut h, from, to, egui::Modifiers::SHIFT);
    let (p, a) = position_and_anchor(&h, box_id);
    assert!(close(a, [70.0, 40.0]) && close(p, [130.0, 100.0]), "Shift keeps to x: {a:?} {p:?}");
    // Ctrl+double-click the tool's button: Center Anchor Point in Layer Content (the layer stays).
    let ctrl = egui::Modifiers { ctrl: true, command: true, ..Default::default() };
    let b = rect(&h, "tools.PanBehind").center();
    h.input_mut().events.push(Event::ModifiersChanged(ctrl));
    h.input_mut().events.push(Event::PointerMoved(b));
    h.step();
    for _ in 0..2 {
        h.input_mut().events.push(Event::PointerButton { pos: b, button: egui::PointerButton::Primary, pressed: true, modifiers: ctrl });
        h.input_mut().events.push(Event::PointerButton { pos: b, button: egui::PointerButton::Primary, pressed: false, modifiers: ctrl });
    }
    h.run_steps(2);
    h.input_mut().events.push(Event::ModifiersChanged(Default::default()));
    h.step();
    let (p, a) = position_and_anchor(&h, box_id);
    assert!(close(a, [40.0, 40.0]) && close(p, [100.0, 100.0]), "anchor point centred: {a:?} {p:?}");
}

/// Drag from `from` to `to` with `modifiers` held, holding the end point a frame (the viewer's
/// gestures apply the pointer of the previous frame).
fn hold_drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2, modifiers: egui::Modifiers) {
    h.input_mut().events.push(Event::ModifiersChanged(modifiers));
    h.input_mut().events.push(Event::PointerMoved(from));
    h.input_mut().events.push(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers });
    h.step();
    for i in 1..=10 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (i.min(8) as f32 / 8.0)));
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers });
    h.run_steps(2);
    h.input_mut().events.push(Event::ModifiersChanged(Default::default()));
    h.step();
}

/// A layer's Position and Anchor Point.
fn position_and_anchor(h: &Harness<'_, EffectcraftApp>, layer: LayerId) -> ([f64; 3], [f64; 3]) {
    let l = h.state().session.active_comp().unwrap().layer(layer).unwrap().clone();
    (l.props.prop("transform/position").unwrap().value.as_vec3(), l.props.prop("transform/anchor").unwrap().value.as_vec3())
}

#[test]
fn reference_axes_toggle_from_the_grid_menu() {
    let mut h = harness();
    // A 3D layer makes the axes relevant.
    let box_id = h.state().session.active_comp().unwrap().layers[0].id;
    h.state_mut().session.execute("layer.setSwitch", json!({"layers": [box_id.0], "switch": "threeD", "value": true})).unwrap();
    h.run_steps(3);
    assert!(h.state().auto.find("viewer.referenceAxes").is_some(), "on by default (Settings ▸ 3D)");
    // Issue #64: one compass, not two.
    assert_eq!(h.state().auto.query("viewer.referenceAxes").len(), 1);
    let g = rect(&h, "viewer.grid").center();
    click(&mut h, g);
    h.run_steps(2);
    let item = rect(&h, "viewer.gridItem.6").center();
    click(&mut h, item);
    h.run_steps(3);
    assert!(!h.state().session.prefs.three_d.show_reference_axes);
    assert!(h.state().auto.find("viewer.referenceAxes").is_none());
}

/// A Box layer pinned with two Position pins and a Bend pin in the middle (Bend tool active).
fn puppet_harness() -> (Harness<'static, EffectcraftApp>, u64, u64) {
    let mut h = harness();
    let s = &mut h.state_mut().session;
    // No full-frame Plate behind the Box: the comp around the Box is empty, like the canvas
    // around a character.
    s.execute("edit.clear", json!({"layers": ["Plate"]})).unwrap();
    s.execute("layer.select", json!({"layers": ["Box"]})).unwrap();
    let id = s.state.selected_layers[0].0;
    s.execute("puppet.addPin", json!({"layer": id, "position": [10, 40]})).unwrap();
    s.execute("puppet.addPin", json!({"layer": id, "position": [70, 40]})).unwrap();
    let bend = s.execute("puppet.addPin", json!({"layer": id, "kind": "bend", "position": [40, 40]})).unwrap()["pin"].as_u64().unwrap();
    h.state_mut().ui.tool = Tool::PuppetBend;
    h.run_steps(3);
    (h, id, bend)
}

fn pin_value(h: &Harness<'_, EffectcraftApp>, layer: u64, pin: u64, prop: &str) -> f64 {
    let l = h.state().session.active_comp().unwrap().layer(LayerId(layer)).unwrap().clone();
    l.props.find_group(pin).unwrap().get(prop).unwrap().value.as_f64()
}

#[test]
fn puppet_bend_pin_rotates_by_its_ring_and_scales_by_its_square() {
    let (mut h, id, bend) = puppet_harness();
    let center = rect(&h, &format!("viewer.puppetPin.{bend}")).center();
    let ring = rect(&h, &format!("viewer.puppetPin.{bend}.rotate")).center();
    let square = rect(&h, &format!("viewer.puppetPin.{bend}.scale")).center();
    // At 0° the square is on the right of the ring, the rotate target opposite it.
    assert!(square.x > center.x + 7.0 && (square.y - center.y).abs() < 0.5, "{square:?} {center:?}");
    // Drag the ring a quarter turn clockwise (left → top): +90°, one undo step.
    let undo_before = h.state().session.history.undo.len();
    drag(&mut h, ring, center + vec2(0.0, -(center.x - ring.x)));
    let r = pin_value(&h, id, bend, "rotation");
    assert!((r - 90.0).abs() < 1.0, "rotation {r}");
    assert_eq!(h.state().session.history.undo.len(), undo_before + 1);
    // The square follows the rotation (now below the pin; the bend carries the pin itself a
    // little); dragging it twice as far from the pin scales 200 %.
    let center = rect(&h, &format!("viewer.puppetPin.{bend}")).center();
    let square = rect(&h, &format!("viewer.puppetPin.{bend}.scale")).center();
    assert!(square.y > center.y + 7.0, "{square:?}");
    drag(&mut h, square, center + (square - center) * 2.0);
    let sc = pin_value(&h, id, bend, "scale");
    assert!((sc - 200.0).abs() < 2.0, "scale {sc}");
}

#[test]
fn puppet_pin_click_selects_and_delete_removes_only_the_pins() {
    let (mut h, id, bend) = puppet_harness();
    let pin = |h: &Harness<'_, EffectcraftApp>| h.state().session.state.selected_props.iter().map(|(_, u)| *u).collect::<Vec<_>>();
    // Clicking a pin selects it (instead of adding a pin on top of it).
    let at = rect(&h, &format!("viewer.puppetPin.{bend}")).center();
    click(&mut h, at);
    assert_eq!(pin(&h), vec![bend]);
    let info = h.state_mut().session.execute("puppet.info", json!({"layer": id})).unwrap();
    assert_eq!(info["meshes"][0]["pins"].as_array().unwrap().len(), 3, "no pin added");
    // Delete removes the pin; the layer stays.
    h.input_mut().events.push(Event::Key { key: egui::Key::Delete, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
    h.run_steps(2);
    let info = h.state_mut().session.execute("puppet.info", json!({"layer": id})).unwrap();
    assert_eq!(info["meshes"][0]["pins"].as_array().unwrap().len(), 2);
    assert!(h.state().session.active_comp().unwrap().layer(LayerId(id)).is_some());
}

/// A pin's Position property and its keys' times (seconds).
fn pin_keys(h: &Harness<'_, EffectcraftApp>, layer: u64, pin: u64) -> (u64, Vec<f64>) {
    let l = h.state().session.active_comp().unwrap().layer(LayerId(layer)).unwrap().clone();
    let pos = l.props.find_group(pin).unwrap().get("position").unwrap().clone();
    (pos.uid, pos.keys.iter().map(|k| k.time.seconds()).collect())
}

/// The keys the Timeline draws for a property, left to right.
fn timeline_keys(h: &Harness<'_, EffectcraftApp>, prop: u64) -> Vec<Pos2> {
    let mut ks: Vec<Pos2> =
        h.state().auto.query(&format!("timeline.key.{prop}.")).iter().map(|e| pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0)).collect();
    ks.sort_by(|a, b| a.x.total_cmp(&b.x));
    ks
}

/// The pin the last click placed (placing a pin selects it).
fn placed_pin(h: &Harness<'_, EffectcraftApp>) -> u64 {
    h.state().session.state.selected_props.last().map(|(_, u)| *u).unwrap()
}

/// #273: U shows each pin's keyframed Position under its pin (Puppet ▸ Mesh 1 ▸ Deform ▸ Puppet
/// Pin 1 ▸ Position) rather than as one more "Position"; a pin moved or placed in the viewer
/// afterwards shows its keys there too (they were made but not shown); and pin keys select, move
/// and delete like any others.
#[test]
fn puppet_pin_keys_show_under_their_pins_in_the_timeline() {
    let (mut h, id, bend) = puppet_harness();
    h.state_mut().ui.tool = Tool::Puppet;
    let pins: Vec<u64> = h.state().auto.previous.iter().filter_map(|e| e.id.strip_prefix("viewer.puppetPin.")?.parse().ok()).filter(|p| *p != bend).collect();
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "anim.reveal", json!({"kind": "keyframes"})).unwrap();
    h.run_steps(3);
    assert_eq!(pins.len(), 2);
    for pin in &pins {
        let row = h.state().auto.find(&format!("timeline.group.{pin}.name")).map(|e| e.label.clone()).unwrap_or_default();
        assert!(row.starts_with("Puppet Pin"), "the pin's row: {row:?}");
        assert_eq!(timeline_keys(&h, pin_keys(&h, id, *pin).0).len(), 1, "its key at 0 s");
    }
    // Moved at 1 s: keyed there, in view.
    h.state_mut().session.set_time(effectcraft_engine::time::Tick::from_seconds_f64(1.0));
    h.run_steps(2);
    let pa = rect(&h, &format!("viewer.puppetPin.{}", pins[0])).center();
    drag(&mut h, pa, pa + vec2(20.0, 10.0));
    let (pos, times) = pin_keys(&h, id, pins[0]);
    assert_eq!(times.len(), 2);
    assert_eq!(timeline_keys(&h, pos).len(), 2, "both keys drawn");
    // A pin placed after U shows with its key too.
    let at = screen(&h, [320.0, 210.0]);
    click(&mut h, at);
    let new = placed_pin(&h);
    assert!(!pins.contains(&new) && new != bend);
    assert!(h.state().auto.find(&format!("timeline.group.{new}.name")).is_some(), "the new pin's row");
    assert_eq!(timeline_keys(&h, pin_keys(&h, id, new).0).len(), 1, "and its key");
    // Click the 1 s key, drag it half a second later, delete it.
    let ks = timeline_keys(&h, pos);
    click(&mut h, ks[1]);
    assert_eq!(h.state().session.state.selected_keys.iter().map(|k| k.prop).collect::<Vec<_>>(), [pos]);
    let half = (ks[1].x - ks[0].x) / 2.0;
    drag(&mut h, ks[1], ks[1] + vec2(half, 0.0));
    let t = pin_keys(&h, id, pins[0]).1;
    assert!((t[1] - 1.5).abs() < 0.05, "moved: {t:?}");
    h.input_mut().events.push(Event::Key { key: egui::Key::Delete, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
    h.run_steps(2);
    assert_eq!(pin_keys(&h, id, pins[0]).1, [0.0], "deleted; the pin stays");
}

/// #273: on a layer twirled open in the Timeline, placing a pin twirls open Effects ▸ Puppet ▸
/// Mesh 1 ▸ Deform ▸ the pin, so its Position key shows; a collapsed layer stays collapsed, as
/// in After Effects.
#[test]
fn placing_a_pin_opens_its_groups_on_a_twirled_open_layer() {
    let mut h = harness();
    let s = &mut h.state_mut().session;
    s.execute("edit.clear", json!({"layers": ["Plate"]})).unwrap();
    s.execute("layer.select", json!({"layers": ["Box"]})).unwrap();
    let id = s.state.selected_layers[0].0;
    h.state_mut().ui.tool = Tool::Puppet;
    h.run_steps(2);
    let at = screen(&h, [300.0, 160.0]);
    click(&mut h, at);
    assert!(h.state().ui.timeline.open_groups.is_empty() && h.state().ui.timeline.open_layers.is_empty(), "collapsed stays collapsed");
    let twirl = rect(&h, &format!("timeline.layer.{id}.twirl")).center();
    click(&mut h, twirl);
    assert!(h.state().ui.timeline.open_layers.contains(&id));
    let at = screen(&h, [340.0, 200.0]);
    click(&mut h, at);
    let pin = placed_pin(&h);
    assert_eq!(timeline_keys(&h, pin_keys(&h, id, pin).0).len(), 1, "the new pin's Position key shows");
}

/// #284: the Puppet tools stay on the layer being rigged: a pin goes on its mesh where the
/// deformation has stretched it past the layer's own bounds (over another layer), and a click on
/// another layer neither selects it nor rigs it.
#[test]
fn puppet_pins_go_on_the_deformed_mesh_and_the_tool_stays_on_the_rigged_layer() {
    let mut h = harness();
    let s = &mut h.state_mut().session;
    s.execute("layer.select", json!({"layers": ["Box"]})).unwrap();
    let (boxl, plate) = (s.state.selected_layers[0], s.active_comp().unwrap().layers[1].id);
    s.execute("puppet.addPin", json!({"layer": boxl.0, "position": [10, 40]})).unwrap();
    let right = s.execute("puppet.addPin", json!({"layer": boxl.0, "position": [70, 40]})).unwrap()["pin"].as_u64().unwrap();
    // Stretch the Box to the right: its right edge (layer x 80, comp x 360) moves well past 400.
    s.execute("puppet.movePin", json!({"layer": boxl.0, "pin": right, "position": [140, 40]})).unwrap();
    h.state_mut().ui.tool = Tool::Puppet;
    h.run_steps(3);
    let pins = |h: &mut Harness<'_, EffectcraftApp>| {
        let info = h.state_mut().session.execute("puppet.info", json!({"layer": boxl.0})).unwrap();
        info["meshes"].as_array().unwrap().iter().map(|m| m["pins"].as_array().unwrap().len()).sum::<usize>()
    };
    let rigged = |h: &Harness<'_, EffectcraftApp>, l: LayerId| {
        h.state().session.active_comp().unwrap().layer(l).unwrap().effects().is_some_and(|f| f.groups().next().is_some())
    };
    // Comp (380, 180) is layer (100, 40): past the Box's own edge, over the Plate, on the
    // stretched mesh.
    let at = screen(&h, [380.0, 180.0]);
    click(&mut h, at);
    assert_eq!(pins(&mut h), 3, "the pin went on the deformed mesh");
    assert_eq!(h.state().session.state.selected_layers, vec![boxl]);
    assert!(!rigged(&h, plate), "the Plate got no Puppet");
    // Far from the Box, on the Plate only: nothing.
    let at = screen(&h, [40.0, 40.0]);
    click(&mut h, at);
    assert_eq!(pins(&mut h), 3);
    assert_eq!(h.state().session.state.selected_layers, vec![boxl], "the tool stays on the Box");
    assert!(!rigged(&h, plate));
}

/// Filled circles painted this frame (flattening nested shape lists).
fn circles(h: &Harness<'_, EffectcraftApp>) -> Vec<egui::epaint::CircleShape> {
    fn walk(s: &egui::Shape, out: &mut Vec<egui::epaint::CircleShape>) {
        match s {
            egui::Shape::Circle(c) => out.push(*c),
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let mut out = vec![];
    for c in &h.output().shapes {
        walk(&c.shape, &mut out);
    }
    out
}

/// Issue #96: a selected puppet pin is filled with its colour and ringed; an unselected one is
/// hollow (dark centre, coloured outline), like selected and unselected mask vertices. They used to
/// differ by 1 px and a slight brightness change.
#[test]
fn selected_puppet_pins_are_filled_and_unselected_hollow() {
    let (mut h, _id, bend) = puppet_harness();
    let pins: Vec<u64> = h.state().auto.previous.iter().filter_map(|e| e.id.strip_prefix("viewer.puppetPin.").and_then(|r| r.parse().ok())).collect();
    let position: Vec<u64> = pins.into_iter().filter(|p| *p != bend).collect();
    let (a, b) = (position[0], position[1]);
    let pa = rect(&h, &format!("viewer.puppetPin.{a}")).center();
    let pb = rect(&h, &format!("viewer.puppetPin.{b}")).center();
    click(&mut h, pa);
    assert_eq!(h.state().session.state.selected_props.iter().map(|(_, u)| *u).collect::<Vec<_>>(), vec![a]);
    // Pointer away from the pins (hover enlarges a pin), then look at the painted pins.
    h.input_mut().events.push(Event::PointerMoved(pos2(5.0, 5.0)));
    h.run_steps(2);
    let col = effectcraft_ui_egui::panels::puppet_tool::pin_color(effectcraft_engine::effects::puppet::PinKind::Position);
    let at = |p: Pos2| circles(&h).into_iter().filter(move |c| c.center.distance(p) < 0.5).collect::<Vec<_>>();
    let dark = |c: egui::Color32| c.r().max(c.g()).max(c.b()) < 0x60;
    let sel = at(pa);
    assert!(sel.iter().any(|c| c.fill == col), "the selected pin is filled with its colour: {sel:?}");
    assert!(sel.iter().any(|c| c.fill == egui::Color32::TRANSPARENT && c.stroke.color == egui::Color32::WHITE), "and ringed in white: {sel:?}");
    let unsel = at(pb);
    assert!(!unsel.is_empty(), "the unselected pin is drawn");
    assert!(unsel.iter().all(|c| c.fill == egui::Color32::TRANSPARENT || dark(c.fill)), "the unselected pin is hollow: {unsel:?}");
    assert!(unsel.iter().any(|c| c.stroke.color == col), "with a coloured outline: {unsel:?}");
}

#[test]
fn puppet_marquee_selects_pins_and_alt_drag_works_over_the_art() {
    let (mut h, _id, bend) = puppet_harness();
    let p1 = rect(&h, &format!("viewer.puppetPin.{bend}")).center();
    let pins: Vec<u64> = h.state().auto.previous.iter().filter_map(|e| e.id.strip_prefix("viewer.puppetPin.").and_then(|r| r.parse().ok())).collect();
    assert_eq!(pins.len(), 3, "{pins:?}");
    // A box around every pin, started on empty canvas (outside all art).
    let comp = rect(&h, "viewer.comp");
    drag(&mut h, comp.min + vec2(4.0, 4.0), comp.max - vec2(4.0, 4.0));
    let mut sel: Vec<u64> = h.state().session.state.selected_props.iter().map(|(_, u)| *u).collect();
    sel.sort();
    let mut want = pins.clone();
    want.sort();
    assert_eq!(sel, want);
    // Alt-drag a small box over the art (around the Bend pin only).
    h.input_mut().events.push(Event::PointerMoved(p1 - vec2(6.0, 6.0)));
    h.input_mut().events.push(Event::PointerButton {
        pos: p1 - vec2(6.0, 6.0),
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::ALT,
    });
    h.step();
    for i in 1..=6 {
        h.input_mut().events.push(Event::PointerMoved(p1 - vec2(6.0, 6.0) + vec2(12.0, 12.0) * (i as f32 / 6.0)));
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton {
        pos: p1 + vec2(6.0, 6.0),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::ALT,
    });
    h.run_steps(2);
    let sel: Vec<u64> = h.state().session.state.selected_props.iter().map(|(_, u)| *u).collect();
    assert_eq!(sel, vec![bend]);
}

/// Drag with `button` from `from` to `to` in steps, then release.
fn drag_with(h: &mut Harness<'_, EffectcraftApp>, button: egui::PointerButton, from: Pos2, to: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(from));
    h.input_mut().events.push(Event::PointerButton { pos: from, button, pressed: true, modifiers: Default::default() });
    h.step();
    for i in 1..=8 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (i as f32 / 8.0)));
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton { pos: to, button, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
}

#[test]
fn middle_and_hand_drags_pan_the_viewer_and_the_pan_stays_after_release() {
    let mut h = harness();
    let c = rect(&h, "viewer.comp").center();
    let before = h.state().ui.viewer.pan;
    // Middle-button drag with the Selection tool: pans, and stays panned after the release.
    drag_with(&mut h, egui::PointerButton::Middle, c, c + vec2(120.0, 60.0));
    let after = h.state().ui.viewer.pan;
    assert!((after[0] - before[0] - 120.0).abs() < 1.0 && (after[1] - before[1] - 60.0).abs() < 1.0, "{before:?} → {after:?}");
    assert_eq!(h.state().ui.tool, Tool::Selection, "the tool is unchanged");
    // The Hand tool's primary drag keeps its pan too.
    h.state_mut().ui.tool = Tool::Hand;
    h.run_steps(2);
    let c = rect(&h, "viewer.comp").center();
    drag_with(&mut h, egui::PointerButton::Primary, c, c - vec2(50.0, 30.0));
    let end = h.state().ui.viewer.pan;
    assert!((end[0] - after[0] + 50.0).abs() < 1.0 && (end[1] - after[1] + 30.0).abs() < 1.0, "{after:?} → {end:?}");
}

/// A Spacebar press or release (`repeat`: the keyboard's auto-repeat while it is held).
fn space(h: &mut Harness<'_, EffectcraftApp>, pressed: bool, repeat: bool) {
    h.input_mut().events.push(Event::Key { key: egui::Key::Space, physical_key: None, pressed, repeat, modifiers: Default::default() });
    h.step();
}

/// #227: a Spacebar tap starts and stops the preview when it is released; held, Spacebar is the
/// Hand tool, so a drag pans the viewer and neither starts nor stops the preview. Auto-repeat
/// while it is held toggles nothing more, and a space typed in a text field doesn't preview.
#[test]
fn spacebar_taps_preview_and_held_spacebar_pans_the_viewer() {
    let mut h = harness();
    let playing = |h: &Harness<'_, EffectcraftApp>| h.state().playback.playing;
    let tap = |h: &mut Harness<'_, EffectcraftApp>| {
        space(h, true, false);
        space(h, false, false);
    };
    space(&mut h, true, false);
    assert!(!playing(&h), "the press alone doesn't play");
    space(&mut h, false, false);
    assert!(playing(&h), "the release does");
    tap(&mut h);
    assert!(!playing(&h), "another tap stops");
    // Held with auto-repeat: one toggle, on the release.
    space(&mut h, true, false);
    for _ in 0..5 {
        space(&mut h, true, true);
    }
    assert!(!playing(&h));
    space(&mut h, false, false);
    assert!(playing(&h));
    tap(&mut h);

    // Held and dragged: pans the viewer (the Box under the pointer stays), and plays nothing.
    let box_pos = |h: &Harness<'_, EffectcraftApp>| h.state().session.active_comp().unwrap().layers[0].props.prop("transform/position").unwrap().value.clone();
    let (pos, before) = (box_pos(&h), h.state().ui.viewer.pan);
    let c = rect(&h, "viewer.comp").center();
    space(&mut h, true, false);
    drag_with(&mut h, egui::PointerButton::Primary, c, c + vec2(80.0, 40.0));
    space(&mut h, false, false);
    let after = h.state().ui.viewer.pan;
    assert!((after[0] - before[0] - 80.0).abs() < 1.0 && (after[1] - before[1] - 40.0).abs() < 1.0, "{before:?} → {after:?}");
    assert_eq!(box_pos(&h), pos);
    assert!(!playing(&h), "a Spacebar drag doesn't start the preview");
    // Nor stop one.
    tap(&mut h);
    let c = rect(&h, "viewer.comp").center();
    space(&mut h, true, false);
    drag_with(&mut h, egui::PointerButton::Primary, c, c - vec2(30.0, 0.0));
    space(&mut h, false, false);
    assert!(playing(&h), "a Spacebar drag doesn't stop the preview");
    tap(&mut h);
    assert!(!playing(&h));

    // In a text field Spacebar types.
    let search = rect(&h, "timeline.search").center();
    click(&mut h, search);
    tap(&mut h);
    assert!(!playing(&h), "a space typed in the Timeline search doesn't preview");
}

#[test]
fn timeline_rows_drag_to_reorder_layers() {
    let mut h = harness();
    let names = |h: &Harness<'_, EffectcraftApp>| h.state().session.active_comp().unwrap().layers.iter().map(|l| l.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&h), ["Box", "Plate"]);
    let row = |h: &Harness<'_, EffectcraftApp>, name: &str| {
        let l = h.state().session.active_comp().unwrap().layers.iter().find(|l| l.name == name).unwrap().id.0;
        rect(h, &format!("timeline.layer.{l}.row"))
    };
    // Drag "Box" (top) below "Plate": it becomes the bottom layer, in one undo step.
    let (from, plate) = (row(&h, "Box").center(), row(&h, "Plate"));
    let steps = h.state().session.history.undo.len();
    drag(&mut h, from, plate.center() + vec2(0.0, plate.height() * 0.4));
    assert_eq!(names(&h), ["Plate", "Box"]);
    assert_eq!(h.state().session.history.undo.len(), steps + 1);
    // Dropping a layer where it already is changes nothing.
    let b = row(&h, "Box").center();
    drag(&mut h, b, b + vec2(0.0, 3.0));
    assert_eq!(names(&h), ["Plate", "Box"]);
    assert_eq!(h.state().session.history.undo.len(), steps + 1);
    // And back to the top.
    let (from, top) = (row(&h, "Box").center(), row(&h, "Plate"));
    drag(&mut h, from, top.center() - vec2(0.0, top.height() * 0.4));
    assert_eq!(names(&h), ["Box", "Plate"]);
}

fn layer_id(h: &Harness<'_, EffectcraftApp>, name: &str) -> u64 {
    h.state().session.active_comp().unwrap().layers.iter().find(|l| l.name == name).unwrap().id.0
}

/// Uid of a layer's property by match path (`transform/position`).
fn prop_uid(h: &Harness<'_, EffectcraftApp>, layer: &str, path: &str) -> u64 {
    let id = layer_id(h, layer);
    h.state().session.active_comp().unwrap().layer(LayerId(id)).unwrap().props.prop(path).unwrap().uid
}

/// Is the property's row on screen in the Timeline?
fn shown(h: &Harness<'_, EffectcraftApp>, uid: u64) -> bool {
    h.state().auto.find(&format!("timeline.prop.{uid}.stopwatch")).is_some()
}

fn key(h: &mut Harness<'_, EffectcraftApp>, key: egui::Key, modifiers: egui::Modifiers) {
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
    h.run_steps(2);
}

#[test]
fn timeline_twirl_arrows_open_layers_and_groups() {
    let mut h = harness();
    let id = layer_id(&h, "Box");
    let transform = {
        let comp = h.state().session.active_comp().unwrap().clone();
        comp.layer(LayerId(id)).unwrap().props.sub("transform").unwrap().uid
    };
    let at = rect(&h, &format!("timeline.layer.{id}.twirl")).center();
    click(&mut h, at);
    assert!(h.state().ui.timeline.open_layers.contains(&id), "the layer twirls open");
    let at = rect(&h, &format!("timeline.group.{transform}.twirl")).center();
    click(&mut h, at);
    for p in ["anchor", "position", "scale", "rotation", "opacity"] {
        assert!(shown(&h, prop_uid(&h, "Box", &format!("transform/{p}"))), "{p} row");
    }
    // And closed again.
    let at = rect(&h, &format!("timeline.layer.{id}.twirl")).center();
    click(&mut h, at);
    h.run_steps(2);
    assert!(!h.state().ui.timeline.open_layers.contains(&id));
    assert!(!shown(&h, prop_uid(&h, "Box", "transform/position")));
}

#[test]
fn timeline_rename_commits_on_enter_and_on_click_away() {
    // The rename field (double-click a name, or Enter on a selected layer) used to grab the
    // keyboard every frame: clicking away or Enter could never leave it. (The harness can't
    // double-click — its frames are longer than a double-click — so this opens it with Enter.)
    let mut h = harness();
    h.state_mut().session.execute("layer.newNull", json!({"name": "Ctrl"})).unwrap();
    let id = layer_id(&h, "Ctrl");
    let name = |h: &Harness<'_, EffectcraftApp>| h.state().session.active_comp().unwrap().layer(LayerId(id)).unwrap().name.clone();
    let rename = |h: &mut Harness<'_, EffectcraftApp>| {
        h.state_mut().session.execute("layer.select", json!({"layers": [id]})).unwrap();
        h.run_steps(2);
        key(h, egui::Key::Enter, Default::default());
        h.run_steps(1);
        assert!(h.ctx.memory(|m| m.focused().is_some()), "the name field has focus");
    };
    // The whole name is selected: typing replaces it; Enter commits and frees the keyboard.
    rename(&mut h);
    h.input_mut().events.push(Event::Text("Rig".into()));
    h.step();
    key(&mut h, egui::Key::Enter, Default::default());
    assert_eq!(name(&h), "Rig");
    assert!(h.ctx.memory(|m| m.focused().is_none()), "the field let go of the keyboard");
    key(&mut h, egui::Key::P, Default::default());
    assert_eq!(h.state().ui.timeline.reveal, vec!["position"], "P works after renaming");
    // Clicking away commits too.
    rename(&mut h);
    h.input_mut().events.push(Event::Text("Hand".into()));
    h.step();
    let away = rect(&h, "viewer.comp").center();
    click(&mut h, away);
    assert_eq!(name(&h), "Hand");
    assert!(h.ctx.memory(|m| m.focused().is_none()));
    // Escape cancels.
    rename(&mut h);
    h.input_mut().events.push(Event::Text("Nope".into()));
    h.step();
    key(&mut h, egui::Key::Escape, Default::default());
    assert_eq!(name(&h), "Hand");
}

/// Press `k` twice within one frame (a quick double press: the harness' frames are long).
fn key_twice(h: &mut Harness<'_, EffectcraftApp>, k: egui::Key) {
    for _ in 0..2 {
        h.input_mut().events.push(Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
        h.input_mut().events.push(Event::Key { key: k, physical_key: None, pressed: false, repeat: false, modifiers: Default::default() });
    }
    h.run_steps(2);
}

#[test]
fn property_shortcuts_reveal_and_key_transform_properties() {
    let mut h = harness();
    h.state_mut().session.execute("layer.select", json!({"layers": ["Box"]})).unwrap();
    h.run_steps(2);
    let t = |h: &Harness<'_, EffectcraftApp>, p: &str| shown(h, prop_uid(h, "Box", &format!("transform/{p}")));
    let only = |h: &Harness<'_, EffectcraftApp>, want: &[&str]| {
        for p in ["anchor", "position", "scale", "rotation", "opacity"] {
            assert_eq!(t(h, p), want.contains(&p), "{p} shown? want {want:?}");
        }
    };
    let none = egui::Modifiers::default();
    // P, then T: one property at a time; Shift+S adds Scale; P again hides.
    key(&mut h, egui::Key::P, none);
    only(&h, &["position"]);
    key(&mut h, egui::Key::T, none);
    only(&h, &["opacity"]);
    key(&mut h, egui::Key::S, egui::Modifiers::SHIFT);
    only(&h, &["scale", "opacity"]);
    key(&mut h, egui::Key::A, none);
    only(&h, &["anchor"]);
    key(&mut h, egui::Key::R, none);
    only(&h, &["rotation"]);
    // Alt+Shift+P: a Position keyframe at the current time; again removes it.
    let pos_keys = |h: &Harness<'_, EffectcraftApp>| {
        let id = layer_id(h, "Box");
        h.state().session.active_comp().unwrap().layer(LayerId(id)).unwrap().props.prop("transform/position").unwrap().keys.len()
    };
    let alt_shift = egui::Modifiers { alt: true, shift: true, ..Default::default() };
    key(&mut h, egui::Key::P, alt_shift);
    assert_eq!(pos_keys(&h), 1);
    key(&mut h, egui::Key::P, alt_shift);
    assert_eq!(pos_keys(&h), 0);
    // U: only keyframed properties (Opacity animated); U again later hides; UU modified ones.
    h.state_mut().session.execute("prop.addKey", json!({"layer": "Box", "path": "transform/opacity", "time": 0, "value": 50})).unwrap();
    h.state_mut().session.execute("prop.addKey", json!({"layer": "Box", "path": "transform/opacity", "time": 1, "value": 100})).unwrap();
    h.run_steps(2);
    key(&mut h, egui::Key::U, none);
    only(&h, &["opacity"]);
    h.run_steps(40);
    key(&mut h, egui::Key::U, none);
    only(&h, &[]);
    h.state_mut().session.execute("prop.set", json!({"layer": "Box", "path": "transform/rotation", "value": 30})).unwrap();
    h.run_steps(40);
    key_twice(&mut h, egui::Key::U);
    assert!(t(&h, "opacity") && t(&h, "rotation"), "UU: animated and changed properties");
    assert!(!t(&h, "scale"), "an untouched property stays hidden");
    // Ctrl+`: twirl the selected layer open, then closed.
    h.run_steps(40);
    key(&mut h, egui::Key::Backtick, egui::Modifiers::COMMAND);
    let id = layer_id(&h, "Box");
    assert!(h.state().ui.timeline.open_layers.contains(&id) && h.state().ui.timeline.reveal.is_empty());
    key(&mut h, egui::Key::Backtick, egui::Modifiers::COMMAND);
    assert!(!h.state().ui.timeline.open_layers.contains(&id));
}

#[test]
fn double_press_shortcuts_reveal_their_second_set() {
    let mut h = harness();
    h.state_mut().session.execute("layer.select", json!({"layers": ["Box"]})).unwrap();
    h.state_mut().session.execute("prop.setExpression", json!({"layer": "Box", "path": "transform/scale", "expression": "value"})).unwrap();
    h.run_steps(2);
    let reveal = |h: &Harness<'_, EffectcraftApp>| h.state().ui.timeline.reveal.clone();
    // EE: properties with expressions (Scale), replacing E's effects.
    key_twice(&mut h, egui::Key::E);
    assert_eq!(reveal(&h), vec!["expressions"]);
    assert!(shown(&h, prop_uid(&h, "Box", "transform/scale")));
    h.run_steps(40);
    // TT: Mask Opacity; MM: every mask property; a single M: Mask Path.
    key_twice(&mut h, egui::Key::T);
    assert_eq!(reveal(&h), vec!["maskOpacity"]);
    h.run_steps(40);
    key(&mut h, egui::Key::M, Default::default());
    assert_eq!(reveal(&h), vec!["maskPath"]);
    h.run_steps(40);
    key_twice(&mut h, egui::Key::M);
    assert_eq!(reveal(&h), vec!["masks"]);
    h.run_steps(40);
    // RR Time Remap, AA Material Options, PP paint/puppet, SS selected properties, FF missing effects.
    for (k, want) in
        [(egui::Key::R, "timeRemap"), (egui::Key::A, "material"), (egui::Key::P, "paint"), (egui::Key::S, "selected"), (egui::Key::F, "missingEffects")]
    {
        key_twice(&mut h, k);
        assert_eq!(reveal(&h), vec![want], "{k:?}{k:?}");
        h.run_steps(40);
    }
}

/// Drag through `path` with `modifiers` held (press at the first point, release at the last).
fn drag_path(h: &mut Harness<'_, EffectcraftApp>, path: &[Pos2], modifiers: egui::Modifiers) {
    h.input_mut().events.push(Event::ModifiersChanged(modifiers));
    h.input_mut().events.push(Event::PointerMoved(path[0]));
    h.input_mut().events.push(Event::PointerButton { pos: path[0], button: egui::PointerButton::Primary, pressed: true, modifiers });
    h.step();
    for w in path.windows(2) {
        for i in 1..=6 {
            h.input_mut().events.push(Event::PointerMoved(w[0] + (w[1] - w[0]) * (i as f32 / 6.0)));
            h.step();
        }
    }
    let end = *path.last().unwrap();
    h.input_mut().events.push(Event::PointerButton { pos: end, button: egui::PointerButton::Primary, pressed: false, modifiers });
    h.run_steps(2);
    // The keys come up after the button, as a hand does.
    h.input_mut().events.push(Event::ModifiersChanged(Default::default()));
    h.step();
}

/// Issue #63: a bounding-box handle scales about the anchor point so the grabbed corner follows
/// the pointer, relative to the scale the drag began with: dragging through the anchor and back
/// out recovers (it used to stick at 0), edges scale one axis, Shift keeps the proportions.
#[test]
fn handle_drags_scale_about_the_anchor_and_follow_the_pointer() {
    let mut h = harness();
    let box_id = h.state().session.active_comp().unwrap().layers[0].id;
    let set = |h: &mut Harness<'_, EffectcraftApp>, path: &str, v: serde_json::Value| {
        h.state_mut().session.execute("prop.set", json!({"layer": box_id.0, "path": path, "value": v})).unwrap();
    };
    let scale = |h: &Harness<'_, EffectcraftApp>| {
        let l = h.state().session.active_comp().unwrap().layer(box_id).unwrap().clone();
        l.props.prop("transform/scale").unwrap().value.as_vec3()
    };
    let close = |a: [f64; 3], b: [f64; 2]| (a[0] - b[0]).abs() < 1.0 && (a[1] - b[1]).abs() < 1.0;
    h.state_mut().session.execute("layer.select", json!({"layers": [box_id.0]})).unwrap();
    // The 80×80 box is centred at (320, 180) with its anchor in the middle.
    h.state_mut().session.execute("view.snapping", json!({"value": false})).unwrap();
    h.run_steps(2);
    let undo0 = h.state().session.history.undo.len();
    // Bottom-right corner (360, 220) to (400, 260): twice as far from the anchor.
    let corner = rect(&h, &format!("viewer.handle.{}.2", box_id.0)).center();
    let path = [corner, screen(&h, [400.0, 260.0])];
    drag_path(&mut h, &path, Default::default());
    assert!(close(scale(&h), [200.0, 200.0]), "{:?}", scale(&h));
    assert_eq!(h.state().session.history.undo.len(), undo0 + 1, "one undo step per drag");
    // Through the anchor (scale ≈ 0) and back out to 1.5×: the layer follows, nothing sticks.
    set(&mut h, "transform/scale", json!([100, 100, 100]));
    h.run_steps(2);
    let corner = rect(&h, &format!("viewer.handle.{}.2", box_id.0)).center();
    let path = [corner, screen(&h, [320.0, 180.0]), screen(&h, [300.0, 170.0]), screen(&h, [380.0, 240.0])];
    drag_path(&mut h, &path, Default::default());
    assert!(close(scale(&h), [150.0, 150.0]), "{:?}", scale(&h));
    // Past the anchor the layer flips, as in After Effects.
    set(&mut h, "transform/scale", json!([100, 100, 100]));
    h.run_steps(2);
    let corner = rect(&h, &format!("viewer.handle.{}.2", box_id.0)).center();
    let path = [corner, screen(&h, [280.0, 140.0])];
    drag_path(&mut h, &path, Default::default());
    assert!(close(scale(&h), [-100.0, -100.0]), "{:?}", scale(&h));
    // The right edge (handle 5) scales x only.
    set(&mut h, "transform/scale", json!([100, 100, 100]));
    h.run_steps(2);
    let edge = rect(&h, &format!("viewer.handle.{}.5", box_id.0)).center();
    let path = [edge, screen(&h, [380.0, 200.0])];
    drag_path(&mut h, &path, Default::default());
    assert!(close(scale(&h), [150.0, 100.0]), "{:?}", scale(&h));
    // Shift on a corner keeps the proportions: the pointer's place along the diagonal.
    set(&mut h, "transform/scale", json!([100, 100, 100]));
    h.run_steps(2);
    let corner = rect(&h, &format!("viewer.handle.{}.2", box_id.0)).center();
    let path = [corner, screen(&h, [400.0, 230.0])];
    drag_path(&mut h, &path, egui::Modifiers::SHIFT);
    assert!(close(scale(&h), [162.5, 162.5]), "{:?}", scale(&h));
}

/// #252: with snapping on, a dragged handle snaps to the comp's corners and edges (and other
/// layers'), so a layer scales exactly to the comp; Shift keeps the proportions and still lands
/// on the comp's size.
#[test]
fn handle_drags_snap_to_the_comp_edges() {
    let mut h = harness();
    let box_id = h.state().session.active_comp().unwrap().layers[0].id.0;
    let scale = |h: &Harness<'_, EffectcraftApp>, id: u64| {
        let l = h.state().session.active_comp().unwrap().layer(LayerId(id)).unwrap().clone();
        l.props.prop("transform/scale").unwrap().value.as_vec3()
    };
    let close = |a: [f64; 3], b: [f64; 2]| (a[0] - b[0]).abs() < 0.01 && (a[1] - b[1]).abs() < 0.01;
    let drag_handle = |h: &mut Harness<'_, EffectcraftApp>, id: u64, handle: usize, to: [f32; 2], mods: egui::Modifiers| {
        h.state_mut().session.execute("prop.set", json!({"layer": id, "path": "transform/scale", "value": [100, 100, 100]})).unwrap();
        h.state_mut().session.execute("layer.select", json!({"layers": [id]})).unwrap();
        h.run_steps(2);
        let from = rect(h, &format!("viewer.handle.{id}.{handle}")).center();
        let to = screen(h, to);
        drag_path(h, &[from, to], mods);
        scale(h, id)
    };
    // The 80×80 box at the comp centre: its bottom right corner (360, 220) dropped 3 px inside
    // the comp's (640, 360) lands on it, its right edge 3 px inside the comp's right edge too.
    let s = drag_handle(&mut h, box_id, 2, [637.0, 357.0], Default::default());
    assert!(close(s, [800.0, 450.0]), "{s:?}");
    let s = drag_handle(&mut h, box_id, 5, [637.0, 200.0], Default::default());
    assert!(close(s, [800.0, 100.0]), "{s:?}");
    // Snapping off: where the pointer is.
    h.state_mut().session.execute("view.snapping", json!({"value": false})).unwrap();
    let s = drag_handle(&mut h, box_id, 2, [637.0, 357.0], Default::default());
    assert!(s[0] < 795.0, "{s:?}");
    h.state_mut().session.execute("view.snapping", json!({"value": true})).unwrap();
    // A comp-shaped layer at 110 % Shift-scaled down by its corner to 3 px outside the comp's:
    // the comp's size exactly.
    let wide = h.state_mut().session.execute("layer.newSolid", json!({"name": "Wide", "color": "#20e040", "width": 640, "height": 360})).unwrap()["layer"]
        .as_u64()
        .unwrap();
    h.state_mut().session.execute("prop.set", json!({"layer": wide, "path": "transform/scale", "value": [110, 110, 100]})).unwrap();
    h.state_mut().session.execute("layer.select", json!({"layers": [wide]})).unwrap();
    h.run_steps(2);
    let path = [rect(&h, &format!("viewer.handle.{wide}.2")).center(), screen(&h, [643.0, 362.0])];
    drag_path(&mut h, &path, egui::Modifiers::SHIFT);
    let s = scale(&h, wide);
    assert!(close(s, [100.0, 100.0]), "{s:?}");
}

/// Each bounding-box handle shows the resize cursor along the direction it scales, turning with
/// the layer (#393: every handle showed the north-west / south-east cursor).
#[test]
fn handle_cursors_follow_each_handle_and_the_layer_rotation() {
    use egui::CursorIcon::{ResizeHorizontal as H, ResizeNeSw as NeSw, ResizeNwSe as NwSe, ResizeVertical as V};
    let mut h = harness();
    let box_id = h.state().session.active_comp().unwrap().layers[0].id.0;
    let set = |h: &mut Harness<'_, EffectcraftApp>, path: &str, v: serde_json::Value| {
        h.state_mut().session.execute("prop.set", json!({"layer": box_id, "path": path, "value": v})).unwrap();
        h.run_steps(2);
    };
    let cursors = |h: &mut Harness<'_, EffectcraftApp>| {
        (0..8)
            .map(|i| {
                let c = rect(h, &format!("viewer.handle.{box_id}.{i}")).center();
                h.input_mut().events.push(Event::PointerMoved(c));
                h.run_steps(2);
                h.output().platform_output.cursor_icon
            })
            .collect::<Vec<_>>()
    };
    h.state_mut().session.execute("layer.select", json!({"layers": [box_id]})).unwrap();
    // A wide box: its corners still take the diagonal cursors.
    set(&mut h, "transform/scale", json!([300, 100, 100]));
    // Corners clockwise from the top left, then the top, right, bottom and left edges.
    assert_eq!(cursors(&mut h), [NwSe, NeSw, NwSe, NeSw, V, H, V, H]);
    set(&mut h, "transform/rotation", json!(90));
    assert_eq!(cursors(&mut h), [NeSw, NwSe, NeSw, NwSe, H, V, H, V], "turned a quarter");
    set(&mut h, "transform/rotation", json!(45));
    assert_eq!(cursors(&mut h), [V, H, V, H, NeSw, NwSe, NeSw, NwSe], "turned an eighth");
}

/// The arrow keys over the Composition panel nudge the selected layer 1 pixel at the viewer's
/// magnification (half a comp pixel at 200 %), Shift+arrow 10, one undo step each (#290).
#[test]
fn arrow_keys_nudge_the_selected_layer_at_the_viewer_magnification() {
    let mut h = harness();
    let box_id = h.state().session.active_comp().unwrap().layers[0].id;
    h.state_mut().session.execute("prop.set", json!({"layer": box_id.0, "path": "transform/position", "value": [100, 100, 0]})).unwrap();
    h.state_mut().session.execute("view.snapping", json!({"value": false})).unwrap();
    h.run_steps(2);
    let at = screen(&h, [100.0, 100.0]);
    click(&mut h, at);
    assert_eq!(h.state().session.state.selected_layers, vec![box_id]);
    let pos = |h: &Harness<'_, EffectcraftApp>| {
        h.state().session.active_comp().unwrap().layer(box_id).unwrap().props.prop("transform/position").unwrap().value.as_vec3()
    };
    let ppp = h.ctx.pixels_per_point();
    h.state_mut().ui.viewer.zoom = Some(1.0 / ppp);
    h.run_steps(2);
    let undo = h.state().session.history.undo.len();
    key(&mut h, egui::Key::ArrowRight, egui::Modifiers::NONE);
    key(&mut h, egui::Key::ArrowDown, egui::Modifiers::SHIFT);
    assert_eq!(pos(&h), [101.0, 110.0, 0.0]);
    assert_eq!(h.state().session.history.undo.len(), undo + 2, "one undo step per press");
    h.state_mut().ui.viewer.zoom = Some(2.0 / ppp);
    h.run_steps(2);
    key(&mut h, egui::Key::ArrowLeft, egui::Modifiers::NONE);
    key(&mut h, egui::Key::ArrowUp, egui::Modifiers::NONE);
    assert_eq!(pos(&h), [100.5, 109.5, 0.0], "sub-pixel steps when zoomed in");
}

/// A comp wider than the GPU's texture limit at Full resolution shows its frame (averaged down
/// into a texture the renderer accepts) instead of failing the upload (#201: 11000×2200).
#[test]
fn full_resolution_frames_wider_than_the_texture_limit_fit() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Wide", "width": 2400, "height": 400, "duration": 1})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    // (egui's font atlas needs 1024.)
    h.input_mut().max_texture_side = Some(1024);
    h.state_mut().ui.viewer.res = effectcraft_ui_egui::state::Resolution::Full;
    let full = |h: &mut Harness<'_, EffectcraftApp>| h.state_mut().viewer_pixels().is_some_and(|px| px.size == [2400, 400]);
    for _ in 0..400 {
        h.step();
        if full(&mut h) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(full(&mut h), "the full-size frame is shown (and stays readable): {:?}", h.state().ui.viewer.res);
    assert_eq!(h.state().viewer_texture_size(), Some([800, 134]), "2400×400 averaged by 3");
}

/// #509: ⌘/Ctrl-drag in the Layer panel resizes the Roto Brush (as in After Effects) without
/// painting, the next stroke uses the new size, and the tool options show the Diameter.
#[test]
fn roto_brush_ctrl_drag_resizes_the_brush() {
    let mut h = harness();
    let id = h.state().session.active_comp().unwrap().layers.iter().find(|l| l.name == "Box").unwrap().id.0;
    h.state_mut().session.execute("layer.select", json!({"layers": [id]})).unwrap();
    h.state_mut().ui.layer_panel = Some(id);
    h.state_mut().show_panel(effectcraft_ui_egui::dock::PanelKind::Layer);
    h.state_mut().ui.tool = Tool::RotoBrush;
    h.run_steps(3);
    assert!(h.state().auto.find("header.roto.diameter").is_some(), "the Diameter field");
    let canvas = rect(&h, "layerPanel.canvas");
    let zoom = canvas.width() / 80.0;
    let c = canvas.center();
    h.input_mut().events.push(Event::ModifiersChanged(egui::Modifiers::COMMAND));
    drag(&mut h, c, c + vec2(20.0 * zoom, 0.0));
    h.input_mut().events.push(Event::ModifiersChanged(egui::Modifiers::NONE));
    h.run_steps(2);
    let dia = h.state().session.state.roto.diameter;
    assert!((dia - 40.0).abs() <= 1.0, "a 20 px drag makes a 40 px brush: {dia}");
    let strokes = |h: &Harness<'_, EffectcraftApp>| {
        let l = h.state().session.active_comp().unwrap().layer(LayerId(id)).unwrap().clone();
        let roto = effectcraft_engine::effects::roto::ID;
        let g = l
            .effects()
            .and_then(|fx| fx.groups().find(|g| matches!(&g.kind, effectcraft_engine::project::GroupKind::Effect { effect } if effect == roto)).cloned());
        g.map(|g| effectcraft_engine::effects::roto::data(&effectcraft_engine::effects::flatten_params(&g, &mut |p| p.value.clone())).strokes.clone())
            .unwrap_or_default()
    };
    assert!(strokes(&h).is_empty(), "resizing paints nothing");
    drag(&mut h, c - vec2(10.0 * zoom, 0.0), c + vec2(10.0 * zoom, 0.0));
    let s = strokes(&h);
    assert_eq!(s.len(), 1);
    assert!((s[0].radius - dia / 2.0).abs() < 1e-6, "{} vs {dia}", s[0].radius);
}
