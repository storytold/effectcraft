//! macOS: the menu bar in the system menu bar (`NSMenu` through `muda`), built from
//! [`effectcraft_ui_egui::native_menu`]'s spec of the engine's After Effects menu tree.
//!
//! Each frame [`NativeBar::update`] runs menu activations (the same command ids as the in-window
//! bar), and when [`native_menu::state_key`] or whether a text field has the keyboard changed it
//! rebuilds the spec and pushes labels, enabled and check state into the existing items; a
//! structural change (Open Recent, a new shortcut preset) rebuilds the menu. Settings ▸
//! Appearance ▸ Use In-Window Menu Bar on macOS swaps back to the in-window bar.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, channel};

use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::native_menu::{self, NativeMenu, NativeNode, Role};
use muda::{CheckMenuItem, Menu, MenuEvent, PredefinedMenuItem, Submenu};

/// The `muda` accelerator for a spec accelerator string.
pub fn parse_accelerator(s: &str) -> Option<muda::accelerator::Accelerator> {
    s.parse().ok()
}

pub struct NativeBar {
    menu: Option<Menu>,
    items: HashMap<String, CheckMenuItem>,
    spec: NativeMenu,
    structure: u64,
    state: Option<(u64, bool)>,
    events: Receiver<MenuEvent>,
    /// The in-window-bar setting last applied (None: not yet).
    in_window: Option<bool>,
}

impl NativeBar {
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let _ = tx.send(e);
            ctx.request_repaint();
        }));
        NativeBar { menu: None, items: HashMap::new(), spec: NativeMenu::default(), structure: 0, state: None, events: rx, in_window: None }
    }

    /// Run activations and keep the native menu in step with the app (call once per frame).
    pub fn update(&mut self, app: &mut EffectcraftApp, ctx: &egui::Context) {
        let in_window = app.session.prefs.appearance.in_window_menu_bar_mac;
        if self.in_window != Some(in_window) {
            self.in_window = Some(in_window);
            app.ui.show_menu_bar = in_window;
            if in_window {
                if let Some(m) = self.menu.take() {
                    m.remove_for_nsapp();
                }
                self.items.clear();
            } else {
                self.state = None;
                self.structure = 0;
            }
        }
        while let Ok(e) = self.events.try_recv() {
            if let Err(err) = native_menu::activate(app, ctx, &self.spec, &e.id.0) {
                log::warn!("menu: {err}");
            }
            // Check items toggle themselves when clicked: resync with the app.
            self.state = None;
        }
        if in_window {
            return;
        }
        // While a text field has the keyboard, the Edit items it takes stay enabled.
        let key = (native_menu::state_key(app), native_menu::text_field_window(ctx).is_some());
        if self.state == Some(key) && self.menu.is_some() {
            return;
        }
        self.state = Some(key);
        let spec = native_menu::build_now(app, ctx);
        let structure = spec.structure();
        if self.menu.is_none() || structure != self.structure {
            self.install(&spec);
            self.structure = structure;
        } else {
            self.refresh(&spec);
        }
        self.spec = spec;
    }

    fn install(&mut self, spec: &NativeMenu) {
        if let Some(m) = self.menu.take() {
            m.remove_for_nsapp();
        }
        self.items.clear();
        let menu = Menu::new();
        for node in &spec.menus {
            if let NativeNode::Submenu { label, children } = node {
                let sub = Submenu::new(label, true);
                self.fill(&sub, children);
                match label.as_str() {
                    "Window" | "ウィンドウ" => sub.set_as_windows_menu_for_nsapp(),
                    "Help" | "ヘルプ" => sub.set_as_help_menu_for_nsapp(),
                    _ => {}
                }
                if let Err(e) = menu.append(&sub) {
                    log::warn!("menu {label}: {e}");
                }
            }
        }
        menu.init_for_nsapp();
        self.menu = Some(menu);
    }

    fn fill(&mut self, parent: &Submenu, nodes: &[NativeNode]) {
        for n in nodes {
            let r = match n {
                NativeNode::Separator => parent.append(&PredefinedMenuItem::separator()),
                NativeNode::Predefined { role, label, .. } => {
                    let item = match role {
                        Role::Services => PredefinedMenuItem::services(Some(label)),
                        Role::Hide => PredefinedMenuItem::hide(Some(label)),
                        Role::HideOthers => PredefinedMenuItem::hide_others(Some(label)),
                        Role::ShowAll => PredefinedMenuItem::show_all(Some(label)),
                    };
                    parent.append(&item)
                }
                NativeNode::Submenu { label, children } => {
                    let sub = Submenu::new(label, true);
                    self.fill(&sub, children);
                    parent.append(&sub)
                }
                NativeNode::Item(i) => {
                    let accel = i.accelerator.as_deref().and_then(parse_accelerator);
                    let item = CheckMenuItem::with_id(i.id.clone(), &i.label, i.enabled, i.checked == Some(true), accel);
                    let r = parent.append(&item);
                    self.items.insert(i.id.clone(), item);
                    r
                }
            };
            if let Err(e) = r {
                log::warn!("menu item: {e}");
            }
        }
    }

    /// Push label / enabled / check changes into the existing items.
    fn refresh(&self, spec: &NativeMenu) {
        let old: HashMap<&str, &native_menu::NativeItem> = self.spec.items().into_iter().map(|i| (i.id.as_str(), i)).collect();
        for i in spec.items() {
            let Some(native) = self.items.get(&i.id) else { continue };
            let prev = old.get(i.id.as_str());
            if prev.is_none_or(|p| p.label != i.label) {
                native.set_text(&i.label);
            }
            if prev.is_none_or(|p| p.enabled != i.enabled) {
                native.set_enabled(i.enabled);
            }
            let checked = i.checked == Some(true);
            if native.is_checked() != checked {
                native.set_checked(checked);
            }
        }
    }
}

/// `app.hide` / `app.hideOthers` / `app.showAll` through NSApplication (main thread only).
pub fn app_action(id: &str) -> bool {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    let Some(mtm) = MainThreadMarker::new() else { return false };
    let app = NSApplication::sharedApplication(mtm);
    match id {
        "app.hide" => app.hide(None),
        "app.hideOthers" => app.hideOtherApplications(None),
        "app.showAll" => app.unhideAllApplications(None),
        _ => return false,
    }
    true
}

/// The general pasteboard's text.
pub fn clipboard_text() -> Option<String> {
    use objc2_app_kit::NSPasteboard;
    // NSPasteboardTypeString's value (the UTI), without touching the extern static.
    let ty = objc2_foundation::NSString::from_str("public.utf8-plain-text");
    NSPasteboard::generalPasteboard().stringForType(&ty).map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_accelerator_parses_with_muda() {
        let mut s = effectcraft_engine::Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let app = EffectcraftApp::new(s);
        let spec = native_menu::build(&app);
        let mut n = 0;
        for i in spec.items() {
            if let Some(a) = &i.accelerator {
                assert!(parse_accelerator(a).is_some(), "{} `{a}` does not parse", i.label);
                n += 1;
            }
        }
        assert!(n > 50, "only {n} accelerators");
        // Spot checks against the AE shortcuts.
        let accel = |cmd: &str| spec.items().into_iter().find(|i| i.command == cmd).and_then(|i| i.accelerator.clone());
        assert_eq!(accel("file.saveAs").as_deref(), Some("Cmd+Shift+S"));
        assert_eq!(accel("comp.new").as_deref(), Some("Cmd+N"));
        assert_eq!(parse_accelerator("Cmd+Shift+S"), "CmdOrCtrl+Shift+S".parse().ok());
    }
}
