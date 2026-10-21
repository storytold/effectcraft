//! `app.scheduleTask(code, delay, repeat)` / `app.cancelTask(id)`.
//!
//! Tasks belong to the script context (host, see [`crate::ui`]) that scheduled them and run in
//! it. A script file's one-shot tasks run right after the script; everything else (repeating
//! tasks, and one-shot tasks scheduled later by handlers or other tasks) runs when it is due, as
//! frontends call [`run_tasks`] every frame (`ScriptUi::run_tasks`, `script.runTasks`).
//!
//! Lifetime: a repeating task runs every `delay` ms until `app.cancelTask(id)`, until it throws
//! (the error is reported once and the task cancelled), or until its owner goes away: a script
//! whose windows (ScriptUI panel, palette) were open stops when the last one closes; a script
//! without windows (and the Script Console) keeps its tasks until the project is closed or
//! replaced. Bounds: repeating tasks run at most every [`MIN_REPEAT_MS`], a script has at most
//! [`MAX_TASKS`] pending tasks, every run has the script engine's usual limits, and a due task
//! runs once per pass (no catch-up bursts after a long frame).
//!
//! Each thread keeps the tasks of the hosts it runs: a script-host thread its own, the caller's
//! thread those of inline hosts (the Script Console, web runs).

use std::cell::RefCell;
use std::collections::BTreeMap;

use boa_engine::Context;
use effectcraft_engine::Session;
use serde_json::{Value as J, json};
use web_time::{Duration, Instant};

use crate::runtime::{ScriptError, eval_in};

/// The shortest interval of a repeating task (milliseconds).
pub const MIN_REPEAT_MS: u64 = 50;
/// The most tasks one script may have pending.
pub const MAX_TASKS: usize = 64;
/// The longest delay (milliseconds), as in browsers' `setTimeout`.
const MAX_DELAY_MS: f64 = 2_147_483_647.0;

struct Task {
    id: u32,
    code: String,
    interval: Duration,
    due: Instant,
    repeat: bool,
}

#[derive(Default)]
struct HostTasks {
    next_id: u32,
    list: Vec<Task>,
    /// The script's name (inline hosts without windows run under it).
    name: String,
    /// [`effectcraft_engine::scriptui::ScriptUi::project_epoch`] when the host last ran (inline
    /// hosts; the caller tracks script-host threads).
    epoch: u64,
}

thread_local! {
    static TASKS: RefCell<BTreeMap<u32, HostTasks>> = const { RefCell::new(BTreeMap::new()) };
}

/// Queue a task for `host`; its id.
pub(crate) fn schedule(host: u32, name: &str, epoch: u64, code: String, delay_ms: f64, repeat: bool, now: Instant) -> Result<u32, String> {
    crate::runtime::check_nesting(&code).map_err(|e| format!("app.scheduleTask: {}", e.trim_start_matches("Error: ")))?;
    let ms = if delay_ms.is_finite() { delay_ms.clamp(0.0, MAX_DELAY_MS) } else { 0.0 } as u64;
    let interval = Duration::from_millis(if repeat { ms.max(MIN_REPEAT_MS) } else { ms });
    let due = now.checked_add(interval).ok_or("app.scheduleTask: the delay is too long")?;
    TASKS.with(|t| {
        let mut t = t.borrow_mut();
        let h = t.entry(host).or_default();
        if h.list.len() >= MAX_TASKS {
            return Err(format!("app.scheduleTask: a script can have at most {MAX_TASKS} scheduled tasks"));
        }
        h.next_id = h.next_id.wrapping_add(1).max(1);
        let id = h.next_id;
        h.list.push(Task { id, code, interval, due, repeat });
        h.name = name.to_string();
        h.epoch = epoch;
        Ok(id)
    })
}

/// `app.cancelTask(id)`.
pub(crate) fn cancel(host: u32, id: u32) {
    TASKS.with(|t| {
        if let Some(h) = t.borrow_mut().get_mut(&host) {
            h.list.retain(|task| task.id != id);
        }
    });
}

/// Take the one-shot tasks of `host` (run right after a script file).
pub(crate) fn take_one_shots(host: u32) -> Vec<String> {
    TASKS.with(|t| {
        let mut t = t.borrow_mut();
        let Some(h) = t.get_mut(&host) else { return vec![] };
        let (once, keep) = std::mem::take(&mut h.list).into_iter().partition(|task| !task.repeat);
        h.list = keep;
        once.into_iter().map(|task: Task| task.code).collect()
    })
}

/// Take the tasks due at `now`: one-shot tasks leave the queue, repeating ones are due again an
/// interval from now. (id, code, repeat).
fn take_due(host: u32, now: Instant) -> Vec<(u32, String, bool)> {
    TASKS.with(|t| {
        let mut t = t.borrow_mut();
        let Some(h) = t.get_mut(&host) else { return vec![] };
        let mut due = vec![];
        h.list.retain_mut(|task| {
            if task.due > now {
                return true;
            }
            due.push((task.id, task.code.clone(), task.repeat));
            match now.checked_add(task.interval) {
                Some(next) if task.repeat => {
                    task.due = next;
                    true
                }
                _ => false,
            }
        });
        due
    })
}

/// Whether `host` has pending tasks.
pub(crate) fn has(host: u32) -> bool {
    TASKS.with(|t| t.borrow().get(&host).is_some_and(|h| !h.list.is_empty()))
}

/// When `host`'s next task is due.
pub(crate) fn next_due(host: u32) -> Option<Instant> {
    TASKS.with(|t| t.borrow().get(&host).and_then(|h| h.list.iter().map(|task| task.due).min()))
}

/// Forget `host`'s tasks.
pub(crate) fn drop_host(host: u32) {
    TASKS.with(|t| t.borrow_mut().remove(&host));
}

/// The hosts on this thread with tasks: (host, script name, epoch).
pub(crate) fn hosts() -> Vec<(u32, String, u64)> {
    TASKS.with(|t| t.borrow().iter().filter(|(_, h)| !h.list.is_empty()).map(|(id, h)| (*id, h.name.clone(), h.epoch)).collect())
}

pub(crate) fn set_epoch(host: u32, epoch: u64) {
    TASKS.with(|t| {
        if let Some(h) = t.borrow_mut().get_mut(&host) {
            h.epoch = epoch;
        }
    });
}

/// Run `host`'s due tasks in `ctx` (the session is active): how many ran, and the first error.
/// A task that throws is cancelled (a repeating one would only repeat the error).
pub(crate) fn run_due(ctx: &mut Context, host: u32, now: Instant) -> (usize, Option<ScriptError>) {
    let (mut ran, mut first) = (0, None);
    for (id, code, repeat) in take_due(host, now) {
        ran += 1;
        if let Err(mut e) = eval_in(ctx, &code, "scheduled task") {
            if repeat {
                cancel(host, id);
                e.message = format!("{} (repeating task {id} cancelled)", e.message);
            }
            let line = e.line.map(|l| format!(" (line {l})")).unwrap_or_default();
            let _ = crate::runtime::with_active(|a| {
                a.output.push(format!("Error in scheduled task {id}{line}: {}", e.message));
                Ok(())
            });
            first.get_or_insert(e);
        }
    }
    (ran, first)
}

/// What a pass over the scheduled tasks did.
#[derive(Default)]
pub(crate) struct Report {
    ran: usize,
    output: Vec<String>,
    errors: Vec<ScriptError>,
    next: Option<Instant>,
}

impl Report {
    pub(crate) fn add(&mut self, out: crate::Outcome) {
        self.ran += out.result.as_u64().unwrap_or(0) as usize;
        self.output.extend(out.output);
        self.errors.extend(out.error);
    }

    pub(crate) fn due(&mut self, at: Option<Instant>) {
        if let Some(at) = at {
            self.next = Some(self.next.map_or(at, |n| n.min(at)));
        }
    }
}

/// The [`effectcraft_engine::scriptui::ScriptTaskRunner`] this crate provides: run the tasks
/// that are due now.
pub fn run_tasks(session: &mut Session) -> J {
    run_tasks_at(session, Instant::now())
}

/// [`run_tasks`] at `now`.
pub(crate) fn run_tasks_at(session: &mut Session, now: Instant) -> J {
    let mut report = Report::default();
    // Not from inside a script (`script.runTasks` through `app.executeCommand`).
    let running = crate::runtime::ACTIVE.with(|a| a.try_borrow().map(|g| g.is_some()).unwrap_or(true));
    if !running {
        #[cfg(not(target_arch = "wasm32"))]
        crate::ui::tick_threaded(session, now, &mut report);
        crate::ui::tick_inline(session, now, &mut report);
        crate::ui::prune_task_hosts(session);
    }
    let errors: Vec<J> = report.errors.iter().map(|e| json!({"message": e.message, "line": e.line, "column": e.column, "file": e.file})).collect();
    let next = report.next.map(|n| n.saturating_duration_since(now).as_millis() as u64);
    json!({"ran": report.ran, "output": report.output, "errors": errors, "next": next})
}
