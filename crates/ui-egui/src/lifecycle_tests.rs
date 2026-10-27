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

fn screenshot_request(app: &mut EffectcraftApp, ctx: &egui::Context, path: Option<&str>, target: ShotTarget) -> std::sync::mpsc::Receiver<serde_json::Value> {
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
