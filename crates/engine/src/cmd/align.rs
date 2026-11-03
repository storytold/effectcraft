//! Align panel: `layer.align {edge, to}` and `layer.distribute {mode}`; the arrow keys'
//! `layer.nudge {x, y}`.
//!
//! Layers are aligned by their content bounds in comp space (source size, text glyphs, shape
//! contents through the layer's transform and its parents'). Each layer moves by a comp-space
//! offset that is converted into its parent's space before it is added to Position (Separate
//! Dimensions writes X/Y Position), at the current time (a key when Position is animated). All
//! moves are one undo step.

use effectcraft_geom::{Mat4, Vec3};
use effectcraft_keyframe::Value as KValue;
use effectcraft_project::{ItemId, Layer, LayerId};
use effectcraft_render::EvalCtx;
use serde_json::{Value, json};

use super::{CommandSpec, bad, f_p, has_layers, layer_mut, layers_p, merge_p, str_p};
use crate::{EngineError, Result, Session, cmd};

/// A layer's comp-space bounds and how a comp-space offset maps into its Position.
struct Placed {
    id: LayerId,
    /// `[x0, y0, x1, y1]` in comp space.
    b: [f64; 4],
    /// Comp space → the parent's space (the space Position lives in).
    to_parent: Mat4,
}

fn placed(s: &Session, cid: ItemId, ids: &[LayerId]) -> Result<Vec<Placed>> {
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let ctx = EvalCtx { footage: Some(s.footage.as_ref()), expr: s.expr.as_deref(), ..EvalCtx::new(&s.project, cid, comp, s.time()) };
    let mut out = vec![];
    for &id in ids {
        let Some(l) = comp.layer(id) else { continue };
        if l.transform().is_none() {
            continue;
        }
        let Some(b) = effectcraft_render::content_bounds(&ctx, l) else { continue };
        let w = ctx.world_matrix(l);
        let pts = [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]].map(|p| w.apply(Vec3::from([p[0], p[1], 0.0])));
        let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for p in pts {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        out.push(Placed { id, b: [x0, y0, x1, y1], to_parent: to_parent(&ctx, l) });
    }
    Ok(out)
}

/// Comp space → the space of `l`'s Position (its parent's).
fn to_parent(ctx: &EvalCtx, l: &Layer) -> Mat4 {
    // Parent chain: world(layer) = local(pn) ⋯ local(p1) · local(layer).
    let mut par = Mat4::IDENTITY;
    let mut cur = l.parent;
    let mut guard = 0;
    while let Some(pid) = cur {
        guard += 1;
        let Some(p) = ctx.comp.layer(pid).filter(|_| guard < 64) else { break };
        par = ctx.local_matrix(p) * par;
        cur = p.parent;
    }
    par.inverse().unwrap_or(Mat4::IDENTITY)
}

/// Move layers by comp-space offsets, as one undo step (merged into the last one with the same
/// `merge` key).
fn apply_moves(s: &mut Session, cid: ItemId, label: &str, moves: Vec<(LayerId, Mat4, [f64; 2])>, merge: Option<&str>) -> Result<Value> {
    let t = s.time();
    let moved: Vec<u64> = moves.iter().filter(|m| m.2[0].abs() > 1e-9 || m.2[1].abs() > 1e-9).map(|m| m.0.0).collect();
    s.edit(label, merge, |proj, _| {
        for (lid, to_parent, d) in &moves {
            if d[0].abs() <= 1e-9 && d[1].abs() <= 1e-9 {
                continue;
            }
            let o = to_parent.apply(Vec3::from([0.0, 0.0, 0.0]));
            let q = to_parent.apply(Vec3::from([d[0], d[1], 0.0]));
            let dp = [q.x - o.x, q.y - o.y, q.z - o.z];
            let l = layer_mut(proj, cid, *lid)?;
            let lt = l.layer_time(t);
            let three = l.is_3d();
            let tr = l.transform_mut().ok_or_else(|| bad("layer.align", "layer has no transform"))?;
            if tr.get("positionX").is_some() {
                let dims = if three { 3 } else { 2 };
                for (k, m) in ["positionX", "positionY", "positionZ"].iter().enumerate().take(dims) {
                    if let Some(pr) = tr.get_mut(m) {
                        let v = pr.value_at(lt).as_f64() + dp[k];
                        pr.set_value_at(lt, KValue::Scalar(v));
                    }
                }
            } else if let Some(pr) = tr.get_mut("position") {
                let cur = pr.value_at(lt);
                let a = cur.to_json();
                let get = |k: usize| a.get(k).and_then(Value::as_f64).unwrap_or(0.0);
                let nv = json!([get(0) + dp[0], get(1) + dp[1], get(2) + if three { dp[2] } else { 0.0 }]);
                if let Some(v) = cur.coerce_json(&nv) {
                    pr.set_value_at(lt, v);
                }
            }
        }
        Ok(())
    })?;
    Ok(json!({"moved": moved}))
}

const EDGES: &str = "left|hcenter|right|top|vcenter|bottom";

/// The coordinate of an edge (or centre) of bounds; `true` for horizontal edges.
fn edge_of(b: &[f64; 4], edge: &str) -> Option<(f64, bool)> {
    Some(match edge {
        "left" => (b[0], true),
        "hcenter" | "horizontalCenter" => ((b[0] + b[2]) * 0.5, true),
        "right" => (b[2], true),
        "top" => (b[1], false),
        "vcenter" | "verticalCenter" => ((b[1] + b[3]) * 0.5, false),
        "bottom" => (b[3], false),
        _ => return None,
    })
}

fn align(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "layer.align";
    let edge = str_p(p, "edge").ok_or_else(|| bad(c, format!("missing `edge` ({EDGES})")))?;
    let to = str_p(p, "to").unwrap_or("composition");
    let (cid, ids) = layers_p(s, p)?;
    let items = placed(s, cid, &ids)?;
    if items.is_empty() {
        return Err(bad(c, "no layers with bounds to align"));
    }
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let target: [f64; 4] = match to {
        "composition" | "comp" => [0.0, 0.0, comp.width as f64, comp.height as f64],
        "selection" => {
            if items.len() < 2 {
                return Err(bad(c, "aligning to the selection needs two or more layers"));
            }
            items.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |a, it| {
                [a[0].min(it.b[0]), a[1].min(it.b[1]), a[2].max(it.b[2]), a[3].max(it.b[3])]
            })
        }
        x => return Err(bad(c, format!("unknown `to` `{x}` (selection|composition)"))),
    };
    let (goal, horiz) = edge_of(&target, edge).ok_or_else(|| bad(c, format!("unknown edge `{edge}` ({EDGES})")))?;
    let moves = items
        .into_iter()
        .map(|it| {
            let (v, _) = edge_of(&it.b, edge).unwrap_or((goal, horiz));
            let d = if horiz { [goal - v, 0.0] } else { [0.0, goal - v] };
            (it.id, it.to_parent, d)
        })
        .collect();
    apply_moves(s, cid, "Align Layers", moves, None)
}

fn distribute(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "layer.distribute";
    let mode = str_p(p, "mode").or(str_p(p, "edge")).ok_or_else(|| bad(c, format!("missing `mode` ({EDGES})")))?;
    let (cid, ids) = layers_p(s, p)?;
    let mut items = placed(s, cid, &ids)?;
    if items.len() < 3 {
        return Err(bad(c, "distributing needs three or more layers"));
    }
    edge_of(&items[0].b, mode).ok_or_else(|| bad(c, format!("unknown mode `{mode}` ({EDGES})")))?;
    let key = |it: &Placed| edge_of(&it.b, mode).map_or(0.0, |e| e.0);
    items.sort_by(|a, b| key(a).total_cmp(&key(b)));
    let (first, last) = (key(&items[0]), key(&items[items.len() - 1]));
    let n = items.len() - 1;
    let horiz = edge_of(&items[0].b, mode).is_some_and(|e| e.1);
    let moves = items
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let goal = first + (last - first) * i as f64 / n as f64;
            let dv = goal - key(it);
            (it.id, it.to_parent, if horiz { [dv, 0.0] } else { [0.0, dv] })
        })
        .collect();
    apply_moves(s, cid, "Distribute Layers", moves, None)
}

/// The arrow keys in the Composition panel: move the selected (unlocked) layers by `x`, `y` comp
/// pixels; After Effects nudges 1 pixel at the panel's magnification, 10 with Shift. Each call
/// is one undo step.
fn nudge(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "layer.nudge";
    let d = [f_p(p, "x").unwrap_or(0.0), f_p(p, "y").unwrap_or(0.0)];
    if !d.iter().all(|v| v.is_finite()) {
        return Err(bad(c, "`x` and `y` must be numbers"));
    }
    let (cid, ids) = layers_p(s, p)?;
    let ids = super::unlocked(s, cid, ids, c)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let ctx = EvalCtx { footage: Some(s.footage.as_ref()), expr: s.expr.as_deref(), ..EvalCtx::new(&s.project, cid, comp, s.time()) };
    let moves = ids.iter().filter_map(|id| comp.layer(*id)).filter(|l| l.transform().is_some()).map(|l| (l.id, to_parent(&ctx, l), d)).collect();
    apply_moves(s, cid, "Nudge", moves, merge_p(p))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("layer.nudge", "Nudge Layers", [], None, "{x?, y? (comp pixels), layers?, merge?}", has_layers, nudge),
        cmd!(
            "layer.align",
            "Align Layers",
            [],
            None,
            "{edge: left|hcenter|right|top|vcenter|bottom, to?: composition|selection (default composition), layers?}",
            has_layers,
            align
        ),
        cmd!(
            "layer.distribute",
            "Distribute Layers",
            [],
            None,
            "{mode: left|hcenter|right|top|vcenter|bottom (edges or centres), layers?}",
            has_layers,
            distribute
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        let mut s = Session::default();
        s.execute("comp.new", json!({"width": 400, "height": 300, "duration": 2})).unwrap();
        s
    }

    fn solid(s: &mut Session, name: &str, w: u32, h: u32, pos: [f64; 2]) -> u64 {
        let id = s.execute("layer.newSolid", json!({"name": name, "width": w, "height": h})).unwrap()["layer"].as_u64().unwrap();
        s.execute("prop.set", json!({"layer": id, "path": "transform/position", "value": [pos[0], pos[1], 0.0]})).unwrap();
        id
    }

    fn bounds(s: &Session, id: u64) -> [f64; 4] {
        let cid = s.active_comp_id().unwrap();
        placed(s, cid, &[LayerId(id)]).unwrap()[0].b
    }

    #[test]
    fn align_to_composition_and_selection_is_one_undo_step() {
        let mut s = session();
        let a = solid(&mut s, "A", 50, 40, [100.0, 100.0]);
        let b = solid(&mut s, "B", 80, 20, [250.0, 180.0]);
        s.execute("layer.align", json!({"edge": "left", "layers": [a, b]})).unwrap();
        assert!(bounds(&s, a)[0].abs() < 1e-6 && bounds(&s, b)[0].abs() < 1e-6);
        // One step undoes both.
        s.execute("edit.undo", json!({})).unwrap();
        assert!((bounds(&s, a)[0] - 75.0).abs() < 1e-6);
        assert!((bounds(&s, b)[0] - 210.0).abs() < 1e-6);
        s.execute("layer.align", json!({"edge": "bottom", "to": "composition", "layers": [a, b]})).unwrap();
        assert!((bounds(&s, a)[3] - 300.0).abs() < 1e-6 && (bounds(&s, b)[3] - 300.0).abs() < 1e-6);
        s.execute("edit.undo", json!({})).unwrap();
        // Selection: to the selection's right edge (B's) and vertical centre.
        s.execute("layer.align", json!({"edge": "right", "to": "selection", "layers": [a, b]})).unwrap();
        assert!((bounds(&s, a)[2] - 290.0).abs() < 1e-6 && (bounds(&s, b)[2] - 290.0).abs() < 1e-6);
        s.execute("layer.align", json!({"edge": "vcenter", "to": "selection", "layers": [a, b]})).unwrap();
        let (ca, cb) = (bounds(&s, a), bounds(&s, b));
        assert!(((ca[1] + ca[3]) / 2.0 - (cb[1] + cb[3]) / 2.0).abs() < 1e-6);
        assert!(s.execute("layer.align", json!({"edge": "left", "to": "selection", "layers": [a]})).is_err());
        assert!(s.execute("layer.align", json!({"edge": "diagonal", "layers": [a]})).is_err());
    }

    /// #352: shape layers align and snap by their visual edges (half the stroke, no
    /// anti-aliasing padding), so two rectangles lined up edge to edge touch with no gap.
    #[test]
    fn shape_layers_align_by_their_visual_edges() {
        let mut s = session();
        let shape = |s: &mut Session, extra: Value| {
            let mut p = json!({"kind": "rect", "size": [100, 100], "fill": "#ff0000"});
            p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            s.execute("layer.newShape", p).unwrap()["layer"].as_u64().unwrap()
        };
        let a = shape(&mut s, json!({}));
        let b = shape(&mut s, json!({"stroke": "#ffffff", "strokeWidth": 10}));
        let width = |s: &Session, id| {
            let r = bounds(s, id);
            r[2] - r[0]
        };
        assert!((width(&s, a) - 100.0).abs() < 1e-6 && (width(&s, b) - 110.0).abs() < 1e-6, "{} {}", width(&s, a), width(&s, b));
        s.execute("layer.align", json!({"edge": "left", "layers": [a, b]})).unwrap();
        assert!(bounds(&s, a)[0].abs() < 1e-6 && bounds(&s, b)[0].abs() < 1e-6);
    }

    #[test]
    fn align_honours_parent_scale_and_rotation() {
        let mut s = session();
        let parent = solid(&mut s, "P", 10, 10, [200.0, 150.0]);
        s.execute("prop.set", json!({"layer": parent, "path": "transform/scale", "value": [200.0, 200.0, 100.0]})).unwrap();
        s.execute("prop.set", json!({"layer": parent, "path": "transform/rotation", "value": 90.0})).unwrap();
        let child = solid(&mut s, "C", 20, 10, [30.0, 10.0]);
        s.execute("layer.setParent", json!({"layers": [child], "parent": parent})).unwrap();
        s.execute("layer.align", json!({"edge": "top", "layers": [child]})).unwrap();
        assert!(bounds(&s, child)[1].abs() < 1e-6, "{:?}", bounds(&s, child));
        s.execute("layer.align", json!({"edge": "hcenter", "layers": [child]})).unwrap();
        let b = bounds(&s, child);
        assert!(((b[0] + b[2]) / 2.0 - 200.0).abs() < 1e-6 && b[1].abs() < 1e-6);
    }

    #[test]
    fn distribute_edges_and_centres() {
        let mut s = session();
        let a = solid(&mut s, "A", 20, 20, [50.0, 50.0]);
        let b = solid(&mut s, "B", 60, 20, [90.0, 100.0]);
        let c = solid(&mut s, "C", 20, 20, [350.0, 200.0]);
        s.execute("layer.distribute", json!({"mode": "hcenter", "layers": [a, b, c]})).unwrap();
        let cx = |id| {
            let b = bounds(&s, id);
            (b[0] + b[2]) / 2.0
        };
        assert!((cx(b) - 200.0).abs() < 1e-6 && (cx(a) - 50.0).abs() < 1e-6 && (cx(c) - 350.0).abs() < 1e-6);
        s.execute("layer.distribute", json!({"mode": "top", "layers": [a, b, c]})).unwrap();
        assert!((bounds(&s, b)[1] - 115.0).abs() < 1e-6);
        assert!(s.execute("layer.distribute", json!({"mode": "top", "layers": [a, b]})).is_err());
    }

    #[test]
    fn separated_position() {
        let mut s = session();
        let a = solid(&mut s, "A", 40, 40, [100.0, 100.0]);
        s.execute("prop.separateDimensions", json!({"layer": a, "value": true})).unwrap();
        assert!(s.active_comp().unwrap().layers.iter().any(|l| l.transform().is_some_and(|t| t.get("positionX").is_some())));
        s.execute("layer.align", json!({"edge": "right", "layers": [a]})).unwrap();
        assert!((bounds(&s, a)[2] - 400.0).abs() < 1e-6);
    }

    /// The arrow keys' nudge (#290): comp pixels through a parent's scale, sub-pixel steps, one
    /// undo step each, a key at the current time when Position is animated; locked layers stay.
    #[test]
    fn nudge_moves_layers_by_comp_pixels() {
        let mut s = session();
        let pos = |s: &Session, id: u64| s.active_comp().unwrap().layer(LayerId(id)).unwrap().props.prop("transform/position").unwrap().value.as_vec3();
        let a = solid(&mut s, "A", 40, 40, [100.0, 100.0]);
        let parent = solid(&mut s, "P", 10, 10, [0.0, 0.0]);
        s.execute("prop.set", json!({"layer": parent, "path": "transform/scale", "value": [200.0, 200.0, 100.0]})).unwrap();
        let child = solid(&mut s, "C", 10, 10, [10.0, 10.0]);
        s.execute("layer.setParent", json!({"layers": [child], "parent": parent})).unwrap();
        let undo = s.history.undo.len();
        s.execute("layer.nudge", json!({"layers": [a, child], "x": 1.0})).unwrap();
        s.execute("layer.nudge", json!({"layers": [a, child], "y": -0.25})).unwrap();
        assert_eq!(pos(&s, a), [101.0, 99.75, 0.0]);
        assert_eq!(pos(&s, child), [10.5, 9.875, 0.0], "half as far under a 200 % parent");
        assert_eq!(s.history.undo.len(), undo + 2, "one undo step per nudge");
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(pos(&s, a), [101.0, 100.0, 0.0]);
        // Animated Position gets a key at the current time.
        s.execute("prop.addKey", json!({"layer": a, "path": "transform/position", "time": 0.0, "value": [0, 0, 0]})).unwrap();
        s.execute("time.set", json!({"time": 1.0})).unwrap();
        s.execute("layer.nudge", json!({"layers": [a], "x": 10.0})).unwrap();
        let keys = s.active_comp().unwrap().layer(LayerId(a)).unwrap().props.prop("transform/position").unwrap().keys.clone();
        assert_eq!(
            keys.iter().map(|k| (k.time, k.value.as_vec3())).collect::<Vec<_>>(),
            vec![(effectcraft_time::Tick::ZERO, [0.0; 3]), (s.time(), [10.0, 0.0, 0.0])]
        );
        // A locked layer doesn't move.
        s.execute("layer.setSwitch", json!({"layers": [child], "switch": "lock", "value": true})).unwrap();
        assert!(s.execute("layer.nudge", json!({"layers": [child], "x": 1.0})).is_err());
        assert_eq!(pos(&s, child), [10.5, 10.0, 0.0]);
    }
}
