//! The camera solve pipeline (see the module docs of [`super`]).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;

use super::bundle::{self, BaCam, BaObs, BaOpts, FocalMode};
use super::geometry::{self, Pose};
use super::linalg::{V3, add, log_rot, mm, mt, mtv, mv, norm, normalize, rodrigues, scale, slerp, sub, sym_eigen};
use super::{CameraSolve, CameraTracks, Distortion, ShotType, SolveMethod, SolveSettings, SolvedFrame, SolvedPoint};
use crate::fit::{Model, ransac};

fn debug() -> bool {
    std::env::var_os("EC_CAMTRACK_DEBUG").is_some()
}

/// Why a solve failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SolveError(pub String);

impl std::fmt::Display for SolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn err<T>(m: &str) -> Result<T, SolveError> {
    Err(SolveError(m.to_string()))
}

/// Tracks in solver form: pixels relative to the principal point.
struct Data {
    n: usize,
    w: f64,
    /// (track id, first frame, positions).
    tracks: Vec<(u32, usize, Vec<[f64; 2]>)>,
    /// Per frame: (track index, position).
    by_frame: Vec<Vec<(u32, [f64; 2])>>,
    /// Tracking noise scale (pixels): starts from the analysis resolution and is re-estimated
    /// from the residuals once the keyframes are solved.
    noise_bits: std::sync::atomic::AtomicU64,
}

// Input budgets apply before building the duplicated per-frame/per-track lookup tables.
const MAX_SOLVE_FRAMES: usize = 100_000;
const MAX_SOLVE_TRACKS: usize = 200_000;
const MAX_SOLVE_OBSERVATIONS: usize = 2_000_000;

fn validate_tracks(t: &CameraTracks) -> Result<(), SolveError> {
    if t.frames as usize > MAX_SOLVE_FRAMES || t.tracks.len() > MAX_SOLVE_TRACKS {
        return err("camera tracks exceed the supported frame or track count");
    }
    if t.size.iter().any(|v| !v.is_finite() || *v <= 0.0)
        || !t.factor.is_finite()
        || t.factor < 0.0
        || !t.start.is_finite()
        || !t.frame_duration.is_finite()
        || t.frame_duration <= 0.0
    {
        return err("camera tracks have invalid dimensions, timing or analysis scale");
    }
    let mut count = 0usize;
    for tr in &t.tracks {
        count = count
            .checked_add(tr.pts.len())
            .filter(|n| *n <= MAX_SOLVE_OBSERVATIONS)
            .ok_or_else(|| SolveError("camera tracks exceed the supported observation count".into()))?;
        let end = u32::try_from(tr.pts.len()).ok().and_then(|n| tr.start.checked_add(n));
        if end.is_none_or(|end| end > t.frames) {
            return err("camera track extends beyond the clip's frame range");
        }
        if tr.pts.iter().flatten().any(|v| !v.is_finite()) {
            return err("camera track positions must be finite");
        }
    }
    Ok(())
}

impl Data {
    fn new(t: &CameraTracks, deleted: &[u32]) -> Result<Data, SolveError> {
        validate_tracks(t)?;
        if deleted.len() > MAX_SOLVE_TRACKS {
            return err("too many deleted camera track ids");
        }
        let n = t.frames as usize;
        let c = [t.size[0] * 0.5, t.size[1] * 0.5];
        let del: HashSet<u32> = deleted.iter().copied().collect();
        let mut tracks = vec![];
        let mut by_frame = Vec::new();
        by_frame.try_reserve_exact(n).map_err(|_| SolveError("not enough memory for camera track frames".into()))?;
        by_frame.resize_with(n, Vec::new);
        for tr in t.tracks.iter().filter(|tr| !del.contains(&tr.id) && tr.pts.len() >= 3) {
            let ti = tracks.len() as u32;
            let pts: Vec<[f64; 2]> = tr.pts.iter().map(|p| [p[0] as f64 - c[0], p[1] as f64 - c[1]]).collect();
            for (k, p) in pts.iter().enumerate() {
                if let Some(v) = by_frame.get_mut(tr.start as usize + k) {
                    v.push((ti, *p));
                }
            }
            tracks.push((tr.id, tr.start as usize, pts));
        }
        let noise = (0.5 * t.factor).max(0.5);
        Ok(Data { n, w: t.size[0], tracks, by_frame, noise_bits: std::sync::atomic::AtomicU64::new(noise.to_bits()) })
    }
    fn at(&self, ti: u32, k: usize) -> Option<[f64; 2]> {
        let (_, s, p) = &self.tracks[ti as usize];
        k.checked_sub(*s).and_then(|i| p.get(i).copied())
    }
    fn noise(&self) -> f64 {
        f64::from_bits(self.noise_bits.load(Ordering::Relaxed))
    }
    fn set_noise(&self, v: f64) {
        self.noise_bits.store(v.to_bits(), Ordering::Relaxed);
    }
    fn inlier_thr(&self) -> f64 {
        4.0 * self.noise()
    }
    fn huber(&self) -> f64 {
        self.noise()
    }
}

/// Parallax-based keyframes: a new keyframe when the median feature motion since the last
/// exceeds 1.5 % of the width, when fewer than 70 % of its features survive, after 12 frames, or
/// before the shared features drop below 24. The first and last frames with features are
/// keyframes.
fn keyframes(d: &Data) -> Vec<usize> {
    let Some(first) = (0..d.n).find(|k| d.by_frame[*k].len() >= 8) else { return vec![] };
    let mut kf = vec![first];
    let mut last = first;
    let mut j = first + 1;
    while j < d.n {
        let base = &d.by_frame[last];
        let mut disp = vec![];
        for (ti, p) in base {
            if let Some(q) = d.at(*ti, j) {
                disp.push((q[0] - p[0]).hypot(q[1] - p[1]));
            }
        }
        if disp.len() < 24 && j > last + 1 {
            // Too few shared features: the previous frame becomes a keyframe.
            last = j - 1;
            kf.push(last);
            continue;
        }
        disp.sort_by(f64::total_cmp);
        let med = disp.get(disp.len() / 2).copied().unwrap_or(0.0);
        let ratio = disp.len() as f64 / base.len().max(1) as f64;
        if med > 0.015 * d.w || ratio < 0.7 || disp.len() < 24 || j - last >= 12 {
            last = j;
            kf.push(j);
        }
        j += 1;
    }
    if let Some(lastf) = (0..d.n).rev().find(|k| d.by_frame[*k].len() >= 8)
        && kf.last().is_some_and(|l| *l < lastf)
    {
        kf.push(lastf);
    }
    kf
}

/// A (partial) reconstruction.
#[derive(Clone, Default)]
struct Recon {
    cams: BTreeMap<usize, BaCam>,
    pts: HashMap<u32, V3>,
    /// Rejected observations (track index, frame).
    outliers: HashSet<(u32, usize)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    General,
    Tripod,
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct Cfg {
    kind: Kind,
    /// Focal mode of the global adjustments.
    focal: FocalMode,
    /// Registration estimates a focal length per frame.
    per_frame_f: bool,
    /// The global adjustments also adjust the lens distortion.
    distortion: bool,
}

const F_PRIOR: f64 = 30.0;

impl Recon {
    /// Observations among `frames` (inliers, with a 3D point).
    fn observations(&self, d: &Data, frames: &[usize]) -> (Vec<BaCam>, Vec<V3>, Vec<BaObs>, Vec<u32>) {
        let cams: Vec<BaCam> = frames.iter().map(|k| self.cams[k]).collect();
        let mut pidx: HashMap<u32, u32> = HashMap::new();
        let mut pts = vec![];
        let mut ids = vec![];
        let mut obs = vec![];
        for (ci, k) in frames.iter().enumerate() {
            for (ti, uv) in &d.by_frame[*k] {
                let Some(x) = self.pts.get(ti) else { continue };
                if self.outliers.contains(&(*ti, *k)) {
                    continue;
                }
                let pi = *pidx.entry(*ti).or_insert_with(|| {
                    pts.push(*x);
                    ids.push(*ti);
                    (pts.len() - 1) as u32
                });
                obs.push(BaObs { cam: ci as u32, pt: pi, uv: *uv });
            }
        }
        (cams, pts, obs, ids)
    }

    /// Bundle-adjust the cameras of `frames` (the first stays fixed) and their points.
    fn adjust(&mut self, d: &Data, frames: &[usize], cfg: &Cfg, focal: FocalMode, iters: usize) -> Result<f64, SolveError> {
        let (mut cams, mut pts, obs, ids) = self.observations(d, frames);
        if obs.is_empty() {
            return Ok(f64::INFINITY);
        }
        let mut fixed = vec![false; cams.len()];
        fixed[0] = true;
        let rep = bundle::adjust(
            &mut cams,
            &mut pts,
            &obs,
            &BaOpts { focal, fix_points: false, fix_centers: cfg.kind == Kind::Tripod, fixed, huber: d.huber(), max_iter: iters, distortion: cfg.distortion },
        )?;
        for (k, c) in frames.iter().zip(&cams) {
            self.cams.insert(*k, *c);
        }
        for (ti, x) in ids.iter().zip(&pts) {
            self.pts.insert(*ti, *x);
        }
        Ok(rep.cost)
    }

    /// Mark observations above the inlier threshold as outliers; drop points left with fewer
    /// than two observations among the registered frames.
    fn reject(&mut self, d: &Data) {
        let thr2 = d.inlier_thr().powi(2);
        let mut count: HashMap<u32, usize> = HashMap::new();
        for (k, c) in &self.cams {
            for (ti, uv) in &d.by_frame[*k] {
                let Some(x) = self.pts.get(ti) else { continue };
                if self.outliers.contains(&(*ti, *k)) {
                    continue;
                }
                match bundle::residual(c, *x, *uv) {
                    Some(r) if r[0] * r[0] + r[1] * r[1] <= thr2 => *count.entry(*ti).or_default() += 1,
                    _ => {
                        self.outliers.insert((*ti, *k));
                    }
                }
            }
        }
        self.pts.retain(|ti, _| count.get(ti).copied().unwrap_or(0) >= 2);
    }

    /// Triangulate tracks seen on ≥ 2 registered frames that have no point yet.
    fn triangulate(&mut self, d: &Data, kind: Kind, min_angle_deg: f64, check: f64) {
        let mut seen: HashMap<u32, Vec<usize>> = HashMap::new();
        for k in self.cams.keys() {
            for (ti, _) in &d.by_frame[*k] {
                if !self.pts.contains_key(ti) {
                    seen.entry(*ti).or_default().push(*k);
                }
            }
        }
        let thr2 = (d.inlier_thr() * check).powi(2);
        let new: Vec<(u32, V3)> = seen
            .par_iter()
            .filter(|(_, ks)| ks.len() >= 2)
            .filter_map(|(ti, ks)| {
                let views: Vec<(Pose, [f64; 2], &BaCam)> = ks
                    .iter()
                    .map(|k| {
                        let c = &self.cams[k];
                        let uv = d.at(*ti, *k)?;
                        Some((c.pose, [uv[0] / c.f, uv[1] / c.f], c))
                    })
                    .collect::<Option<_>>()?;
                let x = match kind {
                    Kind::Tripod => normalize(views.iter().fold([0.0; 3], |a, (p, o, _)| add(a, normalize(mtv(&p.r, [o[0], o[1], 1.0]))))),
                    Kind::General => {
                        let v: Vec<(Pose, [f64; 2])> = views.iter().map(|(p, o, _)| (*p, *o)).collect();
                        let x = geometry::triangulate(&v)?;
                        // Enough parallax between the most separated views.
                        let (a, b) = (views[0].0.c, views[views.len() - 1].0.c);
                        if geometry::ray_angle(a, b, x).to_degrees() < min_angle_deg {
                            return None;
                        }
                        x
                    }
                };
                for (k, (_, _, c)) in ks.iter().zip(&views) {
                    let uv = d.at(*ti, *k)?;
                    let r = bundle::residual(c, x, uv)?;
                    if r[0] * r[0] + r[1] * r[1] > thr2 {
                        return None;
                    }
                }
                Some((*ti, x))
            })
            .collect();
        self.pts.extend(new);
    }

    /// Resect frame `k` from the existing points starting at `init` (pose, and the focal
    /// length when `per_frame_f`). Returns false when too few points agree.
    fn register(&mut self, d: &Data, k: usize, init: BaCam, cfg: &Cfg) -> bool {
        let mut pts = vec![];
        let mut obs = vec![];
        let mut ids = vec![];
        for (ti, uv) in &d.by_frame[k] {
            if let Some(x) = self.pts.get(ti) {
                obs.push(BaObs { cam: 0, pt: pts.len() as u32, uv: *uv });
                pts.push(*x);
                ids.push(*ti);
            }
        }
        if obs.len() < 6 {
            return false;
        }
        let thr2 = d.inlier_thr().powi(2);
        let opts = |huber: f64| BaOpts {
            focal: if cfg.per_frame_f { FocalMode::PerCamera(0.0) } else { FocalMode::Fixed },
            fix_points: true,
            fix_centers: cfg.kind == Kind::Tripod,
            fixed: vec![false],
            huber,
            max_iter: 40,
            distortion: false,
        };
        let mut best: Option<(usize, BaCam)> = None;
        // Robust start (large Huber threshold), then tighten; from the prediction and, failing
        // that, from the nearest registered camera.
        let nearest = self.cams.iter().min_by_key(|(c, _)| c.abs_diff(k)).map(|(_, c)| *c);
        for start in std::iter::once(init).chain(nearest) {
            if best.as_ref().is_some_and(|b| b.0 as f64 >= 0.8 * obs.len() as f64) {
                break;
            }
            let mut cams = vec![start];
            if bundle::adjust(&mut cams, &mut pts.clone(), &obs, &opts(d.inlier_thr() * 4.0)).is_err() {
                return false;
            }
            if bundle::adjust(&mut cams, &mut pts.clone(), &obs, &opts(d.huber())).is_err() {
                return false;
            }
            let inl = obs.iter().filter(|o| bundle::residual(&cams[0], pts[o.pt as usize], o.uv).is_some_and(|r| r[0] * r[0] + r[1] * r[1] <= thr2)).count();
            if best.as_ref().is_none_or(|b| inl > b.0) {
                best = Some((inl, cams[0]));
            }
        }
        let Some((inl, cam)) = best else { return false };
        if inl < 6 || (inl as f64) < 0.5 * obs.len() as f64 {
            return false;
        }
        // Refine on the inliers only.
        let keep: Vec<BaObs> =
            obs.iter().filter(|o| bundle::residual(&cam, pts[o.pt as usize], o.uv).is_some_and(|r| r[0] * r[0] + r[1] * r[1] <= thr2)).copied().collect();
        let mut cams = vec![cam];
        if bundle::adjust(&mut cams, &mut pts.clone(), &keep, &opts(d.huber())).is_err() {
            return false;
        }
        for o in &obs {
            let ok = bundle::residual(&cams[0], pts[o.pt as usize], o.uv).is_some_and(|r| r[0] * r[0] + r[1] * r[1] <= thr2);
            if !ok {
                self.outliers.insert((ids[o.pt as usize], k));
            }
        }
        self.cams.insert(k, cams[0]);
        true
    }

    /// How well the reconstruction explains every observation on its frames: the mean of the
    /// reprojection error truncated at the inlier threshold, an observation without a point
    /// counting as the threshold.
    fn score(&self, d: &Data) -> f64 {
        let thr = d.inlier_thr();
        let mut s = 0.0;
        let mut n = 0usize;
        for (k, c) in &self.cams {
            for (ti, uv) in &d.by_frame[*k] {
                n += 1;
                s += match self.pts.get(ti).and_then(|x| bundle::residual(c, *x, *uv)) {
                    Some(r) => r[0].hypot(r[1]).min(thr),
                    None => thr,
                };
            }
        }
        if n > 0 { s / n as f64 } else { f64::INFINITY }
    }

    /// Average reprojection error (pixels) and observation count over the inliers.
    fn error(&self, d: &Data) -> (f64, usize) {
        let mut s = 0.0;
        let mut n = 0;
        for (k, c) in &self.cams {
            for (ti, uv) in &d.by_frame[*k] {
                let Some(x) = self.pts.get(ti) else { continue };
                if self.outliers.contains(&(*ti, *k)) {
                    continue;
                }
                if let Some(r) = bundle::residual(c, *x, *uv) {
                    s += r[0].hypot(r[1]);
                    n += 1;
                }
            }
        }
        (if n > 0 { s / n as f64 } else { f64::INFINITY }, n)
    }
}

/// Interpolated / extrapolated starting camera for frame `k` from the registered ones.
fn predict(r: &Recon, k: usize) -> Option<BaCam> {
    let before = r.cams.range(..k).next_back().map(|(a, b)| (*a, *b));
    let after = r.cams.range(k + 1..).next().map(|(a, b)| (*a, *b));
    match (before, after) {
        (Some((ka, a)), Some((kb, b))) => {
            let t = (k - ka) as f64 / (kb - ka) as f64;
            Some(BaCam {
                pose: Pose { r: slerp(&a.pose.r, &b.pose.r, t), c: add(scale(a.pose.c, 1.0 - t), scale(b.pose.c, t)) },
                f: a.f * (b.f / a.f).powf(t),
                dist: a.dist,
            })
        }
        (Some((ka, a)), None) => {
            // Constant velocity from the two previous cameras.
            match r.cams.range(..ka).next_back() {
                Some((kp, p)) => {
                    let t = (k - ka) as f64 / (ka - kp) as f64;
                    let dr = log_rot(&mm(&a.pose.r, &mt(&p.pose.r)));
                    let rr = mm(&rodrigues(scale(dr, t.min(2.0))), &a.pose.r);
                    let c = add(a.pose.c, scale(sub(a.pose.c, p.pose.c), t.min(2.0)));
                    Some(BaCam { pose: Pose { r: rr, c }, f: a.f, dist: a.dist })
                }
                None => Some(a),
            }
        }
        (None, Some((kb, b))) => match r.cams.range(kb + 1..).next() {
            Some((kn, nx)) => {
                let t = (kb - k) as f64 / (kn - kb) as f64;
                let dr = log_rot(&mm(&b.pose.r, &mt(&nx.pose.r)));
                let rr = mm(&rodrigues(scale(dr, t.min(2.0))), &b.pose.r);
                let c = add(b.pose.c, scale(sub(b.pose.c, nx.pose.c), t.min(2.0)));
                Some(BaCam { pose: Pose { r: rr, c }, f: b.f, dist: b.dist })
            }
            None => Some(b),
        },
        (None, None) => None,
    }
}

/// Shared tracks of frames `a` and `b`: (track index, pixels in a, pixels in b).
fn shared(d: &Data, a: usize, b: usize) -> Vec<(u32, [f64; 2], [f64; 2])> {
    d.by_frame[a].iter().filter_map(|(ti, p)| d.at(*ti, b).map(|q| (*ti, *p, q))).collect()
}

/// Two-view initialisations of frames `a`, `b` at focal length `f`: the second camera and the
/// triangulated points (the first camera is the identity).
fn init_pair(d: &Data, a: usize, b: usize, f: f64, use_e: bool, use_h: bool) -> Vec<(Pose, HashMap<u32, V3>)> {
    let sh = shared(d, a, b);
    if sh.len() < 12 {
        return vec![];
    }
    let x1: Vec<[f64; 2]> = sh.iter().map(|s| [s.1[0] / f, s.1[1] / f]).collect();
    let x2: Vec<[f64; 2]> = sh.iter().map(|s| [s.2[0] / f, s.2[1] / f]).collect();
    let thr = 2.0 * d.noise() / f;
    let mut cands: Vec<Pose> = vec![];
    if use_e && let Some((e, inl)) = geometry::ransac_essential(&x1, &x2, thr, 400, (a * 7919 + b) as u64) {
        let (p1, p2): (Vec<[f64; 2]>, Vec<[f64; 2]>) = (0..x1.len()).filter(|i| inl[*i]).map(|i| (x1[i], x2[i])).unzip();
        if let Some((pose, _)) = geometry::select_pose(&geometry::decompose_essential(&e), &p1, &p2) {
            cands.push(pose);
        }
    }
    if use_h {
        let (s1, s2): (Vec<[f64; 2]>, Vec<[f64; 2]>) = sh.iter().map(|s| (s.1, s.2)).unzip();
        if let Some(fit) = ransac(Model::Homography, &s1, &s2, 2.0 * d.noise(), 500, (a * 31 + b) as u64) {
            let h = fit.h.0;
            // Normalised: K⁻¹ H K with K = diag(f, f, 1).
            let hn = [[h[0][0], h[0][1], h[0][2] / f], [h[1][0], h[1][1], h[1][2] / f], [h[2][0] * f, h[2][1] * f, h[2][2]]];
            let (p1, p2): (Vec<[f64; 2]>, Vec<[f64; 2]>) = (0..x1.len()).filter(|i| fit.inliers[*i]).map(|i| (x1[i], x2[i])).unzip();
            for (r, td, _) in geometry::decompose_homography(&hn, &p1, &p2) {
                if norm(td) > 1e-6 {
                    cands.push(Pose::from_rt(r, normalize(td)));
                }
            }
        }
    }
    let thr2 = (d.inlier_thr() / f).powi(2);
    cands
        .into_iter()
        .filter_map(|pose| {
            let mut pts = HashMap::new();
            for (i, s) in sh.iter().enumerate() {
                let Some(x) = geometry::triangulate(&[(Pose::IDENTITY, x1[i]), (pose, x2[i])]) else { continue };
                let ok = geometry::reproj2(&Pose::IDENTITY, x, x1[i]).is_some_and(|e| e < thr2)
                    && geometry::reproj2(&pose, x, x2[i]).is_some_and(|e| e < thr2)
                    && geometry::ray_angle([0.0; 3], pose.c, x).to_degrees() > 0.3;
                if ok {
                    pts.insert(s.0, x);
                }
            }
            (pts.len() >= 10 && pts.len() * 3 >= sh.len()).then_some((pose, pts))
        })
        .collect()
}

/// Grow a reconstruction over keyframes `kfs` from an initial pair (`kfs[0]`, `kfs[j]`).
fn grow(d: &Data, kfs: &[usize], j: usize, init: &(Pose, HashMap<u32, V3>), f: f64, cfg: &Cfg, cancel: Option<&AtomicBool>) -> Option<Recon> {
    let (a, b) = (kfs[0], kfs[j]);
    let mut r = Recon::default();
    r.cams.insert(a, BaCam { pose: Pose::IDENTITY, f, dist: Distortion::NONE });
    r.cams.insert(b, BaCam { pose: init.0, f, dist: Distortion::NONE });
    r.pts = init.1.clone();
    let inner = |r: &mut Recon, frames: &[usize]| {
        let focal = if cfg.per_frame_f && frames.len() > 2 { FocalMode::PerCamera(F_PRIOR) } else { FocalMode::Fixed };
        r.adjust(d, frames, cfg, focal, 10)?;
        r.reject(d);
        Ok::<(), SolveError>(())
    };
    inner(&mut r, &[a, b]).ok()?;
    let order: Vec<usize> = kfs[1..j].iter().chain(&kfs[j + 1..]).copied().collect();
    let mut done = 0;
    for k in order {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return None;
        }
        let start = predict(&r, k)?;
        if !r.register(d, k, start, cfg) {
            // A failed keyframe ends the reconstruction (a later frame is too far from the known
            // points); the caller resects the rest from what is known.
            break;
        }
        r.triangulate(d, cfg.kind, 1.0, 1.0);
        done += 1;
        let frames: Vec<usize> = r.cams.keys().copied().collect();
        if done % 3 == 0 || frames.len() <= 6 {
            inner(&mut r, &frames).ok()?;
        }
    }
    let frames: Vec<usize> = r.cams.keys().copied().collect();
    inner(&mut r, &frames).ok()?;
    (r.cams.len() >= 2 && r.pts.len() >= 8).then_some(r)
}

/// Tripod reconstruction over keyframes `kfs` at focal length `f`.
fn grow_tripod(d: &Data, kfs: &[usize], f: f64, cfg: &Cfg, cancel: Option<&AtomicBool>) -> Option<Recon> {
    let mut r = Recon::default();
    r.cams.insert(kfs[0], BaCam { pose: Pose::IDENTITY, f, dist: Distortion::NONE });
    let ray = |p: [f64; 2], f: f64| normalize([p[0] / f, p[1] / f, 1.0]);
    for w in kfs.windows(2) {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return None;
        }
        let (a, b) = (w[0], w[1]);
        let sh = shared(d, a, b);
        if sh.len() < 6 {
            break;
        }
        let ra: Vec<V3> = sh.iter().map(|s| ray(s.1, f)).collect();
        let rb: Vec<V3> = sh.iter().map(|s| ray(s.2, f)).collect();
        let mut rel = geometry::procrustes(&ra, &rb)?;
        // One round of outlier trimming.
        let errs: Vec<f64> = ra.iter().zip(&rb).map(|(x, y)| norm(sub(*y, mv(&rel, *x)))).collect();
        let mut sorted = errs.clone();
        sorted.sort_by(f64::total_cmp);
        let lim = (sorted[sorted.len() / 2] * 3.0).max(d.inlier_thr() / f);
        let (ia, ib): (Vec<V3>, Vec<V3>) = ra.iter().zip(&rb).zip(&errs).filter(|(_, e)| **e <= lim).map(|((x, y), _)| (*x, *y)).unzip();
        if ia.len() >= 4 {
            rel = geometry::procrustes(&ia, &ib)?;
        }
        let pa = r.cams[&a].pose.r;
        r.cams.insert(b, BaCam { pose: Pose { r: mm(&rel, &pa), c: [0.0; 3] }, f, dist: Distortion::NONE });
    }
    r.triangulate(d, Kind::Tripod, 0.0, 20.0);
    let frames: Vec<usize> = r.cams.keys().copied().collect();
    let focal = if cfg.per_frame_f && frames.len() > 2 { FocalMode::PerCamera(F_PRIOR) } else { FocalMode::Fixed };
    r.adjust(d, &frames, cfg, focal, 15).ok()?;
    r.reject(d);
    r.triangulate(d, Kind::Tripod, 0.0, 1.0);
    r.adjust(d, &frames, cfg, focal, 15).ok()?;
    (r.cams.len() >= 2 && r.pts.len() >= 8).then_some(r)
}

/// Focal length (pixels) of a horizontal angle of view.
fn focal_of(w: f64, hfov: f64) -> f64 {
    w * 0.5 / (hfov.clamp(1.0, 170.0).to_radians() * 0.5).tan()
}

struct Attempt {
    recon: Recon,
    err: f64,
    method: SolveMethod,
}

/// Choose the initial pair: the first keyframe and the later keyframe (among the next few)
/// that keeps the most points while having enough parallax.
fn best_init(d: &Data, kfs: &[usize], f: f64, method: SolveMethod) -> Option<(usize, (Pose, HashMap<u32, V3>), bool)> {
    let (use_e, use_h) = match method {
        SolveMethod::Typical => (true, false),
        SolveMethod::MostlyFlat => (false, true),
        _ => (true, true),
    };
    let mut best: Option<(f64, usize, (Pose, HashMap<u32, V3>), bool)> = None;
    for j in 1..kfs.len().min(6) {
        let cands = init_pair(d, kfs[0], kfs[j], f, use_e, false);
        let hc = if use_h { init_pair(d, kfs[0], kfs[j], f, false, true) } else { vec![] };
        for (c, is_h) in cands.into_iter().map(|c| (c, false)).chain(hc.into_iter().map(|c| (c, true))) {
            // Score: points × median triangulation angle (capped).
            let mut ang: Vec<f64> = c.1.values().map(|x| geometry::ray_angle([0.0; 3], c.0.c, *x).to_degrees()).collect();
            ang.sort_by(f64::total_cmp);
            let med = ang.get(ang.len() / 2).copied().unwrap_or(0.0);
            let score = c.1.len() as f64 * med.min(4.0);
            if best.as_ref().is_none_or(|b| score > b.0) {
                best = Some((score, j, c, is_h));
            }
        }
    }
    best.map(|b| (b.1, b.2, b.3))
}

/// All initial candidates for the pair chosen by [`best_init`] (E and H decompositions).
fn init_candidates(d: &Data, kfs: &[usize], f: f64, method: SolveMethod) -> Vec<(usize, (Pose, HashMap<u32, V3>), bool)> {
    let Some((j, first, is_h)) = best_init(d, kfs, f, method) else { return vec![] };
    let mut out = vec![(j, first, is_h)];
    if matches!(method, SolveMethod::Auto | SolveMethod::MostlyFlat) {
        for c in init_pair(d, kfs[0], kfs[j], f, false, true) {
            if out.iter().all(|o| norm(log_rot(&mm(&o.1.0.r, &mt(&c.0.r)))) > 1e-3 || norm(sub(o.1.0.c, c.0.c)) > 1e-3) {
                out.push((j, c, true));
            }
        }
    }
    out
}

/// A general (translating camera) reconstruction of keyframes `kfs` at focal length `f`.
fn general(d: &Data, kfs: &[usize], f: f64, method: SolveMethod, cfg: &Cfg, cancel: Option<&AtomicBool>) -> Option<Attempt> {
    let cands = init_candidates(d, kfs, f, method);
    let res: Vec<Attempt> = cands
        .par_iter()
        .filter_map(|(j, init, is_h)| {
            let r = grow(d, kfs, *j, init, f, cfg, cancel)?;
            let (_, n) = r.error(d);
            // Prefer reconstructions covering more keyframes.
            let cover = r.cams.len() as f64 / kfs.len() as f64;
            (n > 0).then(|| Attempt { err: r.score(d) / cover.max(0.05), recon: r, method: if *is_h { SolveMethod::MostlyFlat } else { SolveMethod::Typical } })
        })
        .collect();
    res.into_iter().min_by(|a, b| a.err.total_cmp(&b.err))
}

fn tripod(d: &Data, kfs: &[usize], f: f64, cfg: &Cfg, cancel: Option<&AtomicBool>) -> Option<Attempt> {
    let r = grow_tripod(d, kfs, f, cfg, cancel)?;
    let cover = r.cams.len() as f64 / kfs.len() as f64;
    Some(Attempt { err: r.score(d) / cover.max(0.05), recon: r, method: SolveMethod::TripodPan })
}

/// Search the focal length minimising `eval(f)` (a reprojection error): a log-spaced scan of
/// horizontal angles of view, then golden-section refinement around the best.
fn search_focal(w: f64, eval: &(dyn Fn(f64) -> f64 + Sync)) -> f64 {
    let fovs = [8.0, 12.0, 16.0, 20.0, 25.0, 30.0, 36.0, 43.0, 51.0, 60.0, 70.0, 82.0, 96.0, 112.0];
    let fs: Vec<f64> = fovs.iter().map(|a| focal_of(w, *a)).collect();
    let errs: Vec<f64> = fs.par_iter().map(|f| eval(*f)).collect();
    if debug() {
        eprintln!("focal scan: {:?}", fs.iter().zip(&errs).map(|(f, e)| format!("{f:.0}:{e:.3}")).collect::<Vec<_>>());
    }
    let i = (0..fs.len()).min_by(|a, b| errs[*a].total_cmp(&errs[*b])).unwrap_or(5);
    let (mut lo, mut hi) = (fs[(i + 1).min(fs.len() - 1)].ln(), fs[i.saturating_sub(1)].ln());
    if lo > hi {
        std::mem::swap(&mut lo, &mut hi);
    }
    let g = 0.618_033_988_75;
    let (mut x1, mut x2) = (hi - g * (hi - lo), lo + g * (hi - lo));
    let (mut e1, mut e2) = rayon::join(|| eval(x1.exp()), || eval(x2.exp()));
    for _ in 0..7 {
        if e1 < e2 {
            hi = x2;
            x2 = x1;
            e2 = e1;
            x1 = hi - g * (hi - lo);
            e1 = eval(x1.exp());
        } else {
            lo = x1;
            x1 = x2;
            e1 = e2;
            x2 = lo + g * (hi - lo);
            e2 = eval(x2.exp());
        }
    }
    let best = if e1 < e2 { (x1, e1) } else { (x2, e2) };
    if best.1 <= errs[i] { best.0.exp() } else { fs[i] }
}

/// A solve before its canonical output.
struct Core {
    d: Data,
    r: Recon,
    method: SolveMethod,
    kind: Kind,
    cfg: Cfg,
    focal: FocalMode,
    kfs: Vec<usize>,
}

/// `tracks` with every position undistorted by `dist`.
fn undistorted(tracks: &CameraTracks, dist: &Distortion) -> CameraTracks {
    let c = [tracks.size[0] * 0.5, tracks.size[1] * 0.5];
    let mut t = tracks.clone();
    for tr in t.tracks.iter_mut() {
        for p in tr.pts.iter_mut() {
            let q = dist.undistort([p[0] as f64 - c[0], p[1] as f64 - c[1]]);
            *p = [(q[0] + c[0]) as f32, (q[1] + c[1]) as f32];
        }
    }
    t
}

/// Solve the camera of a tracked clip.
pub fn solve(tracks: &CameraTracks, s: &SolveSettings, cancel: Option<&AtomicBool>) -> Result<CameraSolve, SolveError> {
    if !s.lens_distortion {
        let c = solve_core(tracks, s, cancel)?;
        return finish(&c.d, tracks, s, c.r, c.method, c.kind);
    }
    // Lens distortion: solve, adjust (k1, k2) jointly on the raw tracks, then solve again on
    // tracks undistorted with the estimate and adjust jointly once more.
    let mut dist = Distortion::for_size(tracks.size);
    let mut out = None;
    for pass in 0..2 {
        let und = (pass > 0).then(|| undistorted(tracks, &dist));
        let c = solve_core(und.as_ref().unwrap_or(tracks), s, cancel)?;
        let d = Data::new(tracks, &s.deleted)?;
        d.set_noise(c.d.noise());
        let mut r = c.r;
        for cam in r.cams.values_mut() {
            cam.dist = dist;
        }
        let cfg = Cfg { distortion: true, ..c.cfg };
        let all: Vec<usize> = r.cams.keys().copied().collect();
        let frames: Vec<usize> = if all.len() <= 160 { all.clone() } else { c.kfs.iter().copied().filter(|k| r.cams.contains_key(k)).collect() };
        if frames.len() < 2 {
            return err("the camera could not be solved (not enough parallax or features)");
        }
        // The pinhole solve's outliers include the distorted frame edges: start from all
        // observations (the Huber loss keeps real outliers in check), then reject again.
        r.outliers.clear();
        r.adjust(&d, &frames, &cfg, c.focal, 30)?;
        r.outliers.clear();
        r.reject(&d);
        r.adjust(&d, &frames, &cfg, c.focal, 30)?;
        dist = r.cams[&frames[0]].dist;
        for cam in r.cams.values_mut() {
            cam.dist = dist;
        }
        if frames.len() < all.len() {
            // Refine the other frames against the adjusted points and distortion.
            let base = r.clone();
            let rcfg = Cfg { distortion: false, ..cfg };
            let refined: Vec<(usize, Option<BaCam>)> = all
                .par_iter()
                .filter(|k| !frames.contains(k))
                .map(|k| {
                    let mut local = Recon { cams: base.cams.clone(), pts: base.pts.clone(), outliers: HashSet::new() };
                    let start = local.cams[k];
                    let ok = local.register(&d, *k, start, &rcfg);
                    (*k, if ok { local.cams.get(k).copied() } else { None })
                })
                .collect();
            for (k, cam) in refined {
                if let Some(cam) = cam {
                    r.cams.insert(k, cam);
                }
            }
            r.reject(&d);
        }
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return err("cancelled");
        }
        if debug() {
            eprintln!("lens distortion pass {pass}: k1 {:.5} k2 {:.5}", dist.k1, dist.k2);
        }
        let mut sol = finish(&d, tracks, s, r, c.method, c.kind)?;
        sol.distortion = Some(dist);
        out = Some(sol);
    }
    out.ok_or_else(|| SolveError("the camera could not be solved".into()))
}

fn solve_core(tracks: &CameraTracks, s: &SolveSettings, cancel: Option<&AtomicBool>) -> Result<Core, SolveError> {
    let d = Data::new(tracks, &s.deleted)?;
    if d.n < 2 {
        return err("the clip must be at least two frames long");
    }
    let kfs = keyframes(&d);
    if kfs.len() < 2 {
        return err("not enough features were tracked to solve the camera");
    }
    let cancelled = || cancel.is_some_and(|c| c.load(Ordering::Relaxed));
    let fixed_f = (s.shot == ShotType::SpecifyAngle).then(|| focal_of(d.w, if s.hfov > 0.0 { s.hfov } else { 40.0 }));
    let variable = s.shot == ShotType::VariableZoom;
    let gcfg = Cfg { kind: Kind::General, focal: FocalMode::Fixed, per_frame_f: false, distortion: false };
    let tcfg = Cfg { kind: Kind::Tripod, ..gcfg };
    // The focal-length search runs on the first keyframes only.
    let mini: Vec<usize> = kfs.iter().copied().take(10).collect();
    let try_general = matches!(s.method, SolveMethod::Auto | SolveMethod::Typical | SolveMethod::MostlyFlat);
    let try_tripod = matches!(s.method, SolveMethod::Auto | SolveMethod::TripodPan);

    let t0 = web_time::Instant::now();
    let lap = |what: &str| {
        if debug() {
            eprintln!("{what}: {:.2} s", t0.elapsed().as_secs_f64());
        }
    };
    let mut results: Vec<(Attempt, f64)> = vec![];
    if try_general {
        let f = match fixed_f {
            Some(f) => f,
            None => {
                search_focal(d.w, &|f| general(&d, &mini, f, s.method, &Cfg { per_frame_f: variable, ..gcfg }, cancel).map(|a| a.err).unwrap_or(f64::INFINITY))
            }
        };
        if cancelled() {
            return err("cancelled");
        }
        if let Some(a) = general(&d, &kfs, f, s.method, &Cfg { per_frame_f: variable, ..gcfg }, cancel) {
            results.push((a, f));
        }
    }
    lap("general");
    // Auto Detect: a quick tripod check at the general solve's focal length; when a rotation
    // clearly cannot explain the tracks, the tripod search is skipped.
    let tripod_plausible = match (s.method, results.first()) {
        (SolveMethod::Auto, Some((g, f))) => {
            let quick = tripod(&d, &mini, *f, &tcfg, cancel).map(|a| a.err).unwrap_or(f64::INFINITY);
            if debug() {
                eprintln!("quick tripod check: {quick:.3} vs general {:.3}", g.err);
            }
            quick <= 3.0 * g.err + d.noise()
        }
        _ => true,
    };
    if try_tripod && tripod_plausible && !cancelled() {
        let f = match fixed_f {
            Some(f) => f,
            None => search_focal(d.w, &|f| tripod(&d, &mini, f, &Cfg { per_frame_f: variable, ..tcfg }, cancel).map(|a| a.err).unwrap_or(f64::INFINITY)),
        };
        if let Some(a) = tripod(&d, &kfs, f, &Cfg { per_frame_f: variable, ..tcfg }, cancel) {
            results.push((a, f));
        }
    }
    if cancelled() {
        return err("cancelled");
    }
    // Auto Detect keeps the tripod model when it explains the tracks about as well.
    let pick = {
        let g = results.iter().position(|r| r.0.method != SolveMethod::TripodPan);
        let t = results.iter().position(|r| r.0.method == SolveMethod::TripodPan);
        match (g, t) {
            (Some(g), Some(t)) => {
                let (eg, et) = (results[g].0.err, results[t].0.err);
                // A general solve whose camera hardly moves relative to the scene depth is a pan.
                let rg = &results[g].0.recon;
                let cs: Vec<V3> = rg.cams.values().map(|c| c.pose.c).collect();
                let base = cs.iter().flat_map(|a| cs.iter().map(move |b| norm(sub(*a, *b)))).fold(0.0, f64::max);
                let c0 = rg.cams.values().next().map_or([0.0; 3], |c| c.pose.c);
                let mut depth: Vec<f64> = rg.pts.values().map(|x| norm(sub(*x, c0))).collect();
                depth.sort_by(f64::total_cmp);
                let med = depth.get(depth.len() / 2).copied().unwrap_or(1.0);
                if debug() {
                    eprintln!("auto: general {eg:.4} tripod {et:.4} baseline/depth {:.4}", base / med);
                }
                if et <= (eg * 1.25).max(eg + 0.05 * d.noise()) || base / med < 0.01 { t } else { g }
            }
            (Some(g), None) => g,
            (None, Some(t)) => t,
            (None, None) => return err("the camera could not be solved (not enough parallax or features)"),
        }
    };
    if debug() {
        for (a, f) in &results {
            eprintln!("candidate {:?}: err {:.4} f {f:.1} cams {} pts {}", a.method, a.err, a.recon.cams.len(), a.recon.pts.len());
        }
    }
    let (att, _) = results.swap_remove(pick);
    let kind = if att.method == SolveMethod::TripodPan { Kind::Tripod } else { Kind::General };
    let cfg = Cfg { kind, focal: FocalMode::Fixed, per_frame_f: variable, distortion: false };
    let mut r = att.recon;
    let focal = match (fixed_f, variable) {
        (Some(_), _) => FocalMode::Fixed,
        (None, true) => FocalMode::PerCamera(F_PRIOR),
        (None, false) => FocalMode::Shared,
    };
    lap("tripod");
    // Keyframes: adjust with the focal length free, reject, re-triangulate, adjust again.
    let kf_frames: Vec<usize> = r.cams.keys().copied().collect();
    r.adjust(&d, &kf_frames, &cfg, focal, 40)?;
    r.reject(&d);
    // Re-estimate the tracking noise from the inlier residuals (robustly, from their median:
    // for 2D Gaussian noise of deviation σ the median distance is 1.1774 σ), then tighten the
    // thresholds: observations of moving objects and occlusion edges become outliers.
    let mut res: Vec<f64> = vec![];
    for (k, c) in &r.cams {
        for (ti, uv) in &d.by_frame[*k] {
            if r.outliers.contains(&(*ti, *k)) {
                continue;
            }
            if let Some(e) = r.pts.get(ti).and_then(|x| bundle::residual(c, *x, *uv)) {
                res.push(e[0].hypot(e[1]));
            }
        }
    }
    if res.len() > 20 {
        res.sort_by(f64::total_cmp);
        let sigma = res[res.len() / 2] / 1.1774;
        let floor = 0.05 * tracks.factor.max(1.0);
        d.set_noise((1.5 * sigma).clamp(floor, d.noise()));
        if debug() {
            eprintln!("noise: σ {sigma:.3} → {:.3}", d.noise());
        }
        r.outliers.clear();
        r.reject(&d);
    }
    r.triangulate(&d, kind, 1.0, 1.0);
    r.adjust(&d, &kf_frames, &cfg, focal, 40)?;
    r.reject(&d);
    r.adjust(&d, &kf_frames, &cfg, focal, 20)?;
    if cancelled() {
        return err("cancelled");
    }
    lap("keyframe adjustment");
    // Every other frame: resect from the points (nearest registered camera first).
    let mut todo: Vec<usize> = (0..d.n).filter(|k| !r.cams.contains_key(k)).collect();
    todo.sort_by_key(|k| r.cams.keys().map(|c| c.abs_diff(*k)).min().unwrap_or(usize::MAX));
    let base = r.clone();
    let resected: Vec<(usize, Option<BaCam>)> = todo
        .par_iter()
        .map(|k| {
            let mut local = Recon { cams: base.cams.clone(), pts: base.pts.clone(), outliers: HashSet::new() };
            let ok = predict(&local, *k).is_some_and(|start| local.register(&d, *k, start, &cfg));
            (*k, if ok { local.cams.get(k).copied() } else { None })
        })
        .collect();
    for (k, c) in resected {
        if let Some(c) = c {
            r.cams.insert(k, c);
        }
    }
    if cancelled() {
        return err("cancelled");
    }
    lap("resection");
    // Points for the tracks that only lived between keyframes, then the final adjustment over
    // every frame (when that stays affordable).
    r.triangulate(&d, kind, 1.0, 1.0);
    let all: Vec<usize> = r.cams.keys().copied().collect();
    r.reject(&d);
    if all.len() <= 160 {
        r.adjust(&d, &all, &cfg, focal, 25)?;
        r.reject(&d);
    } else {
        // Points only (cameras fixed) would be cheap but the keyframe solution is already
        // adjusted; refine each non-keyframe once more against the final points.
        let base = r.clone();
        let refined: Vec<(usize, Option<BaCam>)> = all
            .par_iter()
            .filter(|k| !kf_frames.contains(k))
            .map(|k| {
                let mut local = Recon { cams: base.cams.clone(), pts: base.pts.clone(), outliers: HashSet::new() };
                let start = local.cams[k];
                let ok = local.register(&d, *k, start, &cfg);
                (*k, if ok { local.cams.get(k).copied() } else { None })
            })
            .collect();
        for (k, c) in refined.into_iter() {
            if let Some(c) = c {
                r.cams.insert(k, c);
            }
        }
        r.reject(&d);
    }
    // Method Used: the chosen model; Auto Detect calls a general solve "Mostly Flat Scene" when
    // its points are close to one plane.
    let method = match (s.method, att.method) {
        (_, SolveMethod::TripodPan) => SolveMethod::TripodPan,
        (SolveMethod::Auto, _) => {
            let p: Vec<V3> = r.pts.values().copied().collect();
            let flat = cov_ratio(&p) < 0.08;
            if flat { SolveMethod::MostlyFlat } else { SolveMethod::Typical }
        }
        (m, _) => m,
    };
    lap("final adjustment");
    Ok(Core { d, r, method, kind, cfg, focal, kfs: kf_frames })
}

/// Thickness of a point cloud: √(smallest / middle eigenvalue of its covariance).
fn cov_ratio(p: &[V3]) -> f64 {
    if p.len() < 3 {
        return 1.0;
    }
    let k = 1.0 / p.len() as f64;
    let c = scale(p.iter().fold([0.0; 3], |a, b| add(a, *b)), k);
    let mut cov = [0.0; 9];
    for q in p {
        let d = sub(*q, c);
        for i in 0..3 {
            for j in 0..3 {
                cov[i * 3 + j] += d[i] * d[j] * k;
            }
        }
    }
    let (vals, _) = sym_eigen(&cov, 3);
    (vals[0].max(0.0) / vals[1].max(1e-300)).sqrt()
}

/// Canonical frame, per-frame output (unsolved frames interpolated), points and errors.
fn finish(d: &Data, tracks: &CameraTracks, s: &SolveSettings, r: Recon, method: SolveMethod, kind: Kind) -> Result<CameraSolve, SolveError> {
    let Some((k0, c0)) = r.cams.iter().next().map(|(k, c)| (*k, *c)) else { return err("the camera could not be solved (no frame was solved)") };
    // Scale: median depth of the points seen by the first solved camera is 1.
    let mut depths: Vec<f64> = d.by_frame[k0].iter().filter_map(|(ti, _)| r.pts.get(ti)).map(|x| c0.pose.to_cam(*x)[2]).filter(|z| *z > 0.0).collect();
    depths.sort_by(f64::total_cmp);
    let sc = match kind {
        Kind::Tripod => 1.0,
        Kind::General => depths.get(depths.len() / 2).copied().filter(|v| *v > 1e-12).unwrap_or(1.0),
    };
    let r0 = c0.pose.r;
    let cc = c0.pose.c;
    let to_world = |x: V3| scale(mv(&r0, sub(x, cc)), 1.0 / sc);
    let mut frames = vec![SolvedFrame::default(); d.n];
    for (k, c) in &r.cams {
        let rr = mm(&c.pose.r, &mt(&r0));
        frames[*k] = SolvedFrame { rot: log_rot(&rr), center: to_world(c.pose.c), focal: c.f, solved: true };
    }
    // Hold / interpolate unsolved frames so the camera path stays continuous.
    let solved: Vec<usize> = (0..d.n).filter(|k| frames[*k].solved).collect();
    for k in 0..d.n {
        if frames[k].solved {
            continue;
        }
        let a = solved.iter().rev().find(|x| **x < k).copied();
        let b = solved.iter().find(|x| **x > k).copied();
        frames[k] = match (a, b) {
            (Some(a), Some(b)) => {
                let t = (k - a) as f64 / (b - a) as f64;
                let (fa, fb) = (frames[a], frames[b]);
                let rr = slerp(&fa.r(), &fb.r(), t);
                SolvedFrame {
                    rot: log_rot(&rr),
                    center: add(scale(fa.center, 1.0 - t), scale(fb.center, t)),
                    focal: fa.focal + (fb.focal - fa.focal) * t,
                    solved: false,
                }
            }
            (Some(a), None) => SolvedFrame { solved: false, ..frames[a] },
            (None, Some(b)) => SolvedFrame { solved: false, ..frames[b] },
            (None, None) => frames[k],
        };
    }
    let mut points = vec![];
    let (mut esum, mut en) = (0.0, 0usize);
    let mut ids: Vec<&u32> = r.pts.keys().collect();
    ids.sort();
    for ti in ids {
        let x = r.pts[ti];
        let (id, start, pts) = &d.tracks[*ti as usize];
        let (mut s1, mut n1) = (0.0, 0);
        for (i, uv) in pts.iter().enumerate() {
            let k = start + i;
            if r.outliers.contains(&(*ti, k)) {
                continue;
            }
            if let Some(c) = r.cams.get(&k)
                && let Some(res) = bundle::residual(c, x, *uv)
            {
                s1 += res[0].hypot(res[1]);
                n1 += 1;
            }
        }
        if n1 < 2 {
            continue;
        }
        esum += s1;
        en += n1;
        points.push(SolvedPoint { id: *id, pos: to_world(x), error: (s1 / n1 as f64) as f32, first: *start as u32, last: (start + pts.len() - 1) as u32 });
    }
    Ok(CameraSolve {
        version: 1,
        size: tracks.size,
        start: tracks.start,
        frame_duration: tracks.frame_duration,
        shot: s.shot,
        method_used: method,
        average_error: if en > 0 { esum / en as f64 } else { 0.0 },
        frames,
        points,
        ground: None,
        distortion: None,
    })
}
