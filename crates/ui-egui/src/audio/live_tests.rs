//! #2104: exercise the real mixer and queue without relying on thread timing or an audio device.

use super::{AudioDevice, AudioFeed, AudioPlayback, Feeder, PlaySpan, PreviewRate, feed_block};
use crate::frames::RenderSource;
use effectcraft_engine::color::Label;
use effectcraft_engine::keyframe::Value;
use effectcraft_engine::project::{Comp, Expression, Footage, FootageKind, ItemId, ItemKind, Layer, LayerSource, Project, Property, build};
use effectcraft_engine::render::{EvalCtx, ExprHost, FootageSource, LayerCache};
use effectcraft_engine::time::{FrameRate, Tick};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

struct Device;
impl AudioDevice for Device {
    fn sample_rate(&self) -> u32 {
        48000
    }
    fn start(&mut self, _: Arc<AudioFeed>) -> Result<(), String> {
        Ok(())
    }
    fn stop(&mut self) {}
}

/// A ramp identifies the exact source sample, exposing skips and repeats after an edit.
struct Ramp;
impl FootageSource for Ramp {
    fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<effectcraft_engine::raster::Image>> {
        None
    }
    fn audio(&self, _: ItemId, _: &Footage, t: Tick, frames: usize, rate: u32) -> Option<Vec<f32>> {
        let first = t.to_units_floor(i64::from(rate));
        Some((0..frames).flat_map(|i| [(first + i as i64) as f32 / 128.0; 2]).collect())
    }
}

/// Construct a paused feeder whose short nonzero span makes every source sample observable.
fn playback(footage: Arc<dyn FootageSource>) -> AudioPlayback {
    let mut project = Project::default();
    let f = Footage { path: "ramp.wav".into(), kind: FootageKind::Audio, has_audio: true, ..Default::default() };
    let item = project.add_item("Ramp", Label::SeaFoam, None, ItemKind::Footage(f));
    let mut comp = Comp::new(64, 36, FrameRate::new(30, 1), Tick::from_seconds_f64(1.0));
    let layer = build::layer(&mut project, &comp, "Ramp", LayerSource::Footage { item }, (0, 0), None);
    comp.layers = vec![layer].into();
    let cid = project.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    let src = RenderSource {
        project: Arc::new(project),
        footage,
        expr: None,
        layer_cache: Arc::new(LayerCache::default()),
        gpu: None,
        gpu_display: false,
        disk: None,
    };
    AudioPlayback {
        feed: Arc::new(AudioFeed::default()),
        device: Box::new(Device),
        stop: Arc::new(AtomicBool::new(false)),
        thread: None,
        state: Arc::new(Mutex::new(Feeder { src, cursor: 24, generation: 0 })),
        comp: cid,
        mix: 48000,
        rate: 48000,
        span: PlaySpan { start: 24, wa: 16, wb: 64, looping: false },
        timeline: PreviewRate::new(48000, 1.0, Tick::ZERO).unwrap(),
    }
}

/// Run exactly one production feeder block, without a background refill racing assertions.
fn mix(p: &AudioPlayback) {
    feed_block(&p.state, p.comp, &p.feed, &p.span, p.timeline, p.mix);
}

/// Copy the active project and apply a layer edit through the same snapshot boundary as the UI.
fn edit(p: &AudioPlayback, update: impl FnOnce(&mut Layer)) -> Arc<Project> {
    let mut project = p.project().unwrap();
    update(&mut Arc::make_mut(&mut project).comp_mut(p.comp).unwrap().layers[0]);
    project
}

/// Levels edits retain sample position across the loop boundary and the exclusive non-looping end.
#[test]
fn project_replacement_resumes_at_the_first_unconsumed_sample() {
    for looping in [false, true] {
        let mut p = playback(Arc::new(Ramp));
        p.span.looping = looping;
        mix(&p);
        let mut out = [0.0; 64];
        p.feed.pull(&mut out);
        assert_eq!(out[0], 24.0 / 128.0);
        assert_eq!(p.feed.consumed(), 32);
        let updated = edit(&p, |l| l.props.sub_mut("audio").unwrap().get_mut("levels").unwrap().value = Value::Vec2([-6.020599913279624; 2]));
        p.follow_project(updated);
        assert_eq!(p.feed.queued_frames(), 0);
        assert_eq!(p.state.lock().unwrap().cursor, 56);
        mix(&p);
        let mut tail = [0.0; 16];
        p.feed.pull(&mut tail);
        assert!((tail[0] - 56.0 / 256.0).abs() < 1e-6);
        assert!((tail[14] - 63.0 / 256.0).abs() < 1e-6);
        p.set_project(p.project().unwrap());
        assert_eq!(p.state.lock().unwrap().cursor, if looping { 16 } else { 64 });
        mix(&p);
        if looping {
            let mut first = [0.0; 2];
            p.feed.pull(&mut first);
            assert!((first[0] - 16.0 / 256.0).abs() < 1e-6, "resume at the work-area start");
        } else {
            assert!(p.finished(), "an edit after the last sample must not replay it");
            assert_eq!(p.feed.queued_frames(), 0);
        }
    }
}

struct Gated {
    entered: mpsc::Sender<()>,
    resume: Mutex<mpsc::Receiver<()>>,
}
impl FootageSource for Gated {
    fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<effectcraft_engine::raster::Image>> {
        None
    }
    fn audio(&self, _: ItemId, _: &Footage, _: Tick, frames: usize, _: u32) -> Option<Vec<f32>> {
        self.entered.send(()).unwrap();
        self.resume.lock().unwrap().recv_timeout(Duration::from_secs(10)).unwrap();
        Some(vec![0.5; frames * 2])
    }
}

/// A slow mix must not block edits or publish samples from the superseded project generation.
#[test]
fn a_block_in_flight_cannot_publish_after_a_mute() {
    let (entered, waiting) = mpsc::channel();
    let (resume, gate) = mpsc::channel();
    let mut p = playback(Arc::new(Gated { entered, resume: Mutex::new(gate) }));
    let (state, feed, span, comp, rate) = (p.state.clone(), p.feed.clone(), p.span, p.comp, p.rate);
    let worker = std::thread::spawn(move || feed_block(&state, comp, &feed, &span, PreviewRate::new(rate, 1.0, Tick::ZERO).unwrap(), rate));
    waiting.recv_timeout(Duration::from_secs(10)).unwrap();
    // The edit completes while the mixer is blocked, without waiting on its work.
    p.follow_project(edit(&p, |l| l.switches.audio = false));
    resume.send(()).unwrap();
    worker.join().unwrap();
    assert_eq!(p.feed.queued_frames(), 0, "the old generation cannot publish");
    assert_eq!(p.state.lock().unwrap().cursor, 24, "nor advance the replacement cursor");
    mix(&p);
    let mut out = [1.0; 16];
    p.feed.pull(&mut out);
    assert_eq!(out, [0.0; 16]);
}

/// Models a levels expression that reads a video property through the renderer's expression API.
struct OpacityLevels;
impl ExprHost for OpacityLevels {
    fn eval(&self, _: &EvalCtx, layer: &Layer, _: &Property, _: &Value) -> Result<Value, String> {
        let opacity = layer.transform().unwrap().get("opacity").unwrap().value.as_f64();
        Ok(Value::Vec2([if opacity == 0.0 { -96.0 } else { 0.0 }; 2]))
    }
}

/// Video properties are irrelevant to a plain mix, but may be dependencies of audio expressions.
#[test]
fn video_edits_keep_the_queue_unless_audio_expressions_can_read_them() {
    for expression in [false, true] {
        let mut p = playback(Arc::new(Ramp));
        if expression {
            let updated = edit(&p, |l| {
                l.props.sub_mut("audio").unwrap().get_mut("levels").unwrap().expr =
                    Some(Expression { text: "[transform.opacity, transform.opacity]".into(), enabled: true });
            });
            let mut state = p.state.lock().unwrap();
            state.src.project = updated;
            state.src.expr = Some(Arc::new(OpacityLevels));
        }
        mix(&p);
        assert_eq!(p.feed.queued_frames(), 40);
        let updated = edit(&p, |l| l.props.sub_mut("transform").unwrap().get_mut("opacity").unwrap().value = Value::Scalar(0.0));
        p.follow_project(updated);
        if expression {
            assert_eq!(p.feed.queued_frames(), 0, "the expression can read the edited video property");
            mix(&p);
            let mut out = [1.0; 16];
            p.feed.pull(&mut out);
            assert_eq!(out, [0.0; 16]);
        } else {
            assert_eq!(p.feed.queued_frames(), 40, "an unrelated edit cannot interrupt audio");
        }
    }
}

/// Reused nested comps cost linear visits while edits to their descendants remain detectable.
#[test]
fn shared_nested_comps_are_compared_once_per_depth() {
    let mut project = Project::default();
    let mut child = None;
    for _ in 0..10 {
        let mut comp = Comp::new(64, 36, FrameRate::new(30, 1), Tick::from_seconds_f64(1.0));
        if let Some(item) = child {
            for _ in 0..10 {
                let mut layer = build::layer(&mut project, &comp, "Nested", LayerSource::Comp { item }, (64, 36), None);
                layer.switches.audio = false;
                comp.layers.push(layer);
            }
        }
        child = Some(project.add_item("Nested", Label::Sandstone, None, ItemKind::Comp(comp.into())));
    }
    let root = child.unwrap();
    let mut edited = project.clone();
    edited.comp_mut(root).unwrap().layers[0].name = "Renamed".into();
    let mut seen = std::collections::HashSet::new();
    assert!(!super::audio_changed_inner(&project, &edited, root, &mut seen));
    assert_eq!(seen.len(), 10, "compare the ten comps, not a billion paths through them");
    let leaf = *seen.iter().find(|(_, depth)| *depth == 9).map(|(item, _)| item).unwrap();
    let mut layer = project.comp(root).unwrap().layers[0].clone();
    layer.source = LayerSource::Null;
    edited.comp_mut(leaf).unwrap().layers.push(layer);
    assert!(super::audio_changed(&project, &edited, root), "memoization must still find a nested edit");
}
