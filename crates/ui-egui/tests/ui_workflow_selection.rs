//! User-reported selection, Project deletion, and render-queue drop regressions.
use effectcraft_engine::{Session, project::LayerId};
use effectcraft_ui_egui::{EffectcraftApp, dock::PanelKind};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use serde_json::json;

fn harness() -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name":"Study", "width":640,"height":360,"duration":4})).unwrap();
    for n in 0..4 {
        s.execute("layer.newSolid", json!({"name":format!("Tile {n}"),"width":80,"height":80,"color":"#406080"})).unwrap();
    }
    s.execute("edit.deselectAll", json!({})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    h
}
fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("missing {id}"));
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}
fn pointer(h: &mut Harness<'_, EffectcraftApp>, p: Pos2, down: bool, m: Modifiers) {
    h.input_mut().events.push(Event::ModifiersChanged(m));
    h.input_mut().events.push(Event::PointerMoved(p));
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: down, modifiers: m });
    h.step();
}
fn click(h: &mut Harness<'_, EffectcraftApp>, p: Pos2, m: Modifiers) {
    pointer(h, p, true, m);
    pointer(h, p, false, m);
    h.run_steps(2);
}
fn drag(h: &mut Harness<'_, EffectcraftApp>, a: Pos2, b: Pos2, m: Modifiers) {
    pointer(h, a, true, m);
    for n in 1..=8 {
        h.input_mut().events.push(Event::PointerMoved(a + (b - a) * (n as f32 / 8.0)));
        h.step();
    }
    pointer(h, b, false, m);
    h.run_steps(2);
}
#[test]
fn timeline_shift_click_selects_range_and_ctrl_toggles() {
    let mut h = harness();
    let ids: Vec<LayerId> = h.state().session.active_comp().unwrap().layers.iter().map(|l| l.id).collect();
    let point = |h: &Harness<'_, EffectcraftApp>, id: LayerId| {
        let r = rect(h, &format!("timeline.layer.{}.row", id.0));
        pos2(r.min.x + 210.0, r.center().y)
    };
    let p = point(&h, ids[0]);
    click(&mut h, p, Modifiers::NONE);
    let p = point(&h, ids[2]);
    click(&mut h, p, Modifiers::SHIFT);
    assert_eq!(h.state().session.state.selected_layers, ids[..3]);
    let p = point(&h, ids[1]);
    click(&mut h, p, Modifiers::COMMAND);
    assert_eq!(h.state().session.state.selected_layers, vec![ids[0], ids[2]]);
}
/// #284: Shift-clicking layer bars in the time graph selects the range from the last layer
/// clicked, as Shift-clicking their names does.
#[test]
fn timeline_shift_click_on_layer_bars_selects_the_range() {
    let mut h = harness();
    let ids: Vec<LayerId> = h.state().session.active_comp().unwrap().layers.iter().map(|l| l.id).collect();
    let bar = |h: &Harness<'_, EffectcraftApp>, id: LayerId| rect(h, &format!("timeline.layer.{}.bar", id.0)).center();
    let p = bar(&h, ids[3]);
    click(&mut h, p, Modifiers::NONE);
    assert_eq!(h.state().session.state.selected_layers, vec![ids[3]]);
    let p = bar(&h, ids[1]);
    click(&mut h, p, Modifiers::SHIFT);
    assert_eq!(h.state().session.state.selected_layers, ids[1..]);
    // From a name to a bar too.
    let r = rect(&h, &format!("timeline.layer.{}.row", ids[0].0));
    click(&mut h, pos2(r.min.x + 210.0, r.center().y), Modifiers::NONE);
    let p = bar(&h, ids[2]);
    click(&mut h, p, Modifiers::SHIFT);
    assert_eq!(h.state().session.state.selected_layers, ids[..3]);
}
#[test]
fn timeline_empty_outline_marquee_selects_layers() {
    let mut h = harness();
    let ids: Vec<LayerId> = h.state().session.active_comp().unwrap().layers.iter().map(|l| l.id).collect();
    let area = rect(&h, "timeline.layerMarquee");
    let top = rect(&h, &format!("timeline.layer.{}.row", ids[0].0));
    let last = rect(&h, &format!("timeline.layer.{}.row", ids[3].0));
    drag(&mut h, pos2(area.min.x + 210.0, last.max.y + 25.0), pos2(area.min.x + 215.0, top.min.y + 2.0), Modifiers::NONE);
    assert_eq!(h.state().session.state.selected_layers, ids);
}
#[test]
fn project_clear_deletes_items_and_undo_restores_them() {
    let mut h = harness();
    let id = h.state().session.active_comp_id().unwrap();
    h.state_mut().session.state.project_selection = vec![id];
    h.state_mut().ui.focused = PanelKind::Project;
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "edit.clear", json!({})).unwrap();
    assert!(h.state().session.project.comp(id).is_none());
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert!(h.state().session.project.comp(id).is_some());
}
#[test]
fn project_comp_drop_queues_the_dragged_comp() {
    let mut h = harness();
    let id = h.state().session.active_comp_id().unwrap();
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "window.panel", json!({"panel":"renderQueue"})).unwrap();
    h.run_steps(3);
    let a = rect(&h, &format!("project.item.{}.name", id.0)).center();
    let b = rect(&h, "renderQueue.drop").center();
    drag(&mut h, a, b, Modifiers::NONE);
    assert_eq!(h.state().session.project.render_queue.len(), 1);
    assert_eq!(h.state().session.project.render_queue[0].comp, id);
}

#[test]
fn viewer_marquee_crosses_a_layer_interior_and_shift_preserves_selection() {
    let mut h = harness();
    let ids: Vec<LayerId> = h.state().session.active_comp().unwrap().layers.iter().map(|l| l.id).collect();
    for (n, id) in ids.iter().enumerate() {
        h.state_mut().session.execute("prop.set", json!({"layer":id.0,"path":"transform/position","value":[120.0+n as f64*120.0,180.0]})).unwrap();
    }
    h.run_steps(3);
    let comp = rect(&h, "viewer.comp");
    let screen = |x: f32, y: f32| comp.min + vec2(x, y) * (comp.width() / 640.0);
    drag(&mut h, screen(60.0, 170.0), screen(180.0, 190.0), Modifiers::NONE);
    assert_eq!(h.state().session.state.selected_layers, vec![ids[0]]);
    drag(&mut h, screen(200.0, 100.0), screen(300.0, 230.0), Modifiers::SHIFT);
    assert!(h.state().session.state.selected_layers.contains(&ids[0]));
    assert!(h.state().session.state.selected_layers.contains(&ids[1]));
    drag(&mut h, screen(200.0, 100.0), screen(300.0, 230.0), Modifiers::SHIFT);
    assert_eq!(h.state().session.state.selected_layers, vec![ids[0]]);
}

#[test]
fn render_queue_delete_preserves_selected_timeline_layers() {
    let mut h = harness();
    let id = h.state().session.active_comp().unwrap().layers[0].id;
    h.state_mut().session.execute("layer.select", json!({"layers":[id.0]})).unwrap();
    h.state_mut().session.execute("renderQueue.add", json!({})).unwrap();
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "window.panel", json!({"panel":"renderQueue"})).unwrap();
    h.run_steps(3);
    let row = rect(&h, "renderQueue.item.1.row");
    click(&mut h, pos2(row.max.x - 15.0, row.center().y), Modifiers::NONE);
    assert_eq!(h.state().ui.focused, PanelKind::RenderQueue);
    h.input_mut().events.push(Event::Key { key: egui::Key::Delete, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert!(h.state().session.project.render_queue.is_empty());
    assert!(h.state().session.active_comp().unwrap().layer(id).is_some());
    assert_eq!(h.state().session.state.selected_layers, vec![id]);
}

#[test]
fn project_shift_selects_visible_range_and_ctrl_removes_item() {
    let mut h = harness();
    let mut ids = vec![];
    for name in ["A Range", "B Range", "C Range"] {
        ids.push(h.state_mut().session.execute("comp.new", json!({"name":name,"width":64,"height":64,"duration":1})).unwrap()["comp"].as_u64().unwrap());
    }
    h.run_steps(3);
    let p = rect(&h, &format!("project.item.{}.name", ids[0])).center();
    click(&mut h, p, Modifiers::NONE);
    let p = rect(&h, &format!("project.item.{}.name", ids[2])).center();
    click(&mut h, p, Modifiers::SHIFT);
    assert_eq!(h.state().session.state.project_selection.iter().map(|i| i.0).collect::<Vec<_>>(), ids);
    let p = rect(&h, &format!("project.item.{}.name", ids[1])).center();
    click(&mut h, p, Modifiers::COMMAND);
    assert_eq!(h.state().session.state.project_selection.iter().map(|i| i.0).collect::<Vec<_>>(), vec![ids[0], ids[2]]);
}

#[test]
fn effect_scrubbing_uses_adaptive_resolution_and_settles_on_release() {
    let mut h = harness();
    let id = h.state().session.active_comp().unwrap().layers[0].id;
    h.state_mut().session.execute("layer.select", json!({"layers":[id.0]})).unwrap();
    h.state_mut().session.execute("effect.apply", json!({"effect":"Gaussian Blur"})).unwrap();
    h.state_mut().session.execute("view.fastPreviewMode", json!({"mode":"adaptive"})).unwrap();
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "window.panel", json!({"panel":"effectControls"})).unwrap();
    h.run_steps(3);
    let prop = h.state().session.active_comp().unwrap().layer(id).unwrap().effects().unwrap().groups().next().unwrap().get("blurriness").unwrap().uid;
    let a = rect(&h, &format!("effectControls.prop.{prop}.value")).center();
    pointer(&mut h, a, true, Modifiers::NONE);
    h.input_mut().events.push(Event::PointerMoved(a + vec2(30.0, 0.0)));
    h.run_steps(2);
    assert!(h.state().ui.viewer.property_interacting);
    assert!(h.state().viewer_scale(1.0, 1.0) < 1.0);
    h.state_mut().session.execute("view.res.full", json!({})).unwrap();
    assert_eq!(h.state().viewer_scale(1.0, 1.0), 1.0);
    pointer(&mut h, a + vec2(30.0, 0.0), false, Modifiers::NONE);
    h.run_steps(2);
    assert!(!h.state().ui.viewer.property_interacting);
    h.state_mut().session.execute("view.res.auto", json!({})).unwrap();
    assert_eq!(h.state().viewer_scale(1.0, 1.0), 1.0);
}

#[test]
fn viewer_right_click_targets_hit_and_duplicate_preserves_group() {
    let mut h = harness();
    let ids: Vec<LayerId> = h.state().session.active_comp().unwrap().layers.iter().map(|l| l.id).collect();
    for (n, id) in ids.iter().enumerate() {
        h.state_mut().session.execute("prop.set", json!({"layer":id.0,"path":"transform/position","value":[120.0+n as f64*120.0,180.0]})).unwrap();
    }
    h.state_mut().session.execute("layer.select", json!({"layers":[ids[2].0]})).unwrap();
    h.run_steps(3);
    let comp = rect(&h, "viewer.comp");
    let p = comp.min + vec2(120.0, 180.0) * (comp.width() / 640.0);
    let right_click = |h: &mut Harness<'_, EffectcraftApp>| {
        for pressed in [true, false] {
            h.input_mut().events.push(Event::PointerMoved(p));
            h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
            h.step();
        }
        h.run_steps(3);
    };
    right_click(&mut h);
    assert_eq!(h.state().session.state.selected_layers, vec![ids[0]]);
    if let Ok(out) = std::env::var("CONTEXT_MENU_SNAPSHOT") {
        let comp = h.state().session.active_comp_id().unwrap();
        let key = effectcraft_ui_egui::frames::FrameKey { frame: 0, ..h.state().shown_series(comp) };
        for _ in 0..600 {
            h.step();
            if h.state().frames.is_cached(&key) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(h.state().frames.is_cached(&key));
        h.run_steps(3);
        h.render().expect("offscreen context menu render").save(out).unwrap();
    }
    h.get_by_label("Duplicate").click();
    h.run_steps(3);
    assert_eq!(h.state().session.active_comp().unwrap().layers.len(), 5);
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    h.state_mut().session.execute("layer.select", json!({"layers":[ids[0].0,ids[1].0]})).unwrap();
    h.run_steps(3);
    right_click(&mut h);
    assert_eq!(h.state().session.state.selected_layers.len(), 2);
    h.get_by_label("Duplicate").click();
    h.run_steps(3);
    assert_eq!(h.state().session.active_comp().unwrap().layers.len(), 6);
}

#[test]
fn viewer_context_select_reaches_an_obscured_layer() {
    let mut h = harness();
    let target = h.state().session.active_comp().unwrap().layers.iter().find(|l| l.name == "Tile 1").unwrap().id;
    let p = rect(&h, "viewer.comp").center();
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerMoved(p));
        h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
    h.get_by_label("Select ⏵").click();
    h.run_steps(3);
    h.get_by_label("Tile 1").click();
    h.run_steps(3);
    assert_eq!(h.state().session.state.selected_layers, vec![target]);
}

/// Original four-tile UI fixture; no Adobe assets or reference screenshots.
#[test]
fn modal_blocks_clicks_on_underlying_timeline_layers() {
    let mut h = harness();
    let ids: Vec<LayerId> = h.state().session.active_comp().unwrap().layers.iter().map(|l| l.id).collect();
    h.state_mut().session.execute("layer.select", json!({"layers":[ids[0].0]})).unwrap();
    let row = rect(&h, &format!("timeline.layer.{}.row", ids[2].0));
    h.state_mut().dialog = Some(effectcraft_ui_egui::Dialog::About);
    h.run_steps(3);
    click(&mut h, pos2(row.min.x + 210.0, row.center().y), Modifiers::NONE);
    assert_eq!(h.state().session.state.selected_layers, vec![ids[0]]);
    assert_eq!(h.state().dialog, Some(effectcraft_ui_egui::Dialog::About));
}

#[test]
fn layer_style_escape_rolls_back_live_preview() {
    let mut h = harness();
    let id = h.state().session.active_comp().unwrap().layers[0].id;
    h.state_mut().session.execute("layer.select", json!({"layers":[id.0]})).unwrap();
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "layer.style.dropShadow", json!({})).unwrap();
    h.run_steps(3);
    h.state_mut().session.execute("prop.set", json!({"layer":id.0,"path":"layerStyles/dropShadow/distance","value":25})).unwrap();
    h.input_mut().events.push(Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
    let styles = h.state_mut().session.execute("layer.style.list", json!({})).unwrap();
    assert!(styles["styles"].as_array().unwrap().is_empty());
}

#[test]
fn style_cancel_restores_checkpoint_with_bounded_history_and_reopened_scrub() {
    let mut h = harness();
    let id = h.state().session.active_comp().unwrap().layers[0].id;
    h.state_mut().session.execute("layer.select", json!({"layers":[id.0]})).unwrap();
    h.state_mut().session.execute("layer.style.dropShadow", json!({})).unwrap();
    h.state_mut().session.prefs.general.undo_levels = 1;
    let ctx = h.ctx.clone();
    let open = |h: &mut Harness<'_, EffectcraftApp>| {
        effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "layer.style.options", json!({"style":"dropShadow"})).unwrap();
    };
    open(&mut h);
    let before = h.state().session.project.clone();
    let history = h.state().session.history.undo.len();
    h.state_mut().session.execute("prop.set", json!({"layer":id.0,"path":"layerStyles/dropShadow/distance","value":25,"merge":"same-style-control"})).unwrap();
    effectcraft_ui_egui::panels::layer_styles_dialog::finish(h.state_mut(), false);
    assert!(std::sync::Arc::ptr_eq(&h.state().session.project, &before));
    assert_eq!(h.state().session.history.undo.len(), history);
    open(&mut h);
    h.state_mut().session.execute("prop.set", json!({"layer":id.0,"path":"layerStyles/dropShadow/distance","value":50,"merge":"same-style-control"})).unwrap();
    effectcraft_ui_egui::panels::layer_styles_dialog::finish(h.state_mut(), true);
    let accepted = h.state().session.project.clone();
    open(&mut h);
    h.state_mut().session.execute("prop.set", json!({"layer":id.0,"path":"layerStyles/dropShadow/distance","value":75,"merge":"same-style-control"})).unwrap();
    effectcraft_ui_egui::panels::layer_styles_dialog::finish(h.state_mut(), false);
    assert!(std::sync::Arc::ptr_eq(&h.state().session.project, &accepted));
}

#[test]
fn project_edit_commands_do_not_target_timeline_layers() {
    let mut h = harness();
    let comp = h.state().session.active_comp_id().unwrap();
    let id = h.state().session.active_comp().unwrap().layers[0].id;
    h.state_mut().session.execute("layer.select", json!({"layers":[id.0]})).unwrap();
    h.state_mut().session.execute("project.select", json!({"items":[comp.0]})).unwrap();
    h.state_mut().ui.focused = PanelKind::Project;
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "edit.duplicate", json!({})).unwrap();
    assert_eq!(h.state().session.active_comp().unwrap().layers.len(), 4);
    assert_ne!(h.state().session.state.project_selection, vec![comp]);
    assert!(effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "edit.cut", json!({})).is_err());
    assert!(h.state().session.active_comp().unwrap().layer(id).is_some());
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "edit.deselectAll", json!({})).unwrap();
    assert!(h.state().session.state.project_selection.is_empty());
    assert_eq!(h.state().session.state.selected_layers, vec![id]);
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "edit.selectAll", json!({})).unwrap();
    assert!(h.state().session.state.project_selection.contains(&comp));
    assert_eq!(h.state().session.state.selected_layers, vec![id]);
}

#[test]
fn compact_settings_keeps_confirmation_buttons_clickable() {
    let mut h = Harness::builder().with_size(vec2(1024.0, 768.0)).build_eframe(|_| EffectcraftApp::new(Session::default()));
    h.state_mut().dialog = Some(effectcraft_ui_egui::Dialog::Settings);
    h.run_steps(4);
    h.get_by_label("   OK   ").click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
}

#[test]
fn filtered_project_select_all_does_not_delete_hidden_assets() {
    let mut h = harness();
    let comp = h.state().session.active_comp_id().unwrap();
    let before = h.state().session.project.items.len();
    h.state_mut().ui.focused = PanelKind::Project;
    h.state_mut().ui.project_search = "Study".into();
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "edit.selectAll", json!({})).unwrap();
    assert_eq!(h.state().session.state.project_selection, vec![comp]);
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "edit.clear", json!({})).unwrap();
    assert_eq!(h.state().session.project.items.len(), before - 1);
}

/// Original four-tile UI fixture; no Adobe assets or reference screenshots.
#[test]
#[ignore]
fn workflow_snapshot() {
    let mut h = harness();
    let ids: Vec<LayerId> = h.state().session.active_comp().unwrap().layers.iter().map(|l| l.id).collect();
    for (n, id) in ids.iter().enumerate() {
        h.state_mut().session.execute("prop.set", json!({"layer":id.0,"path":"transform/position","value":[120.0+n as f64*120.0,180.0]})).unwrap();
    }
    h.state_mut().session.execute("layer.select", json!({"layers":ids[..3].iter().map(|i|i.0).collect::<Vec<_>>()})).unwrap();
    h.run_steps(3);
    let out = std::env::var("WORKFLOW_SNAPSHOT").expect("set WORKFLOW_SNAPSHOT to the output PNG");
    let comp = h.state().session.active_comp_id().unwrap();
    let key = effectcraft_ui_egui::frames::FrameKey { frame: 0, ..h.state().shown_series(comp) };
    for _ in 0..600 {
        h.step();
        if h.state().frames.is_cached(&key) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(h.state().frames.is_cached(&key), "current composition rendered before capture");
    h.run_steps(3);
    h.render().expect("offscreen UI render").save(out).unwrap();
}

/// Filled rects painted this frame (flattening nested shape lists).
fn filled_rects(h: &Harness<'_, EffectcraftApp>) -> Vec<(Rect, egui::Color32)> {
    fn walk(s: &egui::Shape, out: &mut Vec<(Rect, egui::Color32)>) {
        match s {
            egui::Shape::Rect(r) => out.push((r.rect, r.fill)),
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

/// Issues #91 / #97: clicking an effect's name in the Timeline selects it, and its row then
/// highlights like a selected property row (it used to stay unhighlighted).
#[test]
fn timeline_selected_effect_row_highlights() {
    let mut h = harness();
    let id = h.state().session.active_comp().unwrap().layers[0].id;
    let fx = h.state_mut().session.execute("effect.apply", json!({"layers": [id.0], "effect": "Gaussian Blur"})).unwrap()["effects"][0].as_u64().unwrap();
    let fx_group = h.state().session.active_comp().unwrap().layer(id).unwrap().props.sub("effects").unwrap().uid;
    h.state_mut().ui.timeline.open_layers.insert(id.0);
    h.state_mut().ui.timeline.open_groups.insert(fx_group);
    // `effect.apply` selects the new effect; start from no property selection.
    h.state_mut().session.state.selected_props.clear();
    h.run_steps(3);
    let row_selected = h.state().tokens.row_selected;
    let name = rect(&h, &format!("timeline.group.{fx}.name"));
    let highlighted = |h: &Harness<'_, EffectcraftApp>| filled_rects(h).iter().any(|(r, c)| *c == row_selected && r.contains(name.center()));
    assert!(!highlighted(&h), "not highlighted before it is selected");
    click(&mut h, name.center(), Modifiers::NONE);
    assert!(h.state().session.state.selected_props.contains(&(id, fx)), "the click selects the effect");
    assert!(highlighted(&h), "the selected effect's row is highlighted");
}
