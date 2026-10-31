//! The app: startup (storage, session restore, hooks), the eframe app and the page glue.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use effectcraft_engine::Session;

use crate::startup::Startup;
use crate::{api, audio, browse, config_file, diskcache, files, frames, persist, storage, worker};
use effectcraft_ui_egui::EffectcraftApp;
use serde_json::json;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/js/host.js")]
extern "C" {
    #[wasm_bindgen(js_name = showDeviceLost)]
    fn show_device_lost(message: &str, kept: bool);
    #[wasm_bindgen(js_name = installCanvasShortcuts)]
    fn install_canvas_shortcuts(canvas: &web_sys::HtmlCanvasElement);
}

/// Work for the UI thread with access to the app (posted by async tasks: picked files…).
type Action = Box<dyn FnOnce(&mut EffectcraftApp)>;

static WORKER: AtomicBool = AtomicBool::new(false);

/// Running inside a Web Worker (no page, no downloads).
pub fn is_worker() -> bool {
    WORKER.load(Ordering::Relaxed)
}

pub(crate) fn set_worker() {
    WORKER.store(true, Ordering::Relaxed);
}

/// `rel` resolved against the page's URL (`worker.js`, `audio-worklet.js`); `""` = the page's
/// folder.
pub fn page_url(rel: &str) -> String {
    let href = web_sys::window().and_then(|w| w.location().href().ok()).unwrap_or_default();
    web_sys::Url::new_with_base(if rel.is_empty() { "./" } else { rel }, &href).map(|u| u.href()).unwrap_or_else(|_| rel.to_string())
}

thread_local! {
    static ACTIONS: RefCell<Vec<Action>> = const { RefCell::new(Vec::new()) };
    static CTX: RefCell<Option<egui::Context>> = const { RefCell::new(None) };
}

/// Run `f` on the UI thread at the next frame.
pub fn post(f: impl FnOnce(&mut EffectcraftApp) + 'static) {
    ACTIONS.with(|a| a.borrow_mut().push(Box::new(f)));
    repaint();
}

/// Ask for a UI frame.
pub fn repaint() {
    CTX.with(|c| {
        if let Some(c) = c.borrow().as_ref() {
            c.request_repaint();
        }
    });
}

/// Open a project or import media from table paths (picked or dropped files).
pub(crate) fn open_or_import(app: &mut EffectcraftApp, paths: Vec<String>) {
    let (projects, media): (Vec<String>, Vec<String>) = paths.into_iter().partition(|p| p.to_ascii_lowercase().ends_with(".ecproj"));
    if let Some(p) = projects.last()
        && let Err(e) = app.session.execute("file.open", json!({"path": p}))
    {
        app.ui.status = e.to_string();
    }
    if !media.is_empty() {
        match app.session.execute("file.import", json!({"paths": media})) {
            Ok(r) => {
                if let Some(errs) = r["errors"].as_array().filter(|e| !e.is_empty()) {
                    app.ui.status = errs.iter().filter_map(|e| e.as_str()).collect::<Vec<_>>().join("; ");
                }
            }
            Err(e) => app.ui.status = e.to_string(),
        }
    }
}

/// The eframe app: the shared EffectCraft UI plus the browser host's per-frame duties.
pub struct WebApp {
    pub app: EffectcraftApp,
    /// Keep the session snapshot (off with `?empty`; off with an explicit `"demo"` startup until
    /// the user's own work begins, see `demo_start`).
    pub persist: bool,
    /// Started on the demo by the config: the snapshot stays off, so the demo never replaces the
    /// last visit's project, until the user opens a project from a file or edits the demo.
    pub demo_start: bool,
}

impl eframe::App for WebApp {
    /// Take dropped files out of `raw_input` before eframe can replay them on a hidden tab, read them asynchronously
    /// through `open_or_import`, then run the shared app's input hook.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // Files dropped on the page are input: read them here, where the browser's drop event is
        // still the current pass's, not from a later (possibly logic-only) tick.
        //
        // The list is *taken* out of the raw input: a hidden tab runs `logic` and eframe appends
        // the same prepared raw input back for the next pass, so leaving the files there would
        // import them again on every hidden logic tick.
        let dropped_files = std::mem::take(&mut raw_input.dropped_files);
        let dropped: Vec<web_sys::File> = dropped_files.iter().filter_map(|f| f.web_file().cloned()).collect();
        if !dropped.is_empty() {
            files::read_files(dropped, |paths| post(move |app| open_or_import(app, paths)));
        }
        self.app.raw_input_hook(ctx, raw_input);
    }

    /// The browser host's logic tick: run queued UI actions, tick the shared app, poll API replies, flush Render
    /// Queue downloads, persist the session snapshot when enabled, publish GPU/frame-worker/disk-cache info and close
    /// the IME pass.
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let actions = ACTIONS.with(|a| std::mem::take(&mut *a.borrow_mut()));
        for f in actions {
            f(&mut self.app);
        }
        self.app.tick(ctx, frame);
        api::poll_replies();
        // Render Queue outputs written this frame (here or by a worker) → downloads.
        files::flush_downloads();
        // The user's own work after a demo start is worth keeping too: turn the snapshot on.
        if self.demo_start && (self.app.session.path.is_some() || self.app.session.is_dirty()) {
            self.demo_start = false;
            self.persist = true;
        }
        // What a reload comes back to.
        if self.persist {
            // The app's monotonic clock: `ctx.input(|i| i.time)` is stale in a logic-only tick.
            persist::snapshot(&self.app.session, self.app.now(), 1.0);
        }
        api::set_info("gpu", json!({"compositor": self.app.gpu_adapter(), "viewerOnGpu": self.app.viewer_on_gpu()}));
        api::set_info("frameWorkers", frames::stats());
        // Settings ▸ Disk ▸ Disk Cache (frames in the Origin Private File System).
        let d = &self.app.session.prefs.disk;
        diskcache::configure(d.disk_cache_enabled, d.disk_cache_max_gb, storage::quota());
        api::set_info("diskCache", diskcache::stats());
        self.app.finish_ime_logic(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.app.ui(ui, frame);
    }
}

fn query() -> Option<web_sys::UrlSearchParams> {
    web_sys::window().and_then(|w| w.location().search().ok()).and_then(|s| web_sys::UrlSearchParams::new_with_str(&s).ok())
}

fn query_flag(name: &str) -> bool {
    query().is_some_and(|p| p.has(name))
}

fn query_value(name: &str) -> Option<String> {
    query().and_then(|p| p.get(name))
}

/// A session wired for the browser: media decoding from memory, expressions, scripting, Render
/// Queue export to downloads, settings in browser storage, renders and analyses in workers.
pub fn session() -> Session {
    let pool = Arc::new(effectcraft_media::MediaPool::new());
    persist::register_media(&pool);
    Session {
        services: Arc::new(files::WebServices),
        footage: pool.clone(),
        importer: Some(Arc::new(files::WebImporter { pool })),
        expr: Some(Arc::new(effectcraft_expr::Expressions)),
        expr_check: Some(effectcraft_expr::check_syntax),
        script: Some(effectcraft_host::script::runner),
        exporter: Some(Arc::new(effectcraft_host::FileExporter { sink: Some(files::export_sink()) })),
        config: Some(Arc::new(persist::config())),
        offload: (!query_flag("noworkers")).then(|| {
            // Job workers render on a WebGPU device of their own (`?nogpuworkers`: their CPU).
            worker::set_job_worker_gpu(!query_flag("nogpuworkers"));
            Arc::new(worker::WorkerOffload) as Arc<dyn effectcraft_engine::offload::Offload>
        }),
        browser: Some(Arc::new(browse::WebBrowser)),
        storage: Some(Arc::new(storage::WebStorage)),
        ..Default::default()
    }
}

/// Browser file pickers in place of the desktop's file dialogs.
fn install_hooks(app: &mut EffectcraftApp) {
    app.hooks.pick_files = Some(Box::new(|exts: &[&str]| {
        let accept = exts.iter().map(|e| format!(".{e}")).collect::<Vec<_>>().join(",");
        if let Err(e) = files::pick(&accept, true, |paths| post(move |app| open_or_import(app, paths))) {
            log::warn!("file picker: {e:?}");
        }
        // the import happens when the browser hands over the files
        Vec::new()
    }));
    app.hooks.pick_open_project = Some(Box::new(|| {
        if let Err(e) = files::pick(".ecproj", false, |paths| post(move |app| open_or_import(app, paths))) {
            log::warn!("file picker: {e:?}");
        }
        None
    }));
    // Saving downloads the file: the chosen name is all a "save dialog" needs.
    app.hooks.pick_save = Some(Box::new(|name: &str| Some(files::path_for(if name.is_empty() { "Untitled Project.ecproj" } else { name }))));
    app.hooks.audio_device = Some(Box::new(audio::open));
    app.hooks.audio_devices = Some(Box::new(|| vec!["Browser default output".into()]));
}

/// Entry point (called by the page's bootstrap script once the wasm module is instantiated).
#[wasm_bindgen]
pub async fn start(canvas_id: String) -> Result<(), JsValue> {
    eframe::WebLogger::init(log::LevelFilter::Info).ok();
    // Match System follows `?lang=` in the address, then the browser's languages; a language
    // chosen in Settings still wins.
    let navigator = web_sys::window().map(|w| w.navigator());
    let browser = navigator.iter().flat_map(|n| n.languages().iter().filter_map(|v| v.as_string()).collect::<Vec<_>>());
    effectcraft_ui_egui::i18n::set_system_locales(query_value("lang"), browser.collect());
    let doc = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
    let canvas: web_sys::HtmlCanvasElement = doc.get_element_by_id(&canvas_id).ok_or("no canvas")?.dyn_into()?;
    install_canvas_shortcuts(&canvas);
    let empty = query_flag("empty");
    let home = query_flag("home");
    // Browser storage: settings, recent projects, imported media, saved projects, auto-saves and
    // the last session come back.
    // Monotonic (performance.now) rather than the wall clock: a system-time jump must not turn
    // the storage-load report negative or huge.
    let t0 = web_time::Instant::now();
    let stored = match persist::load(&query_value("storage").unwrap_or_default()).await {
        Ok(n) => n,
        Err(e) => {
            log::warn!("browser storage unavailable: {e}");
            0
        }
    };
    api::set_info("storage", json!({"backend": persist::backend(), "entries": stored, "loadMs": t0.elapsed().as_secs_f64() * 1000.0}));
    audio::install_unlock();
    // `./effectcraft-config.json`: what to open on load.
    // `?empty` and `?demo` decide by themselves, so they skip the request.
    let startup = if empty || query_flag("demo") { Startup::default() } else { config_file::startup().await };
    // An explicit `"demo"` start leaves the saved session alone until the user opens or edits something.
    let keep_session = !empty && startup.keeps_session();
    browse::restore();
    diskcache::init();
    storage::refresh();
    let runner = eframe::WebRunner::new();
    runner
        .start(
            canvas,
            eframe::WebOptions::default(),
            Box::new(move |cc| {
                EffectcraftApp::install_ime_reset_hooks(&cc.egui_ctx);
                CTX.with(|c| *c.borrow_mut() = Some(cc.egui_ctx.clone()));
                // The page owns this shared presentation device; worker devices retain their
                // separate owned-device handlers. No browser error-scope future is blocked on.
                let gpu_failures = effectcraft_ui_egui::gpu_failure::GpuFailureBridge::new(&cc.egui_ctx);
                if let Some(rs) = &cc.wgpu_render_state {
                    let errors = gpu_failures.clone();
                    rs.device.on_uncaptured_error(Arc::new(move |error| {
                        let message = format!("uncaptured GPU error: {error}");
                        if errors.report(&message, false) {
                            log::error!("{message}");
                        }
                    }));
                    // The canvas can't present any more (#225): the page says so over it and
                    // offers a reload, which brings the session back when browser storage keeps it.
                    let lost = gpu_failures.clone();
                    let kept = keep_session && persist::backend() != "memory";
                    rs.device.set_device_lost_callback(move |reason, message| {
                        let message = format!("GPU device lost ({reason:?}): {message}");
                        lost.report(&message, true);
                        show_device_lost(&message, kept);
                    });
                }
                let mut session = session();
                session.load_settings();
                // Scripts in Scripts/Startup run before a project opens.
                let _ = session.execute("file.runStartupScripts", json!({}));
                let recovery = session.begin_recovery();
                // `?empty`: a blank project; `?demo`: the demo project; otherwise what the config's
                // `startup` says (default: the last visit's project, the demo on a first visit).
                let demo = query_flag("demo");
                let restored = !empty && !demo && startup.restores() && persist::restore(&mut session);
                if !restored && !empty && (demo || startup.demo_when_nothing_restored()) {
                    let _ = session.execute("file.openDemoProject", json!({}));
                }
                api::set_info("restored", json!(restored));
                // The ScriptUI panels open on the last visit open again.
                let _ = session.execute("window.restoreScriptPanels", json!({}));
                let mut app = EffectcraftApp::new(session);
                app.set_gpu_failure_bridge(gpu_failures);
                app.ui.start_screen = home;
                if let Some(r) = recovery.filter(|r| !restored && r.autosave.is_some()) {
                    app.offer_recovery(r);
                }
                install_hooks(&mut app);
                // Viewer frames render in frame workers (`?frameworkers=N`, 0 or `?noworkers`:
                // on the page's thread), each on its own WebGPU device where the browser has
                // WebGPU in workers (`?nogpuworkers`: their CPU).
                let n = if query_flag("noworkers") { 0 } else { query_value("frameworkers").and_then(|v| v.parse().ok()).unwrap_or(2usize).min(8) };
                if n > 0 {
                    app.frames.set_remote(Some(Arc::new(frames::WebFrames::start(n, !query_flag("nogpuworkers")))));
                }
                app.frames.set_virtual_media(files::STORE.media_epoch(), Arc::new(|path| files::STORE.file_identity(path)));
                let (tx, rx) = std::sync::mpsc::channel();
                api::set_sender(tx);
                app = app.with_control(rx);
                let backend = cc.wgpu_render_state.as_ref().map(|rs| format!("{:?}", rs.adapter.get_info().backend));
                api::set_info("backend", json!(backend));
                Ok(Box::new(WebApp { app, persist: keep_session, demo_start: !empty && !keep_session }))
            }),
        )
        .await?;
    // Hide the loading message once the app runs.
    if let Some(el) = doc.get_element_by_id("loading") {
        el.remove();
    }
    Ok(())
}
