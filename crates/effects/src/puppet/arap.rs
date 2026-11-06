//! As-rigid-as-possible mesh deformation, after Igarashi, Moscovich & Hughes,
//! "As-Rigid-As-Possible Shape Manipulation" (SIGGRAPH 2005).
//!
//! Two linear least-squares steps per frame:
//!
//! 1. **Similarity step.** Each triangle edge `(i, j)` expresses the third vertex `k` in the
//!    edge's local frame, `v_k = v_i + x·(v_j − v_i) + y·R90(v_j − v_i)`. Minimising the error of
//!    that relation over all three edges of every triangle, with the pinned vertices fixed,
//!    gives a deformation that is locally a similarity (rotation + uniform scale).
//! 2. **Fit.** Each triangle of the step-1 result is fitted with a target: its rest shape turned, and
//!    changed towards keeping its area (see [`fit_triangle`]); the positions then minimise the difference between every triangle's edge vectors and
//!    its fitted edge vectors. Repeating this is the local/global iteration of Liu et al., "A
//!    Local/Global Approach to Mesh Parameterization" (2008), with its cotangent edge weights,
//!    so the energy is the area integral of `|F - R|²` and doesn't depend on how the mesh happens
//!    to be triangulated.
//!
//! Rigidity alone concentrates a pin's pull: the strain of a point load goes as 1/r, so the mesh
//! around a pin stretches hard. Step 2 therefore also minimises a bending energy, `λ ∫ |∇F|²`, the
//! squared jump of the deformation gradient across each inner edge (Bergou et al., "A Quadratic
//! Bending Model for Inextensible Surfaces", 2006, flat case), which bounds the strain at a point.
//! Bending costs nothing for an affine map, though, so on its own it extends a pull's stretch across
//! the whole shape, and the shape grows. A pull towards where the pins as a whole put the mesh (their
//! best rigid fit), which sets in away from the pins, makes the deformation die out instead: `μ(x)
//! |v − T(x)|²`. All the lengths go with the mesh's size, so adding a pin doesn't change them.
//!
//! Material should also keep its volume, like rubber, so the fit's target gives way to a change of
//! shape at constant area and pushes back against a change of area (see [`fit_triangle`]).
//!
//! All of that pays only a bounded price for collapse, so a hard push folds the mesh at the pin however
//! long the solve runs. Triangles close to collapse or far from their area are therefore stiffened and the solve
//! repeated a bounded number of times (see [`barrier_weight`]). Pins are hard to the end: pins dragged
//! through each other still land, with whatever folds that takes.
//!
//! Triangle weights stiffen regions (Starch); edge-vector targets implement Bend/Advanced pin
//! rotation and scale. Both systems are symmetric positive definite (a tiny regulariser towards
//! the initial guess keeps under-constrained meshes well-posed). The matrices don't change
//! between rounds, so each is factored once (banded Cholesky in reverse Cuthill-McKee order) and
//! a round is two back-substitutions; if a factor can't be built, Jacobi-preconditioned conjugate
//! gradients solve the same system.

use std::collections::BTreeMap;

/// A soft target for the vector `v_j − v_i`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeTarget {
    pub i: usize,
    pub j: usize,
    pub vec: [f64; 2],
    pub weight: f64,
}

/// A stiff soft target for the point at barycentric `bary` in the triangle `verts`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointTarget {
    pub verts: [usize; 3],
    pub bary: [f64; 3],
    pub position: [f64; 2],
}

/// Constraints for one solve.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Handles {
    /// Hard position constraints (vertex, position).
    pub fixed: Vec<(usize, [f64; 2])>,
    /// Points inside triangles held at positions.
    pub points: Vec<PointTarget>,
    pub edges: Vec<EdgeTarget>,
}

/// Sparse symmetric matrix in row maps (assembly), then CSR for the solve.
#[derive(Clone)]
struct Sparse {
    rows: Vec<BTreeMap<usize, f64>>,
}

impl Sparse {
    fn new(n: usize) -> Sparse {
        Sparse { rows: vec![BTreeMap::new(); n] }
    }
    fn add(&mut self, i: usize, j: usize, v: f64) {
        *self.rows[i].entry(j).or_insert(0.0) += v;
    }
}

struct Csr {
    ptr: Vec<usize>,
    col: Vec<usize>,
    val: Vec<f64>,
    diag: Vec<f64>,
}

impl Csr {
    fn mul(&self, x: &[f64], y: &mut [f64]) {
        for (r, yr) in y.iter_mut().enumerate() {
            let mut s = 0.0;
            for k in self.ptr[r]..self.ptr[r + 1] {
                s += self.val[k] * x[self.col[k]];
            }
            *yr = s;
        }
    }
}

/// Solve `A x = b` restricted to the free unknowns (`free[i]`), with the fixed unknowns at
/// their values in `x`. Jacobi-preconditioned conjugate gradients, starting from `x`.
fn solve(a: &Sparse, b: &[f64], x: &mut [f64], free: &[bool]) {
    let n = b.len();
    let idx: Vec<usize> = (0..n).filter(|i| free[*i]).collect();
    if idx.is_empty() {
        return;
    }
    let mut pos = vec![usize::MAX; n];
    for (k, i) in idx.iter().enumerate() {
        pos[*i] = k;
    }
    let m = idx.len();
    // Reduced system: A_ff x_f = b_f − A_fc x_c.
    let mut ptr = vec![0];
    let mut col = vec![];
    let mut val = vec![];
    let mut diag = vec![1.0; m];
    let mut rhs = vec![0.0; m];
    for (k, &i) in idx.iter().enumerate() {
        let mut r = b[i];
        for (&j, &v) in &a.rows[i] {
            if free[j] {
                col.push(pos[j]);
                val.push(v);
                if j == i {
                    diag[k] = if v.abs() > 1e-300 { v } else { 1.0 };
                }
            } else {
                r -= v * x[j];
            }
        }
        rhs[k] = r;
        ptr.push(col.len());
    }
    let csr = Csr { ptr, col, val, diag };
    let mut xf: Vec<f64> = idx.iter().map(|i| x[*i]).collect();
    let mut ax = vec![0.0; m];
    csr.mul(&xf, &mut ax);
    let mut r: Vec<f64> = rhs.iter().zip(&ax).map(|(b, a)| b - a).collect();
    let bnorm = rhs.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-30);
    let mut z: Vec<f64> = r.iter().zip(&csr.diag).map(|(r, d)| r / d).collect();
    let mut p = z.clone();
    let mut rz: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
    let mut ap = vec![0.0; m];
    for _ in 0..(4 * m + 50) {
        if r.iter().map(|v| v * v).sum::<f64>().sqrt() <= 1e-13 * bnorm {
            break;
        }
        csr.mul(&p, &mut ap);
        let pap: f64 = p.iter().zip(&ap).map(|(a, b)| a * b).sum();
        if pap.abs() < 1e-300 {
            break;
        }
        let alpha = rz / pap;
        for k in 0..m {
            xf[k] += alpha * p[k];
            r[k] -= alpha * ap[k];
        }
        for k in 0..m {
            z[k] = r[k] / csr.diag[k];
        }
        let rz2: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
        let beta = rz2 / rz;
        rz = rz2;
        for k in 0..m {
            p[k] = z[k] + beta * p[k];
        }
    }
    for (k, i) in idx.iter().enumerate() {
        x[*i] = xf[k];
    }
}

/// The reduced system `A_ff x_f = b_f − A_fc x_c` as a banded Cholesky factor, in reverse
/// Cuthill-McKee order of the free unknowns.
struct Factor {
    /// Factor row → unknown.
    idx: Vec<usize>,
    /// Half bandwidth.
    p: usize,
    /// Row `i` holds `L[i][i - p ..= i]`.
    l: Vec<f64>,
    /// Couplings to fixed unknowns: (factor row, fixed unknown, value).
    fixed: Vec<(usize, usize, f64)>,
}

/// Largest factor kept in memory, in values.
const MAX_FACTOR: usize = 40_000_000;

impl Factor {
    /// `None` when the band would be too large or the matrix isn't numerically positive definite.
    fn new(a: &Sparse, free: &[bool]) -> Option<Factor> {
        let n = a.rows.len();
        let degree = |i: usize| a.rows[i].keys().filter(|j| free[**j]).count();
        let mut seen = vec![false; n];
        let mut idx = Vec::with_capacity(n);
        let mut starts: Vec<usize> = (0..n).filter(|i| free[*i]).collect();
        starts.sort_by_key(|i| degree(*i));
        for s in starts {
            if seen[s] {
                continue;
            }
            seen[s] = true;
            let mut head = idx.len();
            idx.push(s);
            while let Some(&v) = idx.get(head) {
                head += 1;
                let mut next: Vec<usize> = a.rows[v].keys().copied().filter(|j| free[*j] && !seen[*j]).collect();
                next.sort_by_key(|j| degree(*j));
                for j in next {
                    seen[j] = true;
                    idx.push(j);
                }
            }
        }
        idx.reverse();
        let m = idx.len();
        let mut pos = vec![usize::MAX; n];
        for (k, i) in idx.iter().enumerate() {
            pos[*i] = k;
        }
        let mut p = 0;
        for (k, &i) in idx.iter().enumerate() {
            for j in a.rows[i].keys() {
                if free[*j] {
                    p = p.max(k.abs_diff(pos[*j]));
                }
            }
        }
        let w = p + 1;
        if m.checked_mul(w)? > MAX_FACTOR {
            return None;
        }
        let mut l = vec![0.0; m * w];
        let mut fixed = vec![];
        for (k, &i) in idx.iter().enumerate() {
            for (&j, &v) in &a.rows[i] {
                if !free[j] {
                    fixed.push((k, j, v));
                } else if pos[j] <= k {
                    l[k * w + pos[j] + p - k] = v;
                }
            }
        }
        for i in 0..m {
            let lo = i.saturating_sub(p);
            for j in lo..=i {
                let mut s = l[i * w + j + p - i];
                for k in j.saturating_sub(p).max(lo)..j {
                    s -= l[i * w + k + p - i] * l[j * w + k + p - j];
                }
                if i == j {
                    if s <= 0.0 || !s.is_finite() {
                        return None;
                    }
                    l[i * w + p] = s.sqrt();
                } else {
                    l[i * w + j + p - i] = s / l[j * w + p];
                }
            }
        }
        Some(Factor { idx, p, l, fixed })
    }

    fn solve(&self, b: &[f64], x: &mut [f64]) {
        let (m, p, w) = (self.idx.len(), self.p, self.p + 1);
        let mut y: Vec<f64> = self.idx.iter().map(|i| b[*i]).collect();
        for &(k, j, v) in &self.fixed {
            y[k] -= v * x[j];
        }
        for i in 0..m {
            let mut s = y[i];
            for k in i.saturating_sub(p)..i {
                s -= self.l[i * w + k + p - i] * y[k];
            }
            y[i] = s / self.l[i * w + p];
        }
        for i in (0..m).rev() {
            let mut s = y[i];
            for k in i + 1..(i + w).min(m) {
                s -= self.l[k * w + i + p - k] * y[k];
            }
            y[i] = s / self.l[i * w + p];
        }
        for (k, i) in self.idx.iter().enumerate() {
            x[*i] = y[k];
        }
    }
}

/// One system of the solve: the factor if it could be built, else conjugate gradients.
struct System {
    a: Sparse,
    free: Vec<bool>,
    factor: Option<Factor>,
}

impl System {
    fn new(a: Sparse, free: Vec<bool>) -> System {
        let factor = Factor::new(&a, &free);
        System { a, free, factor }
    }

    fn solve(&self, b: &[f64], x: &mut [f64]) {
        match &self.factor {
            Some(f) => f.solve(b, x),
            None => solve(&self.a, b, x, &self.free),
        }
    }
}

/// Half the cotangents of a triangle's angles: `[k]` weighs the edge opposite corner `k`.
/// Degenerate triangles weigh nothing.
fn cot_weights(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> [f64; 3] {
    let cross = ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs();
    if cross <= 1e-12 {
        return [0.0; 3];
    }
    let half_cot = |o: [f64; 2], u: [f64; 2], v: [f64; 2]| 0.5 * ((u[0] - o[0]) * (v[0] - o[0]) + (u[1] - o[1]) * (v[1] - o[1])) / cross;
    [half_cot(a, b, c), half_cot(b, c, a), half_cot(c, a, b)]
}

const REG: f64 = 1e-9;

/// How far bending carries a pin's pull, against the mesh's size. Higher is smoother at the pins
/// and spreads further; lower folds the mesh at the pins.
const BEND_LENGTH: f64 = 0.0675;
/// How fast displacement dies out beyond the pins, against the mesh's size. Higher lets the shape
/// grow and drift; lower squeezes the mesh between the pins and the outline until it folds.
const SETTLE_LENGTH: f64 = 0.12;
/// How far from the pins that sets in, against the mesh's size.
const SETTLE_FROM: f64 = 0.28;

/// How far a triangle's target gives way towards its current shape at its rest area (1 would offer no
/// resistance to shear), and how hard its target pushes back against a change of area.
const FIT_GIVE: f64 = 0.75;
const FIT_PRESSURE: f64 = 0.5;
/// Bounds on what the fit reads from a triangle, so a collapsed one can't throw its target far.
const FIT_SMALLEST: f64 = 0.05;
const FIT_MOST_SHAPE: f64 = 25.0;
/// Stiffening per squared log of a triangle's area ratio, past a slack of that log. Much more makes
/// the passes swing rather than settle.
const AREA_STIFFNESS: f64 = 16.0;
const AREA_SLACK: f64 = 0.1;

/// Smallest stretch at which a triangle starts getting stiffer. Higher spreads compression further
/// and costs more of the shape's volume and locality.
const BARRIER_FROM: f64 = 0.7;
/// Stretch that counts as fully collapsed (or flipped), which bounds the weight.
const BARRIER_FLOOR: f64 = 0.03;
/// Times the weights are updated, each with a new factor, and rounds on each set of weights.
const BARRIER_PASSES: usize = 4;
const BARRIER_ROUNDS: usize = 3;

/// Stiffness of a point held inside a triangle, against the mesh's own (Laplacian and bending).
const POINT_STIFFNESS: f64 = 1e5;

fn point_at(p: &[[f64; 2]], q: &PointTarget) -> [f64; 2] {
    let [a, b, c] = q.verts;
    let [l0, l1, l2] = q.bary;
    [l0 * p[a][0] + l1 * p[b][0] + l2 * p[c][0], l0 * p[a][1] + l1 * p[b][1] + l2 * p[c][1]]
}

/// Deform a mesh: `rest` vertices, CCW `tris` with per-triangle `weights` (1 = normal,
/// larger = stiffer), constraints, and `refine` extra rigid-fit iterations. Returns the
/// deformed vertex positions.
pub fn deform(rest: &[[f64; 2]], tris: &[[usize; 3]], weights: &[f64], h: &Handles, refine: usize) -> Vec<[f64; 2]> {
    let n = rest.len();
    if n == 0 {
        return vec![];
    }
    // Initial guess: rest translated by the mean handle displacement.
    let mut t = [0.0, 0.0];
    for (i, p) in &h.fixed {
        t[0] += p[0] - rest[*i][0];
        t[1] += p[1] - rest[*i][1];
    }
    for q in &h.points {
        let r = point_at(rest, q);
        t[0] += q.position[0] - r[0];
        t[1] += q.position[1] - r[1];
    }
    let handles = h.fixed.len() + h.points.len();
    if handles > 0 {
        t[0] /= handles as f64;
        t[1] /= handles as f64;
    }
    let x0: Vec<[f64; 2]> = rest.iter().map(|p| [p[0] + t[0], p[1] + t[1]]).collect();
    // One pin and no edge targets: every rotation and scale about the pin costs nothing, and
    // the regulariser that picks the translation is too weak to stop CG turning round-off
    // into a visible rotation (#313). The translation is the exact solution.
    if handles == 1 && h.edges.is_empty() {
        return x0;
    }
    let mut fixed = vec![false; n];
    for (i, _) in &h.fixed {
        fixed[*i] = true;
    }
    let w = |k: usize| weights.get(k).copied().unwrap_or(1.0);
    let scale2 = mean_edge2(rest, tris).max(1e-12);

    // ---- Step 1: similarity-invariant energy over 2n unknowns (x0, y0, x1, y1, …).
    let mut a = Sparse::new(2 * n);
    let mut b = vec![0.0; 2 * n];
    for (k, t) in tris.iter().enumerate() {
        let wk = w(k);
        for (i, j, l) in [(t[0], t[1], t[2]), (t[1], t[2], t[0]), (t[2], t[0], t[1])] {
            let e = [rest[j][0] - rest[i][0], rest[j][1] - rest[i][1]];
            let d = [rest[l][0] - rest[i][0], rest[l][1] - rest[i][1]];
            let ee = e[0] * e[0] + e[1] * e[1];
            if ee <= 1e-18 {
                continue;
            }
            // R90(u) = (-u.y, u.x).
            let r90 = [-e[1], e[0]];
            let x = (d[0] * e[0] + d[1] * e[1]) / ee;
            let y = (d[0] * r90[0] + d[1] * r90[1]) / ee;
            // residual = B_l v_l + B_i v_i + B_j v_j with 2x2 blocks; R = [[0,-1],[1,0]].
            let bl = [[1.0, 0.0], [0.0, 1.0]];
            let bi = [[x - 1.0, -y], [y, x - 1.0]];
            let bj = [[-x, y], [-y, -x]];
            let blocks = [(l, bl), (i, bi), (j, bj)];
            for (va, ba) in &blocks {
                for (vb, bb) in &blocks {
                    // ba^T bb
                    for r in 0..2 {
                        for c in 0..2 {
                            let v = ba[0][r] * bb[0][c] + ba[1][r] * bb[1][c];
                            if v != 0.0 {
                                a.add(2 * va + r, 2 * vb + c, wk * v);
                            }
                        }
                    }
                }
            }
        }
    }
    for e in &h.edges {
        let ww = e.weight;
        for c in 0..2 {
            a.add(2 * e.i + c, 2 * e.i + c, ww);
            a.add(2 * e.j + c, 2 * e.j + c, ww);
            a.add(2 * e.i + c, 2 * e.j + c, -ww);
            a.add(2 * e.j + c, 2 * e.i + c, -ww);
            b[2 * e.j + c] += ww * e.vec[c];
            b[2 * e.i + c] -= ww * e.vec[c];
        }
    }
    // Stiff enough to land within a small fraction of a pixel, against either system's diagonal.
    let area: f64 = tris.iter().map(|t| tri_area(rest, t)).sum();
    let mut pins_from: Vec<[f64; 2]> = h.fixed.iter().map(|(i, _)| rest[*i]).collect();
    let mut pins_to: Vec<[f64; 2]> = h.fixed.iter().map(|(_, p)| *p).collect();
    pins_from.extend(h.points.iter().map(|q| point_at(rest, q)));
    pins_to.extend(h.points.iter().map(|q| q.position));
    let size = extent(rest);
    let bend = (BEND_LENGTH * size).powi(2);
    let hold = POINT_STIFFNESS * (1.0 + bend / (area / tris.len().max(1) as f64));
    add_points_interleaved(&mut a, &mut b, &h.points, hold);
    let reg = REG * scale2.recip().min(1.0);
    for i in 0..n {
        for c in 0..2 {
            a.add(2 * i + c, 2 * i + c, reg);
            b[2 * i + c] += reg * x0[i][c];
        }
    }
    let mut x: Vec<f64> = x0.iter().flat_map(|p| [p[0], p[1]]).collect();
    for (i, p) in &h.fixed {
        x[2 * i] = p[0];
        x[2 * i + 1] = p[1];
    }
    let free2: Vec<bool> = (0..2 * n).map(|k| !fixed[k / 2]).collect();
    System::new(a, free2).solve(&b, &mut x);
    let mut cur: Vec<[f64; 2]> = (0..n).map(|i| [x[2 * i], x[2 * i + 1]]).collect();

    // ---- Step 2: fit rotations, then match edge vectors (x and y separately).
    let cots: Vec<[f64; 3]> = tris.iter().map(|t| cot_weights(rest[t[0]], rest[t[1]], rest[t[2]])).collect();
    // Corners of an edge of a triangle, and the corner opposite it.
    const EDGES: [(usize, usize, usize); 3] = [(1, 2, 0), (2, 0, 1), (0, 1, 2)];
    let mut base = Sparse::new(n);
    for e in &h.edges {
        base.add(e.i, e.i, e.weight);
        base.add(e.j, e.j, e.weight);
        base.add(e.i, e.j, -e.weight);
        base.add(e.j, e.i, -e.weight);
    }
    for i in 0..n {
        base.add(i, i, reg);
    }
    add_bending(&mut base, rest, tris, bend);
    let settle = settling(rest, tris, &pins_from, &pins_to, size);
    for (i, (weight, _)) in settle.iter().enumerate() {
        base.add(i, i, *weight);
    }
    add_points(&mut base, &h.points, hold);
    let free: Vec<bool> = (0..n).map(|i| !fixed[i]).collect();
    let assemble = |tw: &[f64]| {
        let mut l = base.clone();
        for (k, t) in tris.iter().enumerate() {
            for (a2, b2, o) in EDGES {
                let (i, j, c) = (t[a2], t[b2], tw[k] * cots[k][o]);
                l.add(i, i, c);
                l.add(j, j, c);
                l.add(i, j, -c);
                l.add(j, i, -c);
            }
        }
        System::new(l, free.clone())
    };
    let round = |system: &System, tw: &[f64], cur: &[[f64; 2]]| -> Vec<[f64; 2]> {
        let mut bx = vec![0.0; n];
        let mut by = vec![0.0; n];
        for (k, t) in tris.iter().enumerate() {
            let f = fit_triangle([rest[t[0]], rest[t[1]], rest[t[2]]], [cur[t[0]], cur[t[1]], cur[t[2]]], w(k));
            for (a2, b2, o) in EDGES {
                let (i, j, c) = (t[a2], t[b2], tw[k] * cots[k][o]);
                let d = [f[b2][0] - f[a2][0], f[b2][1] - f[a2][1]];
                bx[j] += c * d[0];
                bx[i] -= c * d[0];
                by[j] += c * d[1];
                by[i] -= c * d[1];
            }
        }
        for e in &h.edges {
            bx[e.j] += e.weight * e.vec[0];
            bx[e.i] -= e.weight * e.vec[0];
            by[e.j] += e.weight * e.vec[1];
            by[e.i] -= e.weight * e.vec[1];
        }
        add_points_rhs(&mut bx, &mut by, &h.points, hold);
        for i in 0..n {
            bx[i] += reg * x0[i][0] + settle[i].0 * settle[i].1[0];
            by[i] += reg * x0[i][1] + settle[i].0 * settle[i].1[1];
        }
        let mut xs: Vec<f64> = cur.iter().map(|p| p[0]).collect();
        let mut ys: Vec<f64> = cur.iter().map(|p| p[1]).collect();
        for (i, p) in &h.fixed {
            xs[*i] = p[0];
            ys[*i] = p[1];
        }
        system.solve(&bx, &mut xs);
        system.solve(&by, &mut ys);
        (0..n).map(|i| [xs[i], ys[i]]).collect()
    };
    let mut tw: Vec<f64> = (0..tris.len()).map(w).collect();
    let system = assemble(&tw);
    for _ in 0..=refine {
        cur = round(&system, &tw, &cur);
    }
    // Triangles close to collapse or far from their area get stiffer, and the strain moves on to
    // their neighbours.
    for _ in 0..BARRIER_PASSES {
        let mut active = false;
        for (k, t) in tris.iter().enumerate() {
            let stretch = smallest_stretch([rest[t[0]], rest[t[1]], rest[t[2]]], [cur[t[0]], cur[t[1]], cur[t[2]]]);
            let area_ratio = (tri_area(&cur, t) / tri_area(rest, t).max(1e-12)).max(0.01);
            let off = (area_ratio.ln().abs() - AREA_SLACK).max(0.0);
            let next = w(k) * barrier_weight(stretch) * (1.0 + AREA_STIFFNESS * off * off);
            active |= next != w(k) || tw[k] != w(k);
            // Half way there in logs: all the way overshoots, and the weights swing.
            tw[k] = (tw[k] * next).sqrt();
        }
        if !active {
            break;
        }
        let system = assemble(&tw);
        for _ in 0..BARRIER_ROUNDS {
            cur = round(&system, &tw, &cur);
        }
    }
    cur
}

/// The smaller singular value of a triangle's deformation gradient, negative if it's flipped.
fn smallest_stretch(rest: [[f64; 2]; 3], cur: [[f64; 2]; 3]) -> f64 {
    let g = gradient(rest, cur);
    let (q, r) = (f64::hypot(g[0] + g[3], g[2] - g[1]) / 2.0, f64::hypot(g[0] - g[3], g[2] + g[1]) / 2.0);
    if g[0] * g[3] - g[1] * g[2] < 0.0 { -(q - r).abs() } else { (q - r).abs() }
}

/// How much stiffer a triangle gets at smallest stretch `s`. Rigidity alone pays a bounded price for
/// squashing a triangle flat or through itself, so the solve would rather fold the mesh at a pin than
/// compress what's ahead of it. Below [`BARRIER_FROM`] this follows the curvature of the symmetric
/// Dirichlet energy against rigidity's (the reweighting of Rabinovich et al., "Scalable Locally
/// Injective Mappings", 2017, as a single weight per triangle so x and y stay separate systems), which
/// grows as `s⁻³`. Above it nothing changes, so gentle drags stay as rigid as they were.
fn barrier_weight(s: f64) -> f64 {
    let curvature = |x: f64| (x - x.powi(-3)) / (x - 1.0);
    let s = s.max(BARRIER_FLOOR);
    if s >= BARRIER_FROM { 1.0 } else { curvature(s) / curvature(BARRIER_FROM) }
}

/// The longer side of the mesh's bounding box.
fn extent(rest: &[[f64; 2]]) -> f64 {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in rest {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    (x1 - x0).max(y1 - y0).max(1e-9)
}

/// For each vertex, the weight of the pull towards the pins' best rigid fit, and where that fit puts
/// the vertex. The weight is the vertex's share of the mesh's area over the settling length squared,
/// fading to nothing at the pins.
fn settling(rest: &[[f64; 2]], tris: &[[usize; 3]], from: &[[f64; 2]], to: &[[f64; 2]], size: f64) -> Vec<(f64, [f64; 2])> {
    let mut out = vec![(0.0, [0.0, 0.0]); rest.len()];
    if from.is_empty() {
        return out;
    }
    let k = from.len() as f64;
    let cf = from.iter().fold([0.0, 0.0], |a, p| [a[0] + p[0] / k, a[1] + p[1] / k]);
    let ct = to.iter().fold([0.0, 0.0], |a, p| [a[0] + p[0] / k, a[1] + p[1] / k]);
    let (mut sd, mut sc) = (0.0, 0.0);
    for (p, q) in from.iter().zip(to) {
        let (u, v) = ([p[0] - cf[0], p[1] - cf[1]], [q[0] - ct[0], q[1] - ct[1]]);
        sd += u[0] * v[0] + u[1] * v[1];
        sc += u[0] * v[1] - u[1] * v[0];
    }
    let (sn, cs) = if f64::hypot(sd, sc) > 1e-9 { f64::atan2(sc, sd).sin_cos() } else { (0.0, 1.0) };
    let mu = (SETTLE_LENGTH * size).powi(-2);
    for t in tris {
        let share = mu * tri_area(rest, t) / 3.0;
        for i in t {
            out[*i].0 += share;
        }
    }
    let far = SETTLE_FROM * size;
    for (o, r) in out.iter_mut().zip(rest) {
        let u = [r[0] - cf[0], r[1] - cf[1]];
        o.1 = [ct[0] + cs * u[0] - sn * u[1], ct[1] + sn * u[0] + cs * u[1]];
        let d = from.iter().map(|p| (p[0] - r[0]).hypot(p[1] - r[1])).fold(f64::MAX, f64::min);
        o.0 *= 1.0 - (-(d / far).powi(2)).exp();
    }
    out
}

/// The held points' terms in the system over (x0, y0, x1, y1, …).
fn add_points_interleaved(a: &mut Sparse, b: &mut [f64], points: &[PointTarget], hold: f64) {
    for q in points {
        for (u, lu) in q.verts.iter().zip(q.bary) {
            for c in 0..2 {
                for (v, lv) in q.verts.iter().zip(q.bary) {
                    a.add(2 * u + c, 2 * v + c, hold * lu * lv);
                }
                b[2 * u + c] += hold * lu * q.position[c];
            }
        }
    }
}

/// The held points' terms in a per-axis matrix.
fn add_points(a: &mut Sparse, points: &[PointTarget], hold: f64) {
    for q in points {
        for (u, lu) in q.verts.iter().zip(q.bary) {
            for (v, lv) in q.verts.iter().zip(q.bary) {
                a.add(*u, *v, hold * lu * lv);
            }
        }
    }
}

/// The held points' terms in the per-axis right-hand sides.
fn add_points_rhs(bx: &mut [f64], by: &mut [f64], points: &[PointTarget], hold: f64) {
    for q in points {
        for (u, lu) in q.verts.iter().zip(q.bary) {
            bx[*u] += hold * lu * q.position[0];
            by[*u] += hold * lu * q.position[1];
        }
    }
}

fn tri_area(rest: &[[f64; 2]], t: &[usize; 3]) -> f64 {
    0.5 * ((rest[t[1]][0] - rest[t[0]][0]) * (rest[t[2]][1] - rest[t[0]][1]) - (rest[t[1]][1] - rest[t[0]][1]) * (rest[t[2]][0] - rest[t[0]][0])).abs()
}

/// Add `lambda` times the bending energy to a per-axis system. Across an inner edge of length `l`
/// the gradient of a piecewise-linear map jumps by `j` along the edge's normal, and the energy of
/// the edge is `3 l² / (A1 + A2) |j|²`.
fn add_bending(a: &mut Sparse, rest: &[[f64; 2]], tris: &[[usize; 3]], lambda: f64) {
    let mut by_edge: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();
    for (k, t) in tris.iter().enumerate() {
        for (i, j) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            by_edge.entry((i.min(j), i.max(j))).or_default().push(k);
        }
    }
    // The gradient of the linear function that is 1 at corner `k` and 0 at the others.
    let corner_gradient = |t: &[usize; 3], k: usize| {
        let (o, p, q) = (rest[t[k]], rest[t[(k + 1) % 3]], rest[t[(k + 2) % 3]]);
        let n = [p[1] - q[1], q[0] - p[0]];
        let at = n[0] * (o[0] - p[0]) + n[1] * (o[1] - p[1]);
        if at.abs() < 1e-12 { [0.0, 0.0] } else { [n[0] / at, n[1] / at] }
    };
    for ((i, j), sides) in &by_edge {
        let [ka, kb] = sides[..] else { continue };
        let (ta, tb) = (&tris[ka], &tris[kb]);
        let e = [rest[*j][0] - rest[*i][0], rest[*j][1] - rest[*i][1]];
        let len = e[0].hypot(e[1]);
        let both = tri_area(rest, ta) + tri_area(rest, tb);
        if len < 1e-12 || both < 1e-12 {
            continue;
        }
        let normal = [-e[1] / len, e[0] / len];
        let w = lambda * 3.0 * len * len / both;
        // The jump as a sum over the four vertices of the diamond.
        let mut jump: Vec<(usize, f64)> = Vec::with_capacity(4);
        for (t, sign) in [(ta, 1.0), (tb, -1.0)] {
            for k in 0..3 {
                let g = corner_gradient(t, k);
                let c = sign * (g[0] * normal[0] + g[1] * normal[1]);
                match jump.iter_mut().find(|s| s.0 == t[k]) {
                    Some(s) => s.1 += c,
                    None => jump.push((t[k], c)),
                }
            }
        }
        for (u, cu) in &jump {
            for (v, cv) in &jump {
                a.add(*u, *v, w * cu * cv);
            }
        }
    }
}

fn mean_edge2(rest: &[[f64; 2]], tris: &[[usize; 3]]) -> f64 {
    if tris.is_empty() {
        return 1.0;
    }
    let mut s = 0.0;
    for t in tris {
        let (a, b) = (rest[t[0]], rest[t[1]]);
        s += (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2);
    }
    s / tris.len() as f64
}

/// The target for triangle `cur`: its rest shape about the centroid, turned by the rotation of its
/// deformation gradient `F`, with singular values `s1 >= s2`. A rotation alone resists a squeeze and a
/// bulge alike, so nothing makes material squeezed one way spread the other. So the target gives way
/// towards the shape it has now at the same area (`t1 t2 = 1`, with `t1 / t2` a power of `s1 / s2`),
/// and takes on a pressure against the area it has lost or gained (a power of `s1 s2`). Stiffer
/// triangles (`weight`, Starch) give less.
pub fn fit_triangle(rest: [[f64; 2]; 3], cur: [[f64; 2]; 3], weight: f64) -> [[f64; 2]; 3] {
    let cr = [(rest[0][0] + rest[1][0] + rest[2][0]) / 3.0, (rest[0][1] + rest[1][1] + rest[2][1]) / 3.0];
    let cc = [(cur[0][0] + cur[1][0] + cur[2][0]) / 3.0, (cur[0][1] + cur[1][1] + cur[2][1]) / 3.0];
    let [fa, fb, fc, fd] = gradient(rest, cur);
    // F = q Rot(a2) + r Reflect(a1), with s1 = q + r and s2 = |q - r|.
    let (q, r) = (f64::hypot(fa + fd, fc - fb) / 2.0, f64::hypot(fa - fd, fc + fb) / 2.0);
    let (a2, a1) = (f64::atan2(fc - fb, fa + fd), f64::atan2(fc + fb, fa - fd));
    let (s1, s2) = (q + r, (q - r).abs().max(FIT_SMALLEST));
    let t1 = (s1 / s2).clamp(1.0, FIT_MOST_SHAPE).powf(FIT_GIVE / 2.0 / weight.max(1.0));
    let pressure = (s1 * s2).clamp(FIT_MOST_SHAPE.recip(), FIT_MOST_SHAPE).powf(-FIT_PRESSURE / 2.0);
    let (qt, rt) = (pressure * (t1 + t1.recip()) / 2.0, pressure * (t1 - t1.recip()) / 2.0);
    let (sn2, cs2) = a2.sin_cos();
    let (sn1, cs1) = a1.sin_cos();
    let t = [qt * cs2 + rt * cs1, rt * sn1 - qt * sn2, qt * sn2 + rt * sn1, qt * cs2 - rt * cs1];
    let mut out = [[0.0; 2]; 3];
    for k in 0..3 {
        let p = [rest[k][0] - cr[0], rest[k][1] - cr[1]];
        out[k] = [cc[0] + t[0] * p[0] + t[1] * p[1], cc[1] + t[2] * p[0] + t[3] * p[1]];
    }
    out
}

/// A triangle's deformation gradient, row by row; the identity for a degenerate rest triangle.
fn gradient(rest: [[f64; 2]; 3], cur: [[f64; 2]; 3]) -> [f64; 4] {
    let (e1, e2) = ([rest[1][0] - rest[0][0], rest[1][1] - rest[0][1]], [rest[2][0] - rest[0][0], rest[2][1] - rest[0][1]]);
    let (f1, f2) = ([cur[1][0] - cur[0][0], cur[1][1] - cur[0][1]], [cur[2][0] - cur[0][0], cur[2][1] - cur[0][1]]);
    let det = e1[0] * e2[1] - e2[0] * e1[1];
    if det.abs() < 1e-12 {
        return [1.0, 0.0, 0.0, 1.0];
    }
    [(f1[0] * e2[1] - f2[0] * e1[1]) / det, (f2[0] * e1[0] - f1[0] * e2[0]) / det, (f1[1] * e2[1] - f2[1] * e1[1]) / det, (f2[1] * e1[0] - f1[1] * e2[0]) / det]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bending_costs_nothing_for_an_affine_map() {
        // Two triangles of different shapes sharing an edge.
        let rest = [[0.0, 0.0], [10.0, 1.0], [4.0, 9.0], [7.0, -8.0]];
        let tris = [[0, 1, 2], [1, 0, 3]];
        let mut a = Sparse::new(4);
        add_bending(&mut a, &rest, &tris, 1.0);
        let mapped = rest.map(|p| [1.3 * p[0] - 0.4 * p[1] + 5.0, 0.2 * p[0] + 0.9 * p[1] - 2.0]);
        for c in 0..2 {
            let x: Vec<f64> = mapped.iter().map(|p| p[c]).collect();
            for r in &a.rows {
                let y: f64 = r.iter().map(|(j, v)| v * x[*j]).sum();
                assert!(y.abs() < 1e-9, "{y}");
            }
        }
        // A crease does cost: lift the far vertex off the plane of the map.
        let x = [0.0, 10.0, 4.0, 12.0];
        let e: f64 = a.rows.iter().enumerate().map(|(i, r)| x[i] * r.iter().map(|(j, v)| v * x[*j]).sum::<f64>()).sum();
        assert!(e > 1e-3, "{e}");
    }

    #[test]
    fn the_factor_solves_what_conjugate_gradients_solve() {
        // A path of springs with one end held.
        let n = 40;
        let mut a = Sparse::new(n);
        for i in 0..n - 1 {
            a.add(i, i, 2.0);
            a.add(i + 1, i + 1, 2.0);
            a.add(i, i + 1, -2.0);
            a.add(i + 1, i, -2.0);
        }
        let b: Vec<f64> = (0..n).map(|i| (i as f64 * 0.7).sin()).collect();
        let free: Vec<bool> = (0..n).map(|i| i != 7).collect();
        let (mut x, mut y) = (vec![0.0; n], vec![0.0; n]);
        x[7] = 3.0;
        y[7] = 3.0;
        let f = Factor::new(&a, &free).unwrap();
        f.solve(&b, &mut x);
        solve(&a, &b, &mut y, &free);
        assert_eq!(x[7], 3.0);
        for (p, q) in x.iter().zip(&y) {
            assert!((p - q).abs() < 1e-8, "{p} vs {q}");
        }
    }

    #[test]
    fn a_singular_matrix_has_no_factor_and_the_solve_still_runs() {
        let mut a = Sparse::new(2);
        a.add(0, 0, 1.0);
        a.add(1, 1, 1.0);
        a.add(0, 1, -1.0);
        a.add(1, 0, -1.0);
        let s = System::new(a, vec![true, true]);
        assert!(s.factor.is_none());
        let mut x = vec![1.0, 1.0];
        s.solve(&[0.0, 0.0], &mut x);
        assert!(x.iter().all(|v| v.is_finite()));
    }
}
