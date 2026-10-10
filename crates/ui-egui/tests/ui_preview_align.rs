//! Headless checks for M13.8: the Preview panel's per-shortcut options drive playback (range,
//! play from, resolution, overlays, full screen, move time), the preview shortcuts pick their
//! options, and the Align panel runs `layer.align` / `layer.distribute`.

use effectcraft_engine::Session;
use effectcraft_engine::preview::PreviewShortcut;
use effectcraft_engine::time::Tick;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use effectcraft_ui_egui::state::Resolution;
use egui::{Event, Pos2, pos2};
use egui_kittest::Harness;
use serde_json::json;

fn harness() -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Preview", "width": 320, "height": 180, "frameRate": 24, "duration": 10})).unwrap();
    s.execute("layer.newSolid", json!({"name": "A", "color": "#406080", "width": 40, "height": 30})).unwrap();
    s.execute("layer.newSolid", json!({"name": "B", "color": "#806040", "width": 60, "height": 20})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    h
}

fn invoke(h: &mut Harness<'_, EffectcraftApp>, id: &str, p: serde_json::Value) -> serde_json::Value {
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, id, p).unwrap()
}

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}")).clone();
    let p: Pos2 = pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
    h.input_mut().events.push(Event::PointerMoved(p));
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
}

fn set(h: &mut Harness<'_, EffectcraftApp>, p: serde_json::Value) {
    h.state_mut().session.execute("playback.settings.set", p).unwrap();
}

#[test]
fn preview_panel_edits_the_shown_shortcut() {
    let mut h = harness();
    invoke(&mut h, "window.panel", json!({"panel": "preview"}));
    // The default workspace keeps the Preview panel short: maximize it.
    h.state_mut().toggle_maximize(PanelKind::Preview);
    h.run_steps(3);
    for id in [
        "preview.shortcut",
        "preview.includeVideo",
        "preview.includeAudio",
        "preview.includeOverlays",
        "preview.includeLayerControls",
        "preview.loop",
        "preview.cacheBeforePlayback",
        "preview.range",
        "preview.playFrom",
        "preview.frameRate",
        "preview.skip",
        "preview.resolution",
        "preview.fullScreen",
        "preview.playCachedFrames",
        "preview.moveTimeToPreviewTime",
    ] {
        assert!(h.state().auto.find(id).is_some(), "{id}");
    }
    assert!(h.state().session.prefs.preview.spacebar.loop_);
    click(&mut h, "preview.loop");
    assert!(!h.state().session.prefs.preview.spacebar.loop_);
    // Shortcut popup: Shift+Numpad 0; its options are shown and edited from now on.
    click(&mut h, "preview.shortcut");
    click(&mut h, "preview.shortcut.3");
    assert_eq!(h.state().session.prefs.preview.current, PreviewShortcut::ShiftNumpad0);
    click(&mut h, "preview.fullScreen");
    assert!(h.state().session.prefs.preview.shift_numpad0.full_screen);
    assert!(!h.state().session.prefs.preview.spacebar.full_screen);
    // Range popup → Play Around Current Time shows the pre/post-roll.
    click(&mut h, "preview.range");
    click(&mut h, "preview.range.3");
    h.run_steps(2);
    assert!(h.state().auto.find("preview.preRoll").is_some());
}

#[test]
fn playback_follows_the_shortcut_options() {
    let mut h = harness();
    // Numpad 0: Work Area from its start; the time returns when the preview stops.
    h.state_mut().session.execute("comp.workArea", json!({"start": 2.0, "end": 4.0})).unwrap();
    set(&mut h, json!({"shortcut": "numpad0", "moveTimeToPreviewTime": false, "resolution": "quarter", "includeOverlays": false}));
    h.state_mut().session.set_time(Tick::from_seconds_f64(7.0));
    invoke(&mut h, "playback.ramPreview", json!({}));
    let app = h.state();
    assert!(app.playback.playing);
    assert_eq!(app.playback.shortcut, PreviewShortcut::Numpad0);
    let pl = app.playback.plan.unwrap();
    assert_eq!((pl.start, pl.end, pl.first), (48, 95, 48));
    assert_eq!(app.session.time(), Tick::from_seconds_f64(2.0));
    assert_eq!(app.viewer_scale(1.0, 1.0), 0.25);
    assert!(!app.overlays_visible());
    invoke(&mut h, "playback.stop", json!({}));
    assert!(!h.state().playback.playing);
    assert_eq!(h.state().session.time(), Tick::from_seconds_f64(7.0));
    assert!(h.state().overlays_visible());

    // Spacebar with Full Screen: the viewer is maximized while it plays.
    set(&mut h, json!({"shortcut": "spacebar", "fullScreen": true, "skip": 2}));
    invoke(&mut h, "playback.toggle", json!({}));
    assert_eq!(h.state().ui.maximized, Some(PanelKind::Composition));
    assert_eq!(h.state().playback.plan.unwrap().step, 3);
    invoke(&mut h, "playback.toggle", json!({}));
    assert_eq!(h.state().ui.maximized, None);

    // Shift+Spacebar from the keyboard plays with its own options.
    let ev = Event::Key { key: egui::Key::Space, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::SHIFT };
    h.input_mut().events.push(ev);
    h.step();
    assert!(h.state().playback.playing);
    assert_eq!(h.state().playback.shortcut, PreviewShortcut::ShiftSpacebar);
    assert_eq!(h.state().playback.plan.unwrap().step, 2);
}

#[test]
fn cache_before_playback_waits_then_plays_cached() {
    let mut h = harness();
    set(&mut h, json!({"shortcut": "spacebar", "cacheBeforePlayback": true, "range": "entireDuration", "playFrom": "rangeStart"}));
    invoke(&mut h, "playback.toggle", json!({}));
    assert!(h.state().playback.caching);
    // Pressing again while caching ("If caching, play cached frames") stops only when nothing
    // is cached yet, otherwise plays the cached frames.
    h.run_steps(4);
    invoke(&mut h, "playback.toggle", json!({}));
    let st = &h.state().playback;
    assert!(!st.caching);
    if st.playing {
        let pl = st.plan.unwrap();
        assert!(pl.end < 239);
    }
}

#[test]
fn align_panel_runs_align_and_distribute() {
    let mut h = harness();
    invoke(&mut h, "window.panel", json!({"panel": "align"}));
    h.state_mut().toggle_maximize(PanelKind::Align);
    h.run_steps(3);
    for id in ["align.to", "align.left", "align.vcenter", "distribute.hcenter", "distribute.bottom"] {
        assert!(h.state().auto.find(id).is_some(), "{id}");
    }
    let ids: Vec<u64> = h.state().session.active_comp().unwrap().layers.iter().map(|l| l.id.0).collect();
    h.state_mut().session.execute("layer.select", json!({"layers": ids})).unwrap();
    h.run_steps(2);
    // (x position, width) of A and B (anchors in the middle of the solids).
    let pos = |h: &Harness<'_, EffectcraftApp>| -> Vec<(f64, f64)> {
        let c = h.state().session.active_comp().unwrap();
        ["A", "B"]
            .iter()
            .map(|n| {
                let l = c.layers.iter().find(|l| l.name == *n).unwrap();
                let x = l.transform().unwrap().get("position").unwrap().value_at(Tick::ZERO).to_json()[0].as_f64().unwrap();
                (x, if *n == "A" { 40.0 } else { 60.0 })
            })
            .collect()
    };
    let orig = pos(&h);
    click(&mut h, "align.left");
    assert_eq!(pos(&h), vec![(20.0, 40.0), (30.0, 60.0)]);
    // One undo step for the whole alignment.
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(pos(&h), orig);
    // Align Layers to: Selection — right edges meet at the rightmost.
    click(&mut h, "align.to");
    click(&mut h, "align.to");
    h.state_mut().ui.align_to_selection = true;
    h.run_steps(2);
    assert_eq!(h.state().auto.find("align.to").unwrap().label, "Selection");
    click(&mut h, "align.right");
    let p = pos(&h);
    let right = |(x, w): (f64, f64)| x + w / 2.0;
    let goal = orig.iter().map(|v| right(*v)).fold(f64::NEG_INFINITY, f64::max);
    assert!((right(p[0]) - goal).abs() < 1e-9 && (right(p[1]) - goal).abs() < 1e-9, "{p:?}");
    // Distribute needs three layers: nothing moves.
    click(&mut h, "distribute.hcenter");
    assert_eq!(pos(&h), p);
}

#[test]
fn cache_before_playback_plays_what_fits_in_ram() {
    let mut h = harness();
    set(&mut h, json!({"shortcut": "spacebar", "cacheBeforePlayback": true, "range": "entireDuration", "playFrom": "rangeStart"}));
    h.run_steps(2);
    // Room for about 12 full-size frames: the 240-frame range can't all be cached.
    h.state().frames.set_budget(320 * 180 * 4 * 12);
    invoke(&mut h, "playback.toggle", json!({}));
    assert!(h.state().playback.caching);
    for _ in 0..600 {
        h.step();
        if !h.state().playback.caching {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let st = &h.state().playback;
    assert!(st.playing && !st.caching, "caching finished and the preview plays");
    let pl = st.plan.unwrap();
    assert!(pl.start == 0 && pl.end < 239 && pl.end >= 9, "cut to what fits: {}..{}", pl.start, pl.end);
}

#[test]
fn auto_resolution_is_sharp_when_paused_and_cheap_during_playback() {
    let mut h = harness();
    let cid = h.state().session.active_comp_id().unwrap();
    assert_eq!(h.state().ui.viewer.res, Resolution::Auto);
    assert_eq!(h.state().viewer_scale(0.2, 1.0), 1.0);
    let still_key = h.state().frame_key(cid, 0, 1.0);
    h.state_mut().playback.playing = true;
    assert_eq!(h.state().viewer_scale(0.2, 1.0), 0.25);
    assert_ne!(still_key.scale, h.state().frame_key(cid, 0, 0.25).scale);
    h.state_mut().playback.playing = false;
    h.state_mut().ui.viewer.property_interacting = true;
    assert_eq!(h.state().viewer_scale(0.2, 1.0), 0.25);
    h.state_mut().ui.viewer.property_interacting = false;
    assert_eq!(h.state().viewer_scale(0.2, 1.0), 1.0);
    h.state_mut().ui.viewer.res = Resolution::Half;
    assert_eq!(h.state().viewer_scale(0.2, 1.0), 0.5);
}

#[test]
fn paused_auto_resolution_caps_large_comps() {
    assert_eq!(Resolution::paused_auto_scale(1920, 1080), 1.0);
    assert_eq!(Resolution::paused_auto_scale(4096, 2160), 1.0);
    assert_eq!(Resolution::paused_auto_scale(8192, 4320), 0.5);
    assert_eq!(Resolution::paused_auto_scale(16384, 8640), 0.25);
}

#[test]
fn dragging_the_time_ruler_uses_interactive_resolution() {
    let mut h = harness();
    let cti = h.state().auto.find("timeline.cti").unwrap().clone();
    let from = pos2(cti.rect[0] + cti.rect[2] / 2.0, cti.rect[1] + cti.rect[3] / 2.0);
    h.input_mut().events.push(Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    let to = from + egui::vec2(40.0, 0.0);
    h.input_mut().events.push(Event::PointerMoved(to));
    h.step();
    assert!(h.state().ui.viewer.property_interacting);
    assert_eq!(h.state().viewer_scale(0.2, 1.0), 0.25);
    h.input_mut().events.push(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
    assert!(!h.state().ui.viewer.property_interacting);
    assert_eq!(h.state().viewer_scale(0.2, 1.0), 1.0);
}

#[test]
fn achieved_frame_rate_and_real_time() {
    let mut p = effectcraft_ui_egui::Playback { playing: true, fps: 24.0, ..Default::default() };
    assert_eq!(p.achieved_fps(), None);
    for i in 0..25 {
        p.frame_shown(i as f64 / 24.0);
    }
    assert!((p.achieved_fps().unwrap() - 24.0).abs() < 1e-9);
    assert_eq!(p.real_time(), Some(true));
    let mut slow = effectcraft_ui_egui::Playback { playing: true, fps: 24.0, ..Default::default() };
    for i in 0..13 {
        slow.frame_shown(i as f64 / 12.0);
    }
    assert_eq!(slow.real_time(), Some(false));
    assert_eq!(slow.shown_at.len(), 13, "only the last second counts");
}

#[test]
fn cache_frames_when_idle_fills_the_work_area() {
    let mut h = harness();
    let cid = h.state().session.active_comp_id().unwrap();
    let cached = |h: &Harness<'_, EffectcraftApp>| h.state().frames.cached_frames(&h.state().shown_series(cid)).len();
    // Off: only a few frames ahead of the current time are prefetched (the work area is the
    // whole 240 frames).
    for _ in 0..120 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let before = cached(&h);
    assert!(before < 240, "{before}");
    invoke(&mut h, "playback.cacheWhenIdle", json!({"value": true}));
    let mut n = 0;
    for _ in 0..4000 {
        h.step();
        n = cached(&h);
        if n >= 240 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(n, 240, "the work area is cached while idle");
}

#[test]
fn cache_frames_when_idle_survives_an_inverted_work_area() {
    let mut h = harness();
    // Start set past the end: an empty work area (it panicked in `clamp` / `rem_euclid`).
    invoke(&mut h, "comp.workArea", json!({"end": 2.0}));
    invoke(&mut h, "comp.workArea", json!({"start": 5.0}));
    invoke(&mut h, "playback.cacheWhenIdle", json!({"value": true}));
    for _ in 0..150 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(!h.state().playback.playing);
}
