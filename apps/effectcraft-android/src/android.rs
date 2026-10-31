//! The Android shell: session, storage and the eframe app, started from [`crate::android_main`].
//!
//! Storage is the app's own internal directory (`<internal data>/EffectCraft`), handed to us by
//! the platform: settings, shortcuts, auto-saves, models, plug-ins and the session log land
//! there, readable on a debug build with `adb shell run-as ai.storyteller.effectcraft`. There is
//! no `HOME` on Android, so [`effectcraft_host::config_dir`] has nothing to say; the shell wires
//! the session's config itself instead.
//!
//! The file-dialog hook uses an in-app browser (Android has no `rfd`); preview audio goes
//! through cpal/AAudio.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use effectcraft_ui_egui::EffectcraftApp;
use serde_json::json;
use winit::platform::android::activity::AndroidApp;

use crate::audio_out;
use crate::file_browser;

/// The app's data folder, created when the session is wired.
fn data_dir(app: &AndroidApp) -> Option<PathBuf> {
    app.internal_data_path().map(|p| p.join("EffectCraft"))
}

/// Start the app: logging, the session log, then the event loop. Runs on the thread
/// `native_app_glue` spawns for `android_main`; returning ends the activity's native side.
pub(crate) fn run(android_app: AndroidApp) {
    // First, so the panic hook and every start-up record reaches the session log.
    effectcraft_engine::logging::install();
    effectcraft_engine::logging::install_panic_hook();
    let data = data_dir(&android_app);
    if let Some(dir) = &data {
        match std::fs::create_dir_all(dir) {
            Ok(()) => match effectcraft_engine::logging::attach_session_log(&effectcraft_engine::logging::log_dir(Some(dir))) {
                Ok(path) => log::info!("effectcraft {}, log file {}", env!("CARGO_PKG_VERSION"), path.display()),
                Err(e) => log::warn!("no log file: {e}"),
            },
            Err(e) => log::warn!("{}: {e}", dir.display()),
        }
    }
    // The font menus list the installed fonts: read their names while the window opens.
    effectcraft_engine::text::fonts::scan_system_in_background();
    // The shell keeps a handle to the activity: its window reports the content rect (what the
    // system bars leave) every frame (`Shell::fit_to_content`).
    let shell_app = android_app.clone();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("EffectCraft"),
        wgpu_options: wgpu_options(gpu_backends(data.as_deref())),
        // winit's event loop needs the app it was started for; without it the loop refuses.
        android_app: Some(android_app),
        ..Default::default()
    };
    log::info!("effectcraft {}: opening the window", env!("CARGO_PKG_VERSION"));
    let result = eframe::run_native(
        "EffectCraft",
        options,
        Box::new(move |cc| {
            match &cc.wgpu_render_state {
                Some(rs) => log::info!("graphics: {:?}", rs.adapter.get_info()),
                None => log::info!("graphics: no wgpu device"),
            }
            // The same GPU failure bridge the desktop app installs: uncaptured errors and a lost
            // device become a report the UI can show instead of a silent crash (#591).
            let gpu_failures = effectcraft_ui_egui::gpu_failure::GpuFailureBridge::new(&cc.egui_ctx);
            if let Some(rs) = &cc.wgpu_render_state {
                let errors = gpu_failures.clone();
                rs.device.on_uncaptured_error(Arc::new(move |error| {
                    let message = format!("uncaptured GPU error: {error}");
                    if errors.report(&message, false) {
                        log::error!("{message}");
                    }
                }));
                let lost = gpu_failures.clone();
                rs.device.set_device_lost_callback(move |reason, message| {
                    lost.report(&format!("GPU device lost ({reason:?}): {message}"), true);
                });
            }
            let mut session = effectcraft_host::session();
            // Lazy open and non-blocking auto-save (M13.14), as on the desktop.
            session.check_footage_on_open = true;
            session.autosave.background = true;
            // The browser starts where the data lives (always readable).
            let browser_start = data.clone();
            if let Some(dir) = data {
                // WebAssembly effect plug-ins in <data>/Plug-ins load before the menus are built.
                let plugins = dir.join("Plug-ins");
                if plugins.is_dir() {
                    let _ = session.execute("effect.plugins.load", json!({"folder": plugins.to_string_lossy()}));
                }
                session.config = Some(Arc::new(effectcraft_engine::config::DirConfig::new(dir)));
            }
            session.load_settings();
            let _ = session.execute("window.restoreScriptPanels", json!({}));
            let mut app = EffectcraftApp::new(session);
            app.set_gpu_failure_bridge(gpu_failures);
            // Preview audio (AAudio via cpal, same design as the desktop app).
            app.hooks.audio_device = Some(Box::new(audio_out::open));
            app.hooks.audio_devices = Some(Box::new(audio_out::devices));
            // In-app file browser (Android has no `rfd`): non-blocking, so the frame keeps
            // pumping while the dialog is open.
            let pending: Arc<Mutex<Option<file_browser::Pending>>> = Arc::new(Mutex::new(None));
            let pending_hook = pending.clone();
            let ctx_hook = cc.egui_ctx.clone();
            let start_hook = browser_start.clone();
            app.hooks.file_dialog_async = Some(Box::new(move |dialog, done| {
                file_browser::request(&pending_hook, &ctx_hook, dialog, done, start_hook.clone());
            }));
            // The Home screen first, like the desktop: there are no files to open on launch.
            app.ui.start_screen = app.session.prefs.startup.show_home_on_launch;
            if app.ui.status.is_empty() {
                app.ui.status = "EffectCraft on Android: use the file browser for Import/Open/Save.".into();
            }
            log::info!("app created; the window shows after its first frame");
            let shell = Shell { app, android_app: Some(shell_app), pending, content: None, drawn: false };
            Ok(Box::new(shell) as Box<dyn eframe::App>)
        }),
    );
    match result {
        Ok(()) => log::info!("the window closed"),
        Err(e) => log::error!("EffectCraft couldn't start its window: {e}"),
    }
}

/// The eframe app: the shared EffectCraft UI, without the desktop shell's menu bar, window
/// tracker and launch guard (no menu bar, no window to move on Android).
struct Shell {
    app: EffectcraftApp,
    /// The activity, whose window reports the area the system bars leave us.
    android_app: Option<AndroidApp>,
    /// The in-app file browser's pending request (the hook stores, `ui` draws).
    pending: Arc<Mutex<Option<file_browser::Pending>>>,
    /// The last content rect, so a change is logged once.
    content: Option<(i32, i32, i32, i32)>,
    /// The first frame was drawn (logged once).
    drawn: bool,
}

impl eframe::App for Shell {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.app.logic(ctx, frame);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.app.ui(ui, frame);
        // The in-app file browser floats above the workspace while a command waits for it.
        file_browser::show(&self.pending, ui.ctx());
        if !self.drawn {
            self.drawn = true;
            log::info!("first frame drawn");
        }
    }

    fn on_exit(&mut self) {
        self.app.on_exit();
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.fit_to_content(ctx, raw_input);
        self.app.raw_input_hook(ctx, raw_input);
    }
}

impl Shell {
    /// A native Android window covers the whole display — system bars included — so a tablet's
    /// taskbar would sit over the bottom of the timeline and the status line. The platform tells
    /// us what is left of the window after the system window insets ([`AndroidApp::content_rect`],
    /// delivered as the activity's content rect): lay the UI out inside it, and eframe's clear
    /// colour fills the strip the bars cover. No JNI: hiding the bars themselves would need a
    /// Java call on the UI thread, which no native activity can make.
    ///
    /// Runs before egui sees the input, so the first frame is already inset; the rect follows the
    /// device (rotation, a taskbar that slides away) because it is read every frame.
    fn fit_to_content(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let Some(app) = self.android_app.as_ref() else { return };
        let r = app.content_rect();
        let now = (r.left, r.top, r.right, r.bottom);
        if self.content != Some(now) {
            self.content = Some(now);
            log::info!("window content rect: left {} top {} right {} bottom {}", r.left, r.top, r.right, r.bottom);
        }
        let ppp = raw_input
            .viewports
            .get(&egui::ViewportId::ROOT)
            .and_then(|v| v.native_pixels_per_point)
            .filter(|p| p.is_finite() && *p > 0.0)
            .unwrap_or_else(|| ctx.pixels_per_point());
        let Some(rect) = raw_input.screen_rect else { return };
        // Physical pixels inside the content rect, in points. Shrinking only: the status bar is
        // hidden by the theme, so the content starts at the window's own origin.
        let w = (r.right - r.left).max(0) as f32 / ppp;
        let h = (r.bottom - r.top).max(0) as f32 / ppp;
        if w <= 0.0 || h <= 0.0 || w >= rect.width() && h >= rect.height() {
            return;
        }
        if r.left != 0 || r.top != 0 {
            log::warn!("content rect starts at {}, {}: the UI keeps the window's origin", r.left, r.top);
        }
        raw_input.screen_rect = Some(egui::Rect::from_min_size(rect.min, egui::vec2(w, h)));
    }
}

/// Which backends the window's device asks for (`None`: wgpu's own order, Vulkan first).
///
/// Vulkan is what a tablet wants, but the emulator's Vulkan is SwiftShader, which renders our
/// pipelines through a software rasteriser and dies on them (the emulator's own process crashes)
/// — there the OpenGL ES path is both reliable and faster. So: `<data>/EffectCraft/gpu-backend`
/// (`gl` or `vulkan`) forces a backend, otherwise a Vulkan adapter that *is* a software
/// rasteriser switches the window to GL.
fn gpu_backends(data: Option<&Path>) -> Option<eframe::wgpu::Backends> {
    use eframe::wgpu::{Backends, Instance, InstanceDescriptor, PowerPreference, RequestAdapterOptions};
    let forced = data.and_then(|dir| std::fs::read_to_string(dir.join("gpu-backend")).ok()).map(|v| v.trim().to_ascii_lowercase());
    if forced.as_deref() == Some("gl") {
        return Some(Backends::GL);
    }
    if forced.as_deref() == Some("vulkan") {
        return Some(Backends::VULKAN);
    }
    let mut descriptor = InstanceDescriptor::new_without_display_handle();
    descriptor.backends = Backends::VULKAN;
    let instance = Instance::new(descriptor);
    let options = RequestAdapterOptions { power_preference: PowerPreference::None, ..Default::default() };
    let adapter = pollster::block_on(instance.request_adapter(&options));
    match adapter {
        Ok(a) => {
            let name = a.get_info().name.to_ascii_lowercase();
            let software = ["swiftshader", "llvmpipe", "softpipe", "lavapipe", "software"].iter().any(|s| name.contains(s));
            if software {
                log::info!("Vulkan adapter is `{}` (software): using the OpenGL ES path", a.get_info().name);
            }
            software.then_some(Backends::GL)
        }
        Err(e) => {
            log::info!("no Vulkan adapter ({e}): letting wgpu pick");
            None
        }
    }
}

/// The window's wgpu device asks for the adapter's own limits (textures up to 16384 px) instead
/// of eframe's fixed set, so an older GPU still opens the app. Same as the desktop shell's
/// `wgpu_options` in `apps/effectcraft/src/main.rs` (which this crate cannot depend on).
fn wgpu_options(backends: Option<eframe::wgpu::Backends>) -> eframe::WgpuConfiguration {
    use eframe::egui_wgpu::WgpuSetup;
    let mut config = eframe::WgpuConfiguration::default();
    if let WgpuSetup::CreateNew(setup) = &mut config.wgpu_setup {
        if let Some(b) = backends {
            setup.instance_descriptor.backends = b;
        }
        // Nothing draws or dispatches indirectly, so the validation check only costs a compute
        // shader at device creation (see the desktop shell's `wgpu_options`).
        setup.instance_descriptor.flags = setup.instance_descriptor.flags.difference(eframe::wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL).with_env();
        setup.device_descriptor = Arc::new(|adapter| device_descriptor(adapter.limits()));
    }
    config
}

/// The window device's descriptor: the adapter's limits, with textures capped at 16384 px like
/// the headless renderer's device.
fn device_descriptor(adapter: eframe::wgpu::Limits) -> eframe::wgpu::DeviceDescriptor<'static> {
    eframe::wgpu::DeviceDescriptor {
        label: Some("EffectCraft device"),
        required_limits: device_limits(adapter),
        memory_hints: eframe::wgpu::MemoryHints::MemoryUsage,
        ..Default::default()
    }
}

/// The limits to request for `adapter`: its own, never more than it offers.
fn device_limits(adapter: eframe::wgpu::Limits) -> eframe::wgpu::Limits {
    eframe::wgpu::Limits { max_texture_dimension_2d: adapter.max_texture_dimension_2d.min(16384), ..adapter }
}
