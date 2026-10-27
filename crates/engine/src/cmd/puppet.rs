//! Puppet tools: pins on the layer's Puppet effect (Position, Advanced, Bend, Starch and
//! Overlap pins), mesh options, and a query of the current mesh for overlays and agents.
//!
//! Pins are selected like properties (`puppet.selectPins`; Edit ▸ Clear / Delete removes the
//! selected pins, not their layer). Follow-Through (`puppet.follow`) makes pins trail a leader pin
//! with a delay, for hair, cloth and overlapping action.
//!
//! Pin positions are in effect space. A new pin is attached to the mesh under it: the click is
//! mapped back through the current deformation to the rest mesh (its hidden rest position), and
//! Position / Advanced pins get a Position keyframe at the current time, as in After Effects.

use effectcraft_effects::puppet::{self, MeshOpts, PinKind};
use effectcraft_effects::{Buf, Params};
use effectcraft_keyframe::{Keyframe, Value as KV};
use effectcraft_project::build::Ids;
use effectcraft_project::{ItemId, Layer, LayerId, Node, PropGroup, Uid};
use effectcraft_render::{EvalCtx, RenderOpts, Renderer};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::{CommandSpec, bad, f_p, has_comp, layer_mut, layer_p, merge_p, str_p};
use crate::{EngineError, Result, Session, cmd};

fn dist2(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}

fn pt(v: Option<&Value>) -> Option<[f64; 2]> {
    let a = v?.as_array()?;
    Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?])
}

/// The Puppet effect's input pixels and evaluated parameters at comp time `t`. The input comes
/// from the layer cache, shared: moving pins doesn't change it (#212).
pub fn puppet_eval(s: &Session, cid: ItemId, lid: LayerId, fx_uid: Uid, t: Tick) -> Option<(std::sync::Arc<Buf>, Params)> {
    let comp = s.project.comp(cid)?;
    let layer = comp.layer(lid)?;
    let fx = layer.effects()?;
    let index = fx.groups().position(|g| g.uid == fx_uid)?;
    let g = fx.groups().nth(index)?;
    let ctx = EvalCtx { project: &s.project, comp_id: cid, comp, time: t, expr: s.expr.as_deref(), footage: None };
    let mut r = Renderer::new(&s.project, &*s.footage, RenderOpts::default());
    r.expr = s.expr.as_deref();
    r.cache = Some(&s.layer_cache);
    let buf = r.layer_input(&ctx, layer, index)?;
    let params = effectcraft_effects::flatten_params(g, &mut |p| ctx.value(layer, p));
    Some((buf, params))
}

fn find_puppet(s: &Session, cid: ItemId, lid: LayerId) -> Option<Uid> {
    s.project.comp(cid)?.layer(lid)?.effects()?.groups().find(|g| puppet::is_puppet(g)).map(|g| g.uid)
}

fn snapped(s: &Session, cid: ItemId, p: &Value) -> Result<Tick> {
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    Ok(comp.frame_rate.snap_nearest(super::time_p(s, p, Some(comp))))
}

fn layer_time(s: &Session, cid: ItemId, lid: LayerId, t: Tick) -> Result<Tick> {
    let l = s.project.comp(cid).and_then(|c| c.layer(lid)).ok_or(EngineError::NoComp)?;
    Ok(l.layer_time(t))
}

fn add_pin(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "puppet.addPin";
    let (cid, lid) = layer_p(s, p, cmd)?;
    let kind = match str_p(p, "kind") {
        Some(k) => PinKind::from_name(k).ok_or_else(|| bad(cmd, format!("unknown kind `{k}` (position | advanced | bend | starch | overlap)")))?,
        None => PinKind::Position,
    };
    let at = pt(p.get("position")).ok_or_else(|| bad(cmd, "missing `position` [x, y] (effect space)"))?;
    let t = snapped(s, cid, p)?;
    let lt = layer_time(s, cid, lid, t)?;
    {
        let l = s.project.comp(cid).and_then(|c| c.layer(lid)).ok_or(EngineError::NoComp)?;
        if l.effects().is_none() || !l.source.is_av() {
            return Err(bad(cmd, "the Puppet tools work on footage, solid, text, shape and precomp layers"));
        }
    }
    // Which mesh: the one under the click (through the current deformation), else a new one.
    let fx_uid = find_puppet(s, cid, lid);
    let want_mesh = p.get("mesh").and_then(Value::as_u64);
    let new_mesh = p.get("newMesh").and_then(Value::as_bool).unwrap_or(false);
    // (mesh, the pin's rest point, where it is now)
    let mut target: Option<(Uid, [f64; 2], [f64; 2])> = None;
    if let Some(fu) = fx_uid
        && !new_mesh
        && let Some((buf, params)) = puppet_eval(s, cid, lid, fu, t)
    {
        let meshes = puppet::overlay(&buf, &params);
        for (uid, mesh, def, _) in &meshes {
            if want_mesh.is_some_and(|w| w != *uid) || mesh.tris.is_empty() {
                continue;
            }
            let inside = puppet::contains(mesh, def, at);
            let near = def.iter().map(|v| (v[0] - at[0]).hypot(v[1] - at[1])).fold(f64::MAX, f64::min) < 12.0;
            if inside || near || want_mesh.is_some() {
                let rest = puppet::unmap(mesh, def, at);
                // A moving pin holds its nearest mesh vertex at `position + (vertex - rest)`
                // (puppet::solve). Choose the position that keeps that vertex where it is now, so
                // adding a pin to a bent mesh moves nothing (After Effects, #318); on an unbent
                // mesh that is the click itself.
                let now = (0..mesh.verts.len().min(def.len()))
                    .filter(|_| kind.moves())
                    .min_by(|a, b| dist2(mesh.verts[*a], rest).total_cmp(&dist2(mesh.verts[*b], rest)))
                    .and_then(|v| {
                        let (r, d) = (mesh.verts.get(v)?, def.get(v)?);
                        Some([d[0] - (r[0] - rest[0]), d[1] - (r[1] - rest[1])])
                    })
                    // Round-off only (an unbent mesh): exactly the click.
                    .filter(|n| dist2(*n, at) > 1e-12)
                    .unwrap_or(at);
                target = Some((*uid, rest, now));
                break;
            }
        }
        if target.is_none()
            && let Some(w) = want_mesh
        {
            return Err(bad(cmd, format!("no mesh {w}")));
        }
    }
    let opts = MeshOpts {
        density: f_p(p, "density").unwrap_or(s.state.puppet.density),
        expansion: f_p(p, "expansion").unwrap_or(s.state.puppet.expansion),
        triangles: f_p(p, "triangles").unwrap_or(s.state.puppet.triangles),
    };
    let size = {
        let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
        let l = comp.layer(lid).ok_or(EngineError::NoComp)?;
        effectcraft_render::effect_bounds(&s.project, comp, l).0
    };
    let label = match kind {
        PinKind::Starch => "Add Starch Pin",
        PinKind::Overlap => "Add Overlap Pin",
        _ => "Add Puppet Pin",
    };
    let (fx, mesh, pin) = s.edit(label, None, |proj, st| {
        let mut next = proj.next_id;
        let l = layer_mut(proj, cid, lid)?;
        let fxg = l.props.sub_mut("effects").ok_or_else(|| bad(cmd, "no effects"))?;
        let mut ids = Ids(&mut next);
        let fx_uid = match fx_uid {
            Some(u) => u,
            None => {
                let spec = effectcraft_effects::find(puppet::ID).ok_or_else(|| EngineError::Other("Puppet effect missing".into()))?;
                let g = effectcraft_effects::instantiate(spec, &mut ids, "Puppet", size);
                let u = g.uid;
                fxg.children.push(g.into());
                u
            }
        };
        let pg = fxg.find_group_mut(fx_uid).ok_or_else(|| bad(cmd, "Puppet effect gone"))?;
        let (mesh_uid, rest, now) = match target {
            Some(t) => t,
            None => {
                let n = puppet::meshes(pg).count();
                let g = puppet::mesh_group(&mut ids, &format!("Mesh {}", n + 1), at, &opts);
                let u = g.uid;
                pg.children.push(g.into());
                (u, at, at)
            }
        };
        let name = puppet::next_pin_name(pg, kind);
        let mg = pg.find_group_mut(mesh_uid).ok_or_else(|| bad(cmd, "mesh gone"))?;
        let mut pin = puppet::pin_group(&mut ids, &name, kind, rest);
        if let Some(pos) = pin.get_mut("position") {
            if kind.moves() {
                pos.value = KV::Vec2(now);
                pos.set_animated(true, lt);
            } else {
                pos.value = KV::Vec2(rest);
            }
        }
        let pin_uid = pin.uid;
        let deform = mg.sub_mut("deform").ok_or_else(|| bad(cmd, "mesh has no Deform group"))?;
        deform.children.push(pin.into());
        proj.next_id = next;
        st.selected_props = vec![(lid, pin_uid)];
        Ok((fx_uid, mesh_uid, pin_uid))
    })?;
    Ok(json!({"effect": fx, "mesh": mesh, "pin": pin, "rest": [target.map(|x| x.1).unwrap_or(at)[0], target.map(|x| x.1).unwrap_or(at)[1]]}))
}

/// The pin groups of a layer's Puppet effects, in stack order.
pub(crate) fn layer_pins(l: &Layer) -> impl Iterator<Item = &PropGroup> {
    l.effects().into_iter().flat_map(|fx| fx.groups().filter(|g| puppet::is_puppet(g))).flat_map(puppet::meshes).flat_map(puppet::pins)
}

/// Selected puppet pins of the active comp: (layer, pin uid).
pub(crate) fn selected_pins(s: &Session) -> Vec<(LayerId, Uid)> {
    let Some(c) = s.active_comp() else { return vec![] };
    s.state.selected_props.iter().filter(|(l, u)| c.layer(*l).is_some_and(|l| layer_pins(l).any(|g| g.uid == *u))).copied().collect()
}

/// Edit ▸ Select All with puppet pins selected: every pin of the selected pins' kinds on their
/// layers. `None` when no pin is selected (Select All then selects layers).
pub(crate) fn select_all_of_kind(s: &mut Session) -> Option<Value> {
    let sel = selected_pins(s);
    let comp = s.active_comp()?;
    let kind = |g: &PropGroup| PinKind::from_index(g.get("kind").map(|k| k.value.as_enum()).unwrap_or(0));
    let mut kinds = vec![];
    for (l, u) in &sel {
        if let Some(g) = comp.layer(*l).and_then(|l| l.props.find_group(*u)) {
            kinds.push((*l, kind(g)));
        }
    }
    if kinds.is_empty() {
        return None;
    }
    let all: Vec<(LayerId, Uid)> =
        comp.layers.iter().flat_map(|l| layer_pins(l).filter(|g| kinds.contains(&(l.id, kind(g)))).map(|g| (l.id, g.uid)).collect::<Vec<_>>()).collect();
    s.state.selected_props = all.clone();
    Some(json!({ "pins": all.iter().map(|(_, u)| *u).collect::<Vec<_>>() }))
}

/// Find a pin group (uid or name) in the layer's Puppet effects.
fn pin_uid(s: &Session, cid: ItemId, lid: LayerId, key: &Value, cmd: &str) -> Result<Uid> {
    let l = s.project.comp(cid).and_then(|c| c.layer(lid)).ok_or(EngineError::NoComp)?;
    let hit = |pin: &&PropGroup| match key {
        Value::Number(n) => n.as_u64() == Some(pin.uid),
        Value::String(name) => &pin.name == name,
        _ => false,
    };
    layer_pins(l).find(hit).map(|g| g.uid).ok_or_else(|| bad(cmd, format!("no puppet pin {key}")))
}

/// Pins named by `pin` or `pins` (uids or names) on `layer`, else the selected pins (of `layer`
/// when given): (comp, layer, pin uid).
pub(crate) fn pins_p(s: &Session, p: &Value, cmd: &str) -> Result<Vec<(ItemId, LayerId, Uid)>> {
    let keys: Vec<Value> = match (p.get("pin"), p.get("pins")) {
        (Some(k), _) => vec![k.clone()],
        (None, Some(Value::Array(a))) => a.clone(),
        (None, Some(_)) => return Err(bad(cmd, "`pins` must be an array of pin uids or names")),
        (None, None) => vec![],
    };
    if !keys.is_empty() {
        let (cid, lid) = layer_p(s, p, cmd)?;
        return keys.iter().map(|k| pin_uid(s, cid, lid, k, cmd).map(|u| (cid, lid, u))).collect();
    }
    let cid = s.active_comp_id().ok_or(EngineError::NoComp)?;
    let only = match p.get("layer") {
        Some(_) => Some(layer_p(s, p, cmd)?.1),
        None => None,
    };
    let sel: Vec<_> = selected_pins(s).into_iter().filter(|(l, _)| only.is_none_or(|o| o == *l)).map(|(l, u)| (cid, l, u)).collect();
    if sel.is_empty() {
        return Err(bad(cmd, "missing `pin` / `pins` (uids or names), and no puppet pins are selected"));
    }
    Ok(sel)
}

/// Select pins (the viewer's click / Shift-click on a pin). Plain: only these pins; `add`: add
/// them; `toggle`: flip each. An empty `pins` deselects every pin.
fn select_pins(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "puppet.selectPins";
    let want = match p.get("pins") {
        Some(Value::Array(a)) if a.is_empty() => vec![],
        _ => pins_p(s, p, cmd)?,
    };
    let (add, toggle) = (super::b_p(p, "add").unwrap_or(false), super::b_p(p, "toggle").unwrap_or(false));
    let st = &mut s.state;
    if !add && !toggle {
        st.selected_props.clear();
        st.selected_keys.clear();
    }
    for (_, l, u) in want {
        let at = st.selected_props.iter().position(|x| *x == (l, u));
        match at {
            Some(i) if toggle => {
                st.selected_props.remove(i);
            }
            Some(_) => {}
            None => st.selected_props.push((l, u)),
        }
        if !st.selected_layers.contains(&l) {
            st.selected_layers.push(l);
        }
    }
    Ok(json!({ "pins": selected_pins(s).iter().map(|(_, u)| *u).collect::<Vec<_>>() }))
}

fn with_pin<T>(s: &mut Session, p: &Value, cmd: &str, label: &str, f: impl FnOnce(&mut PropGroup, Tick) -> Result<T>) -> Result<T> {
    let (cid, lid) = layer_p(s, p, cmd)?;
    let key = p.get("pin").cloned().ok_or_else(|| bad(cmd, "missing `pin` (uid or name)"))?;
    let uid = pin_uid(s, cid, lid, &key, cmd)?;
    let t = snapped(s, cid, p)?;
    let lt = layer_time(s, cid, lid, t)?;
    s.edit(label, merge_p(p), |proj, st| {
        let l = layer_mut(proj, cid, lid)?;
        let g = l.props.find_group_mut(uid).ok_or_else(|| bad(cmd, "pin gone"))?;
        let r = f(g, lt)?;
        st.selected_props = vec![(lid, uid)];
        Ok(r)
    })
}

fn move_pin(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "puppet.movePin";
    let to = pt(p.get("position")).ok_or_else(|| bad(cmd, "missing `position` [x, y] (effect space)"))?;
    with_pin(s, p, cmd, "Move Puppet Pin", |g, lt| {
        let kind = PinKind::from_index(g.get("kind").map(|k| k.value.as_enum()).unwrap_or(0));
        let pos = g.get_mut("position").ok_or_else(|| bad(cmd, "Bend pins have no position (use puppet.setPin rotation/scale)"))?;
        pos.set_value_at(lt, KV::Vec2(to));
        // Starch and Overlap pins sit on the rest mesh: moving them moves their region.
        if !kind.moves()
            && let Some(r) = g.get_mut("rest")
        {
            r.value = KV::Vec2(to);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

fn set_pin(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "puppet.setPin";
    let fields: Vec<(&str, KV)> = [("scale", "scale"), ("rotation", "rotation"), ("amount", "amount"), ("extent", "extent"), ("inFront", "in_front")]
        .iter()
        .filter_map(|(k, m)| f_p(p, k).map(|v| (*m, KV::Scalar(v))))
        .chain(pt(p.get("position")).map(|v| ("position", KV::Vec2(v))))
        .collect();
    if fields.is_empty() {
        return Err(bad(cmd, "nothing to set (position, scale, rotation, amount, extent, inFront)"));
    }
    with_pin(s, p, cmd, "Edit Puppet Pin", |g, lt| {
        for (m, v) in fields {
            let pr = g.get_mut(m).ok_or_else(|| bad(cmd, format!("this pin has no `{m}`")))?;
            pr.set_value_at(lt, v);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

fn remove_pin(s: &mut Session, p: &Value) -> Result<Value> {
    let pins = pins_p(s, p, "puppet.removePin")?;
    let label = if pins.len() == 1 { "Delete Puppet Pin" } else { "Delete Puppet Pins" };
    s.edit(label, None, |proj, st| {
        for (cid, lid, uid) in &pins {
            let l = layer_mut(proj, *cid, *lid)?;
            if let Some(parent) = l.props.parent_of_mut(*uid) {
                parent.children.retain(|c| c.uid() != *uid);
            }
            st.selected_props.retain(|(_, u)| u != uid);
        }
        Ok(())
    })?;
    Ok(json!({ "removed": pins.len() }))
}

fn mesh_opts(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "puppet.mesh";
    let fields: Vec<(&str, f64)> = ["density", "expansion", "triangles"].iter().filter_map(|k| f_p(p, k).map(|v| (*k, v))).collect();
    if let Some(b) = p.get("showMesh").and_then(Value::as_bool) {
        s.state.puppet.show_mesh = b;
    }
    if fields.is_empty() {
        return Ok(serde_json::to_value(&s.state.puppet).unwrap_or(Value::Null));
    }
    // Without a layer: set the tool defaults for new meshes.
    let has_layer = p.get("layer").is_some() || !s.state.selected_layers.is_empty();
    let (cid, lid) = match layer_p(s, p, cmd) {
        Ok(x) => x,
        Err(_) if !has_layer => {
            for (k, v) in &fields {
                match *k {
                    "density" => s.state.puppet.density = v.clamp(0.0, 100.0),
                    "expansion" => s.state.puppet.expansion = *v,
                    _ => s.state.puppet.triangles = v.max(10.0),
                }
            }
            return Ok(serde_json::to_value(&s.state.puppet).unwrap_or(Value::Null));
        }
        Err(e) => return Err(e),
    };
    let fx = find_puppet(s, cid, lid).ok_or_else(|| bad(cmd, "the layer has no Puppet effect (add a pin first)"))?;
    let want = p.get("mesh").and_then(Value::as_u64);
    let t = snapped(s, cid, p)?;
    let lt = layer_time(s, cid, lid, t)?;
    for (k, v) in &fields {
        match *k {
            "density" => s.state.puppet.density = v.clamp(0.0, 100.0),
            "expansion" => s.state.puppet.expansion = *v,
            _ => s.state.puppet.triangles = v.max(10.0),
        }
    }
    s.edit("Puppet Mesh", merge_p(p), |proj, _| {
        let l = layer_mut(proj, cid, lid)?;
        let g = l.props.find_group_mut(fx).ok_or_else(|| bad(cmd, "Puppet effect gone"))?;
        let mut done = 0;
        for c in g.children.iter_mut() {
            let Node::Group(m) = c else { continue };
            if m.match_id != "mesh" || want.is_some_and(|w| w != m.uid) {
                continue;
            }
            for (k, v) in &fields {
                if let Some(pr) = m.get_mut(k) {
                    pr.set_value_at(lt, KV::Scalar(*v));
                }
            }
            done += 1;
        }
        if done == 0 { Err(bad(cmd, "no such mesh")) } else { Ok(()) }
    })?;
    Ok(Value::Null)
}

/// Parse recorded samples `[[t, x, y]…]` (t = seconds since the drag began, effect space).
fn samples(p: &Value, cmd: &str) -> Result<Vec<(f64, [f64; 2])>> {
    let arr = p
        .get("samples")
        .and_then(Value::as_array)
        .ok_or_else(|| bad(cmd, "missing `samples`: [[t, x, y]…] (t = seconds since the drag began, effect space)"))?;
    let mut rec = Vec::with_capacity(arr.len());
    for (i, v) in arr.iter().enumerate() {
        let a: Vec<f64> = v.as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
        if a.len() < 3 || a.iter().any(|x| !x.is_finite()) {
            return Err(bad(cmd, format!("sample {i}: expected [t, x, y]")));
        }
        rec.push((a[0], [a[1], a[2]]));
    }
    if rec.is_empty() {
        return Err(bad(cmd, "need at least one sample"));
    }
    rec.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(rec)
}

/// Puppet tool Record mode (⌘/Ctrl-drag a pin): the pin's drag, sampled in real time, becomes
/// Position keyframes at the comp frame rate from the current time on, replacing the keys in
/// the recorded span. Record Options: `speed` (%: 100 plays back at the speed of the drag,
/// 200 twice as slow) and `smoothing` (removes extraneous keys like the Smoother; 0 = a key
/// on every frame). The current time stays where recording began.
fn record_pin(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "puppet.recordPin";
    let (cid, lid) = layer_p(s, p, cmd)?;
    let key = p.get("pin").cloned().ok_or_else(|| bad(cmd, "missing `pin` (uid or name)"))?;
    let uid = pin_uid(s, cid, lid, &key, cmd)?;
    let rec = samples(p, cmd)?;
    // Several selected pins dragged together (`pins`): each follows the drag's displacement from
    // where it is when recording begins; one undo step.
    if let Some(Value::Array(others)) = p.get("pins")
        && !others.is_empty()
    {
        let start = match p.get("start").and_then(Value::as_f64) {
            Some(t) => Tick::from_seconds_f64(t),
            None => s.time_of(cid),
        };
        let lt = layer_time(s, cid, lid, start)?;
        let mut pins = vec![(uid, rec[0].1)];
        for k in others {
            let u = pin_uid(s, cid, lid, k, cmd)?;
            if pins.iter().any(|(x, _)| *x == u) {
                continue;
            }
            let l = s.project.comp(cid).and_then(|c| c.layer(lid)).ok_or(EngineError::NoComp)?;
            let at = l.props.find_group(u).and_then(|g| g.get("position")).map(|pr| pr.value_at(lt).as_vec2());
            pins.push((u, at.ok_or_else(|| bad(cmd, "only Position and Advanced pins record motion"))?));
        }
        let o = rec[0].1;
        return super::app_more::grouped(s, "Record Puppet Pins", |s| {
            let mut last = Value::Null;
            for (u, base) in &pins {
                let moved: Vec<Value> = rec.iter().map(|(t, q)| json!([t, base[0] + q[0] - o[0], base[1] + q[1] - o[1]])).collect();
                let mut one = p.clone();
                if let Some(m) = one.as_object_mut() {
                    m.remove("pins");
                    m.insert("pin".into(), json!(u));
                    m.insert("samples".into(), Value::Array(moved));
                }
                last = s.execute(cmd, one)?;
            }
            last["pins"] = json!(pins.iter().map(|(u, _)| *u).collect::<Vec<_>>());
            Ok(last)
        });
    }
    let opts = &s.state.puppet;
    let speed = f_p(p, "speed").unwrap_or(opts.record_speed).clamp(1.0, 10_000.0) / 100.0;
    // Smoothing 10 (the default) keeps the path within 1 layer pixel of the drag.
    let smoothing = f_p(p, "smoothing").unwrap_or(opts.record_smoothing).max(0.0) / 10.0;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let fr = comp.frame_rate;
    let fd = fr.frame_duration().seconds();
    let start = match p.get("start").and_then(Value::as_f64) {
        Some(t) => fr.snap_nearest(Tick::from_seconds_f64(t)),
        None => s.time_of(cid),
    };
    let f0 = fr.frame_at(start);
    let end_frame = fr.frame_at(comp.duration - comp.frame_duration());
    let t_rec0 = rec[0].0;
    let span = (rec[rec.len() - 1].0 - t_rec0) * speed;
    let nframes = (span / fd).round().max(0.0) as i64;
    let at = |secs: f64| {
        let i = rec.partition_point(|r| r.0 <= secs).clamp(1, rec.len().max(2) - 1);
        if rec.len() == 1 {
            return rec[0].1;
        }
        let (a, b) = (rec[i - 1], rec[i]);
        let f = if b.0 > a.0 { ((secs - a.0) / (b.0 - a.0)).clamp(0.0, 1.0) } else { 1.0 };
        [a.1[0] + (b.1[0] - a.1[0]) * f, a.1[1] + (b.1[1] - a.1[1]) * f]
    };
    let n = s.edit("Record Puppet Pin", None, |proj, st| {
        let l = layer_mut(proj, cid, lid)?;
        let lc = l.clone();
        let g = l.props.find_group_mut(uid).ok_or_else(|| bad(cmd, "pin gone"))?;
        let kind = PinKind::from_index(g.get("kind").map(|k| k.value.as_enum()).unwrap_or(0));
        if !kind.moves() {
            return Err(bad(cmd, "only Position and Advanced pins record motion"));
        }
        let pos = g.get_mut("position").ok_or_else(|| bad(cmd, "the pin has no Position"))?;
        let mut new: Vec<Keyframe> = vec![];
        for k in 0..=nframes {
            let f = f0 + k;
            if f > end_frame {
                break;
            }
            let q = at(t_rec0 + k as f64 * fd / speed);
            new.push(Keyframe::new(lc.layer_time(fr.tick_of(f)), KV::Vec2(q)));
        }
        let (Some(t0), Some(t1)) = (new.first().map(|k| k.time), new.last().map(|k| k.time)) else { return Ok(0) };
        let mut keys: Vec<Keyframe> = pos.keys.iter().filter(|k| k.time < t0 || k.time > t1).cloned().collect();
        keys.extend(new);
        keys.sort_by_key(|k| k.time);
        let (a, b) = (keys.iter().position(|k| k.time == t0).unwrap_or(0), keys.iter().position(|k| k.time == t1).unwrap_or(0));
        if smoothing > 0.0 && b > a + 1 {
            let samples: Vec<(Tick, Vec<f64>)> = keys[a..=b].iter().map(|k| (k.time, k.value.components())).collect();
            keys = super::anim_tools::smooth_keys(&keys, a, b, &samples, smoothing, true);
        }
        pos.keys = keys;
        pos.value = KV::Vec2(rec[0].1);
        let puid = pos.uid;
        st.selected_props = vec![(lid, uid)];
        st.selected_keys = pos.keys.iter().filter(|k| k.time >= t0 && k.time <= t1).map(|k| crate::KeyRef { layer: lid, prop: puid, time: k.time }).collect();
        Ok(st.selected_keys.len())
    })?;
    // The current time returns to where recording began.
    s.state.times.insert(cid, start);
    let end = fr.tick_of((f0 + nframes).min(end_frame));
    Ok(json!({"keys": n, "start": start.seconds(), "end": end.seconds(), "frames": nframes + 1}))
}

/// Follow-Through (hair, cloth, tails, overlapping action): each follower pin trails the leader
/// pin's motion by a delay, through an expression on its Position. The followers nearest the
/// leader trail least; with `cascade` the k-th nearest trails k × `delay`, otherwise all trail by
/// `delay`. `amount` (%) scales the motion they inherit; their own keyframes still add on top.
/// The leader is `leader`, else the first selected pin; the followers are `pins`, else the other
/// selected pins of the leader's layer. One undo step.
fn follow(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "puppet.follow";
    let (cid, lid, leader, mut followers) = match p.get("leader") {
        Some(k) => {
            let (cid, lid) = layer_p(s, p, cmd)?;
            let leader = pin_uid(s, cid, lid, k, cmd)?;
            let rest: Vec<Uid> = pins_p(s, p, cmd)?.into_iter().filter(|(_, l, u)| *l == lid && *u != leader).map(|x| x.2).collect();
            (cid, lid, leader, rest)
        }
        None => {
            let sel = pins_p(s, p, cmd)?;
            let (cid, lid, leader) = sel[0];
            (cid, lid, leader, sel.iter().skip(1).filter(|(_, l, _)| *l == lid).map(|x| x.2).collect())
        }
    };
    followers.dedup();
    if followers.is_empty() {
        return Err(bad(cmd, "select the leader pin first, then the pins that follow it (or pass `leader` and `pins`)"));
    }
    let delay = f_p(p, "delay").unwrap_or(0.1).clamp(0.0, 10.0);
    let amount = f_p(p, "amount").unwrap_or(100.0) / 100.0;
    let cascade = super::b_p(p, "cascade").unwrap_or(true);
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?;
    // (Position uid, rest) of a pin that moves.
    let moving = |uid: Uid| -> Result<(Uid, [f64; 2], String)> {
        let g = layer.props.find_group(uid).ok_or_else(|| bad(cmd, "pin gone"))?;
        if !PinKind::from_index(g.get("kind").map(|k| k.value.as_enum()).unwrap_or(0)).moves() {
            return Err(bad(cmd, format!("`{}` has no Position (only Position and Advanced pins follow or lead)", g.name)));
        }
        let pos = g.get("position").ok_or_else(|| bad(cmd, "the pin has no Position"))?;
        Ok((pos.uid, g.get("rest").map(|r| r.value.as_vec2()).unwrap_or([0.0; 2]), g.name.clone()))
    };
    let (lead_pos, lead_rest, lead_name) = moving(leader)?;
    let reference = super::link::reference(comp, layer, layer, lead_pos, true).ok_or_else(|| bad(cmd, "can't reference the leader pin"))?;
    let mut rigs = followers.iter().map(|u| moving(*u).map(|(pos, rest, _)| (*u, pos, rest))).collect::<Result<Vec<_>>>()?;
    let dist = |r: [f64; 2]| (r[0] - lead_rest[0]).hypot(r[1] - lead_rest[1]);
    rigs.sort_by(|a, b| dist(a.2).total_cmp(&dist(b.2)));
    let out = super::app_more::grouped(s, "Follow-Through", |s| {
        let mut out = vec![];
        for (k, (pin, pos, _)) in rigs.iter().enumerate() {
            let d = if cascade { delay * (k + 1) as f64 } else { delay };
            let e = format!(
                "// Follow-Through: trails {} by {d} s\nvar lead = {reference};\nvalue + (lead.valueAtTime(time - {d}) - [{}, {}]) * {amount}",
                serde_json::to_string(&lead_name).unwrap_or_default(),
                lead_rest[0],
                lead_rest[1]
            );
            s.execute("prop.setExpression", json!({"comp": cid.0, "layer": lid.0, "prop": pos, "expression": e}))?;
            out.push(json!({"pin": pin, "delay": d}));
        }
        Ok(out)
    })?;
    Ok(json!({"leader": leader, "pins": out}))
}

/// Record Options (the Puppet tool's Record Options… dialog): `speed`, `smoothing`,
/// `useDraftDeformation`, `showMesh`. Returns the options.
fn record_options(s: &mut Session, p: &Value) -> Result<Value> {
    let o = &mut s.state.puppet;
    if let Some(v) = f_p(p, "speed") {
        o.record_speed = v.clamp(1.0, 10_000.0);
    }
    if let Some(v) = f_p(p, "smoothing") {
        o.record_smoothing = v.clamp(0.0, 100.0);
    }
    if let Some(v) = p.get("useDraftDeformation").and_then(Value::as_bool) {
        o.record_draft = v;
    }
    if let Some(v) = p.get("showMesh").and_then(Value::as_bool) {
        o.record_show_mesh = v;
    }
    Ok(json!({"speed": o.record_speed, "smoothing": o.record_smoothing, "useDraftDeformation": o.record_draft, "showMesh": o.record_show_mesh}))
}

/// Meshes, pins and the deformed mesh at the current time (effect space).
fn info(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid) = layer_p(s, p, "puppet.info")?;
    let Some(fx) = find_puppet(s, cid, lid) else { return Ok(json!({"meshes": []})) };
    let t = snapped(s, cid, p)?;
    let (buf, params) = puppet_eval(s, cid, lid, fx, t).ok_or_else(|| bad("puppet.info", "the layer has no pixels"))?;
    let meshes: Vec<Value> = puppet::overlay(&buf, &params)
        .into_iter()
        .map(|(uid, mesh, def, pins)| {
            json!({
                "mesh": uid,
                "triangles": mesh.tris.len(),
                "vertices": mesh.verts.len(),
                "outline": mesh.outline,
                "deformed": def,
                "tris": mesh.tris,
                "pins": pins.iter().map(|pn| json!({"pin": pn.uid, "kind": pn.kind.name(), "rest": pn.rest, "position": pn.position})).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(json!({"effect": fx, "meshes": meshes}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "puppet.addPin",
            "Add Puppet Pin",
            [],
            None,
            "{layer?, kind?: position|advanced|bend|starch|overlap, position: [x,y] (effect space), time? (s) | frame?, mesh?: uid, newMesh?, density?, expansion?, triangles?}",
            has_comp,
            add_pin
        ),
        cmd!("puppet.movePin", "Move Puppet Pin", [], None, "{layer?, pin: uid | name, position: [x,y], time? | frame?, merge?}", has_comp, move_pin),
        cmd!(
            "puppet.setPin",
            "Edit Puppet Pin",
            [],
            None,
            "{layer?, pin: uid | name, position?, scale? (%), rotation? (°), amount? (%), extent? (px), inFront?, time? | frame?, merge?}",
            has_comp,
            set_pin
        ),
        cmd!(
            "puppet.removePin",
            "Delete Puppet Pin",
            [],
            None,
            "{layer?, pin?: uid | name, pins?: [uid | name…]} (default: the selected pins)",
            has_comp,
            remove_pin
        ),
        cmd!("puppet.selectPins", "Select Puppet Pins", [], None, "{layer?, pins: [uid | name…] ([] deselects), add?, toggle?}", has_comp, select_pins),
        cmd!(
            "puppet.mesh",
            "Puppet Mesh Options",
            [],
            None,
            "{layer?, mesh?: uid, density?, expansion?, triangles?, showMesh?, time?, merge?}",
            super::always,
            mesh_opts
        ),
        crate::query!("puppet.info", "Puppet Mesh Info", "{layer?, time? | frame?}", info),
        cmd!(
            "puppet.recordPin",
            "Record Puppet Pin",
            [],
            None,
            "{layer?, pin: uid | name, samples: [[t, x, y]…] (t = seconds since the drag began, effect space), pins?: [uid | name…] (more pins that move by the same displacement), start? (s, default current time), speed? (%, Record Options), smoothing? (Record Options)}",
            has_comp,
            record_pin
        ),
        cmd!(
            "puppet.follow",
            "Follow-Through...",
            [],
            None,
            "{layer?, leader?: uid | name (default: the first selected pin), pins?: [uid | name…] (default: the other selected pins), delay? (s, 0.1), amount? (%, 100), cascade? (true: the k-th nearest pin trails k × delay)}",
            has_comp,
            follow
        ),
        cmd!(
            "puppet.recordOptions",
            "Record Options...",
            [],
            None,
            "{speed? (%, 100), smoothing? (0-100), useDraftDeformation?, showMesh?}",
            super::always,
            record_options
        ),
    ]
}

/// Puppet tool options (the Tools bar's Mesh: Show / Expansion / Density for new meshes).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PuppetOptions {
    pub density: f64,
    pub expansion: f64,
    pub triangles: f64,
    /// Show the mesh in the viewer.
    pub show_mesh: bool,
    /// Record Options ▸ Speed (%): 100 plays a recording back at the speed of the drag.
    pub record_speed: f64,
    /// Record Options ▸ Smoothing: removes extraneous keys from recorded motion (0 = a key per
    /// frame).
    pub record_smoothing: f64,
    /// Record Options ▸ Use Draft Deformation: the viewer shows the draft (unexpanded) mesh
    /// while recording.
    pub record_draft: bool,
    /// Record Options ▸ Show Mesh while recording.
    pub record_show_mesh: bool,
}

impl Default for PuppetOptions {
    fn default() -> Self {
        let d = MeshOpts::default();
        PuppetOptions {
            density: d.density,
            expansion: d.expansion,
            triangles: d.triangles,
            show_mesh: false,
            record_speed: 100.0,
            record_smoothing: 10.0,
            record_draft: false,
            record_show_mesh: false,
        }
    }
}
