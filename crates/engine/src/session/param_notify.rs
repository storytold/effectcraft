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
//! The wait is bounded per command: one command (an edit, an `engine.batch`, a script with all
//! the commands it runs) waits at most [`plugin::param_change_timeout`] for all its answers
//! together. Once that is spent, or an answer failed, the rest of the command reports nothing
//! (logged).
//!
//! [`EffectPlugin::param_changed`]: effectcraft_effects::plugin::EffectPlugin::param_changed

use effectcraft_effects::plugin::{self, ParamChange};
use effectcraft_keyframe::Value;
use effectcraft_project::{GroupKind, ItemId, LayerId, ParamUi, Project, Property, Uid};
use effectcraft_time::Tick;
use web_time::{Duration, Instant};

use super::Session;

/// The plug-in wait of the command running ([`Session::execute`]): one deadline for all its
/// edits, and whether a plug-in already failed in it.
#[derive(Debug, Default)]
pub(crate) struct NotifyBudget {
    /// Nesting of [`Session::execute`] (a batch or a script runs commands inside a command).
    depth: u32,
    deadline: Option<Instant>,
    stalled: bool,
}

impl Session {
    /// Enter a command: the outermost one starts a fresh wait budget.
    pub(crate) fn notify_enter(&mut self) {
        if self.param_notify.depth == 0 {
            self.param_notify.deadline = None;
            self.param_notify.stalled = false;
        }
        self.param_notify.depth = self.param_notify.depth.saturating_add(1);
    }

    /// Leave a command entered with [`Session::notify_enter`].
    pub(crate) fn notify_leave(&mut self) {
        self.param_notify.depth = self.param_notify.depth.saturating_sub(1);
    }
}

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
    pub(super) fn notify_param_changes(&mut self, before: &Project, p: &mut Project) {
        if !plugin::any_notify_params() {
            return;
        }
        let changes = changes(before, p, &|cid| self.time_of(cid));
        if changes.is_empty() {
            return;
        }
        // Outside a command (an edit made directly) the budget is this edit's own.
        if self.param_notify.depth == 0 {
            self.param_notify.deadline = None;
            self.param_notify.stalled = false;
        }
        let deadline = *self.param_notify.deadline.get_or_insert_with(|| {
            let now = Instant::now();
            now.checked_add(plugin::param_change_timeout()).unwrap_or(now)
        });
        for c in changes.into_iter().take(MAX_NOTIFICATIONS) {
            let left = deadline.saturating_duration_since(Instant::now());
            if self.param_notify.stalled || left == Duration::ZERO {
                self.param_notify.stalled = true;
                log::warn!("effect `{}`: the change of `{}` was not reported: the wait for plug-ins in this command is used up", c.effect, c.param);
                return;
            }
            if let Err(e) = notify(p, &c, left) {
                // The user's change stays; nothing else is asked in this command, so a helper
                // that doesn't answer holds it up once at most.
                log::warn!("effect `{}`: the change of `{}` was not reported: {e}", c.effect, c.param);
                self.param_notify.stalled = true;
                return;
            }
        }
    }
}

/// The notifying parameters `p` changed against `before`: effects that exist in both (same
/// property group), whose plug-in lists the parameter in its `notify_params`, whose value at the
/// edit's time changed (the time of a keyframe the edit set, else the current time). Changing a
/// keyframe's interpolation, ease or label, or turning the stopwatch on, doesn't change a value,
/// and an expression's text doesn't count. Comps and layers the edit didn't touch (still shared
/// with `before`) and effects that don't notify are skipped without looking inside.
fn changes(before: &Project, p: &Project, cti: &dyn Fn(ItemId) -> Tick) -> Vec<Change> {
    let mut out = vec![];
    for (cid, comp) in p.comps() {
        let Some(old_comp) = before.comp(*cid) else { continue };
        if std::ptr::eq(comp, old_comp) {
            continue;
        }
        for layer in &comp.layers {
            let Some(old_layer) = old_comp.layer(layer.id) else { continue };
            if std::ptr::eq(layer, old_layer) {
                continue;
            }
            let Some(fx) = layer.effects() else { continue };
            for g in fx.groups() {
                let GroupKind::Effect { effect } = &g.kind else { continue };
                if !plugin::notifies(effect) {
                    continue;
                }
                let Some(old) = old_layer.props.find_group(g.uid) else { continue };
                if g == old {
                    continue;
                }
                let Some(pl) = plugin::plugin(effect) else { continue };
                for id in pl.notify_params() {
                    let (Some(now), Some(was)) = (g.get(id), old.get(id)) else { continue };
                    if now.value == was.value && now.keys == was.keys {
                        continue;
                    }
                    // A keyframe the edit set or changed the value of is where it happened; else
                    // the current time.
                    let keyed = now.keys.iter().find(|k| !was.keys.iter().any(|o| o.time == k.time && o.value == k.value)).map(|k| k.time);
                    let time = keyed.unwrap_or_else(|| layer.layer_time(cti(*cid)));
                    if now.value_at(time) == was.value_at(time) {
                        continue;
                    }
                    out.push(Change { comp: *cid, layer: layer.id, group: g.uid, effect: effect.clone(), param: id.clone(), time });
                }
            }
        }
    }
    out
}

/// Ask the plug-in about one change and apply its answer to `p`.
fn notify(p: &mut Project, c: &Change, budget: Duration) -> Result<(), String> {
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
        budget: Some(budget),
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
    // (a project can hold a slider whose range is upside down: then there is nothing to clamp to)
    if let (ParamUi::Slider { min, max, .. }, Value::Scalar(x)) = (&pr.ui, &nv)
        && min <= max
    {
        nv = Value::Scalar(x.clamp(*min, *max));
    }
    pr.set_value_at(t, nv);
}

#[cfg(test)]
mod perf {
    use super::*;
    use serde_json::json;

    /// Rough cost of looking for changes after a one-layer edit in a project with 2000 effects
    /// (200 layers × 10 Gaussian Blurs) while a notifying plug-in is registered, against walking
    /// every effect and looking each up in the plug-in registry (what it did before). Run with
    /// `--ignored --nocapture`.
    #[test]
    #[ignore]
    fn looking_for_changes_in_a_big_project() {
        use effectcraft_effects::plugin::{EffectPlugin, PluginFrame, PluginManifest, PluginParams, register_plugin};
        struct N(PluginManifest, Vec<String>);
        impl EffectPlugin for N {
            fn manifest(&self) -> &PluginManifest {
                &self.0
            }
            fn render(&self, _: &mut PluginFrame, _: &PluginParams, _: f64) -> Result<(), String> {
                Ok(())
            }
            fn notify_params(&self) -> &[String] {
                &self.1
            }
        }
        let m: PluginManifest = serde_json::from_value(json!({"api": 2, "id": "org.test.perf.n", "name": "N", "category": "Test",
            "params": [{"id": "a", "name": "A", "type": "slider", "default": 0}]}))
        .unwrap();
        register_plugin(std::sync::Arc::new(N(m, vec!["a".into()]))).unwrap();
        let mut s = Session::default();
        s.execute("comp.new", json!({"name": "Main", "width": 640, "height": 360, "frameRate": 30, "duration": 10})).unwrap();
        let mut ids = vec![];
        for _ in 0..200 {
            ids.push(s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 64, "height": 64})).unwrap()["layer"].as_u64().unwrap());
        }
        for _ in 0..10 {
            s.execute("effect.apply", json!({"layers": ids, "effect": "Gaussian Blur"})).unwrap();
        }
        let cid = s.active_comp_id().unwrap();
        let before = s.project.clone();
        let mut after = (*before).clone();
        // One layer edited (as an edit does: only that layer is copied).
        let layer = &mut after.comp_mut(cid).unwrap().layers[0];
        layer.name = "edited".into();
        let n = 200;
        let t0 = web_time::Instant::now();
        for _ in 0..n {
            assert!(changes(&before, &after, &|_| Tick::ZERO).is_empty());
        }
        let now = t0.elapsed() / n;
        let t0 = web_time::Instant::now();
        for _ in 0..n {
            let mut found = 0;
            for (_, comp) in after.comps() {
                for layer in &comp.layers {
                    for g in layer.effects().into_iter().flat_map(|fx| fx.groups()) {
                        if let GroupKind::Effect { effect } = &g.kind
                            && plugin::plugin(effect).is_some_and(|p| !p.notify_params().is_empty())
                        {
                            found += 1;
                        }
                    }
                }
            }
            assert_eq!(found, 0);
        }
        let walk = t0.elapsed() / n;
        eprintln!("2000 effects, one layer edited: {now:?} per edit now, {walk:?} walking every effect through the registry");
    }
}
