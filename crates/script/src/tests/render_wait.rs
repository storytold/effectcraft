//! #3289: a native frontend may service the event loop while a script waits for rendering.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use effectcraft_engine::project::render_queue::OutputFormat;
use effectcraft_engine::{ExportJob, ExportResult, Exporter, Session};
use serde_json::{Value, json};

/// A slow exporter without expensive frames. A bounded wait makes the old blocking path
/// fail the assertions instead of hanging the test process.
struct Gate {
    entered: AtomicUsize,
    permits: AtomicUsize,
    failed: bool,
}

impl Exporter for Gate {
    fn formats(&self) -> Vec<OutputFormat> {
        vec![OutputFormat::H264]
    }
    fn export(&self, job: &ExportJob, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<ExportResult, String> {
        let call = self.entered.fetch_add(1, Ordering::SeqCst) + 1;
        let end = Instant::now() + Duration::from_secs(2);
        while self.permits.load(Ordering::SeqCst) < call {
            if !progress(0, 1) {
                return Err(effectcraft_engine::render_queue::CANCELLED.into());
            }
            if Instant::now() > end {
                return Err("test exporter timed out waiting for the frontend".into());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        if self.failed { Err("test encoder failure".into()) } else { Ok(ExportResult { path: job.path.into(), frames: 1, ..Default::default() }) }
    }
}

fn session(permits: usize, failed: bool) -> (Session, Arc<Gate>) {
    let gate = Arc::new(Gate { entered: AtomicUsize::new(0), permits: AtomicUsize::new(permits), failed });
    let mut s = super::session();
    s.exporter = Some(gate.clone());
    s.execute("comp.new", json!({"name": "Render", "width": 16, "height": 16, "duration": 1})).unwrap();
    let path = std::env::temp_dir().join(format!("ec-script-render-{}.mp4", std::process::id()));
    s.execute("renderQueue.add", json!({"output": path.to_string_lossy()})).unwrap();
    s.prefs.auto_save.enabled = false;
    (s, gate)
}

fn finish(s: &mut Session) -> Value {
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(result) = crate::ui::poll_render_wait(s) {
            return result;
        }
        assert!(Instant::now() < end, "script did not resume");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn render_yields_without_finishing_the_script_or_its_undo_group() {
    let (mut s, gate) = session(0, false);
    s.script_ui.yield_renders = true;
    let before = s.history.undo.len();
    let out = crate::run_code(
        &mut s,
        "app.beginUndoGroup('Render transaction'); app.project.activeItem.layers.addNull().name = 'Before'; $.writeln('before'); app.project.renderQueue.render(); app.project.activeItem.layers.addNull().name = 'After'; $.writeln('after'); app.endUndoGroup(); [42, app.project.renderQueue.rendering];",
        "render.jsx",
    );
    assert!(out.error.is_none(), "{out:?}");
    assert!(out.render_host.is_some(), "blocking render returned instead of yielding: {out:?}");
    assert!(!out.waiting, "a render suspension is not a modal dialog");
    assert!(out.output.is_empty(), "output belongs to the eventual reply");
    assert_eq!(s.active_comp().unwrap().layers.len(), 1, "the continuation has not run");
    assert!(crate::ui::poll_render_wait(&mut s).is_none());
    assert!(!s.undo(), "undo cannot discard the suspended script's live edits");
    assert!(s.execute("engine.batch", json!({"steps": [{"command": "layer.newNull"}]})).is_err());
    assert!(s.execute("jobs.list", json!({})).is_err(), "status polling must not apply unrelated jobs");
    // The frontend resets the invocation-only opt-in after script.run returns.
    s.script_ui.yield_renders = false;
    gate.permits.store(1, Ordering::SeqCst);
    let done = finish(&mut s);
    assert_eq!(done["ok"], true, "{done}");
    assert_eq!(done["result"], json!([42, false]));
    assert_eq!(done["output"], "before\nafter");
    assert_eq!(s.project.render_queue[0].status.label(), "Done");
    assert_eq!(s.history.undo.len(), before + 1, "one undo group spans the render");
    assert_eq!(s.history.undo.last().unwrap().0, "Render transaction");
    assert!(s.script_ui.render_host.is_none());
    s.undo();
    assert!(s.active_comp().unwrap().layers.is_empty());
}

#[test]
fn a_second_render_keeps_the_same_script_pending() {
    let (mut s, gate) = session(0, false);
    s.script_ui.yield_renders = true;
    let out = crate::run_code(
        &mut s,
        "app.project.renderQueue.render(); app.project.renderQueue.item(1).render = true; app.project.renderQueue.render(); 17;",
        "twice.jsx",
    );
    let host = out.render_host.unwrap();
    s.script_ui.yield_renders = false;
    gate.permits.store(1, Ordering::SeqCst);
    let end = Instant::now() + Duration::from_secs(3);
    while gate.entered.load(Ordering::SeqCst) < 2 {
        assert!(crate::ui::poll_render_wait(&mut s).is_none(), "no premature reply after the first render");
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(s.script_ui.render_host, Some(host));
    gate.permits.store(2, Ordering::SeqCst);
    assert_eq!(finish(&mut s)["result"], 17);
}

#[test]
fn cancellation_and_encoder_failure_resume_with_terminal_statuses() {
    for failed in [false, true] {
        let (mut s, gate) = session(0, failed);
        s.script_ui.yield_renders = true;
        let out = crate::run_code(&mut s, "app.project.renderQueue.render(); [9, app.project.renderQueue.rendering];", "stopped.jsx");
        assert!(out.render_host.is_some());
        if failed {
            gate.permits.store(1, Ordering::SeqCst);
        } else {
            s.stop_render();
        }
        let done = finish(&mut s);
        assert_eq!(done["ok"], true, "the synchronous command reports status, not a thrown script error: {done}");
        assert_eq!(done["result"], json!([9, false]));
        assert_ne!(s.project.render_queue[0].status.label(), "Done");
    }
}

#[test]
fn headless_scripts_still_wait_and_start_errors_do_not_suspend() {
    let (mut s, _) = session(1, false);
    let out = crate::run_code(&mut s, "app.project.renderQueue.render(); [7, app.project.renderQueue.rendering];", "headless.jsx");
    assert!(out.error.is_none(), "{out:?}");
    assert_eq!(out.result, json!([7, false]));
    assert!(out.render_host.is_none());
    assert_eq!(s.project.render_queue[0].status.label(), "Done");
    s.script_ui.yield_renders = true;
    let out = crate::run_code(&mut s, "app.project.renderQueue.render();", "empty.jsx");
    assert!(out.error.is_some());
    assert!(out.render_host.is_none());
    assert!(s.script_ui.render_host.is_none());
}

#[test]
fn retained_palette_callbacks_can_yield_after_the_original_control_invocation() {
    let (mut s, gate) = session(0, false);
    s.script_ui.yield_renders = true;
    let out = crate::run_code(
        &mut s,
        "var w = new Window('palette', 'Render'); var b = w.add('button', undefined, 'Go', {name:'go'}); b.onClick = function () { app.project.renderQueue.render(); $.writeln('resumed'); }; w.show();",
        "palette-render.jsx",
    );
    assert!(out.error.is_none(), "{out:?}");
    s.script_ui.yield_renders = false;
    let result = s.execute("scriptui.click", json!({"widget": "go"})).unwrap();
    assert!(result["renderHost"].is_number(), "{result}");
    gate.permits.store(1, Ordering::SeqCst);
    assert_eq!(finish(&mut s)["output"], "resumed");
    let window = s.script_ui.windows[0].id;
    s.execute("scriptui.close", json!({"window": window})).unwrap();
}

#[test]
fn post_render_actions_complete_before_the_script_resumes() {
    use effectcraft_engine::project::{Footage, FootageKind};
    struct Probe;
    impl effectcraft_engine::Importer for Probe {
        fn probe(&self, path: &str) -> Result<Footage, String> {
            Ok(Footage { path: path.into(), kind: FootageKind::Still, width: 16, height: 16, has_video: true, ..Default::default() })
        }
    }
    for action in ["import", "setProxy"] {
        let (mut s, gate) = session(0, false);
        s.importer = Some(Arc::new(Probe));
        let id = s.project.render_queue[0].id;
        s.execute("renderQueue.setOutputModule", json!({"item": id, "postRenderAction": action})).unwrap();
        let before = s.project.items.len();
        let comp = s.active_comp_id().unwrap();
        s.script_ui.yield_renders = true;
        let out = crate::run_code(&mut s, "app.project.renderQueue.render(); 29;", "post-render.jsx");
        assert!(out.render_host.is_some(), "{out:?}");
        gate.permits.store(1, Ordering::SeqCst);
        // A query must not consume updates while the edit guard is still active.
        let end = Instant::now() + Duration::from_secs(3);
        while s.is_rendering() {
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
        s.execute("renderQueue.list", json!({})).unwrap();
        assert_eq!(s.project.items.len(), before);
        assert_eq!(finish(&mut s)["result"], 29);
        if action == "import" {
            assert_eq!(s.project.items.len(), before + 1);
        } else {
            assert!(s.project.item(comp).unwrap().proxy.is_some());
        }
    }
}

#[test]
fn deferred_errors_keep_their_included_source_location() {
    let (mut s, gate) = session(0, false);
    let dir = std::env::temp_dir().join(format!("ec-render-include-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("render.jsx"), "app.project.renderQueue.render();\nmissingAfterRender();").unwrap();
    std::fs::write(dir.join("main.jsx"), "#include 'render.jsx'").unwrap();
    s.script_ui.yield_renders = true;
    let out = crate::run_code(&mut s, "#include 'render.jsx'", &dir.join("main.jsx").to_string_lossy());
    assert!(out.render_host.is_some(), "{out:?}");
    gate.permits.store(1, Ordering::SeqCst);
    let done = finish(&mut s);
    assert_eq!(done["ok"], false);
    assert_eq!(done["error"]["line"], 2);
    assert!(done["error"]["file"].as_str().unwrap().ends_with("render.jsx"), "{done}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn included_error_mapping_survives_a_modal_yield_after_rendering() {
    let (mut s, gate) = session(0, false);
    let dir = std::env::temp_dir().join(format!("ec-render-modal-include-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("render.jsx"), "app.project.renderQueue.render();\nvar d = new Window('dialog', 'Continue'); d.add('button', undefined, 'OK', {name:'ok'}); d.show();\nmissingAfterModal();").unwrap();
    std::fs::write(dir.join("main.jsx"), "#include 'render.jsx'").unwrap();
    s.script_ui.yield_renders = true;
    let out = crate::run_code(&mut s, "#include 'render.jsx'", &dir.join("main.jsx").to_string_lossy());
    assert!(out.render_host.is_some(), "{out:?}");
    gate.permits.store(1, Ordering::SeqCst);
    assert_eq!(finish(&mut s)["waiting"], true);
    let done = s.execute("scriptui.click", json!({"widget": "ok"})).unwrap();
    assert_eq!(done["ok"], false);
    assert_eq!(done["error"]["line"], 3);
    assert!(done["error"]["file"].as_str().unwrap().ends_with("render.jsx"), "{done}");
    std::fs::remove_dir_all(dir).unwrap();
}
