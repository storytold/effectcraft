use effectcraft_project::TrackMatte;

use super::{
    EngineError, ItemKind, Label, Layer, LayerId, LayerSource, Result, Session, Tick, Value, b_p, bad, build, f_p, json, layer_mut, layers_p, merge_p,
    resolve_layer, str_p, unlocked, with,
};

pub(super) fn timing(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let t = s.time();
    let op = str_p(p, "op").unwrap_or("set").to_string();
    // Layer times stay on frame boundaries of the comp (as in AE).
    let fr = s.project.comp(cid).ok_or(EngineError::NoComp)?.frame_rate;
    let snap = |k: &str| f_p(p, k).map(|v| fr.snap_nearest(Tick::from_seconds_f64(v)));
    let (delta, start, inp, outp) = (snap("delta"), snap("start"), snap("in"), snap("out"));
    // Relative trims: each layer moves its own in/out point by the same amount, so dragging an
    // edge of several selected layers trims them all together (as in AE).
    let (in_delta, out_delta) = (snap("inDelta"), snap("outDelta"));
    s.edit("Layer Timing", merge_p(p), |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let fd = comp.frame_duration();
        let comp_dur = comp.duration;
        for l in comp.layers.iter_mut_where(|l| ids.contains(&l.id)) {
            let shift = |l: &mut Layer, d: Tick| {
                l.start_time += d;
                l.in_point += d;
                l.out_point += d;
            };
            match op.as_str() {
                "moveInToTime" => {
                    let d = t - l.in_point;
                    shift(l, d);
                }
                "moveOutToTime" => {
                    let d = t - l.out_point;
                    shift(l, d);
                }
                "moveInToStart" => {
                    let d = Tick::ZERO - l.in_point;
                    shift(l, d);
                }
                "moveOutToEnd" => {
                    let d = comp_dur - l.out_point;
                    shift(l, d);
                }
                "trimInToTime" => l.in_point = t.min(l.out_point - fd),
                // Alt+]: the Out edge lands one frame after the playhead, so its frame stays the
                // layer's last visible one (as in After Effects), never before the In point.
                "trimOutToTime" => l.out_point = t.max(l.in_point) + fd,
                _ => {
                    if let Some(d) = delta {
                        shift(l, d);
                    }
                    if let Some(st) = start {
                        let d = st - l.start_time;
                        shift(l, d);
                    }
                    if let Some(i) = inp {
                        l.in_point = i.min(l.out_point - fd);
                    }
                    if let Some(o) = outp {
                        l.out_point = o.max(l.in_point + fd);
                    }
                    // Stops at the comp start/end (a point already past it doesn't jump onto
                    // it), keeping at least one frame of layer.
                    if let Some(d) = in_delta {
                        l.in_point = (l.in_point + d).max(Tick::ZERO.min(l.in_point)).min(l.out_point - fd);
                    }
                    if let Some(d) = out_delta {
                        l.out_point = (l.out_point + d).min(comp_dur.max(l.out_point)).max(l.in_point + fd);
                    }
                }
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Move the layers `frames` frames in time (Alt+Page Up / Page Down, with Shift ten frames).
pub(super) fn move_frames(s: &mut Session, p: &Value, frames: i64) -> Result<Value> {
    let (cid, _) = layers_p(s, p)?;
    let delta = s.project.comp(cid).ok_or(EngineError::NoComp)?.frame_rate.tick_of(frames).seconds();
    timing(s, &with(p, "delta", json!(delta)))
}

/// Slip edit: move a layer's source (footage or precomp) under its fixed in and out points, like
/// dragging the source-duration bar in After Effects. `delta` in seconds or `frames`; positive
/// moves the source later. Clamped so the source still covers the in–out span: the result gives
/// the `requested` shift and, per layer, the `delta` applied and whether it was `clamped` (#2371).
/// Keyframes and layer markers are in layer time, so they travel with the source.
pub(super) fn slip(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let fr = comp.frame_rate;
    let delta = match (p.get("frames").and_then(Value::as_i64), f_p(p, "delta")) {
        (Some(f), _) => fr.tick_of(f),
        (None, Some(d)) => fr.snap_nearest(Tick::from_seconds_f64(d)),
        _ => return Err(bad("layer.slip", "delta (seconds) or frames required")),
    };
    // Allowed shift per layer: the source span [lo, hi] in comp time must keep covering in..out.
    let mut shifts: Vec<(LayerId, Tick)> = Vec::new();
    for l in comp.layers.iter().filter(|l| ids.contains(&l.id)) {
        if l.props.get("timeRemap").is_some() {
            continue;
        }
        let dur = match &l.source {
            LayerSource::Footage { item } => match s.project.item(*item).map(|i| &i.kind) {
                Some(ItemKind::Footage(f)) if f.kind != effectcraft_project::FootageKind::Still => Some(f.looped_duration()),
                _ => None,
            },
            LayerSource::Comp { item } => s.project.comp(*item).map(|c| c.duration),
            _ => None,
        };
        let Some(dur) = dur else { continue };
        let (a, b) = (l.comp_time(Tick::ZERO), l.comp_time(dur));
        let (lo, hi) = (a.min(b), a.max(b));
        let (min_d, max_d) = (l.out_point - hi, l.in_point - lo);
        if min_d > max_d {
            continue;
        }
        shifts.push((l.id, delta.max(min_d).min(max_d)));
    }
    if shifts.is_empty() {
        return Err(bad("layer.slip", "select a footage or precomp layer (without time remapping)"));
    }
    s.edit("Slip Edit", merge_p(p), |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        for (id, d) in &shifts {
            if let Some(l) = comp.layer_mut(*id) {
                l.start_time += *d;
            }
        }
        Ok(())
    })?;
    Ok(json!({
        "requested": delta.seconds(),
        "slipped": shifts.iter().map(|(id, d)| json!({"layer": id.0, "delta": d.seconds(), "clamped": *d != delta})).collect::<Vec<_>>(),
    }))
}

pub(super) fn arrange(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let ids = unlocked(s, cid, ids, "layer.arrange")?;
    let how = str_p(p, "to").unwrap_or("front").to_string();
    let index = p.get("index").and_then(Value::as_u64).map(|i| i as usize);
    // `above`: put the layers right above that one (the timeline's drag to reorder).
    let above = match p.get("above") {
        Some(v) => {
            let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
            let a = resolve_layer(comp, v).ok_or_else(|| bad("layer.arrange", "no such layer for `above`"))?;
            if ids.contains(&a) {
                return Err(bad("layer.arrange", "`above` must be a layer that isn't being moved"));
            }
            Some(a)
        }
        None => None,
    };
    s.edit("Arrange Layers", None, |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        // Bring Forward / Send Backward move each layer one step past the next layer that isn't
        // moving (a run of selected layers moves as one; one already at the end stays).
        if above.is_none() && index.is_none() && matches!(how.as_str(), "forward" | "backward") {
            let picked = |l: &Layer| ids.contains(&l.id);
            let n = comp.layers.len();
            if how == "forward" {
                for i in 1..n {
                    if picked(&comp.layers[i]) && !picked(&comp.layers[i - 1]) {
                        let run_end = (i..n).find(|j| !picked(&comp.layers[*j])).unwrap_or(n);
                        comp.layers.rotate_range_left(i - 1..run_end, 1);
                    }
                }
            } else {
                for i in (0..n.saturating_sub(1)).rev() {
                    if picked(&comp.layers[i]) && !picked(&comp.layers[i + 1]) {
                        let run_start = (0..=i).rev().find(|j| !picked(&comp.layers[*j])).map_or(0, |j| j + 1);
                        comp.layers.rotate_range_right(run_start..=i + 1, 1);
                    }
                }
            }
            return Ok(());
        }
        let mut picked: Vec<Layer> = Vec::new();
        comp.layers.retain(|l| {
            if ids.contains(&l.id) {
                picked.push(l.clone());
                false
            } else {
                true
            }
        });
        let at = match (how.as_str(), index) {
            _ if above.is_some() => comp.layers.iter().position(|l| Some(l.id) == above).unwrap_or(comp.layers.len()),
            (_, Some(i)) => i.saturating_sub(1).min(comp.layers.len()),
            ("back", _) => comp.layers.len(),
            _ => 0,
        };
        for (k, l) in picked.into_iter().enumerate() {
            comp.layers.insert((at + k).min(comp.layers.len()), l);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// "Pre-comp N" with the lowest N no item already uses.
fn unused_precomp_name(proj: &effectcraft_project::Project) -> String {
    (1u32..).map(|n| format!("Pre-comp {n}")).find(|n| !proj.items.values().any(|i| &i.name == n)).unwrap_or_else(|| "Pre-comp".to_string())
}

/// Layer ▸ Pre-compose. `mode: move` (default; "Move all attributes into the new composition")
/// nests the selected layers as they are; `mode: leave` ("Leave all attributes in", one footage,
/// solid or precomp layer) nests only the layer's source, sized to it, and keeps the layer's
/// transform, masks, effects and timing in the current comp. `adjustDuration` (move) trims the
/// new comp to the selected layers' span; `open` opens the new comp.
pub(super) fn precompose(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "layer.precompose";
    let (cid, ids) = layers_p(s, p)?;
    if ids.is_empty() {
        return Err(bad(c, "no layers"));
    }
    let name = match str_p(p, "name") {
        Some(n) => n.to_string(),
        None => unused_precomp_name(&s.project),
    };
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let leave = match str_p(p, "mode").unwrap_or("move") {
        "move" => false,
        "leave" => true,
        x => return Err(bad(c, format!("mode: move|leave, got `{x}`"))),
    };
    let adjust = b_p(p, "adjustDuration").unwrap_or(false);
    if leave {
        let [lid] = ids[..] else { return Err(bad(c, "\"Leave all attributes\" needs exactly one layer")) };
        let layer = comp.layer(lid).ok_or(EngineError::NoComp)?.clone();
        let Some(src_item) = layer.source.item() else {
            return Err(bad(c, "\"Leave all attributes\" is available for footage, solid and precomp layers only"));
        };
        let (w, h) = effectcraft_render::source_size(&s.project, &layer);
        let (w, h) = if w == 0 || h == 0 { (comp.width, comp.height) } else { (w, h) };
        let dur = match s.project.item(src_item).map(|i| &i.kind) {
            Some(ItemKind::Comp(nc)) => nc.duration,
            Some(ItemKind::Footage(f)) if f.duration > Tick(0) => f.duration,
            _ => comp.duration,
        };
        let new = s.edit("Pre-compose", None, |proj, st| {
            let mut inner = comp.nested_like(w, h, dur);
            let mut il = build::layer(proj, &inner, &layer.name, layer.source.clone(), (w, h), Some(dur));
            il.name = layer.name.clone();
            inner.layers.push(il);
            let iid = proj.add_item(&name, Label::Sandstone, None, ItemKind::Comp(inner.into()));
            let l = layer_mut(proj, cid, lid)?;
            l.source = LayerSource::Comp { item: iid };
            l.name = name.clone();
            st.selected_layers = vec![lid];
            Ok((iid, lid))
        })?;
        if b_p(p, "open").unwrap_or(false) {
            s.open_comp(new.0);
        }
        return Ok(json!({"comp": new.0.0, "layer": new.1.0}));
    }
    let span = {
        let sel: Vec<&effectcraft_project::Layer> = comp.layers.iter().filter(|l| ids.contains(&l.id)).collect();
        let a = sel.iter().map(|l| l.in_point).min().unwrap_or(Tick(0)).max(Tick(0));
        let b = sel.iter().map(|l| l.out_point).max().unwrap_or(comp.duration).min(comp.duration);
        (a, b)
    };
    let (offset, dur) = if adjust && span.1 > span.0 { (span.0, span.1 - span.0) } else { (Tick(0), comp.duration) };
    let new = s.edit("Pre-compose", None, |proj, st| {
        let mut inner = comp.nested_like(comp.width, comp.height, dur);
        inner.layers = comp.layers.iter().filter(|l| ids.contains(&l.id)).cloned().collect();
        for l in &mut inner.layers {
            if l.parent.is_some_and(|p| !ids.contains(&p)) {
                l.parent = None;
            }
            if l.track_matte.is_some_and(|m| !ids.contains(&m.layer)) {
                l.track_matte = None;
            }
            l.start_time -= offset;
            l.in_point -= offset;
            l.out_point -= offset;
        }
        let iid = proj.add_item(&name, Label::Sandstone, None, ItemKind::Comp(inner.into()));
        let mut l = build::layer(proj, &comp, &name, LayerSource::Comp { item: iid }, (comp.width, comp.height), Some(dur));
        l.name = name.clone();
        l.start_time = offset;
        l.in_point = offset;
        l.out_point = offset + dur;
        let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let at = c.layers.iter().position(|l| ids.contains(&l.id)).unwrap_or(0);
        c.layers.retain(|l| !ids.contains(&l.id));
        // Layers left behind must not keep pointing at layers that moved into the new comp.
        for l in &mut c.layers {
            if l.parent.is_some_and(|p| ids.contains(&p)) {
                l.parent = None;
            }
            if l.track_matte.as_ref().is_some_and(|m| ids.contains(&m.layer)) {
                l.track_matte = None;
            }
        }
        let lid = l.id;
        c.layers.insert(at.min(c.layers.len()), l);
        st.selected_layers = vec![lid];
        Ok((iid, lid))
    })?;
    if b_p(p, "open").unwrap_or(false) {
        s.open_comp(new.0);
    }
    Ok(json!({"comp": new.0.0, "layer": new.1.0}))
}

/// Why expanding a pre-comp layer in place would change the picture or drop something, one entry
/// per cause. Un-precompose keeps the picture exactly for a pre-comp layer that only places the
/// nested composition (Position/Anchor Point, timing); anything the layer itself adds to the look,
/// and what only holds inside the nested comp, is listed. These are warnings: `force` unpacks
/// anyway and drops it. A lock is not listed here: it always refuses.
fn unprecompose_warnings(comp: &effectcraft_project::Comp, layer: &Layer, nested: &effectcraft_project::Comp) -> Vec<String> {
    let mut why = vec![];
    if layer.parent.is_some() {
        why.push("the layer has a parent".to_string());
    }
    if layer.track_matte.is_some() {
        why.push("the layer has a track matte".to_string());
    }
    if comp.layers.iter().any(|o| o.id != layer.id && (o.parent == Some(layer.id) || o.track_matte.is_some_and(|m| m.layer == layer.id))) {
        why.push("another layer uses it as its parent or track matte (that link is removed)".to_string());
    }
    if layer.is_3d() || layer.switches.collapse || layer.switches.adjustment || layer.preserve_transparency {
        why.push("the layer is 3D, collapses transformations, is an adjustment layer or preserves transparency".to_string());
    }
    if !layer.switches.video || !layer.switches.audio {
        why.push("the layer's Video or Audio switch is off".to_string());
    }
    if layer.switches.solo || layer.switches.guide {
        // Solo hides the comp's other layers and a guide layer is left out of renders; the
        // expanded copies would carry neither, so the picture would change.
        why.push("the layer's Solo or Guide switch is on".to_string());
    }
    if comp.layers.iter().any(|o| o.id != layer.id && refers_to(&o.props, layer.id)) {
        why.push("a property of another layer refers to it (that reference is cleared)".to_string());
    }
    why.extend(expression_warnings(comp, layer, nested.layers.len()));
    if layer.blend_mode != effectcraft_color::BlendMode::default() {
        why.push("the layer has a blending mode".to_string());
    }
    if layer.stretch != 100.0 || layer.props.prop("timeRemap").is_some() {
        why.push("the layer is time-stretched or time-remapped".to_string());
    }
    if layer.effects().is_some_and(|e| !e.children.is_empty()) {
        why.push("the layer has effects".to_string());
    }
    if layer.masks().is_some_and(|m| !m.children.is_empty()) {
        why.push("the layer has masks".to_string());
    }
    if layer.styles_change_pixels() {
        why.push("the layer has layer styles".to_string());
    }
    if let Some(tr) = layer.transform() {
        let at = |m: &str, d: [f64; 3]| tr.get(m).map(|p| p.value.as_vec3()).unwrap_or(d);
        let animated = tr.props().any(|p| !p.keys.is_empty() || p.expr.is_some());
        let separated = tr.get("positionX").is_some();
        if animated || separated {
            why.push("its transform is animated, has an expression or has separated dimensions".to_string());
        } else {
            let (sc, rot, ori) = (at("scale", [100.0; 3]), at("rotation", [0.0; 3]), at("orientation", [0.0; 3]));
            let op = tr.get("opacity").map(|p| p.value.as_f64()).unwrap_or(100.0);
            if sc[0] != 100.0 || sc[1] != 100.0 || rot[0] != 0.0 || ori != [0.0; 3] || op != 100.0 {
                why.push("its Scale, Rotation or Opacity is not at its default".to_string());
            }
        }
    }
    if nested.layers.iter().any(|l| l.is_camera() || l.is_light()) {
        why.push("the composition has a camera or light".to_string());
    }
    // Inside the nested comp these composite against the nested stack only; in the parent they
    // would act on everything below.
    if nested.layers.iter().any(|l| {
        l.switches.adjustment || l.switches.collapse || l.is_3d() || l.preserve_transparency || l.blend_mode != effectcraft_color::BlendMode::default()
    }) {
        why.push("the composition has adjustment, 3D or collapsing layers or layers with a blending mode or Preserve Transparency".to_string());
    }
    // A solo hides only the nested comp's other layers; a soloed copy would hide the parent's.
    if nested.layers.iter().any(|l| l.switches.solo) {
        why.push("the composition has a soloed layer".to_string());
    }
    why.extend(sampling_warnings(comp, nested));
    why.extend(position_expression_warnings(layer, nested));
    why
}

/// What the nested comp's own frame sampling does that the expanded layers lose: a preserved
/// frame rate other than the parent's (its layers then change only on its own frames), and
/// motion blur with a shutter other than the parent's.
fn sampling_warnings(comp: &effectcraft_project::Comp, nested: &effectcraft_project::Comp) -> Vec<String> {
    let mut why = vec![];
    if nested.preserve_frame_rate && nested.frame_rate != comp.frame_rate {
        why.push("the composition preserves a frame rate other than this one's (its layers would be sampled at this comp's frames)".to_string());
    }
    let shutter = nested.shutter_angle != comp.shutter_angle || nested.shutter_phase != comp.shutter_phase;
    if shutter && nested.layers.iter().any(|l| l.switches.motion_blur) {
        why.push("the composition's motion blur shutter differs from this one's".to_string());
    }
    why
}

/// The move from the pre-comp layer's Position against its Anchor Point, which expansion adds to
/// the Position of the nested comp's unparented layers.
fn placement_offset(layer: &Layer) -> (f64, f64) {
    let tr = layer.transform();
    let at = |m: &str| tr.and_then(|t| t.get(m)).map(|p| p.value.as_vec3()).unwrap_or([0.0; 3]);
    let (anchor, position) = (at("anchor"), at("position"));
    (position[0] - anchor[0], position[1] - anchor[1])
}

/// Unparented nested layers whose Position (or a separated axis) has an expression: the move from
/// [`placement_offset`] is added to the stored value and keys, which an expression can ignore.
fn position_expression_warnings(layer: &Layer, nested: &effectcraft_project::Comp) -> Vec<String> {
    let (dx, dy) = placement_offset(layer);
    if dx == 0.0 && dy == 0.0 {
        return vec![];
    }
    nested
        .layers
        .iter()
        .filter(|l| l.parent.is_none())
        .filter(|l| l.transform().is_some_and(|tr| ["position", "positionX", "positionY"].iter().any(|m| tr.get(m).is_some_and(|p| p.expr.is_some()))))
        .map(|l| format!("layer “{}” has a Position expression, so the move to the pre-comp layer's Position may not apply to it", l.name))
        .collect()
}

/// One warning per property of another layer in `comp` whose expression names `layer`: by its
/// name in quotes, or as `layer(<index>)` with its 1-based index (not on another comp's
/// `comp("…").layer(…)`), and one per expression naming a layer by a number below it while the
/// `added` layers replacing it renumber those. Detection is by text; the expressions are left as
/// they are.
fn expression_warnings(comp: &effectcraft_project::Comp, layer: &Layer, added: usize) -> Vec<String> {
    let index = comp.layers.iter().position(|l| l.id == layer.id).map(|i| i.saturating_add(1));
    let quoted = [format!("\"{}\"", layer.name), format!("'{}'", layer.name)];
    let names_it = |text: &str| quoted.iter().any(|q| text.contains(q.as_str())) || index.is_some_and(|i| indexed_layers(text).contains(&i));
    let renumbered = |text: &str| added != 1 && index.is_some_and(|i| indexed_layers(text).iter().any(|n| *n > i));
    let mut out = vec![];
    for o in comp.layers.iter().filter(|o| o.id != layer.id) {
        o.props.walk("", &mut |_, p| {
            if p.expr.as_ref().is_some_and(|e| !names_it(&e.text) && renumbered(&e.text)) {
                out.push(format!("layer “{}” {} names a layer by number, and the layers below “{}” are renumbered", o.name, p.name, layer.name));
            }
            if p.expr.as_ref().is_some_and(|e| names_it(&e.text)) {
                out.push(format!("layer “{}” {} has an expression that refers to “{}” (it will no longer find it)", o.name, p.name, layer.name));
            }
        });
    }
    out
}

/// The indexes expression `text` calls `layer(<index>)` with (spaces allowed) on this comp: bare,
/// `thisComp.layer(…)`, not `comp("Other").layer(…)` or `thisLayer(…)`.
fn indexed_layers(text: &str) -> Vec<usize> {
    text.match_indices("layer(")
        .filter_map(|(at, _)| {
            let before = text.get(..at).unwrap_or("");
            let on_other = before.ends_with(").") || before.chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_');
            if on_other && !before.ends_with("thisComp.") {
                return None;
            }
            let rest = text.get(at + "layer(".len()..).unwrap_or("").trim_start();
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            let after = rest.get(digits.len()..).unwrap_or("").trim_start();
            digits.parse::<usize>().ok().filter(|_| after.starts_with(')'))
        })
        .collect()
}

/// Whether any property (value or key) in `props` holds a reference to layer `id`.
fn refers_to(props: &effectcraft_project::PropGroup, id: LayerId) -> bool {
    let is = |v: &effectcraft_keyframe::Value| matches!(v, effectcraft_keyframe::Value::Layer(Some(l)) if *l == id.0);
    props.children.iter().any(|c| match c {
        effectcraft_project::Node::Prop(p) => is(&p.value) || p.keys.iter().any(|k| is(&k.value)),
        effectcraft_project::Node::Group(g) => refers_to(g, id),
    })
}

/// Clear every reference to layer `id` (`Value::Layer`, value or key) in `props`; references to
/// other layers stay.
fn clear_layer_refs(props: &mut effectcraft_project::PropGroup, id: LayerId) {
    let fix = |v: &mut effectcraft_keyframe::Value| {
        if matches!(v, effectcraft_keyframe::Value::Layer(Some(l)) if *l == id.0) {
            *v = effectcraft_keyframe::Value::Layer(None);
        }
    };
    for c in &mut props.children {
        match c {
            effectcraft_project::Node::Prop(p) => {
                fix(&mut p.value);
                for k in &mut p.keys {
                    fix(&mut k.value);
                }
            }
            effectcraft_project::Node::Group(g) => clear_layer_refs(g, id),
        }
    }
}

/// The layers of `comp` other than `id` let go of it: parent, track matte and property references
/// to it are removed, so nothing points at a layer that is gone.
fn detach_from(comp: &mut effectcraft_project::Comp, id: LayerId) {
    for l in comp.layers.iter_mut().filter(|l| l.id != id) {
        if l.parent == Some(id) {
            l.parent = None;
        }
        if l.track_matte.is_some_and(|m| m.layer == id) {
            l.track_matte = None;
        }
        clear_layer_refs(&mut l.props, id);
    }
}

/// Replace layer references (`Value::Layer`) found in `props` by `map` (old id → new id), the
/// way copies of a set of layers keep pointing at each other. A reference to a layer outside the
/// set is cleared: it named a layer of the nested comp that is no longer there.
fn remap_layer_values(props: &mut effectcraft_project::PropGroup, map: &[(LayerId, LayerId)]) {
    let fix = |v: &mut effectcraft_keyframe::Value| {
        if let effectcraft_keyframe::Value::Layer(Some(id)) = v {
            *v = effectcraft_keyframe::Value::Layer(map.iter().find(|(o, _)| o.0 == *id).map(|(_, n)| n.0));
        }
    };
    for c in &mut props.children {
        match c {
            effectcraft_project::Node::Prop(p) => {
                fix(&mut p.value);
                for k in &mut p.keys {
                    fix(&mut k.value);
                }
            }
            effectcraft_project::Node::Group(g) => remap_layer_values(g, map),
        }
    }
}

/// Layer ▸ Un-precompose: expand a selected pre-comp layer back into its composition. The nested
/// comp's layers become copies in this comp at the pre-comp layer's place, in their order and
/// with their keyframes, effects, masks, parenting, mattes, markers and audio; their timing is
/// moved by the pre-comp layer's Start and trimmed to the span it showed, and an Anchor Point
/// different from Position moves the unparented ones by the difference. The nested comp stays in
/// the project. The nested comp's frame no longer clips what the layers draw outside it.
///
/// A locked pre-comp layer is refused, changing nothing. Anything else that would change the
/// picture or drop something (see [`unprecompose_warnings`]) is an error listing the reasons unless
/// `force` is set; with `force` the layer is expanded the same way, what the pre-comp layer itself
/// carried (effects, masks, styles, transform beyond Position, time remap, blending mode, switches,
/// parent, matte) goes with it, and the other layers' parent, matte and property links to it are
/// removed. The result lists the reasons as `warnings`; `dryRun` only lists them.
pub(super) fn unprecompose(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "layer.unprecompose";
    let (cid, ids) = layers_p(s, p)?;
    let [lid] = ids[..] else { return Err(bad(c, "select exactly one pre-comp layer")) };
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?.clone();
    let LayerSource::Comp { item } = layer.source else { return Err(bad(c, format!("“{}” is not a pre-comp layer", layer.name))) };
    if layer.switches.locked {
        return Err(bad(c, format!("can't un-precompose “{}”: the layer is locked (unlock it first)", layer.name)));
    }
    let nested = s.project.comp(item).ok_or_else(|| bad(c, "the layer's composition is missing"))?.clone();
    let warnings = unprecompose_warnings(&comp, &layer, &nested);
    if b_p(p, "dryRun").unwrap_or(false) {
        return Ok(json!({"dryRun": true, "warnings": warnings}));
    }
    if !warnings.is_empty() && !b_p(p, "force").unwrap_or(false) {
        let why = warnings.join("; ");
        return Err(bad(
            c,
            format!(
                "can't un-precompose “{}” without changing it: {why} (nothing changed: pass `force` to unpack anyway, or `dryRun` to list the reasons)",
                layer.name
            ),
        ));
    }
    let (dx, dy) = placement_offset(&layer);
    let shown = (layer.in_point.max(layer.start_time), layer.out_point.min(layer.start_time + nested.duration));
    let new = s.edit("Un-precompose", None, |proj, st| {
        let mut next = proj.next_id;
        let mut copies: Vec<Layer> = nested.layers.iter().cloned().collect();
        let mut map = vec![];
        for l in &mut copies {
            let old = l.id;
            crate::cmd::edit::reid(l, &mut next);
            map.push((old, l.id));
        }
        let link = |id: LayerId| map.iter().find(|(o, _)| *o == id).map(|(_, n)| *n);
        for l in &mut copies {
            l.parent = l.parent.and_then(link);
            l.track_matte = l.track_matte.and_then(|m| link(m.layer).map(|layer| TrackMatte { layer, ..m }));
            remap_layer_values(&mut l.props, &map);
            l.start_time += layer.start_time;
            l.in_point = (l.in_point + layer.start_time).clamp(shown.0, shown.1.max(shown.0));
            l.out_point = (l.out_point + layer.start_time).clamp(l.in_point, shown.1.max(l.in_point));
            if l.parent.is_none()
                && let Some(tr) = l.transform_mut()
            {
                super::offset_position(tr, dx, dy);
            }
        }
        proj.next_id = next;
        let target = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        detach_from(target, lid);
        let at = target.layers.iter().position(|l| l.id == lid).unwrap_or(0);
        target.layers.retain(|l| l.id != lid);
        let mut made = vec![];
        for (k, mut l) in copies.into_iter().enumerate() {
            l.name = target.unique_layer_name(&l.name);
            made.push(l.id);
            target.layers.insert((at + k).min(target.layers.len()), l);
        }
        for m in &nested.markers {
            let mut m = m.clone();
            m.time += layer.start_time;
            target.markers.push(m);
        }
        // Comp markers are kept in time order (`thisComp.marker.key(n)`, next/previous marker).
        target.markers.sort_by_key(|m| m.time);
        st.selected_layers = made.clone();
        Ok(made)
    })?;
    Ok(json!({"layers": new.iter().map(|l| l.0).collect::<Vec<_>>(), "warnings": warnings}))
}
