//! "Enable Time Remapping" on an audio layer changes what is heard (#3297): the keys it adds start
//! as an identity map and any edit to them (here, a faster end key) plays the sound at that speed,
//! the same way Time Stretch does, through the layer's real command path.
use effectcraft_engine::Session;
use effectcraft_engine::render::audio::mix_comp;
use effectcraft_project::{Footage, FootageKind, ItemId, ItemKind};
use effectcraft_render::{FootageSource, Image};
use effectcraft_time::{FrameRate, Tick};
use serde_json::json;
use std::sync::Arc;

const RATE: u32 = 8000;

/// Ten seconds of audio whose left channel is its own source time, so a sample says where it came from.
struct Ramp;
impl FootageSource for Ramp {
    fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
        None
    }
    fn audio(&self, _: ItemId, _: &Footage, start: Tick, frames: usize, rate: u32) -> Option<Vec<f32>> {
        let t0 = start.seconds();
        Some((0..frames).map(|i| (t0 + i as f64 / rate as f64).clamp(0.0, 10.0) as f32).flat_map(|t| [t, -t]).collect())
    }
}

fn session_with_audio_layer() -> (Session, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 64, "height": 64, "frameRate": 25, "duration": 4})).unwrap();
    let footage = Footage {
        path: "voice.wav".into(),
        kind: FootageKind::Audio,
        pixel_aspect: 1.0,
        frame_rate: FrameRate::new(25, 1),
        duration: Tick::from_seconds_f64(10.0),
        has_audio: true,
        ..Default::default()
    };
    Arc::make_mut(&mut s.project).add_item("voice.wav", effectcraft_color::Label::SeaFoam, None, ItemKind::Footage(footage));
    let layer = s.execute("layer.addItem", json!({"item": "voice.wav"})).unwrap()["layer"].as_u64().unwrap();
    // The layer plays the first two seconds of the source.
    s.execute("layer.timing", json!({"layers": [layer], "out": 2.0})).unwrap();
    s.execute("layer.select", json!({"layers": [layer]})).unwrap();
    (s, layer)
}

/// The left sample heard at comp time `t`.
fn heard(s: &Session, t: f64) -> f32 {
    let cid = s.active_comp_id().unwrap();
    mix_comp(&s.project, &Ramp, None, cid, Tick::from_seconds_f64(t), 1, RATE).unwrap()[0]
}

#[test]
fn enable_time_remapping_then_editing_its_keys_changes_the_audio() {
    let (mut s, layer) = session_with_audio_layer();
    assert!((heard(&s, 0.5) - 0.5).abs() < 1e-3, "unremapped: comp time is source time");

    // Enabling alone is an identity map: the sound is unchanged.
    s.execute("layer.enableTimeRemap", json!({})).unwrap();
    assert!((heard(&s, 0.5) - 0.5).abs() < 1e-3, "identity remap: {}", heard(&s, 0.5));

    // Move the end key so the two seconds play four seconds of source: twice as fast.
    s.execute("keys.set", json!({"layer": layer, "path": "timeRemap", "time": 2.0, "value": 4.0})).unwrap();
    for t in [0.25, 0.5, 1.0, 1.5] {
        let expected = t as f32 * 2.0;
        assert!((heard(&s, t) - expected).abs() < 2e-3, "at {t} s: heard source {} expected {expected}", heard(&s, t));
    }

    // Slow it down instead: half speed.
    s.execute("keys.set", json!({"layer": layer, "path": "timeRemap", "time": 2.0, "value": 1.0})).unwrap();
    for t in [0.5, 1.0, 1.5] {
        let expected = t as f32 * 0.5;
        assert!((heard(&s, t) - expected).abs() < 2e-3, "at {t} s: heard source {} expected {expected}", heard(&s, t));
    }

    // Turning it off again plays the sound as it was.
    s.execute("layer.enableTimeRemap", json!({})).unwrap();
    assert!((heard(&s, 1.0) - 1.0).abs() < 1e-3);
}
