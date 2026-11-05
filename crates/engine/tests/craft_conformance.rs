//! The craft-command conformance suite (T1 to T6) run against the EffectCraft command seam.
//!
//! UI commands use the same `open_ui` / `close_ui` path as `menus::invoke`. This test has no
//! egui context, so the UI body is a success with the rewritten params. The gate still runs.

#![allow(clippy::disallowed_methods)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use craft_command::conformance::{ChangeCause, CommandHost, OutcomeClass};
use craft_command::{Call, CallInfo, DynCommand, Enablement, Interceptor, Interest, Outcome, Owner, Verdict, Visibility};
use effectcraft_engine::project::Project;
use effectcraft_engine::shortcuts::{APP_SCOPE, Bindable};
use effectcraft_engine::{EngineError, Session, UiOutcome};
use serde_json::{Value, json};

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

struct Recorder {
    interest: Interest,
    veto: AtomicBool,
    before: Mutex<Vec<String>>,
    after: Mutex<Vec<OutcomeClass>>,
}

impl Interceptor<Session> for Recorder {
    fn interest(&self) -> &Interest {
        &self.interest
    }

    fn before(&self, _session: &mut Session, call: &Call) -> Verdict {
        lock(&self.before).push(call.id.to_string());
        if self.veto.load(Ordering::Relaxed) { Verdict::Veto { reason: "conformance".into(), by: "conformance".into() } } else { Verdict::Pass }
    }

    fn after(&self, _call: &CallInfo, outcome: &Outcome) {
        let class = match outcome {
            Outcome::Ok => OutcomeClass::Success,
            Outcome::Err | Outcome::Panicked => OutcomeClass::Failure,
            Outcome::Vetoed => OutcomeClass::Vetoed,
            Outcome::Disabled => OutcomeClass::Disabled,
        };
        lock(&self.after).push(class);
    }
}

/// The plug-in's own gate: matches every command id, passes the call on, and records the id.
struct PassGate {
    interest: Interest,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Interceptor<Session> for PassGate {
    fn interest(&self) -> &Interest {
        &self.interest
    }

    fn before(&self, _session: &mut Session, call: &Call) -> Verdict {
        lock(&self.seen).push(call.id.to_string());
        Verdict::Pass
    }
}

struct Host {
    session: Session,
    recorder: Arc<Recorder>,
    plugin_seen: Arc<Mutex<Vec<String>>>,
    sub: u64,
}

fn probe_bindable(id: &str) -> Bindable {
    Bindable {
        key: id.to_string(),
        command: id.to_string(),
        params: Value::Null,
        label: "Ping".into(),
        path: vec!["Plugin".into()],
        scope: APP_SCOPE.into(),
        defaults: Vec::new(),
    }
}

impl Host {
    fn map_result(result: Result<Value, EngineError>) -> OutcomeClass {
        match result {
            Ok(_) => OutcomeClass::Success,
            Err(EngineError::Disabled(_, _)) => OutcomeClass::Disabled,
            Err(EngineError::Command(craft_command::CommandError::Vetoed { .. })) => OutcomeClass::Vetoed,
            Err(_) => OutcomeClass::Failure,
        }
    }

    fn keep_outer_after(&self, start: usize) {
        let mut after = lock(&self.recorder.after);
        if after.len() > start + 1 {
            let outer = after.pop().unwrap_or(OutcomeClass::Failure);
            after.truncate(start);
            after.push(outer);
        }
    }
}

/// The command layer measures gate time against the wall clock and drops a gate that overruns
/// 2 ms or 6 ms per window. A busy machine
/// preempts the test thread for longer, which would turn this test into a load meter. A frozen
/// clock keeps the budgets out of the way; `craft_command`'s own tests cover the overrun.
struct FrozenClock;

impl craft_command::Clock for FrozenClock {
    fn now_ms(&self) -> u64 {
        1_000_000
    }
}

impl CommandHost for Host {
    fn fresh() -> Self {
        let session = Session { clock: Arc::new(FrozenClock), ..Session::default() };
        let recorder = Arc::new(Recorder {
            interest: Interest { globs: vec!["*".into()], priority: -1000 },
            veto: AtomicBool::new(false),
            before: Mutex::new(Vec::new()),
            after: Mutex::new(Vec::new()),
        });
        let owner = Owner { plugin_id: Arc::from("conformance"), generation: 1 };
        session.hooks.register(&owner, recorder.clone());
        let sub = session.hooks.change_bus::<Arc<Project>>().subscribe();
        Host { session, recorder, plugin_seen: Arc::new(Mutex::new(Vec::new())), sub }
    }

    fn command_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        self.session.registry.for_each(|view| ids.push(view.id.to_string()));
        ids
    }

    fn ui_command_ids(&self) -> Vec<String> {
        effectcraft_engine::ui_ids::ui_command_ids().iter().filter(|id| self.session.registry.resolve(id).is_none()).map(|id| (*id).to_string()).collect()
    }

    fn execute(&mut self, id: &str, params: &Value) -> OutcomeClass {
        let start = lock(&self.recorder.after).len();
        let class = Self::map_result(self.session.execute(id, params.clone()));
        self.keep_outer_after(start);
        class
    }

    fn invoke_ui(&mut self, id: &str, params: &Value) -> OutcomeClass {
        let start = lock(&self.recorder.after).len();
        let class = match self.session.open_ui(id, params) {
            Err(_) => OutcomeClass::Vetoed,
            Ok(gate) => {
                self.session.close_ui(gate, UiOutcome::Ok);
                OutcomeClass::Success
            }
        };
        self.keep_outer_after(start);
        class
    }

    fn fingerprint(&self) -> u64 {
        let bytes = serde_json::to_vec(&**self.session.project).unwrap_or_default();
        fnv(&bytes)
    }

    fn revision(&self) -> u64 {
        self.session.revision
    }

    fn set_veto_all(&mut self, enabled: bool) {
        self.recorder.veto.store(enabled, Ordering::Relaxed);
    }

    fn take_before_ids(&mut self) -> Vec<String> {
        std::mem::take(&mut *lock(&self.recorder.before))
    }

    fn take_plugin_gate_ids(&mut self) -> Vec<String> {
        std::mem::take(&mut *lock(&self.plugin_seen))
    }

    fn take_after(&mut self) -> Vec<OutcomeClass> {
        std::mem::take(&mut *lock(&self.recorder.after))
    }

    fn schema(&self, id: &str) -> Option<Value> {
        self.session.registry.schema(id).map(|value| (*value).clone())
    }

    fn script_len(&self) -> usize {
        1
    }

    fn run_script_step(&mut self, _index: usize) -> u64 {
        let _ = self.session.hooks.change_bus::<Arc<Project>>().drain(self.sub);
        let before = self.session.revision;
        let result = self.session.execute("comp.new", json!({"name": "Conformance", "width": 64, "height": 64}));
        let count = self.session.hooks.change_bus::<Arc<Project>>().drain(self.sub).len() as u64;
        assert!(result.is_ok(), "comp.new failed: {result:?}");
        assert!(self.session.revision > before, "comp.new did not bump");
        count
    }

    fn write_without_bump(&mut self) {
        let _ = self.session.project.write();
    }

    fn flush(&mut self) -> Vec<ChangeCause> {
        let bus = self.session.hooks.change_bus::<Arc<Project>>();
        let id = bus.subscribe();
        self.session.hooks.flush(&mut self.session.project, self.session.revision);
        bus.drain(id)
            .into_iter()
            .map(|change| if change.cause == craft_command::Cause::Untracked { ChangeCause::Untracked } else { ChangeCause::Other })
            .collect()
    }

    fn prepare_unregister(&mut self) -> (String, String) {
        let owner = Owner { plugin_id: Arc::from("probe"), generation: 7 };
        let id = "plugin.probe.ping";
        let cmd = DynCommand {
            id: Arc::from(id),
            label: Arc::from("Ping"),
            params: Arc::from("{}"),
            shortcut: None,
            menu: Arc::from([Arc::from("Plugin")]),
            owner: owner.clone(),
            enablement: Enablement::Always,
            visibility: Visibility { menu: true, palette: true, shortcut: true },
            journal: false,
            run: Arc::new(|_session, _params| Ok(Value::Null)),
        };
        self.session.registry.register(cmd).unwrap_or_else(|err| panic!("register probe: {err}"));
        let gate = Arc::new(PassGate { interest: Interest { globs: vec!["*".into()], priority: 100 }, seen: Arc::clone(&self.plugin_seen) });
        self.session.hooks.register(&owner, gate);
        let bound = self.session.keymaps.set(&probe_bindable(id), &["Cmd+Alt+Shift+F12".into()]);
        assert!(bound.is_ok(), "keymaps.set rejected the plugin command: {bound:?}");
        (owner.plugin_id.to_string(), id.to_string())
    }

    fn unregister_owner(&mut self, owner: &str) {
        let owner = Owner { plugin_id: Arc::from(owner), generation: 7 };
        self.session.registry.unregister_owner(&owner);
        self.session.hooks.unregister_owner(&owner);
    }

    fn resolves(&self, id: &str) -> bool {
        self.session.registry.resolve(id).is_some()
    }

    fn menu_lists(&self, id: &str) -> bool {
        self.session.dynamic_menu_rows().iter().any(|(command_id, _, _)| command_id == id)
    }

    fn user_shortcut_kept(&self, id: &str) -> bool {
        !self.session.keymaps.keys_of(&probe_bindable(id)).is_empty()
    }
}

#[test]
fn craft_conformance_t1_through_t6() {
    craft_command::conformance::run_all::<Host>().unwrap_or_else(|err| panic!("{err}"));
}
