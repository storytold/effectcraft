//! The remote storage dialog: pick a project to open, media to import, or a place to save, on
//! the host's remote mounts ([`crate::Hooks::remote`], the web build's WebDAV mounts). It lists
//! with the Media Browser's `Browser::list`; a folder that is still being read answers
//! [`crate::RemoteInfo::LOADING`] and is asked again on the next frames.
//!
//! Automation ids: `remoteDialog.crumb.<n>`, `remoteDialog.volume.<name>`,
//! `remoteDialog.entry.<name>`, `remoteDialog.filename`, `remoteDialog.accept`,
//! `remoteDialog.cancel`.

use std::time::Duration;

use effectcraft_engine::media_browser::Entry;
use egui::vec2;
use serde_json::{Value, json};

use crate::i18n::tr;
use crate::theme::Tokens;
use crate::{Dialog, EffectcraftApp, RemoteInfo};

/// What the dialog chooses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RemoteMode {
    /// One project file to open.
    #[default]
    OpenProject,
    /// One or more media files to import.
    Import,
    /// A folder and a project file name (`.ecproj` is enforced).
    Save,
}

/// Dialog state while it is open.
#[derive(Clone, Debug, Default)]
pub struct RemoteDialog {
    mode: RemoteMode,
    /// The folder shown; empty shows the mounts.
    dir: String,
    selected: Vec<String>,
    name: String,
}

/// `name` in the folder `dir`.
fn join(dir: &str, name: &str) -> String {
    format!("{}/{name}", dir.trim_end_matches('/'))
}

/// The path to save to: `name` in `dir`, with `.ecproj` added. Refuses names that are empty, `.`,
/// `..`, or hold a separator or control character, so a typed name never leaves the folder.
pub fn save_target(dir: &str, name: &str) -> Result<String, String> {
    let name = name.trim();
    if dir.is_empty() {
        return Err(tr("Choose a folder first").to_string());
    }
    if name.is_empty() || name == "." || name == ".." || name.chars().any(|c| c == '/' || c == '\\' || c.is_control()) {
        return Err(tr("Enter a valid file name").to_string());
    }
    let name = if name.to_ascii_lowercase().ends_with(".ecproj") { name.to_string() } else { format!("{name}.ecproj") };
    Ok(join(dir, &name))
}

/// Open the remote dialog in `mode` with `name` in the file field.
fn open(app: &mut EffectcraftApp, mode: RemoteMode, name: String) -> Result<Value, String> {
    let Some(info) = app.hooks.remote.as_ref() else { return Err("remote storage is not configured".into()) };
    let dir = if let [(_, only)] = info.mounts.as_slice() { only.clone() } else { String::new() };
    app.dialog_state.remote = RemoteDialog { mode, dir, selected: Vec::new(), name };
    app.dialog = Some(Dialog::Remote);
    Ok(json!({"dialog": "remote"}))
}

/// File ▸ Open Remote Project…, Import from Remote… and Save As Remote…: the commands that show the dialog.
pub(crate) fn command(app: &mut EffectcraftApp, id: &str, _params: &Value) -> Option<Result<Value, String>> {
    match id {
        "file.openRemote" => Some(open(app, RemoteMode::OpenProject, String::new())),
        "file.importRemote" => Some(open(app, RemoteMode::Import, String::new())),
        "file.saveAsRemote" => {
            let name = super::unsaved::project_name(app);
            Some(open(app, RemoteMode::Save, name))
        }
        _ => None,
    }
}

/// The breadcrumb trail of `dir`: (label, path) from its mount down.
fn crumbs(dir: &str, mounts: &[(String, String)]) -> Vec<(String, String)> {
    let Some((name, path)) = mounts.iter().find(|(_, p)| dir == p || dir.strip_prefix(p.as_str()).is_some_and(|r| r.starts_with('/'))) else {
        return Vec::new();
    };
    let mut out = vec![(name.clone(), path.clone())];
    let mut at = path.clone();
    for seg in dir.strip_prefix(path.as_str()).unwrap_or("").split('/').filter(|s| !s.is_empty()) {
        at = join(&at, seg);
        out.push((seg.to_string(), at.clone()));
    }
    out
}

/// Does the dialog show `e` in `mode`?
fn visible(mode: &RemoteMode, e: &Entry) -> bool {
    match mode {
        _ if e.is_dir => true,
        RemoteMode::Import => e.kind.is_some() && e.kind != Some("project"),
        _ => e.kind == Some("project"),
    }
}

/// Draw the dialog (called with the other dialogs while it is open).
pub fn show(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let mut st = std::mem::take(&mut app.dialog_state.remote);
    let mounts = app.hooks.remote.as_ref().map(|r| r.mounts.clone()).unwrap_or_default();
    let listing = (!st.dir.is_empty()).then(|| match app.session.media_browser() {
        Some(b) => b.list(&st.dir, false),
        None => Err("the Media Browser is not available".to_string()),
    });
    let multi = st.mode == RemoteMode::Import;
    let (mut go, mut accept, mut cancel) = (None::<String>, false, false);
    let (title, action) = match st.mode {
        RemoteMode::OpenProject => ("Open Remote Project", "Open"),
        RemoteMode::Import => ("Import from Remote", "Import"),
        RemoteMode::Save => ("Save to Remote", "Save"),
    };
    super::dialogs::modal(ctx, title, vec2(560.0, 440.0), t, |ui| {
        ui.horizontal_wrapped(|ui| {
            let root = ui.link(tr("Remote"));
            app.auto.add("remoteDialog.crumb.0", root.rect, "Remote");
            if root.clicked() {
                go = Some(String::new());
            }
            for (i, (label, path)) in crumbs(&st.dir, &mounts).into_iter().enumerate() {
                ui.label("›");
                let c = ui.link(&label);
                app.auto.add(&format!("remoteDialog.crumb.{}", i + 1), c.rect, &label);
                if c.clicked() {
                    go = Some(path);
                }
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().max_height(300.0).auto_shrink([false, false]).show(ui, |ui| match &listing {
            None => {
                for (name, path) in &mounts {
                    let r = ui.selectable_label(false, name);
                    app.auto.add(&format!("remoteDialog.volume.{name}"), r.rect, name);
                    if r.clicked() {
                        go = Some(path.clone());
                    }
                }
                if mounts.is_empty() {
                    ui.label(tr("No remote storage"));
                }
            }
            Some(Ok(entries)) => {
                let shown: Vec<&Entry> = entries.iter().filter(|e| visible(&st.mode, e)).collect();
                for e in &shown {
                    let label = if e.is_dir { format!("{}/", e.name) } else { e.name.clone() };
                    let r = ui.selectable_label(st.selected.contains(&e.path), label);
                    app.auto.add(&format!("remoteDialog.entry.{}", e.name), r.rect, &e.name);
                    if r.clicked() && e.is_dir {
                        go = Some(e.path.clone());
                    } else if r.clicked() {
                        if multi {
                            match st.selected.iter().position(|p| *p == e.path) {
                                Some(i) => drop(st.selected.remove(i)),
                                None => st.selected.push(e.path.clone()),
                            }
                        } else {
                            st.selected = vec![e.path.clone()];
                            if st.mode == RemoteMode::Save {
                                st.name = e.name.clone();
                            }
                        }
                    }
                    if r.double_clicked() && !e.is_dir && !multi && st.mode != RemoteMode::Save {
                        accept = true;
                    }
                }
                if shown.is_empty() {
                    ui.label(tr("Nothing here"));
                }
            }
            Some(Err(e)) if e == RemoteInfo::LOADING => {
                ui.label(tr("Loading…"));
                ctx.request_repaint_after(Duration::from_millis(100));
            }
            Some(Err(e)) => {
                ui.label(egui::RichText::new(e).color(t.text_dim));
            }
        });
        ui.separator();
        let saving = st.mode == RemoteMode::Save;
        let mut ready = saving || !st.selected.is_empty();
        if saving {
            let f = ui.add(egui::TextEdit::singleline(&mut st.name).desired_width(f32::INFINITY));
            app.auto.add("remoteDialog.filename", f.rect, &st.name);
            if let Err(e) = save_target(&st.dir, &st.name) {
                ready = false;
                ui.label(egui::RichText::new(e).size(11.5).color(t.text_dim));
            }
        }
        ui.add_space(8.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let b = ui.add_enabled(ready, egui::Button::new(egui::RichText::new(tr(action)).color(egui::Color32::WHITE)).fill(t.accent));
            app.auto.add("remoteDialog.accept", b.rect, action);
            accept |= b.clicked();
            let b = ui.button(tr("Cancel"));
            app.auto.add("remoteDialog.cancel", b.rect, "Cancel");
            cancel |= b.clicked();
        });
    });
    if let Some(d) = go {
        st.dir = d;
        st.selected.clear();
    }
    if cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.dialog = None;
        return;
    }
    if !accept {
        app.dialog_state.remote = st;
        return;
    }
    let paths = if st.mode == RemoteMode::Save {
        match save_target(&st.dir, &st.name) {
            Ok(p) => vec![p],
            Err(e) => {
                app.ui.status = e;
                app.dialog_state.remote = st;
                return;
            }
        }
    } else {
        st.selected.clone()
    };
    app.dialog = None;
    let (id, params) = match st.mode {
        // `file.open` and `mediaBrowser.import` carry the unsaved-changes prompt and the host's fetch.
        RemoteMode::OpenProject => ("file.open", json!({"path": paths.first()})),
        RemoteMode::Import => ("mediaBrowser.import", json!({"paths": paths})),
        RemoteMode::Save => ("file.saveAs", json!({"path": paths.first()})),
    };
    if let Err(e) = crate::menus::invoke(app, ctx, id, params) {
        app.ui.status = e;
    }
}
