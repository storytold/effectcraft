//! Headless checks for M13.10 (web app depth), run natively with stand-ins for the browser's
//! workers:
//!
//! - viewer frames go to a [`RemoteFrames`] renderer (the browser's frame worker, here an
//!   in-process [`FrameServer`] fed through JSON with project diffs) instead of rendering on the
//!   UI thread, the viewer's frame first, and come back identical;
//! - a control-channel `wait: true` command whose job is offloaded replies only when the job
//!   ends, while the UI keeps running.

use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, Mutex};

use effectcraft_engine::Session;
use effectcraft_engine::offload::{Inbox, Offload, Post, WorkerRequest};
use effectcraft_engine::remote::{FrameMsg, FrameReply, FrameServer, Mirror};
use effectcraft_engine::render::RenderOpts;
use effectcraft_ui_egui::frames::{FrameImage, FrameKey, Frames, RemoteDone, RemoteFrame, RemoteFrames, RemoteJob, RenderSource};
use effectcraft_ui_egui::{ControlRequest, EffectcraftApp};
use egui_kittest::Harness;
use serde_json::json;

/// A frame worker in the same process: messages go through JSON like `postMessage`; rendered
/// frames are held until [`FakeWorker::deliver`] (a busy worker) or dropped by
/// [`FakeWorker::lose`].
struct FakeWorker {
    slots: usize,
    state: Mutex<(Mirror, FrameServer, Vec<(RemoteDone, RemoteFrame)>, u64)>,
    /// Patch / full-project syncs sent.
    syncs: Mutex<Vec<&'static str>>,
}

impl FakeWorker {
    fn new(s: &Session, slots: usize) -> FakeWorker {
        FakeWorker { slots, state: Mutex::new((Mirror::default(), FrameServer::new(s.footage.clone(), s.expr.clone()), vec![], 0)), syncs: Mutex::default() }
    }
    fn pending(&self) -> usize {
        self.state.lock().unwrap().2.len()
    }
    fn take(&self) -> Vec<(RemoteDone, RemoteFrame)> {
        std::mem::take(&mut self.state.lock().unwrap().2)
    }
    /// Post every rendered frame back.
    fn deliver(&self) {
        for (done, f) in self.take() {
            done(Ok(f));
        }
    }
    /// The worker died: its frames never arrive.
    fn lose(&self) {
        for (done, _) in self.take() {
            done(Err("worker lost".into()));
        }
    }
}

fn rt<T: serde::Serialize + for<'de> serde::Deserialize<'de>>(v: &T) -> T {
    serde_json::from_str(&serde_json::to_string(v).unwrap()).unwrap()
}

impl RemoteFrames for FakeWorker {
    fn slots(&self) -> usize {
        self.slots
    }
    fn start(&self, job: RemoteJob, done: RemoteDone) {
        let mut st = self.state.lock().unwrap();
        let (mirror, server, pending, next) = &mut *st;
        if let Some(m) = mirror.sync(job.revision, &job.project) {
            self.syncs.lock().unwrap().push(if matches!(m, FrameMsg::Patch { .. }) { "patch" } else { "project" });
            assert!(server.handle(rt(&m)).is_empty());
        }
        *next += 1;
        let out = server.handle(rt(&FrameMsg::Render {
            id: *next,
            revision: job.revision,
            comp: job.comp,
            time: job.t,
            opts: Box::new(job.opts),
            disk: None,
            layers: false,
            prefetch: vec![],
        }));
        match out.into_iter().next() {
            Some((FrameReply::Frame { width, height, ms, .. }, Some(rgba))) => pending.push((done, RemoteFrame { width, height, rgba, ms })),
            other => done(Err(format!("{:?}", other.map(|o| o.0)))),
        }
    }
}

fn source(s: &Session) -> RenderSource {
    RenderSource {
        project: s.project.clone(),
        footage: s.footage.clone(),
        expr: s.expr.clone(),
        layer_cache: s.layer_cache.clone(),
        gpu: None,
        gpu_display: false,
        disk: None,
    }
}

#[test]
fn viewer_frames_render_remotely_from_project_diffs() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let comp = s.active_comp_id().unwrap();
    let c = s.project.comp(comp).unwrap().clone();
    let opts = RenderOpts { scale: 0.25, guides: true, ..Default::default() };
    let key = |s: &Session, f: i64| FrameKey {
        revision: s.revision,
        content: effectcraft_ui_egui::frames::comp_content(&s.project, comp),
        comp: comp.0,
        frame: f,
        scale: 250,
        view: 0,
        opts: 0,
    };
    let worker = Arc::new(FakeWorker::new(&s, 2));
    let mut frames = Frames::default();
    frames.set_remote(Some(worker.clone()));
    assert!(frames.remote_active());
    // Three prefetch frames, then the viewer's frame.
    for f in 1..=3 {
        frames.request(&source(&s), key(&s, f), comp, c.frame_rate.tick_of(f), opts);
    }
    frames.request_urgent(&source(&s), key(&s, 10), comp, c.frame_rate.tick_of(10), opts);
    assert_eq!(frames.inflight(), 4);
    // Nothing renders until dispatched (no frame threads with a remote renderer); the viewer's
    // frame goes first, then prefetch in request order.
    assert_eq!(frames.dispatch_remote(), 2);
    assert_eq!(frames.remote_busy(), 2);
    assert_eq!(worker.pending(), 2);
    assert_eq!(frames.dispatch_remote(), 0, "both slots are busy");
    assert_eq!(*worker.syncs.lock().unwrap(), vec!["project"], "the first sync is the whole project");
    // Frames arrive: the cache holds exactly what the page would have rendered.
    worker.deliver();
    assert_eq!(frames.remote_busy(), 0);
    let img = match frames.get(&key(&s, 10)).expect("viewer frame cached") {
        FrameImage::Cpu(c) => c,
        FrameImage::Gpu(_) => unreachable!(),
    };
    let local = effectcraft_ui_egui::frames::to_color_image(&s.render(comp, c.frame_rate.tick_of(10), opts));
    assert_eq!(img.size, local.size);
    assert_eq!(img.pixels, local.pixels);
    assert!(frames.is_cached(&key(&s, 1)));
    assert!(!frames.is_cached(&key(&s, 2)));
    // The rest; an edit later travels as a patch.
    frames.dispatch_remote();
    worker.deliver();
    frames.dispatch_remote();
    worker.deliver();
    assert_eq!(frames.inflight(), 0);
    s.execute("layer.newSolid", json!({"color": "#ff8800"})).unwrap();
    frames.request_urgent(&source(&s), key(&s, 10), comp, c.frame_rate.tick_of(10), opts);
    frames.dispatch_remote();
    worker.deliver();
    assert_eq!(worker.syncs.lock().unwrap().last(), Some(&"patch"));
    let FrameImage::Cpu(img) = frames.get(&key(&s, 10)).unwrap() else { unreachable!() };
    let local = effectcraft_ui_egui::frames::to_color_image(&s.render(comp, c.frame_rate.tick_of(10), opts));
    assert_eq!(img.pixels, local.pixels);
    // A lost frame is released (requested again later), not cached.
    frames.request(&source(&s), key(&s, 20), comp, c.frame_rate.tick_of(20), opts);
    frames.dispatch_remote();
    worker.lose();
    assert_eq!(frames.inflight(), 0);
    assert!(!frames.is_cached(&key(&s, 20)));
    // Without slots the remote renderer is unusable (frames render here again).
    frames.set_remote(Some(Arc::new(FakeWorker::new(&s, 0))));
    assert!(!frames.remote_active());
}

/// Holds offloaded jobs until the test runs them.
#[derive(Default)]
struct Manual(Mutex<Vec<(WorkerRequest, Arc<Inbox>)>>);

impl Offload for Manual {
    fn start(&self, req: WorkerRequest, inbox: Arc<Inbox>) -> Result<(), String> {
        self.0.lock().unwrap().push((req, inbox));
        Ok(())
    }
    fn cancel(&self, _: u64) {}
}

fn exporter() -> Arc<effectcraft_host::FileExporter> {
    let sink: Arc<dyn Fn(&str, Vec<u8>) + Send + Sync> = Arc::new(|_: &str, _: Vec<u8>| {});
    Arc::new(effectcraft_host::FileExporter { sink: Some(sink) })
}

#[test]
fn waiting_commands_reply_when_their_offloaded_job_ends() {
    let off = Arc::new(Manual::default());
    let mut s = Session { offload: Some(off.clone()), exporter: Some(exporter()), ..Default::default() };
    s.execute("comp.new", json!({"name": "W", "width": 64, "height": 48, "frameRate": 10, "duration": 1})).unwrap();
    s.execute("layer.newSolid", json!({"color": "#406080"})).unwrap();
    s.execute("renderQueue.add", json!({"format": "gif", "output": "/w.gif"})).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|_| EffectcraftApp::new(s).with_control(rx));
    h.run_steps(2);
    // `renderQueue.render` waits by default: the job goes to the worker, the reply waits.
    let (req, reply) = ControlRequest::new("engine.execute", json!({"command": "renderQueue.render", "params": {}}));
    tx.send(req).unwrap();
    h.run_steps(4);
    assert_eq!(reply.try_recv().unwrap_err(), TryRecvError::Empty, "no reply while the job runs");
    assert!(h.state().session.is_rendering());
    let (job, inbox) = off.0.lock().unwrap().pop().expect("the render went to the worker");
    // The worker runs it.
    let mut w = Session { exporter: Some(exporter()), ..Default::default() };
    let post: Post = std::rc::Rc::new(move |r| inbox.push(r));
    effectcraft_engine::offload::run_request(&mut w, job, &post);
    h.run_steps(4);
    let v = reply.try_recv().expect("replied once the job ended");
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["result"]["rendering"], false, "{v}");
    assert_eq!(v["result"]["items"][0]["status"], "Done", "{v}");
    // `wait: false` replies at once.
    h.state_mut().session.execute("renderQueue.add", json!({"format": "gif", "output": "/w2.gif"})).unwrap();
    let (req, reply) = ControlRequest::new("engine.execute", json!({"command": "renderQueue.render", "params": {"wait": false}}));
    tx.send(req).unwrap();
    h.run_steps(2);
    let v = reply.try_recv().expect("immediate reply");
    assert_eq!(v["result"]["rendering"], true, "{v}");
}

/// Completion errors belong to the waiting command's jobs, including when several
/// different analyses complete in the same UI frame.
#[test]
fn waiting_commands_report_only_their_own_completion_errors() {
    use effectcraft_engine::offload::JobKind;
    use effectcraft_ui_egui::control::JobWaiter;

    for (kind, job_id) in [
        (JobKind::Render, "render"),
        (JobKind::Warp, "warp"),
        (JobKind::WarpPlan, "warpPlan"),
        (JobKind::Camera, "camera"),
        (JobKind::RotoFreeze, "roto"),
        (JobKind::RotoPropagate, "roto"),
        (JobKind::Track, "track"),
        (JobKind::MaskTrack, "maskTrack"),
        (JobKind::ContentFill, "contentFill"),
    ] {
        let other = if kind == JobKind::Track { JobKind::Warp } else { JobKind::Track };
        let mut app = EffectcraftApp::new(Session::default());
        app.session.offload_log = vec![(kind, Some("old error".into())), (other, Some("unrelated failure".into())), (kind, None)];
        let waiter = || JobWaiter { command: "analysis".into(), params: json!({}), result: json!({"running": true}), jobs: vec![job_id.into()], log_cursor: 1 };
        let result = waiter().finish(&mut app);
        assert_eq!(result, json!({"ok": true, "result": {"running": false}}), "{kind:?}");
        app.session.offload_log.push((kind, Some("nothing was applied".into())));
        let result = waiter().finish(&mut app);
        assert_eq!(result, json!({"ok": false, "error": "nothing was applied"}), "{kind:?}");
    }
}

#[test]
fn waiting_command_sees_its_error_after_the_completion_log_rolls_over() {
    use effectcraft_engine::offload::JobKind;
    use effectcraft_ui_egui::control::JobWaiter;
    let mut app = EffectcraftApp::new(Session::default());
    app.session.offload_log = vec![(JobKind::Warp, None); 31];
    app.session.offload_log.push((JobKind::Track, Some("nothing was applied".into())));
    app.session.offload_log_start = 1;
    let waiter = JobWaiter { command: "track.analyze".into(), params: json!({}), result: json!({}), jobs: vec!["track".into()], log_cursor: 32 };
    assert_eq!(waiter.finish(&mut app), json!({"ok": false, "error": "nothing was applied"}));
}
