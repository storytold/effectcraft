use super::clock::AppClock;
use super::hooks::Hooks;
use super::state::{FrameWindow, PlaybackPass, ShotTarget};
use super::*;

mod clipboard;
mod files;
mod frame;
mod gpu;
mod input;
mod playback;
mod prefetch;
mod prefs;
mod screenshot;
mod viewer_frames;
mod window;

pub use gpu::{GPU_PREPARING, GpuBuild};
#[cfg(test)]
pub(crate) use screenshot::{MAX_PENDING_SHOTS, SCREENSHOT_SETTLE_PASSES, SCREENSHOT_SETTLE_S, SCREENSHOT_TIMEOUT_S};

impl EffectcraftApp {
    /// Build the app around `session`.
    /// Loads and applies the workspaces saved in the session's config directory (a failed load is reported in
    /// `ui.status`), initializes the default UI, playback and render state, and registers the frontend's commands as
    /// shortcutable `UiCommand`s on the session.
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
            forwarded: ForwardedInput::default(),
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
            #[cfg(not(target_arch = "wasm32"))]
            gpu_build: None,
            gpu_failures: None,
            gpu_failure_applied: None,
            footage_watcher: None,
            footage_watch_key: None,
            device_lost_close: None,
            dialog_answers: std::sync::mpsc::channel(),
            file_dialogs: FileDialogsUp::default(),
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
            native_viewports: vec![],
            unpublished_viewports: vec![],
            pending_panel_raise: None,
            home_thumbs: Default::default(),
            home_thumb_saved: None,
            window_title: String::new(),
            template_thumbs: Default::default(),
            template_list: None,
            updates_rt: updates_ui::Runtime::default(),
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

    pub fn with_control(mut self, rx: Receiver<ControlRequest>) -> Self {
        self.control_rx = Some(rx);
        self
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

    /// Feed pending synthetic input into `raw_input` only once fonts are ready (so a non-drawing pass never consumes
    /// a batch), then prepare pointer wrapping; ending a numeric scrub clears the history merge key so the next edit
    /// starts a new undo step.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        raw_input.events.append(&mut self.forwarded.events);
        raw_input.dropped_files.append(&mut self.forwarded.files);
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

    /// The logic-only tick: run `tick`, close the IME pass with `finish_ime_logic` and, last, hand a close to the
    /// platform quit (`panels::unsaved::quit_on_close`). It does not read pass-time
    /// input state (events, pointer and time are stale here), only the viewport information eframe refreshed.
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Never `ctx.input(|i| i.time)` here: `Context::run_logic` leaves events, pointer state and
        // time at the previous full pass's values (only the viewport information is refreshed).
        self.tick(ctx, frame);
        self.finish_ime_logic(ctx);
        crate::panels::unsaved::quit_on_close(ctx);
    }

    /// Draw one UI pass through `draw`.
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.draw(ui, frame);
    }
}
