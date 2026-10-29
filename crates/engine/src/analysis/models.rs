//! Trained models (Settings ▸ Roto Brush and Settings ▸ Face Tracking; `roto.model*` and
//! `face.model*` commands): which models the registry offers (`effectcraft_segment::MODELS`, all
//! open source), which are installed (in the `models` folder), installing one from a file or
//! downloading the official weights (verified against the registry's SHA-256 either way, and kept
//! with a notice naming their authors and licence), choosing one per task, and loading it in the
//! background: into the Roto Brush effect (`effects::roto::set_model`) or the face tracker
//! ([`Models::face`]).
//!
//! Weights are never bundled: the install stays small and nothing is fetched unless asked for.
//! Downloads use the system's `curl` (shipped with Windows 10+, macOS and most Linux systems),
//! so no networking code is linked into EffectCraft; without it, the file can be downloaded in a
//! browser and installed with Install from File. The browser build uses the built-in engines
//! only, for now.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use effectcraft_segment::face::FaceModel;
use effectcraft_segment::{self as seg, Loaded, ModelInfo, Task};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, b_p, bad, str_p};
use crate::{EngineError, Result, Session, cmd, query};

/// What a task's background worker reports.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// What is happening now ("Downloading MobileSAM…").
    pub busy: Option<String>,
    /// The model in use.
    pub active: Option<String>,
    /// The last failure, for the Settings page.
    pub error: Option<String>,
    /// A finished job's message and whether it failed, for a toast (taken by
    /// [`Session::poll_models`]).
    pub done: Option<(String, bool)>,
}

/// The session's trained models: per task, the worker's status; and the face model in use.
#[derive(Default)]
pub struct Models {
    roto: Arc<Mutex<Status>>,
    face: Arc<Mutex<Status>>,
    face_model: FaceSlot,
}

impl Models {
    fn lane(&self, task: Task) -> &Arc<Mutex<Status>> {
        match task {
            Task::Mask => &self.roto,
            Task::Face => &self.face,
        }
    }

    pub fn status(&self, task: Task) -> Status {
        self.lane(task).lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// The face model face tracking uses (`None`: the classic tracker).
    pub fn face(&self) -> Option<Arc<dyn FaceModel>> {
        self.face_model.lock().ok().and_then(|m| m.clone())
    }

    fn set_active(&self, task: Task, model: Option<Loaded>) {
        activate(self.lane(task), &self.face_model, task, model);
    }
}

type FaceSlot = Arc<Mutex<Option<Arc<dyn FaceModel>>>>;

/// Put a loaded model to use (or drop `task`'s): Roto Brush's lives in the effect, the face
/// tracker's in `face`.
fn activate(lane: &Arc<Mutex<Status>>, face: &FaceSlot, task: Task, model: Option<Loaded>) {
    let id = model.as_ref().map(|m| match m {
        Loaded::Mask(m) => m.info().id.to_string(),
        Loaded::Face(m) => m.info().id.to_string(),
    });
    match (task, model) {
        (Task::Mask, Some(Loaded::Mask(m))) => crate::effects::roto::set_model(Some(m)),
        (Task::Mask, _) => crate::effects::roto::set_model(None),
        (Task::Face, m) => {
            if let Ok(mut slot) = face.lock() {
                *slot = match m {
                    Some(Loaded::Face(f)) => Some(f),
                    _ => None,
                };
            }
        }
    }
    set(lane, |s| s.active = id);
}

fn set(status: &Arc<Mutex<Status>>, f: impl FnOnce(&mut Status)) {
    if let Ok(mut s) = status.lock() {
        f(&mut s);
    }
}

/// How a task shows in Settings, toasts and the command list.
struct Kind {
    /// Command prefix and settings key section.
    prefix: &'static str,
    label: &'static str,
    classic_name: &'static str,
    classic_description: &'static str,
}

const ROTO: Kind = Kind {
    prefix: "roto",
    label: "Roto Brush",
    classic_name: "Classic (graph cut)",
    classic_description: "Built in: colour models and graph cuts, propagated by optical flow. Always available.",
};

const FACE: Kind = Kind {
    prefix: "face",
    label: "Face Tracking",
    classic_name: "Classic (shape model)",
    classic_description: "Built in: a skin colour model, facial feature shapes and a fitted face shape model. Best on front-facing, evenly lit faces. Always available.",
};

fn kind(task: Task) -> &'static Kind {
    match task {
        Task::Mask => &ROTO,
        Task::Face => &FACE,
    }
}

impl Session {
    /// The folder installed models live in: the host's, else `models` next to the settings.
    pub fn models_dir(&self) -> Option<PathBuf> {
        self.models_dir.clone().or_else(|| Some(self.config.as_ref()?.dir()?.join("models")))
    }

    /// Where `m` is installed, if it is (a file of the published size).
    pub fn model_path(&self, m: &ModelInfo) -> Option<PathBuf> {
        let p = self.models_dir()?.join(m.file_name);
        std::fs::metadata(&p).ok().filter(|md| md.len() == m.size).map(|_| p)
    }

    /// The model chosen for `task` (Settings).
    pub fn chosen_model(&self, task: Task) -> &str {
        match task {
            Task::Mask => &self.prefs.roto.model,
            Task::Face => &self.prefs.face.model,
        }
    }

    fn choose_model(&mut self, task: Task, id: &str) {
        let slot = match task {
            Task::Mask => &mut self.prefs.roto.model,
            Task::Face => &mut self.prefs.face.model,
        };
        *slot = id.to_string();
    }

    /// Load (or drop) the models Settings chooses; called whenever settings change.
    pub fn apply_models(&mut self) {
        for task in [Task::Mask, Task::Face] {
            self.apply_model(task, false);
        }
    }

    /// Load (or drop) `task`'s chosen model; `wait`: before returning (scripts and agents).
    fn apply_model(&mut self, task: Task, wait: bool) {
        let want = self.chosen_model(task).to_string();
        let lane = self.models.lane(task).clone();
        let st = self.models.status(task);
        let info = seg::info(&want).filter(|m| m.task == task);
        let Some(info) = info else {
            if st.active.is_some() {
                self.models.set_active(task, None);
            }
            return;
        };
        if st.active.as_deref() == Some(info.id) || st.busy.is_some() {
            return;
        }
        let Some(path) = self.model_path(info) else {
            self.models.set_active(task, None);
            return;
        };
        let (face_slot, label) = (self.models.face_model.clone(), kind(task).label);
        spawn(&lane, label, format!("Loading {}…", info.name), wait, move |status| {
            let m = load_file(&path, info.id)?;
            activate(status, &face_slot, task, Some(m));
            Ok(format!("{label}: {} loaded", info.name))
        });
    }

    /// Finish what the model workers did: a toast, and loading a model that just arrived.
    pub fn poll_models(&mut self) {
        for task in [Task::Mask, Task::Face] {
            let done = self.models.lane(task).lock().ok().and_then(|mut s| s.done.take());
            if let Some((message, error)) = done {
                self.events.push(crate::Event::Toast { message, error });
                self.apply_model(task, false);
            }
        }
    }
}

fn load_file(path: &Path, id: &str) -> std::result::Result<Loaded, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    seg::load(id, &bytes)
}

/// Run `job` on a worker thread (`wait`: right here), reporting through `status` (desktop; the
/// browser build has no trained models yet).
fn spawn(
    status: &Arc<Mutex<Status>>,
    label: &'static str,
    what: String,
    wait: bool,
    job: impl FnOnce(&Arc<Mutex<Status>>) -> std::result::Result<String, String> + Send + 'static,
) {
    if cfg!(target_arch = "wasm32") {
        set(status, |s| s.error = Some("trained models are available in the desktop app".into()));
        return;
    }
    set(status, |s| {
        s.busy = Some(what);
        s.error = None;
    });
    let st = status.clone();
    let run = move || {
        let r = job(&st);
        set(&st, |s| {
            s.busy = None;
            match r {
                Ok(msg) => s.done = Some((msg, false)),
                Err(e) => {
                    s.done = Some((format!("{label} model: {e}"), true));
                    s.error = Some(e);
                }
            }
        });
    };
    if wait {
        run();
    } else if let Err(e) = std::thread::Builder::new().name("ec-model".into()).spawn(run) {
        set(status, |s| {
            s.busy = None;
            s.error = Some(format!("cannot start: {e}"));
        });
    }
}

/// The notice kept next to `m`'s weights.
fn notice_path(dir: &Path, m: &ModelInfo) -> PathBuf {
    dir.join(format!("{}.NOTICE.txt", m.file_name))
}

/// Copy verified weights into the models folder (atomically), with their notice.
fn install_bytes(dir: &Path, m: &ModelInfo, bytes: &[u8]) -> std::result::Result<PathBuf, String> {
    if bytes.len() as u64 != m.size || seg::sha256::hex(bytes) != m.sha256 {
        return Err(format!("this is not the published {} file (size or SHA-256 differs)", m.name));
    }
    use crate::config::FileOps;
    let path = dir.join(m.file_name);
    let files = crate::config::StdFiles;
    files.write(&path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    files.write(&notice_path(dir, m), seg::notice(m).as_bytes()).map_err(|e| format!("cannot write the notice: {e}"))?;
    Ok(path)
}

fn model_p(p: &Value, cmd: &str, task: Task) -> Result<&'static ModelInfo> {
    let id = str_p(p, "id").ok_or_else(|| bad(cmd, "missing `id`"))?;
    seg::info(id).filter(|m| m.task == task).ok_or_else(|| bad(cmd, format!("unknown model `{id}` (see {}.models)", kind(task).prefix)))
}

fn list(s: &mut Session, task: Task) -> Result<Value> {
    let k = kind(task);
    let st = s.models.status(task);
    let chosen = s.chosen_model(task).to_string();
    let mut v = vec![json!({
        "id": seg::CLASSICAL,
        "name": k.classic_name,
        "description": k.classic_description,
        "licence": "MIT OR Apache-2.0",
        "installed": true,
        "selected": chosen == seg::CLASSICAL,
        "active": st.active.is_none(),
    })];
    for m in seg::models(task) {
        v.push(json!({
            "id": m.id,
            "name": m.name,
            "description": m.description,
            "authors": m.authors,
            "licence": m.licence,
            "licenceUrl": m.licence_url,
            "homepage": m.homepage,
            "url": m.url,
            "size": m.size,
            "sha256": m.sha256,
            "installed": s.model_path(m).is_some(),
            "selected": chosen == m.id,
            "active": st.active.as_deref() == Some(m.id),
        }));
    }
    Ok(json!({"models": v, "busy": st.busy, "error": st.error, "folder": s.models_dir().map(|d| d.display().to_string())}))
}

/// After a `wait`: the job's failure, as the command's.
fn waited(s: &Session, task: Task, cmd: &str) -> Result<()> {
    match s.models.status(task).error {
        Some(e) => Err(bad(cmd, e)),
        None => Ok(()),
    }
}

fn select(s: &mut Session, p: &Value, task: Task) -> Result<Value> {
    let cmd = format!("{}.model.select", kind(task).prefix);
    let id = str_p(p, "id").ok_or_else(|| bad(&cmd, "missing `id`"))?;
    if id != seg::CLASSICAL {
        model_p(p, &cmd, task)?;
    }
    let wait = b_p(p, "wait").unwrap_or(false);
    s.choose_model(task, id);
    if wait {
        s.apply_model(task, true);
        waited(s, task, &cmd)?;
    }
    s.save_prefs();
    s.prefs_changed();
    Ok(json!({"model": id, "active": s.models.status(task).active}))
}

fn install(s: &mut Session, p: &Value, task: Task) -> Result<Value> {
    let cmd = format!("{}.model.install", kind(task).prefix);
    let path = str_p(p, "path").ok_or_else(|| bad(&cmd, "missing `path` (the downloaded weights file)"))?;
    let dir = s.models_dir().ok_or_else(|| bad(&cmd, "no settings folder to install into"))?;
    let bytes = std::fs::read(path).map_err(|e| EngineError::Other(format!("cannot read {path}: {e}")))?;
    // Which model it is: the one asked for, or the one whose checksum it has.
    let m = match str_p(p, "id") {
        Some(_) => model_p(p, &cmd, task)?,
        None => {
            let h = seg::sha256::hex(&bytes);
            seg::models(task).find(|m| m.sha256 == h).ok_or_else(|| bad(&cmd, "this file is not a published model in the registry (SHA-256 differs)"))?
        }
    };
    let dest = install_bytes(&dir, m, &bytes).map_err(|e| bad(&cmd, e))?;
    let wait = b_p(p, "wait").unwrap_or(false);
    s.apply_model(task, wait);
    if wait {
        waited(s, task, &cmd)?;
    }
    Ok(json!({"id": m.id, "path": dest.display().to_string()}))
}

fn download(s: &mut Session, p: &Value, task: Task) -> Result<Value> {
    let k = kind(task);
    let cmd = format!("{}.model.download", k.prefix);
    let m = model_p(p, &cmd, task)?;
    let dir = s.models_dir().ok_or_else(|| bad(&cmd, "no settings folder to install into"))?;
    if s.models.status(task).busy.is_some() {
        return Err(bad(&cmd, "a model is already downloading or loading"));
    }
    let size = if m.size >= 10_000_000 { format!("{} MB", m.size / 1_000_000) } else { format!("{:.1} MB", m.size as f64 / 1e6) };
    let wait = b_p(p, "wait").unwrap_or(false);
    spawn(s.models.lane(task), k.label, format!("Downloading {} ({size})…", m.name), wait, move |_| {
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let tmp = dir.join(format!("{}.part", m.file_name));
        let out = std::process::Command::new("curl")
            .args(["-fsSL", "--retry", "2", "--connect-timeout", "20", "-o"])
            .arg(&tmp)
            .arg(m.url)
            .output()
            .map_err(|e| format!("downloading needs curl ({e}); or download {} in a browser and use Install from File", m.url))?;
        if !out.status.success() {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("download failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        let bytes = std::fs::read(&tmp).map_err(|e| format!("cannot read the download: {e}"))?;
        let _ = std::fs::remove_file(&tmp);
        install_bytes(&dir, m, &bytes)?;
        Ok(format!("{}: {} downloaded and verified", k.label, m.name))
    });
    if wait {
        waited(s, task, &cmd)?;
        s.apply_model(task, true);
        waited(s, task, &cmd)?;
        return Ok(json!({"id": m.id, "installed": true}));
    }
    Ok(json!({"id": m.id, "downloading": true}))
}

fn remove(s: &mut Session, p: &Value, task: Task) -> Result<Value> {
    let m = model_p(p, &format!("{}.model.remove", kind(task).prefix), task)?;
    let Some(path) = s.model_path(m) else { return Ok(json!({"removed": false})) };
    if s.models.status(task).active.as_deref() == Some(m.id) {
        s.models.set_active(task, None);
    }
    std::fs::remove_file(&path).map_err(|e| EngineError::Other(format!("cannot remove {}: {e}", path.display())))?;
    if let Some(dir) = path.parent() {
        let _ = std::fs::remove_file(notice_path(dir, m));
    }
    Ok(json!({"removed": true}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        query!("roto.models", "Roto Brush Models", "{}", |s, _| list(s, Task::Mask)),
        cmd!(
            "roto.model.select",
            "Use Roto Brush Model",
            [],
            None,
            "{id: classical|mobilesam, wait?} (Roto Brush 2.0 / 3.0 use it; wait: loaded before returning)",
            always,
            |s, p| select(s, p, Task::Mask)
        ),
        cmd!(
            "roto.model.install",
            "Install Roto Brush Model",
            [],
            None,
            "{path: weights file, id?, wait?} — verified against the registry's SHA-256",
            always,
            |s, p| install(s, p, Task::Mask)
        ),
        cmd!(
            "roto.model.download",
            "Download Roto Brush Model",
            [],
            None,
            "{id, wait?} — the official weights, verified (desktop; uses the system curl)",
            always,
            |s, p| download(s, p, Task::Mask)
        ),
        cmd!("roto.model.remove", "Remove Roto Brush Model", [], None, "{id}", always, |s, p| remove(s, p, Task::Mask)),
        query!("face.models", "Face Tracking Models", "{}", |s, _| list(s, Task::Face)),
        cmd!(
            "face.model.select",
            "Use Face Tracking Model",
            [],
            None,
            "{id: classical|mediapipe-face, wait?} (face tracking uses it; wait: loaded before returning)",
            always,
            |s, p| select(s, p, Task::Face)
        ),
        cmd!(
            "face.model.install",
            "Install Face Tracking Model",
            [],
            None,
            "{path: model file, id?, wait?} — verified against the registry's SHA-256",
            always,
            |s, p| install(s, p, Task::Face)
        ),
        cmd!(
            "face.model.download",
            "Download Face Tracking Model",
            [],
            None,
            "{id, wait?} — the official model, verified (desktop; uses the system curl)",
            always,
            |s, p| download(s, p, Task::Face)
        ),
        cmd!("face.model.remove", "Remove Face Tracking Model", [], None, "{id}", always, |s, p| remove(s, p, Task::Face)),
    ]
}
