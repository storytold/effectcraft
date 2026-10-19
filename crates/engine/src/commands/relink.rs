//! Recover moved media without replacing interpretation, sequence ranges or layer timing.

use std::collections::BTreeMap;
use std::path::Path;

use effectcraft_project::{Footage, FootageKind, ItemId, ItemKind};
use serde_json::{Value, json};

use super::{CommandSpec, bad, str_p};
use crate::{Result, Session, cmd};

const COMMAND: &str = "file.relinkFootage";
const ENTRY_LIMIT: usize = 100_000;

fn enabled(_: &Session) -> std::result::Result<(), String> {
    if cfg!(target_arch = "wasm32") { Err("folder relinking requires the desktop filesystem".into()) } else { Ok(()) }
}

fn files(folder: &Path, ctl: &crate::jobs::TaskCtl) -> std::result::Result<BTreeMap<String, Vec<String>>, String> {
    let mut pending = vec![folder.to_path_buf()];
    let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut count = 0usize;
    while let Some(dir) = pending.pop() {
        ctl.message(dir.display().to_string());
        let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries {
            if ctl.cancelled() {
                return Err(crate::render_queue::CANCELLED.into());
            }
            count = count.saturating_add(1);
            if count > ENTRY_LIMIT {
                return Err(format!("search exceeds {ENTRY_LIMIT} entries; choose a smaller folder"));
            }
            let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
            let ty = entry.file_type().map_err(|e| format!("{}: {e}", entry.path().display()))?;
            // Do not follow symlinks: no cycles or traversal outside the chosen tree.
            if ty.is_dir() {
                pending.push(entry.path());
            } else if ty.is_file()
                && let (Some(name), Some(path)) = (entry.file_name().to_str(), entry.path().to_str())
            {
                found.entry(name.to_string()).or_default().push(path.to_string());
            }
        }
    }
    for paths in found.values_mut() {
        paths.sort();
    }
    Ok(found)
}

fn compatible(old: &Footage, new: &Footage) -> bool {
    let kind = if old.kind == FootageKind::Sequence { FootageKind::Still } else { old.kind };
    kind == new.kind
        && old.width == new.width
        && old.height == new.height
        && old.has_video == new.has_video
        && old.has_audio == new.has_audio
        && (old.kind == FootageKind::Audio || (old.width > 0 && old.height > 0))
        && (old.codec.is_empty() || old.codec == new.codec)
        && (!matches!(old.kind, FootageKind::Video | FootageKind::Audio)
            || (old.native_rate.unwrap_or(old.frame_rate) == new.frame_rate && old.frame_rate.frame_at(old.duration) == new.frame_rate.frame_at(new.duration)))
}

fn sequence_at(old: &Footage, path: &str) -> Option<Vec<String>> {
    let parent = Path::new(path).parent()?;
    old.sequence
        .iter()
        .map(|file| {
            let mapped = parent.join(Path::new(file).file_name()?);
            std::fs::symlink_metadata(&mapped).is_ok_and(|m| m.is_file()).then(|| mapped.to_str().map(str::to_string)).flatten()
        })
        .collect()
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let folder = str_p(p, "folder").filter(|f| !f.trim().is_empty()).ok_or_else(|| bad(COMMAND, "missing `folder`"))?;
    let folder = std::fs::canonicalize(folder).map_err(|e| bad(COMMAND, format!("{folder}: {e}")))?;
    if !folder.is_dir() {
        return Err(bad(COMMAND, "folder must be a directory"));
    }
    let importer = s.importer.clone().ok_or_else(|| bad(COMMAND, "media import is not available in this build"))?;
    let services = s.services.clone();
    let stored = s.stored_paths.clone();
    let missing: Vec<(ItemId, Footage)> = s
        .project
        .items
        .values()
        .filter_map(|item| match &item.kind {
            ItemKind::Footage(f) if !f.path.is_empty() && !stored.exists(services.as_ref(), &f.path) => Some((item.id, f.clone())),
            _ => None,
        })
        .collect();
    let dry_run = p.get("dryRun").and_then(Value::as_bool).unwrap_or(false);
    let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(false);
    s.spawn_task("relink", "Relinking missing footage", wait, move |ctl| {
        let index = files(&folder, ctl)?;
        let mut rows = vec![];
        let mut updates = vec![];
        for (n, (id, old)) in missing.iter().enumerate() {
            if !ctl.progress(n as u64, missing.len() as u64) {
                return Err(crate::render_queue::CANCELLED.into());
            }
            let supported = matches!(old.kind, FootageKind::Still | FootageKind::Sequence | FootageKind::Video | FootageKind::Audio)
                && old.layer.is_none() && old.page == 0;
            let name = Path::new(&old.path).file_name().and_then(|n| n.to_str()).unwrap_or("");
            let paths = index.get(name).cloned().unwrap_or_default();
            let mut candidates = vec![];
            let mut accepted = vec![];
            for path in paths {
                if ctl.cancelled() {
                    return Err(crate::render_queue::CANCELLED.into());
                }
                let mut reason = "source type requires manual replacement".to_string();
                let mut confidence = "low";
                if supported {
                    match importer.probe(&path) {
                        Ok(f) if compatible(old, &f) => {
                            if old.kind != FootageKind::Sequence || (!old.sequence.is_empty() && sequence_at(old, &path).is_some()) {
                                confidence = "high";
                                reason = "exact filename and compatible source metadata".into();
                                accepted.push(path.clone());
                            } else {
                                reason = "sequence is incomplete".into();
                            }
                        }
                        Ok(_) => reason = "source metadata differs".into(),
                        Err(e) => reason = format!("cannot decode candidate: {e}"),
                    }
                }
                if ctl.cancelled() {
                    return Err(crate::render_queue::CANCELLED.into());
                }
                candidates.push(json!({"path": path, "confidence": confidence, "reason": reason}));
            }
            let status = if !supported { "unsupported" } else if candidates.is_empty() { "notFound" } else if candidates.len() > 1 { "ambiguous" } else if accepted.len() == 1 { "matched" } else { "incompatible" };
            if status == "matched" && let Some(path) = accepted.first() {
                let mut new = old.clone();
                new.path = path.clone();
                if old.kind == FootageKind::Sequence {
                    new.sequence = sequence_at(old, path).ok_or_else(|| "sequence changed during search; run the search again".to_string())?;
                }
                new.missing = false;
                updates.push((*id, old.clone(), new));
            }
            rows.push(json!({"item": id.0, "original": old.path, "status": status, "candidates": candidates}));
        }
        ctl.progress(missing.len() as u64, missing.len() as u64);
        let apply: crate::jobs::Apply = Box::new(move |s| {
            // A background search must not overwrite edits made while it was running.
            for (id, old, new) in &updates {
                if s.project.item(*id).is_none_or(|i| !matches!(&i.kind, ItemKind::Footage(f) if f == old))
                    || stored.exists(services.as_ref(), &old.path)
                    || !services.exists(&new.path)
                    || (new.kind == FootageKind::Sequence && new.sequence.iter().any(|p| !services.exists(p)))
                {
                    return Err(bad(COMMAND, "footage changed during search; run the search again"));
                }
            }
            let matched = updates.len();
            if !dry_run && !updates.is_empty() {
                for (_, _, new) in &updates {
                    s.stored_paths.allow(new);
                }
                s.edit("Relink Missing Footage", None, |project, _| {
                    for (id, _, new) in updates {
                        if let Some(item) = project.item_mut(id) {
                            item.kind = ItemKind::Footage(new);
                        }
                    }
                    Ok(())
                })?;
            }
            let unresolved = rows.len().saturating_sub(matched);
            let toast = format!("{} {matched} high-confidence match(es); {unresolved} unresolved. Duplicate filenames are left for Replace Footage.", if dry_run { "Found" } else { "Relinked" });
            Ok(json!({"dryRun": dry_run, "matched": matched, "relinked": if dry_run { 0 } else { matched }, "unresolved": unresolved, "items": rows, "toast": toast}))
        });
        Ok(apply)
    })
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "file.relinkFootage",
        "Relink Missing Footage...",
        ["File", "Dependencies"],
        None,
        "{folder: string (recursive desktop search, no symlinks; max 100000 entries), dryRun?: bool (report only), wait?: bool (return results inline; otherwise jobs.list/jobs.wait)} — relinks only unique exact filenames with compatible metadata; preserves interpretation and sequence ranges. Reports high/low confidence and ambiguous, incompatible, unsupported or notFound items.",
        enabled,
        run
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct Probe;
    impl crate::Importer for Probe {
        fn probe(&self, path: &str) -> std::result::Result<Footage, String> {
            if std::fs::read(path).map_err(|e| e.to_string())? != b"valid" {
                return Err("invalid media".into());
            }
            Ok(Footage { path: path.into(), width: 64, height: 32, has_video: true, codec: "PNG".into(), ..Default::default() })
        }
    }

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!("ec-relink-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
            std::fs::create_dir(&dir).unwrap();
            Self(std::fs::canonicalize(dir).unwrap())
        }
        fn file(&self, name: &str, bytes: &[u8]) -> String {
            let path = self.0.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, bytes).unwrap();
            path.to_str().unwrap().into()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn setup(dir: &Fixture, name: &str) -> (Session, ItemId, Footage) {
        let mut s = Session { importer: Some(Arc::new(Probe)), ..Default::default() };
        let path = dir.file(&format!("old/{name}"), b"valid");
        let id = s.execute("file.import", json!({"paths": [path], "sequence": false})).unwrap()["items"][0].as_u64().unwrap();
        let id = ItemId(id);
        let ItemKind::Footage(f) = &mut Arc::make_mut(&mut s.project).item_mut(id).unwrap().kind else { panic!("footage") };
        f.invert_alpha = true;
        f.loop_count = 3;
        f.start_timecode = Some(1001);
        let old = f.clone();
        std::fs::remove_file(&old.path).unwrap();
        (s, id, old)
    }

    fn search(s: &mut Session, dir: &Fixture, dry: bool) -> Value {
        s.execute(COMMAND, json!({"folder": dir.0.join("new"), "wait": true, "dryRun": dry})).unwrap()
    }

    fn get(s: &Session, id: ItemId) -> Footage {
        let ItemKind::Footage(f) = &s.project.item(id).unwrap().kind else { panic!("footage") };
        f.clone()
    }

    #[test]
    fn recursive_relink_preserves_interpretation_and_supports_undo_redo() {
        let dir = Fixture::new();
        let (mut s, id, old) = setup(&dir, "plate.png");
        let new = dir.file("new/deep/shots/plate.png", b"valid");
        s.execute("comp.new", json!({"name": "Main", "width": 64, "height": 32, "duration": 1})).unwrap();
        s.execute("layer.addItem", json!({"item": id.0})).unwrap();
        let key = |s: &Session| {
            let cid = s.active_comp_id().unwrap();
            let c = s.project.comp(cid).unwrap();
            let ctx = crate::render::eval::EvalCtx::new(&s.project, cid, c, crate::time::Tick::ZERO);
            crate::render::cache::input_key(&ctx, &c.layers[0], 1.0, false, false, 0).unwrap()
        };
        let original_key = key(&s);
        let revision = s.revision;
        let before = s.history.undo.len();
        let report = search(&mut s, &dir, true);
        assert_eq!(report["items"][0]["candidates"][0]["confidence"], "high");
        assert_eq!(report["relinked"], 0);
        assert_eq!(get(&s, id), old);
        assert_eq!(s.history.undo.len(), before);
        assert_eq!(search(&mut s, &dir, false)["relinked"], 1);
        let expected = Footage { path: new, missing: false, ..old.clone() };
        assert_eq!(get(&s, id), expected);
        assert!(s.revision > revision);
        assert_ne!(key(&s), original_key, "restored footage cannot reuse the missing source's rendered layer");
        assert_eq!(s.history.undo.len(), before + 1);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(get(&s, id), old);
        assert_eq!(key(&s), original_key);
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(get(&s, id), expected);
    }

    #[test]
    fn duplicates_and_invalid_media_are_reported_without_edits() {
        let dir = Fixture::new();
        let (mut s, id, old) = setup(&dir, "plate.png");
        dir.file("new/a/plate.png", b"valid");
        let b = dir.file("new/b/plate.png", b"valid");
        assert_eq!(search(&mut s, &dir, false)["items"][0]["status"], "ambiguous");
        assert_eq!(get(&s, id), old);
        std::fs::remove_file(b).unwrap();
        dir.file("new/a/plate.png", b"corrupt");
        let r = search(&mut s, &dir, false);
        assert_eq!(r["items"][0]["status"], "incompatible");
        assert_eq!(r["items"][0]["candidates"][0]["confidence"], "low");
        assert_eq!(get(&s, id), old);
        std::fs::remove_file(dir.0.join("new/a/plate.png")).unwrap();
        assert_eq!(search(&mut s, &dir, false)["items"][0]["status"], "notFound");
    }

    #[test]
    fn sequence_relink_preserves_picked_range_and_requires_every_saved_frame() {
        let dir = Fixture::new();
        let (mut s, id, mut old) = setup(&dir, "shot_0002.png");
        old.kind = FootageKind::Sequence;
        old.sequence = vec![old.path.clone(), dir.0.join("old/shot_0004.png").to_str().unwrap().into()];
        Arc::make_mut(&mut s.project).item_mut(id).unwrap().kind = ItemKind::Footage(old.clone());
        dir.file("new/shot_0002.png", b"valid");
        assert_eq!(search(&mut s, &dir, false)["relinked"], 0);
        dir.file("new/shot_0004.png", b"valid");
        dir.file("new/shot_0001.png", b"valid");
        assert_eq!(search(&mut s, &dir, false)["relinked"], 1);
        let f = get(&s, id);
        assert_eq!(f.sequence.len(), 2);
        assert!(f.sequence[0].ends_with("shot_0002.png"));
        assert!(f.sequence[1].ends_with("shot_0004.png"));
        assert_eq!(f.start_timecode, old.start_timecode);
    }

    #[test]
    fn invalid_folder_errors_and_existing_media_is_untouched() {
        let dir = Fixture::new();
        let (mut s, id, old) = setup(&dir, "plate.png");
        assert!(s.execute(COMMAND, json!({"folder": dir.0.join("absent"), "wait": true})).is_err());
        assert!(s.execute(COMMAND, json!({"wait": true})).is_err());
        dir.file("old/plate.png", b"valid");
        dir.file("new/plate.png", b"valid");
        assert_eq!(search(&mut s, &dir, false)["relinked"], 0);
        assert_eq!(get(&s, id), old);
    }

    struct BlockingProbe {
        started: std::sync::mpsc::Sender<()>,
        release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl crate::Importer for BlockingProbe {
        fn probe(&self, path: &str) -> std::result::Result<Footage, String> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.started.send(()).map_err(|e| e.to_string())?;
            self.release.lock().map_err(|e| e.to_string())?.recv_timeout(std::time::Duration::from_secs(10)).map_err(|e| e.to_string())?;
            Probe.probe(path)
        }
    }

    #[test]
    fn cancellation_between_candidate_probes_leaves_project_untouched() {
        let dir = Fixture::new();
        let (mut s, id, old) = setup(&dir, "plate.png");
        dir.file("new/a/plate.png", b"valid");
        dir.file("new/b/plate.png", b"valid");
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        s.importer = Some(Arc::new(BlockingProbe { started: started_tx, release: std::sync::Mutex::new(release_rx), calls: calls.clone() }));
        let r = s.execute(COMMAND, json!({"folder": dir.0.join("new")})).unwrap();
        started_rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert!(s.cancel_job(r["job"].as_str().unwrap()));
        release_tx.send(()).unwrap();
        s.execute("jobs.wait", json!({})).unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(get(&s, id), old);
        assert_eq!(s.job_log.last().unwrap().status, "cancelled");
    }

    #[test]
    fn background_result_does_not_overwrite_interpretation_edited_during_search() {
        let dir = Fixture::new();
        let (mut s, id, old) = setup(&dir, "plate.png");
        dir.file("new/plate.png", b"valid");
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        s.importer = Some(Arc::new(BlockingProbe { started: started_tx, release: std::sync::Mutex::new(release_rx), calls: Arc::default() }));
        s.execute(COMMAND, json!({"folder": dir.0.join("new")})).unwrap();
        started_rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        s.execute("file.interpretFootage", json!({"items": [id.0], "invertAlpha": false})).unwrap();
        release_tx.send(()).unwrap();
        s.execute("jobs.wait", json!({})).unwrap();
        assert_eq!(get(&s, id).path, old.path);
        assert!(!get(&s, id).invert_alpha);
        assert_eq!(s.job_log.last().unwrap().status, "failed");
    }

    #[test]
    fn movie_confidence_checks_native_frame_count_and_rate() {
        let new = Footage {
            kind: FootageKind::Video,
            width: 64,
            height: 32,
            has_video: true,
            frame_rate: crate::time::FrameRate::FPS_30,
            duration: crate::time::Tick::from_seconds_f64(1.0),
            ..Default::default()
        };
        let mut old = new.clone();
        old.native_rate = Some(old.frame_rate);
        old.frame_rate = crate::time::FrameRate::FPS_24;
        old.duration = old.frame_rate.tick_of(30);
        assert!(compatible(&old, &new), "conformed playback preserves native frame count");
        assert!(!compatible(&old, &Footage { duration: crate::time::Tick::from_seconds_f64(2.0), ..new.clone() }));
        assert!(!compatible(&old, &Footage { width: 128, ..new }));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_cycles_and_external_files_are_skipped() {
        let dir = Fixture::new();
        let (mut s, _, _) = setup(&dir, "plate.png");
        let outside = dir.file("outside/plate.png", b"valid");
        std::fs::create_dir(dir.0.join("new")).unwrap();
        std::os::unix::fs::symlink(dir.0.join("new"), dir.0.join("new/cycle")).unwrap();
        std::os::unix::fs::symlink(outside, dir.0.join("new/plate.png")).unwrap();
        assert_eq!(search(&mut s, &dir, false)["items"][0]["status"], "notFound");
    }
}
