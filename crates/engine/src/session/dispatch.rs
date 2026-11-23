use super::{EngineError, Event, Result, Session, Value, commands, guard};

impl Session {
    pub fn new() -> Session {
        Session::default()
    }

    /// Run a command by id.
    pub fn execute(&mut self, id: &str, params: Value) -> Result<Value> {
        if self.script_ui.render_host.is_some() && !crate::scriptui::allowed_during_render(id) {
            return Err(EngineError::Other(crate::scriptui::RENDER_WAIT_MESSAGE.into()));
        }
        let spec = commands::find(id).ok_or_else(|| EngineError::UnknownCommand(id.to_string()))?;
        if let Some(why) = self.linked_source_command_problem(id, &params) {
            return Err(EngineError::Other(why.into()));
        }
        if let Err(why) = (spec.enabled)(self) {
            // Explicit targets (agents, scripts) don't need a UI selection.
            let explicit = (["layer", "layers", "prop", "keys"].iter().any(|k| params.get(k).is_some()) && commands::has_comp(self).is_ok())
                || ["item", "items"].iter().any(|k| params.get(k).is_some())
                // An explicit composition (the enablement looks at the active one).
                || params.get("comp").is_some_and(|c| !c.is_null() && self.resolve_comp(Some(c)).is_ok());
            if !explicit {
                return Err(EngineError::Disabled(id.to_string(), why));
            }
        }
        // Last-resort guard: a panicking command reports an error instead of crashing the app.
        let nesting = crate::replay::Nesting::enter(spec.journal);
        let caught = guard::caught();
        let r = guard::guarded(&format!("command `{id}`"), || (spec.run)(self, &params));
        let r = crate::replay::after_command(self, id, &params, &nesting, caught, r)?;
        self.learn_observe(id, &params);
        if spec.journal && !nesting.inside_journaled() {
            // Revert and Open Recent resolve a file the parameters may not name. Store
            // the successful read as an explicit open boundary while its resolved path is known.
            let entry = if matches!(id, "file.revert" | "file.openRecent") {
                self.path.as_ref().map(|path| ("file.open".to_string(), serde_json::json!({"path": path})))
            } else {
                None
            };
            self.journal.push(entry.unwrap_or_else(|| (id.to_string(), params)));
            if self.journal.len() > 10_000 {
                self.journal.drain(..1000);
            }
        }
        Ok(r)
    }

    /// [`Session::execute`] for agents and scripts (MCP, CLI, control channel): unknown top-level
    /// parameters are rejected with the accepted keys instead of being silently ignored.
    pub fn execute_checked(&mut self, id: &str, params: Value) -> Result<Value> {
        let spec = commands::find(id).ok_or_else(|| EngineError::UnknownCommand(id.to_string()))?;
        commands::check_params(spec, &params)?;
        self.execute(id, params)
    }

    /// [`Session::execute`] for a command a script asked for (the JavaScript engine's
    /// `app.run`, ScriptUI callbacks…). The command and everything it runs count as the script's:
    /// it may not change the settings that limit scripts (Settings ▸ Scripting & Expressions ▸
    /// Application Scripting), which only the user can, from the UI, CLI or control channel.
    pub fn execute_for_script(&mut self, id: &str, params: Value) -> Result<Value> {
        self.script_calls.0 = self.script_calls.0.saturating_add(1);
        let r = self.execute(id, params);
        self.script_calls.0 = self.script_calls.0.saturating_sub(1);
        r
    }

    /// Whether the command running was issued by a script ([`Session::execute_for_script`]).
    pub fn running_for_script(&self) -> bool {
        self.script_calls.0 > 0
    }

    /// Whether `id` names a known command whose `enabled` predicate returns `Ok` for this session;
    /// an unknown id or a predicate error reads as disabled.
    pub fn is_enabled(&self, id: &str) -> bool {
        if self.script_ui.render_host.is_some() && !crate::scriptui::allowed_during_render(id) {
            return false;
        }
        if self.linked_source_command_problem(id, &Value::Null).is_some() {
            return false;
        }
        commands::find(id).is_some_and(|c| (c.enabled)(self).is_ok())
    }

    /// Linked compositions expose a flat source, not editable footage interpretation. Check
    /// the operation itself, before selection-based enablement can be bypassed by an explicit
    /// item. Destination layer properties and ordinary project-item edits remain available.
    fn linked_source_command_problem(&self, id: &str, params: &Value) -> Option<&'static str> {
        use effectcraft_project::{FootageKind, ItemId, ItemKind, LayerSource};

        let linked = |id: ItemId| matches!(self.project.item(id).map(|i| &i.kind), Some(ItemKind::Footage(f)) if f.kind == FootageKind::LinkedComposition);
        if matches!(id, "file.collectFiles" | "essential.exportTemplate" | "file.exportAep" | "file.exportLottie" | "file.exportTimeline") {
            return self.project.items.keys().any(|id| linked(*id)).then_some(
                "Collect Files, templates and interchange export do not yet support linked compositions in this sandbox; render frames or save the EffectCraft project instead",
            );
        }
        if matches!(
            id,
            "file.replaceFootage"
                | "file.replaceWithPlaceholder"
                | "file.replaceWithSolid"
                | "file.replaceWithLayeredComp"
                | "file.replaceWithPrecomp"
                | "file.relinkFootage"
                | "footage.relink"
        ) {
            return self.project.items.keys().any(|id| linked(*id)).then_some(
                "Replace Footage and Relink are not yet supported in projects containing linked compositions; edit and save the source, then use project.refreshLinkedCompositions",
            );
        }
        let proxy = matches!(id, "file.setProxy" | "file.setProxyNone" | "file.useProxy" | "file.interpretProxy" | "file.createProxy");
        if !proxy && !matches!(id, "file.rememberInterpretation" | "file.applyInterpretation") {
            return None;
        }
        let explicit = params.get("items").or_else(|| params.get("item"));
        let targets: Vec<ItemId> = match explicit {
            Some(Value::Array(items)) => items.iter().filter_map(Value::as_u64).map(ItemId).collect(),
            Some(Value::Number(n)) => n.as_u64().map(ItemId).into_iter().collect(),
            Some(Value::String(name)) if proxy => self.project.find_by_name(name).map(|i| i.id).into_iter().collect(),
            _ if proxy => self.state.project_selection.clone(),
            _ => {
                let selected: Vec<ItemId> = self
                    .state
                    .project_selection
                    .iter()
                    .copied()
                    .filter(|id| matches!(self.project.item(*id).map(|i| &i.kind), Some(ItemKind::Footage(_))))
                    .collect();
                if !selected.is_empty() {
                    selected
                } else {
                    self.active_comp()
                        .into_iter()
                        .flat_map(|c| self.state.selected_layers.iter().filter_map(|id| c.layer(*id)))
                        .filter_map(|l| if let LayerSource::Footage { item } = l.source { Some(item) } else { None })
                        .collect()
                }
            }
        };
        targets.into_iter().any(linked).then_some(
            "Linked sources support color overrides through Interpret Footage. Copied interpretation and proxies are not supported; edit and save the source project, then use Reload Footage",
        )
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.events.push(Event::Toast { message: msg.into(), error: false });
    }

    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_project::{Footage, FootageKind, ItemKind};
    use serde_json::json;

    fn linked_session() -> (Session, crate::ItemId, crate::ItemId) {
        let mut s = Session::default();
        let project = std::sync::Arc::make_mut(&mut s.project);
        let linked = project.add_item(
            "Linked Title",
            Default::default(),
            None,
            ItemKind::Footage(Footage {
                kind: FootageKind::LinkedComposition,
                has_video: true,
                width: 32,
                height: 32,
                duration: crate::Tick::from_seconds_f64(2.0),
                ..Default::default()
            }),
        );
        let ordinary = project.add_item("Ordinary", Default::default(), None, ItemKind::Footage(Footage::default()));
        (s, linked, ordinary)
    }

    #[test]
    fn linked_sources_reject_footage_mutations_and_incomplete_collection() {
        let (mut s, linked, ordinary) = linked_session();
        let before = s.project.clone();
        for command in ["file.interpretFootage", "file.applyInterpretation", "file.rememberInterpretation", "file.setProxy"] {
            let error = s.execute(command, json!({"items": [linked.0], "item": linked.0, "pixelAspect": 2.0, "path": "unused"})).unwrap_err();
            assert!(error.to_string().contains("Linked sources"), "{command}: {error}");
            assert!(s.project.same_content(&before));
        }
        // Replacement can also retarget other missing media in the same folder: the sandbox
        // refuses the whole operation even when its explicit item is ordinary footage.
        for command in ["file.replaceFootage", "file.relinkFootage", "footage.relink", "file.collectFiles", "essential.exportTemplate"] {
            let error = s.execute(command, json!({"item": ordinary.0, "path": "unused", "folder": "unused"})).unwrap_err();
            assert!(error.to_string().contains("linked compositions"), "{command}: {error}");
            assert!(s.project.same_content(&before));
        }
    }

    #[test]
    fn linked_layer_instances_remain_editable_and_ordinary_footage_can_be_interpreted() {
        let (mut s, linked, ordinary) = linked_session();
        s.execute("comp.new", json!({"name": "Destination", "width": 64, "height": 64, "duration": 2})).unwrap();
        let layer = s.execute("layer.addItem", json!({"item": linked.0})).unwrap()["layer"].as_u64().unwrap();
        s.state.project_selection.clear();
        let error = s.execute("file.interpretFootage", json!({"pixelAspect": 2.0})).unwrap_err();
        assert!(error.to_string().contains("Linked sources"), "{error}");
        let before = s.execute("prop.get", json!({"layer": layer, "path": "transform/opacity"})).unwrap()["value"].as_f64().unwrap();
        s.execute("prop.set", json!({"layer": layer, "path": "transform/opacity", "value": 50.0})).unwrap();
        let value = s.execute("prop.get", json!({"layer": layer, "path": "transform/opacity"})).unwrap();
        assert!(value["value"].as_f64().unwrap() < before);
        s.state.project_selection = vec![linked];
        s.execute("file.interpretFootage", json!({"item": ordinary.0, "pixelAspect": 2.0})).unwrap();
        let Some(ItemKind::Footage(f)) = s.project.item(ordinary).map(|i| &i.kind) else { panic!("ordinary footage vanished") };
        assert!(f.pixel_aspect > 1.0);
        s.execute("project.rename", json!({"item": linked.0, "name": "Instance source"})).unwrap();
        assert_eq!(s.project.item(linked).unwrap().name, "Instance source");
    }
}
