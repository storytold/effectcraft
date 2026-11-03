//! Roto Brush benchmark: synthetic clips with known mattes (movement, colour change, occlusion,
//! leaving and re-entering the frame) and the metrics that compare trackers on them.
//!
//! Needs no weights: the classical engine always runs, and a trained [`MaskModel`] (MobileSAM,
//! later SAM 2) is added when the caller has one. `effectcraft-cli bench --roto` prints the table.

use effectcraft_raster::Image;
use std::sync::Arc;

use effectcraft_segment::{ClipFrame, MaskModel, TrackRequest};
use serde::Serialize;
use web_time::Instant;

use super::{FrameSeg, SegOpts, Stroke, StrokeKind, iou, propagate_with, rgb_of, segment_tracked, segment_with, tracker_prompt_boxed};

pub const W: u32 = 200;
pub const H: u32 = 150;
const R: f64 = 32.0;

/// A clip with the true matte of the visible object on every frame.
#[derive(Clone)]
pub struct Scenario {
    pub name: String,
    pub what: String,
    /// Frame size in pixels (the synthetic clips are `W × H`; a real clip has its own).
    pub w: usize,
    pub h: usize,
    pub frames: Vec<Image>,
    /// Visible-object matte per frame (`w × h`, 0/1); empty when the object is out of view.
    pub truth: Vec<Vec<u8>>,
    /// Frame the strokes are painted on.
    pub base: usize,
    pub strokes: Vec<Stroke>,
}

/// One tracker's result on one scenario.
#[derive(Clone, Debug, Serialize)]
pub struct Run {
    pub scenario: String,
    pub tracker: String,
    pub mean_iou: f64,
    pub min_iou: f64,
    /// Mean IoU between consecutive mattes minus the same for the truth, as a loss (0 is steady).
    pub flicker: f64,
    pub secs_per_frame: f64,
    /// Mean IoU over the frames after the object's last absence (re-appearance), if it had one.
    pub after_return: Option<f64>,
    pub per_frame: Vec<f64>,
    /// Correction rounds needed before every frame reaches the target IoU (`>N`: not within N); set by
    /// [`with_fixes`], `None` when not measured.
    pub fixes: Option<String>,
    /// The most memory (MB) the tracker held for its memory banks and cached frames (trackers that have none: `None`).
    pub peak_mb: Option<f64>,
    /// The matte of every frame (not serialised).
    #[serde(skip)]
    pub mattes: Vec<Vec<u8>>,
}

fn noise(x: f64, y: f64, seed: u32) -> f64 {
    fn h(i: i64, j: i64, s: u32) -> f64 {
        let mut v = (i.wrapping_mul(73856093) ^ j.wrapping_mul(19349663) ^ i64::from(s).wrapping_mul(83492791)) as u64;
        v ^= v >> 13;
        v = v.wrapping_mul(0x5bd1e995);
        v ^= v >> 15;
        (v % 10_000) as f64 / 10_000.0
    }
    let mut out = 0.0;
    for (cell, amp) in [(9.0, 0.6), (3.5, 0.4)] {
        let (fx, fy) = (x / cell, y / cell);
        let (i, j) = (fx.floor() as i64, fy.floor() as i64);
        let (tx, ty) = (fx - i as f64, fy - j as f64);
        let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
        let a = h(i, j, seed) + (h(i + 1, j, seed) - h(i, j, seed)) * sx;
        let b = h(i, j + 1, seed) + (h(i + 1, j + 1, seed) - h(i, j + 1, seed)) * sx;
        out += amp * (a + (b - a) * sy);
    }
    out
}

/// What a frame shows: the disk centre, an optional occluding bar `[x0, y0, x1, y1]` and a
/// brightness gain applied to the whole frame.
#[derive(Clone, Copy)]
struct Look {
    c: [f64; 2],
    bar: Option<[f64; 4]>,
    gain: f32,
}

fn render(l: Look) -> (Image, Vec<u8>) {
    let mut img = Image::new(W, H);
    let mut gt = vec![0u8; (W * H) as usize];
    for y in 0..H {
        for x in 0..W {
            let p = [f64::from(x) + 0.5, f64::from(y) + 0.5];
            let d = (p[0] - l.c[0]).hypot(p[1] - l.c[1]);
            let a = (R + 0.5 - d).clamp(0.0, 1.0) as f32;
            let n = noise(p[0] - l.c[0], p[1] - l.c[1], 11);
            let f = [(0.75 + 0.25 * n) as f32, (0.35 + 0.3 * n) as f32, (0.1 + 0.15 * n) as f32];
            let (bn, bm) = (noise(p[0], p[1], 5), noise(p[0] * 1.7, p[1] * 1.7, 9));
            let b = [(0.05 + 0.25 * bm) as f32, (0.3 + 0.4 * bn) as f32, (0.45 + 0.4 * bm) as f32];
            let mut px = [0.0f32; 3];
            for k in 0..3 {
                px[k] = f[k] * a + b[k] * (1.0 - a);
            }
            let under_bar = l.bar.is_some_and(|r| p[0] >= r[0] && p[0] < r[2] && p[1] >= r[1] && p[1] < r[3]);
            if under_bar {
                let t = noise(p[0] * 0.8, p[1] * 0.8, 23) as f32;
                px = [0.15 + 0.1 * t, 0.15 + 0.1 * t, 0.2 + 0.1 * t];
            }
            let g = l.gain;
            img.set(x, y, [(px[0] * g).min(1.0), (px[1] * g).min(1.0), (px[2] * g).min(1.0), 1.0]);
            if let Some(v) = gt.get_mut((y * W + x) as usize) {
                *v = (d <= R && !under_bar) as u8;
            }
        }
    }
    (img, gt)
}

fn line(kind: StrokeKind, frame: i64, a: [f64; 2], b: [f64; 2], radius: f64) -> Stroke {
    let pt = |k: u32| {
        let t = f64::from(k) / 10.0;
        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
    };
    Stroke { kind, frame, radius, points: (0..=10).map(pt).collect(), object: 0 }
}

/// The strokes an artist would paint on the base frame: one through the disk, two around it.
fn strokes_for(c: [f64; 2]) -> Vec<Stroke> {
    vec![
        line(StrokeKind::Fg, 0, [c[0] - 18.0, c[1] - 6.0], [c[0] + 18.0, c[1] + 6.0], 5.0),
        line(StrokeKind::Bg, 0, [c[0] - 50.0, c[1] - 45.0], [c[0] + 50.0, c[1] - 45.0], 5.0),
        line(StrokeKind::Bg, 0, [c[0] + 50.0, c[1] + 45.0], [c[0] - 45.0, c[1] + 45.0], 5.0),
    ]
}

fn scenario(name: &'static str, what: &'static str, n: usize, look: impl Fn(usize) -> Look) -> Scenario {
    let (mut frames, mut truth) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for f in 0..n {
        let (img, gt) = render(look(f));
        frames.push(img);
        truth.push(gt);
    }
    Scenario { name: name.to_string(), what: what.to_string(), w: W as usize, h: H as usize, frames, truth, base: 0, strokes: strokes_for(look(0).c) }
}

/// A scenario from the user's own clip: frames and their true mattes (0/1, same size as the frames). The strokes
/// are made from the first frame with an object: a short foreground stroke through its centre and background
/// strokes along the top and bottom edges where the truth is empty. Nothing is bundled: the clip stays with the
/// user (a licensed dataset, say), and so does its licence.
pub fn scenario_from(name: &str, frames: Vec<Image>, truth: Vec<Vec<u8>>) -> std::result::Result<Scenario, String> {
    let first = frames.first().ok_or("a clip needs at least one frame")?;
    let (w, h) = (first.width as usize, first.height as usize);
    if w == 0
        || h == 0
        || frames.len() != truth.len()
        || frames.iter().any(|f| (f.width as usize, f.height as usize) != (w, h))
        || truth.iter().any(|t| t.len() != w * h)
    {
        return Err("frames and truth masks must be the same size and count".into());
    }
    let base = truth.iter().position(|t| t.iter().any(|v| *v != 0)).ok_or("no frame has the object in its truth mask")?;
    let t = truth.get(base).map(Vec::as_slice).unwrap_or(&[]);
    let (mut x0, mut x1, mut sx, mut sy, mut n) = (usize::MAX, 0usize, 0.0f64, 0.0f64, 0.0f64);
    for i in t.iter().enumerate().filter(|(_, v)| **v != 0).map(|(i, _)| i) {
        let (x, y) = (i % w, i / w);
        (x0, x1) = (x0.min(x), x1.max(x));
        sx += x as f64 + 0.5;
        sy += y as f64 + 0.5;
        n += 1.0;
    }
    let (cx, cy) = (sx / n, sy / n);
    let reach = (((x1 - x0) as f64) * 0.15).max(2.0);
    let radius = ((w.min(h) as f64) * 0.03).max(3.0);
    let mut strokes = vec![line(StrokeKind::Fg, base as i64, [cx - reach, cy], [cx + reach, cy], radius)];
    for y in [h as f64 * 0.04, h as f64 * 0.96] {
        let row = y as usize;
        let free = t.get(row * w..(row + 1) * w).is_some_and(|r| r.iter().all(|v| *v == 0));
        if free {
            strokes.push(line(StrokeKind::Bg, base as i64, [w as f64 * 0.05, y], [w as f64 * 0.95, y], radius));
        }
    }
    Ok(Scenario { name: name.to_string(), what: "a clip supplied by the user".to_string(), w, h, frames, truth, base, strokes })
}

/// The benchmark clips, `n` frames each (at least 12, so every event happens).
pub fn scenarios(n: usize) -> Vec<Scenario> {
    let n = n.clamp(12, 240);
    let k = n as f64 / 24.0;
    let drift = move |f: usize| {
        let t = f as f64 / k;
        [60.0 + 3.1 * t, 70.0 + 1.3 * t + 4.0 * (t * 0.3).sin()]
    };
    vec![
        scenario("move", "disk drifts across a textured background", n, move |f| Look { c: drift(f), bar: None, gain: 1.0 }),
        scenario("light", "same motion while the whole frame brightens by 40 %", n, move |f| Look {
            c: drift(f),
            bar: None,
            gain: 1.0 + 0.4 * f as f32 / n as f32,
        }),
        scenario("occlusion", "a bar crosses and hides most of the disk, then clears it", n, move |f| {
            let t = f as f64 / n as f64;
            let c = drift(f);
            let x = -40.0 + 320.0 * ((t - 0.3) / 0.4).clamp(0.0, 1.0);
            Look { c, bar: (0.3..0.7).contains(&t).then_some([x - 40.0, 0.0, x + 40.0, f64::from(H)]), gain: 1.0 }
        }),
        scenario("reenter", "disk leaves through the right edge and comes back", n, move |f| {
            let t = f as f64 / n as f64;
            let x = 60.0 + 190.0 * (1.0 - (2.0 * t - 1.0).powi(2));
            Look { c: [x, 70.0 + 4.0 * (t * 6.0).sin()], bar: None, gain: 1.0 }
        }),
    ]
}

fn metrics(sc: &Scenario, tracker: &str, mattes: &[Vec<u8>], secs: f64) -> Run {
    let per_frame: Vec<f64> = mattes.iter().zip(&sc.truth).map(|(m, t)| iou(m, t)).collect();
    let n = per_frame.len().max(1) as f64;
    let steady = |v: &[Vec<u8>]| {
        let pairs: Vec<f64> = v.windows(2).filter_map(|w| Some(iou(w.first()?, w.get(1)?))).collect();
        pairs.iter().sum::<f64>() / pairs.len().max(1) as f64
    };
    let last_gap = sc.truth.iter().rposition(|t| t.iter().all(|v| *v == 0));
    let after_return = last_gap.and_then(|g| {
        let tail = per_frame.get(g + 1..)?;
        (!tail.is_empty()).then(|| tail.iter().sum::<f64>() / tail.len() as f64)
    });
    Run {
        scenario: sc.name.clone(),
        tracker: tracker.to_string(),
        mean_iou: per_frame.iter().sum::<f64>() / n,
        min_iou: per_frame.iter().copied().fold(1.0, f64::min),
        flicker: (steady(&sc.truth) - steady(mattes)).max(0.0),
        secs_per_frame: secs / n,
        after_return,
        per_frame,
        fixes: None,
        peak_mb: None,
        mattes: mattes.to_vec(),
    }
}

/// Segment the base frame from the strokes and propagate to the end of the clip, as the Roto
/// Brush does, with `model` (or none: the classical engine).
pub fn run_flow(sc: &Scenario, tracker: &str, model: Option<&dyn MaskModel>) -> Run {
    let opts = SegOpts::default();
    let start = Instant::now();
    let refs: Vec<&Stroke> = sc.strokes.iter().filter(|s| s.frame == sc.base as i64).collect();
    let mut segs: Vec<Option<FrameSeg>> = vec![None; sc.frames.len()];
    if let Some(first) = sc.frames.get(sc.base) {
        let base = segment_with(first, &refs, None, &opts, 0.0, model);
        if let Some(slot) = segs.get_mut(sc.base) {
            *slot = Some(base);
        }
    }
    for f in sc.base + 1..sc.frames.len() {
        let (Some(prev_img), Some(img)) = (sc.frames.get(f - 1), sc.frames.get(f)) else { break };
        let Some(Some(prev)) = segs.get(f - 1) else { break };
        let here: Vec<&Stroke> = sc.strokes.iter().filter(|s| s.frame == f as i64).collect();
        let next = propagate_with(prev_img, prev, img, &here, &opts, 0.0, model);
        if let Some(slot) = segs.get_mut(f) {
            *slot = Some(next);
        }
    }
    let secs = start.elapsed().as_secs_f64();
    let empty = vec![0u8; sc.w * sc.h];
    let mattes: Vec<Vec<u8>> = segs.into_iter().map(|s| s.map_or_else(|| empty.clone(), |s| s.matte)).collect();
    metrics(sc, tracker, &mattes, secs)
}

/// Follow the object through the whole clip with the model's video tracker, as the Roto Brush's
/// tracking pass does (prompt from the strokes, probability maps refined by the same graph cut).
/// `None` when the model has no tracker or the tracker fails.
pub fn run_tracker(sc: &Scenario, tracker: &str, model: &dyn MaskModel) -> Option<Run> {
    let t = model.tracker()?;
    let opts = SegOpts::default();
    let at = |f: usize| -> Vec<&Stroke> { sc.strokes.iter().filter(|s| s.frame == f as i64).collect() };
    let clip: Vec<Arc<ClipFrame>> = sc.frames.iter().map(|i| Arc::new(ClipFrame { rgb: rgb_of(i), w: i.width as usize, h: i.height as usize })).collect();
    // a prompt per stroke frame: a few clicks and the box of the classic matte (as Chain::track does)
    let mut prompts = vec![];
    for f in 0..sc.frames.len() {
        let strokes = at(f);
        let Some(img) = sc.frames.get(f).filter(|_| strokes.iter().any(|s| s.kind == StrokeKind::Fg)) else { continue };
        prompts.push((f as i64, tracker_prompt_boxed(&strokes, 1.0, &super::segment(img, &strokes, None, &opts, 0.0))));
    }
    let req = TrackRequest::single(0, sc.frames.len(), prompts);
    let start = Instant::now();
    let mut mattes: Vec<Vec<u8>> = vec![vec![0u8; sc.w * sc.h]; sc.frames.len()];
    let mut get = |f: i64| usize::try_from(f).ok().and_then(|i| clip.get(i).cloned());
    let r = t.track(&req, &mut get, &|| false, &mut |_, f, tf| {
        let (Ok(i), true) = (usize::try_from(f), tf.prob.len() == sc.w * sc.h) else { return };
        let Some(img) = sc.frames.get(i) else { return };
        let strokes = at(i);
        if let Some(slot) = mattes.get_mut(i) {
            *slot = segment_tracked(img, &strokes, tf.prob, tf.occluded, &opts, 0.0).matte;
        }
    });
    r.ok()?;
    let mut run = metrics(sc, tracker, &mattes, start.elapsed().as_secs_f64());
    run.peak_mb = model.peak_state_bytes().map(|b| b as f64 / (1 << 20) as f64);
    Some(run)
}

/// The correction a user would paint on frame `f` where `pred` is wrong: a short foreground stroke through
/// the true object's centre, and a background stroke at the centroid of all false-positive pixels.
fn correction(sc: &Scenario, f: usize, pred: &[u8]) -> Vec<Stroke> {
    let truth = sc.truth.get(f).map(Vec::as_slice).unwrap_or(&[]);
    let centre = |keep: &dyn Fn(usize) -> bool| -> Option<[f64; 2]> {
        let (mut sx, mut sy, mut n) = (0.0, 0.0, 0.0);
        for i in (0..truth.len().min(pred.len())).filter(|i| keep(*i)) {
            sx += (i % sc.w.max(1)) as f64 + 0.5;
            sy += (i / sc.w.max(1)) as f64 + 0.5;
            n += 1.0;
        }
        (n > 0.0).then(|| [sx / n, sy / n])
    };
    let mut out = vec![];
    if let Some(c) = centre(&|i| truth.get(i).is_some_and(|v| *v != 0)) {
        out.push(line(StrokeKind::Fg, f as i64, [c[0] - 12.0, c[1]], [c[0] + 12.0, c[1]], 5.0));
    }
    if let Some(c) = centre(&|i| pred.get(i).is_some_and(|v| *v != 0) && truth.get(i).is_some_and(|v| *v == 0)) {
        out.push(line(StrokeKind::Bg, f as i64, [c[0] - 6.0, c[1]], [c[0] + 6.0, c[1]], 6.0));
    }
    out
}

/// Repeat `run` on `sc`, each time painting the correction a user would on the first frame under `target` IoU,
/// until every frame reaches it or `max` correction rounds were needed. Sets `fixes` on the first run's result:
/// the number of rounds (each can add foreground and background strokes), or `>max`.
pub fn with_fixes(sc: &Scenario, target: f64, max: usize, run: &dyn Fn(&Scenario) -> Option<Run>) -> Option<Run> {
    let mut cur = sc.clone();
    let mut first: Option<Run> = None;
    for k in 0..=max {
        let r = run(&cur)?;
        let bad = r.per_frame.iter().position(|v| *v < target);
        let first_run = first.get_or_insert_with(|| r.clone());
        let Some(f) = bad else {
            first_run.fixes = Some(k.to_string());
            break;
        };
        if k == max {
            first_run.fixes = Some(format!(">{max}"));
            break;
        }
        let pred = r.mattes.get(f).cloned().unwrap_or_default();
        let fix = correction(&cur, f, &pred);
        if fix.is_empty() {
            first_run.fixes = Some(format!(">{max}"));
            break;
        }
        cur.strokes.extend(fix);
    }
    first
}

/// A fixed-width table of the runs, grouped by scenario.
pub fn table(runs: &[Run]) -> String {
    let mut out = format!(
        "{:<10} {:<12} {:>8} {:>8} {:>8} {:>10} {:>9} {:>6} {:>7}\n",
        "clip", "tracker", "meanIoU", "minIoU", "flicker", "after ret.", "s/frame", "fixes", "peakMB"
    );
    for r in runs {
        let back = r.after_return.map_or_else(|| "-".to_string(), |v| format!("{v:.3}"));
        out.push_str(&format!(
            "{:<10} {:<12} {:>8.3} {:>8.3} {:>8.3} {:>10} {:>9.3} {:>6} {:>7}\n",
            r.scenario,
            r.tracker,
            r.mean_iou,
            r.min_iou,
            r.flicker,
            back,
            r.secs_per_frame,
            r.fixes.as_deref().unwrap_or("-"),
            r.peak_mb.map_or_else(|| "-".to_string(), |v| format!("{v:.0}"))
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(v: &[u8]) -> usize {
        v.iter().filter(|x| **x != 0).count()
    }

    #[test]
    fn scenarios_have_the_events_they_claim() {
        let all = scenarios(24);
        assert_eq!(all.len(), 4);
        for s in &all {
            assert_eq!((s.frames.len(), s.truth.len()), (24, 24), "{}", s.name);
        }
        let full = all.first().map(|s| area(&s.truth[0])).unwrap_or(0);
        assert!(full > 3000, "{full}");
        let occ = all.iter().find(|s| s.name == "occlusion").map(|s| s.truth.iter().map(|t| area(t)).min().unwrap_or(0)).unwrap_or(full);
        assert!(occ < full / 2, "occluded minimum {occ} of {full}");
        let gone = all.iter().find(|s| s.name == "reenter").map(|s| s.truth.iter().filter(|t| area(t) == 0).count()).unwrap_or(0);
        assert!(gone > 0, "the disk never leaves");
        assert!(all.iter().find(|s| s.name == "reenter").is_some_and(|s| area(s.truth.last().map_or(&[][..], |v| v)) > 0), "it must come back");
    }

    #[test]
    fn classical_tracks_plain_motion_and_the_table_lists_it() {
        let all = scenarios(24);
        let Some(sc) = all.first() else { return };
        let run = run_flow(sc, "classical", None);
        assert!(run.mean_iou > 0.9, "mean IoU {}", run.mean_iou);
        assert_eq!(run.per_frame.len(), 24);
        assert!(table(&[run]).contains("classical"));
    }

    #[test]
    fn corrections_are_counted_until_every_frame_reaches_the_target() {
        let all = scenarios(24);
        let classical = |s: &Scenario| Some(run_flow(s, "classical", None));
        // plain motion needs none
        let easy = all.iter().find(|s| s.name == "move").and_then(|s| with_fixes(s, 0.9, 6, &classical)).unwrap();
        assert_eq!(easy.fixes.as_deref(), Some("0"));
        // after the disk has left and returned the classic engine has lost it: it needs a correction (or never gets there)
        let hard = all.iter().find(|s| s.name == "reenter").and_then(|s| with_fixes(s, 0.9, 6, &classical)).unwrap();
        assert_ne!(hard.fixes.as_deref(), Some("0"), "{:?}", hard.fixes);
        assert!(table(&[easy, hard]).contains("fixes"));
    }

    #[test]
    fn a_users_clip_becomes_a_scenario_and_bad_ones_are_refused() {
        let all = scenarios(24);
        let src = all.iter().find(|s| s.name == "move").unwrap();
        let sc = scenario_from("mine", src.frames.clone(), src.truth.clone()).unwrap();
        assert_eq!((sc.w, sc.h, sc.base), (W as usize, H as usize, 0));
        assert!(sc.strokes.iter().any(|s| s.kind == StrokeKind::Fg) && sc.strokes.iter().any(|s| s.kind == StrokeKind::Bg));
        let run = run_flow(&sc, "classical", None);
        assert!(run.mean_iou > 0.9, "{}", run.mean_iou);
        assert!(scenario_from("x", vec![], vec![]).is_err());
        assert!(scenario_from("x", src.frames.clone(), src.truth[..3].to_vec()).is_err());
        assert!(scenario_from("x", src.frames.clone(), vec![vec![0; sc.w * sc.h]; src.frames.len()]).is_err(), "no object anywhere");
        let mut short = src.truth.clone();
        short[0].pop();
        assert!(scenario_from("x", src.frames.clone(), short).is_err());
    }

    #[test]
    fn tiny_clip_requests_are_clamped() {
        assert!(scenarios(0).iter().all(|s| s.frames.len() == 12));
    }
}
