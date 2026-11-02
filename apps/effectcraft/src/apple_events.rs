//! macOS open-documents and quit Apple events.
//!
//! Finder double-clicks, Open With, drops on the Dock icon and `open -a Visualab Effects x.ecproj` don't
//! pass paths on the command line: LaunchServices sends the running (or just-launched) app a
//! `kAEOpenDocuments` ('odoc') Apple event. winit 0.30 doesn't handle it and owns the
//! `NSApplicationDelegate`, so the project never opened. Handling it ourselves needs Objective-C
//! class declarations, i.e. `unsafe`, which this workspace forbids. The audited `fmv-macos-events`
//! crate (also used by PhotoCraft and PdfCraft) wraps exactly that, an `NSAppleEventManager`
//! handler registered before Finder's launch event that leaves winit's delegate alone, behind a
//! safe main-thread API.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fmv_macos_events::{Event, Inbox, Registration};

/// Keeps the Apple-event handlers registered; hold it until the event loop returns.
pub struct AppleEvents {
    _registration: Registration,
    inbox: Inbox,
}

impl AppleEvents {
    /// Register the handlers. Call on the main thread before the event loop starts, so the event
    /// that launched the app (a Finder double-click) is caught too.
    pub fn install() -> Self {
        let (registration, inbox) = Registration::install();
        Self { _registration: registration, inbox }
    }

    /// The queue the app drains every frame ([`Opened::feed`]); events arriving later wake `ctx`.
    pub fn connect(&self, ctx: &egui::Context) -> Opened {
        let ctx = ctx.clone();
        self.inbox.set_wake(move || ctx.request_repaint());
        Opened { inbox: self.inbox.clone(), pending: Vec::new() }
    }
}

/// Documents macOS asked us to open, waiting for the UI to take them.
pub struct Opened {
    inbox: Inbox,
    /// Paths that arrived before the UI was ready (the Finder event that launched the app comes
    /// before the first frame is drawn).
    pending: Vec<PathBuf>,
}

impl Opened {
    /// Hand the documents to the dropped-file handling once the UI is `ready` (a project opens,
    /// asking about unsaved changes first; media is imported), and act on quit requests.
    pub fn feed(&mut self, ready: bool, ctx: &egui::Context, raw: &mut egui::RawInput) {
        for e in self.inbox.drain() {
            match e {
                Event::Open(paths) => self.pending.extend(paths),
                Event::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
        if self.pending.is_empty() {
            return;
        }
        if !ready {
            ctx.request_repaint();
            return;
        }
        raw.dropped_files.extend(self.pending.drain(..).map(|p| Arc::new(OpenedFile(p)) as egui::DroppedFileHandle));
    }
}

/// A document macOS asked us to open, presented to egui as a dropped file.
#[derive(Debug)]
struct OpenedFile(PathBuf);

impl egui::DroppedFile for OpenedFile {
    fn path(&self) -> &Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}
