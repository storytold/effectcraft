//! Long calls over MCP (docs/mcp.md "Progress and cancellation"). While [`McpServer::serve`]
//! runs, a call that would hold the server for a long time runs in the background instead, and
//! the server keeps reading stdin:
//!
//! * **Background-job commands** ([`SPECS`]): `command_run` / `execute_command` of a command
//!   that starts an engine background job and waits for it (`renderQueue.render`, the analyses
//!   with `wait: true`, `jobs.wait`, …). The command starts with `wait: false`; its job is
//!   watched through `jobs.list` / `renderQueue.list` (the same commands in the headless session
//!   and over the bridge to the app); progress goes out as `notifications/progress` when the
//!   request carried `_meta.progressToken`; `notifications/cancelled` cancels the job
//!   (`jobs.cancel`); and the reply is what the waiting command returns.
//! * **Scripts** (`run_script`, headless): the script runs on a worker thread that owns the
//!   session meanwhile; `notifications/cancelled` stops it through the session's
//!   [`ScriptInterrupt`].
//!
//! Meanwhile `ping` is answered at once and `notifications/cancelled` for a queued request drops
//! it. During a background job other requests are served between polls (the job works on a
//! snapshot of the project or writes its results when it ends, as in the app). During a script
//! (which owns the session), and behind a long call that waits for the running one, requests
//! wait in line and are served in order.

use std::collections::VecDeque;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use effectcraft_engine::ScriptInterrupt;
use serde_json::{Value, json};

use crate::server::{McpServer, guarded_run};
use crate::tools::{self, Reply};
use crate::{Error, Result};

/// How often a job is polled (and at most how often progress is reported).
const POLL: Duration = Duration::from_millis(100);

/// When a command blocks.
#[derive(Clone, Copy)]
enum Waits {
    /// Unless `wait: false` (`renderQueue.render`).
    ByDefault,
    /// Only with `wait: true` (the analyses).
    Asked,
    /// Always (`jobs.wait`).
    Always,
}

/// Which engine jobs (`jobs.list` ids) a command's wait covers.
#[derive(Clone, Copy)]
enum Job {
    /// The render queue (watched through `renderQueue.list`).
    Render,
    /// A fixed job id (`track`, `warp`, `camera`, `roto`, …).
    Named(&'static str),
    /// The one its reply names (`{job: "task:<n>"}`); none when the reply names none.
    Reply,
    /// Every running generic task (`task:<n>`), including ones started meanwhile.
    Tasks,
}

/// How the reply of the waiting command is rebuilt once the job ended.
#[derive(Clone, Copy)]
enum Finish {
    /// The queue items it rendered, `failed` counting the ones that did not finish.
    Render,
    /// The start reply, with `running: false` and `progress: null` (the job is gone).
    Reply,
    /// The start reply plus `status` from this query (`{layer, effect, comp}` of the reply);
    /// fields the two share come from the query.
    Status(&'static str),
    /// The finished task's result; an error when it failed or was cancelled.
    Task,
    /// Run the command itself (`jobs.wait`: nothing is left to wait for).
    Rerun,
}

/// A blocking command that runs on a background job over MCP.
pub(crate) struct Spec {
    pub command: &'static str,
    waits: Waits,
    job: Job,
    finish: Finish,
}

/// The table of blocking commands with an engine background job. Everything else (including
/// batch tools) runs synchronously.
pub(crate) const SPECS: &[Spec] = &[
    Spec { command: "renderQueue.render", waits: Waits::ByDefault, job: Job::Render, finish: Finish::Render },
    Spec { command: "track.analyze", waits: Waits::Asked, job: Job::Named("track"), finish: Finish::Reply },
    Spec { command: "track.mask", waits: Waits::Asked, job: Job::Named("maskTrack"), finish: Finish::Reply },
    Spec { command: "warp.analyze", waits: Waits::Asked, job: Job::Named("warp"), finish: Finish::Status("warp.status") },
    Spec { command: "track.warpStabilizer", waits: Waits::Asked, job: Job::Named("warp"), finish: Finish::Reply },
    Spec { command: "camera.analyze", waits: Waits::Asked, job: Job::Named("camera"), finish: Finish::Status("camera.solveStatus") },
    Spec { command: "track.camera", waits: Waits::Asked, job: Job::Named("camera"), finish: Finish::Status("camera.solveStatus") },
    Spec { command: "camera.deletePoints", waits: Waits::Asked, job: Job::Named("camera"), finish: Finish::Reply },
    Spec { command: "roto.propagate", waits: Waits::Asked, job: Job::Named("roto"), finish: Finish::Status("roto.status") },
    Spec { command: "roto.freeze", waits: Waits::Asked, job: Job::Named("roto"), finish: Finish::Status("roto.status") },
    Spec { command: "contentFill.generate", waits: Waits::Asked, job: Job::Reply, finish: Finish::Task },
    Spec { command: "layer.sceneEditDetection", waits: Waits::Asked, job: Job::Reply, finish: Finish::Task },
    Spec { command: "footage.check", waits: Waits::Asked, job: Job::Reply, finish: Finish::Task },
    Spec { command: "jobs.wait", waits: Waits::Always, job: Job::Tasks, finish: Finish::Rerun },
];

/// What a long call runs.
pub enum What {
    Command { spec: &'static Spec, params: Value },
    Script,
}

/// A `tools/call` that runs as a long call.
pub struct LongCall {
    pub id: Value,
    pub tool: String,
    pub args: Value,
    pub token: Option<Value>,
    /// The request declared MCP 2026-07-28+ (its result then carries `resultType`).
    pub modern: bool,
    pub what: What,
}

/// Whether `msg` is a long call (valid arguments; scripts only headless).
pub fn long_call(bridge: bool, msg: &Value) -> Option<LongCall> {
    let id = msg.get("id").filter(|i| i.is_string() || i.is_number())?;
    if msg.get("method").and_then(Value::as_str) != Some("tools/call") {
        return None;
    }
    let p = msg.get("params")?;
    let name = p.get("name")?.as_str()?;
    let args = p.get("arguments").cloned().unwrap_or(json!({}));
    let tool = tools::available(bridge).find(|t| t.name == name)?;
    if tool.unknown_arg(&args).is_some() {
        return None; // the normal path reports it
    }
    let what = match name {
        "run_script" if !bridge => What::Script,
        "command_run" | "execute_command" => {
            let command = args.get("id").or_else(|| args.get("command")).and_then(Value::as_str)?;
            let spec = SPECS.iter().find(|s| s.command == command)?;
            let params = match args.get("params") {
                None | Some(Value::Null) => json!({}),
                Some(v) if v.is_object() => v.clone(),
                _ => return None,
            };
            let wait = params.get("wait").and_then(Value::as_bool);
            let blocks = match spec.waits {
                Waits::ByDefault => wait != Some(false),
                Waits::Asked => wait == Some(true),
                Waits::Always => true,
            };
            if !blocks {
                return None;
            }
            What::Command { spec, params }
        }
        _ => return None,
    };
    let token = p.get("_meta").and_then(|m| m.get("progressToken")).filter(|t| t.is_string() || t.is_number()).cloned();
    Some(LongCall { id: id.clone(), tool: name.to_string(), args, token, modern: crate::server::is_modern(p), what })
}

/// The server's input: stdin lines (read on their own thread) and the requests waiting in line.
pub(crate) struct Inbox {
    rx: Receiver<Option<String>>,
    queue: VecDeque<String>,
    open: bool,
}

impl Inbox {
    pub(crate) fn new(rx: Receiver<Option<String>>) -> Self {
        Inbox { rx, queue: VecDeque::new(), open: true }
    }

    /// The next line to handle: the queue first, then stdin. `None` once both are done.
    pub(crate) fn next(&mut self) -> Option<String> {
        if let Some(line) = self.queue.pop_front() {
            return Some(line);
        }
        if !self.open {
            return None;
        }
        match self.rx.recv() {
            Ok(Some(line)) => Some(line),
            _ => {
                self.open = false;
                None
            }
        }
    }

    /// A stdin line within [`POLL`] (after stdin closed: waits [`POLL`] and returns `None`).
    fn poll(&mut self) -> Option<String> {
        if !self.open {
            std::thread::sleep(POLL);
            return None;
        }
        match self.rx.recv_timeout(POLL) {
            Ok(Some(line)) => Some(line),
            Ok(None) | Err(RecvTimeoutError::Disconnected) => {
                self.open = false;
                None
            }
            Err(RecvTimeoutError::Timeout) => None,
        }
    }
}

fn send(out: &mut impl Write, v: &Value) -> std::io::Result<()> {
    out.write_all(v.to_string().as_bytes())?;
    out.write_all(b"\n")?;
    out.flush()
}

/// What to do with a line that arrived during a long call.
enum Seen {
    /// The long call itself was cancelled.
    Cancel,
    /// Answered, dropped or queued.
    Done,
    /// A request to serve now (when the session is free).
    Serve(String),
}

/// Sort a line that arrived during `call`: cancel it, answer `ping`, drop a cancelled queued
/// request, queue long calls and everything behind them.
fn triage(line: String, call: &LongCall, bridge: bool, inbox: &mut Inbox, out: &mut impl Write) -> std::io::Result<Seen> {
    let Ok(msg) = serde_json::from_str::<Value>(line.trim()) else {
        return Ok(if inbox.queue.is_empty() { Seen::Serve(line) } else { queue(inbox, line) });
    };
    match msg.get("method").and_then(Value::as_str) {
        Some("notifications/cancelled") => {
            let Some(rid) = msg.get("params").and_then(|p| p.get("requestId")) else { return Ok(Seen::Done) };
            if *rid == call.id {
                return Ok(Seen::Cancel);
            }
            // A request still waiting in line is dropped (MCP: no response is due); unknown and
            // completed ids are ignored.
            inbox.queue.retain(|q| serde_json::from_str::<Value>(q.trim()).ok().and_then(|m| m.get("id").cloned()).as_ref() != Some(rid));
            Ok(Seen::Done)
        }
        Some("ping") if msg.get("id").is_some() => {
            send(out, &json!({"jsonrpc": "2.0", "id": msg["id"], "result": {}}))?;
            Ok(Seen::Done)
        }
        _ if !inbox.queue.is_empty() => Ok(queue(inbox, line)),
        _ => match long_call(bridge, &msg) {
            // A second render while one runs is refused at once rather than waiting in line
            // (docs/mcp.md "Requests during a render").
            Some(next) if is_render(&next) && is_render(call) => Ok(Seen::Serve(line)),
            Some(_) => Ok(queue(inbox, line)),
            None => Ok(Seen::Serve(line)),
        },
    }
}

/// Whether `call` waits for a render-queue render.
fn is_render(call: &LongCall) -> bool {
    matches!(&call.what, What::Command { spec, .. } if matches!(spec.job, Job::Render))
}

fn queue(inbox: &mut Inbox, line: String) -> Seen {
    inbox.queue.push_back(line);
    Seen::Done
}

/// Turns job progress into strictly increasing MCP progress values: a job that starts a new
/// phase (its count goes back, or its total changes) continues from where the last one ended.
#[derive(Default)]
struct Meter {
    offset: f64,
    done: f64,
    total: f64,
}

impl Meter {
    fn update(&mut self, done: f64, total: f64) -> (f64, Option<f64>) {
        if done < self.done || (self.total > 0.0 && total > 0.0 && total != self.total) {
            self.offset += self.total.max(self.done);
        }
        (self.done, self.total) = (done, total);
        (self.offset + done, (total > 0.0).then_some(self.offset + total))
    }
    /// The end of the last phase (the final progress of a job that finished).
    fn end(&self) -> f64 {
        self.offset + self.total.max(self.done)
    }
}

/// Progress notifications for one call: strictly increasing, at most one per [`POLL`].
struct Reporter {
    token: Option<Value>,
    last: f64,
    at: Option<Instant>,
}

impl Reporter {
    fn report(&mut self, out: &mut impl Write, progress: f64, total: Option<f64>, message: &str, last_one: bool) -> std::io::Result<()> {
        let Some(token) = &self.token else { return Ok(()) };
        if !progress.is_finite() || progress <= self.last || !(last_one || self.at.is_none_or(|at| at.elapsed() >= POLL)) {
            return Ok(());
        }
        self.last = progress;
        self.at = Some(Instant::now());
        let mut params = json!({"progressToken": token, "progress": progress, "message": message});
        if let Some(t) = total.filter(|t| t.is_finite() && *t >= progress) {
            params["total"] = json!(t);
        }
        send(out, &json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": params}))
    }
}

/// A job's state at one poll.
struct Poll {
    running: bool,
    progress: Option<(f64, Option<f64>, String)>,
}

impl McpServer {
    fn exec_json(&mut self, command: &str, params: Value) -> Result<Value> {
        match self.guarded("command_run", &json!({"id": command, "params": params}))? {
            Reply::Json(v) => Ok(v),
            Reply::Image { .. } => Err(Error::Other("unexpected image".into())),
        }
    }

    /// Reply to `call` with `r` (the same post-processing as any tool call).
    fn reply(&mut self, call: &LongCall, r: Result<Reply>, out: &mut impl Write) -> std::io::Result<()> {
        let structured = call.modern || self.structured(&json!({}));
        let mut result = self.finish_call(&call.tool, r, structured);
        if call.modern {
            result["resultType"] = json!("complete");
        }
        send(out, &json!({"jsonrpc": "2.0", "id": call.id, "result": result}))?;
        self.flush_notifications(out)
    }

    /// Run a long call to its end or cancellation, handling the other input meanwhile.
    pub(crate) fn run_long(&mut self, call: LongCall, inbox: &mut Inbox, out: &mut impl Write) -> std::io::Result<()> {
        match &call.what {
            What::Script => self.run_script(&call, inbox, out),
            What::Command { spec, params } => self.run_job(&call, spec, params.clone(), inbox, out),
        }
    }

    fn run_job(&mut self, call: &LongCall, spec: &Spec, mut params: Value, inbox: &mut Inbox, out: &mut impl Write) -> std::io::Result<()> {
        let bridge = self.backend().is_bridge();
        // The comp the command acts on (the status query names it: the active comp may change).
        let comp = match spec.finish {
            Finish::Status(_) => self.exec_json("comp.info", params.get("comp").map(|c| json!({"comp": c})).unwrap_or(json!({}))).ok().map(|c| c["id"].clone()),
            _ => None,
        };
        let start = match spec.job {
            Job::Tasks => Ok(json!({})),
            _ => {
                params["wait"] = json!(false);
                self.exec_json(spec.command, params)
            }
        };
        let start = match start {
            Ok(v) => v,
            Err(e) => return self.reply(call, Err(e), out),
        };
        let items: Vec<u64> = start["items"].as_array().map(|a| a.iter().filter_map(|i| i["id"].as_u64()).collect()).unwrap_or_default();
        let ids: Vec<String> = match spec.job {
            Job::Named(id) => vec![id.to_string()],
            Job::Reply => start["job"].as_str().map(str::to_string).into_iter().collect(),
            Job::Render | Job::Tasks => vec![],
        };
        let mut reporter = Reporter { token: call.token.clone(), last: f64::NEG_INFINITY, at: None };
        let mut meter = Meter::default();
        let mut seen_tasks: Vec<String> = vec![];
        let mut cancelled = false;
        let mut failure = None;
        loop {
            let poll = match self.poll_job(spec.job, &ids, &items, &mut meter, &mut seen_tasks) {
                Ok(p) => p,
                Err(e) => {
                    failure = Some(e);
                    break;
                }
            };
            if let (Some((progress, total, message)), false) = (&poll.progress, cancelled) {
                reporter.report(out, *progress, *total, message, false)?;
            }
            if !poll.running {
                break;
            }
            let Some(line) = inbox.poll() else { continue };
            match triage(line, call, bridge, inbox, out)? {
                Seen::Cancel if !cancelled => {
                    cancelled = true;
                    if matches!(spec.job, Job::Tasks) {
                        return Ok(()); // jobs.wait: only the waiting stops
                    }
                    let cancel: Vec<String> = if matches!(spec.job, Job::Render) { vec!["render".into()] } else { ids.clone() };
                    for job in cancel {
                        let _ = self.exec_json("jobs.cancel", json!({"job": job}));
                    }
                }
                Seen::Cancel | Seen::Done => {}
                Seen::Serve(line) => {
                    if let Some(reply) = self.handle_line(&line) {
                        out.write_all(reply.as_bytes())?;
                        out.write_all(b"\n")?;
                        out.flush()?;
                    }
                    self.flush_notifications(out)?;
                }
            }
        }
        if let Some(s) = self.backend().session() {
            s.poll_background();
            s.drain_events();
        }
        if cancelled {
            return Ok(()); // MCP: no response for a cancelled request
        }
        let result = match failure {
            Some(e) => Err(e),
            None => self.finish_job(call, spec, &start, comp, &items, &meter, &seen_tasks, &mut reporter, out),
        };
        self.reply(call, result.map(Reply::Json), out)
    }

    /// Where the watched jobs stand (headless: after bringing every job up to date).
    fn poll_job(&mut self, job: Job, ids: &[String], items: &[u64], meter: &mut Meter, seen_tasks: &mut Vec<String>) -> Result<Poll> {
        if let Some(s) = self.backend().session() {
            s.poll_background();
            s.drain_events();
        }
        if let Job::Render = job {
            let list = self.exec_json("renderQueue.list", json!({}))?;
            let st = &list["progress"];
            let rendering = list["rendering"].as_bool().unwrap_or(false);
            let progress = st.as_object().filter(|_| rendering).map(|_| {
                let n = |k: &str| st[k].as_f64().unwrap_or(0.0);
                let (done, total, items_done, items_total) = (n("done"), n("total"), n("items_done"), n("items_total").max(items.len() as f64));
                let frac = if total > 0.0 { (done / total).min(1.0) } else { 0.0 };
                // the current item's frames count until it is done
                let progress = (items_done + if items_done < items_total { frac } else { 0.0 }).min(items_total);
                let message = format!("Rendering item {} of {}: frame {} of {}", (items_done + 1.0).min(items_total.max(1.0)), items_total, done, total);
                (progress, Some(items_total), message)
            });
            return Ok(Poll { running: rendering, progress });
        }
        let list = self.exec_json("jobs.list", json!({}))?;
        let running: Vec<&Value> = list["running"].as_array().map(|a| a.iter().filter(|j| j["running"] == true).collect()).unwrap_or_default();
        if let Job::Tasks = job {
            // jobs.wait: every task, counted as one unit each
            for j in &running {
                if let Some(id) = j["id"].as_str().filter(|id| id.starts_with("task:"))
                    && !seen_tasks.iter().any(|s| s == id)
                {
                    seen_tasks.push(id.to_string());
                }
            }
            let tasks: Vec<&&Value> = running.iter().filter(|j| j["id"].as_str().is_some_and(|id| id.starts_with("task:"))).collect();
            let fractions: f64 = tasks.iter().map(|j| j["fraction"].as_f64().unwrap_or(0.0)).sum();
            let progress = (seen_tasks.len().saturating_sub(tasks.len()) as f64) + fractions;
            let message = match tasks.first() {
                Some(j) => format!("{} task(s) running: {}", tasks.len(), j["label"].as_str().unwrap_or("task")),
                None => "Done".into(),
            };
            return Ok(Poll { running: !tasks.is_empty(), progress: Some((progress, Some(seen_tasks.len() as f64), message)) });
        }
        let mine: Vec<&&Value> = running.iter().filter(|j| j["id"].as_str().is_some_and(|id| ids.iter().any(|i| i == id))).collect();
        let Some(j) = mine.first() else { return Ok(Poll { running: false, progress: None }) };
        let (done, total) = (j["done"].as_f64().unwrap_or(0.0), j["total"].as_f64().unwrap_or(0.0));
        let (progress, total_now) = meter.update(done, total);
        let label = j["label"].as_str().unwrap_or("Job");
        let detail = j["message"].as_str().filter(|m| !m.is_empty()).map(|m| format!(" ({m})")).unwrap_or_default();
        let message = if total > 0.0 { format!("{label}{detail}: {done} of {total}") } else { format!("{label}{detail}") };
        Ok(Poll { running: true, progress: Some((progress, total_now, message)) })
    }

    /// The reply of the waiting command, once its job ended.
    #[allow(clippy::too_many_arguments)]
    fn finish_job(
        &mut self,
        call: &LongCall,
        spec: &Spec,
        start: &Value,
        comp: Option<Value>,
        items: &[u64],
        meter: &Meter,
        seen_tasks: &[String],
        reporter: &mut Reporter,
        out: &mut impl Write,
    ) -> Result<Value> {
        let end = match spec.job {
            Job::Render => items.len() as f64,
            Job::Tasks => seen_tasks.len() as f64,
            _ => meter.end(),
        };
        if end > 0.0 {
            reporter.report(out, end, Some(end), "Done", true).map_err(|e| Error::Other(e.to_string()))?;
        }
        match spec.finish {
            Finish::Render => {
                let list = self.exec_json("renderQueue.list", json!({}))?;
                let mine: Vec<Value> = list["items"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|i| i["id"].as_u64().is_some_and(|id| items.contains(&id)))
                    .collect();
                let failed = items.len().saturating_sub(mine.len()).saturating_add(mine.iter().filter(|i| i["statusLabel"].as_str() != Some("Done")).count());
                Ok(json!({"rendering": false, "items": mine, "failed": failed}))
            }
            Finish::Reply => {
                let mut v = start.clone();
                for (k, x) in [("running", json!(false)), ("progress", Value::Null)] {
                    if let Some(f) = v.get_mut(k) {
                        *f = x;
                    }
                }
                Ok(v)
            }
            Finish::Status(query) => {
                let mut p = json!({"layer": start["layer"], "effect": start["effect"]});
                if let Some(c) = comp.filter(|c| !c.is_null()) {
                    p["comp"] = c;
                }
                let status = self.exec_json(query, p)?;
                let mut v = start.clone();
                if let (Some(o), Some(st)) = (v.as_object_mut(), status.as_object()) {
                    for (k, x) in o.iter_mut() {
                        if let Some(s) = st.get(k) {
                            *x = s.clone();
                        }
                    }
                }
                v["status"] = status;
                Ok(v)
            }
            Finish::Task => {
                let Some(job) = start["job"].as_str() else { return Ok(start.clone()) };
                let list = self.exec_json("jobs.list", json!({}))?;
                let rec = list["finished"].as_array().and_then(|a| a.iter().rev().find(|r| r["id"] == job)).cloned();
                match rec {
                    Some(r) if r["status"] == "done" => Ok(r["result"].clone()),
                    Some(r) => Err(Error::Other(match r["message"].as_str().filter(|m| !m.is_empty()) {
                        Some(m) => m.to_string(),
                        None => r["status"].as_str().unwrap_or("failed").to_string(),
                    })),
                    None => Err(Error::Other(format!("{job} ended without a result (it was stopped)"))),
                }
            }
            Finish::Rerun => self.exec_json(spec.command, call.args.get("params").cloned().unwrap_or(json!({}))),
        }
    }

    /// `run_script` on a worker thread that owns the session while the script runs; this thread
    /// answers `ping`, queues other requests and stops the script on `notifications/cancelled`.
    fn run_script(&mut self, call: &LongCall, inbox: &mut Inbox, out: &mut impl Write) -> std::io::Result<()> {
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some(s) = self.backend().session() {
            s.script_interrupt = Some(ScriptInterrupt { cancel: cancel.clone(), deadline: None });
        }
        let mut cancelled = false;
        let (tool, args) = (call.tool.clone(), call.args.clone());
        let ran = std::thread::scope(|scope| -> std::io::Result<Result<Reply>> {
            let backend = self.backend();
            let worker = match std::thread::Builder::new().name("mcp-script".into()).spawn_scoped(scope, move || guarded_run(backend, &tool, &args)) {
                Ok(w) => w,
                Err(e) => return Ok(Err(Error::Other(format!("cannot start the script: {e}")))),
            };
            while !worker.is_finished() {
                let Some(line) = inbox.poll() else { continue };
                match triage(line, call, false, inbox, out) {
                    Ok(Seen::Cancel) => {
                        cancelled = true;
                        cancel.store(true, Ordering::Relaxed);
                    }
                    Ok(Seen::Done) => {}
                    Ok(Seen::Serve(line)) => inbox.queue.push_back(line),
                    Err(e) => {
                        // the client is gone: stop the script instead of waiting for it
                        cancel.store(true, Ordering::Relaxed);
                        return Err(e);
                    }
                }
            }
            Ok(worker.join().unwrap_or_else(|_| Err(Error::Other("internal error: the script worker stopped".into()))))
        });
        if let Some(s) = self.backend().session() {
            s.script_interrupt = None;
        }
        let ran = ran?;
        if cancelled {
            return Ok(()); // MCP: no response for a cancelled request
        }
        self.reply(call, ran, out)
    }
}

#[cfg(test)]
pub(crate) fn spec_commands() -> Vec<&'static str> {
    SPECS.iter().map(|s| s.command).collect()
}
