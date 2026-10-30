use super::entries::{entry_checked, entry_enabled, entry_label, entry_shortcut, menu_choice};
use super::invoke::invoke;
use super::shortcuts::shortcut_text;
use super::{EffectcraftApp, MenuEntry, MenuNode, Value, json};

/// The id under which [`menu_bar`] remembers which of its menus was open at the end of the last
/// frame (its title's open look, and the hover that switches between the titles).
fn open_menu_id() -> egui::Id {
    egui::Id::unique("menu-bar-open")
}

/// The id under which [`menu_bar`] remembers a title press still in progress (press-and-browse).
fn browse_menu_id() -> egui::Id {
    egui::Id::unique("menu-bar-browse")
}

/// The id under which [`menu_bar`] remembers where it last saw the pointer (`None`: outside the
/// window).
fn last_pointer_id() -> egui::Id {
    egui::Id::unique("menu-bar-last-pointer")
}

/// Draw the in-window menu bar (the engine's AE menu tree).
///
/// The titles open on the press, not the release (native menu bars): while one is held the menus
/// can be browsed by moving onto them, and releasing over an entry chooses it. Hovering another
/// title switches the open menu to it, and the open one closes on a press outside it or when the
/// window loses focus. The pointer leaving the window — onto the OS title bar, directly above the
/// bar — closes nothing, however long: the platform forwards none of the title bar's clicks to
/// egui (only the cursor leaving), so there is no click to close on and a graze must not drop the
/// menu. The closing press itself still reaches the widget under it, as a click does in After
/// Effects.
pub fn menu_bar(app: &mut EffectcraftApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let mut clicked: Option<(String, Value)> = None;
    let mut nav = crate::menu_keys::Nav::default();
    // The open menu as of the last frame: the titles are drawn before this frame can open or
    // close anything, and what happened last frame is what they must react to.
    let open_prev: Option<usize> = ctx.data(|d| d.get_temp(open_menu_id())).flatten();
    if ctx.data(|d| d.get_temp(browse_menu_id())).unwrap_or(false) {
        nav.set_browsing();
    }
    // Whether the pointer has moved since the last frame. Kept by position rather than egui's
    // `delta`, which is zero for the first position after the pointer comes back into the window:
    // coming back in over another title must switch to it too (#1290).
    let pointer = ctx.pointer_hover_pos();
    let last = ctx.data_mut(|d| std::mem::replace(d.get_temp_mut_or(last_pointer_id(), None), pointer));
    let moved = pointer.is_some() && pointer != last;
    let mut drawn = 0;
    ui.horizontal_centered(|ui| {
        ui.add_space(6.0);
        egui::MenuBar::new().config(egui::containers::menu::MenuConfig::new().style(crate::widgets::menu_style)).ui(ui, |ui| {
            for node in effectcraft_engine::menus::menu_bar() {
                let MenuNode::Submenu { label, children } = node else { continue };
                // The open title takes the open button's look while its menu is up (as in AE).
                let inactive = ui.style().visuals.widgets.inactive;
                if open_prev == Some(drawn) {
                    ui.style_mut().visuals.widgets.inactive = ui.style().visuals.widgets.open;
                }
                let mut response = ui.add(egui::Button::new(crate::i18n::label(app, "", label)));
                ui.style_mut().visuals.widgets.inactive = inactive;
                // A press on a title opens its menu at once (a second press on the open one closes
                // it), and while it is held the menus below are browsable. The keyboard and the
                // accessibility activation report `clicked` without the primary button (the press
                // then happened somewhere else), and toggle the menu. Hovering another title
                // switches to it once the pointer has moved: merely resting there must not take a
                // keyboard-driven menu over.
                // With the button down elsewhere nothing is hovered, so the title under the
                // pointer takes the highlight by hand: the hover-switch to it shows, one frame
                // later like egui's own hover (the open title keeps its open look).
                if moved && response.contains_pointer() && open_prev.is_some_and(|p| p != drawn) {
                    response = response.highlight();
                }
                let pressed = ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary)) && response.hovered();
                let cmd = if pressed {
                    nav.set_browsing();
                    Some(egui::SetOpenCommand::Bool(open_prev != Some(drawn)))
                } else if response.clicked() && !response.clicked_by(egui::PointerButton::Primary) {
                    Some(egui::SetOpenCommand::Toggle)
                } else if moved && response.enabled() && response.contains_pointer() && open_prev.is_some_and(|p| p != drawn) {
                    Some(egui::SetOpenCommand::Bool(true))
                } else {
                    None
                };
                let inner = egui::Popup::menu(&response)
                    .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
                    .style(crate::widgets::menu_style)
                    .open_memory(cmd)
                    .show(|ui| {
                        ui.set_min_width(if label == "Effect" { 200.0 } else { 280.0 });
                        let browsing = nav.browsing();
                        crate::widgets::menu_scroll(ui, browsing, |ui| menu_nodes(app, ui, children, &mut clicked, &mut nav, 0));
                    });
                nav.top(&response);
                // The titles and the open menus: a press inside them is not an outside press.
                nav.cover(response.rect);
                if let Some(inner) = &inner {
                    nav.cover(inner.response.rect);
                }
                app.auto.add(&format!("menu.{label}"), response.rect, label);
                drawn += 1;
            }
        });
    });
    crate::menu_keys::handle(&ctx, &nav);
    // egui leaves the closing to us (`IgnoreClicks`), so the menus close here: at once on a press
    // outside them (the press itself still reaches the widget below) and when the window loses
    // focus. The pointer leaving the window closes nothing: moving onto the OS title bar fires
    // the cursor-left event and the platform forwards none of its clicks to egui, so there is no
    // click to close on — and a graze or a stay must not drop the menu, as in native apps.
    if let Some(top) = nav.open_popup(&ctx) {
        let focus_lost = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::WindowFocused(false))));
        let outside_press = ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !nav.covers(p)));
        if focus_lost || outside_press {
            egui::Popup::close_id(&ctx, top);
        }
    }
    if let Some((id, params)) = clicked.map(|(id, params)| menu_choice(app, id, params))
        && let Err(e) = invoke(app, &ctx, &id, if params.is_null() { json!({}) } else { params })
    {
        app.ui.status = e;
    }
    // What the next frame starts from: the open title, and a title press still in progress.
    let open = nav.open_index(&ctx);
    let browsing = open.is_some() && nav.browsing() && ctx.input(|i| i.pointer.primary_down());
    ctx.data_mut(|d| {
        d.insert_temp(open_menu_id(), open);
        d.insert_temp(browse_menu_id(), browsing);
    });
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
                let (nodes, empty) = effectcraft_engine::menus::dynamic_nodes(&app.session, name, &dyn_ctx(&ws, &saved));
                if nodes.is_empty()
                    && let Some(e) = empty
                {
                    ui.add_enabled(false, egui::Button::new((gutter(false), e)));
                }
                for (i, n) in nodes.iter().enumerate() {
                    // A folder of scripts or panels: a submenu.
                    let MenuNode::Item(e) = n else {
                        menu_nodes(app, ui, std::slice::from_ref(n), clicked, nav, depth);
                        continue;
                    };
                    let tip = match name.as_str() {
                        "recentProjects" => app.session.prefs.recent_projects.get(i).cloned(),
                        "recentFootage" => app.session.prefs.recent_footage.get(i).cloned(),
                        "recentPresets" => app.session.prefs.recent_presets.get(i).cloned(),
                        "recentScripts" => app.session.prefs.recent_scripts.get(i).map(|r| r.text().to_string()),
                        _ => None,
                    };
                    let mut b = egui::Button::new((gutter(entry_checked(app, e) == Some(true)), e.label.as_str()));
                    // File ▸ Scripts entries show the key a script was given.
                    if let Some(s) = entry_shortcut(app, e) {
                        b = b.shortcut_text(shortcut_text(&s));
                    }
                    let r = ui.add_enabled(entry_enabled(app, e), b);
                    let r = match &tip {
                        Some(t) => r.on_hover_text(t),
                        None => r,
                    };
                    let r = browse_hover(nav, r);
                    app.auto.add(&format!("menu.{name}.{i}"), r.rect, &e.label);
                    nav.entry(ui, depth, &r, false);
                    if r.clicked() || browse_pick(nav, ui, &r) {
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
                let browsing = nav.browsing();
                let egui::InnerResponse { inner, response } = ui.menu_button((gutter(false), shown.as_str()), |ui| {
                    ui.set_min_width(if children.len() > 30 { 200.0 } else { 240.0 });
                    crate::widgets::menu_scroll(ui, browsing, |ui| menu_nodes(app, ui, children, clicked, nav, depth + 1));
                });
                let response = browse_hover(nav, response);
                nav.entry(ui, depth, &response, true);
                if inner.is_some() {
                    // `ui.menu_button` drops the popup's own response on the floor: read it back,
                    // so that a press inside this submenu doesn't count as an outside press.
                    let sub = egui::containers::menu::SubMenu::id_from_widget_id(response.id);
                    if let Some(resp) = ui.ctx().read_response(sub) {
                        nav.cover(resp.rect);
                    }
                    nav.opened(depth, &response);
                }
            }
            MenuNode::Item(e) => {
                let r = browse_hover(nav, menu_entry(app, ui, e));
                nav.entry(ui, depth, &r, false);
                if r.clicked() || browse_pick(nav, ui, &r) {
                    *clicked = Some((e.command.clone(), e.params.clone()));
                    ui.close();
                }
            }
        }
    }
}

/// The row `r` highlighted while the bar is mid press-and-browse: with a button down egui reports
/// nothing as hovered — which would leave browsing with no hover feedback at all — so the row
/// under the pointer takes the highlight by hand. It reads as the hover look one frame later,
/// exactly egui's own hover latency; disabled rows ignore it (`Noninteractive`), as in native
/// menus. Context menus use their own [`crate::menu_keys::Nav`], where the bar never browses.
fn browse_hover(nav: &crate::menu_keys::Nav, r: egui::Response) -> egui::Response {
    if nav.browsing() && r.contains_pointer() { r.highlight() } else { r }
}

/// Whether a press-and-browse (a menu-bar title still held down) releases over entry `r`, which
/// then chooses it like a click does. egui reports neither for it: the release is far from the
/// press, so there is no click, and nothing but the pressed widget is hovered while a button is
/// down, so the row has to be tested by the pointer being over it.
fn browse_pick(nav: &crate::menu_keys::Nav, ui: &egui::Ui, r: &egui::Response) -> bool {
    nav.browsing() && r.enabled() && r.contains_pointer() && ui.input(|i| i.pointer.button_released(egui::PointerButton::Primary))
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
