//! Puppet meshes: the layer's alpha outline → contour → simplified polygon → expansion →
//! constrained triangulation with density control.
//!
//! * The alpha channel is thresholded at 50 % and grown (or shrunk) by Expansion with an exact
//!   Euclidean distance transform (the lower-envelope-of-parabolas method of Felzenszwalb &
//!   Huttenlocher, "Distance Transforms of Sampled Functions").
//! * The 4-connected region under the click (holes filled) is traced along pixel cracks into a
//!   closed polygon, simplified with Douglas–Peucker and split so no edge is longer than the
//!   target edge length.
//! * Interior points on a jittered hexagonal lattice are added, everything is triangulated with
//!   Bowyer–Watson Delaunay insertion, missing outline edges are recovered by edge flips (the
//!   textbook constrained-Delaunay edge-recovery procedure), and triangles outside the outline
//!   are dropped.

use std::collections::{BTreeMap, HashMap, VecDeque};

/// A triangle mesh in layer space. Triangles are counter-clockwise in the (x right, y down)
/// layer coordinates, i.e. `cross(b - a, c - a) > 0`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub verts: Vec<[f64; 2]>,
    pub tris: Vec<[usize; 3]>,
    /// The outline polygon (layer space).
    pub outline: Vec<[f64; 2]>,
}

/// Binary coverage grid in buffer pixels.
#[derive(Clone, Debug)]
pub struct Grid {
    pub w: usize,
    pub h: usize,
    pub on: Vec<bool>,
}

impl Grid {
    #[inline]
    pub fn at(&self, x: i64, y: i64) -> bool {
        x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h && self.on[y as usize * self.w + x as usize]
    }
}

pub fn cross(o: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

fn dist2(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}

/// 1D squared-distance transform of `f` (Felzenszwalb & Huttenlocher).
fn dt1(f: &[f64], out: &mut [f64]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let mut v = vec![0usize; n];
    let mut z = vec![0.0f64; n + 1];
    let mut k = 0usize;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    let inter = |q: usize, p: usize| ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64)) / (2.0 * q as f64 - 2.0 * p as f64);
    for q in 1..n {
        let mut s = inter(q, v[k]);
        while s <= z[k] {
            k -= 1;
            s = inter(q, v[k]);
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    let mut k = 0;
    for (q, o) in out.iter_mut().enumerate() {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let p = v[k];
        *o = (q as f64 - p as f64).powi(2) + f[p];
    }
}

/// Squared Euclidean distance from every cell to the nearest cell where `src` is true.
pub fn distance_sq(w: usize, h: usize, src: &[bool]) -> Vec<f64> {
    let inf = 1e20;
    let mut g = vec![inf; w * h];
    let mut col = vec![0.0; h];
    let mut colo = vec![0.0; h];
    for x in 0..w {
        for y in 0..h {
            col[y] = if src[y * w + x] { 0.0 } else { inf };
        }
        dt1(&col, &mut colo);
        for y in 0..h {
            g[y * w + x] = colo[y];
        }
    }
    let mut out = vec![0.0; w * h];
    for y in 0..h {
        dt1(&g[y * w..(y + 1) * w], &mut out[y * w..(y + 1) * w]);
    }
    out
}

/// Grow (`e > 0`) or shrink (`e < 0`) a grid by `e` cells.
pub fn expand(g: &Grid, e: f64) -> Grid {
    if e.abs() < 1e-9 {
        return g.clone();
    }
    if e > 0.0 {
        let d = distance_sq(g.w, g.h, &g.on);
        Grid { w: g.w, h: g.h, on: d.iter().map(|v| *v <= e * e).collect() }
    } else {
        let off: Vec<bool> = g.on.iter().map(|b| !b).collect();
        let d = distance_sq(g.w, g.h, &off);
        Grid { w: g.w, h: g.h, on: d.iter().zip(&g.on).map(|(v, on)| *on && *v > e * e).collect() }
    }
}

/// The 4-connected region containing (or nearest to) `seed`, with holes filled.
pub fn region(g: &Grid, seed: [f64; 2]) -> Option<Grid> {
    let (w, h) = (g.w, g.h);
    if w == 0 || h == 0 {
        return None;
    }
    let sx = (seed[0].floor() as i64).clamp(0, w as i64 - 1);
    let sy = (seed[1].floor() as i64).clamp(0, h as i64 - 1);
    let start = if g.at(sx, sy) {
        (sx as usize, sy as usize)
    } else {
        // Nearest on-cell.
        let mut best: Option<(i64, usize, usize)> = None;
        for y in 0..h {
            for x in 0..w {
                if g.on[y * w + x] {
                    let d = (x as i64 - sx).pow(2) + (y as i64 - sy).pow(2);
                    if best.is_none_or(|b| d < b.0) {
                        best = Some((d, x, y));
                    }
                }
            }
        }
        let b = best?;
        (b.1, b.2)
    };
    let mut on = vec![false; w * h];
    let mut q = VecDeque::new();
    on[start.1 * w + start.0] = true;
    q.push_back(start);
    while let Some((x, y)) = q.pop_front() {
        let mut visit = |nx: i64, ny: i64| {
            if g.at(nx, ny) && !on[ny as usize * w + nx as usize] {
                on[ny as usize * w + nx as usize] = true;
                q.push_back((nx as usize, ny as usize));
            }
        };
        visit(x as i64 - 1, y as i64);
        visit(x as i64 + 1, y as i64);
        visit(x as i64, y as i64 - 1);
        visit(x as i64, y as i64 + 1);
    }
    // Fill holes: everything not reachable from the border through off-cells.
    let mut outside = vec![false; w * h];
    let mut q = VecDeque::new();
    for x in 0..w {
        for y in [0, h - 1] {
            if !on[y * w + x] && !outside[y * w + x] {
                outside[y * w + x] = true;
                q.push_back((x, y));
            }
        }
    }
    for y in 0..h {
        for x in [0, w - 1] {
            if !on[y * w + x] && !outside[y * w + x] {
                outside[y * w + x] = true;
                q.push_back((x, y));
            }
        }
    }
    while let Some((x, y)) = q.pop_front() {
        for (dx, dy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
            let (nx, ny) = (x as i64 + dx, y as i64 + dy);
            if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                continue;
            }
            let i = ny as usize * w + nx as usize;
            if !on[i] && !outside[i] {
                outside[i] = true;
                q.push_back((nx as usize, ny as usize));
            }
        }
    }
    Some(Grid { w, h, on: outside.iter().map(|o| !o).collect() })
}

/// Trace the outer boundary of a hole-free region along pixel cracks: a closed polygon through
/// pixel corners, region on the left when walking (counter-clockwise in y-down coordinates).
pub fn trace(g: &Grid) -> Vec<[f64; 2]> {
    // Directed boundary edges between corners; region on the right of the edge in y-down
    // screen terms (we fix orientation at the end anyway).
    let mut next: BTreeMap<(i64, i64), Vec<(i64, i64)>> = BTreeMap::new();
    for y in 0..g.h as i64 {
        for x in 0..g.w as i64 {
            if !g.at(x, y) {
                continue;
            }
            if !g.at(x, y - 1) {
                next.entry((x, y)).or_default().push((x + 1, y));
            }
            if !g.at(x + 1, y) {
                next.entry((x + 1, y)).or_default().push((x + 1, y + 1));
            }
            if !g.at(x, y + 1) {
                next.entry((x + 1, y + 1)).or_default().push((x, y + 1));
            }
            if !g.at(x - 1, y) {
                next.entry((x, y + 1)).or_default().push((x, y));
            }
        }
    }
    let mut best: Vec<[f64; 2]> = vec![];
    let mut best_area = 0.0;
    while let Some(&start) = next.iter().find(|(_, v)| !v.is_empty()).map(|(k, _)| k) {
        let mut lp = vec![start];
        let mut cur = start;
        let mut dir = (0i64, 0i64);
        let mut guard = 0;
        loop {
            guard += 1;
            if guard > 4 * (g.w + 2) * (g.h + 2) {
                break;
            }
            let Some(outs) = next.get_mut(&cur) else { break };
            if outs.is_empty() {
                break;
            }
            // At a saddle corner pick the right-most turn relative to the incoming direction
            // (keeps diagonal-touching parts separate).
            let k = if outs.len() == 1 || dir == (0, 0) {
                0
            } else {
                let score = |o: &(i64, i64)| {
                    let d = (o.0 - cur.0, o.1 - cur.1);
                    dir.0 * d.1 - dir.1 * d.0
                };
                (0..outs.len()).max_by_key(|i| score(&outs[*i])).unwrap_or(0)
            };
            let nx = outs.swap_remove(k);
            dir = (nx.0 - cur.0, nx.1 - cur.1);
            cur = nx;
            if cur == start {
                break;
            }
            lp.push(cur);
        }
        let pts: Vec<[f64; 2]> = lp.iter().map(|p| [p.0 as f64, p.1 as f64]).collect();
        let a = signed_area(&pts).abs();
        if a > best_area {
            best_area = a;
            best = pts;
        }
    }
    // Drop collinear corners.
    let n = best.len();
    if n < 3 {
        return best;
    }
    let mut out = vec![];
    for i in 0..n {
        let (a, b, c) = (best[(i + n - 1) % n], best[i], best[(i + 1) % n]);
        if cross(a, b, c).abs() > 1e-12 {
            out.push(b);
        }
    }
    out
}

pub fn signed_area(p: &[[f64; 2]]) -> f64 {
    let n = p.len();
    let mut a = 0.0;
    for i in 0..n {
        let (u, v) = (p[i], p[(i + 1) % n]);
        a += u[0] * v[1] - v[0] * u[1];
    }
    a * 0.5
}

fn seg_dist2(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let l = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if l > 0.0 { (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / l).clamp(0.0, 1.0) } else { 0.0 };
    dist2(p, [a[0] + ab[0] * t, a[1] + ab[1] * t])
}

/// Douglas–Peucker simplification of a closed polygon.
pub fn simplify(p: &[[f64; 2]], tol: f64) -> Vec<[f64; 2]> {
    let n = p.len();
    if n <= 4 {
        return p.to_vec();
    }
    // Split the loop at vertex 0 and the vertex farthest from it; simplify both open chains.
    let far = (1..n).max_by(|a, b| dist2(p[0], p[*a]).total_cmp(&dist2(p[0], p[*b]))).unwrap_or(n / 2);
    let mut ext = p.to_vec();
    ext.push(p[0]);
    let mut keep = vec![false; n + 1];
    let tol2 = tol * tol;
    let mut stack = vec![(0usize, far), (far, n)];
    keep[0] = true;
    keep[far] = true;
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let mut best = (0.0, 0);
        for i in a + 1..b {
            let d = seg_dist2(ext[i], ext[a], ext[b]);
            if d > best.0 {
                best = (d, i);
            }
        }
        if best.0 > tol2 {
            keep[best.1] = true;
            stack.push((a, best.1));
            stack.push((best.1, b));
        }
    }
    (0..n).filter(|i| keep[*i]).map(|i| p[i]).collect()
}

/// Split polygon edges so none is longer than `max_len`.
pub fn densify(p: &[[f64; 2]], max_len: f64) -> Vec<[f64; 2]> {
    let n = p.len();
    let mut out = vec![];
    for i in 0..n {
        let (a, b) = (p[i], p[(i + 1) % n]);
        out.push(a);
        let l = dist2(a, b).sqrt();
        let k = (l / max_len.max(1e-6)).ceil() as usize;
        for j in 1..k {
            let t = j as f64 / k as f64;
            out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
        }
    }
    out
}

/// Even-odd point-in-polygon test.
pub fn inside(poly: &[[f64; 2]], p: [f64; 2]) -> bool {
    let n = poly.len();
    let mut c = false;
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            c = !c;
        }
        j = i;
    }
    c
}

fn poly_dist2(poly: &[[f64; 2]], p: [f64; 2]) -> f64 {
    let n = poly.len();
    (0..n).map(|i| seg_dist2(p, poly[i], poly[(i + 1) % n])).fold(f64::MAX, f64::min)
}

/// Circumcircle test: is `d` strictly inside the circumcircle of CCW (`cross > 0`) triangle abc?
fn in_circle(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let (ax, ay) = (a[0] - d[0], a[1] - d[1]);
    let (bx, by) = (b[0] - d[0], b[1] - d[1]);
    let (cx, cy) = (c[0] - d[0], c[1] - d[1]);
    let (la, lb, lc) = (ax * ax + ay * ay, bx * bx + by * by, cx * cx + cy * cy);
    let det = la * (bx * cy - cx * by) - lb * (ax * cy - cx * ay) + lc * (ax * by - bx * ay);
    let m = la.max(lb).max(lc);
    det > 1e-12 * m * m
}

/// Bowyer–Watson Delaunay triangulation of `pts` (CCW triangles, `cross > 0`).
pub fn delaunay(pts: &[[f64; 2]]) -> Vec<[usize; 3]> {
    let n = pts.len();
    if n < 3 {
        return vec![];
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in pts {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    let d = (x1 - x0).max(y1 - y0).max(1.0) * 20.0;
    let (cx, cy) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
    let mut v = pts.to_vec();
    v.push([cx - d, cy - d]);
    v.push([cx + d, cy - d]);
    v.push([cx, cy + d]);
    let mut tris: Vec<[usize; 3]> = vec![orient(&v, [n, n + 1, n + 2])];
    for i in 0..n {
        let p = v[i];
        let mut bad = vec![];
        for (k, t) in tris.iter().enumerate() {
            if in_circle(v[t[0]], v[t[1]], v[t[2]], p) {
                bad.push(k);
            }
        }
        if bad.is_empty() {
            // On a circle or exactly on an edge: find the containing triangle instead.
            if let Some(k) =
                tris.iter().position(|t| cross(v[t[0]], v[t[1]], p) >= 0.0 && cross(v[t[1]], v[t[2]], p) >= 0.0 && cross(v[t[2]], v[t[0]], p) >= 0.0)
            {
                bad.push(k);
            } else {
                continue;
            }
        }
        // Cavity boundary: edges of bad triangles not shared by two of them.
        let mut count: HashMap<(usize, usize), u32> = HashMap::new();
        for k in &bad {
            let t = tris[*k];
            for e in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *count.entry((e.0.min(e.1), e.0.max(e.1))).or_default() += 1;
            }
        }
        let mut edges = vec![];
        for k in &bad {
            let t = tris[*k];
            for e in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                if count[&(e.0.min(e.1), e.0.max(e.1))] == 1 {
                    edges.push(e);
                }
            }
        }
        let mut sorted = bad.clone();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        for k in sorted {
            tris.swap_remove(k);
        }
        for (a, b) in edges {
            if cross(v[a], v[b], p).abs() > 1e-12 {
                tris.push(orient(&v, [a, b, i]));
            }
        }
    }
    tris.retain(|t| t.iter().all(|i| *i < n));
    tris
}

fn orient(v: &[[f64; 2]], t: [usize; 3]) -> [usize; 3] {
    if cross(v[t[0]], v[t[1]], v[t[2]]) < 0.0 { [t[0], t[2], t[1]] } else { t }
}

fn seg_cross(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let d1 = cross(a, b, c);
    let d2 = cross(a, b, d);
    let d3 = cross(c, d, a);
    let d4 = cross(c, d, b);
    ((d1 > 1e-12 && d2 < -1e-12) || (d1 < -1e-12 && d2 > 1e-12)) && ((d3 > 1e-12 && d4 < -1e-12) || (d3 < -1e-12 && d4 > 1e-12))
}

fn key(a: usize, b: usize) -> (usize, usize) {
    (a.min(b), a.max(b))
}

/// Recover constraint edges in a triangulation by flipping crossing edges.
pub fn recover_edges(v: &[[f64; 2]], tris: &mut [[usize; 3]], constraints: &[(usize, usize)]) {
    let mut edge_tris: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();
    for (k, t) in tris.iter().enumerate() {
        for e in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            edge_tris.entry(key(e.0, e.1)).or_default().push(k);
        }
    }
    let fixed: std::collections::HashSet<(usize, usize)> = constraints.iter().map(|&(a, b)| key(a, b)).collect();
    for &(a, b) in constraints {
        if edge_tris.get(&key(a, b)).is_some_and(|t| !t.is_empty()) {
            continue;
        }
        let mut queue: VecDeque<(usize, usize)> =
            edge_tris.iter().filter(|(e, t)| t.len() == 2 && seg_cross(v[a], v[b], v[e.0], v[e.1])).map(|(e, _)| *e).collect();
        let mut guard = 0;
        while let Some(e) = queue.pop_front() {
            guard += 1;
            if guard > 20_000 {
                break;
            }
            let Some(ts) = edge_tris.get(&e).cloned() else { continue };
            if ts.len() != 2 || fixed.contains(&e) {
                continue;
            }
            let (t0, t1) = (tris[ts[0]], tris[ts[1]]);
            let p = t0.iter().copied().find(|x| *x != e.0 && *x != e.1);
            let q = t1.iter().copied().find(|x| *x != e.0 && *x != e.1);
            let (Some(p), Some(q)) = (p, q) else { continue };
            // Convex quad (u, p, w, q)? Then the diagonal pq crosses uw.
            if !seg_cross(v[p], v[q], v[e.0], v[e.1]) {
                if queue.is_empty() {
                    break;
                }
                queue.push_back(e);
                continue;
            }
            // Flip.
            for (k, t) in [(ts[0], t0), (ts[1], t1)] {
                for ed in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                    if let Some(l) = edge_tris.get_mut(&key(ed.0, ed.1)) {
                        l.retain(|x| *x != k);
                    }
                }
            }
            let n0 = orient(v, [p, q, e.0]);
            let n1 = orient(v, [p, q, e.1]);
            tris[ts[0]] = n0;
            tris[ts[1]] = n1;
            for (k, t) in [(ts[0], n0), (ts[1], n1)] {
                for ed in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                    edge_tris.entry(key(ed.0, ed.1)).or_default().push(k);
                }
            }
            edge_tris.retain(|_, l| !l.is_empty());
            if seg_cross(v[a], v[b], v[p], v[q]) {
                queue.push_back(key(p, q));
            }
        }
    }
}

/// Mesh options (Puppet ▸ Mesh: Triangles, Density, Expansion).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshOpts {
    /// Target triangle count.
    pub triangles: f64,
    /// 0–100; 50 = the target count, each 25 doubles / halves it.
    pub density: f64,
    /// Outline growth in layer pixels.
    pub expansion: f64,
}

impl Default for MeshOpts {
    fn default() -> Self {
        MeshOpts { triangles: 350.0, density: 50.0, expansion: 3.0 }
    }
}

/// Deterministic jitter in [-0.5, 0.5).
fn jitter(i: u64) -> f64 {
    let mut x = i.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x2545_f491_4f6c_dd1d;
    x ^= x >> 31;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 29;
    (x >> 11) as f64 / (1u64 << 53) as f64 - 0.5
}

/// Build a mesh over a polygon outline (layer space) with the option's target size.
pub fn triangulate(outline: &[[f64; 2]], opts: &MeshOpts) -> Mesh {
    let mut poly = outline.to_vec();
    if signed_area(&poly) < 0.0 {
        poly.reverse();
    }
    let area = signed_area(&poly).abs();
    if poly.len() < 3 || area <= 0.0 {
        return Mesh::default();
    }
    let target = (opts.triangles.max(2.0) * 2f64.powf((opts.density - 50.0) / 25.0)).clamp(2.0, 20_000.0);
    let l = (4.0 * area / (3f64.sqrt() * target)).sqrt().max(0.5);
    let boundary = densify(&poly, l);
    let nb = boundary.len();
    let mut pts = boundary.clone();
    // Interior: a jittered hexagonal lattice, kept away from the outline.
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in &poly {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    let dy = l * 3f64.sqrt() * 0.5;
    let mut row = 0u64;
    let mut y = y0 + dy * 0.5;
    let min_d2 = (0.55 * l).powi(2);
    while y < y1 {
        let mut x = x0 + if row % 2 == 1 { l * 0.5 } else { 0.0 } + l * 0.25;
        let mut col = 0u64;
        while x < x1 {
            let id = row * 100_003 + col;
            let p = [x + jitter(id * 2) * l * 0.08, y + jitter(id * 2 + 1) * l * 0.08];
            if inside(&poly, p) && poly_dist2(&boundary, p) >= min_d2 {
                pts.push(p);
            }
            x += l;
            col += 1;
        }
        y += dy;
        row += 1;
    }
    let mut tris = delaunay(&pts);
    let constraints: Vec<(usize, usize)> = (0..nb).map(|i| (i, (i + 1) % nb)).collect();
    recover_edges(&pts, &mut tris, &constraints);
    tris.retain(|t| {
        let c = [(pts[t[0]][0] + pts[t[1]][0] + pts[t[2]][0]) / 3.0, (pts[t[0]][1] + pts[t[1]][1] + pts[t[2]][1]) / 3.0];
        cross(pts[t[0]], pts[t[1]], pts[t[2]]) > 1e-9 && inside(&poly, c)
    });
    // Compact the vertex list.
    let mut map = vec![usize::MAX; pts.len()];
    let mut verts = vec![];
    for t in tris.iter_mut() {
        for i in t.iter_mut() {
            if map[*i] == usize::MAX {
                map[*i] = verts.len();
                verts.push(pts[*i]);
            }
            *i = map[*i];
        }
    }
    Mesh { verts, tris, outline: poly }
}

/// The outline of the alpha region under `seed` (buffer pixels → layer space via `scale` and
/// `offset`), expanded by `expansion` layer pixels.
pub fn outline(alpha: &[f32], w: usize, h: usize, scale: f64, offset: [f64; 2], seed: [f64; 2], expansion: f64) -> Vec<[f64; 2]> {
    // Pad so expansion can grow past the buffer edge.
    let pad = (expansion.max(0.0) * scale).ceil() as usize + 2;
    let (pw, ph) = (w + 2 * pad, h + 2 * pad);
    let mut on = vec![false; pw * ph];
    for y in 0..h {
        for x in 0..w {
            on[(y + pad) * pw + x + pad] = alpha[y * w + x] >= 0.5;
        }
    }
    let g = expand(&Grid { w: pw, h: ph, on }, expansion * scale);
    let sp = [seed[0] * scale + offset[0] + pad as f64, seed[1] * scale + offset[1] + pad as f64];
    let Some(r) = region(&g, sp) else { return vec![] };
    let c = trace(&r);
    let c = simplify(&c, 0.75);
    c.iter().map(|p| [(p[0] - pad as f64 - offset[0]) / scale, (p[1] - pad as f64 - offset[1]) / scale]).collect()
}
