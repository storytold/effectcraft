//! Small dense linear algebra for the camera solver, written from the textbook methods:
//!
//! - 3-vectors / 3 × 3 matrices and rotations (Rodrigues' formula and its inverse);
//! - the **one-sided Jacobi SVD** (Hestenes 1958; Golub & Van Loan, *Matrix Computations*,
//!   §8.6.4), accurate for the small, badly scaled systems of the 8-point algorithm;
//! - the **cyclic Jacobi eigenvalue method** for symmetric matrices (Golub & Van Loan §8.5);
//! - **Cholesky** factorisation for the bundle adjustment's normal equations;
//! - the closed-form **similarity alignment** of two point sets (Umeyama, "Least-squares
//!   estimation of transformation parameters between two point patterns", PAMI 1991).

pub type V3 = [f64; 3];
pub type M3 = [[f64; 3]; 3];

pub const I3: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn scale(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}
pub fn normalize(a: V3) -> V3 {
    let n = norm(a);
    if n > 0.0 { scale(a, 1.0 / n) } else { a }
}
pub fn mv(m: &M3, v: V3) -> V3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}
/// `mᵀ v`.
pub fn mtv(m: &M3, v: V3) -> V3 {
    [m[0][0] * v[0] + m[1][0] * v[1] + m[2][0] * v[2], m[0][1] * v[0] + m[1][1] * v[1] + m[2][1] * v[2], m[0][2] * v[0] + m[1][2] * v[1] + m[2][2] * v[2]]
}
pub fn mm(a: &M3, b: &M3) -> M3 {
    let mut o = [[0.0; 3]; 3];
    for (i, row) in o.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    o
}
pub fn mt(a: &M3) -> M3 {
    [[a[0][0], a[1][0], a[2][0]], [a[0][1], a[1][1], a[2][1]], [a[0][2], a[1][2], a[2][2]]]
}
pub fn madd(a: &M3, b: &M3, k: f64) -> M3 {
    let mut o = *a;
    for i in 0..3 {
        for j in 0..3 {
            o[i][j] += k * b[i][j];
        }
    }
    o
}
pub fn mscale(a: &M3, k: f64) -> M3 {
    madd(&[[0.0; 3]; 3], a, k)
}
pub fn det3(m: &M3) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}
pub fn skew(v: V3) -> M3 {
    [[0.0, -v[2], v[1]], [v[2], 0.0, -v[0]], [-v[1], v[0], 0.0]]
}
pub fn outer(a: V3, b: V3) -> M3 {
    [[a[0] * b[0], a[0] * b[1], a[0] * b[2]], [a[1] * b[0], a[1] * b[1], a[1] * b[2]], [a[2] * b[0], a[2] * b[1], a[2] * b[2]]]
}
pub fn col(m: &M3, j: usize) -> V3 {
    [m[0][j], m[1][j], m[2][j]]
}
pub fn from_cols(a: V3, b: V3, c: V3) -> M3 {
    [[a[0], b[0], c[0]], [a[1], b[1], c[1]], [a[2], b[2], c[2]]]
}
pub fn inv3(m: &M3) -> Option<M3> {
    let d = det3(m);
    if d.abs() < 1e-300 {
        return None;
    }
    let c = |r0: usize, c0: usize, r1: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    let adj = [[c(1, 1, 2, 2), -c(0, 1, 2, 2), c(0, 1, 1, 2)], [-c(1, 0, 2, 2), c(0, 0, 2, 2), -c(0, 0, 1, 2)], [c(1, 0, 2, 1), -c(0, 0, 2, 1), c(0, 0, 1, 1)]];
    Some(mscale(&adj, 1.0 / d))
}

/// Rotation matrix of an axis-angle vector (Rodrigues' formula).
pub fn rodrigues(w: V3) -> M3 {
    let th = norm(w);
    let k = skew(w);
    if th < 1e-12 {
        return madd(&I3, &k, 1.0);
    }
    let a = th.sin() / th;
    let b = (1.0 - th.cos()) / (th * th);
    madd(&madd(&I3, &k, a), &mm(&k, &k), b)
}

/// Axis-angle vector of a rotation matrix (inverse of [`rodrigues`]).
pub fn log_rot(r: &M3) -> V3 {
    let c = ((r[0][0] + r[1][1] + r[2][2] - 1.0) * 0.5).clamp(-1.0, 1.0);
    let th = c.acos();
    let v = [r[2][1] - r[1][2], r[0][2] - r[2][0], r[1][0] - r[0][1]];
    if th < 1e-9 {
        return scale(v, 0.5);
    }
    if std::f64::consts::PI - th < 1e-6 {
        // Near π: the axis is the dominant column of R + I.
        let b = madd(r, &I3, 1.0);
        let j = (0..3).max_by(|&a, &bb| b[a][a].total_cmp(&b[bb][bb])).unwrap_or(0);
        let axis = normalize(col(&b, j));
        return scale(axis, th);
    }
    scale(v, th / (2.0 * th.sin()))
}

/// The rotation closest to `m` (Frobenius norm), via the SVD.
pub fn nearest_rotation(m: &M3) -> M3 {
    let (u, _, v) = svd3(m);
    let mut r = mm(&u, &mt(&v));
    if det3(&r) < 0.0 {
        let mut u2 = u;
        for row in u2.iter_mut() {
            row[2] = -row[2];
        }
        r = mm(&u2, &mt(&v));
    }
    r
}

/// Spherical interpolation between two rotations.
pub fn slerp(a: &M3, b: &M3, t: f64) -> M3 {
    let d = log_rot(&mm(b, &mt(a)));
    mm(&rodrigues(scale(d, t)), a)
}

/// Thin SVD `A = U diag(s) Vᵀ` of an `m × n` row-major matrix (one-sided Jacobi). `U` is
/// `max(m, n) × n` (rows beyond `m` belong to zero padding), `s` is sorted descending and `V`
/// is `n × n` (columns are the right singular vectors).
pub fn svd(a: &[f64], m: usize, n: usize) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let rows = m.max(n);
    let mut u = vec![0.0; rows * n];
    u[..m * n].copy_from_slice(&a[..m * n]);
    let mut v = vec![0.0; n * n];
    for i in 0..n {
        v[i * n + i] = 1.0;
    }
    for _sweep in 0..60 {
        let mut off = 0.0f64;
        for p in 0..n {
            for q in p + 1..n {
                let (mut alpha, mut beta, mut gamma) = (0.0, 0.0, 0.0);
                for r in 0..rows {
                    let (x, y) = (u[r * n + p], u[r * n + q]);
                    alpha += x * x;
                    beta += y * y;
                    gamma += x * y;
                }
                if gamma == 0.0 {
                    continue;
                }
                let c0 = gamma.abs() / (alpha * beta).sqrt().max(1e-300);
                off = off.max(c0);
                if c0 < 1e-15 {
                    continue;
                }
                let zeta = (beta - alpha) / (2.0 * gamma);
                let t = zeta.signum() / (zeta.abs() + (1.0 + zeta * zeta).sqrt());
                let t = if zeta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (1.0 + t * t).sqrt();
                let s = c * t;
                for r in 0..rows {
                    let (x, y) = (u[r * n + p], u[r * n + q]);
                    u[r * n + p] = c * x - s * y;
                    u[r * n + q] = s * x + c * y;
                }
                for r in 0..n {
                    let (x, y) = (v[r * n + p], v[r * n + q]);
                    v[r * n + p] = c * x - s * y;
                    v[r * n + q] = s * x + c * y;
                }
            }
        }
        if off < 1e-15 {
            break;
        }
    }
    let mut s: Vec<f64> = (0..n).map(|j| (0..rows).map(|r| u[r * n + j] * u[r * n + j]).sum::<f64>().sqrt()).collect();
    for (j, sj) in s.iter().enumerate() {
        if *sj > 1e-300 {
            for r in 0..rows {
                u[r * n + j] /= sj;
            }
        }
    }
    // Sort descending.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| s[b].total_cmp(&s[a]));
    let (mut u2, mut v2) = (vec![0.0; rows * n], vec![0.0; n * n]);
    for (k, &j) in order.iter().enumerate() {
        for r in 0..rows {
            u2[r * n + k] = u[r * n + j];
        }
        for r in 0..n {
            v2[r * n + k] = v[r * n + j];
        }
    }
    s = order.iter().map(|&j| s[j]).collect();
    (u2, s, v2)
}

/// SVD of a 3 × 3 matrix: (U, singular values descending, V) with `m = U diag(s) Vᵀ`.
pub fn svd3(m: &M3) -> (M3, V3, M3) {
    let a: Vec<f64> = m.iter().flatten().copied().collect();
    let (u, s, v) = svd(&a, 3, 3);
    let g = |x: &[f64]| [[x[0], x[1], x[2]], [x[3], x[4], x[5]], [x[6], x[7], x[8]]];
    let mut um = g(&u);
    // Columns of U for zero singular values are undefined: complete them to an orthonormal basis.
    if s[2] < 1e-300 {
        let c = cross(col(&um, 0), col(&um, 1));
        for (r, row) in um.iter_mut().enumerate() {
            row[2] = c[r];
        }
    }
    (um, [s[0], s[1], s[2]], g(&v))
}

/// The unit vector `x` minimising `|A x|` (`m × n` row-major): the right singular vector of
/// the smallest singular value.
pub fn null_vector(a: &[f64], m: usize, n: usize) -> Vec<f64> {
    let (_, _, v) = svd(a, m, n);
    (0..n).map(|r| v[r * n + n - 1]).collect()
}

/// Eigen-decomposition of a symmetric `n × n` matrix (cyclic Jacobi): eigenvalues ascending
/// and the matching eigenvectors as the columns of the returned row-major matrix.
pub fn sym_eigen(a: &[f64], n: usize) -> (Vec<f64>, Vec<f64>) {
    let mut a = a.to_vec();
    let mut v = vec![0.0; n * n];
    for i in 0..n {
        v[i * n + i] = 1.0;
    }
    for _ in 0..100 {
        let mut off = 0.0;
        for p in 0..n {
            for q in p + 1..n {
                off += a[p * n + q] * a[p * n + q];
            }
        }
        if off < 1e-30 {
            break;
        }
        for p in 0..n {
            for q in p + 1..n {
                let apq = a[p * n + q];
                if apq.abs() < 1e-300 {
                    continue;
                }
                let theta = (a[q * n + q] - a[p * n + p]) / (2.0 * apq);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..n {
                    let (akp, akq) = (a[k * n + p], a[k * n + q]);
                    a[k * n + p] = c * akp - s * akq;
                    a[k * n + q] = s * akp + c * akq;
                }
                for k in 0..n {
                    let (apk, aqk) = (a[p * n + k], a[q * n + k]);
                    a[p * n + k] = c * apk - s * aqk;
                    a[q * n + k] = s * apk + c * aqk;
                }
                for k in 0..n {
                    let (vkp, vkq) = (v[k * n + p], v[k * n + q]);
                    v[k * n + p] = c * vkp - s * vkq;
                    v[k * n + q] = s * vkp + c * vkq;
                }
            }
        }
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&x, &y| a[x * n + x].total_cmp(&a[y * n + y]));
    let vals = order.iter().map(|&j| a[j * n + j]).collect();
    let mut vecs = vec![0.0; n * n];
    for (k, &j) in order.iter().enumerate() {
        for r in 0..n {
            vecs[r * n + k] = v[r * n + j];
        }
    }
    (vals, vecs)
}

/// Dot product, four lanes (lets the compiler vectorise).
fn dot_n(a: &[f64], b: &[f64]) -> f64 {
    let mut acc = [0.0f64; 4];
    let ((ca, ra), (cb, rb)) = (a.as_chunks::<4>(), b.as_chunks::<4>());
    for (x, y) in ca.iter().zip(cb) {
        for k in 0..4 {
            acc[k] += x[k] * y[k];
        }
    }
    let mut s = acc[0] + acc[1] + acc[2] + acc[3];
    for (x, y) in ra.iter().zip(rb) {
        s += x * y;
    }
    s
}

/// Solve `A x = b` for a symmetric positive definite `A` (`n × n`, row-major) in place by
/// Cholesky factorisation. Returns false when `A` is not positive definite.
pub fn cholesky_solve(a: &mut [f64], n: usize, b: &mut [f64]) -> bool {
    for j in 0..n {
        let mut d = a[j * n + j];
        for k in 0..j {
            d -= a[j * n + k] * a[j * n + k];
        }
        if d <= 0.0 || !d.is_finite() {
            return false;
        }
        let d = d.sqrt();
        a[j * n + j] = d;
        let (head, tail) = a.split_at_mut((j + 1) * n);
        let rowj = &head[j * n..j * n + j];
        // Rows below j (sequential: per-column parallel dispatch costs more than it saves).
        for row in tail.chunks_mut(n) {
            row[j] = (row[j] - dot_n(&row[..j], rowj)) / d;
        }
    }
    // Forward: L y = b.
    for i in 0..n {
        let mut s = b[i];
        for k in 0..i {
            s -= a[i * n + k] * b[k];
        }
        b[i] = s / a[i * n + i];
    }
    // Back: Lᵀ x = y.
    for i in (0..n).rev() {
        let mut s = b[i];
        for k in i + 1..n {
            s -= a[k * n + i] * b[k];
        }
        b[i] = s / a[i * n + i];
    }
    b.iter().all(|v| v.is_finite())
}

/// A similarity transform `x ↦ s R x + t`.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Similarity {
    pub s: f64,
    pub r: M3,
    pub t: V3,
}

impl Default for Similarity {
    fn default() -> Self {
        Similarity { s: 1.0, r: I3, t: [0.0; 3] }
    }
}

impl Similarity {
    pub fn apply(&self, x: V3) -> V3 {
        add(scale(mv(&self.r, x), self.s), self.t)
    }
}

/// The similarity minimising `Σ |dst − (s R src + t)|²` (Umeyama 1991).
pub fn umeyama(src: &[V3], dst: &[V3]) -> Option<Similarity> {
    let n = src.len().min(dst.len());
    if n < 3 {
        return None;
    }
    let k = 1.0 / n as f64;
    let ms = scale(src[..n].iter().fold([0.0; 3], |a, b| add(a, *b)), k);
    let md = scale(dst[..n].iter().fold([0.0; 3], |a, b| add(a, *b)), k);
    let mut cov = [[0.0; 3]; 3];
    let mut var = 0.0;
    for i in 0..n {
        let (a, b) = (sub(src[i], ms), sub(dst[i], md));
        cov = madd(&cov, &outer(b, a), k);
        var += dot(a, a) * k;
    }
    if var < 1e-300 {
        return None;
    }
    let (u, d, v) = svd3(&cov);
    let mut sgn = [1.0, 1.0, 1.0];
    if det3(&u) * det3(&v) < 0.0 {
        sgn[2] = -1.0;
    }
    let ud = from_cols(scale(col(&u, 0), sgn[0]), scale(col(&u, 1), sgn[1]), scale(col(&u, 2), sgn[2]));
    let r = mm(&ud, &mt(&v));
    let s = (d[0] * sgn[0] + d[1] * sgn[1] + d[2] * sgn[2]) / var;
    let t = sub(md, scale(mv(&r, ms), s));
    Some(Similarity { s, r, t })
}

/// Euler angles (degrees) `[x, y, z]` with `m = Rx(x) Ry(y) Rz(z)`: After Effects' Orientation
/// order (Z is applied to the vector first, then Y, then X; see `effectcraft_geom::Mat4::orientation`).
pub fn euler_xyz(m: &M3) -> V3 {
    let sy = m[0][2].clamp(-1.0, 1.0);
    let y = sy.asin();
    let (x, z) = if sy.abs() < 0.999_999 { ((-m[1][2]).atan2(m[2][2]), (-m[0][1]).atan2(m[0][0])) } else { (m[2][1].atan2(m[1][1]), 0.0) };
    [x.to_degrees(), y.to_degrees(), z.to_degrees()]
}

/// `Rx(x) Ry(y) Rz(z)` from degrees.
pub fn from_euler_xyz(e: V3) -> M3 {
    let (sx, cx) = e[0].to_radians().sin_cos();
    let (sy, cy) = e[1].to_radians().sin_cos();
    let (sz, cz) = e[2].to_radians().sin_cos();
    let rx = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
    let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let rz = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
    mm(&rx, &mm(&ry, &rz))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svd_reconstructs() {
        let a = [3.0, 1.0, 2.0, -1.0, 4.0, 0.5, 2.0, 2.0, 7.0, 0.1, -3.0, 1.0];
        let (u, s, v) = svd(&a, 4, 3);
        for r in 0..4 {
            for c in 0..3 {
                let x: f64 = (0..3).map(|k| u[r * 3 + k] * s[k] * v[c * 3 + k]).sum();
                assert!((x - a[r * 3 + c]).abs() < 1e-10);
            }
        }
        assert!(s[0] >= s[1] && s[1] >= s[2]);
    }

    #[test]
    fn rotations_round_trip() {
        let w = [0.3, -1.2, 0.7];
        let r = rodrigues(w);
        let w2 = log_rot(&r);
        assert!(norm(sub(w, w2)) < 1e-12);
        let e = euler_xyz(&r);
        let r2 = from_euler_xyz(e);
        for i in 0..3 {
            for j in 0..3 {
                assert!((r[i][j] - r2[i][j]).abs() < 1e-12);
            }
        }
        assert!((det3(&r) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn eigen_and_cholesky() {
        let a = [4.0, 1.0, 0.5, 1.0, 3.0, 0.2, 0.5, 0.2, 2.0];
        let (vals, vecs) = sym_eigen(&a, 3);
        for k in 0..3 {
            let v = [vecs[k], vecs[3 + k], vecs[6 + k]];
            let av = [dot([a[0], a[1], a[2]], v), dot([a[3], a[4], a[5]], v), dot([a[6], a[7], a[8]], v)];
            assert!(norm(sub(av, scale(v, vals[k]))) < 1e-10);
        }
        let mut m = a;
        let mut b = [1.0, 2.0, 3.0];
        assert!(cholesky_solve(&mut m, 3, &mut b));
        let r = [dot([a[0], a[1], a[2]], b), dot([a[3], a[4], a[5]], b), dot([a[6], a[7], a[8]], b)];
        assert!(norm(sub(r, [1.0, 2.0, 3.0])) < 1e-12);
    }

    #[test]
    fn umeyama_recovers_a_similarity() {
        let t = Similarity { s: 2.5, r: rodrigues([0.1, 0.4, -0.3]), t: [1.0, -2.0, 5.0] };
        let src = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 2.0, 3.0]];
        let dst: Vec<V3> = src.iter().map(|p| t.apply(*p)).collect();
        let e = umeyama(&src, &dst).unwrap();
        assert!((e.s - 2.5).abs() < 1e-10);
        assert!(norm(sub(e.t, t.t)) < 1e-9);
    }
}
