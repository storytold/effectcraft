//! EffectCraft in the browser.
//!
//! The same engine and egui UI as the desktop app, compiled to `wasm32-unknown-unknown` and run
//! by eframe's web runner (WebGPU, WebGL2 fallback). What the desktop shell gets from the OS, this
//! crate gets from the browser:
//!
//! | Desktop | Web |
//! |---|---|
//! | file system (`FsServices`, media reads, export writes) | a virtual file table (`files`) persisted to the Origin Private File System / IndexedDB (`persist`, [`store`]) |
//! | config directory (settings, shortcuts, recent projects, auto-saves) | the same store ([`store::WebConfig`]) |
//! | rfd file dialogs | `<input type=file>` pickers; drop files on the page |
//! | Media Browser on the file system | browser storage and folders opened with the File System Access API (`browse`) |
//! | written files (Save, Render Queue) | browser downloads (several render files as one `.zip`) |
//! | frame render threads | frame workers, each with a project replica fed by diffs and its own WebGPU device (`frames`); `Frames::pump` on the UI thread without workers |
//! | disk cache folder | frames in the Origin Private File System, written by the frame workers (`diskcache`) |
//! | (none) | the storage manager: usage, quota, persistence, clearing (`storage`) |
//! | Render Queue / analysis threads | Web Workers running their own engine instance (`worker`) |
//! | cpal audio output | Web Audio: an AudioWorklet fed from the preview mixdown (`audio`) |
//! | TCP control channel / MCP | `window.effectcraft` JavaScript API (`api`) |
//!
//! [`store`] and [`startup`] are portable (unit-tested natively); everything else is `wasm32`-only.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod startup;
pub mod store;

#[cfg(target_arch = "wasm32")]
pub mod api;
#[cfg(target_arch = "wasm32")]
mod app;
#[cfg(target_arch = "wasm32")]
pub mod audio;
#[cfg(target_arch = "wasm32")]
pub mod browse;
#[cfg(target_arch = "wasm32")]
pub mod config_file;
#[cfg(target_arch = "wasm32")]
pub mod diskcache;
#[cfg(target_arch = "wasm32")]
pub mod files;
#[cfg(target_arch = "wasm32")]
pub mod frames;
#[cfg(target_arch = "wasm32")]
pub mod persist;
#[cfg(target_arch = "wasm32")]
pub mod storage;
#[cfg(target_arch = "wasm32")]
pub mod worker;

#[cfg(target_arch = "wasm32")]
pub use app::*;
