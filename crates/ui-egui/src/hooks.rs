use super::*;

/// The desktop's light or dark appearance where egui cannot see it (Wayland compositors that send
/// winit no theme). `None` = the host does not know.
pub type SystemThemeFn = Box<dyn Fn(&egui::Context) -> Option<egui::Theme>>;

/// A file dialog a host shows without blocking the frame ([`Hooks::file_dialog_async`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileDialog {
    /// Files to open or import with these extensions; several when `many`.
    Open { exts: Vec<String>, many: bool },
    /// An EffectCraft project to open.
    OpenProject,
    /// A folder.
    Folder,
    /// Where to save `default` (a file name or path) with extension `ext` (empty: no filter);
    /// `project`: the project save dialog, given the file name.
    Save { default: String, ext: String, project: bool },
}

impl FileDialog {
    /// The save dialog for `default`, as [`Hooks::save_dialog`] picks it.
    pub fn save(default: &str) -> Self {
        let ext = std::path::Path::new(default).extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
        Self::Save { default: default.to_string(), project: ext.eq_ignore_ascii_case("ecproj"), ext }
    }
}

/// Called with the paths a [`FileDialog`] picked (empty: cancelled), from any thread.
pub type DialogDone = Box<dyn FnOnce(Vec<String>) + Send>;

/// Host hooks provided by the native app (file pickers etc.).
#[derive(Default)]
pub struct Hooks {
    /// The system appearance for Settings ▸ Appearance ▸ Sync with System (the desktop app reads
    /// the Linux desktop portal); without it, what egui reports.
    pub system_theme: Option<SystemThemeFn>,
    pub pick_files: Option<Box<dyn Fn(&[&str]) -> Vec<String>>>,
    pub pick_save: Option<Box<dyn Fn(&str) -> Option<String>>>,
    pub pick_open_project: Option<Box<dyn Fn() -> Option<String>>>,
    /// Opens the audio output for preview playback (the desktop app uses cpal).
    pub audio_device: Option<audio::AudioDeviceFactory>,
    /// Lists audio output devices (Settings ▸ Audio).
    pub audio_devices: Option<Box<dyn Fn() -> Vec<String>>>,
    /// Picks a folder (Settings paths).
    pub pick_folder: Option<Box<dyn Fn() -> Option<String>>>,
    /// Save dialog for other file kinds: (default name or path, extension; empty: no filter). A
    /// default path opens the dialog in its folder.
    pub pick_save_file: Option<Box<dyn Fn(&str, &str) -> Option<String>>>,
    /// The system clipboard's text (native menu Edit ▸ Paste into a text field).
    pub clipboard_text: Option<Box<dyn Fn() -> Option<String>>>,
    /// Application actions the OS performs (`app.hide`, `app.hideOthers`, `app.showAll` on
    /// macOS). Returns false when the host doesn't handle the id.
    pub app_action: Option<Box<dyn Fn(&str) -> bool>>,
    /// Shows a command's file dialog without blocking the frame and calls `done` with the
    /// answer (the host then asks for a repaint). Used in place of the blocking pickers above for
    /// the commands' dialogs (File ▸ Import, Open, Save As…) where blocking the frame is unsafe or
    /// leaves the window unresponsive: on macOS a nested modal event loop there aborted the app on
    /// a drag over the window (#511), and on Linux a blocking desktop-portal dialog never pumps
    /// events, so the compositor marks the window as not responding (#1275).
    pub file_dialog_async: Option<Box<dyn Fn(FileDialog, DialogDone)>>,
    /// Tells the user the graphics device was lost and the window is closing, once. `saved` is
    /// the auto-save of unsaved changes (`None`: nothing was unsaved), its path or why it failed.
    pub device_lost: Option<Box<dyn Fn(Option<Result<String, String>>)>>,
}

impl Hooks {
    /// The host's save dialog for `default` (a file name or path), offering its extension: the
    /// project dialog for project files, else [`Hooks::pick_save_file`]. The project dialog is
    /// the fallback for hosts without that (the web, whose dialog only names the file): on macOS
    /// a dialog filtered to projects appends `.ecproj` to any other name (#293). `None` when the
    /// host has no save dialog; `Some(None)` when it was cancelled.
    pub fn save_dialog(&self, default: &str) -> Option<Option<String>> {
        let path = std::path::Path::new(default);
        let ext = path.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
        match (&self.pick_save_file, &self.pick_save) {
            (Some(pick), _) if !ext.eq_ignore_ascii_case("ecproj") => Some(pick(default, &ext)),
            (_, Some(pick)) => Some(pick(&path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())),
            (Some(pick), None) => Some(pick(default, &ext)),
            (None, None) => None,
        }
    }
}
