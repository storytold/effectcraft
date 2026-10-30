//! Two-view and multi-view geometry for the camera solver, from Hartley & Zisserman (H&Z),
//! *Multiple View Geometry in Computer Vision* (2nd ed.) and Ma et al., *An Invitation to 3-D
//! Vision* (2004).
//!
//! All image points here are **normalised** (calibrated) coordinates `x = (u − c) / f`.
//! A camera is `(R, C)` with `X_cam = R (X − C)`.

use super::linalg::{
    I3, M3, V3, add, col, cross, det3, dot, from_cols, madd, mm, mscale, mt, mtv, mv, nearest_rotation, normalize, null_vector, outer, scale, sub, svd3,
    sym_eigen,
};

/// A deterministic xorshift generator for RANSAC samples.
pub struct Rng(pub u64);
impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }
    /// `k` distinct indices below `n`.
    pub fn sample(&mut self, n: usize, k: usize, out: &mut Vec<usize>) {
        out.clear();
        let mut guard = 0;
        while out.len() < k && guard < 1000 {
            guard += 1;
            let c = self.below(n);
            if !out.contains(&c) {
                out.push(c);
            }
        }
    }
}

/// Hartley normalisation (H&Z §4.4.4): centroid to the origin, mean distance √2.
fn normalizer(p: &[[f64; 2]]) -> M3 {
    let n = p.len().max(1) as f64;
    let c = [p.iter().map(|q| q[0]).sum::<f64>() / n, p.iter().map(|q| q[1]).sum::<f64>() / n];
    let d = p.iter().map(|q| (q[0] - c[0]).hypot(q[1] - c[1])).sum::<f64>() / n;
    let s = if d > 1e-12 { std::f64::consts::SQRT_2 / d } else { 1.0 };
    [[s, 0.0, -s * c[0]], [0.0, s, -s * c[1]], [0.0, 0.0, 1.0]]
}

fn h2(m: &M3, p: [f64; 2]) -> V3 {
    mv(m, [p[0], p[1], 1.0])
}

/// The essential matrix from ≥ 8 correspondences (`x2ᵀ E x1 = 0`) by the normalised 8-point
/// algorithm (H&Z Alg. 11.1), with the essential constraint (two equal singular values, one
/// zero) enforced (H&Z §9.6.1 / Result 9.18).
pub fn essential_8pt(x1: &[[f64; 2]], x2: &[[f64; 2]]) -> Option<M3> {
    let n = x1.len().min(x2.len());
    if n < 8 {
        return None;
    }
    let (t1, t2) = (normalizer(&x1[..n]), normalizer(&x2[..n]));
    let mut a = Vec::with_capacity(n * 9);
    for i in 0..n {
        let p = h2(&t1, x1[i]);
        let q = h2(&t2, x2[i]);
        a.extend_from_slice(&[q[0] * p[0], q[0] * p[1], q[0], q[1] * p[0], q[1] * p[1], q[1], p[0], p[1], 1.0]);
    }
    let e = null_vector(&a, n, 9);
    let en = [[e[0], e[1], e[2]], [e[3], e[4], e[5]], [e[6], e[7], e[8]]];
    // Denormalise: E = T2ᵀ Ê T1.
    let e = mm(&mt(&t2), &mm(&en, &t1));
    let (u, s, v) = svd3(&e);
    let m = (s[0] + s[1]) * 0.5;
    if m <= 0.0 || !m.is_finite() {
        return None;
    }
    let d = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]];
    Some(mm(&u, &mm(&d, &mt(&v))))
}

/// Squared Sampson distance of a correspondence to `x2ᵀ E x1 = 0` (H&Z §11.4.3), in the units of
/// the coordinates.
pub fn sampson2(e: &M3, p: [f64; 2], q: [f64; 2]) -> f64 {
    let x1 = [p[0], p[1], 1.0];
    let x2 = [q[0], q[1], 1.0];
    let ex1 = mv(e, x1);
    let etx2 = mtv(e, x2);
    let num = dot(x2, ex1);
    let den = ex1[0] * ex1[0] + ex1[1] * ex1[1] + etx2[0] * etx2[0] + etx2[1] * etx2[1];
    if den < 1e-300 { f64::INFINITY } else { num * num / den }
}

/// RANSAC essential matrix (8-point samples, Sampson distance below `thr`), refitted on the
/// consensus set. Returns the matrix and the inlier mask.
pub fn ransac_essential(x1: &[[f64; 2]], x2: &[[f64; 2]], thr: f64, iterations: usize, seed: u64) -> Option<(M3, Vec<bool>)> {
    let n = x1.len().min(x2.len());
    if n < 8 {
        return None;
    }
    let t2 = thr * thr;
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut best: Option<(usize, f64, M3)> = None;
    let mut idx = vec![];
    let mut max_iter = iterations;
    let mut it = 0;
    let (mut s1, mut s2) = (vec![[0.0; 2]; 8], vec![[0.0; 2]; 8]);
    while it < max_iter {
        it += 1;
        rng.sample(n, 8, &mut idx);
        if idx.len() < 8 {
            break;
        }
        for (k, &i) in idx.iter().enumerate() {
            s1[k] = x1[i];
            s2[k] = x2[i];
        }
        let Some(e) = essential_8pt(&s1, &s2) else { continue };
        let (mut c, mut err) = (0, 0.0);
        for i in 0..n {
            let d = sampson2(&e, x1[i], x2[i]);
            if d < t2 {
                c += 1;
                err += d;
            }
        }
        if best.as_ref().is_none_or(|b| c > b.0 || (c == b.0 && err < b.1)) {
            best = Some((c, err, e));
            // Adaptive iteration count for 99 % confidence.
            let w = c as f64 / n as f64;
            let p_good = w.powi(8);
            if p_good > 1e-9 {
                let need = ((1.0 - 0.99f64).ln() / (1.0 - p_good).max(1e-12).ln()).ceil();
                if need.is_finite() {
                    max_iter = max_iter.min((need as usize).max(30));
                }
            }
        }
    }
    let (_, _, mut e) = best?;
    // Refit on the inliers (twice: the consensus may grow).
    let mut inl = vec![false; n];
    for _ in 0..2 {
        for i in 0..n {
            inl[i] = sampson2(&e, x1[i], x2[i]) < t2;
        }
        let (a, b): (Vec<[f64; 2]>, Vec<[f64; 2]>) = (0..n).filter(|i| inl[*i]).map(|i| (x1[i], x2[i])).unzip();
        match essential_8pt(&a, &b) {
            Some(e2) => e = e2,
            None => break,
        }
    }
    for i in 0..n {
        inl[i] = sampson2(&e, x1[i], x2[i]) < t2;
    }
    Some((e, inl))
}

/// The four `(R, t)` with `x2 ~ R x1 + t` (|t| = 1) of an essential matrix (H&Z Result 9.19).
pub fn decompose_essential(e: &M3) -> [(M3, V3); 4] {
    let (mut u, _, mut v) = svd3(e);
    if det3(&u) < 0.0 {
        for row in u.iter_mut() {
            row[2] = -row[2];
        }
    }
    if det3(&v) < 0.0 {
        for row in v.iter_mut() {
            row[2] = -row[2];
        }
    }
    let w = [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
    let r1 = mm(&u, &mm(&w, &mt(&v)));
    let r2 = mm(&u, &mm(&mt(&w), &mt(&v)));
    let t = col(&u, 2);
    [(r1, t), (r1, scale(t, -1.0)), (r2, t), (r2, scale(t, -1.0))]
}

/// A camera `(R, C)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub r: M3,
    pub c: V3,
}

impl Pose {
    pub const IDENTITY: Pose = Pose { r: I3, c: [0.0; 3] };
    /// From `x_cam = R X + t`.
    pub fn from_rt(r: M3, t: V3) -> Pose {
        Pose { r, c: scale(mtv(&r, t), -1.0) }
    }
    pub fn t(&self) -> V3 {
        scale(mv(&self.r, self.c), -1.0)
    }
    pub fn to_cam(&self, x: V3) -> V3 {
        mv(&self.r, sub(x, self.c))
    }
}

/// Linear (DLT) triangulation from normalised observations (H&Z §12.2), minimising the
/// algebraic error over all views.
pub fn triangulate(views: &[(Pose, [f64; 2])]) -> Option<V3> {
    if views.len() < 2 {
        return None;
    }
    let mut ata = [0.0f64; 16];
    for (p, x) in views {
        let t = p.t();
        let rows = [[p.r[0][0], p.r[0][1], p.r[0][2], t[0]], [p.r[1][0], p.r[1][1], p.r[1][2], t[1]], [p.r[2][0], p.r[2][1], p.r[2][2], t[2]]];
        let a0 = [0, 1, 2, 3].map(|j| x[0] * rows[2][j] - rows[0][j]);
        let a1 = [0, 1, 2, 3].map(|j| x[1] * rows[2][j] - rows[1][j]);
        for a in [a0, a1] {
            let nrm = a.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-300);
            for i in 0..4 {
                for j in 0..4 {
                    ata[i * 4 + j] += a[i] * a[j] / (nrm * nrm);
                }
            }
        }
    }
    let (_, vecs) = sym_eigen(&ata, 4);
    let h = [vecs[0], vecs[4], vecs[8], vecs[12]];
    if h[3].abs() < 1e-12 {
        return None;
    }
    let x = [h[0] / h[3], h[1] / h[3], h[2] / h[3]];
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Angle (radians) between the rays from two camera centres to `x`.
pub fn ray_angle(a: V3, b: V3, x: V3) -> f64 {
    let (u, v) = (normalize(sub(x, a)), normalize(sub(x, b)));
    dot(u, v).clamp(-1.0, 1.0).acos()
}

/// Squared reprojection error (normalised units) of `x` in `pose` against observation `o`
/// (`None` behind the camera).
pub fn reproj2(pose: &Pose, x: V3, o: [f64; 2]) -> Option<f64> {
    let c = pose.to_cam(x);
    if c[2] <= 1e-9 {
        return None;
    }
    let (u, v) = (c[0] / c[2] - o[0], c[1] / c[2] - o[1]);
    Some(u * u + v * v)
}

/// Pick the essential decomposition with the most points in front of both cameras; returns
/// the second camera (the first is the identity) and that count.
pub fn select_pose(cands: &[(M3, V3)], x1: &[[f64; 2]], x2: &[[f64; 2]]) -> Option<(Pose, usize)> {
    if x1.len() != x2.len() || x1.is_empty() || x1.iter().chain(x2).flatten().any(|v| !v.is_finite()) {
        return None;
    }
    let mut best: Option<(Pose, usize)> = None;
    for (r, t) in cands {
        let p2 = Pose::from_rt(*r, *t);
        let mut c = 0;
        for i in 0..x1.len() {
            if let Some(x) = triangulate(&[(Pose::IDENTITY, x1[i]), (p2, x2[i])])
                && x[2] > 0.0
                && p2.to_cam(x)[2] > 0.0
            {
                c += 1;
            }
        }
        if best.as_ref().is_none_or(|b| c > b.1) {
            best = Some((p2, c));
        }
    }
    best
}

/// Decompose a calibrated planar homography `H ~ R + t nᵀ / d` (normalised coordinates,
/// `x2 ~ H x1`) into its motions `(R, t/d, n)` (Ma et al. §5.3.3). Up to four solutions; those
/// that put the plane behind the first camera are dropped.
pub fn decompose_homography(h: &M3, x1: &[[f64; 2]], x2: &[[f64; 2]]) -> Vec<(M3, V3, V3)> {
    let (_, s, _) = svd3(h);
    if s[1] <= 1e-12 {
        return vec![];
    }
    let mut hn = mscale(h, 1.0 / s[1]);
    // Sign: x2ᵀ H x1 > 0 for the correspondences (positive depth).
    let pos = x1.iter().zip(x2).filter(|(p, q)| dot([q[0], q[1], 1.0], mv(&hn, [p[0], p[1], 1.0])) > 0.0).count();
    if pos * 2 < x1.len() {
        hn = mscale(&hn, -1.0);
    }
    let hth = mm(&mt(&hn), &hn);
    let a: Vec<f64> = hth.iter().flatten().copied().collect();
    let (vals, vecs) = sym_eigen(&a, 3);
    // Descending: σ1² ≥ σ2² = 1 ≥ σ3².
    let v1 = [vecs[2], vecs[5], vecs[8]];
    let v2 = [vecs[1], vecs[4], vecs[7]];
    let v3 = [vecs[0], vecs[3], vecs[6]];
    let (s1, s3) = (vals[2].max(1.0), vals[0].clamp(0.0, 1.0));
    let mut out = vec![];
    if s1 - s3 < 1e-10 {
        // H is a rotation: no translation (pure rotation or plane at infinity).
        return vec![(nearest_rotation(&hn), [0.0; 3], [0.0, 0.0, 1.0])];
    }
    let k = (s1 - s3).sqrt();
    let (a1, a3) = ((1.0 - s3).sqrt(), (s1 - 1.0).sqrt());
    let u1 = scale(add(scale(v1, a1), scale(v3, a3)), 1.0 / k);
    let u2 = scale(sub(scale(v1, a1), scale(v3, a3)), 1.0 / k);
    for u in [u1, u2] {
        let uu = from_cols(v2, u, cross(v2, u));
        let hv2 = mv(&hn, v2);
        let hu = mv(&hn, u);
        let ww = from_cols(hv2, hu, cross(hv2, hu));
        let r = mm(&ww, &mt(&uu));
        let n = cross(v2, u);
        let t = mv(&madd(&hn, &r, -1.0), n);
        for sgn in [1.0, -1.0] {
            let (n2, t2) = (scale(n, sgn), scale(t, sgn));
            // The plane must be in front of the first camera: nᵀ x1 > 0 (with n pointing away).
            let front = x1.iter().filter(|p| dot(n2, [p[0], p[1], 1.0]) > 0.0).count();
            if front * 2 >= x1.len() {
                out.push((r, t2, n2));
            }
        }
    }
    out
}

/// Least-squares plane through points: (centroid, unit normal, RMS in-plane spread).
pub fn fit_plane(p: &[V3]) -> Option<(V3, V3, f64)> {
    if p.len() < 3 {
        return None;
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
    let (vals, vecs) = sym_eigen(&cov, 3);
    if vals[1] <= 1e-18 {
        return None;
    }
    let n = normalize([vecs[0], vecs[3], vecs[6]]);
    Some((c, n, (vals[1] + vals[2]).max(0.0).sqrt()))
}

/// The rotation `R` minimising `Σ |b − R a|²` over unit rays (orthogonal Procrustes / Kabsch).
pub fn procrustes(a: &[V3], b: &[V3]) -> Option<M3> {
    if a.len() < 2 {
        return None;
    }
    let mut m = [[0.0; 3]; 3];
    for (x, y) in a.iter().zip(b) {
        m = madd(&m, &outer(*y, *x), 1.0);
    }
    let (u, _, v) = svd3(&m);
    let mut r = mm(&u, &mt(&v));
    if det3(&r) < 0.0 {
        let mut u2 = u;
        for row in u2.iter_mut() {
            row[2] = -row[2];
        }
        r = mm(&u2, &mt(&v));
    }
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::super::linalg::{log_rot, norm, rodrigues};
    use super::*;

    fn scene() -> (Vec<V3>, Pose) {
        let mut rng = Rng(12345);
        let pts: Vec<V3> = (0..60)
            .map(|_| {
                let f = |r: &mut Rng| (r.next_u64() % 10_000) as f64 / 10_000.0 - 0.5;
                [f(&mut rng) * 4.0, f(&mut rng) * 3.0, 5.0 + f(&mut rng) * 3.0]
            })
            .collect();
        let p2 = Pose { r: rodrigues([0.02, -0.15, 0.03]), c: [0.6, 0.1, -0.2] };
        (pts, p2)
    }

    fn proj(p: &Pose, x: V3) -> [f64; 2] {
        let c = p.to_cam(x);
        [c[0] / c[2], c[1] / c[2]]
    }

    #[test]
    fn pose_selection_rejects_mismatched_or_non_finite_correspondences() {
        let candidates = [(I3, [0.1, 0.0, 0.0])];
        assert!(select_pose(&candidates, &[[0.0; 2]], &[]).is_none());
        assert!(select_pose(&candidates, &[], &[[0.0; 2]]).is_none());
        assert!(select_pose(&candidates, &[[f64::NAN, 0.0]], &[[0.0; 2]]).is_none());
    }

    #[test]
    fn essential_recovers_relative_pose() {
        let (pts, p2) = scene();
        let x1: Vec<[f64; 2]> = pts.iter().map(|x| proj(&Pose::IDENTITY, *x)).collect();
        let mut x2: Vec<[f64; 2]> = pts.iter().map(|x| proj(&p2, *x)).collect();
        // Outliers.
        x2[3] = [0.3, -0.2];
        x2[17] = [-0.1, 0.25];
        let (e, inl) = ransac_essential(&x1, &x2, 1e-3, 500, 7).unwrap();
        assert!(!inl[3] && !inl[17]);
        assert!(inl.iter().filter(|b| **b).count() >= 57);
        let (pose, n) = select_pose(&decompose_essential(&e), &x1, &x2).unwrap();
        assert!(n >= 57);
        let r_err = norm(log_rot(&mm(&pose.r, &mt(&p2.r))));
        assert!(r_err < 1e-6, "{r_err}");
        let dir = normalize(p2.c);
        assert!(norm(sub(normalize(pose.c), dir)) < 1e-5, "{:?} {:?}", pose.c, dir);
    }

    #[test]
    fn homography_decomposition_contains_the_motion() {
        // Plane z = 4 (normal (0,0,1) in camera 1), seen after a motion.
        let p2 = Pose { r: rodrigues([0.05, 0.2, -0.04]), c: [0.8, -0.3, 0.4] };
        let pts: Vec<V3> = (0..30).map(|i| [((i * 37) % 17) as f64 / 8.0 - 1.0, ((i * 11) % 13) as f64 / 6.0 - 1.0, 4.0]).collect();
        let x1: Vec<[f64; 2]> = pts.iter().map(|x| proj(&Pose::IDENTITY, *x)).collect();
        let x2: Vec<[f64; 2]> = pts.iter().map(|x| proj(&p2, *x)).collect();
        // H = R + t nᵀ / d with t = −R C, n = (0,0,1), d = 4.
        let t = p2.t();
        let h = madd(&p2.r, &outer(t, [0.0, 0.0, 0.25]), 1.0);
        let sols = decompose_homography(&mscale(&h, 3.7), &x1, &x2);
        assert!(!sols.is_empty());
        let ok = sols
            .iter()
            .any(|(r, td, n)| norm(log_rot(&mm(r, &mt(&p2.r)))) < 1e-6 && norm(sub(*td, scale(t, 0.25))) < 1e-6 && norm(sub(*n, [0.0, 0.0, 1.0])) < 1e-6);
        assert!(ok, "{sols:?}");
    }

    #[test]
    fn triangulation_and_plane() {
        let (pts, p2) = scene();
        for x in &pts[..5] {
            let y = triangulate(&[(Pose::IDENTITY, proj(&Pose::IDENTITY, *x)), (p2, proj(&p2, *x))]).unwrap();
            assert!(norm(sub(*x, y)) < 1e-8);
        }
        let plane: Vec<V3> = (0..10).map(|i| [i as f64, (i * i % 7) as f64, 2.0 * i as f64 + 1.0]).collect();
        let (_, n, _) = fit_plane(&plane).unwrap();
        // All points satisfy z = 2x + 1: normal ∝ (2, 0, −1).
        assert!(norm(cross(n, normalize([2.0, 0.0, -1.0]))) < 1e-9);
    }
}
