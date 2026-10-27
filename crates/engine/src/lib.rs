//! The EffectCraft engine façade.
//!
//! Every user-visible action is a command with a stable id (`layer.newSolid`, `prop.set`,
//! `keys.easyEase`…) and JSON parameters, dispatched through [`Session::execute`]. The egui UI,
//! the CLI, the control channel and the MCP server all use this one entry point; that is what
//! makes the UI swappable and the whole app agent-drivable.
//!
//! The project is an `Arc<Project>` edited copy-on-write; undo keeps whole-project snapshots
//! (compositions are `Arc`s, so untouched comps are shared).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod analysis;
mod background;
pub mod cmd;
mod footage;
pub mod history;
mod home;
mod interchange;
pub mod menus;
pub mod preview;
pub mod render_queue;
pub mod scriptui;
mod settings;
mod system;
pub mod viewer;

use std::sync::Arc;

use effectcraft_project::{Comp, ItemId, Layer, LayerId, Project, Uid};
use effectcraft_raster::Image;
use effectcraft_render::{ExprHost, FootageSource, LayerCache, NoFootage, RenderOpts, Renderer};
use effectcraft_time::Tick;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use analysis::camera_track;
pub use analysis::mask_track;
pub use analysis::models;
pub use analysis::roto;
pub use analysis::tracking;
pub use analysis::warp;
pub use background::jobs;
pub use background::offload;
pub use background::remote;
pub use cmd as commands;
pub use cmd::{CommandSpec, command_specs, find as find_command};
pub use effectcraft_color as color;
pub use effectcraft_effects as effects;
pub use effectcraft_geom as geom;
pub use effectcraft_keyframe as keyframe;
pub use effectcraft_project as project;
pub use effectcraft_raster as raster;
pub use effectcraft_render as render;
pub use effectcraft_segment as segment;
pub use effectcraft_text as text;
pub use effectcraft_time as time;
pub use effectcraft_track as track;
pub use footage::doc_path;
pub use footage::footage_check;
pub use footage::footage_reload;
pub use footage::media_browser;
pub use footage::media_cache;
pub use footage::sequence;
pub use history::{Branch, History, HistoryNode};
pub use home::demo;
pub use home::learn;
pub use home::links;
pub use home::templates;
pub use interchange::psd_import;
pub use interchange::vector;
pub use interchange::xml_project;
pub use render_queue::{ExportJob, ExportResult, Exporter, JobState};
pub use settings::autosave;
pub use settings::config;
pub use settings::prefs;
pub use settings::shortcuts;
pub use system::guard;
pub use system::logging;
pub use system::perf;
pub use system::storage;
pub use system::sysinfo;

mod doc_state;
mod error;
mod event;
mod fonts;
mod services;
mod session;

pub use cmd::{animation_presets, text_presets};
pub use doc_state::{CompSelection, EditorState, KeyClip, KeyMove, KeyRef, LinkClip, VertexRef};
pub use error::{EngineError, Result};
pub use event::Event;
pub use fonts::{FontRow, font_features, font_menu, font_preview, font_styles, text_families};
pub use services::{FsServices, Importer, Services};
pub use session::upgrade_effects;

pub struct Session {
    pub project: Arc<Project>,
    pub history: History,
    pub revision: u64,
    /// Process-unique monotonic revision of the on-canvas editor's committed text and session.
    /// Unlike [`Session::revision`], unrelated project edits do not advance it; the IME receipt
    /// uses it when surrounding text is unavailable. Read it with
    /// [`commands::text_edit::ime_stamp_revision`].
    ///
    /// Stamps come from a process-global checked allocator and are never rewound: a failed
    /// atomic batch keeps the highest intermediate stamp. On allocator exhaustion no stamp is
    /// issued, so an unavailable receipt cannot be accepted with a reused revision.
    pub ime_revision: u64,
    pub saved_revision: u64,
    /// The project as last opened or saved: the project is unmodified while it is this very
    /// snapshot, so undoing back to the saved state clears the modified mark.
    pub saved_project: Arc<Project>,
    pub path: Option<String>,
    pub state: EditorState,
    pub services: Arc<dyn Services>,
    pub footage: Arc<dyn FootageSource>,
    pub expr: Option<Arc<dyn ExprHost>>,
    /// GPU compositor (Mercury GPU Acceleration), when the frontend has one: used by renders
    /// that ask for [`effectcraft_render::Backend::Gpu`]/`Auto`, and by Render Queue exports
    /// when the project's renderer is the GPU.
    pub accel: Option<Arc<dyn effectcraft_render::Accelerator>>,
    /// Why there is no [`Session::accel`] (`render.backend`'s `why`): the host's reason, e.g. a
    /// headless start without `--gpu` or the compositor's setup error.
    pub accel_note: Option<String>,
    /// The graphics adapter and backend the window draws with (`NVIDIA GeForce RTX 4070
    /// (Vulkan)`), as the host reports it: the System Compatibility Report shows it.
    pub window_adapter: Option<String>,
    /// Expression syntax checker (set by the host that links the expression engine).
    pub expr_check: Option<fn(&str) -> std::result::Result<(), String>>,
    pub importer: Option<Arc<dyn Importer>>,
    /// Render Queue encoder (the export layer); `None` = export unavailable.
    pub exporter: Option<Arc<dyn Exporter>>,
    /// Free-space hook for Render Settings ▸ Use Storage Overflow (`None`: never full).
    pub storage_quota: Option<Arc<dyn effectcraft_project::render_queue::StorageQuota>>,
    /// The running (or finished, not yet polled) render.
    pub render_job: Option<render_queue::RenderJob>,
    /// The running (or finished, not yet polled) track analysis.
    pub track_job: Option<tracking::TrackJob>,
    /// The running (or finished, not yet polled) mask track.
    pub mask_job: Option<mask_track::MaskTrackJob>,
    /// The running (or finished, not yet polled) Warp Stabilizer analysis.
    pub warp_job: Option<warp::WarpJob>,
    /// Warp Stabilizers waiting for (re-)analysis: (comp, layer, effect uid).
    pub warp_pending: Vec<(ItemId, LayerId, Uid)>,
    /// The running (or finished, not yet polled) 3D Camera Tracker analysis.
    pub camera_job: Option<camera_track::CameraJob>,
    /// 3D Camera Trackers waiting for (re-)analysis: (comp, layer, effect uid).
    pub camera_pending: Vec<(ItemId, LayerId, Uid)>,
    /// The running (or finished, not yet polled) Roto Brush propagation / Freeze.
    pub roto_job: Option<roto::RotoJob>,
    /// Trained models for Roto Brush and face tracking (Settings ▸ Roto Brush / Face Tracking).
    pub models: models::Models,
    /// Where installed models live (hosts set it; else `models` next to the settings).
    pub models_dir: Option<std::path::PathBuf>,
    /// Roto Brush instances edited since their last propagation: (comp, layer, effect uid).
    pub roto_pending: Vec<(ItemId, LayerId, Uid)>,
    pub events: Vec<Event>,
    /// Commands executed: (id, params).
    pub journal: Vec<(String, Value)>,
    /// Processed-layer pixels reused across frames and edits (content-keyed, never stale).
    pub layer_cache: Arc<LayerCache>,
    /// The persistent disk cache (Settings ▸ Media & Disk Cache), when enabled.
    pub disk_cache: Option<Arc<effectcraft_render::disk_cache::DiskCache>>,
    /// Settings (Preferences).
    pub prefs: prefs::Prefs,
    /// Bumped whenever settings change (frontends re-apply theme, labels…).
    pub prefs_revision: u64,
    /// Where settings, shortcut presets and the crash-recovery sentinel are stored (`None` =
    /// nothing persists).
    pub config: Option<Arc<dyn config::ConfigStore>>,
    /// Keyboard shortcut presets.
    pub keymaps: shortcuts::Keymaps,
    /// Scripts' `app.settings`, kept in the settings store (see
    /// [`commands::scripts::SCRIPT_SETTINGS_FILE`]).
    pub script_settings: commands::scripts::ScriptSettings,
    /// Frontend-only commands offered for binding.
    pub ui_commands: Vec<shortcuts::UiCommand>,
    /// Cache of the resolved active preset (read it with [`Session::shortcuts`]).
    pub shortcut_table: std::sync::OnceLock<shortcuts::ShortcutTable>,
    /// Auto-save bookkeeping.
    pub autosave: autosave::AutoSaveState,
    /// Host-owned isolation for all auto-saves (e.g. one MCP session). Independent of settings
    /// and project bookkeeping, so opening/saving a project or changing preferences keeps it.
    pub autosave_folder_override: Option<std::path::PathBuf>,
    /// The viewer snapshot (Take Snapshot / Show Snapshot).
    pub snapshot: Option<viewer::Snapshot>,
    /// The JavaScript scripting engine (set by the host that links `effectcraft-script`):
    /// `script.run`, File ▸ Scripts ▸ Run Script File… (`.jsx`/`.js`) and the Script Console.
    pub script: Option<ScriptRunner>,
    /// Loads WebAssembly effect plug-ins (set by the host that links `effectcraft-plugin` with
    /// its runtime): `effect.plugins.load`.
    pub plugin_loader: Option<PluginLoader>,
    /// ScriptUI windows and panels opened by scripts (see [`scriptui`]).
    pub script_ui: scriptui::ScriptUi,
    /// The last physical-memory reading (Settings ▸ Memory & CPU budgets); `None` = unknown
    /// (headless sessions don't query it, see [`Session::memory_tick`]).
    pub sys_memory: Option<sysinfo::SysMemory>,
    /// The keyframe drag in progress (`keys.move` with a merge key).
    pub key_move: Option<KeyMove>,
    /// Reads the system's memory in the background for [`Session::memory_tick`].
    pub memory_watch: sysinfo::MemoryWatch,
    /// Switches Affect Nested Comps as last applied (a change empties the layer cache).
    pub applied_nested_switches: Option<bool>,
    /// Runs renders and analyses off the UI thread where there are no threads (the web app's
    /// Web Workers, see [`offload`]); `None` = threads (desktop) or inline (wasm32).
    pub offload: Option<Arc<dyn offload::Offload>>,
    /// Analyses running in the [`Session::offload`] worker.
    pub offloaded: Vec<offload::OffloadedJob>,
    /// What the Media Browser browses (`None`: the local file system where there is one).
    pub browser: Option<Arc<dyn media_browser::Browser>>,
    /// The browser's storage (the web app; Settings ▸ Disk ▸ Browser Storage, `storage.*`).
    pub storage: Option<Arc<dyn storage::StorageHost>>,
    /// Offloaded jobs that ended, newest last (kind, error): how a caller waiting for one
    /// (`wait: true` over the control channel in the browser) learns its outcome.
    pub offload_log: Vec<(offload::JobKind, Option<String>)>,
    /// Number of older completion entries evicted from the bounded offload log.
    pub offload_log_start: usize,
    /// Generic background tasks (Content-Aware Fill, Scene Edit Detection), see [`jobs`].
    pub tasks: Vec<jobs::Task>,
    /// Finished tasks, newest last (Progress panel, `jobs.list`).
    pub job_log: Vec<jobs::JobRecord>,
    pub next_task_id: u64,
    /// The running Learn tutorial (Home ▸ Learn), see [`learn`].
    pub learn: Option<learn::Progress>,
    /// Check footage in the background after File ▸ Open ([`footage_check`]): frontends with an
    /// event loop (the desktop app) turn this on; headless sessions run `footage.check` when
    /// they want it.
    pub check_footage_on_open: bool,
    /// Footage paths that came from a document, with that document ([`doc_path`]).
    pub stored_paths: doc_path::StoredPaths,
    /// Each footage file's size and modification time when last seen (Settings ▸ Import ▸
    /// Reload Footage Changed on Disk, [`footage_reload`]).
    pub footage_stamps: std::collections::HashMap<String, footage_reload::Stamp>,
    /// The stamps being taken on a worker thread ([`footage_reload::start_scan`]).
    pub footage_scan: Option<footage_reload::Scan>,
    /// The disk cache's footage salt and the footage it was taken for ([`Session::refresh_disk_salt`]).
    pub disk_salt: Option<(u64, u64)>,
    /// How many script-issued commands are running ([`Session::execute_for_script`]): while one
    /// is, settings that guard what scripts may do can't change (see
    /// [`prefs::Scripting::same_permissions`]).
    pub script_calls: ScriptCalls,
}

/// The count behind [`Session::running_for_script`]; only the engine changes it.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScriptCalls(u32);

/// A thumbnail render ([`Session::thumbnail_job`]): (width, height, RGBA8).
pub type ThumbnailJob = Box<dyn FnOnce() -> Option<(u32, u32, Vec<u8>)> + Send>;

/// A script to run (see [`Session::script`]).
#[derive(Clone, Copy, Debug)]
pub struct ScriptRequest<'a> {
    pub code: &'a str,
    /// File name shown in errors and `$.fileName` (`console` for the Script Console).
    pub name: &'a str,
    /// Run in the Script Console's persistent context (variables survive between runs).
    pub console: bool,
}

/// Runs a script against the session and reports `{ok, result, output, error: {message, line,
/// column, file} | null}`.
pub type ScriptRunner = fn(&mut Session, &ScriptRequest) -> Value;

/// Loads one WebAssembly effect plug-in (`bytes`, `source` path) into the effect registry and
/// describes it (`{id, name, category, version, api, params}`).
pub type PluginLoader = fn(&[u8], &str) -> std::result::Result<Value, String>;

impl Default for Session {
    /// A new unsaved session: a default project that is both current and saved, no path, no
    /// active jobs, the plain `FsServices`/`NoFootage` defaults, default preferences and keymaps,
    /// empty history, events and journal, and a fresh process-wide IME stamp (`u64::MAX` when the
    /// allocator is exhausted).
    fn default() -> Self {
        let project = Arc::new(Project::default());
        Session {
            saved_project: project.clone(),
            project,
            history: History::default(),
            revision: 0,
            // A fresh session gets its own process-monotonic stamp even before its first edit.
            ime_revision: commands::text_edit::next_ime_revision().unwrap_or(u64::MAX),
            saved_revision: 0,
            path: None,
            state: EditorState { snapping: true, view_layout: 1, ..Default::default() },
            services: Arc::new(FsServices),
            footage: Arc::new(NoFootage),
            expr: None,
            accel: None,
            accel_note: None,
            window_adapter: None,
            expr_check: None,
            importer: None,
            exporter: None,
            storage_quota: None,
            render_job: None,
            track_job: None,
            mask_job: None,
            warp_job: None,
            warp_pending: vec![],
            camera_job: None,
            camera_pending: vec![],
            roto_job: None,
            models: Default::default(),
            models_dir: None,
            roto_pending: vec![],
            events: vec![],
            journal: vec![],
            layer_cache: Arc::new(LayerCache::default()),
            disk_cache: None,
            prefs: prefs::Prefs::default(),
            prefs_revision: 0,
            config: None,
            keymaps: shortcuts::Keymaps::default(),
            script_settings: Default::default(),
            ui_commands: vec![],
            shortcut_table: std::sync::OnceLock::new(),
            autosave: autosave::AutoSaveState::default(),
            autosave_folder_override: None,
            snapshot: None,
            script: None,
            plugin_loader: None,
            script_ui: scriptui::ScriptUi::default(),
            sys_memory: None,
            key_move: None,
            memory_watch: Default::default(),
            applied_nested_switches: None,
            offload: None,
            offloaded: vec![],
            offload_log: vec![],
            offload_log_start: 0,
            browser: None,
            storage: None,
            tasks: vec![],
            job_log: vec![],
            next_task_id: 1,
            learn: None,
            check_footage_on_open: false,
            stored_paths: Default::default(),
            footage_stamps: Default::default(),
            disk_salt: None,
            footage_scan: None,
            script_calls: ScriptCalls::default(),
        }
    }
}

#[cfg(test)]
mod rq_tests;
#[cfg(test)]
mod tests;
