//! Settings ▸ Roto Brush and Settings ▸ Face Tracking: the model registry, choosing, installing
//! (verified, with its notice) and loading trained models.

use std::sync::Arc;

use effectcraft_segment::Task;
use serde_json::json;

use crate::Session;
use crate::config::{ConfigStore, DirConfig};

fn session(tag: &str) -> (Session, std::path::PathBuf) {
    let d = std::env::temp_dir().join(format!("ec-models-{tag}-{}", std::process::id()));
    let store: Arc<dyn ConfigStore> = Arc::new(DirConfig::new(d.join("config")));
    (Session { config: Some(store), ..Default::default() }, d)
}

/// Poll until `task`'s model is in use (or a few seconds pass).
fn wait_loaded(s: &mut Session, task: Task) {
    for _ in 0..400 {
        s.poll_models();
        if s.models.status(task).active.is_some() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn models_are_listed_chosen_and_verified() {
    crate::tests::roto::hold_roto_cache_test_lock();
    let (mut s, d) = session("list");
    std::fs::create_dir_all(&d).unwrap();
    let bogus = d.join("bogus.bin");
    std::fs::write(&bogus, b"not the weights").unwrap();
    for (prefix, model, task) in [("roto", "mobilesam", Task::Mask), ("face", "mediapipe-face", Task::Face)] {
        let list = s.execute_checked(&format!("{prefix}.models"), json!({})).unwrap();
        let ids: Vec<&str> = list["models"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["classical", model]);
        let m = &list["models"][1];
        assert_eq!((m["licence"].as_str(), m["installed"].as_bool()), (Some("Apache-2.0"), Some(false)));
        assert!(m["url"].as_str().unwrap().starts_with("https://") && !m["authors"].as_str().unwrap().is_empty());
        // Unknown ids, the other task's models and files that aren't the published weights are
        // refused.
        let other = if task == Task::Mask { "mediapipe-face" } else { "mobilesam" };
        for id in ["nope", other] {
            assert!(s.execute_checked(&format!("{prefix}.model.select"), json!({"id": id})).is_err(), "{prefix} {id}");
        }
        let path = bogus.to_string_lossy();
        let e = s.execute_checked(&format!("{prefix}.model.install"), json!({"path": path})).unwrap_err().to_string();
        assert!(e.contains("SHA-256"), "{e}");
        assert!(s.execute_checked(&format!("{prefix}.model.install"), json!({"path": path, "id": model})).is_err());
        // Choosing a model that isn't installed keeps the classic engine.
        s.execute_checked(&format!("{prefix}.model.select"), json!({"id": model})).unwrap();
        assert_eq!(s.chosen_model(task), model);
        assert!(s.models.status(task).active.is_none());
        assert!(crate::prefs::pages().iter().any(|p| p.id == prefix));
    }
    assert!(s.models.face().is_none());
    assert_eq!(crate::prefs::page_id("Face Tracking"), Some("face"));
    // With the real weights (EFFECTCRAFT_MOBILESAM=path/to/mobile_sam.pt): install, load, use.
    if let Ok(path) = std::env::var("EFFECTCRAFT_MOBILESAM") {
        s.execute_checked("roto.model.install", json!({"path": path})).unwrap();
        wait_loaded(&mut s, Task::Mask);
        assert_eq!(s.models.status(Task::Mask).active.as_deref(), Some("mobilesam"));
        // Roto Brush 3.0 (the default version) now segments with it.
        let params = crate::effects::Params { values: [("version".to_string(), effectcraft_keyframe::Value::Enum(2))].into_iter().collect() };
        assert_eq!(crate::effects::roto::model_for(&params).map(|m| m.info().id), Some("mobilesam"));
        s.execute_checked("roto.model.select", json!({"id": "classical"})).unwrap();
        assert!(crate::effects::roto::model_for(&params).is_none());
    }
    // EFFECTCRAFT_FACE_LANDMARKER=path/to/face_landmarker.task: installed with its notice,
    // loaded for face tracking, dropped when the classic tracker is chosen, removed.
    if let Ok(path) = std::env::var("EFFECTCRAFT_FACE_LANDMARKER") {
        let r = s.execute_checked("face.model.install", json!({"path": path})).unwrap();
        assert_eq!(r["id"], "mediapipe-face");
        let dir = s.models_dir().unwrap();
        let notice = std::fs::read_to_string(dir.join("face_landmarker.task.NOTICE.txt")).unwrap();
        assert!(notice.contains("Google LLC") && notice.contains("Apache-2.0"), "{notice}");
        wait_loaded(&mut s, Task::Face);
        assert_eq!(s.models.face().map(|m| m.info().id), Some("mediapipe-face"));
        s.execute_checked("face.model.select", json!({"id": "classical"})).unwrap();
        assert!(s.models.face().is_none());
        assert_eq!(s.execute_checked("face.model.remove", json!({"id": "mediapipe-face"})).unwrap()["removed"], true);
        assert!(!dir.join("face_landmarker.task").exists() && !dir.join("face_landmarker.task.NOTICE.txt").exists());
    }
    let _ = std::fs::remove_dir_all(d);
}
