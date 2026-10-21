//! Cached viewer diagnostics. Native scans run on one persistent worker, never while painting.
//! Each viewer keeps only its newest request; obsolete scans stop between properties. On the
//! single-threaded web build, one shared scan runs after interaction settles (no thread fallback).

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use effectcraft_engine::Session;
use effectcraft_engine::commands::expr_tools::{self, ExprError};
use effectcraft_engine::project::{ItemId, Project};
use effectcraft_engine::render::{EvalCtx, ExprHost, FootageSource};
use effectcraft_engine::time::Tick;
use web_time::Instant;

const QUIET: Duration = Duration::from_millis(150);
const MAX_VIEWERS: usize = 8;
type Errors = Arc<[ExprError]>;
type SyntaxCheck = fn(&str) -> Result<(), String>;

#[derive(Clone)]
struct Snapshot {
    project: Arc<Project>,
    revision: u64,
    comp: ItemId,
    time: Tick,
    expr: Option<Arc<dyn ExprHost>>,
    footage: Arc<dyn FootageSource>,
    check: Option<SyntaxCheck>,
}

impl Snapshot {
    fn new(s: &Session, comp: ItemId) -> Self {
        Self {
            project: s.project.clone(),
            revision: s.revision,
            comp,
            time: s.time_of(comp),
            expr: s.expr.clone(),
            footage: s.footage.clone(),
            check: s.expr_check,
        }
    }

    fn matches(&self, s: &Session, comp: ItemId) -> bool {
        self.revision == s.revision && self.comp == comp && self.time == s.time_of(comp) && Arc::ptr_eq(&self.project, &s.project)
    }
}

struct Request {
    snapshot: Snapshot,
    cancelled: Arc<AtomicBool>,
    #[cfg(not(target_arch = "wasm32"))]
    repaint: egui::Context,
}

struct Completed {
    #[cfg(not(target_arch = "wasm32"))]
    comp: ItemId,
    #[cfg(not(target_arch = "wasm32"))]
    cancelled: Arc<AtomicBool>,
    result: Result<Option<Vec<ExprError>>, String>,
}

impl Request {
    fn run(&self) -> Completed {
        let s = &self.snapshot;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let comp = s.project.comp(s.comp)?;
            let ctx = EvalCtx { expr: s.expr.as_deref(), footage: Some(s.footage.as_ref()), ..EvalCtx::new(&s.project, s.comp, comp, s.time) };
            expr_tools::errors_in(&ctx, s.check, || !self.cancelled.load(Ordering::Relaxed))
        }))
        .map_err(|_| "Expression diagnostics stopped unexpectedly; edit an expression to retry.".to_string());
        Completed {
            #[cfg(not(target_arch = "wasm32"))]
            comp: s.comp,
            #[cfg(not(target_arch = "wasm32"))]
            cancelled: self.cancelled.clone(),
            result,
        }
    }
}

struct Entry {
    snapshot: Snapshot,
    changed: Instant,
    cancelled: Arc<AtomicBool>,
    submitted: bool,
    errors: Errors,
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub(crate) struct Diagnostics {
    entries: VecDeque<Entry>,
    #[cfg(not(target_arch = "wasm32"))]
    worker: Option<Worker>,
    failure: Option<String>,
}

impl Diagnostics {
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn take_failure(&mut self) -> Option<String> {
        self.failure.take()
    }

    /// Paint from completed results only. Changes discard stale results immediately, including
    /// errors whose layer/property no longer exists. Pointer interaction and playback defer scans.
    pub fn errors(&mut self, s: &Session, ctx: &egui::Context, comp: ItemId, interacting: bool) -> Errors {
        self.update(s, ctx, comp, interacting, Instant::now())
    }

    fn update(&mut self, s: &Session, ctx: &egui::Context, comp: ItemId, interacting: bool, now: Instant) -> Errors {
        if !s.prefs.scripting.error_banner || s.expr.is_none() {
            self.clear();
            return Arc::from([]);
        }
        let mut entry = self.entries.iter().position(|e| e.snapshot.comp == comp).and_then(|i| self.entries.remove(i));
        if entry.as_ref().is_none_or(|e| !e.snapshot.matches(s, comp)) {
            entry = Some(Entry {
                snapshot: Snapshot::new(s, comp),
                changed: now,
                cancelled: Arc::new(AtomicBool::new(false)),
                submitted: false,
                errors: Arc::from([]),
            });
        }
        let Some(mut entry) = entry else { return Arc::from([]) };
        if interacting {
            entry.changed = now;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(worker) = &self.worker {
            let done: Vec<_> = worker.shared.completed.lock().unwrap_or_else(PoisonError::into_inner).drain(..).collect();
            for done in done {
                let target = if done.comp == comp { Some(&mut entry) } else { self.entries.iter_mut().find(|e| e.snapshot.comp == done.comp) };
                if let Some(target) = target
                    && Arc::ptr_eq(&target.cancelled, &done.cancelled)
                    && !done.cancelled.load(Ordering::Relaxed)
                {
                    match done.result {
                        Ok(Some(errors)) => target.errors = errors.into(),
                        Ok(None) => {}
                        Err(e) => self.failure = Some(e),
                    }
                }
            }
        }
        if !entry.submitted && !interacting {
            let quiet = now.saturating_duration_since(entry.changed);
            if quiet < QUIET {
                ctx.request_repaint_after(QUIET - quiet);
            } else {
                entry.submitted = true;
                let request = Request {
                    snapshot: entry.snapshot.clone(),
                    cancelled: entry.cancelled.clone(),
                    #[cfg(not(target_arch = "wasm32"))]
                    repaint: ctx.clone(),
                };
                #[cfg(not(target_arch = "wasm32"))]
                {
                    if self.worker.is_none() {
                        match Worker::new() {
                            Ok(worker) => self.worker = Some(worker),
                            Err(e) => self.failure = Some(format!("Could not start expression diagnostics: {e}")),
                        }
                    }
                    if let Some(worker) = &self.worker {
                        worker.submit(request);
                    }
                }
                // wasm32 cannot spawn Rust threads. Preserve diagnostics there, but eliminate
                // the duplicate scan and defer it until scrubbing/dragging/playback stops.
                #[cfg(target_arch = "wasm32")]
                match request.run().result {
                    Ok(Some(errors)) => entry.errors = errors.into(),
                    Ok(None) => {}
                    Err(e) => self.failure = Some(e),
                }
            }
        }
        let result = entry.errors.clone();
        self.entries.push_back(entry);
        if self.entries.len() > MAX_VIEWERS {
            self.entries.pop_front();
        }
        result
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct Shared {
    pending: Mutex<VecDeque<Request>>,
    completed: Mutex<VecDeque<Completed>>,
    wake: std::sync::Condvar,
    stopped: AtomicBool,
}

#[cfg(not(target_arch = "wasm32"))]
struct Worker {
    shared: Arc<Shared>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Worker {
    fn new() -> std::io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let work = shared.clone();
        std::thread::Builder::new().name("ec-expression-diagnostics".into()).spawn(move || {
            loop {
                let request = {
                    let mut pending = work.pending.lock().unwrap_or_else(PoisonError::into_inner);
                    while pending.is_empty() && !work.stopped.load(Ordering::Relaxed) {
                        pending = work.wake.wait(pending).unwrap_or_else(PoisonError::into_inner);
                    }
                    if work.stopped.load(Ordering::Relaxed) {
                        return;
                    }
                    pending.pop_front()
                };
                let Some(request) = request else { continue };
                if request.cancelled.load(Ordering::Relaxed) {
                    continue;
                }
                let result = request.run();
                if !result.cancelled.load(Ordering::Relaxed) {
                    let mut completed = work.completed.lock().unwrap_or_else(PoisonError::into_inner);
                    completed.retain(|r| r.comp != result.comp && !r.cancelled.load(Ordering::Relaxed));
                    if completed.len() >= MAX_VIEWERS {
                        completed.pop_front();
                    }
                    completed.push_back(result);
                }
                request.repaint.request_repaint();
            }
        })?;
        Ok(Self { shared })
    }

    fn submit(&self, request: Request) {
        let mut pending = self.shared.pending.lock().unwrap_or_else(PoisonError::into_inner);
        pending.retain(|r| r.snapshot.comp != request.snapshot.comp && !r.cancelled.load(Ordering::Relaxed));
        if pending.len() >= MAX_VIEWERS {
            pending.pop_front();
        }
        pending.push_back(request);
        self.shared.wake.notify_one();
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Worker {
    fn drop(&mut self) {
        // Hold the predicate lock when waking, so shutdown cannot race with entering wait.
        let mut pending = self.shared.pending.lock().unwrap_or_else(PoisonError::into_inner);
        self.shared.stopped.store(true, Ordering::Relaxed);
        pending.clear();
        self.shared.wake.notify_one();
        // Do not join an in-flight expression on the UI thread. Entries cancel it on drop.
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use super::*;
    use effectcraft_engine::keyframe::Value;
    use effectcraft_engine::project::{Layer, Property};
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;

    struct Host {
        main: std::thread::ThreadId,
        on_ui: AtomicUsize,
        calls: AtomicUsize,
        entered: mpsc::Sender<()>,
        gate: (Mutex<bool>, std::sync::Condvar),
        fail: bool,
    }
    impl ExprHost for Host {
        fn eval(&self, ctx: &EvalCtx, _: &Layer, _: &Property, value: &Value) -> Result<Value, String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if std::thread::current().id() == self.main {
                self.on_ui.fetch_add(1, Ordering::Relaxed);
                return Err("diagnostics evaluated on UI thread".into());
            }
            let _ = self.entered.send(());
            let (lock, wake) = &self.gate;
            let guard = lock.lock().unwrap();
            let (_guard, timed) = wake.wait_timeout_while(guard, Duration::from_secs(5), |open| !*open).unwrap();
            assert!(!timed.timed_out(), "test did not release the worker");
            if self.fail { Err(format!("failure at {}", ctx.time.0)) } else { Ok(value.clone()) }
        }
    }
    impl Host {
        fn release(&self) {
            *self.gate.0.lock().unwrap() = true;
            self.gate.1.notify_all();
        }
    }
    fn fixture(fail: bool, blocked: bool) -> (Session, Arc<Host>, mpsc::Receiver<()>) {
        let (tx, rx) = mpsc::channel();
        let host = Arc::new(Host {
            main: std::thread::current().id(),
            on_ui: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            entered: tx,
            gate: (Mutex::new(!blocked), std::sync::Condvar::new()),
            fail,
        });
        let mut s = Session { expr: Some(host.clone()), ..Session::default() };
        s.execute("comp.new", json!({"width":320,"height":180,"duration":5})).unwrap();
        let hidden = s.execute("layer.newSolid", json!({"name":"Hidden expression"})).unwrap()["layer"].as_u64().unwrap();
        s.execute("prop.setExpression", json!({"layer":hidden,"path":"transform/opacity","expression":"value"})).unwrap();
        let visible = s.execute("layer.newSolid", json!({"name":"Solo plate"})).unwrap()["layer"].as_u64().unwrap();
        s.execute("layer.setSwitch", json!({"layers":[visible],"switch":"solo","value":true})).unwrap();
        s.execute("layer.select", json!({"layers":[visible]})).unwrap();
        (s, host, rx)
    }
    fn wait_done(d: &Diagnostics) {
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            if d.worker.as_ref().is_some_and(|w| !w.shared.completed.lock().unwrap().is_empty()) {
                return;
            }
            assert!(Instant::now() < until, "diagnostics did not finish");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn start(d: &mut Diagnostics, s: &Session, ctx: &egui::Context, now: Instant) {
        let cid = s.active_comp_id().unwrap();
        d.update(s, ctx, cid, false, now);
        d.update(s, ctx, cid, false, now + QUIET);
    }

    #[test]
    fn successful_hidden_expressions_run_once_off_the_ui_thread() {
        let (s, host, _) = fixture(false, false);
        let (mut d, ctx, now) = (Diagnostics::default(), egui::Context::default(), Instant::now());
        let cid = s.active_comp_id().unwrap();
        start(&mut d, &s, &ctx, now);
        wait_done(&d);
        for _ in 0..5 {
            assert!(d.update(&s, &ctx, cid, false, now + QUIET).is_empty());
        }
        assert_eq!(host.calls.load(Ordering::Relaxed), 1);
        assert_eq!(host.on_ui.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn newest_request_replaces_queued_times_and_discards_stale_errors() {
        let (mut s, host, entered) = fixture(true, true);
        let (mut d, ctx, now) = (Diagnostics::default(), egui::Context::default(), Instant::now());
        let cid = s.active_comp_id().unwrap();
        start(&mut d, &s, &ctx, now);
        entered.recv_timeout(Duration::from_secs(3)).unwrap();
        for i in 1..20 {
            s.set_time(Tick::from_seconds_f64(i as f64 / 30.));
            start(&mut d, &s, &ctx, now + Duration::from_secs(i));
            assert!(d.entries.back().unwrap().errors.is_empty());
        }
        assert_eq!(d.worker.as_ref().unwrap().shared.pending.lock().unwrap().len(), 1);
        host.release();
        wait_done(&d);
        let errors = d.update(&s, &ctx, cid, false, now + Duration::from_secs(30));
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].message, format!("failure at {}", s.time().0));
        assert_eq!(host.calls.load(Ordering::Relaxed), 2);
        assert_eq!(host.on_ui.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn warning_preference_and_interaction_prevent_scanning_and_clear_stale_results() {
        let (mut s, host, _) = fixture(true, false);
        let (mut d, ctx, now) = (Diagnostics::default(), egui::Context::default(), Instant::now());
        let cid = s.active_comp_id().unwrap();
        for i in 0..10 {
            d.update(&s, &ctx, cid, true, now + Duration::from_secs(i));
        }
        assert!(d.worker.is_none());
        s.prefs.scripting.error_banner = false;
        start(&mut d, &s, &ctx, now + Duration::from_secs(10));
        assert!(d.worker.is_none());
        assert_eq!(host.calls.load(Ordering::Relaxed), 0);
        s.prefs.scripting.error_banner = true;
        start(&mut d, &s, &ctx, now + Duration::from_secs(11));
        wait_done(&d);
        assert_eq!(d.update(&s, &ctx, cid, false, now + Duration::from_secs(12)).len(), 1);
        s.prefs.scripting.error_banner = false;
        assert!(d.update(&s, &ctx, cid, false, now + Duration::from_secs(13)).is_empty());
        assert!(d.entries.is_empty());
    }

    #[test]
    fn replacement_project_with_reused_ids_does_not_reuse_errors() {
        let (s, _, _) = fixture(true, false);
        let (mut d, ctx, now) = (Diagnostics::default(), egui::Context::default(), Instant::now());
        let cid = s.active_comp_id().unwrap();
        start(&mut d, &s, &ctx, now);
        wait_done(&d);
        assert_eq!(d.update(&s, &ctx, cid, false, now + QUIET).len(), 1);
        let (replacement, _, _) = fixture(false, false);
        assert_eq!(s.revision, replacement.revision);
        assert!(d.update(&replacement, &ctx, cid, false, now + QUIET).is_empty());
    }

    #[test]
    fn scans_keep_disabled_syntax_errors_and_non_solo_runtime_errors() {
        let (mut s, _, _) = fixture(true, false);
        let cid = s.active_comp_id().unwrap();
        let selected = s.state.selected_layers[0];
        s.expr_check = Some(|_| Err("invalid syntax".into()));
        s.execute("prop.setExpression", json!({"layer":selected.0,"path":"transform/rotation","expression":"bad("})).unwrap();
        let (mut d, ctx, now) = (Diagnostics::default(), egui::Context::default(), Instant::now());
        start(&mut d, &s, &ctx, now);
        wait_done(&d);
        let errors = d.update(&s, &ctx, cid, false, now + QUIET);
        assert_eq!(errors.len(), 2);
        assert!(errors.iter().any(|e| e.disabled && e.message == "invalid syntax"));
        assert!(errors.iter().any(|e| !e.disabled && e.layer_name == "Hidden expression"));
    }

    #[test]
    fn viewer_edits_do_not_wait_for_hidden_diagnostics_with_or_without_warnings() {
        use crate::{EffectcraftApp, bench::Driver};
        let (s, host, entered) = fixture(false, true);
        let mut d = Driver::new(EffectcraftApp::new(s), egui::vec2(1680., 1020.));
        for _ in 0..3 {
            d.step();
        }
        let now = Instant::now() + Duration::from_secs(1);
        start(&mut d.app.expression_diagnostics, &d.app.session, &d.ctx, now);
        entered.recv_timeout(Duration::from_secs(3)).unwrap();
        let layer = d.app.session.state.selected_layers[0];
        d.app.session.state.snapping = false;
        let handle = format!("viewer.handle.{}.0", layer.0);
        let before = d.app.auto.find(&handle).unwrap().rect;
        let comp_rect = d.app.auto.find("viewer.comp").unwrap().rect;
        let from = egui::pos2(comp_rect[0] + comp_rect[2] * 0.4, comp_rect[1] + comp_rect[3] * 0.4);
        let to = from + egui::vec2(40., 20.);
        d.step_with(vec![
            egui::Event::PointerMoved(from),
            egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
        ]);
        d.step_with(vec![egui::Event::PointerMoved(to)]);
        d.step_with(vec![egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }]);
        d.step();
        let after = d.app.auto.find(&handle).unwrap().rect;
        assert!(after[0] > before[0] + 30., "selection bounds did not follow the drag: {before:?} -> {after:?}");
        for warnings in [true, false] {
            d.app.session.prefs.scripting.error_banner = warnings;
            for i in 0..4 {
                d.app.session.set_time(Tick::from_seconds_f64(i as f64 / 30.));
                d.app.session.execute("layer.setTransform", json!({"layer":layer.0,"prop":"position","value":[170.+i as f64,90.,0.]})).unwrap();
                d.app.session.execute("layer.setSwitch", json!({"layers":[layer.0],"switch":"video","value":i%2==0})).unwrap();
                d.step();
            }
        }
        // The diagnostic expression is still blocked on the worker, but every UI frame above
        // finished. This assertion also catches reintroducing a second, synchronous banner.
        assert!(!*host.gate.0.lock().unwrap());
        assert_eq!(host.on_ui.load(Ordering::Relaxed), 0);
        host.release();
    }
}
