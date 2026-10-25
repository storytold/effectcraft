//! Edit ▸ Keyboard Shortcuts: After Effects' visual shortcut editor.
//!
//! Top: the preset menu (EffectCraft Default = After Effects' defaults, read-only; custom presets
//! can be duplicated, renamed, deleted, imported and exported) and the search field. Middle: an
//! on-screen keyboard for the toggled (or held) modifiers, keys coloured by what they run in
//! that combination: purple for application-wide commands, green for panel-specific ones, both
//! when a key has each. Clicking a key lists everything bound to it. Bottom: every bindable
//! command with its shortcuts; select one, then record keys (press them in the shortcut field)
//! or assign the key selected on the keyboard. Conflicts are shown before assigning.
//!
//! All changes go through the `shortcuts.*` engine commands, so agents can do the same.
//! Automation ids: `shortcuts.preset`, `shortcuts.search`, `shortcuts.mod.<ctrl|cmd|alt|shift>`,
//! `shortcuts.key.<Key>`, `shortcuts.row.<n>`, `shortcuts.record`, `shortcuts.assign`,
//! `shortcuts.removeKey.<n>`, `shortcuts.resetCommand`, `shortcuts.duplicate`,
//! `shortcuts.rename`, `shortcuts.delete` (asks first: `dialog.deletePreset.delete`,
//! `dialog.deletePreset.cancel`), `shortcuts.import`, `shortcuts.export`,
//! `shortcuts.resetPreset`, `shortcuts.close`.

use crate::i18n::{tr, tr_args};
use effectcraft_engine::shortcuts::{APP_SCOPE, Bindable, DEFAULT_PRESET, normalize};
use egui::{Align2, Color32, Rect, RichText, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::{Value, json};

use crate::theme::Tokens;
use crate::{Dialog, EffectcraftApp};

/// Editor state (in `DialogState`).
#[derive(Clone, Debug, Default)]
pub struct EditorState {
    pub query: String,
    /// Binding key of the selected command.
    pub selected: Option<String>,
    /// Keyboard key selected on the on-screen keyboard (shortcut key name).
    pub key: Option<String>,
    /// Modifier toggles: Ctrl, Cmd, Alt, Shift.
    pub mods: [bool; 4],
    /// Recording a shortcut from the keyboard.
    pub recording: bool,
    /// Recorded (or typed) shortcut waiting to be assigned.
    pub pending: String,
    /// Rename field for the active preset.
    pub rename: Option<String>,
    /// The preset the Delete Preset prompt asks about.
    pub confirm_delete: Option<String>,
}

const APP_COLOR: Color32 = Color32::from_rgb(0x8e, 0x6c, 0xe8);
const PANEL_COLOR: Color32 = Color32::from_rgb(0x3f, 0xa8, 0x6c);

pub fn open(app: &mut EffectcraftApp) {
    app.dialog_state.shortcuts = EditorState::default();
    app.dialog = Some(Dialog::Shortcuts);
}

/// `true` while the editor records keys (the dispatcher must not close the dialog on Escape).
pub fn recording(app: &EffectcraftApp) -> bool {
    app.dialog == Some(Dialog::Shortcuts) && app.dialog_state.shortcuts.recording
}

/// `true` while Delete Preset asks over the editor (Escape answers it, not the editor).
pub fn confirming(app: &EffectcraftApp) -> bool {
    app.dialog == Some(Dialog::Shortcuts) && app.dialog_state.shortcuts.confirm_delete.is_some()
}

/// Shortcut text of a key event (`Cmd+Shift+K`).
pub fn combo_text(m: egui::Modifiers, k: egui::Key) -> Option<String> {
    use egui::Key::*;
    let key = match k {
        Semicolon => ";",
        Quote => "'",
        Comma => ",",
        Period => ".",
        Slash => "/",
        Backslash => "\\",
        Equals => "=",
        Minus => "-",
        Backtick => "`",
        OpenBracket => "[",
        CloseBracket => "]",
        Plus => "+",
        Colon => ";",
        _ => k.name(),
    };
    let mut s = String::new();
    if cfg!(target_os = "macos") {
        if m.ctrl {
            s.push_str("Ctrl+");
        }
        if m.mac_cmd || m.command {
            s.push_str("Cmd+");
        }
    } else if m.ctrl || m.command {
        s.push_str("Cmd+");
    }
    if m.alt {
        s.push_str("Alt+");
    }
    if m.shift {
        s.push_str("Shift+");
    }
    if key == "+" {
        // `Cmd++` names the plus key.
        return normalize(&format!("{s}+"));
    }
    s.push_str(key);
    normalize(&s)
}

/// Split normalized shortcut text into (ctrl, cmd, alt, shift) and the key.
fn split(sc: &str) -> ([bool; 4], String) {
    let (head, key) = match sc.strip_suffix("++") {
        Some(h) => (h.to_string(), "+".to_string()),
        None if sc == "+" => (String::new(), "+".into()),
        None => match sc.rsplit_once('+') {
            Some((h, k)) => (h.to_string(), k.to_string()),
            None => (String::new(), sc.to_string()),
        },
    };
    let has = |m: &str| head.split('+').any(|p| p == m);
    ([has("Ctrl"), has("Cmd"), has("Alt"), has("Shift")], key)
}

fn mods_text(m: [bool; 4]) -> String {
    let mut s = String::new();
    for (on, n) in m.iter().zip(["Ctrl+", "Cmd+", "Alt+", "Shift+"]) {
        if *on {
            s.push_str(n);
        }
    }
    s
}

/// The on-screen keyboard: rows of (label, key name, width in key units).
fn keyboard_rows() -> Vec<Vec<(&'static str, &'static str, f32)>> {
    let mut rows: Vec<Vec<(&str, &str, f32)>> = vec![];
    rows.push(vec![
        ("Esc", "Escape", 1.0),
        ("", "", 0.5),
        ("F1", "F1", 1.0),
        ("F2", "F2", 1.0),
        ("F3", "F3", 1.0),
        ("F4", "F4", 1.0),
        ("", "", 0.3),
        ("F5", "F5", 1.0),
        ("F6", "F6", 1.0),
        ("F7", "F7", 1.0),
        ("F8", "F8", 1.0),
        ("", "", 0.3),
        ("F9", "F9", 1.0),
        ("F10", "F10", 1.0),
        ("F11", "F11", 1.0),
        ("F12", "F12", 1.0),
    ]);
    let mut r: Vec<(&str, &str, f32)> = vec![("`", "`", 1.0)];
    for d in ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"] {
        r.push((d, d, 1.0));
    }
    r.extend([("-", "-", 1.0), ("=", "=", 1.0), ("Delete", "Backspace", 1.6)]);
    rows.push(r);
    let mut r: Vec<(&str, &str, f32)> = vec![("Tab", "Tab", 1.5)];
    for c in ["Q", "W", "E", "R", "T", "Y", "U", "I", "O", "P"] {
        r.push((c, c, 1.0));
    }
    r.extend([("[", "[", 1.0), ("]", "]", 1.0), ("\\", "\\", 1.1)]);
    rows.push(r);
    let mut r: Vec<(&str, &str, f32)> = vec![("Caps", "", 1.8)];
    for c in ["A", "S", "D", "F", "G", "H", "J", "K", "L"] {
        r.push((c, c, 1.0));
    }
    r.extend([(";", ";", 1.0), ("'", "'", 1.0), ("Return", "Enter", 1.8)]);
    rows.push(r);
    let mut r: Vec<(&str, &str, f32)> = vec![("Shift", "#shift", 2.3)];
    for c in ["Z", "X", "C", "V", "B", "N", "M"] {
        r.push((c, c, 1.0));
    }
    r.extend([(",", ",", 1.0), (".", ".", 1.0), ("/", "/", 1.0), ("Shift", "#shift", 2.3)]);
    rows.push(r);
    let (cmd, alt) = if cfg!(target_os = "macos") { ("⌘ Cmd", "⌥ Opt") } else { ("Ctrl", "Alt") };
    rows.push(vec![
        ("Ctrl", if cfg!(target_os = "macos") { "#ctrl" } else { "#cmd" }, 1.4),
        (alt, "#alt", 1.4),
        (cmd, "#cmd", 1.6),
        ("Space", "Space", 6.0),
        (cmd, "#cmd", 1.6),
        (alt, "#alt", 1.4),
    ]);
    rows
}

/// Navigation and numeric keypad block: (label, key name, column, row).
const SIDE_KEYS: &[(&str, &str, f32, f32)] = &[
    ("Home", "Home", 0.0, 1.0),
    ("PgUp", "PageUp", 1.0, 1.0),
    ("Del", "Delete", 0.0, 2.0),
    ("End", "End", 1.0, 2.0),
    ("PgDn", "PageDown", 2.0, 2.0),
    ("↑", "ArrowUp", 1.0, 4.0),
    ("←", "ArrowLeft", 0.0, 5.0),
    ("↓", "ArrowDown", 1.0, 5.0),
    ("→", "ArrowRight", 2.0, 5.0),
];

pub fn show(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let mut st = std::mem::take(&mut app.dialog_state.shortcuts);
    let mut run: Vec<(String, Value)> = vec![];
    let mut close = false;
    // Record keys while the shortcut field listens.
    if st.recording {
        let got = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                _ => None,
            })
        });
        // The shortcut field must not also type the key it just recorded (Alt+R came out as `rAlt+R`).
        ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_) | egui::Event::Key { .. })));
        if let Some((k, m)) = got {
            st.recording = false;
            if k != egui::Key::Escape
                && let Some(txt) = combo_text(m, k)
            {
                st.pending = txt;
            }
        }
    }
    // Enter is OK, unless it belongs to the rename field, an open preset menu or the delete
    // confirmation (where Enter is not Delete). A recorded Enter never gets here: the recorder
    // took the frame's key events above.
    close |= st.rename.is_none()
        && st.confirm_delete.is_none()
        && !egui::Popup::is_any_open(ctx)
        && ctx.input(|i| i.modifiers.is_none() && i.key_pressed(egui::Key::Enter));
    // Modifiers: toggles, or the ones held on the keyboard.
    let held = ctx.input(|i| i.modifiers);
    let held = [held.ctrl && cfg!(target_os = "macos"), held.command, held.alt, held.shift];
    let mods = if held.iter().any(|m| *m) && !st.recording { held } else { st.mods };

    let table = app.session.shortcuts().clone();
    let presets = app.session.keymaps.names();
    let active = app.session.keymaps.active.clone();
    // key name → (app?, panel?) for the current modifiers.
    let mut keymap: std::collections::HashMap<String, (bool, bool)> = Default::default();
    for (sc, b) in table.bindings() {
        let (m, k) = split(sc);
        if m == mods {
            let e = keymap.entry(k).or_default();
            if b.scope == APP_SCOPE {
                e.0 = true;
            } else {
                e.1 = true;
            }
        }
    }

    // No outer scroll area: the command list takes the height left over, so OK stays in view.
    super::dialogs::modal_fit(ctx, "Keyboard Shortcuts", vec2(1000.0, 760.0), t, |ui, height| {
        let top = ui.cursor().top();
        // ---- preset bar
        ui.horizontal(|ui| {
            ui.label(tr("Preset:"));
            let r = egui::ComboBox::from_id_salt("sc-preset").selected_text(&active).width(220.0).show_ui(ui, |ui| {
                for n in &presets {
                    if ui.selectable_label(*n == active, n).clicked() {
                        run.push(("shortcuts.preset".into(), json!({"op": "select", "name": n})));
                    }
                }
            });
            app.auto.add("shortcuts.preset", r.response.rect, "Preset");
            let custom = active != DEFAULT_PRESET;
            let b = ui.button(tr("Duplicate"));
            app.auto.add("shortcuts.duplicate", b.rect, "Duplicate");
            if b.clicked() {
                run.push(("shortcuts.preset".into(), json!({"op": "duplicate", "from": active, "name": format!("{active} copy")})));
            }
            if let Some(name) = &mut st.rename {
                let r = ui.add(egui::TextEdit::singleline(name).desired_width(140.0));
                app.auto.add("shortcuts.renameField", r.rect, "New name");
                if ui.button(tr("Rename")).clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                    run.push(("shortcuts.preset".into(), json!({"op": "rename", "name": active, "newName": name})));
                    st.rename = None;
                }
            } else {
                let b = ui.add_enabled(custom, egui::Button::new(tr("Rename")));
                app.auto.add("shortcuts.rename", b.rect, "Rename");
                if b.clicked() {
                    st.rename = Some(active.clone());
                }
            }
            // A deleted preset can't be brought back: Delete asks first (the prompt below).
            let b = ui.add_enabled(custom, egui::Button::new(tr("Delete")));
            app.auto.add("shortcuts.delete", b.rect, "Delete");
            if b.clicked() {
                st.confirm_delete = Some(active.clone());
            }
            let b = ui.add_enabled(custom, egui::Button::new(tr("Reset Preset")));
            app.auto.add("shortcuts.resetPreset", b.rect, "Reset Preset");
            if b.clicked() {
                run.push(("shortcuts.reset".into(), json!({})));
            }
            let b = ui.button(tr("Import..."));
            app.auto.add("shortcuts.import", b.rect, "Import");
            if b.clicked()
                && let Some(p) = app.hooks.pick_files.as_ref().and_then(|f| f(&["json"]).into_iter().next())
            {
                run.push(("shortcuts.import".into(), json!({"path": p})));
            }
            let b = ui.button(tr("Export..."));
            app.auto.add("shortcuts.export", b.rect, "Export");
            if b.clicked()
                && let Some(p) = app.hooks.pick_save_file.as_ref().and_then(|f| f(&format!("{active}.json"), "json"))
            {
                run.push(("shortcuts.export".into(), json!({"preset": active, "path": p})));
            }
        });
        ui.add_space(6.0);
        // ---- modifiers + legend
        ui.horizontal(|ui| {
            ui.label(tr("Modifiers:"));
            let names = if cfg!(target_os = "macos") { ["⌃ Ctrl", "⌘ Cmd", "⌥ Opt", "⇧ Shift"] } else { ["", "Ctrl", "Alt", "Shift"] };
            for (i, (n, id)) in names.iter().zip(["ctrl", "cmd", "alt", "shift"]).enumerate() {
                if n.is_empty() {
                    continue;
                }
                let r = ui.add(egui::Button::selectable(mods[i], *n));
                app.auto.add(&format!("shortcuts.mod.{id}"), r.rect, n);
                if r.clicked() {
                    st.mods[i] = !st.mods[i];
                }
            }
            ui.add_space(20.0);
            for (c, l) in [(APP_COLOR, "Application-wide"), (PANEL_COLOR, "Panel-specific")] {
                let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                ui.painter().rect_filled(r, 2.0, c);
                ui.label(RichText::new(l).color(t.text_dim));
            }
        });
        ui.add_space(6.0);
        // ---- keyboard
        let unit = 40.0;
        let gap = 3.0;
        let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), unit * 6.0 + gap * 6.0 + 4.0), Sense::hover());
        let p = ui.painter_at(area);
        let draw_key = |app: &mut EffectcraftApp, r: Rect, label: &str, name: &str, st: &mut EditorState| {
            let mod_idx = match name {
                "#ctrl" => Some(0),
                "#cmd" => Some(1),
                "#alt" => Some(2),
                "#shift" => Some(3),
                _ => None,
            };
            let (a, pn) = keymap.get(name).copied().unwrap_or((false, false));
            let fill = match mod_idx {
                Some(i) if mods[i] => t.accent,
                _ => t.field_bg,
            };
            p.rect_filled(r, 4.0, fill);
            if a && pn {
                let half = Rect::from_min_max(r.min, pos2(r.center().x, r.max.y));
                p.rect_filled(half, 4.0, APP_COLOR);
                p.rect_filled(Rect::from_min_max(pos2(r.center().x, r.min.y), r.max), 4.0, PANEL_COLOR);
            } else if a {
                p.rect_filled(r, 4.0, APP_COLOR);
            } else if pn {
                p.rect_filled(r, 4.0, PANEL_COLOR);
            }
            let sel = st.key.as_deref() == Some(name) && !name.is_empty();
            p.rect_stroke(r, 4.0, Stroke::new(if sel { 2.0 } else { 1.0 }, if sel { Color32::WHITE } else { t.field_border }), StrokeKind::Inside);
            p.text(
                r.left_top() + vec2(5.0, 4.0),
                Align2::LEFT_TOP,
                label,
                Tokens::ui(11.0),
                if a || pn || mod_idx.is_some_and(|i| mods[i]) { Color32::WHITE } else { t.text },
            );
            if name.is_empty() {
                return;
            }
            let id = format!("shortcuts.key.{}", name.trim_start_matches('#'));
            app.auto.add(&id, r, label);
            let resp = ui.interact(r, egui::Id::unique(("sc-key", name, r.min.x as i32)), Sense::click());
            if resp.clicked() {
                if let Some(i) = mod_idx {
                    st.mods[i] = !st.mods[i];
                } else {
                    st.key = Some(name.to_string());
                    st.pending = format!("{}{}", mods_text(mods), name);
                }
            }
            if !name.starts_with('#') {
                let who: Vec<String> =
                    table.bindings().into_iter().filter(|(sc, _)| split(sc) == (mods, name.to_string())).map(|(_, b)| b.label.clone()).collect();
                if !who.is_empty() {
                    resp.on_hover_text(who.join("\n"));
                }
            }
        };
        let mut y = area.min.y + 2.0;
        for row in keyboard_rows() {
            let mut x = area.min.x + 2.0;
            for (label, name, w) in row {
                let r = Rect::from_min_size(pos2(x, y), vec2(unit * w - gap, unit - gap));
                if !(label.is_empty() && name.is_empty()) {
                    draw_key(app, r, label, name, &mut st);
                }
                x += unit * w;
            }
            y += unit + gap;
        }
        let side_x = area.min.x + unit * 16.0;
        for (label, name, c, r) in SIDE_KEYS {
            let rect = Rect::from_min_size(pos2(side_x + c * unit, area.min.y + 2.0 + r * (unit + gap)), vec2(unit - gap, unit - gap));
            draw_key(app, rect, label, name, &mut st);
        }
        // Key details: everything bound to the selected key.
        let info_x = side_x + unit * 3.5;
        let info = Rect::from_min_max(pos2(info_x, area.min.y + 2.0), pos2(area.max.x - 2.0, area.max.y - 2.0));
        p.rect_filled(info, 4.0, t.row);
        let p = ui.painter_at(info.shrink(2.0));
        if let Some(k) = &st.key {
            p.text(info.left_top() + vec2(8.0, 6.0), Align2::LEFT_TOP, tr_args("Key: {}", &[&k]), Tokens::semibold(12.0), t.tab_text_active);
            let mut yy = info.min.y + 26.0;
            let mut list: Vec<(String, String, String)> =
                table.bindings().into_iter().filter(|(sc, _)| split(sc).1 == *k).map(|(sc, b)| (sc.to_string(), b.label.clone(), b.scope.clone())).collect();
            list.sort();
            if list.is_empty() {
                p.text(pos2(info.min.x + 8.0, yy), Align2::LEFT_TOP, tr("Not assigned"), Tokens::ui(11.5), t.text_dim);
            }
            for (sc, label, scope) in list.into_iter().take(9) {
                let col = if scope == APP_SCOPE { APP_COLOR } else { PANEL_COLOR };
                p.text(pos2(info.min.x + 8.0, yy), Align2::LEFT_TOP, crate::menus::shortcut_text(&sc), Tokens::ui(11.5), col);
                p.text(pos2(info.min.x + 78.0, yy), Align2::LEFT_TOP, label, Tokens::ui(11.5), t.text);
                yy += 17.0;
            }
        } else {
            p.text(info.left_top() + vec2(8.0, 6.0), Align2::LEFT_TOP, tr("Click a key to see its commands."), Tokens::ui(11.5), t.text_dim);
        }
        ui.add_space(4.0);
        // ---- search + command list + editor
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut st.query).hint_text(tr("Search commands or shortcuts")).desired_width(360.0));
            crate::widgets::focus_on_open(ui.ctx(), &r, &st.query);
            app.auto.add("shortcuts.search", r.rect, "Search");
        });
        ui.add_space(4.0);
        let q = st.query.to_lowercase();
        let rows: Vec<&Bindable> = table
            .bindables
            .iter()
            .filter(|b| {
                if q.is_empty() {
                    return true;
                }
                let keys = table.keys.get(&b.key).map(|k| k.join(" ")).unwrap_or_default();
                let hay = format!("{} {} {} {}", b.label, b.command, b.path.join(" "), keys).to_lowercase();
                q.split_whitespace().all(|w| hay.contains(w))
            })
            .collect();
        let footer = 6.0 + ui.spacing().interact_size.y + ui.spacing().item_spacing.y * 2.0;
        let list_h = (height - (ui.cursor().top() - top) - footer).max(140.0);
        ui.allocate_ui_with_layout(vec2(ui.available_width(), list_h), egui::Layout::left_to_right(egui::Align::Min), |ui| {
            ui.set_height(list_h);
            // Command list.
            ui.vertical(|ui| {
                ui.set_width(620.0);
                let row_h = 20.0;
                egui::ScrollArea::vertical().id_salt("sc-rows").max_height(list_h - 40.0).auto_shrink([false, false]).show_rows(
                    ui,
                    row_h,
                    rows.len(),
                    |ui, range| {
                        for i in range {
                            let b = rows[i];
                            let keys = table.keys.get(&b.key).cloned().unwrap_or_default();
                            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                            let sel = st.selected.as_deref() == Some(b.key.as_str());
                            if sel {
                                ui.painter().rect_filled(r, 0.0, t.row_selected);
                            } else if i % 2 == 1 {
                                ui.painter().rect_filled(r, 0.0, t.row_alt);
                            }
                            let pp = ui.painter();
                            pp.text(pos2(r.min.x + 6.0, r.center().y), Align2::LEFT_CENTER, &b.label, Tokens::ui(11.5), t.text);
                            pp.text(pos2(r.min.x + 230.0, r.center().y), Align2::LEFT_CENTER, b.path.join(" > "), Tokens::ui(10.5), t.text_faint);
                            let ks: Vec<String> = keys.iter().map(|k| crate::menus::shortcut_text(k)).collect();
                            let col = if b.scope == APP_SCOPE { APP_COLOR } else { PANEL_COLOR };
                            pp.text(
                                pos2(r.max.x - 8.0, r.center().y),
                                Align2::RIGHT_CENTER,
                                ks.join(", "),
                                Tokens::ui(11.5),
                                if keys.is_empty() { t.text_faint } else { col },
                            );
                            app.auto.add(&format!("shortcuts.row.{i}"), r, &b.label);
                            if resp.clicked() {
                                st.selected = Some(b.key.clone());
                                st.pending.clear();
                            }
                        }
                    },
                );
                ui.label(RichText::new(format!("{} of {} commands", rows.len(), table.bindables.len())).color(t.text_faint));
            });
            ui.separator();
            // Selected command editor.
            ui.vertical(|ui| {
                ui.set_width(310.0);
                let Some(b) = st.selected.as_ref().and_then(|k| table.find(k)).cloned() else {
                    ui.label(RichText::new(tr("Select a command to change its shortcut.")).color(t.text_dim));
                    return;
                };
                ui.label(RichText::new(&b.label).font(Tokens::semibold(13.0)).color(t.tab_text_active));
                ui.label(RichText::new(format!("{} • {}", b.command, b.scope)).color(t.text_faint));
                ui.add_space(6.0);
                let keys = table.keys.get(&b.key).cloned().unwrap_or_default();
                if keys.is_empty() {
                    ui.label(RichText::new(tr("No shortcut")).color(t.text_dim));
                }
                for (n, k) in keys.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(crate::menus::shortcut_text(k)).monospace());
                        let r = ui.small_button("Remove");
                        app.auto.add(&format!("shortcuts.removeKey.{n}"), r.rect, "Remove");
                        if r.clicked() {
                            let rest: Vec<&String> = keys.iter().filter(|x| *x != k).collect();
                            run.push(("shortcuts.set".into(), json!({"command": b.command, "params": b.params, "keys": rest})));
                        }
                    });
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let hint = if st.recording { "Press keys…" } else { "Click, then press keys" };
                    let r = ui.add(egui::TextEdit::singleline(&mut st.pending).hint_text(hint).desired_width(170.0));
                    app.auto.add("shortcuts.record", r.rect, "Shortcut");
                    if r.gained_focus() || (r.clicked() && !st.recording) {
                        st.recording = true;
                    }
                    if r.lost_focus() {
                        st.recording = false;
                    }
                    let norm = normalize(&st.pending);
                    let b2 = ui.add_enabled(norm.is_some(), egui::Button::new(tr("Assign")));
                    app.auto.add("shortcuts.assign", b2.rect, "Assign");
                    if b2.clicked()
                        && let Some(k) = norm
                    {
                        let mut all = keys.clone();
                        if !all.contains(&k) {
                            all.push(k);
                        }
                        run.push(("shortcuts.set".into(), json!({"command": b.command, "params": b.params, "keys": all})));
                        st.pending.clear();
                    }
                });
                if let Some(k) = normalize(&st.pending) {
                    let conflicts = table.conflicts(&b, &k);
                    if !conflicts.is_empty() {
                        ui.add_space(4.0);
                        ui.label(RichText::new(tr_args("⚠ {} is used by:", &[&(crate::menus::shortcut_text(&k))])).color(t.warning));
                        for c in conflicts.iter().take(4) {
                            ui.label(RichText::new(format!("   {} ({})", c.label, c.scope)).color(t.warning));
                        }
                    }
                }
                ui.add_space(6.0);
                let r = ui.add_enabled(keys != b.defaults, egui::Button::new(tr("Reset to Default")));
                app.auto.add("shortcuts.resetCommand", r.rect, "Reset to Default");
                if r.clicked() {
                    run.push(("shortcuts.reset".into(), json!({"command": b.command, "params": b.params})));
                }
                if !b.defaults.is_empty() {
                    let d: Vec<String> = b.defaults.iter().map(|k| crate::menus::shortcut_text(k)).collect();
                    ui.label(RichText::new(tr_args("Default: {}", &[&(d.join(", "))])).color(t.text_faint));
                }
                if active == DEFAULT_PRESET {
                    ui.add_space(6.0);
                    ui.label(RichText::new(tr("Changing a shortcut creates a custom preset; the default stays as it is.")).color(t.text_faint).small());
                }
            });
        });
        ui.add_space(6.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.add(egui::Button::new(RichText::new(tr("   OK   ")).color(Color32::WHITE)).fill(t.accent));
            app.auto.add("shortcuts.close", r.rect, "OK");
            close |= r.clicked();
        });
    });
    // Delete Preset, over the editor.
    if let Some(name) = st.confirm_delete.clone() {
        let mut answer: Option<bool> = None;
        super::dialogs::modal(ctx, "Delete Preset", vec2(460.0, 150.0), t, |ui| {
            ui.label(RichText::new(format!("Delete the preset “{name}”?")).color(t.tab_text_active));
            ui.add_space(4.0);
            ui.label(RichText::new(tr("You can't undo this.")).color(t.text_dim).size(11.0));
            ui.add_space(18.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let r = ui.add(egui::Button::new(RichText::new(tr("  Delete  ")).color(Color32::WHITE)).fill(t.danger));
                app.auto.add("dialog.deletePreset.delete", r.rect, "Delete");
                if r.clicked() {
                    answer = Some(true);
                }
                let r = ui.button(tr("Cancel"));
                app.auto.add("dialog.deletePreset.cancel", r.rect, "Cancel");
                if r.clicked() {
                    answer = Some(false);
                }
            });
        });
        if answer.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            answer = Some(false);
        }
        // Cmd+D (Ctrl elsewhere) is Delete.
        if answer.is_none() && super::dialogs::ui_command_key(ctx, egui::Key::D) {
            answer = Some(true);
        }
        if let Some(delete) = answer {
            st.confirm_delete = None;
            if delete {
                run.push(("shortcuts.preset".into(), json!({"op": "delete", "name": name})));
            }
        }
    }
    app.dialog_state.shortcuts = st;
    for (cmd, params) in run {
        if let Err(e) = app.session.execute(&cmd, params) {
            app.ui.status = e.to_string();
        }
    }
    if close {
        app.dialog = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combo_text_round_trips_through_the_parser() {
        let cases = [
            (egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::K),
            (egui::Modifiers::ALT, egui::Key::OpenBracket),
            (egui::Modifiers::NONE, egui::Key::F9),
        ];
        for (m, k) in cases {
            let txt = combo_text(m, k).unwrap();
            let (pm, pk) = crate::menus::parse_shortcut(&txt).unwrap();
            assert_eq!(pk, k, "{txt}");
            assert_eq!((pm.command, pm.alt, pm.shift), (m.command, m.alt, m.shift), "{txt}");
        }
        assert_eq!(split("Cmd+Shift+K"), ([false, true, false, true], "K".into()));
        assert_eq!(split("Cmd++"), ([false, true, false, false], "+".into()));
        // The plus key, with and without modifiers, parses back (typed by hand or recorded).
        for txt in ["+", "Cmd++", "Alt++"] {
            assert_eq!(crate::menus::parse_shortcut(txt).map(|(_, k)| k), Some(egui::Key::Plus), "{txt}");
        }
        assert_eq!(crate::menus::parse_shortcut("Alt+R").map(|(m, k)| (m.alt, k)), Some((true, egui::Key::R)));
        assert_eq!(split("F9"), ([false; 4], "F9".into()));
    }

    fn frame(app: &mut EffectcraftApp, ctx: &egui::Context, events: Vec<egui::Event>) {
        app.auto.begin_frame();
        let mut out = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| crate::panels::dialogs::show(app, ui.ctx()));
        out.textures_delta.clear();
    }

    #[test]
    fn the_ok_button_stays_inside_the_dialog_in_a_small_window() {
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &app.tokens, crate::i18n::language(&app));
        open(&mut app);
        for size in [vec2(1680.0, 1020.0), vec2(1280.0, 720.0)] {
            for _ in 0..3 {
                app.auto.begin_frame();
                let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), size)), ..Default::default() };
                let mut out = ctx.run_ui(input, |ui| crate::panels::dialogs::show(&mut app, ui.ctx()));
                out.textures_delta.clear();
            }
            let [_, y, _, h] = app.auto.find("shortcuts.close").expect("OK button").rect;
            // The window less the modal's margin and its frame padding.
            assert!(y + h <= size.y - 24.0 - 18.0 + 0.5, "OK ends at {} in a {size:?} window", y + h);
        }
    }

    #[test]
    fn enter_is_ok_except_while_recording_or_renaming() {
        let enter = || egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() };
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &app.tokens, crate::i18n::language(&app));
        // Recording: Enter is the recorded key.
        open(&mut app);
        app.dialog_state.shortcuts.recording = true;
        frame(&mut app, &ctx, vec![enter()]);
        assert_eq!(app.dialog, Some(Dialog::Shortcuts));
        assert_eq!(app.dialog_state.shortcuts.pending, "Enter");
        // Renaming a preset: Enter belongs to the name field.
        app.dialog_state.shortcuts.rename = Some("Mine".into());
        frame(&mut app, &ctx, vec![enter()]);
        assert_eq!(app.dialog, Some(Dialog::Shortcuts));
        // Otherwise it closes the editor, also with the search field focused.
        app.dialog_state.shortcuts.rename = None;
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![enter()]);
        assert_eq!(app.dialog, None);
    }

    #[test]
    fn deleting_a_preset_asks_first() {
        fn click(app: &mut EffectcraftApp, ctx: &egui::Context, id: &str) {
            let [x, y, w, h] = app.auto.find(id).unwrap_or_else(|| panic!("{id} is not on screen")).rect;
            let pos = pos2(x + w / 2.0, y + h / 2.0);
            let button = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
            for event in [egui::Event::PointerMoved(pos), button(true), button(false)] {
                frame(app, ctx, vec![event]);
            }
            // Two more: the automation ids of the clicked frame age out.
            frame(app, ctx, vec![]);
            frame(app, ctx, vec![]);
        }
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &app.tokens, crate::i18n::language(&app));
        app.session.execute("shortcuts.preset", json!({"op": "duplicate", "from": DEFAULT_PRESET, "name": "Mine"})).unwrap();
        app.session.execute("shortcuts.preset", json!({"op": "select", "name": "Mine"})).unwrap();
        let has = |app: &EffectcraftApp| app.session.keymaps.names().iter().any(|n| n == "Mine");
        open(&mut app);
        for _ in 0..3 {
            frame(&mut app, &ctx, vec![]);
        }
        assert!(app.auto.find("dialog.deletePreset.delete").is_none());
        click(&mut app, &ctx, "shortcuts.delete");
        assert!(has(&app) && confirming(&app), "the click only asks");
        click(&mut app, &ctx, "dialog.deletePreset.cancel");
        assert!(has(&app) && app.auto.find("dialog.deletePreset.delete").is_none(), "Cancel keeps the preset");
        assert_eq!(app.dialog, Some(Dialog::Shortcuts), "the editor stays open");
        click(&mut app, &ctx, "shortcuts.delete");
        click(&mut app, &ctx, "dialog.deletePreset.delete");
        assert!(!has(&app), "confirmed: the preset is gone");
        assert!(!confirming(&app) && app.dialog == Some(Dialog::Shortcuts));
        // Cmd+D is Delete; Enter is not.
        app.session.execute("shortcuts.preset", json!({"op": "duplicate", "from": DEFAULT_PRESET, "name": "Mine"})).unwrap();
        app.session.execute("shortcuts.preset", json!({"op": "select", "name": "Mine"})).unwrap();
        app.dialog_state.shortcuts.confirm_delete = Some("Mine".into());
        crate::panels::dialogs::press_key(&mut app, &ctx, egui::Key::Enter, egui::Modifiers::NONE);
        assert!(has(&app) && confirming(&app));
        crate::panels::dialogs::press_key(&mut app, &ctx, egui::Key::D, egui::Modifiers::COMMAND);
        assert!(!has(&app) && !confirming(&app));
    }

    #[test]
    fn recording_a_shortcut_does_not_also_type_the_key_into_the_field() {
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &app.tokens, crate::i18n::language(&app));
        open(&mut app);
        app.dialog_state.shortcuts.selected = app.session.shortcuts().bindables.first().map(|b| b.key.clone());
        for _ in 0..3 {
            frame(&mut app, &ctx, vec![]);
        }
        let [x, y, w, h] = app.auto.find("shortcuts.record").expect("shortcut field").rect;
        let at = pos2(x + w / 2.0, y + h / 2.0);
        let click = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(&mut app, &ctx, vec![egui::Event::PointerMoved(at), click(true)]);
        frame(&mut app, &ctx, vec![click(false)]);
        frame(&mut app, &ctx, vec![]);
        assert!(app.dialog_state.shortcuts.recording);
        // Alt+R arrives as the key and as the typed text `r`.
        let key = egui::Event::Key { key: egui::Key::R, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::ALT };
        frame(&mut app, &ctx, vec![key, egui::Event::Text("r".into())]);
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.dialog_state.shortcuts.pending, "Alt+R");
    }
}
