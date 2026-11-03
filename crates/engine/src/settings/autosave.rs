//! Auto-save and crash recovery (PRJ-7).
//!
//! - **Auto-save**: every *n* minutes a dirty project is written to an "EffectCraft Auto-Save"
//!   folder next to it (or a custom folder) as `<name> auto-save N.ecproj`, the After Effects
//!   naming. Slots rotate through 1…max versions, overwriting the oldest. Writes are atomic
//!   (temporary file + rename), so a crash mid-write never damages the previous auto-save.
//! - **Crash recovery**: while the app runs, a sentinel (`session.lock` in the config store)
//!   records the open project and its latest auto-save. A clean exit removes it; finding one at
//!   launch means the last session ended unexpectedly, and the frontend offers to open the
//!   latest auto-save.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{ConfigStore, FileOps, StdFiles};
use crate::prefs::Prefs;

/// Folder created next to a project for its auto-saves.
pub const AUTOSAVE_FOLDER: &str = "EffectCraft Auto-Save";
/// Crash-recovery sentinel in the config store.
pub const SENTINEL: &str = "session.lock";
const EXT: &str = "ecproj";

/// Auto-save bookkeeping of a session.
#[derive(Clone, Debug, Default)]
pub struct AutoSaveState {
    /// Serialise and write on a background thread (frontends with an event loop turn this on;
    /// ignored on wasm32). The UI thread then only picks the slot and snapshots the project
    /// (an `Arc` clone), so a large project's auto-save never stalls a frame.
    pub background: bool,
    /// The background write in flight, if any.
    pub pending: Option<PendingWrite>,
    /// Wall-clock seconds of the last auto-save (or of the first tick).
    pub last: Option<f64>,
    /// Slot written last in this session.
    pub last_slot: Option<u32>,
    /// Revision written last (an unchanged project isn't saved again).
    pub saved_revision: Option<u64>,
    /// Path of the latest auto-save.
    pub last_path: Option<String>,
    /// This session owns the crash-recovery sentinel ([`crate::Session::begin_recovery`]).
    pub sentinel: bool,
}

/// A background auto-save write ([`AutoSaveState::background`]).
#[derive(Clone)]
pub struct PendingWrite {
    pub path: PathBuf,
    pub(crate) slot: u32,
    pub(crate) revision: u64,
    /// Originating project identity: the path changes on Save As, the epoch on Open/New.
    pub(crate) project_path: Option<String>,
    pub(crate) project_epoch: u64,
    handle: std::sync::Arc<std::sync::Mutex<Option<std::thread::JoinHandle<std::io::Result<()>>>>>,
}

impl std::fmt::Debug for PendingWrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingWrite").field("path", &self.path).finish()
    }
}

impl PendingWrite {
    pub fn spawn(
        path: PathBuf,
        slot: u32,
        revision: u64,
        project_path: Option<String>,
        project_epoch: u64,
        work: impl FnOnce() -> std::io::Result<()> + Send + 'static,
    ) -> std::io::Result<PendingWrite> {
        let h = std::thread::Builder::new().name("autosave".into()).spawn(work)?;
        Ok(PendingWrite { path, slot, revision, project_path, project_epoch, handle: std::sync::Arc::new(std::sync::Mutex::new(Some(h))) })
    }
    pub fn is_finished(&self) -> bool {
        self.handle.lock().map(|h| h.as_ref().is_none_or(|h| h.is_finished())).unwrap_or(true)
    }
    /// Wait for the write; its result (`Ok` if already collected).
    pub fn join(&self) -> std::io::Result<()> {
        let h = self.handle.lock().ok().and_then(|mut h| h.take());
        match h {
            Some(h) => h.join().unwrap_or_else(|_| Err(std::io::Error::other("auto-save thread panicked"))),
            None => Ok(()),
        }
    }
}

/// File-name stem of a project (`/a/Intro.ecproj` → `Intro`; untitled → `Untitled Project`).
pub fn project_stem(path: Option<&str>) -> String {
    path.and_then(|p| Path::new(p).file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled Project".into())
}

/// `<stem> auto-save N.ecproj`.
pub fn file_name(stem: &str, n: u32) -> String {
    format!("{stem} auto-save {n}.{EXT}")
}

/// The slot number of an auto-save file name for `stem`.
pub fn slot_of(stem: &str, name: &str) -> Option<u32> {
    let rest = name.strip_prefix(stem)?.strip_prefix(" auto-save ")?;
    rest.strip_suffix(&format!(".{EXT}"))?.parse().ok()
}

/// Where a project's auto-saves go: next to the project, or the custom folder; untitled projects
/// use the custom folder or `fallback` (the app's own Auto-Save folder).
pub fn folder(prefs: &Prefs, project: Option<&str>, fallback: Option<&Path>) -> Option<PathBuf> {
    let a = &prefs.auto_save;
    if a.location == "custom" && !a.folder.trim().is_empty() {
        return Some(PathBuf::from(a.folder.trim()));
    }
    match project.and_then(|p| Path::new(p).parent()) {
        Some(dir) => Some(dir.join(AUTOSAVE_FOLDER)),
        None => fallback.map(|f| f.join(AUTOSAVE_FOLDER)),
    }
}

/// Existing auto-saves of `stem` in `dir`: (slot, path, modified).
pub fn existing(dir: &Path, stem: &str) -> Vec<(u32, PathBuf, std::time::SystemTime)> {
    existing_in(&StdFiles, dir, stem)
}

/// [`existing`] through `fs`.
pub fn existing_in(fs: &dyn FileOps, dir: &Path, stem: &str) -> Vec<(u32, PathBuf, std::time::SystemTime)> {
    let mut v: Vec<_> = fs.list(dir).into_iter().filter_map(|(name, m)| Some((slot_of(stem, &name)?, dir.join(&name), m))).collect();
    v.sort_by_key(|x| x.0);
    v
}

/// The slot to write next: after `last` (this session), else after the newest file on disk,
/// wrapping at `max`.
pub fn next_slot(existing: &[(u32, PathBuf, std::time::SystemTime)], last: Option<u32>, max: u32) -> u32 {
    let max = max.max(1);
    let after = last.or_else(|| existing.iter().filter(|e| e.0 <= max).max_by_key(|e| (e.2, e.0)).map(|e| e.0));
    match after {
        Some(n) => n % max + 1,
        None => 1,
    }
}

/// Write one auto-save of `json` for the project at `project` and rotate. Returns its path.
pub fn write(prefs: &Prefs, project: Option<&str>, fallback: Option<&Path>, last_slot: Option<u32>, json: &str) -> std::io::Result<(PathBuf, u32)> {
    write_in(&StdFiles, prefs, project, fallback, last_slot, json)
}

/// [`write()`] through `fs`.
pub fn write_in(
    fs: &dyn FileOps,
    prefs: &Prefs,
    project: Option<&str>,
    fallback: Option<&Path>,
    last_slot: Option<u32>,
    json: &str,
) -> std::io::Result<(PathBuf, u32)> {
    let plan = plan_in(fs, prefs, project, fallback, last_slot)?;
    plan.write(fs, json.as_bytes())?;
    Ok((plan.path, plan.slot))
}

/// Where the next auto-save goes ([`plan_in`]).
#[derive(Clone, Debug)]
pub struct WritePlan {
    pub path: PathBuf,
    pub slot: u32,
    /// Slots above the (lowered) maximum, removed after the write.
    pub stale: Vec<PathBuf>,
}

impl WritePlan {
    pub fn write(&self, fs: &dyn FileOps, data: &[u8]) -> std::io::Result<()> {
        fs.write(&self.path, data)?;
        for p in &self.stale {
            let _ = fs.remove(p);
        }
        Ok(())
    }
}

/// Pick the next auto-save slot (without writing anything).
pub fn plan_in(fs: &dyn FileOps, prefs: &Prefs, project: Option<&str>, fallback: Option<&Path>, last_slot: Option<u32>) -> std::io::Result<WritePlan> {
    let dir = folder(prefs, project, fallback).ok_or_else(|| std::io::Error::other("no auto-save folder for an untitled project"))?;
    let stem = project_stem(project);
    let max = prefs.auto_save.max_versions.max(1);
    let have = existing_in(fs, &dir, &stem);
    let slot = next_slot(&have, last_slot, max);
    let path = dir.join(file_name(&stem, slot));
    // Max versions lowered: drop slots above it.
    let stale = have.into_iter().filter(|(n, _, _)| *n > max).map(|(_, p, _)| p).collect();
    Ok(WritePlan { path, slot, stale })
}

/// The newest auto-save of a project, if any.
pub fn latest(prefs: &Prefs, project: Option<&str>, fallback: Option<&Path>) -> Option<PathBuf> {
    latest_in(&StdFiles, prefs, project, fallback)
}

/// [`latest`] through `fs`.
pub fn latest_in(fs: &dyn FileOps, prefs: &Prefs, project: Option<&str>, fallback: Option<&Path>) -> Option<PathBuf> {
    let dir = folder(prefs, project, fallback)?;
    existing_in(fs, &dir, &project_stem(project)).into_iter().max_by_key(|e| e.2).map(|e| e.1)
}

/// What the crash-recovery sentinel records.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Sentinel {
    pub pid: u32,
    /// The open project (None = untitled).
    pub project: Option<String>,
    /// Latest auto-save of this run.
    pub autosave: Option<String>,
    /// Whether the project had unsaved changes.
    pub dirty: bool,
}

/// A previous run that didn't exit cleanly.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recovery {
    pub project: Option<String>,
    /// The auto-save to offer (exists on disk).
    pub autosave: Option<String>,
    pub dirty: bool,
}

/// At launch: report an unclean previous exit (a leftover sentinel), then start this run's
/// sentinel.
pub fn begin(store: &dyn ConfigStore, prefs: &Prefs) -> Option<Recovery> {
    let found = store.read(SENTINEL).map(|t| serde_json::from_str::<Sentinel>(&t).unwrap_or_default());
    let _ = store.write(SENTINEL, &serde_json::to_string(&Sentinel { pid: pid(), ..Default::default() }).unwrap_or_default());
    let s = found?;
    let fallback = store.dir();
    let autosave = s
        .autosave
        .clone()
        .filter(|p| store.files().is_file(Path::new(p)))
        .or_else(|| latest_in(store.files(), prefs, s.project.as_deref(), fallback.as_deref()).map(|p| p.to_string_lossy().to_string()));
    Some(Recovery { project: s.project, autosave, dirty: s.dirty })
}

/// This process id (0 where processes don't exist, e.g. the browser).
pub fn pid() -> u32 {
    #[cfg(not(target_arch = "wasm32"))]
    return std::process::id();
    #[cfg(target_arch = "wasm32")]
    0
}

/// Record the current project / latest auto-save in the sentinel.
pub fn update(store: &dyn ConfigStore, s: &Sentinel) {
    let _ = store.write(SENTINEL, &serde_json::to_string(s).unwrap_or_default());
}

/// Clean exit: remove the sentinel.
pub fn end(store: &dyn ConfigStore) {
    let _ = store.remove(SENTINEL);
}

/// After Effects' Increment and Save name: `Intro.ecproj` → `Intro 2.ecproj`, `Intro 2.ecproj`
/// → `Intro 3.ecproj` (skipping names that exist; the extension, `.ecprojx` too, is kept).
pub fn increment_path(path: &str, exists: impl Fn(&str) -> bool) -> String {
    let p = Path::new(path);
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = p.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_else(|| EXT.to_string());
    let (base, mut n) = match stem.rsplit_once(' ') {
        Some((b, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => (b.to_string(), n.parse::<u32>().unwrap_or(1) + 1),
        _ => (stem.clone(), 2),
    };
    loop {
        let cand = p.with_file_name(format!("{base} {n}.{ext}")).to_string_lossy().to_string();
        if !exists(&cand) {
            return cand;
        }
        n += 1;
    }
}
