//! Extruded, bevelled meshes from 2D outlines (Advanced 3D text and shape layers' Geometry
//! Options: Bevel Style, Bevel Depth, Hole Bevel Depth, Extrusion Depth).
//!
//! Input outlines are closed polylines in layer space (x right, y down). The mesh is in layer
//! space too: the front cap lies on the layer plane (z = 0, facing the camera, −z), the back cap
//! at z = depth. Each contour gets a ring of vertices per profile step (inset from the outline
//! by the bevel, at a depth along the profile); consecutive rings are joined by quads, the caps
//! are triangulated from the innermost front/back rings with holes. Caps and walls share
//! positions exactly, so the mesh is watertight (welded by position).

use serde::{Deserialize, Serialize};

use crate::Primitive;
use crate::triangulate::{signed_area, triangulate};

type P = [f64; 2];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BevelStyle {
    #[default]
    None,
    Angular,
    Concave,
    Convex,
}

impl BevelStyle {
    pub const ALL: [BevelStyle; 4] = [BevelStyle::None, BevelStyle::Angular, BevelStyle::Concave, BevelStyle::Convex];
    pub fn label(self) -> &'static str {
        match self {
            BevelStyle::None => "None",
            BevelStyle::Angular => "Angular",
            BevelStyle::Concave => "Concave",
            BevelStyle::Convex => "Convex",
        }
    }
    pub fn from_index(i: u32) -> BevelStyle {
        BevelStyle::ALL.get(i as usize).copied().unwrap_or_default()
    }
    /// (inset fraction 1 → 0, depth fraction 0 → 1) along the profile at `t` in 0..1.
    fn profile(self, t: f64) -> (f64, f64) {
        let th = t * std::f64::consts::FRAC_PI_2;
        match self {
            BevelStyle::None | BevelStyle::Angular => (1.0 - t, t),
            // Quarter circle bulging out towards the outline corner.
            BevelStyle::Convex => (1.0 - th.sin(), 1.0 - th.cos()),
            // Quarter circle scooped in.
            BevelStyle::Concave => (th.cos(), th.sin()),
        }
    }
    fn steps(self) -> usize {
        match self {
            BevelStyle::None => 0,
            BevelStyle::Angular => 1,
            _ => 6,
        }
    }
}

/// Geometry Options of an extruded layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExtrudeParams {
    pub bevel: BevelStyle,
    /// Pixels.
    pub bevel_depth: f64,
    /// Hole bevels as a fraction of the bevel depth (0..1).
    pub hole_bevel: f64,
    /// Pixels (> 0).
    pub depth: f64,
}

impl Default for ExtrudeParams {
    fn default() -> Self {
        ExtrudeParams { bevel: BevelStyle::None, bevel_depth: 2.0, hole_bevel: 1.0, depth: 50.0 }
    }
}

/// An outer contour with its holes.
#[derive(Clone, Debug, PartialEq)]
pub struct Outline {
    pub outer: Vec<P>,
    pub holes: Vec<Vec<P>>,
}

fn point_in(p: P, c: &[P]) -> bool {
    let n = c.len();
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (c[i], c[j]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Group closed contours into outlines by nesting depth (even depth = outer, odd = hole of the
/// innermost containing outer contour). Degenerate contours are dropped.
pub fn group_contours(contours: Vec<Vec<P>>) -> Vec<Outline> {
    let cs: Vec<Vec<P>> = contours
        .into_iter()
        .map(|mut c| {
            c.dedup();
            while c.len() > 1 && c.first() == c.last() {
                c.pop();
            }
            c
        })
        .filter(|c| c.len() >= 3 && signed_area(c).abs() > 1e-9)
        .collect();
    let n = cs.len();
    let area: Vec<f64> = cs.iter().map(|c| signed_area(c).abs()).collect();
    // Containers of each contour (by a vertex inside; nested contours do not cross).
    let parents: Vec<Vec<usize>> = (0..n)
        .map(|i| {
            let probe = cs[i][0];
            (0..n).filter(|&j| j != i && area[j] > area[i] && point_in(probe, &cs[j])).collect()
        })
        .collect();
    let mut outlines: Vec<Outline> = vec![];
    let mut slot = vec![usize::MAX; n];
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| parents[i].len());
    for i in order {
        let depth = parents[i].len();
        if depth.is_multiple_of(2) {
            slot[i] = outlines.len();
            outlines.push(Outline { outer: cs[i].clone(), holes: vec![] });
        } else if let Some(&p) = parents[i].iter().filter(|&&j| parents[j].len() == depth - 1).min_by(|&&a, &&b| area[a].total_cmp(&area[b]))
            && slot[p] != usize::MAX
        {
            outlines[slot[p]].holes.push(cs[i].clone());
        }
    }
    outlines
}

struct Contour {
    pts: Vec<P>,
    /// Outward unit normal of edge i (pts[i] → pts[i+1]).
    edge_n: Vec<P>,
    /// Miter offset direction per vertex (outward, length ≥ 1).
    miter: Vec<P>,
    /// Smooth-shaded vertex (shallow corner).
    smooth: Vec<bool>,
    bevel: f64,
}

fn contour(mut pts: Vec<P>, outer: bool, bevel: f64) -> Contour {
    // Outer counter-clockwise (in x/y), holes clockwise: the outward normal is (dy, −dx).
    let ccw = signed_area(&pts) > 0.0;
    if ccw != outer {
        pts.reverse();
    }
    let n = pts.len();
    let edge_n: Vec<P> = (0..n)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let l = dx.hypot(dy).max(1e-12);
            [dy / l, -dx / l]
        })
        .collect();
    let mut miter = vec![[0.0; 2]; n];
    let mut smooth = vec![false; n];
    for i in 0..n {
        let (n0, n1) = (edge_n[(i + n - 1) % n], edge_n[i]);
        let d = n0[0] * n1[0] + n0[1] * n1[1];
        let s = [n0[0] + n1[0], n0[1] + n1[1]];
        let k = 1.0 + d;
        miter[i] = if k < 0.25 {
            // Very sharp corner: limit the miter (offset along the bisector, length 2.8).
            let l = s[0].hypot(s[1]).max(1e-12);
            [s[0] / l * 2.8, s[1] / l * 2.8]
        } else {
            [s[0] / k, s[1] / k]
        };
        smooth[i] = d > 0.82; // < ~35° turn
    }
    Contour { pts, edge_n, miter, smooth, bevel }
}

/// Rings of a contour: (inset, z) from the front cap to the back cap.
fn rings(style: BevelStyle, bevel: f64, depth: f64) -> Vec<(f64, f64)> {
    let b = bevel.clamp(0.0, depth * 0.5);
    let steps = if b > 1e-9 { style.steps() } else { 0 };
    if steps == 0 {
        return vec![(0.0, 0.0), (0.0, depth)];
    }
    let front: Vec<(f64, f64)> = (0..=steps)
        .map(|k| {
            let (f, g) = style.profile(k as f64 / steps as f64);
            (b * f, b * g)
        })
        .collect();
    let mut out = front.clone();
    let back: Vec<(f64, f64)> = front.iter().rev().map(|&(i, z)| (i, depth - z)).collect();
    // Skip a duplicate ring where the walls have zero height (bevel = depth / 2).
    let start = if (back[0].1 - out[out.len() - 1].1).abs() < 1e-9 { 1 } else { 0 };
    out.extend_from_slice(&back[start..]);
    out
}

/// A surface of an extruded mesh, which After Effects colours and shades separately (the
/// Front / Bevel / Side / Back material groups).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Surface {
    /// The front cap (facing the camera, on the layer plane).
    Front,
    /// The bevelled rings between the caps and the walls (front and back).
    Bevel,
    /// The straight walls.
    Side,
    /// The back cap.
    Back,
}

impl Surface {
    pub const ALL: [Surface; 4] = [Surface::Front, Surface::Bevel, Surface::Side, Surface::Back];
    /// Index into [`Surface::ALL`] (and into the primitives of [`extrude_surfaces`]).
    pub fn index(self) -> usize {
        self as usize
    }
}

/// Build the extruded mesh of outlines (one primitive, material 0).
pub fn extrude(outlines: &[Outline], p: &ExtrudeParams) -> Primitive {
    build(outlines, p, false).into_iter().next().unwrap_or_default()
}

/// [`extrude`] split by surface: one primitive (material 0) per [`Surface`], in
/// [`Surface::ALL`] order. Together they hold the same triangles as [`extrude`].
pub fn extrude_surfaces(outlines: &[Outline], p: &ExtrudeParams) -> [Primitive; 4] {
    let mut v = build(outlines, p, true).into_iter();
    std::array::from_fn(|_| v.next().unwrap_or_default())
}

/// The extruded mesh in one primitive (`split` false) or one per [`Surface`].
fn build(outlines: &[Outline], p: &ExtrudeParams, split: bool) -> Vec<Primitive> {
    let mut prims = vec![Primitive { material: Some(0), ..Default::default() }; if split { 4 } else { 1 }];
    let slot = |s: Surface| if split { s.index() } else { 0 };
    let depth = p.depth.max(1e-3);
    let push = |prim: &mut Primitive, pos: [f64; 3], n: [f64; 3]| -> u32 {
        prim.positions.push([pos[0] as f32, pos[1] as f32, pos[2] as f32]);
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-12);
        prim.normals.push([(n[0] / l) as f32, (n[1] / l) as f32, (n[2] / l) as f32]);
        prim.uvs.push([0.0, 0.0]);
        (prim.positions.len() - 1) as u32
    };
    for o in outlines {
        let mut cs = vec![contour(o.outer.clone(), true, p.bevel_depth)];
        for h in &o.holes {
            cs.push(contour(h.clone(), false, p.bevel_depth * p.hole_bevel.clamp(0.0, 1.0)));
        }
        // Positions per contour per ring.
        let mut ring_pos: Vec<Vec<Vec<[f64; 3]>>> = vec![];
        let mut ring_par: Vec<Vec<(f64, f64)>> = vec![];
        for c in &cs {
            let rs = rings(p.bevel, c.bevel, depth);
            let pos: Vec<Vec<[f64; 3]>> =
                rs.iter().map(|&(inset, z)| c.pts.iter().zip(&c.miter).map(|(q, m)| [q[0] - m[0] * inset, q[1] - m[1] * inset, z]).collect()).collect();
            ring_pos.push(pos);
            ring_par.push(rs);
        }
        // Walls and bevels.
        for (ci, c) in cs.iter().enumerate() {
            let rs = &ring_par[ci];
            let n = c.pts.len();
            let curved = matches!(p.bevel, BevelStyle::Concave | BevelStyle::Convex);
            // Profile normal (outward weight, z weight) of segment k.
            let seg_n = |k: usize| -> (f64, f64) {
                let (i0, z0) = rs[k];
                let (i1, z1) = rs[k + 1];
                let (di, dz) = (i1 - i0, z1 - z0);
                let l = di.hypot(dz).max(1e-12);
                (dz / l, di / l)
            };
            for k in 0..rs.len() - 1 {
                let is_wall = (rs[k].0 - rs[k + 1].0).abs() < 1e-12;
                let (a0, a1) = (seg_n(k), seg_n(k));
                // Curved bevels: average with the neighbouring bevel segments at shared rings.
                let blend = |k2: Option<usize>, base: (f64, f64)| -> (f64, f64) {
                    match k2 {
                        Some(k2) if curved && !is_wall && k2 < rs.len() - 1 && (rs[k2].0 - rs[k2 + 1].0).abs() >= 1e-12 => {
                            let o = seg_n(k2);
                            (base.0 + o.0, base.1 + o.1)
                        }
                        _ => base,
                    }
                };
                let pa = blend(k.checked_sub(1), a0);
                let pb = blend(Some(k + 1), a1);
                for i in 0..n {
                    let j = (i + 1) % n;
                    let en = c.edge_n[i];
                    let vn = |v: usize| -> P {
                        if c.smooth[v] {
                            let m = c.miter[v];
                            let l = m[0].hypot(m[1]).max(1e-12);
                            [m[0] / l, m[1] / l]
                        } else {
                            en
                        }
                    };
                    let (ni, nj) = (vn(i), vn(j));
                    let nrm = |xy: P, pr: (f64, f64)| [xy[0] * pr.0, xy[1] * pr.0, pr.1];
                    let Some(prim) = prims.get_mut(slot(if is_wall { Surface::Side } else { Surface::Bevel })) else { continue };
                    let ai = push(prim, ring_pos[ci][k][i], nrm(ni, pa));
                    let aj = push(prim, ring_pos[ci][k][j], nrm(nj, pa));
                    let bj = push(prim, ring_pos[ci][k + 1][j], nrm(nj, pb));
                    let bi = push(prim, ring_pos[ci][k + 1][i], nrm(ni, pb));
                    prim.indices.extend([ai, aj, bj, ai, bj, bi]);
                }
            }
        }
        // Caps: triangulate in x/y (counter-clockwise → +z normal: the back cap; the front cap
        // is reversed to face the camera).
        let outer2: Vec<P> = ring_pos[0][0].iter().map(|q| [q[0], q[1]]).collect();
        let holes2: Vec<Vec<P>> = ring_pos[1..].iter().map(|r| r[0].iter().map(|q| [q[0], q[1]]).collect()).collect();
        let tris = triangulate(&outer2, &holes2);
        let flat: Vec<(usize, usize)> = (0..cs.len()).flat_map(|ci| (0..cs[ci].pts.len()).map(move |i| (ci, i))).collect();
        for (front, nz) in [(true, -1.0), (false, 1.0)] {
            let Some(prim) = prims.get_mut(slot(if front { Surface::Front } else { Surface::Back })) else { continue };
            let base = prim.positions.len() as u32;
            for &(ci, i) in &flat {
                let r = if front { 0 } else { ring_pos[ci].len() - 1 };
                push(prim, ring_pos[ci][r][i], [0.0, 0.0, nz]);
            }
            for t in &tris {
                let (a, b, c) = (base + t[0] as u32, base + t[1] as u32, base + t[2] as u32);
                if front { prim.indices.extend([a, c, b]) } else { prim.indices.extend([a, b, c]) }
            }
        }
    }
    prims
}

/// Gap (pixels) between stacked extrusions of one layer: paint drawn later (a stroke over its
/// fill, an upper shape group) sits this much nearer the camera per level, so coplanar front
/// caps never fight in the depth buffer and the stacking order of the 2D layer is kept.
pub const STACK_GAP: f64 = 0.02;

/// [`extrude`] at stacking level `level` (0 = bottom paint of the layer): the mesh moves
/// `level × STACK_GAP` towards the camera (−z).
pub fn extrude_stacked(outlines: &[Outline], p: &ExtrudeParams, level: usize) -> Primitive {
    let mut prim = extrude(outlines, p);
    stack(&mut prim, level);
    prim
}

/// [`extrude_surfaces`] at stacking level `level` (see [`extrude_stacked`]).
pub fn extrude_surfaces_stacked(outlines: &[Outline], p: &ExtrudeParams, level: usize) -> [Primitive; 4] {
    let mut prims = extrude_surfaces(outlines, p);
    for prim in &mut prims {
        stack(prim, level);
    }
    prims
}

fn stack(prim: &mut Primitive, level: usize) {
    let dz = (level as f64 * STACK_GAP) as f32;
    for v in &mut prim.positions {
        v[2] -= dz;
    }
}
