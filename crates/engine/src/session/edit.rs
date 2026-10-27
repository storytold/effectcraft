use super::*;

impl Session {
    /// Apply an undoable edit. `merge`: consecutive edits with the same key fold into one step.
    ///
    /// Every edit is an external boundary for the on-canvas text editor: if it replaced the
    /// document the editor is showing, the selection is clamped and the editor gets a fresh IME
    /// epoch. The editor's own typing goes through `edit_text_doc`, which owns that
    /// document and must not treat its own keystrokes as a replacement.
    pub fn edit<T>(&mut self, label: &str, merge: Option<&str>, f: impl FnOnce(&mut Project, &mut EditorState) -> Result<T>) -> Result<T> {
        let edited = commands::text_edit::active_editor_snapshot(self);
        let out = self.edit_inner(label, merge, f)?;
        commands::text_edit::after_external_change(self, edited.as_ref());
        Ok(out)
    }

    /// [`Session::edit`] for the on-canvas editor's own text changes. Its document is the one
    /// the editor owns, so a change here is typing, not a replacement behind the IME's back.
    pub(crate) fn edit_text_doc<T>(&mut self, label: &str, merge: Option<&str>, f: impl FnOnce(&mut Project, &mut EditorState) -> Result<T>) -> Result<T> {
        self.edit_inner(label, merge, f)
    }

    /// The shared body of [`Session::edit`] and [`Session::edit_text_doc`].
    ///
    /// Runs `f` on a clone of the project and editor state, then synchronizes derived data (global
    /// light and Essential Graphics), queues invalidated Warp Stabilizer/camera-tracker work and
    /// Roto Brush instances needing re-propagation in the `*_pending` lists. If the clone has the same
    /// content the project and history are left as they were (only editor-state changes from `f`
    /// are kept) and no revision is bumped; otherwise the previous project is recorded as an
    /// undo step under `label`, unless it merges with the current merge key (then the redo branch
    /// is dropped). An `Err` from `f` propagates with the session unchanged.
    fn edit_inner<T>(&mut self, label: &str, merge: Option<&str>, f: impl FnOnce(&mut Project, &mut EditorState) -> Result<T>) -> Result<T> {
        let before = self.project.clone();
        let mut p = (*self.project).clone();
        let mut st = self.state.clone();
        let r = f(&mut p, &mut st)?;
        // Layer styles: one Global Light per comp, whichever layer edited it.
        effectcraft_project::styles::sync_global_light(&before, &mut p);
        // Essential Properties of precomp layers follow their comps' Essential Graphics.
        effectcraft_project::essential::sync_project(&before, &mut p);
        // Warp Stabilizer analyses made from other frames are cleared (and queued again).
        for w in warp::invalidate(&before, &mut p) {
            if !self.warp_pending.contains(&w) {
                self.warp_pending.push(w);
            }
        }
        // So are 3D Camera Tracker tracks (and solves made with other settings).
        for w in camera_track::invalidate(&before, &mut p) {
            if !self.camera_pending.contains(&w) {
                self.camera_pending.push(w);
            }
        }
        // Roto Brush Input Keys follow the frames; edited instances propagate again.
        for r in roto::sync(&before, &mut p) {
            if !self.roto_pending.contains(&r) {
                self.roto_pending.push(r);
            }
        }
        // Nothing changed (a value set to what it already was): no undo step, not modified.
        if p.same_content(&before) {
            self.state = st;
            return Ok(r);
        }
        let same = merge.is_some() && merge.map(str::to_string) == self.history.merge_key;
        if !same {
            // Undone steps aren't lost: they stay in the History panel as a branch.
            let levels = self.prefs.general.undo_levels.max(1) as usize;
            self.history.record(label, before, levels);
        } else {
            self.history.abandon_redo(&before);
        }
        self.history.merge_key = merge.map(str::to_string);
        self.project = Arc::new(p);
        self.state = st;
        self.bump();
        Ok(r)
    }

    /// Note a change to the on-canvas editor's committed text or session.
    ///
    /// Frontends compare this with [`commands::text_edit::ime_stamp_revision`]; an unrelated
    /// project change must not advance it.
    pub(crate) fn note_text_edit_change(&mut self) {
        // Checked process-wide allocation: a stamp is never wrapped or reused. On exhaustion
        // keep the previous value; an unavailable before/after pair then compares equal and
        // earns no receipt instead of accepting a stale revision.
        if let Some(revision) = commands::text_edit::next_ime_revision() {
            self.ime_revision = self.ime_revision.max(revision);
        }
    }

    pub fn bump(&mut self) {
        self.revision += 1;
        // Back at the saved state (undo, redo, a history jump or a rolled-back batch).
        if Arc::ptr_eq(&self.project, &self.saved_project) {
            self.saved_revision = self.revision;
        }
        self.refresh_disk_salt(false);
        self.events.push(Event::ProjectChanged { revision: self.revision });
    }

    /// Salt the disk cache's layer keys with the project's footage files
    /// ([`effectcraft_render::disk_cache::footage_salt`]). Taking it reads the size and time of
    /// every file, every frame of an image sequence included, so it is taken again only when the
    /// footage items changed, or with `force` (a project opened, footage reloaded, the disk cache
    /// attached), not at every edit: with a few large sequences that cost each keystroke in a
    /// text field over half a second (#1926).
    pub fn refresh_disk_salt(&mut self, force: bool) -> Option<u64> {
        use std::hash::{Hash, Hasher};
        if self.disk_cache.is_none() {
            self.disk_salt = None;
            return None;
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for (id, it) in &self.project.items {
            if let effectcraft_project::ItemKind::Footage(f) = &it.kind {
                id.hash(&mut h);
                format!("{f:?}").hash(&mut h);
            }
        }
        let footage = h.finish();
        if let Some((seen, salt)) = self.disk_salt
            && seen == footage
            && !force
        {
            return Some(salt);
        }
        let salt = effectcraft_render::disk_cache::footage_salt(&self.project);
        self.disk_salt = Some((footage, salt));
        self.layer_cache.set_disk_salt(salt);
        Some(salt)
    }

    /// Set a comp's Resolution. As in After Effects it is saved with the project (which becomes
    /// modified) but is no undo step, and it changes no pixels: the comp's cached frames stay
    /// valid ([`effectcraft_project::Comp::same_pixels`]).
    pub fn set_resolution(&mut self, comp: ItemId, r: effectcraft_project::Resolution) -> Result<()> {
        if self.project.comp(comp).ok_or(EngineError::NoComp)?.resolution == r {
            return Ok(());
        }
        Arc::make_mut(&mut self.project).comp_mut(comp).ok_or(EngineError::NoComp)?.resolution = r;
        self.bump();
        Ok(())
    }

    /// Keep what lives outside the undo history (each comp's Resolution) as it is in `from`, the
    /// project before undo, redo or a history jump restored another state.
    pub(crate) fn keep_view_settings(&mut self, from: &Project) {
        let keep: Vec<(ItemId, effectcraft_project::Resolution)> =
            self.project.comps().filter_map(|(id, c)| from.comp(*id).map(|f| (*id, f.resolution)).filter(|(_, r)| *r != c.resolution)).collect();
        if keep.is_empty() {
            return;
        }
        let p = Arc::make_mut(&mut self.project);
        for (id, r) in keep {
            if let Some(c) = p.comp_mut(id) {
                c.resolution = r;
            }
        }
    }

    /// Undoes one step: the current project goes onto the redo stack, per-comp Resolution is kept
    /// as it was, the merge key is cleared, selections are sanitized for the replaced state (the
    /// on-canvas editor gets a new IME epoch only when its own document changed) and the session is
    /// bumped. Returns `false`, changing nothing, when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some((label, p)) = self.history.undo.pop() else { return false };
        let edited = commands::text_edit::active_editor_snapshot(self);
        let cur = std::mem::replace(&mut self.project, p);
        self.keep_view_settings(&cur);
        self.history.redo.push((label, cur));
        self.history.merge_key = None;
        self.sanitize_after_replacement(edited.as_ref());
        self.bump();
        true
    }

    /// Redoes one step: the current project goes back onto the undo stack, per-comp Resolution is
    /// kept, the merge key is cleared, selections are sanitized for the replaced state (a changed
    /// text document starts a new IME epoch) and the session is bumped. Returns `false`, changing
    /// nothing, when there is nothing to redo.
    pub fn redo(&mut self) -> bool {
        let Some((label, p)) = self.history.redo.pop() else { return false };
        let edited = commands::text_edit::active_editor_snapshot(self);
        let cur = std::mem::replace(&mut self.project, p);
        self.keep_view_settings(&cur);
        self.history.undo.push((label, cur));
        self.history.merge_key = None;
        self.sanitize_after_replacement(edited.as_ref());
        self.bump();
        true
    }

    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    /// The project as it is now is the saved one (after opening or saving it).
    pub fn mark_saved(&mut self) {
        self.saved_project = self.project.clone();
        self.saved_revision = self.revision;
    }

    /// The project has changes its file doesn't (a session restored with unsaved changes): it
    /// stays modified until it is saved, whatever is undone.
    pub fn mark_unsaved(&mut self) {
        self.saved_project = Arc::new(Project::default());
        self.bump();
    }

    /// Replace the whole project (open/new), resetting history and state.
    pub fn replace_project(&mut self, mut p: Project, path: Option<String>) {
        if self.stop_render()
            && let Some(mut job) = self.render_job.take()
        {
            // Let the worker notice the cancel; its updates refer to the old project.
            job.wait();
        }
        self.render_job = None;
        self.stored_paths = Default::default();
        self.stop_track();
        self.track_job = None;
        self.stop_mask_track();
        self.mask_job = None;
        self.stop_warp();
        self.warp_job = None;
        self.warp_pending.clear();
        self.stop_camera();
        self.camera_job = None;
        self.camera_pending.clear();
        self.stop_roto();
        self.roto_job = None;
        self.roto_pending.clear();
        self.drop_tasks();
        self.script_ui.project_epoch = self.script_ui.project_epoch.wrapping_add(1);
        p.fix_next_id();
        upgrade_effects(&mut p);
        // The media layer keys decoded frames by path: files of the new project may have changed
        // since an earlier project decoded them (#1070).
        for it in p.items.values() {
            if let effectcraft_project::ItemKind::Footage(f) = &it.kind {
                self.forget_decoded(f);
            }
        }
        self.layer_cache.clear();
        self.project = Arc::new(p);
        // Its files may have changed since they were last stamped.
        self.disk_salt = None;
        self.history = History::default();
        self.state = EditorState { snapping: true, view_layout: 1, ..Default::default() };
        self.path = path;
        self.bump();
        self.mark_saved();
        let first = self.project.comps().next().map(|(id, _)| *id);
        if let Some(c) = first {
            self.open_comp(c);
        }
    }
}

/// Bring effect instances saved by earlier versions up to date with the effect registry
/// (renamed / regrouped / added parameters, reordered popups; see
/// [`effectcraft_effects::migrate`]), and a project from before
/// [`effectcraft_project::SCHEMA_EFFECT_SPACE`] into effect space
/// ([`effectcraft_effects::migrate::to_effect_space`]). Stamps the current schema.
pub fn upgrade_effects(p: &mut Project) {
    let to_effect_space = p.schema < effectcraft_project::SCHEMA_EFFECT_SPACE;
    // Effect bounds per layer: (size, top-left in layer coordinates).
    let mut bounds = std::collections::HashMap::new();
    for (cid, c) in p.comps() {
        for l in &c.layers {
            bounds.insert((*cid, l.id), effectcraft_render::effect_bounds(p, c, l));
        }
    }
    let mut next = p.next_id;
    for (cid, it) in p.items.iter_mut() {
        let effectcraft_project::ItemKind::Comp(c) = &mut it.kind else { continue };
        for l in &mut std::sync::Arc::make_mut(c).layers {
            let (size, origin) = bounds.get(&(*cid, l.id)).copied().unwrap_or(([100.0, 100.0], [0.0; 2]));
            // Effect space starts at the bounds' top-left: positions move by minus that corner.
            // Adjustment layers run their effects on the comp below as it is: nothing moves.
            let shift = |o: [f64; 2]| [-o[0], -o[1]];
            let d = if l.switches.adjustment { [0.0; 2] } else { shift(origin) };
            let clone_d = |src: Option<u64>| match src {
                Some(id) => bounds.get(&(*cid, effectcraft_project::LayerId(id))).map_or([0.0; 2], |b| shift(b.1)),
                None => d,
            };
            let Some(fx) = l.props.sub_mut("effects") else { continue };
            for n in &mut fx.children {
                let effectcraft_project::Node::Group(g) = n else { continue };
                let effectcraft_project::GroupKind::Effect { effect } = &g.kind else { continue };
                if let Some(spec) = effectcraft_effects::find(effect) {
                    effectcraft_effects::migrate::upgrade_instance(spec, g, &mut effectcraft_project::build::Ids(&mut next), size);
                    if to_effect_space {
                        effectcraft_effects::migrate::to_effect_space(spec, g, size, d, &clone_d);
                    }
                }
            }
        }
    }
    p.next_id = next;
    p.schema = effectcraft_project::SCHEMA_VERSION;
}
