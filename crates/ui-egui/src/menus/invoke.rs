use super::frontend::{file_dialog, frontend, run_engine};
use super::shortcuts::system_paste_id;
use super::*;

pub fn panel_command_id(p: PanelKind) -> String {
    format!("window.panel.{}", p.id())
}

/// The reveal shortcuts (P, S, R, T, A, E, M, F, L…). `add` (Shift+key) adds the property to
/// (or removes it from) those already revealed instead of replacing them.
pub fn reveal(app: &mut EffectcraftApp, kind: &str, now: f64, add: bool) {
    // The double-press shortcuts: the same key again within 0.6 s reveals its second set (LL
    // Waveform, TT Mask Opacity, MM all mask properties, EE Expressions, RR Time Remap, AA
    // Material Options, PP paint / Roto Brush / Puppet, SS selected properties, FF missing
    // effects). A single M shows Mask Path.
    let double = |k: &str| -> Option<&'static str> {
        Some(match k {
            "levels" => "waveform",
            "opacity" => "maskOpacity",
            "masks" => "masks",
            "effects" => "expressions",
            "rotation" => "timeRemap",
            "anchor" => "material",
            "position" => "paint",
            "scale" => "selected",
            "feather" => "missingEffects",
            _ => return None,
        })
    };
    let pressed = kind;
    // (With Shift a second press removes the property again instead.)
    let again = !add && matches!(app.last_reveal.take(), Some((k, t)) if k == pressed && now - t < 0.6);
    app.last_reveal = Some((if again { String::new() } else { pressed.to_string() }, now));
    let kind = match (again, double(pressed)) {
        (true, Some(d)) => {
            // The second press replaces what the first one revealed.
            let first = if pressed == "masks" { "maskPath" } else { pressed };
            app.ui.timeline.reveal.retain(|k| k != first);
            d
        }
        _ if pressed == "masks" => "maskPath",
        _ => pressed,
    };
    let targets = reveal_targets(app);
    let tl = &mut app.ui.timeline;
    let present = tl.reveal.iter().any(|k| k == kind);
    let mut kinds = tl.reveal.clone();
    if add {
        if present {
            kinds.retain(|k| k != kind);
        } else {
            kinds.retain(|k| k != "props" || !tl.reveal_props.is_empty());
            kinds.push(kind.to_string());
        }
    } else if present && kinds.len() == 1 {
        kinds.clear();
    } else {
        kinds = vec![kind.to_string()];
    }
    tl.apply_reveal(&targets, kinds.clone());
    crate::panels::timeline::open_revealed(app, &targets, &kinds);
}

/// Show Time Remap on the layers of a `layer.enableTimeRemap` (`layers` by id, else the
/// selected ones) that have it now.
fn reveal_time_remap(app: &mut EffectcraftApp, params: &Value) {
    let asked: Vec<u64> = match params.get("layers").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(Value::as_u64).collect(),
        None => app.session.state.selected_layers.iter().map(|l| l.0).collect(),
    };
    let Some(comp) = app.session.active_comp() else { return };
    let layers: Vec<u64> = comp.layers.iter().filter(|l| asked.contains(&l.id.0) && l.props.get("timeRemap").is_some()).map(|l| l.id.0).collect();
    if !layers.is_empty() {
        app.ui.timeline.apply_reveal(&layers, vec!["timeRemap".into()]);
    }
}

/// After Alt+Shift+P (A, S, R, T) the Timeline shows the property on each layer it keyed, as in
/// After Effects: added to what a reveal shortcut shows, else with the layer's Transform twirled
/// open, else alone like P.
fn reveal_keyed(app: &mut EffectcraftApp, keyed: &Value, kind: &str) {
    let layers: std::collections::BTreeSet<u64> = keyed.as_array().into_iter().flatten().filter_map(|k| k.get("layer").and_then(Value::as_u64)).collect();
    let Some(comp) = app.session.active_comp() else { return };
    let tl = &mut app.ui.timeline;
    for id in layers {
        let shown = tl.layer_reveal.get(&id).filter(|k| !k.is_empty()).cloned();
        match shown {
            Some(mut kinds) => {
                if !kinds.iter().any(|k| k == kind) {
                    kinds.push(kind.to_string());
                    tl.layer_reveal.insert(id, kinds);
                }
            }
            None if tl.open_layers.contains(&id) => {
                if let Some(tr) = comp.layer(effectcraft_engine::project::LayerId(id)).and_then(|l| l.transform()) {
                    tl.open_groups.insert(tr.uid);
                }
            }
            None => {
                tl.open_layers.insert(id);
                tl.layer_reveal.insert(id, vec![kind.to_string()]);
            }
        }
    }
}

/// The layers a reveal shortcut acts on: the selected ones, else every layer of the active comp.
fn reveal_targets(app: &EffectcraftApp) -> Vec<u64> {
    if app.session.state.selected_layers.is_empty() {
        app.session.active_comp().map(|c| c.layers.iter().map(|l| l.id.0).collect()).unwrap_or_default()
    } else {
        app.session.state.selected_layers.iter().map(|l| l.0).collect()
    }
}

/// The arrow keys nudge the selected layers 1 pixel at the viewer's magnification (`up`,
/// `down`, `left`, `right`), 10 with Shift (`up10`…), as After Effects does, so zoomed in
/// they move by fractions of a comp pixel. From any panel except Project (which owns the
/// arrow keys for its own keyboard navigation, #961); not while typing, with a dialog or
/// menu open. With mask points selected the arrows move the points instead (#1288).
fn nudge(app: &mut EffectcraftApp, ctx: &egui::Context, dir: &str) -> Result<Value, String> {
    let (side, n) = match dir.strip_suffix("10") {
        Some(side) => (side, 10.0),
        None => (dir, 1.0),
    };
    let (x, y) = match side {
        "up" => (0.0, -n),
        "down" => (0.0, n),
        "left" => (-n, 0.0),
        "right" => (n, 0.0),
        _ => return Err(format!("unknown nudge `{dir}`")),
    };
    let st = &app.session.state;
    if app.ui.focused == PanelKind::Project || (st.selected_layers.is_empty() && st.selected_vertices.is_empty()) || st.text_edit.is_some() {
        return Ok(Value::Null);
    }
    // Screen pixels per comp pixel.
    let zoom = (app.ui.viewer.zoom.unwrap_or_else(|| crate::panels::viewer::last_fit(ctx)) * ctx.pixels_per_point()) as f64;
    let k = if zoom.is_finite() && zoom > 1e-6 { 1.0 / zoom } else { 1.0 };
    if let Some(layer) = st.selected_vertices.first().map(|v| v.layer) {
        let d = vertex_delta(app, layer, [x * k, y * k]);
        return run_engine(app, ctx, "mask.moveVertices", json!({"delta": d}));
    }
    run_engine(app, ctx, "layer.nudge", json!({"x": x * k, "y": y * k}))
}

/// A comp-space step `d` in the layer space of `layer`, where mask points live. Like a drag of
/// the points in the viewer, every selected point takes the first one's layer space.
fn vertex_delta(app: &EffectcraftApp, layer: effectcraft_engine::project::LayerId, d: [f64; 2]) -> [f64; 2] {
    use effectcraft_engine::geom::vec2;
    let s = &app.session;
    let (Some(cid), Some(comp)) = (s.active_comp_id(), s.active_comp()) else { return d };
    let Some(l) = comp.layer(layer) else { return d };
    let ectx = effectcraft_engine::render::EvalCtx { project: &s.project, comp_id: cid, comp, time: s.time(), expr: s.expr.as_deref(), footage: None };
    let (l2c, _) = crate::panels::viewer::l2c(&ectx, l);
    l2c.inverse().map_or(d, |inv| {
        let v = inv.apply_vec(vec2(d[0], d[1]));
        [v.x, v.y]
    })
}

fn no_params(p: &Value) -> bool {
    p.as_object().is_none_or(|m| m.is_empty())
}

/// `{"comp": id}` and nothing else: Composition Settings opens on that comp instead of
/// applying no settings to it.
fn only_comp(p: &Value) -> Option<effectcraft_engine::project::ItemId> {
    let m = p.as_object().filter(|m| m.len() == 1)?;
    m.get("comp")?.as_u64().map(effectcraft_engine::project::ItemId)
}

/// Execute a UI or engine command by id.
pub fn invoke(app: &mut EffectcraftApp, ctx: &egui::Context, id: &str, params: Value) -> Result<Value, String> {
    if matches!(id, "effects.copyPresetsFolderPath" | "mediaBrowser.copySourcePath") {
        return frontend(app, ctx, id, params);
    }
    // Layer ▸ New ▸ Text (menu, shortcut, context menus) puts an insertion point in the centre of
    // the comp, as After Effects does, so typing goes into Source Text rather than running
    // shortcuts (#866). `engine.execute` keeps the headless default.
    if id == "layer.newText" && no_params(&params) {
        return run_engine(app, ctx, id, json!({"text": "", "edit": true}));
    }
    // A new folder (File ▸ New, its shortcut, the Project panel's button and context menu) opens
    // with its name ready to type, as in After Effects (#1658). The command selects it.
    // `engine.execute` keeps the headless default.
    if id == "project.newFolder" && no_params(&params) {
        let r = run_engine(app, ctx, id, params);
        if r.is_ok() {
            app.show_panel(PanelKind::Project);
            crate::panels::project::begin_rename(app, ctx);
        }
        return r;
    }
    // The Project-panel focus redirects apply only while no text edit is open: creating a text
    // layer leaves focus on Project while the insertion point lives in the comp, so during an
    // edit these commands address the text context — redirected, Deselect All would clear a
    // project selection the user is not looking at and silently swallow the edit's own end.
    if no_params(&params) && app.ui.focused == PanelKind::Project && app.session.state.text_edit.is_none() {
        match id {
            "edit.duplicate" => return run_engine(app, ctx, "project.duplicate", json!({})),
            "edit.selectAll" => {
                let items: Vec<u64> = crate::panels::project::visible_rows(app)
                    .into_iter()
                    .filter(|(id, _)| app.ui.project_search.is_empty() || app.session.project.item(*id).is_some_and(|it| !it.is_folder()))
                    .map(|(id, _)| id.0)
                    .collect();
                return run_engine(app, ctx, "project.select", json!({"items":items}));
            }
            "edit.deselectAll" => return run_engine(app, ctx, "project.select", json!({"items":[]})),
            "edit.cut" | "edit.copy" | "edit.paste" | "edit.copyWithPropertyLinks" | "edit.copyWithRelativePropertyLinks" | "edit.copyExpressionOnly" => {
                return Err("Project clipboard actions are not supported yet".into());
            }
            _ => {}
        }
    }
    // Edit ▸ Paste with SVG markup on the system clipboard pastes it as a shape layer (After
    // Effects 26.3). Copying in the app replaces the system clipboard with a note naming what
    // was copied, so SVG there was copied after anything copied in the app.
    if id == "edit.paste" && no_params(&params) && app.session.state.text_edit.is_none() {
        let text = ctx.data_mut(|d| d.remove_temp::<String>(system_paste_id())).or_else(|| app.hooks.clipboard_text.as_ref().and_then(|f| f()));
        if let Some(svg) = text.filter(|t| effectcraft_engine::vector::is_svg_text(t)) {
            return run_engine(app, ctx, "edit.pasteSvg", json!({"svg": svg}));
        }
    }
    // Clear follows panel focus, including the Edit menu and keyboard shortcut.
    if id == "edit.clear" && params.as_object().is_some_and(|p| p.is_empty()) && app.ui.focused == PanelKind::Project {
        // Through `invoke`, so deleting items that compositions use still asks first (M3.16).
        return invoke(app, ctx, "project.delete", json!({}));
    }
    if id == "edit.clear" && params.as_object().is_some_and(|p| p.is_empty()) && app.ui.focused == PanelKind::RenderQueue {
        let selected = ctx.data(|d| d.get_temp::<(Vec<u64>, Option<u64>)>(egui::Id::unique("rq-ui"))).and_then(|(_, id)| id);
        return match selected {
            Some(item) => run_engine(app, ctx, "renderQueue.remove", json!({"item": item})),
            None => Err("select a Render Queue item first".into()),
        };
    }
    // Shift+Delete deletes the Project panel's selection without asking; it does nothing in the
    // other panels.
    if id == "project.deleteWithoutConfirmation" {
        if app.ui.focused != PanelKind::Project {
            return Ok(Value::Null);
        }
        return crate::panels::delete_items::delete_confirmed(app, ctx, json!({}));
    }
    let now = app.now();
    // Closing a modified project asks to save it first; the command runs once answered.
    if crate::panels::unsaved::guard(app, id, &params) {
        return Ok(json!({"dialog": "unsavedChanges"}));
    }
    // Deleting Project items that compositions use asks first, as in After Effects.
    if crate::panels::delete_items::guard(app, id, &params) {
        return Ok(json!({"dialog": "deleteItems"}));
    }
    // J / K and Select All Keyframes act on what the Timeline shows (its revealed properties).
    if matches!(id, "time.nextKey" | "time.previousKey" | "keys.selectAll")
        && ["visible", "layers", "layer", "prop"].iter().all(|k| params.get(*k).is_none())
        && let Some(c) = app.session.active_comp_arc()
    {
        let mut p = if params.is_object() { params.clone() } else { json!({}) };
        p["visible"] = json!(crate::panels::timeline::visible_props(app, &c));
        return run_engine(app, ctx, id, p);
    }
    // New Camera/Light and Camera/Light Settings without parameters open their dialogs.
    if crate::panels::dialogs_3d::route(app, id, &params)? {
        return Ok(Value::Null);
    }
    // New 3D Primitive without parameters opens its dialog.
    if crate::panels::primitive_dialog::route(app, id, &params)? {
        return Ok(Value::Null);
    }
    // Layer Settings on a solid, adjustment layer or null opens its settings dialog.
    if crate::panels::dialogs::route_layer_settings(app, id, &params)? {
        return Ok(Value::Null);
    }
    // Layer ▸ Layer Styles ▸ <style> from the menu adds the style and opens the dialog on it.
    if crate::panels::layer_styles_dialog::route(app, id, &params)? {
        return Ok(Value::Null);
    }
    // Shared docking: `window.dockAction`, craft-ui's `window.panel.*` commands, `ui.dock`,
    // `ui.dock.reset` and `window.panelWindow`.
    if let Some(result) = crate::dock_ui::route(app, ctx, id, &params) {
        return result;
    }
    // Legacy per-panel / per-workspace ids (`window.panel.Project`, `window.workspace.default`).
    if let Some(rest) = id.strip_prefix("window.panel.") {
        return frontend(app, ctx, "window.panel", json!({"panel": rest}));
    }
    if let Some(ws) = id.strip_prefix("window.workspace.") {
        if ws == "reset" {
            return frontend(app, ctx, "window.resetWorkspace", json!({}));
        }
        return frontend(app, ctx, "window.workspace", json!({"name": ws}));
    }
    if let Some(t) = id.strip_prefix("tool.") {
        // C cycles through the camera tools.
        if t == "camera" {
            let tools = [Tool::Orbit, Tool::PanCamera, Tool::Dolly];
            let next = match tools.iter().position(|x| *x == app.ui.tool) {
                Some(i) => tools[(i + 1) % tools.len()],
                None => Tool::Orbit,
            };
            app.ui.tool = next;
            return Ok(json!({"tool": next}));
        }
        // Slot shortcuts cycle through the slot's tools.
        let slot = match t {
            "shape" => Some(8),
            "pen" => Some(9),
            "type" => Some(10),
            "brush" => Some(11),
            "rotoBrush" => Some(14),
            "puppet" => Some(15),
            _ => None,
        };
        if let Some(si) = slot {
            let tools: Vec<Tool> = match si {
                11 => vec![Tool::Brush, Tool::Clone, Tool::Eraser],
                _ => crate::state::Tool::SLOTS[si].to_vec(),
            };
            let cur = tools.iter().position(|x| *x == app.ui.tool);
            let next = match cur {
                Some(i) => tools[(i + 1) % tools.len()],
                None => app.ui.slot_tools.get(si).copied().filter(|x| tools.contains(x)).unwrap_or(tools[0]),
            };
            app.ui.tool = next;
            if si < app.ui.slot_tools.len() && si != 11 {
                app.ui.slot_tools[si] = next;
            }
            return Ok(json!({"tool": next}));
        }
        let tool = Tool::from_name(t).ok_or_else(|| format!("unknown tool `{t}`"))?;
        app.ui.tool = tool;
        return Ok(json!({"tool": tool}));
    }
    if let Some(dir) = id.strip_prefix("viewer.nudge.") {
        return nudge(app, ctx, dir);
    }
    if let Some(k) = id.strip_prefix("timeline.reveal.") {
        if k == "animated" {
            return run_engine(app, ctx, "anim.reveal", json!({"kind": "keyframes"}));
        }
        reveal(app, k, now, false);
        return Ok(Value::Null);
    }
    if let Some(k) = id.strip_prefix("timeline.keyAt.") {
        let r = run_engine(app, ctx, "keys.toggleTransform", json!({"prop": k}))?;
        reveal_keyed(app, &r, k);
        return Ok(r);
    }
    if let Some(k) = id.strip_prefix("timeline.revealAdd.") {
        reveal(app, k, now, true);
        return Ok(json!({"revealed": app.ui.timeline.reveal}));
    }
    // U reveals keyframed properties; UU (pressed twice quickly) all modified properties; U
    // again later hides them.
    if id == "anim.reveal" && params.get("kind").and_then(Value::as_str) == Some("keyframes") {
        let last = app.last_reveal.take();
        app.last_reveal = Some(("keyframes".into(), now));
        match last {
            Some((k, t)) if k == "keyframes" && now - t < 0.6 => {
                return run_engine(app, ctx, "anim.reveal", json!({"kind": "modified"}));
            }
            Some((k, _)) if k == "keyframes" && app.ui.timeline.reveal == ["props"] => {
                let targets = reveal_targets(app);
                app.ui.timeline.apply_reveal(&targets, vec![]);
                return Ok(json!({"revealed": 0}));
            }
            _ => {}
        }
    }
    // A UI-only command (not in the engine's menus), so `frontend` gets it from here.
    if id == "window.maximizeApp" {
        return frontend(app, ctx, id, params);
    }
    // Docking: {panel, anchor, zone: center|left|right|top|bottom}, {panel, rect?}, {panel?}.
    if let Some(op) = id.strip_prefix("window.").filter(|o| matches!(*o, "maximizePanel" | "dockPanel" | "floatPanel" | "closePanel")) {
        let panel_of = |k: &str| params.get(k).and_then(Value::as_str).map(|s| PanelKind::from_name(s).ok_or(format!("unknown panel `{s}`")));
        match op {
            "maximizePanel" => {
                let p = match panel_of("panel") {
                    Some(p) => p?,
                    None => app.panel_at(ctx.pointer_hover_pos()),
                };
                app.toggle_maximize(p);
                return Ok(json!({"maximized": app.ui.maximized}));
            }
            "closePanel" => {
                let p = panel_of("panel").ok_or("closePanel: need `panel`")??;
                if app.ui.dock.is_open(p) {
                    app.apply_dock(craft_ui::docking::Action::Close { panel: p })?;
                }
                return Ok(Value::Null);
            }
            "floatPanel" => {
                let p = panel_of("panel").ok_or("floatPanel: need `panel`")??;
                let r = match params.get("rect") {
                    Some(value) => serde_json::from_value::<[f32; 4]>(value.clone()).map_err(|_| "rect must contain four finite numbers")?,
                    None => crate::dock_ui::default_float_rect(ctx.content_rect()),
                };
                app.apply_dock(craft_ui::docking::Action::Float { panel: p, rect: r }).map_err(|e| format!("can't undock {}: {e}", p.title()))?;
                return Ok(Value::Null);
            }
            _ => {
                // craft-ui's `window.panel.dock`: zones center/tab, left/right/top/bottom (a split
                // beside the anchor), above/below; without an anchor a side zone docks along that
                // edge of the whole workspace.
                let p = panel_of("panel").ok_or("dockPanel: need `panel`")??;
                let mut command = json!({"command": "window.panel.dock", "panel": p});
                if let Some(anchor) = panel_of("anchor") {
                    command["anchor"] = json!(anchor?);
                    command["zone"] = json!("center");
                }
                if let Some(zone) = params.get("zone") {
                    let zone = zone.as_str().ok_or("zone: center|tab|left|right|top|bottom|above|below")?.to_ascii_lowercase();
                    if command.get("anchor").is_some() || zone != "center" {
                        command["zone"] = json!(zone);
                    }
                }
                if let Some(before) = panel_of("before") {
                    command["before"] = json!(before?);
                }
                let command: craft_ui::docking::DockCommand<PanelKind> =
                    serde_json::from_value(command).map_err(|e| format!("dockPanel: zone: center|tab|left|right|top|bottom|above|below ({e})"))?;
                app.run_dock_command(ctx, command).map_err(|e| format!("can't dock {}: {e}", p.title()))?;
                app.show_panel(p);
                return Ok(Value::Null);
            }
        }
    }
    if id == "markers.dialog" {
        // The Composition/Layer Marker dialog (double-click a marker): {layer?, index}.
        let index = params.get("index").and_then(Value::as_u64).ok_or("markers.dialog: need `index`")? as usize;
        let layer = params.get("layer").and_then(Value::as_u64);
        crate::panels::markers_ui::open_dialog(app, crate::panels::markers_ui::MarkerRef { layer, index })?;
        return Ok(json!({"dialog": id}));
    }

    if id == "view.theme.toggle" {
        app.cycle_appearance(ctx);
        return Ok(json!({"appearanceMode": app.session.prefs.appearance.appearance_mode}));
    }
    if let Some(th) = id.strip_prefix("view.theme.") {
        let k = crate::theme::ThemeKind::from_name(th).ok_or("unknown theme")?;
        app.set_theme(ctx, k);
        return Ok(Value::Null);
    }
    let timing = |app: &mut EffectcraftApp, op: &str| app.session.execute("layer.timing", json!({"op": op})).map_err(|e| e.to_string());
    match id {
        "playback.ramPreview" => {
            app.toggle_play_with(now, effectcraft_engine::preview::PreviewShortcut::Numpad0);
            return Ok(json!({"playing": app.playback.playing}));
        }
        "playback.preview.shiftSpacebar" | "playback.preview.shiftNumpad0" | "playback.preview.altNumpad0" => {
            let sc = effectcraft_engine::preview::PreviewShortcut::parse(id.trim_start_matches("playback.preview.")).ok_or("unknown preview shortcut")?;
            app.toggle_play_with(now, sc);
            return Ok(json!({"playing": app.playback.playing}));
        }
        "playback.stop" => {
            app.stop();
            return Ok(Value::Null);
        }
        // The focused Layer panel has its own magnification; otherwise the Composition viewer.
        "view.fit" if app.ui.focused == PanelKind::Layer => {
            app.ui.layer_zoom = None;
            app.ui.layer_pan = [0.0, 0.0];
            return Ok(Value::Null);
        }
        "view.fit" => {
            app.ui.viewer.zoom = None;
            app.ui.viewer.fit_up_to_100 = false;
            app.ui.viewer.pan = [0.0, 0.0];
            return Ok(Value::Null);
        }
        "timeline.scrollToTime" => app.ui.timeline.scroll_to = Some(crate::state::ScrollTo::Time),
        "timeline.scrollToLayer" => app.ui.timeline.scroll_to = Some(crate::state::ScrollTo::Layer),
        "timeline.parentColumn" => {
            let on = !crate::panels::timeline::column_visible(&app.ui.timeline, "parent");
            crate::panels::timeline::set_column(&mut app.ui.timeline, "parent", on)?;
            return Ok(json!({"parent": on}));
        }
        "view.fitUpTo100" => {
            app.ui.viewer.zoom = None;
            app.ui.viewer.fit_up_to_100 = true;
            app.ui.viewer.pan = [0.0, 0.0];
            return Ok(Value::Null);
        }
        // Shift+, / Shift+.: the previous / next open composition (the viewer's tabs).
        "view.nextItem" | "view.previousItem" => {
            let open = app.session.state.open_comps.clone();
            let Some(i) = app.session.state.active_comp.and_then(|c| open.iter().position(|o| *o == c)) else { return Ok(Value::Null) };
            let step = if id == "view.nextItem" { 1 } else { open.len().saturating_sub(1) };
            let Some(next) = open.get((i + step) % open.len().max(1)).copied().filter(|n| Some(*n) != app.session.state.active_comp) else {
                return Ok(Value::Null);
            };
            return run_engine(app, ctx, "comp.open", json!({"comp": next.0}));
        }
        // \\: between the Composition viewer and the Timeline.
        "window.toggleCompTimeline" => {
            let to = if app.ui.focused == PanelKind::Timeline { crate::panels::viewers::active_panel(app) } else { PanelKind::Timeline };
            app.show_panel(to);
            return Ok(json!({"focused": to.id()}));
        }
        // Alt+Shift+, / Alt+Shift+.: the previous / next tab of the focused panel's group.
        "window.nextPanel" | "window.previousPanel" => {
            let focused = app.ui.focused;
            let group = app.ui.dock.group_of(focused);
            let Some(i) = group.iter().position(|p| *p == focused) else { return Ok(Value::Null) };
            let step = if id == "window.nextPanel" { 1 } else { group.len().saturating_sub(1) };
            if let Some(next) = group.get((i + step) % group.len().max(1)).copied() {
                app.show_panel(next);
            }
            return Ok(json!({"focused": app.ui.focused.id()}));
        }
        // Ctrl+Shift+W: with a viewer or the Timeline active, every open composition closes;
        // any other panel closes itself.
        "window.closeAll" => {
            let focused = app.ui.focused;
            if matches!(focused, PanelKind::Composition | PanelKind::Timeline | PanelKind::Viewer(_)) {
                for c in app.session.state.open_comps.clone() {
                    run_engine(app, ctx, "comp.close", json!({"comp": c.0}))?;
                }
            } else {
                app.close_panel(focused);
            }
            return Ok(Value::Null);
        }
        "view.actualSize" if app.ui.focused == PanelKind::Layer => {
            app.ui.layer_zoom = Some(1.0 / ctx.pixels_per_point());
            app.ui.layer_pan = [0.0, 0.0];
            return Ok(Value::Null);
        }
        "view.actualSize" => {
            app.ui.viewer.zoom = Some(1.0 / ctx.pixels_per_point());
            app.ui.viewer.pan = [0.0, 0.0];
            return Ok(Value::Null);
        }
        "view.safeMargins" => app.ui.viewer.safe_margins = !app.ui.viewer.safe_margins,
        "view.transparencyGrid" => app.ui.viewer.transparency_grid = !app.ui.viewer.transparency_grid,
        "view.fastPreviews" => {
            // With a mode: Fast Previews ▸ Off / Adaptive Resolution / Draft / Fast Draft /
            // Wireframe; without: toggle Draft quality (Settings ▸ Previews).
            if params.get("mode").is_some() {
                return run_engine(app, ctx, "view.fastPreviewMode", params);
            }
            let on = !app.ui.viewer.fast_preview;
            app.set_pref("previews.fastPreviews", json!(on))?;
            app.ui.viewer.fast_preview = on;
        }
        "timeline.zoomIn" | "timeline.zoomOut" => {
            let k = if id == "timeline.zoomIn" { crate::panels::timeline::ZOOM_STEP } else { 1.0 / crate::panels::timeline::ZOOM_STEP };
            crate::panels::timeline::zoom(app, ctx, k);
            return Ok(Value::Null);
        }
        "timeline.zoomFit" => crate::panels::timeline::toggle_fit_zoom(app, ctx),
        "timeline.zoomFrameToggle" => crate::panels::timeline::toggle_frame_zoom(app, ctx),
        "timeline.column" => {
            let col = params.get("column").and_then(Value::as_str).ok_or("timeline.column: need `column`")?;
            let on = params.get("visible").and_then(Value::as_bool).unwrap_or(!crate::panels::timeline::column_visible(&app.ui.timeline, col));
            crate::panels::timeline::set_column(&mut app.ui.timeline, col, on)?;
            let shown: Vec<&str> =
                crate::panels::timeline::COLUMNS.iter().map(|c| c.0).filter(|c| crate::panels::timeline::column_visible(&app.ui.timeline, c)).collect();
            return Ok(json!({"columns": shown}));
        }
        "timeline.sourceName" => {
            let tl = &mut app.ui.timeline;
            tl.source_name = params.get("value").and_then(Value::as_bool).unwrap_or(!tl.source_name);
            return Ok(json!({"sourceName": tl.source_name}));
        }
        "flowchart.options" | "flowchart.graph" => return crate::panels::flowchart::command(app, id, &params),
        "timeline.search" => {
            app.ui.timeline.search = params.get("query").and_then(Value::as_str).unwrap_or_default().to_string();
            app.show_panel(PanelKind::Timeline);
        }
        "timeline.graphEditor" => app.ui.timeline.graph_editor = !app.ui.timeline.graph_editor,
        "timeline.switchesModes" => app.ui.timeline.show_modes = !app.ui.timeline.show_modes,
        "timeline.collapseAll" => {
            app.ui.timeline.open_layers.clear();
            app.ui.timeline.open_groups.clear();
            app.ui.timeline.reveal.clear();
            app.ui.timeline.layer_reveal.clear();
        }
        // Ctrl+`: twirl the selected layers open (all closed ones) or closed (all open). From a
        // reveal shortcut's view it opens their full property trees.
        "timeline.twirlSelected" => {
            let sel: Vec<u64> = app.session.state.selected_layers.iter().map(|l| l.0).collect();
            let tl = &mut app.ui.timeline;
            let revealing = sel.iter().any(|l| tl.layer_reveal.contains_key(l));
            tl.reveal.clear();
            for l in &sel {
                tl.layer_reveal.remove(l);
            }
            if !revealing && sel.iter().all(|l| tl.open_layers.contains(l)) {
                for l in &sel {
                    tl.open_layers.remove(l);
                }
            } else {
                tl.open_layers.extend(sel);
            }
        }
        "timeline.workAreaBegin" => return app.session.execute("comp.workArea", json!({"set": "begin"})).map_err(|e| e.to_string()),
        "timeline.workAreaEnd" => return app.session.execute("comp.workArea", json!({"set": "end"})).map_err(|e| e.to_string()),
        "timeline.moveInToTime" => return timing(app, "moveInToTime"),
        "timeline.moveOutToTime" => return timing(app, "moveOutToTime"),
        "timeline.trimInToTime" => return timing(app, "trimInToTime"),
        "timeline.trimOutToTime" => return timing(app, "trimOutToTime"),
        "app.home" => app.ui.start_screen = !app.ui.start_screen,
        "app.newComp" | "comp.new" if no_params(&params) => crate::panels::dialogs::open_new_comp(app),
        "app.compSettings" | "comp.settings" if no_params(&params) => crate::panels::dialogs::open_comp_settings(app)?,
        "app.compSettings" | "comp.settings" if only_comp(&params).is_some() => {
            crate::panels::dialogs::open_comp_settings_for(app, only_comp(&params).ok_or("missing `comp`")?)?
        }
        "app.solidSettings" | "layer.newSolid" if no_params(&params) => crate::panels::dialogs::open_new_solid(app)?,
        "keys.velocity" if no_params(&params) => crate::panels::key_dialogs::open_velocity(app)?,
        "keys.interpolation" if no_params(&params) => crate::panels::key_dialogs::open_interpolation(app)?,
        "layer.timeStretch" if no_params(&params) => crate::panels::key_dialogs::open_time_stretch(app)?,
        _ => {
            if id == "renderQueue.render" && params.get("wait").is_none() {
                // The UI renders in the background; the panel shows progress.
                let mut p = params.clone();
                if let Some(m) = p.as_object_mut() {
                    m.insert("wait".into(), json!(false));
                } else {
                    p = json!({"wait": false});
                }
                app.show_panel(PanelKind::RenderQueue);
                return app.session.execute(id, p).map_err(|e| e.to_string());
            }
            if id == "renderQueue.add" {
                let r = app.session.execute(id, params).map_err(|e| e.to_string());
                match &r {
                    Ok(_) => app.show_panel(PanelKind::RenderQueue),
                    Err(e) => app.ui.status = e.clone(),
                }
                return r;
            }
            if let Some(r) = file_dialog(app, id, &params) {
                return r;
            }
            if id == "layer.precompose" && params.get("name").is_none() {
                crate::panels::precomp::open(app, &params)?;
                return Ok(json!({"dialog": id}));
            }
            if crate::panels::dialogs::open_form(app, id, &params) {
                return Ok(json!({"dialog": id}));
            }
            if id == "renderQueue.render" && params.get("wait").is_none() {
                // The UI renders in the background; the panel shows progress.
                let mut p = params.clone();
                if let Some(m) = p.as_object_mut() {
                    m.insert("wait".into(), json!(false));
                } else {
                    p = json!({"wait": false});
                }
                app.show_panel(PanelKind::RenderQueue);
                return app.session.execute(id, p).map_err(|e| e.to_string());
            }
            if id == "renderQueue.add" {
                let r = app.session.execute(id, params).map_err(|e| e.to_string());
                match &r {
                    Ok(_) => app.show_panel(PanelKind::RenderQueue),
                    Err(e) => app.ui.status = e.clone(),
                }
                return r;
            }
            // An effect added to a layer (Effect menu, Last Effect, dropped on a layer) brings
            // up Effect Controls on that layer, as in After Effects.
            if matches!(id, "effect.apply" | "effect.applyLast") {
                let r = run_engine(app, ctx, id, params.clone());
                if let Ok(applied) = &r {
                    crate::panels::effect_controls::reveal_applied(app, &params);
                    crate::panels::effect_controls::choose_files_after_apply(app, applied);
                }
                return r;
            }
            // Enabling time remapping twirls its layers open on Time Remap and its two keys.
            if id == "layer.enableTimeRemap" {
                let r = run_engine(app, ctx, id, params.clone());
                if r.as_ref().is_ok_and(|on| on.as_bool() == Some(true)) {
                    reveal_time_remap(app, &params);
                }
                return r;
            }
            if id == "layer.rename" && params.get("name").is_none() {
                crate::panels::timeline::begin_rename(app, ctx);
                return Ok(Value::Null);
            }
            return run_engine(app, ctx, id, params);
        }
    }
    Ok(Value::Null)
}
