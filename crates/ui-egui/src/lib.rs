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
pub mod control;
pub mod credits;
pub mod dock;
pub mod dock_ui;
pub mod frames;
pub mod gpu_failure;
pub mod header;
pub mod i18n;
pub mod icons;
pub mod menus;
pub mod native_menu;
pub mod panels;
pub mod prefs_live;
pub mod state;
pub mod theme;
pub mod widgets;

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

const SCREENSHOT_TIMEOUT_S: f64 = 4.0;
/// Seconds without a project change before paused prefetch resumes (see
/// [`EffectcraftApp::editing`]).
const EDIT_QUIET: f64 = 0.3;

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
}

/// Host hooks provided by the native app (file pickers etc.).
#[derive(Default)]
pub struct Hooks {
    pub pick_files: Option<Box<dyn Fn(&[&str]) -> Vec<String>>>,
    pub pick_save: Option<Box<dyn Fn(&str) -> Option<String>>>,
    pub pick_open_project: Option<Box<dyn Fn() -> Option<String>>>,
    /// Opens the audio output for preview playback (the desktop app uses cpal).
    pub audio_device: Option<audio::AudioDeviceFactory>,
    /// Lists audio output devices (Settings ▸ Audio).
    pub audio_devices: Option<Box<dyn Fn() -> Vec<String>>>,
    /// Picks a folder (Settings paths).
    pub pick_folder: Option<Box<dyn Fn() -> Option<String>>>,
    /// Save dialog for other file kinds: (default name or path, extension). A default path opens
    /// the dialog in its folder.
    pub pick_save_file: Option<Box<dyn Fn(&str, &str) -> Option<String>>>,
    /// The system clipboard's text (native menu Edit ▸ Paste into a text field).
    pub clipboard_text: Option<Box<dyn Fn() -> Option<String>>>,
    /// Application actions the OS performs (`app.hide`, `app.hideOthers`, `app.showAll` on
    /// macOS). Returns false when the host doesn't handle the id.
    pub app_action: Option<Box<dyn Fn(&str) -> bool>>,
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
    pub tokens: Tokens,
    pub auto: automation::Registry,
    pub frames: Frames,
    pub playback: Playback,
    pub dialog: Option<Dialog>,
    pub hooks: Hooks,
    pub fps: f32,
    last_time: f64,
    styled: bool,
    fonts_ready: bool,
    pub(crate) control_rx: Option<Receiver<ControlRequest>>,
    pub(crate) deferred: Vec<(ControlRequest, f64)>,
    pub(crate) synthetic: Vec<egui::Event>,
    pub(crate) input_waiters: Vec<Sender<serde_json::Value>>,
    /// Control requests waiting for an offloaded job (`wait: true` in the browser).
    pub(crate) job_waiters: Vec<(control::JobWaiter, Sender<serde_json::Value>)>,
    queued_screenshots: Vec<(u64, f64, u32)>,
    pending_screenshots: Vec<(u64, Option<String>, Option<[f32; 4]>, Sender<serde_json::Value>, f64)>,
    next_token: u64,
    pub(crate) last_ui_time: f64,
    /// When the user last did something (input or an edit): Cache Frames When Idle waits for
    /// a second of quiet. (time, project revision then)
    last_activity: (f64, u64),
    /// When the project last changed, and its revision then (see [`Self::editing`]).
    last_edit: (f64, u64),
    pub(crate) toast: Option<(String, f64)>,
    /// Texture of the last CPU frame shown in the viewer and the key it came from.
    pub(crate) viewer_tex: Option<(egui::TextureHandle, FrameKey)>,
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
    /// Audio panel VU meters.
    pub meter: audio::Meter,
    /// Waveform peak summaries per footage item (see `panels::waveform`).
    pub(crate) waveforms: panels::waveform::Cache,
    /// Last reveal shortcut and when (double-press shortcuts such as LL).
    pub(crate) last_reveal: Option<(String, f64)>,
    /// Settings revision applied to the theme, tooltips and caches.
    applied_prefs: Option<u64>,
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

impl EffectcraftApp {
    pub fn new(session: Session) -> Self {
        EffectcraftApp {
            session,
            ui: UiState::default(),
            tokens: Tokens::for_kind(Default::default()),
            auto: Default::default(),
            frames: Frames::default(),
            playback: Playback::default(),
            dialog: None,
            hooks: Hooks::default(),
            fps: 60.0,
            last_time: 0.0,
            styled: false,
            fonts_ready: false,
            control_rx: None,
            deferred: vec![],
            synthetic: vec![],
            input_waiters: vec![],
            job_waiters: vec![],
            queued_screenshots: vec![],
            pending_screenshots: vec![],
            next_token: 1,
            last_ui_time: 0.0,
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
            command_inbox: None,
            pointer_comp: None,
            dialog_state: Default::default(),
            integrated_titlebar: false,
            audio: None,
            scrub: None,
            meter: Default::default(),
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
        }
        .with_ui_commands()
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
        if self.applied_prefs == Some(self.session.prefs_revision) {
            return;
        }
        self.applied_prefs = Some(self.session.prefs_revision);
        let p = &self.session.prefs;
        self.ui.theme = theme::ThemeKind::from_name(&p.appearance.theme).unwrap_or_default();
        self.tokens = Tokens::from_prefs(p);
        theme::apply_visuals(ctx, &self.tokens);
        let tips = p.general.show_tool_tips;
        ctx.all_styles_mut(|s| s.interaction.tooltip_delay = if tips { 0.5 } else { f32::INFINITY });
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

    pub fn set_theme(&mut self, ctx: &egui::Context, k: theme::ThemeKind) {
        let name = match k {
            theme::ThemeKind::Dark => "dark",
            theme::ThemeKind::Darker => "darker",
            theme::ThemeKind::Light => "light",
        };
        let _ = self.set_pref("appearance.theme", json!(name));
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
        if let Some(f) = self.ui.floating.iter_mut().find(|f| f.panels.contains(&p)) {
            f.active = f.panels.iter().position(|x| *x == p).unwrap_or(0);
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
        self.viewer_shown = self.viewer_tex.as_ref().map(|(texture, key)| (texture.id(), *key));
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
        self.viewer_tex.as_ref().map(|(t, _)| t.size())
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

    /// The render scale used by the viewer right now.
    pub fn viewer_scale(&self, zoom: f32, ppp: f32) -> f64 {
        // The preview's Resolution (when not Auto) while it plays.
        if self.playback.playing
            && let Some(f) = self.playback.res
        {
            let r = match f {
                1 => state::Resolution::Full,
                2 => state::Resolution::Half,
                3 => state::Resolution::Third,
                4 => state::Resolution::Quarter,
                n => state::Resolution::Custom(n.min(40) as u8),
            };
            return r.scale(zoom, ppp);
        }
        // Auto follows the zoom alike while playing and paused, so frames cached while scrubbing
        // play back, and frames cached by a preview stay on screen when it stops.
        let full = self.ui.viewer.res.scale(zoom, ppp);
        // Fast Previews ▸ Adaptive Resolution / Fast Draft: lower resolution while dragging.
        let (_, k) = self.session.state.viewer.fast_previews.render(self.ui.viewer.interacting || self.ui.viewer.property_interacting);
        if k < 1.0 && self.ui.viewer.res == state::Resolution::Auto {
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
    fn advance_playback(&mut self, ctx: &egui::Context, scale: f64) {
        let Some(cid) = self.session.active_comp_id() else { return };
        let Some(c) = self.session.project.comp_arc(cid) else { return };
        let now = ctx.input(|i| i.time);
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
        // While paused (scrubbing, editing) the viewer's frame comes first: prefetch only a few
        // frames, only once it is done, and not while edits keep coming (a layer dragged in the
        // viewer): those frames would be stale at the next step and would hold the CPU and GPU
        // the next viewer frame needs.
        let ahead = if self.playback.playing {
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
        };
        let mut queued = self.frames.inflight();
        let series = self.frame_series(cid, scale);
        if self.playback.caching
            && let Some(p) = plan
        {
            // Cache Before Playback: the range in play order, then start the clock. A range
            // larger than the RAM preview budget is cut to the frames that fit (caching more
            // would evict the first ones and never finish).
            let mut all = true;
            let n = (p.end - p.start) / p.step + 1;
            let fit = self.frames_that_fit(&c, scale);
            if n > fit {
                let mut cut = p;
                (cut.start, cut.end) = (p.first, (p.first + (fit - 1) * p.step).min(p.end));
                self.playback.plan = Some(cut);
                self.toast = Some((format!("Preview cut to {fit} of {n} frames: no more fit in the RAM preview (Settings ▸ Memory & CPU)"), now));
                ctx.request_repaint();
                return;
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
            return;
        }
        let step = plan.map_or(1, |p| p.step);
        for k in 0..ahead * 3 {
            if queued >= ahead as usize || !video {
                break;
            }
            let f = match plan {
                Some(p) => match p.frame_after(cur, k) {
                    Some(f) => f,
                    None => break,
                },
                None => {
                    let mut f = cur + k;
                    if f > wb {
                        f = wa + (f - wb - 1).rem_euclid((wb - wa + 1).max(1));
                    }
                    f
                }
            };
            if !self.frames.is_cached(&FrameKey { frame: f, ..series }) {
                self.request_frame(cid, f, scale);
                queued += 1;
            }
        }
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
        if !self.playback.playing {
            self.cache_when_idle(ctx, &c, &series, now, (wa, wb));
            return;
        }
        let snap = |f: i64| wa + (f - wa).div_euclid(step) * step;
        // Held sound starts once the frames ahead are cached (see `Playback::audio_held`): the
        // prefetch window when the silent preview keeps real time, else the rest of the preview.
        let fit = self.frames_that_fit(&c, scale);
        if self.playback.audio_held
            && let Some(p) = plan
            && ((self.playback.keeps_up(now) && self.cached_ahead(&p, &series, cur, ahead.min(fit))) || self.cached_ahead(&p, &series, cur, fit))
        {
            self.playback.audio_held = false;
            self.playback.start_wall = now;
            self.playback.start_frame = cur;
            self.start_audio(fr.tick_of(cur));
        }
        // Audio clock drives playback: show the frame under the audible sample.
        if let Some(a) = &self.audio {
            if a.finished() {
                if video {
                    self.session.set_time(fr.tick_of(wb));
                } else {
                    self.playback.audio_frame = Some(wb);
                }
                self.stop();
                return;
            }
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
            return;
        }
        let elapsed = now - self.playback.start_wall;
        let n = ((elapsed * self.playback.fps) / step as f64).floor() as i64;
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
    fn cache_when_idle(&mut self, ctx: &egui::Context, comp: &effectcraft_engine::project::Comp, series: &FrameKey, now: f64, (wa, wb): (i64, i64)) {
        let active = ctx.input(|i| !i.events.is_empty() || i.pointer.any_down() || i.pointer.is_moving());
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

    fn drain_control(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.control_rx.take() else { return };
        let now = ctx.input(|i| i.time);
        let mut reqs: Vec<(ControlRequest, f64)> = std::mem::take(&mut self.deferred);
        while let Ok(req) = rx.try_recv() {
            reqs.push((req, now + 3.0));
        }
        for (req, deadline) in reqs {
            let reply = req.reply.clone();
            match control::handle(self, ctx, &req) {
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
                control::Outcome::AfterInput => self.input_waiters.push(reply),
                control::Outcome::AwaitJobs(w) => {
                    self.job_waiters.push((w, reply));
                    ctx.request_repaint();
                }
                control::Outcome::Screenshot { path, crop } => {
                    // A covered or minimized window presents no frames (macOS skips its redraws), so
                    // a screenshot would never arrive and nothing would tick its timeout.
                    if ctx.input(|i| i.viewport().occluded == Some(true) || i.viewport().minimized == Some(true)) {
                        let _ = reply.send(json!({"ok": false, "error": "window is covered or minimized: call ui.focus first (render.frame renders comp pixels without the window)"}));
                        continue;
                    }
                    let token = self.next_token;
                    self.next_token += 1;
                    let settle = now + 0.35;
                    self.queued_screenshots.push((token, settle, 0));
                    self.pending_screenshots.push((token, path, crop, reply, settle + SCREENSHOT_TIMEOUT_S));
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

    fn issue_screenshots(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let mut any = false;
        let busy = self.frames.inflight() > 0;
        self.queued_screenshots.retain_mut(|(token, at, frames)| {
            *frames += 1;
            any = true;
            // Wait for the viewer's frame to finish rendering (bounded by the timeout below).
            if now >= *at && *frames >= 3 && (!busy || now > *at + 2.5) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(*token)));
                false
            } else {
                true
            }
        });
        self.pending_screenshots.retain(|(.., reply, deadline)| {
            if now > *deadline {
                let _ = reply.send(json!({"ok": false, "error": "no frame was presented (window hidden or display asleep)"}));
                false
            } else {
                true
            }
        });
        if any {
            ctx.request_repaint();
        } else if !self.pending_screenshots.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    fn collect_screenshots(&mut self, ctx: &egui::Context) {
        if self.pending_screenshots.is_empty() {
            return;
        }
        let events: Vec<_> = ctx.input(|i| {
            i.raw
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Screenshot { user_data, image, .. } => {
                        let token = user_data.data.as_ref().and_then(|d| d.downcast_ref::<u64>()).copied()?;
                        Some((token, image.clone()))
                    }
                    _ => None,
                })
                .collect()
        });
        for (token, image) in events {
            if let Some(i) = self.pending_screenshots.iter().position(|(t, ..)| *t == token) {
                let (_, path, crop, reply, _) = self.pending_screenshots.remove(i);
                let r = control::save_screenshot(ctx, &image, path.as_deref(), crop);
                let _ = reply.send(r);
            }
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
                effectcraft_engine::Event::Toast { message, .. } => self.toast = Some((message, ctx.input(|i| i.time))),
                effectcraft_engine::Event::OpenUrl(url) => ctx.open_url(egui::OpenUrl::new_tab(url)),
                // Frames are keyed by content: an edit keeps those of comps it doesn't touch.
                effectcraft_engine::Event::ProjectChanged { .. } => {}
                effectcraft_engine::Event::Frontend { command, params } => {
                    if let Err(e) = crate::menus::frontend(self, ctx, &command, params) {
                        self.ui.status = e;
                    }
                }
                effectcraft_engine::Event::PurgeCaches => self.frames.clear(),
            }
        }
    }

    fn frame(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.apply_gpu_failure(&ctx);
        self.auto.begin_frame();
        self.frames.set_context(&ctx);
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
        // 3D Camera Tracker: likewise (tracking, then solving).
        if self.session.camera_job.is_some() || !self.session.camera_pending.is_empty() {
            self.session.poll_camera(true);
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.apply_prefs(&ctx);
        self.handle_events(&ctx);
        panels::unsaved::on_close_requested(self, &ctx);
        let title = panels::unsaved::window_title(self);
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
        self.tick_autosave(&ctx);
        prefs_live::frame(self, &ctx);
        if let Some(rx) = self.command_inbox.take() {
            while let Ok(id) = rx.try_recv() {
                if let Err(e) = menus::invoke(self, &ctx, &id, json!({})) {
                    self.ui.status = e;
                }
            }
            self.command_inbox = Some(rx);
        }
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
        panels::precomp::mini_flowchart(self, &ctx);
        panels::home::capture_thumbnail(self);
        panels::dialogs::show(self, &ctx);
        panels::scriptui_view::sync_panels(self);
        panels::scriptui_view::show_windows(self, &ctx);
        panels::learn::coach(self, &ctx);
        let lift = panels::media_panels::import_card(self, ui, full);
        self.draw_toast(ui, full, lift);
    }

    /// The status message or the latest toast, bottom left (`lift` points higher while the
    /// Importing card is there).
    fn draw_toast(&mut self, ui: &mut egui::Ui, full: egui::Rect, lift: f32) {
        let now = ui.input(|i| i.time);
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
            let resp = ui.interact(r, egui::Id::new("toast"), egui::Sense::click());
            if resp.clicked() {
                self.ui.status.clear();
                self.toast = None;
            }
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
            if !self.ui.status.is_empty() {
                let since: f64 = ui.data(|d| d.get_temp(egui::Id::new("status-since")).unwrap_or(now));
                ui.data_mut(|d| d.insert_temp(egui::Id::new("status-since"), since));
                if now - since > 5.0 {
                    self.ui.status.clear();
                    ui.data_mut(|d| d.remove::<f64>(egui::Id::new("status-since")));
                }
            }
        }
    }
}

impl EffectcraftApp {
    /// Auto-save a dirty project when the interval has passed (Settings ▸ Project ▸ Auto-Save).
    fn tick_autosave(&mut self, ctx: &egui::Context) {
        let now = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        if let Some(Err(e)) = self.session.autosave_tick(now) {
            self.ui.status = e.to_string();
        }
        if self.session.prefs.auto_save.enabled {
            ctx.request_repaint_after(std::time::Duration::from_secs(30));
        }
        // File ▸ Watch Folder: look for new projects every 10 seconds.
        if self.session.state.watch_folder.is_some() {
            let key = egui::Id::new("watchFolder.lastPoll");
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
    }

    /// Take the pending synthetic input (from `ui.click`, `ui.key`, …). Hosts that don't call
    /// [`eframe::App::raw_input_hook`] (the headless test harness) feed these in themselves.
    pub fn take_synthetic_input(&mut self) -> Vec<egui::Event> {
        std::mem::take(&mut self.synthetic)
    }
}

impl eframe::App for EffectcraftApp {
    fn on_exit(&mut self) {
        // Clean exit: no crash recovery next launch.
        self.session.end_recovery();

        #[cfg(target_os = "macos")]
        {
            std::process::exit(0);
        }
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if !self.synthetic.is_empty() {
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
            raw_input.events.extend(self.synthetic.drain(..n));
        }
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_gpu_failure(ctx);
        if !self.styled {
            theme::install(ctx, &self.tokens);
            fit_window(ctx);
            self.styled = true;
            ctx.request_repaint();
        } else {
            self.fonts_ready = true;
        }
        let now = ctx.input(|i| i.time);
        let dt = (now - self.last_time) as f32;
        if dt > 0.0 {
            self.fps = self.fps * 0.9 + (1.0 / dt).min(480.0) * 0.1;
        }
        self.last_time = now;
        self.drain_control(ctx);
        if !self.synthetic.is_empty() {
            ctx.request_repaint();
        } else if !self.input_waiters.is_empty() {
            for w in self.input_waiters.drain(..) {
                let _ = w.send(json!({"ok": true, "result": null}));
            }
        }
        self.issue_screenshots(ctx);
        self.collect_screenshots(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.init_gpu(frame);
        if !self.fonts_ready {
            ui.ctx().request_repaint();
            return;
        }
        self.frame(ui);
        let ctx = ui.ctx().clone();
        // wasm32: no frame threads; render queued frames now, between UI frames.
        if cfg!(target_arch = "wasm32") && self.frames.pump(std::time::Duration::from_millis(if self.playback.playing { 24 } else { 40 })) {
            ctx.request_repaint();
        }
        self.last_ui_time = ctx.input(|i| i.time);
        if !self.synthetic.is_empty() {
            ctx.request_repaint();
        } else if !self.input_waiters.is_empty() {
            ctx.request_repaint();
            for w in self.input_waiters.drain(..) {
                let _ = w.send(json!({"ok": true, "result": null}));
            }
        }
        if self.frames.inflight() > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
    }
}

/// Advance playback with the viewer's scale (called by the viewer panel each frame).
pub(crate) fn tick_playback(app: &mut EffectcraftApp, ctx: &egui::Context, scale: f64) {
    app.advance_playback(ctx, scale);
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
