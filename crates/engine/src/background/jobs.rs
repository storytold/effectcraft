//! Background jobs: one list (Window ▸ Progress, `jobs.list`, `jobs.cancel`) over the render
//! queue, the analyses (tracker, mask tracker, Warp Stabilizer, 3D Camera Tracker, Roto Brush) and
//! the generic background tasks started here (Content-Aware Fill, Scene Edit Detection, footage
//! checks and imports).
//!
//! A generic task runs a closure on a background thread (inline on wasm32 or when the caller
//! waits). The closure reports progress through [`TaskCtl`] and returns an [`Apply`]: a function
//! the session runs on the UI thread when [`Session::poll_jobs`] sees the task finish (that is
//! where the project is edited, as one undo step).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::{Value, json};

use crate::Session;
use crate::tracking::lock;

/// Applied to the session when a task finishes successfully.
pub type Apply = Box<dyn FnOnce(&mut Session) -> crate::Result<Value> + Send>;

/// Progress reporting and cancellation for a running task.
#[derive(Default)]
pub struct TaskCtl {
    cancel: AtomicBool,
    state: Mutex<TaskState>,
}

#[derive(Clone, Debug, Default)]
struct TaskState {
    done: u64,
    total: u64,
    message: String,
    finished: bool,
}

impl TaskCtl {
    /// Report progress; returns `false` once the task was cancelled.
    pub fn progress(&self, done: u64, total: u64) -> bool {
        {
            let mut s = lock(&self.state);
            s.done = done;
            s.total = total;
        }
        crate::offload::report(done, total);
        !self.cancelled()
    }
    pub fn message(&self, m: impl Into<String>) {
        lock(&self.state).message = m.into();
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

type Work = Box<dyn FnOnce(&TaskCtl) -> Result<Apply, String> + Send>;

/// A generic background task.
pub struct Task {
    pub id: u64,
    pub kind: &'static str,
    pub label: String,
    pub ctl: Arc<TaskCtl>,
    started: web_time::Instant,
    result: Arc<Mutex<Option<Result<Apply, String>>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// A finished job (kept for the Progress panel and agents).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRecord {
    pub id: String,
    pub kind: String,
    pub label: String,
    /// `done`, `cancelled` or `failed`.
    pub status: String,
    pub message: String,
    pub result: Value,
    pub seconds: f64,
}

/// One row of the Progress panel.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobInfo {
    /// `render`, `track`, `maskTrack`, `warp`, `camera`, `roto`, or `task:<n>`.
    pub id: String,
    pub kind: String,
    pub label: String,
    pub done: u64,
    pub total: u64,
    /// 0…1 (`None` when the total is not known yet).
    pub fraction: Option<f64>,
    pub running: bool,
    pub message: String,
    pub cancellable: bool,
}

fn info(id: &str, kind: &str, label: String, done: u64, total: u64, running: bool, message: String) -> JobInfo {
    JobInfo {
        id: id.into(),
        kind: kind.into(),
        label,
        done,
        total,
        fraction: (total > 0).then(|| (done as f64 / total as f64).clamp(0.0, 1.0)),
        running,
        message,
        cancellable: running,
    }
}

impl Session {
    /// Start a background task. `wait` (and wasm32) runs it to completion and applies it before
    /// returning; the result is then the apply's value.
    pub fn spawn_task(
        &mut self,
        kind: &'static str,
        label: impl Into<String>,
        wait: bool,
        work: impl FnOnce(&TaskCtl) -> Result<Apply, String> + Send + 'static,
    ) -> crate::Result<Value> {
        let id = self.next_task_id;
        self.next_task_id += 1;
        let ctl = Arc::new(TaskCtl::default());
        let result: Arc<Mutex<Option<Result<Apply, String>>>> = Arc::new(Mutex::new(None));
        let label = label.into();
        let wait = wait || cfg!(target_arch = "wasm32");
        let work: Work = Box::new(work);
        let run = {
            let (ctl, result) = (ctl.clone(), result.clone());
            move || {
                let r = work(&ctl);
                *lock(&result) = Some(r);
                lock(&ctl.state).finished = true;
            }
        };
        let thread = if wait {
            run();
            None
        } else {
            Some(std::thread::Builder::new().name(format!("task-{kind}")).spawn(run).map_err(|e| crate::EngineError::Other(e.to_string()))?)
        };
        self.tasks.push(Task { id, kind, label, ctl, started: web_time::Instant::now(), result, thread });
        if wait {
            let rec = self.finish_task(self.tasks.len() - 1);
            return match rec.status.as_str() {
                "done" => Ok(rec.result),
                _ => Err(crate::EngineError::Other(if rec.message.is_empty() { rec.status } else { rec.message })),
            };
        }
        Ok(json!({"job": format!("task:{id}"), "started": true}))
    }

    fn finish_task(&mut self, i: usize) -> JobRecord {
        let mut t = self.tasks.remove(i);
        if let Some(th) = t.thread.take() {
            let _ = th.join();
        }
        let r = lock(&t.result).take();
        let cancelled = t.ctl.cancelled();
        let (status, message, result) = match r {
            Some(Ok(apply)) if !cancelled => match apply(self) {
                // A task may say how it went (`toast`: "Imported 3 items").
                Ok(v) => ("done", v.get("toast").and_then(Value::as_str).unwrap_or_default().to_string(), v),
                Err(e) => ("failed", e.to_string(), Value::Null),
            },
            Some(Ok(_)) => ("cancelled", String::new(), Value::Null),
            Some(Err(e)) if cancelled || e == crate::render_queue::CANCELLED => ("cancelled", String::new(), Value::Null),
            Some(Err(e)) => ("failed", e, Value::Null),
            None => ("failed", "the task stopped".into(), Value::Null),
        };
        let rec = JobRecord {
            id: format!("task:{}", t.id),
            kind: t.kind.into(),
            label: t.label.clone(),
            status: status.into(),
            message: message.clone(),
            result,
            seconds: t.started.elapsed().as_secs_f64(),
        };
        let toast = match status {
            "done" if !message.is_empty() => message.clone(),
            "done" => format!("{} finished", t.label),
            "cancelled" => format!("{} cancelled", t.label),
            _ => format!("{} failed: {message}", t.label),
        };
        self.events.push(crate::Event::Toast { message: toast, error: status == "failed" });
        self.job_log.push(rec.clone());
        if self.job_log.len() > 50 {
            self.job_log.remove(0);
        }
        rec
    }

    /// Apply finished tasks. Frontends call this every frame while [`Session::tasks`] is not empty.
    pub fn poll_jobs(&mut self) -> bool {
        let mut any = false;
        while let Some(i) = self.tasks.iter().position(|t| lock(&t.ctl.state).finished) {
            self.finish_task(i);
            any = true;
        }
        any
    }

    /// Wait for every running task (CLI, tests).
    pub fn wait_jobs(&mut self) {
        while !self.tasks.is_empty() {
            if let Some(th) = self.tasks.iter_mut().find_map(|t| t.thread.take()) {
                let _ = th.join();
            }
            self.poll_jobs();
            if self.tasks.iter().all(|t| t.thread.is_none() && !lock(&t.ctl.state).finished) {
                break;
            }
        }
    }

    /// Every running background job.
    pub fn jobs(&self) -> Vec<JobInfo> {
        let mut out = vec![];
        if let Some(st) = self.render_progress()
            && !st.finished
        {
            let name = st
                .current
                .and_then(|id| self.project.render_queue.iter().find(|i| i.id == id))
                .and_then(|i| self.project.comp(i.comp).map(|_| i.comp))
                .and_then(|c| self.project.item(c))
                .map(|i| i.name.clone());
            out.push(info(
                "render",
                "render",
                format!("Render Queue{}", name.map(|n| format!(": {n}")).unwrap_or_default()),
                st.done,
                st.total,
                true,
                format!("item {} of {}", (st.items_done + 1).min(st.items_total.max(1)), st.items_total),
            ));
        }
        if let Some(p) = self.track_progress()
            && !p.finished
        {
            out.push(info("track", "track", "Track Motion".into(), p.done, p.total, true, String::new()));
        }
        if let Some(p) = self.mask_track_progress()
            && !p.finished
        {
            out.push(info("maskTrack", "maskTrack", "Track Mask".into(), p.done, p.total, true, String::new()));
        }
        if let Some(p) = self.warp_progress()
            && !p.finished
        {
            out.push(info("warp", "warp", "Warp Stabilizer analysis".into(), p.done, p.total, true, format!("step {}", p.step)));
        }
        if let Some(p) = self.camera_progress()
            && !p.finished
        {
            out.push(info("camera", "camera", "3D Camera Tracker analysis".into(), p.done, p.total, true, format!("step {}", p.step)));
        }
        if let Some(p) = self.roto_progress()
            && !p.finished
        {
            out.push(info("roto", "roto", "Roto Brush propagation".into(), p.done, p.total, true, String::new()));
        }
        if let Some(j) = self.offloaded(crate::offload::JobKind::WarpPlan) {
            out.push(info("warpPlan", "warp", "Warp Stabilizer: Stabilizing".into(), j.progress.done, j.progress.total, true, String::new()));
        }
        if let Some(j) = self.offloaded(crate::offload::JobKind::ContentFill) {
            out.push(info("contentFill", "contentFill", "Content-Aware Fill".into(), j.progress.done, j.progress.total, true, String::new()));
        }
        for t in &self.tasks {
            let s = lock(&t.ctl.state).clone();
            out.push(info(&format!("task:{}", t.id), t.kind, t.label.clone(), s.done, s.total, !s.finished, s.message));
        }
        out
    }

    /// Cancel a job by id (`render`, `track`, …, `task:<n>`; `all` cancels everything).
    pub fn cancel_job(&mut self, id: &str) -> bool {
        match id {
            "render" => self.stop_render(),
            "track" => self.stop_track(),
            "maskTrack" => self.stop_mask_track(),
            "warp" => self.stop_warp(),
            "camera" => self.stop_camera(),
            "roto" => self.stop_roto(),
            "warpPlan" => self.cancel_offloaded(crate::offload::JobKind::WarpPlan),
            "contentFill" => self.cancel_offloaded(crate::offload::JobKind::ContentFill),
            "all" => {
                let ids: Vec<String> = self.jobs().into_iter().map(|j| j.id).collect();
                ids.iter().fold(false, |a, j| self.cancel_job(j) | a)
            }
            _ => {
                let Some(n) = id.strip_prefix("task:").and_then(|n| n.parse::<u64>().ok()) else { return false };
                match self.tasks.iter().find(|t| t.id == n) {
                    Some(t) => {
                        t.ctl.cancel.store(true, Ordering::Relaxed);
                        true
                    }
                    None => false,
                }
            }
        }
    }

    /// Fold the undo steps recorded since the project was `snap` into one step called `name`
    /// (commands built from several commands).
    pub fn collapse_undo(&mut self, name: &str, snap: &Arc<effectcraft_project::Project>) {
        let h = &mut self.history;
        let Some(start) = h.undo.iter().rposition(|(_, p)| Arc::ptr_eq(p, snap)) else { return };
        let first = h.undo[start].1.clone();
        h.undo.truncate(start);
        h.undo.push((name.to_string(), first));
        h.merge_key = None;
    }

    /// Cancel and drop every task (new/open project).
    pub(crate) fn drop_tasks(&mut self) {
        for t in &self.tasks {
            t.ctl.cancel.store(true, Ordering::Relaxed);
        }
        for mut t in std::mem::take(&mut self.tasks) {
            if let Some(th) = t.thread.take() {
                let _ = th.join();
            }
        }
    }
}
