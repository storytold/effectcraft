//! Audio scrubbing: Ctrl/Cmd-dragging the current time plays one frame of the comp's mix at each
//! new frame (each snippet replacing what is still queued); holding still repeats nothing; silent
//! comps open no output; an idle scrub closes it. Previews with audio start their sound at once.

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

/// A preview with audio starts its sound with the first frame (#947): the opening is heard
/// instead of being skipped while frames render, and no frame is skipped before it. A frame that
/// isn't rendered then doesn't stop the sound: the preview slows it down, and the clock with it,
/// until the picture catches up.
#[test]
fn preview_with_audio_starts_the_sound_at_once() {
    let fake = Fake::default();
    let (mut app, cid) = setup(true, &fake);
    // Hold rendering in the test: a frame is cached only when the test releases it.
    let gate = GatedFrames::default();
    let renderer: Arc<dyn RemoteFrames> = Arc::new(gate.clone());
    app.frames.set_remote(Some(renderer));
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 30.0).build_eframe(|_| app);
    h.state_mut().use_manual_clock();
    h.run_steps(2);
    gate.discard(&mut h);
    let frame = |h: &Harness<'_, EffectcraftApp>| {
        let app = h.state();
        app.session.project.comp(cid).unwrap().frame_rate.frame_at(app.session.time())
    };
    let now = h.state().now();
    h.state_mut().play(now);
    assert!(h.state().audio.is_some(), "the sound starts with the first frame");
    assert!(fake.0.lock().unwrap().0.is_some(), "the output was given the feed");
    // The fake output never pulls, so take one frame's worth of samples each step: the audio
    // clock then moves one comp frame per step, as a device playing in real time would.
    let mut one_frame = vec![0.0f32; 3200];
    let mut last = frame(&h);
    let mut moved = 0;
    for _ in 0..60 {
        let target = last + 1;
        gate.render(&mut h, cid, &[target]);
        if let Some(a) = &h.state().audio {
            a.feed.pull(&mut one_frame);
        }
        h.step();
        let f = frame(&h);
        assert!(f == last || f == last + 1, "{last} → {f}: a frame was skipped");
        if f != last {
            moved += 1;
        }
        last = f;
        if moved >= 3 {
            break;
        }
    }
    assert!(moved >= 3, "the preview moved with the sound");
    // A frame that isn't rendered holds the preview and slows the sound instead of stopping it.
    let waiting = last;
    h.state().frames.set_budget(0);
    h.state().frames.clear();
    gate.discard(&mut h);
    if let Some(a) = &h.state().audio {
        a.feed.pull(&mut one_frame);
    }
    h.step();
    assert_eq!(frame(&h), waiting, "an uncached frame holds the preview instead of being skipped");
    assert!(h.state().playback.waiting, "the picture waits for the frame");
    let audio = h.state().audio.as_ref().expect("the sound keeps playing while the picture waits");
    // Top the queue up: the feeder mixes in real time, and the test reads far faster than that.
    audio.feed.clear();
    audio.feed.push(&vec![0.5f32; 200_000]);
    audio.feed.pull(&mut one_frame);
    assert!(one_frame.iter().any(|v| v.abs() > 0.01), "the sound is still there, not silence");
    for _ in 0..40 {
        audio.feed.pull(&mut one_frame);
    }
    assert!(audio.feed.speed() < 0.9, "the sound slowed down: {}", audio.feed.speed());
}

/// A long preview's sound starts at once too — it no longer waits for the work area to be cached
/// (#208) — and keeps playing while frames render in time.
#[test]
fn a_long_preview_starts_its_sound_at_once_and_keeps_it() {
    let fake = Fake::default();
    let (mut app, cid) = setup_long(true, &fake, 30.0);
    let gate = GatedFrames::default();
    let renderer: Arc<dyn RemoteFrames> = Arc::new(gate.clone());
    app.frames.set_remote(Some(renderer));
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 30.0).build_eframe(|_| app);
    h.state_mut().use_manual_clock();
    h.run_steps(2);
    gate.discard(&mut h);
    let frame = |h: &Harness<'_, EffectcraftApp>| {
        let app = h.state();
        app.session.project.comp(cid).unwrap().frame_rate.frame_at(app.session.time())
    };
    let start = h.state().now();
    h.state_mut().play(start);
    assert!(h.state().audio.is_some() && fake.0.lock().unwrap().0.is_some(), "the sound starts at once");
    let mut one_frame = vec![0.0f32; 3200];
    let mut last = frame(&h);
    for _ in 0..60 {
        let target = last + 1;
        let window: Vec<i64> = (target..target + 4).collect();
        gate.render(&mut h, cid, &window);
        if let Some(a) = &h.state().audio {
            a.feed.pull(&mut one_frame);
        }
        h.step();
        last = frame(&h);
        assert!(h.state().audio.is_some() && h.state().playback.playing, "the sound kept playing at frame {last}");
    }
}
