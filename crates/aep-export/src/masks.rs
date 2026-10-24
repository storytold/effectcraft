//! Layer masks: the plan (file units, losses reported) and the chunks, cloned from the mask
//! prototypes of `template/masks.aep` (our own fixtures 091, 093 and 208).
//!
//! What is written, each with its verified layout (`docs/aep-format.md` 3.4, 3.5 and 3.8):
//! - per mask a `mkif` (inverted at byte 0, mode `u32` at 4, the layer's running mask counter at
//!   8; the other bytes are copied from the prototype) and an atom with the user name;
//! - the path as `LIST shap` (`shph`: open bit `0x08` of byte 3, the box of vertices and handles
//!   normalised to the layer source size; `list`: three `f32` points per vertex, vertex `i`, out
//!   handle `i`, in handle `i + 1`, normalised inside the box);
//! - path keys as 64-byte records (time, interpolation, label, the rest copied from the prototype)
//!   with one `shap` per key;
//! - Feather (static or keyed, plain template with two components), Opacity (fraction) and
//!   Expansion (pixels), each only when it is not at its default.

use effectcraft_aep::Chunk;
use effectcraft_keyframe::{Ease, Interp, ShapePath, Value};
use effectcraft_project::{FeatherFalloff, GroupKind, Layer, MaskMode, MaskMotionBlur, PropGroup};

use crate::plan::{CompPlan, EaseSlot, KeyPlan, LIMIT, cut_name};
use crate::report::{Collector, LossCategory};
use crate::tree::{R, child_data, child_mut, kids_mut, list_mut, put_f32, put_f64, put_i32, put_u8, put_u16, put_u32, tpl};

/// The most vertices one mask path may have (as the importer).
pub(crate) const MAX_VERTICES: usize = 65_536;

/// One path shape in the file's units.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ShapePlan {
    /// Left, top, right, bottom as fractions of the layer source size.
    pub bbox: [f32; 4],
    pub open: bool,
    /// Three points per vertex, normalised inside the box.
    pub points: Vec<[f32; 2]>,
}

/// One path keyframe.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PathKeyPlan {
    pub time: i32,
    /// 1 linear, 2 bezier, 3 hold.
    pub in_interp: u8,
    pub out_interp: u8,
    /// The ease of a bezier side: speed (as stored) and influence (a fraction), at bytes 24..55
    /// as in the spatial template (599, 700).
    pub in_ease: (f64, f64),
    pub out_ease: (f64, f64),
    pub label: u8,
    pub shape: ShapePlan,
}

/// A mask in the file's units.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MaskPlan {
    pub name: String,
    /// `mkif` mode id: None 0, Add 1, Subtract 2, Intersect 3, Lighten 4, Darken 5, Difference 6.
    pub mode: u32,
    pub inverted: bool,
    /// The static path (the first key's when the path is keyed).
    pub shape: ShapePlan,
    pub path_keys: Vec<PathKeyPlan>,
    /// Pixels; `None` at the default `[0, 0]`.
    pub feather: Option<[f64; 2]>,
    pub feather_keys: Vec<KeyPlan>,
    /// A fraction; `None` at the default 1.
    pub opacity: Option<f64>,
    /// Pixels; `None` at the default 0.
    pub expansion: Option<f64>,
}

fn mode_id(m: MaskMode) -> u32 {
    match m {
        MaskMode::None => 0,
        MaskMode::Add => 1,
        MaskMode::Subtract => 2,
        MaskMode::Intersect => 3,
        MaskMode::Lighten => 4,
        MaskMode::Darken => 5,
        MaskMode::Difference => 6,
    }
}

fn finite(v: f64) -> Option<f64> {
    (v.is_finite() && v.abs() <= LIMIT).then_some(v)
}

/// A project path (layer pixels) in the file's units for a layer source of `size` pixels.
pub(crate) fn shape_plan(p: &ShapePath, size: (u32, u32)) -> Result<ShapePlan, &'static str> {
    let n = p.vertices.len();
    if n == 0 {
        return Err("its path has no vertices");
    }
    if n > MAX_VERTICES {
        return Err("its path has more than 65 536 vertices");
    }
    let (w, h) = (f64::from(size.0.max(1)), f64::from(size.1.max(1)));
    let at = |i: usize| -> Option<([f64; 2], [f64; 2], [f64; 2])> {
        let v = *p.vertices.get(i)?;
        let t_in = p.in_tangents.get(i).copied().unwrap_or([0.0; 2]);
        let t_out = p.out_tangents.get(i).copied().unwrap_or([0.0; 2]);
        let add = |a: [f64; 2], b: [f64; 2]| [a[0] + b[0], a[1] + b[1]];
        Some((v, add(v, t_in), add(v, t_out)))
    };
    // Absolute points in layer pixels: vertex i, out handle i, in handle i + 1.
    let mut abs: Vec<[f64; 2]> = Vec::with_capacity(n.saturating_mul(3));
    for i in 0..n {
        let (v, _, out) = at(i).ok_or("its path is incomplete")?;
        let (_, next_in, _) = at((i + 1) % n).ok_or("its path is incomplete")?;
        abs.extend([v, out, next_in]);
    }
    if !abs.iter().flatten().all(|c| finite(*c).is_some()) {
        return Err("its path holds a number that is not valid");
    }
    // Normalised to the layer size, then the box of all points.
    let norm: Vec<[f64; 2]> = abs.iter().map(|[x, y]| [x / w, y / h]).collect();
    let (mut left, mut top, mut right, mut bottom) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for [x, y] in &norm {
        left = left.min(*x);
        right = right.max(*x);
        top = top.min(*y);
        bottom = bottom.max(*y);
    }
    let bbox = [left as f32, top as f32, right as f32, bottom as f32];
    let [l, t, r, b] = bbox.map(f64::from);
    // A box without width or height (all points on one line) stores 0 for that coordinate.
    let unit = |v: f64, lo: f64, hi: f64| if hi - lo > 0.0 { ((v - lo) / (hi - lo)) as f32 } else { 0.0 };
    let points = norm.iter().map(|[x, y]| [unit(*x, l, r), unit(*y, t, b)]).collect();
    Ok(ShapePlan { bbox, open: !p.closed, points })
}

/// Plans the masks of a layer whose source is `size` pixels.
pub(crate) fn plan_masks(layer: &Layer, size: (u32, u32), cp: &CompPlan, comp_name: &str, col: &mut Collector) -> Vec<MaskPlan> {
    let Some(masks) = layer.masks() else { return Vec::new() };
    let mut out = Vec::new();
    for g in masks.groups() {
        let what = format!("{comp_name}: '{}' {}", layer.name, g.name);
        if let Some(m) = plan_mask(g, size, cp, &what, col) {
            col.exported.masks = col.exported.masks.saturating_add(1);
            out.push(m);
        }
    }
    out
}

fn plan_mask(g: &PropGroup, size: (u32, u32), cp: &CompPlan, what: &str, col: &mut Collector) -> Option<MaskPlan> {
    let GroupKind::Mask { mode, inverted, locked, motion_blur, feather_falloff, roto_bezier, .. } = g.kind else {
        col.lose(LossCategory::Masks, format!("{what}: not a mask, not exported"));
        return None;
    };
    if !g.enabled {
        col.lose(LossCategory::Masks, format!("{what}: a disabled mask is not exported"));
        return None;
    }
    let Some(path_prop) = g.get("path") else {
        col.lose(LossCategory::Masks, format!("{what}: no path, not exported"));
        return None;
    };
    let Value::Path(static_path) = &path_prop.value else {
        col.lose(LossCategory::Masks, format!("{what}: the path is not a path, not exported"));
        return None;
    };
    // Switches the file layout does not hold (not decoded).
    if locked {
        col.lose(LossCategory::Masks, format!("{what}: the lock is not written"));
    }
    if motion_blur != MaskMotionBlur::SameAsLayer {
        col.lose(LossCategory::Masks, format!("{what}: mask motion blur is written as Same As Layer"));
    }
    if feather_falloff != FeatherFalloff::Smooth {
        col.lose(LossCategory::Masks, format!("{what}: Linear feather falloff is written as Smooth"));
    }
    if roto_bezier {
        col.lose(LossCategory::Masks, format!("{what}: RotoBezier is not written (the tangents are)"));
    }
    for id in ["path", "feather", "opacity", "expansion"] {
        if g.get(id).is_some_and(|p| p.has_expression()) {
            col.lose(LossCategory::Masks, format!("{what}: the expression on Mask {id} is not written"));
        }
    }
    // The path and its keys.
    let mut path_keys: Vec<PathKeyPlan> = Vec::new();
    let mut keys: Vec<(i64, &effectcraft_keyframe::Keyframe)> = Vec::new();
    for k in &path_prop.keys {
        let (u, inexact) = cp.tb.units(k.time);
        if inexact {
            col.lose(LossCategory::RoundedTimes, format!("{what}: path key at {:.4} s moved to the time base", k.time.seconds()));
        }
        keys.push((u, k));
    }
    keys.sort_by_key(|(u, _)| *u);
    let before = keys.len();
    keys.dedup_by_key(|(u, _)| *u);
    if keys.len() != before {
        col.lose(LossCategory::ApproximatedKeyframes, format!("{what}: path keys at the same time were merged"));
    }
    let side = |i: Interp| -> u8 {
        match i {
            Interp::Linear => 1,
            Interp::Hold => 3,
            Interp::Bezier => 2,
        }
    };
    let ease = |e: &[effectcraft_keyframe::Ease], on: bool| -> (f64, f64) {
        if !on {
            return (0.0, 0.0);
        }
        let e = e.first().copied().unwrap_or(effectcraft_keyframe::Ease { speed: 0.0, influence: 1.0 / 3.0 });
        let speed = finite(e.speed).filter(|v| v.abs() <= 1e9).unwrap_or(0.0);
        let influence = if e.influence.is_finite() { e.influence.clamp(0.001, 1.0) } else { 1.0 / 3.0 };
        (speed, influence)
    };
    let mut feather_points = !static_path.feather.is_empty();
    for (u, k) in &keys {
        let Ok(time) = i32::try_from(*u) else {
            col.lose(LossCategory::InvalidValues, format!("{what}: path key time out of range, key dropped"));
            continue;
        };
        let Value::Path(p) = &k.value else {
            col.lose(LossCategory::InvalidValues, format!("{what}: a path key without a path was dropped"));
            continue;
        };
        feather_points |= !p.feather.is_empty();
        let shape = match shape_plan(p, size) {
            Ok(s) => s,
            Err(why) => {
                col.lose(LossCategory::InvalidValues, format!("{what}: a path key was dropped, {why}"));
                continue;
            }
        };
        if k.roving {
            col.lose(LossCategory::ApproximatedKeyframes, format!("{what}: a roving path key is written as a normal key"));
        }
        let (in_interp, out_interp) = (side(k.in_interp), side(k.out_interp));
        if (in_interp == 2 && k.in_ease.first().is_some_and(|e| e.speed != 0.0)) || (out_interp == 2 && k.out_ease.first().is_some_and(|e| e.speed != 0.0)) {
            col.lose(LossCategory::ApproximatedKeyframes, format!("{what}: a path key ease speed other than 0 is written as stored (its unit is not sampled)"));
        }
        let (in_ease, out_ease) = (ease(&k.in_ease, in_interp == 2), ease(&k.out_ease, out_interp == 2));
        path_keys.push(PathKeyPlan { time, in_interp, out_interp, in_ease, out_ease, label: k.label.min(16), shape });
        col.exported.keyframes = col.exported.keyframes.saturating_add(1);
    }
    if feather_points {
        col.lose(LossCategory::Masks, format!("{what}: variable-width feather points are not written"));
    }
    let shape = match path_keys.first() {
        Some(k) => k.shape.clone(),
        None => match shape_plan(static_path, size) {
            Ok(s) => s,
            Err(why) => {
                col.lose(LossCategory::Masks, format!("{what}: not exported, {why}"));
                return None;
            }
        },
    };
    // Feather, opacity, expansion.
    let feather_prop = g.get("feather");
    let feather_value = match feather_prop.map(|p| &p.value) {
        Some(Value::Vec2(v)) => *v,
        _ => [0.0, 0.0],
    };
    let feather = match (finite(feather_value[0]), finite(feather_value[1])) {
        (Some(x), Some(y)) if x >= 0.0 && y >= 0.0 => ([x, y] != [0.0, 0.0]).then_some([x, y]),
        _ => {
            col.lose(LossCategory::InvalidValues, format!("{what}: feather {feather_value:?} is not valid, the default was written"));
            None
        }
    };
    let feather_keys = feather_prop.map_or_else(Vec::new, |p| plan_feather_keys(&p.keys, cp, what, col));
    let scalar = |id: &str, default: f64, lo: f64, hi: f64, col: &mut Collector| -> Option<f64> {
        let p = g.get(id)?;
        if !p.keys.is_empty() {
            col.lose(LossCategory::Masks, format!("{what}: keys on Mask {id} are not written (no keyed prototype), the static value is"));
        }
        let Value::Scalar(v) = p.value else { return None };
        match finite(v) {
            Some(v) if (lo..=hi).contains(&v) => ((v - default).abs() > 1e-12).then_some(v),
            _ => {
                col.lose(LossCategory::InvalidValues, format!("{what}: Mask {id} {v} is not valid, the default was written"));
                None
            }
        }
    };
    let opacity = scalar("opacity", 100.0, 0.0, 100.0, col).map(|v| v / 100.0);
    let expansion = scalar("expansion", 0.0, -LIMIT, LIMIT, col);
    let name = cut_name(&g.name, 255, what, col);
    Some(MaskPlan { name, mode: mode_id(mode), inverted, shape, path_keys, feather, feather_keys, opacity, expansion })
}

/// Feather keys (plain template, two components, speed in pixels per second).
fn plan_feather_keys(src: &[effectcraft_keyframe::Keyframe], cp: &CompPlan, what: &str, col: &mut Collector) -> Vec<KeyPlan> {
    let mut keys: Vec<(i64, &effectcraft_keyframe::Keyframe)> = Vec::with_capacity(src.len());
    for k in src {
        let (u, inexact) = cp.tb.units(k.time);
        if inexact {
            col.lose(LossCategory::RoundedTimes, format!("{what}: feather key at {:.4} s moved to the time base", k.time.seconds()));
        }
        keys.push((u, k));
    }
    keys.sort_by_key(|(u, _)| *u);
    let before = keys.len();
    keys.dedup_by_key(|(u, _)| *u);
    if keys.len() != before {
        col.lose(LossCategory::ApproximatedKeyframes, format!("{what}: feather keys at the same time were merged"));
    }
    let mut out = Vec::with_capacity(keys.len());
    for (u, k) in &keys {
        let Ok(time) = i32::try_from(*u) else {
            col.lose(LossCategory::InvalidValues, format!("{what}: feather key time out of range, key dropped"));
            continue;
        };
        let value = match &k.value {
            Value::Vec2([x, y]) => match (finite(*x), finite(*y)) {
                (Some(x), Some(y)) if x >= 0.0 && y >= 0.0 => vec![x, y],
                _ => {
                    col.lose(LossCategory::InvalidValues, format!("{what}: a feather key that is not valid was dropped"));
                    continue;
                }
            },
            _ => {
                col.lose(LossCategory::InvalidValues, format!("{what}: a feather key that is not a pair was dropped"));
                continue;
            }
        };
        let code = |i: Interp| match i {
            Interp::Linear => 1u8,
            Interp::Bezier => 2,
            Interp::Hold => 3,
        };
        let (in_interp, out_interp) = (code(k.in_interp), code(k.out_interp));
        let ease = |eases: &[Ease], i: usize, on: bool| -> EaseSlot {
            if !on {
                return EaseSlot::default();
            }
            let e = eases.get(i).or_else(|| eases.last()).copied().unwrap_or(Ease { speed: 0.0, influence: 1.0 / 3.0 });
            EaseSlot { speed: finite(e.speed).unwrap_or(0.0), influence: if e.influence.is_finite() { e.influence.clamp(0.001, 1.0) } else { 1.0 / 3.0 } }
        };
        out.push(KeyPlan {
            time,
            value,
            in_interp,
            out_interp,
            in_ease: (0..2).map(|i| ease(&k.in_ease, i, in_interp == 2)).collect(),
            out_ease: (0..2).map(|i| ease(&k.out_ease, i, out_interp == 2)).collect(),
            spatial_in: [0.0; 3],
            spatial_out: [0.0; 3],
            spatial_auto: false,
            roving: false,
            label: k.label.min(16),
        });
        if k.roving {
            col.lose(LossCategory::ApproximatedKeyframes, format!("{what}: a roving feather key is written as a normal key"));
        }
        col.exported.keyframes = col.exported.keyframes.saturating_add(1);
    }
    out
}

// ---------------------------------------------------------------- chunks

/// The mask prototypes.
pub(crate) struct MaskProtos {
    /// `tdmn ADBE Mask Parade`.
    pub parade_name: Chunk<'static>,
    /// The parade group children before its first match name (`tdsb`, `tdsn`).
    pub parade_header: Vec<Chunk<'static>>,
    /// `tdmn ADBE Mask Atom`, `mkif`, the atom `tdgp` (with static Feather, Opacity, Offset).
    pub atom_name: Chunk<'static>,
    pub mkif: Chunk<'static>,
    pub atom: Chunk<'static>,
    /// `tdmn ADBE Group End`.
    pub group_end: Chunk<'static>,
    /// The keyed path property (`tdbs`, two 64-byte records).
    pub keyed_path: Chunk<'static>,
    /// The keyed Mask Feather property (`tdbs`, two 88-byte records).
    pub keyed_feather: Chunk<'static>,
}

pub(crate) fn tdmn_name(c: &Chunk<'_>) -> Option<String> {
    let d = c.bytes()?;
    Some(d.iter().take_while(|b| **b != 0).map(|b| char::from(*b)).collect())
}

fn is_name(c: &Chunk<'_>, name: &str) -> bool {
    c.is(b"tdmn") && tdmn_name(c).as_deref() == Some(name)
}

impl MaskProtos {
    pub(crate) fn from_file(protos: &effectcraft_aep::AepFile<'static>) -> R<MaskProtos> {
        let mpar = protos.list(b"mpar").ok_or_else(|| tpl("mask prototypes without a parade"))?.children();
        let parade_name = mpar.first().filter(|c| is_name(c, "ADBE Mask Parade")).ok_or_else(|| tpl("mask parade name missing"))?.clone();
        let parade = mpar.get(1).filter(|c| c.is_list(b"tdgp")).ok_or_else(|| tpl("mask parade missing"))?.children();
        let first = parade.iter().position(|c| c.is(b"tdmn")).ok_or_else(|| tpl("empty mask parade"))?;
        let parade_header = parade.get(..first).ok_or_else(|| tpl("mask parade header"))?.to_vec();
        let atom_name = parade.get(first).filter(|c| is_name(c, "ADBE Mask Atom")).ok_or_else(|| tpl("mask atom name missing"))?.clone();
        let mkif = parade.get(first + 1).filter(|c| c.is(b"mkif")).ok_or_else(|| tpl("mkif missing"))?.clone();
        let atom = parade.get(first + 2).filter(|c| c.is_list(b"tdgp")).ok_or_else(|| tpl("mask atom missing"))?.clone();
        let group_end = parade.iter().rev().find(|c| is_name(c, "ADBE Group End")).ok_or_else(|| tpl("mask parade end missing"))?.clone();
        let one = |ty: &[u8; 4]| -> R<Chunk<'static>> {
            protos.list(ty).and_then(|l| l.children().first()).filter(|c| c.is_list(b"tdbs")).cloned().ok_or_else(|| tpl("keyed mask prototype missing"))
        };
        Ok(MaskProtos { parade_name, parade_header, atom_name, mkif, atom, group_end, keyed_path: one(b"mpth")?, keyed_feather: one(b"mfea")? })
    }
}

/// `tdmn` + `ADBE Mask Parade` group for the masks of a layer (source `size` pixels).
pub(crate) fn build_parade(t: &MaskProtos, masks: &[MaskPlan], size: (u32, u32)) -> R<[Chunk<'static>; 2]> {
    let mut kids = t.parade_header.clone();
    for (i, m) in masks.iter().enumerate() {
        let mut mkif = t.mkif.clone();
        {
            let d = crate::tree::data_mut(&mut mkif)?;
            put_u8(d, 0, u8::from(m.inverted))?;
            put_u32(d, 4, m.mode)?;
            put_u32(d, 8, u32::try_from(i.saturating_add(1)).map_err(|_| tpl("too many masks"))?)?;
        }
        kids.push(t.atom_name.clone());
        kids.push(mkif);
        kids.push(build_atom(t, m, size)?);
    }
    kids.push(t.group_end.clone());
    Ok([t.parade_name.clone(), Chunk::list(*b"tdgp", kids)])
}

fn build_atom(t: &MaskProtos, m: &MaskPlan, size: (u32, u32)) -> R<Chunk<'static>> {
    let mut atom = t.atom.clone();
    {
        let tdsn = child_mut(&mut atom, b"tdsn")?;
        crate::tree::set_utf8(tdsn, &m.name)?;
    }
    let kids = kids_mut(&mut atom)?;
    let first = kids.iter().position(|c| c.is(b"tdmn")).ok_or_else(|| tpl("empty mask atom"))?;
    let header: Vec<Chunk<'static>> = kids.drain(..first).collect();
    let mut pairs: Vec<(String, [Chunk<'static>; 2])> = Vec::new();
    let mut end: Option<Chunk<'static>> = None;
    let mut it = std::mem::take(kids).into_iter();
    while let Some(c) = it.next() {
        let name = tdmn_name(&c).unwrap_or_default();
        if name == "ADBE Group End" {
            end = Some(c);
            continue;
        }
        let node = it.next().ok_or_else(|| tpl("mask match name without a node"))?;
        pairs.push((name, [c, node]));
    }
    let end = end.ok_or_else(|| tpl("mask atom without its end"))?;
    let mut out = header;
    for (name, [tdmn, mut node]) in pairs {
        let keep = match name.as_str() {
            "ADBE Mask Shape" => {
                build_shape(t, &mut node, m, size)?;
                true
            }
            "ADBE Mask Feather" => {
                if !m.feather_keys.is_empty() {
                    node = keyed_feather(t, &m.feather_keys)?;
                    true
                } else if let Some([x, y]) = m.feather {
                    set_cdat(&mut node, &[x, y])?;
                    true
                } else {
                    false
                }
            }
            "ADBE Mask Opacity" => match m.opacity {
                Some(v) => {
                    set_cdat(&mut node, &[v])?;
                    true
                }
                None => false,
            },
            "ADBE Mask Offset" => match m.expansion {
                Some(v) => {
                    set_cdat(&mut node, &[v])?;
                    true
                }
                None => false,
            },
            _ => true,
        };
        if keep {
            out.push(tdmn);
            out.push(node);
        }
    }
    out.push(end);
    *kids_mut(&mut atom)? = out;
    Ok(atom)
}

fn set_cdat(tdbs: &mut Chunk<'static>, values: &[f64]) -> R<()> {
    let cdat = child_data(tdbs, b"cdat")?;
    cdat.iter_mut().for_each(|b| *b = 0);
    for (i, v) in values.iter().enumerate() {
        put_f64(cdat, i * 8, *v)?;
    }
    Ok(())
}

/// Rewrites a key list (`lhd3` count words and `ldat`).
fn set_key_list(tdbs: &mut Chunk<'static>, records: Vec<u8>, count: usize, size: usize) -> R<()> {
    let list = list_mut(tdbs, b"list")?;
    let n = u32::try_from(count).map_err(|_| tpl("too many keys"))?;
    {
        let lhd3 = child_data(list, b"lhd3")?;
        put_u32(lhd3, 8, n)?;
        put_u32(lhd3, 12, n.div_ceil(4))?;
        put_u32(lhd3, 16, u32::try_from(size).map_err(|_| tpl("record size"))?)?;
        put_u32(lhd3, 28, n.div_ceil(4).saturating_mul(4))?;
    }
    let ld = kids_mut(list)?;
    match ld.iter_mut().find(|c| c.is(b"ldat")) {
        Some(c) => *c = Chunk::data(*b"ldat", records),
        None => ld.push(Chunk::data(*b"ldat", records)),
    }
    Ok(())
}

fn keyed_feather(t: &MaskProtos, keys: &[KeyPlan]) -> R<Chunk<'static>> {
    let mut tdbs = t.keyed_feather.clone();
    // The plain template with two components (208): 8 + 40 * 2 bytes.
    let size = 88;
    let mut ldat = Vec::with_capacity(size * keys.len());
    for k in keys {
        ldat.extend(crate::emit::plain_record(2, k, size)?);
    }
    set_key_list(&mut tdbs, ldat, keys.len(), size)?;
    Ok(tdbs)
}

/// The `om-s` of the path: the property (static or keyed) and one `shap` per shape.
fn build_shape(t: &MaskProtos, oms: &mut Chunk<'static>, m: &MaskPlan, size: (u32, u32)) -> R<()> {
    let aspect = f64::from(size.0.max(1)) / f64::from(size.1.max(1));
    let shapes: Vec<&ShapePlan> = if m.path_keys.is_empty() { vec![&m.shape] } else { m.path_keys.iter().map(|k| &k.shape).collect() };
    let kids = kids_mut(oms)?;
    // The shape prototype (one static `shap`).
    let omks = kids.iter_mut().find(|c| c.is_list(b"omks")).ok_or_else(|| tpl("mask path without omks"))?;
    let proto = kids_mut(omks)?.iter().find(|c| c.is_list(b"shap")).cloned().ok_or_else(|| tpl("mask path without a shape"))?;
    let mut list = Vec::with_capacity(shapes.len());
    for s in &shapes {
        list.push(build_shap(&proto, s)?);
    }
    *kids_mut(omks)? = list;
    let tdbs = kids.iter_mut().find(|c| c.is_list(b"tdbs")).ok_or_else(|| tpl("mask path without its property"))?;
    if !m.path_keys.is_empty() {
        let mut keyed = t.keyed_path.clone();
        // The two prototype records (093). Only the common fields are decoded; the rest of the 64
        // bytes is copied: the first key from the first record, every later key from the second
        // (093 differs there: an `f64` 1 at byte 16 of the first record, 0 in the second).
        let protos: Vec<Vec<u8>> = {
            let list = keyed.child_list(b"list").ok_or_else(|| tpl("keyed path without keys"))?;
            let ldat = list.child(b"ldat").and_then(|c| c.bytes()).ok_or_else(|| tpl("keyed path without records"))?;
            ldat.as_chunks::<64>().0.iter().map(|r| r.to_vec()).collect()
        };
        let (first_rec, later_rec) = match protos.as_slice() {
            [a, b, ..] => (a.clone(), b.clone()),
            _ => return Err(tpl("keyed path prototype without two records")),
        };
        let mut ldat = Vec::with_capacity(64 * m.path_keys.len());
        for (i, k) in m.path_keys.iter().enumerate() {
            let mut r = if i == 0 { first_rec.clone() } else { later_rec.clone() };
            let flags = u16::from_be_bytes([r.get(6).copied().unwrap_or(0), r.get(7).copied().unwrap_or(1)]) & !0x03e0;
            put_i32(&mut r, 0, k.time)?;
            put_u8(&mut r, 4, k.in_interp)?;
            put_u8(&mut r, 5, k.out_interp)?;
            put_u16(&mut r, 6, flags | (u16::from(k.label) << 6))?;
            // The ease of bezier sides at 24..55 (599, 700); linear and hold sides keep the
            // prototype's zeros.
            if k.in_interp == 2 {
                put_f64(&mut r, 24, k.in_ease.0)?;
                put_f64(&mut r, 32, k.in_ease.1)?;
            }
            if k.out_interp == 2 {
                put_f64(&mut r, 40, k.out_ease.0)?;
                put_f64(&mut r, 48, k.out_ease.1)?;
            }
            ldat.extend(r);
        }
        set_key_list(&mut keyed, ldat, m.path_keys.len(), 64)?;
        *tdbs = keyed;
    }
    // The width / height of the layer source, as for the Anchor Point (090, 093: 16 / 9 on a
    // 1920 x 1080 solid; 309: 1 on a 100 x 100 solid).
    put_f64(child_data(tdbs, b"tdb4")?, 24, aspect)?;
    Ok(())
}

pub(crate) fn build_shap(proto: &Chunk<'static>, s: &ShapePlan) -> R<Chunk<'static>> {
    let mut shap = proto.clone();
    {
        let shph = child_data(&mut shap, b"shph")?;
        crate::tree::put_bits(shph, 3, 0x08, s.open)?;
        for (i, v) in s.bbox.iter().enumerate() {
            put_f32(shph, 4 + 4 * i, *v)?;
        }
    }
    let list = list_mut(&mut shap, b"list")?;
    let records = s.points.len();
    let vertices = u32::try_from(records / 3).map_err(|_| tpl("too many vertices"))?;
    {
        // Point lists: word 2 the record count, words 3 and 7 the vertex count and four times it
        // (090: 12 records, 4 and 16; 233: 9 records, 3 and 12), word 4 the record size 8.
        let lhd3 = child_data(list, b"lhd3")?;
        put_u32(lhd3, 8, u32::try_from(records).map_err(|_| tpl("too many points"))?)?;
        put_u32(lhd3, 12, vertices)?;
        put_u32(lhd3, 16, 8)?;
        put_u32(lhd3, 28, vertices.saturating_mul(4))?;
    }
    let mut ldat = vec![0u8; records.saturating_mul(8)];
    for (i, [u, v]) in s.points.iter().enumerate() {
        put_f32(&mut ldat, i * 8, *u)?;
        put_f32(&mut ldat, i * 8 + 4, *v)?;
    }
    let ld = kids_mut(list)?;
    match ld.iter_mut().find(|c| c.is(b"ldat")) {
        Some(c) => *c = Chunk::data(*b"ldat", ldat),
        None => ld.push(Chunk::data(*b"ldat", ldat)),
    }
    Ok(shap)
}
