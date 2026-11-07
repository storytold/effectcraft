//! File dialogs that don't block the frame (macOS and Linux), for `Hooks::file_dialog_async`.
//!
//! A blocking `rfd::FileDialog` runs a nested modal event loop inside eframe's frame, that is
//! inside winit's event handler. On macOS a file dragged over the window while that loop ran
//! re-entered winit's handler, which panics across the Objective-C boundary and aborts the app
//! (#511). On Linux the blocking dialog is worse than slow: rfd uses the XDG desktop portal, so
//! the main thread sits waiting on the portal's answer and never pumps winit's events — the
//! window stops answering the compositor and the desktop offers to force-quit the app while the
//! Open dialog is up (#1275). `rfd::AsyncFileDialog` shows the dialog as a sheet on the window
//! (macOS) or through the portal from a worker thread (Linux) and returns at once; the answer
//! arrives when the dialog closes, outside the frame, and the app runs the command then.

use effectcraft_ui_egui::{DialogDone, FileDialog};
use rfd::{AsyncFileDialog, FileHandle};
use std::future::Future;
use std::pin::Pin;

type Answer = Pin<Box<dyn Future<Output = Vec<String>> + Send>>;

/// A command's dialog waiting for [`show_asked`].
pub type Asked = (FileDialog, DialogDone);

/// Show the dialogs the commands asked for in this frame (on the main thread: a sheet opens
/// here) and hand each one's paths to its `done` once it closes, then wake the UI.
///
/// On macOS the sheet goes on the main window (`frame`'s). Without a parent rfd puts it on
/// AppKit's main window, which is a floating panel's OS window after a click in one; that
/// window hides while the app has no focused window, and a sheet whose window goes away ends
/// without answering, with later sheets queued behind it on the same window.
pub fn show_asked(asked: &std::sync::mpsc::Receiver<Asked>, frame: &eframe::Frame, ctx: &egui::Context) {
    while let Ok((dialog, done)) = asked.try_recv() {
        show(dialog, done, frame, ctx.clone());
    }
}

fn show(dialog: FileDialog, done: DialogDone, frame: &eframe::Frame, ctx: egui::Context) {
    let answer = open(dialog, frame);
    let spawned = std::thread::Builder::new().name("ec-file-dialog".into()).spawn(move || {
        done(pollster::block_on(answer));
        ctx.request_repaint();
    });
    if let Err(e) = spawned {
        log::error!("file dialog: {e}");
    }
}

/// A dialog on the main window. Linux keeps the portal's dialog unparented: rfd exports the
/// window for the portal from the dialog's worker thread there.
fn parented(frame: &eframe::Frame) -> AsyncFileDialog {
    let d = AsyncFileDialog::new();
    if cfg!(target_os = "macos") { d.set_parent(frame) } else { d }
}

fn paths(files: Option<Vec<FileHandle>>) -> Vec<String> {
    files.unwrap_or_default().into_iter().map(|f| f.path().to_string_lossy().to_string()).collect()
}

fn path(file: Option<FileHandle>) -> Vec<String> {
    paths(file.map(|f| vec![f]))
}

/// The filters of a script's file dialog (#2778): its own, under the script's name for it (else
/// "Files"), then "All Files" when the script's filter offers every file. Without extensions, no
/// filter: every file. macOS takes all the filters' extensions together as the allowed types, so a
/// dialog that also offers every file gets no filter there.
macro_rules! script_filters {
    ($dialog:expr, $label:expr, $exts:expr, $any:expr) => {{
        let (d, label, exts, any): (_, &str, &[String], bool) = ($dialog, $label, $exts, $any);
        if exts.is_empty() || (any && cfg!(target_os = "macos")) {
            d
        } else {
            let d = d.add_filter(if label.is_empty() { "Files" } else { label }, exts);
            if any { d.add_filter("All Files", &["*"]) } else { d }
        }
    }};
}

/// A script's file dialog, blocking until it closes (`Hooks::pick_file_dialog`, Windows): its
/// paths, empty when cancelled. Other dialogs have their own blocking hooks in `main.rs`.
pub fn pick_blocking(dialog: FileDialog) -> Vec<String> {
    let all =
        |files: Option<Vec<std::path::PathBuf>>| -> Vec<String> { files.unwrap_or_default().into_iter().map(|p| p.to_string_lossy().to_string()).collect() };
    let one = |file: Option<std::path::PathBuf>| all(file.map(|f| vec![f]));
    match dialog {
        FileDialog::Script { label, exts, any, many } => {
            let d = script_filters!(rfd::FileDialog::new(), &label, &exts, any);
            if many { all(d.pick_files()) } else { one(d.pick_file()) }
        }
        FileDialog::Open { exts, many } => {
            let d = if exts.is_empty() { rfd::FileDialog::new() } else { rfd::FileDialog::new().add_filter("Media", &exts) };
            if many { all(d.pick_files()) } else { one(d.pick_file()) }
        }
        FileDialog::OpenProject => one(rfd::FileDialog::new().add_filter("EffectCraft Project", &["ecproj", "ecprojx"]).pick_file()),
        FileDialog::Folder => one(rfd::FileDialog::new().pick_folder()),
        FileDialog::Save { default, ext, project } => {
            let at = std::path::Path::new(&default);
            let name = at.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let mut d = if project {
                rfd::FileDialog::new().add_filter("EffectCraft Project", &["ecproj"])
            } else if ext.is_empty() {
                rfd::FileDialog::new()
            } else {
                rfd::FileDialog::new().add_filter(&ext, &[ext.as_str()])
            };
            if let Some(dir) = at.parent().filter(|d| d.is_dir()) {
                d = d.set_directory(dir);
            }
            one(d.set_file_name(name).save_file())
        }
    }
}

/// The dialog, as the blocking hooks in `main.rs` show it.
fn open(dialog: FileDialog, frame: &eframe::Frame) -> Answer {
    match dialog {
        FileDialog::Open { exts, many } => {
            // No extensions (a script's File.openDialog without a filter): every file.
            let d = if exts.is_empty() { parented(frame) } else { parented(frame).add_filter("Media", &exts) };
            if many {
                let f = d.pick_files();
                Box::pin(async move { paths(f.await) })
            } else {
                let f = d.pick_file();
                Box::pin(async move { path(f.await) })
            }
        }
        FileDialog::Script { label, exts, any, many } => {
            let d = script_filters!(parented(frame), &label, &exts, any);
            if many {
                let f = d.pick_files();
                Box::pin(async move { paths(f.await) })
            } else {
                let f = d.pick_file();
                Box::pin(async move { path(f.await) })
            }
        }
        FileDialog::OpenProject => {
            let f = parented(frame).add_filter("EffectCraft Project", &["ecproj", "ecprojx"]).pick_file();
            Box::pin(async move { path(f.await) })
        }
        FileDialog::Folder => {
            let f = parented(frame).pick_folder();
            Box::pin(async move { path(f.await) })
        }
        FileDialog::Save { default, ext, project } => {
            let at = std::path::Path::new(&default);
            let name = at.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let mut d = if project {
                parented(frame).add_filter("EffectCraft Project", &["ecproj"])
            } else if ext.is_empty() {
                parented(frame)
            } else {
                parented(frame).add_filter(&ext, &[ext.as_str()])
            };
            // A default with a folder that exists opens the dialog there.
            if let Some(dir) = at.parent().filter(|d| d.is_dir()) {
                d = d.set_directory(dir);
            }
            let f = d.set_file_name(name).save_file();
            Box::pin(async move { path(f.await) })
        }
    }
}
