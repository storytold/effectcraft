use super::frontend::run_engine;
use super::invoke::invoke;
use super::{EffectcraftApp, PanelKind, Value, json};

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

/// Whether the `key`+`mods` chord is the current binding of `command` in the active preset
/// (so callers that must match a shortcut themselves — like the viewer's text editor, which
/// holds the keyboard while the shortcut dispatcher is skipped — follow rebinds).
/// `Physical+`-only bindings never match here, exactly as for the app-wide dispatcher.
pub fn chord_matches(session: &effectcraft_engine::Session, command: &str, key: egui::Key, mods: egui::Modifiers) -> bool {
    bindings(session).into_iter().any(|(m, k, id, _)| id == command && k == key && mods_match(m, mods))
}

/// Whether a Deselect All chord matched by [`chord_matches`] may run while a text editor holds
/// the keyboard. Chords carrying Cmd/Ctrl/Alt are command chords (the editor treats them as
/// such for undo and the rest), and function keys never type, so those forward from the editor;
/// a chord that could instead produce a character — a bare or Shift-only letter, digit or
/// punctuation key, as from a pathological bare-letter rebinding — stays with the editor so
/// typing keeps working, the same invariant the app-wide dispatcher honors by skipping
/// focused text fields.
pub fn forwardable_during_text_edit(mods: egui::Modifiers, key: egui::Key) -> bool {
    mods.command || mods.ctrl || mods.alt || (egui::Key::F1..=egui::Key::F35).contains(&key)
}

/// Set (egui temp data) from a plain Spacebar press until its release while a tap would still
/// run the Spacebar shortcut.
/// The system clipboard's text of this frame's paste event, for the Edit ▸ Paste it runs.
pub(super) fn system_paste_id() -> egui::Id {
    egui::Id::unique("menus-system-paste")
}

/// The temp-data id of the flag set by a plain Space press and taken on its release, so a tap
/// runs the Space shortcut while holding Space stays the Hand tool.
fn space_tap_id() -> egui::Id {
    egui::Id::unique("spacebar-tap")
}

/// The modifiers held when this frame's input began: the state the last pass ended with, if it
/// ran this (egui keeps only the state after the frame's events). Records this pass's state.
pub(super) fn modifiers_at_start(ctx: &egui::Context) -> Option<egui::Modifiers> {
    let id = egui::Id::unique("shortcut-modifiers");
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

/// Escape with a dialog open: close the popup/picker first, else cancel the dialog.
fn escape_dialog(app: &mut EffectcraftApp, ctx: &egui::Context) {
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
        app.dialog = None;
    }
}

type KeyEvent = (egui::Key, egui::Modifiers, bool, Option<egui::Key>);

/// The frame's key presses, with clipboard events mapped back to Ctrl+C / X / V.
fn collect_key_events(ctx: &egui::Context, start: Option<egui::Modifiers>, clipboard: bool, project: bool) -> Vec<KeyEvent> {
    ctx.input(|i| {
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
    })
}

/// Numpad * (or `*` typed with Shift+8): Add Marker (#275). egui has no key for it, so it
/// comes as text.
fn numpad_star_marker(app: &mut EffectcraftApp, ctx: &egui::Context) {
    if !ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(t) if t == "*"))) {
        return;
    }
    let bound = app.session.shortcuts().bindings().into_iter().find(|(k, _)| *k == "Num*").map(|(_, b)| (b.command.clone(), b.params.clone()));
    // As in After Effects, it marks the selected layers; only with none selected does it add
    // a composition marker (#976).
    let bound =
        bound.map(
            |(id, params)| {
                if id == "comp.addMarker" && !app.session.state.selected_layers.is_empty() { ("layer.addMarker".to_owned(), params) } else { (id, params) }
            },
        );
    if let Some((id, params)) = bound
        && let Err(e) = invoke(app, ctx, &id, if params.is_null() { json!({}) } else { params })
    {
        app.ui.status = e;
    }
}

fn handle_shortcuts_impl(app: &mut EffectcraftApp, ctx: &egui::Context, tab_only: bool) {
    let start = modifiers_at_start(ctx);
    if !tab_only && app.dialog.is_some() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        escape_dialog(app, ctx);
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
    let mut events = collect_key_events(ctx, start, clipboard, project);
    events.retain(|(key, _, _, _)| (*key == egui::Key::Tab) == tab_only);
    if !tab_only && clipboard && app.dialog.is_none() {
        numpad_star_marker(app, ctx);
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
            // Escape in Settings is Cancel: restore the settings from when it opened.
            if app.dialog == Some(crate::Dialog::Settings)
                && let Some(p) = app.dialog_state.prefs_snapshot.take()
            {
                app.session.prefs = p;
                app.session.prefs_changed();
            }
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
