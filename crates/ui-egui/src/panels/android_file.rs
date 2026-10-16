//! Android's in-app file dialogs: Save / Open Project / folder picks without a system picker.
//!
//! Android has no rfd backend, so the blocking hooks stay empty there. When the app-private
//! storage directory is known ([`crate::Hooks::app_files_dir`], resolved by the host from
//! `AndroidApp::internal_data_path`), the commands' dialogs ([`crate::FileDialog`]) are served
//! here instead of failing with "no file dialog available": project saves open in `Projects/`,
//! other saves (frame stills, render output, presets…) in `Exports/`, Open Project lists the
//! saved projects (or browses for one anywhere), folder picks (Settings ▸ Project ▸ auto-save
//! location…) and media *import* browse for them. The Save prompts carry a Location row whose
//! browser also creates folders. Media import is served too, but only through this browser (no
//! Storage Access Framework round-trip), so it lists the same two roots below.
//!
//! The browser browses two roots: the app-private base, and (once "All files access" is
//! granted) the shared internal storage root. A root's own tree can never be escaped: entries are
//! only ever *chosen* from listings, never typed, and "Up" is clamped to the current root's top,
//! so no answer can leave the root the user is browsing. The whole dialog compiles on every
//! platform — a host enables it by setting [`crate::Hooks::app_files_dir`] (the Android host
//! does; desktop hosts leave it `None` and keep their blocking pickers), so the modals and the
//! listing run in the host's own tests.

use std::path::{Path, PathBuf};

/// Project extensions the Open Project prompt lists.
const PROJECT_EXTS: [&str; 2] = ["ecproj", "ecprojx"];

/// Where a save goes when no location was chosen: projects with their own folder, everything
/// else with the exports.
pub fn save_dir(base: &Path, project: bool) -> PathBuf {
    base.join(if project { "Projects" } else { "Exports" })
}

/// A user-typed name reduced to a single file name inside the dialog's folder: surrounding
/// whitespace and any typed directories are dropped so the answer can never escape the base
/// directory. Empty (or separator-only) input falls back to `fallback`.
pub fn sanitize_name(input: &str, fallback: &str) -> String {
    let name = Path::new(input.trim()).file_name().and_then(|n| n.to_str()).unwrap_or("").trim().to_string();
    if name.is_empty() || name == "." || name == ".." { fallback.to_string() } else { name }
}

/// Project saves keep their extension: anything without one gains `.ecproj` (as the desktop
/// project dialog appends it, #293).
pub fn enforce_project_ext(name: &str) -> String {
    let project = Path::new(name).extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("ecproj") || e.eq_ignore_ascii_case("ecprojx"));
    if project { name.to_string() } else { format!("{name}.ecproj") }
}

/// The full save path for a save into `dir` answered with `name`.
pub fn save_path_in(dir: &Path, project: bool, name: &str, fallback: &str) -> PathBuf {
    let mut name = sanitize_name(name, fallback);
    if project {
        name = enforce_project_ext(&name);
    }
    dir.join(name)
}

/// The full save path for a [`crate::FileDialog::Save`] request answered with `name`.
pub fn save_path(base: &Path, project: bool, name: &str, fallback: &str) -> PathBuf {
    save_path_in(&save_dir(base, project), project, name, fallback)
}

/// Saved projects, newest first. Unreadable directories (or entries) are skipped, never an error.
pub fn list_projects(base: &Path) -> Vec<PathBuf> {
    let mut out: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    let Ok(entries) = std::fs::read_dir(base.join("Projects")) else { return Vec::new() };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_project = path.extension().and_then(|e| e.to_str()).is_some_and(|e| PROJECT_EXTS.iter().any(|p| e.eq_ignore_ascii_case(p)));
        if is_project {
            let modified = entry.metadata().and_then(|m| m.modified()).unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            out.push((modified, path));
        }
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.0));
    out.into_iter().map(|(_, p)| p).collect()
}

/// The sub-folders of `dir` for the browser, sorted case-insensitively. Unreadable directories
/// and non-directories are skipped, never an error.
pub fn list_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<PathBuf> = entries.flatten().filter_map(|e| e.path().is_dir().then(|| e.path())).collect();
    out.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default());
    out
}

/// The root the folder browser is currently inside: the app-private base, or the shared
/// internal-storage root once "All files access" is granted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageRoot {
    /// The app-private directory (`AndroidApp::internal_data_path`), always browsable.
    App(std::path::PathBuf),
    /// Shared internal storage (typically `/storage/emulated/0`), browsable only while the
    /// "All files access" grant holds.
    Shared(std::path::PathBuf),
}

impl StorageRoot {
    /// The directory this root starts at.
    pub fn dir(&self) -> &Path {
        match self {
            StorageRoot::App(base) | StorageRoot::Shared(base) => base,
        }
    }

    /// The directory "Up" leads to, clamped to this root's top: the parent while it stays inside
    /// the root, else the root itself (so the browser can never climb out).
    pub fn up_from(&self, dir: &Path) -> PathBuf {
        let root = self.dir();
        match dir.parent() {
            Some(parent) if parent.starts_with(root) && parent != root => parent.to_path_buf(),
            _ => root.to_path_buf(),
        }
    }
}

/// The file names in `dir` whose extension is in `exts` (case-insensitive; an empty `exts`
/// matches every file), sorted case-insensitively. Directories and unreadable entries are
/// skipped, never an error.
pub fn list_files(dir: &Path, exts: &[String]) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let is_file = path.is_file();
            let matches = exts.is_empty() || path.extension().and_then(|x| x.to_str()).is_some_and(|x| exts.iter().any(|e| x.eq_ignore_ascii_case(e)));
            (is_file && matches).then_some(path)
        })
        .collect();
    out.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default());
    out
}

/// Whether `dir` can be listed (it exists and is readable). The browser uses this to tell an
/// unreadable root apart from an empty one and offer the "All files access" grant.
pub fn readable(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok()
}

/// How the picker lists entries: one column of rows (the original), or a grid of icons in one
/// of three sizes — so what a file *is* (its picture) is visible before it is imported. Stored
/// at `general.pickerView` and switched with the picker's own view buttons.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PickView {
    List,
    Small,
    Medium,
    /// Icons large enough for real thumbnails — the default.
    #[default]
    Large,
}

impl PickView {
    /// The stored value at `general.pickerView`; anything unknown reads as the default, so a
    /// hand-edited settings file cannot break the picker.
    pub fn parse(value: &str) -> PickView {
        match value {
            "list" => PickView::List,
            "small" => PickView::Small,
            "medium" => PickView::Medium,
            "large" => PickView::Large,
            _ => PickView::Large,
        }
    }

    /// The value stored at `general.pickerView` (one of the engine's `PICKER_VIEWS`).
    pub fn as_str(self) -> &'static str {
        match self {
            PickView::List => "list",
            PickView::Small => "small",
            PickView::Medium => "medium",
            PickView::Large => "large",
        }
    }

    /// An icon view's tile: width, height and the characters of the name it fits (`List` has
    /// no tiles — its rows size themselves).
    fn tile(self) -> (f32, f32, usize) {
        match self {
            PickView::List => (0.0, 0.0, 0),
            PickView::Small => (84.0, 84.0, 14),
            PickView::Medium => (120.0, 112.0, 18),
            PickView::Large => (152.0, 136.0, 24),
        }
    }
}

/// How many tiles of `tile_w` fit across `width`, never zero (a sliver still lists its
/// entries one per row).
fn grid_cols(width: f32, tile_w: f32) -> usize {
    ((width - 8.0) / tile_w).floor().max(1.0) as usize
}

/// The extension of `path`, upper-cased: what an icon tile says when the file has no picture
/// to show (`WAV`, `JSON`, `TXT`).
fn ext_label(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).map(|e| e.to_uppercase()).filter(|e| !e.is_empty()).unwrap_or_else(|| crate::i18n::tr("File").to_string())
}

/// The view the picker opens in, remembered across dialogs at `general.pickerView`.
fn stored_view(app: &crate::EffectcraftApp) -> PickView {
    match app.session.prefs.get("general.pickerView") {
        Some(v) => PickView::parse(v.as_str().unwrap_or("large")),
        None => PickView::default(),
    }
}

/// Switch the open picker to `view` and remember it for the next dialog.
fn remember_view(app: &mut crate::EffectcraftApp, view: PickView) {
    app.dialog_state.android_file.view = view;
    if let Err(e) = app.session.prefs.set("general.pickerView", serde_json::json!(view.as_str())) {
        log::warn!("picker view: {e}");
    }
    app.session.prefs_changed();
}

/// The picker's four view buttons; returns the one tapped this frame (applied after the modal
/// returns, like the dialog's other actions).
fn view_toolbar(ui: &mut egui::Ui, app: &mut crate::EffectcraftApp, view: PickView) -> Option<PickView> {
    use crate::i18n::tr;

    let mut picked = None;
    ui.horizontal(|ui| {
        ui.label(tr("View"));
        for (mode, label) in [(PickView::List, tr("List")), (PickView::Small, tr("Small")), (PickView::Medium, tr("Medium")), (PickView::Large, tr("Large"))] {
            let b = ui.add(egui::Button::new(label).selected(view == mode));
            app.auto.add(&format!("filePicker.view.{}", mode.as_str()), b.rect, mode.as_str());
            if b.clicked() {
                picked = Some(mode);
            }
        }
    });
    picked
}

/// What an icon tile shows in place of, or beside, its label.
enum Tile {
    /// A sub-folder (entered on tap).
    Folder,
    /// A saved project: the stored thumbnail the Home recents show.
    Project(PathBuf),
    /// A file: its picture where one decodes, else its extension.
    File(PathBuf),
}

/// One entry of the picker's icon grid.
struct TileItem {
    label: String,
    tile: Tile,
    selected: bool,
}

/// Draws `id` fitted inside `area` (never stretched): a thumbnail's letterbox.
fn paint_picture(lp: &egui::Painter, area: egui::Rect, id: egui::TextureId, size: egui::Vec2) {
    use egui::{Rect, pos2};

    let s = (area.width() / size.x.max(1.0)).min(area.height() / size.y.max(1.0));
    lp.image(id, Rect::from_center_size(area.center(), size * s), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), egui::Color32::WHITE);
}

/// Draws the icon grid over `rect` (tiles laid out like the Media Browser's, scrolled with a
/// finger through [`crate::widgets::PanelScroll`]) and returns the index of the entry tapped
/// this frame. Thumbnails decode at most twice per frame, the rest arrive on later repaints.
fn icon_grid(
    app: &mut crate::EffectcraftApp,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    t: &crate::theme::Tokens,
    rect: egui::Rect,
    view: PickView,
    items: &[TileItem],
    scroll_key: &str,
) -> Option<usize> {
    use crate::theme::Tokens;
    use egui::{Align2, Rect, pos2, vec2};

    let (tw, th, chars) = view.tile();
    let cols = grid_cols(rect.width(), tw);
    let rows = items.len().div_ceil(cols);
    let mut budget = 2u32;
    let scroll = crate::widgets::PanelScroll::begin(ui, egui::Id::new(("filePicker.grid", scroll_key)), rect);
    let lp = ui.painter().with_clip_rect(rect);
    let mut clicked = None;
    for (i, item) in items.iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(rect.min.x + 4.0 + (i % cols) as f32 * tw, rect.min.y + 4.0 + (i / cols) as f32 * th - scroll.offset),
            vec2(tw - 8.0, th - 8.0),
        );
        if !r.intersects(rect) {
            continue;
        }
        let resp = ui.interact(r.intersect(rect), egui::Id::new(("filePicker.entry", i, item.label.as_str())), egui::Sense::click());
        app.auto.add(&format!("filePicker.entry.{}", item.label), r, &item.label);
        lp.rect_filled(
            r,
            4.0,
            if item.selected {
                t.row_selected
            } else if resp.hovered() {
                t.hover
            } else {
                t.row
            },
        );
        let picture = Rect::from_min_max(r.min + vec2(4.0, 4.0), pos2(r.max.x - 4.0, r.max.y - 24.0));
        match &item.tile {
            Tile::Folder => {
                crate::icons::paint(&lp, Rect::from_center_size(picture.center(), vec2(34.0, 30.0)), crate::icons::Icon::Folder, t.text_dim);
            }
            Tile::Project(path) => {
                let key = path.to_string_lossy();
                match super::home::thumb_texture(app, ctx, &key) {
                    Some(tex) => {
                        let [w, h] = tex.size();
                        paint_picture(&lp, picture, tex.id(), vec2(w as f32, h as f32));
                    }
                    None => {
                        lp.text(picture.center(), Align2::CENTER_CENTER, ext_label(path), Tokens::ui(10.0), t.text_faint);
                    }
                }
            }
            Tile::File(path) => {
                let key = path.to_string_lossy();
                match super::thumbs::media(&*app, ctx, &key, &mut budget) {
                    Some((tex, [w, h])) => {
                        paint_picture(&lp, picture, tex.id(), vec2(w as f32, h as f32));
                    }
                    None => {
                        lp.text(picture.center(), Align2::CENTER_CENTER, ext_label(path), Tokens::ui(10.0), t.text_faint);
                    }
                }
            }
        }
        // The name under the picture, shortened to what the tile fits.
        let name = if item.label.chars().count() > chars {
            format!("{}…", item.label.chars().take(chars.saturating_sub(1)).collect::<String>())
        } else {
            item.label.clone()
        };
        lp.text(pos2(r.center().x, r.max.y - 12.0), Align2::CENTER_CENTER, name, Tokens::ui(11.0), t.text);
        if item.selected {
            lp.text(pos2(r.max.x - 6.0, r.min.y + 12.0), Align2::RIGHT_CENTER, "✓", Tokens::ui(11.0), t.text);
        }
        if resp.clicked() {
            clicked = Some(i);
        }
    }
    scroll.end(ui, &mut app.auto, &format!("filePicker.gridScroll.{scroll_key}"), rows as f32 * th + 8.0, t);
    if budget == 0 {
        // More thumbnails than one frame's budget: keep decoding.
        ctx.request_repaint();
    }
    clicked
}

/// The pending in-app prompt (lives in the dialog state while the modal is open).
#[derive(Clone, Debug, Default)]
pub struct AndroidFileDraft {
    /// The request being served.
    pub request: Option<crate::FileDialog>,
    /// The command to continue when the prompt is answered.
    pub(crate) answer: Option<crate::DialogAnswer>,
    /// The folder pick answers this preference (Settings) instead of a command.
    pub(crate) pref_key: Option<String>,
    /// The save filename field.
    pub name: String,
    /// The folder a save lands in (`None`: [`save_dir`]).
    pub folder: Option<PathBuf>,
    /// The folder browser is open at this directory.
    pub browser: Option<PathBuf>,
    /// The root the browser is clamped to (`None`: [`StorageRoot::App`] at the base).
    pub root: Option<StorageRoot>,
    /// File pick: the files chosen so far (the browser's checkmarks).
    pub selected: Vec<PathBuf>,
    /// File pick: extensions to list (empty: every file).
    pub exts: Vec<String>,
    /// File pick: several files may be chosen.
    pub many: bool,
    /// How the browser and the Open Project list show their entries ([`stored_view`] at open,
    /// [`remember_view`] on switch).
    pub view: PickView,
}

/// Serve a file-dialog request with the in-app prompt: stash it, open the modal, and continue
/// the command when the answer arrives (like [`crate::Hooks::file_dialog_async`]). `id` is the
/// command, for its `{"dialog": id}` acknowledgement. Save, OpenProject, Folder and Open are
/// served; anything else is `None` (the caller keeps its "no dialog" error).
pub(crate) fn spawn(
    app: &mut crate::EffectcraftApp,
    id: &str,
    request: crate::FileDialog,
    answer: crate::DialogAnswer,
) -> Option<Result<serde_json::Value, String>> {
    use serde_json::json;

    if !matches!(request, crate::FileDialog::Save { .. } | crate::FileDialog::OpenProject | crate::FileDialog::Folder | crate::FileDialog::Open { .. }) {
        return None;
    }
    if app.dialog.is_some() {
        return Some(Err("a dialog is already open".to_string()));
    }
    // One draft built once. Stashing these onto the previous value and then replacing the
    // whole struct threw them away: the Save field started blank, an Open listed every file
    // instead of its extensions, and multi-select never reached the browser.
    let name = match request {
        crate::FileDialog::Save { ref default, .. } => Path::new(default).file_name().and_then(|n| n.to_str()).unwrap_or(default).to_string(),
        _ => String::new(),
    };
    let (exts, many) = match request {
        crate::FileDialog::Open { ref exts, many } => (exts.clone(), many),
        _ => (Vec::new(), false),
    };
    let view = stored_view(app);
    app.dialog_state.android_file =
        AndroidFileDraft { request: Some(request), answer: Some(answer), browser: None, name, exts, many, view, ..Default::default() };
    app.dialog = Some(crate::Dialog::AndroidFile);
    Some(Ok(json!({"dialog": id})))
}

/// Settings ▸ … folder buttons: pick a folder for the preference `key` while the Settings
/// dialog stays open (the browser renders over it).
pub(crate) fn pick_folder_pref(app: &mut crate::EffectcraftApp, key: &str) {
    let Some(base) = app.hooks.app_files_dir.clone() else { return };
    if app.dialog_state.android_file.request.is_some() {
        return;
    }
    let view = stored_view(app);
    app.dialog_state.android_file = AndroidFileDraft { request: Some(crate::FileDialog::Folder), browser: Some(base), view, ..Default::default() };
    app.dialog_state.android_file.pref_key = Some(key.to_string());
}

/// Settings ▸ … file buttons: pick a file for the preference `key` while the Settings dialog
/// stays open (the browser renders over it) — Template Project, which needs a project file.
/// Answers true when the browser was opened, so the caller can stay silent only then.
pub(crate) fn pick_project_pref(app: &mut crate::EffectcraftApp, key: &str) -> bool {
    let Some(base) = app.hooks.app_files_dir.clone() else { return false };
    if app.dialog_state.android_file.request.is_some() {
        return false;
    }
    let exts: Vec<String> = PROJECT_EXTS.iter().map(|e| e.to_string()).collect();
    let view = stored_view(app);
    app.dialog_state.android_file = AndroidFileDraft {
        request: Some(crate::FileDialog::Open { exts: exts.clone(), many: false }),
        browser: Some(base),
        exts,
        many: false,
        pref_key: Some(key.to_string()),
        view,
        ..Default::default()
    };
    true
}

/// The pending request answered with `paths` (empty: cancelled). A preference pick writes the
/// preference and leaves the dialog that opened it (Settings) alone.
fn finish(app: &mut crate::EffectcraftApp, paths: Vec<String>) {
    let draft = std::mem::take(&mut app.dialog_state.android_file);
    if let Some(key) = draft.pref_key {
        if let Some(path) = paths.into_iter().next() {
            let _ = app.session.prefs.set(&key, serde_json::json!(path));
            app.session.prefs_changed();
        }
        return;
    }
    if let Some(mut answer) = draft.answer {
        answer.paths = paths;
        let _ = app.dialog_answers.0.send(answer);
    }
    if app.dialog == Some(crate::Dialog::AndroidFile) {
        app.dialog = None;
    }
}

/// The modal for the pending prompt, if the host gave us the storage directory.
pub fn show(app: &mut crate::EffectcraftApp, ctx: &egui::Context, t: &crate::theme::Tokens) {
    use crate::i18n::tr;

    let Some(base) = app.hooks.app_files_dir.clone() else {
        // The directory went away while the prompt was open: cancel, don't strand the command.
        finish(app, Vec::new());
        return;
    };
    let Some(request) = app.dialog_state.android_file.request.clone() else {
        if app.dialog == Some(crate::Dialog::AndroidFile) {
            app.dialog = None;
        }
        return;
    };
    // The browser is open (for a folder pick, a file pick, or a save's Location row): it owns
    // the answer.
    if let Some(dir) = app.dialog_state.android_file.browser.clone() {
        browser(app, ctx, t, &base, dir, request);
        return;
    }
    match request {
        crate::FileDialog::Open { .. } => {
            // A file request always answers through the browser.
            app.dialog_state.android_file.browser = Some(app.dialog_state.android_file.folder.clone().unwrap_or_else(|| base.clone()));
        }
        crate::FileDialog::Save { default, ext: _, project } => {
            let fallback = Path::new(&default).file_name().and_then(|n| n.to_str()).unwrap_or("Untitled.ecproj").to_string();
            let loc = app.dialog_state.android_file.folder.clone().unwrap_or_else(|| save_dir(&base, project));
            // (The tablet's soft keyboard appears when the field is tapped; winit raises the IME.)
            let mut name = std::mem::take(&mut app.dialog_state.android_file.name);
            let mut save = false;
            let mut cancel = false;
            let mut change = false;
            super::dialogs::modal(ctx, if project { "Save Project" } else { "Save File" }, egui::vec2(440.0, 240.0), t, |ui| {
                ui.label(format!("Location: {}", loc.to_string_lossy()));
                if ui.button(tr("Change…")).clicked() {
                    change = true;
                }
                ui.add_space(8.0);
                ui.add(egui::TextEdit::singleline(&mut name).hint_text(&fallback).desired_width(f32::INFINITY));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    save = ui.button(tr("Save")).clicked();
                    cancel = ui.button(tr("Cancel")).clicked();
                });
            });
            app.dialog_state.android_file.name = name;
            if change {
                app.dialog_state.android_file.folder = Some(loc.clone());
                app.dialog_state.android_file.browser = Some(loc);
            } else if save {
                let path = save_path_in(&loc, project, &app.dialog_state.android_file.name, &fallback);
                finish(app, vec![path.to_string_lossy().to_string()]);
            } else if cancel {
                finish(app, Vec::new());
            }
        }
        crate::FileDialog::OpenProject => {
            let projects = list_projects(&base);
            let view = app.dialog_state.android_file.view;
            let mut open: Option<String> = None;
            let mut cancel = false;
            let mut browse = false;
            let mut new_view: Option<PickView> = None;
            // Icons get the room for a project's picture; the rows keep the original dialog.
            // The body is exactly `size.y − 68` tall (frame margins + title): 500 leaves the
            // Browse/Cancel row clear of the scroll fold under the toolbar and the 300 px grid.
            let size = if view == PickView::List { egui::vec2(420.0, 320.0) } else { egui::vec2(660.0, 500.0) };
            super::dialogs::modal(ctx, "Open Project", size, t, |ui| {
                if let Some(v) = view_toolbar(ui, app, view) {
                    new_view = Some(v);
                }
                if projects.is_empty() {
                    ui.label(tr("No saved projects yet."));
                } else if view == PickView::List {
                    for path in &projects {
                        let label = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                        let b = ui.button(label);
                        app.auto.add(&format!("filePicker.entry.{label}"), b.rect, label);
                        if b.clicked() {
                            open = Some(path.to_string_lossy().to_string());
                        }
                    }
                } else {
                    let items: Vec<TileItem> = projects
                        .iter()
                        .map(|p| TileItem {
                            label: p.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string(),
                            tile: Tile::Project(p.clone()),
                            selected: false,
                        })
                        .collect();
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 300.0), egui::Sense::hover());
                    if let Some(i) = icon_grid(app, ui, ctx, t, rect, view, &items, "openProject")
                        && let Some(p) = projects.get(i)
                    {
                        open = Some(p.to_string_lossy().to_string());
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let b = ui.button(tr("Browse…"));
                    app.auto.add("filePicker.browse", b.rect, "Browse");
                    if b.clicked() {
                        browse = true;
                    }
                    let b = ui.button(tr("Cancel"));
                    app.auto.add("filePicker.cancel", b.rect, "Cancel");
                    cancel = b.clicked();
                });
            });
            if let Some(v) = new_view {
                remember_view(app, v);
            }
            if let Some(path) = open {
                finish(app, vec![path]);
            } else if browse {
                // Open a project from anywhere: switch to the file browser, filtered to project
                // extensions (the browser reads the filter from the draft, not the request).
                app.dialog_state.android_file.exts = PROJECT_EXTS.iter().map(|e| e.to_string()).collect();
                app.dialog_state.android_file.many = false;
                app.dialog_state.android_file.request =
                    Some(crate::FileDialog::Open { exts: PROJECT_EXTS.iter().map(|e| e.to_string()).collect(), many: false });
                app.dialog_state.android_file.browser = Some(base.clone());
            } else if cancel {
                finish(app, Vec::new());
            }
        }
        crate::FileDialog::Folder => {
            // A folder request always answers through the browser.
            app.dialog_state.android_file.browser = Some(app.dialog_state.android_file.folder.clone().unwrap_or(base));
        }
    }
}

/// The file/folder browser: switch root, enter sub-folders, create one, choose files (a file
/// pick) or the shown directory (a folder pick). Clamped to the current [`StorageRoot`].
fn browser(app: &mut crate::EffectcraftApp, ctx: &egui::Context, t: &crate::theme::Tokens, base: &Path, dir: PathBuf, request: crate::FileDialog) {
    use crate::i18n::tr;

    let shared = app.hooks.shared_storage_root.clone();
    let granted = app.hooks.shared_storage_granted.as_ref().is_some_and(|g| g());
    // The root the browser may not leave: the shared root while inside it, else the app base.
    let root = app.dialog_state.android_file.root.clone().unwrap_or_else(|| StorageRoot::App(base.to_path_buf()));
    let is_file = matches!(request, crate::FileDialog::Open { .. });

    let mut enter: Option<PathBuf> = None;
    let mut up = false;
    let mut choose = false;
    let mut cancel = false;
    let mut create = false;
    let mut new_name = String::new();
    let mut toggle: Option<usize> = None;
    let mut to_shared = false;
    let mut to_app = false;
    let mut ask_grant = false;
    let mut new_view: Option<PickView> = None;
    let view = app.dialog_state.android_file.view;
    // Icons get the room for real thumbnails; the rows keep the original dialog. The body is
    // exactly `size.y − 68` tall (frame margins + title): 560 leaves the footer clear of the
    // scroll fold with the location/toast row, the selection status and an "Up" row on screen.
    let size = if view == PickView::List { egui::vec2(440.0, 380.0) } else { egui::vec2(660.0, 560.0) };
    super::dialogs::modal(ctx, if is_file { "Choose Files" } else { "Choose Folder" }, size, t, |ui| {
        // Root switcher: two real buttons, the browsed root highlighted. These were
        // `selectable_label`s, which draw as flat text, so "Shared storage (grant access)"
        // read as a caption and the grant behind it was never findable.
        ui.horizontal(|ui| {
            let in_shared = matches!(root, StorageRoot::Shared(_));
            let b = ui.add(egui::Button::new(tr("App storage")).selected(!in_shared));
            app.auto.add("filePicker.root.app", b.rect, "App storage");
            if b.clicked() {
                to_app = true;
            }
            if let Some(_sroot) = &shared {
                let label = if granted { tr("Shared storage") } else { tr("Shared storage (grant access)") };
                let b = ui.add(egui::Button::new(label).selected(in_shared));
                app.auto.add("filePicker.root.shared", b.rect, "Shared storage");
                if b.clicked() {
                    if granted {
                        to_shared = true;
                    } else {
                        ask_grant = true;
                    }
                }
            }
        });
        // Android never grants "All files access" silently, so ask for it where it can be
        // acted on: the button above is the only other mention and reads as a caption.
        if shared.is_some() && !granted {
            ui.horizontal_wrapped(|ui| {
                ui.label(tr("EffectCraft needs \"All files access\" to read and write outside its own folder."));
                if ui.button(tr("Allow…")).clicked() {
                    ask_grant = true;
                }
            });
        }
        // The status toast is painted on the base layer, beneath this modal's backdrop, so a
        // failure here ("Could not create the folder: …", "Select at least one file.") looked
        // exactly like nothing happening. Show it here — in the location row, because the
        // modal's body is exactly `size` tall: an extra row would push the footer buttons
        // below the scroll fold, where they cannot be tapped.
        let (shown, shown_id) = if app.ui.status.is_empty() {
            (format!("Location: {}", dir.to_string_lossy()), "filePicker.location")
        } else {
            (app.ui.status.clone(), "filePicker.toast")
        };
        let lr = ui.label(shown.clone());
        app.auto.add(shown_id, lr.rect, &shown);
        if dir != root.dir() {
            let b = ui.button(tr("Up"));
            app.auto.add("filePicker.up", b.rect, "Up");
            if b.clicked() {
                up = true;
            }
        }
        if let Some(v) = view_toolbar(ui, app, view) {
            new_view = Some(v);
        }
        let dirs = list_dirs(&dir);
        let files: Vec<PathBuf> = if is_file {
            let exts: Vec<String> = app.dialog_state.android_file.exts.clone();
            list_files(&dir, &exts)
        } else {
            Vec::new()
        };
        let selected = app.dialog_state.android_file.selected.clone();
        if dirs.is_empty() && files.is_empty() {
            // An unreadable shared root is the grant not yet held, not an empty folder.
            if matches!(root, StorageRoot::Shared(_)) && !readable(&dir) {
                ui.label(tr("This folder can't be read yet. Grant EffectCraft access to all files."));
                if ui.button(tr("Grant access")).clicked() {
                    ask_grant = true;
                }
            } else {
                ui.label(tr("(empty)"));
            }
        }
        if view == PickView::List {
            // The original rows, unchanged — with their automation ids for tests and agents.
            egui::ScrollArea::vertical().max_height(180.0).auto_shrink([false, true]).show(ui, |ui| {
                for sub in dirs {
                    let label = sub.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                    let b = ui.button(format!("{label}/"));
                    app.auto.add(&format!("filePicker.entry.{label}"), b.rect, label);
                    if b.clicked() {
                        enter = Some(sub);
                    }
                }
                for (i, file) in files.iter().enumerate() {
                    let label = file.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                    let is_sel = selected.contains(file);
                    let mark = if is_file && is_sel { "✓ " } else { "" };
                    let b = ui.button(format!("{mark}{label}"));
                    app.auto.add(&format!("filePicker.entry.{label}"), b.rect, label);
                    if b.clicked() {
                        toggle = Some(i);
                    }
                }
            });
        } else if !dirs.is_empty() || !files.is_empty() {
            // Icons: folders first (a folder glyph), then files — a project's stored thumbnail
            // for a project file, the file's own picture where one decodes, else its extension.
            let mut items: Vec<TileItem> = dirs
                .iter()
                .map(|sub| TileItem { label: sub.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string(), tile: Tile::Folder, selected: false })
                .collect();
            items.extend(files.iter().map(|f| {
                let is_project = f.extension().and_then(|e| e.to_str()).is_some_and(|e| PROJECT_EXTS.iter().any(|p| e.eq_ignore_ascii_case(p)));
                let tile = if is_project { Tile::Project(f.clone()) } else { Tile::File(f.clone()) };
                TileItem { label: f.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string(), tile, selected: is_file && selected.contains(f) }
            }));
            let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 300.0), egui::Sense::hover());
            if let Some(i) = icon_grid(app, ui, ctx, t, rect, view, &items, "browser") {
                if let Some(sub) = dirs.get(i) {
                    enter = Some(sub.clone());
                } else if let Some(file_i) = i.checked_sub(dirs.len()) {
                    toggle = Some(file_i);
                }
            }
        }
        if is_file {
            let n = app.dialog_state.android_file.selected.len();
            let status = if n == 0 { tr("Tap files to select them.").to_string() } else { format!("{n} selected") };
            let sr = ui.label(status.clone());
            app.auto.add("filePicker.status", sr.rect, &status);
        } else {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut new_name).hint_text(tr("New folder")).desired_width(160.0));
                create = ui.button(tr("Create")).clicked();
            });
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let b = ui.button(if is_file { tr("Open") } else { tr("Choose this folder") });
            app.auto.add("filePicker.open", b.rect, "Open");
            choose = b.clicked();
            let b = ui.button(tr("Cancel"));
            app.auto.add("filePicker.cancel", b.rect, "Cancel");
            cancel = b.clicked();
        });
    });
    if let Some(v) = new_view {
        remember_view(app, v);
    }

    if ask_grant {
        if let Some(ask) = app.hooks.shared_storage_grant.as_ref() {
            ask();
            app.ui.status = "Allow \"All files access\" for EffectCraft, then tap Shared storage.".into();
        } else {
            app.ui.status = "This build can't request shared-storage access.".into();
        }
    } else if to_app {
        app.dialog_state.android_file.root = Some(StorageRoot::App(base.to_path_buf()));
        app.dialog_state.android_file.browser = Some(base.to_path_buf());
    } else if to_shared && let Some(sroot) = shared.clone() {
        app.dialog_state.android_file.root = Some(StorageRoot::Shared(sroot.clone()));
        app.dialog_state.android_file.browser = Some(sroot);
    } else if let Some(i) = toggle {
        let files: Vec<PathBuf> = {
            let exts = app.dialog_state.android_file.exts.clone();
            list_files(&dir, &exts)
        };
        if let Some(file) = files.get(i) {
            let d = &mut app.dialog_state.android_file;
            if d.many {
                if let Some(pos) = d.selected.iter().position(|f| f == file) {
                    d.selected.remove(pos);
                } else {
                    d.selected.push(file.clone());
                }
            } else {
                d.selected = vec![file.clone()];
                // A single-file pick answers as soon as a file is tapped.
                let picked: Vec<String> = d.selected.iter().map(|p| p.to_string_lossy().to_string()).collect();
                finish(app, picked);
            }
        }
    } else if let Some(sub) = enter {
        app.dialog_state.android_file.browser = Some(sub);
    } else if up {
        app.dialog_state.android_file.browser = Some(root.up_from(&dir));
    } else if create {
        let name = sanitize_name(&new_name, "");
        if name.is_empty() {
            app.ui.status = "Name the new folder first".into();
        } else {
            let made = dir.join(&name);
            match std::fs::create_dir_all(&made) {
                Ok(()) => app.dialog_state.android_file.browser = Some(made),
                Err(e) => app.ui.status = format!("Could not create the folder: {e}"),
            }
        }
    } else if choose {
        if is_file {
            let picked: Vec<String> = app.dialog_state.android_file.selected.iter().map(|p| p.to_string_lossy().to_string()).collect();
            if picked.is_empty() {
                app.ui.status = "Select at least one file.".into();
            } else {
                finish(app, picked);
            }
        } else {
            app.dialog_state.android_file.browser = None;
            app.dialog_state.android_file.folder = Some(dir.clone());
            if matches!(request, crate::FileDialog::Folder) {
                finish(app, vec![dir.to_string_lossy().to_string()]);
            }
        }
    } else if cancel {
        if is_file {
            // A file pick's Cancel answers the command as cancelled.
            finish(app, Vec::new());
        } else {
            app.dialog_state.android_file.browser = None;
            if matches!(request, crate::FileDialog::Folder) {
                finish(app, Vec::new());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn saves_split_into_projects_and_exports() {
        let base = Path::new("/files");
        assert_eq!(save_dir(base, true), PathBuf::from("/files/Projects"));
        assert_eq!(save_dir(base, false), PathBuf::from("/files/Exports"));
    }

    #[test]
    fn names_cannot_escape_the_dialog_folder() {
        assert_eq!(sanitize_name("Scene.ecproj", "F.ecproj"), "Scene.ecproj");
        assert_eq!(sanitize_name("  spaced .ecproj  ", "F.ecproj"), "spaced .ecproj");
        assert_eq!(sanitize_name("../../evil.ecproj", "F.ecproj"), "evil.ecproj");
        assert_eq!(sanitize_name("/abs/path/x.ecproj", "F.ecproj"), "x.ecproj");
        assert_eq!(sanitize_name("", "Fallback.ecproj"), "Fallback.ecproj");
        assert_eq!(sanitize_name("   ", "Fallback.ecproj"), "Fallback.ecproj");
        assert_eq!(sanitize_name("..", "Fallback.ecproj"), "Fallback.ecproj");
    }

    #[test]
    fn project_saves_keep_their_extension() {
        assert_eq!(enforce_project_ext("Scene.ecproj"), "Scene.ecproj");
        assert_eq!(enforce_project_ext("Copy.ECPROJX"), "Copy.ECPROJX");
        assert_eq!(enforce_project_ext("Scene"), "Scene.ecproj");
        assert_eq!(enforce_project_ext("Scene.png"), "Scene.png.ecproj");
    }

    #[test]
    fn save_paths_stay_inside_the_chosen_folder() {
        let base = Path::new("/files");
        assert_eq!(save_path(base, true, "Scene", "F.ecproj"), PathBuf::from("/files/Projects/Scene.ecproj"));
        assert_eq!(save_path(base, false, "Frame.png", "F.png"), PathBuf::from("/files/Exports/Frame.png"));
        assert_eq!(save_path(base, true, "../x", "F.ecproj"), PathBuf::from("/files/Projects/x.ecproj"));
        assert_eq!(save_path_in(Path::new("/files/Exports/Shots"), false, "Frame.png", "F.png"), PathBuf::from("/files/Exports/Shots/Frame.png"));
        assert_eq!(save_path_in(Path::new("/files/Projects"), true, "Copy", "F.ecproj"), PathBuf::from("/files/Projects/Copy.ecproj"));
    }

    #[test]
    fn only_projects_list_newest_first() {
        let dir = std::env::temp_dir().join(format!("ec-android-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let projects = dir.join("Projects");
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::write(projects.join("old.ecproj"), "{}").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(projects.join("new.ecprojx"), "{}").unwrap();
        std::fs::write(projects.join("clip.mp4"), "x").unwrap();
        let listed = list_projects(&dir);
        assert_eq!(listed, vec![projects.join("new.ecprojx"), projects.join("old.ecproj")]);
        assert!(list_projects(&dir.join("missing")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_browser_lists_only_directories_sorted() {
        let dir = std::env::temp_dir().join(format!("ec-android-dirs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("zeta")).unwrap();
        std::fs::create_dir_all(dir.join("Alpha")).unwrap();
        std::fs::write(dir.join("a-file.txt"), "x").unwrap();
        assert_eq!(list_dirs(&dir), vec![dir.join("Alpha"), dir.join("zeta")]);
        assert!(list_dirs(&dir.join("missing")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn up_is_clamped_to_the_root() {
        let app = StorageRoot::App("/files".into());
        assert_eq!(app.dir(), Path::new("/files"));
        assert_eq!(app.up_from(Path::new("/files/Projects/Deep")), PathBuf::from("/files/Projects"));
        assert_eq!(app.up_from(Path::new("/files/Projects")), PathBuf::from("/files"));
        // The root's own top is as far up as the browser may go — never out of the root.
        assert_eq!(app.up_from(Path::new("/files")), PathBuf::from("/files"));
        let shared = StorageRoot::Shared("/storage/emulated/0".into());
        assert_eq!(shared.up_from(Path::new("/storage/emulated/0/DCIM/Cam")), PathBuf::from("/storage/emulated/0/DCIM"));
        assert_eq!(shared.up_from(Path::new("/storage/emulated/0")), PathBuf::from("/storage/emulated/0"));
    }

    #[test]
    fn files_list_filtered_by_extension_sorted() {
        let dir = std::env::temp_dir().join(format!("ec-android-files2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("Clip.MP4"), "x").unwrap();
        std::fs::write(dir.join("still.png"), "x").unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        let mp4: Vec<String> = vec!["mp4".to_string()];
        let listed = list_files(&dir, &mp4);
        // Case-insensitive match, directories never listed.
        assert_eq!(listed, vec![dir.join("Clip.MP4")]);
        // An empty filter matches every file, sorted, still excluding directories.
        assert_eq!(list_files(&dir, &[]), vec![dir.join("Clip.MP4"), dir.join("notes.txt"), dir.join("still.png")]);
        assert!(list_files(&dir.join("missing"), &[]).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn readable_tells_an_unreadable_folder_from_an_empty_one() {
        let dir = std::env::temp_dir().join(format!("ec-android-read-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!readable(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(readable(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ------------------------------------------------------------ the view modes

    #[test]
    fn views_parse_size_and_store() {
        assert_eq!(PickView::parse("list"), PickView::List);
        assert_eq!(PickView::parse("small"), PickView::Small);
        assert_eq!(PickView::parse("medium"), PickView::Medium);
        assert_eq!(PickView::parse("large"), PickView::Large);
        // A broken setting reads as the default instead of breaking the picker.
        assert_eq!(PickView::parse("huge"), PickView::default());
        assert_eq!(PickView::parse(""), PickView::default());
        assert_eq!(PickView::default(), PickView::Large, "big enough to see what a file is");
        for v in [PickView::List, PickView::Small, PickView::Medium, PickView::Large] {
            assert_eq!(PickView::parse(v.as_str()), v, "round trip");
        }
        // Tiles grow with the view, and the name budget with them.
        let (s, m, l) = (PickView::Small.tile(), PickView::Medium.tile(), PickView::Large.tile());
        assert!(s.0 < m.0 && m.0 < l.0, "widths grow: {s:?} {m:?} {l:?}");
        assert!(s.1 < m.1 && m.1 < l.1, "heights grow: {s:?} {m:?} {l:?}");
        assert!(s.2 < m.2 && m.2 < l.2, "names grow: {s:?} {m:?} {l:?}");
        // The grid never collapses to zero columns, even in a sliver.
        assert_eq!(grid_cols(0.0, 120.0), 1);
        assert_eq!(grid_cols(500.0, 120.0), 4);
        // Extensions read as what they are.
        assert_eq!(ext_label(Path::new("/a/clip.MP4")), "MP4");
        assert_eq!(ext_label(Path::new("/a/notes")), "File");
    }

    /// A temp directory laid out like a device: a sub-folder, a saved project, media, a
    /// single-pick file and a text file with no picture.
    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ec-picker-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Projects")).unwrap();
        std::fs::create_dir_all(dir.join("shots")).unwrap();
        std::fs::write(dir.join("Projects").join("Scene.ecproj"), "{}").unwrap();
        std::fs::write(dir.join("clip.mp4"), "x").unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        std::fs::write(dir.join("guide.json"), "{}").unwrap();
        dir
    }

    /// The picker's app: the in-app dialog enabled by the host's storage directory, with the
    /// config store a project's thumbnail is read from.
    fn picker(dir: &Path) -> (crate::EffectcraftApp, egui::Context) {
        let ctx = egui::Context::default();
        let mut app = crate::EffectcraftApp::new(effectcraft_engine::Session::default());
        app.session.config = Some(std::sync::Arc::new(effectcraft_engine::config::MemoryConfig::default()));
        crate::theme::install(&ctx, &app.tokens, crate::i18n::language(&app));
        app.hooks.app_files_dir = Some(dir.to_path_buf());
        (app, ctx)
    }

    fn frame(app: &mut crate::EffectcraftApp, ctx: &egui::Context) {
        let mut out = ctx.run_ui(Default::default(), |ui| {
            app.auto.begin_frame();
            crate::panels::dialogs::show(app, ui.ctx());
        });
        out.textures_delta.clear();
    }

    fn frame_with(app: &mut crate::EffectcraftApp, ctx: &egui::Context, events: Vec<egui::Event>) {
        let input = egui::RawInput { events, ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| {
            app.auto.begin_frame();
            crate::panels::dialogs::show(app, ui.ctx());
        });
        out.textures_delta.clear();
    }

    /// The automation element `id` as the last rendered frame registered it (`auto::find`
    /// prefers the frame before, for control handlers that look up mid-frame).
    fn element<'a>(app: &'a crate::EffectcraftApp, id: &str) -> Option<&'a crate::automation::Element> {
        app.auto.elements.iter().find(|e| e.id == id)
    }

    /// Tap the automation element `id` the way a finger does: hover it, then press and release
    /// where it is *after* that frame — the headless modal settles its centered position on
    /// some pointer frames, so a rect read earlier may already be stale.
    fn tap(app: &mut crate::EffectcraftApp, ctx: &egui::Context, id: &str) {
        let e = element(app, id).unwrap_or_else(|| panic!("no `{id}` on screen")).clone();
        let p = egui::pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
        frame_with(app, ctx, vec![egui::Event::PointerMoved(p)]);
        let e = element(app, id).unwrap_or_else(|| panic!("no `{id}` after hover")).clone();
        let p = egui::pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
        debug_assert!(p.x > 0.0 && p.y > 0.0, "`{id}` is on screen");
        frame_with(
            app,
            ctx,
            vec![egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::default() }],
        );
        frame_with(
            app,
            ctx,
            vec![egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::default() }],
        );
        frame(app, ctx);
    }

    /// Without the host's storage directory the commands keep asking the blocking hooks (the
    /// desktop pickers): the in-app dialog serves the hosts that offer it and shadows nothing.
    #[test]
    fn the_in_app_dialog_needs_the_hosts_storage_directory() {
        let mut app = crate::EffectcraftApp::new(effectcraft_engine::Session::default());
        let ctx = egui::Context::default();
        let err = crate::menus::invoke(&mut app, &ctx, "file.import", json!({})).unwrap_err();
        assert!(err.contains("no file dialog available"), "{err}");
        assert_eq!(app.dialog, None);
    }

    #[test]
    fn the_view_is_remembered_across_dialogs() {
        let dir = fixture("remember");
        let (mut app, _ctx) = picker(&dir);
        assert_eq!(stored_view(&app), PickView::Large, "Large icons by default");
        remember_view(&mut app, PickView::Small);
        assert_eq!(app.dialog_state.android_file.view, PickView::Small);
        assert_eq!(app.session.prefs.general.picker_view, "small");
        assert_eq!(stored_view(&app), PickView::Small, "the next dialog opens in it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The picker's view buttons switch the listing between rows and three icon sizes, and the
    /// choice is remembered for the next dialog (`general.pickerView`).
    #[test]
    fn the_file_picker_switches_views_and_remembers_them() {
        let dir = fixture("views");
        let (mut app, ctx) = picker(&dir);
        crate::menus::invoke(&mut app, &ctx, "file.import", json!({})).unwrap();
        assert_eq!(app.dialog, Some(crate::Dialog::AndroidFile));
        assert!(app.dialog_state.android_file.many, "import picks many files");
        // The file request opens the browser on the frame after the dialog.
        frame(&mut app, &ctx);
        frame(&mut app, &ctx);
        // Large icons by default: a tile, not a row.
        for v in ["list", "small", "medium", "large"] {
            assert!(element(&app, &format!("filePicker.view.{v}")).is_some(), "the {v} view button");
        }
        let tile = element(&app, "filePicker.entry.clip.mp4").unwrap().clone();
        assert!((tile.rect[3] - 128.0).abs() < 1.0, "a Large tile is 136-8 tall: {}", tile.rect[3]);
        // Rows, the way the picker listed before.
        tap(&mut app, &ctx, "filePicker.view.list");
        let row = element(&app, "filePicker.entry.clip.mp4").unwrap().clone();
        assert!(row.rect[3] <= 40.0, "a row, not a tile: {}", row.rect[3]);
        assert!(element(&app, "filePicker.view.large").is_some(), "the toolbar stays to switch back");
        assert_eq!(app.session.prefs.general.picker_view, "list", "the view is remembered");
        // Open with nothing selected: the toast takes the location row — never the footer's
        // room — so both footer buttons stay tappable while it shows (it used to push them
        // below the scroll fold, where the clip made them dead).
        tap(&mut app, &ctx, "filePicker.open");
        assert!(app.ui.status.contains("Select at least"), "the picker explains: {}", app.ui.status);
        assert!(app.dialog.is_some(), "still open: nothing was selected");
        assert!(element(&app, "filePicker.toast").is_some(), "the toast shows inside the modal");
        assert!(element(&app, "filePicker.location").is_none(), "it replaces the location row");
        // Medium icons.
        tap(&mut app, &ctx, "filePicker.view.medium");
        let med = element(&app, "filePicker.entry.clip.mp4").unwrap().clone();
        assert!((med.rect[3] - 104.0).abs() < 1.0, "a Medium tile is 112-8 tall: {}", med.rect[3]);
        // The next picker opens in the remembered view — and Cancel answers even with the
        // toast still up.
        tap(&mut app, &ctx, "filePicker.cancel");
        assert!(app.dialog.is_none());
        crate::menus::invoke(&mut app, &ctx, "file.import", json!({})).unwrap();
        frame(&mut app, &ctx);
        frame(&mut app, &ctx);
        let med = element(&app, "filePicker.entry.clip.mp4").unwrap().clone();
        assert!((med.rect[3] - 104.0).abs() < 1.0, "remembered Medium: {}", med.rect[3]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The icon view keeps the picker's semantics: a tap toggles a file, a folder is entered,
    /// Up comes back, and Open answers with the selection (or says nothing is selected).
    #[test]
    fn the_file_picker_selects_opens_and_enters_folders() {
        let dir = fixture("select");
        let (mut app, ctx) = picker(&dir);
        crate::menus::invoke(&mut app, &ctx, "file.import", json!({})).unwrap();
        // The file request opens the browser on the frame after the dialog.
        frame(&mut app, &ctx);
        frame(&mut app, &ctx);
        // A tap toggles a file and the count shows.
        tap(&mut app, &ctx, "filePicker.entry.clip.mp4");
        assert_eq!(app.dialog_state.android_file.selected, vec![dir.join("clip.mp4")]);
        assert_eq!(element(&app, "filePicker.status").unwrap().label, "1 selected");
        // A folder is entered on tap; Up comes back (the selection survives navigation).
        tap(&mut app, &ctx, "filePicker.entry.shots");
        assert!(element(&app, "filePicker.location").unwrap().label.contains("shots"));
        assert!(element(&app, "filePicker.up").is_some());
        tap(&mut app, &ctx, "filePicker.up");
        assert!(!element(&app, "filePicker.location").unwrap().label.contains("shots"));
        assert_eq!(app.dialog_state.android_file.selected, vec![dir.join("clip.mp4")]);
        // Tapping it again untoggles it; Open with nothing selected says so and stays open.
        tap(&mut app, &ctx, "filePicker.entry.clip.mp4");
        assert!(app.dialog_state.android_file.selected.is_empty());
        tap(&mut app, &ctx, "filePicker.open");
        assert!(app.ui.status.contains("Select at least"), "the picker explains: {}", app.ui.status);
        assert!(app.dialog.is_some(), "still open: nothing was selected");
        // With a selection it answers the command and closes.
        tap(&mut app, &ctx, "filePicker.entry.clip.mp4");
        tap(&mut app, &ctx, "filePicker.open");
        assert!(app.dialog.is_none());
        assert!(app.dialog_state.android_file.request.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A single-file pick answers the moment a file is tapped (no Open to hunt for).
    #[test]
    fn a_single_file_pick_answers_the_moment_a_file_is_tapped() {
        let dir = fixture("single");
        let (mut app, ctx) = picker(&dir);
        // `view.importGuides` asks for one JSON.
        crate::menus::invoke(&mut app, &ctx, "view.importGuides", json!({})).unwrap();
        assert_eq!(app.dialog, Some(crate::Dialog::AndroidFile));
        assert!(!app.dialog_state.android_file.many);
        // The file request opens the browser on the frame after the dialog.
        frame(&mut app, &ctx);
        frame(&mut app, &ctx);
        assert!(element(&app, "filePicker.entry.guide.json").is_some());
        tap(&mut app, &ctx, "filePicker.entry.guide.json");
        assert!(app.dialog.is_none(), "a single pick answers on the tap");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Open Project prompt offers the same views, and an icon view shows the project's
    /// stored thumbnail — the Home recents' picture, not a re-decode.
    #[test]
    fn open_project_lists_projects_with_their_thumbnails() {
        let dir = fixture("open");
        let (mut app, ctx) = picker(&dir);
        let path = dir.join("Projects").join("Scene.ecproj");
        let key = path.to_string_lossy().to_string();
        let img = egui::ColorImage::new([96, 54], vec![egui::Color32::from_rgb(40, 120, 200); 96 * 54]);
        let hex = super::super::home::encode_thumb(&img);
        app.session.config.as_ref().unwrap().write(&super::super::home::thumb_name(&key), &hex).unwrap();
        crate::menus::invoke(&mut app, &ctx, "file.open", json!({})).unwrap();
        frame(&mut app, &ctx);
        let tile = element(&app, "filePicker.entry.Scene.ecproj").unwrap().clone();
        assert!((tile.rect[3] - 128.0).abs() < 1.0, "Large by default: {}", tile.rect[3]);
        assert!(app.home_thumbs.get(&key).is_some_and(|t| t.is_some()), "the project's stored thumbnail is loaded");
        // Tap it: the prompt answers with the project and closes.
        tap(&mut app, &ctx, "filePicker.entry.Scene.ecproj");
        assert!(app.dialog.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
