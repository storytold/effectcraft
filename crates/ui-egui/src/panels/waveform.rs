//! Audio waveforms for the Timeline's Audio > Waveform property (revealed with `LL`).
//!
//! Each footage item's audio is summarised once into min/max peaks
//! (`effectcraft_render::audio::footage_peaks`, [`BINS_PER_SEC`] bins per second) on a
//! background thread; the timeline draws from that summary at any zoom.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use effectcraft_engine::project::{ItemId, ItemKind, Layer, LayerSource, Project, Property};
use effectcraft_engine::render::audio::db_to_gain;
use effectcraft_engine::time::Tick;
use egui::{Color32, Painter, Rect, Stroke, pos2};

use super::timeline::TMap;
use crate::EffectcraftApp;

/// Peak-summary resolution.
pub const BINS_PER_SEC: u32 = 200;

/// A footage item's peak summary: `[min L, max L, min R, max R]` per bin.
pub struct Summary {
    pub bins_per_sec: u32,
    pub peaks: Vec<[f32; 4]>,
}

/// Summaries by footage item (`None` while computing).
#[derive(Default)]
pub struct Cache {
    map: Arc<Mutex<HashMap<u64, Option<Arc<Summary>>>>>,
}

/// The footage item whose audio a layer plays, when it has audio.
pub fn audio_item(project: &Project, l: &Layer) -> Option<ItemId> {
    let LayerSource::Footage { item } = &l.source else { return None };
    match &project.item(*item)?.kind {
        ItemKind::Footage(f) if f.has_audio && !f.missing => Some(*item),
        _ => None,
    }
}

/// The summary for `item`, starting its computation on first request.
pub fn summary(app: &EffectcraftApp, ctx: &egui::Context, item: ItemId) -> Option<Arc<Summary>> {
    let mut m = app.waveforms.map.lock().ok()?;
    if let Some(s) = m.get(&item.0) {
        return s.clone();
    }
    let ItemKind::Footage(f) = &app.session.project.item(item)?.kind else { return None };
    m.insert(item.0, None);
    let (f, footage, map, ctx) = (f.clone(), app.session.footage.clone(), app.waveforms.map.clone(), ctx.clone());
    // Settings ▸ Disk ▸ Database and Cache Folder: summaries are kept on disk between sessions.
    let cache = app.session.media_cache_folder();
    std::thread::Builder::new()
        .name("ec-waveform".into())
        .spawn(move || {
            use effectcraft_engine::media_cache;
            let file = cache.filter(|_| !f.missing).and_then(|d| media_cache::peaks_path(&d, &f.path, BINS_PER_SEC));
            let peaks = match file.as_deref().and_then(media_cache::load_peaks) {
                Some(p) => p,
                None => {
                    let p = effectcraft_engine::render::audio::footage_peaks(footage.as_ref(), item, &f, f.duration, BINS_PER_SEC);
                    if let Some(file) = &file
                        && let Err(e) = media_cache::store_peaks(file, &p)
                    {
                        log::warn!("media cache: {}: {e}", file.display());
                    }
                    p
                }
            };
            if let Ok(mut m) = map.lock() {
                m.insert(item.0, Some(Arc::new(Summary { bins_per_sec: BINS_PER_SEC, peaks })));
            }
            ctx.request_repaint();
        })
        .ok()?;
    None
}

/// Left/right gain of `layer`'s Audio Levels (`levels`, dB) at comp time `t`; unity without the
/// property. Keyframes are in layer time, so they are read at the layer time of `t` (start
/// offset and stretch), as the mix reads them. A reversed layer (negative stretch) is still
/// drawn, though the mix leaves it silent.
fn level_gains(layer: &Layer, levels: Option<&Property>, t: Tick) -> [f32; 2] {
    match levels {
        Some(p) => {
            let [l, r] = p.value_at(layer.layer_time(t)).as_vec2();
            [db_to_gain(l), db_to_gain(r)]
        }
        None => [1.0, 1.0],
    }
}

/// Draw `layer`'s waveform in `rect` (left channel above right), columns mapped through the
/// timeline's time map and the layer's time mapping, limited to its [in, out) span.
pub(crate) fn draw(p: &Painter, rect: Rect, tm: &TMap, layer: &Layer, s: &Summary, color: Color32) {
    let mid = [rect.min.y + rect.height() * 0.25, rect.min.y + rect.height() * 0.75];
    let half = rect.height() * 0.25 - 1.0;
    let x0 = tm.x(layer.in_point.seconds()).max(rect.min.x).floor() as i32;
    let x1 = tm.x(layer.out_point.seconds()).min(rect.max.x).ceil() as i32;
    let n = s.peaks.len() as i64;
    let levels = layer.props.sub("audio").and_then(|g| g.get("levels"));
    for c in 0..2 {
        p.line_segment([pos2(x0 as f32, mid[c]), pos2(x1 as f32, mid[c])], Stroke::new(1.0, color.gamma_multiply(0.5)));
    }
    for x in x0..x1 {
        let (ta, tb) = (tm.t(x as f32), tm.t(x as f32 + 1.0));
        let src = |t: f64| layer.layer_time(Tick::from_seconds_f64(t)).seconds() * s.bins_per_sec as f64;
        let (ba, bb) = (src(ta).floor() as i64, src(tb).ceil() as i64);
        let (ba, bb) = (ba.min(bb).clamp(0, n), ba.max(bb).max(ba + 1).clamp(0, n));
        if ba >= bb {
            continue;
        }
        let mut agg = [0.0f32; 4];
        for b in &s.peaks[ba as usize..bb as usize] {
            agg[0] = agg[0].min(b[0]);
            agg[1] = agg[1].max(b[1]);
            agg[2] = agg[2].min(b[2]);
            agg[3] = agg[3].max(b[3]);
        }
        let gain = level_gains(layer, levels, Tick::from_seconds_f64((ta + tb) * 0.5));
        for c in 0..2 {
            let (lo, hi) = ((agg[c * 2] * gain[c]).clamp(-1.0, 1.0), (agg[c * 2 + 1] * gain[c]).clamp(-1.0, 1.0));
            let (ya, yb) = (mid[c] - hi * half, mid[c] - lo * half);
            p.line_segment([pos2(x as f32 + 0.5, ya), pos2(x as f32 + 0.5, yb.max(ya + 1.0))], Stroke::new(1.0, color));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_engine::color::Label;
    use effectcraft_engine::project::{Comp, Footage, FootageKind, Keyframe, Value, build};
    use effectcraft_engine::render::{FootageSource, Image, audio::mix_comp};
    use effectcraft_engine::time::FrameRate;

    #[test]
    fn waveform_follows_audio_levels() {
        let p = Property::new(1, "levels", "Audio Levels", Value::Vec2([-6.0206, -96.0]));
        let mut project = Project::default();
        let comp = Comp::new(64, 64, FrameRate::new(25, 1), Tick::from_seconds_f64(2.0));
        let l = build::layer(&mut project, &comp, "a", LayerSource::Null, (0, 0), None);
        let g = level_gains(&l, Some(&p), Tick::ZERO);
        assert!((g[0] - 0.5).abs() < 1e-3, "{}", g[0]);
        assert_eq!(g[1], 0.0);
        assert_eq!(level_gains(&l, None, Tick::ZERO), [1.0, 1.0]);
    }

    /// Constant 0.5 audio on both channels, at any source time.
    struct Half;
    impl FootageSource for Half {
        fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
            None
        }
        fn audio(&self, _: ItemId, _: &Footage, _: Tick, frames: usize, _: u32) -> Option<Vec<f32>> {
            Some(vec![0.5; frames * 2])
        }
    }

    /// #3224: keyframed Audio Levels scale the drawn waveform at the layer time the mix reads
    /// them, so on a layer that starts later (or is stretched) a column's gain is the gain the
    /// rendered audio has at that comp time.
    #[test]
    fn keyed_levels_scale_the_waveform_as_the_mix_does() {
        let mut p = Project::default();
        let f = Footage {
            path: "x.wav".into(),
            kind: FootageKind::Audio,
            has_audio: true,
            frame_rate: FrameRate::new(25, 1),
            duration: Tick::from_seconds_f64(10.0),
            ..Default::default()
        };
        let fid = p.add_item("x.wav", Label::SeaFoam, None, ItemKind::Footage(f));
        let base = Comp::new(64, 64, FrameRate::new(25, 1), Tick::from_seconds_f64(10.0));
        let mut l = build::layer(&mut p, &base, "a", LayerSource::Footage { item: fid }, (0, 0), None);
        // -6 dB at layer time 0 s, 0 dB at layer time 1 s.
        let Some(levels) = l.props.sub_mut("audio").and_then(|g| g.get_mut("levels")) else { panic!("no Audio Levels") };
        levels.keys = vec![Keyframe::new(Tick::ZERO, Value::Vec2([-6.0206, -6.0206])), Keyframe::new(Tick::from_seconds_f64(1.0), Value::Vec2([0.0, 0.0]))];
        for stretch in [100.0, 200.0] {
            let mut l = l.clone();
            l.start_time = Tick::from_seconds_f64(2.0);
            l.stretch = stretch;
            l.in_point = l.start_time;
            l.out_point = Tick::from_seconds_f64(9.0);
            let mut comp = base.clone();
            comp.layers = vec![l.clone()].into();
            let cid = p.add_item("C", Label::Sandstone, None, ItemKind::Comp(comp.into()));
            let levels = l.props.sub("audio").and_then(|g| g.get("levels"));
            // The layer's start, a quarter and half of its ramp, its end, and after it.
            for s in [2.0, 2.25, 2.5, 3.0, 4.0, 6.0] {
                let t = Tick::from_seconds_f64(s);
                let drawn = level_gains(&l, levels, t);
                let mixed = mix_comp(&p, &Half, None, cid, t, 1, 1000);
                for c in 0..2 {
                    let heard = mixed.get(c).copied().unwrap_or(f32::NAN) / 0.5;
                    assert!((drawn[c] - heard).abs() < 2e-3, "stretch {stretch}, {s} s, channel {c}: drawn {} heard {heard}", drawn[c]);
                }
            }
            // At the layer's start (layer time 0) it is the -6 dB key, not the 0 dB one.
            assert!((level_gains(&l, levels, Tick::from_seconds_f64(2.0))[0] - 0.5).abs() < 1e-3);
        }
    }
}
