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

#[test]
fn model_list_and_direct_selection_agree_on_target_availability() {
    super::roto::hold_roto_cache_test_lock();
    let mut s = Session::default();
    let list = s.execute_checked("roto.models", json!({})).unwrap();
    let models = list["models"].as_array().unwrap();
    for id in ["sam2.1-hiera-tiny", "sam2.1-hiera-small", "sam2.1-hiera-base-plus", "sam2.1-hiera-large"] {
        let chosen = s.chosen_model(Task::Mask).to_string();
        let generation = s.models.status(Task::Mask).generation;
        let selected = s.execute_checked("roto.model.select", json!({"id": id}));
        if cfg!(target_arch = "wasm32") {
            assert!(!models.iter().any(|m| m["id"] == id));
            assert!(selected.is_err());
            assert_eq!(s.chosen_model(Task::Mask), chosen);
            assert_eq!(s.models.status(Task::Mask).generation, generation, "rejection must not alter model state");
        } else {
            assert!(models.iter().any(|m| m["id"] == id));
            selected.unwrap();
            assert_eq!(s.chosen_model(Task::Mask), id);
        }
    }
    s.execute_checked("roto.model.select", json!({"id": "classical"})).unwrap();
    assert_eq!(s.chosen_model(Task::Mask), "classical");
    assert!(models.iter().any(|m| m["id"] == "mobilesam"));
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
        // Roto Brush also offers SAM 2.1 in four sizes (video trackers; see crates/sam2).
        let expect: Vec<&str> = if task == Task::Mask && !cfg!(target_arch = "wasm32") {
            vec!["classical", model, "sam2.1-hiera-tiny", "sam2.1-hiera-small", "sam2.1-hiera-base-plus", "sam2.1-hiera-large"]
        } else {
            vec!["classical", model]
        };
        assert_eq!(ids, expect);
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
    #[cfg(not(target_arch = "wasm32"))]
    verify_installed_sam2_sizes(&mut s);
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

#[cfg(not(target_arch = "wasm32"))]
fn verify_installed_sam2_sizes(s: &mut Session) {
    // The SAM 2.1 sizes, each from its own file (EFFECTCRAFT_SAM2, _SAM2_SMALL, _SAM2_BASE_PLUS, _SAM2_LARGE =
    // path/to/model.safetensors of facebook/sam2.1-hiera-*): installed with its notice, loaded (verified, native
    // only) and used by Roto Brush 3.0, then dropped and removed.
    for (var, id) in [
        ("EFFECTCRAFT_SAM2", "sam2.1-hiera-tiny"),
        ("EFFECTCRAFT_SAM2_SMALL", "sam2.1-hiera-small"),
        ("EFFECTCRAFT_SAM2_BASE_PLUS", "sam2.1-hiera-base-plus"),
        ("EFFECTCRAFT_SAM2_LARGE", "sam2.1-hiera-large"),
    ] {
        let Ok(path) = std::env::var(var) else { continue };
        let r = s.execute_checked("roto.model.install", json!({"path": path})).unwrap();
        assert_eq!(r["id"], id, "{var}");
        let dir = s.models_dir().unwrap();
        let notice = std::fs::read_to_string(dir.join(format!("{id}.safetensors.NOTICE.txt"))).unwrap();
        assert!(notice.contains("Meta AI") && notice.contains("Apache-2.0"), "{notice}");
        s.execute_checked("roto.model.select", json!({"id": id})).unwrap();
        wait_loaded(s, Task::Mask);
        assert_eq!(s.models.status(Task::Mask).active.as_deref(), Some(id), "{var}");
        let params = crate::effects::Params { values: [("version".to_string(), effectcraft_keyframe::Value::Enum(2))].into_iter().collect() };
        assert_eq!(crate::effects::roto::model_for(&params).map(|m| m.info().id), Some(id));
        // the list says it is installed, and exactly one size is recommended
        let list = s.execute_checked("roto.models", json!({})).unwrap();
        let entry = list["models"].as_array().unwrap().iter().find(|m| m["id"] == id).unwrap();
        assert_eq!(entry["installed"], true);
        assert_eq!(list["models"].as_array().unwrap().iter().filter(|m| m["recommended"] == true).count(), 1);
        s.execute_checked("roto.model.select", json!({"id": "classical"})).unwrap();
        assert!(crate::effects::roto::model_for(&params).is_none());
        assert_eq!(s.execute_checked("roto.model.remove", json!({"id": id})).unwrap()["removed"], true);
        assert!(!dir.join(format!("{id}.safetensors")).exists(), "{id} removed");
    }
}

/// A Roto Brush model that has been idle for ten minutes is unloaded (the choice stays), one in use is not,
/// and a model that does not track use is never unloaded.
#[test]
fn an_idle_roto_model_is_unloaded() {
    use effectcraft_segment::{Loaded, MaskModel, ModelInfo, Prompt};
    struct Fake(Option<f64>);
    impl MaskModel for Fake {
        fn info(&self) -> &'static ModelInfo {
            &effectcraft_segment::SAM2_TINY
        }
        fn segment(&self, _: &[[f32; 3]], _: usize, _: usize, _: &Prompt) -> effectcraft_segment::Result<Vec<f32>> {
            Err("no weights".into())
        }
        fn idle_secs(&self) -> Option<f64> {
            self.0
        }
    }
    crate::tests::roto::hold_roto_cache_test_lock();
    let (mut s, d) = session("idle");
    for (idle, unloaded) in [(Some(5.0), false), (None, false), (Some(601.0), true)] {
        let m: Arc<dyn MaskModel> = Arc::new(Fake(idle));
        s.models.set_active(Task::Mask, Some(Loaded::Mask(m)));
        assert_eq!(s.models.status(Task::Mask).active.as_deref(), Some("sam2.1-hiera-tiny"));
        s.poll_models();
        assert_eq!(s.models.status(Task::Mask).active.is_none(), unloaded, "idle {idle:?}");
        assert_eq!(crate::effects::roto::installed_model().is_none(), unloaded, "the effect's model follows");
        if unloaded {
            assert!(s.events.iter().any(|e| matches!(e, crate::Event::Toast { message, .. } if message.contains("unloaded after 10 minutes"))));
        }
    }
    // the choice survives, and with the classic engine chosen reloading does nothing
    s.reload_roto_model();
    assert!(s.models.status(Task::Mask).active.is_none());
    crate::effects::roto::set_model(None);
    let _ = std::fs::remove_dir_all(d);
}

/// A model that finishes loading after the choice changed is dropped, not activated.
#[test]
fn a_load_for_an_old_choice_does_not_activate() {
    use effectcraft_segment::{Loaded, MaskModel, ModelInfo, Prompt};
    struct Fake;
    impl MaskModel for Fake {
        fn info(&self) -> &'static ModelInfo {
            &effectcraft_segment::SAM2_TINY
        }
        fn segment(&self, _: &[[f32; 3]], _: usize, _: usize, _: &Prompt) -> effectcraft_segment::Result<Vec<f32>> {
            Err("no weights".into())
        }
    }
    super::roto::hold_roto_cache_test_lock();
    let (mut s, d) = session("stale-load");
    let started = s.models.status(Task::Mask).generation;
    // the user picks another engine while the load runs
    s.execute_checked("roto.model.select", json!({"id": "classical"})).unwrap();
    s.execute_checked("roto.model.select", json!({"id": "sam2.1-hiera-tiny"})).unwrap();
    assert_ne!(s.models.status(Task::Mask).generation, started, "a changed choice is counted");
    let m: Arc<dyn MaskModel> = Arc::new(Fake);
    assert!(!s.models.set_active_if_current(Task::Mask, Loaded::Mask(m.clone()), started), "the stale load is dropped");
    assert!(s.models.status(Task::Mask).active.is_none() && crate::effects::roto::installed_model().is_none());
    let now = s.models.status(Task::Mask).generation;
    assert!(s.models.set_active_if_current(Task::Mask, Loaded::Mask(m), now), "a load for the current choice activates");
    assert_eq!(s.models.status(Task::Mask).active.as_deref(), Some("sam2.1-hiera-tiny"));
    crate::effects::roto::set_model(None);
    let _ = std::fs::remove_dir_all(d);
}

/// A stroke reloading an unloaded model never waits for the load and never retries one that already failed.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn reloading_for_a_stroke_does_not_block_or_retry_a_failed_load() {
    super::roto::hold_roto_cache_test_lock();
    let (mut s, d) = session("reload-failed");
    let info = effectcraft_segment::info("mobilesam").unwrap();
    let dir = s.models_dir().unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    // the right size, the wrong content: found as installed, then fails to load
    std::fs::File::create(dir.join(info.file_name)).unwrap().set_len(info.size).unwrap();
    s.execute_checked("roto.model.select", json!({"id": "mobilesam"})).unwrap();
    let t0 = std::time::Instant::now();
    while s.models.status(Task::Mask).busy.is_some() && t0.elapsed().as_secs() < 30 {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let failed = s.models.status(Task::Mask).error;
    assert!(failed.is_some(), "the unreadable file fails to load");
    s.reload_roto_model();
    let st = s.models.status(Task::Mask);
    assert!(st.busy.is_none(), "a failed model is not loaded again by every stroke");
    assert_eq!(st.error, failed);
    s.poll_models();
    let st = s.models.status(Task::Mask);
    assert_eq!(st.error, failed, "polling must keep the actionable error until an explicit retry");
    assert!(st.busy.is_none(), "a failed load must not restart on every poll");
    // an unloaded, not failed model loads on the worker: the call returns with the load still pending or done
    s.execute_checked("roto.model.select", json!({"id": "classical"})).unwrap();
    crate::effects::roto::set_model(None);
    let _ = std::fs::remove_dir_all(d);
}

/// With `EFFECTCRAFT_DOWNLOAD_SIZE=mobilesam` (or another downloadable registry id): the in-app
/// download verifies and loads the file. SAM 2 must refuse until a storytold mirror is available.
/// Skipped without the variable: downloadable entries need the network and large files.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn downloading_a_size_installs_and_loads_it() {
    let Ok(id) = std::env::var("EFFECTCRAFT_DOWNLOAD_SIZE") else { return };
    crate::tests::roto::hold_roto_cache_test_lock();
    let (mut s, d) = session("download");
    let info = effectcraft_segment::info(&id).unwrap();
    if info.url.is_empty() {
        assert!(s.execute_checked("roto.model.download", json!({"id": id, "wait": true})).unwrap_err().to_string().contains("Install from File"));
        return;
    }
    let r = s.execute_checked("roto.model.download", json!({"id": id, "wait": true})).unwrap();
    assert_eq!(r["id"], id.as_str(), "{r}");
    let dir = s.models_dir().unwrap();
    let file = dir.join(info.file_name);
    assert_eq!(std::fs::metadata(&file).unwrap().len(), info.size, "the whole file arrived");
    assert!(dir.join(format!("{}.NOTICE.txt", info.file_name)).exists());
    assert!(!dir.join(format!("{}.part", info.file_name)).exists(), "no partial file is left");
    s.execute_checked("roto.model.select", json!({"id": id})).unwrap();
    wait_loaded(&mut s, Task::Mask);
    assert_eq!(s.models.status(Task::Mask).active.as_deref(), Some(id.as_str()));
    let _ = std::fs::remove_dir_all(d);
}
