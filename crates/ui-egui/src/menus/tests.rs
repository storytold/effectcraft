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
    let group = app.ui.dock.group_of(PanelKind::Timeline);
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
    assert!(!app.ui.dock.is_open(PanelKind::Project));
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

/// File ▸ Scripts ▸ Recent Script Files and its key run a script again without a file dialog.
#[test]
fn recent_script_files_run_without_a_dialog() {
    let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
    let ctx = egui::Context::default();
    let dir = std::env::temp_dir().join(format!("ec-recent-script-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("New Comp.jsonl");
    std::fs::write(&script, r#"{"command": "comp.new", "params": {"name": "From Script", "width": 64, "height": 36, "frameRate": 30, "duration": 1}}"#)
        .unwrap();
    app.session.execute("file.runScript", json!({"path": script.to_string_lossy()})).unwrap();
    let (m, k, id, params) =
        bindings(&app.session).into_iter().find(|b| b.2 == "file.runScript" && b.3 == json!({"recent": 0})).expect("the most recent script has a key");
    assert_eq!((m, k), parse_shortcut("Cmd+Alt+Shift+D").unwrap());
    assert_eq!(invoke(&mut app, &ctx, &id, params).unwrap()["steps"], 1);
    assert_eq!(app.session.project.items.values().filter(|i| i.name.starts_with("From Script")).count(), 2);
    let _ = std::fs::remove_dir_all(&dir);
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

/// While a command's OS file dialog is up, floating panels' OS windows are blocked, as for
/// a modal dialog; a dialog that ends, answered or not, unblocks them, and Save As opens a
/// dialog again. A macOS sheet can end without answering (its window went away): once one of
/// the app's windows has the keyboard again, or keeps it as if no dialog showed, floating
/// windows are not left blocked.
#[test]
fn file_dialogs_block_floating_windows_only_while_up() {
    use std::sync::{Arc, Mutex};
    let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
    let ctx = egui::Context::default();
    let pending: Arc<Mutex<Vec<crate::DialogDone>>> = Arc::default();
    let queue = pending.clone();
    app.hooks.file_dialog_async = Some(Box::new(move |_dialog, done| queue.lock().unwrap().push(done)));
    let blocked = |app: &mut EffectcraftApp, focused: bool, time: f64| {
        let input = egui::RawInput { focused, time: Some(time), ..Default::default() };
        let mut out = false;
        ctx.run_ui(input, |ui| out = app.floats_blocked(ui.ctx())).textures_delta.clear();
        out
    };
    assert!(!blocked(&mut app, true, 0.0));
    // Cancelled.
    invoke(&mut app, &ctx, "file.saveAs", json!({})).unwrap();
    assert!(blocked(&mut app, true, 0.1), "blocked while the dialog takes the keyboard");
    assert!(blocked(&mut app, false, 0.2));
    let done = pending.lock().unwrap().pop().expect("a dialog was shown");
    done(Vec::new());
    app.run_dialog_answers();
    assert!(!blocked(&mut app, false, 0.3));
    // Save As asks again; the sheet ends without answering.
    invoke(&mut app, &ctx, "file.saveAs", json!({})).unwrap();
    assert_eq!(pending.lock().unwrap().len(), 1, "Save As opens a dialog again");
    assert!(blocked(&mut app, true, 1.0));
    assert!(blocked(&mut app, false, 1.1));
    assert!(blocked(&mut app, false, 9.0), "a dialog may stay up for long");
    assert!(!blocked(&mut app, true, 9.1), "an app window has the keyboard again");
    // A dialog that never takes the keyboard.
    invoke(&mut app, &ctx, "file.saveAs", json!({})).unwrap();
    assert!(blocked(&mut app, true, 10.0));
    assert!(blocked(&mut app, true, 11.0));
    assert!(!blocked(&mut app, true, 12.5));
    // Answers of dialogs taken for gone arrive late: nothing stays blocked.
    for done in pending.lock().unwrap().drain(..) {
        done(Vec::new());
    }
    app.run_dialog_answers();
    assert!(!blocked(&mut app, false, 13.0));
    invoke(&mut app, &ctx, "file.saveAs", json!({})).unwrap();
    assert!(blocked(&mut app, false, 13.1), "a later dialog blocks again");
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

/// Cmd+W with the Render Queue focused closes the queue; the composition next to it stays
/// open (it used to be closed, and the queue stayed).
#[test]
fn cmd_w_in_the_render_queue_closes_the_queue_not_the_comp() {
    let ctx = egui::Context::default();
    let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
    let a = app.session.execute("comp.new", json!({"name": "A", "duration": 4})).unwrap()["comp"].as_u64();
    app.show_panel(PanelKind::RenderQueue);
    assert_eq!(app.ui.focused, PanelKind::RenderQueue);
    invoke(&mut app, &ctx, "file.close", json!({})).unwrap();
    assert!(!app.ui.dock.is_visible(PanelKind::RenderQueue));
    assert_eq!(app.session.state.active_comp.map(|c| c.0), a);
    assert_eq!(app.session.state.open_comps.len(), 1);
    // The focus left the closed queue: the next Close closes the composition.
    assert_ne!(app.ui.focused, PanelKind::RenderQueue);
    invoke(&mut app, &ctx, "file.close", json!({})).unwrap();
    assert_eq!(app.session.state.open_comps.len(), 0, "a second Close acts on the composition");
}

#[test]
fn file_close_is_available_for_an_empty_render_queue_without_a_comp() {
    for params in [Value::Null, json!({})] {
        let ctx = egui::Context::default();
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        assert!(app.session.active_comp().is_none());
        app.show_panel(PanelKind::RenderQueue);
        assert!(menu_items(&app).iter().find(|item| item.id == "file.close").unwrap().enabled);
        invoke(&mut app, &ctx, "file.close", params).unwrap();
        assert!(!app.ui.dock.is_open(PanelKind::RenderQueue));
    }
}

#[test]
fn closing_the_render_queue_reveals_the_panel_receiving_focus() {
    for maximized in [false, true] {
        let ctx = egui::Context::default();
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        app.close_panel(PanelKind::Timeline);
        app.close_panel(PanelKind::Composition);
        app.show_panel(PanelKind::EffectControls);
        assert!(!app.ui.dock.is_visible(PanelKind::Project));
        app.show_panel(PanelKind::RenderQueue);
        if maximized {
            app.apply_dock(craft_ui::docking::Action::Float { panel: PanelKind::RenderQueue, rect: [50.0, 60.0, 400.0, 200.0] }).unwrap();
            app.toggle_maximize(PanelKind::EffectControls);
            app.ui.focused = PanelKind::RenderQueue;
            assert!(app.ui.dock.is_floating(PanelKind::RenderQueue));
        }
        invoke(&mut app, &ctx, "file.close", json!({})).unwrap();
        assert_eq!(app.ui.focused, PanelKind::Project);
        assert!(app.ui.dock.is_visible(app.ui.focused), "focus must belong to a front tab");
        assert_eq!(app.ui.maximized, None, "the focused docked panel must be on screen");
    }
}

/// Closing a floating queue must focus the viewer that is actually shown, retaining maximization.
#[test]
fn closing_a_floating_queue_preserves_the_maximized_composition() {
    let mut session = effectcraft_engine::Session::default();
    session.execute("comp.new", json!({"name": "A", "width": 32, "height": 32, "duration": 2})).unwrap();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|_| EffectcraftApp::new(session));
    h.run_steps(3);
    let ctx = h.ctx.clone();
    let app = h.state_mut();
    app.show_panel(PanelKind::Composition);
    app.show_panel(PanelKind::RenderQueue);
    app.apply_dock(craft_ui::docking::Action::Float { panel: PanelKind::RenderQueue, rect: [50.0, 60.0, 400.0, 200.0] }).unwrap();
    app.toggle_maximize(PanelKind::Composition);
    app.ui.focused = PanelKind::RenderQueue;
    assert!(app.ui.dock.is_visible(PanelKind::Timeline), "the hidden underlying tab triggers the regression");
    assert!(!panel_shown(app, PanelKind::Timeline));
    invoke(app, &ctx, "file.close", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().ui.focused, PanelKind::Composition);
    assert_eq!(h.state().ui.maximized, Some(PanelKind::Composition));
    assert!(!h.state().ui.dock.is_open(PanelKind::RenderQueue));
    assert_eq!(h.state().session.state.open_comps.len(), 1);
}

#[test]
fn queue_close_preserves_state_when_workspace_is_locked() {
    let ctx = egui::Context::default();
    let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
    app.session.execute("comp.new", json!({"name": "A", "duration": 2})).unwrap();
    app.show_panel(PanelKind::RenderQueue);
    app.ui.dock.locked = true;
    let workspace = app.ui.dock.clone();
    let comp = app.session.active_comp_id();
    for _ in 0..2 {
        assert!(invoke(&mut app, &ctx, "file.close", Value::Null).is_err());
        assert_eq!(app.ui.dock, workspace);
        assert_eq!(app.ui.focused, PanelKind::RenderQueue);
        assert_eq!(app.session.active_comp_id(), comp);
    }
    assert!(!menu_items(&app).iter().find(|item| item.id == "file.close").unwrap().enabled);
    app.ui.dock.locked = false;
    invoke(&mut app, &ctx, "file.close", Value::Null).unwrap();
    assert!(!app.ui.dock.is_open(PanelKind::RenderQueue));
}

#[test]
fn queue_close_preserves_viewer_command_target() {
    let ctx = egui::Context::default();
    let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
    let a = app.session.execute("comp.new", json!({"name": "A", "duration": 2})).unwrap()["comp"].as_u64().unwrap();
    let b = app.session.execute("comp.new", json!({"name": "B", "duration": 2})).unwrap()["comp"].as_u64().unwrap();
    app.show_panel(PanelKind::Viewer(1));
    app.ui.viewers.insert(0, Some(a));
    app.ui.viewers.insert(1, Some(b));
    app.ui.active_viewer = 1;
    app.apply_dock(craft_ui::docking::Action::Move {
        panel: PanelKind::Timeline,
        anchor: PanelKind::Composition,
        placement: craft_ui::docking::Placement::Tab { before: None },
    })
    .unwrap();
    app.close_panel(PanelKind::Timeline);
    app.show_panel(PanelKind::Composition);
    app.show_panel(PanelKind::RenderQueue);
    assert_eq!(app.session.active_comp_id().map(|id| id.0), Some(b));
    invoke(&mut app, &ctx, "file.close", json!({})).unwrap();
    assert_eq!(app.ui.focused, PanelKind::Composition);
    assert_eq!(app.ui.active_viewer, 0);
    assert_eq!(app.session.active_comp_id().map(|id| id.0), Some(a));
    invoke(&mut app, &ctx, "file.close", json!({})).unwrap();
    assert!(!app.session.state.open_comps.iter().any(|id| id.0 == a));
    assert!(app.session.state.open_comps.iter().any(|id| id.0 == b));
}

#[test]
fn queue_close_keeps_passive_viewer_focus_across_frames() {
    let mut session = effectcraft_engine::Session::default();
    let a = session.execute("comp.new", json!({"name": "A", "width": 32, "height": 32, "duration": 2})).unwrap()["comp"].as_u64().unwrap();
    let b = session.execute("comp.new", json!({"name": "B", "width": 32, "height": 32, "duration": 2})).unwrap()["comp"].as_u64().unwrap();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|_| EffectcraftApp::new(session));
    h.run_steps(3);
    let ctx = h.ctx.clone();
    let app = h.state_mut();
    app.show_panel(PanelKind::Viewer(1));
    app.ui.viewers.insert(0, Some(a));
    app.ui.viewers.insert(1, Some(b));
    app.ui.active_viewer = 1;
    app.apply_dock(craft_ui::docking::Action::Move {
        panel: PanelKind::Timeline,
        anchor: PanelKind::Composition,
        placement: craft_ui::docking::Placement::Tab { before: None },
    })
    .unwrap();
    app.close_panel(PanelKind::Timeline);
    app.show_panel(PanelKind::Composition);
    app.show_panel(PanelKind::RenderQueue);
    invoke(app, &ctx, "file.close", Value::Null).unwrap();
    h.run_steps(4);
    let app = h.state_mut();
    assert_eq!(app.ui.focused, PanelKind::Composition);
    assert_eq!(app.ui.active_viewer, 0);
    assert_eq!(app.session.active_comp_id().map(|id| id.0), Some(a));
    assert!(app.ui.dock.is_visible(PanelKind::Composition));
    assert!(!app.ui.dock.is_open(PanelKind::RenderQueue));
    invoke(app, &ctx, "file.close", json!({})).unwrap();
    assert!(!app.session.state.open_comps.iter().any(|id| id.0 == a));
    assert!(app.session.state.open_comps.iter().any(|id| id.0 == b));
    // A later explicit Open still routes normally, rather than inheriting a deferred focus override.
    invoke(app, &ctx, "comp.open", json!({"comp": b})).unwrap();
    h.run_steps(3);
    assert!(h.state().ui.dock.is_visible(PanelKind::Timeline));
    assert_eq!(h.state().session.active_comp_id().map(|id| id.0), Some(b));
}

#[test]
fn menu_items_cover_the_tree() {
    assert_eq!(menus().last(), Some(&"Help"));
    assert!(effectcraft_engine::menus::entries().len() > 500);
}
