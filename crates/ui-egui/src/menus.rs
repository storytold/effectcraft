//! Menus, keyboard shortcuts and UI-level commands. The menu bar is the engine's After Effects
//! menu tree (`effectcraft_engine::menus`): every entry is an engine command, and frontend-only
//! commands (viewer zoom, panels, dialogs…) come back as `Event::Frontend` and are performed by
//! [`frontend`]. [`UI_COMMANDS`] holds the remaining UI-only shortcuts (tools, timeline
//! navigation, reveal keys). `invoke` is the single entry point used by menus, shortcuts and the
//! control channel.

use serde_json::{Value, json};

use crate::EffectcraftApp;
use crate::dock::PanelKind;
use crate::state::{Resolution, Tool};
use effectcraft_engine::menus::{MenuEntry, MenuNode};

pub struct UiCommand {
    pub id: &'static str,
    pub label: &'static str,
    pub menu: &'static [&'static str],
    pub shortcut: Option<&'static str>,
}

macro_rules! uic {
    ($id:literal, $label:literal, [$($m:literal),*], $sc:expr) => {
        UiCommand { id: $id, label: $label, menu: &[$($m),*], shortcut: $sc }
    };
}

/// UI-only commands (not in the AE menus; shortcuts and the command palette).
pub const UI_COMMANDS: &[UiCommand] = &[
    uic!("window.dockAction", "Docking Action", [], None),
    uic!("playback.ramPreview", "Preview (Numpad 0)", [], Some("Num0")),
    uic!("playback.preview.shiftSpacebar", "Preview (Shift+Spacebar)", [], Some("Shift+Space")),
    uic!("playback.preview.shiftNumpad0", "Preview (Shift+Numpad 0)", [], Some("Shift+Num0")),
    uic!("playback.preview.altNumpad0", "Preview (Alt+Numpad 0)", [], Some("Alt+Num0")),
    uic!("playback.stop", "Stop", [], None),
    uic!("view.fit", "Fit", [], Some("Shift+/")),
    uic!("view.actualSize", "100%", [], Some("/")),
    uic!("view.fitUpTo100", "Fit up to 100%", [], Some("Alt+/")),
    uic!("view.nextItem", "Next Composition in Viewer", [], Some("Shift+.")),
    uic!("view.previousItem", "Previous Composition in Viewer", [], Some("Shift+,")),
    uic!("viewer.nudge.up", "Move Selected Layers Up 1 Pixel at Current Magnification", [], Some("ArrowUp")),
    uic!("viewer.nudge.down", "Move Selected Layers Down 1 Pixel at Current Magnification", [], Some("ArrowDown")),
    uic!("viewer.nudge.left", "Move Selected Layers Left 1 Pixel at Current Magnification", [], Some("ArrowLeft")),
    uic!("viewer.nudge.right", "Move Selected Layers Right 1 Pixel at Current Magnification", [], Some("ArrowRight")),
    uic!("viewer.nudge.up10", "Move Selected Layers Up 10 Pixels at Current Magnification", [], Some("Shift+ArrowUp")),
    uic!("viewer.nudge.down10", "Move Selected Layers Down 10 Pixels at Current Magnification", [], Some("Shift+ArrowDown")),
    uic!("viewer.nudge.left10", "Move Selected Layers Left 10 Pixels at Current Magnification", [], Some("Shift+ArrowLeft")),
    uic!("viewer.nudge.right10", "Move Selected Layers Right 10 Pixels at Current Magnification", [], Some("Shift+ArrowRight")),
    uic!("view.safeMargins", "Title/Action Safe", [], Some("'")),
    uic!("view.transparencyGrid", "Transparency Grid", [], None),
    uic!("view.fastPreviews", "Fast Previews", [], None),
    uic!("view.theme.toggle", "Next Appearance Mode", [], None),
    uic!("view.theme.dark", "Theme: Dark", [], None),
    uic!("view.theme.darker", "Theme: Darker", [], None),
    uic!("view.theme.light", "Theme: Light", [], None),
    uic!("timeline.zoomIn", "Zoom In Time", [], Some("=")),
    uic!("timeline.zoomOut", "Zoom Out Time", [], Some("-")),
    uic!("timeline.zoomFit", "Zoom to Fit Comp", [], Some("Shift+;")),
    uic!("timeline.scrollToTime", "Scroll to Current Time", [], Some("D")),
    uic!("timeline.scrollToLayer", "Scroll Topmost Selected Layer to Top", [], Some("X")),
    uic!("timeline.parentColumn", "Show/Hide Parent Column", [], Some("Shift+F4")),
    uic!("timeline.zoomFrameToggle", "Zoom In to Frame Level / Out to the Whole Comp", [], Some(";")),
    uic!("timeline.graphEditor", "Graph Editor", [], Some("Shift+F3")),
    uic!("timeline.switchesModes", "Toggle Switches / Modes", [], Some("F4")),
    uic!("timeline.workAreaBegin", "Set Work Area Begin", [], Some("B")),
    uic!("timeline.workAreaEnd", "Set Work Area End", [], Some("N")),
    uic!("timeline.moveInToTime", "Move Layer In Point to Current Time", [], Some("[")),
    uic!("timeline.moveOutToTime", "Move Layer Out Point to Current Time", [], Some("]")),
    uic!("timeline.trimInToTime", "Trim Layer In Point to Current Time", [], Some("Alt+[")),
    uic!("timeline.trimOutToTime", "Trim Layer Out Point to Current Time", [], Some("Alt+]")),
    uic!("timeline.reveal.position", "Reveal Position", [], Some("P")),
    uic!("timeline.reveal.scale", "Reveal Scale", [], Some("S")),
    uic!("timeline.reveal.rotation", "Reveal Rotation", [], Some("R")),
    uic!("timeline.reveal.opacity", "Reveal Opacity", [], Some("T")),
    uic!("timeline.reveal.anchor", "Reveal Anchor Point", [], Some("A")),
    uic!("timeline.reveal.effects", "Reveal Effects", [], Some("E")),
    uic!("timeline.reveal.masks", "Reveal Masks", [], Some("M")),
    uic!("timeline.reveal.feather", "Reveal Mask Feather", [], Some("F")),
    uic!("timeline.reveal.levels", "Reveal Audio Levels (press twice: Waveform)", [], Some("L")),
    uic!("timeline.reveal.timeRemap", "Reveal Time Remap", [], None),
    uic!("timeline.reveal.expressions", "Reveal Expressions", [], None),
    uic!("timeline.reveal.maskOpacity", "Reveal Mask Opacity", [], None),
    uic!("timeline.reveal.maskPath", "Reveal Mask Path", [], None),
    uic!("timeline.reveal.material", "Reveal Material Options", [], None),
    uic!("timeline.reveal.paint", "Reveal Paint, Roto Brush and Puppet", [], None),
    uic!("timeline.reveal.selected", "Reveal Selected Properties", [], None),
    uic!("timeline.reveal.missingEffects", "Reveal Missing Effects", [], None),
    uic!("timeline.reveal.waveform", "Reveal Audio Waveform", [], None),
    uic!("timeline.revealAdd.position", "Add Position to Revealed Properties", [], Some("Shift+P")),
    uic!("timeline.revealAdd.scale", "Add Scale to Revealed Properties", [], Some("Shift+S")),
    uic!("timeline.revealAdd.rotation", "Add Rotation to Revealed Properties", [], Some("Shift+R")),
    uic!("timeline.revealAdd.opacity", "Add Opacity to Revealed Properties", [], Some("Shift+T")),
    uic!("timeline.revealAdd.anchor", "Add Anchor Point to Revealed Properties", [], Some("Shift+A")),
    uic!("timeline.revealAdd.effects", "Add Effects to Revealed Properties", [], Some("Shift+E")),
    uic!("timeline.revealAdd.masks", "Add Masks to Revealed Properties", [], Some("Shift+M")),
    uic!("timeline.revealAdd.feather", "Add Mask Feather to Revealed Properties", [], Some("Shift+F")),
    uic!("timeline.revealAdd.levels", "Add Audio Levels to Revealed Properties", [], Some("Shift+L")),
    uic!("timeline.revealAdd.animated", "Add Animated Properties to Revealed Properties", [], Some("Shift+U")),
    uic!("timeline.column", "Show/Hide Timeline Column", [], None),
    uic!("timeline.sourceName", "Source Name / Layer Name", [], None),
    uic!("timeline.search", "Search Timeline", [], None),
    uic!("flowchart.options", "Flowchart Options", [], None),
    uic!("flowchart.graph", "Flowchart Graph", [], None),
    uic!("timeline.collapseAll", "Collapse All", [], None),
    uic!("timeline.twirlSelected", "Twirl Selected Layers Open / Closed", [], Some("Cmd+`")),
    uic!("timeline.keyAt.anchor", "Add or Remove Anchor Point Keyframe", [], Some("Alt+Shift+A")),
    uic!("timeline.keyAt.position", "Add or Remove Position Keyframe", [], Some("Alt+Shift+P")),
    uic!("timeline.keyAt.scale", "Add or Remove Scale Keyframe", [], Some("Alt+Shift+S")),
    uic!("timeline.keyAt.rotation", "Add or Remove Rotation Keyframe", [], Some("Alt+Shift+R")),
    uic!("timeline.keyAt.opacity", "Add or Remove Opacity Keyframe", [], Some("Alt+Shift+T")),
    uic!("tool.selection", "Selection Tool", [], Some("V")),
    uic!("tool.hand", "Hand Tool", [], Some("H")),
    uic!("tool.zoom", "Zoom Tool", [], Some("Z")),
    uic!("tool.orbit", "Orbit Tool", [], Some("1")),
    uic!("tool.panCamera", "Pan Camera Tool", [], Some("2")),
    uic!("tool.dolly", "Dolly Tool", [], Some("3")),
    uic!("tool.camera", "Camera Tools", [], Some("C")),
    uic!("tool.rotate", "Rotation Tool", [], Some("W")),
    uic!("tool.panBehind", "Pan Behind Tool", [], Some("Y")),
    uic!("tool.shape", "Shape Tools", [], Some("Q")),
    uic!("tool.pen", "Pen Tool", [], Some("G")),
    uic!("tool.type", "Type Tools", [], Some("Cmd+T")),
    uic!("tool.brush", "Brush Tools", [], Some("Cmd+B")),
    uic!("tool.rotoBrush", "Roto Brush & Refine Edge Tools", [], Some("Alt+W")),
    uic!("tool.puppet", "Puppet Tools", [], Some("Cmd+P")),
    uic!("app.newComp", "New Composition...", [], None),
    uic!("app.compSettings", "Composition Settings...", [], None),
    uic!("app.solidSettings", "New Solid...", [], None),
    uic!("app.home", "Home", [], None),
    uic!("markers.dialog", "Marker Settings...", [], None),
    uic!("window.maximizePanel", "Maximize Panel Under Pointer", [], Some("`")),
    uic!("window.maximizeApp", "Maximize or Restore Application Window", [], Some("Cmd+\\")),
    uic!("window.toggleCompTimeline", "Switch Between Composition and Timeline Panel", [], Some("\\")),
    uic!("window.nextPanel", "Next Panel in Frame", [], Some("Alt+Shift+.")),
    uic!("window.previousPanel", "Previous Panel in Frame", [], Some("Alt+Shift+,")),
    uic!("window.closeAll", "Close Panel or All Viewers", [], Some("Cmd+Shift+W")),
    uic!("window.dockPanel", "Dock Panel", [], None),
    uic!("window.floatPanel", "Undock Panel", [], None),
    uic!("window.closePanel", "Close Panel", [], None),
    uic!("window.panelWindow", "Move Panel to New Window", [], None),
];

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
    if no_params(&params) && app.ui.focused == PanelKind::Project {
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
        let selected = ctx.data(|d| d.get_temp::<(Vec<u64>, Option<u64>)>(egui::Id::new("rq-ui"))).and_then(|(_, id)| id);
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
    let now = ctx.input(|i| i.time);
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
    // Layer Settings on a solid, adjustment layer or null opens its settings dialog.
    if crate::panels::dialogs::route_layer_settings(app, id, &params)? {
        return Ok(Value::Null);
    }
    // Layer ▸ Layer Styles ▸ <style> from the menu adds the style and opens the dialog on it.
    if crate::panels::layer_styles_dialog::route(app, id, &params)? {
        return Ok(Value::Null);
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
    if id == "window.dockAction" {
        let value = params.get("action").cloned().ok_or("missing docking action")?;
        let action: craft_ui::docking::Action<PanelKind> = serde_json::from_value(value).map_err(|error| format!("invalid docking action: {error}"))?;
        if matches!(&action, craft_ui::docking::Action::Open { .. } | craft_ui::docking::Action::OpenAt { .. }) {
            return Err("use the panel open commands to preserve viewer and ScriptUI lifecycle callbacks".into());
        }
        if let craft_ui::docking::Action::Close { panel } = &action {
            let mut layout = crate::dock_shared::encode(&app.ui.dock, &app.ui.floating)?;
            layout.apply(action.clone()).map_err(|error| error.to_string())?;
            app.close_panel(*panel);
            return Ok(Value::Null);
        }
        let mut error = None;
        if !app.edit_layout(|layout| match layout.apply(action) {
            Ok(()) => true,
            Err(reason) => {
                error = Some(reason);
                false
            }
        }) {
            return Err(error.unwrap_or_else(|| "invalid docking layout".into()));
        }
        return Ok(Value::Null);
    }
    // Multiple monitors: {panel, window?: bool} moves a panel into its own native window (or
    // back into the main window; no `window` toggles).
    if id == "window.panelWindow" {
        // A panel name ("Info", "Effect Controls") or its serialized form ({"Viewer": 2}).
        let value = params.get("panel").ok_or("panelWindow: need `panel`")?;
        let panel = match value.as_str() {
            Some(name) => PanelKind::from_name(name).ok_or(format!("unknown panel `{name}`"))?,
            None => serde_json::from_value::<PanelKind>(value.clone()).map_err(|_| "panelWindow: unknown `panel`".to_string())?,
        };
        let window = match params.get("window") {
            None | Some(Value::Null) => None,
            Some(value) => Some(value.as_bool().ok_or("panelWindow: `window` must be true or false")?),
        };
        let window = app.set_panel_window(ctx, panel, window)?;
        return Ok(json!({"window": window}));
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
                app.close_panel(p);
                return Ok(Value::Null);
            }
            "floatPanel" => {
                let p = panel_of("panel").ok_or("floatPanel: need `panel`")??;
                let r = match params.get("rect") {
                    Some(value) => serde_json::from_value::<[f32; 4]>(value.clone()).map_err(|_| "rect must contain four finite numbers")?,
                    None => crate::dock_ui::default_float_rect(ctx.content_rect()),
                };
                if !app.edit_layout(|l| l.float(p, r)) {
                    return Err(format!("can't undock {}", p.title()));
                }
                return Ok(Value::Null);
            }
            _ => {
                let p = panel_of("panel").ok_or("dockPanel: need `panel`")??;
                let anchor = panel_of("anchor").ok_or("dockPanel: need `anchor` (a docked panel)")??;
                let zone = params
                    .get("zone")
                    .and_then(Value::as_str)
                    .map_or(Some(crate::dock::Zone::Center), crate::dock::Zone::from_name)
                    .ok_or("zone: center|left|right|top|bottom")?;
                if !app.edit_layout(|l| l.dock(p, anchor, zone)) {
                    return Err(format!("can't dock {} at {} ({zone:?})", p.title(), anchor.title()));
                }
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
            let group: Vec<PanelKind> = match app.ui.floating.iter().find(|f| f.panels.contains(&focused)) {
                Some(f) => f.panels.clone(),
                None => app.ui.dock.group_of(focused).map(<[PanelKind]>::to_vec).unwrap_or_default(),
            };
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

/// Run an engine command and perform the frontend events it emitted right away.
/// Commands that fill the internal clipboard.
const COPY_COMMANDS: &[&str] =
    &["edit.copy", "edit.cut", "edit.copyWithPropertyLinks", "edit.copyWithRelativePropertyLinks", "edit.copyExpressionOnly", "keys.copy", "effect.copy"];

/// A line for the system clipboard naming what EffectCraft copied. The system clipboard must
/// hold something: with nothing on it, the windowing layer sends no paste event for Ctrl+V.
fn clipboard_note(s: &effectcraft_engine::Session) -> String {
    let st = &s.state;
    let n = |n: usize, one: &str, many: &str| format!("EffectCraft: {n} {}", if n == 1 { one } else { many });
    if st.clip_is_keys && !st.key_clipboard.is_empty() {
        n(st.key_clipboard.iter().map(|c| c.keys.len()).sum(), "keyframe", "keyframes")
    } else if !st.effect_clipboard.is_empty() {
        n(st.effect_clipboard.len(), "effect", "effects")
    } else if !st.contents_clipboard.is_empty() {
        n(st.contents_clipboard.len(), "shape item", "shape items")
    } else if st.link_clipboard.is_some() {
        "EffectCraft: property links".into()
    } else {
        n(st.clipboard.len(), "layer", "layers")
    }
}

pub(crate) fn run_engine(app: &mut EffectcraftApp, ctx: &egui::Context, id: &str, params: Value) -> Result<Value, String> {
    // Events already queued belong to earlier invocations (`engine.execute` defers them).
    let queued = app.session.events.len();
    let mut r = app.session.execute(id, params).map_err(|e| e.to_string());
    if let Err(e) = &r {
        app.ui.status = e.clone();
    } else if COPY_COMMANDS.contains(&id) && app.session.state.text_edit.is_none() {
        ctx.copy_text(clipboard_note(&app.session));
    }
    let events = app.session.drain_events();
    for (index, ev) in events.into_iter().enumerate() {
        match ev {
            effectcraft_engine::Event::Frontend { command, params } => {
                if let Err(e) = frontend(app, ctx, &command, params) {
                    app.ui.status = e.clone();
                    // A frontend-only command's own failure is the command's result (#696: a
                    // workspace save that didn't reach disk reported success). An earlier
                    // invocation's failure only sets the status.
                    if index >= queued && command == id && r.is_ok() {
                        r = Err(e);
                    }
                }
            }
            other => app.session.events.push(other),
        }
    }
    r
}

fn toggle(slot: &mut bool, p: &Value) -> Value {
    *slot = p.get("value").and_then(Value::as_bool).unwrap_or(!*slot);
    json!(*slot)
}

/// Perform a frontend command (from `Event::Frontend`).
pub fn frontend(app: &mut EffectcraftApp, ctx: &egui::Context, id: &str, p: Value) -> Result<Value, String> {
    let now = ctx.input(|i| i.time);
    let v = &mut app.ui.viewer;
    Ok(match id {
        "mediaBrowser.copySourcePath" => {
            let path = p.get("path").and_then(Value::as_str).filter(|path| !path.is_empty()).ok_or("A source path is required")?;
            ctx.copy_text(path.to_string());
            json!({"path": path})
        }
        "effects.copyPresetsFolderPath" => {
            if cfg!(target_arch = "wasm32") {
                return Err("The web app has no local presets folder".into());
            }
            let path = crate::panels::effects_presets::presets_dir().filter(|path| !path.as_os_str().is_empty()).ok_or("No presets folder is configured")?;
            let path = std::path::absolute(path).map_err(|e| format!("Cannot resolve the presets folder path: {e}"))?;
            let path = path.to_str().ok_or("The presets folder path is not valid UTF-8")?;
            ctx.copy_text(path.to_string());
            json!({"path": path})
        }
        "app.about" => {
            app.dialog = Some(crate::Dialog::About);
            Value::Null
        }
        // Window ▸ <ScriptUI panel>: dock (or bring forward) the panel the script built.
        // Audio scrubbing: one frame of audio at `time` (default the current time).
        "playback.scrubAudio" => {
            let cid = app.session.active_comp_id().ok_or("no composition")?;
            let t = p.get("time").and_then(Value::as_f64).map_or(app.session.time(), effectcraft_engine::time::Tick::from_seconds_f64);
            app.scrub_audio(cid, t, now);
            json!({"playing": app.scrub.is_some()})
        }
        // View ▸ New Viewer.
        "view.newViewer" => json!({"viewer": crate::panels::viewers::new_viewer(app)}),
        // Run by id (control channel, MCP): the Project panel's selection, whatever has focus.
        "project.deleteWithoutConfirmation" => crate::panels::delete_items::delete_confirmed(app, ctx, json!({}))?,
        // A gradient property's "Edit Gradient…" (the Timeline's value): the Gradient Editor.
        "prop.editGradient" => crate::panels::gradient_editor::open(app, &p)?,
        "window.scriptPanel" => {
            let id = p.get("window").and_then(Value::as_u64).ok_or("no ScriptUI panel window")? as u32;
            app.show_panel(PanelKind::ScriptPanel(id));
            json!({"window": id})
        }
        "layer.style.options" => {
            crate::panels::layer_styles_dialog::open(app, &p)?;
            Value::Null
        }
        "app.settings" => {
            let page = p.get("page").and_then(Value::as_str).unwrap_or("general");
            crate::panels::settings::open(app, effectcraft_engine::prefs::page_id(page).unwrap_or("general"));
            Value::Null
        }
        "app.gpuInfo" => {
            crate::panels::dialogs::info(
                app,
                "GPU Information",
                &format!(
                    "{}Layer cache: {} MB  •  Preview cache: {} MB.",
                    effectcraft_engine::sysinfo::gpu_summary(&app.session),
                    app.session.layer_cache.budget() >> 20,
                    app.frames.budget() >> 20
                ),
            );
            Value::Null
        }
        "app.hide" | "app.hideOthers" | "app.showAll" => {
            // The desktop app asks macOS (NSApplication hide / hideOtherApplications /
            // unhideAllApplications); elsewhere Hide minimizes and the others don't apply.
            if app.hooks.app_action.as_ref().is_some_and(|f| f(id)) {
                return Ok(Value::Null);
            }
            match id {
                "app.hide" => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
                _ => return Err(format!("`{id}` is only available on macOS")),
            }
            Value::Null
        }
        "app.quit" => {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            Value::Null
        }
        "app.commandPalette" => {
            app.dialog_state.palette_query = p.get("query").and_then(Value::as_str).unwrap_or_default().to_string();
            app.dialog_state.palette_sel = 0;
            app.dialog = Some(crate::Dialog::CommandPalette);
            Value::Null
        }
        "app.keyboardShortcuts" => {
            crate::panels::shortcut_editor::open(app);
            Value::Null
        }
        "app.templates" => {
            let kind = p.get("kind").and_then(Value::as_str).unwrap_or("renderSettings");
            crate::panels::rq_templates::open(app, ctx, kind);
            Value::Null
        }
        "renderQueue.notify" => {
            // Render finished with Notify on: a toast, the dock/taskbar attention request, and the
            // host's system sound (the desktop app handles `renderQueue.notify`).
            let msg = p.get("message").and_then(Value::as_str).unwrap_or("Render finished").to_string();
            app.toast = Some((msg, now));
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
            if let Some(f) = app.hooks.app_action.as_ref() {
                f("renderQueue.notify");
            }
            Value::Null
        }
        "app.find" => {
            if let Some(q) = p.get("query").and_then(Value::as_str) {
                app.ui.project_search = q.to_string();
            }
            app.show_panel(PanelKind::Project);
            Value::Null
        }
        "playback.toggle" => {
            // The play button and Spacebar: `{shortcut?}` picks which Preview panel shortcut's
            // options to play with (default Spacebar).
            let sc = match p.get("shortcut").and_then(Value::as_str) {
                Some(k) => effectcraft_engine::preview::PreviewShortcut::parse(k).ok_or_else(|| format!("unknown preview shortcut `{k}`"))?,
                None => effectcraft_engine::preview::PreviewShortcut::Spacebar,
            };
            app.toggle_play_with(now, sc);
            json!({"playing": app.playback.playing, "shortcut": sc.id()})
        }
        "playback.cacheWhenIdle" => {
            let r = toggle(&mut app.ui.cache_when_idle, &p);
            app.set_pref("previews.cacheFramesWhenIdle", r.clone())?;
            r
        }
        "playback.audio" => {
            // Include Audio of the shortcut shown in the Preview panel.
            let mut on = app.session.prefs.preview.active().include_audio;
            let r = toggle(&mut on, &p);
            app.session.execute("playback.settings.set", json!({"includeAudio": on})).map_err(|e| e.to_string())?;
            if !on {
                app.audio = None;
            }
            r
        }
        // Zoom In / Zoom Out act on the focused viewer: the Layer panel when it has focus, else
        // the Composition viewer.
        "view.zoomIn" | "view.zoomOut" if app.ui.focused == PanelKind::Layer => {
            let cur = app.ui.layer_zoom.unwrap_or_else(|| crate::panels::layer_panel::last_fit(ctx));
            // The Layer panel zooms in points (pixels-per-point 1).
            let next = zoom_step(cur, 1.0, id == "view.zoomIn");
            app.ui.layer_zoom = Some(next);
            json!({"zoom": next, "panel": "layer"})
        }
        "view.zoomIn" | "view.zoomOut" => {
            let ppp = ctx.pixels_per_point();
            let cur = v.zoom.unwrap_or_else(|| crate::panels::viewer::last_fit(ctx));
            let next = zoom_step(cur, ppp, id == "view.zoomIn");
            v.zoom = Some(next);
            json!({"zoom": next})
        }
        // View ▸ Resolution ▸ Custom without a factor (the engine sets the comp's resolution).
        "view.res.custom" => {
            let cur = match app.viewer_res() {
                Resolution::Custom(n) => f64::from(n),
                _ => 2.0,
            };
            crate::panels::dialogs::form(
                app,
                "Custom Resolution",
                "view.res.custom",
                json!({}),
                vec![crate::panels::dialogs::Field::num("factor", "Render every n-th pixel", cur)],
            );
            Value::Null
        }
        "view.rulers" => toggle(&mut v.rulers, &p),
        "view.guides" => toggle(&mut v.guides, &p),
        "view.snapToGuides" => toggle(&mut v.snap_guides, &p),
        "view.lockGuides" => toggle(&mut v.lock_guides, &p),
        "view.grid" => toggle(&mut v.grid, &p),
        "view.snapToGrid" => toggle(&mut v.snap_grid, &p),
        "view.layerControls" => toggle(&mut v.show_layer_controls, &p),
        "view.options" => {
            app.dialog = Some(crate::Dialog::ViewOptions);
            Value::Null
        }
        "view.panelBackground" => {
            if p.get("pick").and_then(Value::as_bool) == Some(true) {
                let [r, g, b] = v.custom_pasteboard;
                crate::panels::dialogs::form(
                    app,
                    "Panel Background Color",
                    "view.panelBackground",
                    json!({}),
                    vec![crate::panels::dialogs::Field::text("color", "Color (#rrggbb)", &format!("#{r:02x}{g:02x}{b:02x}"))],
                );
                return Ok(Value::Null);
            }
            let c = p.get("color").and_then(Value::as_str).unwrap_or("mediumGray");
            v.pasteboard = match c {
                "black" => Some([0, 0, 0]),
                "darkGray" => Some([0x2a, 0x2a, 0x2a]),
                "mediumGray" => None,
                "lightGray" => Some([0xb4, 0xb4, 0xb4]),
                "white" => Some([0xff, 0xff, 0xff]),
                "custom" => Some(v.custom_pasteboard),
                hex => {
                    let c = effectcraft_engine::color::Rgba::from_hex(hex).ok_or_else(|| format!("unknown colour `{hex}`"))?;
                    let rgb = [(c.r * 255.0).round() as u8, (c.g * 255.0).round() as u8, (c.b * 255.0).round() as u8];
                    v.custom_pasteboard = rgb;
                    Some(rgb)
                }
            };
            Value::Null
        }
        // Ctrl+\ / Cmd+\, as in After Effects (#336): `{maximized}`, the window's new state.
        "window.maximizeApp" => {
            let maximized = !ctx.input(|i| i.viewport().maximized.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(maximized));
            json!({"maximized": maximized})
        }
        "view.fullScreen" => {
            let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
            json!(!full)
        }
        "window.panel" => {
            let name = p.get("panel").and_then(Value::as_str).unwrap_or_default();
            if name.eq_ignore_ascii_case("tools") {
                app.ui.status = "The Tools panel is the toolbar in the header".into();
                return Ok(Value::Null);
            }
            let panel = PanelKind::from_name(name).ok_or_else(|| format!("unknown panel `{name}`"))?;
            app.show_panel(panel);
            Value::Null
        }
        "window.workspace" => {
            let want = p.get("name").and_then(Value::as_str).unwrap_or("Default").to_ascii_lowercase().replace(' ', "");
            let name = app
                .workspace_names()
                .into_iter()
                .find(|w| w.to_ascii_lowercase().replace(' ', "") == want)
                .ok_or_else(|| format!("unknown workspace `{want}`"))?;
            app.set_workspace(&name);
            json!({"workspace": name})
        }
        "window.resetWorkspace" => {
            let name = app.ui.workspace.clone();
            app.set_workspace(&name);
            Value::Null
        }
        "window.saveWorkspace" => {
            let name = app.ui.workspace.clone();
            app.ui.saved_workspaces.insert(name.clone(), app.ui.dock.clone());
            app.ui.saved_floating.insert(name.clone(), app.ui.floating.clone());
            app.store_saved_workspaces()?;
            json!({"workspace": name})
        }
        "window.saveWorkspaceAs" => {
            let Some(name) = p.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).map(str::to_string) else {
                crate::panels::dialogs::form(
                    app,
                    "New Workspace",
                    "window.saveWorkspaceAs",
                    json!({}),
                    vec![crate::panels::dialogs::Field::text("name", "Name", "Untitled Workspace")],
                );
                return Ok(Value::Null);
            };
            app.ui.saved_workspaces.insert(name.clone(), app.ui.dock.clone());
            app.ui.saved_floating.insert(name.clone(), app.ui.floating.clone());
            app.ui.workspace = name.clone();
            app.store_saved_workspaces()?;
            json!({"workspace": name})
        }
        "window.editWorkspaces" => {
            let Some(name) = p.get("name").and_then(Value::as_str).map(str::to_string) else {
                let cur = app.ui.workspace.clone();
                crate::panels::dialogs::form(
                    app,
                    "Edit Workspaces",
                    "window.editWorkspaces",
                    json!({}),
                    vec![
                        crate::panels::dialogs::Field::text("name", "Workspace", &cur),
                        crate::panels::dialogs::Field::text("rename", "Rename to", &cur),
                        crate::panels::dialogs::Field::bool("delete", "Delete", false),
                    ],
                );
                return Ok(Value::Null);
            };
            let builtin = crate::dock::WORKSPACES.contains(&name.as_str());
            if p.get("delete").and_then(Value::as_bool) == Some(true) {
                app.ui.saved_floating.remove(&name);
                if app.ui.saved_workspaces.remove(&name).is_none() && !builtin {
                    return Err(format!("no workspace `{name}`"));
                }
                // Leave the deleted workspace first: a current workspace needs a definition.
                if app.ui.workspace == name {
                    app.set_workspace("Default");
                }
                app.store_saved_workspaces()?;
                return Ok(json!({"deleted": name}));
            }
            if let Some(new) = p.get("rename").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty() && *n != name) {
                if builtin {
                    return Err(format!("`{name}` is a built-in workspace: save it as a new workspace instead"));
                }
                let dock = app.ui.saved_workspaces.remove(&name).ok_or_else(|| format!("no workspace `{name}`"))?;
                app.ui.saved_workspaces.insert(new.to_string(), dock);
                if let Some(f) = app.ui.saved_floating.remove(&name) {
                    app.ui.saved_floating.insert(new.to_string(), f);
                }
                if app.ui.workspace == name {
                    app.ui.workspace = new.to_string();
                }
                app.store_saved_workspaces()?;
                return Ok(json!({"renamed": new}));
            }
            json!({"workspaces": app.workspace_names()})
        }
        "view.lookAt" => {
            let r: Vec<f64> = p.get("rect").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
            if r.len() == 4 {
                let map: Option<crate::panels::viewer::ViewerMap> = ctx.data(|d| d.get_temp(egui::Id::new("viewer-map")));
                if let Some(m) = map {
                    let (rw, rh) = (((r[2] - r[0]) as f32).max(1.0), ((r[3] - r[1]) as f32).max(1.0));
                    let zoom = ((m.area.width() - 40.0) / rw).min((m.area.height() - 40.0) / rh).clamp(0.015, 32.0);
                    let rc = [(r[0] + r[2]) as f32 / 2.0, (r[1] + r[3]) as f32 / 2.0];
                    let v = &mut app.ui.viewer;
                    v.zoom = Some(zoom);
                    v.pan = [m.comp[0] * zoom / 2.0 - rc[0] * zoom, m.comp[1] * zoom / 2.0 - rc[1] * zoom];
                }
            }
            Value::Null
        }
        "comp.flowchart" => {
            // Chart the active comp; Layer ▸ Reveal ▸ Reveal Layer in Project Flowchart shows
            // layers and selects the layer's node.
            app.ui.flowchart.root = app.session.active_comp_id().map(|c| c.0);
            if let (Some(c), Some(l)) = (app.session.active_comp_id(), app.session.state.selected_layers.first()) {
                app.ui.flowchart.layers = true;
                app.ui.flowchart.selected = Some(format!("layer:{}:{}", c.0, l.0));
            }
            app.show_panel(PanelKind::Flowchart);
            Value::Null
        }
        "comp.miniFlowchart" => {
            let at = ctx.input(|i| i.pointer.hover_pos()).unwrap_or_else(|| ctx.content_rect().center());
            app.ui.mini_flowchart = Some([at.x, at.y]);
            Value::Null
        }
        "track.editTargetDialog" => {
            crate::panels::tracker::open_target(app)?;
            Value::Null
        }
        "track.optionsDialog" => {
            crate::panels::tracker::open_options(app)?;
            Value::Null
        }
        "layer.openLayer" => {
            let id = p.get("layer").and_then(Value::as_u64).or_else(|| app.session.state.selected_layers.first().map(|l| l.0));
            // Another layer opens at Fit.
            if id != app.ui.layer_panel {
                app.ui.layer_zoom = None;
                app.ui.layer_pan = [0.0, 0.0];
            }
            app.ui.layer_panel = id;
            app.ui.layer_view = None;
            app.show_panel(PanelKind::Layer);
            Value::Null
        }
        "app.confirmExecute" => {
            // Settings ▸ Scripting ▸ Warn User When Executing Files.
            let path = p.get("path").and_then(Value::as_str).unwrap_or_default().to_string();
            crate::panels::forms::form(
                app,
                "A script wants to open a file",
                "file.executeFile",
                json!({"path": path, "confirmed": true}),
                vec![crate::panels::forms::Field::text("path", "Open", &path)],
            );
            Value::Null
        }
        "app.showReport" => {
            let title = p.get("title").and_then(Value::as_str).unwrap_or("Report").to_string();
            let text = p.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
            crate::panels::dialogs::info(app, &title, &text);
            Value::Null
        }
        "window.assignWorkspaceShortcut" => {
            let mut q = p.clone();
            q["workspace"] = json!(app.ui.workspace);
            return run_engine(app, ctx, "window.assignWorkspaceShortcut", q);
        }
        "file.interpretFootage" => {
            // Import ▸ Interpret Unlabeled Alpha As: Ask User.
            if let Some(i) = p.get("item").and_then(Value::as_u64) {
                app.session.state.project_selection = vec![effectcraft_engine::project::ItemId(i)];
            }
            crate::panels::dialogs::open_form(app, "file.interpretFootage", &json!({}));
            Value::Null
        }
        // Help ▸ In-App Tutorials: the Home screen's Learn tab.
        "help.inAppTutorials" => {
            app.ui.start_screen = true;
            app.ui.home_learn = true;
            app.ui.home_templates = false;
            Value::Null
        }
        // File ▸ New ▸ New Project from Template…: the Home screen's Templates tab.
        "file.newFromTemplate" => {
            app.ui.start_screen = true;
            app.ui.home_learn = false;
            app.ui.home_templates = true;
            Value::Null
        }
        "effect.manage" | "anim.browsePresets" => {
            app.show_panel(PanelKind::EffectsPresets);
            Value::Null
        }
        "timeline.revealProps" => {
            let props = p.get("props").and_then(Value::as_array).cloned().unwrap_or_default();
            let uids = |k: &str| -> Vec<u64> { props.iter().filter_map(|x| x.get(k).and_then(Value::as_u64)).collect() };
            let (found, with_props) = (uids("prop"), uids("layer"));
            // The layers it looked at (`layers`), else those it found something on. Layers that
            // have nothing to show close; the others' reveals stay.
            let targets: Vec<u64> = match p.get("layers").and_then(Value::as_array) {
                Some(l) => l.iter().filter_map(Value::as_u64).collect(),
                None => with_props.clone(),
            };
            // The targets' earlier picks go (U after UU shows the keyframed ones only).
            let comp = app.session.active_comp_arc();
            let theirs = |u: u64| {
                comp.as_ref().is_some_and(|c| {
                    targets
                        .iter()
                        .filter_map(|l| c.layer(effectcraft_engine::project::LayerId(*l)))
                        .any(|l| l.props.find(u).is_some() || l.props.find_group(u).is_some())
                })
            };
            let tl = &mut app.ui.timeline;
            tl.reveal_props.retain(|u| !theirs(*u));
            tl.reveal_props.extend(found.iter().copied());
            let (shown, hidden): (Vec<u64>, Vec<u64>) = targets.into_iter().partition(|l| with_props.contains(l));
            tl.apply_reveal(&hidden, vec![]);
            if !shown.is_empty() {
                tl.apply_reveal(&shown, vec!["props".into()]);
            }
            // Properties inside effects (puppet pins…) show under their effect, twirled open.
            for (layer, prop) in props.iter().filter_map(|x| Some((x.get("layer")?.as_u64()?, x.get("prop")?.as_u64()?))) {
                crate::panels::timeline::open_effect_paths(app, layer, &[prop]);
            }
            json!({"revealed": found.len()})
        }
        _ => return Err(format!("`{id}` is not a frontend command")),
    })
}

/// Commands that need a file or folder path: ask the host's file dialog when `params` lacks one.
fn file_dialog(app: &mut EffectcraftApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    enum Ask {
        Folder,
        Import,
        OpenProject,
        Save(&'static str),
        Open(&'static [&'static str]),
    }
    let (key, ask) = match id {
        "file.import" | "file.importMultiple" => ("paths", Ask::Import),
        "file.open" => ("path", Ask::OpenProject),
        "file.save" if app.session.path.is_none() => ("path", Ask::Save("Untitled Project.ecproj")),
        "file.saveAs" | "file.saveCopy" => ("path", Ask::Save("Untitled Project.ecproj")),
        "comp.saveFrameAs" => ("path", Ask::Save("Frame.png")),
        "comp.saveFrameAsPsd" => ("path", Ask::Save("Frame.psd")),
        "comp.saveFrameAsExr" => ("path", Ask::Save("Frame.exr")),
        "file.watchFolder" if params.get("stop").is_none() => ("folder", Ask::Save("Watch Folder")),
        "anim.savePreset" => ("path", Ask::Save("Preset.ecpreset")),
        "anim.applyPreset" => ("path", Ask::Open(&["ecpreset", "json"])),
        "view.exportGuides" => ("path", Ask::Save("Guides.json")),
        "view.importGuides" => ("path", Ask::Open(&["json"])),
        "file.exportLottie" => ("path", Ask::Save("Animation.json")),
        "file.importLottie" => ("path", Ask::Open(&["json", "lottie"])),
        "file.importProject" => ("path", Ask::Open(&["ecproj", "ecprojx"])),
        "file.importTimeline" => ("path", Ask::Open(&["xml", "fcpxml", "otio", "edl", "aaf", "omf"])),
        "render.saveCurrentPreview" => ("path", Ask::Save("Preview.mp4")),
        "file.runScript" if params.get("name").is_none() => ("path", Ask::Open(&["jsx", "js", "jsonl", "json", "txt"])),
        "file.installScript" | "file.installScriptUIPanel" => ("path", Ask::Open(&["jsx", "js"])),
        "effect.plugins.load" if params.get("folder").is_none() => ("path", Ask::Open(&["wasm", "wat"])),
        "file.replaceFootage" => ("path", Ask::Import),
        "file.relinkFootage" => ("folder", Ask::Folder),
        "file.collectFiles" => ("folder", Ask::Save("Collected Files")),
        "file.saveCopyAsXml" => ("path", Ask::Save("Untitled Project.ecprojx")),
        "keys.rpfCameraImport" => ("path", Ask::Open(&["json", "csv", "txt"])),
        "essential.exportTemplate" => ("path", Ask::Save("Template.ectemplate")),
        "essential.importTemplate" => ("path", Ask::Open(&["ectemplate"])),
        "file.setProxy" => {
            ("path", Ask::Open(&["mp4", "mov", "m4v", "mkv", "webm", "mxf", "png", "jpg", "jpeg", "gif", "webp", "tif", "tiff", "bmp", "tga", "exr"]))
        }
        _ => return None,
    };
    if params.get(key).is_some()
        || params.get("path").is_some()
        || params.get("paths").is_some()
        || params.get("steps").is_some()
        || params.get("preset").is_some()
    {
        return None;
    }
    const MEDIA: &[&str] = &[
        "mp4", "mov", "m4v", "mkv", "webm", "mxf", "png", "jpg", "jpeg", "gif", "webp", "tif", "tiff", "bmp", "exr", "dpx", "cin", "wav", "aif", "aiff", "mp3",
        "flac", "ogg", "opus", "svg", "pdf", "ai", "eps", "psd", "psb", "tga", "hdr", "gltf", "glb", "obj", "stl", "fbx", "json", "csv", "tsv",
    ];
    // A host whose blocking dialogs are unsafe inside this frame shows the dialog without
    // blocking — macOS: the nested event loop re-entered winit's handler on a drag over the
    // window and aborted the app (#511); Linux: the desktop portal's answer never gets pumped,
    // so the compositor marks the window as not responding (#1275) — and the command continues
    // when the answer arrives.
    if let Some(spawn) = app.hooks.file_dialog_async.as_ref() {
        let request = match ask {
            Ask::Folder => crate::FileDialog::Folder,
            Ask::Import => crate::FileDialog::Open { exts: MEDIA.iter().map(|e| e.to_string()).collect(), many: key == "paths" },
            Ask::OpenProject => crate::FileDialog::OpenProject,
            Ask::Open(exts) => crate::FileDialog::Open { exts: exts.iter().map(|e| e.to_string()).collect(), many: false },
            Ask::Save(default) => crate::FileDialog::save(default),
        };
        let tx = app.dialog_answers.0.clone();
        let answer = crate::DialogAnswer { command: id.to_string(), key, params: params.clone(), paths: Vec::new() };
        spawn(
            request,
            Box::new(move |paths| {
                let _ = tx.send(crate::DialogAnswer { paths, ..answer });
            }),
        );
        return Some(Ok(json!({"dialog": id})));
    }
    let picked: Option<Value> = match ask {
        Ask::Folder => {
            let Some(f) = app.hooks.pick_folder.as_ref() else { return Some(Err("no folder dialog available (pass `folder`)".into())) };
            f().map(Value::from)
        }
        Ask::Import => {
            let Some(f) = app.hooks.pick_files.as_ref() else { return Some(Err("no file dialog available (pass `paths`)".into())) };
            picked_value(key, f(MEDIA))
        }
        Ask::OpenProject => {
            let Some(f) = app.hooks.pick_open_project.as_ref() else { return Some(Err("no file dialog available (pass `path`)".into())) };
            f().map(|s| json!(s))
        }
        Ask::Open(exts) => {
            let Some(f) = app.hooks.pick_files.as_ref() else { return Some(Err("no file dialog available (pass `path`)".into())) };
            f(exts).first().map(|s| json!(s))
        }
        Ask::Save(default) => {
            let Some(picked) = app.hooks.save_dialog(default) else { return Some(Err("no file dialog available (pass `path`)".into())) };
            picked.map(|s| json!(s))
        }
    };
    let Some(v) = picked else { return Some(Ok(Value::Null)) };
    Some(run_with_picked(app, id, key, params, v))
}

/// The dialog's answer as the command's `key` parameter (`None`: cancelled).
fn picked_value(key: &str, paths: Vec<String>) -> Option<Value> {
    match (paths.is_empty(), key) {
        (true, _) => None,
        (false, "paths") => Some(json!(paths)),
        (false, _) => paths.into_iter().next().map(Value::from),
    }
}

/// A file dialog shown without blocking ([`crate::Hooks::file_dialog_async`]) answered: run its
/// command with the picked path (nothing when it was cancelled).
pub(crate) fn dialog_answered(app: &mut EffectcraftApp, answer: crate::DialogAnswer) {
    let Some(v) = picked_value(answer.key, answer.paths) else { return };
    if let Err(e) = run_with_picked(app, &answer.command, answer.key, &answer.params, v) {
        app.ui.status = e;
    }
}

/// Run command `id` with the path picked for it in parameter `key`.
fn run_with_picked(app: &mut EffectcraftApp, id: &str, key: &str, params: &Value, v: Value) -> Result<Value, String> {
    let mut p = params.as_object().cloned().unwrap_or_default();
    p.insert(key.to_string(), v);
    // Picked files import in the background, with the Importing card showing progress (#270).
    if matches!(id, "file.import" | "file.importMultiple") {
        p.insert("background".into(), Value::Bool(true));
    }
    // Photoshop files ask how to import them first; numbered stills whether to import them as an
    // image sequence.
    let picked = Value::Object(p.clone());
    if id == "file.import" && crate::panels::dialogs::open_form(app, id, &picked)
        || matches!(id, "file.import" | "file.importMultiple") && crate::panels::forms::open_import_sequence(app, &picked)
    {
        return Ok(json!({"dialog": id}));
    }
    // Save (untitled) becomes Save As.
    let id = if id == "file.save" { "file.saveAs" } else { id };
    let r = app.session.execute(id, Value::Object(p)).map_err(|e| e.to_string());
    if let Err(e) = &r {
        app.ui.status = e.clone();
    }
    r
}

/// A menu entry for display / `ui.menu.list`.
#[derive(Clone, Debug, serde::Serialize)]
pub struct MenuItem {
    pub id: String,
    pub label: String,
    pub path: Vec<String>,
    pub shortcut: Option<String>,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked: Option<bool>,
    /// Parameters bound to the entry (e.g. the effect for Effect menu items).
    #[serde(skip_serializing_if = "Value::is_null")]
    pub params: Value,
}

/// Top-level menus in order.
pub fn menus() -> Vec<&'static str> {
    effectcraft_engine::menus::top_level()
}

pub(crate) fn entry_label(app: &EffectcraftApp, e: &MenuEntry) -> String {
    let shown = match e.command.as_str() {
        "edit.undo" => app.session.history.undo.last().map(|u| format!("Undo {}", u.0)).unwrap_or_else(|| "Can't Undo".into()),
        "edit.redo" => app.session.history.redo.last().map(|u| format!("Redo {}", u.0)).unwrap_or_else(|| "Can't Redo".into()),
        // Window ▸ Layer: the layer open in the Layer panel.
        "window.panel" if e.params.get("panel").and_then(Value::as_str) == Some("layer") => {
            let name = app
                .ui
                .layer_panel
                .and_then(|id| app.session.active_comp().and_then(|c| c.layer(effectcraft_engine::project::LayerId(id))))
                .map(|l| l.name.clone());
            format!("Layer: {}", name.unwrap_or_else(|| "(none)".into()))
        }
        _ => effectcraft_engine::menus::entry_label(&app.session, e),
    };
    crate::i18n::entry(app, e, shown)
}

/// The entry's shortcut in the active keyboard shortcut preset.
pub(crate) fn entry_shortcut(app: &EffectcraftApp, e: &MenuEntry) -> Option<String> {
    app.session.shortcuts().shortcut_of(&e.command, &e.params).map(str::to_string)
}

/// Whether panel `p` is on screen: the front tab of a floating window, or of a docked group
/// (an open frame in a stack) that a maximized panel does not cover.
pub(crate) fn panel_shown(app: &EffectcraftApp, p: PanelKind) -> bool {
    if app.ui.floating.iter().any(|f| f.panels.get(f.active) == Some(&p)) {
        return true;
    }
    match app.ui.maximized.filter(|m| app.ui.dock.contains(*m)) {
        Some(m) => m == p,
        None => app.ui.dock.is_visible(p),
    }
}

/// The command a menu entry runs when chosen from a menu: Window ▸ <panel> closes a panel whose
/// entry is checked, as in After Effects (#1630). Shortcuts and agents calling `window.panel`
/// still only show the panel.
pub(crate) fn menu_choice(app: &EffectcraftApp, id: String, params: Value) -> (String, Value) {
    if id == "window.panel"
        && let Some(p) = params.get("panel").and_then(Value::as_str).and_then(PanelKind::from_name)
        && panel_shown(app, p)
    {
        return ("window.closePanel".into(), params);
    }
    (id, params)
}

/// Check-mark state of frontend toggles (engine state is answered by the engine).
pub(crate) fn entry_checked(app: &EffectcraftApp, e: &MenuEntry) -> Option<bool> {
    let v = &app.ui.viewer;
    let pstr = |k: &str| e.params.get(k).and_then(Value::as_str);
    match e.command.as_str() {
        "view.rulers" => Some(v.rulers),
        "view.guides" => Some(v.guides),
        "view.snapToGuides" => Some(v.snap_guides),
        "view.lockGuides" => Some(v.lock_guides),
        "view.grid" => Some(v.grid),
        "view.snapToGrid" => Some(v.snap_grid),
        "view.layerControls" => Some(v.show_layer_controls),
        "playback.cacheWhenIdle" => Some(app.ui.cache_when_idle),
        "playback.audio" => Some(app.session.prefs.preview.active().include_audio),
        "window.workspace" => Some(pstr("name").is_some_and(|n| n == app.ui.workspace)),
        // The Tools panel is the header toolbar: always on screen.
        "window.panel" => Some(pstr("panel").is_some_and(|n| n.eq_ignore_ascii_case("tools") || PanelKind::from_name(n).is_some_and(|p| panel_shown(app, p)))),
        "view.panelBackground" if e.params.get("pick").is_none() => {
            let cur = match v.pasteboard {
                None => "mediumGray",
                Some([0, 0, 0]) => "black",
                Some([0x2a, 0x2a, 0x2a]) => "darkGray",
                Some([0xb4, 0xb4, 0xb4]) => "lightGray",
                Some([0xff, 0xff, 0xff]) => "white",
                Some(_) => "custom",
            };
            Some(pstr("color") == Some(cur))
        }
        _ => effectcraft_engine::menus::checked(&app.session, &e.command, &e.params),
    }
}

pub(crate) fn entry_enabled(app: &EffectcraftApp, e: &MenuEntry) -> bool {
    if app.ui.focused == PanelKind::Project {
        match e.command.as_str() {
            "edit.duplicate" => {
                return !app.session.state.project_selection.is_empty()
                    && app.session.state.project_selection.iter().all(|id| app.session.project.item(*id).is_some_and(|i| !i.is_folder()));
            }
            "edit.selectAll" => return !app.session.project.items.is_empty(),
            "edit.deselectAll" => return !app.session.state.project_selection.is_empty(),
            "edit.cut" | "edit.copy" | "edit.paste" | "edit.copyWithPropertyLinks" | "edit.copyWithRelativePropertyLinks" | "edit.copyExpressionOnly" => {
                return false;
            }
            _ => {}
        }
    }
    if e.command == "edit.clear" && app.ui.focused == PanelKind::Project {
        return app.session.is_enabled("project.delete");
    }
    if e.command == "edit.clear" && app.ui.focused == PanelKind::RenderQueue {
        return !app.session.is_rendering() && !app.session.project.render_queue.is_empty();
    }
    if e.command == "window.panel" {
        return true;
    }
    app.session.is_enabled(&e.command)
}

/// Every menu entry, flattened with its submenu path.
pub fn menu_items(app: &EffectcraftApp) -> Vec<MenuItem> {
    effectcraft_engine::menus::entries()
        .into_iter()
        .map(|(path, e)| MenuItem {
            id: e.command.clone(),
            label: entry_label(app, e),
            path,
            shortcut: entry_shortcut(app, e),
            enabled: entry_enabled(app, e),
            checked: entry_checked(app, e),
            params: e.params.clone(),
        })
        .collect()
}

/// Shortcut text in this OS's notation (`⇧⌘K` on macOS, `Ctrl+Shift+K` elsewhere).
pub fn shortcut_text(s: &str) -> String {
    if cfg!(target_os = "macos") {
        let mut out = String::new();
        let parts: Vec<&str> = match s.strip_suffix("++") {
            Some(head) => vec![head, "+"],
            None => s.split('+').collect(),
        };
        let (mods, key) = parts.split_at(parts.len().saturating_sub(1));
        for m in mods {
            out.push_str(match *m {
                "Ctrl" => "⌃",
                "Alt" => "⌥",
                "Shift" => "⇧",
                "Cmd" => "⌘",
                x => x,
            });
        }
        out.push_str(match key.first().copied().unwrap_or("") {
            "Space" => "Space",
            "Escape" => "Esc",
            "Home" => "↖",
            k => k,
        });
        out
    } else {
        s.replace("Cmd", "Ctrl")
    }
}

/// Parse "Cmd+Shift+K" into modifiers + key.
pub fn parse_shortcut(s: &str) -> Option<(egui::Modifiers, egui::Key)> {
    let mut m = egui::Modifiers::NONE;
    let mut key = None;
    // The plus key itself is written `+` or `Cmd++`.
    let (head, plus) = match s.strip_suffix("++") {
        Some(h) => (h, true),
        None if s == "+" => ("", true),
        None => (s, false),
    };
    let mut parts: Vec<&str> = if head.is_empty() { vec![] } else { head.split('+').collect() };
    if plus {
        parts.push("+");
    }
    for p in parts {
        match p {
            "Cmd" => {
                m.command = true;
                m.mac_cmd = cfg!(target_os = "macos");
                if !cfg!(target_os = "macos") {
                    m.ctrl = true;
                }
            }
            "Physical" => {}
            "Shift" => m.shift = true,
            "Alt" => m.alt = true,
            "Ctrl" => m.ctrl = true,
            k => {
                key = match k {
                    ";" => Some(egui::Key::Semicolon),
                    "'" => Some(egui::Key::Quote),
                    "," => Some(egui::Key::Comma),
                    "." => Some(egui::Key::Period),
                    "/" => Some(egui::Key::Slash),
                    "\\" => Some(egui::Key::Backslash),
                    "=" => Some(egui::Key::Equals),
                    "-" => Some(egui::Key::Minus),
                    "`" => Some(egui::Key::Backtick),
                    "[" => Some(egui::Key::OpenBracket),
                    "]" => Some(egui::Key::CloseBracket),
                    "Num0" => Some(egui::Key::Num0),
                    "Num*" => None,
                    _ => egui::Key::from_name(k),
                }
            }
        }
    }
    key.map(|k| (m, k))
}

/// The key some keyboards report for a shifted punctuation key (`Shift+=` arrives as `+`).
fn shifted_alias(k: egui::Key) -> Option<egui::Key> {
    use egui::Key::*;
    Some(match k {
        Equals => Plus,
        Semicolon => Colon,
        OpenBracket => OpenCurlyBracket,
        CloseBracket => CloseCurlyBracket,
        Slash => Questionmark,
        Backslash => Pipe,
        _ => return None,
    })
}

/// A key binding: modifiers, key, command id and bound params.
pub type Binding = (egui::Modifiers, egui::Key, String, Value);

/// All bindings of the active keyboard shortcut preset, most modifiers first.
pub fn bindings(session: &effectcraft_engine::Session) -> Vec<Binding> {
    let mut v: Vec<Binding> = Vec::new();
    for (sc, b) in session.shortcuts().bindings() {
        if sc.starts_with("Physical+") {
            continue;
        }
        // Off macOS `Cmd` is Ctrl, so a custom Ctrl+Cmd shortcut would
        // steal the plain Cmd one: such macOS-only shortcuts aren't bound there.
        if !cfg!(target_os = "macos") && sc.split('+').any(|p| p == "Ctrl") && sc.split('+').any(|p| p == "Cmd") {
            continue;
        }
        if let Some((m, k)) = parse_shortcut(sc) {
            if m.shift
                && let Some(alias) = shifted_alias(k)
            {
                v.push((m, alias, b.command.clone(), b.params.clone()));
            }
            v.push((m, k, b.command.clone(), b.params.clone()));
        }
    }
    v.sort_by_key(|(m, ..)| std::cmp::Reverse(m.command as u8 + m.shift as u8 + m.alt as u8 + m.ctrl as u8));
    v
}

fn mods_match(want: egui::Modifiers, got: egui::Modifiers) -> bool {
    want.command == got.command && want.shift == got.shift && want.alt == got.alt && (want.ctrl == got.ctrl || got.command && cfg!(not(target_os = "macos")))
}

/// Set (egui temp data) from a plain Spacebar press until its release while a tap would still
/// run the Spacebar shortcut.
/// The system clipboard's text of this frame's paste event, for the Edit ▸ Paste it runs.
fn system_paste_id() -> egui::Id {
    egui::Id::new("menus-system-paste")
}

fn space_tap_id() -> egui::Id {
    egui::Id::new("spacebar-tap")
}

/// The modifiers held when this frame's input began: the state the last pass ended with, if it
/// ran this (egui keeps only the state after the frame's events). Records this pass's state.
fn modifiers_at_start(ctx: &egui::Context) -> Option<egui::Modifiers> {
    let id = egui::Id::new("shortcut-modifiers");
    let pass = ctx.cumulative_pass_nr();
    let now = ctx.input(|i| i.modifiers);
    ctx.data_mut(|d| {
        let last = d.get_temp::<(u64, egui::Modifiers, Option<egui::Modifiers>)>(id);
        if let Some((p, _, start)) = last
            && p == pass
        {
            return start;
        }
        let start = last.and_then(|(p, m, _)| (p.checked_add(1) == Some(pass)).then_some(m));
        d.insert_temp(id, (pass, now, start));
        start
    })
}

/// Only Shift held (Shift+Delete, not Ctrl/Alt+Shift+Delete).
fn only_shift(m: egui::Modifiers) -> bool {
    m.shift && !m.alt && !m.ctrl && !m.command && !m.mac_cmd
}

/// Dispatch keyboard shortcuts (skipped while typing in a text field).
pub fn handle_shortcuts(app: &mut EffectcraftApp, ctx: &egui::Context) {
    handle_shortcuts_impl(app, ctx, false);
}

/// Tab can acquire widget focus while the current frame is drawn. Dispatch its application
/// binding only afterward, and only when no widget owned focus before or after layout.
pub fn handle_tab_shortcut(app: &mut EffectcraftApp, ctx: &egui::Context) {
    if ctx.input(|i| i.key_pressed(egui::Key::Tab)) {
        handle_shortcuts_impl(app, ctx, true);
    }
}

fn handle_shortcuts_impl(app: &mut EffectcraftApp, ctx: &egui::Context, tab_only: bool) {
    let start = modifiers_at_start(ctx);
    // Dialog cancellation owns Escape even when a text field has keyboard focus.
    if !tab_only && app.dialog.is_some() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        // A popup or colour picker takes Escape first (the picker goes back to its original colour).
        if crate::panels::shortcut_editor::recording(app)
            || crate::panels::shortcut_editor::confirming(app)
            || egui::Popup::is_any_open(ctx)
            || crate::color_picker::open_recently(ctx)
        {
            return;
        }
        ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if app.dialog == Some(crate::Dialog::LayerStyles) {
            crate::panels::layer_styles_dialog::finish(app, false);
        } else if app.dialog == Some(crate::Dialog::GradientEditor) {
            // Escape is Cancel: the preview rolls back.
            crate::panels::gradient_editor::finish(app, false, ctx);
        } else if app.dialog == Some(crate::Dialog::Info) {
            crate::panels::forms::close_info(app);
        } else {
            if app.dialog == Some(crate::Dialog::Settings)
                && let Some(p) = app.dialog_state.prefs_snapshot.take()
            {
                app.session.prefs = p;
                app.session.prefs_changed();
            }
            if app.dialog == Some(crate::Dialog::UnsavedChanges) {
                app.dialog_state.unsaved.command = None;
            }
            // Escape is Cancel: a Preview that edited the project rolls back.
            crate::panels::live_preview::cancel(app);
            app.dialog = None;
        }
        return;
    }
    // A mouse button down while Spacebar is held belongs to the Hand tool: its release won't
    // preview.
    if ctx.input(|i| i.pointer.any_down()) || ctx.egui_wants_keyboard_input() {
        ctx.data_mut(|d| d.remove::<bool>(space_tap_id()));
    }
    // While a menu or popup list is open, keys go to it, not to the app (Space must not start a
    // preview behind the menu, #279).
    if ctx.egui_wants_keyboard_input() || crate::widgets::any_menu_open(ctx) {
        return;
    }
    // Text editing in the viewer takes the clipboard events itself.
    let clipboard = app.session.state.text_edit.is_none();
    let project = app.ui.focused == PanelKind::Project;
    // The paste event's text: Edit ▸ Paste pastes SVG markup as a shape layer.
    let pasted =
        if clipboard { ctx.input(|i| i.events.iter().rev().find_map(|e| if let egui::Event::Paste(t) = e { Some(t.clone()) } else { None })) } else { None };
    let mut events: Vec<(egui::Key, egui::Modifiers, bool, Option<egui::Key>)> = ctx.input(|i| {
        // The windowing layer turns Ctrl+C / Ctrl+X / Ctrl+V into clipboard events instead of
        // key presses: map them back to the keys (with the modifiers held) so Edit ▸ Copy, Cut,
        // Paste and their variants (Ctrl+Alt+C…) run. Clipboard events carry no modifiers and
        // `i.modifiers` is the state after all of the frame's events, so each is read with the
        // modifiers where it is in the stream: walk the events from the frame's starting state.
        // `None` when that isn't known.
        let changed = i.events.iter().any(|e| matches!(e, egui::Event::ModifiersChanged(_) | egui::Event::WindowFocused(false)));
        let mut at = if changed { start } else { Some(i.modifiers) };
        let held = |at: Option<egui::Modifiers>| at.filter(|m| m.command).unwrap_or(egui::Modifiers::COMMAND);
        let mut out = Vec::new();
        for e in &i.events {
            match e {
                egui::Event::ModifiersChanged(m) => at = Some(*m),
                // egui forgets the modifiers held when the window loses focus.
                egui::Event::WindowFocused(false) => at = Some(egui::Modifiers::NONE),
                egui::Event::Key { key, physical_key, pressed: true, modifiers, repeat, .. }
                    if !*repeat
                        || matches!(
                            key,
                            egui::Key::PageUp | egui::Key::PageDown | egui::Key::ArrowLeft | egui::Key::ArrowRight | egui::Key::ArrowUp | egui::Key::ArrowDown
                        ) =>
                {
                    out.push((*key, *modifiers, true, *physical_key));
                }
                egui::Event::Key { key: egui::Key::Space, pressed: false, .. } => out.push((egui::Key::Space, egui::Modifiers::NONE, false, None)),
                egui::Event::Copy if clipboard => out.push((egui::Key::C, held(at), true, None)),
                // On Windows, Shift+Delete is the old Cut key and arrives as Cut too. The Project
                // panel has no clipboard: there a Cut with only Shift held is Shift+Delete (Delete
                // Project Items Without Confirmation). Never when the modifiers aren't known.
                egui::Event::Cut if clipboard && project && at.is_some_and(only_shift) => {
                    out.push((egui::Key::Delete, egui::Modifiers::SHIFT, true, None));
                }
                egui::Event::Cut if clipboard => out.push((egui::Key::X, held(at), true, None)),
                egui::Event::Paste(_) if clipboard => out.push((egui::Key::V, held(at), true, None)),
                _ => {}
            }
        }
        out
    });
    events.retain(|(key, _, _, _)| (*key == egui::Key::Tab) == tab_only);
    // Numpad * (or `*` typed with Shift+8): Add Marker (#275). egui has no key for it, so it
    // comes as text.
    if !tab_only && clipboard && app.dialog.is_none() && ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(t) if t == "*"))) {
        let bound = app.session.shortcuts().bindings().into_iter().find(|(k, _)| *k == "Num*").map(|(_, b)| (b.command.clone(), b.params.clone()));
        // As in After Effects, it marks the selected layers; only with none selected does it add
        // a composition marker (#976).
        let bound = bound.map(|(id, params)| {
            if id == "comp.addMarker" && !app.session.state.selected_layers.is_empty() { ("layer.addMarker".to_owned(), params) } else { (id, params) }
        });
        if let Some((id, params)) = bound
            && let Err(e) = invoke(app, ctx, &id, if params.is_null() { json!({}) } else { params })
        {
            app.ui.status = e;
        }
    }
    if events.is_empty() {
        return;
    }
    let binds = bindings(&app.session);
    for (key, mods, pressed, physical_key) in events {
        // Plain Spacebar runs its shortcut (Play Current Preview) on the release: held, it is
        // the Hand tool (After Effects), and a drag with it must not start or stop playback.
        if key == egui::Key::Space && (!pressed || !mods.any()) {
            if pressed {
                if app.dialog.is_none() {
                    ctx.data_mut(|d| d.insert_temp(space_tap_id(), true));
                }
                continue;
            }
            if !ctx.data_mut(|d| d.remove_temp::<bool>(space_tap_id())).unwrap_or(false) {
                continue;
            }
        }
        // Escape closes dialogs.
        if key == egui::Key::Escape && app.dialog.is_some() {
            // Escape while recording a shortcut cancels the recording, not the editor; so does
            // Escape in its Delete Preset prompt.
            if crate::panels::shortcut_editor::recording(app) || crate::panels::shortcut_editor::confirming(app) {
                continue;
            }
            if app.dialog == Some(crate::Dialog::Info) {
                crate::panels::forms::close_info(app);
                continue;
            }
            if app.dialog == Some(crate::Dialog::GradientEditor) {
                crate::panels::gradient_editor::finish(app, false, ctx);
                continue;
            }
            // Escape in Settings is Cancel: restore the settings from when it opened.
            if app.dialog == Some(crate::Dialog::Settings)
                && let Some(p) = app.dialog_state.prefs_snapshot.take()
            {
                app.session.prefs = p;
                app.session.prefs_changed();
            }
            crate::panels::live_preview::cancel(app);
            app.dialog = None;
            continue;
        }
        if app.dialog.is_some() {
            continue;
        }
        // Enter in the Project panel renames the selected item (handled by the panel).
        if key == egui::Key::Enter && !mods.any() && app.ui.focused == PanelKind::Project {
            continue;
        }
        // The Project panel has its own selection; a previously selected Timeline layer
        // must not be deleted while the Project panel has focus.
        if matches!(key, egui::Key::Delete | egui::Key::Backspace) && !mods.any() {
            if matches!(app.ui.focused, PanelKind::Composition | PanelKind::Viewer(_)) && !app.session.state.camera_points.is_empty() {
                if let Err(e) = run_engine(app, ctx, "camera.deletePoints", json!({})) {
                    app.ui.status = e;
                }
                ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key));
                continue;
            }
            // The queue panel consumes the event after its selection state is drawn.
            if app.ui.focused == PanelKind::RenderQueue {
                continue;
            }
            let _ = invoke(app, ctx, "edit.clear", json!({}));
            continue;
        }
        // Explicit physical bindings distinguish keys that a layout reports with the same
        // logical name (for example AZERTY's minus and right parenthesis).
        let physical = physical_key.and_then(|key| {
            app.session.shortcuts().bindings().into_iter().find_map(|(sc, b)| {
                let (m, k) = parse_shortcut(sc.strip_prefix("Physical+")?)?;
                (k == key && mods_match(m, mods)).then(|| (b.command.clone(), b.params.clone()))
            })
        });
        if let Some((id, params)) = physical {
            if let Err(e) = invoke(app, ctx, &id, if params.is_null() { json!({}) } else { params }) {
                app.ui.status = e;
            }
            continue;
        }
        if let Some((_, _, id, params)) = binds.iter().filter(|(m, k, ..)| *k == key && mods_match(*m, mods)).min_by_key(|(_, _, id, params)| {
            // The focused panel wins over an application command sharing its chord.
            let key = effectcraft_engine::shortcuts::binding_key(id, params);
            app.session.shortcuts().find(&key).is_none_or(|b| b.scope != app.ui.focused.id())
        }) {
            let (id, params) = (id.clone(), if params.is_null() { json!({}) } else { params.clone() });
            if id == "edit.paste"
                && let Some(t) = &pasted
            {
                ctx.data_mut(|d| d.insert_temp(system_paste_id(), t.clone()));
            }
            if let Err(e) = invoke(app, ctx, &id, params) {
                app.ui.status = e;
            }
            ctx.data_mut(|d| d.remove_temp::<String>(system_paste_id()));
        }
    }
}

/// Draw the in-window menu bar (the engine's AE menu tree).
pub fn menu_bar(app: &mut EffectcraftApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let mut clicked: Option<(String, Value)> = None;
    let mut nav = crate::menu_keys::Nav::default();
    ui.horizontal_centered(|ui| {
        ui.add_space(6.0);
        egui::MenuBar::new().config(egui::containers::menu::MenuConfig::new().style(crate::widgets::menu_style)).ui(ui, |ui| {
            for node in effectcraft_engine::menus::menu_bar() {
                if let MenuNode::Submenu { label, children } = node {
                    let r = ui.menu_button(crate::i18n::label(app, "", label), |ui| {
                        ui.set_min_width(if label == "Effect" { 200.0 } else { 280.0 });
                        crate::widgets::menu_scroll(ui, |ui| menu_nodes(app, ui, children, &mut clicked, &mut nav, 0));
                    });
                    nav.top(&r.response);
                    app.auto.add(&format!("menu.{label}"), r.response.rect, label);
                }
            }
        });
    });
    crate::menu_keys::handle(&ctx, &nav);
    if let Some((id, params)) = clicked.map(|(id, params)| menu_choice(app, id, params))
        && let Err(e) = invoke(app, &ctx, &id, if params.is_null() { json!({}) } else { params })
    {
        app.ui.status = e;
    }
}

/// The entries of top-level menu `name` (the Effect menu is the Effect Controls panel's context
/// menu); returns the chosen command and its params.
pub(crate) fn menu_contents(app: &mut EffectcraftApp, ui: &mut egui::Ui, name: &str) -> Option<(String, Value)> {
    let mut clicked = None;
    if let Some(MenuNode::Submenu { children, .. }) =
        effectcraft_engine::menus::menu_bar().iter().find(|n| matches!(n, MenuNode::Submenu { label, .. } if label == name))
    {
        menu_nodes(app, ui, children, &mut clicked, &mut crate::menu_keys::Nav::default(), 0);
    }
    clicked.map(|(id, params)| (id, if params.is_null() { json!({}) } else { params }))
}

/// The entries `nodes` of a menu `depth` submenus deep, registered with `nav` for the keyboard.
fn menu_nodes(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    nodes: &[MenuNode],
    clicked: &mut Option<(String, Value)>,
    nav: &mut crate::menu_keys::Nav,
    depth: usize,
) {
    for n in nodes {
        match n {
            MenuNode::Separator => {
                ui.separator();
            }
            MenuNode::Dynamic { name } => {
                // Recent projects / footage / presets, undo history, shortcut slots, viewers,
                // saved workspaces.
                let ws = app.ui.workspace.clone();
                let saved = app.saved_workspace_names();
                let (entries, empty) = effectcraft_engine::menus::dynamic(&app.session, name, &dyn_ctx(&ws, &saved));
                if entries.is_empty()
                    && let Some(e) = empty
                {
                    ui.add_enabled(false, egui::Button::new((gutter(false), e)));
                }
                for (i, e) in entries.iter().enumerate() {
                    let tip = match name.as_str() {
                        "recentProjects" => app.session.prefs.recent_projects.get(i).cloned(),
                        "recentFootage" => app.session.prefs.recent_footage.get(i).cloned(),
                        "recentPresets" => app.session.prefs.recent_presets.get(i).cloned(),
                        _ => None,
                    };
                    let r = ui.add_enabled(entry_enabled(app, e), egui::Button::new((gutter(entry_checked(app, e) == Some(true)), e.label.as_str())));
                    let r = match &tip {
                        Some(t) => r.on_hover_text(t),
                        None => r,
                    };
                    app.auto.add(&format!("menu.{name}.{i}"), r.rect, &e.label);
                    nav.entry(ui, depth, &r, false);
                    if r.clicked() {
                        *clicked = Some((e.command.clone(), e.params.clone()));
                        ui.close();
                    }
                }
            }
            MenuNode::Submenu { label, children } => {
                let ws = app.ui.workspace.clone();
                // The generated Effect menu's category submenus draw the catalog's name (#821);
                // every other submenu goes through the menu catalog.
                let shown = crate::i18n::effect_category(children, label)
                    .unwrap_or_else(|| crate::i18n::submenu(app, label, effectcraft_engine::menus::submenu_label(&app.session, label, &dyn_ctx(&ws, &[]))));
                let r = ui.menu_button((gutter(false), shown.as_str()), |ui| {
                    ui.set_min_width(if children.len() > 30 { 200.0 } else { 240.0 });
                    crate::widgets::menu_scroll(ui, |ui| menu_nodes(app, ui, children, clicked, nav, depth + 1));
                });
                nav.entry(ui, depth, &r.response, true);
                if r.inner.is_some() {
                    nav.opened(depth, &r.response);
                }
            }
            MenuNode::Item(e) => {
                let r = menu_entry(app, ui, e);
                nav.entry(ui, depth, &r, false);
                if r.clicked() {
                    *clicked = Some((e.command.clone(), e.params.clone()));
                    ui.close();
                }
            }
        }
    }
}

/// The menu bar's submenu at `path` (`["File", "Interpret Footage"]`) in a panel's context menu,
/// with the menu bar's labels, shortcuts and enabled state; a chosen entry goes to `clicked`.
pub(crate) fn submenu_at(app: &mut EffectcraftApp, ui: &mut egui::Ui, path: &[&str], clicked: &mut Option<(String, Value)>) {
    let mut nodes = effectcraft_engine::menus::menu_bar();
    let mut found = None;
    for label in path {
        let Some(n) = nodes.iter().find(|n| matches!(n, MenuNode::Submenu { label: l, .. } if l == label)) else { return };
        if let MenuNode::Submenu { children, .. } = n {
            nodes = children;
        }
        found = Some(n);
    }
    if let Some(n) = found {
        menu_nodes(app, ui, std::slice::from_ref(n), clicked, &mut crate::menu_keys::Nav::default(), 0);
    }
    if let Some((_, p)) = clicked.as_mut()
        && p.is_null()
    {
        *p = json!({});
    }
}

/// The menu bar's entry for `command` in a panel's context menu (see [`submenu_at`]).
pub(crate) fn entry_for(app: &EffectcraftApp, ui: &mut egui::Ui, command: &str, clicked: &mut Option<(String, Value)>) {
    if let Some((_, e)) = effectcraft_engine::menus::entries().into_iter().find(|(_, e)| e.command == command)
        && menu_entry(app, ui, e).clicked()
    {
        *clicked = Some((e.command.clone(), e.params_or_empty()));
        ui.close();
    }
}

/// Frontend state for dynamic menus (the current workspace and the saved ones).
pub(crate) fn dyn_ctx<'a>(workspace: &'a str, saved_workspaces: &'a [String]) -> effectcraft_engine::menus::DynCtx<'a> {
    effectcraft_engine::menus::DynCtx { workspace: Some(workspace), saved_workspaces }
}

/// The check-mark column every menu row reserves (like macOS / After Effects menus).
fn gutter(checked: bool) -> egui::Atom<'static> {
    use egui::AtomExt;
    (if checked { "✔" } else { "" }).atom_size(egui::vec2(14.0, 14.0))
}

fn menu_entry(app: &EffectcraftApp, ui: &mut egui::Ui, e: &MenuEntry) -> egui::Response {
    let (label, check, enabled) = (entry_label(app, e), gutter(entry_checked(app, e) == Some(true)), entry_enabled(app, e));
    // Edit ▸ Label's entries show the label's colour before its name.
    if e.command == "edit.label"
        && let Some(l) = e.params.get("label").and_then(Value::as_str).and_then(effectcraft_engine::color::Label::from_name)
    {
        let out = ui.add_enabled_ui(enabled, |ui| egui::Button::new((check, crate::widgets::label_swatch(), label)).atom_ui(ui)).inner;
        crate::widgets::paint_label_swatch(ui, &out, l, &app.tokens);
        return out.response;
    }
    let mut b = egui::Button::new((check, label));
    if let Some(s) = entry_shortcut(app, e) {
        b = b.shortcut_text(shortcut_text(&s));
    }
    ui.add_enabled(enabled, b)
}

/// View ▸ Zoom In / Zoom Out: the next standard magnification (as a share of screen pixels, so
/// 100 % is one comp pixel per screen pixel at any display scaling) above or below `cur`, which is
/// in points per comp pixel like the result (#882).
fn zoom_step(cur: f32, ppp: f32, zoom_in: bool) -> f32 {
    const STEPS: [f32; 16] = [0.015, 0.03, 0.0625, 0.125, 0.25, 0.333, 0.5, 0.66, 1.0, 1.5, 2.0, 3.0, 4.0, 8.0, 16.0, 32.0];
    let ppp = if ppp.is_finite() && ppp > 0.0 { ppp } else { 1.0 };
    let shown = cur * ppp;
    let next = if zoom_in {
        STEPS.iter().copied().find(|s| *s > shown + 1e-3).unwrap_or(32.0)
    } else {
        STEPS.iter().rev().copied().find(|s| *s < shown - 1e-3).unwrap_or(0.015)
    };
    next / ppp
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #882: at 125 % / 150 % display scaling Zoom In reaches 100 % (one comp pixel per screen
    /// pixel), not 100 % in points (125 % / 150 % on screen).
    #[test]
    fn zoom_steps_land_on_whole_screen_magnifications() {
        for ppp in [1.0f32, 1.25, 1.5, 2.0] {
            let at_100 = zoom_step(0.7 / ppp, ppp, true);
            assert!((at_100 * ppp - 1.0).abs() < 1e-6, "ppp {ppp}: {}", at_100 * ppp);
            assert!((zoom_step(at_100, ppp, true) * ppp - 1.5).abs() < 1e-6);
            assert!((zoom_step(at_100, ppp, false) * ppp - 0.66).abs() < 1e-6);
        }
        assert!(zoom_step(1.0, 0.0, true).is_finite());
    }

    #[test]
    fn shortcut_passes_share_frame_start_modifiers() {
        let ctx = egui::Context::default();
        let mut starts = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput { events: vec![egui::Event::ModifiersChanged(egui::Modifiers::SHIFT)], ..Default::default() }, |ui| {
            let ctx = ui.ctx();
            starts.push(modifiers_at_start(ctx));
            starts.push(modifiers_at_start(ctx));
        });
        output.textures_delta.clear();
        assert_eq!(starts, vec![None, None]);
        starts.clear();
        let mut output = ctx.run_ui(egui::RawInput { events: vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE)], ..Default::default() }, |ui| {
            let ctx = ui.ctx();
            starts.push(modifiers_at_start(ctx));
            starts.push(modifiers_at_start(ctx));
        });
        output.textures_delta.clear();
        assert_eq!(starts, vec![Some(egui::Modifiers::SHIFT), Some(egui::Modifiers::SHIFT)]);
    }

    #[test]
    fn every_ui_shortcut_parses() {
        for c in UI_COMMANDS {
            if let Some(sc) = c.shortcut {
                assert!(parse_shortcut(sc).is_some() || sc == "Num*", "{sc}");
            }
        }
        for (_, e) in effectcraft_engine::menus::entries() {
            if let Some(sc) = &e.shortcut {
                assert!(parse_shortcut(sc).is_some() || sc == "Num*", "{} ({sc})", e.label);
            }
        }
    }

    #[test]
    fn full_screen_default_is_platform_specific() {
        let app = EffectcraftApp::new(effectcraft_engine::Session::default());
        let full_screen = app.session.shortcuts().shortcut_of("view.fullScreen", &Value::Null);
        if cfg!(target_os = "macos") {
            assert_eq!(full_screen, Some("Ctrl+Cmd+F"));
        } else {
            assert_eq!(full_screen, None);
            assert_eq!(app.session.shortcuts().shortcut_of("window.maximizeApp", &Value::Null), Some("Cmd+\\"));
        }
    }

    #[test]
    fn ui_and_menu_shortcuts_do_not_collide() {
        let app = EffectcraftApp::new(effectcraft_engine::Session::default());
        let mut seen: std::collections::BTreeMap<(String, String), (String, String)> = Default::default();
        for (m, k, id, params) in bindings(&app.session) {
            let key = (format!("{m:?}"), format!("{k:?}"));
            let target = (id.clone(), params.to_string());
            if let Some(prev) = seen.get(&key) {
                assert_eq!(prev, &target, "{key:?} bound to {prev:?} and {target:?}");
            } else {
                seen.insert(key, target);
            }
        }
    }

    /// #336: Ctrl+\ (Cmd+\ on macOS) maximizes the application window, and again restores
    /// it, as in After Effects.
    #[test]
    fn ctrl_backslash_maximizes_or_restores_the_window() {
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        let (m, k) = parse_shortcut("Cmd+\\").unwrap();
        assert!(m.command && k == egui::Key::Backslash, "{m:?} {k:?}");
        assert!(bindings(&app.session).iter().any(|b| (b.0, b.1) == (m, k) && b.2 == "window.maximizeApp"));
        let ctx = egui::Context::default();
        let run = |app: &mut EffectcraftApp, maximized: bool| {
            let input = egui::RawInput {
                viewports: std::iter::once((egui::ViewportId::ROOT, egui::ViewportInfo { maximized: Some(maximized), ..Default::default() })).collect(),
                ..Default::default()
            };
            let mut r = Value::Null;
            let mut out = ctx.run_ui(input, |ui| r = invoke(app, ui.ctx(), "window.maximizeApp", json!({})).unwrap());
            out.textures_delta.clear();
            let cmds = out.viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.commands.clone()).unwrap_or_default();
            (r, cmds)
        };
        let (r, cmds) = run(&mut app, false);
        assert_eq!(r, json!({"maximized": true}));
        assert!(cmds.contains(&egui::ViewportCommand::Maximized(true)), "{cmds:?}");
        let (r, cmds) = run(&mut app, true);
        assert_eq!(r, json!({"maximized": false}));
        assert!(cmds.contains(&egui::ViewportCommand::Maximized(false)), "{cmds:?}");
    }

    /// The default keys of the keyboard-only commands parse and reach them.
    #[test]
    fn keyboard_only_commands_are_bound() {
        let app = EffectcraftApp::new(effectcraft_engine::Session::default());
        for (id, keys) in [
            ("time.workAreaStart", "Shift+Home"),
            ("time.workAreaEnd", "Shift+End"),
            ("time.previousInOut", "Cmd+Alt+Shift+ArrowLeft"),
            ("time.nextInOut", "Cmd+Alt+Shift+ArrowRight"),
            ("comp.workAreaToSelection", "Cmd+Alt+B"),
            ("comp.activateRecent", "Shift+Escape"),
            ("keys.deselectAll", "Shift+F2"),
            ("keys.deselectAll", "Cmd+Alt+Shift+A"),
            ("layer.extendSelectionNext", "Cmd+Shift+ArrowDown"),
            ("layer.moveInToStart", "Alt+Home"),
            ("layer.moveEarlier", "Alt+PageUp"),
            ("layer.moveLater10", "Alt+Shift+PageDown"),
            ("layer.stretchInToTime", "Cmd+Shift+,"),
            ("layer.stretchOutToTime", "Cmd+Alt+,"),
            ("layer.mask.selectNext", "Alt+`"),
            ("layer.mask.selectPrevious", "Alt+Shift+`"),
            ("layer.castsShadows", "Alt+Shift+C"),
            ("timeline.scrollToTime", "D"),
            ("timeline.scrollToLayer", "X"),
            ("timeline.parentColumn", "Shift+F4"),
            ("view.fitUpTo100", "Alt+/"),
            ("view.nextItem", "Shift+."),
            ("view.previousItem", "Shift+,"),
            ("window.toggleCompTimeline", "\\"),
            ("window.nextPanel", "Alt+Shift+."),
            ("window.previousPanel", "Alt+Shift+,"),
            ("window.closeAll", "Cmd+Shift+W"),
            ("tool.camera", "C"),
        ] {
            assert!(bindings(&app.session).into_iter().any(|x| x.2 == id && Some((x.0, x.1)) == parse_shortcut(keys)), "{id} on {keys}");
        }
    }

    #[test]
    fn timeline_keys_scroll_and_toggle_the_parent_column() {
        let ctx = egui::Context::default();
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        let run = |app: &mut EffectcraftApp, id: &str| invoke(app, &ctx, id, json!({})).unwrap();
        // D and X leave a scroll for the Timeline to do when it is drawn.
        run(&mut app, "timeline.scrollToTime");
        assert_eq!(app.ui.timeline.scroll_to, Some(crate::state::ScrollTo::Time));
        run(&mut app, "timeline.scrollToLayer");
        assert_eq!(app.ui.timeline.scroll_to, Some(crate::state::ScrollTo::Layer));
        // Shift+F4 shows and hides the Parent column.
        let parent = crate::panels::timeline::column_visible(&app.ui.timeline, "parent");
        run(&mut app, "timeline.parentColumn");
        assert_eq!(crate::panels::timeline::column_visible(&app.ui.timeline, "parent"), !parent);
        run(&mut app, "timeline.parentColumn");
        assert_eq!(crate::panels::timeline::column_visible(&app.ui.timeline, "parent"), parent);
    }

    #[test]
    fn viewer_keys_fit_and_step_through_open_comps() {
        let ctx = egui::Context::default();
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        let run = |app: &mut EffectcraftApp, id: &str| invoke(app, &ctx, id, json!({})).unwrap();
        let new = |app: &mut EffectcraftApp, name: &str| app.session.execute("comp.new", json!({"name": name, "duration": 4})).unwrap()["comp"].as_u64();
        let active = |app: &EffectcraftApp| app.session.state.active_comp.map(|c| c.0);
        // With one composition open there is nothing to step to.
        let a = new(&mut app, "A");
        run(&mut app, "view.nextItem");
        assert_eq!(active(&app), a);
        // Shift+, / Shift+.: through the open compositions, wrapping.
        let b = new(&mut app, "B");
        assert_eq!(active(&app), b);
        run(&mut app, "view.nextItem");
        assert_eq!(active(&app), a);
        run(&mut app, "view.previousItem");
        assert_eq!(active(&app), b);
        // Alt+/ fits the viewer without magnifying past 100 %; Shift+/ fits fully again.
        app.ui.viewer.zoom = Some(3.0);
        run(&mut app, "view.fitUpTo100");
        assert!(app.ui.viewer.zoom.is_none() && app.ui.viewer.fit_up_to_100);
        run(&mut app, "view.fit");
        assert!(!app.ui.viewer.fit_up_to_100);
    }

    #[test]
    fn panel_keys_switch_cycle_and_close() {
        let ctx = egui::Context::default();
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        let run = |app: &mut EffectcraftApp, id: &str| invoke(app, &ctx, id, json!({})).unwrap();
        for name in ["A", "B"] {
            app.session.execute("comp.new", json!({"name": name, "duration": 4})).unwrap();
        }
        // \\ switches between the Timeline and the Composition viewer.
        app.ui.focused = PanelKind::Timeline;
        run(&mut app, "window.toggleCompTimeline");
        assert_eq!(app.ui.focused, PanelKind::Composition);
        run(&mut app, "window.toggleCompTimeline");
        assert_eq!(app.ui.focused, PanelKind::Timeline);
        // Alt+Shift+. / Alt+Shift+,: the tabs of the focused panel's group, wrapping.
        let group = app.ui.dock.group_of(PanelKind::Timeline).unwrap().to_vec();
        assert!(group.len() > 1, "{group:?}");
        run(&mut app, "window.nextPanel");
        assert_eq!(app.ui.focused, group[(group.iter().position(|p| *p == PanelKind::Timeline).unwrap() + 1) % group.len()]);
        run(&mut app, "window.previousPanel");
        assert_eq!(app.ui.focused, PanelKind::Timeline);
        // Ctrl+Shift+W with the Timeline active closes every composition.
        run(&mut app, "window.closeAll");
        assert!(app.session.state.open_comps.is_empty());
        // With another panel active it closes that panel.
        app.show_panel(PanelKind::Project);
        run(&mut app, "window.closeAll");
        assert!(!app.ui.dock.contains(PanelKind::Project));
    }

    #[test]
    fn c_cycles_the_camera_tools() {
        let ctx = egui::Context::default();
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        app.ui.tool = Tool::Selection;
        let mut seen = vec![];
        for _ in 0..4 {
            invoke(&mut app, &ctx, "tool.camera", json!({})).unwrap();
            seen.push(app.ui.tool);
        }
        assert_eq!(seen, [Tool::Orbit, Tool::PanCamera, Tool::Dolly, Tool::Orbit]);
    }

    #[test]
    fn dispatch_uses_the_active_custom_preset() {
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        let find = |app: &EffectcraftApp, id: &str| bindings(&app.session).into_iter().filter(|b| b.2 == id).map(|b| (b.0, b.1)).collect::<Vec<_>>();
        let (m, k) = parse_shortcut("Cmd+Alt+Shift+Y").unwrap();
        assert!(find(&app, "layer.newNull").contains(&(m, k)));
        // A UI command rebound in a custom preset.
        app.session.execute("shortcuts.set", json!({"command": "tool.hand", "keys": "Ctrl+Alt+H"})).unwrap();
        assert_eq!(app.session.keymaps.active, "Custom");
        let (m2, k2) = parse_shortcut("Ctrl+Alt+H").unwrap();
        assert_eq!(find(&app, "tool.hand"), vec![(m2, k2)]);
        app.session.execute("shortcuts.set", json!({"command": "layer.newNull", "keys": "F6"})).unwrap();
        assert_eq!(find(&app, "layer.newNull"), vec![(egui::Modifiers::NONE, egui::Key::F6)]);
        // The menus show the preset's shortcut.
        let item = menu_items(&app).into_iter().find(|i| i.id == "layer.newNull").unwrap();
        assert_eq!(item.shortcut.as_deref(), Some("F6"));
        // Back to the default preset.
        app.session.execute("shortcuts.preset", json!({"op": "select", "name": effectcraft_engine::shortcuts::DEFAULT_PRESET})).unwrap();
        assert!(find(&app, "layer.newNull").contains(&(m, k)));
    }

    #[test]
    fn renamed_label_shows_in_the_label_menu() {
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        app.set_pref("labels.1.name", json!("Sunflower")).unwrap();
        let labels: Vec<String> = menu_items(&app).into_iter().filter(|i| i.id == "edit.label").map(|i| i.label).collect();
        assert!(labels.contains(&"Sunflower".to_string()), "{labels:?}");
        assert!(!labels.contains(&"Yellow".to_string()));
    }

    /// Saving anything but a project asks the save dialog for that file's extension (#293): the
    /// project dialog, filtered to `.ecproj`, made macOS append `.ecproj` (`Frame.png.ecproj`).
    /// Hosts with only the project dialog (the web) still get it, given the file name.
    #[test]
    fn save_dialogs_offer_the_file_extension() {
        use std::cell::RefCell;
        use std::rc::Rc;
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        app.session.execute("comp.new", json!({"name": "A", "width": 64, "height": 36, "frameRate": 30, "duration": 1})).unwrap();
        let ctx = egui::Context::default();
        let asked: Rc<RefCell<Vec<String>>> = Rc::default();
        let (a, b) = (asked.clone(), asked.clone());
        app.hooks.pick_save = Some(Box::new(move |name: &str| {
            a.borrow_mut().push(format!("project {name}"));
            None
        }));
        app.hooks.pick_save_file = Some(Box::new(move |name: &str, ext: &str| {
            b.borrow_mut().push(format!("file {name} [{ext}]"));
            None
        }));
        for id in ["comp.saveFrameAs", "render.saveCurrentPreview", "file.exportLottie", "file.saveCopyAsXml", "file.collectFiles", "file.saveAs"] {
            invoke(&mut app, &ctx, id, json!({})).unwrap();
        }
        assert_eq!(
            *asked.borrow(),
            [
                "file Frame.png [png]",
                "file Preview.mp4 [mp4]",
                "file Animation.json [json]",
                "file Untitled Project.ecprojx [ecprojx]",
                "file Collected Files []",
                "project Untitled Project.ecproj",
            ]
        );
        // Without the other dialog, the project one names any file.
        app.hooks.pick_save_file = None;
        asked.borrow_mut().clear();
        invoke(&mut app, &ctx, "comp.saveFrameAs", json!({})).unwrap();
        assert_eq!(*asked.borrow(), ["project Frame.png"]);
    }

    /// #511: with a host's non-blocking dialogs, a command's file dialog opens without running
    /// a nested event loop inside the frame (a drag over the window then aborted the app on
    /// macOS); the command runs on a later frame with the answer, and a cancelled one not at all.
    #[test]
    fn file_dialogs_can_answer_on_a_later_frame() {
        use std::sync::{Arc, Mutex};
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        app.session.execute("comp.new", json!({"name": "A", "width": 64, "height": 36, "frameRate": 30, "duration": 1})).unwrap();
        let ctx = egui::Context::default();
        app.hooks.pick_save = Some(Box::new(|_: &str| panic!("the blocking dialog is not used")));
        let pending: Arc<Mutex<Vec<(crate::FileDialog, crate::DialogDone)>>> = Arc::default();
        let queue = pending.clone();
        app.hooks.file_dialog_async = Some(Box::new(move |dialog, done| queue.lock().unwrap().push((dialog, done))));
        assert_eq!(invoke(&mut app, &ctx, "file.saveAs", json!({})).unwrap(), json!({"dialog": "file.saveAs"}));
        let (dialog, done) = pending.lock().unwrap().pop().expect("a dialog was shown");
        assert_eq!(dialog, crate::FileDialog::Save { default: "Untitled Project.ecproj".into(), ext: "ecproj".into(), project: true });
        assert!(app.session.path.is_none(), "nothing saved until the dialog answers");
        let dir = std::env::temp_dir().join(format!("ec-dialog-answer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Answered.ecproj");
        // Answered from another thread, as the host's dialog does.
        std::thread::spawn(move || done(vec![path.to_string_lossy().to_string()])).join().unwrap();
        app.run_dialog_answers();
        assert_eq!(app.session.path.as_deref(), Some(dir.join("Answered.ecproj").to_string_lossy().as_ref()));
        assert!(dir.join("Answered.ecproj").is_file());
        // Cancelled: nothing runs.
        invoke(&mut app, &ctx, "comp.saveFrameAs", json!({})).unwrap();
        let (dialog, done) = pending.lock().unwrap().pop().expect("a dialog was shown");
        assert_eq!(dialog, crate::FileDialog::Save { default: "Frame.png".into(), ext: "png".into(), project: false });
        done(Vec::new());
        app.run_dialog_answers();
        assert!(!dir.join("Frame.png").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tga_is_available_in_import_replace_and_proxy_dialogs() {
        use std::sync::{Arc, Mutex};
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        let ctx = egui::Context::default();
        let pending = Arc::new(Mutex::new(Vec::new()));
        let queue = pending.clone();
        app.hooks.file_dialog_async = Some(Box::new(move |dialog, _done| queue.lock().unwrap().push(dialog)));
        for command in ["file.import", "file.importMultiple", "file.replaceFootage", "file.setProxy"] {
            assert_eq!(invoke(&mut app, &ctx, command, json!({})).unwrap(), json!({"dialog": command}));
            let crate::FileDialog::Open { exts, .. } = pending.lock().unwrap().pop().unwrap() else { panic!("not an open dialog") };
            assert!(exts.iter().any(|ext| ext == "tga"), "{command}: {exts:?}");
        }
    }

    #[test]
    fn menu_items_cover_the_tree() {
        assert_eq!(menus().last(), Some(&"Help"));
        assert!(effectcraft_engine::menus::entries().len() > 500);
    }
}
