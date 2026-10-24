//! EffectCraft on Android — GameActivity + eframe/wgpu, reusing the desktop engine and egui UI.
//!
//! Nothing here is Android-specific beyond the entry point, the data directory, the touch size
//! adjustments, the font fallback and the storage bridge: the same `effectcraft_host::session()`
//! and `effectcraft_ui_egui::EffectcraftApp` the desktop shell builds are wired up here, and every
//! action goes through the app's own command ids over the app's own control channel.
//!
//! | Desktop (`apps/effectcraft`) | Android (this crate) |
//! |---|---|
//! | `std::env::args()` (`--control`, `--demo`, project/media files) | [`winit::platform::android::activity::AndroidApp`]; the config dir, autosaves, plug-ins and the font folder come from `$HOME` = the app's private data dir |
//! | rfd file dialogs in `Hooks` | the SAF pickers in [`saf`]; the result reaches the engine as the *same* `file.import` / `file.open` / `file.saveAs` the desktop menus run |
//! | 1680×1020 desktop window (min 960×600) | [`phone_layout`] installs a "Phone" workspace through the app's own dock model; [`TouchShell`] enlarges touch targets after EffectCraft installs its theme |
//! | cpal audio output (`audio_out.rs`) | not wired (no `audio_device` hook: previews play silently, see `Hooks::audio_device`) |
//! | macOS `muda` native menu bar | not installed; the in-window menus and the touch command palette are used |
//!
//! The engine, the UI and the theme are untouched — the menus, the import dialog, the Timeline and
//! the Composition viewer are the desktop ones. The shell only adjusts egui's spacing *after*
//! EffectCraft installs its own theme on the first frame (adding fonts any earlier drops the theme's
//! `FontFamily::Name("semibold")` / `("medium")` bindings and egui panics).
#![cfg(target_os = "android")]

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, OnceLock};

use effectcraft_ui_egui::dock::{DockNode, PanelKind, SplitSize};
use effectcraft_ui_egui::{ControlRequest, EffectcraftApp};
use winit::platform::android::activity::AndroidApp;

mod saf;
mod selftest;

/// How much to scale the desktop UI up on a phone before the layout reflows.
///
/// Measured on a 1080×2400 @ 420 dpi phone: egui's logical size is 411×914 pt at
/// `pixels_per_point = 2.625`, while EffectCraft's layout is designed for 1680×1020 (the desktop
/// window's minimum is 960×600). Scaling up only shrinks the logical viewport further and makes the
/// panels overlap more, so the shell does **not** scale: the answer is a phone workspace
/// ([`phone_layout`]) with finger-sized targets, not a zoom. Keep this at 1.0.
const TOUCH_SCALE: f32 = 1.0;

/// Name of the workspace [`phone_layout`] installs (a saved workspace, so it shows up in
/// Window ▸ Workspace and the user can switch away from it).
const PHONE_WORKSPACE: &str = "Phone";

/// The name of the CJK fallback inside egui's definitions.
const SYSTEM_FONT: &str = "system-cjk";

/// The UI context of the running app, published on the first frame: the SAF threads use it to ask
/// for a repaint so the control channel is serviced right away (an idle egui app does not run a
/// frame on its own, and the request would sit in the queue until the next touch).
static UI_CTX: OnceLock<egui::Context> = OnceLock::new();

fn wake() {
    if let Some(ctx) = UI_CTX.get() {
        ctx.request_repaint();
    }
}

/// Touch shell: forward everything to [`EffectcraftApp`] and apply the phone adjustments once,
/// after EffectCraft's own theme is in place.
struct TouchShell {
    inner: EffectcraftApp,
    /// The theme install ran and the touch sizes were applied.
    tuned: bool,
    /// Commands for the app's control channel (the same entry point the desktop control server,
    /// the CLI and MCP use).
    control: Sender<ControlRequest>,
    /// The touch command palette is open.
    palette: bool,
    /// Press bookkeeping for the long-press gesture.
    press: Option<(std::time::Instant, egui::Pos2)>,
}

impl TouchShell {
    /// Phone-sized adjustments to egui's spacing: a comfortable hit target (40 pt ≈ 105 px at
    /// 420 dpi), roomier padding and a little more separation so neighbouring controls are harder
    /// to hit by accident. EffectCraft's theme sets the colours and the panel sizes; this only
    /// touches the metrics a finger cares about.
    fn tune(ctx: &egui::Context) {
        let before = ctx.pixels_per_point();
        if (TOUCH_SCALE - 1.0).abs() > f32::EPSILON {
            ctx.set_pixels_per_point((before * TOUCH_SCALE).min(4.0));
        }
        ctx.all_styles_mut(|style| {
            style.spacing.interact_size = egui::vec2(40.0, 40.0);
            style.spacing.button_padding = egui::vec2(10.0, 8.0);
            style.spacing.item_spacing = egui::vec2(8.0, 8.0);
            style.spacing.slider_width = style.spacing.slider_width.max(160.0);
            style.spacing.icon_width = style.spacing.icon_width.max(28.0);
            // A resting finger must not pop tooltips (they cover the panel they describe).
            style.interaction.show_tooltips_only_when_still = true;
        });
        let rect = ctx.viewport_rect();
        log::info!(
            "effectcraft-android: touch mode applied (ppp {before}, logical {}×{} pt, interact {} pt)",
            rect.width().round(),
            rect.height().round(),
            ctx.style_of(ctx.theme()).spacing.interact_size.x,
        );
    }

    /// Whether the theme's own fonts are live (it defines the named families "semibold" and
    /// "medium"), i.e. the first frame's theme install has happened.
    fn theme_fonts_live(ctx: &egui::Context) -> bool {
        ctx.fonts(|f| f.definitions().families.keys().any(|k| matches!(k, egui::FontFamily::Name(_))))
    }

    /// Add the device's CJK font to egui's current definitions when the theme's own fallback could
    /// not: EffectCraft's `theme::install` appends a system face to every family when the *text
    /// engine* found one (see [`install_text_engine_font`] — on Android it only does once the
    /// system font has been copied into `$HOME/.fonts`), and this is the belt-and-braces path that
    /// guarantees the UI itself has glyphs.
    ///
    /// Read-modify-write on the *current* definitions, so the theme's own families (including
    /// `FontFamily::Name("semibold")` / `("medium")`) stay bound; the font is appended to every
    /// family, so text drawn with the theme's semibold style has glyphs too.
    fn ensure_system_font(ctx: &egui::Context) {
        let known = ctx.fonts(|f| {
            let d = f.definitions();
            // "japanese-system" is the face `theme::install` appends when the text engine has one.
            d.font_data.contains_key(SYSTEM_FONT) || d.font_data.contains_key("japanese-system")
        });
        if known {
            return;
        }
        if !Self::theme_fonts_live(ctx) {
            return; // the theme's install would drop it again
        }
        for path in SYSTEM_FONT_FILES {
            let Ok(bytes) = std::fs::read(path) else { continue };
            let mut fonts = ctx.fonts(|f| f.definitions().clone());
            fonts.font_data.insert(SYSTEM_FONT.to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
            for family in fonts.families.values_mut() {
                if !family.iter().any(|name| name == SYSTEM_FONT) {
                    family.push(SYSTEM_FONT.to_owned());
                }
            }
            ctx.set_fonts(fonts);
            log::info!("effectcraft-android: loaded system CJK font {path}");
            return;
        }
        log::warn!("effectcraft-android: no system CJK font found; CJK text will be tofu");
    }
}

/// The device's CJK faces, in preference order (all present on a stock Android image).
const SYSTEM_FONT_FILES: &[&str] = &[
    "/system/fonts/NotoSansCJK-Regular.ttc",
    "/system/fonts/NotoSerifCJK-Regular.ttc",
    "/system/fonts/NotoSansSC-Regular.otf",
    "/system/fonts/DroidSansFallback.ttf",
];

/// EffectCraft's own font scanner looks at the platform's *desktop* font folders
/// (`/usr/share/fonts`, `$HOME/.fonts`, …), not at Android's `/system/fonts`, so the text engine
/// (the Text tool, the font menus, `theme::install`'s fallback) sees no CJK face. Copying one into
/// `$HOME/.fonts` — a folder the scanner does look at — fixes both at once, with upstream code
/// doing the work: the scan finds it, and the theme appends it to every egui family.
fn install_text_engine_font(home: &Path) {
    let dir = home.join(".fonts");
    for name in ["NotoSansCJK-Regular.ttc", "NotoSerifCJK-Regular.ttc", "DroidSansFallback.ttf"] {
        let src = Path::new("/system/fonts").join(name);
        if !src.is_file() {
            continue;
        }
        let dst = dir.join(name);
        if !dst.is_file() {
            if let Err(e) = std::fs::create_dir_all(&dir) {
                log::warn!("effectcraft-android: cannot create {}: {e}", dir.display());
                return;
            }
            match std::fs::copy(&src, &dst) {
                Ok(bytes) => log::info!("effectcraft-android: system CJK font {name} → {} ({bytes} bytes)", dst.display()),
                Err(e) => {
                    log::warn!("effectcraft-android: cannot copy {name}: {e}");
                    return;
                }
            }
        }
        // Reads the folder on a worker thread; the text engine blocks on its own scan if a
        // fallback is asked for before this finishes.
        effectcraft_engine::text::fonts::scan_system_in_background();
        return;
    }
    log::warn!("effectcraft-android: no system CJK font to give the text engine");
}

impl eframe::App for TouchShell {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let _ = UI_CTX.set(ctx.clone());
        // Fully qualified: the app's entry points live in its own `eframe::App` impl.
        eframe::App::logic(&mut self.inner, ctx, frame);
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        eframe::App::raw_input_hook(&mut self.inner, ctx, raw_input);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        eframe::App::ui(&mut self.inner, ui, frame);
        if !self.tuned {
            // The first `logic` pass runs EffectCraft's theme install, so the touch adjustments
            // must come after it.
            self.tuned = true;
            Self::tune(ui.ctx());
        }
        Self::ensure_system_font(ui.ctx());
        self.gestures(ui.ctx());
        if self.palette {
            self.command_palette(ui.ctx());
        }
    }

    fn on_exit(&mut self) {
        // Clean exit: no crash recovery next launch (the app's own `on_exit`).
        eframe::App::on_exit(&mut self.inner);
    }
}

impl TouchShell {
    /// Long-press at the top of the window opens the command palette: the desktop menu bar's small
    /// items cannot be hit with a finger, so the same commands get finger-sized targets. The strip
    /// is the top 8 % of the window, where the menu bar and the workspace header sit.
    fn gestures(&mut self, ctx: &egui::Context) {
        let (down, pos, released) = ctx.input(|i| (i.pointer.primary_down(), i.pointer.latest_pos(), i.pointer.primary_released()));
        if !down || released {
            self.press = None;
            return;
        }
        let Some(p) = pos else { return };
        // While the finger is down, keep frames coming so the 500 ms threshold can pass.
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        let strip = ctx.viewport_rect().height() * 0.08;
        match self.press {
            None => self.press = Some((std::time::Instant::now(), p)),
            Some((started, origin)) => {
                if origin.distance(p) > 16.0 {
                    self.press = None; // a drag, not a press
                } else if started.elapsed() > std::time::Duration::from_millis(500) {
                    if p.y < strip && !self.palette {
                        self.palette = true;
                        log::info!("effectcraft-android: long-press on the menu strip → command palette");
                    }
                    self.press = None;
                }
            }
        }
    }

    /// The touch replacement for the menu bar: EffectCraft's own command ids as full-width buttons.
    ///
    /// Commands that ask the host for a file (`file.import`, `file.open`, `file.saveAs`, …) go
    /// through `ui.menu.invoke` — the app's menu dispatcher, which calls the `Hooks` in [`services`]
    /// and then runs the *engine* command of the same name. Everything else goes through
    /// `engine.execute`, the entry point the desktop control server uses.
    fn command_palette(&mut self, ctx: &egui::Context) {
        const COMMANDS: [(&str, &str, &str, &str); 13] = [
            ("导入素材…", "ui.menu.invoke", "file.import", ""),
            ("打开工程…", "ui.menu.invoke", "file.open", ""),
            ("保存", "ui.menu.invoke", "file.save", ""),
            ("另存为…", "ui.menu.invoke", "file.saveAs", ""),
            ("新建合成…", "ui.menu.invoke", "app.newComp", ""),
            ("打开示例工程", "engine.execute", "file.openDemoProject", ""),
            ("命令面板", "ui.menu.invoke", "app.commandPalette", ""),
            ("加入渲染队列", "engine.execute", "renderQueue.add", ""),
            ("主页", "ui.menu.invoke", "app.home", ""),
            ("设置", "ui.menu.invoke", "app.settings", ""),
            ("关于 EffectCraft", "ui.menu.invoke", "app.about", ""),
            ("工作区：Phone", "ui.menu.invoke", "window.workspace", r#"{"name":"Phone"}"#),
            ("退出", "ui.menu.invoke", "app.quit", ""),
        ];
        let mut close = false;
        egui::Area::new(egui::Id::new("command-palette")).anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0)).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(300.0);
                ui.label(egui::RichText::new("命令（长按顶部菜单条打开）").size(13.0));
                ui.separator();
                egui::ScrollArea::vertical().max_height(ctx.viewport_rect().height() * 0.6).show(ui, |ui| {
                    for (label, method, id, params) in COMMANDS {
                        if ui.add_sized([300.0, 52.0], egui::Button::new(egui::RichText::new(label).size(18.0))).clicked() {
                            self.send(label, method, id, params);
                            close = true;
                        }
                    }
                    if ui.add_sized([300.0, 44.0], egui::Button::new(egui::RichText::new("取消").size(16.0))).clicked() {
                        close = true;
                    }
                });
            });
        });
        if close {
            self.palette = false;
        }
    }

    /// Run one command over the app's control channel and log the reply: logcat then carries what
    /// the palette did, and whether the engine accepted it. `params` is the command's JSON
    /// parameters (empty for none).
    fn send(&mut self, label: &str, method: &str, id: &str, params: &str) {
        let params: serde_json::Value = if params.is_empty() { serde_json::json!({}) } else { serde_json::from_str(params).unwrap_or_default() };
        let payload = serde_json::json!({"command": id, "params": params});
        let (request, reply) = ControlRequest::new(method, payload);
        match self.control.send(request) {
            Ok(()) => log::info!("palette: {label} → {method} {id}"),
            Err(_) => {
                log::warn!("palette: control channel is closed");
                return;
            }
        }
        // The app services the control channel while it runs a frame: an idle editor does not
        // repaint on its own, so nudge it (and log the reply from here, off the UI thread).
        wake();
        // `label` and `method` are borrows, so copy them before moving into the thread.
        let (label, method) = (label.to_owned(), method.to_owned());
        std::thread::spawn(move || match reply.recv_timeout(std::time::Duration::from_secs(120)) {
            Ok(value) => log::info!("palette: {label} ({method}) replied {value}"),
            Err(e) => log::warn!("palette: no reply for {label}: {e}"),
        });
    }
}

/// The phone workspace: the Composition viewer on top, the Timeline below, with the Project and
/// Effect Controls panels as tabs of the same dock. Installed as a *saved* workspace through the
/// app's own dock model, so Window ▸ Workspace lists it and the user can switch away from it.
fn phone_workspace() -> DockNode {
    let viewer = DockNode::Tabs { panels: vec![PanelKind::Composition, PanelKind::Layer], active: 0 };
    let lower = DockNode::Tabs { panels: vec![PanelKind::Timeline, PanelKind::Project, PanelKind::EffectControls], active: 0 };
    DockNode::Split { vertical: true, size: SplitSize::Ratio(0.45), a: Box::new(viewer), b: Box::new(lower) }
}

/// Install the phone workspace (see [`phone_workspace`]) and skip the desktop-sized Home screen, so
/// a phone opens straight into a usable layout. Called before the first frame.
fn phone_layout(app: &mut EffectcraftApp) {
    app.ui.saved_workspaces.insert(PHONE_WORKSPACE.to_owned(), phone_workspace());
    app.set_workspace(PHONE_WORKSPACE);
    app.ui.start_screen = false;
}

/// The host hooks EffectCraft asks for. Everything the desktop does with `rfd` goes through the SAF
/// pickers in [`saf`]; the file kinds that have no Android equivalent (an external editor, the
/// system clipboard for the native menu bar, audio devices) are left unset, which the UI reports
/// instead of failing.
fn services(control: Sender<ControlRequest>, activity: usize, data_dir: PathBuf, exports: PathBuf) -> effectcraft_ui_egui::Hooks {
    // File ▸ Import ▸ File… / Import Multiple Files…: the picker opens asynchronously and the
    // picked paths arrive over the control channel as the desktop's own `file.import`.
    let pick_files = {
        let (control, data_dir) = (control.clone(), data_dir.clone());
        move |_exts: &[&str]| {
            spawn_pick(control.clone(), activity, data_dir.clone(), "file.import", "paths");
            Vec::new()
        }
    };
    // File ▸ Open Project…: `file.open` with the path the picker copied into the app.
    let pick_open_project = {
        let (control, data_dir) = (control.clone(), data_dir.clone());
        move || {
            spawn_pick(control.clone(), activity, data_dir.clone(), "file.open", "path");
            None
        }
    };
    // File ▸ Save As… and every other "save this file" dialog: the app writes a plain path, so the
    // hook answers with a scratch file in the app's export directory and a watcher publishes the
    // finished file where the user pointed ACTION_CREATE_DOCUMENT.
    let save = |exports: PathBuf| {
        move |name: &str| {
            let scratch = scratch_path(&exports, name);
            spawn_publish(activity, scratch.clone());
            Some(scratch)
        }
    };
    let pick_folder = {
        let exports = exports.clone();
        move || {
            log::info!("effectcraft-android: folder picker → {}", exports.display());
            Some(exports.to_string_lossy().into_owned())
        }
    };
    effectcraft_ui_egui::Hooks {
        pick_files: Some(Box::new(pick_files)),
        pick_open_project: Some(Box::new(pick_open_project)),
        pick_save: Some(Box::new(save(exports.clone()))),
        pick_save_file: Some(Box::new({
            let save = save(exports);
            move |name: &str, _ext: &str| save(name)
        })),
        // Settings paths, Collect Files, Watch Folder: the app's own export directory (a
        // `content://` tree from SAF cannot be written to by path).
        pick_folder: Some(Box::new(pick_folder)),
        // Preview playback plays silently: cpal's Android backend needs permissions and routing,
        // which this shell does not request; `None` makes the audio path a no-op, never an error.
        audio_device: None,
        audio_devices: None,
        // The native menu bar's Edit ▸ Paste and the macOS app actions: not on Android.
        clipboard_text: None,
        app_action: None,
        // Upstream 0.4.1 added these three hooks for the desktop hosts: the system appearance
        // service, the non-blocking file dialogs (macOS/Linux sheet dialogs) and the GPU device-lost
        // message. Android has no system theme service, and its SAF hooks above already return
        // without blocking the frame, so the command fallback (`pick_files`, `pick_save`, …) is the
        // right path here. `device_lost` stays unset because the port reports a lost device through
        // its own `GpuFailureBridge` (set below on the app).
        system_theme: None,
        file_dialog_async: None,
        device_lost: None,
    }
}

/// A scratch path for a "save" dialog: a sanitized file name inside the app's export directory (the
/// engine writes with plain file APIs, so a real path is what it needs).
fn scratch_path(dir: &Path, name: &str) -> String {
    let leaf = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let leaf = if leaf.is_empty() { "Untitled Project.ecproj" } else { leaf };
    let path = dir.join(leaf);
    log::info!("effectcraft-android: save dialog → {}", path.display());
    path.to_string_lossy().into_owned()
}

/// Start the SAF picker for one of EffectCraft's own file dialogs.
///
/// The hooks are synchronous (they answer "which files?" inside the frame) and a picker cannot be
/// answered synchronously without freezing the UI thread, so the hook starts this background task
/// and answers "nothing"; the chosen files reach the engine a moment later over the control
/// channel, which runs the *normal* command (`file.import` / `file.open`) — the same path the
/// desktop menu takes once the files are known.
fn spawn_pick(control: Sender<ControlRequest>, activity: usize, data_dir: PathBuf, method: &'static str, key: &'static str) {
    std::thread::spawn(move || {
        let paths = saf::pick_media(activity, &data_dir, std::time::Duration::from_secs(300));
        if paths.is_empty() {
            log::info!("effectcraft-android: nothing picked");
            return;
        }
        let params = if key == "path" { serde_json::json!({key: paths[0]}) } else { serde_json::json!({key: paths}) };
        let (request, reply) = ControlRequest::new("engine.execute", serde_json::json!({"command": method, "params": params}));
        match control.send(request) {
            Ok(()) => log::info!("effectcraft-android: sent {method} for {} file(s)", paths.len()),
            Err(_) => {
                log::warn!("effectcraft-android: control channel is closed");
                return;
            }
        }
        wake();
        match reply.recv_timeout(std::time::Duration::from_secs(300)) {
            Ok(value) => log::info!("effectcraft-android: {method} replied {value}"),
            Err(e) => log::warn!("effectcraft-android: no reply for {method}: {e}"),
        }
    });
}

/// Ask where a saved file should land, wait for the app to finish writing the scratch file, then
/// publish it there. Nothing is lost when the user cancels: the file stays in the export directory.
fn spawn_publish(activity: usize, scratch: String) {
    std::thread::spawn(move || {
        let title = scratch.rsplit('/').next().unwrap_or("export").to_owned();
        if !saf::pick_save(activity, &title, std::time::Duration::from_secs(300)) {
            log::info!("SAF: no export destination chosen; the file stays at {scratch}");
            return;
        }
        // The app writes the file after the dialog: wait until it exists and its size has stopped
        // changing for a moment.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(900);
        let (mut size, mut stable_since) = (0u64, std::time::Instant::now());
        loop {
            let now = std::fs::metadata(&scratch).map(|m| m.len()).unwrap_or(0);
            if now != size {
                size = now;
                stable_since = std::time::Instant::now();
            } else if size > 0 && stable_since.elapsed() > std::time::Duration::from_millis(1500) {
                break;
            }
            if std::time::Instant::now() > deadline {
                log::warn!("SAF: giving up on publishing {scratch} (nothing was written)");
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
        if saf::publish(activity, &scratch) {
            log::info!("SAF: published {scratch} ({size} bytes) to the chosen destination");
        } else {
            log::warn!("SAF: publishing {scratch} failed; the file stays in the export directory");
        }
    });
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag("EffectCraft"));
    log::info!("effectcraft-android: android_main entered");
    // Application-private storage: the settings, autosaves, plug-ins, font folder and self test all
    // live under it. `internal_data_path` is the same `filesDir` the Java side uses.
    let data_dir = app.internal_data_path().unwrap_or_else(std::env::temp_dir);
    // EffectCraft finds its config dir (`$HOME/.config/effectcraft`), the models folder and the
    // font folders through `$HOME`; on Android there is no home directory, so it becomes the app's
    // private one and every engine path stays inside the sandbox (nothing tries to write to `/`,
    // which would fail on every settings save).
    //
    // SAFETY: `android_main` runs on the process's main thread before eframe starts, and eframe
    // starts every EffectCraft worker afterwards — no other thread exists that could read the
    // environment concurrently, which is the only hazard `set_var` has.
    unsafe { std::env::set_var("HOME", &data_dir) };
    log::info!("effectcraft-android: HOME → {}", data_dir.display());
    // Warnings and panics go to logcat; `install()` is deliberately not called (android_logger owns
    // the logger, and the engine's own install would then be a no-op anyway).
    effectcraft_engine::logging::install_panic_hook();
    // The text engine's font scanner does not look at /system/fonts; give it a folder it does look
    // at, so CJK text (UI, Text tool, font menus) has glyphs.
    install_text_engine_font(&data_dir);
    // Self test on its own thread: decode the embedded clip and still through EffectCraft's own
    // media layer, import the clip with the engine's own command and render a frame (logcat
    // evidence that the whole spine works on the device). It never panics.
    {
        let scratch = data_dir.clone();
        std::thread::spawn(move || log::info!("effectcraft-android: {}", selftest::run(&scratch)));
    }
    // Exports and saved projects: the app's external files dir when there is one (reachable over
    // MTP/`adb pull`), else the private one.
    let exports = app.external_data_path().map(|dir| dir.join("exports")).unwrap_or_else(|| data_dir.join("exports"));
    if let Err(e) = std::fs::create_dir_all(&exports) {
        log::warn!("effectcraft-android: cannot create {}: {e}", exports.display());
    }
    log::info!("effectcraft-android: export directory {}", exports.display());
    // The Java activity object: `ndk_context` only hands out the Application, which cannot start
    // the document picker for a result.
    let activity = app.activity_as_ptr() as usize;
    let config_dir = effectcraft_host::config_dir();
    log::info!("effectcraft-android: config directory {}", config_dir.as_deref().map(|d| d.display().to_string()).unwrap_or_else(|| "(none)".into()));
    let started = eframe::run_native(
        "EffectCraft",
        eframe::NativeOptions { android_app: Some(app), ..Default::default() },
        Box::new(move |cc| {
            // The executable owns the shared device's handlers: install them before EffectCraft's
            // compositor pipelines are built, exactly as the desktop app does, so a GPU failure is
            // reported in the UI instead of taking the process down silently.
            let gpu_failures = effectcraft_ui_egui::gpu_failure::GpuFailureBridge::new(&cc.egui_ctx);
            if let Some(rs) = &cc.wgpu_render_state {
                let info = rs.adapter.get_info();
                log::info!("effectcraft-android: graphics adapter {info:?}");
                // A software rasterizer (the emulator's SwiftShader, llvmpipe) *can* run the
                // compositor, but creating its device and compiling its compute pipelines takes
                // minutes there, and every frame is slower afterwards. Report it as a failure up
                // front — the same path a real GPU failure takes — so the app composites on the
                // CPU, which is what it does by itself when a device cannot run the pipelines.
                if info.device_type == eframe::wgpu::DeviceType::Cpu {
                    log::info!("effectcraft-android: {} is a software rasterizer; compositing on the CPU", info.name);
                    gpu_failures.report("software rasterizer (CPU adapter): compositing on the CPU", false);
                }
                let errors = gpu_failures.clone();
                rs.device.on_uncaptured_error(Arc::new(move |error| {
                    let message = format!("uncaptured GPU error: {error}");
                    if errors.report(&message, false) {
                        log::error!("effectcraft-android: {message}");
                    }
                }));
                let lost = gpu_failures.clone();
                rs.device.set_device_lost_callback(move |reason, message| {
                    lost.report(&format!("GPU device lost ({reason:?}): {message}"), true);
                });
            }
            // The fully wired session the desktop starts from (media, importer, expressions,
            // scripting, Render Queue export).
            let mut session = effectcraft_host::session();
            session.check_footage_on_open = true;
            session.autosave.background = true;
            if let Some(dir) = &config_dir {
                session.config = Some(Arc::new(effectcraft_engine::config::DirConfig::new(dir)));
            }
            session.load_settings();
            // A previous run that did not exit cleanly: offer its autosave, as the desktop does.
            let recovery = session.begin_recovery();
            let recovered = recovery.is_some();
            if !recovered {
                // Nothing to recover: open the demo project, so the first launch shows a real
                // composition instead of an empty viewer (Help ▸ Open Demo Project runs the same
                // command later).
                if let Err(e) = session.execute("file.openDemoProject", serde_json::json!({})) {
                    log::warn!("effectcraft-android: the demo project did not open: {e}");
                }
            }
            let (control, requests) = std::sync::mpsc::channel::<ControlRequest>();
            let mut app = EffectcraftApp::new(session);
            app.set_gpu_failure_bridge(gpu_failures);
            phone_layout(&mut app);
            if let Some(r) = recovery {
                app.offer_recovery(r);
            }
            app.ui.status = format!("Exports and saved projects go to {}.", exports.display());
            app.hooks = services(control.clone(), activity, data_dir.clone(), exports.clone());
            let app = app.with_control(requests);
            log::info!("effectcraft-android: app created; the window shows after its first frame");
            Ok(Box::new(TouchShell { inner: app, tuned: false, control, palette: false, press: None }))
        }),
    );
    if let Err(e) = started {
        log::error!("effectcraft-android: eframe exited: {e}");
    }
}
