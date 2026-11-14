//! Every native dialog the desktop app opens (rfd), through one path: [`guarded`].
//!
//! [`guarded`] opens a dialog only on the main thread and catches a panic while it is made, so a
//! dialog that can't open becomes an error in the status line instead of taking the app, and
//! the open project, down with it. On macOS rfd makes the panel with `+[NSOpenPanel openPanel]`,
//! and objc2's binding panics when AppKit returns nil there (#3084). That panic used to unwind
//! out of the frame into winit's AppKit callback, which stops the app and re-raises it: the app
//! quit. The engine's last-resort guard (`effectcraft_engine::guard`) didn't catch it because it
//! wraps the commands only; a command that needs a path returns at once and its dialog opens
//! later, at the end of the frame ([`show_asked`]), outside any command.
//!
//! The commands' dialogs don't block the frame (macOS and Linux, `Hooks::file_dialog_async`).
//! A blocking `rfd::FileDialog` runs a nested modal event loop inside eframe's frame, that is
//! inside winit's event handler. On macOS a file dragged over the window while that loop ran
//! re-entered winit's handler, which panics across the Objective-C boundary and aborts the app
//! (#511). On Linux the blocking dialog is worse than slow: rfd uses the XDG desktop portal, so
//! the main thread sits waiting on the portal's answer and never pumps winit's events — the
//! window stops answering the compositor and the desktop offers to force-quit the app while the
//! Open dialog is up (#1275). `rfd::AsyncFileDialog` shows the dialog as a sheet on the window
//! (macOS) or through the portal from a worker thread (Linux) and returns at once; the answer
//! arrives when the dialog closes, outside the frame, and the app runs the command then.
//!
//! The panels' own pickers (Settings paths, Effect Controls, the shortcut editor) still use the
//! blocking hooks [`install_pickers`] sets; they go through [`guarded`] as well.

use effectcraft_ui_egui::{DialogDone, EffectcraftApp, FileDialog};
use rfd::{AsyncFileDialog, FileHandle};
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};

type Answer = Pin<Box<dyn Future<Output = Vec<String>> + Send>>;

/// A command's dialog waiting for [`show_asked`].
pub type Asked = (FileDialog, DialogDone);

/// Open dialog `what` (its title, for the error) with `open`: on the main thread only, and a
/// panic while it opens is an error instead of a crash (#3084).
pub fn guarded<T>(what: &str, open: impl FnOnce() -> T) -> Result<T, String> {
    guarded_on(on_main_thread(), what, open)
}

/// Whether this is the thread AppKit's panels belong to. AppKit makes panels on the main thread
/// only, and rfd would block this thread on the main one to get there; the hooks aren't `Send`,
/// so the app only ever asks from the main thread. Other platforms' dialogs open on any thread.
fn on_main_thread() -> bool {
    #[cfg(target_os = "macos")]
    {
        objc2::MainThreadMarker::new().is_some()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

fn guarded_on<T>(main_thread: bool, what: &str, open: impl FnOnce() -> T) -> Result<T, String> {
    if !main_thread {
        log::error!("{what} dialog: asked for off the main thread");
        return Err(couldnt_open(what, "it was asked for off the main thread"));
    }
    std::panic::catch_unwind(AssertUnwindSafe(open)).map_err(|payload| {
        // The panic hook has logged the panic and where it happened.
        let why = panic_message(payload.as_ref());
        log::error!("{what} dialog: {why}");
        couldnt_open(what, &why)
    })
}

fn couldnt_open(what: &str, why: &str) -> String {
    format!("The system's {what} dialog couldn't be created ({why}). Your project is still open.")
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| payload.downcast_ref::<String>().cloned()).unwrap_or_else(|| "internal error".into())
}

/// Where a blocking picker's error goes: the status line, through `Desktop`, which `ctx`
/// wakes so it shows without waiting for input.
#[derive(Clone)]
struct Errors {
    tx: Sender<String>,
    ctx: egui::Context,
}

/// Run blocking dialog `what`; one that can't open sends why to `errors` and counts as
/// cancelled (the blocking hooks can't return an error).
fn blocking<T>(errors: &Errors, what: &str, open: impl FnOnce() -> Option<T>) -> Option<T> {
    guarded(what, open).unwrap_or_else(|e| {
        let _ = errors.tx.send(e);
        errors.ctx.request_repaint();
        None
    })
}

fn path_string(p: std::path::PathBuf) -> String {
    p.to_string_lossy().to_string()
}

/// Install the blocking pickers (`Hooks::pick_files` and friends), each through [`guarded`].
/// Returns where the errors of those that couldn't open arrive, for the status line; `ctx` is
/// woken when one does.
pub fn install_pickers(app: &mut EffectcraftApp, ctx: &egui::Context) -> Receiver<String> {
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let tx = Errors { tx, ctx: ctx.clone() };
    let errors = tx.clone();
    app.hooks.pick_files = Some(Box::new(move |exts: &[&str]| {
        blocking(&errors, "Open", || rfd::FileDialog::new().add_filter("Media", exts).pick_files()).unwrap_or_default().into_iter().map(path_string).collect()
    }));
    let errors = tx.clone();
    app.hooks.pick_save = Some(Box::new(move |name: &str| {
        blocking(&errors, "Save", || rfd::FileDialog::new().add_filter("EffectCraft Project", &["ecproj"]).set_file_name(name).save_file()).map(path_string)
    }));
    let errors = tx.clone();
    app.hooks.pick_open_project = Some(Box::new(move || {
        blocking(&errors, "Open", || rfd::FileDialog::new().add_filter("EffectCraft Project", &["ecproj", "ecprojx"]).pick_file()).map(path_string)
    }));
    let errors = tx.clone();
    app.hooks.pick_folder = Some(Box::new(move || blocking(&errors, "Choose Folder", || rfd::FileDialog::new().pick_folder()).map(path_string)));
    let errors = tx;
    app.hooks.pick_save_file = Some(Box::new(move |name: &str, ext: &str| {
        blocking(&errors, "Save", || {
            // A default with a folder that exists opens the dialog there.
            let path = std::path::Path::new(name);
            // (No filter for a name without an extension, such as a folder to create.)
            let mut d = if ext.is_empty() { rfd::FileDialog::new() } else { rfd::FileDialog::new().add_filter(ext, &[ext]) };
            if let Some(dir) = path.parent().filter(|d| d.is_dir()) {
                d = d.set_directory(dir);
            }
            let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            d.set_file_name(file).save_file()
        })
        .map(path_string)
    }));
    rx
}

/// An error message box, through [`guarded`] (one that can't open is logged).
pub fn message(title: &str, description: String) {
    let shown = guarded("message", || rfd::MessageDialog::new().set_level(rfd::MessageLevel::Error).set_title(title).set_description(description).show());
    if let Err(e) = shown {
        log::error!("{e}");
    }
}

/// Show the dialogs the commands asked for in this frame (on the main thread: a sheet opens
/// here) and hand each one's paths to its `done` once it closes, then wake the UI.
///
/// On macOS the sheet goes on the main window (`frame`'s). Without a parent rfd puts it on
/// AppKit's main window, which is a floating panel's OS window after a click in one; that
/// window hides while the app has no focused window, and a sheet whose window goes away ends
/// without answering, with later sheets queued behind it on the same window.
pub fn show_asked(asked: &Receiver<Asked>, frame: &eframe::Frame, ctx: &egui::Context) {
    while let Ok((dialog, done)) = asked.try_recv() {
        show(dialog, done, ctx.clone(), |dialog| open(dialog, frame));
    }
}

/// The dialog's title, for errors.
fn title(dialog: &FileDialog) -> &'static str {
    match dialog {
        FileDialog::Open { .. } | FileDialog::OpenProject => "Open",
        FileDialog::Folder => "Choose Folder",
        FileDialog::Save { .. } => "Save",
    }
}

/// Open `dialog` with `open` through [`guarded`] and wait for its answer on a worker thread.
/// `done` always gets an answer: the paths, or why the dialog couldn't open or answer.
fn show(dialog: FileDialog, done: DialogDone, ctx: egui::Context, open: impl FnOnce(FileDialog) -> Answer) {
    let what = title(&dialog);
    let answer = match guarded(what, || open(dialog)) {
        Ok(answer) => answer,
        Err(e) => {
            done(Err(e));
            ctx.request_repaint();
            return;
        }
    };
    // Kept here too, so a worker that can't start still answers.
    let done = Arc::new(Mutex::new(Some(done)));
    let take = |done: &Mutex<Option<DialogDone>>| done.lock().unwrap_or_else(PoisonError::into_inner).take();
    let (worker_done, worker_ctx) = (done.clone(), ctx.clone());
    let spawned = std::thread::Builder::new().name("ec-file-dialog".into()).spawn(move || {
        let paths = std::panic::catch_unwind(AssertUnwindSafe(|| pollster::block_on(answer))).map_err(|payload| {
            let why = panic_message(payload.as_ref());
            log::error!("{what} dialog: {why}");
            format!("The system's {what} dialog stopped without an answer ({why}). Your project is still open.")
        });
        if let Some(done) = take(&worker_done) {
            done(paths);
        }
        worker_ctx.request_repaint();
    });
    if let Err(e) = spawned {
        log::error!("file dialog: {e}");
        if let Some(done) = take(&done) {
            done(Err(couldnt_open(what, &e.to_string())));
        }
        ctx.request_repaint();
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

/// The dialog, as the blocking hooks show it. On macOS rfd makes the panel here, before the
/// future is returned, so a panel AppKit can't make panics here, inside [`guarded`].
///
/// No blocking fallback when that fails: rfd's app-modal dialog makes its panel with the same
/// `+[NSOpenPanel openPanel]`, so it would fail the same way, and it would bring back the
/// nested modal loop of #511. What [`guarded`] can't reach: rfd's own panics inside AppKit's
/// completion handler (its `URL().unwrap()`, or a poisoned lock of its future's state) run
/// under an Objective-C callback and still abort; only rfd can fix those.
fn open(dialog: FileDialog, frame: &eframe::Frame) -> Answer {
    match dialog {
        FileDialog::Open { exts, many } => {
            let d = parented(frame).add_filter("Media", &exts);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    /// The panic objc2 raises when AppKit returns nil for the panel (#3084).
    const NIL_PANEL: &str = "unexpected NULL returned from +[NSOpenPanel openPanel]";

    fn answered(dialog: FileDialog, open: impl FnOnce(FileDialog) -> Answer) -> Result<Vec<String>, String> {
        let (tx, rx) = channel();
        show(dialog, Box::new(move |r| tx.send(r).unwrap()), egui::Context::default(), open);
        rx.recv_timeout(Duration::from_secs(10)).expect("the dialog answers")
    }

    /// #3084: a panel AppKit can't make panics inside rfd; the command's dialog answers with
    /// the error instead, and the panic doesn't leave the frame.
    #[test]
    fn a_panel_that_cannot_be_made_answers_with_an_error() {
        let r = answered(FileDialog::Open { exts: vec!["aep".into(), "aepx".into()], many: false }, |_| panic!("{NIL_PANEL}"));
        let e = r.unwrap_err();
        assert!(e.starts_with("The system's Open dialog couldn't be created") && e.contains(NIL_PANEL) && e.contains("still open"), "{e}");
        let e = answered(FileDialog::Folder, |_| std::panic::panic_any(7_u8)).unwrap_err();
        assert!(e.contains("Choose Folder") && e.contains("internal error"), "{e}");
    }

    /// A dialog that opens answers with its paths from the worker thread; one whose answer
    /// panics answers with an error rather than never.
    #[test]
    fn a_dialog_that_opens_answers_with_its_paths() {
        let r = answered(FileDialog::OpenProject, |_| Box::pin(async { vec!["/a/b.ecproj".to_string()] }));
        assert_eq!(r, Ok(vec!["/a/b.ecproj".to_string()]));
        assert_eq!(answered(FileDialog::save("Frame.png"), |_| Box::pin(async { Vec::new() })), Ok(Vec::new()), "cancelled");
        fn lost() -> Vec<String> {
            panic!("lost the sheet")
        }
        let e = answered(FileDialog::OpenProject, |_| Box::pin(async { lost() })).unwrap_err();
        assert!(e.contains("stopped without an answer") && e.contains("lost the sheet"), "{e}");
    }

    /// Off the main thread a dialog isn't opened at all (AppKit makes panels on the main thread
    /// only); a panic while one opens is an error; an answer passes through.
    #[test]
    fn guarded_opens_on_the_main_thread_and_catches_panics() {
        let mut opened = false;
        let e = guarded_on(false, "Open", || opened = true).unwrap_err();
        assert!(!opened && e.contains("off the main thread"), "{e}");
        assert_eq!(guarded_on(true, "Open", || 5), Ok(5));
        let e = guarded_on(true, "Save", || -> u8 { panic!("{NIL_PANEL}") }).unwrap_err();
        assert!(e.contains("Save") && e.contains(NIL_PANEL), "{e}");
    }

    /// A blocking picker that can't open counts as cancelled and reports why.
    #[test]
    fn a_blocking_picker_that_cannot_open_reports_why() {
        let (tx, rx) = channel();
        let tx = Errors { tx, ctx: egui::Context::default() };
        assert_eq!(blocking(&tx, "Open", || -> Option<u8> { panic!("{NIL_PANEL}") }), None);
        assert!(rx.try_recv().is_ok_and(|e| e.contains(NIL_PANEL)));
        assert_eq!(blocking(&tx, "Open", || Some(3)), Some(3));
        assert!(rx.try_recv().is_err(), "nothing to report");
    }

    /// #3084: every rfd dialog in the desktop app goes through this module (and so through
    /// [`guarded`]); no other source file opens one directly.
    #[test]
    fn every_dialog_opens_through_this_module() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = vec![];
        let mut dirs = vec![src];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    files.push(path);
                }
            }
        }
        assert!(files.len() > 3, "{files:?}");
        let exempt = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("file_dialogs.rs");
        assert!(files.contains(&exempt));
        // `rfd` as a word: a path, a `use`, an alias or `extern crate`.
        let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        for file in files.iter().filter(|f| **f != exempt) {
            let text = std::fs::read_to_string(file).unwrap();
            let named = text.match_indices("rfd").any(|(i, _)| !word(text[..i].chars().next_back()) && !word(text[i + 3..].chars().next()));
            assert!(!named, "{}: open dialogs through `file_dialogs` (#3084)", file.display());
        }
    }
}
