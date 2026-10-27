use super::*;

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
    let now = app.now();
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
        "app.checkForUpdates" | "help.checkForUpdates" => {
            crate::updates_ui::check(app);
            Value::Null
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
        // Reveal Layer Source / Reveal Composition: show the Project panel on the item, opening
        // its folders and clearing the search so the row is visible.
        "project.reveal" => {
            let ids: Vec<effectcraft_engine::project::ItemId> = p
                .get("items")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_u64).map(effectcraft_engine::project::ItemId).collect())
                .or_else(|| p.get("item").and_then(Value::as_u64).map(|item| vec![effectcraft_engine::project::ItemId(item)]))
                .unwrap_or_default();
            crate::panels::project::reveal(app, ctx, &ids);
            json!({"items": ids.iter().map(|item| item.0).collect::<Vec<_>>()})
        }
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
                let map: Option<crate::panels::viewer::ViewerMap> = ctx.data(|d| d.get_temp(egui::Id::unique("viewer-map")));
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
pub(super) fn file_dialog(app: &mut EffectcraftApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
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
        app.file_dialogs.opened();
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

/// View ▸ Zoom In / Zoom Out: the next standard magnification (as a share of screen pixels, so
/// 100 % is one comp pixel per screen pixel at any display scaling) above or below `cur`, which is
/// in points per comp pixel like the result (#882).
pub(super) fn zoom_step(cur: f32, ppp: f32, zoom_in: bool) -> f32 {
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
