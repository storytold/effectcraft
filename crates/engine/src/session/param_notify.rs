//! Plug-in effects that asked to hear about the user's parameter changes
//! ([`effectcraft_effects::plugin::EffectPlugin::notify_params`], #3663): a preset menu that sets
//! other parameters, a "link" checkbox. Every undoable edit ([`Session::edit`]) is compared with
//! the project before it; for each such parameter whose value or keyframes it changed, the plug-in
//! is asked ([`EffectPlugin::param_changed`]) and the values it answers are written into the
//! same edit, the way the user's own value was (a keyframe at the edit's time when the property is
//! animated), so the change and its consequences are one undo step.
//!
//! Only edits get here: undo, redo, rendering and expression evaluation never do. What the
//! answers change is not looked at again, so an answer can't start another round. A plug-in that
//! fails or times out leaves the user's change as it is.
//!
//! [`EffectPlugin::param_changed`]: effectcraft_effects::plugin::EffectPlugin::param_changed

use effectcraft_effects::plugin::{self, ParamChange};
use effectcraft_keyframe::Value;
use effectcraft_project::{GroupKind, ItemId, LayerId, ParamUi, Project, Property, Uid};
use effectcraft_time::Tick;

use super::Session;

/// The most parameter changes reported for one edit (pasting values into many effects).
const MAX_NOTIFICATIONS: usize = 8;

/// One reported parameter change.
#[derive(Debug)]
struct Change {
    comp: ItemId,
    layer: LayerId,
    /// The effect instance (its property group).
    group: Uid,
    effect: String,
    param: String,
    /// The layer time the user's change is at.
    time: Tick,
}

impl Session {
    /// Ask the plug-ins about the parameters that edit `before` → `p` changed, and write their
    /// answers into `p` (see the module docs).
    pub(super) fn notify_param_changes(&self, before: &Project, p: &mut Project) {
        if !plugin::any_notify_params() {
            return;
        }
        let changes = changes(before, p, &|cid| self.time_of(cid));
        for c in changes.into_iter().take(MAX_NOTIFICATIONS) {
            if let Err(e) = notify(p, &c) {
                // The user's change stays; nothing else is asked in this edit, so a helper that
                // doesn't answer holds the edit up once at most.
                log::warn!("effect `{}`: the change of `{}` was not reported: {e}", c.effect, c.param);
                return;
            }
        }
    }
}

/// The notifying parameters `p` changed against `before`: effects that exist in both (same
/// property group), whose plug-in lists the parameter in its `notify_params`, with another
/// stored value or other keyframes (an expression's text doesn't count).
fn changes(before: &Project, p: &Project, cti: &dyn Fn(ItemId) -> Tick) -> Vec<Change> {
    let mut out = vec![];
    for (cid, comp) in p.comps() {
        let Some(old_comp) = before.comp(*cid) else { continue };
        for layer in &comp.layers {
            let Some(fx) = layer.effects() else { continue };
            let Some(old_layer) = old_comp.layer(layer.id) else { continue };
            for g in fx.groups() {
                let GroupKind::Effect { effect } = &g.kind else { continue };
                let Some(pl) = plugin::plugin(effect) else { continue };
                let ids = pl.notify_params();
                if ids.is_empty() {
                    continue;
                }
                let Some(old) = old_layer.props.find_group(g.uid) else { continue };
                for id in ids {
                    let (Some(now), Some(was)) = (g.get(id), old.get(id)) else { continue };
                    if now.value == was.value && now.keys == was.keys {
                        continue;
                    }
                    // A keyframe set or changed by the edit is where it happened; else the
                    // current time.
                    let keyed = now.keys.iter().find(|k| !was.keys.iter().any(|o| o.time == k.time && o.value == k.value)).map(|k| k.time);
                    let time = keyed.unwrap_or_else(|| layer.layer_time(cti(*cid)));
                    out.push(Change { comp: *cid, layer: layer.id, group: g.uid, effect: effect.clone(), param: id.clone(), time });
                }
            }
        }
    }
    out
}

/// Ask the plug-in about one change and apply its answer to `p`.
fn notify(p: &mut Project, c: &Change) -> Result<(), String> {
    let pl = plugin::plugin(&c.effect).ok_or("the plug-in is gone")?;
    let m = pl.manifest();
    let comp = p.comp(c.comp).ok_or("no comp")?;
    let layer = comp.layer(c.layer).ok_or("no layer")?;
    let g = layer.props.find_group(c.group).ok_or("no effect")?;
    let params = effectcraft_effects::flatten_params(g, &mut |pr| pr.value_at(c.time));
    let change = ParamChange {
        param: c.param.clone(),
        instance_key: plugin::instance_key_for(c.group),
        time: c.time.seconds(),
        layer_size: effectcraft_render::effect_bounds(p, comp, layer).0,
        values: plugin::param_values(m, &params),
        texts: plugin::flatten_texts(m, &params),
    };
    let answer = pl.param_changed(&change)?;
    let layers: Vec<u64> = comp.layers.iter().map(|l| l.id.0).collect();
    let layer = p.comp_mut(c.comp).and_then(|cm| cm.layer_mut(c.layer)).ok_or("no layer")?;
    let g = layer.props.find_group_mut(c.group).ok_or("no effect")?;
    for (id, v) in answer {
        if let Some(pr) = g.get_mut(&id) {
            set_checked(pr, c.time, &v, &layers);
        }
    }
    Ok(())
}

/// Set `pr` to the plug-in's value `v` at layer time `t` (a keyframe when it is animated), when
/// the value fits the property: the property's kind, finite numbers, a popup index in range, a
/// layer of the comp; slider values are clamped as `prop.set` clamps them. Anything else is
/// ignored.
fn set_checked(pr: &mut Property, t: Tick, v: &serde_json::Value, layers: &[u64]) {
    let cur = pr.value_at(t);
    if matches!(cur, Value::Path(_) | Value::Text(_) | Value::Gradient(_)) {
        return;
    }
    match (&cur, &pr.ui) {
        (Value::Enum(_), ParamUi::Popup { options }) => {
            let ok = v.as_f64().is_some_and(|i| i.is_finite() && i >= 0.0 && i.fract() == 0.0 && i < options.len() as f64);
            if !ok {
                return;
            }
        }
        (Value::Layer(_), _) if !(v.is_null() || v.as_u64().is_some_and(|id| layers.contains(&id))) => return,
        (Value::Str(_), _) if !v.is_string() => return,
        _ => {}
    }
    // Arrays of the property's size only (a colour may leave out alpha); one number sets a
    // single-component value.
    let dims = cur.dims();
    match v.as_array() {
        Some(a) if a.len() != dims && !(matches!(cur, Value::Color(_)) && a.len() == 3) => return,
        None if dims > 1 => return,
        _ => {}
    }
    let Some(mut nv) = cur.coerce_json(v) else { return };
    if nv.components().iter().any(|x| !x.is_finite()) || std::mem::discriminant(&nv) != std::mem::discriminant(&cur) {
        return;
    }
    if let (ParamUi::Slider { min, max, .. }, Value::Scalar(x)) = (&pr.ui, &nv) {
        nv = Value::Scalar(x.clamp(*min, *max));
    }
    pr.set_value_at(t, nv);
}
