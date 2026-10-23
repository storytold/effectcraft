//! Headless real-export regressions for the engine's one-shot output review.

use effectcraft_engine::Session;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("effectcraft-overwrite-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
    fn write(&self, name: &str, bytes: &[u8]) {
        std::fs::write(self.0.join(name), bytes).unwrap();
    }
    fn read(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.0.join(name)).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn session() -> Session {
    let mut s = effectcraft_host::session();
    s.execute("comp.new", json!({"name": "Overwrite", "width": 4, "height": 4, "frameRate": 10, "duration": 0.2})).unwrap();
    s
}

fn accept(s: &mut Session, review: &Value) {
    let result = s.execute_checked("renderQueue.render", json!({"wait": true, "confirmation": review["confirmation"], "overwrite": true})).unwrap();
    for item in result["items"].as_array().unwrap() {
        assert_eq!(item["statusLabel"], "Done", "{item}");
    }
}

#[test]
fn same_filename_and_stored_queue_rerender_require_fresh_acceptance() {
    let f = Fixture::new();
    f.write("movie.gif", b"old movie");
    let mut s = session();
    s.execute("renderQueue.add", json!({"format": "gif", "output": f.path("movie.gif")})).unwrap();
    assert!(s.execute("renderQueue.render", json!({})).is_err());
    let review = s.execute("renderQueue.preflight", json!({})).unwrap();
    assert_eq!(review["collisions"], json!([f.path("movie.gif")]));
    assert_eq!(f.read("movie.gif"), b"old movie");
    s.execute("renderQueue.cancelPreflight", json!({"confirmation": review["confirmation"]})).unwrap();
    assert!(s.execute("renderQueue.render", json!({"confirmation": review["confirmation"], "overwrite": true})).is_err());
    assert_eq!(f.read("movie.gif"), b"old movie");
    let review = s.execute("renderQueue.preflight", json!({})).unwrap();
    accept(&mut s, &review);
    let rendered = f.read("movie.gif");
    assert!(rendered.starts_with(b"GIF89a"));
    s.execute("renderQueue.setRender", json!({"render": true})).unwrap();
    assert!(s.execute("renderQueue.render", json!({})).is_err());
    assert_eq!(f.read("movie.gif"), rendered);
    let review = s.execute("renderQueue.preflight", json!({})).unwrap();
    accept(&mut s, &review);
}

#[test]
fn sequence_review_covers_exact_frames_and_keeps_old_tail_and_unrelated_files() {
    let f = Fixture::new();
    for name in ["seq_00.png", "seq_01.png", "seq_02.png", "unrelated.png"] {
        f.write(name, name.as_bytes());
    }
    let mut s = session();
    s.execute("renderQueue.add", json!({"format": "png", "output": f.path("seq_[##].png")})).unwrap();
    let review = s.execute("renderQueue.preflight", json!({})).unwrap();
    assert_eq!(review["collisions"], json!([f.path("seq_00.png"), f.path("seq_01.png")]));
    s.execute("renderQueue.cancelPreflight", json!({"confirmation": review["confirmation"]})).unwrap();
    assert_eq!(f.read("seq_00.png"), b"seq_00.png");
    assert_eq!(f.read("seq_01.png"), b"seq_01.png");
    let review = s.execute("renderQueue.preflight", json!({})).unwrap();
    accept(&mut s, &review);
    for name in ["seq_00.png", "seq_01.png"] {
        assert!(f.read(name).starts_with(b"\x89PNG"));
    }
    assert_eq!(f.read("seq_02.png"), b"seq_02.png");
    assert_eq!(f.read("unrelated.png"), b"unrelated.png");
}

#[test]
fn fresh_destination_needs_no_acceptance_and_late_files_are_not_approved() {
    let f = Fixture::new();
    let mut s = session();
    s.execute("renderQueue.add", json!({"format": "png", "output": f.path("seq_[##].png")})).unwrap();
    let review = s.execute("renderQueue.preflight", json!({})).unwrap();
    assert_eq!(review["collisions"], json!([]));
    f.write("seq_01.png", b"appeared after review");
    let r = s.execute("renderQueue.render", json!({"wait": true, "confirmation": review["confirmation"]})).unwrap();
    assert_eq!(r["items"][0]["statusLabel"], "Failed");
    assert_eq!(f.read("seq_01.png"), b"appeared after review");
    assert!(!f.0.join("seq_00.png").exists(), "the backend refuses before publishing any frame");
    s.execute("renderQueue.setOutput", json!({"path": f.path("fresh_[##].png")})).unwrap();
    s.execute("renderQueue.setRender", json!({"render": true})).unwrap();
    let r = s.execute("renderQueue.render", json!({"wait": true})).unwrap();
    assert_eq!(r["items"][0]["statusLabel"], "Done", "{r}");
    assert!(f.read("fresh_00.png").starts_with(b"\x89PNG"));
}

#[test]
fn cancelled_confirmed_sequence_preserves_previous_bytes() {
    use effectcraft_engine::project::render_queue::OutputFormat;
    use effectcraft_engine::render_queue::{ExportPreflight, LocalFuture};
    use effectcraft_engine::{ExportJob, ExportResult, Exporter};
    struct Cancel(effectcraft_host::FileExporter);
    impl Exporter for Cancel {
        fn formats(&self) -> Vec<OutputFormat> {
            self.0.formats()
        }
        fn preflight(&self, job: &ExportJob) -> Result<ExportPreflight, String> {
            self.0.preflight(job)
        }
        fn export(&self, job: &ExportJob, _: &mut dyn FnMut(u64, u64) -> bool) -> Result<ExportResult, String> {
            self.0.export(job, &mut |_, _| false)
        }
        fn export_authorized_async<'a>(
            &'a self,
            job: &'a ExportJob<'a>,
            authorization: &'a Value,
            _: &'a mut dyn FnMut(u64, u64) -> bool,
        ) -> LocalFuture<'a, Result<ExportResult, String>> {
            Box::pin(async move { self.0.export_authorized(job, authorization, &mut |_, _| false) })
        }
    }
    let f = Fixture::new();
    f.write("seq_00.png", b"old frame zero");
    f.write("seq_01.png", b"old frame one");
    let mut s = session();
    s.exporter = Some(std::sync::Arc::new(Cancel(effectcraft_host::FileExporter::default())));
    s.execute("renderQueue.add", json!({"format": "png", "output": f.path("seq_[##].png")})).unwrap();
    let review = s.execute("renderQueue.preflight", json!({})).unwrap();
    let r = s.execute("renderQueue.render", json!({"confirmation": review["confirmation"], "overwrite": true})).unwrap();
    assert_eq!(r["items"][0]["statusLabel"], "User Stopped", "{r}");
    assert_eq!(f.read("seq_00.png"), b"old frame zero");
    assert_eq!(f.read("seq_01.png"), b"old frame one");
    assert_eq!(std::fs::read_dir(&f.0).unwrap().count(), 2, "cancelled staging is cleaned up");
}
