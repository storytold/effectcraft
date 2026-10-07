//! Effect menu and Effect Controls gestures.

use effectcraft_project::build::Ids;
use effectcraft_project::{GroupKind, LayerId, PropGroup, Uid};
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
        ids.iter()
            .filter_map(|id| comp.layer(*id))
            .map(|l| {
                let (w, h) = effectcraft_render::source_size(&s.project, l);
                (l.id, if w == 0 { [comp.width as f64, comp.height as f64] } else { [w as f64, h as f64] })
            })
            .collect()
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
            fx.groups().find(|g| g.uid == n).or_else(|| fx.groups().nth(n.saturating_sub(1) as usize))
        }
        Some(Value::String(name)) => fx.groups().find(|g| &g.name == name || g.match_id == *name),
        _ => s.state.selected_props.iter().find_map(|(l, u)| (*l == lid).then(|| fx.groups().find(|g| g.uid == *u)).flatten()),
    }
    .ok_or_else(|| bad(cmd, "no such effect"))?;
    Ok((cid, lid, g.uid))
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

/// Edit ▸ Copy with effects selected: puts the effect instances on the effect clipboard.
fn copy(s: &mut Session, p: &Value) -> Result<Value> {
    let fx = effects_p(s, p, "effect.copy")?;
    let comp = s.active_comp().ok_or(EngineError::NoComp)?;
    let groups: Vec<PropGroup> =
        fx.iter().filter_map(|(l, u)| comp.layer(*l).and_then(|l| l.effects()).and_then(|e| e.groups().find(|g| g.uid == *u)).cloned()).collect();
    s.state.clipboard.clear();
    s.state.key_clipboard.clear();
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
/// layer's selected effect when it has one, else at the end of its stack.
fn paste(s: &mut Session, p: &Value) -> Result<Value> {
    let clip = s.state.effect_clipboard.clone();
    if clip.is_empty() {
        return Err(bad("effect.paste", "no effects on the clipboard"));
    }
    let (cid, ids) = layers_p(s, p)?;
    if ids.is_empty() {
        return Err(bad("effect.paste", "select a layer to paste effects onto"));
    }
    let sel = selected_effects(s);
    let label = if clip.len() == 1 { format!("Paste {}", clip[0].name) } else { "Paste Effects".to_string() };
    let uids = s.edit(&label, None, |proj, st| {
        let mut out = vec![];
        let mut next = proj.next_id;
        let mut new_sel = vec![];
        for lid in &ids {
            let l = layer_mut(proj, cid, *lid)?;
            let Some(fx) = l.props.sub_mut("effects") else { continue };
            let at = sel
                .iter()
                .filter(|(sl, _)| sl == lid)
                .filter_map(|(_, u)| fx.children.iter().position(|c| c.uid() == *u))
                .max()
                .map(|i| i + 1)
                .unwrap_or(fx.children.len());
            for (k, g) in clip.iter().enumerate() {
                let mut g = g.clone();
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

/// Set an effect instance's presentation label without changing effect parameters.
fn set_label(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "effect.setLabel";
    let label = match p.get("label") {
        Some(Value::Number(n)) => n.as_u64().and_then(|n| usize::try_from(n).ok()).and_then(|n| effectcraft_color::Label::ALL.get(n)).copied(),
        Some(Value::String(name)) => s.prefs.label_from_name(name),
        _ => None,
    }
    .ok_or_else(|| bad(c, "missing or unknown `label` (name or index 0-16)"))?;
    let (cid, lid, uid) = match p.get("effect") {
        Some(Value::Number(number)) => {
            let number = number.as_u64().filter(|number| *number > 0).ok_or_else(|| bad(c, "effect must be a positive uid or 1-based index"))?;
            let (cid, lid) = super::layer_p(s, p, c)?;
            let effects = s.project.comp(cid).and_then(|comp| comp.layer(lid)).and_then(|layer| layer.effects()).ok_or_else(|| bad(c, "no effects"))?;
            let group = effects
                .groups()
                .find(|group| group.uid == number)
                .or_else(|| usize::try_from(number - 1).ok().and_then(|index| effects.groups().nth(index)))
                .ok_or_else(|| bad(c, "no such effect"))?;
            (cid, lid, group.uid)
        }
        None | Some(Value::String(_)) => find_fx(s, p, c)?,
        _ => return Err(bad(c, "effect must be a uid, 1-based index or name")),
    };
    let is_effect = s
        .project
        .comp(cid)
        .and_then(|comp| comp.layer(lid))
        .and_then(|layer| layer.props.find_group(uid))
        .is_some_and(|group| matches!(group.kind, GroupKind::Effect { .. }));
    if !is_effect {
        return Err(bad(c, "not an effect instance"));
    }
    s.edit("Effect Label", None, |proj, _| {
        let group = layer_mut(proj, cid, lid)?.props.find_group_mut(uid).ok_or_else(|| bad(c, "effect disappeared"))?;
        group.effect_label = label;
        Ok(())
    })?;
    Ok(json!({"effect": uid, "label": label.name()}))
}

/// Effect Controls' Reset: every parameter back to its default. Animated parameters get a
/// keyframe at the current time with the default value (as in After Effects); static ones
/// change their value.
fn reset(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid, uid) = find_fx(s, p, "effect.reset")?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?;
    let (w, h) = effectcraft_render::source_size(&s.project, layer);
    let size = if w == 0 { [comp.width as f64, comp.height as f64] } else { [w as f64, h as f64] };
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
            if let Some(pr) = g.get_mut(ps.id) {
                pr.set_value_at(lt, effectcraft_effects::default_value(ps, size));
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
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
            json!({"id": e.id, "name": e.name, "category": e.category, "gpu": e.gpu, "float": e.float, "params": e.params.iter().map(|p| json!({"id": p.id, "name": p.name, "default": p.default.to_json()})).collect::<Vec<_>>()})
        })
        .collect();
    Ok(json!(v))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("effect.apply", "Apply Effect", [], None, "{effect: id|name (e.g. Gaussian Blur), layers?}", has_layers, apply),
        cmd!(
            "effect.pickColor",
            "Pick Effect Colour",
            [],
            None,
            "{layer?, effect?: index|uid|name, param: id (e.g. screenColour) | prop: uid, x, y (layer px), average?: bool (5×5), time?} — the colour of the effect's input there",
            has_layers,
            pick_color
        ),
        cmd!("effect.applyLast", "Last Effect", ["Effect"], Some("Cmd+Alt+Shift+E"), "{layers?}", has_layers, last),
        cmd!("effect.removeAll", "Remove All", ["Effect"], Some("Cmd+Shift+E"), "{layers?}", has_layers, remove_all),
        cmd!("effect.remove", "Remove Effect", [], None, "{layer?, effect: index|uid|name}", has_layers, remove),
        cmd!("effect.toggle", "Toggle Effect", [], None, "{layer?, effect, value?}", has_layers, toggle),
        cmd!("effect.reorder", "Reorder Effect", [], None, "{layer?, effect, index}", has_layers, reorder),
        cmd!("effect.duplicate", "Duplicate Effect", [], None, "{layer?, effect}", has_layers, duplicate),
        cmd!("effect.copy", "Copy Effects", [], None, "{layer?, effect? | effects?: [uid|name|index]} (default: the selected effects)", has_layers, copy),
        cmd!("effect.paste", "Paste Effects", [], None, "{layers?} — adds the copied effects to the layers", has_layers, paste),
        cmd!("effect.setLabel", "Set Effect Label", [], None, "{layer?, effect?: uid|1-based index|name, label: name|0-16}", has_layers, set_label),
        cmd!("effect.reset", "Reset Effect", [], None, "{layer?, effect}", has_layers, reset),
        crate::query!("effect.list", "List Effects", "{filter?}", list),
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
