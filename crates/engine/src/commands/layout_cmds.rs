//! `layer.bounds`: where layers are on screen at a comp time (MCP `get_layout`). Headless agents
//! can't eyeball a render; this gives them each layer's comp-space box, visibility and overlaps.
//! The geometry itself is [`crate::layout`]. Also holds small helpers the other analysis queries
//! share.

use effectcraft_project::{Comp, LayerId};
use effectcraft_render::{RenderOpts, Renderer};
use serde_json::{Value, json};

use super::{CommandSpec, b_p, bad, comp_id, resolve_layer, time_p};
use crate::{EngineError, Result, Session, layout, query};

pub(super) fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}
pub(super) fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}
pub(super) fn r4(v: f64) -> f64 {
    (v * 10000.0).round() / 10000.0
}
pub(super) fn pt(p: [f64; 2]) -> Value {
    json!([r3(p[0]), r3(p[1])])
}
pub(super) fn rect(r: [f64; 4]) -> Value {
    json!(r.map(r3))
}

/// The layers a query names (`layers: [..]` or `layer`), or every layer of the comp.
pub(super) fn named_layers(comp: &Comp, p: &Value, cmd: &str) -> Result<Vec<LayerId>> {
    let one = |v: &Value| resolve_layer(comp, v).ok_or_else(|| bad(cmd, format!("no layer {v} (use a layer id, \"#n\" or a name from comp.info)")));
    match p.get("layers").or_else(|| p.get("layer")).filter(|v| !v.is_null()) {
        Some(Value::Array(a)) => a.iter().map(one).collect(),
        Some(v) => Ok(vec![one(v)?]),
        None => Ok(comp.layers.iter().map(|l| l.id).collect()),
    }
}

/// A renderer with the session's footage, expressions, layer cache and preferences.
pub(super) fn renderer(s: &Session, opts: RenderOpts) -> Renderer<'_> {
    let opts = RenderOpts { nested_switches: s.prefs.general.switches_affect_nested_comps, draft_shadows: s.prefs.three_d.realtime_shadows, ..opts };
    let mut r = Renderer::new(&s.project, s.footage.as_ref(), opts);
    r.expr = s.expr.as_deref();
    r.cache = Some(&s.layer_cache);
    r
}

// ---------------------------------------------------------------- layer.bounds

/// Most layers whose pairwise overlaps are computed.
const MAX_OVERLAP_LAYERS: usize = 256;

fn layer_bounds(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "layer.bounds";
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let t = time_p(s, p, Some(comp));
    let ids = named_layers(comp, p, C)?;
    let pixels = b_p(p, "pixels").unwrap_or(false);
    let r = renderer(s, RenderOpts::default());
    let ctx = r.eval_ctx(cid, t).ok_or(EngineError::NoComp)?;
    let (w, h) = (comp.width as f64, comp.height as f64);
    let mut out = vec![];
    let mut quads: Vec<(LayerId, [[f64; 2]; 4], f64)> = vec![];
    for id in ids {
        let Some(l) = comp.layer(id) else { continue };
        let index = comp.layers.iter().position(|x| x.id == id).map_or(0, |i| i + 1);
        let (visible, why) = layout::visibility(&ctx, l);
        let tr = l.transform();
        let mut o = json!({
            "id": id.0,
            "index": index,
            "name": l.name,
            "type": l.source.type_name(),
            "visible": visible,
            "why": why,
            "opacity": r2(ctx.opacity(l) * 100.0),
            "threeD": l.is_3d(),
            "adjustment": l.switches.adjustment,
            "parent": l.parent.map(|p| p.0),
            "inPoint": l.in_point.seconds(),
            "outPoint": l.out_point.seconds(),
            "position": tr.map(|tr| { let v = ctx.position(l, tr); if l.is_3d() { json!(v.map(r3)) } else { json!([r3(v[0]), r3(v[1])]) } }),
            "anchor": tr.map(|tr| { let a = ctx.v3(l, tr, "anchor", [0.0; 3]); json!([r3(a[0]), r3(a[1])]) }),
            "anchorInComp": layout::anchor_in_comp(&ctx, l).map(pt),
        });
        let mattes: Vec<u64> = comp.layers.iter().filter(|x| x.track_matte.as_ref().is_some_and(|m| m.layer == id)).map(|x| x.id.0).collect();
        if !mattes.is_empty() {
            o["matteFor"] = json!(mattes);
        }
        match layout::layer_geometry(&ctx, l) {
            Some(g) => {
                o["source"] = json!({"left": r3(g.source[0]), "top": r3(g.source[1]), "width": r3(g.source[2]), "height": r3(g.source[3])});
                o["projected"] = json!(g.projected);
                if g.projected {
                    o["bounds"] = rect(g.bounds);
                    o["corners"] = json!(g.corners.map(pt));
                    o["center"] = pt(g.center);
                    o["onScreen"] = json!(r4(layout::on_screen_fraction(&g.corners, w, h)));
                    let area = layout::polygon_area(&g.corners);
                    if visible && area > 1e-9 {
                        quads.push((id, g.corners, area));
                    }
                } else {
                    o["bounds"] = Value::Null;
                    o["note"] = json!("part of this 3D layer is behind the active camera, so its corners can't be projected");
                }
            }
            None => {
                o["bounds"] = Value::Null;
                o["note"] = json!(if l.is_camera() || l.is_light() { "cameras and lights have no content bounds" } else { "no content at this time" });
            }
        }
        if pixels {
            o["pixelBounds"] = if visible { layout::pixel_bounds(&r, &ctx, l).map_or(Value::Null, rect) } else { Value::Null };
        }
        out.push(o);
    }
    let mut v = json!({"comp": cid.0, "time": t.seconds(), "width": comp.width, "height": comp.height, "layers": out});
    if b_p(p, "overlaps").unwrap_or(false) {
        let mut pairs = vec![];
        let q = quads.get(..MAX_OVERLAP_LAYERS.min(quads.len())).unwrap_or(&[]);
        for (i, a) in q.iter().enumerate() {
            for b in q.iter().skip(i + 1) {
                let area = layout::overlap_area(&a.1, &b.1);
                if area > 1e-6 {
                    pairs.push(json!({"layers": [a.0.0, b.0.0], "area": r2(area), "fraction": [r4(area / a.2), r4(area / b.2)]}));
                }
            }
        }
        v["overlaps"] = json!(pairs);
        if quads.len() > MAX_OVERLAP_LAYERS {
            v["overlapsNote"] = json!(format!("overlaps computed for the first {MAX_OVERLAP_LAYERS} visible layers"));
        }
    }
    Ok(v)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![query!(
        "layer.bounds",
        "Layer Bounds",
        "{comp?, time?|frame?, layers?: [id|name|#n] (default every layer), pixels? (also measure the drawn pixels: masks, effects, styles), overlaps? (pairwise overlaps of the visible layers)} → per layer: visible + why, comp-space bounds [x,y,w,h], corners, center, anchorInComp, onScreen fraction, source rect",
        layer_bounds
    )]
}
