use super::*;

/// `t` as a current time of `comp`: a frame within its duration.
fn comp_frame_at(comp: &Comp, t: Tick) -> Tick {
    comp.frame_rate.snap_nearest(t.min(comp.duration - comp.frame_duration()).max(Tick::ZERO))
}

impl Session {
    /// Keep selections valid after the project was replaced by another state (undo, redo, a
    /// history jump, a rollback): the same layer id can name a different document, so the
    /// on-canvas editor gets a fresh IME epoch only when its own document changed.
    ///
    /// Normally `before` is [`commands::text_edit::active_editor_snapshot`] taken from the
    /// state this call is about to replace. Atomic batch rollback is the exception: it has
    /// already restored the pre-batch state and passes the snapshot of the failing intermediate
    /// state, so a transient restart or component switch inside the batch still rejects reviving
    /// the old composition. A restored state that still has the same composition, layer and
    /// editor session and shows byte/style-equivalent text (an opacity or other property undo)
    /// keeps the editor's owner and selection; a different document, comp, layer or session clamps the
    /// selection and starts a new epoch. A closed editor stays closed, and a fallback comp's
    /// matching layer id never inherits the old comp's session. `None` means no editor was open
    /// when the snapshot was taken; if one is open after the replacement it is treated as an
    /// unverified document replacement, so it never inherits an old composition.
    pub fn sanitize_after_replacement(&mut self, before: Option<&commands::text_edit::EditorSnapshot>) {
        self.sanitize_state();
        commands::text_edit::after_replacement(self, before);
    }

    /// Drop selections that no longer exist. Each comp keeps its own selection (#1001); a layer id
    /// counts only inside the comp that owns it, so the active comp never sees another comp's layers.
    pub fn sanitize_state(&mut self) {
        let p = self.project.clone();
        self.state.open_comps.retain(|c| p.comp(*c).is_some());
        self.state.comp_selections.retain(|cid, sel| {
            let Some(c) = p.comp(*cid) else { return false };
            let ids: Vec<LayerId> = c.layers.iter().map(|l| l.id).collect();
            sel.layers.retain(|l| ids.contains(l));
            sel.props.retain(|(l, _)| ids.contains(l));
            sel.keys.retain(|k| ids.contains(&k.layer));
            sel.vertices.retain(|v| ids.contains(&v.layer));
            !sel.is_empty()
        });
        if self.state.active_comp.is_some_and(|c| p.comp(c).is_none()) {
            self.clear_live_selection();
            // A matching layer id in the fallback comp is not the document being edited.
            self.state.text_edit = None;
            self.state.active_comp = self.state.open_comps.first().copied();
            if let Some(id) = self.state.active_comp {
                self.recall_selection(id);
            }
        }
        if let Some(c) = self.active_comp() {
            let ids: Vec<LayerId> = c.layers.iter().map(|l| l.id).collect();
            self.state.selected_layers.retain(|l| ids.contains(l));
            self.state.selected_props.retain(|(l, _)| ids.contains(l));
            self.state.selected_keys.retain(|k| ids.contains(&k.layer));
            self.state.selected_vertices.retain(|v| ids.contains(&v.layer));
        }
        if let Some((l, t)) = self.state.current_track
            && self.active_comp().and_then(|c| c.layer(l)).and_then(|l| l.tracker(t)).is_none()
        {
            self.state.current_track = None;
        }
        self.state.project_selection.retain(|i| p.item(*i).is_some());
        commands::text_edit::sanitize(self);
    }

    pub fn active_comp(&self) -> Option<&Comp> {
        self.project.comp(self.state.active_comp?)
    }

    /// [`Session::active_comp`] as a shared snapshot (an `Arc` clone, not a deep copy).
    pub fn active_comp_arc(&self) -> Option<std::sync::Arc<Comp>> {
        self.project.comp_arc(self.state.active_comp?)
    }

    pub fn active_comp_id(&self) -> Option<ItemId> {
        self.state.active_comp.filter(|c| self.project.comp(*c).is_some())
    }

    /// Current time of a comp.
    pub fn time_of(&self, comp: ItemId) -> Tick {
        self.state.times.get(&comp).copied().unwrap_or(Tick::ZERO)
    }

    /// Current time of the active comp.
    pub fn time(&self) -> Tick {
        self.state.active_comp.and_then(|c| self.state.times.get(&c).copied()).unwrap_or(Tick::ZERO)
    }

    /// Move the active comp's current time.
    ///
    /// Time is an external boundary for the on-canvas text editor too: an animated Source Text
    /// can show a different document at another time. The document is snapshotted only while an
    /// editor is open, so playback with no text editor does no text copying.
    pub fn set_time(&mut self, t: Tick) {
        if let Some(c) = self.state.active_comp {
            let t = self.project.comp(c).map_or(t, |comp| comp_frame_at(comp, t));
            let edited = commands::text_edit::active_editor_snapshot(self);
            self.state.times.insert(c, t);
            if self.prefs.general.sync_time_related_items {
                // Direct call: this `set_time` owns the editor snapshot for both time changes,
                // so a synced move does not take a second snapshot or start a second epoch.
                self.sync_related_times_inner(c, t);
            }
            commands::text_edit::after_external_change(self, edited.as_ref());
        }
    }

    /// Settings ▸ General ▸ Synchronize Time of All Related Items: moving the current time of
    /// `comp` moves the current time of the comps nested in it (through their precomp layers'
    /// timing) and of the comps it is nested in, so every related viewer shows the same moment.
    pub fn sync_related_times(&mut self, comp: ItemId, t: Tick) {
        // Moving a related comp can move the active one, so the editor's document is checked
        // here too; the snapshot is skipped entirely when no text editor is open.
        let edited = commands::text_edit::active_editor_snapshot(self);
        self.sync_related_times_inner(comp, t);
        commands::text_edit::after_external_change(self, edited.as_ref());
    }

    /// Sets the time of every comp related to `comp` through precomp layers: down into the comps
    /// it nests (through each layer's timing) and up into the comps nesting it. A seen-set keeps
    /// cycles and diamonds finite; each found time is clamped to its comp's last frame, snapped to
    /// the nearest frame and stored in `state.times`. The caller owns the text-editor boundary.
    fn sync_related_times_inner(&mut self, comp: ItemId, t: Tick) {
        let mut seen = std::collections::BTreeSet::from([comp]);
        let mut todo = vec![(comp, t)];
        let mut found = vec![];
        while let Some((cid, ct)) = todo.pop() {
            if cid != comp {
                found.push((cid, ct));
            }
            let Some(c) = self.project.comp(cid) else { continue };
            // Down: the comps this one nests.
            for l in &c.layers {
                if let effectcraft_project::LayerSource::Comp { item } = &l.source
                    && seen.insert(*item)
                {
                    todo.push((*item, l.layer_time(ct)));
                }
            }
            // Up: the comps nesting this one.
            for iid in self.project.items.keys() {
                if let Some(pc) = self.project.comp(*iid)
                    && let Some(l) = pc.layers.iter().find(|l| matches!(&l.source, effectcraft_project::LayerSource::Comp { item } if *item == cid))
                    && seen.insert(*iid)
                {
                    todo.push((*iid, l.comp_time(ct)));
                }
            }
        }
        for (cid, ct) in found {
            if let Some(c) = self.project.comp(cid) {
                let ct = c.frame_rate.snap_nearest(ct.clamp(Tick::ZERO, (c.duration - c.frame_duration()).max(Tick::ZERO)));
                self.state.times.insert(cid, ct);
            }
        }
    }

    /// Make `id` the active comp, parking the previous comp's selection and restoring this one's.
    pub fn activate_comp(&mut self, id: ItemId) {
        if self.project.comp(id).is_none() || self.state.active_comp == Some(id) {
            return;
        }
        self.remember_selection();
        self.state.active_comp = Some(id);
        self.recall_selection(id);
        self.state.current_track = None;
        // A text-edit session belongs to the comp that owned it. Switching comps ends it; even
        // a same-numbered layer in the new comp must not inherit the old IME composition.
        self.state.text_edit = None;
    }

    /// Leave no comp active, keeping the one that was active in [`EditorState::comp_selections`].
    pub fn activate_comp_none(&mut self) {
        self.remember_selection();
        self.state.active_comp = None;
        self.clear_live_selection();
        self.state.current_track = None;
        self.state.text_edit = None;
    }

    fn remember_selection(&mut self) {
        let Some(id) = self.state.active_comp else { return };
        let sel = CompSelection {
            layers: self.state.selected_layers.clone(),
            props: self.state.selected_props.clone(),
            keys: self.state.selected_keys.clone(),
            vertices: self.state.selected_vertices.clone(),
        };
        if sel.is_empty() {
            self.state.comp_selections.remove(&id);
        } else {
            self.state.comp_selections.insert(id, sel);
        }
    }

    fn recall_selection(&mut self, id: ItemId) {
        match self.state.comp_selections.remove(&id) {
            Some(sel) => {
                self.state.selected_layers = sel.layers;
                self.state.selected_props = sel.props;
                self.state.selected_keys = sel.keys;
                self.state.selected_vertices = sel.vertices;
            }
            None => self.clear_live_selection(),
        }
    }

    fn clear_live_selection(&mut self) {
        self.state.selected_layers.clear();
        self.state.selected_props.clear();
        self.state.selected_keys.clear();
        self.state.selected_vertices.clear();
    }

    /// Open a comp in the viewer/timeline and make it active.
    pub fn open_comp(&mut self, id: ItemId) {
        if self.project.comp(id).is_none() {
            return;
        }
        if !self.state.open_comps.contains(&id) {
            self.state.open_comps.push(id);
        }
        if self.state.active_comp != Some(id) {
            let previous = self.state.active_comp;
            self.state.recent_comps.retain(|c| *c != id && Some(*c) != previous);
            if let Some(previous) = previous {
                self.state.recent_comps.insert(0, previous);
                self.state.recent_comps.truncate(32);
            }
        }
        self.activate_comp(id);
        self.events.push(Event::OpenComp(id));
    }

    /// The open compositions and their current times, as a project file records them
    /// ([`effectcraft_project::SavedView`]).
    pub fn saved_view(&self) -> effectcraft_project::SavedView {
        let comp_times = self.state.times.iter().filter(|(id, _)| self.project.comp(**id).is_some()).map(|(id, t)| (*id, *t)).collect();
        effectcraft_project::SavedView { active_comp: self.state.active_comp, open_comps: self.state.open_comps.clone(), comp_times }
    }

    /// Reopen the compositions a project file was saved with (#748), each at its saved current
    /// time (#1714). Ids that no longer name a comp are skipped; with none left the project stays
    /// on the comp [`Session::replace_project`] opened (its first), as files from before the view
    /// was saved do.
    pub fn restore_view(&mut self, view: &effectcraft_project::SavedView) {
        for (id, t) in &view.comp_times {
            if let Some(comp) = self.project.comp(*id) {
                let t = comp_frame_at(comp, *t);
                self.state.times.insert(*id, t);
            }
        }
        let mut open: Vec<ItemId> = Vec::new();
        for id in view.open_comps.iter().copied().filter(|id| self.project.comp(*id).is_some()) {
            if !open.contains(&id) {
                open.push(id);
            }
        }
        let Some(active) = view.active_comp.filter(|id| self.project.comp(*id).is_some()).or_else(|| open.first().copied()) else { return };
        self.state.open_comps = open;
        self.open_comp(active);
    }

    /// The 3D view camera of a comp's viewer (None = the active camera).
    pub fn view_camera(&self, comp: ItemId) -> Option<effectcraft_render::three_d::CameraState> {
        let c = self.project.comp(comp)?;
        if !c.has_3d() {
            return None;
        }
        self.state.views3d.get(&comp)?.override_camera(c.width as f64, c.height as f64)
    }

    /// Resolve a comp reference (item id number or name; `None`/null = the active comp).
    pub fn resolve_comp(&self, comp: Option<&Value>) -> Result<ItemId> {
        let p = match comp {
            Some(v) if !v.is_null() => serde_json::json!({ "comp": v }),
            _ => serde_json::json!({}),
        };
        commands::comp_id(self, &p)
    }
}
