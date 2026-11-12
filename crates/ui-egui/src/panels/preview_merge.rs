//! Taking a dialog's Preview back without taking other edits with it.
//!
//! A previewing dialog (Camera, Light, Solid, Composition Settings, the Gradient Editor) puts the
//! project and undo history back to a checkpoint when it is cancelled or Preview goes off. If the
//! project changed in the meantime for another reason (a camera analysis finishing, a script, the
//! control channel), swapping the checkpoint in would throw that edit away. This module undoes
//! only what the preview did: a three-way merge of the checkpoint (before the preview), the project
//! right after the preview (`Applied`) and the project now. Whatever the preview did not touch
//! stays as it is now; what only the preview touched goes back to the checkpoint; what both touched
//! keeps the other edit.

use std::collections::BTreeSet;
use std::sync::Arc;

use effectcraft_engine::commands::text_edit::active_editor_snapshot;
use effectcraft_engine::project::{Comp, Item, ItemId, ItemKind, Layer, LayerId, Project};

use crate::EffectcraftApp;
use crate::panels::layer_styles_dialog::{Checkpoint, restore};

/// The project right after a preview was applied, and the undo depth then.
#[derive(Clone)]
pub(crate) struct Applied {
    project: Arc<Project>,
    undo_len: usize,
}

impl std::fmt::Debug for Applied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Applied").field("undo_len", &self.undo_len).finish_non_exhaustive()
    }
}

/// Note the project as the preview that has just been applied left it.
pub(crate) fn applied(app: &EffectcraftApp) -> Applied {
    Applied { project: app.session.project.clone(), undo_len: app.session.history.undo.len() }
}

/// Put the project back as `checkpoint` has it, minus nothing but the preview: when the project is
/// still what the preview made (or nothing says what that was) the checkpoint is simply restored.
pub(crate) fn rollback(app: &mut EffectcraftApp, checkpoint: Checkpoint, applied: Option<&Applied>) {
    let Some(applied) = applied.filter(|a| !Arc::ptr_eq(&a.project, &app.session.project)) else {
        restore(app, checkpoint);
        return;
    };
    let merged = merge(&checkpoint.project, &applied.project, &app.session.project);
    // One step for what was done meanwhile, from the project before the dialog: Undo takes it back.
    let label = app.session.history.undo.get(applied.undo_len).map(|(label, _)| label.clone()).unwrap_or_else(|| "Edit".to_string());
    let mut history = checkpoint.history;
    history.undo.push((label, checkpoint.project));
    history.redo.clear();
    history.merge_key = None;
    let edited = active_editor_snapshot(&app.session);
    app.session.project = Arc::new(merged);
    app.session.history = history;
    app.session.sanitize_after_replacement(edited.as_ref());
    app.session.bump();
}

/// `now` without the changes between `before` and `after`, keeping any other change.
fn merge(before: &Project, after: &Project, now: &Project) -> Project {
    let mut merged = now.clone();
    let ids: BTreeSet<ItemId> = before.items.keys().chain(after.items.keys()).chain(now.items.keys()).copied().collect();
    for id in ids {
        match item(before.items.get(&id), after.items.get(&id), now.items.get(&id)) {
            Some(it) => {
                merged.items.insert(id, it);
            }
            None => {
                merged.items.remove(&id);
            }
        }
    }
    merged
}

/// The three-way choice for one value: untouched by the preview keeps `now`, touched only by the
/// preview goes back to `before`, touched by both is `both`.
fn pick<T: PartialEq + Clone>(before: Option<&T>, after: Option<&T>, now: Option<&T>, both: impl FnOnce() -> Option<T>) -> Option<T> {
    if after == before {
        now.cloned()
    } else if now == after {
        before.cloned()
    } else {
        both()
    }
}

fn item(before: Option<&Item>, after: Option<&Item>, now: Option<&Item>) -> Option<Item> {
    pick(before, after, now, || {
        if let (Some(b), Some(a), Some(n)) = (before, after, now)
            && let (ItemKind::Comp(bc), ItemKind::Comp(ac), ItemKind::Comp(nc)) = (&b.kind, &a.kind, &n.kind)
        {
            let mut merged = n.clone();
            // Name, folder and the like: only a preview that changed the item itself and nothing else
            // is taken back; the comp's own contents are merged below.
            merged.kind = ItemKind::Comp(Arc::new(comp(bc, ac, nc)));
            return Some(merged);
        }
        now.cloned()
    })
}

fn bare(c: &Comp) -> Comp {
    let mut b = c.clone();
    b.layers.clear();
    b
}

fn layer_of(c: &Comp, id: LayerId) -> Option<&Layer> {
    c.layers.iter().find(|l| l.id == id)
}

fn comp(before: &Comp, after: &Comp, now: &Comp) -> Comp {
    // The comp's own settings, apart from its layers.
    let mut merged = pick(Some(&bare(before)), Some(&bare(after)), Some(&bare(now)), || Some(bare(now))).unwrap_or_else(|| bare(now));
    let merge_layer = |id: LayerId| pick(layer_of(before, id), layer_of(after, id), layer_of(now, id), || layer_of(now, id).cloned());
    // Layers as they stand now, in their order, then the ones the preview removed put back.
    let mut layers: Vec<Layer> = now.layers.iter().filter_map(|l| merge_layer(l.id)).collect();
    for (at, l) in before.layers.iter().enumerate() {
        if layer_of(now, l.id).is_none()
            && let Some(back) = merge_layer(l.id)
        {
            layers.insert(at.min(layers.len()), back);
        }
    }
    merged.layers = layers.into();
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Dialog;
    use crate::panels::live_preview;
    use effectcraft_engine::Session;
    use serde_json::json;

    fn app_with_light() -> (EffectcraftApp, u64) {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name": "Scene", "width": 320, "height": 180, "duration": 1})).unwrap();
        let light = s.execute("layer.newLight", json!({"kind": "Point"})).unwrap()["layer"].as_u64().unwrap();
        (EffectcraftApp::new(s), light)
    }

    fn name_of(app: &EffectcraftApp, layer: u64) -> String {
        app.session.active_comp().unwrap().layer(LayerId(layer)).unwrap().name.clone()
    }

    fn layer_count(app: &EffectcraftApp) -> usize {
        app.session.active_comp().unwrap().layers.len()
    }

    /// The preview renames the light; a null is added meanwhile (what a finished background job
    /// would do). Cancel takes the rename back and keeps the null, with one undo step for it.
    #[test]
    fn cancelling_a_preview_keeps_an_edit_made_while_it_was_open() {
        let (mut app, light) = app_with_light();
        let original = name_of(&app, light);
        app.dialog = Some(Dialog::LightSettings);
        live_preview::begin(&mut app, Dialog::LightSettings);
        let rename = |name: &str| json!({"layer": light, "name": name});
        live_preview::sync(&mut app, "layer.rename", rename(&original));
        live_preview::sync(&mut app, "layer.rename", rename("Previewed"));
        assert_eq!(name_of(&app, light), "Previewed");
        let layers = layer_count(&app);
        app.session.execute("layer.newNull", json!({})).unwrap();
        live_preview::cancel(&mut app);
        assert_eq!(name_of(&app, light), original, "the preview is taken back");
        assert_eq!(layer_count(&app), layers + 1, "the other edit stays");
        assert!(app.session.undo());
        assert_eq!(layer_count(&app), layers, "and has its own undo step");
        assert_eq!(name_of(&app, light), original);
    }

    /// With nothing else done meanwhile the checkpoint is restored as before.
    #[test]
    fn cancelling_an_undisturbed_preview_restores_the_checkpoint() {
        let (mut app, light) = app_with_light();
        let original = name_of(&app, light);
        let steps = app.session.history.undo.len();
        app.dialog = Some(Dialog::LightSettings);
        live_preview::begin(&mut app, Dialog::LightSettings);
        live_preview::sync(&mut app, "layer.rename", json!({"layer": light, "name": original}));
        live_preview::sync(&mut app, "layer.rename", json!({"layer": light, "name": "Previewed"}));
        live_preview::cancel(&mut app);
        assert_eq!(name_of(&app, light), original);
        assert_eq!(app.session.history.undo.len(), steps);
    }

    /// An item both the preview and another edit changed keeps the other edit.
    #[test]
    fn a_layer_both_edits_changed_keeps_the_other_edit() {
        let (mut app, light) = app_with_light();
        let original = name_of(&app, light);
        app.dialog = Some(Dialog::LightSettings);
        live_preview::begin(&mut app, Dialog::LightSettings);
        live_preview::sync(&mut app, "layer.rename", json!({"layer": light, "name": original}));
        live_preview::sync(&mut app, "layer.rename", json!({"layer": light, "name": "Previewed"}));
        app.session.execute("layer.rename", json!({"layer": light, "name": "Renamed elsewhere"})).unwrap();
        live_preview::cancel(&mut app);
        assert_eq!(name_of(&app, light), "Renamed elsewhere");
    }
}
