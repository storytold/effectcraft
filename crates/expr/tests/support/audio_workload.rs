//! Configurable synthetic projects for exercising the real expression engine and audio mixer.
use effectcraft_color::Label;
use effectcraft_keyframe::Value;
use effectcraft_project::{
    Comp, Expression, Footage, FootageKind, ItemId, ItemKind, LayerSource, Project,
    build::{self, Ids},
};
use effectcraft_render::{FootageSource, Image};
use effectcraft_time::{FrameRate, Tick};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

/// Counts decoded frames and reads; samples depend only on source time, never request boundaries.
#[derive(Default)]
pub struct Source {
    pub reads: AtomicUsize,
    pub frames: AtomicUsize,
}
impl FootageSource for Source {
    /// This procedural audio source intentionally supplies no video frames.
    fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
        None
    }
    /// Generate reproducible stereo samples and account for every source request.
    fn audio(&self, _: ItemId, _: &Footage, start: Tick, frames: usize, rate: u32) -> Option<Vec<f32>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.frames.fetch_add(frames, Ordering::Relaxed);
        let first = start.to_units_floor(rate as i64);
        Some(
            (0..frames)
                .flat_map(|i| {
                    let v = ((first + i as i64).rem_euclid(97) as f32 / 97.0 - 0.5) * 0.01;
                    [v, -v]
                })
                .collect(),
        )
    }
}

/// Build `layers` independent audio tracks wrapped in `depth` remapped precompositions.
/// All media is procedural; duration is deliberately longer than every benchmark run.
pub fn project(layers: usize, depth: usize, expression: Option<&str>) -> (Project, ItemId) {
    let mut p = Project::default();
    let duration = Tick::from_seconds_f64(600.0);
    let f = p.add_item(
        "synthetic.wav",
        Label::SeaFoam,
        None,
        ItemKind::Footage(Footage { kind: FootageKind::Audio, has_audio: true, duration, ..Default::default() }),
    );
    let mut c = Comp::new(16, 16, FrameRate::new(24, 1), duration);
    for _ in 0..layers {
        let mut l = build::layer(&mut p, &c, "audio", LayerSource::Footage { item: f }, (0, 0), None);
        let mut prop = Ids(&mut p.next_id).prop("timeRemap", "Time Remap", Value::Scalar(0.0));
        prop.keys =
            vec![effectcraft_keyframe::Keyframe::new(Tick::ZERO, Value::Scalar(0.25)), effectcraft_keyframe::Keyframe::new(duration, Value::Scalar(300.25))];
        prop.expr = expression.map(|text| Expression { text: text.into(), enabled: true });
        l.props.children.push(prop.into());
        c.layers.push(l);
    }
    let mut cid = p.add_item("Audio", Label::Sandstone, None, ItemKind::Comp(c.into()));
    for _ in 0..depth {
        let mut c = Comp::new(16, 16, FrameRate::new(24, 1), duration);
        let mut l = build::layer(&mut p, &c, "nested", LayerSource::Comp { item: cid }, (16, 16), None);
        let mut prop = Ids(&mut p.next_id).prop("timeRemap", "Time Remap", Value::Scalar(0.0));
        prop.keys =
            vec![effectcraft_keyframe::Keyframe::new(Tick::ZERO, Value::Scalar(0.0)), effectcraft_keyframe::Keyframe::new(duration, Value::Scalar(600.0))];
        l.props.children.push(prop.into());
        c.layers.push(l);
        cid = p.add_item("Nested", Label::Sandstone, None, ItemKind::Comp(c.into()));
    }
    (p, cid)
}
