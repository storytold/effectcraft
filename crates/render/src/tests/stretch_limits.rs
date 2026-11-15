//! A project file whose layers carry a time stretch outside ±1 % to ±9900 % (1e-300 %, 0,
//! 1e300 %…) opens with the stretch brought into that range, and a frame of it draws and its audio
//! mixes with bounded source reads. Before, a stretch of 1e-300 % asked the footage for about
//! `usize::MAX` samples and the mix overflowed or tried a huge allocation, aborting the app (#3247).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use effectcraft_color::Label;
use effectcraft_project::{Footage, FootageKind, ItemId, ItemKind, LayerSource, Project, build, limits};
use effectcraft_time::Tick;

use super::{setup, solid};
use crate::{FootageSource, Image, audio::mix_comp, render_frame};

/// Silent audio footage that records the longest read asked of it and refuses a read past a
/// thousand seconds (so a runaway read can't take the machine's memory: the mix then fills the
/// read with silence itself).
#[derive(Default)]
struct Probe {
    longest: AtomicUsize,
}

impl FootageSource for Probe {
    fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
        None
    }
    fn audio(&self, _: ItemId, _: &Footage, _: Tick, frames: usize, rate: u32) -> Option<Vec<f32>> {
        self.longest.fetch_max(frames, Ordering::Relaxed);
        (frames <= rate as usize * 1000).then(|| vec![0.0; frames * 2])
    }
}

/// The project file of a comp holding a red solid and a sound, both with their stretch set to
/// `stretch` in the file text.
fn file_with_stretch(stretch: f64) -> (String, ItemId) {
    let (mut p, cid, comp) = setup();
    let red = solid(&mut p, &comp, [1.0, 0.0, 0.0], 200, 100);
    let f = Footage {
        path: "tone.wav".into(),
        kind: FootageKind::Audio,
        has_audio: true,
        frame_rate: comp.frame_rate,
        duration: Tick::from_seconds_f64(10.0),
        ..Default::default()
    };
    let fid = p.add_item("tone.wav", Label::SeaFoam, None, ItemKind::Footage(f));
    let sound = build::layer(&mut p, &comp, "sound", LayerSource::Footage { item: fid }, (0, 0), None);
    let c = p.comp_mut(cid).unwrap();
    c.layers.push(red);
    c.layers.push(sound);
    // Both layers are at 100 %; `{:e}` writes a JSON number for any finite value, and NaN is
    // written as null, as serializing the project writes it.
    let text = p.to_json();
    assert_eq!(text.matches("\"stretch\": 100.0").count(), 2);
    let value = if stretch.is_nan() { "null".to_string() } else { format!("{stretch:e}") };
    (text.replace("\"stretch\": 100.0", &format!("\"stretch\": {value}")), cid)
}

#[test]
fn a_file_with_an_extreme_stretch_opens_in_range_draws_and_mixes_with_bounded_reads() {
    for (stored, opened) in
        [(1e-300, 1.0), (-1e-300, -1.0), (0.0, 100.0), (1e300, limits::MAX_STRETCH), (-1e300, -limits::MAX_STRETCH), (0.5, 1.0), (f64::NAN, 100.0)]
    {
        let (text, cid) = file_with_stretch(stored);
        let (p, notes) = Project::from_json_repaired(&text).unwrap();
        assert!(notes.iter().filter(|n| n.contains("time stretch")).count() == 2, "{stored}: {notes:?}");
        let c = p.comp(cid).unwrap();
        assert!(c.layers.iter().all(|l| l.stretch == opened), "{stored}: {:?}", c.layers.iter().map(|l| l.stretch).collect::<Vec<_>>());
        // A frame draws: the solid has no keys and keeps its in and out points, so it is red.
        let img = render_frame(&p, cid, Tick::ZERO, 1.0);
        assert_eq!(img.get(10, 10), [1.0, 0.0, 0.0, 1.0], "{stored}");
        // The audio mixes, reading at most 100 source samples per comp sample (a stretch of 1 %).
        let (rate, n) = (8000, 4000);
        let probe = Probe::default();
        let mixed = mix_comp(&p, &probe, None, cid, Tick::from_seconds_f64(0.5), n, rate);
        assert_eq!(mixed.len(), n * 2, "{stored}");
        assert!(probe.longest.load(Ordering::Relaxed) <= 100 * n + 1, "{stored}: a read of {} samples", probe.longest.load(Ordering::Relaxed));
    }
}

/// A stretch set in memory (not through a file, so not repaired) mixes as the clamped stretch the
/// layer's time uses: before, 1e-300 % read about `usize::MAX` source samples and overflowed.
#[test]
fn an_in_memory_extreme_stretch_mixes_with_bounded_reads() {
    for stretch in [1e-300, 0.0, f64::NAN, 1e300] {
        let (text, cid) = file_with_stretch(100.0);
        let (mut p, _) = Project::from_json_repaired(&text).unwrap();
        p.comp_mut(cid).unwrap().layers.iter_mut().for_each(|l| l.stretch = stretch);
        let (rate, n) = (8000, 4000);
        let probe = Probe::default();
        let mixed = mix_comp(&p, &probe, None, cid, Tick::from_seconds_f64(0.5), n, rate);
        assert_eq!(mixed.len(), n * 2, "{stretch}");
        assert!(probe.longest.load(Ordering::Relaxed) <= 100 * n + 1, "{stretch}: a read of {} samples", probe.longest.load(Ordering::Relaxed));
    }
}
