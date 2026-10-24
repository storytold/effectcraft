//! Audio scrubbing: Ctrl/Cmd-dragging the current time plays one frame of the comp's mix at each
//! new frame (each snippet replacing what is still queued); holding still repeats nothing; silent
//! comps open no output; an idle scrub closes it. Previews with audio wait for frames.

use std::sync::{Arc, Mutex};

use effectcraft_engine::Session;
use effectcraft_engine::project::ItemId;
use effectcraft_engine::time::Tick;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::audio::{AudioDevice, AudioFeed};
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

/// A preview with audio waits for frames like a silent one (#103): while the frames ahead aren't
/// cached, every frame shows as it renders, silently; the sound starts once the rest of the
/// preview is cached, and a frame that isn't cached stops it instead of being skipped.
#[test]
fn preview_with_audio_shows_every_frame_and_sounds_once_cached() {
    let fake = Fake::default();
    let (mut app, cid) = setup(true, &fake);
    // Playback requests at most twice as many frames ahead per step as there are render threads.
    // Four times that many frames take at least four steps to cache, so frames are shown silently
    // first however fast the renderer is (with a fixed 2 s comp, a machine with many cores cached
    // all of it before the first step and the sound started at once).
    let ahead = (app.frames.parallelism() * 2).max(4);
    app.session.execute("comp.settings", json!({"comp": cid.0, "duration": (4 * ahead) as f64 / 30.0})).unwrap();
    // A little under one frame of input time per step.
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 31.0).build_eframe(|_| app);
    h.run_steps(2);
    let frame = |h: &Harness<'_, EffectcraftApp>| {
        let app = h.state();
        app.session.project.comp(cid).unwrap().frame_rate.frame_at(app.session.time())
    };
    let now = h.ctx.input(|i| i.time);
    h.state_mut().play(now);
    assert!(h.state().playback.audio_held && h.state().audio.is_none(), "nothing is cached: no sound yet");
    let mut last = frame(&h);
    let mut shown = 0;
    // Wait on the clock, not a step count: under a loaded machine (parallel test binaries) the
    // background cache can take far longer than 2000 short steps to get ahead.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while std::time::Instant::now() < deadline {
        h.step();
        if h.state().audio.is_some() {
            break;
        }
        let f = frame(&h);
        assert!(f == last || f == last + 1 || f < last, "{last} → {f}: a frame was skipped");
        shown += usize::from(f != last);
        last = f;
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(h.state().audio.is_some() && fake.0.lock().unwrap().0.is_some(), "the sound started once the frames ahead were cached");
    assert!(shown > 0, "frames played silently first");
    // A frame that isn't cached stops the sound rather than being skipped. (Frames render fast
    // enough here to be cached again before the check: keep them out of the cache.)
    h.state().frames.set_budget(0);
    h.state().frames.clear();
    h.step();
    assert!(h.state().audio.is_none() && h.state().playback.audio_held && h.state().playback.playing);
}

/// A preview whose frames render in real time sounds without waiting for the whole work area to
/// be cached (#208): once the silent preview has kept up for a moment with the frames ahead
/// cached, the sound starts, still without skipping a frame before it.
#[test]
fn preview_with_audio_sounds_once_rendering_keeps_up() {
    let fake = Fake::default();
    let (app, cid) = setup_long(true, &fake, 30.0);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 30.0).build_eframe(|_| app);
    h.run_steps(2);
    let frame = |h: &Harness<'_, EffectcraftApp>| {
        let app = h.state();
        app.session.project.comp(cid).unwrap().frame_rate.frame_at(app.session.time())
    };
    let now = h.ctx.input(|i| i.time);
    h.state_mut().play(now);
    assert!(h.state().playback.audio_held && h.state().audio.is_none(), "nothing is cached: no sound yet");
    let mut last = frame(&h);
    // Two seconds of playback, each step giving the frame workers time to keep up.
    for _ in 0..60 {
        h.step();
        if h.state().audio.is_some() {
            break;
        }
        let f = frame(&h);
        assert!(f == last || f == last + 1, "{last} → {f}: a frame was skipped");
        last = f;
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(h.state().audio.is_some() && fake.0.lock().unwrap().0.is_some(), "the sound started within 2 s, at frame {last} of 900");
}
