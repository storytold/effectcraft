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
use dock::{PanelKind, WorkspaceExt as _};
use effectcraft_engine::Session;
use effectcraft_engine::project::ItemId;
use effectcraft_engine::render::RenderOpts;
use effectcraft_engine::time::Tick;
use frames::{FrameKey, Frames, RenderSource};
use serde_json::json;
use state::UiState;
use theme::Tokens;

mod app;
mod clock;
mod hooks;

#[cfg(test)]
use app::{MAX_PENDING_SHOTS, SCREENSHOT_SETTLE_PASSES, SCREENSHOT_SETTLE_S, SCREENSHOT_TIMEOUT_S};
pub use clock::AppClock;
pub use hooks::{DialogDone, FileDialog, Hooks, SystemThemeFn};
pub use state::ShotTarget;

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

/// A non-blocking file dialog's answer for the command that asked (`menus::dialog_answered`).
pub(crate) struct DialogAnswer {
    pub command: String,
    pub key: &'static str,
    pub params: serde_json::Value,
    pub paths: Vec<String>,
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
    /// Input from floating panels' own OS windows for the main window's next frame.
    pub(crate) forwarded: ForwardedInput,
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
    /// Those dialogs that are up, for blocking floating panels' OS windows meanwhile.
    pub(crate) file_dialogs: FileDialogsUp,
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
    /// The floating panels' OS windows that drew a panel last frame; one that is gone releases
    /// its IME state.
    pub(crate) native_viewports: Vec<egui::ViewportId>,
    /// The native windows that drew a panel last frame without publishing their IME descriptor.
    pub(crate) unpublished_viewports: Vec<egui::ViewportId>,
    /// A floating panel shown by a command: its window comes to the front next frame.
    pub(crate) pending_panel_raise: Option<PanelKind>,
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

/// Non-blocking file dialogs ([`Hooks::file_dialog_async`]) that are up. While one is, the dock
/// area blocks floating panels' OS windows, as it does for a modal dialog; otherwise craft-ui
/// takes the app for inactive while the dialog has the keyboard, and hides floating palettes.
///
/// A dialog can end without answering: on macOS a sheet whose window goes away ends without its
/// completion handler. A dialog is taken for gone, so floating windows never stay blocked, once
/// one of the app's windows has the keyboard again after none had it, or when one has kept it
/// for [`FileDialogsUp::UNSEEN`] since the dialog was asked for, so the dialog never showed.
#[derive(Default)]
pub(crate) struct FileDialogsUp {
    count: usize,
    /// When the oldest dialog up was first checked ([`FileDialogsUp::up`]).
    since: Option<f64>,
    /// None of the app's windows had the keyboard since then.
    left: bool,
}

impl FileDialogsUp {
    /// Seconds a dialog has to take the keyboard from the app's windows. The desktop portal can
    /// take a moment to show the first dialog on Linux.
    const UNSEEN: f64 = 2.0;

    pub(crate) fn opened(&mut self) {
        if self.count == 0 {
            *self = Self::default();
        }
        self.count = self.count.saturating_add(1);
    }

    pub(crate) fn answered(&mut self) {
        self.count = self.count.saturating_sub(1);
    }

    /// Whether a dialog is up at `now`, `focused`: whether one of the app's windows has the
    /// keyboard.
    pub(crate) fn up(&mut self, focused: bool, now: f64) -> bool {
        if self.count == 0 {
            return false;
        }
        let since = *self.since.get_or_insert(now);
        if !focused {
            self.left = true;
        } else if self.left || now - since >= Self::UNSEEN {
            *self = Self::default();
        }
        self.count > 0
    }
}

/// Keys and dropped files from floating panels' OS windows (craft-ui `DockOutput`).
#[derive(Default)]
pub(crate) struct ForwardedInput {
    pub(crate) events: Vec<egui::Event>,
    pub(crate) files: Vec<egui::DroppedFileHandle>,
}

/// Record the scale the viewer drew at (called by the viewer panel each pass).
///
/// Playback itself advances in [`EffectcraftApp::tick`], so it keeps running while the window is
/// hidden; using the last drawn scale keeps prefetch keys consistent with the visible viewer.
pub(crate) fn note_viewer_scale(app: &mut EffectcraftApp, scale: f64) {
    app.last_viewer_scale = Some(scale);
}

#[cfg(test)]
mod lifecycle_tests;
