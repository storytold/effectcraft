//! The native (macOS) menu bar as a plain, serializable spec. The desktop app turns it into an
//! `NSMenu` with `muda` (`apps/effectcraft/src/native_menu.rs`); everything that decides *what*
//! the menu contains lives here, so it is tested without creating real menus and builds for the
//! web (where the in-window menu bar is used).
//!
//! The spec mirrors the engine's After Effects menu tree ([`effectcraft_engine::menus`]):
//! submenus, separators, labels (Undo/Redo and label names are live), check marks, enabled
//! state, and the active shortcut preset's keys as accelerators. File ▸ Open Recent lists the
//! recent projects. The application menu's Hide / Hide Others / Show All are macOS'
//! predefined items, and a Services submenu is added as on every Mac app.
//!
//! Item ids are positional (`m.3.0.2`), stable while the menu's structure stays the same; the
//! app rebuilds the native menu when [`NativeMenu::structure`] changes and otherwise only pushes
//! label / enabled / checked updates, and only when [`state_key`] changes.

use std::hash::{Hash, Hasher};

use serde::Serialize;
use serde_json::{Value, json};

use crate::EffectcraftApp;
use effectcraft_engine::menus::{MenuNode, menu_bar};

/// What macOS performs itself (no command dispatch).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    Services,
    Hide,
    HideOthers,
    ShowAll,
}

/// One node of the native menu.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum NativeNode {
    Item(NativeItem),
    Separator,
    Submenu {
        label: String,
        children: Vec<NativeNode>,
    },
    /// A predefined macOS item; `command` is the engine entry it stands for (empty for Services).
    Predefined {
        role: Role,
        label: String,
        command: String,
    },
}

/// A clickable item: activating it runs `command` with `params`, exactly like the in-window bar.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NativeItem {
    pub id: String,
    pub label: String,
    pub command: String,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub params: Value,
    /// The shortcut in EffectCraft notation (`Cmd+Shift+S`).
    pub shortcut: Option<String>,
    /// The menu key equivalent in `muda` accelerator notation, when the shortcut can be one (see
    /// [`accelerator`]).
    pub accelerator: Option<String>,
    pub enabled: bool,
    pub checked: Option<bool>,
}

/// The whole menu bar: top-level menus in order (the application menu first).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct NativeMenu {
    pub menus: Vec<NativeNode>,
}

impl NativeMenu {
    /// Every clickable item, depth first.
    pub fn items(&self) -> Vec<&NativeItem> {
        fn rec<'a>(n: &'a [NativeNode], out: &mut Vec<&'a NativeItem>) {
            for x in n {
                match x {
                    NativeNode::Item(i) => out.push(i),
                    NativeNode::Submenu { children, .. } => rec(children, out),
                    _ => {}
                }
            }
        }
        let mut out = vec![];
        rec(&self.menus, &mut out);
        out
    }

    pub fn find(&self, id: &str) -> Option<&NativeItem> {
        self.items().into_iter().find(|i| i.id == id)
    }

    /// Fingerprint of everything that needs a rebuild when it changes (tree shape, commands,
    /// accelerators, submenu labels); labels, enabled and check state are updated in place.
    pub fn structure(&self) -> u64 {
        fn rec(n: &[NativeNode], h: &mut std::collections::hash_map::DefaultHasher) {
            for x in n {
                match x {
                    NativeNode::Item(i) => {
                        0u8.hash(h);
                        i.id.hash(h);
                        i.command.hash(h);
                        i.params.to_string().hash(h);
                        i.accelerator.hash(h);
                    }
                    NativeNode::Separator => 1u8.hash(h),
                    NativeNode::Submenu { label, children } => {
                        2u8.hash(h);
                        label.hash(h);
                        rec(children, h);
                        3u8.hash(h);
                    }
                    NativeNode::Predefined { role, .. } => {
                        4u8.hash(h);
                        role.hash(h);
                    }
                }
            }
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        rec(&self.menus, &mut h);
        h.finish()
    }
}

/// Build the native menu spec for the app's current state.
pub fn build(app: &EffectcraftApp) -> NativeMenu {
    fn rec(app: &EffectcraftApp, nodes: &[MenuNode], path: &str, out: &mut Vec<NativeNode>) {
        let mut services_added = false;
        for (i, n) in nodes.iter().enumerate() {
            let id = format!("{path}.{i}");
            match n {
                MenuNode::Separator => out.push(NativeNode::Separator),
                MenuNode::Dynamic { name } => {
                    // Recent projects / footage / presets, undo history, shortcut slots, viewers,
                    // saved workspaces.
                    let saved = app.saved_workspace_names();
                    let (entries, empty) = effectcraft_engine::menus::dynamic(&app.session, name, &crate::menus::dyn_ctx(&app.ui.workspace, &saved));
                    if entries.is_empty()
                        && let Some(e) = empty
                    {
                        out.push(NativeNode::Item(NativeItem {
                            id: format!("{id}.none"),
                            label: e.to_string(),
                            command: String::new(),
                            params: Value::Null,
                            shortcut: None,
                            accelerator: None,
                            enabled: false,
                            checked: None,
                        }));
                    }
                    for (k, e) in entries.iter().enumerate() {
                        out.push(NativeNode::Item(NativeItem {
                            id: format!("{id}.d{k}"),
                            label: e.label.clone(),
                            command: e.command.clone(),
                            params: e.params.clone(),
                            shortcut: None,
                            accelerator: None,
                            enabled: crate::menus::entry_enabled(app, e),
                            checked: crate::menus::entry_checked(app, e),
                        }));
                    }
                }
                MenuNode::Submenu { label, children } => {
                    let mut kids = vec![];
                    rec(app, children, &id, &mut kids);
                    let shown = effectcraft_engine::menus::submenu_label(&app.session, label, &crate::menus::dyn_ctx(&app.ui.workspace, &[]));
                    out.push(NativeNode::Submenu { label: crate::i18n::submenu(app, label, shown), children: kids });
                }
                MenuNode::Item(e) => {
                    let role = match e.command.as_str() {
                        "app.hide" => Some(Role::Hide),
                        "app.hideOthers" => Some(Role::HideOthers),
                        "app.showAll" => Some(Role::ShowAll),
                        _ => None,
                    };
                    if let Some(role) = role {
                        // Services sits just above Hide, as in every Mac app.
                        if role == Role::Hide && !services_added {
                            out.push(NativeNode::Predefined {
                                role: Role::Services,
                                label: if crate::i18n::japanese(app) {
                                    "サービス"
                                } else if crate::i18n::simplified(app) {
                                    "服务"
                                } else if crate::i18n::traditional(app) {
                                    "服務"
                                } else if crate::i18n::ukrainian(app) {
                                    "Служби"
                                } else {
                                    "Services"
                                }
                                .into(),
                                command: String::new(),
                            });
                            out.push(NativeNode::Separator);
                            services_added = true;
                        }
                        out.push(NativeNode::Predefined { role, label: crate::i18n::label(app, &e.command, &e.label).into(), command: e.command.clone() });
                        continue;
                    }
                    let shortcut = crate::menus::entry_shortcut(app, e);
                    out.push(NativeNode::Item(NativeItem {
                        id,
                        label: crate::menus::entry_label(app, e),
                        command: e.command.clone(),
                        params: e.params.clone(),
                        accelerator: shortcut.as_deref().and_then(accelerator),
                        shortcut,
                        enabled: crate::menus::entry_enabled(app, e),
                        checked: crate::menus::entry_checked(app, e),
                    }));
                }
            }
        }
    }
    let mut menus = vec![];
    rec(app, menu_bar(), "m", &mut menus);
    NativeMenu { menus }
}

/// The `muda` accelerator for an EffectCraft shortcut, or `None` when it must stay a plain
/// keyboard shortcut. Only shortcuts with Cmd or Ctrl (and function keys) become menu key
/// equivalents: a single-key shortcut (`V`, `Space`, `U`) or an Alt/Shift-only one would be
/// swallowed by the menu while typing into a text field. Those still work through the
/// window's shortcut dispatcher, as on the other platforms.
pub fn accelerator(shortcut: &str) -> Option<String> {
    let parts: Vec<&str> = match shortcut.strip_suffix("++") {
        Some(head) => head.split('+').chain(std::iter::once("+")).collect(),
        None => shortcut.split('+').collect(),
    };
    let (key, mods) = parts.split_last()?;
    let mut out: Vec<String> = vec![];
    let mut primary = false;
    for m in mods {
        out.push(
            match *m {
                "Cmd" => {
                    primary = true;
                    "Cmd"
                }
                "Ctrl" => {
                    primary = true;
                    "Ctrl"
                }
                "Alt" => "Alt",
                "Shift" => "Shift",
                _ => return None,
            }
            .to_string(),
        );
    }
    let function_key = key.len() >= 2 && key.starts_with('F') && key[1..].parse::<u8>().is_ok_and(|n| (1..=24).contains(&n));
    if !primary && !function_key {
        return None;
    }
    let k = match *key {
        "Num*" => "NumpadMultiply".to_string(),
        "+" => "Equal".to_string(),
        k if k.starts_with("Num") && k.len() == 4 => format!("Numpad{}", &k[3..]),
        "Escape" | "Space" | "Enter" | "Tab" | "Home" | "End" | "PageUp" | "PageDown" | "Delete" | "Backspace" => key.to_string(),
        "ArrowLeft" | "ArrowRight" | "ArrowUp" | "ArrowDown" => key.to_string(),
        k if k.chars().count() == 1 => {
            let c = k.chars().next()?;
            if c.is_ascii_alphanumeric() || "`\\[],-=./;'".contains(c) {
                c.to_ascii_uppercase().to_string()
            } else {
                return None;
            }
        }
        k if function_key => k.to_string(),
        _ => return None,
    };
    out.push(k);
    Some(out.join("+"))
}

/// Cheap per-frame fingerprint of everything menu labels, check marks and enabled state depend
/// on (project revision, selection, history, settings, executed commands, viewer toggles).
pub fn state_key(app: &EffectcraftApp) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let s = &app.session;
    s.revision.hash(&mut h);
    s.prefs_revision.hash(&mut h);
    s.path.hash(&mut h);
    s.active_comp_id().map(|c| c.0).hash(&mut h);
    for l in &s.state.selected_layers {
        l.0.hash(&mut h);
    }
    s.state.selected_keys.len().hash(&mut h);
    s.state.selected_props.len().hash(&mut h);
    s.state.project_selection.len().hash(&mut h);
    s.state.selected_vertices.len().hash(&mut h);
    s.state.clipboard.len().hash(&mut h);
    s.state.key_clipboard.len().hash(&mut h);
    (s.history.undo.len(), s.history.redo.len()).hash(&mut h);
    s.journal.len().hash(&mut h);
    s.journal.last().map(|j| &j.0).hash(&mut h);
    s.prefs.recent_projects.hash(&mut h);
    (&s.prefs.recent_footage, &s.prefs.recent_presets).hash(&mut h);
    s.history.undo.iter().for_each(|(l, _)| l.hash(&mut h));
    s.state.open_comps.hash(&mut h);
    s.active_comp_id().and_then(|c| s.state.views3d.get(&c)).map(|v| format!("{:?}", v.current)).hash(&mut h);
    s.shortcuts().keys.len().hash(&mut h);
    s.keymaps.active.hash(&mut h);
    let v = &app.ui.viewer;
    (v.rulers, v.guides, v.snap_guides, v.lock_guides, v.grid, v.snap_grid, v.show_layer_controls, v.pasteboard).hash(&mut h);
    (app.ui.cache_when_idle, app.session.prefs.preview.active().include_audio, &app.ui.workspace, app.dialog.is_some()).hash(&mut h);
    h.finish()
}

/// Commands that act on a focused text field instead (Edit menu while typing): the egui event to
/// send in their place.
fn text_field_event(command: &str, clipboard: Option<String>) -> Option<Vec<egui::Event>> {
    let key = |k: egui::Key, shift: bool| egui::Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers { shift, ..egui::Modifiers::COMMAND },
    };
    Some(match command {
        "edit.copy" => vec![egui::Event::Copy],
        "edit.cut" => vec![egui::Event::Cut],
        "edit.paste" => vec![egui::Event::Paste(clipboard?)],
        "edit.selectAll" => vec![key(egui::Key::A, false)],
        "edit.undo" => vec![key(egui::Key::Z, false)],
        "edit.redo" => vec![key(egui::Key::Z, true)],
        _ => return None,
    })
}

/// Run a native menu item (by id) the way the in-window menu bar does. While a text field has
/// the keyboard, Edit ▸ Cut/Copy/Paste/Select All/Undo/Redo edit the text instead.
pub fn activate(app: &mut EffectcraftApp, ctx: &egui::Context, menu: &NativeMenu, id: &str) -> Result<Value, String> {
    let item = menu.find(id).ok_or_else(|| format!("no native menu item `{id}`"))?;
    let (command, params) = (item.command.clone(), item.params.clone());
    if ctx.egui_wants_keyboard_input() {
        let clip = if command == "edit.paste" { app.hooks.clipboard_text.as_ref().and_then(|f| f()) } else { None };
        if let Some(events) = text_field_event(&command, clip) {
            app.synthetic.extend(events);
            ctx.request_repaint();
            return Ok(Value::Null);
        }
    }
    let params = if params.is_null() { json!({}) } else { params };
    let r = crate::menus::invoke(app, ctx, &command, params);
    if let Err(e) = &r {
        app.ui.status = e.clone();
    }
    ctx.request_repaint();
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> EffectcraftApp {
        let mut s = effectcraft_engine::Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        EffectcraftApp::new(s)
    }

    #[test]
    fn every_engine_entry_is_in_the_native_menu() {
        let app = app();
        let spec = build(&app);
        fn commands(n: &[NativeNode], out: &mut Vec<(String, String)>) {
            for x in n {
                match x {
                    // Dynamic entries (recent files, History, viewers…) are not tree entries.
                    NativeNode::Item(i) if !i.id.rsplit('.').next().is_some_and(|l| l.starts_with('d') || l == "none") => {
                        out.push((i.command.clone(), i.params.to_string()))
                    }
                    NativeNode::Predefined { command, .. } if !command.is_empty() => out.push((command.clone(), "null".into())),
                    NativeNode::Submenu { children, .. } => commands(children, out),
                    _ => {}
                }
            }
        }
        let mut got = vec![];
        commands(&spec.menus, &mut got);
        let want: Vec<(String, String)> = effectcraft_engine::menus::entries().into_iter().map(|(_, e)| (e.command.clone(), e.params.to_string())).collect();
        assert_eq!(got, want, "native menu must mirror the engine tree entry for entry");
        // Top-level menus in order.
        let tops: Vec<&str> = spec
            .menus
            .iter()
            .filter_map(|n| match n {
                NativeNode::Submenu { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(tops, effectcraft_engine::menus::top_level());
        // Ids are unique.
        let ids: std::collections::BTreeSet<&str> = spec.items().iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids.len(), spec.items().len());
        // The spec serializes (agents can read it).
        let v = serde_json::to_value(&spec).unwrap();
        assert!(v["menus"].as_array().is_some_and(|a| a.len() >= 9));
    }

    /// Issue #191: saved workspaces are in the native Window ▸ Workspace menu, checked while current.
    #[test]
    fn saved_workspaces_are_in_the_native_workspace_menu() {
        let mut app = app();
        let ctx = egui::Context::default();
        crate::menus::invoke(&mut app, &ctx, "window.saveWorkspaceAs", json!({"name": "My Layout"})).unwrap();
        let entry = |app: &EffectcraftApp| build(app).items().into_iter().find(|i| i.label == "My Layout").cloned();
        let it = entry(&app).expect("the saved workspace is listed");
        assert_eq!((it.command.as_str(), &it.params, it.checked), ("window.workspace", &json!({"name": "My Layout"}), Some(true)));
        crate::menus::invoke(&mut app, &ctx, "window.workspace", json!({"name": "Default"})).unwrap();
        assert_eq!(entry(&app).and_then(|i| i.checked), Some(false));
    }

    #[test]
    fn ukrainian_translates_macos_predefined_items() {
        if !cfg!(target_os = "macos") {
            return;
        }
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        app.session.execute("prefs.set", json!({"key": "general.language", "value": "uk"})).unwrap();
        fn labels(nodes: &[NativeNode], out: &mut Vec<String>) {
            for node in nodes {
                match node {
                    NativeNode::Predefined { label, .. } => out.push(label.clone()),
                    NativeNode::Submenu { children, .. } => labels(children, out),
                    _ => {}
                }
            }
        }
        let mut translated = vec![];
        labels(&build(&app).menus, &mut translated);
        assert_eq!(translated, ["Служби", "Приховати EffectCraft", "Приховати решту", "Показати все"]);
    }

    #[test]
    fn hide_items_are_predefined_on_macos() {
        let spec = build(&app());
        let mut roles = vec![];
        fn rec(n: &[NativeNode], out: &mut Vec<Role>) {
            for x in n {
                match x {
                    NativeNode::Predefined { role, .. } => out.push(*role),
                    NativeNode::Submenu { children, .. } => rec(children, out),
                    _ => {}
                }
            }
        }
        rec(&spec.menus, &mut roles);
        if cfg!(target_os = "macos") {
            assert_eq!(roles, vec![Role::Services, Role::Hide, Role::HideOthers, Role::ShowAll]);
            // Quit and About dispatch our commands (clean shutdown, our About dialog).
            assert!(spec.items().iter().any(|i| i.command == "app.quit" && i.accelerator.as_deref() == Some("Cmd+Q")));
            assert!(spec.items().iter().any(|i| i.command == "app.about"));
        } else {
            assert!(roles.is_empty());
        }
    }

    #[test]
    fn accelerators_convert() {
        assert_eq!(accelerator("Cmd+Shift+S").as_deref(), Some("Cmd+Shift+S"));
        assert_eq!(accelerator("Cmd+Alt+;").as_deref(), Some("Cmd+Alt+;"));
        assert_eq!(accelerator("Cmd+`").as_deref(), Some("Cmd+`"));
        assert_eq!(accelerator("Ctrl+Cmd+F").as_deref(), Some("Ctrl+Cmd+F"));
        assert_eq!(accelerator("F3").as_deref(), Some("F3"));
        assert_eq!(accelerator("Cmd+Alt+0").as_deref(), Some("Cmd+Alt+0"));
        // Keys that would swallow typing stay window shortcuts.
        assert_eq!(accelerator("V"), None);
        assert_eq!(accelerator("Space"), None);
        assert_eq!(accelerator("Shift+="), None);
        assert_eq!(accelerator("Alt+["), None);
        assert_eq!(accelerator("Num*"), None);
        // Every menu shortcut either converts or is a single-key/Alt/Shift shortcut.
        let app = app();
        for i in build(&app).items() {
            if let Some(sc) = &i.shortcut {
                let primary = sc.contains("Cmd") || sc.contains("Ctrl");
                if primary {
                    assert!(i.accelerator.is_some(), "{} ({sc}) has no accelerator", i.label);
                }
            }
        }
    }

    #[test]
    fn state_changes_flow_into_the_spec() {
        let mut app = app();
        let k0 = state_key(&app);
        let spec0 = build(&app);
        let undo = |s: &NativeMenu| s.items().into_iter().find(|i| i.command == "edit.undo").map(|i| (i.label.clone(), i.enabled)).unwrap();
        // Select a layer and toggle Lock: the key changes, the Lock entry gets a check mark.
        let first = app.session.active_comp().unwrap().layers[0].id.0;
        app.session.execute("layer.select", json!({"layers": [first]})).unwrap();
        app.session.execute("layer.setSwitch", json!({"switch": "lock"})).unwrap();
        assert_ne!(state_key(&app), k0);
        let spec1 = build(&app);
        // The new undo step joins Edit ▸ History (a dynamic submenu); nothing else is rebuilt.
        let history = |s: &NativeMenu| s.items().into_iter().filter(|i| i.command == "edit.history").count();
        assert_eq!(history(&spec1), history(&spec0) + 1);
        let lock = spec1.items().into_iter().find(|i| i.command == "layer.setSwitch" && i.params["switch"] == "lock").unwrap().checked;
        assert_eq!(lock, Some(true));
        assert_ne!(undo(&spec0), undo(&spec1));
        // A recent project changes the structure (Open Recent grows).
        app.session.prefs.push_recent("/tmp/Recent Test.ecproj");
        let spec2 = build(&app);
        assert_ne!(spec1.structure(), spec2.structure());
        assert!(spec2.items().iter().any(|i| i.command == "file.openRecent" && i.label == "Recent Test.ecproj"));
    }

    #[test]
    fn activation_dispatches_the_menu_command() {
        let mut app = app();
        let ctx = egui::Context::default();
        let spec = build(&app);
        let id = spec.items().into_iter().find(|i| i.command == "layer.newNull").unwrap().id.clone();
        let n = app.session.active_comp().unwrap().layers.len();
        activate(&mut app, &ctx, &spec, &id).unwrap();
        assert_eq!(app.session.active_comp().unwrap().layers.len(), n + 1);
        assert!(activate(&mut app, &ctx, &spec, "m.999").is_err());
        assert!(text_field_event("edit.copy", None).is_some());
        assert!(text_field_event("edit.paste", None).is_none());
        assert!(text_field_event("layer.newNull", None).is_none());
    }
}
