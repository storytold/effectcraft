//! Sparse bundle adjustment: Levenberg–Marquardt over cameras and points with the **Schur
//! complement** eliminating the points (Triggs, McLauchlan, Hartley & Fitzgibbon, "Bundle
//! Adjustment — A Modern Synthesis", 2000, §6; H&Z Appendix 6). Residuals are reprojection
//! errors in pixels with a **Huber** robust loss (applied by iteratively reweighted least
//! squares, Triggs et al. §3.3).
//!
//! Cameras are `(R, C, f)`; rotations are updated multiplicatively (`R ← exp([δ]×) R`), focal
//! lengths in log space. The focal length is held fixed, shared by all cameras, or per camera
//! (with a smoothness prior between consecutive cameras). Tripod solves hold the centres fixed.

use rayon::prelude::*;

use super::Distortion;
use super::geometry::Pose;
use super::linalg::*;

/// A camera in the adjustment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BaCam {
    pub pose: Pose,
    /// Focal length (pixels).
    pub f: f64,
    /// Lens distortion (shared by all cameras; adjusted when [`BaOpts::distortion`]).
    pub dist: Distortion,
}

/// One observation: camera `cam` sees point `pt` at `uv` (pixels relative to the principal
/// point).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BaObs {
    pub cam: u32,
    pub pt: u32,
    pub uv: [f64; 2],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FocalMode {
    Fixed,
    Shared,
    /// Per camera, with prior weight `k` on the log-focal difference of consecutive cameras.
    PerCamera(f64),
}

#[derive(Clone, Debug)]
pub struct BaOpts {
    pub focal: FocalMode,
    /// Motion only: the points stay fixed.
    pub fix_points: bool,
    /// The camera centres stay fixed (tripod).
    pub fix_centers: bool,
    /// Cameras that stay fixed (gauge), by index; missing entries are free.
    pub fixed: Vec<bool>,
    /// Huber threshold (pixels).
    pub huber: f64,
    pub max_iter: usize,
    /// Adjust the shared radial distortion `(k1, k2)` too.
    pub distortion: bool,
}

impl Default for BaOpts {
    fn default() -> Self {
        BaOpts { focal: FocalMode::Fixed, fix_points: false, fix_centers: false, fixed: vec![], huber: 2.0, max_iter: 30, distortion: false }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BaReport {
    pub initial_cost: f64,
    pub cost: f64,
    pub iterations: usize,
}

const BEHIND: f64 = 1e6;

fn huber(e2: f64, k: f64) -> f64 {
    if e2 <= k * k { e2 } else { 2.0 * k * e2.sqrt() - k * k }
}

/// Residual (pixels) of one observation, `None` behind the camera.
pub fn residual(c: &BaCam, x: V3, uv: [f64; 2]) -> Option<[f64; 2]> {
    let p = c.pose.to_cam(x);
    if p[2] <= 1e-9 {
        return None;
    }
    let q = c.dist.distort([c.f * p[0] / p[2], c.f * p[1] / p[2]]);
    Some([q[0] - uv[0], q[1] - uv[1]])
}

/// Total robust cost.
pub fn cost(cams: &[BaCam], pts: &[V3], obs: &[BaObs], o: &BaOpts) -> f64 {
    let c: f64 = obs
        .par_iter()
        .map(|ob| match residual(&cams[ob.cam as usize], pts[ob.pt as usize], ob.uv) {
            Some(r) => huber(r[0] * r[0] + r[1] * r[1], o.huber),
            None => BEHIND,
        })
        .sum();
    c + prior_cost(cams, o)
}

fn prior_cost(cams: &[BaCam], o: &BaOpts) -> f64 {
    match o.focal {
        FocalMode::PerCamera(k) => cams.windows(2).map(|w| (k * (w[0].f.ln() - w[1].f.ln())).powi(2)).sum(),
        _ => 0.0,
    }
}

/// Parameter layout.
struct Layout {
    /// First pose parameter of each camera (`usize::MAX` when its pose is fixed).
    start: Vec<usize>,
    /// Per-camera focal parameter (`usize::MAX` when none). Gauge-fixed cameras keep a free
    /// focal length.
    fidx: Vec<usize>,
    /// Index of the shared focal parameter.
    shared: Option<usize>,
    /// Index of `k1` (then `k2`) when the distortion is adjusted.
    dist: Option<usize>,
    n: usize,
    rot_only: bool,
}

fn layout(ncam: usize, o: &BaOpts) -> Layout {
    let per_cam_f = matches!(o.focal, FocalMode::PerCamera(_));
    let d = if o.fix_centers { 3 } else { 6 };
    let mut start = vec![usize::MAX; ncam];
    let mut fidx = vec![usize::MAX; ncam];
    let mut n = 0;
    for i in 0..ncam {
        if !o.fixed.get(i).copied().unwrap_or(false) {
            start[i] = n;
            n += d;
        }
        if per_cam_f {
            fidx[i] = n;
            n += 1;
        }
    }
    let shared = matches!(o.focal, FocalMode::Shared).then(|| {
        n += 1;
        n - 1
    });
    let dist = o.distortion.then(|| {
        n += 2;
        n - 2
    });
    Layout { start, fidx, shared, dist, n, rot_only: o.fix_centers }
}

/// Camera-side Jacobian entries of one observation: (parameter index, d r / d param).
fn cam_jac(l: &Layout, ci: usize, c: &BaCam, xc: V3, a: &[[f64; 3]; 2], p: [f64; 2], out: &mut Vec<(usize, [f64; 2])>) {
    out.clear();
    let s = l.start[ci];
    if s != usize::MAX {
        // dXc/dδ = −[Xc]×
        let sk = skew(xc);
        for k in 0..3 {
            let g = [-sk[0][k], -sk[1][k], -sk[2][k]];
            out.push((s + k, [dot(a[0], g), dot(a[1], g)]));
        }
        if !l.rot_only {
            // dXc/dC = −R
            for k in 0..3 {
                let g = [-c.pose.r[0][k], -c.pose.r[1][k], -c.pose.r[2][k]];
                out.push((s + 3 + k, [dot(a[0], g), dot(a[1], g)]));
            }
        }
    }
    if l.fidx[ci] != usize::MAX {
        out.push((l.fidx[ci], p));
    }
    if let Some(g) = l.shared {
        out.push((g, p));
    }
}

struct PtSys {
    v: M3,
    bp: V3,
    w: Vec<(usize, V3)>,
}

struct PreconditionerBlock {
    start: usize,
    dim: usize,
    inverse: Vec<f64>,
}

fn camera_preconditioner(u: &[f64], l: &Layout) -> Option<Vec<PreconditionerBlock>> {
    let mut ranges = Vec::new();
    let pose_dim = if l.rot_only { 3 } else { 6 };
    for (&pose, &focal) in l.start.iter().zip(&l.fidx) {
        if pose != usize::MAX {
            ranges.push((pose, pose_dim + usize::from(focal != usize::MAX)));
        } else if focal != usize::MAX {
            ranges.push((focal, 1));
        }
    }
    if let Some(start) = l.shared.or(l.dist) {
        ranges.push((start, l.n - start));
    }
    ranges
        .into_iter()
        .map(|(start, dim)| {
            let mut matrix = Vec::with_capacity(dim * dim);
            for i in 0..dim {
                for j in 0..dim {
                    matrix.push(*u.get((start + i) * l.n + start + j)?);
                }
            }
            let mut inverse = vec![0.0; dim * dim];
            for col in 0..dim {
                let mut a = matrix.clone();
                let mut b = vec![0.0; dim];
                *b.get_mut(col)? = 1.0;
                if !cholesky_solve(&mut a, dim, &mut b) {
                    return None;
                }
                for (row, value) in b.into_iter().enumerate() {
                    inverse[row * dim + col] = value;
                }
            }
            Some(PreconditionerBlock { start, dim, inverse })
        })
        .collect()
}

/// Apply the point-eliminated system without materialising W V^-1 W^T.
fn iterative_schur(u: &[f64], rhs: &[f64], systems: &[PtSys], vinv: &[Option<M3>], l: &Layout) -> Option<Vec<f64>> {
    if l.n == 0 || l.n.checked_mul(l.n) != Some(u.len()) || rhs.len() != l.n || systems.len() != vinv.len() {
        return None;
    }
    let blocks = camera_preconditioner(u, l)?;
    // U contains camera blocks, global parameters and the focal prior. Store its nonzeros
    // once; all subsequent products are linear in these and the point-camera connections.
    let rows: Vec<Vec<(usize, f64)>> =
        u.chunks_exact(l.n).map(|row| row.iter().enumerate().filter(|(_, v)| **v != 0.0).map(|(i, v)| (i, *v)).collect()).collect();
    let mut reduced_rhs = rhs.to_vec();
    for (ps, vi) in systems.iter().zip(vinv) {
        let Some(vi) = vi else { continue };
        let vb = mv(vi, ps.bp);
        for (i, w) in &ps.w {
            *reduced_rhs.get_mut(*i)? -= dot(*w, vb);
        }
    }
    conjugate_gradient(
        &reduced_rhs,
        |x, y| {
            for (i, row) in rows.iter().enumerate() {
                y[i] = row.iter().map(|(j, v)| v * x[*j]).sum();
            }
            for (ps, vi) in systems.iter().zip(vinv) {
                let Some(vi) = vi else { continue };
                let q = ps.w.iter().fold([0.0; 3], |q, (i, w)| add(q, scale(*w, x[*i])));
                let q = mv(vi, q);
                for (i, w) in &ps.w {
                    y[*i] -= dot(*w, q);
                }
            }
        },
        |r, z| {
            z.fill(0.0);
            for block in &blocks {
                for i in 0..block.dim {
                    z[block.start + i] = (0..block.dim).map(|j| block.inverse[i * block.dim + j] * r[block.start + j]).sum();
                }
            }
        },
    )
}

/// Run Levenberg–Marquardt. `cams` and `pts` are updated in place.
pub fn adjust(cams: &mut [BaCam], pts: &mut [V3], obs: &[BaObs], o: &BaOpts) -> BaReport {
    adjust_impl(cams, pts, obs, o, true)
}

fn adjust_impl(cams: &mut [BaCam], pts: &mut [V3], obs: &[BaObs], o: &BaOpts, allow_iterative: bool) -> BaReport {
    let l = layout(cams.len(), o);
    let npts = pts.len();
    // Observations by point.
    let mut by_pt: Vec<Vec<u32>> = vec![vec![]; npts];
    for (i, ob) in obs.iter().enumerate() {
        by_pt[ob.pt as usize].push(i as u32);
    }
    let mut cur = cost(cams, pts, obs, o);
    let initial = cur;
    let t0 = web_time::Instant::now();
    let mut tries = 0usize;
    let dbg = std::env::var_os("EC_CAMTRACK_DEBUG").is_some() && l.n > 200;
    let mut lambda = 1e-4;
    let mut iters = 0;
    if l.n == 0 && o.fix_points {
        return BaReport { initial_cost: initial, cost: cur, iterations: 0 };
    }
    let n = l.n;
    let parallel = n <= 700;
    for _ in 0..o.max_iter {
        iters += 1;
        // ---- Normal equations (Gauss–Newton with IRLS weights).
        // Per point: V (3×3), bp (3), W entries (cam-side index → 3).
        let build_point = |j: usize, s_acc: &mut Vec<f64>, bc: &mut Vec<f64>| -> PtSys {
            let mut ps = PtSys { v: [[0.0; 3]; 3], bp: [0.0; 3], w: vec![] };
            let mut jc: Vec<(usize, [f64; 2])> = Vec::with_capacity(8);
            for &oi in &by_pt[j] {
                let ob = &obs[oi as usize];
                let ci = ob.cam as usize;
                let c = &cams[ci];
                let xc = c.pose.to_cam(pts[j]);
                if xc[2] <= 1e-9 {
                    continue;
                }
                let iz = 1.0 / xc[2];
                let p0 = [c.f * xc[0] * iz, c.f * xc[1] * iz];
                let pd = c.dist.distort(p0);
                let r = [pd[0] - ob.uv[0], pd[1] - ob.uv[1]];
                let e = (r[0] * r[0] + r[1] * r[1]).sqrt();
                let w = if e <= o.huber { 1.0 } else { o.huber / e };
                let a0 = [[c.f * iz, 0.0, -c.f * xc[0] * iz * iz], [0.0, c.f * iz, -c.f * xc[1] * iz * iz]];
                // Through the distortion: d r = D d p.
                let (dj, dk) = c.dist.jacobian(p0);
                let a = [0, 1].map(|i| [0, 1, 2].map(|k| dj[i][0] * a0[0][k] + dj[i][1] * a0[1][k]));
                let p = [dj[0][0] * p0[0] + dj[0][1] * p0[1], dj[1][0] * p0[0] + dj[1][1] * p0[1]];
                cam_jac(&l, ci, c, xc, &a, p, &mut jc);
                if let Some(g) = l.dist {
                    jc.push((g, [dk[0][0], dk[1][0]]));
                    jc.push((g + 1, [dk[0][1], dk[1][1]]));
                }
                // Point Jacobian: A R.
                let jx = if o.fix_points {
                    [[0.0; 3]; 2]
                } else {
                    let r0 = mtv(&c.pose.r, a[0]);
                    let r1 = mtv(&c.pose.r, a[1]);
                    [r0, r1]
                };
                for (ia, ja) in &jc {
                    bc[*ia] -= w * (ja[0] * r[0] + ja[1] * r[1]);
                    for (ib, jb) in &jc {
                        s_acc[ia * n + ib] += w * (ja[0] * jb[0] + ja[1] * jb[1]);
                    }
                }
                if !o.fix_points {
                    for a1 in 0..3 {
                        ps.bp[a1] -= w * (jx[0][a1] * r[0] + jx[1][a1] * r[1]);
                        for b1 in 0..3 {
                            ps.v[a1][b1] += w * (jx[0][a1] * jx[0][b1] + jx[1][a1] * jx[1][b1]);
                        }
                    }
                    for (ia, ja) in &jc {
                        let wv =
                            [w * (ja[0] * jx[0][0] + ja[1] * jx[1][0]), w * (ja[0] * jx[0][1] + ja[1] * jx[1][1]), w * (ja[0] * jx[0][2] + ja[1] * jx[1][2])];
                        // Each observation of a point is on another camera: only the shared focal
                        // length repeats.
                        let repeats = Some(*ia) == l.shared || l.dist.is_some_and(|g| *ia == g || *ia == g + 1);
                        let found = if repeats { ps.w.iter_mut().find(|e| e.0 == *ia) } else { None };
                        match found {
                            Some(e) => e.1 = add(e.1, wv),
                            None => ps.w.push((*ia, wv)),
                        }
                    }
                }
            }
            ps
        };
        let (mut s_mat, mut bc, systems): (Vec<f64>, Vec<f64>, Vec<PtSys>) = if parallel && npts > 64 {
            let chunks = rayon::current_num_threads().clamp(1, 16);
            let per = npts.div_ceil(chunks);
            let parts: Vec<(Vec<f64>, Vec<f64>, Vec<PtSys>)> = (0..chunks)
                .into_par_iter()
                .map(|t| {
                    let (mut s, mut b) = (vec![0.0; n * n], vec![0.0; n]);
                    let v = (t * per..((t + 1) * per).min(npts)).map(|j| build_point(j, &mut s, &mut b)).collect();
                    (s, b, v)
                })
                .collect();
            let mut s = vec![0.0; n * n];
            let mut b = vec![0.0; n];
            let mut sys = Vec::with_capacity(npts);
            for (ps, pb, pv) in parts {
                for (x, y) in s.iter_mut().zip(ps) {
                    *x += y;
                }
                for (x, y) in b.iter_mut().zip(pb) {
                    *x += y;
                }
                sys.extend(pv);
            }
            (s, b, sys)
        } else {
            let mut s = vec![0.0; n * n];
            let mut b = vec![0.0; n];
            let sys = (0..npts).map(|j| build_point(j, &mut s, &mut b)).collect();
            (s, b, sys)
        };
        // Focal prior.
        if let FocalMode::PerCamera(k) = o.focal {
            for i in 0..cams.len().saturating_sub(1) {
                let r = k * (cams[i].f.ln() - cams[i + 1].f.ln());
                let (a, b) = (l.fidx[i], l.fidx[i + 1]);
                let ia = (a != usize::MAX).then_some(a);
                let ib = (b != usize::MAX).then_some(b);
                for (idx, g) in [(ia, k), (ib, -k)] {
                    let Some(x) = idx else { continue };
                    bc[x] -= g * r;
                    for (idy, h) in [(ia, k), (ib, -k)] {
                        if let Some(y) = idy {
                            s_mat[x * n + y] += g * h;
                        }
                    }
                }
            }
        }
        let base_s = s_mat.clone();
        let base_bc = bc.clone();
        let mean_diag = ((0..n).map(|i| base_s[i * n + i]).sum::<f64>() / n.max(1) as f64).max(1e-12);
        let schur_pairs = systems.iter().fold(0usize, |sum, ps| sum.saturating_add(ps.w.len().saturating_mul(ps.w.len().saturating_add(1)) / 2));
        // Reserve the iterative solve for expensive long-track reductions; small systems
        // retain the direct path and its rounding. Failed solves use the direct path too.
        let mut use_iterative = allow_iterative && !o.fix_points && n >= 120 && schur_pairs > 4_000_000;
        // ---- Try steps with increasing damping.
        let mut accepted = false;
        for _try in 0..12 {
            tries += 1;
            let mut s = base_s.clone();
            let mut rhs = base_bc.clone();
            for i in 0..n {
                let d = s[i * n + i];
                s[i * n + i] = d + lambda * d.max(1e-6 * mean_diag);
            }
            // Schur complement over the points: S −= W V⁻¹ Wᵀ, rhs −= W V⁻¹ bp (in parallel
            // chunks of points when the reduced system is small enough to copy per thread).
            let vinv: Vec<Option<M3>> = if o.fix_points {
                vec![None; npts]
            } else {
                systems
                    .par_iter()
                    .map(|ps| {
                        let mut v = ps.v;
                        let vd = (v[0][0] + v[1][1] + v[2][2]) / 3.0;
                        for k in 0..3 {
                            v[k][k] += lambda * v[k][k].max(1e-6 * vd.max(1e-12)) + 1e-12;
                        }
                        inv3(&v)
                    })
                    .collect()
            };
            let iterative_step = if use_iterative { iterative_schur(&s, &rhs, &systems, &vinv, &l) } else { None };
            if dbg && use_iterative {
                eprintln!("ba iterative n={n} pairs={schur_pairs} solved={}", iterative_step.is_some());
            }
            if use_iterative && iterative_step.is_none() {
                use_iterative = false;
            }
            let dc = if let Some(dc) = iterative_step {
                dc
            } else {
                if !o.fix_points {
                    let reduce = |range: std::ops::Range<usize>, s: &mut [f64], rhs: &mut [f64]| {
                        for j in range {
                            let (Some(vi), ps) = (&vinv[j], &systems[j]) else { continue };
                            let vb = mv(vi, ps.bp);
                            let wv: Vec<V3> = ps.w.iter().map(|(_, w)| mv(vi, *w)).collect();
                            for (a, (ia, wa)) in ps.w.iter().enumerate() {
                                rhs[*ia] -= dot(*wa, vb);
                                for (ib, wb) in &ps.w[a..] {
                                    let d = dot(wv[a], *wb);
                                    s[ia * n + ib] -= d;
                                    if ia != ib {
                                        s[ib * n + ia] -= d;
                                    }
                                }
                            }
                        }
                    };
                    let chunks = rayon::current_num_threads().clamp(1, 8);
                    if n <= 1200 && npts >= 64 && chunks > 1 {
                        let per = npts.div_ceil(chunks);
                        let parts: Vec<(Vec<f64>, Vec<f64>)> = (0..chunks)
                            .into_par_iter()
                            .map(|t| {
                                let (mut ds, mut dr) = (vec![0.0; n * n], vec![0.0; n]);
                                reduce(t * per..((t + 1) * per).min(npts), &mut ds, &mut dr);
                                (ds, dr)
                            })
                            .collect();
                        for (ds, dr) in parts {
                            s.par_iter_mut().zip(ds.par_iter()).for_each(|(x, y)| *x += y);
                            for (x, y) in rhs.iter_mut().zip(dr) {
                                *x += y;
                            }
                        }
                    } else {
                        reduce(0..npts, &mut s, &mut rhs);
                    }
                }
                let mut dc = rhs;
                if n > 0 && !cholesky_solve(&mut s, n, &mut dc) {
                    lambda *= 10.0;
                    continue;
                }
                dc
            };
            // Apply.
            let mut nc: Vec<BaCam> = cams.to_vec();
            for (i, c) in nc.iter_mut().enumerate() {
                let st = l.start[i];
                if st != usize::MAX {
                    let dr = [dc[st], dc[st + 1], dc[st + 2]];
                    c.pose.r = mm(&rodrigues(dr), &c.pose.r);
                    if !l.rot_only {
                        c.pose.c = add(c.pose.c, [dc[st + 3], dc[st + 4], dc[st + 5]]);
                    }
                }
                if l.fidx[i] != usize::MAX {
                    c.f *= dc[l.fidx[i]].clamp(-0.5, 0.5).exp();
                }
                if let Some(g) = l.shared {
                    c.f *= dc[g].clamp(-0.5, 0.5).exp();
                }
                if let Some(g) = l.dist {
                    c.dist.k1 += dc[g].clamp(-0.2, 0.2);
                    c.dist.k2 += dc[g + 1].clamp(-0.2, 0.2);
                }
            }
            let mut np: Vec<V3> = pts.to_vec();
            if !o.fix_points {
                np.par_iter_mut().enumerate().for_each(|(j, x)| {
                    let Some(vi) = &vinv[j] else { return };
                    let ps = &systems[j];
                    let mut b = ps.bp;
                    for (ia, w) in &ps.w {
                        b = sub(b, scale(*w, dc[*ia]));
                    }
                    *x = add(*x, mv(vi, b));
                });
            }
            let c2 = cost(&nc, &np, obs, o);
            if c2.is_finite() && c2 < cur {
                cams.copy_from_slice(&nc);
                pts.copy_from_slice(&np);
                let gain = cur - c2;
                cur = c2;
                lambda = (lambda / 3.0).max(1e-9);
                accepted = gain > 1e-6 * cur.max(1e-12);
                if !accepted {
                    // Converged (tiny improvement).
                    if dbg {
                        eprintln!(
                            "ba n={} pts={} obs={} iters={iters} tries={tries} {:.2}s cost {initial:.1} -> {cur:.1}",
                            l.n,
                            npts,
                            obs.len(),
                            t0.elapsed().as_secs_f64()
                        );
                    }
                    return BaReport { initial_cost: initial, cost: cur, iterations: iters };
                }
                break;
            }
            lambda *= 4.0;
        }
        if !accepted {
            break;
        }
    }
    if dbg {
        eprintln!("ba n={} pts={} obs={} iters={iters} tries={tries} {:.2}s cost {initial:.1} -> {cur:.1}", l.n, npts, obs.len(), t0.elapsed().as_secs_f64());
    }
    BaReport { initial_cost: initial, cost: cur, iterations: iters }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camtrack::geometry::Rng;

    #[test]
    fn long_track_adjustment_matches_direct_reprojections_and_outliers() {
        rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap().install(|| {
            for (focal, fix_centers, distortion) in
                [(FocalMode::Fixed, false, false), (FocalMode::Shared, false, true), (FocalMode::PerCamera(30.0), false, true), (FocalMode::Shared, true, true)]
            {
                let mut rng = Rng(71);
                let mut noise = || (rng.next_u64() % 100_000) as f64 / 100_000.0 - 0.5;
                let truth: Vec<V3> = (0..320).map(|_| [noise() * 8.0, noise() * 5.0, 10.0 + noise() * 5.0]).collect();
                let true_cams: Vec<_> = (0..60)
                    .map(|i| BaCam {
                        pose: Pose {
                            r: rodrigues([0.001 * i as f64, -0.003 * i as f64, 0.0]),
                            c: if fix_centers { [0.0; 3] } else { [0.03 * i as f64, 0.01 * (i as f64).sin(), 0.0] },
                        },
                        f: if matches!(focal, FocalMode::PerCamera(_)) { 900.0 * (1.0 + 0.05 * (i as f64 * 0.07).sin()) } else { 900.0 },
                        dist: if distortion { Distortion { k1: 0.015, k2: -0.005, radius: 1100.0 } } else { Distortion::NONE },
                    })
                    .collect();
                let mut obs = Vec::new();
                for (ci, c) in true_cams.iter().enumerate() {
                    for (pi, p) in truth.iter().enumerate() {
                        let mut uv = residual(c, *p, [0.0; 2]).unwrap();
                        uv[0] += noise() * 0.05;
                        uv[1] += noise() * 0.05;
                        if (ci * truth.len() + pi).is_multiple_of(37) {
                            uv[0] += 25.0;
                            uv[1] -= 30.0;
                        }
                        obs.push(BaObs { cam: ci as u32, pt: pi as u32, uv });
                    }
                }
                let mut cams = true_cams.clone();
                for c in cams.iter_mut().skip(2) {
                    c.pose.r = mm(&rodrigues([0.001, -0.001, 0.0005]), &c.pose.r);
                    if !fix_centers {
                        c.pose.c = add(c.pose.c, [0.002, -0.003, 0.001]);
                    }
                }
                let mut pts: Vec<_> = truth.iter().map(|p| add(*p, [0.01, -0.01, 0.005])).collect();
                let (mut direct_cams, mut direct_pts) = (cams.clone(), pts.clone());
                let opts = BaOpts { focal, fix_centers, distortion, fixed: (0..60).map(|i| i < 2).collect(), max_iter: 10, ..Default::default() };
                let report = adjust(&mut cams, &mut pts, &obs, &opts);
                let direct = adjust_impl(&mut direct_cams, &mut direct_pts, &obs, &opts, false);
                assert!((report.cost - direct.cost).abs() < 1e-5 * direct.cost.max(1.0));
                for ob in &obs {
                    let a = residual(&cams[ob.cam as usize], pts[ob.pt as usize], ob.uv).unwrap();
                    let b = residual(&direct_cams[ob.cam as usize], direct_pts[ob.pt as usize], ob.uv).unwrap();
                    assert!((a[0] - b[0]).hypot(a[1] - b[1]) < 0.001);
                    assert_eq!(a[0].hypot(a[1]) <= 4.0, b[0].hypot(b[1]) <= 4.0);
                }
                for (a, b) in cams.iter().zip(direct_cams) {
                    assert!(norm(sub(a.pose.c, b.pose.c)) < 0.001);
                    assert!(norm(log_rot(&mm(&a.pose.r, &mt(&b.pose.r)))) < 1e-4);
                    assert!((a.f - b.f).abs() < 1e-4 * b.f);
                    assert!((a.dist.k1 - b.dist.k1).abs() < 1e-4 && (a.dist.k2 - b.dist.k2).abs() < 1e-4);
                }
            }
        });
    }

    #[test]
    fn matrix_free_schur_matches_explicit_system_and_rejects_bad_preconditioner() {
        let l = layout(20, &BaOpts::default());
        let n = l.n;
        let mut u = vec![0.0; n * n];
        for i in 0..n {
            u[i * n + i] = 10.0;
        }
        let systems: Vec<_> = (0..80)
            .map(|j| PtSys {
                v: I3,
                bp: [0.01, -0.02, 0.03],
                w: (0..n).map(|i| (i, [((j * 13 + i * 7) % 19) as f64 * 0.001 - 0.009, (i % 7) as f64 * 0.001, (j % 11) as f64 * 0.001])).collect(),
            })
            .collect();
        let vinv = vec![Some(I3); systems.len()];
        let rhs: Vec<_> = (0..n).map(|i| (i % 7) as f64 - 3.0).collect();
        let x = iterative_schur(&u, &rhs, &systems, &vinv, &l).unwrap();
        let mut explicit = u.clone();
        let mut reduced_rhs = rhs.clone();
        for ps in &systems {
            for (i, a) in &ps.w {
                reduced_rhs[*i] -= dot(*a, ps.bp);
                for (j, b) in &ps.w {
                    explicit[i * n + j] -= dot(*a, *b);
                }
            }
        }
        let mut direct = reduced_rhs.clone();
        let mut factor = explicit.clone();
        assert!(cholesky_solve(&mut factor, n, &mut direct));
        for (a, b) in x.iter().zip(direct) {
            assert!((a - b).abs() < 1e-9);
        }
        let mut error2 = 0.0;
        for (i, row) in explicit.chunks_exact(n).enumerate() {
            let ax: f64 = row.iter().zip(&x).map(|(a, x)| a * x).sum();
            error2 += (ax - reduced_rhs[i]).powi(2);
        }
        assert!(error2 < 1e-18 * reduced_rhs.iter().map(|v| v * v).sum::<f64>());
        u[0] = -1.0;
        assert!(iterative_schur(&u, &rhs, &systems, &vinv, &l).is_none());
    }

    #[test]
    fn bundle_adjustment_converges_from_noise() {
        let mut rng = Rng(99);
        let mut u = || (rng.next_u64() % 100_000) as f64 / 100_000.0 - 0.5;
        let truth: Vec<V3> = (0..80).map(|_| [u() * 6.0, u() * 4.0, 6.0 + u() * 4.0]).collect();
        let f = 900.0;
        let cams_true: Vec<BaCam> = (0..6)
            .map(|i| BaCam {
                pose: Pose { r: rodrigues([0.0, -0.05 * i as f64, 0.01 * i as f64]), c: [0.3 * i as f64, 0.05 * i as f64, 0.0] },
                f,
                dist: Distortion::NONE,
            })
            .collect();
        let mut obs = vec![];
        for (ci, c) in cams_true.iter().enumerate() {
            for (pi, x) in truth.iter().enumerate() {
                let r = residual(c, *x, [0.0, 0.0]).unwrap();
                obs.push(BaObs { cam: ci as u32, pt: pi as u32, uv: r });
            }
        }
        let mut cams = cams_true.clone();
        for (i, c) in cams.iter_mut().enumerate().skip(2) {
            c.pose.r = mm(&rodrigues([0.01, -0.01 * i as f64 * 0.3, 0.005]), &c.pose.r);
            c.pose.c = add(c.pose.c, [0.02, -0.03, 0.01]);
        }
        let mut pts: Vec<V3> = truth.iter().map(|x| add(*x, [u() * 0.1, u() * 0.1, u() * 0.2])).collect();
        let mut fixed = vec![false; 6];
        fixed[0] = true;
        fixed[1] = true;
        let rep = adjust(&mut cams, &mut pts, &obs, &BaOpts { fixed, max_iter: 50, ..Default::default() });
        let rms = (rep.cost / obs.len() as f64).sqrt();
        assert!(rms < 1e-4, "{rep:?}");
    }
}
