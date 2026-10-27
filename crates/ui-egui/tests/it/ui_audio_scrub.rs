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

/// A preview with audio waits for frames like a silent one (#103): while the frames ahead aren't
/// cached, every frame shows as it renders, silently; the sound starts once the rest of the
/// preview is cached, and a frame that isn't cached stops it instead of being skipped.
#[test]
fn preview_with_audio_shows_every_frame_and_sounds_once_cached() {
    let fake = Fake::default();
    let (mut app, cid) = setup(true, &fake);
    // Hold rendering in the test: the cache cannot fill ahead of what the test releases, so the
    // silently shown frames and the moment the sound starts do not depend on the host's threads.
    let gate = GatedFrames::default();
    let renderer: Arc<dyn RemoteFrames> = Arc::new(gate.clone());
    app.frames.set_remote(Some(renderer));
    let ahead = (app.frames.parallelism() * 2).max(4);
    app.session.execute("comp.settings", json!({"comp": cid.0, "duration": (4 * ahead) as f64 / 30.0})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 30.0).build_eframe(|_| app);
    // Playback measures time on the app clock, never `ctx.input(|i| i.time)`; drive that clock
    // explicitly at one frame per step.
    h.state_mut().use_manual_clock();
    h.run_steps(2);
    gate.discard(&mut h);
    let frame = |h: &Harness<'_, EffectcraftApp>| {
        let app = h.state();
        app.session.project.comp(cid).unwrap().frame_rate.frame_at(app.session.time())
    };
    let now = h.state().now();
    h.state_mut().play(now);
    assert!(h.state().playback.audio_held && h.state().audio.is_none(), "nothing is cached: no sound yet");
    let mut last = frame(&h);
    let mut shown = 0;
    // Show frames while only those frames are cached: they play silently first.
    for _ in 0..4 {
        let target = last + 1;
        gate.render(&mut h, cid, &[target]);
        h.state_mut().clock_mut().advance(1.0 / 30.0);
        h.step();
        let f = frame(&h);
        // Accumulating 1/30 in f64 can leave the same frame up for one more step; it must never
        // jump over a frame.
        assert!(f == last || f == last + 1, "{last} → {f}: a frame was skipped");
        shown += usize::from(f != last);
        last = f;
    }
    // A frame that has not rendered holds the clock at the frame on screen; once it renders it is
    // shown, never jumped over. This exercises the production hold path, not a test-only shortcut.
    let waiting = last;
    h.state_mut().clock_mut().advance(1.0 / 30.0);
    h.step();
    assert_eq!(frame(&h), waiting, "an uncached frame holds the preview instead of being skipped");
    gate.render(&mut h, cid, &[waiting + 1]);
    // f64 clock accumulation need not put the next step exactly on the next frame boundary;
    // keep stepping until the held frame appears, and never allow it to jump past that frame.
    let mut showed_waiting = false;
    for _ in 0..4 {
        h.state_mut().clock_mut().advance(1.0 / 30.0);
        h.step();
        let f = frame(&h);
        assert!(f == waiting || f == waiting + 1, "{waiting} → {f}: the held frame was skipped");
        if f == waiting + 1 {
            showed_waiting = true;
            break;
        }
    }
    assert!(showed_waiting, "the waiting frame is shown once it renders");
    shown += 1;
    last = waiting + 1;
    assert!(shown > 0, "frames played silently first");
    assert!(h.state().audio.is_none() && h.state().playback.audio_held, "the rest of the range is not cached: the sound waits");
    // Cache everything ahead; only then may the sound start.
    let end = h.state().playback.plan.expect("a preview plan").end;
    let rest: Vec<i64> = ((last + 1)..=end).collect();
    gate.render(&mut h, cid, &rest);
    h.step();
    assert!(h.state().audio.is_some() && fake.0.lock().unwrap().0.is_some(), "the sound started once the frames ahead were cached");
    // A frame that isn't cached stops the sound rather than being skipped.
    gate.discard(&mut h);
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
    let (mut app, cid) = setup_long(true, &fake, 30.0);
    // Release only the frames this test chooses, and drive the app clock: rendering cannot race
    // the clock, so a frame is never skipped by a slow or loaded test host.
    let gate = GatedFrames::default();
    let renderer: Arc<dyn RemoteFrames> = Arc::new(gate.clone());
    app.frames.set_remote(Some(renderer));
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 30.0).build_eframe(|_| app);
    h.state_mut().use_manual_clock();
    h.run_steps(2);
    gate.discard(&mut h);
    let ahead = (h.state().frames.parallelism() * 2).max(4) as i64;
    let frame = |h: &Harness<'_, EffectcraftApp>| {
        let app = h.state();
        app.session.project.comp(cid).unwrap().frame_rate.frame_at(app.session.time())
    };
    let start = h.state().now();
    h.state_mut().play(start);
    assert!(h.state().playback.audio_held && h.state().audio.is_none(), "nothing is cached: no sound yet");
    let mut last = frame(&h);
    let mut shown = 0;
    // Two seconds of playback at one frame per clock step: keep the prefetch window cached, but
    // not the rest of the 30 s comp, so the sound has to start by the real-time rule.
    for step in 1..=60 {
        let target = last + 1;
        let window: Vec<i64> = (target..target + ahead).collect();
        gate.render(&mut h, cid, &window);
        // Set the clock to this step's exact time rather than adding 1/30 repeatedly: accumulated
        // f64 error can put one step just below a frame boundary and the next just above it, so
        // `floor(elapsed × fps)` would otherwise jump from frame N to N + 2 for a one-frame step.
        let now = h.state().now();
        let tick_time = start + step as f64 / 30.0;
        h.state_mut().clock_mut().advance(tick_time - now);
        h.step();
        if h.state().audio.is_some() {
            break;
        }
        let f = frame(&h);
        assert!(f == last || f == last + 1, "{last} → {f}: a frame was skipped");
        shown += usize::from(f != last);
        last = f;
    }
    assert!(h.state().audio.is_some() && fake.0.lock().unwrap().0.is_some(), "the sound started within 2 s, at frame {last} of 900");
    assert!(shown >= 3, "the sound starts only after the silent preview kept up for a moment");
}
