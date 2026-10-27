//! Keyframe assistants of the Wiggler, Smoother and Motion Sketch panels (Window ▸ Wiggler /
//! Smoother / Motion Sketch). Each is one undoable command, so agents get the same results as the
//! panels:
//!
//! * `keys.wiggle` adds random deviation keys between the first and last selected keys of each
//!   property (Spatial or Temporal path, smooth or jagged noise, one / all-the-same / independent
//!   dimensions, frequency in keys per second, magnitude in the property's units). Seeded, so a
//!   given selection and seed always produce the same keys.
//! * `keys.smooth` replaces the selected run of keys by the fewest auto-Bezier keys that stay
//!   within `tolerance` of the original curve (sampled every frame).
//! * `motion.sketch` turns a recorded pointer path (comp pixels, with capture timestamps) into
//!   Position keys, one per frame, then smooths them like the Smoother.

use effectcraft_keyframe::{Interp, Keyframe, Value as KValue, evaluate};
use effectcraft_project::{LayerId, Uid};
use effectcraft_time::{FrameRate, Tick};
use serde_json::{Value, json};

use super::{CommandSpec, bad, f_p, has_comp, has_keys, layer_mut, layer_p, str_p};
use crate::{EngineError, KeyRef, Result, Session, cmd};

// ---------------------------------------------------------------- noise

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// Deterministic uniform noise in [-1, 1] for (seed, stream, index).
fn rnd(seed: u64, stream: u64, i: i64) -> f64 {
    let h = splitmix(seed ^ splitmix(stream.wrapping_mul(0x1000_0000_01b3) ^ (i as u64)));
    (h >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
}

/// Wiggler noise type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Noise {
    /// Gradual deviations: lattice noise at half the key rate with cosine interpolation.
    Smooth,
    /// Independent deviations per key.
    Jagged,
}

/// Noise sample `k` (0-based key index) in [-1, 1].
fn noise(kind: Noise, seed: u64, stream: u64, k: i64) -> f64 {
    match kind {
        Noise::Jagged => rnd(seed, stream, k),
        Noise::Smooth => {
            let (i, f) = (k.div_euclid(2), (k.rem_euclid(2)) as f64 / 2.0);
            let w = (1.0 - (f * std::f64::consts::PI).cos()) / 2.0;
            rnd(seed, stream, i) * (1.0 - w) + rnd(seed, stream, i + 1) * w
        }
    }
}

/// Which dimensions the Wiggler affects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dims {
    /// Only dimension `n`.
    One(usize),
    /// All dimensions by the same amount.
    Same,
    /// Each dimension independently.
    Independent,
}

/// Wiggler settings.
#[derive(Clone, Copy, Debug)]
pub struct WiggleOpts {
    /// Spatial path (deviate values along the motion path) or temporal path (deviate timing on
    /// spatial properties; values on others).
    pub spatial: bool,
    pub noise: Noise,
    pub dims: Dims,
    /// Keys per second.
    pub frequency: f64,
    pub magnitude: f64,
    pub seed: u64,
}

/// Wiggle `keys` between keys `a` and `b` (indices, `a < b`): returns the new key list. New keys
/// fall on whole frames of `fr` (via `to_layer`/`to_comp` time maps). `spatial_prop` selects
/// motion-path semantics.
#[allow(clippy::too_many_arguments)]
pub fn wiggle_keys(
    keys: &[Keyframe],
    a: usize,
    b: usize,
    spatial_prop: bool,
    fr: FrameRate,
    to_comp: &dyn Fn(Tick) -> Tick,
    to_layer: &dyn Fn(Tick) -> Tick,
    o: &WiggleOpts,
    stream: u64,
) -> Vec<Keyframe> {
    let (t0, t1) = (keys[a].time, keys[b].time);
    let (f0, f1) = (fr.frame_at(to_comp(t0)), fr.frame_at(to_comp(t1)));
    let fps = fr.as_f64();
    let step = ((fps / o.frequency.max(0.01)).round() as i64).max(1);
    let orig = keys.to_vec();
    let mut out: Vec<Keyframe> = keys.iter().filter(|k| k.time <= t0 || k.time >= t1).cloned().collect();
    let template = keys[a].clone();
    let dims = template.value.components().len();
    let mut k = 1;
    loop {
        let f = f0 + k * step;
        if f >= f1 {
            break;
        }
        let lt = to_layer(fr.tick_of(f));
        let base_t = if o.spatial || !spatial_prop {
            lt
        } else {
            // Temporal path on a spatial property: sample the path at a jittered time (magnitude in
            // percent of the key spacing), so the speed along the path wiggles.
            let jit = noise(o.noise, o.seed, stream, k) * o.magnitude / 100.0 * step as f64 / fps;
            Tick::from_seconds_f64((lt.seconds() + jit).clamp(t0.seconds(), t1.seconds()))
        };
        let Some(v) = evaluate(&orig, base_t, spatial_prop) else { break };
        let mut c = v.components();
        if o.spatial || !spatial_prop {
            for (d, x) in c.iter_mut().enumerate().take(dims) {
                let n = match o.dims {
                    Dims::One(i) if i != d => 0.0,
                    Dims::One(_) | Dims::Same => noise(o.noise, o.seed, stream, k),
                    Dims::Independent => noise(o.noise, o.seed, stream.wrapping_add(d as u64 * 7919 + 1), k),
                };
                *x += n * o.magnitude;
            }
        }
        let mut key = Keyframe::new(lt, v.with_components(&c));
        if o.noise == Noise::Smooth {
            key.in_interp = Interp::Bezier;
            key.out_interp = Interp::Bezier;
            key.auto_bezier = true;
        }
        out.push(key);
        k += 1;
    }
    out.sort_by_key(|k| k.time);
    out
}

/// Largest distance between the curve of `keys` and `samples`.
fn max_error(keys: &[Keyframe], samples: &[(Tick, Vec<f64>)], spatial: bool) -> (f64, usize) {
    let mut worst = (0.0, 0);
    for (i, (t, v)) in samples.iter().enumerate() {
        let Some(e) = evaluate(keys, *t, spatial) else { continue };
        let d = e.components().iter().zip(v).map(|(a, b)| (a - b) * (a - b)).sum::<f64>().sqrt();
        if d > worst.0 {
            worst = (d, i);
        }
    }
    worst
}

/// Smoother: replace the keys strictly between `a` and `b` (indices) by the fewest auto-Bezier
/// keys at sample times that keep the curve within `tolerance` of `samples` (the original
/// curve, usually one sample per frame). Keys outside `a..=b` are kept.
pub fn smooth_keys(keys: &[Keyframe], a: usize, b: usize, samples: &[(Tick, Vec<f64>)], tolerance: f64, spatial: bool) -> Vec<Keyframe> {
    let template = keys[a].value.clone();
    let make = |t: Tick, c: &[f64]| {
        let mut k = Keyframe::new(t, template.with_components(c));
        if !spatial {
            k.in_interp = Interp::Bezier;
            k.out_interp = Interp::Bezier;
            k.auto_bezier = true;
        }
        k
    };
    let mut chosen: Vec<usize> = vec![];
    let build = |chosen: &[usize]| {
        let mut v: Vec<Keyframe> = keys[..=a].to_vec();
        v.extend(chosen.iter().map(|&i| make(samples[i].0, &samples[i].1)));
        v.extend(keys[b..].iter().cloned());
        v.sort_by_key(|k| k.time);
        v
    };
    loop {
        let cur = build(&chosen);
        let (err, at) = max_error(&cur, samples, spatial);
        let t = samples[at].0;
        if err <= tolerance || chosen.contains(&at) || t <= keys[a].time || t >= keys[b].time || chosen.len() >= samples.len() {
            return cur;
        }
        chosen.push(at);
        chosen.sort_unstable();
    }
}

/// The curve of `keys` sampled at every comp frame between key times `t0` and `t1` (layer time).
fn sample_curve(
    keys: &[Keyframe],
    t0: Tick,
    t1: Tick,
    spatial: bool,
    fr: FrameRate,
    to_comp: &dyn Fn(Tick) -> Tick,
    to_layer: &dyn Fn(Tick) -> Tick,
) -> Vec<(Tick, Vec<f64>)> {
    let (f0, f1) = (fr.frame_at(to_comp(t0)), fr.frame_at(to_comp(t1)));
    (f0..=f1)
        .filter_map(|f| {
            let t = to_layer(fr.tick_of(f));
            (t >= t0 && t <= t1).then(|| evaluate(keys, t, spatial).map(|v| (t, v.components())))?
        })
        .collect()
}

// ---------------------------------------------------------------- commands

fn grouped_selection(s: &Session) -> std::collections::BTreeMap<(LayerId, Uid), Vec<Tick>> {
    let mut g: std::collections::BTreeMap<(LayerId, Uid), Vec<Tick>> = Default::default();
    for k in &s.state.selected_keys {
        g.entry((k.layer, k.prop)).or_default().push(k.time);
    }
    for v in g.values_mut() {
        v.sort();
    }
    g
}

fn parse_wiggle(p: &Value) -> Result<WiggleOpts> {
    let c = "keys.wiggle";
    let spatial = match str_p(p, "apply").unwrap_or("spatial") {
        "spatial" | "Spatial Path" => true,
        "temporal" | "Temporal Path" => false,
        x => return Err(bad(c, format!("apply: spatial|temporal, got `{x}`"))),
    };
    let noise = match str_p(p, "noise").unwrap_or("smooth") {
        "smooth" | "Smooth" => Noise::Smooth,
        "jagged" | "Jagged" => Noise::Jagged,
        x => return Err(bad(c, format!("noise: smooth|jagged, got `{x}`"))),
    };
    let dims = match str_p(p, "dimensions").unwrap_or("independent") {
        "one" => Dims::One(p.get("dimension").and_then(Value::as_u64).unwrap_or(0) as usize),
        "same" | "all" => Dims::Same,
        "independent" => Dims::Independent,
        x => return Err(bad(c, format!("dimensions: one|same|independent, got `{x}`"))),
    };
    let frequency = f_p(p, "frequency").unwrap_or(5.0);
    if frequency.is_nan() || frequency <= 0.0 {
        return Err(bad(c, "frequency must be > 0"));
    }
    Ok(WiggleOpts {
        spatial,
        noise,
        dims,
        frequency,
        magnitude: f_p(p, "magnitude").unwrap_or(1.0).abs(),
        seed: p.get("seed").and_then(Value::as_u64).unwrap_or(0),
    })
}

/// Shared driver: for every property with ≥ `min` selected keys, replace its keys with
/// `f(keys, first, last, spatial, layer_maps)`; select every key in each range afterwards.
#[allow(clippy::type_complexity)]
fn run_on_selection(
    s: &mut Session,
    label: &str,
    cmd: &str,
    min: usize,
    f: &dyn Fn(&[Keyframe], usize, usize, bool, FrameRate, &dyn Fn(Tick) -> Tick, &dyn Fn(Tick) -> Tick, Uid) -> Vec<Keyframe>,
) -> Result<Value> {
    let cid = s.active_comp_id().ok_or(EngineError::NoComp)?;
    let groups = grouped_selection(s);
    if !groups.values().any(|v| v.len() >= min) {
        return Err(bad(cmd, format!("select at least {min} keyframes of a property")));
    }
    let (mut before, mut after) = (0usize, 0usize);
    s.edit(label, None, |proj, st| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let fr = comp.frame_rate;
        let mut sel = vec![];
        for ((lid, uid), times) in groups {
            let Some(l) = comp.layer_mut(lid) else { continue };
            let lc = l.clone();
            let to_comp = |t: Tick| lc.comp_time(t);
            let to_layer = |t: Tick| lc.layer_time(t);
            let Some(pr) = l.props.find_mut(uid) else { continue };
            if times.len() < min || !pr.value.interpolates() {
                continue;
            }
            let (Some(a), Some(b)) =
                (effectcraft_keyframe::key_at(&pr.keys, times[0]), effectcraft_keyframe::key_at(&pr.keys, *times.last().unwrap_or(&times[0])))
            else {
                continue;
            };
            let in_range = |k: &Keyframe| k.time >= pr.keys[a].time && k.time <= pr.keys[b].time;
            before += pr.keys.iter().filter(|k| in_range(k)).count();
            let (t0, t1) = (pr.keys[a].time, pr.keys[b].time);
            pr.keys = f(&pr.keys, a, b, pr.spatial, fr, &to_comp, &to_layer, uid);
            let n: Vec<Tick> = pr.keys.iter().filter(|k| k.time >= t0 && k.time <= t1).map(|k| k.time).collect();
            after += n.len();
            sel.extend(n.into_iter().map(|time| KeyRef { layer: lid, prop: uid, time }));
        }
        st.selected_keys = sel;
        Ok(())
    })?;
    Ok(json!({"before": before, "after": after}))
}

fn wiggle(s: &mut Session, p: &Value) -> Result<Value> {
    let o = parse_wiggle(p)?;
    run_on_selection(s, "Wiggler", "keys.wiggle", 2, &|keys, a, b, spatial, fr, tc, tl, uid| wiggle_keys(keys, a, b, spatial, fr, tc, tl, &o, uid))
}

fn smooth(s: &mut Session, p: &Value) -> Result<Value> {
    let tol = f_p(p, "tolerance").unwrap_or(1.0).max(0.0);
    run_on_selection(s, "Smoother", "keys.smooth", 3, &|keys, a, b, spatial, fr, tc, tl, _| {
        let samples = sample_curve(keys, keys[a].time, keys[b].time, spatial, fr, tc, tl);
        smooth_keys(keys, a, b, &samples, tol, spatial)
    })
}

/// Motion Sketch: `points` are `[t, x, y]` (t = capture seconds from the start) or `[x, y]`
/// (one per frame); recorded from `start` (default the current time).
fn sketch(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "motion.sketch";
    let (cid, lid) = layer_p(s, p, c)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let fr = comp.frame_rate;
    let fd = fr.frame_duration().seconds();
    let pts = p.get("points").and_then(Value::as_array).ok_or_else(|| bad(c, "missing `points`: [[t, x, y]…] or [[x, y]…] in comp pixels"))?;
    let mut rec: Vec<(f64, [f64; 2])> = vec![];
    for (i, v) in pts.iter().enumerate() {
        let a: Vec<f64> = v.as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
        match a.len() {
            2 => rec.push((i as f64 * fd, [a[0], a[1]])),
            3.. => rec.push((a[0], [a[1], a[2]])),
            _ => return Err(bad(c, format!("point {i}: expected [t, x, y] or [x, y]"))),
        }
    }
    if rec.len() < 2 {
        return Err(bad(c, "need at least two points"));
    }
    rec.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Capture speed: 100 % plays back at the recorded speed; 200 % plays back twice as slow.
    let speed = f_p(p, "captureSpeed").unwrap_or(100.0).max(1.0) / 100.0;
    let smoothing = f_p(p, "smoothing").unwrap_or(1.0).max(0.0);
    let start = p.get("start").and_then(Value::as_f64).map(Tick::from_seconds_f64).unwrap_or_else(|| s.time());
    let f0 = fr.frame_at(start);
    let t_rec0 = rec[0].0;
    let span = (rec.last().map(|r| r.0).unwrap_or(0.0) - t_rec0) * speed;
    let nframes = (span / fd).round() as i64;
    let end_frame = fr.frame_at(comp.duration - comp.frame_duration());
    let at = |secs: f64| {
        // Linear interpolation of the recorded path at recording time `secs`.
        let i = rec.partition_point(|r| r.0 <= secs).clamp(1, rec.len() - 1);
        let (a, b) = (rec[i - 1], rec[i]);
        let f = if b.0 > a.0 { ((secs - a.0) / (b.0 - a.0)).clamp(0.0, 1.0) } else { 1.0 };
        [a.1[0] + (b.1[0] - a.1[0]) * f, a.1[1] + (b.1[1] - a.1[1]) * f]
    };
    let n = s.edit("Motion Sketch", None, |proj, st| {
        let l = layer_mut(proj, cid, lid)?;
        let lc = l.clone();
        let pos = l.props.prop_mut("transform/position").ok_or_else(|| bad(c, "the layer has no Position"))?;
        let z = pos.value.as_vec3()[2];
        let three = matches!(pos.value, KValue::Vec3(_));
        let mut new: Vec<Keyframe> = vec![];
        for k in 0..=nframes {
            let f = f0 + k;
            if f > end_frame {
                break;
            }
            let q = at(t_rec0 + k as f64 * fd / speed);
            let v = if three { KValue::Vec3([q[0], q[1], z]) } else { KValue::Vec2(q) };
            new.push(Keyframe::new(lc.layer_time(fr.tick_of(f)), v));
        }
        let (Some(t0), Some(t1)) = (new.first().map(|k| k.time), new.last().map(|k| k.time)) else { return Ok(0) };
        let mut keys: Vec<Keyframe> = pos.keys.iter().filter(|k| k.time < t0 || k.time > t1).cloned().collect();
        keys.extend(new);
        keys.sort_by_key(|k| k.time);
        let (a, b) = (keys.iter().position(|k| k.time == t0).unwrap_or(0), keys.iter().position(|k| k.time == t1).unwrap_or(0));
        if smoothing > 0.0 && b > a + 1 {
            let samples: Vec<(Tick, Vec<f64>)> = keys[a..=b].iter().map(|k| (k.time, k.value.components())).collect();
            keys = smooth_keys(&keys, a, b, &samples, smoothing, true);
        }
        pos.keys = keys;
        let uid = pos.uid;
        st.selected_keys = pos.keys.iter().filter(|k| k.time >= t0 && k.time <= t1).map(|k| KeyRef { layer: lid, prop: uid, time: k.time }).collect();
        Ok(st.selected_keys.len())
    })?;
    Ok(json!({"keys": n}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "keys.wiggle",
            "Wiggler",
            [],
            None,
            "{apply?: spatial|temporal, noise?: smooth|jagged, dimensions?: one|same|independent, dimension? (index for `one`), frequency? (keys/s, 5), magnitude?, seed?}",
            has_keys,
            wiggle
        ),
        cmd!("keys.smooth", "Smoother", [], None, "{tolerance? (property units, 1)}", has_keys, smooth),
        cmd!(
            "motion.sketch",
            "Motion Sketch",
            [],
            None,
            "{layer?, points: [[t, x, y]…] (t = capture seconds) | [[x, y]…] (one per frame), start? (s, default current time), captureSpeed? (%, 100), smoothing? (px, 1)}",
            has_comp,
            sketch
        ),
    ]
}
