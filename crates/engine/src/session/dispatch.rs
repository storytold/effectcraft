use super::{Arc, EngineError, Event, Project, Result, Session, Value, commands, guard};
use crate::{UiGate, UiOutcome};

impl Session {
    pub fn new() -> Session {
        Session::default()
    }

    /// Mutable project. Marks the document dirty until [`Session::bump`].
    pub fn project_mut(&mut self) -> &mut Project {
        Arc::make_mut(self.project.write())
    }

    pub fn set_project(&mut self, project: Arc<Project>) {
        *self.project.write() = project;
    }

    pub fn swap_project(&mut self, project: Arc<Project>) -> Arc<Project> {
        std::mem::replace(self.project.write(), project)
    }

    /// Run a command by id.
    pub fn execute(&mut self, id: &str, params: Value) -> Result<Value> {
        let origin = if self.running_for_script() { craft_command::Origin::Script } else { craft_command::Origin::Execute };
        self.execute_as(id, params, origin)
    }

    /// Run a command by id for `origin`, which the gates see on the `Call`. A plug-in's `cmd.execute`
    /// passes `Origin::Plugin`.
    pub fn execute_as(&mut self, id: &str, params: Value, origin: craft_command::Origin) -> Result<Value> {
        if self.script_ui.render_host.is_some() && !crate::scriptui::allowed_during_render(id) {
            return Err(EngineError::Other(crate::scriptui::RENDER_WAIT_MESSAGE.into()));
        }
        let Some(cmd) = self.registry.resolve(id) else {
            return Err(EngineError::UnknownCommand(id.to_string()));
        };
        // Idle fast path: nothing can observe the call, so skip the cause, gate and depth bookkeeping.
        if self.hooks.is_idle() {
            return self.execute_resolved(id, None, params, cmd, origin);
        }
        let id_arc: Arc<str> = Arc::from(id);
        let hooks = self.hooks.clone();
        let depth = self.exec_depth.saturating_add(1);
        hooks.with_cause(craft_command::Cause::Command { id: Arc::clone(&id_arc), depth, origin }, || {
            self.execute_resolved(id, Some(Arc::clone(&id_arc)), params, cmd, origin)
        })
    }

    fn execute_resolved(
        &mut self,
        id: &str,
        live: Option<Arc<str>>,
        params: Value,
        cmd: craft_command::CommandEntry<'static, Session, EngineError>,
        origin: craft_command::Origin,
    ) -> Result<Value> {
        let journal = match &cmd {
            craft_command::CommandEntry::Static(spec) => spec.journal,
            craft_command::CommandEntry::Dynamic(cmd) => cmd.journal,
        };
        let seq = self.call_seq;
        self.call_seq = self.call_seq.saturating_add(1);
        let (hooks, info, params) = match live {
            Some(id) => {
                let hooks = self.hooks.clone();
                let mut call = craft_command::Call { id, params, depth: self.exec_depth.saturating_add(1), origin, seq };
                // Nested execute does not flush. An inner flush would report a write the outer command still bumps.
                let clock = Arc::clone(&self.clock);
                let verdict = hooks.before(self, &mut call, &*clock);
                let params = call.params.clone();
                let info = call.info();
                if let craft_command::Verdict::Veto { reason, by } = verdict {
                    return self.finish_call(
                        Some((&hooks, &info)),
                        craft_command::Outcome::Vetoed,
                        Err(craft_command::CommandError::Vetoed { reason, by }.into()),
                    );
                }
                (Some(hooks), Some(info), params)
            }
            None => (None, None, params),
        };
        if let Err(why) = self.command_enabled(&cmd) {
            let explicit = (["layer", "layers", "prop", "keys"].iter().any(|k| params.get(k).is_some()) && commands::has_comp(self).is_ok())
                || ["item", "items"].iter().any(|k| params.get(k).is_some())
                || params.get("comp").is_some_and(|c| !c.is_null() && self.resolve_comp(Some(c)).is_ok());
            if !explicit {
                return self.finish_call(hooks.as_ref().zip(info.as_ref()), craft_command::Outcome::Disabled, Err(EngineError::Disabled(id.to_string(), why)));
            }
        }
        self.exec_depth += 1;
        let _depth = hooks.as_ref().map(|h| h.enter());
        let ran = guard::guarded(&format!("command `{id}`"), || self.run_resolved(&cmd, &params));
        self.exec_depth -= 1;
        let _depth = hooks.as_ref().map(|h| h.leave());
        let (outcome, result) = match ran {
            Ok(value) => (craft_command::Outcome::Ok, Ok(value)),
            Err(err) => (craft_command::Outcome::Err, Err(err)),
        };
        if result.is_ok() {
            self.learn_observe(id, &params);
            if journal {
                self.journal.push((id.to_string(), params));
                if self.journal.len() > 10_000 {
                    self.journal.drain(..1000);
                }
            }
        }
        self.finish_call(hooks.as_ref().zip(info.as_ref()), outcome, result)
    }

    /// `after` runs only for a live call (`live` is `Some`); `flush` runs for every outermost call.
    fn finish_call(
        &mut self,
        live: Option<(&craft_command::Hooks<Session>, &craft_command::CallInfo)>,
        outcome: craft_command::Outcome,
        result: Result<Value>,
    ) -> Result<Value> {
        if let Some((hooks, info)) = live {
            hooks.after(info, &outcome, &*self.clock);
        }
        if self.exec_depth == 0 {
            self.hooks.flush(&mut self.project, self.revision);
        }
        result
    }

    /// Gate a command `execute` does not resolve. A veto returns `Err` after `after` has run.
    /// The caller runs the UI body, then [`Session::close_ui`]. Nested `execute` does not flush
    /// until this gate closes and `exec_depth` is 0.
    pub fn open_ui(&mut self, id: &str, params: &Value) -> std::result::Result<UiGate, String> {
        let hooks = self.hooks.clone();
        self.call_seq = self.call_seq.saturating_add(1);
        let mut call = craft_command::Call {
            id: Arc::from(id),
            params: params.clone(),
            depth: self.exec_depth.saturating_add(1),
            origin: craft_command::Origin::Ui,
            seq: self.call_seq,
        };
        let clock = Arc::clone(&self.clock);
        let verdict = hooks.before(self, &mut call, &*clock);
        let info = call.info();
        if let craft_command::Verdict::Veto { reason, by } = verdict {
            let err = craft_command::CommandError::Vetoed { reason, by };
            let message = err.to_string();
            let _ = self.finish_call(Some((&hooks, &info)), craft_command::Outcome::Vetoed, Err(err.into()));
            return Err(message);
        }
        self.exec_depth = self.exec_depth.saturating_add(1);
        let _depth = hooks.enter();
        Ok(UiGate { info, params: call.params })
    }

    /// Pair of [`Session::open_ui`]. `after` runs, then `flush` when this call is the outermost one.
    pub fn close_ui(&mut self, gate: UiGate, outcome: UiOutcome) {
        let hooks = self.hooks.clone();
        self.exec_depth = self.exec_depth.saturating_sub(1);
        let _depth = hooks.leave();
        let outcome = match outcome {
            UiOutcome::Ok => craft_command::Outcome::Ok,
            UiOutcome::Err => craft_command::Outcome::Err,
            UiOutcome::Panicked => craft_command::Outcome::Panicked,
        };
        let _ = self.finish_call(Some((&hooks, &gate.info)), outcome, Ok(Value::Null));
    }

    /// Dynamic commands that have a menu path. Static menu rows stay in `menus::entries`.
    pub fn dynamic_menu_rows(&self) -> Vec<(String, String, Vec<String>)> {
        let mut rows = Vec::new();
        self.registry.for_each(|view| {
            if !view.dynamic {
                return;
            }
            let path = match view.menu {
                craft_command::Menu::Static(parts) => parts.iter().map(|part| (*part).to_string()).collect::<Vec<_>>(),
                craft_command::Menu::Shared(parts) => parts.iter().map(|part| part.to_string()).collect(),
            };
            if path.is_empty() {
                return;
            }
            rows.push((view.id.to_string(), view.label.to_string(), path));
        });
        rows
    }

    /// Journaled commands with an empty menu and an empty params doc. The command palette adds these.
    pub fn palette_commands(&self) -> Vec<(String, String, Option<String>)> {
        let mut rows = Vec::new();
        self.registry.for_each(|view| {
            let empty_menu = match view.menu {
                craft_command::Menu::Static(parts) => parts.is_empty(),
                craft_command::Menu::Shared(parts) => parts.is_empty(),
            };
            if empty_menu && view.journal && view.params == "{}" {
                rows.push((view.id.to_string(), view.label.to_string(), view.shortcut.map(str::to_string)));
            }
        });
        rows
    }

    /// Built-in rows plus dynamic rows.
    pub fn command_count(&self) -> usize {
        let mut count = 0usize;
        self.registry.for_each(|_| count += 1);
        count
    }

    pub(crate) fn command_enabled(&self, cmd: &craft_command::CommandEntry<'static, Session, EngineError>) -> std::result::Result<(), String> {
        match cmd {
            craft_command::CommandEntry::Static(spec) => (spec.enabled)(self),
            craft_command::CommandEntry::Dynamic(cmd) => match &cmd.enablement {
                // The session always holds a project. HasDocument means a comp is open.
                craft_command::Enablement::Always => Ok(()),
                craft_command::Enablement::HasDocument => commands::has_comp(self),
                craft_command::Enablement::HasSelection => {
                    if commands::has_layers(self).is_ok() || commands::has_project_selection(self).is_ok() {
                        Ok(())
                    } else {
                        Err("nothing selected".into())
                    }
                }
                craft_command::Enablement::Cached(state) => state.clone(),
            },
        }
    }

    fn run_resolved(&mut self, cmd: &craft_command::CommandEntry<'static, Session, EngineError>, params: &Value) -> Result<Value> {
        match cmd {
            craft_command::CommandEntry::Static(spec) => (spec.run)(self, params),
            craft_command::CommandEntry::Dynamic(cmd) => (cmd.run)(self, params),
        }
    }

    /// [`Session::execute`] for agents and scripts (MCP, CLI, control channel): unknown top-level
    /// parameters are rejected with the accepted keys instead of being silently ignored.
    pub fn execute_checked(&mut self, id: &str, params: Value) -> Result<Value> {
        match self.registry.resolve(id) {
            Some(craft_command::CommandEntry::Static(spec)) => commands::check_params_doc(spec.id, spec.params, &params)?,
            Some(craft_command::CommandEntry::Dynamic(cmd)) => commands::check_params_doc(cmd.id.as_ref(), cmd.params.as_ref(), &params)?,
            None => return Err(EngineError::UnknownCommand(id.to_string())),
        }
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
        self.registry.resolve(id).is_some_and(|cmd| self.command_enabled(&cmd).is_ok())
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.events.push(Event::Toast { message: msg.into(), error: false });
    }

    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }
}
