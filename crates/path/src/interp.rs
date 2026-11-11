//! Smart mask interpolation (the Mask Interpolation panel): give two mask shapes the same vertex
//! count with a sensible vertex correspondence, and compute in-between shapes.
//!
//! - **Vertex insertion** splits Bezier segments with de Casteljau subdivision, so both shapes
//!   keep their exact outline while gaining vertices; vertices are distributed by **arc length**
//!   so the i-th vertex of each shape sits at a comparable fraction of the outline.
//! - **Correspondence** picks the cyclic offset (and, for closed shapes, the winding) of the
//!   second shape that minimises a matching cost: normalised position differences plus, when
//!   Quality is above 0, the χ² distance of log-polar **shape context** histograms (Belongie,
//!   Malik & Puzicha, "Shape Matching and Object Recognition Using Shape Contexts", PAMI 2002).
//!   *First Vertices Match* pins the offset to 0; *Use 1:1 Vertex Matches* keeps the original
//!   vertices when the counts agree.
//! - **In-betweens** move vertices along straight lines (*Use Linear Vertex Paths*) or, by
//!   default, with the shapes' best-fit rotation and scale (Procrustes) interpolated so turning
//!   shapes turn instead of collapsing; *Bending Resistance* weights that rigid component
//!   against the straight-line one.

use effectcraft_keyframe::ShapePath;

/// Matching Method.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Matching {
    #[default]
    Auto,
    Curve,
    Polyline,
}

/// Add Mask Shape Vertices.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AddVertices {
    /// Pixels between vertices.
    Pixels(f64),
    /// Total vertices.
    Total(usize),
    /// One vertex every this percentage of the outline.
    Percent(f64),
}

/// Mask Interpolation options.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InterpOpts {
    pub linear: bool,
    /// Bending Resistance, 0–100.
    pub bending_resistance: f64,
    /// Quality, 0–100.
    pub quality: f64,
    pub add_vertices: Option<AddVertices>,
    pub matching: Matching,
    pub one_to_one: bool,
    pub first_vertices_match: bool,
}

impl Default for InterpOpts {
    fn default() -> Self {
        InterpOpts {
            linear: false,
            bending_resistance: 50.0,
            quality: 50.0,
            add_vertices: Some(AddVertices::Pixels(15.0)),
            matching: Matching::Auto,
            one_to_one: false,
            first_vertices_match: true,
        }
    }
}

type P = [f64; 2];

fn add(a: P, b: P) -> P {
    [a[0] + b[0], a[1] + b[1]]
}
fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1]]
}
fn mul(a: P, k: f64) -> P {
    [a[0] * k, a[1] * k]
}
fn lerp(a: P, b: P, t: f64) -> P {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

/// Segment `i` (vertex i → i+1, wrapping for closed paths) as cubic control points.
fn segment(p: &ShapePath, i: usize) -> [P; 4] {
    let n = p.len();
    let j = (i + 1) % n;
    let (a, b) = (p.vertices[i], p.vertices[j]);
    let o = p.out_tangents.get(i).copied().unwrap_or([0.0; 2]);
    let it = p.in_tangents.get(j).copied().unwrap_or([0.0; 2]);
    [a, add(a, o), add(b, it), b]
}

fn seg_count(p: &ShapePath) -> usize {
    let n = p.len();
    if n < 2 {
        0
    } else if p.closed {
        n
    } else {
        n - 1
    }
}

fn cubic(c: &[P; 4], t: f64) -> P {
    let u = 1.0 - t;
    let (a, b, cc, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    [a * c[0][0] + b * c[1][0] + cc * c[2][0] + d * c[3][0], a * c[0][1] + b * c[1][1] + cc * c[2][1] + d * c[3][1]]
}

fn seg_len(c: &[P; 4]) -> f64 {
    let mut l = 0.0;
    let mut prev = c[0];
    for k in 1..=24 {
        let q = cubic(c, k as f64 / 24.0);
        l += (q[0] - prev[0]).hypot(q[1] - prev[1]);
        prev = q;
    }
    l
}

/// Outline length.
pub fn length(p: &ShapePath) -> f64 {
    (0..seg_count(p)).map(|i| seg_len(&segment(p, i))).sum()
}

/// Split cubic `c` at parameter values `ts` (increasing, in (0, 1)) into `ts.len() + 1` cubics.
fn split_many(c: &[P; 4], ts: &[f64]) -> Vec<[P; 4]> {
    let mut out = vec![];
    let mut rest = *c;
    let mut t0 = 0.0;
    for &t in ts {
        let u = ((t - t0) / (1.0 - t0)).clamp(0.0, 1.0);
        let (l, r) = split(&rest, u);
        out.push(l);
        rest = r;
        t0 = t;
    }
    out.push(rest);
    out
}

fn split(c: &[P; 4], t: f64) -> ([P; 4], [P; 4]) {
    let p01 = lerp(c[0], c[1], t);
    let p12 = lerp(c[1], c[2], t);
    let p23 = lerp(c[2], c[3], t);
    let a = lerp(p01, p12, t);
    let b = lerp(p12, p23, t);
    let m = lerp(a, b, t);
    ([c[0], p01, a, m], [m, b, p23, c[3]])
}

/// Parameter values splitting `c` into `k` pieces of (approximately) equal arc length.
fn arc_params(c: &[P; 4], k: usize) -> Vec<f64> {
    if k <= 1 {
        return vec![];
    }
    const S: usize = 64;
    let mut cum = vec![0.0; S + 1];
    let mut prev = c[0];
    for i in 1..=S {
        let q = cubic(c, i as f64 / S as f64);
        cum[i] = cum[i - 1] + (q[0] - prev[0]).hypot(q[1] - prev[1]);
        prev = q;
    }
    let total = cum[S];
    if total < 1e-12 {
        return (1..k).map(|j| j as f64 / k as f64).collect();
    }
    (1..k)
        .map(|j| {
            let target = total * j as f64 / k as f64;
            let i = cum.partition_point(|v| *v < target).clamp(1, S);
            let f = (target - cum[i - 1]) / (cum[i] - cum[i - 1]).max(1e-12);
            ((i - 1) as f64 + f) / S as f64
        })
        .collect()
}

/// Rebuild a ShapePath from consecutive cubics (closed: the last ends at the first's start).
fn from_cubics(cs: &[[P; 4]], closed: bool) -> ShapePath {
    let n = if closed { cs.len() } else { cs.len() + 1 };
    let mut v = vec![[0.0; 2]; n];
    let mut ins = vec![[0.0; 2]; n];
    let mut outs = vec![[0.0; 2]; n];
    for (i, c) in cs.iter().enumerate() {
        let j = if closed { (i + 1) % n } else { i + 1 };
        v[i] = c[0];
        outs[i] = sub(c[1], c[0]);
        ins[j] = sub(c[2], c[3]);
        if !closed && j == n - 1 {
            v[j] = c[3];
        }
    }
    ShapePath { vertices: v, in_tangents: ins, out_tangents: outs, closed, feather: Vec::new() }
}

/// Exactly the same outline with `total` vertices (≥ the current count), the extra vertices
/// distributed over segments in proportion to their length.
pub fn with_vertex_count(p: &ShapePath, total: usize) -> ShapePath {
    let m = seg_count(p);
    let base = if p.closed { m } else { m + 1 };
    if m == 0 || total <= base {
        return p.clone();
    }
    let segs: Vec<[P; 4]> = (0..m).map(|i| segment(p, i)).collect();
    let lens: Vec<f64> = segs.iter().map(seg_len).collect();
    let sum: f64 = lens.iter().sum::<f64>().max(1e-12);
    let pieces = total - base + m; // total segments wanted
    // Largest remainder allocation, at least one piece per segment.
    let extra = pieces - m;
    let want: Vec<f64> = lens.iter().map(|l| l / sum * extra as f64).collect();
    let mut k: Vec<usize> = want.iter().map(|w| 1 + w.floor() as usize).collect();
    let mut left = pieces - k.iter().sum::<usize>();
    let mut order: Vec<usize> = (0..m).collect();
    order.sort_by(|a, b| (want[*b] - want[*b].floor()).total_cmp(&(want[*a] - want[*a].floor())).then(a.cmp(b)));
    for i in order.iter().cycle() {
        if left == 0 {
            break;
        }
        k[*i] += 1;
        left -= 1;
    }
    let mut cubics = vec![];
    for (c, ki) in segs.iter().zip(&k) {
        cubics.extend(split_many(c, &arc_params(c, *ki)));
    }
    from_cubics(&cubics, p.closed)
}

/// The same outline as a polyline with `total` vertices at equal arc-length spacing, starting at
/// vertex 0.
pub fn polyline(p: &ShapePath, total: usize) -> ShapePath {
    let m = seg_count(p);
    if m == 0 || total < 2 {
        return ShapePath::polygon(&p.vertices, p.closed);
    }
    // Dense samples with cumulative length.
    let mut pts = vec![p.vertices[0]];
    for i in 0..m {
        let c = segment(p, i);
        for k in 1..=32 {
            pts.push(cubic(&c, k as f64 / 32.0));
        }
    }
    let mut cum = vec![0.0];
    for w in pts.windows(2) {
        let l = cum.last().copied().unwrap_or(0.0) + (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]);
        cum.push(l);
    }
    let total_len = *cum.last().unwrap_or(&0.0);
    let n = total;
    let div = if p.closed { n as f64 } else { (n - 1) as f64 };
    let out: Vec<P> = (0..n)
        .map(|j| {
            let target = total_len * j as f64 / div;
            let i = cum.partition_point(|v| *v < target).clamp(1, cum.len() - 1);
            let f = (target - cum[i - 1]) / (cum[i] - cum[i - 1]).max(1e-12);
            lerp(pts[i - 1], pts[i], f)
        })
        .collect();
    ShapePath::polygon(&out, p.closed)
}

fn has_curves(p: &ShapePath) -> bool {
    p.in_tangents.iter().chain(&p.out_tangents).any(|t| t[0].abs() > 1e-9 || t[1].abs() > 1e-9)
}

/// Rotate a closed path's vertex order to start at `k`.
fn rotate(p: &ShapePath, k: usize) -> ShapePath {
    let n = p.len();
    if n == 0 || k.is_multiple_of(n) {
        return p.clone();
    }
    let r = |v: &Vec<P>| (0..n).map(|i| v[(i + k) % n]).collect();
    ShapePath {
        vertices: r(&p.vertices),
        in_tangents: r(&p.in_tangents),
        out_tangents: r(&p.out_tangents),
        closed: p.closed,
        feather: p.feather.iter().map(|f| effectcraft_keyframe::FeatherPoint { segment: (f.segment + n - k % n) % n, ..*f }).collect(),
    }
}

/// The same path traversed the other way (closed paths keep vertex 0 first).
pub fn reversed(p: &ShapePath) -> ShapePath {
    let n = p.len();
    if n == 0 {
        return p.clone();
    }
    let idx: Vec<usize> = if p.closed { (0..n).map(|i| (n - i) % n).collect() } else { (0..n).rev().collect() };
    ShapePath {
        vertices: idx.iter().map(|i| p.vertices[*i]).collect(),
        in_tangents: idx.iter().map(|i| p.out_tangents[*i]).collect(),
        out_tangents: idx.iter().map(|i| p.in_tangents[*i]).collect(),
        closed: p.closed,
        // Feather points sit on segments that reversal renumbers: they are not carried.
        feather: Vec::new(),
    }
}

fn signed_area(v: &[P]) -> f64 {
    let n = v.len();
    (0..n).map(|i| v[i][0] * v[(i + 1) % n][1] - v[(i + 1) % n][0] * v[i][1]).sum::<f64>() * 0.5
}

/// Centroid and RMS radius of points.
fn frame(v: &[P]) -> (P, f64) {
    let n = v.len().max(1) as f64;
    let c = [v.iter().map(|p| p[0]).sum::<f64>() / n, v.iter().map(|p| p[1]).sum::<f64>() / n];
    let r = (v.iter().map(|p| (p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)).sum::<f64>() / n).sqrt().max(1e-9);
    (c, r)
}

/// Vertex counts above this skip the shape contexts (quadratic to build) and search the starting
/// vertex coarsely; no mask comes near it (the Add Vertices option itself stops at 2000).
const MAX_CONTEXT_VERTICES: usize = 4096;
const RBINS: usize = 5;
const ABINS: usize = 12;

/// Log-polar shape context histograms of every point (normalised by the RMS radius).
fn shape_contexts(v: &[P]) -> Vec<[f64; RBINS * ABINS]> {
    let (_, r) = frame(v);
    let n = v.len();
    (0..n)
        .map(|i| {
            let mut h = [0.0; RBINS * ABINS];
            for j in 0..n {
                if i == j {
                    continue;
                }
                let d = sub(v[j], v[i]);
                let dist = d[0].hypot(d[1]) / r;
                // log radius in [1/8, 2] → 5 bins.
                let lr = (dist.max(1e-6).log2() + 3.0) / 4.0 * RBINS as f64;
                if !(0.0..RBINS as f64).contains(&lr) {
                    continue;
                }
                let a = (d[1].atan2(d[0]) + std::f64::consts::PI) / std::f64::consts::TAU * ABINS as f64;
                let ai = (a as usize).min(ABINS - 1);
                h[lr as usize * ABINS + ai] += 1.0;
            }
            let s: f64 = h.iter().sum::<f64>().max(1.0);
            h.iter_mut().for_each(|x| *x /= s);
            h
        })
        .collect()
}

fn chi2(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| if x + y > 1e-12 { (x - y) * (x - y) / (x + y) } else { 0.0 }).sum::<f64>() * 0.5
}

/// Matching cost of `a[i] ↔ b[(i + k) % n]`.
fn cost(a: &[P], b: &[P], sa: Option<&[[f64; RBINS * ABINS]]>, sb: Option<&[[f64; RBINS * ABINS]]>, k: usize) -> f64 {
    let n = a.len();
    let (ca, ra) = frame(a);
    let (cb, rb) = frame(b);
    let mut c = 0.0;
    for i in 0..n {
        let j = (i + k) % n;
        let pa = mul(sub(a[i], ca), 1.0 / ra);
        let pb = mul(sub(b[j], cb), 1.0 / rb);
        c += (pa[0] - pb[0]).powi(2) + (pa[1] - pb[1]).powi(2);
        if let (Some(sa), Some(sb)) = (sa, sb) {
            c += chi2(&sa[i], &sb[j]);
        }
    }
    c / n as f64
}

/// Give `a` and `b` the same vertex count with matching vertices (the shapes that the first
/// and last keys get, and the input of [`interpolate`]).
pub fn correspond(a: &ShapePath, b: &ShapePath, o: &InterpOpts) -> (ShapePath, ShapePath) {
    if a.is_empty() || b.is_empty() {
        return (a.clone(), b.clone());
    }
    let closed = a.closed && b.closed;
    let curves = match o.matching {
        Matching::Curve => true,
        Matching::Polyline => false,
        Matching::Auto => has_curves(a) || has_curves(b),
    };
    let (mut ra, mut rb);
    if o.one_to_one && a.len() == b.len() {
        (ra, rb) = (a.clone(), b.clone());
    } else {
        let (la, lb) = (length(a), length(b));
        let mut n = a.len().max(b.len());
        if let Some(av) = o.add_vertices {
            let want = match av {
                AddVertices::Pixels(px) => (la.max(lb) / px.max(1.0)).ceil() as usize,
                AddVertices::Total(t) => t,
                AddVertices::Percent(pc) => (100.0 / pc.clamp(0.1, 100.0)).ceil() as usize,
            };
            n = n.max(want.min(2000));
        }
        if curves {
            ra = with_vertex_count(a, n);
            rb = with_vertex_count(b, n);
        } else {
            ra = polyline(a, n);
            rb = polyline(b, n);
        }
        // Proportional splitting can leave counts unequal when one shape already had more
        // vertices than its share: fall back to arc-length polylines then.
        if ra.len() != rb.len() {
            ra = polyline(a, n);
            rb = polyline(b, n);
        }
    }
    if closed && ra.len() == rb.len() && ra.len() >= 3 {
        // Same winding.
        if signed_area(&ra.vertices).signum() != signed_area(&rb.vertices).signum() {
            rb = reversed(&rb);
        }
        if !o.first_vertices_match {
            let n = ra.len();
            let large = n > MAX_CONTEXT_VERTICES;
            let use_sc = o.quality > 0.0 && !large;
            let (sa, sb) = if use_sc { (Some(shape_contexts(&ra.vertices)), Some(shape_contexts(&rb.vertices))) } else { (None, None) };
            // Quality limits the offsets examined exhaustively (coarse step), then refines.
            // (A very large shape tries about 3 √n offsets, not n, each costing a pass over it.)
            let step = if large {
                (n as f64).sqrt().ceil() as usize
            } else if o.quality >= 50.0 {
                1
            } else {
                ((n as f64 / 24.0).ceil() as usize).max(1)
            };
            let mut best = (f64::MAX, 0usize);
            let mut k = 0;
            while k < n {
                let c = cost(&ra.vertices, &rb.vertices, sa.as_deref(), sb.as_deref(), k);
                if c < best.0 {
                    best = (c, k);
                }
                k += step;
            }
            if step > 1 {
                let k0 = best.1;
                for d in 1..step {
                    for k in [(k0 + d) % n, (k0 + n - d) % n] {
                        let c = cost(&ra.vertices, &rb.vertices, sa.as_deref(), sb.as_deref(), k);
                        if c < best.0 {
                            best = (c, k);
                        }
                    }
                }
            }
            rb = rotate(&rb, best.1);
        }
    }
    (ra, rb)
}

/// The in-between shape at fraction `u` of two corresponded shapes (equal vertex counts).
pub fn interpolate(a: &ShapePath, b: &ShapePath, u: f64, o: &InterpOpts) -> ShapePath {
    if a.len() != b.len() || a.is_empty() {
        return if u < 1.0 { a.clone() } else { b.clone() };
    }
    let n = a.len();
    let lin = |pa: &[P], pb: &[P]| -> Vec<P> { pa.iter().zip(pb).map(|(x, y)| lerp(*x, *y, u)).collect() };
    if o.linear {
        return ShapePath {
            vertices: lin(&a.vertices, &b.vertices),
            in_tangents: lin(&a.in_tangents, &b.in_tangents),
            out_tangents: lin(&a.out_tangents, &b.out_tangents),
            closed: a.closed,
            feather: a.feather.clone(),
        };
    }
    // Procrustes: b ≈ s R (a − ca) + cb.
    let (ca, _) = frame(&a.vertices);
    let (cb, _) = frame(&b.vertices);
    let (mut ss, mut dot, mut cross) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (p, q) = (sub(a.vertices[i], ca), sub(b.vertices[i], cb));
        ss += p[0] * p[0] + p[1] * p[1];
        dot += p[0] * q[0] + p[1] * q[1];
        cross += p[0] * q[1] - p[1] * q[0];
    }
    let theta = cross.atan2(dot);
    let s = if ss > 1e-12 { dot.hypot(cross) / ss } else { 1.0 };
    let rot = |v: P, ang: f64, k: f64| {
        let (sn, cs) = ang.sin_cos();
        [k * (cs * v[0] - sn * v[1]), k * (sn * v[0] + cs * v[1])]
    };
    let (th_u, s_u) = (theta * u, s.max(1e-9).powf(u));
    let c_u = lerp(ca, cb, u);
    let w = (o.bending_resistance / 100.0).clamp(0.0, 1.0);
    let mut out = ShapePath { vertices: vec![], in_tangents: vec![], out_tangents: vec![], closed: a.closed, feather: a.feather.clone() };
    for i in 0..n {
        // Rigid path: a's vertex carried by the interpolated similarity, plus the residual
        // (b − fully transformed a) blended in linearly.
        let pa = sub(a.vertices[i], ca);
        let full = add(rot(pa, theta, s), cb);
        let resid = sub(b.vertices[i], full);
        let rigid = add(add(rot(pa, th_u, s_u), c_u), mul(resid, u));
        let linear = lerp(a.vertices[i], b.vertices[i], u);
        out.vertices.push(lerp(linear, rigid, w));
        for (src_a, src_b, dst) in [(&a.in_tangents, &b.in_tangents, &mut out.in_tangents), (&a.out_tangents, &b.out_tangents, &mut out.out_tangents)] {
            let ta = src_a[i];
            let tr = sub(src_b[i], rot(ta, theta, s));
            let rigid_t = add(rot(ta, th_u, s_u), mul(tr, u));
            dst.push(lerp(lerp(ta, src_b[i], u), rigid_t, w));
        }
    }
    out
}

/// Do any two vertex paths (straight segments from `a[i]` to `b[i]`) cross? (A sanity measure of
/// the correspondence: good matches don't make vertices pass through each other.)
pub fn paths_cross(a: &ShapePath, b: &ShapePath) -> usize {
    let n = a.len().min(b.len());
    let mut c = 0;
    let inter = |p1: P, p2: P, p3: P, p4: P| {
        let d = |a: P, b: P, c: P| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        let (d1, d2, d3, d4) = (d(p3, p4, p1), d(p3, p4, p2), d(p1, p2, p3), d(p1, p2, p4));
        d1 * d2 < 0.0 && d3 * d4 < 0.0
    };
    for i in 0..n {
        for j in i + 1..n {
            if inter(a.vertices[i], b.vertices[i], a.vertices[j], b.vertices[j]) {
                c += 1;
            }
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn star(c: P, r: f64, k: usize, rot: f64) -> ShapePath {
        let pts: Vec<P> = (0..2 * k)
            .map(|i| {
                let a = rot + i as f64 * std::f64::consts::PI / k as f64;
                let rr = if i % 2 == 0 { r } else { r * 0.5 };
                [c[0] + rr * a.cos(), c[1] + rr * a.sin()]
            })
            .collect();
        ShapePath::polygon(&pts, true)
    }

    #[test]
    fn splitting_keeps_the_outline() {
        let e = ShapePath::ellipse([50.0, 50.0], 80.0, 40.0);
        let m = with_vertex_count(&e, 23);
        assert_eq!(m.len(), 23);
        assert!((length(&m) - length(&e)).abs() < 0.05, "{} vs {}", length(&m), length(&e));
        // Points on the split path lie on the ellipse.
        for i in 0..m.len() {
            let v = m.vertices[i];
            let q = ((v[0] - 50.0) / 40.0).powi(2) + ((v[1] - 50.0) / 20.0).powi(2);
            assert!((q - 1.0).abs() < 0.01, "{v:?}");
        }
        let r = reversed(&ShapePath::rect([0.0, 0.0], 10.0, 10.0));
        assert_eq!(r.vertices[0], [-5.0, -5.0]);
        assert_eq!(r.vertices[1], [-5.0, 5.0]);
    }

    #[test]
    fn correspondence_finds_the_rotation_and_paths_dont_cross() {
        // A star rotated by 2 points' worth and with its first vertex elsewhere.
        let a = star([100.0, 100.0], 60.0, 5, 0.0);
        let b0 = star([140.0, 110.0], 50.0, 5, 0.3);
        let b = rotate(&b0, 3);
        let o = InterpOpts { first_vertices_match: false, add_vertices: None, ..Default::default() };
        let (ra, rb) = correspond(&a, &b, &o);
        assert_eq!(ra.len(), rb.len());
        // The true correspondence is recovered (b0's vertex 0 matches a's vertex 0).
        assert!((rb.vertices[0][0] - b0.vertices[0][0]).abs() < 1e-9 && (rb.vertices[0][1] - b0.vertices[0][1]).abs() < 1e-9);
        assert!(paths_cross(&ra, &rb) <= 1);
        // With First Vertices Match the given (bad) start is kept: paths cross.
        let o2 = InterpOpts { first_vertices_match: true, add_vertices: None, ..Default::default() };
        let (xa, xb) = correspond(&a, &b, &o2);
        assert!(paths_cross(&xa, &xb) > 5);
        // In-betweens keep the vertex count and stay star-like (area between the ends).
        let mid = interpolate(&ra, &rb, 0.5, &o);
        assert_eq!(mid.len(), ra.len());
        let (aa, ab, am) = (signed_area(&ra.vertices).abs(), signed_area(&rb.vertices).abs(), signed_area(&mid.vertices).abs());
        assert!(am > ab.min(aa) * 0.8 && am < aa.max(ab) * 1.2, "{aa} {ab} {am}");
        // Different vertex counts with added vertices.
        let c = ShapePath::ellipse([100.0, 100.0], 120.0, 120.0);
        let o3 = InterpOpts { first_vertices_match: false, ..Default::default() };
        let (pa, pb) = correspond(&a, &c, &o3);
        assert_eq!(pa.len(), pb.len());
        assert!(pa.len() >= 25);
        assert!(paths_cross(&pa, &pb) <= pa.len() / 10, "{}", paths_cross(&pa, &pb));
    }

    #[test]
    fn rigid_interpolation_turns_instead_of_collapsing() {
        let a = ShapePath::polygon(&[[-50.0, -10.0], [50.0, -10.0], [50.0, 10.0], [-50.0, 10.0]], true);
        // The same bar turned 90°.
        let b = ShapePath::polygon(&[[10.0, -50.0], [10.0, 50.0], [-10.0, 50.0], [-10.0, -50.0]], true);
        let o = InterpOpts { bending_resistance: 100.0, ..Default::default() };
        let m = interpolate(&a, &b, 0.5, &o);
        let len = (m.vertices[1][0] - m.vertices[0][0]).hypot(m.vertices[1][1] - m.vertices[0][1]);
        assert!((len - 100.0).abs() < 1e-6, "{len}");
        let lin = interpolate(&a, &b, 0.5, &InterpOpts { linear: true, ..Default::default() });
        let l2 = (lin.vertices[1][0] - lin.vertices[0][0]).hypot(lin.vertices[1][1] - lin.vertices[0][1]);
        assert!(l2 < 80.0);
    }

    /// A shape with tens of thousands of vertices is matched without the quadratic shape contexts
    /// and exhaustive offset search.
    #[test]
    fn very_large_shapes_are_matched_in_bounded_time() {
        let n = 30_000;
        let circle = |rot: f64| -> ShapePath {
            let pts: Vec<P> = (0..n)
                .map(|i| [100.0 * (rot + i as f64 * std::f64::consts::TAU / n as f64).cos(), 100.0 * (rot + i as f64 * std::f64::consts::TAU / n as f64).sin()])
                .collect();
            ShapePath::polygon(&pts, true)
        };
        let o = InterpOpts { one_to_one: true, quality: 100.0, ..Default::default() };
        let t = std::time::Instant::now();
        let (a, b) = correspond(&circle(0.0), &circle(1.0), &o);
        assert_eq!((a.len(), b.len()), (n, n));
        assert!(t.elapsed().as_secs() < 30, "{:?}", t.elapsed());
    }
}
