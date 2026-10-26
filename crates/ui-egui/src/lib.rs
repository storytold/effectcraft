//! The EffectCraft egui frontend (swappable): docking panels, the Composition viewer, the
//! Timeline with property twirl-downs and keyframes, Effect Controls, Effects & Presets, Preview,
//! Info, Character/Paragraph/Align, menus, dialogs and the control channel.
//!
//! Everything goes through `effectcraft_engine::Session::execute` (or `menus::invoke` for
//! frontend-only commands) so every gesture is also available to agents.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod audio;
pub mod automation;
pub mod bench;
pub mod color_picker;
pub mod control;
pub mod credits;
pub mod dock;
mod dock_shared;
pub mod dock_ui;
mod expression_diagnostics;
pub mod frames;
pub mod gpu_failure;
pub mod header;
pub mod i18n;
pub mod icons;
pub mod menu_keys;
pub mod menus;
pub mod native_menu;
pub mod panels;
mod pointer_wrap;
pub mod prefs_live;
pub mod state;
pub mod theme;
pub mod updates_ui;
pub mod widgets;
mod workspace_state;

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};

pub use control::ControlRequest;
use dock::PanelKind;
use effectcraft_engine::Session;
use effectcraft_engine::project::ItemId;
use effectcraft_engine::render::RenderOpts;
use effectcraft_engine::time::Tick;
use frames::{FrameKey, Frames, RenderSource};
use serde_json::json;
use state::UiState;
use theme::Tokens;

/// How long a screenshot request waits for a frame, from admission (app clock seconds).
const SCREENSHOT_TIMEOUT_S: f64 = 8.0;
/// Seconds between a screenshot's admission and the earliest capture: the UI settles first.
const SCREENSHOT_SETTLE_S: f64 = 0.35;
/// Final (non-discarded) UI passes a screenshot waits for after admission before it is issued.
const SCREENSHOT_SETTLE_PASSES: u64 = 3;
/// Geometry changes a queued screenshot may retry before it reports an error.
const SCREENSHOT_MAX_RETRIES: u32 = 4;
/// Outstanding screenshot requests (queued or waiting for their pixels).
const MAX_PENDING_SHOTS: usize = 4;
/// One capture's pixels: 128 MiB.
const MAX_SHOT_BYTES: usize = 128 * 1024 * 1024;
/// Every pending capture's reserved pixels together: 256 MiB.
const MAX_SHOT_TOTAL_BYTES: usize = 256 * 1024 * 1024;
/// Seconds without a project change before paused prefetch resumes (see
/// [`EffectcraftApp::editing`]).
const EDIT_QUIET: f64 = 0.3;

/// The application's monotonic clock, in seconds.
///
/// Neither `ctx.input(|i| i.time)` nor `RawInput::time` is valid in a logic-only tick:
/// `Context::run_logic` copies only the viewport information, so both are the previous full
/// pass's values. Playback, auto-save, screenshot deadlines and the control channel measure time
/// with this clock instead. Native and web use the same [`web_time::Instant`] source; tests
/// inject [`AppClock::manual`] and advance it.
#[derive(Clone, Debug)]
pub struct AppClock {
    origin: web_time::Instant,
    manual: Option<f64>,
}

impl AppClock {
    /// The host's monotonic clock.
    pub fn real() -> Self {
        Self { origin: web_time::Instant::now(), manual: None }
    }

    /// A clock tests advance explicitly with [`AppClock::advance`].
    pub fn manual() -> Self {
        Self { origin: web_time::Instant::now(), manual: Some(0.0) }
    }

    /// Seconds since the clock started (monotonic on native and in the browser).
    pub fn now(&self) -> f64 {
        match self.manual {
            Some(t) => t,
            None => self.origin.elapsed().as_secs_f64(),
        }
    }

    /// Move a manual clock forward; a real clock ignores this.
    pub fn advance(&mut self, seconds: f64) {
        if let Some(t) = &mut self.manual {
            *t = (*t + seconds.max(0.0)).max(0.0);
        }
    }

    /// Whether the clock is test-driven.
    pub fn is_manual(&self) -> bool {
        self.manual.is_some()
    }
}

impl Default for AppClock {
    fn default() -> Self {
        Self::real()
    }
}

/// The root surface geometry of the last completed (non-discarded) UI pass.
///
/// The window position is deliberately excluded: moving the window does not change the pixels
/// a screenshot captures. Size and scale-factor changes advance [`Self::epoch`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawGeometry {
    pub rect: egui::Rect,
    pub ppp: f32,
    /// Bumped whenever `rect.size()` or `ppp` differs from the previous recorded pass.
    pub epoch: u64,
}

/// Whether a captured image's dimensions match `rect` at `ppp`.
///
/// The surface may be smaller than requested when the adapter-limit fit (fork E2) applies, but a
/// different aspect ratio means the pixels came from another layout than the one issued.
fn shot_image_matches(image: &egui::ColorImage, rect: egui::Rect, ppp: f32) -> bool {
    let [w, h] = image.size;
    if w == 0 || h == 0 || !ppp.is_finite() || ppp <= 0.0 {
        return false;
    }
    let ew = (rect.width() as f64 * ppp as f64).max(1.0);
    let eh = (rect.height() as f64 * ppp as f64).max(1.0);
    let want = ew / eh;
    let got = w as f64 / h as f64;
    (got - want).abs() <= 0.02 * want.max(1.0)
}

/// What a `ui.screenshot` captures.
///
/// A panel or element is an identity, not a rectangle: it is resolved again when the capture is
/// issued, after the settle passes, so a panel that moved or resized while the UI settled is
/// captured where it is now (never with the rectangle from request admission).
#[derive(Clone, Debug, PartialEq)]
pub enum ShotTarget {
    /// The whole root surface.
    Window,
    /// A fixed rectangle in logical points, relative to the root surface's top-left corner.
    Rect([f32; 4]),
    /// One dock panel's automation rectangle (`panel.<id>`).
    Panel(PanelKind),
    /// One automation element id's rectangle.
    Id(String),
}

/// One `ui.screenshot` request: its reply, its admission reservation and its completion slot.
struct PendingShot {
    path: Option<String>,
    /// What to capture; resolved afresh at every issuance.
    target: ShotTarget,
    /// The crop resolved at issuance, **relative to the issued rect's origin** (the image's
    /// origin); `None` captures the whole surface.
    crop: Option<[f32; 4]>,
    reply: Sender<serde_json::Value>,
    /// App-clock instant after which the request fails instead of waiting.
    deadline: f64,
    /// App-clock instant before which the UI must have settled.
    settle_at: f64,
    /// Root geometry and geometry epoch the reservation was admitted against.
    admitted: DrawGeometry,
    /// Final UI passes completed when the request was admitted.
    admitted_pass: u64,
    /// The geometry epoch bound to the issued capture command, if it was issued.
    issued: Option<DrawGeometry>,
    /// The shared completion slot; dropping it cancels a late callback.
    slot: Option<craft_ui::capture::CaptureSlot>,
    /// Bytes reserved against [`EffectcraftApp::shot_bytes`].
    reserved_bytes: usize,
    /// Geometry retries used.
    attempts: u32,
}

/// Modal dialogs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialog {
    About,
    NewComp,
    CompSettings,
    SolidSettings,
    CommandPalette,
    /// Settings (Preferences) on `dialog_state.settings_page`.
    Settings,
    /// Edit ▸ Keyboard Shortcuts.
    Shortcuts,
    /// A generic parameter form for a command (`dialog_state.form`).
    Form,
    /// A message (`dialog_state.info`).
    Info,
    /// View ▸ View Options.
    ViewOptions,
    CameraSettings,
    LightSettings,
    NewPrimitive,
    KeyVelocity,
    KeyInterpolation,
    TimeStretch,
    /// Tracker ▸ Options…
    TrackOptions,
    /// Tracker ▸ Edit Target…
    TrackTarget,
    /// Tracker ▸ Apply (Transform / Stabilize): Apply Dimensions.
    TrackApply,
    /// Crash recovery: offer the latest auto-save (`EffectcraftApp::recovery`).
    Recovery,
    /// Composition/Layer Marker (double-click a marker).
    Marker,
    /// Layer ▸ Pre-compose.
    Precompose,
    /// Layer ▸ Layer Styles ▸ Layer Style Options…
    LayerStyles,
    /// Edit ▸ Templates ▸ Render Settings… / Output Module….
    RenderTemplates,
    /// Save changes before closing the project? (`panels::unsaved`).
    UnsavedChanges,
    /// Delete Project items that compositions use? (`panels::delete_items`).
    DeleteItems,
    /// A gradient property's Gradient Editor (`panels::gradient_editor`, `prop.editGradient`).
    GradientEditor,
    /// Delete a user template from Home? (`panels::templates`).
    DeleteTemplate,
}

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

/// A non-blocking file dialog's answer for the command that asked (`menus::dialog_answered`).
pub(crate) struct DialogAnswer {
    pub command: String,
    pub key: &'static str,
    pub params: serde_json::Value,
    pub paths: Vec<String>,
}

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

#[derive(Default)]
pub struct Playback {
    pub playing: bool,
    /// Wall-clock start and the comp time it corresponds to.
    pub start_wall: f64,
    pub start_time: Tick,
    pub start_frame: i64,
    /// Frames shown / dropped (for the Info/Preview readout).
    pub shown: u64,
    pub waiting: bool,
    pub fps: f64,
    /// The Preview panel shortcut that started this preview and what it plays.
    pub shortcut: effectcraft_engine::preview::PreviewShortcut,
    pub plan: Option<effectcraft_engine::preview::PreviewPlan>,
    /// Cache Before Playback: rendering the range before the clock starts.
    pub caching: bool,
    /// Resolution override while playing (pixel step), `None` = the viewer's.
    pub res: Option<u32>,
    /// Include Overlays / Include Layer Controls.
    pub overlays: bool,
    pub layer_controls: bool,
    /// Full Screen: the maximized panel to restore when the preview stops.
    pub restore_max: Option<Option<PanelKind>>,
    /// Audio-only previews: the frame under the audible sample (the current time stays put).
    pub audio_frame: Option<i64>,
    /// The preview's sound waits for frames: while the frames ahead aren't cached, every frame
    /// shows as it renders, silently; the sound starts once the rest of the preview (or what
    /// fits in the RAM preview) is cached, so it never plays over skipped frames (#103), or
    /// sooner once the silent preview keeps real time with the frames ahead cached (#208).
    pub audio_held: bool,
    /// When the last frames were shown (seconds, the last second's worth): the achieved rate.
    pub shown_at: std::collections::VecDeque<f64>,
}

impl Playback {
    /// A new frame went on screen at `now`.
    pub fn frame_shown(&mut self, now: f64) {
        self.shown += 1;
        self.shown_at.push_back(now);
        while self.shown_at.front().is_some_and(|t| now - t > 1.0) {
            self.shown_at.pop_front();
        }
    }

    /// Frames per second shown over the last second while the preview plays (`None` before two
    /// frames were shown, while caching, or stopped).
    pub fn achieved_fps(&self) -> Option<f64> {
        let (a, b) = (self.shown_at.front()?, self.shown_at.back()?);
        (self.playing && !self.caching && self.shown_at.len() > 1 && b > a).then(|| (self.shown_at.len() - 1) as f64 / (b - a))
    }

    /// The preview keeps up with its frame rate (within 5 %; previews that skip frames count
    /// the frames they mean to show).
    pub fn real_time(&self) -> Option<bool> {
        let step = self.plan.map_or(1, |p| p.step).max(1) as f64;
        self.achieved_fps().map(|f| f * step >= self.fps * 0.95)
    }

    /// The preview has kept real time for the last quarter second (at least two frames) up to
    /// `now`: rendering keeps up, so sound can start without waiting for the whole preview.
    pub fn keeps_up(&self, now: f64) -> bool {
        let step = self.plan.map_or(1, |p| p.step).max(1) as f64;
        let frame = step / self.fps.max(1.0);
        let (Some(a), Some(b)) = (self.shown_at.front(), self.shown_at.back()) else { return false };
        self.shown_at.len() >= 3 && b - a >= (2.0 * frame).max(0.25) && now - b <= 2.0 * frame && self.real_time() == Some(true)
    }
}

pub struct EffectcraftApp {
    pub session: Session,
    pub ui: UiState,
    workspace_store: workspace_state::WorkspaceStore,
    pub tokens: Tokens,
    pub auto: automation::Registry,
    pub frames: Frames,
    pub playback: Playback,
    pub dialog: Option<Dialog>,
    pub hooks: Hooks,
    pub fps: f32,
    /// The app clock and the value it had at the last [`EffectcraftApp::tick`].
    clock: AppClock,
    now: f64,
    last_time: f64,
    styled: bool,
    fonts_ready: bool,
    /// The interface language the fonts were installed for; `""` until the first frame.
    styled_language: &'static str,
    /// The root viewport's focus, from the viewport information a logic tick refreshes.
    focused: bool,
    /// Whether the last UI pass saw events or a moving/held pointer.
    input_active: bool,
    /// The pointer was down in the last UI pass (workspace auto-save waits for it to lift).
    last_pointer_down: bool,
    /// Geometry of the last completed, non-discarded UI pass.
    draw_geom: Option<DrawGeometry>,
    /// Bumped when the root surface size or pixels-per-point changes.
    geometry_epoch: u64,
    /// Per-viewport IME owner, generation and change-cause state for the on-canvas text editor.
    pub(crate) ime_tracker: egui::ImeContextTracker,
    /// Completed, non-discarded UI passes since launch.
    ui_pass_seq: u64,
    /// A hidden window must run a UI pass (queued input, screenshots, playback children, …).
    want_paint: bool,
    /// The synthetic batch moved into raw input has not been applied by a UI pass yet.
    synthetic_in_flight: bool,
    /// The events of that batch while egui applies them (never fed to a second pass).
    synthetic_in_flight_events: Vec<egui::Event>,
    /// The viewer scale the last UI pass drew at: playback prefetches without a viewer pass.
    last_viewer_scale: Option<f64>,
    pub(crate) control_rx: Option<Receiver<ControlRequest>>,
    pub(crate) deferred: Vec<(ControlRequest, f64)>,
    pub(crate) synthetic: Vec<egui::Event>,
    pub(crate) input_waiters: Vec<Sender<serde_json::Value>>,
    /// Control requests waiting for an offloaded job (`wait: true` in the browser).
    pub(crate) job_waiters: Vec<(control::JobWaiter, Sender<serde_json::Value>)>,
    pending_screenshots: Vec<PendingShot>,
    /// Pixels reserved by queued and in-flight screenshot requests.
    shot_bytes: usize,
    /// When the user last did something (input or an edit): Cache Frames When Idle waits for
    /// a second of quiet. (time, project revision then)
    last_activity: (f64, u64),
    /// When the project last changed, and its revision then (see [`Self::editing`]).
    last_edit: (f64, u64),
    pub(crate) toast: Option<(String, f64)>,
    /// Texture of the last CPU frame shown in the viewer, the key it came from and the factor it
    /// was averaged down by for the magnification (`panels::viewer::minify_factor`).
    pub(crate) viewer_tex: Option<(egui::TextureHandle, FrameKey, usize)>,
    /// The frames the other (passive) Composition viewers show, by viewer id.
    pub(crate) passive_tex: std::collections::HashMap<u32, (FrameKey, panels::viewers::PassiveTexture)>,
    /// The last GPU frame shown: its egui texture id (registered with egui-wgpu), key and texture.
    pub(crate) viewer_native: Option<(egui::TextureId, FrameKey, Arc<effectcraft_gpu::DisplayFrame>)>,
    /// The texture the viewer draws and its frame key (CPU or GPU frame).
    pub(crate) viewer_shown: Option<(egui::TextureId, FrameKey)>,
    /// Last displayed image (Info panel colour, eyedroppers, histograms). GPU frames fill it on
    /// demand ([`EffectcraftApp::viewer_pixels`]).
    pub(crate) viewer_image: Option<Arc<egui::ColorImage>>,
    /// wasm32: an asynchronous readback of a GPU viewer frame (key asked for, result).
    viewer_readback: (Option<FrameKey>, Arc<std::sync::Mutex<Option<(FrameKey, Arc<egui::ColorImage>)>>>),
    /// egui-wgpu's device, once the first frame arrives (desktop and WebGPU).
    pub(crate) wgpu: Option<eframe::egui_wgpu::RenderState>,
    /// The GPU compositor on that device (None: no usable adapter → CPU only).
    pub(crate) gpu: Option<effectcraft_gpu::Gpu>,
    gpu_checked: bool,
    gpu_failures: Option<gpu_failure::GpuFailureBridge>,
    gpu_failure_applied: Option<bool>,
    /// The graphics device was lost and the window is closing (see `close_after_device_loss`);
    /// `true` keeps crash recovery for the next launch.
    device_lost_close: Option<bool>,
    /// Answers of file dialogs shown without blocking ([`Hooks::file_dialog_async`]).
    pub(crate) dialog_answers: (Sender<DialogAnswer>, Receiver<DialogAnswer>),
    /// Commands from the native menu bar.
    pub command_inbox: Option<Receiver<String>>,
    /// Pointer position in comp pixels (Info panel).
    pub(crate) pointer_comp: Option<[f32; 2]>,
    pub(crate) dialog_state: panels::dialogs::DialogState,
    pub integrated_titlebar: bool,
    /// Audio preview while playing (audio clock drives playback).
    pub audio: Option<audio::AudioPlayback>,
    /// Audio scrubbing's open output (Ctrl/Cmd-drag the current time, `playback.scrubAudio`).
    pub scrub: Option<audio::AudioScrub>,
    pub(crate) updates_rt: updates_ui::Runtime,
    /// Audio panel VU meters.
    pub meter: audio::Meter,
    /// Waveform peak summaries per footage item (see `panels::waveform`).
    pub(crate) expression_diagnostics: expression_diagnostics::Diagnostics,
    pub(crate) waveforms: panels::waveform::Cache,
    /// Last reveal shortcut and when (double-press shortcuts such as LL).
    pub(crate) last_reveal: Option<(String, f64)>,
    /// Settings revision applied to the theme, tooltips and caches.
    /// The settings revision and system appearance (light?) last applied.
    applied_prefs: Option<(u64, Option<bool>)>,
    /// A previous run that didn't exit cleanly (shown by `Dialog::Recovery`).
    pub recovery: Option<effectcraft_engine::autosave::Recovery>,
    /// Docked groups laid out last frame: (active panel, group rect) — `~` maximizes the one
    /// under the pointer.
    pub(crate) dock_rects: Vec<(PanelKind, egui::Rect)>,
    /// Home screen: recent-project thumbnail textures by path (None = no thumbnail).
    pub(crate) home_thumbs: std::collections::HashMap<String, Option<egui::TextureHandle>>,
    /// The (project path, saved revision) whose thumbnail was stored last.
    pub(crate) home_thumb_saved: Option<(String, u64)>,
    /// The window title last sent (project name, `*` while modified).
    window_title: String,
    /// Home ▸ Templates: thumbnail textures by template id (None = not renderable).
    pub(crate) template_thumbs: std::collections::HashMap<String, Option<egui::TextureHandle>>,
    /// Home ▸ Templates: the gallery, and the user template files it was listed from.
    pub(crate) template_list: Option<(Vec<String>, Vec<effectcraft_engine::templates::TemplateInfo>)>,
}

/// The saved workspaces in the settings store ([`effectcraft_engine::config::ConfigStore`]).
pub const WORKSPACES_FILE: &str = "workspaces.json";

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedWorkspaces {
    #[serde(default)]
    workspaces: std::collections::BTreeMap<String, dock::DockNode>,
    #[serde(default)]
    floating: std::collections::BTreeMap<String, Vec<dock::Floating>>,
}

/// The comp frames a playback pass covers (`first..=last`).
#[derive(Clone, Copy, Debug)]
struct FrameWindow {
    first: i64,
    last: i64,
}

/// The inputs of one playback pass: the viewer series it renders, the preview plan and
/// position, and the frame range and clock settings its steps share. `queued` is a snapshot of
/// the in-flight render count that each step copies. Built once in `advance_playback`.
#[derive(Clone, Copy)]
struct PlaybackPass<'a> {
    ctx: &'a egui::Context,
    comp: &'a effectcraft_engine::project::Comp,
    cid: ItemId,
    scale: f64,
    now: f64,
    frame_rate: effectcraft_engine::time::FrameRate,
    series: FrameKey,
    plan: Option<effectcraft_engine::preview::PreviewPlan>,
    window: FrameWindow,
    cur: i64,
    video: bool,
    looping: bool,
    ahead: i64,
    queued: usize,
}

impl EffectcraftApp {
    pub fn new(session: Session) -> Self {
        let mut ui = UiState::default();
        let workspace_store = workspace_state::WorkspaceStore::load(session.config.as_deref(), &mut ui);
        EffectcraftApp {
            session,
            ui,
            workspace_store,
            tokens: Tokens::for_kind(Default::default()),
            auto: Default::default(),
            frames: Frames::default(),
            playback: Playback::default(),
            dialog: None,
            hooks: Hooks::default(),
            fps: 60.0,
            clock: AppClock::default(),
            now: 0.0,
            last_time: 0.0,
            styled: false,
            fonts_ready: false,
            styled_language: "",
            focused: true,
            input_active: false,
            last_pointer_down: false,
            draw_geom: None,
            geometry_epoch: 0,
            ime_tracker: egui::ImeContextTracker::new(),
            ui_pass_seq: 0,
            want_paint: false,
            synthetic_in_flight: false,
            synthetic_in_flight_events: vec![],
            last_viewer_scale: None,
            control_rx: None,
            deferred: vec![],
            synthetic: vec![],
            input_waiters: vec![],
            job_waiters: vec![],
            pending_screenshots: vec![],
            shot_bytes: 0,
            last_activity: (0.0, 0),
            last_edit: (0.0, 0),
            toast: None,
            viewer_tex: None,
            passive_tex: std::collections::HashMap::new(),
            viewer_native: None,
            viewer_shown: None,
            viewer_image: None,
            viewer_readback: Default::default(),
            wgpu: None,
            gpu: None,
            gpu_checked: false,
            gpu_failures: None,
            gpu_failure_applied: None,
            device_lost_close: None,
            dialog_answers: std::sync::mpsc::channel(),
            command_inbox: None,
            pointer_comp: None,
            dialog_state: Default::default(),
            integrated_titlebar: false,
            audio: None,
            scrub: None,
            meter: Default::default(),
            expression_diagnostics: Default::default(),
            waveforms: Default::default(),
            last_reveal: None,
            applied_prefs: None,
            recovery: None,
            dock_rects: vec![],
            home_thumbs: Default::default(),
            home_thumb_saved: None,
            window_title: String::new(),
            template_thumbs: Default::default(),
            template_list: None,
            updates_rt: updates_ui::Runtime::default(),
        }
        .with_ui_commands()
    }

    /// Keep the saved workspaces for the next session (see [`EffectcraftApp::new`]): After
    /// Effects' saved workspaces last until they are deleted (#320). `Err` when they weren't
    /// written (#696): the change applies for this session and a later save retries it, but until
    /// then a restart brings back what is on disk.
    pub(crate) fn store_saved_workspaces(&mut self) -> Result<(), String> {
        let Err(error) = self.workspace_store.save_now(self.session.config.as_deref(), &self.ui) else { return Ok(()) };
        let message = format!("Workspace changes couldn't be saved and may be lost after a restart: {error}");
        log::warn!("{message}");
        self.ui.status = message.clone();
        // The raw error, so the background retry doesn't report the same failure again.
        self.workspace_store.error = Some(error);
        Err(message)
    }

    /// Offer the frontend's commands (tools, timeline navigation…) for keyboard shortcuts.
    fn with_ui_commands(mut self) -> Self {
        let cmds = menus::UI_COMMANDS
            .iter()
            .map(|c| effectcraft_engine::shortcuts::UiCommand { id: c.id.into(), label: c.label.into(), shortcut: c.shortcut.map(str::to_string) })
            .collect();
        self.session.set_ui_commands(cmds);
        self
    }

    /// Show the crash-recovery dialog for a previous run that didn't exit cleanly.
    pub fn offer_recovery(&mut self, r: effectcraft_engine::autosave::Recovery) {
        if r.autosave.is_some() {
            self.recovery = Some(r);
            self.dialog = Some(Dialog::Recovery);
        }
    }

    /// Apply changed settings: theme and brightness, label colours, tool tips, preview caches.
    pub fn apply_prefs(&mut self, ctx: &egui::Context) {
        // Sync with System re-applies when the system appearance changes (the host wakes the UI;
        // nothing polls).
        let system = if self.session.prefs.appearance.appearance_mode == "auto" { self.system_light(ctx) } else { None };
        if self.applied_prefs == Some((self.session.prefs_revision, system)) {
            return;
        }
        self.applied_prefs = Some((self.session.prefs_revision, system));
        let p = &self.session.prefs;
        self.ui.theme = theme::ThemeKind::from_name(p.appearance.resolved_theme(system)).unwrap_or_default();
        self.tokens = Tokens::from_prefs(p, self.ui.theme);
        theme::apply_visuals(ctx, &self.tokens);
        ctx.set_theme(theme::theme_preference(&p.appearance.appearance_mode, self.ui.theme.is_light()));
        let tips = p.general.show_tool_tips;
        ctx.all_styles_mut(|s| s.interaction.tooltip_delay = if tips { 0.5 } else { f32::INFINITY });
        // Settings ▸ Appearance ▸ UI Scale (#284). egui's own Ctrl+= / Ctrl+- / Ctrl+0 zoom stays
        // off: Ctrl+0 is Window ▸ Project, and the scale is a setting.
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        ctx.set_zoom_factor(p.appearance.ui_scale as f32 / 100.0);
        self.ui.cache_when_idle = p.previews.cache_frames_when_idle;
        self.ui.viewer.fast_preview = p.previews.fast_previews;
        self.frames.set_budget(p.cache_budgets(self.session.sys_memory).preview);
    }

    /// Change settings from the UI (applied next frame and saved).
    pub fn set_pref(&mut self, key: &str, value: serde_json::Value) -> Result<(), String> {
        self.session.prefs.set(key, value)?;
        self.session.prefs_changed();
        self.session.save_prefs();
        Ok(())
    }

    pub fn with_control(mut self, rx: Receiver<ControlRequest>) -> Self {
        self.control_rx = Some(rx);
        self
    }

    /// Show theme `k`: the appearance mode becomes its family's (Dark or Light), and `k` that
    /// family's theme.
    pub fn set_theme(&mut self, ctx: &egui::Context, k: theme::ThemeKind) {
        if let Err(e) = self.set_pref("appearance.theme", json!(k.name())) {
            self.ui.status = e;
        }
        self.apply_prefs(ctx);
    }

    /// The system appearance: light (`Some(true)`), dark, or unknown. The host's answer (the Linux
    /// desktop portal) wins over egui's, which Wayland compositors often leave empty.
    pub fn system_light(&self, ctx: &egui::Context) -> Option<bool> {
        self.hooks.system_theme.as_ref().and_then(|read| read(ctx)).or_else(|| ctx.system_theme()).map(|t| t == egui::Theme::Light)
    }

    /// The Tools bar's appearance button (`view.theme.toggle`): Sync with System, then Light, then
    /// Dark, then Sync with System again. The light and dark theme choices are kept.
    pub fn cycle_appearance(&mut self, ctx: &egui::Context) {
        let next = match self.session.prefs.appearance.appearance_mode.as_str() {
            "auto" => "light",
            "light" => "dark",
            _ => "auto",
        };
        if let Err(e) = self.set_pref("appearance.appearanceMode", json!(next)) {
            self.ui.status = e;
        }
        self.apply_prefs(ctx);
    }

    pub fn set_workspace(&mut self, name: &str) {
        // Learn: the Home screen's Learn tab (tutorials) in the Composition panel. Leaving it for
        // another workspace closes the Home screen again: it stayed over every workspace until a
        // project was created (#272).
        if name == "Learn" {
            self.ui.start_screen = true;
            self.ui.home_learn = true;
            self.ui.home_templates = false;
        } else if self.ui.workspace == "Learn" {
            self.ui.start_screen = false;
            self.ui.home_learn = false;
        }
        self.ui.workspace = name.to_string();
        self.ui.dock = self.ui.saved_workspaces.get(name).cloned().unwrap_or_else(|| dock::workspace(name));
        self.ui.floating = self.ui.saved_floating.get(name).cloned().unwrap_or_else(|| dock::workspace_floating(name));
        self.ui.maximized = None;
    }

    /// Built-in workspaces followed by the saved ones (Window ▸ Workspace ▸ Save as New Workspace).
    pub fn workspace_names(&self) -> Vec<String> {
        let mut v: Vec<String> = dock::WORKSPACES.iter().map(|s| s.to_string()).collect();
        v.extend(self.saved_workspace_names());
        v
    }

    /// The workspaces the user saved under a new name (built-ins whose changes were saved are
    /// not repeated), in name order.
    pub fn saved_workspace_names(&self) -> Vec<String> {
        self.ui.saved_workspaces.keys().filter(|k| !dock::WORKSPACES.contains(&k.as_str())).cloned().collect()
    }

    /// The open Gradient Editor's draft (`panels::gradient_editor`): what OK would set.
    pub fn gradient_draft(&self) -> &panels::gradient_editor::Draft {
        &self.dialog_state.gradient
    }

    /// The open Gradient Editor's draft, to edit (tests and automation; the editor's own fields
    /// do the same).
    pub fn gradient_draft_mut(&mut self) -> &mut panels::gradient_editor::Draft {
        &mut self.dialog_state.gradient
    }

    /// Feed this frame's share of the queued synthetic input (control channel `ui.click`,
    /// `ui.drag`…) into `raw_input`. Pointer events go one per frame so egui sees press → moves
    /// → release as a real gesture.
    ///
    /// Only one batch is ever moved out at a time, even when the host prepares several hidden
    /// logic-only iterations before the forced UI pass: eframe keeps the last prepared raw input
    /// and would otherwise apply a whole drag or click as one burst. A batch is released by the
    /// first UI pass that applies it ([`Self::mark_synthetic_applied`]). Callers must not feed a
    /// pass that cannot draw (fonts still installing): egui applies what it receives, so putting
    /// an applied batch back would apply its pointer/key state twice.
    pub fn feed_synthetic(&mut self, raw_input: &mut egui::RawInput) {
        if self.synthetic_in_flight || self.synthetic.is_empty() {
            return;
        }
        // Pointer events go one per frame so egui sees press → moves → release as a real drag.
        let pointer = |e: &egui::Event| matches!(e, egui::Event::PointerMoved(_) | egui::Event::PointerButton { .. } | egui::Event::MouseWheel { .. });
        let n = if pointer(&self.synthetic[0]) {
            1
        } else {
            self.synthetic
                .iter()
                .position(|e| pointer(e) || matches!(e, egui::Event::Key { pressed: false, .. }))
                .map_or(self.synthetic.len(), |i| if pointer(&self.synthetic[i]) { i.max(1) } else { i + 1 })
        };
        let batch: Vec<egui::Event> = self.synthetic.drain(..n).collect();
        raw_input.events.extend(batch.iter().cloned());
        self.synthetic_in_flight_events.extend(batch);
        self.synthetic_in_flight = true;
    }

    /// Clear the single-pending-batch guard: a UI pass has applied the injected batch.
    ///
    /// [`EffectcraftApp::draw`] calls this itself; harnesses that feed synthetic input
    /// themselves (the headless kittest tests) call it after each pass they ran.
    pub fn mark_synthetic_applied(&mut self) {
        self.synthetic_in_flight_events.clear();
        self.synthetic_in_flight = false;
    }

    /// The events of the injected batch that no UI pass has applied yet.
    pub fn pending_synthetic_events(&self) -> &[egui::Event] {
        &self.synthetic_in_flight_events
    }

    /// Whether a synthetic batch is waiting for its UI pass.
    pub fn synthetic_in_flight(&self) -> bool {
        self.synthetic_in_flight
    }

    /// The app clock's value at the last tick.
    pub fn now(&self) -> f64 {
        self.now
    }

    /// The clock itself (tests advance a manual clock).
    pub fn clock_mut(&mut self) -> &mut AppClock {
        &mut self.clock
    }

    /// Use a test-driven clock from now on.
    pub fn use_manual_clock(&mut self) {
        self.clock = AppClock::manual();
        self.now = 0.0;
    }

    /// Completed, non-discarded UI passes since launch.
    pub fn ui_passes(&self) -> u64 {
        self.ui_pass_seq
    }

    /// The geometry of the last completed UI pass, if one ran.
    pub fn draw_geometry(&self) -> Option<DrawGeometry> {
        self.draw_geom
    }

    /// Show panel `p` (opening it in its usual place if it is closed), bring it to the front and
    /// give it the focus: Window ▸ <panel>.
    pub fn show_panel(&mut self, p: PanelKind) {
        if !self.ui.floating.iter().any(|f| f.panels.contains(&p)) && self.ui.maximized.is_some_and(|m| m != p) {
            self.ui.maximized = None;
        }
        self.raise_panel(p);
        self.ui.focused = p;
    }

    /// Bring panel `p` to the front of its group, opening it in its usual place if it is closed,
    /// without moving the focus (a comp opening brings up its viewer, an effect applied brings up
    /// Effect Controls).
    pub fn raise_panel(&mut self, p: PanelKind) {
        if self.ui.floating.iter().any(|group| group.panels.contains(&p)) {
            self.edit_layout(|layout| layout.apply(craft_ui::docking::Action::Activate { panel: p }).is_ok());
            return;
        }
        if !self.ui.dock.contains(p) {
            // The Composition panel lives in the centre, with the Layer panel and other viewers;
            // with none of those left, above the Timeline.
            let centre = |d: &dock::DockNode| {
                let mut v = Vec::new();
                d.panels(&mut v);
                v.into_iter().find(|q| matches!(q, PanelKind::Layer | PanelKind::Viewer(_) | PanelKind::Flowchart | PanelKind::Footage))
            };
            let near = match p {
                PanelKind::Composition => match centre(&self.ui.dock) {
                    Some(q) => q,
                    None if self.ui.dock.dock_panel(p, PanelKind::Timeline, dock::Zone::Top) => {
                        self.ui.dock.activate(p);
                        return;
                    }
                    None => PanelKind::EffectsPresets,
                },
                PanelKind::Layer | PanelKind::Flowchart | PanelKind::Viewer(_) => PanelKind::Composition,
                PanelKind::RenderQueue => PanelKind::Timeline,
                PanelKind::EffectControls | PanelKind::History => PanelKind::Project,
                _ => PanelKind::EffectsPresets,
            };
            self.ui.dock.open_near(p, near);
        }
        self.ui.dock.activate(p);
    }

    pub fn render_source(&self) -> RenderSource {
        RenderSource {
            project: self.session.project.clone(),
            footage: self.session.footage.clone(),
            expr: self.session.expr.clone(),
            layer_cache: self.session.layer_cache.clone(),
            gpu: self.gpu.clone(),
            gpu_display: self.gpu_display(),
            disk: self.session.disk_cache.clone(),
        }
    }

    /// Keep GPU-composited viewer frames on the GPU (drawn from their texture). The browser can't
    /// read frames back (its thread can't wait for the GPU), so there the viewer shows GPU frames
    /// directly whenever it draws them plainly (RGB, no exposure, no region of interest); the
    /// desktop reads them back (see [`frames::RenderSource::gpu_display`]).
    fn gpu_display(&self) -> bool {
        let v = &self.session.state.viewer;
        cfg!(target_arch = "wasm32")
            && v.channel == effectcraft_engine::viewer::Channel::Rgb
            && v.exposure == 0.0
            && self.session.state.region_of_interest.is_none()
            && self.ui.viewer.extended.is_none()
    }

    /// The UI draws its panels and handles input such as dropped files. Until the fonts are
    /// installed (the first frames) it draws nothing, and that input goes unhandled.
    pub fn is_ready(&self) -> bool {
        self.fonts_ready
    }

    /// Provide the executable/embedding host's device notifications. The host installs the
    /// handlers once; this frontend never replaces handlers on a borrowed device.
    pub fn set_gpu_failure_bridge(&mut self, bridge: gpu_failure::GpuFailureBridge) {
        if let Some(gpu) = &self.gpu {
            bridge.forward_to(gpu.context().device_failure_notifier());
        }
        self.gpu_failures = Some(bridge);
    }

    fn apply_gpu_failure(&mut self, ctx: &egui::Context) {
        let Some(bridge) = &self.gpu_failures else { return };
        // Readback deadlines can retire the compositor without a host device notification.
        if let Some(gpu) = &self.gpu
            && let Err(reason) = gpu.context().check_health()
        {
            bridge.report(&reason, false);
        }
        let Some(failure) = bridge.failure() else { return };
        if self.gpu_failure_applied == Some(failure.presentation_lost) {
            return;
        }
        self.gpu_failure_applied = Some(failure.presentation_lost);
        self.gpu_checked = true;
        self.frames.retire_gpu();
        self.session.accel = None;
        self.session.accel_note = Some(format!("GPU preview retired: {}", failure.reason));
        self.session.layer_cache.clear();
        self.gpu = None;
        if let Some(rs) = &self.wgpu {
            let mut renderer = rs.renderer.write();
            if let Some((id, _, _)) = &self.viewer_native {
                renderer.free_texture(id);
            }
            for (_, texture) in self.passive_tex.values() {
                if let panels::viewers::PassiveTexture::Gpu(id, _) = texture {
                    renderer.free_texture(id);
                }
            }
        }
        self.viewer_native = None;
        self.passive_tex.retain(|_, (_, texture)| matches!(texture, panels::viewers::PassiveTexture::Cpu(_)));
        self.viewer_shown = self.viewer_tex.as_ref().map(|(texture, key, _)| (texture.id(), *key));
        self.viewer_image = None;
        // Old completion closures retain only their detached slot, not the replacement.
        self.viewer_readback = Default::default();
        self.wgpu = None;
        self.ui.status = if failure.presentation_lost {
            "Graphics device lost. The project is still open; presentation requires restarting the application.".into()
        } else {
            "Shared GPU preview disabled; CPU compositing remains available.".into()
        };
        log::warn!("GPU preview retired: {}", failure.reason);
        ctx.request_repaint();
    }

    /// After the graphics device is lost, eframe's renderer cannot draw again: any mesh makes
    /// egui-wgpu panic on the main thread when it cannot stage its index buffer (#591). Draw
    /// nothing from then on, auto-save a project with unsaved changes (keeping the crash
    /// recovery marker, so the next launch offers it) and close the window.
    fn close_after_device_loss(&mut self, ctx: &egui::Context) {
        if self.device_lost_close.is_some() {
            return;
        }
        let keep_recovery = self.session.is_dirty();
        let saved = keep_recovery.then(|| match self.session.autosave_now().and_then(|path| self.session.autosave_wait().map(|()| path)) {
            Ok(path) => {
                log::warn!("graphics device lost: auto-saved {path}; the next launch offers to recover it");
                Ok(path)
            }
            Err(e) => {
                log::error!("graphics device lost: {e}");
                Err(e.to_string())
            }
        });
        self.device_lost_close = Some(keep_recovery);
        log::warn!("graphics device lost: closing the window");
        if let Some(tell) = &self.hooks.device_lost {
            tell(saved);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Run the commands whose non-blocking file dialogs have been answered.
    pub(crate) fn run_dialog_answers(&mut self) {
        while let Ok(answer) = self.dialog_answers.1.try_recv() {
            menus::dialog_answered(self, answer);
        }
    }

    /// Set up the GPU compositor on egui-wgpu's device (once). Without an adapter that runs
    /// compute shaders (WebGL2, software GL) everything stays on the CPU.
    pub(crate) fn init_gpu(&mut self, frame: &eframe::Frame) {
        if self.gpu_checked {
            return;
        }
        self.gpu_checked = true;
        if self.gpu_failures.as_ref().is_some_and(|bridge| bridge.failure().is_some()) {
            return;
        }
        let Some(rs) = frame.wgpu_render_state() else { return };
        // Help ▸ System Compatibility Report names the adapter and backend the window got.
        let info = rs.adapter.get_info();
        self.session.window_adapter = Some(format!(
            "{} ({:?}, {:?}{})",
            info.name,
            info.backend,
            info.device_type,
            if info.driver.is_empty() { String::new() } else { format!(", {} {}", info.driver, info.driver_info) }
        ));
        // Backend validation can panic on shader compilation even when device creation
        // succeeded. Keep the document and the egui renderer, and use CPU compositing.
        let gpu = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| effectcraft_gpu::Gpu::new(&rs.adapter, rs.device.clone(), rs.queue.clone())));
        match gpu {
            Ok(Ok(g)) => {
                if let Some(bridge) = &self.gpu_failures {
                    bridge.forward_to(g.context().device_failure_notifier());
                    if bridge.failure().is_some() {
                        return;
                    }
                }
                log::info!("GPU compositor: {}", effectcraft_engine::render::Accelerator::name(&g));
                self.session.accel = Some(Arc::new(g.clone()));
                self.gpu = Some(g);
                self.wgpu = Some(rs.clone());
            }
            Ok(Err(e)) => {
                log::info!("GPU compositor unavailable: {e}");
                self.session.accel_note = Some(format!("GPU compositor unavailable: {e}"));
            }
            Err(e) => {
                let reason = e.downcast_ref::<String>().map(String::as_str).or_else(|| e.downcast_ref::<&str>().copied()).unwrap_or("backend panicked");
                log::warn!("GPU compositor setup failed; using CPU compositing: {reason}");
                self.session.accel_note = Some(format!("GPU compositor setup failed: {reason}"));
            }
        }
    }

    /// The viewer shows a frame composited on the GPU (drawn from its wgpu texture).
    pub fn viewer_on_gpu(&self) -> bool {
        matches!((&self.viewer_shown, &self.viewer_native), (Some(s), Some(n)) if s.1 == n.1)
    }

    /// The GPU compositor's adapter, when the viewer has one.
    pub fn gpu_adapter(&self) -> Option<String> {
        self.gpu.as_ref().map(effectcraft_engine::render::Accelerator::name)
    }

    /// The size of the viewer frame's texture (CPU frames; fitted to the GPU's texture limit).
    pub fn viewer_texture_size(&self) -> Option<[usize; 2]> {
        self.viewer_tex.as_ref().map(|(t, _, _)| t.size())
    }

    /// The viewer's current pixels as 8-bit premultiplied RGBA, reading a GPU frame back the
    /// first time something asks for it.
    ///
    /// The browser can't wait for the GPU: there the first call starts an asynchronous readback
    /// and returns `None`; a later call (the next frame) has the pixels.
    pub fn viewer_pixels(&mut self) -> Option<Arc<egui::ColorImage>> {
        let to_image = |bytes: Vec<u8>, w: u32, h: u32| {
            let px = bytes.as_chunks::<4>().0.iter().map(|c| egui::Color32::from_rgba_premultiplied(c[0], c[1], c[2], c[3])).collect();
            Arc::new(egui::ColorImage::new([w as usize, h as usize], px))
        };
        if self.viewer_image.is_none()
            && let (Some((_, key, f)), Some(g)) = (&self.viewer_native, &self.gpu)
            && self.viewer_shown.as_ref().map(|s| s.1) == Some(*key)
        {
            if cfg!(target_arch = "wasm32") {
                let ready = self.viewer_readback.1.lock().ok().and_then(|mut r| r.take_if(|r| r.0 == *key));
                if let Some((_, img)) = ready {
                    self.viewer_image = Some(img);
                } else if self.viewer_readback.0 != Some(*key) {
                    self.viewer_readback.0 = Some(*key);
                    let (slot, k, (w, h)) = (self.viewer_readback.1.clone(), *key, (f.width, f.height));
                    let ctx = self.frames.context();
                    g.read_display_async(f, move |bytes| {
                        if let (Some(b), Ok(mut s)) = (bytes, slot.lock()) {
                            *s = Some((k, to_image(b, w, h)));
                        }
                        if let Some(c) = ctx {
                            c.request_repaint();
                        }
                    });
                }
            } else if let Some(bytes) = g.read_display(f) {
                self.viewer_image = Some(to_image(bytes, f.width, f.height));
            }
        }
        self.viewer_image.clone()
    }

    /// The viewer's Resolution: the active composition's (saved with the project).
    pub fn viewer_res(&self) -> state::Resolution {
        self.session.active_comp().map(|c| c.resolution).unwrap_or_default()
    }

    /// The render scale used by the viewer right now.
    pub fn viewer_scale(&self, zoom: f32, ppp: f32) -> f64 {
        // The preview's Resolution (when not Auto) while it plays.
        if self.playback.playing
            && let Some(f) = self.playback.res
        {
            return state::Resolution::from_factor(f.into()).scale(zoom, ppp);
        }
        // Auto follows the zoom alike while playing and paused, so frames cached while scrubbing
        // play back, and frames cached by a preview stay on screen when it stops.
        let res = self.viewer_res();
        let full = res.scale(zoom, ppp);
        // Fast Previews ▸ Adaptive Resolution / Fast Draft: lower resolution while dragging.
        let (_, k) = self.session.state.viewer.fast_previews.render(self.ui.viewer.interacting || self.ui.viewer.property_interacting);
        if k < 1.0 && res.is_auto() {
            return full.min((full * 0.5).max(self.session.prefs.adaptive_limit()));
        }
        full
    }

    /// The cache key of viewer frame `frame` of `comp` at `scale` (see [`Self::frame_series`]).
    pub fn frame_key(&self, comp: ItemId, frame: i64, scale: f64) -> FrameKey {
        FrameKey { frame, ..self.frame_series(comp, scale) }
    }

    /// The key of frame 0 of `comp`'s viewer frames at `scale` (the content, view and render
    /// options are the same for every frame: loops compute this once).
    pub fn frame_series(&self, comp: ItemId, scale: f64) -> FrameKey {
        FrameKey {
            revision: self.session.revision,
            content: self.frames.content_of(&self.session.project, self.session.revision, comp),
            comp: comp.0,
            frame: 0,
            scale: (scale * 1000.0).round() as u32,
            view: self.view_hash(comp),
            opts: frames::opts_hash(&self.frame_opts(comp, scale)),
        }
    }

    /// The frame series the viewer of `comp` shows now (its scale, view and options, for the
    /// comp's current content): what the cache bars count.
    pub fn shown_series(&self, comp: ItemId) -> FrameKey {
        let scale = self.viewer_shown.as_ref().filter(|(_, k)| k.comp == comp.0).map_or(1000, |(_, k)| k.scale);
        self.frame_series(comp, scale as f64 / 1000.0)
    }

    /// Hash of the comp viewer's 3D view camera (0 for the active camera view).
    pub fn view_hash(&self, comp: ItemId) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        // The region of interest changes what is rendered too.
        let roi = self.viewer_region(comp);
        if let Some(r) = roi {
            r.map(f64::to_bits).hash(&mut h);
        }
        let Some(cam) = self.session.view_camera(comp) else { return if roi.is_some() { h.finish() | 1 } else { 0 } };
        for row in cam.view.0 {
            for v in row {
                v.to_bits().hash(&mut h);
            }
        }
        cam.zoom.to_bits().hash(&mut h);
        cam.ortho.hash(&mut h);
        h.finish() | 1
    }

    /// The comp-space region viewer frames of `comp` cover: the region of interest, else the
    /// Extended Viewer's area (frame plus pasteboard), else `None` (the whole frame).
    pub fn viewer_region(&self, comp: ItemId) -> Option<[f64; 4]> {
        if self.session.active_comp_id() != Some(comp) {
            return None;
        }
        self.session.state.region_of_interest.or(self.ui.viewer.extended)
    }

    /// Request a prefetch frame render (no-op if cached/in flight).
    pub fn request_frame(&self, comp: ItemId, frame: i64, scale: f64) {
        self.request_frame_with(comp, frame, scale, false);
    }

    /// Request the frame on screen: rendered before any queued prefetch.
    pub fn request_frame_urgent(&self, comp: ItemId, frame: i64, scale: f64) {
        self.request_frame_with(comp, frame, scale, true);
    }

    /// Render options of viewer frames of `comp` at `scale`.
    pub fn frame_opts(&self, comp: ItemId, scale: f64) -> RenderOpts {
        let (draft, _) = self.session.state.viewer.fast_previews.render(self.ui.viewer.interacting || self.ui.viewer.property_interacting);
        let roi = self.viewer_region(comp);
        RenderOpts {
            scale,
            motion_blur: true,
            guides: true,
            draft: self.ui.viewer.fast_preview || draft,
            view: self.session.view_camera(comp),
            roi,
            backend: effectcraft_engine::render::Backend::Auto,
            nested_switches: self.session.prefs.general.switches_affect_nested_comps,
            draft_shadows: self.session.prefs.three_d.realtime_shadows,
            proxy: Default::default(),
        }
    }

    fn request_frame_with(&self, comp: ItemId, frame: i64, scale: f64, urgent: bool) {
        let Some(c) = self.session.project.comp(comp) else { return };
        let key = self.frame_key(comp, frame, scale);
        let t = c.frame_rate.tick_of(frame);
        let opts = self.frame_opts(comp, scale);
        if urgent {
            self.frames.request_urgent(&self.render_source(), key, comp, t, opts);
        } else {
            self.frames.request(&self.render_source(), key, comp, t, opts);
        }
    }

    // ---------------------------------------------------------------- playback

    pub fn play(&mut self, now: f64) {
        let sc = self.session.prefs.preview.current;
        self.play_with(now, sc);
    }

    /// Start the preview of a Preview panel shortcut (its range, play-from, rate, skip,
    /// resolution, loop, caching, full screen and include options).
    pub fn play_with(&mut self, now: f64, sc: effectcraft_engine::preview::PreviewShortcut) {
        let Some(c) = self.session.active_comp_arc() else { return };
        let preset = self.session.prefs.preview.get(sc).clone();
        let plan = effectcraft_engine::preview::plan(&preset, &c, self.session.time());
        let fr = c.frame_rate;
        if plan.video {
            self.session.set_time(fr.tick_of(plan.first));
        }
        let restore_max = preset.full_screen.then_some(self.ui.maximized);
        let viewer = panels::viewers::active_panel(self);
        if preset.full_screen && self.ui.dock.contains(viewer) {
            self.ui.maximized = Some(viewer);
        }
        self.playback = Playback {
            playing: true,
            start_wall: now,
            start_time: fr.tick_of(plan.first),
            start_frame: plan.first,
            shown: 0,
            waiting: false,
            fps: plan.fps,
            shortcut: sc,
            plan: Some(plan),
            caching: plan.cache_first && plan.video,
            res: preset.resolution.factor(preset.custom_resolution),
            overlays: preset.include_overlays,
            layer_controls: preset.include_layer_controls,
            restore_max,
            audio_frame: (!plan.video).then_some(plan.first),
            audio_held: false,
            shown_at: Default::default(),
        };
        if !self.playback.caching {
            if plan.video {
                self.playback.audio_held = self.audio_plays();
            } else {
                self.start_audio(fr.tick_of(plan.first));
            }
        }
    }

    /// The preview includes audio at the comp's frame rate and the comp has something audible.
    fn audio_plays(&self) -> bool {
        let (Some(pl), Some(cid)) = (self.playback.plan, self.session.active_comp_id()) else { return false };
        let Some(c) = self.session.project.comp(cid) else { return false };
        // A preview at another frame rate plays slower or faster than real time: silent.
        pl.audio && (pl.fps - c.frame_rate.as_f64()).abs() <= 0.01 && effectcraft_engine::render::audio::comp_has_audio(&self.session.project, cid)
    }

    /// Start audio preview from comp time `t` when [`Self::audio_plays`] and an output device
    /// opens.
    fn start_audio(&mut self, t: Tick) {
        self.audio = None;
        self.meter.clipped = [false; 2];
        let (Some(pl), Some(cid)) = (self.playback.plan, self.session.active_comp_id()) else { return };
        let Some(c) = self.session.project.comp_arc(cid).filter(|_| self.audio_plays()) else { return };
        let out = audio::AudioOutput::from_prefs(&self.session.prefs);
        let Some(dev) = self.hooks.audio_device.as_ref().and_then(|f| f(&out)) else { return };
        let mix = self.session.prefs.audio.preview_sample_rate;
        let fr = c.frame_rate;
        match audio::AudioPlayback::start(dev, self.render_source(), cid, t, fr.tick_of(pl.start), fr.tick_of(pl.end + 1), pl.looping, mix) {
            Ok(a) => self.audio = Some(a),
            Err(e) => log::warn!("audio preview: {e}"),
        }
    }

    /// Audio scrubbing: play the frame of `comp` at `t` (see [`audio::AudioScrub`]), opening the
    /// output on first use. Silent comps and missing devices do nothing.
    pub fn scrub_audio(&mut self, comp: ItemId, t: Tick, now: f64) {
        if self.playback.playing || !effectcraft_engine::render::audio::comp_has_audio(&self.session.project, comp) {
            return;
        }
        if self.scrub.is_none() {
            let out = audio::AudioOutput::from_prefs(&self.session.prefs);
            let Some(dev) = self.hooks.audio_device.as_ref().and_then(|f| f(&out)) else { return };
            match audio::AudioScrub::open(dev, now) {
                Ok(s) => self.scrub = Some(s),
                Err(e) => {
                    log::warn!("audio scrubbing: {e}");
                    return;
                }
            }
        }
        let src = self.render_source();
        let mix = self.session.prefs.audio.preview_sample_rate;
        if let Some(s) = &mut self.scrub {
            s.play(&src, comp, t, mix, now);
        }
    }

    pub fn stop(&mut self) {
        let was = std::mem::replace(&mut self.playback.playing, false);
        self.audio = None;
        self.playback.caching = false;
        if !was {
            return;
        }
        if let Some(m) = self.playback.restore_max.take() {
            self.ui.maximized = m;
        }
        let Some(pl) = self.playback.plan else { return };
        let move_time = self.session.prefs.preview.get(self.playback.shortcut).move_time_to_preview_time;
        if !move_time {
            self.session.set_time(pl.origin);
        } else if let Some(f) = self.playback.audio_frame
            && let Some(c) = self.session.active_comp()
        {
            let t = c.frame_rate.tick_of(f);
            self.session.set_time(t);
        }
    }

    pub fn toggle_play(&mut self, now: f64) {
        let sc = self.session.prefs.preview.current;
        self.toggle_play_with(now, sc);
    }

    /// A preview shortcut pressed: start its preview, or stop the running one. While Cache
    /// Before Playback is still rendering, "If caching, play cached frames" plays what is
    /// cached so far instead of stopping.
    pub fn toggle_play_with(&mut self, now: f64, sc: effectcraft_engine::preview::PreviewShortcut) {
        if !self.playback.playing {
            self.play_with(now, sc);
            return;
        }
        if self.playback.caching && self.session.prefs.preview.get(self.playback.shortcut).play_cached_frames && self.play_cached(now) {
            return;
        }
        self.stop();
    }

    /// Cut the caching preview down to the frames cached from its first frame on and play them.
    fn play_cached(&mut self, now: f64) -> bool {
        let (Some(mut pl), Some(cid)) = (self.playback.plan, self.session.active_comp_id()) else { return false };
        let scale = self.viewer_shown.as_ref().map(|(_, k)| k.scale as f64 / 1000.0).unwrap_or(1.0);
        let series = self.frame_series(cid, scale);
        let mut last = None;
        let mut f = pl.first;
        while f <= pl.end && self.frames.is_cached(&FrameKey { frame: f, ..series }) {
            last = Some(f);
            f += pl.step;
        }
        let Some(last) = last else { return false };
        pl.end = last;
        pl.start = pl.first;
        self.playback.plan = Some(pl);
        self.playback.caching = false;
        self.playback.start_wall = now;
        self.playback.start_frame = pl.first;
        let t = self.session.active_comp().map(|c| c.frame_rate.tick_of(pl.first)).unwrap_or_default();
        self.start_audio(t);
        true
    }

    /// A preview plays with Video off in the Preview panel (audio only): the current time stays
    /// put, so the viewer and Timeline say so instead of looking stuck (#1118). The frame under
    /// the audible sample, when one is known.
    pub fn video_preview_off(&self) -> Option<Option<i64>> {
        let p = self.playback.plan.filter(|_| self.playback.playing)?;
        (!p.video).then_some(self.playback.audio_frame)
    }

    /// The viewer shows grids, guides and safe margins (hidden while a preview without
    /// Include Overlays plays).
    pub fn overlays_visible(&self) -> bool {
        !self.playback.playing || self.playback.overlays
    }

    /// The viewer shows layer handles and paths (View ▸ Show Layer Controls, and Include Layer
    /// Controls while a preview plays).
    pub fn layer_controls_visible(&self) -> bool {
        self.ui.viewer.show_layer_controls && (!self.playback.playing || self.playback.layer_controls)
    }

    /// RAM preview: advance at the preview's frame rate when frames are cached; otherwise hold
    /// and keep rendering ahead (the green cache bar fills, then playback runs in real time).
    fn advance_playback(&mut self, ctx: &egui::Context, scale: f64, now: f64, input_active: bool, cid: ItemId) {
        let Some(c) = self.session.project.comp_arc(cid) else { return };
        let fr = c.frame_rate;
        let plan = self.playback.plan.filter(|_| self.playback.playing);
        let (wa, wb) = match plan {
            Some(p) => (p.start, p.end),
            None => (fr.frame_at(c.work_area.0), fr.frame_at(c.work_area.1 - c.frame_duration())),
        };
        let looping = plan.is_none_or(|p| p.looping);
        let video = plan.is_none_or(|p| p.video);
        // Prefetch ahead.
        let cur = fr.frame_at(self.session.time());
        let ahead = self.prefetch_ahead(ctx, now);
        let queued = self.frames.inflight();
        let series = self.frame_series(cid, scale);
        // The preview's position orders prefetch and eviction (#595).
        let playhead = plan.filter(|p| p.video).map(|p| frames::Playhead { series, cur, start: p.start, end: p.end, step: p.step, looping: p.looping });
        self.frames.set_playhead(playhead);
        let pass = PlaybackPass {
            ctx,
            comp: &c,
            cid,
            scale,
            now,
            frame_rate: fr,
            series,
            plan,
            window: FrameWindow { first: wa, last: wb },
            cur,
            video,
            looping,
            ahead,
            queued,
        };
        if self.cache_preview(&pass) {
            return;
        }
        let step = plan.map_or(1, |p| p.step);
        let fit_ahead = self.frames_that_fit(&c, scale) - 1;
        self.prefetch_playback(&pass, fit_ahead);
        self.pump_meters(ctx, now);
        if !self.playback.playing {
            self.cache_when_idle(ctx, &c, &series, now, input_active, (wa, wb));
            return;
        }
        let fit = self.frames_that_fit(&c, scale);
        self.start_held_audio(&pass, fit);
        if self.advance_audio_clock(&pass, step) {
            return;
        }
        self.advance_frame_clock(&pass, step);
    }

    /// While paused (scrubbing, editing) the viewer's frame comes first: prefetch only a few
    /// frames, only once it is done, and not while edits keep coming (a layer dragged in the
    /// viewer): those frames would be stale at the next step and would hold the CPU and GPU
    /// the next viewer frame needs.
    fn prefetch_ahead(&mut self, ctx: &egui::Context, now: f64) -> i64 {
        if self.playback.playing {
            (self.frames_parallelism() * 2).max(4) as i64
        } else if self.frames.urgent_pending() {
            0
        } else if let Some(wait) = self.editing(now) {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
            0
        } else {
            // Keep workers available for a newly edited viewer frame. Running obsolete
            // background renders cannot be preempted by the priority queue.
            self.frames_parallelism().saturating_sub(1).min(2) as i64
        }
    }

    /// Cache Before Playback: the range in play order, then start the clock. A range larger than
    /// the RAM preview budget is cut to the frames that fit (caching more would evict the first
    /// ones and never finish). Returns true when this pass handled the preview.
    fn cache_preview(&mut self, pass: &PlaybackPass<'_>) -> bool {
        let PlaybackPass { ctx, comp, cid, scale, now, frame_rate: fr, series, plan, ahead, mut queued, .. } = *pass;
        if !self.playback.caching {
            return false;
        }
        let Some(p) = plan else { return false };
        let mut all = true;
        let n = (p.end - p.start) / p.step + 1;
        let fit = self.frames_that_fit(comp, scale);
        if n > fit {
            let mut cut = p;
            (cut.start, cut.end) = (p.first, (p.first + (fit - 1) * p.step).min(p.end));
            self.playback.plan = Some(cut);
            self.toast = Some((format!("Preview cut to {fit} of {n} frames: no more fit in the RAM preview (Settings ▸ Memory & CPU)"), now));
            ctx.request_repaint();
            return true;
        }
        let order: Vec<i64> = (0..n).filter_map(|k| p.frame_after(p.first, k)).collect();
        for f in order {
            if self.frames.is_cached(&FrameKey { frame: f, ..series }) {
                continue;
            }
            all = false;
            if queued < ahead as usize {
                self.request_frame(cid, f, scale);
                queued += 1;
            }
        }
        self.playback.waiting = !all;
        if all {
            self.playback.caching = false;
            self.playback.start_wall = now;
            self.playback.start_frame = p.first;
            self.session.set_time(fr.tick_of(p.first));
            self.start_audio(fr.tick_of(p.first));
        }
        ctx.request_repaint();
        true
    }

    /// Never further ahead than the RAM preview holds: those frames would evict the ones the
    /// preview shows first, or be evicted before it gets there (#595). The window starts at the
    /// first uncached frame ahead: after a loop or a restart the frames just ahead are cached,
    /// and a window from the playhead left the workers idle until it came near the end of the
    /// cached run. It stays ahead × 3 frames wide: workers spread over the whole preview render
    /// frames with different layers at once (on the GPU, an atlas each: #1812).
    fn prefetch_playback(&self, pass: &PlaybackPass<'_>, fit_ahead: i64) {
        let PlaybackPass { cid, scale, plan, cur, ahead, series, mut queued, video, .. } = *pass;
        let FrameWindow { first: wa, last: wb } = pass.window;
        let frame = |k: i64| match plan {
            Some(p) => p.frame_after(cur, k),
            None => {
                let f = cur + k;
                Some(if f > wb { wa + (f - wb - 1).rem_euclid((wb - wa + 1).max(1)) } else { f })
            }
        };
        let cached = self.frames.cached_frames(&series);
        let uncached = |f: i64| cached.binary_search(&f).is_err();
        let first = match plan.filter(|_| self.playback.playing) {
            Some(p) => (0..fit_ahead.min((p.end - p.start) / p.step.max(1) + 1))
                .map_while(|k| frame(k).map(|f| (k, f)))
                .find(|&(_, f)| uncached(f))
                .map_or(0, |(k, _)| k),
            None => 0,
        };
        for k in first..(first + ahead * 3).min(fit_ahead) {
            if queued >= ahead as usize || !video {
                break;
            }
            let Some(f) = frame(k) else { break };
            if uncached(f) {
                self.request_frame(cid, f, scale);
                queued += 1;
            }
        }
    }

    fn pump_meters(&mut self, ctx: &egui::Context, now: f64) {
        if let Some(a) = &mut self.audio {
            a.pump();
            self.meter.update(a.feed.take_peaks(), now);
        } else if let Some(s) = &mut self.scrub {
            // Scrubbing: the meters follow the snippets; an idle second closes the output.
            s.pump();
            self.meter.update(s.feed.take_peaks(), now);
            if now - s.last_used > 1.0 {
                self.scrub = None;
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        } else if self.meter.active() {
            self.meter.update([0.0; 2], now);
            ctx.request_repaint();
        }
    }

    /// Held sound starts once the frames ahead are cached (see `Playback::audio_held`): the
    /// prefetch window when the silent preview keeps real time, else the rest of the preview.
    fn start_held_audio(&mut self, pass: &PlaybackPass<'_>, fit: i64) {
        let PlaybackPass { now, plan, series, cur, ahead, frame_rate: fr, .. } = *pass;
        if !self.playback.audio_held {
            return;
        }
        let Some(p) = plan else { return };
        if (self.playback.keeps_up(now) && self.cached_ahead(&p, &series, cur, ahead.min(fit))) || self.cached_ahead(&p, &series, cur, fit) {
            self.playback.audio_held = false;
            self.playback.start_wall = now;
            self.playback.start_frame = cur;
            self.start_audio(fr.tick_of(cur));
        }
    }

    /// Audio clock drives playback: show the frame under the audible sample. Returns true when
    /// the audio path handled this pass (including stopping at the end).
    fn advance_audio_clock(&mut self, pass: &PlaybackPass<'_>, step: i64) -> bool {
        let PlaybackPass { ctx, now, series, video, frame_rate: fr, cur, .. } = *pass;
        let FrameWindow { first: wa, last: wb } = pass.window;
        let Some(a) = &self.audio else { return false };
        if a.finished() {
            if video {
                self.session.set_time(fr.tick_of(wb));
            } else {
                self.playback.audio_frame = Some(wb);
            }
            self.stop();
            return true;
        }
        let snap = |f: i64| wa + (f - wa).div_euclid(step) * step;
        let target = snap(audio::frame_of_sample(a.clock(), a.rate, fr).clamp(wa, wb));
        if !video {
            self.playback.audio_frame = Some(target);
        } else if self.frames.is_cached(&FrameKey { frame: target, ..series }) {
            self.playback.waiting = false;
            if target != cur {
                self.session.set_time(fr.tick_of(target));
                self.playback.frame_shown(now);
            }
        } else {
            // Rendering can't keep up: rather than skip frames, the sound stops and every
            // frame shows as it renders until the frames ahead are cached again.
            self.audio = None;
            self.playback.audio_held = true;
            self.playback.waiting = true;
            // The rate measured before the stall no longer says it keeps up.
            self.playback.shown_at.clear();
            self.playback.start_wall = now;
            self.playback.start_frame = cur;
        }
        ctx.request_repaint();
        true
    }

    /// The wall clock advances the preview when no audio device drives it, and shows the target
    /// frame (or holds when it is not cached yet).
    fn advance_frame_clock(&mut self, pass: &PlaybackPass<'_>, step: i64) {
        let PlaybackPass { ctx, now, plan, video, looping, frame_rate: fr, cur, series, .. } = *pass;
        let FrameWindow { first: wa, last: wb } = pass.window;
        let elapsed = now - self.playback.start_wall;
        // The plan carries the rate the preview was started with (and `play_with` copies it to
        // `Playback::fps`). Deriving the frame from the plan keeps a logic-only tick correct even
        // when a host/test supplies only the plan; a stale duplicate cannot hold playback open.
        let fps = plan.map_or(self.playback.fps, |p| p.fps);
        let n = ((elapsed * fps) / step as f64).floor() as i64;
        let target = match plan {
            Some(p) => p.frame_after(self.playback.start_frame, n),
            None => {
                let t = self.playback.start_frame + n;
                if t <= wb {
                    Some(t)
                } else if looping {
                    Some(wa + (t - wa).rem_euclid((wb - wa + 1).max(1)))
                } else {
                    None
                }
            }
        };
        let Some(target) = target else {
            if video {
                self.session.set_time(fr.tick_of(wb));
            } else {
                self.playback.audio_frame = Some(wb);
            }
            self.stop();
            return;
        };
        if !video {
            // Audio-only preview without an audio device: the clock runs, the time stays.
            self.playback.audio_frame = Some(target);
            ctx.request_repaint();
            return;
        }
        if self.frames.is_cached(&FrameKey { frame: target, ..series }) {
            self.playback.waiting = false;
            if target != cur {
                self.session.set_time(fr.tick_of(target));
                self.playback.frame_shown(now);
            }
        } else {
            // Not cached yet: hold the clock at the current frame (cache first, then play).
            self.playback.waiting = true;
            let next = plan.and_then(|p| p.frame_after(cur, 1)).unwrap_or(if cur + 1 > wb { wa } else { cur + 1 });
            if self.frames.is_cached(&FrameKey { frame: next, ..series }) {
                self.session.set_time(fr.tick_of(next));
            }
            self.playback.start_wall = now;
            self.playback.start_frame = fr.frame_at(self.session.time());
        }
        ctx.request_repaint();
    }

    /// While the project keeps changing (an edit within [`EDIT_QUIET`] seconds), how long until
    /// it counts as settled.
    fn editing(&mut self, now: f64) -> Option<f64> {
        if self.last_edit.1 != self.session.revision {
            self.last_edit = (now, self.session.revision);
        }
        let left = EDIT_QUIET - (now - self.last_edit.0);
        (left > 0.0).then_some(left)
    }

    /// Composition ▸ Preview ▸ Cache Frames When Idle: after a second without input or edits,
    /// render the work area's frames (from the current time on, then from its start) in the
    /// background, a few at a time, until it is cached or the RAM preview budget is full.
    fn cache_when_idle(
        &mut self,
        ctx: &egui::Context,
        comp: &effectcraft_engine::project::Comp,
        series: &FrameKey,
        now: f64,
        input_active: bool,
        (wa, wb): (i64, i64),
    ) {
        let active = input_active;
        if active || self.last_activity.1 != self.session.revision {
            self.last_activity = (now, self.session.revision);
        }
        if !self.ui.cache_when_idle || self.frames.urgent_pending() {
            return;
        }
        let quiet = now - self.last_activity.0;
        if quiet < 1.0 {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(1.0 - quiet));
            return;
        }
        // An empty work area has nothing to cache.
        let n = wb - wa + 1;
        if n < 1 {
            return;
        }
        let cur = comp.frame_rate.frame_at(self.session.time()).clamp(wa, wb);
        let fit = self.frames_that_fit(comp, series.scale as f64 / 1000.0).min(n);
        let slots = (self.frames_parallelism() / 2).max(1);
        let mut queued = self.frames.inflight();
        let scale = series.scale as f64 / 1000.0;
        let mut pending = false;
        for k in 0..fit {
            let f = wa + (cur - wa + k).rem_euclid(n);
            if self.frames.is_cached(&FrameKey { frame: f, ..*series }) {
                continue;
            }
            pending = true;
            if queued >= slots {
                break;
            }
            self.request_frame(ItemId(series.comp), f, scale);
            queued += 1;
        }
        if pending {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// How many viewer frames of `comp` at `scale` the RAM preview budget holds (90 % of it,
    /// at least one).
    /// The frames of `plan` from `cur` to its end, or as many as fit in the RAM preview, are
    /// cached: sound can play from `cur` in real time.
    fn cached_ahead(&self, plan: &effectcraft_engine::preview::PreviewPlan, series: &FrameKey, cur: i64, fit: i64) -> bool {
        let cached = self.frames.cached_frames(series);
        (0..fit).map_while(|k| plan.frame_after(cur, k)).take_while(|f| *f >= cur).all(|f| cached.binary_search(&f).is_ok())
    }

    fn frames_that_fit(&self, comp: &effectcraft_engine::project::Comp, scale: f64) -> i64 {
        let side = |n: u32| (n as f64 * scale).ceil().max(1.0);
        let frame = side(comp.width) * side(comp.height) * 4.0;
        ((self.frames.budget() as f64 * 0.9 / frame) as i64).max(1)
    }

    fn frames_parallelism(&self) -> usize {
        if cfg!(target_arch = "wasm32") {
            // frame workers render one frame each; without them frames render one at a time
            // on the UI thread (`Frames::pump`)
            return self.frames.remote_slots().max(1);
        }
        self.frames.parallelism()
    }

    // ---------------------------------------------------------------- control channel

    pub(crate) fn raise_for_control(&mut self, ctx: &egui::Context) {
        ctx.request_repaint();
    }

    fn drain_control(&mut self, ctx: &egui::Context, now: f64) {
        let Some(rx) = self.control_rx.take() else { return };
        let mut reqs: Vec<(ControlRequest, f64)> = std::mem::take(&mut self.deferred);
        while let Ok(req) = rx.try_recv() {
            reqs.push((req, now + 3.0));
        }
        for (req, deadline) in reqs {
            let reply = req.reply.clone();
            match control::handle(self, ctx, &req, now) {
                control::Outcome::Done(v) => {
                    let _ = reply.send(v);
                }
                control::Outcome::Retry(msg) => {
                    if now < deadline {
                        self.deferred.push((req, deadline));
                        ctx.request_repaint();
                    } else {
                        let _ = reply.send(json!({"ok": false, "error": msg}));
                    }
                }
                control::Outcome::AfterInput => {
                    self.input_waiters.push(reply);
                    // The reply waits for a real UI pass: a hidden window must run one.
                    self.want_paint = true;
                }
                control::Outcome::AwaitJobs(w) => {
                    self.job_waiters.push((w, reply));
                    ctx.request_repaint();
                }
                control::Outcome::Screenshot { path, target } => {
                    // A covered or minimized window presents no frames (macOS skips its redraws), so
                    // a screenshot would never arrive and nothing would tick its timeout. The
                    // hidden-window capture path (a pending screenshot forces a pass in 0.37) needs
                    // a native platform run before this gate is removed; the web build never paints
                    // a hidden tab.
                    if ctx.input(|i| i.viewport().occluded == Some(true) || i.viewport().minimized == Some(true)) {
                        let _ = reply.send(json!({"ok": false, "error": "window is covered or minimized: call ui.focus first (render.frame renders comp pixels without the window)"}));
                        continue;
                    }
                    self.admit_screenshot(ctx, path, target, reply);
                }
            }
        }
        self.control_rx = Some(rx);
        self.finish_job_waiters(ctx);
    }

    /// Reply to the control requests whose offloaded jobs ended.
    fn finish_job_waiters(&mut self, ctx: &egui::Context) {
        if self.job_waiters.is_empty() {
            return;
        }
        self.session.poll_render();
        self.session.poll_offload();
        let waiters = std::mem::take(&mut self.job_waiters);
        for (w, reply) in waiters {
            if w.pending(self) {
                self.job_waiters.push((w, reply));
            } else {
                let v = w.finish(self);
                let _ = reply.send(v);
            }
        }
        if !self.job_waiters.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    // ---------------------------------------------------------------- screenshots

    /// The root surface geometry a screenshot is admitted against.
    ///
    /// The last completed pass's geometry is authoritative. Before any pass, only the viewport's
    /// `inner_rect` (the client surface a capture returns) is accepted: `outer_rect` also covers
    /// the title bar and window frame, so a capture issued against it would be cropped and
    /// validated against a different rectangle than the callback delivers. A request that arrives
    /// before the geometry is known is refused with an actionable error, and the pass that would
    /// establish it is requested.
    fn current_geometry(&self, ctx: &egui::Context) -> Option<DrawGeometry> {
        if let Some(g) = self.draw_geom {
            return Some(g);
        }
        let rect = ctx.input(|i| i.viewport().inner_rect)?;
        (rect.width() > 0.0 && rect.height() > 0.0).then_some(DrawGeometry { rect, ppp: ctx.pixels_per_point(), epoch: self.geometry_epoch })
    }

    /// The point rectangle a request captures: its crop, clamped to the window, or all of it.
    fn shot_target_rect(geom: &DrawGeometry, crop: Option<[f32; 4]>) -> Option<egui::Rect> {
        let r = match crop {
            Some([x, y, w, h]) if [x, y, w, h].iter().all(|v| v.is_finite()) => {
                egui::Rect::from_min_size(geom.rect.min + egui::vec2(x, y), egui::vec2(w.max(0.0), h.max(0.0))).intersect(geom.rect)
            }
            Some(_) => return None,
            None => geom.rect,
        };
        (r.width() > 0.0 && r.height() > 0.0).then_some(r)
    }

    /// The screen-point crop `target` names right now (a panel's or element's current rectangle),
    /// or an actionable error when it is not on screen. Resolved at issuance, never stored from
    /// admission: the UI may move while it settles.
    fn shot_crop(&self, target: &ShotTarget, geom: &DrawGeometry) -> Result<Option<egui::Rect>, String> {
        let crop = match target {
            ShotTarget::Window => None,
            ShotTarget::Rect(r) => Some(*r),
            ShotTarget::Panel(p) => {
                let id = format!("panel.{}", p.id());
                let found = self.auto.find(&id).ok_or_else(|| format!("the {} panel is not on screen", p.title()))?;
                Some(found.rect)
            }
            ShotTarget::Id(id) => Some(self.auto.find(id).ok_or_else(|| format!("no element `{id}` is on screen"))?.rect),
        };
        Self::shot_target_rect(geom, crop).map(Some).ok_or_else(|| "the screenshot area is empty or not a finite rectangle".to_string())
    }

    /// `target` as a crop relative to `geom`'s origin: what the image's own pixels are cut with.
    fn shot_relative_crop(&self, target: &ShotTarget, geom: &DrawGeometry) -> Result<Option<[f32; 4]>, String> {
        let Some(r) = self.shot_crop(target, geom)? else { return Ok(None) };
        Ok(Some([r.min.x - geom.rect.min.x, r.min.y - geom.rect.min.y, r.width(), r.height()]))
    }

    /// Pixel bytes `rect` takes at `ppp`, checked (`width * height * 4`).
    fn shot_pixel_bytes(rect: egui::Rect, ppp: f32) -> Option<usize> {
        if !ppp.is_finite() || ppp <= 0.0 {
            return None;
        }
        let w = (rect.width() as f64 * ppp as f64).ceil().max(1.0) as usize;
        let h = (rect.height() as f64 * ppp as f64).ceil().max(1.0) as usize;
        w.checked_mul(h)?.checked_mul(4)
    }

    /// Reserve and queue one `ui.screenshot`, or reply with an actionable error.
    fn admit_screenshot(&mut self, ctx: &egui::Context, path: Option<String>, target: ShotTarget, reply: Sender<serde_json::Value>) {
        let Some(geom) = self.current_geometry(ctx) else {
            let _ = reply.send(
                json!({"ok": false, "error": "the window geometry is not known yet (it is recorded by a completed UI pass; retry after the next frame)"}),
            );
            // Ask for the pass that records it.
            self.want_paint = true;
            ctx.request_repaint();
            return;
        };
        if self.pending_screenshots.len() >= MAX_PENDING_SHOTS {
            let _ = reply.send(json!({"ok": false, "error": format!("too many screenshots are pending (at most {MAX_PENDING_SHOTS})")}));
            return;
        }
        if let Err(e) = self.shot_crop(&target, &geom) {
            let _ = reply.send(json!({"ok": false, "error": e}));
            return;
        }
        // The capture is always the whole surface (the crop is applied when the PNG is written),
        // so the reservation and the per-image bound follow the surface's actual dimensions.
        let Some(bytes) = Self::shot_pixel_bytes(geom.rect, geom.ppp) else {
            let _ = reply.send(json!({"ok": false, "error": "the screenshot area is too large to reserve"}));
            return;
        };
        if bytes > MAX_SHOT_BYTES {
            let _ = reply
                .send(json!({"ok": false, "error": format!("the screenshot would need about {bytes} bytes, over the {MAX_SHOT_BYTES}-byte per-image limit")}));
            return;
        }
        if self.shot_bytes.checked_add(bytes).is_none_or(|n| n > MAX_SHOT_TOTAL_BYTES) {
            let _ = reply.send(json!({"ok": false, "error": format!("the screenshot would exceed the {MAX_SHOT_TOTAL_BYTES}-byte pending-image budget")}));
            return;
        }
        self.shot_bytes += bytes;
        self.pending_screenshots.push(PendingShot {
            path,
            target,
            crop: None,
            reply,
            deadline: self.now + SCREENSHOT_TIMEOUT_S,
            settle_at: self.now + SCREENSHOT_SETTLE_S,
            admitted: geom,
            admitted_pass: self.ui_pass_seq,
            issued: None,
            slot: None,
            reserved_bytes: bytes,
            attempts: 0,
        });
        // A queued capture needs a settled UI pass before it can be issued.
        self.want_paint = true;
    }

    /// Re-reserve request `i` against `geom` after a surface change; false when it no longer fits.
    fn refresh_shot_reservation(&mut self, i: usize, geom: &DrawGeometry) -> bool {
        let Some(bytes) = Self::shot_pixel_bytes(geom.rect, geom.ppp) else { return false };
        if bytes > MAX_SHOT_BYTES {
            return false;
        }
        let released = self.shot_bytes.saturating_sub(self.pending_screenshots[i].reserved_bytes);
        if released.checked_add(bytes).is_none_or(|n| n > MAX_SHOT_TOTAL_BYTES) {
            return false;
        }
        self.shot_bytes = released + bytes;
        self.pending_screenshots[i].reserved_bytes = bytes;
        true
    }

    /// Reply with an error to request `i` and release its reservation.
    fn fail_shot(&mut self, i: usize, message: impl Into<String>) {
        let shot = self.pending_screenshots.remove(i);
        self.shot_bytes = self.shot_bytes.saturating_sub(shot.reserved_bytes);
        let _ = shot.reply.send(json!({"ok": false, "error": message.into()}));
    }

    /// Collect completed captures, enforce timeouts and issue every settled request.
    ///
    /// Runs from [`Self::tick`] only: the callback stores pixels in its slot and wakes the
    /// viewport, and everything that can write a file or reply happens here, on the app thread.
    fn tick_screenshots(&mut self, ctx: &egui::Context) {
        let Some(geom) = self.current_geometry(ctx) else { return };
        self.drain_completed_screenshots(geom);
        self.timeout_screenshots();
        self.issue_screenshots(ctx, geom);
        // 4. Wake up for the earliest deadline even when no further pass is otherwise needed: an
        //    issued capture whose callback never arrives (hidden window, display asleep, renderer
        //    failure) would otherwise never reach its timeout. This is a timer, not a paint
        //    request: the issued screenshot command already forces the frame it needs.
        if let Some(next) = self.pending_screenshots.iter().map(|s| s.deadline).reduce(f64::min) {
            let wait = (next - self.now).max(0.001);
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
        }
    }

    /// 1. Drain completed slots. A completion whose surface epoch changed since issuance is
    ///    discarded even if the geometry changed back: it was cropped with another frame's
    ///    pixels-per-point.
    fn drain_completed_screenshots(&mut self, geom: DrawGeometry) {
        let mut i = 0;
        while i < self.pending_screenshots.len() {
            let taken = self.pending_screenshots.get(i).and_then(|s| s.slot.as_ref()).and_then(craft_ui::capture::CaptureSlot::take);
            let Some(result) = taken else {
                i += 1;
                continue;
            };
            let issued = self.pending_screenshots[i].issued;
            match result {
                Ok(image) => {
                    if issued.is_none_or(|g| g.epoch != geom.epoch) {
                        self.pending_screenshots[i].slot = None;
                        self.pending_screenshots[i].issued = None;
                        if self.pending_screenshots[i].attempts >= SCREENSHOT_MAX_RETRIES {
                            self.fail_shot(i, "the window resized while the screenshot was being taken");
                        } else if !self.refresh_shot_reservation(i, &geom) {
                            self.fail_shot(i, "the resized window is too large to capture within the screenshot budget");
                        } else {
                            let settle_at = self.now + SCREENSHOT_SETTLE_S;
                            let passes = self.ui_pass_seq;
                            let s = &mut self.pending_screenshots[i];
                            s.attempts += 1;
                            s.admitted = geom;
                            s.admitted_pass = passes;
                            s.settle_at = settle_at;
                            i += 1;
                        }
                        continue;
                    }
                    let Some(issued) = issued else {
                        i += 1;
                        continue;
                    };
                    if !shot_image_matches(&image, issued.rect, issued.ppp) {
                        self.fail_shot(i, "the screenshot's pixels do not match the window it was taken from");
                        continue;
                    }
                    // The returned image can be smaller than requested (the E2 adapter-limit fit),
                    // so the crop uses the image's own scale, never the issued ppp.
                    let [iw, ih] = image.size;
                    let (sx, sy) = (iw as f64 / issued.rect.width() as f64, ih as f64 / issued.rect.height() as f64);
                    if !(sx.is_finite() && sx > 0.0 && sy.is_finite() && sy > 0.0) {
                        self.fail_shot(i, "the screenshot's size does not describe the window it was taken from");
                        continue;
                    }
                    let shot = self.pending_screenshots.remove(i);
                    self.shot_bytes = self.shot_bytes.saturating_sub(shot.reserved_bytes);
                    let r = control::save_screenshot(&image, shot.path.as_deref(), shot.crop, [sx, sy]);
                    let _ = shot.reply.send(r);
                }
                Err(e) => {
                    self.fail_shot(i, e.to_string());
                }
            }
        }
    }

    /// 2. Timeouts: a late callback finds its slot cancelled and writes nothing.
    fn timeout_screenshots(&mut self) {
        let mut i = 0;
        while i < self.pending_screenshots.len() {
            if self.now > self.pending_screenshots[i].deadline {
                self.fail_shot(i, "no frame was presented (window hidden or display asleep)");
            } else {
                i += 1;
            }
        }
    }

    /// 3. Issue settled requests. The command itself requests the paint; a request still
    ///    waiting for its passes needs `want_paint` so a hidden window keeps painting.
    fn issue_screenshots(&mut self, ctx: &egui::Context, geom: DrawGeometry) {
        let mut i = 0;
        while i < self.pending_screenshots.len() {
            if self.pending_screenshots[i].slot.is_some() {
                i += 1;
                continue;
            }
            if geom.epoch != self.pending_screenshots[i].admitted.epoch {
                if self.pending_screenshots[i].attempts >= SCREENSHOT_MAX_RETRIES {
                    self.fail_shot(i, "the window kept resizing before the screenshot could be taken");
                    continue;
                }
                if !self.refresh_shot_reservation(i, &geom) {
                    self.fail_shot(i, "the resized window is too large to capture within the screenshot budget");
                    continue;
                }
                let (now, passes) = (self.now, self.ui_pass_seq);
                let s = &mut self.pending_screenshots[i];
                s.attempts += 1;
                s.admitted = geom;
                s.admitted_pass = passes;
                s.settle_at = now + SCREENSHOT_SETTLE_S;
                self.want_paint = true;
                i += 1;
                continue;
            }
            let settled = self.ui_pass_seq >= self.pending_screenshots[i].admitted_pass.saturating_add(SCREENSHOT_SETTLE_PASSES)
                && self.now >= self.pending_screenshots[i].settle_at;
            if !settled {
                self.want_paint = true;
                i += 1;
                continue;
            }
            // Resolve the crop now, against the settled UI: an element that moved since admission
            // is captured where it is, and its rectangle is relative to this issued surface.
            let target = self.pending_screenshots[i].target.clone();
            let crop = match self.shot_relative_crop(&target, &geom) {
                Ok(c) => c,
                Err(e) => {
                    self.fail_shot(i, e);
                    continue;
                }
            };
            let (slot, callback) = craft_ui::capture::CaptureSlot::new(ctx, egui::ViewportId::ROOT, MAX_SHOT_BYTES);
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(callback));
            let s = &mut self.pending_screenshots[i];
            s.slot = Some(slot);
            s.issued = Some(geom);
            s.crop = crop;
            i += 1;
        }
    }

    // ---------------------------------------------------------------- frame

    fn handle_events(&mut self, ctx: &egui::Context) {
        for ev in self.session.drain_events() {
            match ev {
                effectcraft_engine::Event::OpenComp(c) => {
                    panels::viewers::on_open_comp(self, c);
                    self.ui.timeline.pps = None;
                }
                effectcraft_engine::Event::Toast { message, .. } => self.toast = Some((message, self.now)),
                effectcraft_engine::Event::OpenUrl(url) => ctx.open_url(egui::OpenUrl::new_tab(url)),
                // Frames are keyed by content: an edit keeps those of comps it doesn't touch.
                effectcraft_engine::Event::ProjectChanged { .. } => {}
                effectcraft_engine::Event::Frontend { command, params } => {
                    if let Err(e) = crate::menus::frontend(self, ctx, &command, params) {
                        self.ui.status = e;
                    }
                }
                effectcraft_engine::Event::PurgeCaches => self.frames.clear(),
                effectcraft_engine::Event::CopyImage { width, height, rgba } => {
                    if let Err(e) = copy_image(ctx, width, height, &rgba) {
                        self.ui.status = e;
                    }
                }
            }
        }
    }

    /// The app's only non-paint entry: one tick of background work.
    ///
    /// This runs for every eframe logic callback, including the logic-only ticks of a hidden
    /// window (which never reach [`Self::draw`]). It must therefore do no input handling, no
    /// layout and no presentation: engine/job queues, autosave, playback, audio, the control
    /// channel, screenshots and the close guard. Everything here is idempotent for a pass that
    /// `Context::request_discard` replays.
    pub fn tick(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        self.now = self.clock.now();
        // The compositor exists before the first paint and while hidden (device-loss handling
        // must run there too).
        self.init_gpu(frame);
        self.apply_gpu_failure(ctx);
        if self.gpu_failure_applied == Some(true) {
            self.close_after_device_loss(ctx);
            return;
        }
        // Focus comes from the fresh viewport copy; a full pass's `InputState::focused` is only a
        // fallback (a logic-only tick leaves it at the previous pass's value).
        let focused = ctx.input(|i| i.viewport().focused.or(Some(i.focused)));
        self.focused = focused.unwrap_or(self.focused);
        // The interface language's fallback fonts are installed in `draw` (fonts need a pass), but
        // commands handled here use the right catalog already.
        let language = crate::i18n::language(self);
        crate::i18n::set_current(language);
        if self.styled_language != language {
            self.want_paint = true;
        }
        // Frames and worker completions wake the app even while it is hidden.
        self.frames.set_context(ctx);
        updates_ui::tick(self, ctx);
        if self.session.render_job.is_some() {
            self.session.poll_render();
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if self.session.track_job.is_some() {
            self.session.poll_track();
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        if self.session.mask_job.is_some() {
            self.session.poll_mask_track();
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        // Background tasks (Content-Aware Fill, Scene Edit Detection, imports).
        if !self.session.tasks.is_empty() {
            self.session.poll_jobs();
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        // Analyses running in a worker (the web app).
        if !self.session.offloaded.is_empty() {
            self.session.poll_offload();
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        // Warp Stabilizer: finish analyses and start queued (re-)analyses in the background.
        if self.session.warp_job.is_some() || !self.session.warp_pending.is_empty() {
            self.session.poll_warp(true);
            self.session.poll_roto(true);
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        // Roto Brush propagation and Object Matte tracking / Freeze: finish jobs (a Freeze writes
        // its mattes) and start queued ones in the background.
        if self.session.roto_job.is_some() || !self.session.roto_pending.is_empty() {
            self.session.poll_roto(true);
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        // 3D Camera Tracker: likewise (tracking, then solving).
        if self.session.camera_job.is_some() || !self.session.camera_pending.is_empty() {
            self.session.poll_camera(true);
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.run_script_tasks(ctx);
        self.run_dialog_answers();
        self.handle_events(ctx);
        if let Some(rx) = self.command_inbox.take() {
            while let Ok(id) = rx.try_recv() {
                if let Err(e) = menus::invoke(self, ctx, &id, json!({})) {
                    self.ui.status = e;
                }
            }
            self.command_inbox = Some(rx);
        }
        // A close request (the window's button, Quit, `app.quit`) must be answered while hidden.
        panels::unsaved::on_close_requested(self, ctx);
        let title = panels::unsaved::window_title(self);
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
        self.tick_autosave(ctx);
        self.tick_workspace_autosave(ctx);
        let now = self.now;
        prefs_live::tick(self, ctx, now);
        // Playback advances without a paint (its frames and the audio clock are background work).
        // Idle prefetch keeps its old condition: it only runs once a viewer has drawn.
        if (self.playback.playing || self.last_viewer_scale.is_some())
            && let Some(cid) = self.session.active_comp_id()
        {
            let (scale, active) = (self.last_viewer_scale.unwrap_or(1.0), self.input_active);
            self.advance_playback(ctx, scale, now, active, cid);
        }
        self.drain_control(ctx, now);
        // Screenshot completion, deadlines and issuance are app-side work: the callback only
        // stores pixels and wakes this viewport.
        self.tick_screenshots(ctx);
        // Time-based FPS readout over UI passes (a hidden logic-only tick is not a frame).
        let dt = (now - self.last_time) as f32;
        if dt > 0.0 && dt < 2.0 {
            self.fps = self.fps * 0.9 + (1.0 / dt).min(480.0) * 0.1;
        }
        self.last_time = now;
        // A hidden window paints when work needs a UI pass: queued input and waits, a settled
        // screenshot, or playback into a child viewport.
        let focused = self.focused;
        let child_viewports = !self.ui.floating.is_empty() || prefs_live::video_preview_active(self, focused);
        let paint = self.want_paint
            || !self.synthetic.is_empty()
            || !self.input_waiters.is_empty()
            || self.pending_screenshots.iter().any(|s| s.slot.is_none())
            || (self.playback.playing && child_viewports);
        if paint {
            self.request_hidden_paint(ctx);
        }
    }

    /// Ask the host for a UI pass even when the window is minimized or occluded.
    ///
    /// Native eframe honours `RequestPaintWhileHidden`; the shared web backend does not carry it
    /// yet (a browser does not paint a hidden tab), so the web build only requests a normal
    /// repaint and hidden web screenshots/input remain unqualified.
    fn request_hidden_paint(&mut self, ctx: &egui::Context) {
        self.want_paint = false;
        #[cfg(not(target_arch = "wasm32"))]
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
        ctx.request_repaint();
    }

    /// Install the theme and fonts for the current language. Fonts need a real pass: the pass
    /// that installs them draws nothing, and the next pass draws with them (the two-pass warm-up
    /// the app has always had). Never call this from a logic-only tick.
    fn ui_setup(&mut self, ctx: &egui::Context) {
        panels::viewer_text::install_hooks(ctx);
        let language = crate::i18n::language(self);
        crate::i18n::set_current(language);
        if (!self.styled || self.styled_language != language) && self.synthetic_in_flight {
            // The install needs a pass that draws nothing, but this pass has already applied a
            // synthetic batch: defer the theme/font install by one pass instead of dropping the
            // input (feeds are only put into a pass that can draw).
            ctx.request_repaint();
            return;
        }
        if !self.styled || self.styled_language != language {
            theme::install(ctx, &self.tokens, language);
            self.styled_language = language;
            if !self.styled {
                fit_window(ctx);
                self.styled = true;
            }
            self.fonts_ready = false;
            ctx.request_repaint();
        } else if !self.fonts_ready {
            // The pass after the install draws: egui has the new font definitions by now.
            self.fonts_ready = true;
            ctx.request_repaint();
        }
    }

    /// One UI pass: setup, input-driven shortcuts, layout and painting. Called from
    /// `eframe::App::ui` and by tests with the root `Ui`.
    pub fn draw(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        panels::viewer_text::begin_ui(ui.ctx());
        self.draw_inner(ui, frame);
        // All exits, including fonts/device loss, observe this pass's real custom availability.
        panels::viewer_text::finish_ime(self, ui);
    }

    /// Context creators install these hooks before the first full pass.
    pub fn install_ime_reset_hooks(ctx: &egui::Context) {
        panels::viewer_text::install_hooks(ctx);
    }

    /// Host wrappers call this after their last logic mutation, without replaying UI events.
    pub fn finish_ime_logic(&self, ctx: &egui::Context) {
        panels::viewer_text::finish_logic(self, ctx);
    }

    fn draw_inner(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.gpu_failure_applied == Some(true) {
            self.close_after_device_loss(&ctx);
            return;
        }
        self.ui_setup(&ctx);
        if !self.fonts_ready {
            // A pass that installs fonts draws nothing. No synthetic batch is in flight here (the
            // hook feeds input only to a ready app), and events egui already applied are never
            // fed again: that would double the pointer/key state of one click.
            ctx.request_repaint();
            return;
        }
        self.frame(ui);
        #[cfg(not(target_arch = "wasm32"))]
        pointer_wrap::finish_native(&ctx, frame);
        // wasm32: no frame threads; render queued frames now, between UI frames.
        if cfg!(target_arch = "wasm32") && self.frames.pump(std::time::Duration::from_millis(if self.playback.playing { 24 } else { 40 })) {
            ctx.request_repaint();
        }
        // Input belongs to this known UI phase: record it for the next tick.
        let active = ctx.input(|i| !i.events.is_empty() || i.pointer.any_down() || i.pointer.is_moving());
        self.input_active = active;
        self.last_pointer_down = ctx.input(|i| i.pointer.any_down());
        // Geometry of a completed pass (a discarded pass is laid out again; only the final one
        // describes the surface the next capture will see). Window position is irrelevant.
        if !ctx.will_discard() {
            let rect = ctx.content_rect();
            let ppp = ctx.pixels_per_point();
            let changed = self.draw_geom.is_none_or(|g| g.rect.size() != rect.size() || g.ppp != ppp);
            if changed {
                self.geometry_epoch = self.geometry_epoch.wrapping_add(1);
            }
            self.draw_geom = Some(DrawGeometry { rect, ppp, epoch: self.geometry_epoch });
            self.ui_pass_seq = self.ui_pass_seq.saturating_add(1);
        }
        // A batch is acknowledged once a UI pass applied it, even if layout was later discarded.
        self.mark_synthetic_applied();
        // `AfterInput` replies wait for the final (non-discarded) pass that applied the last
        // synthetic event: a hidden request is answered only after its forced pass, and a pass
        // egui throws away cannot produce an early success reply.
        if !ctx.will_discard() && self.synthetic.is_empty() {
            for w in self.input_waiters.drain(..) {
                let _ = w.send(json!({"ok": true, "result": null}));
            }
        } else if !self.input_waiters.is_empty() {
            self.want_paint = true;
            ctx.request_repaint();
        }
        if self.frames.inflight() > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
    }

    /// The panels, menus, dialogs and overlays of one pass.
    fn frame(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // Only a viewer that draws in *this* pass records a scale: a closed viewer, the Home
        // screen or another comp must stop idle prefetch at a stale scale.
        self.last_viewer_scale = None;
        self.apply_prefs(&ctx);
        // Video Preview is a second viewport: only a real pass can update it.
        prefs_live::frame(self, &ctx);
        self.auto.begin_frame();
        let tab_had_widget_focus = ctx.egui_wants_keyboard_input();
        menus::handle_shortcuts(self, &ctx);
        if !ctx.input(|i| i.pointer.primary_down()) {
            self.ui.viewer.property_interacting = false;
        }
        let t = self.tokens;
        let full = ui.max_rect();
        if self.dialog.is_some() {
            ui.disable();
        }
        ui.painter().rect_filled(full, 0.0, t.app_bg);
        let mut top = full.min.y;
        if self.ui.show_menu_bar {
            let mb = egui::Rect::from_min_size(full.min, egui::vec2(full.width(), 24.0));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(mb));
            child.painter().rect_filled(mb, 0.0, t.header_bg);
            menus::menu_bar(self, &mut child);
            top = mb.max.y;
        }
        let header_h = 40.0;
        let header = egui::Rect::from_min_size(egui::pos2(full.min.x, top), egui::vec2(full.width(), header_h));
        header::show(self, ui, header);
        let body = egui::Rect::from_min_max(egui::pos2(full.min.x + 4.0, header.max.y + 2.0), egui::pos2(full.max.x - 4.0, full.max.y - 4.0));
        self.dock_area(ui, body);
        #[cfg(not(target_arch = "wasm32"))]
        prefs_live::dropped_files(self, ui, full);
        if !tab_had_widget_focus {
            menus::handle_tab_shortcut(self, &ctx);
        }
        panels::precomp::mini_flowchart(self, &ctx);
        panels::home::capture_thumbnail(self);
        panels::dialogs::show(self, &ctx);
        updates_ui::show(self, &ctx);
        panels::scriptui_view::sync_panels(self);
        panels::scriptui_view::show_windows(self, &ctx);
        panels::learn::coach(self, &ctx);
        let lift = panels::media_panels::import_card(self, ui, full);
        self.draw_toast(ui, full, lift);
    }

    /// Workspace-layout auto-save: a background write that also runs while hidden, once the
    /// pointer has lifted.
    fn tick_workspace_autosave(&mut self, ctx: &egui::Context) {
        if self.session.config.is_none() || !self.workspace_store.has_changes(&self.ui) || self.last_pointer_down {
            return;
        }
        if self.workspace_store.due(self.now) {
            self.save_workspace_state();
        }
        if self.workspace_store.has_changes(&self.ui) {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }

    fn save_workspace_state(&mut self) {
        if let Err(error) = self.workspace_store.save(self.session.config.as_deref(), &self.ui) {
            if self.workspace_store.error.as_ref() != Some(&error) {
                log::warn!("{error}");
                self.ui.status = error.clone();
            }
            self.workspace_store.error = Some(error);
        }
    }

    /// The status message or the latest toast, bottom left (`lift` points higher while the
    /// Importing card is there).
    fn draw_toast(&mut self, ui: &mut egui::Ui, full: egui::Rect, lift: f32) {
        let now = self.now;
        let msg = if !self.ui.status.is_empty() {
            Some(self.ui.status.clone())
        } else {
            self.toast.as_ref().filter(|(_, at)| now - at < 4.0).map(|(m, _)| m.clone())
        };
        if let Some(m) = msg {
            let t = &self.tokens;
            let galley = ui.painter().layout_no_wrap(m.clone(), Tokens::ui(12.0), t.text);
            let r = egui::Rect::from_min_size(egui::pos2(full.min.x + 16.0, full.max.y - 44.0 - lift), galley.size() + egui::vec2(24.0, 14.0));
            ui.painter().rect_filled(r, 6.0, t.panel_bg);
            ui.painter().rect_stroke(r, 6.0, egui::Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
            ui.painter().galley(r.min + egui::vec2(12.0, 7.0), galley, t.text);
            self.auto.add("toast", r, &m);
            let resp = ui.interact(r, egui::Id::unique("toast"), egui::Sense::click());
            if resp.clicked() {
                self.ui.status.clear();
                self.toast = None;
            }
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
            if !self.ui.status.is_empty() {
                let since: f64 = ui.data(|d| d.get_temp(egui::Id::unique("status-since")).unwrap_or(now));
                ui.data_mut(|d| d.insert_temp(egui::Id::unique("status-since"), since));
                if now - since > 5.0 {
                    self.ui.status.clear();
                    ui.data_mut(|d| d.remove::<f64>(egui::Id::unique("status-since")));
                }
            }
        }
    }
}

impl EffectcraftApp {
    /// Scripts' `app.scheduleTask` tasks that are due (repeating ones keep a ScriptUI panel
    /// fresh): errors as a toast, the last line printed in the status bar, and a repaint when
    /// the next one is due.
    fn run_script_tasks(&mut self, ctx: &egui::Context) {
        if self.session.script_ui.task_hosts.is_empty() {
            return;
        }
        let previous_comp = self.session.active_comp_id();
        let Some(r) = effectcraft_engine::scriptui::ScriptUi::run_tasks(&mut self.session) else { return };
        if self.session.active_comp_id().is_some() && self.session.active_comp_id() != previous_comp {
            self.ui.start_screen = false;
        }
        if let Some(msg) = r["errors"].as_array().and_then(|e| e.first()).and_then(|e| e["message"].as_str()) {
            self.toast = Some((format!("Scheduled script task: {msg}"), self.now));
        } else if let Some(line) = r["output"].as_array().and_then(|o| o.last()).and_then(|l| l.as_str()) {
            self.ui.status = line.to_string();
        }
        if let Some(next) = r["next"].as_u64() {
            ctx.request_repaint_after(std::time::Duration::from_millis(next));
        }
    }

    /// Auto-save a dirty project when the interval has passed (Settings ▸ Project ▸ Auto-Save).
    fn tick_autosave(&mut self, ctx: &egui::Context) {
        // Monotonic app time: a wall-clock jump must not skip or repeat an auto-save.
        let now = self.now;
        if let Some(Err(e)) = self.session.autosave_tick(now) {
            self.ui.status = e.to_string();
        }
        if self.session.prefs.auto_save.enabled {
            ctx.request_repaint_after(std::time::Duration::from_secs(30));
        }
        // File ▸ Watch Folder: look for new projects every 10 seconds.
        if self.session.state.watch_folder.is_some() {
            let key = egui::Id::unique("watchFolder.lastPoll");
            let last: f64 = ctx.data(|d| d.get_temp(key)).unwrap_or(0.0);
            if now - last >= 10.0 {
                ctx.data_mut(|d| d.insert_temp(key, now));
                match self.session.execute("file.watchFolder.poll", serde_json::json!({})) {
                    Ok(r) => {
                        if let Some(n) = r["rendered"].as_array().map(Vec::len).filter(|n| *n > 0) {
                            self.ui.status = format!("Watch Folder: rendered {n} project(s)");
                        }
                    }
                    Err(e) => self.ui.status = e.to_string(),
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(10));
        }
        self.tick_footage_reload(ctx, now);
    }

    /// Settings ▸ Import ▸ Reload Footage Changed on Disk: every two seconds, and as soon as the
    /// window comes back to the front (after saving in another app), stamp the footage files on a
    /// worker thread and reload the ones that changed.
    fn tick_footage_reload(&mut self, ctx: &egui::Context, now: f64) {
        use effectcraft_engine::footage_reload;
        if let Some(Err(e)) = footage_reload::poll_scan(&mut self.session) {
            self.ui.status = e.to_string();
        }
        let (last_key, focus_key) = (egui::Id::unique("footageReload.lastScan"), egui::Id::unique("footageReload.focused"));
        let focused = self.focused;
        let was_focused: bool = ctx.data(|d| d.get_temp(focus_key)).unwrap_or(focused);
        ctx.data_mut(|d| d.insert_temp(focus_key, focused));
        if self.session.prefs.import.auto_reload_footage == "off" || self.session.project.items.is_empty() {
            return;
        }
        let last: f64 = ctx.data(|d| d.get_temp(last_key)).unwrap_or(0.0);
        if now - last >= 2.0 || (focused && !was_focused) {
            ctx.data_mut(|d| d.insert_temp(last_key, now));
            footage_reload::start_scan(&mut self.session);
        }
        if self.session.footage_scan.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs(2));
        }
    }

    /// Take the pending synthetic input (from `ui.click`, `ui.key`, …). Hosts that don't call
    /// [`eframe::App::raw_input_hook`] (the headless test harness) feed these in themselves.
    pub fn take_synthetic_input(&mut self) -> Vec<egui::Event> {
        std::mem::take(&mut self.synthetic)
    }
}

impl eframe::App for EffectcraftApp {
    fn on_exit(&mut self) {
        self.save_workspace_state();
        // Clean exit: no crash recovery next launch (unless the graphics device was lost with
        // unsaved changes, which are in the auto-save it offers).
        if self.device_lost_close != Some(true) {
            self.session.end_recovery();
        }
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let injected = self.synthetic_in_flight || !self.synthetic.is_empty();
        // Only a pass that can draw may receive synthetic input: `Context::run_ui` applies
        // everything the hook feeds to its input state, so a pass that cannot draw (fonts still
        // installing) would consume a batch it could not put back — putting it back would apply
        // the click/key state twice. The queue waits here for the first pass that draws.
        if self.fonts_ready {
            self.feed_synthetic(raw_input);
        }
        if pointer_wrap::prepare_input(ctx, raw_input, injected) {
            self.session.history.merge_key = None;
        }
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Never `ctx.input(|i| i.time)` here: `Context::run_logic` leaves events, pointer state and
        // time at the previous full pass's values (only the viewport information is refreshed).
        self.tick(ctx, frame);
        self.finish_ime_logic(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.draw(ui, frame);
    }
}

/// Record the scale the viewer drew at (called by the viewer panel each pass).
///
/// Playback itself advances in [`EffectcraftApp::tick`], so it keeps running while the window is
/// hidden; using the last drawn scale keeps prefetch keys consistent with the visible viewer.
pub(crate) fn note_viewer_scale(app: &mut EffectcraftApp, scale: f64) {
    app.last_viewer_scale = Some(scale);
}

/// Put `width` × `height` straight-alpha RGBA8 pixels on the system clipboard (Composition ▸ Copy
/// Frame to Clipboard). The pixel count is checked first: egui's image constructor panics on a
/// size that doesn't match its buffer.
fn copy_image(ctx: &egui::Context, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || w.checked_mul(h).and_then(|n| n.checked_mul(4)) != Some(rgba.len()) {
        return Err(format!("cannot copy a malformed {width}×{height} image to the clipboard"));
    }
    ctx.copy_image(egui::ColorImage::from_rgba_unmultiplied([w, h], rgba));
    Ok(())
}

/// Maximize a first window that doesn't fit the screen, which fits it to the space beside the
/// taskbar or dock. The 1680 × 1020 window on a smaller screen is only shrunk to the monitor's
/// size, so with its title bar it ran under the taskbar and cut menus off at the bottom of the
/// screen (#269).
fn fit_window(ctx: &egui::Context) {
    let (monitor, window) = ctx.input(|i| (i.viewport().monitor_size, i.viewport().outer_rect.or(i.viewport().inner_rect)));
    if let (Some(monitor), Some(window)) = (monitor, window)
        && overflows_screen(window.size(), monitor)
    {
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
    }
}

/// Whether a window `size` leaves no room on a `monitor` for a taskbar or dock (points).
fn overflows_screen(size: egui::Vec2, monitor: egui::Vec2) -> bool {
    const TASKBAR: f32 = 64.0;
    size.x > monitor.x || size.y > monitor.y - TASKBAR
}

#[cfg(test)]
mod fit_window_tests {
    use super::overflows_screen;
    use egui::vec2;

    #[test]
    fn a_window_taller_than_the_screen_beside_its_taskbar_overflows() {
        // The default window on a 1366 × 768 laptop (shrunk to the monitor) and on 1080p.
        assert!(overflows_screen(vec2(1366.0, 799.0), vec2(1366.0, 768.0)));
        assert!(overflows_screen(vec2(1680.0, 1051.0), vec2(1920.0, 1080.0)));
        assert!(!overflows_screen(vec2(1680.0, 1051.0), vec2(2560.0, 1440.0)));
    }
}

#[cfg(test)]
mod copy_frame_tests {
    use super::*;

    fn copied(output: &egui::FullOutput) -> Vec<&egui::ColorImage> {
        output
            .platform_output
            .commands
            .iter()
            .filter_map(|c| match c {
                egui::OutputCommand::CopyImage(image) => Some(image),
                _ => None,
            })
            .collect()
    }

    /// Composition ▸ Copy Frame to Clipboard puts the rendered frame on the system clipboard.
    #[test]
    fn copy_frame_puts_the_frame_on_the_clipboard() {
        let mut session = Session::default();
        session.execute("comp.new", json!({"name": "Copy", "width": 8, "height": 6, "duration": 1})).unwrap();
        session.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap();
        let mut app = EffectcraftApp::new(session);
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            crate::menus::invoke(&mut app, ui.ctx(), "comp.copyFrame", json!({})).unwrap();
            app.handle_events(ui.ctx());
        });
        output.textures_delta.clear();
        let images = copied(&output);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].size, [8, 6]);
        assert!(images[0].pixels.iter().all(|p| *p == egui::Color32::from_rgb(255, 0, 0)));
    }

    /// A malformed image never reaches egui's panicking constructor.
    #[test]
    fn a_malformed_image_is_an_error_not_a_crash() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            assert!(copy_image(ui.ctx(), 4, 4, &[0; 10]).is_err());
            assert!(copy_image(ui.ctx(), 0, 4, &[]).is_err());
            assert!(copy_image(ui.ctx(), u32::MAX, u32::MAX, &[0; 4]).is_err());
        });
        output.textures_delta.clear();
        assert!(copied(&output).is_empty());
    }
}

#[cfg(test)]
mod gpu_failure_tests {
    use super::*;

    #[test]
    fn host_failure_preserves_document_undo_and_cpu_pixels_and_detaches_late_readbacks() {
        let mut session = Session::default();
        let comp = ItemId(session.execute("comp.new", json!({"name": "Recovery", "width": 4, "height": 4, "duration": 1})).unwrap()["comp"].as_u64().unwrap());
        session.execute("layer.newSolid", json!({"comp": comp.0, "color": "#ff0000"})).unwrap();
        let before = session.render(comp, Tick::ZERO, RenderOpts::default());
        let project = session.project.clone();
        let revision = session.revision;
        let prefs = serde_json::to_value(&session.prefs).unwrap();
        let mut app = EffectcraftApp::new(session);
        let ctx = egui::Context::default();
        let bridge = gpu_failure::GpuFailureBridge::new(&ctx);
        app.set_gpu_failure_bridge(bridge.clone());
        let late_slot = app.viewer_readback.1.clone();
        let key = FrameKey { revision, content: 1, comp: comp.0, frame: 0, scale: 1000, view: 0, opts: 0 };
        app.viewer_readback.0 = Some(key);
        bridge.report("synthetic host device failure", true);
        app.apply_gpu_failure(&ctx);
        assert!(Arc::ptr_eq(&project, &app.session.project));
        assert_eq!(app.session.revision, revision);
        assert_eq!(serde_json::to_value(&app.session.prefs).unwrap(), prefs);
        assert!(app.gpu_checked, "failed device cannot be initialized again next frame");
        assert!(app.session.accel.is_none());
        assert!(app.ui.status.contains("presentation requires restarting"));
        assert_eq!(app.session.render(comp, Tick::ZERO, RenderOpts::default()).data, before.data);
        *late_slot.lock().unwrap() = Some((key, Arc::new(egui::ColorImage::new([1, 1], vec![egui::Color32::RED]))));
        assert!(app.viewer_readback.0.is_none());
        assert!(app.viewer_readback.1.lock().unwrap().is_none(), "a late completion cannot refill the replacement slot");
        app.session.execute("edit.undo", json!({})).unwrap();
        assert!(app.session.project.comp(comp).unwrap().layers.is_empty());
        app.session.execute("edit.redo", json!({})).unwrap();
        assert_eq!(app.session.render(comp, Tick::ZERO, RenderOpts::default()).data, before.data);
    }
}

#[cfg(test)]
mod appearance_tests {
    use super::*;
    use crate::theme::ThemeKind;

    fn app() -> EffectcraftApp {
        EffectcraftApp::new(effectcraft_engine::Session::default())
    }

    /// The Tools bar's appearance button: Sync with System, Light, Dark, then Sync with System
    /// again, showing the saved theme of each family without losing either choice.
    #[test]
    fn appearance_button_cycles_modes_without_losing_theme_choices() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.hooks.system_theme = Some(Box::new(|_| Some(egui::Theme::Dark)));
        app.set_pref("appearance.darkTheme", json!("darker")).unwrap();
        app.apply_prefs(&ctx);
        assert_eq!(app.ui.theme, ThemeKind::Darker);
        for (mode, shown) in [("auto", ThemeKind::Darker), ("light", ThemeKind::Light), ("dark", ThemeKind::Darker), ("auto", ThemeKind::Darker)] {
            menus::invoke(&mut app, &ctx, "view.theme.toggle", json!({})).unwrap();
            assert_eq!(app.session.prefs.appearance.appearance_mode, mode);
            assert_eq!(app.ui.theme, shown, "{mode}");
            assert_eq!(app.tokens.kind, shown);
            assert_eq!(app.session.prefs.appearance.dark_theme, "darker");
            assert_eq!(app.session.prefs.appearance.light_theme, "light");
        }
    }

    /// Sync with System follows the host's answer (the Linux portal) when it changes, and falls
    /// back to Dark when nothing reports a system appearance.
    #[test]
    fn auto_follows_a_stubbed_system_theme() {
        use std::sync::{Arc, Mutex};
        let ctx = egui::Context::default();
        let mut app = app();
        let system = Arc::new(Mutex::new(Some(egui::Theme::Light)));
        let read = Arc::clone(&system);
        app.hooks.system_theme = Some(Box::new(move |_| *read.lock().unwrap_or_else(std::sync::PoisonError::into_inner)));
        app.set_pref("appearance.appearanceMode", json!("auto")).unwrap();
        app.apply_prefs(&ctx);
        assert_eq!(app.ui.theme, ThemeKind::Light);
        *system.lock().unwrap() = Some(egui::Theme::Dark);
        app.apply_prefs(&ctx);
        assert_eq!(app.ui.theme, ThemeKind::Dark, "a system change applies without a settings change");
        *system.lock().unwrap() = None;
        app.apply_prefs(&ctx);
        assert_eq!(app.ui.theme, ThemeKind::Dark);
        // A fixed mode ignores the system.
        *system.lock().unwrap() = Some(egui::Theme::Light);
        app.set_pref("appearance.appearanceMode", json!("dark")).unwrap();
        app.apply_prefs(&ctx);
        assert_eq!(app.ui.theme, ThemeKind::Dark);
    }

    /// Sync with System leaves egui's theme preference at System, so egui never gives the native
    /// window an appearance of its own (on macOS that stops system appearance changes reaching
    /// the app, #642); egui's light and dark styles both carry the resolved theme. Fixed modes
    /// pin the window to their family.
    #[test]
    fn auto_mode_leaves_the_native_window_following_the_system() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.hooks.system_theme = Some(Box::new(|_| Some(egui::Theme::Light)));
        app.set_pref("appearance.appearanceMode", json!("auto")).unwrap();
        app.apply_prefs(&ctx);
        assert_eq!(app.ui.theme, ThemeKind::Light);
        assert_eq!(ctx.options(|o| o.theme_preference), egui::ThemePreference::System);
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            assert_eq!(ctx.style_of(theme).visuals.panel_fill, app.tokens.panel_bg, "{theme:?}");
        }
        // egui asks the window to follow the system, never to take Light or Dark.
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
        out.textures_delta.clear();
        let cmds = out.viewport_output.values().flat_map(|v| v.commands.iter()).collect::<Vec<_>>();
        assert!(!cmds.iter().any(|c| matches!(c, egui::ViewportCommand::SetTheme(t) if t != &egui::SystemTheme::SystemDefault)), "{cmds:?}");
        for (mode, pref) in [("light", egui::ThemePreference::Light), ("dark", egui::ThemePreference::Dark)] {
            app.set_pref("appearance.appearanceMode", json!(mode)).unwrap();
            app.apply_prefs(&ctx);
            assert_eq!(ctx.options(|o| o.theme_preference), pref, "{mode}");
            assert_eq!(ctx.global_style().visuals.panel_fill, app.tokens.panel_bg, "{mode}");
        }
    }

    /// Picking a theme (`view.theme.*`, `ui.set {theme}`) fixes the mode to its family.
    #[test]
    fn picking_a_theme_fixes_its_mode() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.set_pref("appearance.appearanceMode", json!("auto")).unwrap();
        menus::invoke(&mut app, &ctx, "view.theme.light", json!({})).unwrap();
        assert_eq!(app.session.prefs.appearance.appearance_mode, "light");
        assert_eq!(app.ui.theme, ThemeKind::Light);
        menus::invoke(&mut app, &ctx, "view.theme.darker", json!({})).unwrap();
        assert_eq!(app.session.prefs.appearance.appearance_mode, "dark");
        assert_eq!(app.session.prefs.appearance.dark_theme, "darker");
        assert_eq!(app.ui.theme, ThemeKind::Darker);
    }
}

#[cfg(test)]
mod lifecycle_tests {
    //! The logic-only / UI-pass split: queue work in `tick`, input, layout and setup in `draw`.
    //!
    //! The driver mirrors eframe's order for a hidden window: `raw_input_hook` prepares the raw
    //! input, a logic-only iteration runs `tick`, and a forced pass runs `draw` with the same
    //! prepared input. Nothing may depend on `current_pass_index`, which is zero in both.

    use super::*;
    use effectcraft_engine::config::{ConfigStore, MemoryConfig};

    fn app_with_comp() -> EffectcraftApp {
        let mut session = Session::default();
        session.execute("comp.new", json!({"name": "Lifecycle", "width": 64, "height": 36, "duration": 1})).unwrap();
        session.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap();
        let mut app = EffectcraftApp::new(session);
        app.ui.start_screen = false;
        app.ui.show_menu_bar = false;
        app.use_manual_clock();
        app
    }

    fn raw(size: egui::Vec2) -> egui::RawInput {
        egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)), ..Default::default() }
    }

    fn tick_with(app: &mut EffectcraftApp, ctx: &egui::Context, frame: &eframe::Frame, raw: &egui::RawInput) -> egui::LogicOutput {
        ctx.run_logic(raw, |ctx| app.tick(ctx, frame))
    }

    fn hidden_tick(app: &mut EffectcraftApp, ctx: &egui::Context, frame: &eframe::Frame) {
        let _ = tick_with(app, ctx, frame, &raw(egui::vec2(800.0, 480.0)));
    }

    /// One host iteration that ends in a UI pass (the forced hidden pass, or a visible frame).
    fn pass(app: &mut EffectcraftApp, ctx: &egui::Context, frame: &eframe::Frame, raw: &mut egui::RawInput) -> egui::FullOutput {
        eframe::App::raw_input_hook(app, ctx, raw);
        let mut out = ctx.run_ui(raw.take(), |ui| app.draw(ui, frame));
        out.textures_delta.clear();
        out
    }

    fn ready(app: &mut EffectcraftApp, ctx: &egui::Context, frame: &eframe::Frame) -> DrawGeometry {
        // The first pass installs the fonts, the second draws with them.
        for _ in 0..3 {
            let mut raw = raw(egui::vec2(800.0, 480.0));
            pass(app, ctx, frame, &mut raw);
        }
        assert!(app.is_ready(), "fonts are ready after the warm-up passes");
        app.draw_geometry().expect("a completed pass records its geometry")
    }

    fn send(app: &mut EffectcraftApp, method: &str, params: serde_json::Value) -> std::sync::mpsc::Receiver<serde_json::Value> {
        let (request, reply) = ControlRequest::new(method, params);
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(request).unwrap();
        app.control_rx = Some(rx);
        reply
    }

    #[test]
    fn a_logic_only_tick_installs_no_fonts_and_records_no_geometry() {
        let ctx = egui::Context::default();
        let frame = eframe::Frame::_new_kittest();
        let mut app = app_with_comp();
        hidden_tick(&mut app, &ctx, &frame);
        assert!(!app.is_ready(), "fonts need a real pass, not a logic-only tick");
        assert!(app.draw_geometry().is_none(), "a logic-only tick is not a frame");
        assert_eq!(app.ui_passes(), 0);
    }

    #[test]
    fn a_control_command_that_needs_no_ui_completes_without_a_pass() {
        let ctx = egui::Context::default();
        let frame = eframe::Frame::_new_kittest();
        let mut app = app_with_comp();
        let reply = send(&mut app, "engine.execute", json!({"command": "comp.new", "params": {"name": "Queued", "width": 32, "height": 18, "duration": 1}}));
        hidden_tick(&mut app, &ctx, &frame);
        let value = reply.try_recv().expect("a command that needs no UI is answered by the tick");
        assert_eq!(value["ok"], true, "{value}");
        assert_eq!(app.ui_passes(), 0);
    }

    #[test]
    fn a_pass_that_cannot_draw_receives_no_synthetic_input() {
        let ctx = egui::Context::default();
        let frame = eframe::Frame::_new_kittest();
        let mut app = app_with_comp();
        assert!(!app.is_ready(), "a fresh app installs its fonts in its first pass");
        let reply = send(&mut app, "ui.click", json!({"x": 40, "y": 40}));
        hidden_tick(&mut app, &ctx, &frame);
        assert!(reply.try_recv().is_err(), "a click is not answered before its UI pass");
        // The font-install pass cannot draw. It must not receive the click: `run_ui` applies what
        // the hook feeds, and a batch cannot be put back without applying its pointer state twice.
        let mut setup = raw(egui::vec2(800.0, 480.0));
        eframe::App::raw_input_hook(&mut app, &ctx, &mut setup);
        assert!(setup.events.is_empty(), "a pass that cannot draw gets no synthetic input");
        assert_eq!(app.synthetic.len(), 5, "the whole gesture still waits for a drawing pass");
        assert!(app.pending_synthetic_events().is_empty());
        let mut setup_pass = ctx.run_ui(setup.take(), |ui| app.draw(ui, &frame));
        setup_pass.textures_delta.clear();
        assert!(!app.is_ready(), "the pass that installs the fonts draws nothing");
        // Once fonts are ready the click reaches the UI exactly once, one event per pass.
        let mut passes = 0;
        while reply.try_recv().is_err() {
            passes += 1;
            assert!(passes < 40, "the click must be applied within a few passes");
            let mut raw = raw(egui::vec2(800.0, 480.0));
            pass(&mut app, &ctx, &frame, &mut raw);
        }
        assert!(reply.try_recv().is_err(), "the reply is sent once");
        assert!(app.pending_synthetic_events().is_empty());
        assert!(app.synthetic.is_empty(), "every event was applied once");
    }

    #[test]
    fn one_pending_synthetic_batch_is_never_duplicated_while_hidden() {
        let ctx = egui::Context::default();
        let frame = eframe::Frame::_new_kittest();
        let mut app = app_with_comp();
        ready(&mut app, &ctx, &frame);
        let reply = send(&mut app, "ui.click", json!({"x": 40, "y": 40}));
        hidden_tick(&mut app, &ctx, &frame);
        assert!(reply.try_recv().is_err(), "a click is not answered before its UI pass");
        // Several hidden host iterations: eframe keeps the first prepared input; only its one
        // event may reach the raw input, and a later hook must not add another.
        let mut first = raw(egui::vec2(800.0, 480.0));
        eframe::App::raw_input_hook(&mut app, &ctx, &mut first);
        assert_eq!(first.events.len(), 1, "the first event of the gesture is fed");
        let mut second = raw(egui::vec2(800.0, 480.0));
        eframe::App::raw_input_hook(&mut app, &ctx, &mut second);
        assert!(second.events.is_empty(), "an unapplied batch blocks the next event");
        assert_eq!(app.pending_synthetic_events().len(), 1);
        // The pass that applies the batch acknowledges it once; it is never fed again.
        pass(&mut app, &ctx, &frame, &mut first);
        assert!(app.pending_synthetic_events().is_empty());
        // Drive real passes until the gesture is applied: exactly one reply, no lost events.
        let mut passes = 0;
        while reply.try_recv().is_err() {
            passes += 1;
            assert!(passes < 40, "the click must be applied within a few passes");
            let mut raw = raw(egui::vec2(800.0, 480.0));
            pass(&mut app, &ctx, &frame, &mut raw);
        }
        assert!(app.pending_synthetic_events().is_empty());
        assert!(app.synthetic.is_empty());
        assert!(reply.try_recv().is_err(), "the reply is sent once");
    }

    #[test]
    fn geometry_epoch_tracks_size_and_scale_but_not_window_position() {
        let ctx = egui::Context::default();
        let frame = eframe::Frame::_new_kittest();
        let mut app = app_with_comp();
        let first = ready(&mut app, &ctx, &frame);
        let mut moved = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::pos2(120.0, 80.0), first.rect.size())), ..Default::default() };
        pass(&mut app, &ctx, &frame, &mut moved);
        assert_eq!(app.draw_geometry().unwrap().epoch, first.epoch, "moving the window does not change a capture");
        let mut resized = raw(egui::vec2(1024.0, 640.0));
        pass(&mut app, &ctx, &frame, &mut resized);
        let after_resize = app.draw_geometry().unwrap();
        assert!(after_resize.epoch > first.epoch, "a resize advances the epoch");
        let mut back = raw(egui::vec2(800.0, 480.0));
        pass(&mut app, &ctx, &frame, &mut back);
        assert!(app.draw_geometry().unwrap().epoch > after_resize.epoch, "changing back is another change, not the old epoch");
    }

    #[test]
    fn only_a_viewer_that_draws_records_a_prefetch_scale() {
        let ctx = egui::Context::default();
        let frame = eframe::Frame::_new_kittest();
        let mut app = app_with_comp();
        ready(&mut app, &ctx, &frame);
        assert!(app.last_viewer_scale.is_some(), "the visible viewer recorded the scale it drew at");
        // The Home screen draws no viewer: the old comp's scale must not keep idle prefetch alive.
        app.ui.start_screen = true;
        let mut input = raw(egui::vec2(800.0, 480.0));
        pass(&mut app, &ctx, &frame, &mut input);
        assert!(app.last_viewer_scale.is_none(), "no viewer drew, so there is no scale to prefetch at");
        // Back to the comp: the next pass records it again.
        app.ui.start_screen = false;
        let mut input = raw(egui::vec2(800.0, 480.0));
        pass(&mut app, &ctx, &frame, &mut input);
        assert!(app.last_viewer_scale.is_some());
    }

    #[test]
    fn playback_advances_on_a_logic_only_tick() {
        let ctx = egui::Context::default();
        let frame = eframe::Frame::_new_kittest();
        let mut app = app_with_comp();
        app.playback.playing = true;
        app.playback.plan = Some(effectcraft_engine::preview::PreviewPlan {
            start: 1,
            end: 1,
            first: 1,
            step: 1,
            fps: 24.0,
            looping: false,
            cache_first: false,
            video: false,
            audio: false,
            origin: Tick::ZERO,
        });
        app.playback.start_wall = 0.0;
        app.playback.start_frame = 1;
        app.clock_mut().advance(1.0);
        hidden_tick(&mut app, &ctx, &frame);
        assert!(!app.playback.playing, "a one-frame preview ends without any UI pass");
        assert_eq!(app.playback.audio_frame, Some(1), "the audio-only playhead reached the plan's end");
        assert_eq!(app.ui_passes(), 0);
    }

    #[test]
    fn workspace_layout_autosave_writes_on_a_hidden_tick() {
        let store = Arc::new(MemoryConfig::default());
        let mut app = EffectcraftApp::new(Session { config: Some(store.clone()), ..Default::default() });
        app.ui.start_screen = false;
        app.use_manual_clock();
        app.ui.focused = PanelKind::Timeline;
        app.clock_mut().advance(2.0);
        let ctx = egui::Context::default();
        let frame = eframe::Frame::_new_kittest();
        hidden_tick(&mut app, &ctx, &frame);
        assert!(store.read(WORKSPACES_FILE).is_some(), "the layout is saved while the window is hidden");
        assert_eq!(app.ui_passes(), 0);
    }

    #[test]
    fn the_close_guard_answers_a_close_request_without_a_pass() {
        let ctx = egui::Context::default();
        let frame = eframe::Frame::_new_kittest();
        let mut app = app_with_comp();
        assert!(app.session.is_dirty(), "creating a comp and a layer leaves unsaved changes");
        let mut raw = raw(egui::vec2(800.0, 480.0));
        raw.viewports.insert(egui::ViewportId::ROOT, egui::ViewportInfo { events: vec![egui::ViewportEvent::Close], ..Default::default() });
        let out = tick_with(&mut app, &ctx, &frame, &raw);
        assert!(app.dialog.is_some(), "a hidden close request opens the unsaved-changes prompt");
        let cancel = out.viewport_commands.values().flatten().any(|c| matches!(c, egui::ViewportCommand::CancelClose));
        assert!(cancel, "the window stays open while the prompt is asking");
        assert_eq!(app.ui_passes(), 0);
    }

    // ---------------------------------------------------------------- screenshots

    fn screenshot_request(
        app: &mut EffectcraftApp,
        ctx: &egui::Context,
        path: Option<&str>,
        target: ShotTarget,
    ) -> std::sync::mpsc::Receiver<serde_json::Value> {
        let (tx, rx) = std::sync::mpsc::channel();
        app.admit_screenshot(ctx, path.map(str::to_string), target, tx);
        rx
    }

    fn shot_callbacks(out: &egui::LogicOutput) -> Vec<egui::ScreenshotCallback> {
        out.viewport_commands
            .values()
            .flatten()
            .filter_map(|c| match c {
                egui::ViewportCommand::Screenshot(cb) => Some(cb.clone()),
                _ => None,
            })
            .collect()
    }

    /// Three settled passes and the settle delay, then the tick that issues the capture.
    fn issue_shot(app: &mut EffectcraftApp, ctx: &egui::Context, frame: &eframe::Frame) -> egui::ScreenshotCallback {
        issue_shot_at(app, ctx, frame, egui::vec2(800.0, 480.0))
    }

    fn issue_shot_at(app: &mut EffectcraftApp, ctx: &egui::Context, frame: &eframe::Frame, size: egui::Vec2) -> egui::ScreenshotCallback {
        for _ in 0..SCREENSHOT_SETTLE_PASSES {
            let mut raw = raw(size);
            pass(app, ctx, frame, &mut raw);
        }
        app.clock_mut().advance(SCREENSHOT_SETTLE_S + 0.01);
        let out = tick_with(app, ctx, frame, &raw(size));
        let cbs = shot_callbacks(&out);
        assert_eq!(cbs.len(), 1, "one settled screenshot is issued once");
        cbs.into_iter().next().unwrap()
    }

    fn image(width: usize, height: usize) -> Arc<egui::ColorImage> {
        Arc::new(egui::ColorImage::new([width, height], vec![egui::Color32::from_rgb(10, 20, 30); width.checked_mul(height).unwrap()]))
    }

    #[test]
    fn a_screenshot_is_issued_only_after_settled_passes_and_writes_from_the_tick() {
        let ctx = egui::Context::default();
        let frame = eframe::Frame::_new_kittest();
        let mut app = app_with_comp();
        ready(&mut app, &ctx, &frame);
        let path = std::env::temp_dir().join(format!("effectcraft-shot-{}-ok.png", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let reply = screenshot_request(&mut app, &ctx, path.to_str(), ShotTarget::Window);
        // Not settled: a tick issues nothing and asks for passes instead.
        let out = tick_with(&mut app, &ctx, &frame, &raw(egui::vec2(800.0, 480.0)));
        assert!(shot_callbacks(&out).is_empty(), "no capture before the UI settles");
        assert_eq!(out.viewport_commands.values().flatten().filter(|c| matches!(c, egui::ViewportCommand::RequestPaintWhileHidden)).count(), 1);
        let cb = issue_shot(&mut app, &ctx, &frame);
        assert!(reply.try_recv().is_err(), "the callback must not write or reply itself");
        cb.complete(image(800, 480));
        hidden_tick(&mut app, &ctx, &frame);
        let value = reply.try_recv().expect("the tick drains the completed slot and replies");
        assert_eq!(value["ok"], true, "{value}");
        assert!(path.exists(), "{value}");
        assert_eq!(value["result"]["width"], 800);
        assert_eq!(value["result"]["height"], 480);
        assert_eq!(app.shot_bytes, 0, "the reservation is released");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_resize_between_issue_and_completion_rejects_the_pixels_and_retries() {
        let ctx = egui::Context::default();
        let frame = eframe::Frame::_new_kittest();
        let mut app = app_with_comp();
        ready(&mut app, &ctx, &frame);
        let path = std::env::temp_dir().join(format!("effectcraft-shot-{}-retry.png", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let reply = screenshot_request(&mut app, &ctx, path.to_str(), ShotTarget::Window);
        let first = issue_shot(&mut app, &ctx, &frame);
        let size = egui::vec2(1024.0, 640.0);
        let mut resized = raw(size);
        pass(&mut app, &ctx, &frame, &mut resized);
        first.complete(image(800, 480));
        let _ = tick_with(&mut app, &ctx, &frame, &raw(size));
        assert!(reply.try_recv().is_err(), "pixels from the old surface epoch are discarded");
        assert!(!path.exists(), "a stale image is never written");
        // The retry is issued against the new geometry and the new image is accepted.
        let second = issue_shot_at(&mut app, &ctx, &frame, size);
        second.complete(image(1024, 640));
        let _ = tick_with(&mut app, &ctx, &frame, &raw(size));
        let value = reply.try_recv().expect("the retried capture replies");
        assert_eq!(value["ok"], true, "{value}");
        assert_eq!(value["result"]["width"], 1024);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn screenshot_admission_bounds_requests_and_a_timeout_releases_its_reservation() {
        let ctx = egui::Context::default();
        let frame = eframe::Frame::_new_kittest();
        let mut app = app_with_comp();
        ready(&mut app, &ctx, &frame);
        let mut replies = Vec::new();
        for _ in 0..MAX_PENDING_SHOTS {
            replies.push(screenshot_request(&mut app, &ctx, None, ShotTarget::Rect([0.0, 0.0, 4.0, 4.0])));
        }
        let refused = screenshot_request(&mut app, &ctx, None, ShotTarget::Rect([0.0, 0.0, 4.0, 4.0]));
        let value = refused.try_recv().expect("the fifth request is refused immediately");
        assert_eq!(value["ok"], false, "{value}");
        // Every queued request times out on a hidden window that presents no frame; each one
        // releases its reservation exactly once.
        app.clock_mut().advance(SCREENSHOT_TIMEOUT_S + 1.0);
        hidden_tick(&mut app, &ctx, &frame);
        for reply in replies {
            let value = reply.try_recv().expect("a timed-out request gets an actionable reply");
            assert_eq!(value["ok"], false, "{value}");
            assert!(value["error"].as_str().is_some_and(|e| e.contains("no frame")), "{value}");
        }
        assert_eq!(app.shot_bytes, 0);
        assert!(app.pending_screenshots.is_empty());
    }

    #[test]
    fn a_late_callback_after_cancellation_writes_nothing_and_replies_once() {
        let ctx = egui::Context::default();
        let frame = eframe::Frame::_new_kittest();
        let mut app = app_with_comp();
        ready(&mut app, &ctx, &frame);
        let path = std::env::temp_dir().join(format!("effectcraft-shot-{}-late.png", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let reply = screenshot_request(&mut app, &ctx, path.to_str(), ShotTarget::Window);
        let late = issue_shot(&mut app, &ctx, &frame);
        app.clock_mut().advance(SCREENSHOT_TIMEOUT_S + 1.0);
        hidden_tick(&mut app, &ctx, &frame);
        let timeout = reply.try_recv().expect("the request times out");
        assert_eq!(timeout["ok"], false, "{timeout}");
        late.complete(image(800, 480));
        hidden_tick(&mut app, &ctx, &frame);
        assert!(reply.try_recv().is_err(), "a late completion sends no second reply");
        assert!(!path.exists(), "a late completion writes no file");
        assert_eq!(app.shot_bytes, 0);
    }

    /// Every repaint delay requested on `ctx` (what a host uses to wake the app).
    fn wake_requests(ctx: &egui::Context) -> Arc<std::sync::Mutex<Vec<std::time::Duration>>> {
        let wakes: Arc<std::sync::Mutex<Vec<std::time::Duration>>> = Arc::default();
        let record = Arc::clone(&wakes);
        ctx.set_repaint_observer(move |i| record.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(i.delay));
        wakes
    }

    #[test]
    fn an_issued_capture_schedules_a_wake_up_for_its_deadline() {
        let ctx = egui::Context::default();
        let frame = eframe::Frame::_new_kittest();
        let mut app = app_with_comp();
        ready(&mut app, &ctx, &frame);
        let reply = screenshot_request(&mut app, &ctx, None, ShotTarget::Window);
        for _ in 0..SCREENSHOT_SETTLE_PASSES {
            let mut raw = raw(egui::vec2(800.0, 480.0));
            pass(&mut app, &ctx, &frame, &mut raw);
        }
        app.clock_mut().advance(SCREENSHOT_SETTLE_S + 0.01);
        // Record the repaint requests of the tick that issues the capture only.
        let wakes = wake_requests(&ctx);
        let out = tick_with(&mut app, &ctx, &frame, &raw(egui::vec2(800.0, 480.0)));
        assert_eq!(shot_callbacks(&out).len(), 1, "the settled capture is issued once");
        let delays = wakes.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        // The deadline is 8 s after admission; the capture was issued ~0.31 s in, so the app must
        // ask for a wake-up close to that timeout, not only for the next paint.
        let deadline_wake = delays.iter().any(|d| d.as_secs_f64() > 7.0 && d.as_secs_f64() <= SCREENSHOT_TIMEOUT_S);
        assert!(deadline_wake, "an issued capture asks the host to wake for its timeout: {delays:?}");
        // The callback never arrives (a hidden window or a display asleep). The host wakes at the
        // requested delay; without this wake-up the timeout below could never be evaluated.
        app.clock_mut().advance(SCREENSHOT_TIMEOUT_S + 1.0);
        hidden_tick(&mut app, &ctx, &frame);
        let value = reply.try_recv().expect("the deadline wake-up produces the timeout reply");
        assert_eq!(value["ok"], false, "{value}");
        assert!(value["error"].as_str().is_some_and(|e| e.contains("no frame")), "{value}");
        assert_eq!(app.shot_bytes, 0);
        assert!(app.pending_screenshots.is_empty());
    }

    #[test]
    fn a_downscaled_capture_is_cropped_with_the_images_own_scale() {
        let ctx = egui::Context::default();
        let frame = eframe::Frame::_new_kittest();
        let mut app = app_with_comp();
        let geom = ready(&mut app, &ctx, &frame);
        assert_eq!((geom.rect.width(), geom.rect.height(), geom.ppp), (800.0, 480.0, 1.0));
        // The adapter-limit fit (E2) returned half the requested pixels. Left red, right blue.
        let fitted =
            Arc::new(egui::ColorImage::new([400, 240], (0..400 * 240).map(|i| if i % 400 < 200 { egui::Color32::RED } else { egui::Color32::BLUE }).collect()));
        let dir = std::env::temp_dir();
        for (name, crop, want) in [("left", [0.0, 0.0, 400.0, 240.0], egui::Color32::RED), ("right", [400.0, 0.0, 400.0, 240.0], egui::Color32::BLUE)] {
            let path = dir.join(format!("effectcraft-shot-{}-scale-{name}.png", std::process::id()));
            let _ = std::fs::remove_file(&path);
            let reply = screenshot_request(&mut app, &ctx, path.to_str(), ShotTarget::Rect(crop));
            let cb = issue_shot(&mut app, &ctx, &frame);
            cb.complete(Arc::clone(&fitted));
            hidden_tick(&mut app, &ctx, &frame);
            let value = reply.try_recv().expect("the downscaled capture replies");
            assert_eq!(value["ok"], true, "{value}");
            assert_eq!(value["result"]["width"], 200, "{value}");
            assert_eq!(value["result"]["height"], 120, "{value}");
            let png = std::fs::read(&path).expect("the PNG is written");
            let decoded = image::load_from_memory(&png).expect("our PNG decodes").to_rgba8();
            assert_eq!((decoded.width(), decoded.height()), (200, 120), "{value}");
            let px = decoded.get_pixel(100, 60).0;
            assert!(px[0] == want.r() && px[1] == want.g() && px[2] == want.b(), "{name}: {px:?} is not from its half of the fitted image");
            let _ = std::fs::remove_file(&path);
        }
    }

    #[test]
    fn a_capture_before_any_pass_never_uses_the_window_frame_rect() {
        let ctx = egui::Context::default();
        let frame = eframe::Frame::_new_kittest();
        let mut app = app_with_comp();
        // Only the outer (title bar included) rectangle is known: it is not the captured surface.
        let outer = egui::Rect::from_min_size(egui::pos2(0.0, -30.0), egui::vec2(800.0, 510.0));
        let mut raw = egui::RawInput::default();
        raw.viewports.insert(egui::ViewportId::ROOT, egui::ViewportInfo { outer_rect: Some(outer), ..Default::default() });
        let _ = tick_with(&mut app, &ctx, &frame, &raw);
        let (tx, reply) = std::sync::mpsc::channel();
        app.admit_screenshot(&ctx, None, ShotTarget::Window, tx);
        let value = reply.try_recv().expect("the request is answered now");
        assert_eq!(value["ok"], false, "{value}");
        assert!(value["error"].as_str().is_some_and(|e| e.contains("geometry is not known")), "{value}");
        assert!(app.pending_screenshots.is_empty(), "nothing was reserved against the wrong rectangle");
        assert_eq!(app.shot_bytes, 0);
        assert!(app.want_paint, "the pass that records the geometry is requested");
    }

    #[test]
    fn an_element_that_moves_while_the_ui_settles_is_captured_where_it_is() {
        let ctx = egui::Context::default();
        let frame = eframe::Frame::_new_kittest();
        let mut app = app_with_comp();
        let geom = ready(&mut app, &ctx, &frame);
        // Any registered panel: the crop follows this element's rectangle.
        let panel = app.auto.previous.iter().chain(app.auto.elements.iter()).find(|e| e.id.starts_with("panel.")).map(|e| e.id.clone());
        let panel = panel.expect("a dock panel is on screen after a completed pass");
        let reply = screenshot_request(&mut app, &ctx, None, ShotTarget::Id(panel.clone()));
        for _ in 0..SCREENSHOT_SETTLE_PASSES {
            let mut raw = raw(egui::vec2(800.0, 480.0));
            pass(&mut app, &ctx, &frame, &mut raw);
        }
        // The panel moves after its last draw: the crop must follow it, not the admission rect.
        let moved = [40.0, 30.0, 300.0, 200.0];
        for list in [&mut app.auto.previous, &mut app.auto.elements] {
            if let Some(e) = list.iter_mut().find(|e| e.id == panel) {
                e.rect = moved;
            }
        }
        app.clock_mut().advance(SCREENSHOT_SETTLE_S + 0.01);
        let out = tick_with(&mut app, &ctx, &frame, &raw(egui::vec2(800.0, 480.0)));
        assert_eq!(shot_callbacks(&out).len(), 1, "the settled capture is issued once");
        let crop = app.pending_screenshots.first().and_then(|s| s.crop).expect("the element crop is resolved at issuance");
        assert!((crop[0] - (moved[0] - geom.rect.min.x)).abs() < 0.01, "{crop:?}");
        assert!((crop[1] - (moved[1] - geom.rect.min.y)).abs() < 0.01, "{crop:?}");
        assert!((crop[2] - moved[2]).abs() < 0.01 && (crop[3] - moved[3]).abs() < 0.01, "{crop:?}");
        // Let it complete and reply, so the request does not outlive the test.
        if let Some(cb) = shot_callbacks(&out).into_iter().next() {
            cb.complete(image(800, 480));
        }
        hidden_tick(&mut app, &ctx, &frame);
        let value = reply.try_recv().expect("the capture replies");
        assert_eq!(value["ok"], true, "{value}");
        assert_eq!((value["result"]["width"].as_u64(), value["result"]["height"].as_u64()), (Some(300), Some(200)), "{value}");
    }
}
