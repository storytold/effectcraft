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

/// Does the layer's [in, out) span play mirrored, as far as its waveform goes
/// ([`audio::plays_backwards`])? A reversed layer (negative stretch) is audible and plays the
/// source span backwards through its time mapping (#3239), which `Layer::layer_time` already
/// gives, so only the Backwards effect adds a mirror, and on a reversed layer it undoes the
/// reversal. The Audio switch and solo are not followed: a muted layer still shows its waveform.
fn played(layer: &Layer) -> bool {
    audio::plays_backwards(layer)
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
/// timeline's time map and the layer's time mapping, limited to its [in, out) span and drawn as
/// the layer plays (see [`played`]).
pub(crate) fn draw(p: &Painter, rect: Rect, tm: &TMap, layer: &Layer, s: &Summary, color: Color32) {
    let mid = [rect.min.y + rect.height() * 0.25, rect.min.y + rect.height() * 0.75];
    let half = rect.height() * 0.25 - 1.0;
    let x0 = tm.x(layer.in_point.seconds()).max(rect.min.x).floor() as i32;
    let x1 = tm.x(layer.out_point.seconds()).min(rect.max.x).ceil() as i32;
    let levels = layer.props.sub("audio").and_then(|g| g.get("levels"));
    for c in 0..2 {
        p.line_segment([pos2(x0 as f32, mid[c]), pos2(x1 as f32, mid[c])], Stroke::new(1.0, color.gamma_multiply(0.5)));
    }
    let mirror = played(layer);
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
        column_bins(l, s, ta, tb, played(l)).map(|bins| bins.iter().fold(f32::MIN, |m, b| m.max(b[1])))
    }

    /// The left-channel sample the mix plays at comp time `t`.
    fn heard(p: &Project, cid: ItemId, t: f64) -> f32 {
        mix_comp(p, &Ramp, None, cid, Tick::from_seconds_f64(t), 1, 8000).first().copied().unwrap_or(f32::NAN)
    }

    /// Columns of `l` over [0, 1) s: each drawn peak against what the mix plays at the column.
    fn assert_columns_match_the_mix(p: &Project, cid: ItemId, l: &Layer, s: &Summary, name: &str) {
        let mut max_drawn = 0.0f32;
        for k in 0..100 {
            let (ta, tb) = (k as f64 / 100.0, (k + 1) as f64 / 100.0);
            let d = drawn(l, s, ta, tb).unwrap_or_else(|| panic!("{name}: no column at {ta} s"));
            let h = heard(p, cid, (ta + tb) * 0.5);
            // The ramp rises 0.01 per column, so a column's peak is within a couple of steps.
            assert!((d - h).abs() < 0.03, "{name}: column at {ta} s draws {d}, heard {h}");
            max_drawn = max_drawn.max(d);
        }
        assert!(max_drawn > 0.9, "{name}: the waveform is drawn (loudest {max_drawn})");
    }

    /// #3238/#3239: a time-reversed layer (Time-Reverse Layer: stretch -100 %, start time at its
    /// out point) is audible, and its waveform columns agree with the mix: loud first, quiet last.
    #[test]
    fn a_reversed_layer_draws_the_waveform_the_mix_plays() {
        let (p, cid, l, s) = setup(&|l| {
            l.stretch = -100.0;
            l.start_time = Tick::from_seconds_f64(1.0);
        });
        assert_columns_match_the_mix(&p, cid, &l, &s, "reversed");
        assert!(heard(&p, cid, 0.005) > 0.98, "the mix plays the clip's last samples first");
        assert!(heard(&p, cid, 0.995) < 0.02, "and its first samples last");
    }

    /// Backwards on a reversed layer plays it forwards again, and the waveform follows.
    #[test]
    fn a_double_reversal_draws_the_waveform_forwards() {
        let (p, cid, l, s) = setup(&|l| {
            l.stretch = -100.0;
            l.start_time = Tick::from_seconds_f64(1.0);
            add_backwards(l, true);
        });
        assert_columns_match_the_mix(&p, cid, &l, &s, "double reversal");
        assert!(heard(&p, cid, 0.005) < 0.02, "the mix plays the clip's first samples first");
        assert!(heard(&p, cid, 0.995) > 0.98, "and its last samples last");
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
