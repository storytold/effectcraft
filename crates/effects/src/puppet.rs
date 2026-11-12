//! Puppet: mesh deformation driven by pins (the Puppet Position, Advanced, Bend, Starch and
//! Overlap Pin tools).
//!
//! ```text
//! Puppet                       (effect `ec.distort.puppet`; Mesh Rotation Refinement)
//!   Mesh 1                     (match `mesh`: Density, Expansion, Triangles; hidden seed)
//!     Deform                   (match `deform`)
//!       Puppet Pin 1           (match `pin`: Position | Scale, Rotation | Amount, Extent |
//!                               In Front, Extent; hidden pin type and rest position)
//! ```
//!
//! Each frame the mesh is built from the effect's input alpha (see [`mesh`]; cached by content),
//! pins are attached to their nearest mesh vertices at rest, the as-rigid-as-possible solver
//! ([`arap`]) moves the vertices, and the input is texture-mapped through the deformed
//! triangles (bilinear sampling, 2×2 coverage anti-aliasing on the outline, Overlap pins'
//! In Front deciding which triangles draw on top).

pub mod arap;
pub mod mesh;

use std::sync::{Arc, Mutex};

use effectcraft_keyframe::Value;
use effectcraft_project::build::Ids;
use effectcraft_project::{GroupKind, ParamUi, PropGroup, Property};
use effectcraft_raster::Image;
use rayon::prelude::*;

use crate::{Buf, EffectCtx, EffectSpec, Params, p, slider};
pub use mesh::{Mesh, MeshOpts};

/// Effect id of the Puppet effect.
pub const ID: &str = "ec.distort.puppet";

/// Puppet pin types (the five Puppet tools).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PinKind {
    #[default]
    Position,
    Advanced,
    Bend,
    Starch,
    Overlap,
}

impl PinKind {
    pub const ALL: [PinKind; 5] = [PinKind::Position, PinKind::Advanced, PinKind::Bend, PinKind::Starch, PinKind::Overlap];
    pub fn index(self) -> u32 {
        PinKind::ALL.iter().position(|k| *k == self).unwrap_or(0) as u32
    }
    pub fn from_index(i: u32) -> PinKind {
        PinKind::ALL.get(i as usize).copied().unwrap_or_default()
    }
    pub fn from_name(s: &str) -> Option<PinKind> {
        let n = s.to_ascii_lowercase().replace([' ', '_', '-'], "");
        Some(match n.trim_end_matches("pin") {
            "position" | "puppet" | "puppetposition" => PinKind::Position,
            "advanced" | "puppetadvanced" => PinKind::Advanced,
            "bend" | "puppetbend" => PinKind::Bend,
            "starch" | "puppetstarch" => PinKind::Starch,
            "overlap" | "puppetoverlap" => PinKind::Overlap,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            PinKind::Position => "position",
            PinKind::Advanced => "advanced",
            PinKind::Bend => "bend",
            PinKind::Starch => "starch",
            PinKind::Overlap => "overlap",
        }
    }
    /// Group name prefix (`Puppet Pin 3`, `Starch 1`, `Overlap 2`).
    pub fn label(self) -> &'static str {
        match self {
            PinKind::Starch => "Starch",
            PinKind::Overlap => "Overlap",
            _ => "Puppet Pin",
        }
    }
    /// Pins that hold a position (Starch and Overlap pins sit on the rest mesh).
    pub fn moves(self) -> bool {
        matches!(self, PinKind::Position | PinKind::Advanced)
    }
}

pub fn specs() -> Vec<EffectSpec> {
    vec![EffectSpec {
        id: ID,
        name: "Puppet",
        category: "Distort",
        params: vec![p("refinement", "Mesh Rotation Refinement", Value::Scalar(20.0), slider(0.0, 100.0, 0.0, 100.0, 0))],
        render,
        gpu: false,
        float: true,
    }]
}

fn hidden(ids: &mut Ids, m: &str, v: Value) -> Property {
    let mut p = ids.prop(m, m, v).with_ui(ParamUi::Hidden);
    p.static_only = true;
    p
}

/// A new mesh group (`Mesh 1`) whose outline is the alpha region under `seed` (layer space).
pub fn mesh_group(ids: &mut Ids, name: &str, seed: [f64; 2], opts: &MeshOpts) -> PropGroup {
    let mut g = ids.group("mesh", name);
    g.kind = GroupKind::Indexed;
    g.children.push(ids.prop("density", "Density", Value::Scalar(opts.density)).with_ui(slider(0.0, 100.0, 0.0, 100.0, 0)).into());
    g.children.push(ids.prop("expansion", "Expansion", Value::Scalar(opts.expansion)).with_ui(slider(-100.0, 200.0, -20.0, 50.0, 1)).into());
    g.children.push(ids.prop("triangles", "Triangles", Value::Scalar(opts.triangles)).with_ui(slider(10.0, 10_000.0, 10.0, 2000.0, 0)).into());
    g.children.push(hidden(ids, "seed", Value::Vec2(seed)).into());
    g.children.push(ids.group("deform", "Deform").into());
    g
}

/// A new pin group at layer-space `rest`.
pub fn pin_group(ids: &mut Ids, name: &str, kind: PinKind, rest: [f64; 2]) -> PropGroup {
    let mut g = ids.group("pin", name);
    g.kind = GroupKind::Indexed;
    g.children.push(hidden(ids, "kind", Value::Enum(kind.index())).into());
    g.children.push(hidden(ids, "rest", Value::Vec2(rest)).into());
    if kind != PinKind::Bend {
        g.children.push(ids.prop("position", "Position", Value::Vec2(rest)).with_ui(ParamUi::Point).spatial().into());
    }
    match kind {
        PinKind::Advanced | PinKind::Bend => {
            g.children.push(ids.prop("scale", "Scale", Value::Scalar(100.0)).with_ui(ParamUi::Percent).into());
            g.children.push(ids.prop("rotation", "Rotation", Value::Scalar(0.0)).with_ui(ParamUi::Angle).into());
        }
        PinKind::Starch => {
            g.children.push(ids.prop("amount", "Amount", Value::Scalar(50.0)).with_ui(slider(0.0, 100.0, 0.0, 100.0, 1)).into());
            g.children.push(ids.prop("extent", "Extent", Value::Scalar(15.0)).with_ui(ParamUi::Pixels).into());
        }
        PinKind::Overlap => {
            g.children.push(ids.prop("in_front", "In Front", Value::Scalar(50.0)).with_ui(slider(-100.0, 100.0, -100.0, 100.0, 1)).into());
            g.children.push(ids.prop("extent", "Extent", Value::Scalar(15.0)).with_ui(ParamUi::Pixels).into());
        }
        PinKind::Position => {}
    }
    g
}

/// Mesh groups of a Puppet effect instance.
pub fn meshes(fx: &PropGroup) -> impl Iterator<Item = &PropGroup> {
    fx.groups().filter(|g| g.match_id == "mesh")
}

/// Pin groups of a mesh group.
pub fn pins(mesh: &PropGroup) -> impl Iterator<Item = &PropGroup> {
    mesh.sub("deform").into_iter().flat_map(|d| d.groups().filter(|g| g.match_id == "pin"))
}

/// Next pin name for a kind, unique across the effect's meshes (`fx` is the Puppet effect).
pub fn next_pin_name(fx: &PropGroup, kind: PinKind) -> String {
    let n = meshes(fx).flat_map(pins).filter(|p| PinKind::from_index(p.get("kind").map(|k| k.value.as_enum()).unwrap_or(0)).label() == kind.label()).count();
    format!("{} {}", kind.label(), n + 1)
}

/// Is `g` a Puppet effect instance?
pub fn is_puppet(g: &PropGroup) -> bool {
    matches!(&g.kind, GroupKind::Effect { effect } if effect == ID)
}

/// A pin evaluated at the render time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pin {
    pub kind: PinKind,
    pub uid: u64,
    pub rest: [f64; 2],
    pub position: [f64; 2],
    pub scale: f64,
    pub rotation: f64,
    pub amount: f64,
    pub extent: f64,
    pub in_front: f64,
}

/// A mesh group evaluated at the render time.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshParams {
    pub uid: u64,
    pub seed: [f64; 2],
    pub opts: MeshOpts,
    pub pins: Vec<Pin>,
}

fn fd(p: &Params, k: &str, d: f64) -> f64 {
    p.get(k).map(Value::as_f64).unwrap_or(d)
}

/// Parse the meshes and pins out of flattened Puppet parameters.
pub fn parse(params: &Params) -> Vec<MeshParams> {
    let mut out = vec![];
    for pre in params.groups("") {
        if params.s(&format!("{pre}@match")) != "mesh" || !params.get(&format!("{pre}@enabled")).is_none_or(Value::as_bool) {
            continue;
        }
        let k = |m: &str| format!("{pre}{m}");
        let mut pins = vec![];
        if let Some(d) = params.group(&pre, "deform") {
            for pp in params.groups(&d) {
                if params.s(&format!("{pp}@match")) != "pin" || !params.get(&format!("{pp}@enabled")).is_none_or(Value::as_bool) {
                    continue;
                }
                let q = |m: &str| format!("{pp}{m}");
                let rest = params.get(&q("rest")).map(Value::as_vec2).unwrap_or([0.0; 2]);
                pins.push(Pin {
                    kind: PinKind::from_index(params.get(&q("kind")).map(Value::as_enum).unwrap_or(0)),
                    uid: fd(params, &q("@uid"), 0.0) as u64,
                    rest,
                    position: params.get(&q("position")).map(Value::as_vec2).unwrap_or(rest),
                    scale: fd(params, &q("scale"), 100.0) / 100.0,
                    rotation: fd(params, &q("rotation"), 0.0),
                    amount: fd(params, &q("amount"), 50.0),
                    extent: fd(params, &q("extent"), 15.0),
                    in_front: fd(params, &q("in_front"), 50.0),
                });
            }
        }
        out.push(MeshParams {
            uid: fd(params, &k("@uid"), 0.0) as u64,
            seed: params.get(&k("seed")).map(Value::as_vec2).unwrap_or([0.0; 2]),
            opts: MeshOpts {
                density: fd(params, &k("density"), 50.0),
                expansion: fd(params, &k("expansion"), 3.0),
                triangles: fd(params, &k("triangles"), 350.0),
            },
            pins,
        });
    }
    out
}

// ------------------------------------------------------------------------------------------
// Mesh cache.

struct MeshCache {
    entries: Vec<(u64, Arc<Mesh>)>,
}

static CACHE: Mutex<MeshCache> = Mutex::new(MeshCache { entries: Vec::new() });

fn fnv(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h ^= *b as u64;
        *h = h.wrapping_mul(0x0100_0000_01b3);
    }
}

/// The mesh for `m` over a buffer's alpha (built once per distinct outline and options).
pub fn mesh_for(buf: &Buf, m: &MeshParams) -> Arc<Mesh> {
    let (w, h) = (buf.img.width as usize, buf.img.height as usize);
    let alpha: Vec<f32> = buf.img.data.iter().map(|p| p[3]).collect();
    let mut key = 0xcbf2_9ce4_8422_2325u64;
    fnv(&mut key, &(w as u64).to_le_bytes());
    fnv(&mut key, &(h as u64).to_le_bytes());
    for v in [buf.scale, buf.offset[0], buf.offset[1], m.seed[0], m.seed[1], m.opts.density, m.opts.expansion, m.opts.triangles] {
        fnv(&mut key, &v.to_bits().to_le_bytes());
    }
    let mut bits = 0u8;
    for (i, a) in alpha.iter().enumerate() {
        bits = (bits << 1) | (*a >= 0.5) as u8;
        if i % 8 == 7 {
            fnv(&mut key, &[bits]);
        }
    }
    fnv(&mut key, &[bits]);
    if let Ok(c) = CACHE.lock()
        && let Some((_, mesh)) = c.entries.iter().find(|(k, _)| *k == key)
    {
        return mesh.clone();
    }
    let outline = mesh::outline(&alpha, w, h, buf.scale.max(1e-9), buf.offset, m.seed, m.opts.expansion);
    let mesh = Arc::new(mesh::triangulate(&outline, &m.opts));
    if let Ok(mut c) = CACHE.lock() {
        c.entries.push((key, mesh.clone()));
        if c.entries.len() > 16 {
            c.entries.remove(0);
        }
    }
    mesh
}

fn nearest(verts: &[[f64; 2]], p: [f64; 2]) -> Option<usize> {
    (0..verts.len()).min_by(|a, b| {
        let da = (verts[*a][0] - p[0]).powi(2) + (verts[*a][1] - p[1]).powi(2);
        let db = (verts[*b][0] - p[0]).powi(2) + (verts[*b][1] - p[1]).powi(2);
        da.total_cmp(&db)
    })
}

/// Per-triangle stiffness from Starch pins.
pub fn starch_weights(mesh: &Mesh, pins: &[Pin]) -> Vec<f64> {
    mesh.tris
        .iter()
        .map(|t| {
            let c = centroid(mesh, t);
            let mut w = 1.0;
            for p in pins.iter().filter(|p| p.kind == PinKind::Starch) {
                let d = ((c[0] - p.rest[0]).powi(2) + (c[1] - p.rest[1]).powi(2)).sqrt();
                let ext = p.extent.max(0.0) + 1e-9;
                let f = (1.0 - d / (ext * 2.0)).clamp(0.0, 1.0);
                w += f * p.amount.max(0.0) * 2.0;
            }
            w
        })
        .collect()
}

fn centroid(mesh: &Mesh, t: &[usize; 3]) -> [f64; 2] {
    let v = &mesh.verts;
    [(v[t[0]][0] + v[t[1]][0] + v[t[2]][0]) / 3.0, (v[t[0]][1] + v[t[1]][1] + v[t[2]][1]) / 3.0]
}

/// Solve the deformation of `mesh` for `pins` (layer space). Returns deformed vertices.
pub fn solve(mesh: &Mesh, pins: &[Pin], refine: usize) -> Vec<[f64; 2]> {
    let mut h = arap::Handles::default();
    let mut adj: Vec<Vec<usize>> = vec![vec![]; mesh.verts.len()];
    for t in &mesh.tris {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            if !adj[a].contains(&b) {
                adj[a].push(b);
            }
            if !adj[b].contains(&a) {
                adj[b].push(a);
            }
        }
    }
    let mut used = std::collections::HashSet::new();
    for p in pins {
        let Some(v) = nearest(&mesh.verts, p.rest) else { continue };
        let r = mesh.verts[v];
        if p.kind.moves() && used.insert(v) {
            h.fixed.push((v, [p.position[0] + r[0] - p.rest[0], p.position[1] + r[1] - p.rest[1]]));
        }
        if matches!(p.kind, PinKind::Advanced | PinKind::Bend) {
            let (s, c) = p.rotation.to_radians().sin_cos();
            for &j in &adj[v] {
                let e = [mesh.verts[j][0] - r[0], mesh.verts[j][1] - r[1]];
                let vec = [p.scale * (e[0] * c - e[1] * s), p.scale * (e[0] * s + e[1] * c)];
                h.edges.push(arap::EdgeTarget { i: v, j, vec, weight: 10.0 });
            }
        }
    }
    if h.fixed.is_empty() && h.edges.is_empty() {
        return mesh.verts.clone();
    }
    let w = starch_weights(mesh, pins);
    arap::deform(&mesh.verts, &mesh.tris, &w, &h, refine)
}

/// Per-triangle depth from Overlap pins (higher draws in front).
pub fn depths(mesh: &Mesh, pins: &[Pin]) -> Vec<f64> {
    mesh.tris
        .iter()
        .map(|t| {
            let c = centroid(mesh, t);
            let mut d: Option<f64> = None;
            for p in pins.iter().filter(|p| p.kind == PinKind::Overlap) {
                let dist = ((c[0] - p.rest[0]).powi(2) + (c[1] - p.rest[1]).powi(2)).sqrt();
                if dist <= p.extent.max(0.0) + 1e-9 {
                    d = Some(d.map_or(p.in_front, |x: f64| x.max(p.in_front)));
                }
            }
            d.unwrap_or(0.0)
        })
        .collect()
}

/// Map a point of the deformed mesh back to rest space (barycentric through the triangle that
/// contains it, or the nearest vertex's offset).
pub fn unmap(mesh: &Mesh, deformed: &[[f64; 2]], p: [f64; 2]) -> [f64; 2] {
    for t in &mesh.tris {
        let (a, b, c) = (deformed[t[0]], deformed[t[1]], deformed[t[2]]);
        if let Some(l) = bary(a, b, c, p)
            && l.iter().all(|x| *x >= -1e-9)
        {
            let v = &mesh.verts;
            return [l[0] * v[t[0]][0] + l[1] * v[t[1]][0] + l[2] * v[t[2]][0], l[0] * v[t[0]][1] + l[1] * v[t[1]][1] + l[2] * v[t[2]][1]];
        }
    }
    match nearest(deformed, p) {
        Some(i) => [p[0] - deformed[i][0] + mesh.verts[i][0], p[1] - deformed[i][1] + mesh.verts[i][1]],
        None => p,
    }
}

/// Whether `p` (layer space) is on the mesh as `deformed` places its vertices.
pub fn contains(mesh: &Mesh, deformed: &[[f64; 2]], p: [f64; 2]) -> bool {
    mesh.tris.iter().any(|t| match (deformed.get(t[0]), deformed.get(t[1]), deformed.get(t[2])) {
        (Some(a), Some(b), Some(c)) => bary(*a, *b, *c, p).is_some_and(|l| l.iter().all(|x| *x >= -1e-9)),
        _ => false,
    })
}

#[inline]
fn bary(a: [f64; 2], b: [f64; 2], c: [f64; 2], p: [f64; 2]) -> Option<[f64; 3]> {
    let d = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
    if d.abs() < 1e-12 {
        return None;
    }
    let l0 = ((b[1] - c[1]) * (p[0] - c[0]) + (c[0] - b[0]) * (p[1] - c[1])) / d;
    let l1 = ((c[1] - a[1]) * (p[0] - c[0]) + (a[0] - c[0]) * (p[1] - c[1])) / d;
    Some([l0, l1, 1.0 - l0 - l1])
}

fn render(ctx: &EffectCtx, buf: Buf) -> Buf {
    let meshes = parse(ctx.params);
    if meshes.is_empty() {
        return buf;
    }
    let refine = (ctx.params.get("refinement").map(Value::as_f64).unwrap_or(20.0) / 10.0).round().clamp(0.0, 10.0) as usize;
    // Triangles of every mesh in pixel space: (rest px, deformed px, depth).
    let mut tris: Vec<([[f64; 2]; 3], [[f64; 2]; 3], f64)> = vec![];
    let px = |q: [f64; 2]| [q[0] * buf.scale + buf.offset[0], q[1] * buf.scale + buf.offset[1]];
    for m in &meshes {
        let mesh = mesh_for(&buf, m);
        if mesh.tris.is_empty() {
            continue;
        }
        let def = solve(&mesh, &m.pins, refine);
        let dep = depths(&mesh, &m.pins);
        for (k, t) in mesh.tris.iter().enumerate() {
            let r = [px(mesh.verts[t[0]]), px(mesh.verts[t[1]]), px(mesh.verts[t[2]])];
            let d = [px(def[t[0]]), px(def[t[1]]), px(def[t[2]])];
            tris.push((r, d, dep[k]));
        }
    }
    if tris.is_empty() {
        return buf;
    }
    // Stable sort: back to front.
    tris.sort_by(|a, b| a.2.total_cmp(&b.2));
    rasterize(&buf, &tris)
}

/// Texture-map `buf` through triangles (rest px → deformed px), back to front.
fn rasterize(buf: &Buf, tris: &[([[f64; 2]; 3], [[f64; 2]; 3], f64)]) -> Buf {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (_, d, _) in tris {
        for p in d {
            x0 = x0.min(p[0]);
            y0 = y0.min(p[1]);
            x1 = x1.max(p[0]);
            y1 = y1.max(p[1]);
        }
    }
    let ox = x0.floor() as i64 - 1;
    let oy = y0.floor() as i64 - 1;
    let w = ((x1.ceil() as i64 + 1 - ox).max(1) as u32).min(16384);
    let h = ((y1.ceil() as i64 + 1 - oy).max(1) as u32).min(16384);
    let mut out = Image::new(w, h);
    // Triangle y-ranges for row bucketing.
    let ranges: Vec<(f64, f64)> = tris.iter().map(|(_, d, _)| (d[0][1].min(d[1][1]).min(d[2][1]), d[0][1].max(d[1][1]).max(d[2][1]))).collect();
    const SUB: [[f64; 2]; 4] = [[0.25, 0.25], [0.75, 0.25], [0.25, 0.75], [0.75, 0.75]];
    out.rows_mut().for_each(|(row, line)| {
        let y = (oy + row as i64) as f64;
        let n = line.len();
        let mut cover = vec![0u8; n];
        let mut center: Vec<Option<usize>> = vec![None; n];
        let mut any: Vec<Option<usize>> = vec![None; n];
        for (k, (_, d, _)) in tris.iter().enumerate() {
            let (ya, yb) = ranges[k];
            if yb < y || ya > y + 1.0 {
                continue;
            }
            let xa = (d[0][0].min(d[1][0]).min(d[2][0]).floor() as i64 - ox).max(0) as usize;
            let xb = ((d[0][0].max(d[1][0]).max(d[2][0]).ceil() as i64 - ox + 1).max(0) as usize).min(n);
            for (i, ((cv, ce), an)) in cover.iter_mut().zip(center.iter_mut()).zip(any.iter_mut()).enumerate().take(xb).skip(xa) {
                let x = (ox + i as i64) as f64;
                for (s, o) in SUB.iter().enumerate() {
                    if let Some(l) = bary(d[0], d[1], d[2], [x + o[0], y + o[1]])
                        && l.iter().all(|v| *v >= -1e-9)
                    {
                        *cv |= 1 << s;
                        *an = Some(k);
                    }
                }
                if let Some(l) = bary(d[0], d[1], d[2], [x + 0.5, y + 0.5])
                    && l.iter().all(|v| *v >= -1e-9)
                {
                    *ce = Some(k);
                }
            }
        }
        for (i, px) in line.iter_mut().enumerate() {
            let c = cover[i].count_ones();
            let Some(k) = center[i].or(any[i]) else { continue };
            if c == 0 && center[i].is_none() {
                continue;
            }
            let (r, d, _) = &tris[k];
            let p = [(ox + i as i64) as f64 + 0.5, y + 0.5];
            let Some(l) = bary(d[0], d[1], d[2], p) else { continue };
            let src = [l[0] * r[0][0] + l[1] * r[1][0] + l[2] * r[2][0], l[0] * r[0][1] + l[1] * r[1][1] + l[2] * r[2][1]];
            let s = buf.img.sample_bilinear(src[0], src[1]);
            let cov = if center[i].is_some() && c == 4 { 1.0 } else { c.max(1) as f32 / 4.0 };
            *px = [s[0] * cov, s[1] * cov, s[2] * cov, s[3] * cov];
        }
    });
    Buf { img: out, offset: [buf.offset[0] - ox as f64, buf.offset[1] - oy as f64], scale: buf.scale }
}

/// Count of nodes in a group tree (for tests and readouts).
pub fn pin_count(fx: &PropGroup) -> usize {
    meshes(fx).map(|m| pins(m).count()).sum()
}

/// The deformed mesh for overlays: mesh, deformed vertices (layer space) per mesh group uid.
pub fn overlay(buf: &Buf, params: &Params) -> Vec<(u64, Arc<Mesh>, Vec<[f64; 2]>, Vec<Pin>)> {
    let refine = (params.get("refinement").map(Value::as_f64).unwrap_or(20.0) / 10.0).round().clamp(0.0, 10.0) as usize;
    parse(params)
        .into_iter()
        .map(|m| {
            let mesh = mesh_for(buf, &m);
            let def = solve(&mesh, &m.pins, refine);
            (m.uid, mesh, def, m.pins)
        })
        .collect()
}

#[cfg(test)]
mod tests;
