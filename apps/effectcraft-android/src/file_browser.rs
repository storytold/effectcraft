//! In-app file browser for Android (`Hooks::file_dialog_async`).
//!
//! Android has no `rfd`, and a `NativeActivity` cannot show the system picker without Java.
//! This browser runs inside the egui UI: the shell's hook stores the request, the shell draws
//! the window each frame, and OK/Cancel answers through `DialogDone`. Roots are the app's own
//! internal storage (always readable) plus the shared `Download`/`DCIM` folders when the
//! platform lets us read them.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use effectcraft_ui_egui::{DialogDone, FileDialog};

/// One row in the browser list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

/// A pending picker: the request, where the browser is, and the answer when it closes.
pub struct Pending {
    request: FileDialog,
    current: PathBuf,
    entries: Vec<Entry>,
    error: Option<String>,
    selected: Vec<PathBuf>,
    filename: String,
    done: Option<DialogDone>,
}

/// Roots to offer: the app's data dir first, then its internal parent, then shared folders
/// when they exist. All I/O errors become an empty list, never a crash.
pub fn roots(data_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(d) = data_dir {
        out.push(d.to_path_buf());
        if let Some(parent) = d.parent() {
            let parent = parent.to_path_buf();
            if parent != *d && !out.contains(&parent) {
                out.push(parent);
            }
        }
    }
    for shared in ["/sdcard/Download", "/sdcard/DCIM", "/storage/emulated/0/Download", "/storage/emulated/0/DCIM"] {
        let p = PathBuf::from(shared);
        if p.is_dir() && !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// List `dir`: directories first, then files, each by name (case-insensitive). At most 2000
/// entries; unreadable directories become an error string, not a crash.
pub fn list_dir(dir: &Path) -> Result<Vec<Entry>, String> {
    let read = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut entries = Vec::new();
    for e in read {
        let Ok(e) = e else { continue };
        let path = e.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let is_dir = path.is_dir();
        entries.push(Entry { name, path, is_dir });
        if entries.len() >= 2000 {
            break;
        }
    }
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(entries)
}

/// Whether `path` is a project file (`.ecproj`, `.ecprojx`).
pub fn is_project(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("ecproj") || e.eq_ignore_ascii_case("ecprojx"))
}

/// The folder a dialog opens in: the path a script (or a save default) names when that is a
/// folder, else the folder that file lives in; `None` when neither exists (the data dir then).
/// Same rule as the desktop's `start_dir` in `apps/effectcraft/src/file_dialogs.rs`.
pub fn dialog_start(request: &FileDialog) -> Option<PathBuf> {
    let start = match request {
        FileDialog::Script { start, .. } | FileDialog::ScriptFolder { start, .. } => start.as_str(),
        FileDialog::Save { default, .. } | FileDialog::ScriptSave { default, .. } => default.as_str(),
        _ => return None,
    };
    if start.is_empty() {
        return None;
    }
    let at = Path::new(start);
    if at.is_dir() { Some(at.to_path_buf()) } else { at.parent().filter(|d| d.is_dir()).map(Path::to_path_buf) }
}

/// The filename field's initial value for a save dialog, clamped to 128 chars.
pub fn save_default(request: &FileDialog) -> String {
    match request {
        FileDialog::Save { default, .. } | FileDialog::ScriptSave { default, .. } => {
            let name = Path::new(default).file_name().and_then(|n| n.to_str()).unwrap_or(default);
            if name.is_empty() {
                return "Untitled Project.ecproj".to_string();
            }
            let mut end = name.len().min(128);
            while !name.is_char_boundary(end) {
                end = end.saturating_sub(1);
            }
            name[..end].to_string()
        }
        _ => String::new(),
    }
}

/// The dialog's window title: the label for the dialogs the app asks for, the script's own
/// `prompt` for the ones a script asks for (empty prompt: the app's label for that kind).
pub fn title(request: &FileDialog) -> String {
    let named = |prompt: &str, fallback: &str| if prompt.is_empty() { fallback.to_string() } else { prompt.to_string() };
    match request {
        FileDialog::Open { .. } => "Import files".into(),
        FileDialog::OpenProject => "Open project".into(),
        FileDialog::Folder => "Choose folder".into(),
        FileDialog::Save { project: true, .. } => "Save project".into(),
        FileDialog::Save { .. } => "Save file".into(),
        FileDialog::Script { prompt, .. } => named(prompt, "Open files"),
        FileDialog::ScriptFolder { prompt, .. } => named(prompt, "Choose folder"),
        FileDialog::ScriptSave { prompt, .. } => named(prompt, "Save file"),
    }
}

/// Whether the request answers with a typed filename (the save dialogs).
pub fn is_save(request: &FileDialog) -> bool {
    matches!(request, FileDialog::Save { .. } | FileDialog::ScriptSave { .. })
}

/// Whether the request answers with the folder the browser is in (the folder dialogs).
pub fn is_folder(request: &FileDialog) -> bool {
    matches!(request, FileDialog::Folder | FileDialog::ScriptFolder { .. })
}

/// Whether the request allows several files.
pub fn allows_many(request: &FileDialog) -> bool {
    matches!(request, FileDialog::Open { many: true, .. } | FileDialog::Script { many: true, .. })
}

/// Whether `entry` can be picked for `request` (folders navigate, so only files are pickable
/// for open dialogs; Folder/save pick the current folder or a named file).
pub fn can_pick(request: &FileDialog, entry: &Entry) -> bool {
    match request {
        FileDialog::Folder | FileDialog::ScriptFolder { .. } | FileDialog::Save { .. } | FileDialog::ScriptSave { .. } => false,
        FileDialog::OpenProject => !entry.is_dir && is_project(&entry.path),
        FileDialog::Open { exts, .. } | FileDialog::Script { exts, .. } => {
            if entry.is_dir {
                return false;
            }
            // A script's `any` filter adds "All Files" beside its own (file_dialogs.rs), so
            // every file is pickable; no extensions means no filter either.
            if exts.is_empty() || matches!(request, FileDialog::Script { any: true, .. }) {
                return true;
            }
            let Some(ext) = entry.path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()) else {
                return false;
            };
            exts.iter().any(|w| w.to_ascii_lowercase() == ext)
        }
    }
}

/// Store a new pending request and wake the UI. The browser starts in `start_dir` (or the
/// first root). Never blocks; I/O errors show inside the browser.
pub fn request(pending: &Arc<Mutex<Option<Pending>>>, ctx: &egui::Context, dialog: FileDialog, done: DialogDone, start_dir: Option<PathBuf>) {
    let current = dialog_start(&dialog)
        .or_else(|| start_dir.filter(|p| p.is_dir()))
        .unwrap_or_else(|| roots(None).into_iter().next().unwrap_or_else(|| PathBuf::from("/")));
    let filename = save_default(&dialog);
    let (entries, error) = match list_dir(&current) {
        Ok(e) => (e, None),
        Err(err) => (Vec::new(), Some(err)),
    };
    let p = Pending { request: dialog, current, entries, error, selected: Vec::new(), filename, done: Some(done) };
    *pending.lock().unwrap_or_else(PoisonError::into_inner) = Some(p);
    ctx.request_repaint();
}

/// Draw the browser window when a request is pending.
pub fn show(pending: &Arc<Mutex<Option<Pending>>>, ctx: &egui::Context) {
    let mut guard = pending.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(p) = guard.as_mut() else {
        return;
    };
    let mut close: Option<Vec<String>> = None;
    let mut navigate: Option<PathBuf> = None;
    let window_title = title(&p.request);
    egui::Window::new(window_title).collapsible(false).resizable(true).min_size(egui::vec2(420.0, 380.0)).show(ctx, |ui| {
        if let Some(err) = p.error.clone() {
            ui.colored_label(egui::Color32::YELLOW, err);
        }
        ui.horizontal(|ui| {
            ui.label(p.current.to_string_lossy().to_string());
            if ui.button("Up").clicked()
                && let Some(parent) = p.current.parent().map(Path::to_path_buf)
            {
                navigate = Some(parent);
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            for i in 0..p.entries.len() {
                let Some(entry) = p.entries.get(i).cloned() else {
                    continue;
                };
                if entry.is_dir {
                    // Tapping a folder enters it (Folder/save pick the current folder
                    // with "Choose here"/"Save" instead).
                    if ui.selectable_label(false, format!("{}/", entry.name)).clicked() {
                        navigate = Some(entry.path.clone());
                    }
                    continue;
                }
                if !can_pick(&p.request, &entry) {
                    ui.add_enabled(false, egui::Label::new(entry.name.clone()));
                    continue;
                }
                let is_selected = p.selected.contains(&entry.path);
                if ui.selectable_label(is_selected, entry.name.clone()).clicked() {
                    if allows_many(&p.request) {
                        if is_selected {
                            p.selected.retain(|s| *s != entry.path);
                        } else {
                            p.selected.push(entry.path.clone());
                        }
                    } else {
                        p.selected.clear();
                        p.selected.push(entry.path.clone());
                    }
                }
            }
        });
        ui.separator();
        if is_save(&p.request) {
            ui.horizontal(|ui| {
                ui.label("Name:");
                ui.text_edit_singleline(&mut p.filename);
            });
        }
        if !p.selected.is_empty() && !is_save(&p.request) && !is_folder(&p.request) {
            let shown: Vec<String> = p.selected.iter().filter_map(|s| s.file_name().and_then(|n| n.to_str()).map(str::to_string)).take(3).collect();
            let mut msg = shown.join(", ");
            if p.selected.len() > 3 {
                msg.push_str(&format!(" +{} more", p.selected.len() - 3));
            }
            ui.label(msg);
        }
        ui.horizontal(|ui| {
            let ok_label = if is_folder(&p.request) {
                "Choose here"
            } else if is_save(&p.request) {
                "Save"
            } else {
                "Open"
            };
            let can_ok = if is_folder(&p.request) {
                true
            } else if is_save(&p.request) {
                !p.filename.trim().is_empty()
            } else {
                !p.selected.is_empty()
            };
            if ui.add_enabled(can_ok, egui::Button::new(ok_label)).clicked() {
                let answer = if is_folder(&p.request) {
                    vec![p.current.to_string_lossy().to_string()]
                } else if is_save(&p.request) {
                    let name = p.filename.trim().to_string();
                    vec![p.current.join(name).to_string_lossy().to_string()]
                } else {
                    p.selected.iter().map(|s| s.to_string_lossy().to_string()).collect()
                };
                close = Some(answer);
            }
            if ui.button("Cancel").clicked() {
                close = Some(Vec::new());
            }
        });
    });
    if let Some(dir) = navigate {
        let (entries, error) = match list_dir(&dir) {
            Ok(e) => (e, None),
            Err(err) => (Vec::new(), Some(err)),
        };
        p.current = dir;
        p.entries = entries;
        p.error = error;
        p.selected.clear();
    }
    if let Some(answer) = close {
        let done = p.done.take();
        drop(guard);
        *pending.lock().unwrap_or_else(PoisonError::into_inner) = None;
        if let Some(done) = done {
            // `DialogDone` answers with the paths, or why the dialog couldn't answer; an empty
            // list is a cancel, never an error (the browser itself shows listing errors).
            done(Ok(answer));
        }
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_filter_passes_matching_extensions_only() {
        let dir = std::env::temp_dir();
        let exts = vec!["mp4".to_string()];
        assert!(can_pick(&FileDialog::Open { exts: exts.clone(), many: false }, &Entry { name: "clip.MP4".into(), path: dir.join("clip.MP4"), is_dir: false }));
        assert!(!can_pick(&FileDialog::Open { exts: exts.clone(), many: true }, &Entry { name: "clip.mov".into(), path: dir.join("clip.mov"), is_dir: false }));
        assert!(!can_pick(&FileDialog::Open { exts, many: false }, &Entry { name: "folder".into(), path: dir.join("folder"), is_dir: true }));
    }

    #[test]
    fn project_detection_is_case_insensitive() {
        assert!(is_project(Path::new("a.ecproj")));
        assert!(is_project(Path::new("a.ECPROJX")));
        assert!(!is_project(Path::new("a.mp4")));
        assert!(!is_project(Path::new("a.ecproj.png")));
    }

    #[test]
    fn save_default_uses_the_file_name_and_clamps() {
        let d = FileDialog::Save { default: "/tmp/Untitled Project.ecproj".into(), ext: "ecproj".into(), project: true };
        assert_eq!(save_default(&d), "Untitled Project.ecproj");
        let long = "x".repeat(300);
        let d = FileDialog::Save { default: long, ext: String::new(), project: false };
        assert!(save_default(&d).len() <= 128);
    }

    #[test]
    fn list_dir_sorts_dirs_first_and_skips_hidden() {
        let base = std::env::temp_dir()
            .join(format!("ec-android-browser-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)));
        let _ = std::fs::create_dir_all(base.join("bdir"));
        let _ = std::fs::create_dir_all(base.join("adir"));
        let _ = std::fs::write(base.join("z.txt"), b"z");
        let _ = std::fs::write(base.join(".hidden"), b"h");
        let entries = list_dir(&base).unwrap_or_default();
        let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&"adir".to_string()));
        assert!(names.contains(&"bdir".to_string()));
        assert!(!names.iter().any(|n| n == ".hidden"));
        let first_file = names.iter().position(|n| n == "z.txt").unwrap_or(usize::MAX);
        let last_dir = names.iter().rposition(|n| *n == "adir" || *n == "bdir").unwrap_or(0);
        assert!(last_dir < first_file);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn roots_start_with_the_data_dir() {
        let data = PathBuf::from("/data/data/x/files/EffectCraft");
        let r = roots(Some(&data));
        assert_eq!(r.first(), Some(&data));
    }

    #[test]
    fn titles_cover_every_dialog_kind() {
        assert_eq!(title(&FileDialog::Open { exts: Vec::new(), many: false }), "Import files");
        assert_eq!(title(&FileDialog::OpenProject), "Open project");
        assert_eq!(title(&FileDialog::Folder), "Choose folder");
        assert_eq!(title(&FileDialog::Save { default: "a.ecproj".into(), ext: "ecproj".into(), project: true }), "Save project");
        assert_eq!(title(&FileDialog::Save { default: "a.png".into(), ext: "png".into(), project: false }), "Save file");
        assert_eq!(
            title(&FileDialog::Script { label: String::new(), exts: Vec::new(), any: false, many: false, prompt: String::new(), start: String::new() }),
            "Open files"
        );
        assert_eq!(
            title(&FileDialog::Script { label: String::new(), exts: Vec::new(), any: false, many: false, prompt: "Pick a plate".into(), start: String::new() }),
            "Pick a plate"
        );
        assert_eq!(title(&FileDialog::ScriptFolder { prompt: String::new(), start: String::new() }), "Choose folder");
        assert_eq!(title(&FileDialog::ScriptFolder { prompt: "Where to write".into(), start: String::new() }), "Where to write");
        assert_eq!(
            title(&FileDialog::ScriptSave { default: String::new(), prompt: String::new(), label: String::new(), exts: Vec::new(), any: false }),
            "Save file"
        );
        assert_eq!(
            title(&FileDialog::ScriptSave { default: String::new(), prompt: "Name the output".into(), label: String::new(), exts: Vec::new(), any: false }),
            "Name the output"
        );
    }

    #[test]
    fn save_and_folder_kinds_answer_their_own_way() {
        assert!(is_save(&FileDialog::Save { default: String::new(), ext: String::new(), project: false }));
        assert!(is_save(&FileDialog::ScriptSave { default: String::new(), prompt: String::new(), label: String::new(), exts: Vec::new(), any: false }));
        assert!(!is_save(&FileDialog::Open { exts: Vec::new(), many: false }));
        assert!(is_folder(&FileDialog::Folder));
        assert!(is_folder(&FileDialog::ScriptFolder { prompt: String::new(), start: String::new() }));
        assert!(!is_folder(&FileDialog::OpenProject));
    }

    #[test]
    fn script_dialogs_filter_and_multi_select_like_the_desktop() {
        let dir = std::env::temp_dir();
        let file = |name: &str| Entry { name: name.into(), path: dir.join(name), is_dir: false };
        let exts = vec!["exr".to_string()];
        let script = |exts: Vec<String>, any: bool, many: bool| FileDialog::Script {
            label: String::new(),
            exts,
            any,
            many,
            prompt: String::new(),
            start: String::new(),
        };
        assert!(can_pick(&script(exts.clone(), false, false), &file("a.EXR")));
        assert!(!can_pick(&script(exts.clone(), false, false), &file("a.png")));
        // `any` adds "All Files" beside the filter, so every file is pickable.
        assert!(can_pick(&script(exts.clone(), true, false), &file("a.png")));
        // No extensions means no filter: every file.
        assert!(can_pick(&script(Vec::new(), false, false), &file("a.png")));
        assert!(allows_many(&script(Vec::new(), false, true)));
        assert!(!allows_many(&script(Vec::new(), false, false)));
        // Folder- and save-like script dialogs never pick a row.
        assert!(!can_pick(&FileDialog::ScriptFolder { prompt: String::new(), start: String::new() }, &file("a.png")));
        assert!(!can_pick(
            &FileDialog::ScriptSave { default: String::new(), prompt: String::new(), label: String::new(), exts: Vec::new(), any: false },
            &file("a.png")
        ));
    }

    #[test]
    fn save_default_covers_a_scripts_save_dialog() {
        let d = FileDialog::ScriptSave { default: "/tmp/shot0001.png".into(), prompt: String::new(), label: String::new(), exts: Vec::new(), any: false };
        assert_eq!(save_default(&d), "shot0001.png");
    }

    #[test]
    fn dialog_start_prefers_the_paths_the_dialog_names() {
        let dir = std::env::temp_dir();
        // A folder path opens there.
        let d = FileDialog::ScriptFolder { prompt: String::new(), start: dir.to_string_lossy().into_owned() };
        assert_eq!(dialog_start(&d), Some(dir.clone()));
        // A file path opens in its folder.
        let file = dir.join("clip.mp4");
        let d = FileDialog::Script {
            label: String::new(),
            exts: Vec::new(),
            any: false,
            many: false,
            prompt: String::new(),
            start: file.to_string_lossy().into_owned(),
        };
        assert_eq!(dialog_start(&d), Some(dir.clone()));
        // Empty, missing or bare-name paths leave the choice to the caller.
        let d = FileDialog::Script { label: String::new(), exts: Vec::new(), any: false, many: false, prompt: String::new(), start: String::new() };
        assert_eq!(dialog_start(&d), None);
        let d = FileDialog::Save { default: "Untitled Project.ecproj".into(), ext: "ecproj".into(), project: true };
        assert_eq!(dialog_start(&d), None);
        let d = FileDialog::Save { default: "/nonexistent-dir-ec/x.mp4".into(), ext: "mp4".into(), project: false };
        assert_eq!(dialog_start(&d), None);
        // The app's own dialogs have no path of their own.
        assert_eq!(dialog_start(&FileDialog::OpenProject), None);
    }
}
