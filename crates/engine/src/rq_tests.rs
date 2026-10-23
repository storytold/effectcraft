//! Render Queue commands against mock exporters (real encoding is tested in `effectcraft-export`
//! and end to end in `effectcraft-host`).

use std::sync::{Arc, Mutex};

use effectcraft_project::render_queue::{OutputFormat, RenderQuality, RenderStatus};
use serde_json::json;

use crate::render_queue::CANCELLED;
use crate::{Event, ExportJob, ExportResult, Exporter, Session};

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

/// Records exports; fails items whose path contains "fail"; honours cancellation.
pub(crate) struct MockExporter {
    pub(crate) log: Mutex<Vec<String>>,
}

impl Exporter for MockExporter {
    fn formats(&self) -> Vec<OutputFormat> {
        OutputFormat::ALL.to_vec()
    }
    fn export(&self, job: &ExportJob, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<ExportResult, String> {
        self.log.lock().unwrap().push(job.path.to_string());
        if job.path.contains("fail") {
            return Err("boom".into());
        }
        let comp = job.project.comp(job.item.comp).ok_or("no comp")?;
        let n = job.item.settings.frame_count(comp);
        for k in 0..=n {
            if !progress(k, n) {
                return Err(CANCELLED.into());
            }
        }
        Ok(ExportResult { path: job.path.to_string(), frames: n, ..Default::default() })
    }
}

fn rq_session() -> (Session, Arc<MockExporter>) {
    let mut s = demo();
    let ex = Arc::new(MockExporter { log: Default::default() });
    s.exporter = Some(ex.clone());
    (s, ex)
}

#[test]
fn add_configure_render() {
    let (mut s, ex) = rq_session();
    assert!(s.is_enabled("renderQueue.add"));
    assert!(!s.is_enabled("renderQueue.render"));
    let a = s.execute("renderQueue.add", json!({})).unwrap();
    let name = s.project.item(s.active_comp_id().unwrap()).unwrap().name.clone();
    assert_eq!(a["index"], 1);
    assert_eq!(a["statusLabel"], "Queued");
    assert!(a["outputPath"].as_str().unwrap().ends_with(&format!("{name}.mp4")), "{a}");
    let id = a["item"].as_u64().unwrap();
    // Render Settings / Output Module / Output To.
    let r = s
        .execute("renderQueue.setRenderSettings", json!({"item": id, "resolution": "half", "timeSpan": "custom", "start": 1.0, "end": 2.0, "frameRate": 10}))
        .unwrap();
    assert_eq!(r["frames"], 10);
    assert_eq!(r["width"], 960);
    let r = s.execute("renderQueue.setOutputModule", json!({"item": id, "format": "png", "channels": "rgba"})).unwrap();
    assert!(r["outputPath"].as_str().unwrap().ends_with("_[#####].png"), "{r}");
    assert!(s.execute("renderQueue.setOutputModule", json!({"item": id, "format": "jpeg", "channels": "rgba"})).is_err());
    let r = s.execute("renderQueue.setOutput", json!({"item": id, "path": crate::tests_stubs::tmp_out("rq/out.mov")})).unwrap();
    assert_eq!(r["output"]["format"], "ProRes", "Output To with .mov switches to ProRes");
    assert_eq!(std::path::Path::new(r["outputPath"].as_str().unwrap()), std::path::PathBuf::from(crate::tests_stubs::tmp_out("rq/out.mov")));
    // A second item that fails, a third that is unqueued.
    let b = s.execute("renderQueue.add", json!({"format": "gif", "output": crate::tests_stubs::tmp_out("rq/fail.gif")})).unwrap();
    let c = s.execute("renderQueue.add", json!({"output": "/tmp/rq/skip.mp4"})).unwrap();
    s.execute("renderQueue.setRender", json!({"item": c["item"], "render": false})).unwrap();
    assert_eq!(s.project.render_queue[2].status.label(), "Unqueued");
    let r = s.execute("renderQueue.render", json!({})).unwrap();
    assert_eq!(r["items"].as_array().unwrap().len(), 2);
    assert_eq!(
        ex.log.lock().unwrap().iter().map(std::path::PathBuf::from).collect::<Vec<_>>(),
        [std::path::PathBuf::from(crate::tests_stubs::tmp_out("rq/out.mov")), std::path::PathBuf::from(crate::tests_stubs::tmp_out("rq/fail.gif"))]
    );
    let q = &s.project.render_queue;
    assert_eq!(q[0].status.label(), "Done");
    assert!(q[0].started.is_some() && q[0].render_time.is_some());
    assert_eq!(std::path::Path::new(q[0].last_output.as_deref().unwrap()), std::path::PathBuf::from(crate::tests_stubs::tmp_out("rq/out.mov")));
    assert_eq!(q[1].status, RenderStatus::Failed("boom".into()));
    assert_eq!(q[2].status.label(), "Unqueued");
    let events = s.drain_events();
    let toast = events.iter().find_map(|e| if let Event::Toast { message, error: true } = e { Some(message.as_str()) } else { None });
    assert_eq!(toast, Some("Render finished with 1 failed item(s): boom"), "the toast says why (#568)");
    // Reorder, duplicate, remove, undo.
    s.execute("renderQueue.move", json!({"item": b["item"], "to": 1})).unwrap();
    assert_eq!(s.project.render_queue[0].id, b["item"].as_u64().unwrap());
    let d = s.execute("renderQueue.duplicate", json!({"index": 2})).unwrap();
    assert_eq!(s.project.render_queue.len(), 4);
    assert_eq!(s.project.render_queue[2].id, d["item"].as_u64().unwrap());
    assert_eq!(s.project.render_queue[2].status.label(), "Queued");
    s.execute("renderQueue.remove", json!({"index": 3})).unwrap();
    assert_eq!(s.project.render_queue.len(), 3);
    s.undo();
    assert_eq!(s.project.render_queue.len(), 4);
}

#[test]
fn persists_in_project_file() {
    let (mut s, _) = rq_session();
    s.execute("renderQueue.add", json!({"format": "tiff", "output": "[compName]/[compName]_[####].tif", "quality": "draft"})).unwrap();
    let json = s.project.to_json();
    let p = effectcraft_project::Project::from_json(&json).unwrap();
    assert_eq!(p.render_queue, s.project.render_queue);
    assert_eq!(p.render_queue[0].settings.quality, RenderQuality::Draft);
    // Projects without a queue still load.
    let old = json.replace("\"render_queue\"", "\"_ignored\"");
    assert!(effectcraft_project::Project::from_json(&old).unwrap().render_queue.is_empty());
}

#[test]
fn failure_toast_names_the_first_reason() {
    struct Failing;
    impl Exporter for Failing {
        fn formats(&self) -> Vec<OutputFormat> {
            OutputFormat::ALL.to_vec()
        }
        fn export(&self, job: &ExportJob, _: &mut dyn FnMut(u64, u64) -> bool) -> Result<ExportResult, String> {
            let name = std::path::Path::new(job.path).file_name().unwrap().to_string_lossy().into_owned();
            Err(format!("cannot write {name}"))
        }
    }
    let mut s = demo();
    s.exporter = Some(Arc::new(Failing));
    s.execute("renderQueue.add", json!({"format": "gif", "output": crate::tests_stubs::tmp_out("rq/first.gif")})).unwrap();
    s.execute("renderQueue.add", json!({"format": "gif", "output": crate::tests_stubs::tmp_out("rq/second.gif")})).unwrap();
    s.execute("renderQueue.render", json!({})).unwrap();
    let events = s.drain_events();
    let toast = events.iter().find_map(|e| if let Event::Toast { message, error: true } = e { Some(message.as_str()) } else { None });
    assert_eq!(toast, Some("Render finished with 2 failed item(s), first: cannot write first.gif"));
}

#[test]
fn background_render_and_stop() {
    struct Slow;
    impl Exporter for Slow {
        fn formats(&self) -> Vec<OutputFormat> {
            OutputFormat::ALL.to_vec()
        }
        fn export(&self, _: &ExportJob, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<ExportResult, String> {
            for k in 0..10_000 {
                std::thread::sleep(std::time::Duration::from_millis(2));
                if !progress(k, 10_000) {
                    return Err(CANCELLED.into());
                }
            }
            Ok(Default::default())
        }
    }
    let mut s = demo();
    s.exporter = Some(Arc::new(Slow));
    s.execute("renderQueue.add", json!({"output": "/tmp/a.mp4"})).unwrap();
    s.execute("renderQueue.add", json!({"output": "/tmp/b.mp4"})).unwrap();
    s.execute("renderQueue.render", json!({"wait": false})).unwrap();
    assert!(s.is_rendering());
    assert!(!s.is_enabled("renderQueue.add"), "queue is locked while rendering");
    let t0 = std::time::Instant::now();
    while s.render_progress().is_some_and(|p| p.done < 3) {
        assert!(t0.elapsed().as_secs() < 20);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    s.poll_render();
    assert_eq!(s.project.render_queue[0].status.label(), "Rendering");
    s.execute("renderQueue.stop", json!({})).unwrap();
    while s.is_rendering() {
        assert!(t0.elapsed().as_secs() < 20);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    s.poll_render();
    assert!(s.render_job.is_none());
    assert_eq!(s.project.render_queue[0].status.label(), "User Stopped");
    assert_eq!(s.project.render_queue[1].status.label(), "User Stopped");
}

#[test]
fn without_exporter() {
    let mut s = demo();
    s.execute("renderQueue.add", json!({})).unwrap();
    assert!(!s.is_enabled("renderQueue.render"));
    let f = s.execute("renderQueue.formats", json!({})).unwrap();
    assert_eq!(f["formats"].as_array().unwrap().len(), 13);
    assert_eq!(f["formats"][0]["available"], false);
}

#[test]
fn queue_add_accepts_render_settings_templates_without_output_module_namesakes() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name":"C","width":64,"height":64,"duration":1})).unwrap();
    let all = s.execute("renderQueue.templates", json!({})).unwrap();
    let names = all["renderSettings"].as_array().unwrap();
    assert!(!names.is_empty());
    for row in names {
        let name = row["name"].as_str().unwrap();
        let expected = s.project.render_templates.render_settings(name).unwrap();
        s.execute("renderQueue.add", json!({"comp":"C","template":name})).unwrap();
        assert_eq!(s.project.render_queue.last().unwrap().settings, expected);
    }
    let count = s.project.render_queue.len();
    assert!(s.execute("renderQueue.add", json!({"comp":"C","template":"Unknown Settings"})).is_err());
    assert_eq!(s.project.render_queue.len(), count);
}

#[test]
fn locked_resize_uses_height_for_height_only_output_module_updates() {
    for change in [json!({"resizeHeight": 32}), json!({"resize": {"height": 32, "lockAspect": true}})] {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name":"Square","width":64,"height":64,"frameRate":10,"duration":1})).unwrap();
        s.execute("renderQueue.add", json!({})).unwrap();
        s.execute("renderQueue.setOutputModule", json!({"resize": {"width": 64, "lockAspect": true}})).unwrap();
        s.execute("renderQueue.setOutputModule", change).unwrap();
        let item = &s.project.render_queue[0];
        let comp = s.project.comp(item.comp).unwrap();
        assert_eq!(item.output.output_size(comp, &item.settings), (32, 32));
        assert_eq!(item.output.resize.height, 32);
    }
    let mut s = Session::default();
    s.execute("comp.new", json!({"name":"Square","width":64,"height":64,"duration":1})).unwrap();
    s.execute("renderQueue.add", json!({})).unwrap();
    s.execute("renderQueue.setOutputModule", json!({"resize": {"width": 32, "height": 54, "lockAspect": true}})).unwrap();
    let item = &s.project.render_queue[0];
    let comp = s.project.comp(item.comp).unwrap();
    assert_eq!(item.output.output_size(comp, &item.settings), (32, 32), "explicit width retains precedence");
}

/// The engine holds backend authorization privately; callers only receive a one-shot review id.
#[test]
fn overwrite_review_is_explicit_bound_and_consumed() {
    use crate::render_queue::{ExportPreflight, LocalFuture};
    struct Reviewed {
        calls: Mutex<Vec<serde_json::Value>>,
        collisions: Vec<String>,
    }
    impl Exporter for Reviewed {
        fn formats(&self) -> Vec<OutputFormat> {
            vec![OutputFormat::Gif]
        }
        fn preflight(&self, job: &ExportJob) -> Result<ExportPreflight, String> {
            Ok(ExportPreflight { collisions: self.collisions.clone(), destinations: vec![job.path.to_string()], authorization: json!({"snapshot": job.path}) })
        }
        fn export(&self, _: &ExportJob, _: &mut dyn FnMut(u64, u64) -> bool) -> Result<ExportResult, String> {
            Err("unreviewed export must not run".into())
        }
        fn export_authorized_async<'a>(
            &'a self,
            job: &'a ExportJob<'a>,
            token: &'a serde_json::Value,
            _: &'a mut dyn FnMut(u64, u64) -> bool,
        ) -> LocalFuture<'a, Result<ExportResult, String>> {
            Box::pin(async move {
                assert_eq!(token["snapshot"], job.path);
                self.calls.lock().unwrap().push(token.clone());
                Ok(ExportResult { path: job.path.into(), ..Default::default() })
            })
        }
    }
    for existing in [false, true] {
        let ex = Arc::new(Reviewed { calls: Mutex::default(), collisions: if existing { vec!["old.gif".into()] } else { vec![] } });
        let mut s = Session { exporter: Some(ex.clone()), ..Default::default() };
        s.execute("comp.new", json!({"width": 4, "height": 4, "duration": 0.2})).unwrap();
        s.execute("renderQueue.add", json!({"format": "gif", "output": crate::tests_stubs::tmp_out("review.gif")})).unwrap();
        if existing {
            assert!(s.start_render(true).unwrap_err().contains("already exists"));
            assert!(s.execute_checked("renderQueue.render", json!({"overwrite": true})).is_err());
            assert!(ex.calls.lock().unwrap().is_empty());
        }
        let review = s.execute_checked("renderQueue.preflight", json!({})).unwrap();
        assert_eq!(review["collisions"].as_array().unwrap().len(), usize::from(existing));
        assert!(review.get("authorization").is_none());
        s.execute_checked("renderQueue.cancelPreflight", json!({"confirmation": review["confirmation"]})).unwrap();
        assert!(s.execute("renderQueue.render", json!({"confirmation": review["confirmation"], "overwrite": true})).is_err());
        assert!(ex.calls.lock().unwrap().is_empty(), "Cancel never calls the exporter");
        let review = s.execute("renderQueue.preflight", json!({})).unwrap();
        s.execute("renderQueue.setOutput", json!({"path": crate::tests_stubs::tmp_out("changed.gif")})).unwrap();
        assert!(
            s.execute("renderQueue.render", json!({"confirmation": review["confirmation"], "overwrite": true})).unwrap_err().to_string().contains("changed")
        );
        assert!(ex.calls.lock().unwrap().is_empty());
        let review = s.execute_checked("renderQueue.preflight", json!({})).unwrap();
        s.execute_checked("renderQueue.render", json!({"confirmation": review["confirmation"], "overwrite": existing})).unwrap();
        assert_eq!(ex.calls.lock().unwrap().len(), 1);
        s.execute("renderQueue.setRender", json!({"render": true})).unwrap();
        assert!(s.execute("renderQueue.render", json!({"confirmation": review["confirmation"], "overwrite": true})).is_err());
        assert_eq!(ex.calls.lock().unwrap().len(), 1, "consent cannot carry over to a stored queue rerender");
        s.execute("renderQueue.duplicate", json!({})).unwrap();
        assert!(s.execute("renderQueue.preflight", json!({})).unwrap_err().to_string().contains("same output destination"));
        assert_eq!(ex.calls.lock().unwrap().len(), 1, "overlapping queue outputs are rejected before rendering");
    }
}

#[test]
fn overwrite_preflight_bounds_segments_before_materializing_them() {
    let mut s = Session::default();
    let ex = Arc::new(MockExporter { log: Mutex::default() });
    s.exporter = Some(ex.clone());
    s.execute("comp.new", json!({"width": 4, "height": 4, "frameRate": 60, "duration": 86400})).unwrap();
    s.execute("renderQueue.add", json!({"format": "png", "output": "long_[#####].png"})).unwrap();
    s.prefs.export.segment_sequences = true;
    s.prefs.export.segment_sequence_files = 1;
    let error = s.preflight_render().unwrap_err();
    assert!(error.contains("100000 output segments"), "{error}");
    assert!(s.render_review.is_none());
    assert!(ex.log.lock().unwrap().is_empty());
}

#[test]
fn overwrite_preflight_bounds_repeated_path_bytes_before_segment_expansion() {
    let mut s = Session::default();
    let ex = Arc::new(MockExporter { log: Mutex::default() });
    s.exporter = Some(ex.clone());
    s.execute("comp.new", json!({"width": 4, "height": 4, "frameRate": 10, "duration": 10})).unwrap();
    let path = format!("{}_[#####].png", "x".repeat(1_000_000));
    s.execute("renderQueue.add", json!({"format": "png", "output": path})).unwrap();
    s.prefs.export.segment_sequences = true;
    s.prefs.export.segment_sequence_files = 1;
    let error = s.preflight_render().unwrap_err();
    assert!(error.contains("output plan exceeds 64 MiB"), "{error}");
    assert!(s.render_review.is_none());
    assert!(ex.log.lock().unwrap().is_empty());
}

#[test]
fn fresh_preflight_accepts_more_than_100000_destinations() {
    use crate::render_queue::ExportPreflight;
    struct LongSequence;
    impl Exporter for LongSequence {
        fn formats(&self) -> Vec<OutputFormat> {
            vec![OutputFormat::PngSequence]
        }
        fn preflight(&self, _: &ExportJob) -> Result<ExportPreflight, String> {
            Ok(ExportPreflight {
                collisions: vec![],
                destinations: (0..100_001).map(|frame| format!("/fresh/f_{frame}.png")).collect(),
                authorization: json!({"bounded": true}),
            })
        }
        fn export(&self, _: &ExportJob, _: &mut dyn FnMut(u64, u64) -> bool) -> Result<ExportResult, String> {
            Err("preflight must not render".into())
        }
    }
    let mut session = Session { exporter: Some(Arc::new(LongSequence)), ..Default::default() };
    session.execute("comp.new", json!({"width":4, "height":4, "duration":1})).unwrap();
    session.execute("renderQueue.add", json!({"format":"png", "output": crate::tests_stubs::tmp_out("long_[#####].png")})).unwrap();
    let review = session.preflight_render().unwrap();
    assert!(review["collisions"].as_array().unwrap().is_empty());
}
