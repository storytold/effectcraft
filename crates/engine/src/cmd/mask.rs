//! Masks drawn and edited in the viewer: Pen tool (new mask, add vertex, Bezier tangents, close
//! path), vertex selection, moving and deleting vertices, and Layer ▸ Mask ▸ Remove (All).
//!
//! Coordinates are in layer space. Edits of an animated Mask Path set a key at the current time.

use effectcraft_keyframe::{ShapePath, Value as KV};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Layer, MaskMode, PropGroup, Uid};
use serde_json::{Value, json};

use super::{CommandSpec, b_p, bad, has_comp, has_layers, layer_mut, layer_p, merge_p, resolve_layer, str_p};
use crate::{EngineError, Result, Session, VertexRef, cmd};

fn pt(v: Option<&Value>) -> Option<[f64; 2]> {
    let a = v?.as_array()?;
    Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?])
}

fn pts(v: Option<&Value>) -> Vec<[f64; 2]> {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(|p| pt(Some(p))).collect()).unwrap_or_default()
}

/// Index of a mask group by uid, 1-based index or name.
fn mask_index(masks: &PropGroup, key: &Value) -> Option<usize> {
    match key {
        Value::Number(n) => {
            let n = n.as_u64()?;
            masks.children.iter().position(|c| c.uid() == n).or_else(|| (n as usize).checked_sub(1).filter(|i| *i < masks.children.len()))
        }
        Value::String(s) => masks.children.iter().position(|c| c.name() == s),
        _ => None,
    }
}

/// Find a mask group by uid, 1-based index or name.
fn find_mask<'a>(masks: &'a mut PropGroup, key: &Value) -> Option<&'a mut PropGroup> {
    let i = mask_index(masks, key)?;
    masks.children.get_mut(i)?.as_group_mut()
}

/// Whether a group holds an editable Bezier path (a mask, or a shape layer's Path item).
pub(crate) fn is_path_group(g: &PropGroup) -> bool {
    matches!(g.get("path").map(|p| &p.value), Some(KV::Path(_))) && (matches!(g.kind, effectcraft_project::GroupKind::Mask { .. }) || g.match_id == "path")
}

/// A mask (uid, 1-based index or name) or, by uid, a shape layer's Path item.
pub(crate) fn path_group<'a>(props: &'a mut PropGroup, key: &Value) -> Option<&'a mut PropGroup> {
    if let Some(i) = props.sub("masks").and_then(|m| mask_index(m, key)) {
        return props.sub_mut("masks")?.children.get_mut(i)?.as_group_mut();
    }
    let uid = key.as_u64()?;
    props.find_group_mut(uid).filter(|g| is_path_group(g))
}

/// Edit the mask path of (layer, mask) at the current time.
fn edit_path<T>(s: &mut Session, p: &Value, label: &str, f: impl FnOnce(&mut ShapePath) -> Result<T>) -> Result<(Uid, T)> {
    edit_path_counted(s, p, label, |sp| f(sp).map(|r| (r, None)))
}

/// A change of a path's vertex count, repeated on the other keyframes of an animated path when
/// Settings ▸ General ▸ Preserve Constant Vertex and Feather Point Count is on.
#[derive(Clone, Debug)]
pub(crate) enum CountOp {
    /// A vertex was inserted at this index.
    Insert(usize),
    /// These vertices were removed.
    Remove(Vec<usize>),
}

/// Apply a vertex-count change to another keyframe's path: an inserted vertex splits the
/// matching segment in half (the shape doesn't change), removed vertices go.
pub(crate) fn apply_count_op(sp: &mut ShapePath, op: &CountOp) {
    match op {
        CountOp::Insert(at) => {
            let n = sp.vertices.len();
            if n == 0 {
                return;
            }
            if *at > 0 && split_segment(sp, at - 1, 0.5).is_some() {
                return;
            }
            let at = (*at).min(n);
            let v = sp.vertices[at.min(n - 1)];
            sp.in_tangents.resize(n, [0.0; 2]);
            sp.out_tangents.resize(n, [0.0; 2]);
            sp.vertices.insert(at, v);
            sp.in_tangents.insert(at, [0.0; 2]);
            sp.out_tangents.insert(at, [0.0; 2]);
        }
        CountOp::Remove(idx) => {
            let mut idx = idx.clone();
            idx.sort_unstable();
            idx.dedup();
            for &i in idx.iter().rev() {
                if i < sp.vertices.len() {
                    sp.vertices.remove(i);
                    if i < sp.in_tangents.len() {
                        sp.in_tangents.remove(i);
                    }
                    if i < sp.out_tangents.len() {
                        sp.out_tangents.remove(i);
                    }
                }
            }
            if sp.vertices.len() < 3 {
                sp.closed = false;
            }
        }
    }
}

/// Repeat `op` on every keyframe of `pr` other than the one at `lt`.
fn sync_keys(pr: &mut effectcraft_project::Property, lt: effectcraft_time::Tick, op: &CountOp) {
    for k in pr.keys.iter_mut().filter(|k| k.time != lt) {
        if let KV::Path(sp) = &mut k.value {
            apply_count_op(sp, op);
        }
    }
}

/// [`edit_path`] whose edit may change the vertex count (`f` says how).
fn edit_path_counted<T>(s: &mut Session, p: &Value, label: &str, f: impl FnOnce(&mut ShapePath) -> Result<(T, Option<CountOp>)>) -> Result<(Uid, T)> {
    let (cid, lid) = layer_p(s, p, label)?;
    let key = p.get("mask").cloned().ok_or_else(|| bad(label, "missing `mask` (uid, index or name)"))?;
    let t = s.time();
    let preserve = s.prefs.general.preserve_constant_vertex_count;
    s.edit(label, merge_p(p), |proj, _| {
        let l = layer_mut(proj, cid, lid)?;
        let lt = l.layer_time(t);
        let g = path_group(&mut l.props, &key).ok_or_else(|| bad(label, "no such mask or shape path"))?;
        let uid = g.uid;
        let roto = super::paths::is_roto(g);
        let pr = g.get_mut("path").ok_or_else(|| bad(label, "mask has no path"))?;
        let KV::Path(mut sp) = pr.value_at(lt) else { return Err(bad(label, "not a path")) };
        // A path read from a file may have fewer tangents than vertices; the edits below index
        // all three arrays by vertex (#1250).
        sp.fit_tangents();
        let (r, op) = f(&mut sp)?;
        if roto {
            super::paths::roto_smooth(&mut sp);
        }
        pr.set_value_at(lt, KV::Path(sp));
        if let Some(op) = op.filter(|_| preserve) {
            sync_keys(pr, lt, &op);
        }
        Ok((uid, r))
    })
}

/// Pen tool: a new (open by default) mask from points.
fn new_mask(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid) = layer_p(s, p, "mask.new")?;
    let v = pts(p.get("vertices"));
    if v.is_empty() {
        return Err(bad("mask.new", "need `vertices` [[x,y], …] in layer space"));
    }
    let n = v.len();
    let mut ins = pts(p.get("inTangents"));
    let mut outs = pts(p.get("outTangents"));
    ins.resize(n, [0.0; 2]);
    outs.resize(n, [0.0; 2]);
    let path = ShapePath { vertices: v, in_tangents: ins, out_tangents: outs, closed: b_p(p, "closed").unwrap_or(false), feather: Vec::new() };
    let mode = str_p(p, "mode").and_then(MaskMode::from_name).unwrap_or(MaskMode::Add);
    let cycle = s.prefs.appearance.cycle_mask_colors;
    let uid = s.edit("New Mask", None, |proj, st| {
        let mut next = proj.next_id;
        let l = layer_mut(proj, cid, lid)?;
        let masks = l.props.sub_mut("masks").ok_or_else(|| bad("mask.new", "this layer can't have masks"))?;
        let k = masks.children.len();
        let g = build::mask(&mut Ids(&mut next), &format!("Mask {}", k + 1), path, mode, crate::prefs::mask_color(cycle, k));
        let uid = g.uid;
        masks.children.push(g.into());
        proj.next_id = next;
        st.selected_vertices = vec![VertexRef { layer: lid, mask: uid, index: n - 1 }];
        Ok(uid)
    })?;
    s.events.push(crate::Event::Frontend {
        command: "timeline.revealMask".into(),
        params: json!({"comp": cid.0, "layer": lid.0, "mask": uid, "projectEpoch": s.script_ui.project_epoch}),
    });
    Ok(json!({"mask": uid}))
}

fn add_vertex(s: &mut Session, p: &Value) -> Result<Value> {
    let point = pt(p.get("point")).ok_or_else(|| bad("mask.addVertex", "missing `point` [x,y]"))?;
    let tin = pt(p.get("in")).unwrap_or([0.0; 2]);
    let tout = pt(p.get("out")).unwrap_or([0.0; 2]);
    let at = p.get("index").and_then(Value::as_u64).map(|i| i as usize);
    let (_, lid) = layer_p(s, p, "mask.addVertex")?;
    let (uid, i) = edit_path_counted(s, p, "mask.addVertex", |sp| {
        let i = at.unwrap_or(sp.vertices.len()).min(sp.vertices.len());
        sp.vertices.insert(i, point);
        sp.in_tangents.insert(i, tin);
        sp.out_tangents.insert(i, tout);
        sp.feather_after_insert(i, None);
        Ok((i, Some(CountOp::Insert(i))))
    })?;
    s.state.selected_vertices = vec![VertexRef { layer: lid, mask: uid, index: i }];
    Ok(json!(i))
}

fn set_vertex(s: &mut Session, p: &Value) -> Result<Value> {
    let i = p.get("index").and_then(Value::as_u64).ok_or_else(|| bad("mask.setVertex", "missing `index`"))? as usize;
    let (point, tin, tout) = (pt(p.get("point")), pt(p.get("in")), pt(p.get("out")));
    edit_path(s, p, "mask.setVertex", |sp| {
        if i >= sp.vertices.len() {
            return Err(bad("mask.setVertex", "no such vertex"));
        }
        if let Some(v) = point {
            sp.vertices[i] = v;
        }
        if let Some(v) = tin {
            sp.in_tangents[i] = v;
        }
        if let Some(v) = tout {
            sp.out_tangents[i] = v;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

fn set_closed(s: &mut Session, p: &Value) -> Result<Value> {
    let c = b_p(p, "closed").unwrap_or(true);
    edit_path(s, p, "mask.setClosed", |sp| {
        sp.closed = c;
        Ok(())
    })?;
    Ok(json!(c))
}

/// Rebuild a rectangular mask or shape path with corner radius `radius` (layer pixels; 0 gives
/// square corners). The rectangle is the path's bounding box. Only a plain rectangle (four
/// corner vertices, straight sides) or a rounded one (eight vertices, straight sides alternating
/// with circular corners) is reshaped; anything else (an ellipse, a star) is refused. The new path
/// keeps the old one's start vertex and direction, so the feather points and the other keys of an
/// animated path still line up; a change between square and rounded corners changes the vertex
/// count, which is refused when the path is keyed or has feather points.
fn set_roundness(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "mask.setRoundness";
    let radius = p.get("roundness").and_then(Value::as_f64).filter(|r| r.is_finite() && *r >= 0.0);
    let radius = radius.ok_or_else(|| bad(c, "need `roundness`: a corner radius in pixels, 0 or more"))?;
    let (cid, lid) = layer_p(s, p, c)?;
    let key = p.get("mask").cloned().ok_or_else(|| bad(c, "missing `mask` (uid, index or name)"))?;
    let t = s.time();
    s.edit(c, merge_p(p), |proj, _| {
        let l = layer_mut(proj, cid, lid)?;
        let lt = l.layer_time(t);
        let g = path_group(&mut l.props, &key).ok_or_else(|| bad(c, "no such mask or shape path"))?;
        let roto = super::paths::is_roto(g);
        let pr = g.get_mut("path").ok_or_else(|| bad(c, "mask has no path"))?;
        let KV::Path(mut sp) = pr.value_at(lt) else { return Err(bad(c, "not a path")) };
        sp.fit_tangents();
        let (lo, hi) = rect_box(&sp).ok_or_else(|| bad(c, "the path is not a closed rectangle or rounded rectangle"))?;
        let size = [hi[0] - lo[0], hi[1] - lo[1]];
        let bez = effectcraft_path::rect(size, [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0], radius);
        let mut new = effectcraft_path::from_kurbo(&bez).into_iter().next().ok_or_else(|| bad(c, "could not rebuild the path"))?;
        if new.vertices.len() != sp.vertices.len() {
            if !pr.keys.is_empty() {
                return Err(bad(c, "the path is keyed: changing between square and rounded corners would change its vertex count"));
            }
            if !sp.feather.is_empty() {
                return Err(bad(c, "the path has feather points: changing between square and rounded corners would change its vertex count"));
            }
        }
        align_like(&mut new, &sp);
        new.feather = std::mem::take(&mut sp.feather);
        if roto {
            super::paths::roto_smooth(&mut new);
        }
        pr.set_value_at(lt, KV::Path(new));
        Ok(())
    })?;
    Ok(Value::Null)
}

/// The bounding box of `sp` when it is a closed plain rectangle (four corner vertices, straight
/// sides) or rounded rectangle (eight vertices on the box's edges, straight sides along the edges
/// alternating with circular corners whose two ends are as far apart across as down).
fn rect_box(sp: &ShapePath) -> Option<([f64; 2], [f64; 2])> {
    let first = *sp.vertices.first()?;
    let (mut lo, mut hi) = (first, first);
    for v in &sp.vertices {
        lo = [lo[0].min(v[0]), lo[1].min(v[1])];
        hi = [hi[0].max(v[0]), hi[1].max(v[1])];
    }
    let n = sp.vertices.len();
    if !sp.closed || !(n == 4 || n == 8) || hi[0] <= lo[0] || hi[1] <= lo[1] {
        return None;
    }
    let eps = 1e-9 * (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1.0);
    let x_edge = |v: [f64; 2]| v[0] == lo[0] || v[0] == hi[0];
    let y_edge = |v: [f64; 2]| v[1] == lo[1] || v[1] == hi[1];
    let zero = |t: Option<&[f64; 2]>| t.is_none_or(|t| *t == [0.0, 0.0]);
    let seg = |i: usize| (sp.vertices[i], sp.vertices[(i + 1) % n]);
    let straight = |i: usize| {
        let (a, b) = seg(i);
        zero(sp.out_tangents.get(i)) && zero(sp.in_tangents.get((i + 1) % n)) && ((a[0] == b[0] && x_edge(a)) || (a[1] == b[1] && y_edge(a)))
    };
    let corner = |i: usize| {
        let (a, b) = seg(i);
        let (dx, dy) = ((b[0] - a[0]).abs(), (b[1] - a[1]).abs());
        dx > eps && (dx - dy).abs() <= eps.max(dx * 1e-9) && ((x_edge(a) && y_edge(b)) || (y_edge(a) && x_edge(b)))
    };
    let ok = if n == 4 {
        sp.vertices.iter().all(|v| x_edge(*v) && y_edge(*v)) && (0..n).all(straight)
    } else {
        sp.vertices.iter().all(|v| x_edge(*v) || y_edge(*v)) && (0..2).any(|k| (0..n).all(|i| if i % 2 == k { straight(i) } else { corner(i) }))
    };
    ok.then_some((lo, hi))
}

/// Turn the rebuilt closed path `new` so it runs the same way round as `old` and starts at the
/// vertex nearest `old`'s first one, keeping vertex correspondence for keys and feather points.
fn align_like(new: &mut ShapePath, old: &ShapePath) {
    let area = |sp: &ShapePath| {
        let n = sp.vertices.len();
        (0..n)
            .map(|i| {
                let (a, b) = (sp.vertices[i], sp.vertices[(i + 1) % n]);
                a[0] * b[1] - b[0] * a[1]
            })
            .sum::<f64>()
    };
    if (area(new) > 0.0) != (area(old) > 0.0) {
        // Reverse: vertex order flips and each vertex's in and out tangents swap; keep vertex 0.
        new.vertices.reverse();
        std::mem::swap(&mut new.in_tangents, &mut new.out_tangents);
        new.in_tangents.reverse();
        new.out_tangents.reverse();
        let n = new.vertices.len();
        if n > 0 {
            new.vertices.rotate_right(1);
            new.in_tangents.rotate_right(1);
            new.out_tangents.rotate_right(1);
        }
    }
    let Some(o) = old.vertices.first() else { return };
    let d = |v: &[f64; 2]| (v[0] - o[0]).powi(2) + (v[1] - o[1]).powi(2);
    let start = new.vertices.iter().enumerate().min_by(|a, b| d(a.1).total_cmp(&d(b.1))).map_or(0, |(i, _)| i);
    new.vertices.rotate_left(start);
    new.in_tangents.rotate_left(start);
    new.out_tangents.rotate_left(start);
}

/// Selected (or given) vertices grouped per (layer, mask).
fn vertex_groups(s: &Session, p: &Value) -> Result<std::collections::BTreeMap<(u64, Uid), Vec<usize>>> {
    let comp = s.active_comp().ok_or(EngineError::NoComp)?;
    let mut g: std::collections::BTreeMap<(u64, Uid), Vec<usize>> = Default::default();
    if let Some(Value::Array(a)) = p.get("vertices") {
        for v in a {
            let l = v.get("layer").and_then(|x| resolve_layer(comp, x)).ok_or_else(|| bad("mask", "vertex needs `layer`"))?;
            let m = v.get("mask").and_then(Value::as_u64).ok_or_else(|| bad("mask", "vertex needs `mask` uid"))?;
            let i = v.get("index").and_then(Value::as_u64).ok_or_else(|| bad("mask", "vertex needs `index`"))?;
            g.entry((l.0, m)).or_default().push(i as usize);
        }
    } else {
        for v in &s.state.selected_vertices {
            g.entry((v.layer.0, v.mask)).or_default().push(v.index);
        }
    }
    Ok(g)
}

fn select_vertices(s: &mut Session, p: &Value) -> Result<Value> {
    let g = vertex_groups(s, &json!({"vertices": p.get("vertices").cloned().unwrap_or(json!([]))}))?;
    let sel: Vec<VertexRef> =
        g.into_iter().flat_map(|((l, m), is)| is.into_iter().map(move |i| VertexRef { layer: effectcraft_project::LayerId(l), mask: m, index: i })).collect();
    if b_p(p, "add").unwrap_or(false) {
        for v in sel {
            if let Some(i) = s.state.selected_vertices.iter().position(|x| *x == v) {
                if b_p(p, "toggle").unwrap_or(false) {
                    s.state.selected_vertices.remove(i);
                }
            } else {
                s.state.selected_vertices.push(v);
            }
        }
    } else {
        s.state.selected_vertices = sel;
    }
    Ok(json!(s.state.selected_vertices.len()))
}

fn has_vertices(s: &Session) -> std::result::Result<(), String> {
    has_comp(s)?;
    if s.state.selected_vertices.is_empty() { Err("select mask vertices first".into()) } else { Ok(()) }
}

/// Apply `f` to the selected vertices' paths (each mask once).
fn edit_vertices(s: &mut Session, p: &Value, label: &str, f: impl Fn(&mut ShapePath, &[usize])) -> Result<Value> {
    edit_vertices_counted(s, p, label, false, f)
}

/// [`edit_vertices`]; `removes`: `f` deletes the given vertices (repeated on the other keyframes
/// when Preserve Constant Vertex Count is on).
fn edit_vertices_counted(s: &mut Session, p: &Value, label: &str, removes: bool, f: impl Fn(&mut ShapePath, &[usize])) -> Result<Value> {
    let cid = s.active_comp_id().ok_or(EngineError::NoComp)?;
    let groups = vertex_groups(s, p)?;
    let t = s.time();
    let preserve = removes && s.prefs.general.preserve_constant_vertex_count;
    s.edit(label, merge_p(p), |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        for ((l, m), idx) in &groups {
            let Some(layer) = comp.layer_mut(effectcraft_project::LayerId(*l)) else { continue };
            let lt = layer.layer_time(t);
            let Some(g) = layer.props.find_group_mut(*m).filter(|g| is_path_group(g)) else { continue };
            let roto = super::paths::is_roto(g);
            let Some(pr) = g.get_mut("path") else { continue };
            let KV::Path(mut sp) = pr.value_at(lt) else { continue };
            sp.fit_tangents();
            f(&mut sp, idx);
            if roto {
                super::paths::roto_smooth(&mut sp);
            }
            pr.set_value_at(lt, KV::Path(sp));
            if preserve {
                sync_keys(pr, lt, &CountOp::Remove(idx.clone()));
            }
        }
        Ok(())
    })?;
    Ok(json!(groups.values().map(Vec::len).sum::<usize>()))
}

fn move_vertices(s: &mut Session, p: &Value) -> Result<Value> {
    let d = pt(p.get("delta")).ok_or_else(|| bad("mask.moveVertices", "missing `delta` [dx,dy] (layer space)"))?;
    edit_vertices(s, p, "Move Mask Vertices", |sp, idx| {
        for &i in idx {
            if let Some(v) = sp.vertices.get_mut(i) {
                v[0] += d[0];
                v[1] += d[1];
            }
        }
    })
}

fn delete_vertices(s: &mut Session, p: &Value) -> Result<Value> {
    let n = edit_vertices_counted(s, p, "Delete Mask Vertices", true, |sp, idx| {
        let mut idx = idx.to_vec();
        idx.sort_unstable();
        idx.dedup();
        for &i in idx.iter().rev() {
            if i < sp.vertices.len() {
                sp.vertices.remove(i);
                sp.in_tangents.remove(i);
                sp.out_tangents.remove(i);
                sp.feather_after_remove(i);
            }
        }
        if sp.vertices.len() < 3 {
            sp.closed = false;
        }
    })?;
    s.state.selected_vertices.clear();
    Ok(n)
}

/// Convert Vertex tool: a corner vertex becomes smooth (tangents along its neighbours, a sixth
/// of their distance each way), a smooth one becomes a corner (no tangents). `smooth` forces one.
fn convert_vertex(s: &mut Session, p: &Value) -> Result<Value> {
    let i = p.get("index").and_then(Value::as_u64).ok_or_else(|| bad("mask.convertVertex", "missing `index`"))? as usize;
    let force = b_p(p, "smooth");
    let (_, smooth) = edit_path(s, p, "Convert Vertex", |sp| {
        let n = sp.vertices.len();
        if i >= n {
            return Err(bad("mask.convertVertex", "no such vertex"));
        }
        sp.in_tangents.resize(n, [0.0; 2]);
        sp.out_tangents.resize(n, [0.0; 2]);
        let is_smooth = sp.in_tangents[i] != [0.0; 2] || sp.out_tangents[i] != [0.0; 2];
        let make_smooth = force.unwrap_or(!is_smooth);
        if make_smooth {
            let prev = if i > 0 {
                Some(i - 1)
            } else if sp.closed {
                Some(n - 1)
            } else {
                None
            };
            let next = if i + 1 < n {
                Some(i + 1)
            } else if sp.closed {
                Some(0)
            } else {
                None
            };
            let v = sp.vertices[i];
            let a = prev.map(|j| sp.vertices[j]).unwrap_or(v);
            let b = next.map(|j| sp.vertices[j]).unwrap_or(v);
            let mut d = [(b[0] - a[0]) / 6.0, (b[1] - a[1]) / 6.0];
            if d == [0.0, 0.0] {
                d = [10.0, 0.0];
            }
            sp.out_tangents[i] = d;
            sp.in_tangents[i] = [-d[0], -d[1]];
        } else {
            sp.in_tangents[i] = [0.0; 2];
            sp.out_tangents[i] = [0.0; 2];
        }
        Ok(make_smooth)
    })?;
    Ok(json!({"smooth": smooth}))
}

/// Split cubic segment `i` (vertex i → i+1) at parameter `t`; the path keeps its shape.
pub(crate) fn split_segment(sp: &mut ShapePath, seg: usize, t: f64) -> Option<usize> {
    let n = sp.vertices.len();
    let j = if seg + 1 < n {
        seg + 1
    } else if sp.closed && seg + 1 == n {
        0
    } else {
        return None;
    };
    sp.in_tangents.resize(n, [0.0; 2]);
    sp.out_tangents.resize(n, [0.0; 2]);
    let p0 = sp.vertices[seg];
    let p3 = sp.vertices[j];
    let p1 = [p0[0] + sp.out_tangents[seg][0], p0[1] + sp.out_tangents[seg][1]];
    let p2 = [p3[0] + sp.in_tangents[j][0], p3[1] + sp.in_tangents[j][1]];
    let lerp = |a: [f64; 2], b: [f64; 2]| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
    let (a, b, c) = (lerp(p0, p1), lerp(p1, p2), lerp(p2, p3));
    let (d, e) = (lerp(a, b), lerp(b, c));
    let m = lerp(d, e);
    sp.out_tangents[seg] = [a[0] - p0[0], a[1] - p0[1]];
    sp.in_tangents[j] = [c[0] - p3[0], c[1] - p3[1]];
    let at = seg + 1;
    sp.vertices.insert(at, m);
    sp.in_tangents.insert(at, [d[0] - m[0], d[1] - m[1]]);
    sp.out_tangents.insert(at, [e[0] - m[0], e[1] - m[1]]);
    sp.feather_after_insert(at, Some(t));
    Some(at)
}

/// Nearest (segment, t, distance) of a path to a point (layer space), sampled per segment.
pub(crate) fn nearest_on_path(sp: &ShapePath, q: [f64; 2]) -> Option<(usize, f64, f64)> {
    let n = sp.vertices.len();
    let mut best: Option<(usize, f64, f64)> = None;
    for seg in 0..sp.segment_count() {
        let j = (seg + 1) % n;
        let p0 = sp.vertices[seg];
        let p3 = sp.vertices[j];
        let o = sp.out_tangents.get(seg).copied().unwrap_or([0.0; 2]);
        let i = sp.in_tangents.get(j).copied().unwrap_or([0.0; 2]);
        let (p1, p2) = ([p0[0] + o[0], p0[1] + o[1]], [p3[0] + i[0], p3[1] + i[1]]);
        let at = |t: f64| {
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            [a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0], a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1]]
        };
        let dist = |t: f64| {
            let p = at(t);
            (p[0] - q[0]).hypot(p[1] - q[1])
        };
        let (mut bt, mut bd) = (0.0, f64::INFINITY);
        for k in 0..=64 {
            let t = k as f64 / 64.0;
            let d = dist(t);
            if d < bd {
                (bt, bd) = (t, d);
            }
        }
        // Refine by bisection-style narrowing.
        let mut step = 1.0 / 64.0;
        for _ in 0..20 {
            step *= 0.5;
            for t in [bt - step, bt + step] {
                let t = t.clamp(0.0, 1.0);
                let d = dist(t);
                if d < bd {
                    (bt, bd) = (t, d);
                }
            }
        }
        if best.is_none_or(|b| bd < b.2) {
            best = Some((seg, bt, bd));
        }
    }
    best
}

/// Whether a layer-space point is inside a closed path.
fn inside_path(sp: &ShapePath, q: [f64; 2]) -> bool {
    sp.closed && {
        let cov = effectcraft_path::fill_coverage(
            &[effectcraft_path::to_kurbo(sp)],
            &effectcraft_geom::Mat3::translate(effectcraft_geom::vec2(-q[0] + 0.5, -q[1] + 0.5)),
            1,
            1,
            effectcraft_path::FillRule::NonZero,
        );
        cov.data[0] >= 0.5
    }
}

/// Point of segment `seg` at parameter `t`.
fn point_on(sp: &ShapePath, seg: usize, t: f64) -> Option<[f64; 2]> {
    let n = sp.vertices.len();
    if seg >= sp.segment_count() {
        return None;
    }
    let j = (seg + 1) % n;
    let (p0, p3) = (sp.vertices[seg], sp.vertices[j]);
    let o = sp.out_tangents.get(seg).copied().unwrap_or([0.0; 2]);
    let i = sp.in_tangents.get(j).copied().unwrap_or([0.0; 2]);
    let (p1, p2) = ([p0[0] + o[0], p0[1] + o[1]], [p3[0] + i[0], p3[1] + i[1]]);
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    Some([a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0], a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1]])
}

fn feather_index(p: &Value, sp: &ShapePath, c: &str) -> Result<usize> {
    let i = p.get("index").and_then(Value::as_u64).ok_or_else(|| bad(c, "missing `index` (feather point, 0-based)"))? as usize;
    if i >= sp.feather.len() {
        return Err(bad(c, format!("no feather point {i} (the mask has {})", sp.feather.len())));
    }
    Ok(i)
}

/// Mask Feather tool: add a feather point on the path, by `segment`/`t` or at the path position
/// nearest `point` (layer space). Without `radius`, a `point` off the path sets it from its
/// distance (outside the mask = outer feather, inside = inner).
fn feather_add(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "mask.featherPoint.add";
    let seg = p.get("segment").and_then(Value::as_u64).map(|v| v as usize);
    let t = super::f_p(p, "t").map(|v| v.clamp(0.0, 1.0));
    let q = pt(p.get("point"));
    let radius = super::f_p(p, "radius");
    let tension = super::f_p(p, "tension").unwrap_or(0.0).clamp(0.0, 100.0) / 100.0;
    let (uid, i) = edit_path(s, p, c, |sp| {
        let (seg, t, r) = match (seg, q) {
            (Some(seg), _) => {
                if seg >= sp.segment_count() {
                    return Err(bad(c, format!("no segment {seg}")));
                }
                (seg, t.unwrap_or(0.5), radius.unwrap_or(0.0))
            }
            (None, Some(q)) => {
                let (seg, t, d) = nearest_on_path(sp, q).ok_or_else(|| bad(c, "the path has no segments"))?;
                (seg, t, radius.unwrap_or(if inside_path(sp, q) { -d } else { d }))
            }
            (None, None) => return Err(bad(c, "give `segment` (and `t`) or `point`")),
        };
        sp.feather.push(effectcraft_keyframe::FeatherPoint { segment: seg, t, radius: r, tension });
        Ok(sp.feather.len() - 1)
    })?;
    Ok(json!({"mask": uid, "index": i}))
}

/// Drag a feather point: move it along the path (`segment`/`t`, or nearest `point`) and/or set
/// its `radius` (signed: negative = inner) and `tension` (%).
fn feather_set(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "mask.featherPoint.set";
    let seg = p.get("segment").and_then(Value::as_u64).map(|v| v as usize);
    let t = super::f_p(p, "t").map(|v| v.clamp(0.0, 1.0));
    let q = pt(p.get("point"));
    let radius = super::f_p(p, "radius");
    let tension = super::f_p(p, "tension").map(|v| v.clamp(0.0, 100.0) / 100.0);
    let toward = pt(p.get("toward"));
    let (_, out) = edit_path(s, p, c, |sp| {
        let i = feather_index(p, sp, c)?;
        if let Some(q) = q {
            let (sg, tt, _) = nearest_on_path(sp, q).ok_or_else(|| bad(c, "the path has no segments"))?;
            sp.feather[i].segment = sg;
            sp.feather[i].t = tt;
        }
        if let Some(sg) = seg {
            if sg >= sp.segment_count() {
                return Err(bad(c, format!("no segment {sg}")));
            }
            sp.feather[i].segment = sg;
        }
        if let Some(t) = t {
            sp.feather[i].t = t;
        }
        if let Some(r) = radius {
            sp.feather[i].radius = r;
        }
        // Dragging the radius handle: the distance from the point's path position to `toward`,
        // outer outside the mask, inner inside.
        if let Some(w) = toward {
            let f = sp.feather[i];
            let at = point_on(sp, f.segment, f.t).ok_or_else(|| bad(c, "the feather point is off the path"))?;
            let d = (w[0] - at[0]).hypot(w[1] - at[1]);
            sp.feather[i].radius = if inside_path(sp, w) { -d } else { d };
        }
        if let Some(k) = tension {
            sp.feather[i].tension = k;
        }
        Ok(serde_json::to_value(sp.feather[i]).unwrap_or(Value::Null))
    })?;
    Ok(out)
}

fn feather_remove(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "mask.featherPoint.remove";
    let all = super::b_p(p, "all").unwrap_or(false);
    let (_, n) = edit_path(s, p, c, |sp| {
        if all {
            sp.feather.clear();
        } else {
            let i = feather_index(p, sp, c)?;
            sp.feather.remove(i);
        }
        Ok(sp.feather.len())
    })?;
    Ok(json!({"remaining": n}))
}

fn feather_list(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "mask.featherPoint.list";
    let (cid, lid) = layer_p(s, p, c)?;
    let key = p.get("mask").cloned().ok_or_else(|| bad(c, "missing `mask`"))?;
    let t = s.time();
    let mut props = s.project.comp(cid).and_then(|cc| cc.layer(lid)).ok_or(EngineError::NoComp)?.props.clone();
    let lt = s.project.comp(cid).and_then(|cc| cc.layer(lid)).map(|l| l.layer_time(t)).unwrap_or(t);
    let g = path_group(&mut props, &key).ok_or_else(|| bad(c, "no such mask"))?;
    let Some(KV::Path(sp)) = g.get("path").map(|pr| pr.value_at(lt)) else { return Err(bad(c, "mask has no path")) };
    Ok(
        json!({"points": sp.feather.iter().map(|f| json!({"segment": f.segment, "t": f.t, "radius": f.radius, "tension": f.tension * 100.0})).collect::<Vec<_>>()}),
    )
}

/// Add Vertex tool: insert a vertex on segment `segment` at parameter `t` (default 0.5).
fn insert_vertex(s: &mut Session, p: &Value) -> Result<Value> {
    let seg =
        p.get("segment").and_then(Value::as_u64).ok_or_else(|| bad("mask.insertVertex", "missing `segment` (index of the segment's first vertex)"))? as usize;
    let t = super::f_p(p, "t").unwrap_or(0.5).clamp(0.0, 1.0);
    let (_, lid) = layer_p(s, p, "mask.insertVertex")?;
    let (uid, i) = edit_path_counted(s, p, "Add Vertex", |sp| {
        let i = split_segment(sp, seg, t).ok_or_else(|| bad("mask.insertVertex", "no such segment"))?;
        Ok((i, Some(CountOp::Insert(i))))
    })?;
    s.state.selected_vertices = vec![VertexRef { layer: lid, mask: uid, index: i }];
    Ok(json!(i))
}

fn remove_masks(s: &mut Session, p: &Value, all: bool) -> Result<Value> {
    let (cid, lid) = layer_p(s, p, "mask.remove")?;
    let key = p.get("mask").cloned();
    if !all && key.is_none() {
        return Err(bad("mask.remove", "missing `mask`"));
    }
    s.edit(if all { "Remove All Masks" } else { "Remove Mask" }, None, |proj, st| {
        let l: &mut Layer = layer_mut(proj, cid, lid)?;
        let masks = l.props.sub_mut("masks").ok_or_else(|| bad("mask.remove", "this layer has no masks"))?;
        if all {
            masks.children.clear();
        } else if let Some(k) = &key {
            let uid = find_mask(masks, k).map(|g| g.uid).ok_or_else(|| bad("mask.remove", "no such mask"))?;
            masks.children.retain(|c| c.uid() != uid);
        }
        st.selected_vertices.retain(|v| v.layer != lid);
        Ok(())
    })?;
    Ok(Value::Null)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("mask.new", "New Mask from Points", [], None, "{layer?, vertices: [[x,y]…], inTangents?, outTangents?, closed?, mode?}", has_layers, new_mask),
        cmd!("mask.addVertex", "Add Mask Vertex", [], None, "{layer?, mask, point: [x,y], in?, out?, index?}", has_layers, add_vertex),
        cmd!("mask.setVertex", "Set Mask Vertex", [], None, "{layer?, mask, index, point?, in?, out?, merge?}", has_layers, set_vertex),
        cmd!("mask.setClosed", "Closed", [], None, "{layer?, mask, closed?}", has_layers, set_closed),
        cmd!("mask.setRoundness", "Set Mask Roundness", [], None, "{layer?, mask, roundness (corner radius in px; 0 = square)}", has_layers, set_roundness),
        cmd!("mask.selectVertices", "Select Mask Vertices", [], None, "{vertices: [{layer, mask, index}], add?, toggle?}", has_comp, select_vertices),
        cmd!("mask.moveVertices", "Move Mask Vertices", [], None, "{vertices?: [{layer, mask, index}], delta: [dx,dy], merge?}", has_vertices, move_vertices),
        cmd!("mask.deleteVertices", "Delete Mask Vertices", [], None, "{vertices?}", has_vertices, delete_vertices),
        cmd!("mask.convertVertex", "Convert Vertex", [], None, "{layer?, mask: uid (mask or shape Path item), index, smooth?}", has_comp, convert_vertex),
        cmd!("mask.insertVertex", "Add Vertex", [], None, "{layer?, mask: uid (mask or shape Path item), segment, t?: 0..1}", has_comp, insert_vertex),
        cmd!(
            "mask.featherPoint.add",
            "Add Mask Feather Point",
            [],
            None,
            "{layer?, mask, segment?, t?: 0..1, point?: [x,y] (layer space), radius? (px, negative = inner), tension? (%)} → {index}",
            has_comp,
            feather_add
        ),
        cmd!(
            "mask.featherPoint.set",
            "Move Mask Feather Point",
            [],
            None,
            "{layer?, mask, index, segment?, t?, point? (slide along the path), radius?, toward?: [x,y] (radius from the drag point, layer space), tension? (%), merge?}",
            has_comp,
            feather_set
        ),
        cmd!("mask.featherPoint.remove", "Delete Mask Feather Point", [], None, "{layer?, mask, index? | all?}", has_comp, feather_remove),
        crate::query!("mask.featherPoint.list", "Mask Feather Points", "{layer?, mask}", feather_list),
        cmd!("mask.remove", "Remove Mask", [], None, "{layer?, mask}", has_layers, |s, p| remove_masks(s, p, false)),
        cmd!("mask.removeAll", "Remove All Masks", [], None, "{layer?}", has_layers, |s, p| remove_masks(s, p, true)),
    ]
}
