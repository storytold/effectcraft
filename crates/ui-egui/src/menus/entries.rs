use super::*;

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
    if app.ui.dock.is_floating(p) {
        return app.ui.dock.is_visible(p);
    }
    match app.ui.maximized.filter(|m| app.ui.dock.is_docked(*m)) {
        Some(m) => m == p,
        None => app.ui.dock.is_visible(p),
    }
}

/// The command a menu entry runs when chosen from a menu: Window ▸ `<panel>` closes a panel whose
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
    // The Project-panel enabled rules mirror the invoke redirects, so they stand down for the
    // same reason (see the redirect block): while a text edit is open the menu items must be
    // enabled by what their routed command can do, or the menu grays out commands that work.
    if app.ui.focused == PanelKind::Project && app.session.state.text_edit.is_none() {
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
        // An open text edit stands down: Clear must not delete the comp being typed into (#2227).
        return app.session.state.text_edit.is_none() && app.session.is_enabled("project.delete");
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
