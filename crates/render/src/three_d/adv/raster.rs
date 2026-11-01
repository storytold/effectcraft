//! The software rasteriser of the Advanced 3D renderer (the CPU reference and the fallback
//! when no GPU is available) and the shadow-map rasteriser shared by both paths.
//!
//! Triangles are clipped against the depth range (`0 ≤ z ≤ w`, as GPUs do), set up in screen
//! space and scanned per band of rows in parallel with edge functions (pixel centres, top-left
//! fill rule) and perspective-correct barycentrics. Opaque triangles resolve visibility first
//! (reversed depth: larger ndc z is nearer) and each pixel is shaded once; transparent triangles
//! (already sorted back to front) are then shaded per fragment and blended over, depth-tested
//! against the opaque surfaces.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use rayon::prelude::*;

use super::scene::{EnvInfo, Scene, ShadowMap, Vertex};
use super::shade::{Frag, sample, shade};

/// A rendered target at raster size.
pub struct Target {
    pub width: u32,
    pub height: u32,
    /// Premultiplied linear RGBA.
    pub color: Vec<[f32; 4]>,
    /// Camera-space depth of the nearest opaque surface (∞ where empty).
    pub depth: Vec<f32>,
}

/// A clipped triangle in screen space; `w` holds, per corner, its weights over the original
/// triangle's three vertices.
#[derive(Clone, Copy)]
struct STri {
    p: [[f64; 2]; 3],
    z: [f32; 3],
    inv_w: [f32; 3],
    w: [[f32; 3]; 3],
    tri: u32,
    front: bool,
    bbox: [i32; 4],
}

#[inline]
fn mat_mul(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 4] {
    let mut o = [0.0; 4];
    for r in 0..4 {
        o[r] = m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3];
    }
    o
}

/// Clip a polygon (clip coords + original weights) against `0 ≤ z ≤ w`.
fn clip_poly(poly: Vec<([f32; 4], [f32; 3])>) -> Vec<([f32; 4], [f32; 3])> {
    let mut p = poly;
    for plane in 0..2 {
        if p.is_empty() {
            break;
        }
        let d = |c: &[f32; 4]| if plane == 0 { c[2] } else { c[3] - c[2] };
        let mut out = Vec::with_capacity(p.len() + 2);
        for i in 0..p.len() {
            let (a, b) = (p[i], p[(i + 1) % p.len()]);
            let (da, db) = (d(&a.0), d(&b.0));
            if da >= 0.0 {
                out.push(a);
            }
            if (da >= 0.0) != (db >= 0.0) {
                let t = da / (da - db);
                let mut c = [0.0; 4];
                let mut w = [0.0; 3];
                for k in 0..4 {
                    c[k] = a.0[k] + (b.0[k] - a.0[k]) * t;
                }
                for k in 0..3 {
                    w[k] = a.1[k] + (b.1[k] - a.1[k]) * t;
                }
                out.push((c, w));
            }
        }
        p = out;
    }
    p
}

/// Set up the screen triangles of index range `range`.
fn setup(s: &Scene, clip: &[[f32; 4]], range: std::ops::Range<usize>) -> Vec<STri> {
    let (w, h) = (s.width as f64, s.height as f64);
    let mut out = vec![];
    for t in range.step_by(3) {
        let ids = [s.indices[t] as usize, s.indices[t + 1] as usize, s.indices[t + 2] as usize];
        let mat = s.vertices[ids[0]].material as usize;
        if !s.materials.get(mat).is_some_and(|m| m.visible) {
            continue;
        }
        let poly = vec![(clip[ids[0]], [1.0, 0.0, 0.0]), (clip[ids[1]], [0.0, 1.0, 0.0]), (clip[ids[2]], [0.0, 0.0, 1.0])];
        let poly = clip_poly(poly);
        if poly.len() < 3 {
            continue;
        }
        let scr: Vec<([f64; 2], f32, f32, [f32; 3])> = poly
            .iter()
            .map(|(c, wt)| {
                let iw = 1.0 / c[3];
                let (nx, ny) = (c[0] * iw, c[1] * iw);
                ([(nx as f64 + 1.0) * 0.5 * w, (1.0 - ny as f64) * 0.5 * h], c[2] * iw, iw, *wt)
            })
            .collect();
        // Facing from the polygon's screen-space winding (y down: negative = front).
        let mut area = 0.0;
        for i in 0..scr.len() {
            let (a, b) = (scr[i].0, scr[(i + 1) % scr.len()].0);
            area += a[0] * b[1] - b[0] * a[1];
        }
        if area == 0.0 || !area.is_finite() {
            continue;
        }
        let front = area < 0.0;
        for k in 1..scr.len() - 1 {
            let c = [scr[0], scr[k], scr[k + 1]];
            let mut x0 = f64::INFINITY;
            let mut y0 = f64::INFINITY;
            let mut x1 = f64::NEG_INFINITY;
            let mut y1 = f64::NEG_INFINITY;
            for v in &c {
                x0 = x0.min(v.0[0]);
                y0 = y0.min(v.0[1]);
                x1 = x1.max(v.0[0]);
                y1 = y1.max(v.0[1]);
            }
            let bbox = [
                (x0.floor() as i64).max(0) as i32,
                (y0.floor() as i64).max(0) as i32,
                (x1.ceil() as i64).min(s.width as i64) as i32,
                (y1.ceil() as i64).min(s.height as i64) as i32,
            ];
            if bbox[0] >= bbox[2] || bbox[1] >= bbox[3] {
                continue;
            }
            out.push(STri {
                p: [c[0].0, c[1].0, c[2].0],
                z: [c[0].1, c[1].1, c[2].1],
                inv_w: [c[0].2, c[1].2, c[2].2],
                w: [c[0].3, c[1].3, c[2].3],
                tri: (t / 3) as u32,
                front,
                bbox,
            });
        }
    }
    out
}

/// Coverage of a pixel centre: (ndc z, perspective-correct weights over the screen corners).
#[inline]
fn cover(t: &STri, px: f64, py: f64) -> Option<(f32, [f32; 3])> {
    let [a, b, c] = t.p;
    let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    // Orient so the area is positive.
    let (b, c, swap) = if area < 0.0 { (c, b, true) } else { (b, c, false) };
    let area = area.abs();
    let edge = |p: [f64; 2], q: [f64; 2]| -> f64 { (q[0] - p[0]) * (py - p[1]) - (q[1] - p[1]) * (px - p[0]) };
    let top_left = |p: [f64; 2], q: [f64; 2]| -> bool {
        let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
        dy < 0.0 || (dy == 0.0 && dx > 0.0)
    };
    let e0 = edge(b, c); // opposite a
    let e1 = edge(c, a); // opposite b
    let e2 = edge(a, b); // opposite c
    let inside = |e: f64, p: [f64; 2], q: [f64; 2]| e > 0.0 || (e == 0.0 && top_left(p, q));
    if !(inside(e0, b, c) && inside(e1, c, a) && inside(e2, a, b)) {
        return None;
    }
    let (l0, l1, l2) = ((e0 / area) as f32, (e1 / area) as f32, (e2 / area) as f32);
    // Back to the stored corner order.
    let l = if swap { [l0, l2, l1] } else { [l0, l1, l2] };
    let z = l[0] * t.z[0] + l[1] * t.z[1] + l[2] * t.z[2];
    let q = [l[0] * t.inv_w[0], l[1] * t.inv_w[1], l[2] * t.inv_w[2]];
    let sum = q[0] + q[1] + q[2];
    if sum <= 0.0 {
        return None;
    }
    Some((z, [q[0] / sum, q[1] / sum, q[2] / sum]))
}

/// Interpolate the original triangle's vertex attributes at screen-corner weights `l`.
fn frag(s: &Scene, t: &STri, l: [f32; 3]) -> Frag {
    let base = t.tri as usize * 3;
    let v: [&Vertex; 3] = [0, 1, 2].map(|k| &s.vertices[s.indices[base + k] as usize]);
    // Weights over the original vertices.
    let mut wv = [0.0f32; 3];
    for c in 0..3 {
        for k in 0..3 {
            wv[k] += l[c] * t.w[c][k];
        }
    }
    let mut f = Frag { pos: [0.0; 3], normal: [0.0; 3], uv: [0.0; 2], tangent: [0.0; 4], material: v[0].material, front: t.front };
    for k in 0..3 {
        let x = wv[k];
        for i in 0..3 {
            f.pos[i] += v[k].pos[i] * x;
            f.normal[i] += v[k].normal[i] * x;
        }
        for i in 0..2 {
            f.uv[i] += v[k].uv[i] * x;
        }
        for i in 0..4 {
            f.tangent[i] += v[k].tangent[i] * x;
        }
    }
    f.tangent[3] = v[0].tangent[3];
    f
}

const BAND: i32 = 16;

/// Rasterise and shade the scene.
pub fn render(s: &Scene) -> Target {
    let (w, h) = (s.width as usize, s.height as usize);
    let clip: Vec<[f32; 4]> = s.vertices.iter().map(|v| mat_mul(&s.clip, v.pos)).collect();
    let opaque = setup(s, &clip, 0..s.opaque_count as usize);
    let trans = setup(s, &clip, s.opaque_count as usize..s.indices.len());
    let bands = (h as i32 + BAND - 1) / BAND;
    let bin = |ts: &[STri]| -> Vec<Vec<u32>> {
        let mut b = vec![vec![]; bands.max(0) as usize];
        for (i, t) in ts.iter().enumerate() {
            for band in (t.bbox[1] / BAND)..((t.bbox[3] + BAND - 1) / BAND).min(bands) {
                b[band as usize].push(i as u32);
            }
        }
        b
    };
    let ob = bin(&opaque);
    let tb = bin(&trans);
    let view = s.view;
    let cam_z = |p: [f32; 3]| view[2][0] * p[0] + view[2][1] * p[1] + view[2][2] * p[2] + view[2][3];
    let mut color = vec![[0.0f32; 4]; w * h];
    let mut depth = vec![f32::INFINITY; w * h];
    color.par_chunks_mut(w * BAND as usize).zip(depth.par_chunks_mut(w * BAND as usize)).enumerate().for_each(|(band, (crow, drow))| {
        let y0 = band as i32 * BAND;
        let rows = crow.len() / w;
        // Visibility: (screen tri, ndc z, weights).
        let mut vis: Vec<(u32, f32, [f32; 3])> = vec![(u32::MAX, 0.0, [0.0; 3]); rows * w];
        for &ti in &ob[band] {
            let t = &opaque[ti as usize];
            for y in t.bbox[1].max(y0)..t.bbox[3].min(y0 + rows as i32) {
                let r = (y - y0) as usize * w;
                for x in t.bbox[0]..t.bbox[2] {
                    if let Some((z, l)) = cover(t, x as f64 + 0.5, y as f64 + 0.5) {
                        let v = &mut vis[r + x as usize];
                        if z > v.1 && z <= 1.0 {
                            *v = (ti, z, l);
                        }
                    }
                }
            }
        }
        // A discarded opaque fragment (alpha test, back face) lets the next nearer-than-nothing
        // surface through only on a second look; keep it simple: resolve again without it.
        for i in 0..rows * w {
            let (ti, _, l) = vis[i];
            if ti == u32::MAX {
                continue;
            }
            let t = &opaque[ti as usize];
            let f = frag(s, t, l);
            match shade(s, &f) {
                Some(c) => {
                    crow[i] = c;
                    drow[i] = cam_z(f.pos);
                }
                None => {
                    // Find the nearest surviving fragment under this pixel.
                    let (x, y) = ((i % w) as f64 + 0.5, (y0 as usize + i / w) as f64 + 0.5);
                    let mut best: Option<(f32, [f32; 4], f32)> = None;
                    for &tj in &ob[band] {
                        if tj == ti {
                            continue;
                        }
                        let t2 = &opaque[tj as usize];
                        if (x as i32) < t2.bbox[0] || (x as i32) >= t2.bbox[2] || (y as i32) < t2.bbox[1] || (y as i32) >= t2.bbox[3] {
                            continue;
                        }
                        if let Some((z, l2)) = cover(t2, x, y)
                            && best.is_none_or(|b| z > b.0)
                        {
                            let f2 = frag(s, t2, l2);
                            if let Some(c) = shade(s, &f2) {
                                best = Some((z, c, cam_z(f2.pos)));
                            }
                        }
                    }
                    if let Some((z, c, d)) = best {
                        crow[i] = c;
                        drow[i] = d;
                        vis[i].1 = z;
                    } else {
                        vis[i].1 = 0.0;
                    }
                }
            }
        }
        // Transparent fragments in order, over the opaque result.
        for &ti in &tb[band] {
            let t = &trans[ti as usize];
            for y in t.bbox[1].max(y0)..t.bbox[3].min(y0 + rows as i32) {
                let r = (y - y0) as usize * w;
                for x in t.bbox[0]..t.bbox[2] {
                    let i = r + x as usize;
                    if let Some((z, l)) = cover(t, x as f64 + 0.5, y as f64 + 0.5)
                        && z > vis[i].1
                        && z <= 1.0
                        && let Some(c) = shade(s, &frag(s, t, l))
                    {
                        let d = crow[i];
                        let k = 1.0 - c[3];
                        crow[i] = [c[0] + d[0] * k, c[1] + d[1] * k, c[2] + d[2] * k, c[3] + d[3] * k];
                    }
                }
            }
        }
    });
    Target { width: s.width, height: s.height, color, depth }
}

// ---------------------------------------------------------------- shadow maps

fn basis(eye: [f64; 3], fwd: [f64; 3], down: [f64; 3]) -> [[f32; 4]; 3] {
    use effectcraft_geom::vec3;
    let m = crate::three_d::camera::basis_view(vec3(eye[0], eye[1], eye[2]), vec3(fwd[0], fwd[1], fwd[2]), vec3(down[0], down[1], down[2]));
    [0, 1, 2].map(|r| m.0[r].map(|v| v as f32))
}

/// Rasterise the shadow-casting triangles' light-space depth into `m`'s texels.
fn shadow_raster(s: &Scene, m: &ShadowMap, casters: &[usize]) -> Vec<f32> {
    let n = m.size as usize;
    let mut d = vec![f32::INFINITY; n * n];
    let c = n as f32 * 0.5;
    const NEAR: f32 = 1.0;
    for &t in casters {
        let ids = [s.indices[3 * t] as usize, s.indices[3 * t + 1] as usize, s.indices[3 * t + 2] as usize];
        let mat = &s.materials[s.vertices[ids[0]].material as usize];
        let alpha_tex = (mat.alpha_mode != 0 && mat.tex_base >= 0).then(|| &s.textures[mat.tex_base as usize]);
        // Light space, clipped against the near plane (perspective).
        let q: Vec<([f32; 3], [f32; 2])> = ids
            .iter()
            .map(|&i| {
                let p = s.vertices[i].pos;
                let v = &m.view;
                ([0, 1, 2].map(|r| v[r][0] * p[0] + v[r][1] * p[1] + v[r][2] * p[2] + v[r][3]), s.vertices[i].uv)
            })
            .collect();
        let mut poly = q;
        if !m.ortho {
            let mut out = vec![];
            for i in 0..poly.len() {
                let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
                let (da, db) = (a.0[2] - NEAR, b.0[2] - NEAR);
                if da >= 0.0 {
                    out.push(a);
                }
                if (da >= 0.0) != (db >= 0.0) {
                    let k = da / (da - db);
                    let lerp = |x: f32, y: f32| x + (y - x) * k;
                    out.push(([lerp(a.0[0], b.0[0]), lerp(a.0[1], b.0[1]), lerp(a.0[2], b.0[2])], [lerp(a.1[0], b.1[0]), lerp(a.1[1], b.1[1])]));
                }
            }
            poly = out;
        }
        if poly.len() < 3 {
            continue;
        }
        // Screen: (x, y, 1/z or 1, depth) per corner.
        let scr: Vec<([f64; 2], f32, f32, [f32; 2])> = poly
            .iter()
            .map(|(p, uv)| {
                if m.ortho {
                    ([(p[0] * m.focal + c) as f64, (p[1] * m.focal + c) as f64], 1.0, p[2], *uv)
                } else {
                    let iz = 1.0 / p[2];
                    ([(p[0] * m.focal * iz + c) as f64, (p[1] * m.focal * iz + c) as f64], iz, p[2], *uv)
                }
            })
            .collect();
        for k in 1..scr.len() - 1 {
            let tri = [scr[0], scr[k], scr[k + 1]];
            let (a, b, cc) = (tri[0].0, tri[1].0, tri[2].0);
            let area = (b[0] - a[0]) * (cc[1] - a[1]) - (b[1] - a[1]) * (cc[0] - a[0]);
            if area.abs() < 1e-12 {
                continue;
            }
            let x0 = a[0].min(b[0]).min(cc[0]).floor().max(0.0) as usize;
            let x1 = (a[0].max(b[0]).max(cc[0]).ceil().max(0.0) as usize).min(n);
            let y0 = a[1].min(b[1]).min(cc[1]).floor().max(0.0) as usize;
            let y1 = (a[1].max(b[1]).max(cc[1]).ceil().max(0.0) as usize).min(n);
            for y in y0..y1 {
                for x in x0..x1 {
                    let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                    let e0 = ((cc[0] - b[0]) * (py - b[1]) - (cc[1] - b[1]) * (px - b[0])) / area;
                    let e1 = ((a[0] - cc[0]) * (py - cc[1]) - (a[1] - cc[1]) * (px - cc[0])) / area;
                    let e2 = 1.0 - e0 - e1;
                    if e0 < 0.0 || e1 < 0.0 || e2 < 0.0 {
                        continue;
                    }
                    let l = [e0 as f32, e1 as f32, e2 as f32];
                    let (z, uv) = if m.ortho {
                        (l[0] * tri[0].2 + l[1] * tri[1].2 + l[2] * tri[2].2, [0, 1].map(|i| l[0] * tri[0].3[i] + l[1] * tri[1].3[i] + l[2] * tri[2].3[i]))
                    } else {
                        let iz = l[0] * tri[0].1 + l[1] * tri[1].1 + l[2] * tri[2].1;
                        let z = 1.0 / iz;
                        (z, [0, 1].map(|i| (l[0] * tri[0].3[i] * tri[0].1 + l[1] * tri[1].3[i] * tri[1].1 + l[2] * tri[2].3[i] * tri[2].1) * z))
                    };
                    let px = &mut d[y * n + x];
                    if z >= *px {
                        continue;
                    }
                    if let Some(t) = alpha_tex
                        && sample(t, &s.texels, uv[0], uv[1])[3] * mat.base[3] * mat.opacity < 0.5
                    {
                        continue;
                    }
                    *px = z;
                }
            }
        }
    }
    d
}

/// Hard cap on the shadow depth texels one render allocates (f32 depth texels).
/// Quality map sizes derive from the raster resolution, and both the resolution and
/// the light count come from the project, so without a scene-wide budget an
/// arbitrarily large comp with arbitrarily many casting lights (point lights cost
/// six faces) could balloon the allocation. 26 M texels (≈ 104 MB) holds one point
/// light's full 2048-texel cube (6 × 2048² ≈ 25.2 M) and stays under WebGPU's default
/// 128 MiB storage-binding limit; adapters with less fall back to the CPU raster.
pub(crate) const MAX_SHADOW_TEXELS: u64 = 26_000_000;

/// Long side of a quality shadow map: one map texel per output pixel, to a power of
/// two, clamped to 512..4096 (raster sizes are user data and may be absurd). Draft
/// (interactive fast-preview) renders keep the flat 512 budget, unchanged.
pub(crate) fn shadow_map_size(width: u32, height: u32, draft: bool) -> u32 {
    if draft {
        return 512;
    }
    let side = width.max(height).clamp(512, 4096);
    let mut size = 512u32;
    while size < side {
        size *= 2;
    }
    size
}

/// Per-face size of a point light's cube map: the full map size, since each face spans
/// 90° and anything smaller is coarser than a spot light's map in the same comp (a
/// point light low over a floor shows exactly that as stair-steps), capped at 2048 so
/// one cube fits [`MAX_SHADOW_TEXELS`]. Draft keeps half the map size as before.
pub(crate) fn shadow_point_face_size(size: u32, draft: bool) -> u32 {
    if draft { (size / 2).max(1) } else { size.min(2048) }
}

/// Per-light map sizes (`None` = this light gets no shadow map). `kinds`: 0 = no
/// shadows (costs nothing), 1 = single map (spot/parallel), 2 = point cube (budgeted
/// as its six faces). Lights start at their full size; while the scene exceeds
/// [`MAX_SHADOW_TEXELS`] the largest map above its floor is halved (power-of-two
/// steps, the earliest light first on ties), and once every light is at its floor the
/// last lights lose their shadows entirely. Deterministic, and O(n log n) in the
/// light count: each light is halved at most a handful of times and dropped once.
pub(crate) fn shadow_sizes(width: u32, height: u32, draft: bool, kinds: &[u32]) -> Vec<Option<u32>> {
    let base = shadow_map_size(width, height, draft);
    let faces = |k: u32| -> u64 { if k == 2 { 6 } else { 1 } };
    let floor = |k: u32| -> u32 { if k == 2 { 128 } else { 512 } };
    let mut sizes: Vec<u32> = kinds
        .iter()
        .map(|&k| match k {
            0 => 0,
            2 => shadow_point_face_size(base, draft),
            _ => base,
        })
        .collect();
    let cost = |z: u32, k: u32| -> u64 { (z as u64) * (z as u64) * faces(k) };
    let mut total: u64 = sizes.iter().zip(kinds).map(|(&z, &k)| cost(z, k)).sum();
    // Largest size first, then the lowest index.
    let mut heap: BinaryHeap<(u32, Reverse<usize>)> =
        sizes.iter().zip(kinds).enumerate().filter(|(_, (z, k))| **z > floor(**k)).map(|(i, (&z, _))| (z, Reverse(i))).collect();
    while total > MAX_SHADOW_TEXELS {
        let Some((z, Reverse(i))) = heap.pop() else { break };
        let k = kinds[i];
        total -= cost(z, k) - cost(z / 2, k);
        sizes[i] = z / 2;
        if z / 2 > floor(k) {
            heap.push((z / 2, Reverse(i)));
        }
    }
    for i in (0..sizes.len()).rev() {
        if total <= MAX_SHADOW_TEXELS {
            break;
        }
        total -= cost(sizes[i], kinds[i]);
        sizes[i] = 0;
    }
    sizes.into_iter().map(|z| (z > 0).then_some(z)).collect()
}

/// The Environment light's shadow map: one orthographic map over the scene's bounding sphere
/// from its dominant direction (the light travels from the bright region into the scene), like
/// a parallel light's.
fn env_shadow_map(e: EnvInfo, centre: [f64; 3], radius: f64, size: u32) -> ShadowMap {
    // After Effects' environment-light shadows are soft out of the box, and this
    // map covers the whole scene bounding sphere — not the view — so its texels
    // can be coarse (all the more in a draft render's flat 512 budget). A
    // penumbra of at least eight output pixels, or four map texels where those
    // are coarser, keeps the edge smooth at any map resolution; Shadow Diffusion
    // widens it further, as for the other lights.
    let texel = 2.0 * radius / size as f64;
    let soft = (4.0 * texel).max(8.0) + (e.shadow_diffusion as f64 * 0.5).min(24.0);
    let sun = e.sun.map(|v| v as f64);
    let dir = [-sun[0], -sun[1], -sun[2]];
    let eye = [centre[0] + sun[0] * radius * 2.0, centre[1] + sun[1] * radius * 2.0, centre[2] + sun[2] * radius * 2.0];
    let hint = if dir[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [0.0, 0.0, 1.0] };
    let focal = (size as f64 * 0.5 / radius) as f32;
    ShadowMap { view: basis(eye, dir, hint), focal, ortho: true, size, offset: 0, bias: 1.5, radius: soft as f32 }
}

/// Build the shadow maps of every shadow-casting light (lights marked `shadow == 0`), and of
/// the Environment light when it casts shadows and its image has a dominant direction (marked
/// `env.shadow == 0`, `env.sun != [0; 3]`): one map from that direction, like a parallel light.
pub(crate) fn shadow_maps(s: &mut Scene, draft: bool) {
    let casters: Vec<usize> = (0..s.indices.len() / 3)
        .filter(|&t| s.materials.get(s.vertices[s.indices[3 * t] as usize].material as usize).is_some_and(|m| m.casts_shadows))
        .collect();
    // The Environment light casts a shadow when marked and its image has a dominant direction;
    // with nothing to cast one, its mark is cleared either way.
    let env_shadow = !casters.is_empty() && s.env.is_some_and(|e| e.shadow == 0 && e.sun != [0.0; 3]);
    if !env_shadow && let Some(e) = &mut s.env {
        e.shadow = -1;
    }
    if casters.is_empty() {
        for l in &mut s.lights {
            l.shadow = -1;
        }
        return;
    }
    // Bounding sphere of the scene (parallel lights' orthographic frustum).
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for v in &s.vertices {
        for k in 0..3 {
            lo[k] = lo[k].min(v.pos[k] as f64);
            hi[k] = hi[k].max(v.pos[k] as f64);
        }
    }
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) * 0.5);
    let radius = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt() * 0.5 + 1.0;
    // Map sizes come from the raster (a quality map gives ~one texel per output
    // pixel); the per-light budget keeps the scene allocation bounded because
    // comp size and light count are user input (point lights cost six faces).
    let kinds: Vec<u32> = s
        .lights
        .iter()
        .map(|l| {
            if l.shadow != 0 {
                0
            } else if l.kind > 1 {
                2
            } else {
                1
            }
        })
        // The Environment light's map is a single orthographic one.
        .chain(std::iter::once(if env_shadow { 1 } else { 0 }))
        .collect();
    let sizes = shadow_sizes(s.width, s.height, draft, &kinds);
    let mut maps: Vec<ShadowMap> = vec![];
    for li in 0..s.lights.len() {
        let Some(size) = sizes.get(li).copied().flatten() else {
            s.lights[li].shadow = -1;
            continue;
        };
        if s.lights[li].shadow != 0 {
            s.lights[li].shadow = -1;
            continue;
        }
        let l = s.lights[li];
        let first = maps.len() as i32;
        // Penumbra width in output pixels (the shader turns it into a tap reach at
        // the receiver); the sampling bias is deliberately independent of it.
        let soft = 1.0 + (l.shadow_diffusion * 0.5).min(24.0);
        let pos = l.pos.map(|v| v as f64);
        let dir = l.dir.map(|v| v as f64);
        let hint = if dir[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [0.0, 0.0, 1.0] };
        match l.kind {
            0 => {
                let eye = [centre[0] - dir[0] * radius * 2.0, centre[1] - dir[1] * radius * 2.0, centre[2] - dir[2] * radius * 2.0];
                let focal = (size as f64 * 0.5 / radius) as f32;
                maps.push(ShadowMap { view: basis(eye, dir, hint), focal, ortho: true, size, offset: 0, bias: 1.5, radius: soft });
            }
            1 => {
                let half = (l.cos_outer.clamp(-1.0, 1.0).acos() as f64 + 0.05).min(1.45);
                let focal = (size as f64 * 0.5 / half.tan()) as f32;
                maps.push(ShadowMap { view: basis(pos, dir, hint), focal, ortho: false, size, offset: 0, bias: 1.5, radius: soft });
            }
            _ => {
                let fs = size;
                for (f, d) in [
                    ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
                    ([-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
                    ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
                    ([0.0, -1.0, 0.0], [0.0, 0.0, 1.0]),
                    ([0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
                    ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
                ] {
                    maps.push(ShadowMap { view: basis(pos, f, d), focal: fs as f32 * 0.5, ortho: false, size: fs, offset: 0, bias: 1.5, radius: soft });
                }
            }
        }
        s.lights[li].shadow = first;
    }
    // The Environment light's shadow: one orthographic map over the scene from its dominant
    // direction (the light travels from the bright region into the scene), after the lights'.
    if env_shadow && let Some(e) = s.env {
        match sizes.get(s.lights.len()).copied().flatten() {
            Some(size) => {
                let first = maps.len() as i32;
                maps.push(env_shadow_map(e, centre, radius, size));
                if let Some(e) = &mut s.env {
                    e.shadow = first;
                }
            }
            // The texel budget dropped it.
            None => {
                if let Some(e) = &mut s.env {
                    e.shadow = -1;
                }
            }
        }
    }
    let texels: Vec<Vec<f32>> = maps.par_iter().map(|m| shadow_raster(s, m, &casters)).collect();
    for (m, t) in maps.iter_mut().zip(texels) {
        m.offset = s.shadow_texels.len() as u32;
        s.shadow_texels.extend(t);
    }
    s.shadows = maps;
}
