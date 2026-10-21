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

/// Left/right gain of the layer's Audio Levels (dB) at comp time `t`; unity without the property.
fn level_gains(levels: Option<&Property>, t: Tick) -> [f32; 2] {
    match levels {
        Some(p) => {
            let [l, r] = p.value_at(t).as_vec2();
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
        let gain = level_gains(levels, Tick::from_seconds_f64((ta + tb) * 0.5));
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
    use effectcraft_engine::project::Value;

    #[test]
    fn waveform_follows_audio_levels() {
        let p = Property::new(1, "levels", "Audio Levels", Value::Vec2([-6.0206, -96.0]));
        let g = level_gains(Some(&p), Tick::ZERO);
        assert!((g[0] - 0.5).abs() < 1e-3, "{}", g[0]);
        assert_eq!(g[1], 0.0);
        assert_eq!(level_gains(None, Tick::ZERO), [1.0, 1.0]);
    }
}
