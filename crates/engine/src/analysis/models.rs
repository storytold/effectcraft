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
use std::time::Duration;

use effectcraft_segment::face::FaceModel;
use effectcraft_segment::{self as seg, Loaded, ModelInfo, Task};
use serde_json::{Value, json};
use web_time::Instant;

use crate::commands::{CommandSpec, always, b_p, bad, str_p};
use crate::{EngineError, Result, Session, cmd, query};

/// What a task's background worker reports.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// What is happening now ("Downloading MobileSAM…").
    pub busy: Option<String>,
    /// Model whose load/download owns this lane, for selection-specific waits.
    pub busy_model: Option<&'static str>,
    /// The model in use.
    pub active: Option<String>,
    /// The last failure, for the Settings page.
    pub error: Option<String>,
    /// Model whose operation failed, so Roto status does not report another model's download.
    pub error_model: Option<&'static str>,
    /// Failure of the selected model's load; downloads cannot block or retry that load.
    pub load_error: Option<String>,
    /// A finished job's message and whether it failed, for a toast (taken by
    /// [`Session::poll_models`]).
    pub done: Option<(String, bool)>,
    /// Counts selection and selected-weight changes: an older load is stale and its model is discarded.
    pub generation: u64,
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

    /// Activate a model that finished loading for the choice counted as `generation`; `false` (and the model is
    /// dropped) when the choice changed meanwhile.
    #[cfg(test)]
    pub(crate) fn set_active_if_current(&self, task: Task, model: Loaded, generation: u64) -> bool {
        activate_if_current(self.lane(task), &self.face_model, task, model, generation)
    }

    pub(crate) fn set_active(&self, task: Task, model: Option<Loaded>) {
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

/// [`activate`] a freshly loaded model unless the choice changed since the load started (`generation`); whether it
/// was activated. The check and the activation happen under the lane's lock, so a selection can't slip between them.
fn activate_if_current(lane: &Arc<Mutex<Status>>, face: &FaceSlot, task: Task, model: Loaded, generation: u64) -> bool {
    let Ok(mut s) = lane.lock() else { return false };
    if s.generation != generation {
        return false;
    }
    let id = match &model {
        Loaded::Mask(m) => m.info().id.to_string(),
        Loaded::Face(m) => m.info().id.to_string(),
    };
    match (task, model) {
        (Task::Mask, Loaded::Mask(m)) => crate::effects::roto::set_model(Some(m)),
        (Task::Face, Loaded::Face(f)) => {
            if let Ok(mut slot) = face.lock() {
                *slot = Some(f);
            }
        }
        _ => return false,
    }
    s.active = Some(id);
    true
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

/// How long a Roto Brush model may sit unused before its memory is freed.
const ROTO_MODEL_IDLE: Duration = Duration::from_secs(600);

/// How long a job that needs the Roto Brush model waits for it to load (or download) before it
/// gives up. A worker that never reports back must not hang the command.
const ROTO_MODEL_WAIT: Duration = Duration::from_secs(600);

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
        let changed = slot != id;
        if changed {
            *slot = id.to_string();
        }
        set(self.models.lane(task), |s| {
            // A load still running for the previous choice must not activate or report an error.
            if changed {
                s.generation = s.generation.wrapping_add(1);
            }
            // Selecting the same model explicitly retries a failed load.
            s.error = None;
            s.error_model = None;
            s.load_error = None;
        });
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
        if st.active.as_deref() == Some(info.id) || st.busy.is_some() || st.load_error.is_some() {
            return;
        }
        let Some(path) = self.model_path(info) else {
            self.models.set_active(task, None);
            return;
        };
        let (face_slot, label) = (self.models.face_model.clone(), kind(task).label);
        let generation = st.generation;
        spawn(&lane, label, format!("Loading {}…", info.name), wait, Some(generation), info.id, move |status| {
            let m = load_file(&path, info.id)?;
            // the choice changed while this loaded: drop the model, `poll_models` loads the new choice
            if !activate_if_current(status, &face_slot, task, m, generation) {
                return Ok(String::new());
            }
            Ok(format!("{label}: {} loaded", info.name))
        });
    }

    /// The Roto Brush model in use (`None`: the classic engine).
    pub fn models_roto_active(&self) -> Option<String> {
        self.models.status(Task::Mask).active
    }

    /// Only installing the chosen model is a retry of that model's failed load.
    fn retry_installed_model(&mut self, task: Task, id: &str, wait: bool) -> std::result::Result<bool, String> {
        if self.chosen_model(task) != id {
            return Ok(false);
        }
        set(self.models.lane(task), |s| {
            s.error = None;
            s.error_model = None;
            s.load_error = None;
            // A load that read the old file must not report failure after these weights replaced it.
            s.generation = s.generation.wrapping_add(1);
        });
        if self.models.status(task).active.as_deref() == Some(id) {
            return Ok(true);
        }
        if wait {
            self.wait_model_worker(task, id)?;
        }
        self.apply_model(task, wait);
        if wait {
            let status = self.models.status(task);
            if status.active.as_deref() != Some(id) {
                return Err(status.load_error.unwrap_or_else(|| format!("{id} is not loaded: try again when the model worker is ready")));
            }
        }
        Ok(true)
    }

    fn wait_model_worker(&self, task: Task, id: &str) -> std::result::Result<(), String> {
        let started = Instant::now();
        loop {
            let status = self.models.status(task);
            let Some(operation) = status.busy else { return Ok(()) };
            if status.busy_model != Some(id) {
                return Err(format!("{id} cannot load while another model operation is running ({operation}): try again when it is ready"));
            }
            if started.elapsed() >= ROTO_MODEL_WAIT {
                return Err(format!("{} is still loading after {} minutes", self.chosen_model(task), ROTO_MODEL_WAIT.as_secs() / 60));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The Roto Brush model Settings chose, when it is one (not the classic engine).
    fn roto_model_info(&self) -> Option<&'static ModelInfo> {
        seg::info(self.chosen_model(Task::Mask)).filter(|m| m.task == Task::Mask)
    }

    /// Whether the chosen Roto Brush model is still loading or downloading.
    pub fn roto_model_loading(&self) -> bool {
        let status = self.models.status(Task::Mask);
        let chosen = self.chosen_model(Task::Mask);
        status.active.as_deref() != Some(chosen) && status.busy.is_some() && status.busy_model == Some(chosen)
    }

    /// The Roto Brush model Settings chose and its state (`roto.status`).
    pub fn roto_model(&self) -> Value {
        let st = self.models.status(Task::Mask);
        json!({
            "selected": self.chosen_model(Task::Mask),
            "installed": self.roto_model_info().is_none_or(|m| self.model_path(m).is_some()),
            "active": st.active,
            "loading": self.roto_model_loading(),
            "error": st.load_error.or(st.error.filter(|_| st.error_model == Some(self.chosen_model(Task::Mask)))),
        })
    }

    /// What `roto.status` says when the chosen model is not the one in use for this layer (`None`:
    /// nothing to say). `uses_model`: the layer segments with a model at all (Roto Brush 1.0 does not).
    pub fn roto_model_notice(&self, uses_model: bool) -> Option<String> {
        let info = self.roto_model_info()?;
        if !uses_model {
            return Some(format!("{} is selected, but this layer is Roto Brush 1.0: it segments with the classic engine", info.name));
        }
        let st = self.models.status(Task::Mask);
        if st.active.as_deref() == Some(info.id) {
            None
        } else if self.model_path(info).is_none() {
            Some(format!("{} is selected but not installed: propagate refuses until it is installed or Classic is chosen", info.name))
        } else if let Some(e) = st.load_error {
            Some(format!("{} is selected but failed to load ({e}): propagate refuses until it is selected again", info.name))
        } else {
            Some(format!("{} is selected but not loaded yet: propagate loads it first with wait: true, and refuses without it", info.name))
        }
    }

    /// What `roto.status` says when the saved tracking applies but Settings choose another model (`None`:
    /// they agree, or nothing is saved).
    pub fn saved_tracking_notice(&self, saved: Option<&str>) -> Option<String> {
        let saved = saved?;
        let chosen = self.chosen_model(Task::Mask);
        if saved == chosen {
            return None;
        }
        let name = |id: &str| seg::info(id).map_or(ROTO.classic_name, |m| m.name);
        Some(format!("The saved tracking was made with {}; {} is chosen: Propagate again to replace it", name(saved), name(chosen)))
    }

    /// Make sure the Roto Brush model Settings chose is in use before a job that segments with it
    /// starts, so the job never runs on the classic engine without saying so. With `wait`, a load
    /// under way is waited for and one not started is loaded here; without it, a model that is not
    /// ready is refused (and starts loading). A model that is not installed, or failed to load, is
    /// refused either way. The classic engine needs nothing.
    pub fn ensure_roto_model(&mut self, wait: bool) -> std::result::Result<(), String> {
        let Some(info) = self.roto_model_info() else { return Ok(()) };
        if self.models.status(Task::Mask).active.as_deref() == Some(info.id) {
            return Ok(());
        }
        // A download or load under way (from Settings, or `select` without `wait`) finishes first.
        // Nothing is in flight in the browser build, so this never sleeps there.
        if wait {
            self.wait_model_worker(Task::Mask, info.id)?;
        }
        if self.model_path(info).is_none() {
            if self.models.status(Task::Mask).busy.is_some() {
                return Err(format!("{} is still downloading: try again when it is ready, or pass wait: true", info.name));
            }
            return Err(format!("{} is selected for Roto Brush but not installed: install or download it, or choose {}", info.name, ROTO.classic_name));
        }
        let st = self.models.status(Task::Mask);
        if st.active.as_deref() != Some(info.id) && st.busy.is_none() && st.load_error.is_none() {
            self.apply_model(Task::Mask, wait);
        }
        let st = self.models.status(Task::Mask);
        if st.active.as_deref() == Some(info.id) {
            return Ok(());
        }
        if let Some(e) = st.load_error {
            return Err(format!("{} failed to load: {e}", info.name));
        }
        Err(format!("{} is still loading: try again when it is ready, or pass wait: true", info.name))
    }

    /// Free a Roto Brush model that has been idle for [`ROTO_MODEL_IDLE`] (and is not under a job). The
    /// choice stays: it loads again when a job or a stroke needs it.
    fn unload_idle_roto_model(&mut self) {
        if self.is_roto_running() || self.models.status(Task::Mask).busy.is_some() {
            return;
        }
        let Some(m) = crate::effects::roto::installed_model() else { return };
        if m.idle_secs().is_some_and(|s| s >= ROTO_MODEL_IDLE.as_secs_f64()) {
            let name = m.info().name;
            drop(m);
            self.models.set_active(Task::Mask, None);
            self.events.push(crate::Event::Toast {
                message: format!("{name} unloaded after {} minutes idle; it loads again when it is used", ROTO_MODEL_IDLE.as_secs() / 60),
                error: false,
            });
        }
    }

    /// Start loading the chosen Roto Brush model again if it was unloaded for being idle (before a stroke uses it).
    /// The load runs on the model worker, so the stroke is never blocked on it (the classic engine segments it
    /// meanwhile and `poll_models` installs the model when it arrives), and a model that already failed to load is
    /// not retried on every stroke: choosing it again clears the error.
    pub fn reload_roto_model(&mut self) {
        let chosen = self.chosen_model(Task::Mask).to_string();
        let st = self.models.status(Task::Mask);
        if chosen != seg::CLASSICAL && st.active.is_none() && st.busy.is_none() && st.load_error.is_none() {
            self.apply_model(Task::Mask, false);
        }
    }

    /// Finish what the model workers did: a toast, and loading a model that just arrived.
    pub fn poll_models(&mut self) {
        self.unload_idle_roto_model();
        for task in [Task::Mask, Task::Face] {
            let done = self.models.lane(task).lock().ok().and_then(|mut s| s.done.take());
            if let Some((message, error)) = done {
                // a discarded (stale) load has no message
                if !message.is_empty() {
                    self.events.push(crate::Event::Toast { message, error });
                }
                self.apply_model(task, false);
            }
        }
    }
}

fn load_file(path: &Path, id: &str) -> std::result::Result<Loaded, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    // SAM 2 runs on candle and is native only; every other model loads through the registry.
    #[cfg(not(target_arch = "wasm32"))]
    if effectcraft_sam2::mask::SIZES.iter().any(|m| m.id == id) {
        return effectcraft_sam2::mask::load(&bytes).map(|m| Loaded::Mask(m));
    }
    seg::load(id, &bytes)
}

/// Run `job` on a worker thread (`wait`: right here), reporting through `status` (desktop; the
/// browser build has no trained models yet).
fn spawn(
    status: &Arc<Mutex<Status>>,
    label: &'static str,
    what: String,
    wait: bool,
    generation: Option<u64>,
    model: &'static str,
    job: impl FnOnce(&Arc<Mutex<Status>>) -> std::result::Result<String, String> + Send + 'static,
) {
    if cfg!(target_arch = "wasm32") {
        set(status, |s| {
            let message = "trained models are available in the desktop app".to_string();
            if generation.is_some() {
                s.load_error = Some(message.clone());
            }
            s.error = Some(message);
            s.error_model = Some(model);
        });
        return;
    }
    set(status, |s| {
        s.busy = Some(what);
        s.busy_model = Some(model);
        s.error = None;
        s.error_model = None;
    });
    let st = status.clone();
    let run = move || {
        let r = job(&st);
        set(&st, |s| {
            s.busy = None;
            s.busy_model = None;
            if generation.is_some_and(|g| g != s.generation) {
                // Keep a completion marker so polling starts the newly selected model.
                s.done = Some((String::new(), false));
                return;
            }
            match r {
                Ok(msg) => s.done = Some((msg, false)),
                Err(e) => {
                    s.done = Some((format!("{label} model: {e}"), true));
                    if generation.is_some() {
                        s.load_error = Some(e.clone());
                    }
                    s.error = Some(e);
                    s.error_model = Some(model);
                }
            }
        });
    };
    if wait {
        run();
    } else if let Err(e) = std::thread::Builder::new().name("ec-model".into()).spawn(run) {
        set(status, |s| {
            s.busy = None;
            s.busy_model = None;
            let message = format!("cannot start: {e}");
            if generation.is_some() {
                s.load_error = Some(message.clone());
            }
            s.error = Some(message);
            s.error_model = Some(model);
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

fn models_for_platform(task: Task, wasm: bool) -> impl Iterator<Item = &'static ModelInfo> {
    seg::models(task).filter(move |m| !wasm || !m.id.starts_with("sam2.1-hiera-"))
}

fn target_models(task: Task) -> impl Iterator<Item = &'static ModelInfo> {
    models_for_platform(task, cfg!(target_arch = "wasm32"))
}

fn model_p(p: &Value, cmd: &str, task: Task) -> Result<&'static ModelInfo> {
    let id = str_p(p, "id").ok_or_else(|| bad(cmd, "missing `id`"))?;
    target_models(task).find(|m| m.id == id).ok_or_else(|| bad(cmd, format!("unknown model `{id}` (see {}.models)", kind(task).prefix)))
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
    for m in target_models(task) {
        #[cfg(not(target_arch = "wasm32"))]
        let (recommended, hint) = (effectcraft_sam2::mask::recommended().id == m.id, effectcraft_sam2::mask::hint(m.id));
        #[cfg(target_arch = "wasm32")]
        let (recommended, hint) = (false, None::<String>);
        v.push(json!({
            "recommended": recommended,
            "hint": hint,
            "id": m.id,
            "name": m.name,
            "description": m.description,
            "authors": m.authors,
            "licence": m.licence,
            "licenceUrl": m.licence_url,
            "homepage": m.homepage,
            "url": m.url,
            "source": m.source,
            "sourceFile": m.source.rsplit('/').next().unwrap_or(m.file_name),
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
    if wait && id != seg::CLASSICAL && s.models.status(task).active.as_deref() != Some(id) {
        s.wait_model_worker(task, id).map_err(|error| bad(&cmd, error))?;
    }
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
            target_models(task)
                .find(|m| m.sha256 == h)
                .ok_or_else(|| bad(&cmd, "this file is not a supported published model in the registry (SHA-256 differs)"))?
        }
    };
    let dest = install_bytes(&dir, m, &bytes).map_err(|e| bad(&cmd, e))?;
    let wait = b_p(p, "wait").unwrap_or(false);
    s.retry_installed_model(task, m.id, wait).map_err(|error| bad(&cmd, error))?;
    Ok(json!({"id": m.id, "path": dest.display().to_string()}))
}

fn download(s: &mut Session, p: &Value, task: Task) -> Result<Value> {
    let k = kind(task);
    let cmd = format!("{}.model.download", k.prefix);
    let m = model_p(p, &cmd, task)?;
    if m.url.is_empty() {
        return Err(bad(&cmd, format!("{} has no app download available: use Install from File with the SHA-256-verified weights", m.name)));
    }
    let dir = s.models_dir().ok_or_else(|| bad(&cmd, "no settings folder to install into"))?;
    if s.models.status(task).busy.is_some() {
        return Err(bad(&cmd, "a model is already downloading or loading"));
    }
    let size = if m.size >= 10_000_000 { format!("{} MB", m.size / 1_000_000) } else { format!("{:.1} MB", m.size as f64 / 1e6) };
    let wait = b_p(p, "wait").unwrap_or(false);
    spawn(s.models.lane(task), k.label, format!("Downloading {} ({size})…", m.name), wait, None, m.id, move |_| {
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
            "{id: classical|mobilesam|sam2.1-hiera-tiny|sam2.1-hiera-small|sam2.1-hiera-base-plus|sam2.1-hiera-large, wait?} (Roto Brush 2.0 / 3.0 use it; wait: loaded before returning)",
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

#[cfg(test)]
mod availability_tests {
    use super::*;

    /// SAM 2 cannot start a network operation; file installation still verifies the published hash.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn sam2_is_install_from_file_only() {
        let mut session = Session::default();
        let listed = session.execute("roto.models", json!({})).unwrap();
        for model in [seg::SAM2_TINY, seg::SAM2_SMALL, seg::SAM2_BASE_PLUS, seg::SAM2_LARGE] {
            let error = session.execute("roto.model.download", json!({"id": model.id})).unwrap_err();
            assert!(error.to_string().contains("Install from File"));
            assert!(session.models.status(Task::Mask).busy.is_none());
            assert!(install_bytes(Path::new("unused"), &model, b"invalid weights").unwrap_err().contains("SHA-256 differs"));
            let row = listed["models"].as_array().unwrap().iter().find(|row| row["id"] == model.id).unwrap();
            assert_eq!(row["source"], model.source);
            assert_eq!(row["sourceFile"], "model.safetensors");
        }
    }

    /// A waiting selection cannot claim completion while unrelated work prevents its model loading.
    #[test]
    fn waiting_selection_refuses_an_unrelated_worker_unless_the_model_is_active() {
        crate::tests::roto::hold_roto_cache_test_lock();
        for active in [false, true] {
            let mut session = Session::default();
            session.choose_model(Task::Mask, if active { "mobilesam" } else { seg::CLASSICAL });
            set(session.models.lane(Task::Mask), |status| {
                status.active = active.then(|| "mobilesam".into());
                status.busy = Some("Downloading SAM 2".into());
                status.busy_model = Some("sam2.1-hiera-tiny");
            });
            let result = session.execute("roto.model.select", json!({"id": "mobilesam", "wait": true}));
            if active {
                assert_eq!(result.unwrap()["active"], "mobilesam");
            } else {
                assert!(result.unwrap_err().to_string().contains("another model operation"));
                assert_eq!(session.chosen_model(Task::Mask), seg::CLASSICAL, "a refused selection must not change settings");
            }
            assert!(session.models.status(Task::Mask).busy.is_some());
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stale_model_worker_completions_do_not_poison_the_current_choice() {
        for failure in [false, true] {
            let lane = Arc::new(Mutex::new(Status::default()));
            spawn(&lane, "Roto Brush", "Loading old choice".into(), true, Some(0), "mobilesam", move |status| {
                set(status, |s| s.generation += 1);
                if failure { Err("old choice failed".into()) } else { Ok("old choice loaded".into()) }
            });
            let status = lane.lock().unwrap();
            assert!(status.busy.is_none());
            assert!(status.error.is_none(), "an obsolete worker must not block the current model");
            assert!(status.load_error.is_none());
            assert_eq!(status.done, Some((String::new(), false)), "polling must still retry the current choice");
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn current_load_and_download_failures_remain_actionable() {
        for generation in [Some(0), None] {
            let lane = Arc::new(Mutex::new(Status::default()));
            spawn(&lane, "Roto Brush", "Working".into(), true, generation, "mobilesam", move |status| {
                if generation.is_none() {
                    set(status, |s| s.generation += 1);
                }
                Err("current operation failed".into())
            });
            let status = lane.lock().unwrap();
            assert!(status.busy.is_none());
            assert_eq!(status.error.as_deref(), Some("current operation failed"));
            assert_eq!(status.load_error.as_deref(), generation.map(|_| "current operation failed"));
            assert_eq!(status.done, Some(("Roto Brush model: current operation failed".into(), true)));
        }
    }

    #[test]
    fn choosing_a_model_clears_its_previous_load_error_even_on_retry() {
        let mut session = Session::default();
        for id in ["mobilesam", "mobilesam", seg::CLASSICAL] {
            let previous = session.chosen_model(Task::Mask).to_string();
            let generation = session.models.status(Task::Mask).generation;
            set(session.models.lane(Task::Mask), |s| {
                s.error = Some("previous load failed".into());
                s.load_error = s.error.clone();
            });
            session.choose_model(Task::Mask, id);
            let status = session.models.status(Task::Mask);
            assert!(status.error.is_none());
            assert!(status.load_error.is_none());
            assert_eq!(status.generation, generation + u64::from(previous != id));
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn selected_active_model_is_ready_despite_an_unrelated_download_error() {
        let dir = std::env::temp_dir().join(format!("ec-model-ready-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let info = seg::info("mobilesam").unwrap();
        let file = std::fs::File::create(dir.join(info.file_name)).unwrap();
        file.set_len(info.size).unwrap();
        drop(file);
        let mut session = Session { models_dir: Some(dir.clone()), ..Default::default() };
        session.choose_model(Task::Mask, info.id);
        set(session.models.lane(Task::Mask), |s| {
            s.active = Some(info.id.into());
            s.error = Some("another model could not be downloaded".into());
        });
        assert_eq!(session.ensure_roto_model(false), Ok(()));
        assert!(session.roto_model()["error"].is_null());
        set(session.models.lane(Task::Mask), |status| {
            status.error_model = Some("mobilesam");
            status.error = Some("selected model download failed".into());
        });
        assert_eq!(session.roto_model()["error"], "selected model download failed");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_active_model_and_selected_install_do_not_wait_for_other_downloads() {
        crate::tests::roto::hold_roto_cache_test_lock();
        let mut session = Session::default();
        session.choose_model(Task::Mask, "mobilesam");
        set(session.models.lane(Task::Mask), |status| {
            status.active = Some("mobilesam".into());
            status.busy = Some("Downloading SAM 2".into());
            status.busy_model = Some("sam2.1-hiera-tiny");
        });
        assert_eq!(session.ensure_roto_model(true), Ok(()));
        assert_eq!(session.retry_installed_model(Task::Mask, "mobilesam", true), Ok(true));
        let status = session.models.status(Task::Mask);
        assert!(status.busy.is_some(), "the unrelated operation must still be running");
        assert!(!session.roto_model_loading());
        assert_eq!(session.roto_model()["loading"], false);
    }

    #[test]
    fn waiting_on_an_inactive_model_refuses_unrelated_work_without_waiting() {
        crate::tests::roto::hold_roto_cache_test_lock();
        let mut session = Session::default();
        session.choose_model(Task::Mask, "mobilesam");
        set(session.models.lane(Task::Mask), |status| {
            status.busy = Some("Downloading SAM 2".into());
            status.busy_model = Some("sam2.1-hiera-tiny");
        });
        assert!(session.ensure_roto_model(true).unwrap_err().contains("another model operation"));
        assert!(session.retry_installed_model(Task::Mask, "mobilesam", true).unwrap_err().contains("another model operation"));
        assert!(session.models.status(Task::Mask).busy.is_some());
        assert!(!session.roto_model_loading());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn downloads_do_not_block_an_inactive_model_or_retry_a_failed_load() {
        crate::tests::roto::hold_roto_cache_test_lock();
        let dir = std::env::temp_dir().join(format!("ec-model-download-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let info = seg::info("mobilesam").unwrap();
        std::fs::File::create(dir.join(info.file_name)).unwrap().set_len(info.size).unwrap();
        let mut session = Session { models_dir: Some(dir.clone()), ..Default::default() };
        session.choose_model(Task::Mask, info.id);
        let lane = session.models.lane(Task::Mask).clone();
        spawn(&lane, "Roto Brush", "Downloading another model".into(), true, None, "sam2.1-hiera-tiny", |_| Err("download failed".into()));
        assert_eq!(session.models.status(Task::Mask).error.as_deref(), Some("download failed"));
        assert!(session.models.status(Task::Mask).load_error.is_none());
        let error = session.ensure_roto_model(true).unwrap_err();
        assert!(!error.contains("download failed"), "the installed model must be loaded despite a download failure");
        let failure = session.models.status(Task::Mask).load_error;
        assert!(failure.is_some(), "the synthetic weights were actually loaded and rejected");
        for download_failed in [true, false] {
            spawn(&lane, "Roto Brush", "Downloading another model".into(), true, None, "sam2.1-hiera-tiny", move |_| {
                if download_failed { Err("download failed".into()) } else { Ok("downloaded".into()) }
            });
            session.poll_models();
            session.reload_roto_model();
            let status = session.models.status(Task::Mask);
            assert_eq!(status.load_error, failure, "a download must not clear the selected model's load failure");
            assert!(status.busy.is_none(), "polling and strokes must not retry a failed model");
            assert!(session.ensure_roto_model(true).unwrap_err().contains(failure.as_deref().unwrap()));
            assert!(session.roto_model_notice(true).unwrap().contains(failure.as_deref().unwrap()));
        }
        crate::effects::roto::set_model(None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn installing_an_unselected_model_keeps_the_selected_models_load_failure() {
        crate::tests::roto::hold_roto_cache_test_lock();
        let mut session = Session::default();
        session.choose_model(Task::Mask, "mobilesam");
        set(session.models.lane(Task::Mask), |s| {
            s.error = Some("selected model failed".into());
            s.load_error = s.error.clone();
        });
        let before = session.models.status(Task::Mask);
        for wait in [false, true] {
            assert!(!session.retry_installed_model(Task::Mask, "sam2.1-hiera-tiny", wait).unwrap());
            assert_eq!(session.models.status(Task::Mask), before);
        }
        // Installing the selected weights is an explicit retry, even if the installed path
        // has since gone away (no load is started from this session's missing model folder).
        assert!(session.retry_installed_model(Task::Mask, "mobilesam", false).unwrap());
        assert!(session.models.status(Task::Mask).load_error.is_none());
        assert!(session.models.status(Task::Mask).error.is_none());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn installing_selected_weights_with_wait_finishes_the_replacement_load() {
        crate::tests::roto::hold_roto_cache_test_lock();
        let dir = std::env::temp_dir().join(format!("ec-model-install-running-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let info = seg::info("mobilesam").unwrap();
        std::fs::File::create(dir.join(info.file_name)).unwrap().set_len(info.size).unwrap();
        let mut session = Session { models_dir: Some(dir.clone()), ..Default::default() };
        session.choose_model(Task::Mask, info.id);
        let generation = session.models.status(Task::Mask).generation;
        let lane = session.models.lane(Task::Mask).clone();
        spawn(&lane, "Roto Brush", "Loading the previous file".into(), false, Some(generation), info.id, move |status| {
            let started = Instant::now();
            while status.lock().unwrap().generation == generation {
                if started.elapsed() > Duration::from_secs(3) {
                    return Err("test did not start the replacement".into());
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Err("previous file failed".into())
        });
        assert!(session.retry_installed_model(Task::Mask, info.id, true).unwrap_err().contains("SHA-256"));
        let status = session.models.status(Task::Mask);
        assert!(status.busy.is_none());
        assert!(
            status.load_error.as_ref().is_some_and(|error| error.contains("SHA-256")),
            "wait must finish loading the replacement weights and report their actual validation error"
        );
        assert_ne!(status.error.as_deref(), Some("previous file failed"));
        crate::effects::roto::set_model(None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn browser_models_exclude_native_trackers_and_keep_portable_models() {
        let browser: Vec<_> = models_for_platform(Task::Mask, true).map(|m| m.id).collect();
        assert_eq!(browser, ["mobilesam"]);
        let desktop: Vec<_> = models_for_platform(Task::Mask, false).map(|m| m.id).collect();
        assert_eq!(desktop, ["mobilesam", "sam2.1-hiera-tiny", "sam2.1-hiera-small", "sam2.1-hiera-base-plus", "sam2.1-hiera-large"]);
        for wasm in [false, true] {
            assert_eq!(models_for_platform(Task::Face, wasm).map(|m| m.id).collect::<Vec<_>>(), ["mediapipe-face"]);
        }
    }
}
