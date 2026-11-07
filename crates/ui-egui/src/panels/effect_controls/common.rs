use super::{EffectcraftApp, Layer, Node, ParamUi, PropGroup, Value, json};

pub(super) type Actions = Vec<(String, serde_json::Value)>;

pub(super) const ROW: f32 = 20.0;

pub(super) const DIAL_ROW: f32 = 64.0;

// The effect title starts at 44 pt. Each level's names sit one tree step farther in;
// reserve the same icon gutter for properties and groups so sibling names align.
pub(super) const PARAM_NAME_X: f32 = 58.0;

pub(super) const TREE_INDENT: f32 = 14.0;

/// Selecting any descendant activates its owning effect without replacing the property
/// selection used by keyframe editing, copy and delete.
pub(super) fn effect_active(g: &PropGroup, layer: effectcraft_engine::project::LayerId, selected: &[(effectcraft_engine::project::LayerId, u64)]) -> bool {
    fn contains(g: &PropGroup, uid: u64, depth: usize) -> bool {
        if g.uid == uid {
            return true;
        }
        if depth >= 16 {
            return false;
        }
        g.children.iter().any(|n| n.uid() == uid || matches!(n, Node::Group(child) if contains(child, uid, depth + 1)))
    }
    selected.iter().any(|(l, uid)| *l == layer && contains(g, *uid, 0))
}

/// The layer Effect Controls shows: the layer it is locked to (its tab's lock) while that layer's
/// comp is the active comp, else the first selected layer of the active comp.
pub(crate) fn shown_layer_id(app: &EffectcraftApp) -> Option<effectcraft_engine::project::LayerId> {
    let comp = app.session.active_comp()?;
    if let Some(lock) = app.ui.effect_controls_lock
        && app.session.active_comp_id().is_some_and(|c| c.0 == lock.comp)
        && comp.layer(effectcraft_engine::project::LayerId(lock.layer)).is_some()
    {
        return Some(effectcraft_engine::project::LayerId(lock.layer));
    }
    app.session.state.selected_layers.first().copied().filter(|id| comp.layer(*id).is_some())
}

/// Returns the layer Effect Controls currently shows, locked or selected, or `None` when there is none.
pub(super) fn shown_layer(app: &EffectcraftApp) -> Option<Layer> {
    let id = shown_layer_id(app)?;
    app.session.active_comp()?.layer(id).cloned()
}

/// Lock Effect Controls on the layer it shows (`on`), or unlock it. Locking with no layer shown
/// does nothing; locking while already locked keeps the layer it is locked to. Returns whether
/// the panel is locked afterwards. The tab's lock icon and `ui.set {effectControlsLocked}`.
pub fn set_lock(app: &mut EffectcraftApp, on: bool) -> bool {
    // A lock left from a replaced project is gone before this one is answered (a control
    // request can run between the replacement and the next frame).
    scope_lock(app);
    if !on {
        app.ui.effect_controls_lock = None;
        return false;
    }
    if app.ui.effect_controls_lock.is_some() {
        return true;
    }
    let Some(comp) = app.session.active_comp_id() else { return false };
    let Some(layer) = shown_layer_id(app) else { return false };
    app.ui.effect_controls_lock = Some(crate::state::EffectControlsLock { comp: comp.0, layer: layer.0 });
    app.ui.effect_controls_lock_epoch = app.session.script_ui.project_epoch;
    true
}

/// Once a frame: a lock made in another project (New, Open: the ids start over) or on a layer or
/// comp since deleted is dropped, whether or not Effect Controls is on screen.
pub(crate) fn scope_lock(app: &mut EffectcraftApp) {
    let epoch = app.session.script_ui.project_epoch;
    if app.ui.effect_controls_lock_epoch != epoch {
        app.ui.effect_controls_lock_epoch = epoch;
        app.ui.effect_controls_lock = None;
    }
    prune_lock(app);
}

/// A lock whose layer or comp no longer exists (deleted, or its comp deleted) is dropped, so the
/// panel follows the selection again and its tab shows it unlocked. Another comp being active
/// keeps the lock: the panel shows the locked layer again when its comp comes back.
pub(super) fn prune_lock(app: &mut EffectcraftApp) {
    let Some(lock) = app.ui.effect_controls_lock else { return };
    let alive = app
        .session
        .project
        .comp(effectcraft_engine::project::ItemId(lock.comp))
        .is_some_and(|c| c.layer(effectcraft_engine::project::LayerId(lock.layer)).is_some());
    if !alive {
        app.ui.effect_controls_lock = None;
    }
}

/// An effect was just applied (`effect.apply` / `effect.applyLast` with `params`): Effect
/// Controls comes up (opened if closed, brought to the front, the focus left where it is) on the
/// layer the effect went to, with the new effect selected, as in After Effects.
pub fn reveal_applied(app: &mut EffectcraftApp, params: &serde_json::Value) {
    // `effect.apply` selects the new effect on (the last of) the layers it went to.
    let target = app.session.state.selected_props.last().map(|(l, _)| *l);
    // Effect Controls locked to a layer of the active comp keeps showing that layer.
    let locked = app.ui.effect_controls_lock.filter(|k| app.session.active_comp_id().is_some_and(|c| c.0 == k.comp));
    if let Some(l) = target
        && !app.session.state.selected_layers.contains(&l)
        && params.get("comp").is_none()
        && app.session.active_comp().is_some_and(|c| c.layer(l).is_some())
        // Dropped on the locked panel: it shows that layer already, the selection stays.
        && locked.is_none_or(|k| k.layer != l.0)
    {
        // Show that layer (dropped on an unselected one), keeping the new effect selected.
        let props = app.session.state.selected_props.clone();
        match app.session.execute("layer.select", json!({"layers": [l.0]})) {
            Ok(_) => app.session.state.selected_props = props,
            Err(e) => app.ui.status = e.to_string(),
        }
    }
    // The effect went to another layer than the one the locked panel shows: say why it isn't
    // there (After Effects opens another Effect Controls tab).
    if let (Some(k), Some(l)) = (locked, target)
        && k.layer != l.0
        && let Some(name) = app.session.active_comp().and_then(|c| c.layer(effectcraft_engine::project::LayerId(k.layer))).map(|x| x.name.clone())
    {
        app.ui.status = crate::i18n::tr_args("Effect Controls is locked to {}", &[&name]);
    }
    app.raise_panel(crate::dock::PanelKind::EffectControls);
}

/// After Effects asks for an effect's file as it applies it (Apply Color LUT opens the file
/// dialog for its LUT). For the instances `effect.apply` made (`applied`, its result), open the
/// file dialog for their first empty file parameter and set the file on each; cancelling
/// leaves the effect without one. Does nothing without a file dialog (tests, agents).
pub fn choose_files_after_apply(app: &mut EffectcraftApp, applied: &serde_json::Value) {
    let Some(pick) = app.hooks.pick_files.as_ref() else { return };
    let Some(comp) = app.session.active_comp() else { return };
    let uids: Vec<u64> = applied.get("effects").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_u64()).collect()).unwrap_or_default();
    let mut targets = vec![];
    let mut filters: Vec<String> = vec![];
    for uid in uids {
        for l in &comp.layers {
            let Some(g) = l.props.find_group(uid) else { continue };
            let mut found = None;
            g.walk("", &mut |_, pr| {
                if found.is_none()
                    && let ParamUi::File { filters } = &pr.ui
                    && matches!(&pr.value, Value::Str(s) if s.trim().is_empty())
                {
                    found = Some((pr.uid, filters.clone()));
                }
            });
            if let Some((prop, f)) = found {
                targets.push((l.id.0, prop));
                filters = f;
            }
        }
    }
    if targets.is_empty() {
        return;
    }
    let exts: Vec<&str> = filters.iter().map(String::as_str).collect();
    let Some(path) = pick(&exts).into_iter().next() else { return };
    for (layer, prop) in targets {
        if let Err(e) = app.session.execute("prop.set", json!({"layer": layer, "prop": prop, "value": path})) {
            app.ui.status = e.to_string();
        }
    }
}

/// An undo-merge key for one gesture on `id`: a new key each time a drag starts, so separate
/// drags are separate undo steps while one drag is one step.
pub(crate) fn gesture_key(ui: &egui::Ui, id: egui::Id, started: bool) -> String {
    let kid = id.with("gesture");
    let n: u64 = if started {
        let n = ui.data(|d| d.get_temp::<u64>(kid)).unwrap_or(0) + 1;
        ui.data_mut(|d| d.insert_temp(kid, n));
        n
    } else {
        ui.data(|d| d.get_temp::<u64>(kid)).unwrap_or(0)
    };
    format!("{id:?}-{n}")
}
