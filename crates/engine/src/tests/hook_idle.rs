//! `Session::execute` idle fast path: calls stay visible once a gate or subscriber exists.
use std::sync::{Arc, Mutex};

use craft_command::{CallInfo, Interceptor, Interest, Outcome, Owner, Verdict};
use serde_json::json;

use crate::Session;

struct Spy {
    interest: Interest,
    before: Mutex<Vec<String>>,
    after: Mutex<Vec<(String, Outcome)>>,
    veto: bool,
}

impl Spy {
    fn new(veto: bool) -> Arc<Spy> {
        Arc::new(Spy { interest: Interest { globs: vec!["*".into()], priority: 0 }, before: Mutex::new(vec![]), after: Mutex::new(vec![]), veto })
    }
}

impl Interceptor<Session> for Spy {
    fn interest(&self) -> &Interest {
        &self.interest
    }
    fn before(&self, _s: &mut Session, call: &craft_command::Call) -> Verdict {
        self.before.lock().unwrap().push(call.id.to_string());
        if self.veto { Verdict::Veto { reason: "no".into(), by: "spy".into() } } else { Verdict::Pass }
    }
    fn after(&self, call: &CallInfo, out: &Outcome) {
        self.after.lock().unwrap().push((call.id.to_string(), *out));
    }
}

fn owner() -> Owner {
    Owner { plugin_id: Arc::from("test"), generation: 1 }
}

#[test]
fn fresh_session_is_idle_and_still_runs_commands() {
    let mut s = Session::default();
    assert!(s.hooks.is_idle());
    let seq = s.call_seq;
    s.execute("file.openDemoProject", json!({})).unwrap();
    assert!(s.call_seq > seq, "call_seq must advance on the idle path");
    assert!(s.active_comp_id().is_some());
    assert_eq!(s.exec_depth, 0);
}

#[test]
fn registered_gate_sees_calls() {
    let mut s = Session::default();
    let spy = Spy::new(false);
    s.hooks.register(&owner(), spy.clone());
    assert!(!s.hooks.is_idle());
    s.execute("file.openDemoProject", json!({})).unwrap();
    assert_eq!(spy.before.lock().unwrap().as_slice(), ["file.openDemoProject"]);
    assert_eq!(spy.after.lock().unwrap().as_slice(), [("file.openDemoProject".to_string(), Outcome::Ok)]);
}

#[test]
fn registered_gate_can_still_veto() {
    let mut s = Session::default();
    s.hooks.register(&owner(), Spy::new(true));
    assert!(s.execute("file.openDemoProject", json!({})).is_err());
    assert!(s.active_comp_id().is_none(), "vetoed command must not run");
}

#[test]
fn unregistering_the_gate_returns_to_idle() {
    let mut s = Session::default();
    let spy = Spy::new(false);
    s.hooks.register(&owner(), spy.clone());
    s.hooks.unregister_owner(&owner());
    assert!(s.hooks.is_idle());
    s.execute("file.openDemoProject", json!({})).unwrap();
    assert!(spy.before.lock().unwrap().is_empty());
}

#[test]
fn observer_subscriber_leaves_the_idle_path() {
    let mut s = Session::default();
    let sub = s.hooks.observer_queue().subscribe(None);
    assert!(!s.hooks.is_idle());
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.hooks.observer_queue().unsubscribe(sub);
    assert!(s.hooks.is_idle());
}

#[test]
fn change_subscriber_sees_command_cause() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let bus = s.hooks.change_bus::<Arc<effectcraft_project::Project>>();
    let sub = bus.subscribe();
    assert!(!s.hooks.is_idle());
    s.execute("layer.newSolid", json!({"name": "Spy"})).unwrap();
    let changes = bus.drain(sub);
    assert!(
        changes.iter().any(|c| matches!(&c.cause, craft_command::Cause::Command { id, .. } if &**id == "layer.newSolid")),
        "{:?}",
        changes.iter().map(|c| c.cause.clone()).collect::<Vec<_>>()
    );
}
