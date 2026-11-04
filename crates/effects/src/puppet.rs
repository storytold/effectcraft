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
//! ([`arap`]) moves the vertices, and the input is texture-mapped through the deformed mesh, split
//! finer along a smooth interpolation of the solve (bilinear sampling, averaged where the mesh
//! squeezes the image; 16 coverage samples per pixel for the outline; Overlap pins' In Front
//! deciding which triangles draw on top).

pub mod arap;
pub mod mesh;

use std::collections::HashMap;
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

/// Solver rounds for the Mesh Rotation Refinement parameter. The default of 20 converges to
/// about 1 % of the mesh's edge length on a far drag.
fn refinement_rounds(params: &Params) -> usize {
    (params.get("refinement").map(Value::as_f64).unwrap_or(20.0) * 2.0).round().clamp(0.0, 200.0) as usize
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
        if p.kind.moves() {
            // The pin holds the very point under it. A pin on a vertex holds that vertex, and one
            // off the mesh its nearest vertex at the same offset.
            match held_point(mesh, p.rest) {
                Some((verts, bary)) => h.points.push(arap::PointTarget { verts, bary, position: p.position }),
                None => {
                    if used.insert(v) {
                        h.fixed.push((v, [p.position[0] + r[0] - p.rest[0], p.position[1] + r[1] - p.rest[1]]));
                    }
                }
            }
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
    if h.fixed.is_empty() && h.points.is_empty() {
        // A Bend pin with nothing held turns the mesh about itself.
        let Some(e) = h.edges.first() else { return mesh.verts.clone() };
        h.fixed.push((e.i, mesh.verts[e.i]));
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

/// The triangle of the rest mesh over `p`, with `p`'s barycentric coordinates in it.
fn locate(mesh: &Mesh, p: [f64; 2]) -> Option<([usize; 3], [f64; 3])> {
    let v = &mesh.verts;
    mesh.tris.iter().find_map(|t| bary(v[t[0]], v[t[1]], v[t[2]], p).filter(|l| l.iter().all(|x| *x >= -1e-9)).map(|l| (*t, l)))
}

/// The point a moving pin at `rest` holds, as a triangle and barycentric coordinates. `None` when it
/// holds a vertex instead: it's on one, or off the mesh.
pub fn held_point(mesh: &Mesh, rest: [f64; 2]) -> Option<([usize; 3], [f64; 3])> {
    locate(mesh, rest).filter(|(_, bary)| bary.iter().all(|l| *l < 1.0 - 1e-6))
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
    let refine = refinement_rounds(ctx.params);
    // Triangles of every mesh in pixel space: (rest px, deformed px, draw order).
    let mut tris: Vec<([[f64; 2]; 3], [[f64; 2]; 3], (f64, f64))> = vec![];
    let px = |q: [f64; 2]| [q[0] * buf.scale + buf.offset[0], q[1] * buf.scale + buf.offset[1]];
    for m in &meshes {
        let mesh = mesh_for(&buf, m);
        if mesh.tris.is_empty() {
            continue;
        }
        let def = solve(&mesh, &m.pins, refine);
        let dep = depths(&mesh, &m.pins);
        // Sheets of one mesh that come to overlap stack by how far they were moved, so the sheet
        // dragged over the rest stays whole instead of interleaving by triangle number.
        let moved: Vec<f64> = mesh
            .tris
            .iter()
            .map(|t| {
                let (c, e) = (centroid(&mesh, t), [(def[t[0]][0] + def[t[1]][0] + def[t[2]][0]) / 3.0, (def[t[0]][1] + def[t[1]][1] + def[t[2]][1]) / 3.0]);
                (c[0] - e[0]).hypot(c[1] - e[1])
            })
            .collect();
        let smooth = smooth_mesh(&mesh, &def, buf.scale);
        for (t, k) in smooth.tris.iter().zip(&smooth.parent) {
            let r = [px(smooth.rest[t[0]]), px(smooth.rest[t[1]]), px(smooth.rest[t[2]])];
            let d = [px(smooth.deformed[t[0]]), px(smooth.deformed[t[1]]), px(smooth.deformed[t[2]])];
            tris.push((r, d, (dep[*k], moved[*k])));
        }
    }
    if tris.is_empty() {
        return buf;
    }
    // Stable sort: back to front.
    tris.sort_by(|a, b| a.2.0.total_cmp(&b.2.0).then(a.2.1.total_cmp(&b.2.1)));
    let tris: Vec<_> = tris.into_iter().map(|(r, d, _)| (r, d)).collect();
    rasterize(&buf, &tris)
}

/// The deformed mesh tessellated for drawing (see [`smooth_mesh`]). `parent` is the triangle of the
/// solver's mesh each small triangle came from.
#[derive(Clone, Debug, PartialEq)]
pub struct SmoothMesh {
    pub rest: Vec<[f64; 2]>,
    pub deformed: Vec<[f64; 2]>,
    pub tris: Vec<[usize; 3]>,
    pub parent: Vec<usize>,
}

/// Longest the tessellated curve may stray from the true one, in buffer pixels.
const SMOOTH_TOLERANCE: f64 = 0.125;
/// Most small triangles a triangle of the mesh splits along an edge, and in all for one mesh.
const SMOOTH_MAX_SPLIT: usize = 16;
const SMOOTH_MAX_TRIS: usize = 100_000;

/// Each vertex's deformation gradient: the area-weighted mean of its triangles'.
fn vertex_gradients(mesh: &Mesh, def: &[[f64; 2]]) -> Vec<[f64; 4]> {
    let mut sum = vec![([0.0; 4], 0.0); mesh.verts.len()];
    for t in &mesh.tris {
        let v = &mesh.verts;
        let (e1, e2) = ([v[t[1]][0] - v[t[0]][0], v[t[1]][1] - v[t[0]][1]], [v[t[2]][0] - v[t[0]][0], v[t[2]][1] - v[t[0]][1]]);
        let (f1, f2) = ([def[t[1]][0] - def[t[0]][0], def[t[1]][1] - def[t[0]][1]], [def[t[2]][0] - def[t[0]][0], def[t[2]][1] - def[t[0]][1]]);
        let det = e1[0] * e2[1] - e2[0] * e1[1];
        if det.abs() < 1e-12 {
            continue;
        }
        let g = [
            (f1[0] * e2[1] - f2[0] * e1[1]) / det,
            (f2[0] * e1[0] - f1[0] * e2[0]) / det,
            (f1[1] * e2[1] - f2[1] * e1[1]) / det,
            (f2[1] * e1[0] - f1[1] * e2[0]) / det,
        ];
        let area = det.abs() * 0.5;
        for i in t {
            for (a, b) in sum[*i].0.iter_mut().zip(g) {
                *a += area * b;
            }
            sum[*i].1 += area;
        }
    }
    sum.into_iter().map(|(g, a)| if a > 0.0 { g.map(|x| x / a) } else { [1.0, 0.0, 0.0, 1.0] }).collect()
}

/// Smooth the piecewise-linear solve for drawing: Phong deformation (James, "Phong Deformation: A
/// Better C0 Interpolation for Embedded Deformation", SIGGRAPH 2020). A point at barycentric `l`
/// goes to `Σ l_i (d_i + F_i (q − r_i))` instead of `Σ l_i d_i`, with `F_i` the gradients at the
/// vertices. It agrees at the vertices and reproduces any affine map, so pins and rigid parts
/// stay exact; between them, straight lines in the image bend along a curve where a linear map
/// creases at every mesh edge. The map is quadratic, and the same along an edge from both sides,
/// so a uniform split of every triangle follows it within [`SMOOTH_TOLERANCE`] with no cracks.
pub fn smooth_mesh(mesh: &Mesh, def: &[[f64; 2]], scale: f64) -> SmoothMesh {
    let n = mesh.tris.len();
    let plain = || SmoothMesh { rest: mesh.verts.clone(), deformed: def.to_vec(), tris: mesh.tris.clone(), parent: (0..n).collect() };
    if def.len() != mesh.verts.len() || n == 0 {
        return plain();
    }
    let grads = vertex_gradients(mesh, def);
    let v = &mesh.verts;
    let phong = |t: &[usize; 3], l: [f64; 3]| {
        let q = [l[0] * v[t[0]][0] + l[1] * v[t[1]][0] + l[2] * v[t[2]][0], l[0] * v[t[0]][1] + l[1] * v[t[1]][1] + l[2] * v[t[2]][1]];
        let mut p = [0.0, 0.0];
        for (k, w) in l.iter().enumerate() {
            let (i, g) = (t[k], grads[t[k]]);
            let e = [q[0] - v[i][0], q[1] - v[i][1]];
            p[0] += w * (def[i][0] + g[0] * e[0] + g[1] * e[1]);
            p[1] += w * (def[i][1] + g[2] * e[0] + g[3] * e[1]);
        }
        (q, p)
    };
    // The bow of the curve over the chord, largest over the mesh, sets the split.
    let mut bow: f64 = 0.0;
    for t in &mesh.tris {
        for l in [[0.5, 0.5, 0.0], [0.0, 0.5, 0.5], [0.5, 0.0, 0.5], [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0]] {
            let (_, p) = phong(t, l);
            let chord = [l[0] * def[t[0]][0] + l[1] * def[t[1]][0] + l[2] * def[t[2]][0], l[0] * def[t[0]][1] + l[1] * def[t[1]][1] + l[2] * def[t[2]][1]];
            bow = bow.max((p[0] - chord[0]).hypot(p[1] - chord[1]));
        }
    }
    let budget = ((SMOOTH_MAX_TRIS / n) as f64).sqrt() as usize;
    let k = ((bow * scale / SMOOTH_TOLERANCE).sqrt().ceil() as usize).clamp(1, SMOOTH_MAX_SPLIT.min(budget.max(1)));
    if k <= 1 {
        return plain();
    }
    let mut rest = mesh.verts.clone();
    let mut deformed = def.to_vec();
    let mut tris = Vec::with_capacity(n * k * k);
    let mut parent = Vec::with_capacity(n * k * k);
    // Points on an edge, shared with the triangle across it: (low vertex, high vertex, step from low).
    let mut on_edge: HashMap<(usize, usize, usize), usize> = HashMap::with_capacity(n * k);
    let mut grid = vec![0usize; (k + 1) * (k + 1)];
    for (pk, t) in mesh.tris.iter().enumerate() {
        for i in 0..=k {
            for j in 0..=k - i {
                // Weights of corners 0, 1, 2 in k-ths.
                let c = [k - i - j, i, j];
                let id = if let Some(corner) = c.iter().position(|x| *x == k) {
                    t[corner]
                } else if let Some(zero) = c.iter().position(|x| *x == 0) {
                    let (a, b) = ((zero + 1) % 3, (zero + 2) % 3);
                    let (lo, hi, step) = if t[a] < t[b] { (a, b, c[b]) } else { (b, a, c[a]) };
                    *on_edge.entry((t[lo], t[hi], step)).or_insert_with(|| {
                        let mut l = [0.0; 3];
                        l[hi] = step as f64 / k as f64;
                        l[lo] = 1.0 - l[hi];
                        let (q, p) = phong(t, l);
                        rest.push(q);
                        deformed.push(p);
                        rest.len() - 1
                    })
                } else {
                    let (q, p) = phong(t, c.map(|x| x as f64 / k as f64));
                    rest.push(q);
                    deformed.push(p);
                    rest.len() - 1
                };
                grid[i * (k + 1) + j] = id;
            }
        }
        for i in 0..k {
            for j in 0..k - i {
                let at = |a: usize, b: usize| grid[a * (k + 1) + b];
                tris.push([at(i, j), at(i + 1, j), at(i, j + 1)]);
                parent.push(pk);
                if i + j + 1 < k {
                    tris.push([at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)]);
                    parent.push(pk);
                }
            }
        }
    }
    SmoothMesh { rest, deformed, tris, parent }
}

/// A triangle ready to rasterize: its edge functions (`a x + b y + c >= 0` inside), the affine map
/// from output pixels back to the source and how many samples per axis that map needs.
struct RasterTri {
    edges: [[f64; 3]; 3],
    /// Source `x = m[0] X + m[1] Y + m[2]`, `y = m[3] X + m[4] Y + m[5]`.
    map: [f64; 6],
    samples: u8,
    bounds: [f64; 4],
}

impl RasterTri {
    fn new(r: &[[f64; 2]; 3], d: &[[f64; 2]; 3]) -> Option<RasterTri> {
        let area = (d[1][0] - d[0][0]) * (d[2][1] - d[0][1]) - (d[1][1] - d[0][1]) * (d[2][0] - d[0][0]);
        if !area.is_finite() || area.abs() < 1e-9 {
            return None;
        }
        let sign = area.signum();
        let edge = |p: [f64; 2], q: [f64; 2]| {
            let (a, b) = (sign * (p[1] - q[1]), sign * (q[0] - p[0]));
            [a, b, -(a * p[0] + b * p[1])]
        };
        // The barycentric gradients give the affine map directly.
        let e = [edge(d[1], d[2]), edge(d[2], d[0]), edge(d[0], d[1])];
        let inv = 1.0 / area.abs();
        let at = |c: usize, k: usize| inv * (e[0][k] * r[0][c] + e[1][k] * r[1][c] + e[2][k] * r[2][c]);
        let map = [at(0, 0), at(0, 1), at(0, 2), at(1, 0), at(1, 1), at(1, 2)];
        // Largest source distance one output pixel covers: past it, one sample would skip texels.
        let (p, q) = ((map[0] * map[0] + map[3] * map[3]).sqrt(), (map[1] * map[1] + map[4] * map[4]).sqrt());
        let samples = match p.max(q) {
            t if t <= 1.25 => 1,
            t if t <= 2.5 => 2,
            _ => 4,
        };
        let bounds =
            [d[0][0].min(d[1][0]).min(d[2][0]), d[0][1].min(d[1][1]).min(d[2][1]), d[0][0].max(d[1][0]).max(d[2][0]), d[0][1].max(d[1][1]).max(d[2][1])];
        Some(RasterTri { edges: e, map, samples, bounds })
    }

    #[inline]
    fn source(&self, x: f64, y: f64) -> [f64; 2] {
        let m = &self.map;
        [m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5]]
    }

    /// Coverage of the pixel at (x, y): bit `i + 4 j` is the sample at `((i + 0.5) / 4, (j + 0.5) / 4)`.
    #[inline]
    fn coverage(&self, x: f64, y: f64) -> u16 {
        let mut all = true;
        for [a, b, c] in &self.edges {
            let v = a * x + b * y + c;
            if v + a.max(0.0) + b.max(0.0) < -1e-9 {
                return 0;
            }
            all &= v + a.min(0.0) + b.min(0.0) >= -1e-9;
        }
        if all {
            return u16::MAX;
        }
        let mut mask = 0;
        for j in 0..4 {
            for i in 0..4 {
                let (sx, sy) = (x + (i as f64 + 0.5) / 4.0, y + (j as f64 + 0.5) / 4.0);
                if self.edges.iter().all(|[a, b, c]| a * sx + b * sy + c >= -1e-9) {
                    mask |= 1 << (i + 4 * j);
                }
            }
        }
        mask
    }
}

/// No triangle over this sample.
const NO_TRI: u32 = u32::MAX;

/// Texture-map `buf` through triangles (rest px → deformed px), back to front. Each pixel has 16
/// coverage samples remembering their top triangle, so the outline gets a smooth ramp and
/// triangles that share an edge leave no seam; a pixel inside the mesh reads the source once
/// unless the triangle under it is minified or two sheets of the mesh meet there, where it
/// averages 4 or 16 reads.
fn rasterize(buf: &Buf, tris: &[([[f64; 2]; 3], [[f64; 2]; 3])]) -> Buf {
    let tris: Vec<RasterTri> = tris.iter().filter_map(|(r, d)| RasterTri::new(r, d)).collect();
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for t in &tris {
        x0 = x0.min(t.bounds[0]);
        y0 = y0.min(t.bounds[1]);
        x1 = x1.max(t.bounds[2]);
        y1 = y1.max(t.bounds[3]);
    }
    if tris.is_empty() {
        return Buf { img: Image::new(1, 1), offset: buf.offset, scale: buf.scale };
    }
    let ox = x0.floor() as i64 - 1;
    let oy = y0.floor() as i64 - 1;
    let w = (x1.ceil() as i64).saturating_add(1).saturating_sub(ox).clamp(1, 16384) as u32;
    let h = (y1.ceil() as i64).saturating_add(1).saturating_sub(oy).clamp(1, 16384) as u32;
    let mut out = Image::new(w, h);
    // The triangles over each row, in draw order.
    let row_range = |t: &RasterTri| {
        let a = ((t.bounds[1].floor() as i64 - oy).max(0) as usize).min(h as usize);
        let b = ((t.bounds[3].floor() as i64 - oy + 1).max(0) as usize).min(h as usize);
        (a, b)
    };
    let mut starts = vec![0usize; h as usize + 1];
    for t in &tris {
        let (a, b) = row_range(t);
        for n in &mut starts[a..b] {
            *n += 1;
        }
    }
    let mut total = 0;
    for n in starts.iter_mut() {
        let c = *n;
        *n = total;
        total += c;
    }
    let mut bucket = vec![0u32; total];
    let mut fill = starts.clone();
    for (k, t) in tris.iter().enumerate() {
        let (a, b) = row_range(t);
        for f in &mut fill[a..b] {
            bucket[*f] = k as u32;
            *f += 1;
        }
    }
    out.rows_mut().for_each(|(row, line)| {
        let y = (oy + row as i64) as f64;
        let n = line.len();
        let mut owners = vec![NO_TRI; n * 16];
        let mut touched = vec![false; n];
        for &k in &bucket[starts[row]..starts[row + 1]] {
            let t = &tris[k as usize];
            let xa = ((t.bounds[0].floor() as i64 - ox).max(0) as usize).min(n);
            let xb = ((t.bounds[2].floor() as i64 - ox + 1).max(0) as usize).min(n);
            for i in xa..xb {
                let mask = t.coverage((ox + i as i64) as f64, y);
                if mask == 0 {
                    continue;
                }
                touched[i] = true;
                let px = &mut owners[i * 16..(i + 1) * 16];
                if mask == u16::MAX {
                    px.fill(k);
                } else {
                    for (s, o) in px.iter_mut().enumerate() {
                        if mask & (1 << s) != 0 {
                            *o = k;
                        }
                    }
                }
            }
        }
        for (i, px) in line.iter_mut().enumerate() {
            if touched[i] {
                *px = resolve(buf, &tris, &owners[i * 16..(i + 1) * 16], (ox + i as i64) as f64, y);
            }
        }
    });
    Buf { img: out, offset: [buf.offset[0] - ox as f64, buf.offset[1] - oy as f64], scale: buf.scale }
}

/// The colour of the pixel at (x, y) from its 16 samples' triangles: premultiplied source reads,
/// weighted by the samples they stand for, so uncovered samples count as transparent.
fn resolve(buf: &Buf, tris: &[RasterTri], owners: &[u32], x: f64, y: f64) -> [f32; 4] {
    // A sample by the pixel's centre decides the read count for pixels inside the mesh.
    let center = [5usize, 6, 9, 10].into_iter().find_map(|s| tris.get(*owners.get(s)? as usize));
    let full = owners.iter().all(|o| *o != NO_TRI);
    let mut grid = 2;
    if full && let Some(c) = center {
        grid = c.samples as usize;
        let first = owners[0];
        if grid == 1 && owners.iter().any(|o| *o != first) {
            // Neighbouring triangles agree on the source along their shared edge: only sheets
            // of the mesh that overlap here disagree by more than a pixel or so.
            let sc = c.source(x + 0.5, y + 0.5);
            let apart = [0usize, 3, 12, 15].into_iter().any(|s| {
                tris.get(owners[s] as usize).is_some_and(|t| {
                    let q = t.source(x + 0.5, y + 0.5);
                    (q[0] - sc[0]).abs().max((q[1] - sc[1]).abs()) > 2.0
                })
            });
            if apart {
                grid = 2;
            }
        }
        if grid == 1 {
            let src = c.source(x + 0.5, y + 0.5);
            return buf.img.sample_bilinear(src[0], src[1]);
        }
    }
    let grid = grid.max(2);
    let cell = 4 / grid;
    let mut sum = [0.0f32; 4];
    for gy in 0..grid {
        for gx in 0..grid {
            // Covered samples in this cell, read at the first of them.
            let mut count = 0.0f32;
            let mut read = None;
            for sy in 0..cell {
                for sx in 0..cell {
                    let (i, j) = (gx * cell + sx, gy * cell + sy);
                    let o = owners[i + 4 * j];
                    if o != NO_TRI {
                        count += 1.0;
                        if read.is_none() {
                            read = Some((o, i, j));
                        }
                    }
                }
            }
            let Some((o, i, j)) = read else { continue };
            let Some(t) = tris.get(o as usize) else { continue };
            // A fully covered cell reads its centre; a partial one, a sample known to be inside.
            let (fx, fy) = if count as usize == cell * cell {
                (gx as f64 + 0.5, gy as f64 + 0.5)
            } else {
                ((i as f64 + 0.5) * grid as f64 / 4.0, (j as f64 + 0.5) * grid as f64 / 4.0)
            };
            let src = t.source(x + fx / grid as f64, y + fy / grid as f64);
            let c = buf.img.sample_bilinear(src[0], src[1]);
            for (a, b) in sum.iter_mut().zip(c) {
                *a += b * count / 16.0;
            }
        }
    }
    sum
}

/// Count of nodes in a group tree (for tests and readouts).
pub fn pin_count(fx: &PropGroup) -> usize {
    meshes(fx).map(|m| pins(m).count()).sum()
}

/// The deformed mesh for overlays: mesh, deformed vertices (layer space) per mesh group uid.
pub fn overlay(buf: &Buf, params: &Params) -> Vec<(u64, Arc<Mesh>, Vec<[f64; 2]>, Vec<Pin>)> {
    let refine = refinement_rounds(params);
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
