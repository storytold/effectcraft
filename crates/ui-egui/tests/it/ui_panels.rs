//! Headless checks for the M13.6 panels: each opens from the Window menu as a real panel and
//! registers its automation ids; the Footage panel's buttons edit into the comp; the Progress
//! panel lists and cancels a job; Lumetri Scopes switch scope through their dropdown.

use std::sync::Arc;

use effectcraft_engine::{Session, config::MemoryConfig};
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui::{Event, Pos2, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

fn harness() -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Panels", "width": 320, "height": 180, "duration": 4})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    h
}

fn open(h: &mut Harness<'_, EffectcraftApp>, panel: &str) {
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "window.panel", json!({"panel": panel})).unwrap();
    h.run_steps(3);
}

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}")).clone();
    let p: Pos2 = pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
    h.input_mut().events.push(Event::PointerMoved(p));
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
}

/// #581: the header's Discord pill fits its label with even padding (it was a fixed 104 px with
/// the text at its left).
#[test]
fn discord_pill_fits_its_label() {
    let h = harness();
    let e = h.state().auto.find("header.discord").expect("the Discord pill").clone();
    let text = h.ctx.fonts_mut(|f| f.layout_no_wrap("Discord".into(), effectcraft_ui_egui::theme::Tokens::semibold(12.0), egui::Color32::WHITE).size().x);
    // Padding 10, the 14 px icon, a 6 px gap, the text, padding 12.
    assert!((e.rect[2] - (text + 42.0)).abs() < 0.5, "{} for a {text} px label", e.rect[2]);
}

#[test]
fn window_menu_opens_the_new_panels() {
    let mut h = harness();
    for (name, kind, auto) in [
        ("lumetriScopes", PanelKind::LumetriScopes, "scopes.kind"),
        ("footage", PanelKind::Footage, "footage.empty"),
        ("mediaBrowser", PanelKind::MediaBrowser, "mediaBrowser.path"),
        ("metadata", PanelKind::Metadata, "metadata.projectComment"),
        ("progress", PanelKind::Progress, ""),
        ("contentAwareFill", PanelKind::ContentAwareFill, "contentFill.method"),
    ] {
        open(&mut h, name);
        assert!(h.state().ui.dock.contains(kind) || h.state().ui.floating.iter().any(|f| f.panels.contains(&kind)), "{name} shown");
        if !auto.is_empty() {
            assert!(h.state().auto.find(auto).is_some(), "{name}: {auto}");
        }
    }
}

/// #1630: choosing a checked panel in the Window menu closes it, as in After Effects (it only
/// brought the panel to the front, so the check mark never went away); choosing it again opens it.
#[test]
fn window_menu_closes_a_checked_panel() {
    let mut h = harness();
    let shown = |h: &Harness<'_, EffectcraftApp>| {
        h.state().ui.dock.contains(PanelKind::Align) || h.state().ui.floating.iter().any(|f| f.panels.contains(&PanelKind::Align))
    };
    open(&mut h, "align");
    assert!(shown(&h));
    click(&mut h, "menu.Window");
    let at = h.query_by_label("✔ Align").expect("Window ▸ Align, checked").rect().center();
    click_at(&mut h, at);
    assert!(!shown(&h), "Align closed");
    click(&mut h, "menu.Window");
    let at = h.query_by_label(" Align").expect("Window ▸ Align, unchecked").rect().center();
    click_at(&mut h, at);
    assert!(shown(&h), "Align open again");
}

#[test]
fn scopes_switch_kind() {
    let mut h = harness();
    open(&mut h, "lumetriScopes");
    assert!(h.state().auto.find("scopes.plot").is_some());
    h.state_mut().ui.scopes.scope = "vectorscopeYuv".into();
    h.run_steps(2);
    assert_eq!(h.state().auto.find("scopes.plot").unwrap().label, "Vectorscope YUV");
    h.state_mut().ui.scopes.scope = "histogram".into();
    h.run_steps(2);
    assert_eq!(h.state().auto.find("scopes.kind").unwrap().label, "Histogram");
}

#[test]
fn footage_panel_buttons_edit_into_the_comp() {
    let mut h = harness();
    let item = h.state().session.project.items.values().find(|i| matches!(i.kind, effectcraft_engine::project::ItemKind::Solid(_))).unwrap().id.0;
    h.state_mut().session.execute("footage.open", json!({"item": item})).unwrap();
    h.run_steps(4);
    assert!(h.state().ui.dock.contains(PanelKind::Footage) || h.state().ui.floating.iter().any(|f| f.panels.contains(&PanelKind::Footage)));
    assert!(h.state().auto.find("footage.overlayEdit").is_some());
    let before = h.state().session.active_comp().unwrap().layers.len();
    click(&mut h, "footage.overlayEdit");
    assert_eq!(h.state().session.active_comp().unwrap().layers.len(), before + 1);
    click(&mut h, "footage.rippleInsertEdit");
    assert!(h.state().session.active_comp().unwrap().layers.len() >= before + 2);
}

#[test]
fn progress_panel_cancels_a_job() {
    let mut h = harness();
    h.state_mut()
        .session
        .spawn_task("test", "Long analysis", false, |ctl| {
            while ctl.progress(1, 2) {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            Err("cancelled".into())
        })
        .unwrap();
    open(&mut h, "progress");
    let id = h.state().session.jobs()[0].id.clone();
    assert!(h.state().auto.find(&format!("progress.job.{id}")).is_some());
    click(&mut h, &format!("progress.cancel.{id}"));
    h.state_mut().session.wait_jobs();
    h.run_steps(2);
    assert!(h.state().session.jobs().is_empty());
    assert_eq!(h.state().session.job_log.last().unwrap().status, "cancelled");
}

fn click_at(h: &mut Harness<'_, EffectcraftApp>, p: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(p));
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
}

fn double_click_at(h: &mut Harness<'_, EffectcraftApp>, pos: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(pos));
    h.step();
    for pressed in [true, false, true, false] {
        h.input_mut().events.push(Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
    }
    h.run_steps(3);
}

fn hover(h: &mut Harness<'_, EffectcraftApp>, p: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(p));
    h.run_steps(3);
}

/// Issue #320: saved workspaces last between sessions (they were only kept in memory): a new
/// app on the same settings store lists them and brings their layout back; renamed and deleted
/// ones stay that way.
#[test]
fn saved_workspaces_persist_across_sessions() {
    use effectcraft_engine::config::{ConfigStore, MemoryConfig};
    use effectcraft_ui_egui::menus::invoke;
    let store = std::sync::Arc::new(MemoryConfig::default());
    let launch = || {
        let s = Session { config: Some(store.clone()), ..Default::default() };
        let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
        h.run_steps(2);
        h
    };
    let mut h = launch();
    let ctx = h.ctx.clone();
    invoke(h.state_mut(), &ctx, "window.workspace", json!({"name": "Minimal"})).unwrap();
    invoke(h.state_mut(), &ctx, "window.saveWorkspaceAs", json!({"name": "My Layout"})).unwrap();
    invoke(h.state_mut(), &ctx, "window.saveWorkspaceAs", json!({"name": "Scratch"})).unwrap();
    let saved = h.state().ui.dock.clone();
    drop(h);
    let mut h = launch();
    assert_eq!(h.state().saved_workspace_names(), ["My Layout", "Scratch"]);
    assert_eq!(h.state().ui.workspace, "Scratch");
    let ctx = h.ctx.clone();
    invoke(h.state_mut(), &ctx, "window.workspace", json!({"name": "My Layout"})).unwrap();
    assert_eq!(h.state().ui.dock, saved);
    invoke(h.state_mut(), &ctx, "window.editWorkspaces", json!({"name": "Scratch", "delete": true})).unwrap();
    invoke(h.state_mut(), &ctx, "window.editWorkspaces", json!({"name": "My Layout", "rename": "Edit Bay"})).unwrap();
    drop(h);
    assert_eq!(launch().state().saved_workspace_names(), ["Edit Bay"]);
    // A damaged file is ignored.
    store.write(effectcraft_ui_egui::WORKSPACES_FILE, "{not json").unwrap();
    assert!(launch().state().saved_workspace_names().is_empty());
}

/// Issue #191: a workspace saved with Save as New Workspace is listed in Window ▸ Workspace
/// (below the built-ins, where the submenu used to be cut off) and choosing it there brings its
/// layout back.
#[test]
fn saved_workspace_is_listed_in_the_workspace_menu() {
    let mut h = harness();
    // Saving needs a settings folder.
    h.state_mut().session.config = Some(Arc::new(MemoryConfig::default()));
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "window.workspace", json!({"name": "Minimal"})).unwrap();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "window.saveWorkspaceAs", json!({"name": "My Layout"})).unwrap();
    h.run_steps(3);
    let saved = h.state().ui.dock.clone();
    assert_eq!(h.state().saved_workspace_names(), ["My Layout"]);
    let cx = effectcraft_engine::menus::DynCtx { workspace: Some("My Layout"), saved_workspaces: &["My Layout".to_string()] };
    let (entries, _) = effectcraft_engine::menus::dynamic(&h.state().session, "savedWorkspaces", &cx);
    assert_eq!(entries.len(), 1);
    assert_eq!((entries[0].label.as_str(), entries[0].command.as_str(), &entries[0].params), ("My Layout", "window.workspace", &json!({"name": "My Layout"})));
    // Leave it, then pick it from the in-window menu bar.
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "window.workspace", json!({"name": "Default"})).unwrap();
    h.run_steps(3);
    assert_ne!(h.state().ui.dock, saved);
    click(&mut h, "menu.Window");
    let ws = h.query_by_label(" Workspace").expect("Window ▸ Workspace").rect();
    hover(&mut h, ws.center());
    let entry = h.state().auto.find("menu.savedWorkspaces.0").expect("the saved workspace is listed").clone();
    assert_eq!(entry.label, "My Layout");
    let at = h.query_by_label(" My Layout").expect("the saved workspace entry").rect().center();
    // Into the submenu, then down it (as a pointer moves).
    hover(&mut h, pos2(at.x, ws.center().y));
    for k in 1..=10 {
        hover(&mut h, pos2(at.x, ws.center().y + (at.y - ws.center().y) * k as f32 / 10.0));
    }
    click_at(&mut h, at);
    assert_eq!(h.state().ui.workspace, "My Layout");
    assert_eq!(h.state().ui.dock, saved, "its layout came back");
}

/// Edit ▸ Label shows each label's colour before its name (#290), and choosing one labels the
/// selected layer.
#[test]
fn edit_label_menu_shows_the_label_colours() {
    let mut h = harness();
    click(&mut h, "menu.Edit");
    let label = h.query_by_label(" Label").expect("Edit ▸ Label").rect();
    hover(&mut h, label.center());
    // (Not Dark Green; the entry's label starts with its check-mark column.)
    let green = h.query_by_label(" Green").expect("the Label submenu is open").rect();
    let col = h.state().tokens.label(effectcraft_engine::color::Label::Green);
    let swatch = |s: &egui::Shape| matches!(s, egui::Shape::Rect(r) if r.fill == col && r.rect.width() < 16.0 && green.contains(r.rect.center()));
    let shapes: Vec<egui::Shape> = h.output().shapes.iter().map(|c| c.shape.clone()).collect();
    assert!(shapes.iter().any(|s| swatch(s) || matches!(s, egui::Shape::Vec(v) if v.iter().any(swatch))), "no Green swatch");
    // Into the submenu, then down to Green.
    hover(&mut h, pos2(green.center().x, label.center().y));
    for k in 1..=10 {
        hover(&mut h, pos2(green.center().x, label.center().y + (green.center().y - label.center().y) * k as f32 / 10.0));
    }
    click_at(&mut h, green.center());
    assert_eq!(h.state().session.active_comp().unwrap().layers[0].label, effectcraft_engine::color::Label::Green);
}

/// Learn opens the Home screen's tutorials; clicking another workspace tab closes them again,
/// however quickly the tabs are clicked (they stayed over every workspace, #272).
#[test]
fn leaving_the_learn_workspace_closes_its_tutorials() {
    let mut h = harness();
    for name in ["Learn", "Default", "Learn", "Review", "Learn", "Learn", "Small Screen", "Standard", "Learn", "Default"] {
        click(&mut h, &format!("header.workspace.{name}"));
        assert_eq!(h.state().ui.workspace, name);
        let learn = name == "Learn";
        assert_eq!(h.state().ui.start_screen, learn, "{name}: the Home screen shows only for Learn");
        assert_eq!(!h.state().auto.query("home.learn.").is_empty(), learn, "{name}: the tutorials show only for Learn");
    }
}

fn open_settings(h: &mut Harness<'_, EffectcraftApp>) {
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "app.settings", json!({"page": "appearance"})).unwrap();
    h.run_steps(3);
}

fn key(h: &mut Harness<'_, EffectcraftApp>, key: egui::Key, modifiers: egui::Modifiers) {
    for pressed in [true, false] {
        h.input_mut().events.push(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
        h.step();
    }
}

/// The UI Scale setting and the zoom it gives.
fn scale(h: &Harness<'_, EffectcraftApp>) -> (u32, f32) {
    (h.state().session.prefs.appearance.ui_scale, h.ctx.zoom_factor())
}

/// Types `text` into Settings' UI Scale a key at a time: nothing applies (and the field keeps
/// the keyboard) until Enter.
fn type_scale(h: &mut Harness<'_, EffectcraftApp>, text: &str) {
    let before = scale(h);
    click(h, "settings.appearance.uiScale");
    for c in text.chars() {
        h.input_mut().events.push(Event::Text(c.to_string()));
        h.run_steps(2);
        assert_eq!(scale(h), before, "{text}: not while typing");
        assert!(h.ctx.memory(|m| m.focused().is_some()), "{text}: the field keeps the keyboard");
    }
    key(h, egui::Key::Enter, egui::Modifiers::NONE);
    // (Settings re-centres a frame after the zoom changes.)
    h.run_steps(4);
}

/// #284, #682: Settings ▸ Appearance ▸ UI Scale takes any typed percentage from 75 to 200 and
/// sizes the whole interface once it is entered, Cancel puts it back, OK keeps it and later
/// sessions start with it; Ctrl+0 (Window ▸ Project) doesn't reset it.
#[test]
fn ui_scale_setting_sizes_the_interface() {
    let mut h = harness();
    let ctx = h.ctx.clone();
    assert_eq!(ctx.zoom_factor(), 1.0);
    open_settings(&mut h);
    type_scale(&mut h, "85");
    assert_eq!(scale(&h), (85, 0.85), "live");
    click(&mut h, "settings.cancel");
    h.run_steps(2);
    assert_eq!(scale(&h), (100, 1.0), "Cancel put it back");
    open_settings(&mut h);
    type_scale(&mut h, "300");
    assert_eq!(scale(&h), (200, 2.0), "at most 200");
    click(&mut h, "settings.cancel");
    h.run_steps(2);
    assert_eq!(scale(&h), (100, 1.0), "Cancel put it back from 200 %");
    open_settings(&mut h);
    type_scale(&mut h, "50");
    assert_eq!(scale(&h), (75, 0.75), "at least 75");
    type_scale(&mut h, "90");
    click(&mut h, "settings.ok");
    h.run_steps(2);
    assert_eq!(h.state().dialog, None);
    assert_eq!(scale(&h), (90, 0.9), "OK kept it");
    // Ctrl+0 opens the Project panel; egui's own Ctrl+0 zoom reset stays out of it.
    key(&mut h, egui::Key::Num0, egui::Modifiers::COMMAND);
    h.run_steps(2);
    assert_eq!(ctx.zoom_factor(), 0.9);
    // A saved scale applies from the first frame.
    let s = Session { prefs: effectcraft_engine::prefs::Prefs::from_json(&h.state().session.prefs.to_json()), ..Default::default() };
    let mut h2 = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h2.run_steps(3);
    assert_eq!(h2.ctx.zoom_factor(), 0.9);
}

/// Settings' Previous / Next / Cancel / OK stay whole and in view at 200 % in a 1600×1000
/// window (they scrolled out of the dialog with its pages): the page scrolls under them, Cancel
/// restores the settings and OK saves them.
#[test]
fn settings_buttons_stay_in_view_at_200_percent() {
    let store = Arc::new(MemoryConfig::default());
    let saved = || {
        let mut s = Session { config: Some(store.clone()), ..Default::default() };
        s.prefs.disk.disk_cache_enabled = false;
        s.load_settings();
        s.prefs.appearance.ui_scale
    };
    let mut s = Session { config: Some(store.clone()), ..Default::default() };
    s.prefs.disk.disk_cache_enabled = false;
    s.prefs.set("appearance.uiScale", json!(200)).unwrap();
    s.save_prefs();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    assert_eq!(scale(&h), (200, 2.0));
    let rect = |h: &Harness<'_, EffectcraftApp>, id: &str| {
        let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
        egui::Rect::from_min_size(pos2(e.rect[0], e.rect[1]), egui::vec2(e.rect[2], e.rect[3]))
    };
    // Whole: inside the window, and painted with none of them clipped away.
    let buttons = |h: &Harness<'_, EffectcraftApp>| {
        ["previous", "next", "cancel", "ok"].map(|b| {
            let r = rect(h, &format!("settings.{b}"));
            let clip = h
                .output()
                .shapes
                .iter()
                .find(|c| matches!(&c.shape, egui::Shape::Rect(s) if (s.rect.center() - r.center()).length() < 0.5))
                .map(|c| c.clip_rect);
            assert!(h.ctx.content_rect().contains_rect(r) && clip.is_some_and(|c| c.contains_rect(r)), "{b} is cut off: {r:?} in {clip:?}");
            r
        })
    };
    let scroll_page = |h: &mut Harness<'_, EffectcraftApp>| {
        let at = rect(h, "settings.appearance.appearanceMode").center();
        h.input_mut().events.push(Event::PointerMoved(at));
        h.input_mut().events.push(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -1000.0),
            modifiers: Default::default(),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
        });
        h.run_steps(5);
    };
    open_settings(&mut h);
    let footer = buttons(&h);
    let field = rect(&h, "settings.appearance.uiScale");
    scroll_page(&mut h);
    assert_eq!(buttons(&h), footer, "the buttons don't scroll with the page");
    let scrolled = rect(&h, "settings.appearance.uiScale");
    assert!(scrolled.top() < field.top() && scrolled.bottom() < footer[2].top(), "the page scrolled to UI Scale: {field:?} -> {scrolled:?}");
    for (button, kept) in [("settings.cancel", 200), ("settings.ok", 150)] {
        if h.state().dialog.is_none() {
            open_settings(&mut h);
            scroll_page(&mut h);
        }
        type_scale(&mut h, "150");
        assert_eq!(scale(&h), (150, 1.5));
        buttons(&h);
        click(&mut h, button);
        h.run_steps(2);
        assert_eq!(h.state().dialog, None, "{button} closed Settings");
        assert_eq!((scale(&h), saved()), ((kept, kept as f32 / 100.0), kept), "{button}");
    }
}

/// Headless look at the panels (wgpu offscreen; needs a GPU adapter). Run with
/// `PANELS_SNAPSHOT=/abs/dir cargo test -p effectcraft-ui-egui --test it ui_panels:: -- --ignored`.
#[test]
#[ignore]
fn panels_snapshot() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let solid = s.project.items.values().find(|i| matches!(i.kind, effectcraft_engine::project::ItemKind::Solid(_))).map(|i| i.id.0);
    let mut app = Some(EffectcraftApp::new(s));
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| app.take().expect("app"));
    h.run_steps(3);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "window.workspace", json!({"name": "All Panels"})).unwrap();
    h.run_steps(3);
    let dir = std::env::var("PANELS_SNAPSHOT").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/test-out").into());
    std::fs::create_dir_all(&dir).unwrap();
    if let Some(id) = solid {
        h.state_mut().session.execute("footage.open", json!({"item": id})).unwrap();
    }
    for (panel, scope) in [
        ("lumetriScopes", "waveformRgb"),
        ("lumetriScopes", "vectorscopeYuv"),
        ("lumetriScopes", "histogram"),
        ("lumetriScopes", "paradeRgb"),
        ("contentAwareFill", ""),
        ("metadata", ""),
        ("progress", ""),
        ("mediaBrowser", ""),
        ("footage", ""),
    ] {
        if !scope.is_empty() {
            h.state_mut().ui.scopes.scope = scope.into();
        }
        open(&mut h, panel);
        h.run_steps(3);
        let img = h.render().expect("render");
        img.save(format!("{dir}/{panel}{scope}.png")).unwrap();
    }
}

/// Drag the automation element `id` horizontally by `dx` points.
fn drag_x(h: &mut Harness<'_, EffectcraftApp>, id: &str, dx: f32) {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}")).clone();
    let from: Pos2 = pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
    h.input_mut().events.push(Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    for k in 1..=10 {
        h.input_mut().events.push(Event::PointerMoved(from + egui::vec2(dx * k as f32 / 10.0, 0.0)));
        h.step();
    }
    let to = from + egui::vec2(dx, 0.0);
    h.input_mut().events.push(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
}

/// #338: an Essential Graphics Scale control keeps After Effects' Constrain Proportions link, on
/// by default: dragging X scales Y with it; unlinked, X moves alone.
#[test]
fn essential_graphics_scale_keeps_constrain_proportions() {
    let mut h = harness();
    let layer = h.state().session.active_comp().unwrap().layers[0].id.0;
    h.state_mut().session.execute("prop.set", json!({"layer": layer, "path": "transform/scale", "value": [100.0, 50.0]})).unwrap();
    let c =
        h.state_mut().session.execute("essential.addProperty", json!({"layer": layer, "path": "transform/scale"})).unwrap()["controls"][0].as_u64().unwrap();
    open(&mut h, "essentialGraphics");
    let value = format!("essential.control.{c}.value");
    let scale = |h: &Harness<'_, EffectcraftApp>| {
        let comp = h.state().session.active_comp().unwrap();
        let l = comp.layers.iter().find(|l| l.id.0 == layer).unwrap();
        l.props.prop("transform/scale").unwrap().value.components()
    };
    assert!(h.state().auto.find(&format!("{value}.link")).is_some(), "the chain link is drawn");
    drag_x(&mut h, &format!("{value}.0"), 40.0);
    let linked = scale(&h);
    assert!(linked[0] > 100.0, "{linked:?}");
    assert!((linked[1] / linked[0] - 0.5).abs() < 1e-6, "Y follows X in proportion: {linked:?}");
    click(&mut h, &format!("{value}.link"));
    drag_x(&mut h, &format!("{value}.0"), 40.0);
    let unlinked = scale(&h);
    assert!(unlinked[0] > linked[0] && unlinked[1] == linked[1], "unlinked, X moves alone: {unlinked:?}");
}

/// #956: the Properties panel's Scale chain link shares the Timeline's Constrain Proportions
/// state: unlinking in Properties unlinks the Timeline row too, and X/Y scale together only
/// while linked.
#[test]
fn properties_scale_link_shares_the_timeline_state() {
    let mut h = harness();
    let layer = h.state().session.active_comp().unwrap().layers[0].id.0;
    h.state_mut().session.execute("layer.select", json!({"layers": [layer]})).unwrap();
    h.state_mut().session.execute("prop.set", json!({"layer": layer, "path": "transform/scale", "value": [100.0, 50.0]})).unwrap();
    open(&mut h, "properties");
    let uid = {
        let comp = h.state().session.active_comp().unwrap();
        let l = comp.layers.iter().find(|l| l.id.0 == layer).unwrap();
        l.props.prop("transform/scale").unwrap().uid
    };
    let link = format!("properties.prop.{uid}.constrain");
    assert!(h.state().auto.find(&link).is_some(), "the chain link is drawn");
    let scale = |h: &Harness<'_, EffectcraftApp>| {
        let comp = h.state().session.active_comp().unwrap();
        let l = comp.layers.iter().find(|l| l.id.0 == layer).unwrap();
        l.props.prop("transform/scale").unwrap().value.components()
    };
    // Linked: dragging X scales Y with it.
    drag_x(&mut h, &format!("properties.prop.{uid}.value.0"), 40.0);
    let linked = scale(&h);
    assert!(linked[0] > 100.0, "{linked:?}");
    assert!((linked[1] / linked[0] - 0.5).abs() < 1e-6, "Y follows X in proportion: {linked:?}");
    // Unlinking in Properties writes the shared state, so the Timeline row unlinks too.
    click(&mut h, &link);
    assert!(h.state().ui.timeline.unlinked.contains(&uid), "unlinking writes the shared state");
    drag_x(&mut h, &format!("properties.prop.{uid}.value.0"), 40.0);
    let unlinked = scale(&h);
    assert!(unlinked[0] > linked[0] && unlinked[1] == linked[1], "unlinked, X moves alone: {unlinked:?}");
    // Re-linking clears it again.
    click(&mut h, &link);
    assert!(!h.state().ui.timeline.unlinked.contains(&uid), "re-linking clears the shared state");
}

#[test]
fn properties_label_double_click_reveals_without_editing() {
    let mut h = harness();
    let layer = h.state().session.active_comp().unwrap().layers[0].id;
    let other = h.state_mut().session.execute("layer.newSolid", json!({"name": "Other", "color": "#204060"})).unwrap()["layer"].as_u64().unwrap();
    h.state_mut().session.execute("prop.addKey", json!({"layer": layer.0, "path": "transform/opacity", "time": 1.0, "value": 50.0})).unwrap();
    h.state_mut().session.execute("layer.select", json!({"layers": [layer.0]})).unwrap();
    let comp = h.state().session.active_comp().unwrap();
    let opacity = comp.layer(layer).unwrap().props.prop("transform/opacity").unwrap().uid;
    let other_scale = comp.layers.iter().find(|l| l.id.0 == other).unwrap().props.prop("transform/scale").unwrap().uid;
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::frontend(h.state_mut(), &ctx, "timeline.revealProps", json!({"props": [{"layer": other, "prop": other_scale}]})).unwrap();
    open(&mut h, "properties");
    open(&mut h, "renderQueue");
    h.state_mut().ui.timeline.search = "no matching property".into();
    h.run_steps(3);
    let value = h.state().auto.find(&format!("properties.prop.{opacity}.value")).unwrap().rect;
    let label = pos2(value[0] - 60.0, value[1] + value[3] / 2.0);
    let row = format!("timeline.prop.{opacity}.row");
    click_at(&mut h, label);
    assert_eq!(h.state().ui.timeline.search, "no matching property");
    assert!(h.state().auto.find(&row).is_none());
    h.run_steps(30);
    let before = serde_json::to_value(h.state().session.project.as_ref()).unwrap();
    let undo = h.state().session.history.undo.len();
    let selected = h.state().session.state.selected_layers.clone();
    for _ in 0..2 {
        double_click_at(&mut h, label);
        assert!(h.state().auto.find(&row).is_some(), "Opacity is visible for keyframing");
        assert!(h.state().auto.find(&format!("timeline.prop.{other_scale}.row")).is_some(), "the other layer keeps its reveal");
        assert_eq!(h.state().ui.timeline.search, "");
        h.run_steps(30);
    }
    assert_eq!(serde_json::to_value(h.state().session.project.as_ref()).unwrap(), before, "revealing preserves values and keyframes");
    assert_eq!(h.state().session.history.undo.len(), undo);
    assert_eq!(h.state().session.state.selected_layers, selected);
}

#[test]
fn properties_reveal_scrolls_to_the_displayed_layer_in_a_multiple_selection() {
    let mut h = harness();
    let layer = h.state().session.active_comp().unwrap().layers[0].id;
    for _ in 0..40 {
        h.state_mut().session.execute("layer.newSolid", json!({"name": "Spacer", "color": "#204060"})).unwrap();
    }
    let top = h.state().session.active_comp().unwrap().layers[0].id;
    h.state_mut().session.execute("layer.select", json!({"layers": [layer.0, top.0]})).unwrap();
    let uid = h.state().session.active_comp().unwrap().layer(layer).unwrap().props.prop("transform/opacity").unwrap().uid;
    open(&mut h, "properties");
    h.state_mut().toggle_maximize(PanelKind::Properties);
    h.run_steps(3);
    let r = h.state().auto.find(&format!("properties.prop.{uid}.label")).unwrap().rect;
    double_click_at(&mut h, pos2(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0));
    assert!(h.state().auto.find(&format!("timeline.prop.{uid}.row")).is_some(), "the displayed layer's property is in view");
    assert_eq!(h.state().session.state.selected_layers, [layer, top]);
}

#[test]
fn properties_transform_labels_reveal_the_corresponding_property() {
    let mut h = harness();
    let layer = h.state().session.active_comp().unwrap().layers[0].id;
    h.state_mut().session.execute("layer.select", json!({"layers": [layer.0]})).unwrap();
    let props: Vec<u64> =
        h.state().session.active_comp().unwrap().layer(layer).unwrap().transform().unwrap().props().filter(|p| !p.three_d_only).map(|p| p.uid).collect();
    open(&mut h, "properties");
    for uid in &props {
        h.state_mut().ui.timeline.open_layers.remove(&layer.0);
        h.run_steps(30);
        assert!(h.state().auto.find(&format!("timeline.prop.{uid}.row")).is_none());
        let r = h.state().auto.find(&format!("properties.prop.{uid}.label")).unwrap().rect;
        double_click_at(&mut h, pos2(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0));
        for candidate in &props {
            assert_eq!(h.state().auto.find(&format!("timeline.prop.{candidate}.row")).is_some(), candidate == uid);
        }
    }
}

/// Gap 7: the Backgrounds category is listed under * Animation Presets and a preset applies on double-click.
#[test]
fn background_presets_are_listed_and_apply() {
    let mut h = harness();
    let layer = h.state().session.active_comp().unwrap().layers[0].id.0;
    h.state_mut().session.execute("layer.select", json!({"layers": [layer]})).unwrap();
    open(&mut h, "effectsPresets");
    h.state_mut().ui.effects_search = "slow fog".into();
    h.run_steps(3);
    assert!(h.state().auto.find("effects.category.presets/Backgrounds").is_some(), "the Backgrounds folder is listed");
    let e = h.state().auto.find("effects.preset.builtin.slowFog").expect("Slow Fog is listed").clone();
    let p: Pos2 = pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    for _ in 0..2 {
        for pressed in [true, false] {
            h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
        }
    }
    h.run_steps(3);
    let comp = h.state().session.active_comp().unwrap();
    let names: Vec<String> = comp.layers[0].effects().unwrap().groups().map(|g| g.name.clone()).collect();
    assert_eq!(names, ["Fractal Noise", "Tint"]);
}

/// #757: Effects & Presets lists the built-in Transitions presets under * Animation Presets;
/// double-clicking one applies it to the selected layer (effect plus keyframes).
#[test]
fn transition_presets_apply_from_effects_and_presets() {
    let mut h = harness();
    let layer = h.state().session.active_comp().unwrap().layers[0].id.0;
    h.state_mut().session.execute("layer.select", json!({"layers": [layer]})).unwrap();
    open(&mut h, "effectsPresets");
    h.state_mut().ui.effects_search = "clock reveal".into();
    h.run_steps(3);
    assert!(h.state().auto.find("effects.category.presets/Transitions").is_some(), "the Transitions folder is listed");
    let e = h.state().auto.find("effects.preset.builtin.clockReveal").expect("Clock Reveal is listed").clone();
    let p: Pos2 = pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    // Both clicks in one frame: the harness' frame time is longer than a double-click's.
    for _ in 0..2 {
        for pressed in [true, false] {
            h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
        }
    }
    h.run_steps(3);
    let comp = h.state().session.active_comp().unwrap();
    let fx = comp.layers[0].effects().unwrap().groups().next().expect("an effect was added").clone();
    assert_eq!(fx.name, "Radial Wipe");
    assert_eq!(fx.prop("completion").unwrap().keys.len(), 2);
}
