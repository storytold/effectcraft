//! Layer ▸ Round Corners (`layer.roundCorners`): rounded corners from the Properties panel's
//! Roundness row, beyond After Effects (whose solids have no roundness).
//!
//! On a shape layer it sets the Roundness of every Rectangle Path in its Contents (nested groups
//! too). A solid is first converted, in the same undo step, into a shape layer drawing a
//! rectangle of the solid's size and colour in the solid's own layer space (so the anchor point,
//! masks, mattes and every other layer setting keep their meaning and nothing moves), and keeps
//! its id, name, timing, transform, parent, switches, blend mode, masks, layer styles, markers
//! and label. Solids whose conversion would change their pixels (effects, which on a shape layer
//! measure from a comp-sized canvas; adjustment layers; non-square pixels; environment layers;
//! 3D layers in an Advanced 3D comp) are refused with the reason instead.

use effectcraft_keyframe::Value as KV;
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Comp, ItemKind, Layer, LayerSource, Node, Project, PropGroup, Renderer, Solid};
use serde_json::{Value, json};

use super::{CommandSpec, bad, has_layers, layer_mut, layers_p, merge_p};
use crate::{EngineError, Result, Session, cmd};

const CMD: &str = "layer.roundCorners";

/// The Rounded Rectangle tool's roundness for a `w`×`h` rectangle: 15% of the shorter side.
fn default_roundness(size: [f64; 2]) -> f64 {
    size[0].min(size[1]).max(0.0) * 0.15
}

/// The solid behind `l`, or why it cannot become a shape layer without changing its pixels.
pub fn convertible_solid(project: &Project, comp: &Comp, l: &Layer) -> std::result::Result<Solid, String> {
    let LayerSource::Solid { item } = l.source else { return Err(format!("{}: not a solid or shape layer", l.name)) };
    let Some(ItemKind::Solid(sol)) = project.item(item).map(|i| &i.kind) else { return Err(format!("{}: its solid is missing", l.name)) };
    let why = if l.switches.adjustment {
        Some("it is an adjustment layer")
    } else if l.effects().is_some_and(|g| !g.children.is_empty()) {
        Some("it has effects, which measure from a shape layer's comp-sized canvas and would move (pre-compose it, or remove the effects first)")
    } else if (sol.pixel_aspect - 1.0).abs() > 1e-9 {
        Some("its solid has non-square pixels")
    } else if l.environment || l.environment_background {
        Some("it is an environment layer")
    } else if l.is_3d() && comp.renderer == Renderer::Advanced3D {
        Some("it is a 3D layer in an Advanced 3D composition (shape layers extrude there)")
    } else {
        None
    };
    match why {
        Some(w) => Err(format!("{}: cannot round its corners: {w}", l.name)),
        None => Ok(sol.clone()),
    }
}

/// The Contents of a converted solid: "Rectangle 1" with a Rectangle Path of the solid's size
/// and a Fill of its colour, centred on the solid's centre (layer space is the solid's).
fn solid_contents(ids: &mut Ids, sol: &Solid, roundness: f64) -> PropGroup {
    let (w, h) = (sol.width as f64, sol.height as f64);
    let rect = build::shape_rect(ids, [w, h], [0.0, 0.0], roundness);
    let c = sol.color;
    let fill = build::shape_fill(ids, [c[0] as f64, c[1] as f64, c[2] as f64, 1.0]);
    let mut g = build::shape_group(ids, "Rectangle 1", vec![rect, fill]);
    if let Some(pr) = g.sub_mut("transform").and_then(|t| t.get_mut("position")) {
        pr.value = KV::Vec2([w / 2.0, h / 2.0]);
    }
    build::contents(ids).with(g)
}

/// Every Rectangle Path below `g` (nested groups included), in Contents order.
pub fn rect_paths(g: &PropGroup) -> Vec<&PropGroup> {
    let mut out = vec![];
    collect_rects(g, &mut out, 0);
    out
}

fn collect_rects<'a>(g: &'a PropGroup, out: &mut Vec<&'a PropGroup>, depth: usize) {
    // Shape groups nest a few levels deep; a malformed file stops here rather than overflowing.
    if depth > 64 {
        return;
    }
    for c in g.groups() {
        if c.match_id == "rect" {
            out.push(c);
        } else {
            collect_rects(c, out, depth + 1);
        }
    }
}

/// Set the Roundness of every Rectangle Path below `g` at layer time `lt` (a key there when it
/// is animated); `None` gives each the Rounded Rectangle tool's for its size. Returns how many.
fn round_rects(g: &mut PropGroup, lt: effectcraft_time::Tick, roundness: Option<f64>, depth: usize) -> usize {
    if depth > 64 {
        return 0;
    }
    let mut n = 0;
    for c in g.children.iter_mut() {
        let Node::Group(c) = c else { continue };
        if c.match_id != "rect" {
            n += round_rects(c, lt, roundness, depth + 1);
            continue;
        }
        let size = match c.get("size").map(|p| p.value_at(lt)) {
            Some(KV::Vec2(s)) => s,
            _ => [0.0, 0.0],
        };
        if let Some(pr) = c.get_mut("roundness") {
            pr.set_value_at(lt, KV::Scalar(roundness.unwrap_or_else(|| default_roundness(size))));
            n += 1;
        }
    }
    n
}

fn round_corners(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lids) = layers_p(s, p)?;
    if lids.is_empty() {
        return Err(bad(CMD, "select a solid or shape layer first"));
    }
    let roundness = match p.get("roundness") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_f64().filter(|v| v.is_finite()).ok_or_else(|| bad(CMD, "`roundness`: pixels"))?.max(0.0)),
    };
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    // Check every layer before changing any: all or nothing.
    let mut solids = vec![];
    for lid in &lids {
        let l = comp.layer(*lid).ok_or_else(|| bad(CMD, format!("no layer {}", lid.0)))?;
        match l.source {
            LayerSource::Shape => {
                if l.props.sub("contents").is_none_or(|c| rect_paths(c).is_empty()) {
                    return Err(bad(CMD, format!("{}: the shape layer has no Rectangle Path", l.name)));
                }
                if l.switches.locked {
                    return Err(bad(CMD, format!("{}: the layer is locked", l.name)));
                }
            }
            _ => {
                if l.switches.locked {
                    return Err(bad(CMD, format!("{}: the layer is locked", l.name)));
                }
                solids.push((*lid, convertible_solid(&s.project, comp, l).map_err(|e| bad(CMD, e))?));
            }
        }
    }
    let t = s.time();
    let converted: Vec<u64> = solids.iter().map(|(l, _)| l.0).collect();
    s.edit("Round Corners", merge_p(p), |proj, _| {
        for lid in &lids {
            if let Some((_, sol)) = solids.iter().find(|(l, _)| l == lid) {
                let r = roundness.unwrap_or_else(|| default_roundness([sol.width as f64, sol.height as f64]));
                let mut next = proj.next_id;
                let contents = solid_contents(&mut Ids(&mut next), sol, r);
                proj.next_id = next;
                let l = layer_mut(proj, cid, *lid)?;
                l.source = LayerSource::Shape;
                // Where a new shape layer keeps its Contents: first.
                l.props.children.insert(0, contents.into());
            } else {
                let l = layer_mut(proj, cid, *lid)?;
                let lt = l.layer_time(t);
                if let Some(c) = l.props.sub_mut("contents") {
                    round_rects(c, lt, roundness, 0);
                }
            }
        }
        Ok(())
    })?;
    Ok(json!({"layers": lids.iter().map(|l| l.0).collect::<Vec<_>>(), "converted": converted}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "layer.roundCorners",
        "Round Corners",
        [],
        None,
        "{layer?|layers? (default the selection), roundness? px (default 15% of the shorter side, as the Rounded Rectangle tool), comp?, merge?} → {layers, converted}: a solid becomes a shape layer drawing a rounded rectangle of its size and colour (same id, name, timing, transform, parent, switches, masks, styles, markers; refused with the reason for solids with effects, adjustment layers, non-square pixels, environment layers, 3D in Advanced 3D); a shape layer gets the roundness on every Rectangle Path in its Contents. One undo step.",
        has_layers,
        round_corners
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_project::LayerId;

    fn session() -> (Session, u64) {
        let mut s = Session::default();
        s.execute("comp.new", json!({"width": 320, "height": 240, "duration": 2})).unwrap();
        let id = s.execute("layer.newSolid", json!({"name": "Red", "color": "#e02020", "width": 200, "height": 120})).unwrap()["layer"].as_u64().unwrap();
        // Off-centre, rotated by a whole turn (a no-op that still goes through the matrix) and
        // with a moved anchor: the conversion must keep the layer where it is.
        s.execute("prop.set", json!({"layer": id, "path": "transform/position", "value": [140, 110]})).unwrap();
        s.execute("prop.set", json!({"layer": id, "path": "transform/anchor", "value": [90, 50]})).unwrap();
        (s, id)
    }

    fn layer(s: &Session, id: u64) -> Layer {
        s.project.comp(s.active_comp_id().unwrap()).unwrap().layer(LayerId(id)).unwrap().clone()
    }

    fn roundness(s: &Session, id: u64) -> Vec<f64> {
        let l = layer(s, id);
        rect_paths(l.props.sub("contents").unwrap()).iter().map(|r| r.get("roundness").unwrap().value.as_f64()).collect()
    }

    #[test]
    fn solid_becomes_a_shape_layer_with_the_same_pixels_at_zero_roundness() {
        let (mut s, id) = session();
        s.execute("layer.addMarker", json!({"layers": [id], "comment": "kept"})).unwrap();
        s.execute("mask.new", json!({"layer": id, "vertices": [[20, 20], [180, 10], [100, 110]], "closed": true})).unwrap();
        let cid = s.active_comp_id().unwrap();
        let before = layer(&s, id);
        let img = s.render(cid, s.time(), Default::default());
        let r = s.execute(CMD, json!({"layer": id, "roundness": 0})).unwrap();
        assert_eq!(r["converted"], json!([id]));
        let after = layer(&s, id);
        assert!(matches!(after.source, LayerSource::Shape));
        // Same layer: id, name, label, timing, switches, transform, masks, markers.
        assert_eq!(
            (after.id, &after.name, after.label, after.in_point, after.out_point),
            (before.id, &before.name, before.label, before.in_point, before.out_point)
        );
        assert_eq!(after.switches, before.switches);
        assert_eq!(after.markers, before.markers);
        assert_eq!(after.transform(), before.transform());
        assert_eq!(after.masks(), before.masks());
        assert_eq!(roundness(&s, id), vec![0.0]);
        // Same pixels.
        let img2 = s.render(cid, s.time(), Default::default());
        let diff = img.data.iter().zip(&img2.data).map(|(a, b)| (0..4).map(|k| (a[k] - b[k]).abs()).fold(0.0f32, f32::max)).fold(0.0f32, f32::max);
        assert!(diff < 1e-4, "converted solid renders differently (max channel diff {diff})");
        // One undo step back to the solid.
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(layer(&s, id), before);
    }

    #[test]
    fn roundness_rounds_the_corners_and_a_shape_layer_takes_it_too() {
        let (mut s, id) = session();
        let cid = s.active_comp_id().unwrap();
        s.execute("prop.set", json!({"layer": id, "path": "transform/anchor", "value": [100, 60]})).unwrap();
        s.execute("prop.set", json!({"layer": id, "path": "transform/position", "value": [160, 120]})).unwrap();
        let sharp = s.render(cid, s.time(), Default::default());
        // The solid spans x 60..260, y 60..180: its top-left corner pixel is red.
        assert!(sharp.get(61, 61)[0] > 0.5);
        s.execute(CMD, json!({"layer": id, "roundness": 40})).unwrap();
        let round = s.render(cid, s.time(), Default::default());
        assert!(round.get(61, 61)[3] < 0.01, "corner still filled: {:?}", round.get(61, 61));
        assert!(round.get(160, 120)[0] > 0.5);
        // Again, now on the shape layer: only the roundness changes.
        let r = s.execute(CMD, json!({"layer": id, "roundness": 12})).unwrap();
        assert_eq!(r["converted"], json!([]));
        assert_eq!(roundness(&s, id), vec![12.0]);
    }

    #[test]
    fn a_scrub_is_one_undo_step() {
        let (mut s, id) = session();
        let before = layer(&s, id);
        for r in [2, 5, 9] {
            s.execute(CMD, json!({"layer": id, "roundness": r, "merge": "props-roundness"})).unwrap();
        }
        assert_eq!(roundness(&s, id), vec![9.0]);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(layer(&s, id), before);
    }

    #[test]
    fn nested_rectangles_of_a_shape_layer_all_round() {
        let mut s = Session::default();
        s.execute("comp.new", json!({"width": 320, "height": 240, "duration": 2})).unwrap();
        let l = s.execute("shape.newShape", json!({"kind": "rect", "size": [100, 50]})).unwrap()["layer"].as_u64().unwrap();
        s.execute("shape.newShape", json!({"layer": l, "kind": "rounded", "size": [80, 40]})).unwrap();
        s.execute("shape.newShape", json!({"layer": l, "kind": "ellipse", "size": [80, 40]})).unwrap();
        s.execute(CMD, json!({"layer": l, "roundness": 7})).unwrap();
        assert_eq!(roundness(&s, l), vec![7.0, 7.0]);
        // Without a roundness: the Rounded Rectangle tool's, per rectangle.
        s.execute(CMD, json!({"layer": l})).unwrap();
        assert_eq!(roundness(&s, l), vec![6.0, 7.5]);
    }

    #[test]
    fn new_shapes_take_a_roundness() {
        let mut s = Session::default();
        s.execute("comp.new", json!({"width": 320, "height": 240, "duration": 2})).unwrap();
        let l = s.execute("shape.newShape", json!({"kind": "rect", "size": [100, 50], "roundness": 9})).unwrap()["layer"].as_u64().unwrap();
        assert_eq!(roundness(&s, l), vec![9.0]);
        let l = s.execute("layer.newShape", json!({"kind": "rounded", "size": [100, 50], "roundness": 4})).unwrap()["layer"].as_u64().unwrap();
        assert_eq!(roundness(&s, l), vec![4.0]);
        let e = s.execute("shape.newShape", json!({"kind": "ellipse", "roundness": 9})).unwrap_err().to_string();
        assert!(e.contains("roundness"), "{e}");
    }

    #[test]
    fn refusals_keep_the_layer_and_explain_why() {
        let (mut s, id) = session();
        s.execute("effect.apply", json!({"layers": [id], "effect": "Gaussian Blur"})).unwrap();
        let before = layer(&s, id);
        let e = s.execute(CMD, json!({"layer": id, "roundness": 10})).unwrap_err().to_string();
        assert!(e.contains("effects"), "{e}");
        assert_eq!(layer(&s, id), before);
        let nl = s.execute("layer.newNull", json!({})).unwrap()["layer"].as_u64().unwrap();
        let e = s.execute(CMD, json!({"layer": nl})).unwrap_err().to_string();
        assert!(e.contains("not a solid or shape layer"), "{e}");
        let e = s.execute(CMD, json!({"layer": id, "roundness": "big"})).unwrap_err().to_string();
        assert!(e.contains("roundness"), "{e}");
    }

    #[test]
    fn listed_for_agents() {
        let spec = crate::commands::find(CMD).unwrap();
        assert!(spec.params.contains("roundness"));
    }
}
