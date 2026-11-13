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
        commands::find(id).is_some_and(|c| (c.enabled)(self).is_ok())
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.events.push(Event::Toast { message: msg.into(), error: false });
    }

    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }
}
