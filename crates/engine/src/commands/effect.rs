//! Effect menu and Effect Controls gestures.

use effectcraft_project::build::Ids;
use effectcraft_project::{GroupKind, Layer, LayerId, PropGroup, Uid};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::{CommandSpec, b_p, bad, f_p, has_layers, layer_mut, layers_p, str_p};
use crate::{EngineError, Result, Session, cmd};

/// `effect.plugins.list`: the registered plug-in effects (WebAssembly or Rust).
fn plugins_list(s: &mut Session, _: &Value) -> Result<Value> {
    use effectcraft_effects::plugin;
    let list: Vec<Value> = plugin::plugins()
        .into_iter()
        .map(|spec| {
            let p = plugin::plugin(spec.id);
            let m = p.as_ref().map(|p| p.manifest().clone());
            json!({
                "id": spec.id,
                "name": spec.name,
                "category": spec.category,
                "version": m.as_ref().map(|m| m.version.clone()),
                "author": m.as_ref().map(|m| m.author.clone()),
                "description": m.as_ref().map(|m| m.description.clone()),
                "source": p.map(|p| p.source()),
                "params": spec.params.iter().map(|p| p.id).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(json!({"api": plugin::PLUGIN_API_VERSION, "wasm": s.plugin_loader.is_some(), "plugins": list}))
}

/// `effect.plugins.load`: load WebAssembly effect plug-ins (a file, or every `.wasm` in a
/// folder) through the host's loader ([`Session::plugin_loader`]).
fn plugins_load(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "effect.plugins.load";
    let loader = s.plugin_loader.ok_or_else(|| EngineError::Other("WebAssembly effect plug-ins are not available in this build".into()))?;
    let files: Vec<String> = match (str_p(p, "path"), str_p(p, "folder")) {
        (Some(path), _) => vec![path.to_string()],
        (None, Some(dir)) => {
            let mut v: Vec<String> = std::fs::read_dir(dir)
                .map_err(|e| bad(c, format!("cannot list {dir}: {e}")))?
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("wasm")))
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            v.sort();
            v
        }
        _ => return Err(bad(c, "give `path` or `folder`")),
    };
    let (mut loaded, mut errors) = (vec![], vec![]);
    for f in &files {
        let r = s.services.read_file(f).map_err(|e| format!("cannot read {f}: {e}")).and_then(|bytes| loader(&bytes, f));
        match r {
            Ok(v) => loaded.push(v),
            Err(e) => errors.push(json!({"path": f, "error": e})),
        }
    }
    if str_p(p, "path").is_some()
        && let Some(e) = errors.first()
    {
        return Err(EngineError::Other(format!("{}: {}", files[0], e["error"].as_str().unwrap_or("failed"))));
    }
    if !loaded.is_empty() {
        s.toast(format!("Loaded {} effect plug-in(s)", loaded.len()));
    }
    Ok(json!({"loaded": loaded, "errors": errors}))
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_p(p, "effect").ok_or_else(|| bad("effect.apply", "missing `effect` (id or name)"))?;
    let spec = effectcraft_effects::lookup(name).ok_or_else(|| bad("effect.apply", format!("unknown effect `{name}`")))?;
    let (cid, ids) = layers_p(s, p)?;
    if ids.is_empty() {
        return Err(bad("effect.apply", "no layer"));
    }
    let sizes: Vec<_> = {
        let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
        ids.iter().filter_map(|id| comp.layer(*id)).map(|l| (l.id, effectcraft_render::effect_bounds(&s.project, comp, l).0)).collect()
    };
    let uids = s.edit(&format!("Apply {}", spec.name), None, |proj, st| {
        let mut out = vec![];
        for (lid, size) in &sizes {
            let mut next = proj.next_id;
            let l = layer_mut(proj, cid, *lid)?;
            let fx = l.props.sub_mut("effects").ok_or_else(|| bad("effect.apply", "this layer type has no effects"))?;
            let same = fx.groups().filter(|g| g.match_id == spec.id).count();
            let name = if same == 0 { spec.name.to_string() } else { format!("{} {}", spec.name, same + 1) };
            let g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), &name, *size);
            out.push(g.uid);
            fx.children.push(g.into());
            proj.next_id = next;
            st.selected_props = vec![(*lid, *out.last().unwrap_or(&0))];
        }
        st.last_effect = Some(spec.id.to_string());
        Ok(out)
    })?;
    // Each instance's property path prefix (`effects/#2`), ready for prop.set paths.
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let paths: Vec<Value> = sizes
        .iter()
        .zip(&uids)
        .map(|((lid, _), uid)| {
            let n = comp.layer(*lid).and_then(|l| l.props.group("effects")).and_then(|fx| fx.children.iter().position(|c| c.uid() == *uid));
            json!(n.map(|n| format!("effects/#{}", n + 1)))
        })
        .collect();
    Ok(json!({"effects": uids, "paths": paths, "effect": spec.id}))
}

fn find_fx(s: &Session, p: &Value, cmd: &str) -> Result<(effectcraft_project::ItemId, effectcraft_project::LayerId, Uid)> {
    let (cid, lid) = super::layer_p(s, p, cmd)?;
    let layer = s.project.comp(cid).and_then(|c| c.layer(lid)).ok_or(EngineError::NoComp)?;
    let fx = layer.effects().ok_or_else(|| bad(cmd, "no effects"))?;
    let g = match p.get("effect") {
        Some(Value::Number(n)) => {
            let n = n.as_u64().unwrap_or(0);
            fx.groups().find(|g| g.uid == n).or_else(|| usize::try_from(n.saturating_sub(1)).ok().and_then(|i| fx.groups().nth(i)))
        }
        Some(Value::String(name)) => fx.groups().find(|g| &g.name == name || g.match_id == *name),
        _ => s.state.selected_props.iter().find_map(|(l, u)| (*l == lid).then(|| fx.groups().find(|g| g.uid == *u)).flatten()),
    }
    .ok_or_else(|| bad(cmd, "no such effect"))?;
    Ok((cid, lid, g.uid))
}

/// `effect.warning`: what Effect Controls warns about an effect (a custom OCIO config it can't
/// read, or transforms it passes through), `null` when it renders as asked.
fn warning(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "effect.warning";
    let (cid, lid, uid) = find_fx(s, p, cmd)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?;
    let g = layer.effects().and_then(|fx| fx.groups().find(|g| g.uid == uid)).ok_or_else(|| bad(cmd, "no such effect"))?;
    let t = super::time_p(s, p, Some(comp));
    let ctx = effectcraft_render::EvalCtx { project: &s.project, comp_id: cid, comp, time: t, expr: s.expr.as_deref(), footage: None };
    Ok(json!({"effect": uid, "warning": effectcraft_effects::warning(g, &mut |pr| ctx.value(layer, pr))}))
}

/// A colour parameter's eyedropper on a keyer (Effect Controls): the colour of the effect's input
/// (the layer's pixels before this effect, so a keyed screen can still be picked) at a
/// layer-space point, set on the parameter. `average` takes the 5 × 5 pixels around it.
fn pick_color(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "effect.pickColor";
    let (cid, lid, fx_uid) = find_fx(s, p, cmd)?;
    let (Some(x), Some(y)) = (f_p(p, "x"), f_p(p, "y")) else { return Err(bad(cmd, "missing `x` / `y` (layer pixels)")) };
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?;
    let fx = layer.effects().ok_or_else(|| bad(cmd, "no effects"))?;
    let index = fx.groups().position(|g| g.uid == fx_uid).ok_or_else(|| bad(cmd, "no such effect"))?;
    let g = fx.groups().nth(index).ok_or_else(|| bad(cmd, "no such effect"))?;
    let prop = match (p.get("prop").and_then(Value::as_u64), str_p(p, "param")) {
        (Some(uid), _) => g.find(uid),
        (None, Some(path)) => g.prop(path),
        _ => return Err(bad(cmd, "missing `param` (e.g. `screenColour`) or `prop` (uid)")),
    }
    .filter(|pr| pr.ui == effectcraft_project::ParamUi::Color)
    .ok_or_else(|| bad(cmd, "not a colour parameter of this effect"))?;
    let t = super::time_p(s, p, Some(comp));
    let ctx = effectcraft_render::EvalCtx { project: &s.project, comp_id: cid, comp, time: t, expr: s.expr.as_deref(), footage: None };
    let mut r = effectcraft_render::Renderer::new(&s.project, &*s.footage, effectcraft_render::RenderOpts::default());
    r.expr = s.expr.as_deref();
    r.cache = Some(&s.layer_cache);
    let buf = r.layer_input(&ctx, layer, index).ok_or_else(|| bad(cmd, "this layer has no pixels"))?;
    let (px, py) = buf.to_px([x, y]);
    let (cx, cy) = (px.floor() as i64, py.floor() as i64);
    let rad = if b_p(p, "average") == Some(true) { 2 } else { 0 };
    let mut sum = [0.0f32; 4];
    for yy in cy.saturating_sub(rad)..=cy.saturating_add(rad) {
        for xx in cx.saturating_sub(rad)..=cx.saturating_add(rad) {
            // Outside the layer reads as transparent.
            let c = buf.img.get(xx, yy);
            (0..4).for_each(|i| sum[i] += c[i]);
        }
    }
    if sum[3] <= 1e-6 {
        return Err(bad(cmd, "nothing to sample there (transparent or outside the layer)"));
    }
    // Straight colour of the (averaged) premultiplied pixels.
    let color: Vec<f64> = (0..3).map(|i| (sum[i] / sum[3]).clamp(0.0, 1.0) as f64).chain([1.0]).collect();
    let uid = prop.uid;
    s.execute("prop.set", json!({"comp": cid.0, "layer": lid.0, "prop": uid, "value": color}))?;
    Ok(json!({"prop": uid, "color": color}))
}

fn remove(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = super::comp_id(s, p)?;
    let fx = effects_p(s, p, "effect.remove")?;
    let label = if fx.len() == 1 { "Remove Effect" } else { "Remove Effects" };
    s.edit(label, None, |proj, st| {
        for (lid, uid) in &fx {
            let l = layer_mut(proj, cid, *lid)?;
            if let Some(fx) = l.props.sub_mut("effects") {
                fx.children.retain(|c| c.uid() != *uid);
            }
            st.selected_props.retain(|(_, u)| u != uid);
        }
        Ok(())
    })?;
    Ok(json!(fx.len()))
}

fn remove_all(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    s.edit("Remove All Effects", None, |proj, _| {
        for lid in &ids {
            if let Some(fx) = layer_mut(proj, cid, *lid)?.props.sub_mut("effects") {
                fx.children.clear();
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

fn toggle(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid, uid) = find_fx(s, p, "effect.toggle")?;
    let v = b_p(p, "value");
    let r = s.edit("Toggle Effect", None, |proj, _| {
        let l = layer_mut(proj, cid, lid)?;
        let g = l.props.find_group_mut(uid).ok_or_else(|| bad("effect.toggle", "gone"))?;
        g.enabled = v.unwrap_or(!g.enabled);
        Ok(g.enabled)
    })?;
    Ok(json!(r))
}

/// `effect.setLabel`: the effect's colour label (None clears it).
fn set_label(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "effect.setLabel";
    let (cid, lid, uid) = find_fx(s, p, c)?;
    let lab = match p.get("label") {
        Some(Value::Number(n)) => n.as_u64().and_then(|i| effectcraft_color::Label::ALL.get(usize::try_from(i).ok()?).copied()),
        Some(Value::String(name)) => s.prefs.label_from_name(name),
        _ => None,
    }
    .ok_or_else(|| bad(c, "missing or unknown `label` (name or index 0-16; 0 / None clears it)"))?;
    s.edit("Effect Label", None, |proj, _| {
        let l = layer_mut(proj, cid, lid)?;
        let g = l.props.find_group_mut(uid).ok_or_else(|| bad(c, "gone"))?;
        g.label = (lab != effectcraft_color::Label::None).then_some(lab);
        Ok(())
    })?;
    Ok(Value::Null)
}

fn reorder(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid, uid) = find_fx(s, p, "effect.reorder")?;
    let to = p.get("index").and_then(Value::as_u64).ok_or_else(|| bad("effect.reorder", "missing `index` (1-based)"))? as usize;
    s.edit("Reorder Effect", None, |proj, _| {
        let fx = layer_mut(proj, cid, lid)?.props.sub_mut("effects").ok_or_else(|| bad("effect.reorder", "no effects"))?;
        let Some(i) = fx.children.iter().position(|c| c.uid() == uid) else { return Ok(()) };
        let g = fx.children.remove(i);
        let to = to.saturating_sub(1).min(fx.children.len());
        fx.children.insert(to, g);
        Ok(())
    })?;
    Ok(Value::Null)
}

fn duplicate(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid, uid) = find_fx(s, p, "effect.duplicate")?;
    let r = s.edit("Duplicate Effect", None, |proj, st| {
        let mut next = proj.next_id;
        let fx = layer_mut(proj, cid, lid)?.props.sub_mut("effects").ok_or_else(|| bad("effect.duplicate", "no effects"))?;
        let Some(i) = fx.children.iter().position(|c| c.uid() == uid) else { return Ok(0) };
        let mut g = fx.children[i].as_group().cloned().ok_or_else(|| bad("effect.duplicate", "not a group"))?;
        g.reassign_uids(&mut next);
        g.name = unique_name(fx, &g.name);
        let u = g.uid;
        fx.children.insert(i + 1, g.into());
        proj.next_id = next + 1;
        st.selected_props = vec![(lid, u)];
        Ok(u)
    })?;
    Ok(json!(r))
}

/// Effect groups selected in Effect Controls / the timeline: (layer, effect uid), in stack order.
pub(crate) fn selected_effects(s: &Session) -> Vec<(LayerId, Uid)> {
    let Some(c) = s.active_comp() else { return vec![] };
    let mut out = vec![];
    for l in &c.layers {
        let Some(fx) = l.effects() else { continue };
        for g in fx.groups() {
            if s.state.selected_props.iter().any(|(sl, u)| *sl == l.id && *u == g.uid) {
                out.push((l.id, g.uid));
            }
        }
    }
    out
}

/// Effects named by `effect` / `effects` (uids, names or 1-based indexes), else the selection.
fn effects_p(s: &Session, p: &Value, cmd: &str) -> Result<Vec<(LayerId, Uid)>> {
    if p.get("effect").is_some() {
        let (_, lid, uid) = find_fx(s, p, cmd)?;
        return Ok(vec![(lid, uid)]);
    }
    if let Some(Value::Array(a)) = p.get("effects") {
        let mut out = vec![];
        for v in a {
            let mut q = p.clone();
            if let Some(o) = q.as_object_mut() {
                o.remove("effects");
                o.insert("effect".into(), v.clone());
            }
            let (_, lid, uid) = find_fx(s, &q, cmd)?;
            out.push((lid, uid));
        }
        return Ok(out);
    }
    let sel = selected_effects(s);
    if sel.is_empty() {
        return Err(bad(cmd, "no effect selected"));
    }
    Ok(sel)
}

fn effect_time_error(cmd: &str) -> EngineError {
    bad(cmd, "an effect keyframe converts outside the supported time range")
}

/// Validate before Layer's saturating conversions, so a bad stretch or time never silently
/// moves a copied key to another frame. Normal-speed layers keep their exact integer ticks.
fn effect_time(layer: &Layer, time: Tick, to_comp: bool, cmd: &str) -> Result<Tick> {
    if !layer.stretch.is_finite() || layer.stretch == 0.0 {
        return Err(bad(cmd, "the layer has an invalid time stretch"));
    }
    let in_range = |t: i64| (Tick::MIN.0..=Tick::MAX.0).contains(&t);
    if !in_range(time.0) {
        return Err(effect_time_error(cmd));
    }
    let offset = if to_comp { time.0 } else { time.0.checked_sub(layer.start_time.0).ok_or_else(|| effect_time_error(cmd))? };
    let scaled = if (layer.stretch - 100.0).abs() < 1e-9 {
        offset
    } else {
        let scaled = if to_comp { offset as f64 * layer.stretch / 100.0 } else { offset as f64 * 100.0 / layer.stretch };
        if !scaled.is_finite() || scaled < Tick::MIN.0 as f64 || scaled > Tick::MAX.0 as f64 {
            return Err(effect_time_error(cmd));
        }
        scaled as i64
    };
    if !in_range(scaled) {
        return Err(effect_time_error(cmd));
    }
    let time = if to_comp { layer.start_time.0.checked_add(scaled).ok_or_else(|| effect_time_error(cmd))? } else { scaled };
    if !in_range(time) {
        return Err(effect_time_error(cmd));
    }
    Ok(Tick(time))
}

/// Change the key time coordinate system throughout an effect, including nested controls.
/// Ease speeds are per second in that coordinate system; reversed time swaps the key sides.
fn retime_effect(group: &mut PropGroup, scale: f64, cmd: &str, mut time: impl FnMut(Tick) -> Result<Tick>) -> Result<()> {
    let mut result = Ok(());
    group.walk_mut(&mut |pr| {
        if result.is_err() {
            return;
        }
        for key in &mut pr.keys {
            let mapped = time(key.time).and_then(|t| {
                if !scale.is_finite() || scale == 0.0 {
                    return Err(effect_time_error(cmd));
                }
                // Spatial paths and non-numeric values ease along positive progress;
                // numeric components instead store a signed value derivative.
                let speed_scale = if pr.spatial || key.value.dims() == 0 { scale.abs() } else { scale };
                for ease in key.in_ease.iter_mut().chain(&mut key.out_ease) {
                    ease.speed /= speed_scale;
                    if !ease.speed.is_finite() {
                        return Err(bad(cmd, "an effect keyframe ease converts outside the supported range"));
                    }
                }
                if scale < 0.0 {
                    std::mem::swap(&mut key.in_interp, &mut key.out_interp);
                    std::mem::swap(&mut key.in_ease, &mut key.out_ease);
                    std::mem::swap(&mut key.spatial_in, &mut key.spatial_out);
                }
                Ok(t)
            });
            match mapped {
                Ok(t) => key.time = t,
                Err(e) => {
                    result = Err(e);
                    return;
                }
            }
        }
        pr.keys.sort_by_key(|key| key.time);
        if pr.keys.windows(2).any(|pair| pair[0].time == pair[1].time) {
            result = Err(bad(cmd, "the layer's time stretch maps effect keyframes to the same tick"));
        }
    });
    result
}

/// Edit ▸ Copy with effects selected: times on the clipboard are comp-time offsets from the
/// earliest copied key, shared by all copied effects and their parameters (#989).
fn copy(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "effect.copy";
    let fx = effects_p(s, p, "effect.copy")?;
    let comp = s.active_comp().ok_or(EngineError::NoComp)?;
    let mut groups = Vec::new();
    let mut earliest: Option<Tick> = None;
    for (lid, uid) in fx {
        let layer = comp.layer(lid).ok_or(EngineError::NoComp)?;
        let mut group = layer.effects().and_then(|fx| fx.groups().find(|g| g.uid == uid)).cloned().ok_or_else(|| bad(CMD, "the effect no longer exists"))?;
        retime_effect(&mut group, layer.stretch / 100.0, CMD, |t| {
            let t = effect_time(layer, t, true, CMD)?;
            earliest = Some(earliest.map_or(t, |first| first.min(t)));
            Ok(t)
        })?;
        groups.push(group);
    }
    if let Some(first) = earliest {
        for group in &mut groups {
            retime_effect(group, 1.0, CMD, |t| Ok(Tick(t.0.checked_sub(first.0).ok_or_else(|| effect_time_error(CMD))?)))?;
        }
    }
    s.state.clipboard.clear();
    s.state.key_clipboard.clear();
    s.state.keyframe_text = None;
    s.state.contents_clipboard.clear();
    s.state.link_clipboard = None;
    s.state.clip_is_keys = false;
    let n = groups.len();
    s.state.effect_clipboard = groups;
    Ok(json!({"effects": n}))
}

/// A unique instance name among `fx`'s effects (`Gaussian Blur`, `Gaussian Blur 2`…).
pub(crate) fn unique_name(fx: &PropGroup, base: &str) -> String {
    if !fx.groups().any(|g| g.name == base) {
        return base.to_string();
    }
    let stem = base.rsplit_once(' ').filter(|(_, n)| n.parse::<u32>().is_ok()).map(|(s, _)| s).unwrap_or(base);
    (2..).map(|i| format!("{stem} {i}")).find(|n| !fx.groups().any(|g| &g.name == n)).unwrap_or_else(|| base.to_string())
}

/// Edit ▸ Paste with effects on the clipboard: adds copies to every target layer, after the
/// layer's selected effect when it has one, else at the end of its stack. The earliest copied
/// key lands at the destination comp's CTI; all other keys keep their comp-time offsets.
fn paste(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "effect.paste";
    let clip = s.state.effect_clipboard.clone();
    if clip.is_empty() {
        return Err(bad("effect.paste", "no effects on the clipboard"));
    }
    let (cid, ids) = layers_p(s, p)?;
    if ids.is_empty() {
        return Err(bad("effect.paste", "select a layer to paste effects onto"));
    }
    let sel = selected_effects(s);
    let cti = s.time_of(cid);
    let label = if clip.len() == 1 { format!("Paste {}", clip[0].name) } else { "Paste Effects".to_string() };
    let uids = s.edit(&label, None, |proj, st| {
        let mut out = vec![];
        let mut next = proj.next_id;
        let mut new_sel = vec![];
        for lid in &ids {
            let l = layer_mut(proj, cid, *lid)?;
            let mut groups = clip.clone();
            for group in &mut groups {
                retime_effect(group, 100.0 / l.stretch, CMD, |offset| {
                    let t = Tick(cti.0.checked_add(offset.0).ok_or_else(|| effect_time_error(CMD))?);
                    effect_time(l, t, false, CMD)
                })?;
            }
            let Some(fx) = l.props.sub_mut("effects") else { continue };
            let at = sel
                .iter()
                .filter(|(sl, _)| sl == lid)
                .filter_map(|(_, u)| fx.children.iter().position(|c| c.uid() == *u))
                .max()
                .map(|i| i + 1)
                .unwrap_or(fx.children.len());
            for (k, mut g) in groups.into_iter().enumerate() {
                g.reassign_uids(&mut next);
                g.name = unique_name(fx, &g.name);
                out.push(g.uid);
                new_sel.push((*lid, g.uid));
                fx.children.insert(at + k, g.into());
            }
        }
        proj.next_id = next + 1;
        st.selected_props = new_sel;
        Ok(out)
    })?;
    Ok(json!({"effects": uids}))
}

/// Effect Controls' Reset: every parameter back to its default. Animated parameters get a
/// keyframe at the current time with the default value (as in After Effects); static ones
/// change their value.
fn reset(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid, uid) = find_fx(s, p, "effect.reset")?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?;
    let size = effectcraft_render::effect_bounds(&s.project, comp, layer).0;
    let lt = layer.layer_time(s.time());
    let g = layer.effects().and_then(|fx| fx.groups().find(|g| g.uid == uid)).ok_or_else(|| bad("effect.reset", "gone"))?;
    let spec = match &g.kind {
        GroupKind::Effect { effect } => effectcraft_effects::find(effect),
        _ => None,
    }
    .ok_or_else(|| bad("effect.reset", "unknown effect"))?;
    let name = g.name.clone();
    s.edit(&format!("Reset {name}"), None, |proj, _| {
        let g = layer_mut(proj, cid, lid)?.props.find_group_mut(uid).ok_or_else(|| bad("effect.reset", "gone"))?;
        for ps in &spec.params {
            // (`group/param` ids live in twirl-down groups.)
            if let Some(pr) = g.prop_mut(ps.id) {
                pr.set_value_at(lt, effectcraft_effects::default_value(ps, size));
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Tint's Swap Colors button edits both swatches at the CTI in one undo step. Like Reset,
/// it keys animated parameters without moving their existing keys or replacing expressions.
fn swap_tint_colors(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "effect.swapTintColors";
    if !p.is_object() || p.get("effect").is_some_and(|v| !v.is_string() && v.as_u64().is_none_or(|n| n == 0)) {
        return Err(bad(cmd, "effect must be a positive index, uid or name"));
    }
    let (cid, lid, uid) = find_fx(s, p, cmd)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?;
    let g = layer.props.find_group(uid).filter(|g| g.match_id == "ec.color.tint").ok_or_else(|| bad(cmd, "not a Tint effect"))?;
    let lt = layer.layer_time(s.time_of(cid));
    let color = |id| {
        g.prop(id)
            .map(|pr| pr.value_at(lt))
            .filter(|v| matches!(v, effectcraft_keyframe::Value::Color(_)))
            .ok_or_else(|| bad(cmd, "Tint is missing a colour parameter"))
    };
    let (black, white) = (color("black")?, color("white")?);
    s.edit("Swap Tint Colors", None, |proj, _| {
        let g = layer_mut(proj, cid, lid)?.props.find_group_mut(uid).ok_or_else(|| bad(cmd, "effect gone"))?;
        for (id, value) in [("black", white), ("white", black)] {
            g.prop_mut(id).ok_or_else(|| bad(cmd, "Tint is missing a colour parameter"))?.set_value_at(lt, value);
        }
        Ok(())
    })?;
    Ok(json!({"effect": uid}))
}

fn last(s: &mut Session, p: &Value) -> Result<Value> {
    let id = s.state.last_effect.clone().ok_or_else(|| bad("effect.applyLast", "no effect applied yet"))?;
    let mut q = p.clone();
    if let Some(o) = q.as_object_mut() {
        o.insert("effect".into(), json!(id));
    } else {
        q = json!({"effect": id});
    }
    apply(s, &q)
}

fn list(_: &mut Session, p: &Value) -> Result<Value> {
    let filter = str_p(p, "filter").map(str::to_lowercase);
    let v: Vec<Value> = effectcraft_effects::all()
        .into_iter()
        .filter(|e| {
            filter.as_ref().is_none_or(|f| effectcraft_effects::name_matches(e, f) || e.id.contains(f.as_str()) || e.category.to_ascii_lowercase().contains(f))
        })
        .map(|e| {
            json!({"id": e.id, "name": e.name, "category": e.category, "gpu": e.gpu, "float": e.float, "params": e.params.iter().map(|p| {
                let mut out = json!({"id": p.id, "name": p.name, "type": p.default.kind_name(), "default": p.default.to_json()});
                match &p.ui {
                    effectcraft_project::ParamUi::Popup { options } => {
                        // Engine/MCP popup values are zero-based; JSX exposes one-based values.
                        out["options"] = json!(options.iter().enumerate().map(|(value, label)| json!({"value": value, "label": label})).collect::<Vec<_>>());
                    }
                    effectcraft_project::ParamUi::Slider { min, max, slider_min, slider_max, decimals } => {
                        out["min"] = json!(min);
                        out["max"] = json!(max);
                        out["sliderMin"] = json!(slider_min);
                        out["sliderMax"] = json!(slider_max);
                        out["decimals"] = json!(decimals);
                    }
                    _ => {}
                }
                out
            }).collect::<Vec<_>>()})
        })
        .collect();
    Ok(json!(v))
}

impl Session {
    /// The named channels of layer `lid` in the active comp at the current time: its
    /// multi-layer OpenEXR footage's layers and channels (`diffuse.R`, `depth.Z`, …), which
    /// EXtractoR and the 3D Channel effects read. `None` for other layers.
    pub fn layer_channels(&self, lid: LayerId) -> Option<std::sync::Arc<effectcraft_raster::AuxChannels>> {
        let layer = self.active_comp()?.layer(lid)?;
        let effectcraft_project::LayerSource::Footage { item } = layer.source else { return None };
        let effectcraft_project::ItemKind::Footage(f) = &self.project.item(item)?.kind else { return None };
        self.footage.aux(item, f, layer.layer_time(self.time()))
    }
}

/// `layer.channels`: the layers and channels of a layer's multi-layer OpenEXR footage, and
/// what each layer shows in red, green, blue and alpha (EXtractoR's popups).
fn channels(s: &mut Session, p: &Value) -> Result<Value> {
    let (_, lid) = super::layer_p(s, p, "layer.channels")?;
    let Some(aux) = s.layer_channels(lid) else { return Ok(json!({"channels": [], "layers": []})) };
    let layers: Vec<Value> = aux.layers().into_iter().map(|l| json!({"name": l, "rgba": aux.layer_rgba(l)})).collect();
    Ok(json!({"channels": aux.names(), "layers": layers}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("effect.apply", "Apply Effect", [], None, "{effect: id|name (e.g. Gaussian Blur), layers?}", has_layers, apply),
        crate::query!(
            "layer.channels",
            "Layer Channels",
            "{layer?} → {channels: [name], layers: [{name, rgba: [r, g, b, a channel]}]} — a multi-layer OpenEXR layer's channels (EXtractoR's red / green / blue / alpha take these names)",
            channels
        ),
        cmd!(
            "effect.pickColor",
            "Pick Effect Colour",
            [],
            None,
            "{layer?, effect?: index|uid|name, param: id (e.g. screenColour) | prop: uid, x, y (effect space), average?: bool (5×5), time?} — the colour of the effect's input there",
            has_layers,
            pick_color
        ),
        cmd!("effect.applyLast", "Last Effect", ["Effect"], Some("Cmd+Alt+Shift+E"), "{layers?}", has_layers, last),
        cmd!("effect.removeAll", "Remove All", ["Effect"], Some("Cmd+Shift+E"), "{layers?}", has_layers, remove_all),
        cmd!("effect.remove", "Remove Effect", [], None, "{layer?, effect: index|uid|name}", has_layers, remove),
        cmd!("effect.toggle", "Toggle Effect", [], None, "{layer?, effect, value?}", has_layers, toggle),
        cmd!("effect.setLabel", "Effect Label", [], None, "{layer?, effect, label: name|index 0-16 (None clears)}", has_layers, set_label),
        cmd!("effect.reorder", "Reorder Effect", [], None, "{layer?, effect, index}", has_layers, reorder),
        cmd!("effect.duplicate", "Duplicate Effect", [], None, "{layer?, effect}", has_layers, duplicate),
        cmd!("effect.copy", "Copy Effects", [], None, "{layer?, effect? | effects?: [uid|name|index]} (default: the selected effects)", has_layers, copy),
        cmd!("effect.paste", "Paste Effects", [], None, "{layers?} — adds the copied effects to the layers", has_layers, paste),
        cmd!("effect.reset", "Reset Effect", [], None, "{layer?, effect}", has_layers, reset),
        cmd!(
            "effect.swapTintColors",
            "Swap Tint Colors",
            [],
            None,
            "{comp?, layer?, effect?: index|uid|name} — swaps Tint's Map Black To and Map White To at the current layer time in one undo step",
            has_layers,
            swap_tint_colors
        ),
        crate::query!(
            "effect.list",
            "List Effects",
            "{filter?} → [{params: [{id, name, type, default, options?, min?, max?, sliderMin?, sliderMax?, decimals?}]}]; popup indices are zero-based (JSX uses one-based values)",
            list
        ),
        crate::query!(
            "effect.warning",
            "Effect Warning",
            "{layer?, effect?: index|uid|name (default: the selected effect), time?} → {effect, warning: string|null} — what Effect Controls warns about (an OCIO config it can't read, transforms it passes through)",
            warning
        ),
        crate::query!(
            "effect.plugins.list",
            "List Effect Plug-ins",
            "{} → {api, wasm, plugins: [{id, name, category, version, author, source, params}]}",
            plugins_list
        ),
        cmd!(
            "effect.plugins.load",
            "Load Effect Plug-in...",
            [],
            None,
            "{path (.wasm / .wat plug-in, API v1) | folder (loads every .wasm in it)}",
            super::always,
            plugins_load
        ),
    ]
}

#[cfg(test)]
mod list_metadata_tests {
    use super::*;

    #[test]
    fn effect_list_exposes_popup_labels_and_numeric_slider_limits() {
        let mut s = Session::default();
        let menus = s.execute("effect.list", json!({"filter": "ec.control.dropdown"})).unwrap();
        assert_eq!(menus.as_array().map(Vec::len), Some(1));
        let menu = &menus[0]["params"][0];
        assert_eq!(menu["type"], "enum");
        assert_eq!(menu["default"], 0);
        assert_eq!(
            menu["options"],
            json!([
                {"value": 0, "label": "Item 1"},
                {"value": 1, "label": "Item 2"},
                {"value": 2, "label": "Item 3"}
            ])
        );
        let sliders = s.execute("effect.list", json!({"filter": "ec.control.slider"})).unwrap();
        assert_eq!(sliders.as_array().map(Vec::len), Some(1));
        let slider = &sliders[0]["params"][0];
        assert_eq!(slider["type"], "scalar");
        assert_eq!(slider["min"], -1_000_000.0);
        assert_eq!(slider["max"], 1_000_000.0);
        assert_eq!(slider["sliderMin"], 0.0);
        assert_eq!(slider["sliderMax"], 100.0);
        assert_eq!(slider["decimals"], 2);
        assert!(slider["options"].is_null());
    }
}
