//! Audio scrubbing: Ctrl/Cmd-dragging the current time plays one frame of the comp's mix at each
//! new frame (each snippet replacing what is still queued); holding still repeats nothing; silent
//! comps open no output; an idle scrub closes it. Previews with audio wait for frames.

use std::sync::{Arc, Mutex};

use effectcraft_engine::Session;
use effectcraft_engine::project::ItemId;
use effectcraft_engine::time::Tick;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::audio::{AudioDevice, AudioFeed};
use effectcraft_ui_egui::frames::{RemoteDone, RemoteFrame, RemoteFrames, RemoteJob};
use egui::{Event, Modifiers, pos2};
use egui_kittest::Harness;
use serde_json::json;

/// An output that records the feed it is given and whether it was stopped.
#[derive(Clone, Default)]
struct Fake(Arc<Mutex<(Option<Arc<AudioFeed>>, bool)>>);

impl AudioDevice for Fake {
    fn sample_rate(&self) -> u32 {
        48000
    }
    fn start(&mut self, feed: Arc<AudioFeed>) -> Result<(), String> {
        self.0.lock().unwrap().0 = Some(feed);
        Ok(())
    }
    fn stop(&mut self) {
        self.0.lock().unwrap().1 = true;
    }
}

/// A frame renderer whose frames the test releases explicitly. Playback then runs against a
/// controlled clock and controlled cache contents instead of racing however many frame workers
/// the host has; a frame that is not released is queued again by the next playback step.
#[derive(Clone, Default)]
struct GatedFrames {
    pending: Arc<Mutex<Vec<(i64, u32, u32, RemoteDone)>>>,
}

impl RemoteFrames for GatedFrames {
    fn slots(&self) -> usize {
        1024
    }

    fn start(&self, job: RemoteJob, done: RemoteDone) {
        let (frame, width, height) = job.project.comp(job.comp).map_or((-1, 1, 1), |c| {
            let scale = job.opts.scale.max(0.001);
            let width = (c.width as f64 * scale).round().clamp(1.0, 16_384.0) as u32;
            let height = (c.height as f64 * scale).round().clamp(1.0, 16_384.0) as u32;
            (c.frame_rate.frame_at(job.t), width, height)
        });
        self.pending.lock().unwrap().push((frame, width, height, done));
    }
}

impl GatedFrames {
    /// Put `frames` in the cache, dropping every other queued frame (playback asks again).
    fn render(&self, h: &mut Harness<'_, EffectcraftApp>, cid: ItemId, frames: &[i64]) {
        let scale = h.state().shown_series(cid).scale as f64 / 1000.0;
        for &f in frames {
            h.state().request_frame(cid, f, scale);
        }
        while h.state().frames.dispatch_remote() != 0 {}
        let pending = std::mem::take(&mut *self.pending.lock().unwrap());
        for (frame, width, height, done) in pending {
            if frames.contains(&frame) {
                let rgba = vec![0; (width as usize).saturating_mul(height as usize).saturating_mul(4)];
                done(Ok(RemoteFrame { width, height, rgba, ms: 1.0 }));
            } else {
                done(Err("held back by the test renderer".into()));
            }
        }
    }

    /// Drop everything queued without caching it (the warm-up passes request viewer frames).
    fn discard(&self, h: &mut Harness<'_, EffectcraftApp>) {
        while h.state().frames.dispatch_remote() != 0 {}
        for (_, _, _, done) in std::mem::take(&mut *self.pending.lock().unwrap()) {
            done(Err("held back by the test renderer".into()));
        }
    }
}

/// A 2 s, 30 fps comp whose solid sounds a tone (Audio switch on).
fn setup(audible: bool, fake: &Fake) -> (EffectcraftApp, ItemId) {
    setup_long(audible, fake, 2.0)
}

/// A `duration`-second, 30 fps comp whose solid sounds a tone (Audio switch on).
fn setup_long(audible: bool, fake: &Fake, duration: f64) -> (EffectcraftApp, ItemId) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "A", "width": 64, "height": 36, "frameRate": 30, "duration": duration})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let l = s.execute("layer.newSolid", json!({"color": "#808080"})).unwrap()["layer"].as_u64().unwrap();
    if audible {
        s.execute("effect.apply", json!({"layers": [l], "effect": "ec.audio.tone"})).unwrap();
        s.execute("layer.setSwitch", json!({"layers": [l], "switch": "audio", "value": true})).unwrap();
    }
    let mut app = EffectcraftApp::new(s);
    let f = fake.clone();
    app.hooks.audio_device = Some(Box::new(move |_| Some(Box::new(f.clone()) as Box<dyn AudioDevice>)));
    (app, cid)
}

#[test]
fn scrubbing_plays_one_frame_per_new_frame() {
    let fake = Fake::default();
    let (mut app, cid) = setup(true, &fake);
    let t = |s: f64| Tick::from_seconds_f64(s);
    app.scrub_audio(cid, t(0.5), 0.0);
    let feed = fake.0.lock().unwrap().0.clone().expect("the output opened");
    // One 30 fps frame at 48 kHz, and not silence.
    assert_eq!(feed.queued_frames(), 1600);
    let mut out = vec![0.0f32; 3200];
    feed.pull(&mut out);
    assert!(out.iter().any(|v| v.abs() > 0.01), "the tone is there");
    // Faded in: the first sample is silent.
    assert_eq!(out[0], 0.0);
    // The same frame again plays nothing; the next frame replaces what is left.
    app.scrub_audio(cid, t(0.51), 0.1);
    assert_eq!(feed.queued_frames(), 0);
    feed.push(&[0.0; 1000]);
    app.scrub_audio(cid, t(0.6), 0.2);
    assert_eq!(feed.queued_frames(), 1600, "replaced, not piled up");
    // A silent comp opens nothing.
    let silent = Fake::default();
    let (mut quiet, qid) = setup(false, &silent);
    quiet.scrub_audio(qid, t(0.5), 0.0);
    assert!(silent.0.lock().unwrap().0.is_none() && quiet.scrub.is_none());
}

#[test]
fn cmd_dragging_the_current_time_scrubs_and_an_idle_scrub_closes() {
    let fake = Fake::default();
    let (app, _) = setup(true, &fake);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| app);
    h.run_steps(3);
    let cti = h.state().auto.find("timeline.cti").expect("current time indicator").clone();
    let from = pos2(cti.rect[0] + cti.rect[2] / 2.0, cti.rect[1] + cti.rect[3] / 2.0);
    let cmd = Modifiers::COMMAND;
    h.event(Event::ModifiersChanged(cmd));
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: cmd });
    h.step();
    for k in 1..=4 {
        h.event(Event::PointerMoved(from + egui::vec2(40.0 * k as f32, 0.0)));
        h.step();
    }
    assert!(h.state().scrub.is_some(), "Cmd-drag opened the scrub output");
    assert!(fake.0.lock().unwrap().0.as_ref().is_some_and(|f| f.queued_frames() > 0));
    h.event(Event::PointerButton { pos: from + egui::vec2(160.0, 0.0), button: egui::PointerButton::Primary, pressed: false, modifiers: cmd });
    h.step();
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
    // An idle second closes the output.
    for _ in 0..90 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(15));
        if h.state().scrub.is_none() {
            break;
        }
    }
    assert!(h.state().scrub.is_none() && fake.0.lock().unwrap().1, "closed and stopped");
}

/// The comp's frame at the current time.
fn frame(h: &Harness<'_, EffectcraftApp>, cid: ItemId) -> i64 {
    let app = h.state();
    app.session.project.comp(cid).unwrap().frame_rate.frame_at(app.session.time())
}

/// The frame under the preview audio's first sample (its clock before the device pulls any).
fn audio_frame(h: &Harness<'_, EffectcraftApp>, cid: ItemId) -> Option<i64> {
    let app = h.state();
    let a = app.audio.as_ref()?;
    Some(effectcraft_ui_egui::audio::frame_of_sample(a.clock(), a.rate, app.session.project.comp(cid).unwrap().frame_rate))
}

/// Step until the preview's sound starts (or 60 s pass, for a loaded machine), checking the
/// playhead holds `first` meanwhile. Returns whether the sound started.
fn until_sound(h: &mut Harness<'_, EffectcraftApp>, cid: ItemId, first: i64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while std::time::Instant::now() < deadline {
        step(h);
        if h.state().audio.is_some() {
            return true;
        }
        assert_eq!(frame(h, cid), first, "the playhead holds the first frame until the sound can start");
        assert!(h.state().playback.starting && h.state().playback.playing);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    false
}

/// One step with the app clock, which playback runs on, a step (1/30 s) further on: these tests
/// count time in steps, however long a step takes on a loaded machine.
fn step(h: &mut Harness<'_, EffectcraftApp>) {
    h.state_mut().clock_mut().advance(1.0 / 30.0);
    h.step();
}

/// Starting a preview with sound before its frames are cached (#947): the playhead holds the
/// first frame while the frames ahead render, then sound and picture start together from it (the
/// sound used to start at a later frame, losing the opening). A frame that isn't cached later on
/// still stops the sound rather than being skipped (#103).
#[test]
fn preview_with_audio_holds_the_first_frame_and_sounds_from_it() {
    let fake = Fake::default();
    let (mut app, cid) = setup(true, &fake);
    // Release only the frames this test chooses, and drive the app clock: rendering cannot race
    // the clock on a slow or loaded test host.
    let gate = GatedFrames::default();
    let renderer: Arc<dyn RemoteFrames> = Arc::new(gate.clone());
    app.frames.set_remote(Some(renderer));
    let ahead = (app.frames.parallelism() * 2).max(4) as i64;
    app.session.execute("comp.settings", json!({"comp": cid.0, "duration": (4 * ahead) as f64 / 30.0})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 30.0).build_eframe(|_| app);
    h.state_mut().use_manual_clock();
    h.run_steps(2);
    gate.discard(&mut h);
    let now = h.state().now();
    h.state_mut().play(now);
    assert!(h.state().playback.starting && h.state().audio.is_none(), "nothing is cached: no sound yet");
    // Half the frames ahead cached: the playhead holds the first frame, silently.
    let half: Vec<i64> = (0..ahead / 2).collect();
    gate.render(&mut h, cid, &half);
    step(&mut h);
    assert!(h.state().playback.starting && h.state().audio.is_none(), "{} of {ahead} frames ahead cached: no sound yet", half.len());
    assert_eq!(frame(&h, cid), 0, "the playhead holds the first frame");
    // All of them: the sound starts, from that frame.
    let all: Vec<i64> = (0..ahead).collect();
    gate.render(&mut h, cid, &all);
    step(&mut h);
    assert!(h.state().audio.is_some() && fake.0.lock().unwrap().0.is_some(), "the sound started once the frames ahead were cached");
    assert_eq!(audio_frame(&h, cid), Some(0), "from the first frame, not a later one");
    assert!(!h.state().playback.starting);
    // A frame that isn't cached stops the sound rather than being skipped.
    gate.discard(&mut h);
    h.state().frames.clear();
    h.step();
    assert!(h.state().audio.is_none() && h.state().playback.audio_held && h.state().playback.playing);
}

/// A long (30 s) preview sounds once the frames ahead of its first frame are cached, not the
/// whole work area (#208), and from that first frame (#947).
#[test]
fn preview_with_audio_sounds_without_caching_the_work_area() {
    let fake = Fake::default();
    let (mut app, cid) = setup_long(true, &fake, 30.0);
    let gate = GatedFrames::default();
    let renderer: Arc<dyn RemoteFrames> = Arc::new(gate.clone());
    app.frames.set_remote(Some(renderer));
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 30.0).build_eframe(|_| app);
    h.state_mut().use_manual_clock();
    h.run_steps(2);
    gate.discard(&mut h);
    let ahead = (h.state().frames.parallelism() * 2).max(4) as i64;
    let now = h.state().now();
    h.state_mut().play(now);
    let frames: Vec<i64> = (0..ahead).collect();
    gate.render(&mut h, cid, &frames);
    step(&mut h);
    assert!(h.state().audio.is_some(), "the sound started");
    assert_eq!(audio_frame(&h, cid), Some(0), "from the first frame");
    let cached = h.state().frames.cached_frames(&h.state().shown_series(cid)).len();
    assert!((1..900).contains(&cached), "it started with {cached} of the 900 frames cached, not the whole work area");
}

/// Pull `frames` stereo frames from the output, waiting for the feeder thread to mix them.
fn pull(feed: &AudioFeed, frames: usize) -> Vec<f32> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while feed.queued_frames() < frames && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let mut out = vec![0.0f32; frames * 2];
    feed.pull(&mut out);
    out
}

/// The opening of the preview is heard (#947): a sound that lasts only the first 100 ms after the
/// start, from the beginning of the comp or from the middle, reaches the output device.
#[test]
fn a_sound_in_the_first_100_ms_is_heard() {
    for start in [0.0, 1.0] {
        let fake = Fake::default();
        let (mut app, cid) = setup_long(false, &fake, 4.0);
        // A tone only from `start` to `start` + 0.1 s.
        let l = app.session.execute("layer.newSolid", json!({"color": "#808080"})).unwrap()["layer"].as_u64().unwrap();
        app.session.execute("effect.apply", json!({"layers": [l], "effect": "ec.audio.tone"})).unwrap();
        app.session.execute("layer.setSwitch", json!({"layers": [l], "switch": "audio", "value": true})).unwrap();
        app.session.execute("layer.timing", json!({"layers": [l], "in": start, "out": start + 0.1})).unwrap();
        app.session.execute("time.set", json!({"time": start})).unwrap();
        let first = (start * 30.0) as i64;
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 30.0).build_eframe(|_| app);
        h.state_mut().use_manual_clock();
        h.run_steps(2);
        let now = h.state().now();
        h.state_mut().play(now);
        assert!(until_sound(&mut h, cid, first), "{start}: the sound started");
        assert_eq!(audio_frame(&h, cid), Some(first), "{start}: from the first frame");
        let feed = fake.0.lock().unwrap().0.clone().expect("the output opened");
        // The first 100 ms at 48 kHz.
        let out = pull(&feed, 4800);
        assert!(out.iter().any(|v| v.abs() > 0.01), "{start}: the opening tone reached the output");
    }
}

/// A preview without video starts its sound at once, from the first frame (unchanged), and so
/// does one whose frames are already cached.
#[test]
fn audio_only_previews_start_at_once() {
    let fake = Fake::default();
    let (app, cid) = setup(true, &fake);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| app);
    h.state_mut().use_manual_clock();
    h.run_steps(2);
    let now = h.state().now();
    h.state_mut().play_with(now, effectcraft_engine::preview::PreviewShortcut::AltNumpad0);
    assert!(h.state().audio.is_some() && !h.state().playback.starting, "Video off: the sound starts at once");
    assert_eq!(audio_frame(&h, cid), Some(0));
}

/// Without an output device the preview still plays, from the first frame, once the frames ahead
/// are cached; Stop while it prepares leaves nothing playing.
#[test]
fn no_device_plays_the_picture_and_stop_ends_the_wait() {
    let fake = Fake::default();
    let (mut app, cid) = setup_long(true, &fake, 10.0);
    app.hooks.audio_device = Some(Box::new(|_| None));
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 30.0).build_eframe(|_| app);
    h.state_mut().use_manual_clock();
    h.run_steps(2);
    let now = h.state().now();
    h.state_mut().play(now);
    assert!(h.state().playback.starting);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while h.state().playback.starting && std::time::Instant::now() < deadline {
        step(&mut h);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(!h.state().playback.starting && h.state().audio.is_none() && h.state().playback.playing, "no device: the picture plays alone");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while frame(&h, cid) == 0 && std::time::Instant::now() < deadline {
        step(&mut h);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(frame(&h, cid) > 0, "from the first frame on");
    // Stop while preparing.
    h.state_mut().stop();
    let now = h.state().now();
    let device = fake.clone();
    h.state_mut().hooks.audio_device = Some(Box::new(move |_| Some(Box::new(device.clone()) as Box<dyn AudioDevice>)));
    h.state().frames.clear();
    h.state_mut().play(now);
    // One step into preparing: the frames ahead are requested and it waits for them.
    step(&mut h);
    assert!(h.state().playback.starting && h.state().playback.waiting && h.state().audio.is_none(), "preparing");
    h.state_mut().stop();
    // The renders it queued finish after Stop: they must not start the sound.
    for _ in 0..30 {
        step(&mut h);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!h.state().playback.playing && !h.state().playback.starting && h.state().audio.is_none(), "Stop while preparing: nothing plays");
    assert!(fake.0.lock().unwrap().0.is_none(), "the output never opened");
}

/// Start a preview and step until it leaves its startup (or 60 s pass). Returns whether it did.
fn until_started(h: &mut Harness<'_, EffectcraftApp>) -> bool {
    let now = h.state().now();
    h.state_mut().play(now);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while h.state().playback.starting && std::time::Instant::now() < deadline {
        step(h);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    !h.state().playback.starting
}

/// RAM preview budgets just below, at and above the prefetch's frames ahead, and small ones, from
/// a cold cache, with and without an output device: the start never waits for a frame the
/// prefetch doesn't request (with a budget of 4 frames it asks for 3 ahead of the playhead).
#[test]
fn startup_finishes_at_any_ram_preview_budget() {
    let probe = Fake::default();
    let (app, _) = setup(true, &probe);
    let ahead = (app.frames.parallelism() * 2).max(4) as i64;
    // A 64×36 frame is 9216 bytes; `fit` frames fit in (fit + ½) frames' worth / 0.9.
    let mut fits = vec![2, 3, 4, ahead - 1, ahead, ahead + 1];
    fits.sort();
    fits.dedup();
    for fit in fits {
        for device in [true, false] {
            let fake = Fake::default();
            let (mut app, cid) = setup_long(true, &fake, 8.0);
            if !device {
                app.hooks.audio_device = Some(Box::new(|_| None));
            }
            let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 30.0).build_eframe(|_| app);
            h.state_mut().use_manual_clock();
            h.run_steps(2);
            let budget = ((fit as f64 + 0.5) * 9216.0 / 0.9) as usize;
            h.state().frames.set_budget(budget);
            h.state().frames.clear();
            assert!(until_started(&mut h), "fit {fit} (ahead {ahead}), device {device}: the preview left its startup");
            assert_eq!(h.state().audio.is_some(), device, "fit {fit}, device {device}");
            if device {
                assert_eq!(audio_frame(&h, cid), Some(0), "fit {fit}: from the first frame");
            }
        }
    }
}

/// An audible 160×120 comp with a 3D layer and Draft 3D, Extended Viewer on: its viewer frames
/// render the pasteboard too, larger than the comp. Returns the harness once the first frame has
/// rendered, the comp, and that frame's size.
fn extended_viewer(fake: &Fake) -> (Harness<'static, EffectcraftApp>, ItemId, [usize; 2]) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "E", "width": 160, "height": 120, "frameRate": 30, "duration": 8})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let l = s.execute("layer.newSolid", json!({"color": "#4060a0"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("effect.apply", json!({"layers": [l], "effect": "ec.audio.tone"})).unwrap();
    s.execute("layer.setSwitch", json!({"layers": [l], "switch": "audio", "value": true})).unwrap();
    s.execute("layer.setSwitch", json!({"layers": [l], "switch": "threeD", "value": true})).unwrap();
    s.execute("comp.setSwitch", json!({"switch": "draft3d", "value": true})).unwrap();
    s.prefs.three_d.extended_viewer = true;
    let mut app = EffectcraftApp::new(s);
    let f = fake.clone();
    app.hooks.audio_device = Some(Box::new(move |_| Some(Box::new(f.clone()) as Box<dyn AudioDevice>)));
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 30.0).build_eframe(|_| app);
    h.state_mut().use_manual_clock();
    // The viewer shows the pasteboard: frames render the extended region.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let size = loop {
        step(&mut h);
        let series = h.state().shown_series(cid);
        if let Some(img) = h.state().frames.get(&series) {
            break img.size();
        }
        assert!(std::time::Instant::now() < deadline, "the first frame rendered");
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert!(h.state().ui.viewer.extended.is_some(), "Extended Viewer is on");
    (h, cid, size)
}

/// Extended Viewer frames render larger than the comp: a budget that would hold `ahead` comp-sized
/// frames holds fewer of them, and the start waits only for as many as really fit (#947).
#[test]
fn startup_finishes_when_extended_viewer_frames_are_larger() {
    let fake = Fake::default();
    let (mut h, cid, size) = extended_viewer(&fake);
    let ahead = (h.state().frames.parallelism() * 2).max(4);
    let (comp_bytes, real_bytes) = (160 * 120 * 4, size[0] * size[1] * 4);
    // Room for `ahead` comp-sized frames (and a half), which is fewer real ones.
    let budget = ((ahead as f64 + 0.5) * comp_bytes as f64 / 0.9) as usize;
    assert!((budget as f64 * 0.9 / real_bytes as f64) < ahead as f64, "real frames ({size:?}) fit fewer than {ahead}");
    h.state().frames.set_budget(budget);
    h.state().frames.clear();
    let now = h.state().now();
    h.state_mut().play(now);
    step(&mut h);
    // The first frame renders before the harness clock moves on: each step is 1/30 s however long
    // it takes, so on a busy machine the first frame could take longer than STARTUP_LIMIT of that
    // clock, and this test is about the wait for the frames ahead.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !h.state().frames.is_cached(&h.state().shown_series(cid)) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    while h.state().playback.starting && std::time::Instant::now() < deadline {
        step(&mut h);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(!h.state().playback.starting, "the preview left its startup");
    assert!(h.state().audio.is_some(), "with its sound");
    assert_eq!(audio_frame(&h, cid), Some(0), "from the first frame");
    let (started, stopped) = {
        let d = fake.0.lock().unwrap();
        (d.0.is_some(), d.1)
    };
    assert!(started && !stopped, "the device started and kept playing");
}

/// A first frame that takes longer than STARTUP_WAIT to be cached (a heavy comp, a busy machine;
/// here it can't be cached until the RAM preview grows): the start keeps holding it, then the
/// sound starts from it and keeps playing. It used to give up two seconds after Play, start the
/// sound on the uncached frame and stop it at once, so the opening played silently.
#[test]
fn a_slow_first_frame_keeps_its_opening_sound() {
    let fake = Fake::default();
    let (mut h, cid, size) = extended_viewer(&fake);
    let (comp_bytes, real_bytes) = (160 * 120 * 4, size[0] * size[1] * 4);
    assert!(real_bytes > comp_bytes * 2, "Extended Viewer frames ({size:?}) are larger than the comp");
    h.state().frames.set_budget((comp_bytes + real_bytes) / 2);
    h.state().frames.clear();
    let start = h.state().now();
    h.state_mut().play(start);
    while h.state().now() - start < 3.0 {
        step(&mut h);
        assert!(h.state().playback.starting, "it still holds the first frame");
        assert_eq!(frame(&h, cid), 0);
    }
    assert!(fake.0.lock().unwrap().0.is_none(), "no sound yet");
    h.state().frames.set_budget(1 << 30);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while h.state().playback.starting && std::time::Instant::now() < deadline {
        step(&mut h);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(!h.state().playback.starting, "the preview left its startup");
    assert_eq!(audio_frame(&h, cid), Some(0), "with its sound from the first frame");
    let (started, stopped) = {
        let d = fake.0.lock().unwrap();
        (d.0.is_some(), d.1)
    };
    assert!(started && !stopped, "the device started and kept playing");
}

/// An output device whose start fails.
#[derive(Clone, Default)]
struct Broken;

impl AudioDevice for Broken {
    fn sample_rate(&self) -> u32 {
        48000
    }
    fn start(&mut self, _feed: Arc<AudioFeed>) -> Result<(), String> {
        Err("device unavailable".into())
    }
    fn stop(&mut self) {}
}

/// A device that fails to start: the picture plays on its own from the first frame.
#[test]
fn a_device_that_fails_to_start_plays_the_picture() {
    let fake = Fake::default();
    let (mut app, cid) = setup_long(true, &fake, 8.0);
    app.hooks.audio_device = Some(Box::new(|_| Some(Box::new(Broken) as Box<dyn AudioDevice>)));
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 30.0).build_eframe(|_| app);
    h.state_mut().use_manual_clock();
    h.run_steps(2);
    assert!(until_started(&mut h), "it left its startup");
    assert!(h.state().audio.is_none() && h.state().playback.playing);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while frame(&h, cid) == 0 && std::time::Instant::now() < deadline {
        step(&mut h);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(frame(&h, cid) > 0, "the picture plays");
}

/// An output that plays: a thread takes the feed's samples in real time, as a sound card would.
#[derive(Clone, Default)]
struct Playing(Arc<Mutex<bool>>);

impl AudioDevice for Playing {
    fn sample_rate(&self) -> u32 {
        48000
    }
    fn start(&mut self, feed: Arc<AudioFeed>) -> Result<(), String> {
        let stop = self.0.clone();
        std::thread::spawn(move || {
            let mut buf = vec![0.0f32; 480 * 2];
            while !*stop.lock().unwrap() {
                feed.pull(&mut buf);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        });
        Ok(())
    }
    fn stop(&mut self) {
        *self.0.lock().unwrap() = true;
    }
}

/// Once started, the sound's clock moves the picture on from the first frame, and a frame that
/// isn't cached still stops the sound rather than being skipped.
#[test]
fn a_started_preview_follows_its_sound() {
    let fake = Fake::default();
    let (mut app, cid) = setup_long(true, &fake, 8.0);
    let device = Playing::default();
    let d = device.clone();
    app.hooks.audio_device = Some(Box::new(move |_| Some(Box::new(d.clone()) as Box<dyn AudioDevice>)));
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 30.0).build_eframe(|_| app);
    h.state_mut().use_manual_clock();
    h.run_steps(2);
    assert!(until_started(&mut h) && h.state().audio.is_some(), "the sound started");
    assert_eq!(audio_frame(&h, cid), Some(0), "from the first frame");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while frame(&h, cid) < 5 && std::time::Instant::now() < deadline && h.state().audio.is_some() {
        step(&mut h);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(frame(&h, cid) >= 5, "the picture follows the sound's clock (at frame {})", frame(&h, cid));
    h.state().frames.set_budget(0);
    h.state().frames.clear();
    step(&mut h);
    assert!(h.state().audio.is_none() && h.state().playback.audio_held && h.state().playback.playing, "a stall stops the sound");
    *device.0.lock().unwrap() = true;
}

/// A RAM preview that can't hold even one frame (none, or half of one): the start doesn't wait
/// for frames that can never be cached. The sound isn't started on a frame that can't be shown
/// (a stall would stop it at once): the preview goes on as a stalled one does.
#[test]
fn startup_does_not_wait_when_the_ram_preview_holds_no_frame() {
    for budget in [0, 9216 / 2] {
        let fake = Fake::default();
        let (app, _) = setup_long(true, &fake, 8.0);
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 30.0).build_eframe(|_| app);
        h.state_mut().use_manual_clock();
        h.run_steps(2);
        h.state().frames.set_budget(budget);
        h.state().frames.clear();
        let now = h.state().now();
        h.state_mut().play(now);
        step(&mut h);
        assert!(!h.state().playback.starting, "budget {budget}: no wait");
        assert!(fake.0.lock().unwrap().0.is_none(), "budget {budget}: no sound started and dropped");
        assert!(h.state().playback.playing && h.state().playback.audio_held, "budget {budget}: it goes on as a stalled preview");
    }
}

/// A frame that never gets cached although the estimate says one fits (an Extended Viewer frame
/// larger than the whole budget): the start gives up after STARTUP_LIMIT instead of holding the
/// first frame for ever, without starting the sound on a frame it can't show.
#[test]
fn startup_gives_up_on_a_first_frame_that_never_caches() {
    let fake = Fake::default();
    let (mut h, cid, size) = extended_viewer(&fake);
    let (comp_bytes, real_bytes) = (160 * 120 * 4, size[0] * size[1] * 4);
    assert!(real_bytes > comp_bytes * 2, "Extended Viewer frames ({size:?}) are larger than the comp");
    // Room for a comp-sized frame but not a real one: the current frame can never be cached.
    let budget = (comp_bytes + real_bytes) / 2;
    h.state().frames.set_budget(budget);
    h.state().frames.clear();
    let start = h.state().now();
    h.state_mut().play(start);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while h.state().playback.starting && std::time::Instant::now() < deadline {
        assert_eq!(frame(&h, cid), 0, "it holds the first frame while it waits");
        step(&mut h);
    }
    let waited = h.state().now() - start;
    assert!(!h.state().playback.starting, "it gave up waiting");
    assert!((9.9..12.0).contains(&waited), "after about ten seconds ({waited:.2} s)");
    assert!(fake.0.lock().unwrap().0.is_none(), "no sound started and dropped");
    assert!(h.state().playback.playing && h.state().playback.audio_held, "it goes on as a stalled preview");
}

/// Adding markers while a preview sounds (Num*, one per beat) keeps the sound going (#1083): a
/// marker changes no pixels, so no cached frame is dropped and no frame has to be rendered again
/// (which would stop the sound until the frames ahead are cached).
#[test]
fn adding_markers_does_not_stop_the_sound() {
    let fake = Fake::default();
    let (app, cid) = setup(true, &fake);
    // Everything is cached before the preview starts, so the sound starts at once.
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 31.0).build_eframe(|_| app);
    h.run_steps(2);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let now = h.ctx.input(|i| i.time);
    h.state_mut().play(now);
    while h.state().audio.is_none() && std::time::Instant::now() < deadline {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(h.state().audio.is_some(), "the sound started");
    for beat in 0..6 {
        let t = 0.1 + 0.05 * beat as f64;
        h.state_mut().session.execute("comp.addMarker", json!({"comp": cid.0, "time": t, "comment": "beat"})).unwrap();
        h.step();
        let app = h.state();
        assert!(app.playback.playing && app.audio.is_some() && !app.playback.audio_held, "marker {beat}: still playing with sound");
    }
    assert_eq!(h.state().session.project.comp(cid).unwrap().markers.len(), 6);
}
