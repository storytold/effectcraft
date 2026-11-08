//! #3289: the control bridge must keep servicing UI frames during scripted and direct renders.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use effectcraft_engine::project::render_queue::OutputFormat;
use effectcraft_engine::{ExportJob, ExportResult, Exporter};
use effectcraft_ui_egui::{ControlRequest, EffectcraftApp};
use egui_kittest::Harness;
use serde_json::{Value, json};

struct Gate(AtomicBool);
impl Exporter for Gate {
    fn formats(&self) -> Vec<OutputFormat> {
        vec![OutputFormat::H264]
    }
    fn export(&self, job: &ExportJob, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<ExportResult, String> {
        let end = Instant::now() + Duration::from_secs(3);
        while !self.0.load(Ordering::SeqCst) {
            if !progress(0, 10) {
                return Err(effectcraft_engine::render_queue::CANCELLED.into());
            }
            if Instant::now() > end {
                return Err("test exporter waited for a blocked UI".into());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(ExportResult { path: job.path.into(), frames: 10, ..Default::default() })
    }
}

fn harness() -> (Harness<'static, EffectcraftApp>, Arc<Gate>, std::sync::mpsc::Sender<ControlRequest>) {
    let gate = Arc::new(Gate(AtomicBool::new(false)));
    let mut s = effectcraft_host::session();
    s.exporter = Some(gate.clone());
    s.prefs.auto_save.enabled = false;
    s.execute("comp.new", json!({"width": 16, "height": 16, "duration": 1})).unwrap();
    let path = std::env::temp_dir().join(format!("ec-ui-script-render-{}.mp4", std::process::id()));
    s.execute("renderQueue.add", json!({"output": path.to_string_lossy()})).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let h = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(|_| EffectcraftApp::new(s).with_control(rx));
    (h, gate, tx)
}

fn send(tx: &std::sync::mpsc::Sender<ControlRequest>, method: &str, params: Value) -> Receiver<Value> {
    let (request, reply) = ControlRequest::new(method, params);
    tx.send(request).unwrap();
    reply
}

fn finish(h: &mut Harness<'_, EffectcraftApp>, reply: &Receiver<Value>) -> Value {
    let end = Instant::now() + Duration::from_secs(4);
    loop {
        h.run_steps(1);
        if let Ok(result) = reply.try_recv() {
            return result;
        }
        assert!(Instant::now() < end, "control reply did not complete");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn script_render_keeps_frames_and_inspection_alive_until_the_continuation_finishes() {
    let (mut h, gate, tx) = harness();
    let reply = send(
        &tx,
        "engine.execute",
        json!({"command": "script.run", "params": {"code": "$.writeln('before'); app.project.renderQueue.render(); app.project.activeItem.layers.addNull().name = 'After'; $.writeln('after'); 42;"}}),
    );
    h.run_steps(3);
    assert!(matches!(reply.try_recv(), Err(TryRecvError::Empty)), "script reply must wait for its continuation");
    assert!(h.state().session.active_comp().unwrap().layers.is_empty());
    let before = h.state().ui_passes();
    let inspect = send(&tx, "ui.inspect", json!({}));
    let edit = send(&tx, "engine.execute", json!({"command": "layer.newNull"}));
    let batch = send(&tx, "engine.execute", json!({"command": "engine.batch", "params": {"steps": [{"command": "layer.newNull"}]}}));
    let jobs = send(&tx, "engine.execute", json!({"command": "jobs.list"}));
    let quit = send(&tx, "app.quit", json!({"force": true}));
    h.run_steps(3);
    assert!(h.state().ui_passes() > before, "frames still draw while the render is gated");
    assert_eq!(inspect.try_recv().unwrap()["ok"], true);
    assert_eq!(edit.try_recv().unwrap()["ok"], false, "unrelated edits cannot enter the script's undo group");
    assert_eq!(batch.try_recv().unwrap()["ok"], false, "batches are not pure queries");
    assert_eq!(jobs.try_recv().unwrap()["ok"], false, "job polling can apply unrelated edits");
    assert_eq!(quit.try_recv().unwrap()["ok"], false, "force quit cannot abandon the script's live session");
    assert!(h.state().auto.find("scriptRender.stop").is_some());
    gate.0.store(true, Ordering::SeqCst);
    let done = finish(&mut h, &reply);
    assert_eq!(done["ok"], true, "{done}");
    assert_eq!(done["result"]["ok"], true, "{done}");
    assert_eq!(done["result"]["result"], 42);
    assert_eq!(done["result"]["output"], "before\nafter");
    assert_eq!(h.state().session.active_comp().unwrap().layers[0].name, "After");
    assert!(h.state().session.script_ui.render_host.is_none());
}

#[test]
fn stop_render_remains_available_while_a_script_waits() {
    let (mut h, _gate, tx) = harness();
    let reply = send(&tx, "engine.execute", json!({"command": "script.run", "params": {"code": "app.project.renderQueue.render(); 23;"}}));
    h.run_steps(2);
    assert!(matches!(reply.try_recv(), Err(TryRecvError::Empty)));
    let stop = send(&tx, "engine.execute", json!({"command": "renderQueue.stop"}));
    h.run_steps(1);
    assert_eq!(stop.try_recv().unwrap()["ok"], true);
    assert_eq!(finish(&mut h, &reply)["result"]["result"], 23);
    assert!(!h.state().session.is_rendering());
}

#[test]
fn closing_when_a_control_script_starts_cannot_abandon_its_new_unsaved_changes() {
    let (mut h, gate, tx) = harness();
    h.state_mut().session.mark_saved();
    let reply = send(
        &tx,
        "engine.execute",
        json!({"command": "script.run", "params": {"code": "app.project.activeItem.layers.addNull(); app.project.renderQueue.render(); 31;"}}),
    );
    h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
    h.step();
    let canceled = h.output().viewport_output.get(&egui::ViewportId::ROOT).is_some_and(|v| v.commands.contains(&egui::ViewportCommand::CancelClose));
    let dirty = h.state().session.is_dirty();
    let pending = matches!(reply.try_recv(), Err(TryRecvError::Empty));
    gate.0.store(true, Ordering::SeqCst);
    assert_eq!(finish(&mut h, &reply)["result"]["result"], 31);
    assert!(canceled, "a close accepted before the control request must be canceled once the script suspends");
    assert!(dirty && pending);
}

#[test]
fn closing_on_the_completion_frame_cannot_skip_the_unsaved_changes_guard() {
    let (mut h, gate, tx) = harness();
    let reply = send(&tx, "engine.execute", json!({"command": "script.run", "params": {"code": "app.project.renderQueue.render(); 23;"}}));
    h.run_steps(2);
    assert!(h.state().session.script_ui.render_host.is_some());
    gate.0.store(true, Ordering::SeqCst);
    let end = Instant::now() + Duration::from_secs(4);
    while h.state().session.is_rendering() {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
    h.step();
    assert!(h.output().viewport_output.get(&egui::ViewportId::ROOT).is_some_and(|v| v.commands.contains(&egui::ViewportCommand::CancelClose)));
    assert_eq!(reply.try_recv().unwrap()["result"]["result"], 23);
    assert!(h.state().session.is_dirty());
}

#[test]
fn direct_native_render_waits_for_its_worker_without_blocking_frames() {
    let (mut h, gate, tx) = harness();
    let reply = send(&tx, "engine.execute", json!({"command": "renderQueue.render"}));
    h.run_steps(3);
    assert!(matches!(reply.try_recv(), Err(TryRecvError::Empty)));
    let inspect = send(&tx, "ui.inspect", json!({}));
    h.run_steps(2);
    assert_eq!(inspect.try_recv().unwrap()["ok"], true);
    gate.0.store(true, Ordering::SeqCst);
    let done = finish(&mut h, &reply);
    assert_eq!(done["result"]["rendering"], false);
    assert_eq!(done["result"]["items"][0]["statusLabel"], "Done", "{done}");
}

#[test]
fn malformed_waiting_render_params_return_an_error() {
    let (mut h, _gate, tx) = harness();
    let reply = send(&tx, "engine.execute", json!({"command": "renderQueue.render", "params": []}));
    h.run_steps(1);
    assert_eq!(reply.try_recv().unwrap()["ok"], false);
    assert!(!h.state().session.is_rendering());
}

fn wait_worker(h: &Harness<'_, EffectcraftApp>) {
    let end = Instant::now() + Duration::from_secs(4);
    while h.state().session.is_rendering() {
        assert!(Instant::now() < end, "render worker did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn direct_render_reply_keeps_its_items_after_queue_removal_and_a_new_render() {
    use effectcraft_ui_egui::control::{Outcome, handle};

    for no_running_jobs in [false, true] {
        let (mut h, gate, _tx) = harness();
        let ctx = egui::Context::default();
        let request = ControlRequest::new("engine.execute", json!({"command": "renderQueue.render"})).0;
        let Outcome::AwaitJobs(mut first) = handle(h.state_mut(), &ctx, &request, 0.0) else { panic!("expected a deferred render") };
        let original = first.result["items"][0].clone();
        // Also cover completion between the command's result and its jobs() snapshot.
        if no_running_jobs {
            first.jobs.clear();
        }
        gate.0.store(true, Ordering::SeqCst);
        wait_worker(&h);
        gate.0.store(false, Ordering::SeqCst);
        // The worker is done, but no frontend completion pass has run. Removing the item
        // must not erase the waiting reply, even when the next queue reuses the same id.
        h.state_mut().session.execute("renderQueue.remove", json!({"item": original["id"]})).unwrap();
        h.state_mut().session.poll_render();
        let second_path = std::env::temp_dir().join(format!("ec-ui-second-render-{}.mp4", std::process::id()));
        h.state_mut().session.execute("renderQueue.add", json!({"output": second_path.to_string_lossy()})).unwrap();
        let Outcome::AwaitJobs(second) = handle(h.state_mut(), &ctx, &request, 0.0) else { panic!("expected a second deferred render") };
        assert!(second.pending(h.state()));
        assert!(!first.pending(h.state()), "the first reply must not wait for a later job with the same jobs.list id");
        let done = first.finish(h.state_mut());
        assert_eq!(done["result"]["rendering"], false, "{done}");
        assert_eq!(done["result"]["items"][0]["statusLabel"], "Done", "{done}");
        assert_eq!(done["result"]["items"][0]["outputPath"], original["outputPath"]);
        assert_eq!(done["result"]["items"][0]["last_output"], original["outputPath"]);
        assert!(h.state().session.is_rendering());
        gate.0.store(true, Ordering::SeqCst);
        wait_worker(&h);
        h.state_mut().session.poll_render();
        assert!(!second.pending(h.state()));
        assert_eq!(second.finish(h.state_mut())["result"]["items"][0]["outputPath"], second_path.to_string_lossy().as_ref());
    }
}

#[test]
fn completed_direct_render_reply_is_not_delayed_by_a_later_script_render() {
    let (mut h, gate, tx) = harness();
    let first = send(&tx, "engine.execute", json!({"command": "renderQueue.render"}));
    h.run_steps(2);
    assert!(matches!(first.try_recv(), Err(TryRecvError::Empty)));
    gate.0.store(true, Ordering::SeqCst);
    wait_worker(&h);
    gate.0.store(false, Ordering::SeqCst);
    h.state_mut().session.poll_render();
    h.state_mut().session.execute("renderQueue.setRender", json!({"index": 1, "render": true})).unwrap();
    let second = send(&tx, "engine.execute", json!({"command": "script.run", "params": {"code": "app.project.renderQueue.render(); 47;"}}));
    h.run_steps(1);
    assert!(h.state().session.script_ui.render_host.is_some());
    assert!(matches!(second.try_recv(), Err(TryRecvError::Empty)));
    let done = first.try_recv().expect("an immutable completed reply does not enter the new script's undo group");
    assert_eq!(done["result"]["rendering"], false);
    assert_eq!(done["result"]["items"][0]["statusLabel"], "Done");
    gate.0.store(true, Ordering::SeqCst);
    assert_eq!(finish(&mut h, &second)["result"]["result"], 47);
}

#[test]
fn replacing_project_before_render_poll_returns_an_explicit_error() {
    use effectcraft_ui_egui::control::{Outcome, handle};

    let (mut h, gate, _tx) = harness();
    let request = ControlRequest::new("engine.execute", json!({"command": "renderQueue.render"})).0;
    let Outcome::AwaitJobs(waiter) = handle(h.state_mut(), &egui::Context::default(), &request, 0.0) else { panic!("expected a deferred render") };
    gate.0.store(true, Ordering::SeqCst);
    wait_worker(&h);
    h.state_mut().session.replace_project(Default::default(), None);
    assert!(!waiter.pending(h.state()));
    let reply = waiter.finish(h.state_mut());
    assert_eq!(reply["ok"], false, "{reply}");
    assert!(reply["error"].as_str().unwrap().contains("replaced before completion"));
}
