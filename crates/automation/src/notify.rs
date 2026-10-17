//! Server-initiated notifications (docs/mcp.md "Notifications"): `notifications/resources/updated`
//! for subscribed resources and `notifications/message` log entries. Requests queue them here;
//! the stdio loop writes them after each reply ([`crate::McpServer::take_notifications`]).

use std::collections::BTreeSet;

use effectcraft_engine::Event;
use serde_json::{Value, json};

use crate::{Backend, resources, tools};

/// RFC 5424 severities, lowest first (MCP `LoggingLevel`).
const LEVELS: [&str; 8] = ["debug", "info", "notice", "warning", "error", "critical", "alert", "emergency"];
/// Until the client sets a level: warnings and worse.
const DEFAULT_LEVEL: usize = 3;
/// Unflushed notifications kept at most (a caller that never takes them doesn't grow memory).
const OUTBOX_LIMIT: usize = 1000;
/// Expression errors listed in one log entry at most.
const EXPR_ERRORS_LISTED: usize = 20;

pub(crate) struct Notifier {
    outbox: Vec<Value>,
    subscriptions: BTreeSet<String>,
    level: usize,
    /// The project revision last seen (`project.summary.revision`).
    revision: Option<u64>,
    /// The active comp's expression errors last reported.
    expr_errors: Vec<String>,
}

impl Notifier {
    pub fn new(backend: &mut Backend) -> Self {
        let revision = backend.session().map(|s| s.revision);
        Self { outbox: Vec::new(), subscriptions: BTreeSet::new(), level: DEFAULT_LEVEL, revision, expr_errors: Vec::new() }
    }

    /// The queued notifications, oldest first.
    pub fn take(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.outbox)
    }

    fn push(&mut self, method: &str, params: Value) {
        if self.outbox.len() >= OUTBOX_LIMIT {
            self.outbox.remove(0);
        }
        self.outbox.push(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    /// `logging/setLevel`.
    pub fn set_level(&mut self, level: &str) -> Result<(), String> {
        self.level = LEVELS.iter().position(|l| *l == level).ok_or_else(|| format!("unknown log level `{level}`; one of: {}", LEVELS.join(", ")))?;
        Ok(())
    }

    /// Whether a `level` entry would be sent.
    pub fn logs(&self, level: &str) -> bool {
        LEVELS.iter().position(|l| *l == level).is_some_and(|i| i >= self.level)
    }

    /// Queue a `notifications/message` when `level` passes the client's threshold.
    pub fn log(&mut self, level: &str, data: Value) {
        if self.logs(level) {
            self.push("notifications/message", json!({"level": level, "logger": "effectcraft", "data": data}));
        }
    }

    /// `resources/subscribe`: `uri` must be readable (a resource or a template match).
    pub fn subscribe(&mut self, backend: &mut Backend, uri: &str) -> Result<(), resources::UriError> {
        resources::parse(uri)?;
        if self.subscriptions.is_empty() {
            // Changes from now on.
            self.revision = revision(backend).or(self.revision);
        }
        self.subscriptions.insert(uri.to_string());
        Ok(())
    }

    pub fn unsubscribe(&mut self, uri: &str) {
        self.subscriptions.remove(uri);
    }

    /// After a tool call: log the session's toasts, missing footage after open_project (headless)
    /// and new expression errors after an edit; tell subscribers what changed.
    pub fn after_tool(&mut self, backend: &mut Backend, name: &str, ok: bool) {
        if let Some(s) = backend.session() {
            let toasts: Vec<(String, bool)> = s
                .drain_events()
                .into_iter()
                .filter_map(|e| match e {
                    Event::Toast { message, error } => Some((message, error)),
                    _ => None,
                })
                .collect();
            let missing = if ok && name == "open_project" && self.logs("warning") {
                let r = s.execute("footage.check", json!({"wait": true}));
                s.drain_events(); // its own toast repeats the count
                r.ok()
            } else {
                None
            };
            for (message, error) in toasts {
                self.log(if error { "error" } else { "info" }, json!({"message": message}));
            }
            if let Some(r) = missing
                && let Some(n) = r.get("missing").and_then(Value::as_u64).filter(|n| *n > 0)
            {
                let message = format!("{n} footage item(s) are missing; find them with command_run file.findMissing {{\"what\":\"footage\"}}");
                self.log("warning", json!({"message": message, "missing": n, "checked": r.get("checked")}));
            }
        }
        self.observe(backend, tools::may_edit(name));
    }

    /// After any other request: subscribers learn about changes made elsewhere (in bridge mode,
    /// edits the user makes in the app; polled only while something is subscribed).
    pub fn after_request(&mut self, backend: &mut Backend) {
        self.observe(backend, false);
    }

    fn observe(&mut self, backend: &mut Backend, after_edit: bool) {
        let check_expr = after_edit && self.logs("warning");
        if self.subscriptions.is_empty() && !check_expr {
            return;
        }
        let Some(now) = revision(backend) else { return };
        if self.revision.replace(now) == Some(now) {
            return;
        }
        let changed: Vec<String> = self.subscriptions.iter().filter(|uri| resources::parse(uri).is_ok_and(|t| t.follows_project())).cloned().collect();
        for uri in changed {
            self.push("notifications/resources/updated", json!({"uri": uri}));
        }
        if check_expr {
            self.check_expressions(backend);
        }
    }

    /// Log the active comp's expression errors that weren't there before.
    fn check_expressions(&mut self, backend: &mut Backend) {
        let Ok(r) = backend.exec("expr.errors", json!({})) else {
            self.expr_errors.clear();
            return;
        };
        let now: Vec<String> = r.get("text").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
        let new: Vec<&String> = now.iter().filter(|e| !self.expr_errors.contains(e)).take(EXPR_ERRORS_LISTED).collect();
        if !new.is_empty() {
            let message = format!("{} expression error(s) in the active composition (expr.errors); the expressions are disabled until fixed", now.len());
            self.log("warning", json!({"message": message, "comp": r.get("comp"), "time": r.get("time"), "errors": new}));
        }
        self.expr_errors = now;
    }
}

/// The project's revision: it changes with every edit, undo and redo, never for queries. An app
/// too old to report it is fingerprinted by its summary instead.
fn revision(backend: &mut Backend) -> Option<u64> {
    if let Some(s) = backend.session() {
        return Some(s.revision);
    }
    let s = backend.exec("project.summary", json!({})).ok()?;
    if let Some(r) = s.get("revision").and_then(Value::as_u64) {
        return Some(r);
    }
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for k in ["path", "dirty", "items", "undo"] {
        s.get(k).map(Value::to_string).hash(&mut h);
    }
    Some(h.finish())
}
