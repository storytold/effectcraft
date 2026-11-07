//! Audio waveforms for the Timeline's Audio > Waveform property (revealed with `LL`).
//!
//! Each footage item's audio is summarised once into min/max peaks
//! (`effectcraft_render::audio::footage_peaks`, [`BINS_PER_SEC`] bins per second) on a
//! background thread; the timeline draws from that summary at any zoom.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use effectcraft_engine::project::{ItemId, ItemKind, Layer, LayerSource, Project, Property};
use effectcraft_engine::render::audio::{self, db_to_gain};
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

/// How the layer plays its audio, as far as its waveform goes: `None` when it is silent
/// (reversed, see [`audio::stretch_is_mixed`]), else whether the Backwards effect plays its
/// [in, out) span mirrored ([`audio::plays_backwards`]). The Audio switch and solo are not
/// followed: a muted layer still shows its waveform.
fn played(layer: &Layer) -> Option<bool> {
    audio::stretch_is_mixed(layer).then(|| audio::plays_backwards(layer))
}

/// The summary bins under the Timeline column for comp times `ta .. tb` (seconds, limited to
/// the layer's [in, out) span), as the layer plays them: mirrored over that span when `mirror`
/// (see [`played`]), so the first column shows the last samples.
fn column_bins<'a>(layer: &Layer, s: &'a Summary, ta: f64, tb: f64, mirror: bool) -> Option<&'a [[f32; 4]]> {
    let (i, o) = (layer.in_point.seconds(), layer.out_point.seconds());
    let (ta, tb) = (ta.max(i).min(o), tb.max(i).min(o));
    if tb <= ta {
        return None;
    }
    let span = i + o;
    let src = |t: f64| {
        let t = if mirror { span - t } else { t };
        layer.layer_time(Tick::from_seconds_f64(t)).seconds() * s.bins_per_sec as f64
    };
    let (u, v) = (src(ta), src(tb));
    let n = s.peaks.len() as i64;
    let ba = u.min(v).floor() as i64;
    let bb = (u.max(v).ceil() as i64).max(ba.saturating_add(1));
    let (ba, bb) = (ba.clamp(0, n), bb.clamp(0, n));
    if ba >= bb {
        return None;
    }
    s.peaks.get(ba as usize..bb as usize)
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
/// timeline's time map and the layer's time mapping, limited to its [in, out) span and drawn as
/// the layer plays (see [`played`]): only the baselines for a silent layer.
pub(crate) fn draw(p: &Painter, rect: Rect, tm: &TMap, layer: &Layer, s: &Summary, color: Color32) {
    let mid = [rect.min.y + rect.height() * 0.25, rect.min.y + rect.height() * 0.75];
    let half = rect.height() * 0.25 - 1.0;
    let x0 = tm.x(layer.in_point.seconds()).max(rect.min.x).floor() as i32;
    let x1 = tm.x(layer.out_point.seconds()).min(rect.max.x).ceil() as i32;
    let levels = layer.props.sub("audio").and_then(|g| g.get("levels"));
    for c in 0..2 {
        p.line_segment([pos2(x0 as f32, mid[c]), pos2(x1 as f32, mid[c])], Stroke::new(1.0, color.gamma_multiply(0.5)));
    }
    let Some(mirror) = played(layer) else { return };
    for x in x0..x1 {
        let (ta, tb) = (tm.t(x as f32), tm.t(x as f32 + 1.0));
        let Some(bins) = column_bins(layer, s, ta, tb, mirror) else { continue };
        let mut agg = [0.0f32; 4];
        for b in bins {
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

#[cfg(test)]
mod played_tests {
    use super::*;
    use effectcraft_engine::color::Label;
    use effectcraft_engine::project::{Comp, Footage, FootageKind, GroupKind, build};
    use effectcraft_engine::render::audio::{footage_peaks, mix_comp};
    use effectcraft_engine::render::{FootageSource, Image};
    use effectcraft_engine::time::FrameRate;

    /// Footage audio equal to its source time (seconds) on both channels over [0, 2) s.
    struct Ramp;
    impl FootageSource for Ramp {
        fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
            None
        }
        fn audio(&self, _: ItemId, _: &Footage, start: Tick, frames: usize, rate: u32) -> Option<Vec<f32>> {
            let t0 = start.seconds();
            Some(
                (0..frames)
                    .flat_map(|k| {
                        let t = t0 + k as f64 / rate as f64;
                        let v = if (0.0..2.0).contains(&t) { t as f32 } else { 0.0 };
                        [v, v]
                    })
                    .collect(),
            )
        }
    }

    /// A project with a 2 s ramp clip and a comp holding one layer of it (start and in 0 s,
    /// out 1 s) changed by `edit`, and the clip's peak summary.
    fn setup(edit: &dyn Fn(&mut Layer)) -> (Project, ItemId, Layer, Summary) {
        let mut p = Project::default();
        let f = Footage {
            path: "ramp.wav".into(),
            kind: FootageKind::Audio,
            has_audio: true,
            frame_rate: FrameRate::new(25, 1),
            duration: Tick::from_seconds_f64(2.0),
            ..Default::default()
        };
        let fid = p.add_item("ramp.wav", Label::SeaFoam, None, ItemKind::Footage(f.clone()));
        let mut comp = Comp::new(64, 64, FrameRate::new(25, 1), Tick::from_seconds_f64(2.0));
        let mut l = build::layer(&mut p, &comp, "a", LayerSource::Footage { item: fid }, (0, 0), None);
        (l.start_time, l.in_point, l.out_point) = (Tick::ZERO, Tick::ZERO, Tick::from_seconds_f64(1.0));
        edit(&mut l);
        comp.layers = vec![l.clone()].into();
        let cid = p.add_item("C", Label::Sandstone, None, ItemKind::Comp(comp.into()));
        let peaks = footage_peaks(&Ramp, fid, &f, f.duration, BINS_PER_SEC);
        (p, cid, l, Summary { bins_per_sec: BINS_PER_SEC, peaks })
    }

    fn add_backwards(l: &mut Layer, enabled: bool) {
        let mut n = 1_000_000;
        let mut g = build::Ids(&mut n).group("backwards", "Backwards");
        g.kind = GroupKind::Effect { effect: "ec.audio.backwards".into() };
        g.enabled = enabled;
        l.props.group_mut("effects").expect("effects group").children.push(g.into());
    }

    /// The loudest left-channel peak drawn in the column for comp times `ta .. tb`.
    fn drawn(l: &Layer, s: &Summary, ta: f64, tb: f64) -> Option<f32> {
        played(l).and_then(|mirror| column_bins(l, s, ta, tb, mirror)).map(|bins| bins.iter().fold(f32::MIN, |m, b| m.max(b[1])))
    }

    /// The left-channel sample the mix plays at comp time `t`.
    fn heard(p: &Project, cid: ItemId, t: f64) -> f32 {
        mix_comp(p, &Ramp, None, cid, Tick::from_seconds_f64(t), 1, 8000).first().copied().unwrap_or(f32::NAN)
    }

    /// #3238: a time-reversed layer (Time-Reverse Layer: stretch -100 %, start time at its out
    /// point) is silent in the mix, so the Timeline draws no waveform columns for it.
    #[test]
    fn a_reversed_layer_draws_no_waveform_as_the_mix_is_silent() {
        let (p, cid, l, s) = setup(&|l| {
            l.stretch = -100.0;
            l.start_time = Tick::from_seconds_f64(1.0);
        });
        for k in 0..200 {
            let (ta, tb) = (k as f64 / 200.0, (k + 1) as f64 / 200.0);
            assert_eq!(drawn(&l, &s, ta, tb), None, "column at {ta} s");
        }
        for t in [0.1, 0.5, 0.9] {
            assert_eq!(heard(&p, cid, t), 0.0, "the mix plays the reversed layer at {t} s");
        }
    }

    /// #3238: with the Backwards audio effect the first column shows the clip's last samples
    /// and the last column its first, matching what the mix plays there.
    #[test]
    fn backwards_draws_the_waveform_mirrored_as_the_mix_plays_it() {
        let (p, cid, l, s) = setup(&|l| add_backwards(l, true));
        let first = drawn(&l, &s, 0.0, 0.005).expect("first column");
        let h = heard(&p, cid, 0.0);
        assert!(first > 0.98, "first column draws {first}, not the clip's last samples");
        assert!((first - h).abs() < 0.01, "first column {first}, heard {h}");
        let last = drawn(&l, &s, 0.995, 1.0).expect("last column");
        let h = heard(&p, cid, 0.995);
        assert!(last < 0.02, "last column draws {last}, not the clip's first samples");
        assert!((last - h).abs() < 0.01, "last column {last}, heard {h}");
    }

    /// No Backwards effect, a disabled one, or the layer's Effects switch off: the mix plays
    /// forwards, and so the waveform is drawn.
    #[test]
    fn without_an_active_backwards_effect_the_waveform_runs_forwards() {
        let edits: [(&str, &dyn Fn(&mut Layer)); 3] = [
            ("no effect", &|_| {}),
            ("effect disabled", &|l| add_backwards(l, false)),
            ("effects switch off", &|l| {
                add_backwards(l, true);
                l.switches.effects = false;
            }),
        ];
        for (name, edit) in edits {
            let (p, cid, l, s) = setup(edit);
            let first = drawn(&l, &s, 0.0, 0.005).expect("first column");
            let h = heard(&p, cid, 0.004);
            assert!(first < 0.02, "{name}: first column draws {first}");
            assert!((first - h).abs() < 0.01, "{name}: first column {first}, heard {h}");
            // A column reaching past the out point draws only up to it.
            let edge = drawn(&l, &s, 0.998, 1.003).expect("edge column");
            assert!(edge < 1.001, "{name}: the edge column draws {edge}, past the out point");
            assert_eq!(drawn(&l, &s, 1.0, 1.005), None, "{name}: a column after the out point");
        }
    }
}
