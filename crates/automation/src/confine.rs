//! File access confinement for automation (`effectcraft-cli mcp --automation-read-root DIR
//! --automation-write-root DIR`, as in PhotoCraft): an agent may only read files under a read
//! root (or a write root) and only write under a write root.
//!
//! Every engine command an agent runs ([`crate::Backend::exec`], also each step of an
//! `engine.batch`) is checked before it runs: its file parameters must be absolute paths inside
//! the roots (symbolic links resolved, no `..`), any other absolute path in its parameters too,
//! and commands that write to a file they are not given (Save, Increment and Save, rendering the
//! Render Queue) are checked against where they would write. Queueing a render without naming
//! its output needs a saved project, because the default output of an unsaved one is the home
//! folder. In a headless session the footage and other files a project or an imported file names
//! are read only inside the roots ([`ConfinedFootage`], [`ConfinedServices`]). Commands that run
//! code or open other applications (scripts, ScriptUI panels and the events that reach them,
//! Execute File, effect helpers, Reveal in Finder…) are refused while confined, as is granting scripts the file
//! and network access the user withheld.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use effectcraft_engine::Services;
use effectcraft_engine::project::{Footage, ItemId};
use effectcraft_engine::raster::{AuxChannels, Image};
use effectcraft_engine::render::{FloatCurve, FootageSource};
use effectcraft_engine::time::Tick;
use serde_json::Value;

use crate::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

/// The folders an agent may read and write.
#[derive(Clone, Debug)]
pub struct Confinement {
    read: Vec<PathBuf>,
    write: Vec<PathBuf>,
}

/// Commands that run code, act outside the project or open other applications, besides the
/// commands that run scripts ([`effectcraft_engine::commands::scripts::RUNS_SCRIPTS`]).
const REFUSED: &[&str] = &[
    "file.installScript",
    "file.installScriptUIPanel",
    "file.uninstallScript",
    // Copies a plug-in (and a bundle's ScriptUI panels) from a user-chosen path into the folders
    // the app loads code from: an agent must not plant code that runs on every start, and its
    // sources are checked no better than `file.installScript`'s.
    "effect.plugins.install",
    // A ScriptUI panel's script runs when the panel opens, when an event reaches one of its
    // controls, and when one of its `app.scheduleTask` tasks comes due.
    "window.scriptPanel",
    "window.restoreScriptPanels",
    "script.runTasks",
    "scriptui.click",
    "scriptui.set",
    "scriptui.mouse",
    "scriptui.close",
    "file.executeFile",
    "effect.helpers.scan",
    "effect.helpers.load",
    "file.revealInFinder",
    "layer.openSource",
    "help.revealLogFile",
    "file.autoSave",
    // The Plug-in Library downloads code, installs it to load at start-up and reads `file://` catalogs.
    "library.refresh",
    "library.install",
    "library.update",
    "library.uninstall",
    "library.loadInstalled",
    "library.preview",
];

/// Most numbered folders `check_render_folder` inspects for one output; more are refused.
const MAX_NUMBERED_FOLDERS: usize = 4096;

/// Commands that read files named by these parameters.
const READS: &[(&str, &[&str])] = &[
    ("file.open", &["path"]),
    ("file.openRecent", &["path"]),
    ("file.import", &["path", "paths"]),
    ("file.importMultiple", &["paths"]),
    ("file.importRecent", &["path"]),
    ("file.importLottie", &["path"]),
    ("file.importAep", &["path"]),
    ("file.importTimeline", &["path"]),
    ("file.replaceFootage", &["path"]),
    ("file.relinkFootage", &["folder"]),
    ("file.setProxy", &["path"]),
    ("file.watchFolder", &["folder"]),
    ("anim.applyPreset", &["path"]),
    ("anim.applyRecentPreset", &["path"]),
    ("keys.rpfCameraImport", &["path"]),
    ("essential.importTemplate", &["path"]),
    ("view.importGuides", &["path"]),
    ("shortcuts.import", &["path"]),
    ("effect.plugins.load", &["path", "folder"]),
    ("mediaBrowser.go", &["path"]),
    ("mediaBrowser.addFavorite", &["path"]),
    // #1243: these read the files they are given but were missing, so a relative path went
    // unchecked.
    ("essential.templateInfo", &["path"]),
    ("file.importProject", &["path"]),
    ("file.watchFolder.poll", &["folder"]),
    ("footage.relink", &["path"]),
    ("layer.newModel", &["path"]),
    ("mediaBrowser.fileInfo", &["path"]),
    ("mediaBrowser.list", &["path"]),
    ("mediaBrowser.import", &["paths"]),
    ("mediaBrowser.removeFavorite", &["path"]),
    ("face.model.install", &["path"]),
    ("roto.model.install", &["path"]),
    // Looks up what the file at `path` is (writable roots are readable too).
    ("file.checkOutput", &["path"]),
    // Reads the template at `path`; what it writes is in `MIXED_WRITES`.
    ("templates.create", &["path"]),
];

/// Commands that read some parameters (listed in [`READS`]) and write others.
const MIXED_WRITES: &[(&str, &[&str])] = &[("templates.create", &["projectPath", "footageDir"])];

/// Commands that write files named by these parameters.
const WRITES: &[(&str, &[&str])] = &[
    ("camera.exportBlender", &["path"]),
    ("file.save", &["path"]),
    ("file.saveAs", &["path"]),
    ("file.saveCopy", &["path"]),
    ("file.saveCopyAsXml", &["path"]),
    ("file.collectFiles", &["folder"]),
    // This query checks a proposed write destination, so it follows write confinement.
    ("file.checkOutput", &["path"]),
    ("file.exportLottie", &["path"]),
    ("file.exportAep", &["path"]),
    ("comp.saveFrameAs", &["path"]),
    ("comp.saveFrameAsPsd", &["path"]),
    ("comp.saveFrameAsExr", &["path"]),
    ("anim.savePreset", &["path"]),
    ("view.exportGuides", &["path"]),
    ("render.saveCurrentPreview", &["path"]),
    ("render.preRender", &["output"]),
    ("essential.exportTemplate", &["path"]),
    ("shortcuts.export", &["path"]),
    ("renderQueue.add", &["output", "path"]),
    ("file.exportTimeline", &["path"]),
    ("journal.export", &["path"]),
    ("help.exportRepro", &["path"]),
    // `path` / `output` may also be a file-name template, as for render.preRender.
    ("file.createProxy", &["path", "output"]),
];

/// Commands that write where they are not told: the project's file, the Render Queue's outputs.
const IMPLICIT_WRITES: &[&str] = &["file.save", "file.incrementAndSave", "renderQueue.render", "render.preRender"];

/// Commands that queue a render, whose output defaults to the project's folder when not given.
const QUEUES_AN_OUTPUT: &[&str] = &["renderQueue.add", "render.preRender", "file.createProxy"];

/// What must be looked up before an implicitly writing command may run.
#[derive(Debug, PartialEq, Eq)]
pub enum Lookup {
    /// The open project's file (`project.summary` → `path`).
    ProjectFile,
    /// Every output of the queued Render Queue items (`renderQueue.list`).
    RenderOutputs,
    /// Whether the open project has a file (`project.summary` → `path`): a render queued without
    /// `output` (Render Queue, Pre-render, Create Proxy) writes beside it, or to the home folder
    /// before the first save.
    QueueDefault,
}

impl Confinement {
    /// Roots that must exist (each resolved to its real path). With no write root nothing may be
    /// written; write roots may also be read.
    pub fn new(read: &[PathBuf], write: &[PathBuf]) -> Result<Self> {
        let real = |p: &PathBuf| p.canonicalize().map_err(|e| Error::BadArgs(format!("automation root {}: {e}", p.display())));
        let read = read.iter().map(real).collect::<Result<Vec<_>>>()?;
        let write = write.iter().map(real).collect::<Result<Vec<_>>>()?;
        Ok(Self { read, write })
    }

    fn roots(&self, access: Access) -> impl Iterator<Item = &PathBuf> {
        let read = if access == Access::Read { &self.read[..] } else { &[] };
        read.iter().chain(&self.write)
    }

    /// `path` resolved, if it is inside the roots for `access`.
    pub fn check(&self, path: &str, access: Access) -> Result<PathBuf> {
        let denied = |why: &str| {
            let kind = if access == Access::Read { "read" } else { "write" };
            let roots: Vec<String> = self.roots(access).map(|r| r.display().to_string()).collect();
            let roots = if roots.is_empty() { "none".to_string() } else { roots.join(", ") };
            Error::BadArgs(format!("cannot {kind} `{path}`: {why} (automation {kind} roots: {roots})"))
        };
        let p = Path::new(path);
        if !p.is_absolute() {
            return Err(denied("give an absolute path"));
        }
        if p.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(denied("`..` is not allowed"));
        }
        let real = resolve(p).ok_or_else(|| denied("it can't be resolved"))?;
        if self.roots(access).any(|r| real.starts_with(r)) { Ok(real) } else { Err(denied("it is outside the automation roots")) }
    }

    /// A render output's folder holds no symbolic link that leads outside the write roots. An
    /// image sequence is written to numbered names (`out_[#####].png` → `out_00001.png`, and a
    /// sequence output without `#` gets `_00001` too) that the check of its template never
    /// resolved, and each frame file is created through whatever link already sits at its name,
    /// truncating the file it points to. A link that can't be resolved counts as outside: the
    /// render would create its target. When the frame number is in a folder name, the same goes for
    /// each existing numbered folder and the folder it holds the frames in.
    fn check_render_folder(&self, output: &str) -> Result<()> {
        let real = self.check(output, Access::Write)?;
        if let Some(dir) = real.parent() {
            self.check_no_outward_links(dir, output)?;
        }
        // The frame number may sit in a folder name (`shots_[##]/f_[#####].png`): the folders the
        // frames go to are the numbered ones that exist, which the template's own parent is not.
        let (before, _, after) = effectcraft_engine::project::render_queue::sequence_parts(output);
        let sep = ['/', '\\'];
        let Some((dir_tail, rest)) = after.split_once(sep) else { return Ok(()) };
        let (parent, prefix) = before.rsplit_once(sep).unwrap_or(("", &before));
        // A parent that doesn't exist has no numbered folders; one that can't be listed might.
        let scan = if parent.is_empty() { Path::new("/") } else { Path::new(parent) };
        let entries = match std::fs::read_dir(scan) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(Error::BadArgs(format!("cannot render to `{output}`: its numbered folders in `{}` can't be checked: {e}", scan.display()))),
        };
        let mut seen = 0;
        for e in entries {
            let e =
                e.map_err(|e| Error::BadArgs(format!("cannot render to `{output}`: its numbered folders in `{}` can't be checked: {e}", scan.display())))?;
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(digits) = name.strip_prefix(prefix).and_then(|n| n.strip_suffix(dir_tail)) else { continue };
            if !digits.strip_prefix('-').unwrap_or(digits).bytes().all(|b| b.is_ascii_digit()) || digits.is_empty() {
                continue;
            }
            seen += 1;
            if seen > MAX_NUMBERED_FOLDERS {
                return Err(Error::BadArgs(format!("cannot render to `{output}`: more than {MAX_NUMBERED_FOLDERS} numbered folders already exist to check")));
            }
            // The numbered folder itself may be a link out; `check` resolves every existing part.
            let frame_file = e.path().join(rest);
            let frame_file = frame_file.to_string_lossy();
            self.check(&frame_file, Access::Write)?;
            if let Some(dir) = Path::new(&*frame_file).parent().and_then(resolve) {
                self.check_no_outward_links(&dir, output)?;
            }
        }
        Ok(())
    }

    /// `dir` holds no symbolic link that leads outside the write roots.
    fn check_no_outward_links(&self, dir: &Path, output: &str) -> Result<()> {
        // A folder that does not exist yet holds no link; one that can't be listed might.
        let cannot = |e: std::io::Error| Error::BadArgs(format!("cannot render to `{output}`: its folder `{}` can't be checked for links: {e}", dir.display()));
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(cannot(e)),
        };
        for e in entries {
            let e = e.map_err(cannot)?;
            if !e.file_type().map_err(cannot)?.is_symlink() {
                continue;
            }
            let link = e.path();
            if !link.canonicalize().is_ok_and(|r| self.write.iter().any(|w| r.starts_with(w))) {
                return Err(Error::BadArgs(format!(
                    "cannot render to `{output}`: its folder holds `{}`, a link that leads outside the automation write roots, where a numbered frame could be written",
                    link.display()
                )));
            }
        }
        Ok(())
    }

    /// Check engine command `id` with `params` before it runs. Returns what must still be looked
    /// up and passed to [`Confinement::check_lookup`] for commands that write implicitly.
    pub fn check_command(&self, id: &str, params: &Value) -> Result<Vec<Lookup>> {
        if REFUSED.contains(&id) || effectcraft_engine::commands::scripts::RUNS_SCRIPTS.contains(&id) {
            return Err(Error::BadArgs(format!("`{id}` is not available while file access is confined (--automation-read-root/--automation-write-root)")));
        }
        // Scripts can't grant themselves the access the user withheld
        // (`effectcraft_engine::prefs::Scripting::same_permissions`), and neither may a confined
        // agent: `prefs.set` saves the settings file, so a script run long after this session
        // would still have it. (`prefs.reset` only puts the defaults back, which grant nothing.)
        if id == "prefs.set" && prefs_keys(params).iter().any(|k| is_scripting_pref(k)) {
            return Err(Error::BadArgs(
                "`prefs.set`: the Scripting settings decide what a script may do, so they can't be changed while file access is confined".into(),
            ));
        }
        if id == "engine.batch" {
            for step in params.get("steps").and_then(Value::as_array).into_iter().flatten() {
                let Some(inner) = step.get("command").or_else(|| step.get("id")).and_then(Value::as_str) else { continue };
                let inner_params = step.get("params").cloned().unwrap_or(Value::Null);
                // A whole `params` given as a `$N` reference becomes an earlier step's result
                // only as the batch runs, so none of its file paths would have been checked
                // (`essential.templateInfo` returns a manifest naming any file at all).
                if !matches!(inner_params, Value::Null | Value::Object(_)) {
                    return Err(Error::BadArgs(format!("`{inner}` in a batch: `params` must be an object while file access is confined")));
                }
                let named = keys_of(READS, inner).or_else(|| keys_of(WRITES, inner)).unwrap_or(&[]);
                let mixed = keys_of(MIXED_WRITES, inner).unwrap_or(&[]);
                // A step's `$N` reference is resolved only as the batch runs: never for a file.
                if named.iter().chain(mixed).filter_map(|k| inner_params.get(*k)).any(has_reference) {
                    return Err(Error::BadArgs(format!("`{inner}` in a batch: file paths can't be `$N` references while file access is confined")));
                }
                if !self.check_command(inner, &inner_params)?.is_empty() {
                    return Err(Error::BadArgs(format!("`{inner}` writes to its project's or queue's files: run it on its own while file access is confined")));
                }
            }
            return Ok(vec![]);
        }
        let (named, access) = match (keys_of(WRITES, id), keys_of(READS, id)) {
            (Some(k), _) => (k, Access::Write),
            (None, Some(k)) => (k, Access::Read),
            (None, None) if id.starts_with("renderQueue.") || id == "prefs.set" => (&[][..], Access::Write),
            (None, None) => (&[][..], Access::Read),
        };
        let mut given = false;
        // Whether one of them is a file: not a Render Queue template, whose own output is relative
        // and resolved like one the agent never gave, and not an array, which the commands that
        // take a single output read as nothing at all.
        let mut gave_a_file = false;
        for key in named {
            let Some(v) = params.get(*key).filter(|v| !v.is_null()) else { continue };
            given = true;
            let paths: Vec<&Value> = match v {
                Value::Array(a) => a.iter().collect(),
                v => vec![v],
            };
            for p in paths {
                // A Render Queue output may name a template instead of a file.
                match p.as_str() {
                    Some(s) if matches!(id, "render.preRender" | "file.createProxy") && !looks_like_path(s) => {}
                    Some(s) => {
                        gave_a_file |= v.is_string();
                        self.check(s, access)?;
                        if QUEUES_AN_OUTPUT.contains(&id) || id.starts_with("renderQueue.") {
                            self.check_render_folder(s)?;
                        }
                    }
                    None => return Err(Error::BadArgs(format!("`{id}`: `{key}` must be a path"))),
                }
            }
        }
        // Parameters of a reading command that it writes to.
        let mixed = keys_of(MIXED_WRITES, id).unwrap_or(&[]);
        for key in mixed {
            if let Some(p) = params.get(*key).filter(|v| !v.is_null()) {
                let p = p.as_str().ok_or_else(|| Error::BadArgs(format!("`{id}`: `{key}` must be a path")))?;
                self.check(p, Access::Write)?;
            }
        }
        // Any other absolute path the command is given.
        let named: Vec<&str> = named.iter().chain(mixed).copied().collect();
        let mut other = vec![];
        absolute_strings(params, &named, &mut other);
        for s in other {
            self.check(&s, access)?;
        }
        // An entry of a recent list by index is a path the agent didn't give.
        if matches!(id, "file.openRecent" | "file.importRecent" | "anim.applyRecentPreset") && !given {
            return Err(Error::BadArgs(format!("`{id}`: give `path` while file access is confined")));
        }
        // The default repro destination is the settings store, not an automation write root.
        // Require a single explicit path so missing, null or array parameters cannot bypass it.
        if id == "help.exportRepro" && params.get("path").and_then(Value::as_str).is_none() {
            return Err(Error::BadArgs("`help.exportRepro`: give `path` inside an automation write root while file access is confined".into()));
        }
        let mut lookups = vec![];
        // An output the agent did not name is derived from where the project sits: the Default
        // Output Folder preference when that is set, beside the project file otherwise — and the
        // home folder when the project has never been saved, outside every root (#2479). Only
        // that last case is certainly outside, and reading the preference here would duplicate
        // the engine's precedence, so an unsaved project is told to name its output instead.
        if QUEUES_AN_OUTPUT.contains(&id) && !gave_a_file {
            lookups.push(Lookup::QueueDefault);
        }
        if IMPLICIT_WRITES.contains(&id) && !(given && id == "file.save") {
            lookups.push(if id == "file.save" || id == "file.incrementAndSave" { Lookup::ProjectFile } else { Lookup::RenderOutputs });
        }
        Ok(lookups)
    }

    /// Check what a [`Lookup`] found: `project.summary` or `renderQueue.list`'s result.
    pub fn check_lookup(&self, lookup: &Lookup, found: &Value) -> Result<()> {
        match lookup {
            Lookup::ProjectFile => match found.get("path").and_then(Value::as_str) {
                // Increment and Save writes beside it.
                Some(p) => self.check(p, Access::Write).map(drop),
                None => Err(Error::BadArgs("the project has no file yet: give `path` (Save As) inside an automation write root".into())),
            },
            // A saved project's default output sits beside it, and rendering checks it then.
            Lookup::QueueDefault => match found.get("path").and_then(Value::as_str) {
                Some(_) => Ok(()),
                None => Err(Error::BadArgs(
                    "the project has no file yet, so the default output would go to the home folder: give `output` inside an automation write root, or save the project inside one first".into(),
                )),
            },
            Lookup::RenderOutputs => {
                let items = found.get("items").or(Some(found)).and_then(Value::as_array).cloned().unwrap_or_default();
                for it in items.iter().filter(|it| it.get("render").and_then(Value::as_bool) != Some(false)) {
                    let extra = it.get("extraOutputs").and_then(Value::as_array).into_iter().flatten().map(|o| o.get("outputPath"));
                    for p in std::iter::once(it.get("outputPath")).chain(extra).flatten() {
                        if let Some(p) = p.as_str() {
                            self.check(p, Access::Write)?;
                            self.check_render_folder(p)?;
                        }
                    }
                    if let Some(p) = it.get("logPath").and_then(Value::as_str) {
                        self.check(p, Access::Write)?;
                    }
                }
                Ok(())
            }
        }
    }
}

/// The session's footage, read only inside the read roots (#2131 F-11, "Indirect reads"). The
/// commands an agent runs are checked by the paths they are given, but a project opened or
/// imported from a read root names its own footage, which is read when it renders: a file
/// outside the roots shows as missing footage instead.
pub struct ConfinedFootage {
    inner: Arc<dyn FootageSource>,
    confinement: Confinement,
}

impl ConfinedFootage {
    pub fn new(inner: Arc<dyn FootageSource>, confinement: Confinement) -> Self {
        Self { inner, confinement }
    }

    /// Whether every file of `f` (its file and each frame of a sequence) is inside the roots.
    /// Checked on every read, not remembered: a symbolic link inside a root can be pointed
    /// elsewhere after it was first read.
    fn inside(&self, f: &Footage) -> bool {
        std::iter::once(&f.path).chain(&f.sequence).filter(|p| !p.is_empty()).all(|p| self.confinement.check(p, Access::Read).is_ok())
    }

    /// [`Self::inside`] for a read: a refused one is counted on this thread, so the render
    /// [`recording`](FootageSource::recording) it can't list what it made without the footage.
    fn readable(&self, f: &Footage) -> bool {
        let inside = self.inside(f);
        if !inside {
            let _ = REFUSED_READS.try_with(|n| n.set(n.get().wrapping_add(1)));
        }
        inside
    }
}

thread_local! {
    /// Footage reads refused on this thread (see [`ConfinedFootage::readable`]).
    static REFUSED_READS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The number of reads this thread has refused for lying outside the allowed roots.
fn refused_reads() -> u64 {
    REFUSED_READS.try_with(std::cell::Cell::get).unwrap_or(0)
}

impl FootageSource for ConfinedFootage {
    /// The frame from the inner source, or `None` when the footage lies outside the allowed roots.
    fn frame(&self, item: ItemId, footage: &Footage, t: Tick) -> Option<Arc<Image>> {
        self.readable(footage).then(|| self.inner.frame(item, footage, t)).flatten()
    }
    /// The frame scaled to `width` by `height`, or `None` when the footage lies outside the allowed roots.
    fn frame_at_size(&self, item: ItemId, footage: &Footage, t: Tick, width: u32, height: u32) -> Option<Arc<Image>> {
        self.readable(footage).then(|| self.inner.frame_at_size(item, footage, t, width, height)).flatten()
    }
    /// The frame with an explicit size and float curve, or `None` when the footage lies outside the allowed roots.
    fn frame_with(&self, item: ItemId, footage: &Footage, t: Tick, size: Option<(u32, u32)>, float: FloatCurve) -> Option<Arc<Image>> {
        self.readable(footage).then(|| self.inner.frame_with(item, footage, t, size, float)).flatten()
    }
    /// The frame as 8-bit levels, or `None` when the footage lies outside the allowed roots.
    fn frame8(&self, item: ItemId, footage: &Footage, t: Tick, float: FloatCurve) -> Option<Arc<effectcraft_engine::render::disk_cache::Frame8>> {
        self.readable(footage).then(|| self.inner.frame8(item, footage, t, float)).flatten()
    }
    /// The footage audio from the inner source, or `None` when the footage lies outside the allowed roots.
    fn audio(&self, item: ItemId, footage: &Footage, t: Tick, frames: usize, rate: u32) -> Option<Vec<f32>> {
        self.readable(footage).then(|| self.inner.audio(item, footage, t, frames, rate)).flatten()
    }
    /// The auxiliary channels of the footage frame, or `None` when the footage lies outside the allowed roots.
    fn aux(&self, item: ItemId, footage: &Footage, t: Tick) -> Option<Arc<AuxChannels>> {
        self.readable(footage).then(|| self.inner.aux(item, footage, t)).flatten()
    }
    /// Forwards the auxiliary footage paths to the inner source.
    fn set_aux_footage(&self, paths: &[String]) {
        self.inner.set_aux_footage(paths)
    }
    /// The 3D model for the footage, or `None` when the footage lies outside the allowed roots.
    fn model(&self, item: ItemId, footage: &Footage) -> Option<Arc<effectcraft_model::Model>> {
        self.readable(footage).then(|| self.inner.model(item, footage)).flatten()
    }
    /// The rasterised vector footage at `scale`, or `None` when the footage lies outside the allowed roots.
    fn vector_frame(&self, item: ItemId, footage: &Footage, scale: f64) -> Option<Arc<Image>> {
        self.readable(footage).then(|| self.inner.vector_frame(item, footage, scale)).flatten()
    }
    /// Says why footage outside the roots shows nothing, so the agent can act on it.
    fn problem(&self, footage: &Footage) -> Option<String> {
        if !self.inside(footage) {
            return Some(format!("{} is outside the automation read roots, so it is not read", footage.path));
        }
        self.inner.problem(footage)
    }
    fn set_cache_budget(&self, bytes: usize) {
        self.inner.set_cache_budget(bytes)
    }
    fn purge(&self) {
        self.inner.purge()
    }
    fn forget(&self, path: &str) {
        self.inner.forget(path)
    }
    fn set_conform_folder(&self, folder: Option<PathBuf>) {
        self.inner.set_conform_folder(folder)
    }
    /// The inner source's cache budget.
    fn cache_budget(&self) -> Option<usize> {
        self.inner.cache_budget()
    }
    /// Runs `f` under the inner source's cancel flag.
    fn cancellable(&self, cancel: &Arc<AtomicBool>, f: &mut dyn FnMut()) {
        self.inner.cancellable(cancel, f)
    }
    /// Whether the inner source has been called off.
    fn called_off(&self) -> bool {
        self.inner.called_off()
    }
    /// The files read inside the roots, as the inner source notes them. Footage outside reads
    /// nothing, and what is made without it can't be listed by its files: a refused read makes
    /// the list incomplete, so it never outlives the session in a disk cache (another session
    /// with other roots would read the footage).
    fn recording(&self, reads: &effectcraft_engine::render::reads::ReadLog, f: &mut dyn FnMut()) {
        let refused = refused_reads();
        self.inner.recording(reads, f);
        if refused_reads() != refused {
            reads.mark_incomplete();
        }
    }
}

/// The session's file reads, only inside the read roots: what a project or an imported file
/// pulls in by itself (an SVG's or Lottie's images, a timeline's media) is checked like a path
/// the agent gives. Writes are left to the command checks.
pub struct ConfinedServices {
    inner: Arc<dyn Services>,
    confinement: Confinement,
}

impl ConfinedServices {
    pub fn new(inner: Arc<dyn Services>, confinement: Confinement) -> Self {
        Self { inner, confinement }
    }

    fn inside(&self, path: &str) -> bool {
        self.confinement.check(path, Access::Read).is_ok()
    }
}

impl Services for ConfinedServices {
    fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
        self.confinement.check(path, Access::Read).map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string()))?;
        self.inner.read_file(path)
    }
    fn write_file(&self, path: &str, data: &[u8]) -> std::io::Result<()> {
        self.inner.write_file(path, data)
    }
    fn exists(&self, path: &str) -> bool {
        self.inside(path) && self.inner.exists(path)
    }
    fn stamp(&self, path: &str) -> Option<(u64, u128)> {
        if self.inside(path) { self.inner.stamp(path) } else { None }
    }
    fn store_file(&self, path: &str, data: &[u8]) -> std::io::Result<()> {
        self.inner.store_file(path, data)
    }
    fn list_dir(&self, dir: &str) -> Vec<String> {
        if self.inside(dir) { self.inner.list_dir(dir) } else { vec![] }
    }
    fn is_dir(&self, path: &str) -> bool {
        self.inside(path) && self.inner.is_dir(path)
    }
    fn full_path(&self, path: &str) -> Option<String> {
        self.inner.full_path(path)
    }
}

/// Whether a `prefs.set` key reaches the Scripting page. `Prefs::set` turns the key into a JSON
/// pointer (`key.replace('.', "/")`), so `scripting/allowScriptsWriteFiles` resolves as well as
/// the dotted spelling, and the page on its own replaces every setting on it.
fn is_scripting_pref(key: &str) -> bool {
    key.split(['.', '/']).next().is_some_and(|page| page.eq_ignore_ascii_case("scripting"))
}

/// The settings `prefs.set` is given: `key`, and the keys of `values`.
fn prefs_keys(params: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = params.get("key").and_then(Value::as_str).into_iter().collect();
    if let Some(Value::Object(m)) = params.get("values") {
        keys.extend(m.keys().map(String::as_str));
    }
    keys
}

fn keys_of(table: &[(&str, &'static [&'static str])], id: &str) -> Option<&'static [&'static str]> {
    table.iter().find(|(c, _)| *c == id).map(|(_, k)| *k)
}

fn has_reference(v: &Value) -> bool {
    match v {
        Value::String(s) => s.starts_with('$'),
        Value::Array(a) => a.iter().any(has_reference),
        _ => false,
    }
}

/// A file path rather than a template name: has a separator or an extension.
fn looks_like_path(s: &str) -> bool {
    s.contains(['/', '\\']) || Path::new(s).extension().is_some()
}

/// Strings in `v` (recursively, except under the top-level `skip` keys) that are absolute paths,
/// `~` paths or `file:` URLs.
fn absolute_strings(v: &Value, skip: &[&str], out: &mut Vec<String>) {
    fn walk(v: &Value, out: &mut Vec<String>, depth: usize) {
        if depth > 64 {
            return;
        }
        match v {
            Value::String(s) => {
                let t = s.trim();
                let drive = t.len() > 2 && t.as_bytes().get(1) == Some(&b':') && t.as_bytes().first().is_some_and(u8::is_ascii_alphabetic);
                if t.starts_with('/') || t.starts_with('\\') || t.starts_with('~') || drive || t.to_ascii_lowercase().starts_with("file:") {
                    out.push(t.to_string());
                }
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, out, depth + 1)),
            Value::Object(o) => o.values().for_each(|x| walk(x, out, depth + 1)),
            _ => {}
        }
    }
    match v {
        Value::Object(o) => o.iter().filter(|(k, _)| !skip.contains(&k.as_str())).for_each(|(_, x)| walk(x, out, 0)),
        v => walk(v, out, 0),
    }
}

/// `p` with symbolic links resolved: its longest existing ancestor's real path plus the rest
/// (which doesn't exist yet, so can't be a link).
fn resolve(p: &Path) -> Option<PathBuf> {
    let mut rest = vec![];
    let mut at = p;
    loop {
        if let Ok(real) = at.canonicalize() {
            return Some(rest.iter().rev().fold(real, |acc: PathBuf, c| acc.join(c)));
        }
        rest.push(at.file_name()?.to_os_string());
        at = at.parent()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Commands whose parameters are named like files but aren't: property paths
    /// (`path|prop`), project folders, layers to track or target.
    const NOT_FILES: &[&str] = &[
        "camera.createFromSolve",
        "edit.label",
        "edit.pasteReversedKeyframes",
        "effect.editDropdown",
        "essential.addProperty",
        "essential.canAdd",
        "essential.linkProperty",
        "essential.unlinkProperty",
        "keys.paste",
        "keys.set",
        "keys.setEase",
        "keys.setSpatialTangents",
        "paths.nullsFollowPoints",
        "paths.pointsFollowNulls",
        "paths.tracePath",
        "project.move",
        "prop.addKey",
        "prop.convertExpressionToKeyframes",
        "prop.deselect",
        "prop.editGradient",
        "prop.get",
        "prop.pickWhip",
        "prop.reset",
        "prop.select",
        "prop.set",
        "prop.setExpression",
        "prop.toggleAnimation",
        "prop.toggleKey",
        "track.new",
        "track.property",
        "track.setTarget",
    ];

    /// Every engine command with a parameter named like a file is classified, so a new one
    /// can't silently skip confinement (#1243): it reads or writes the files it is given, is
    /// refused, writes where it isn't told (checked by a lookup), sets Render Queue outputs
    /// (checked when the queue renders), or is listed in `NOT_FILES`.
    #[test]
    fn every_command_with_file_parameters_is_classified() {
        const FILE_KEYS: &[&str] =
            &["path", "paths", "folder", "file", "files", "dir", "directory", "output", "outputs", "source", "target", "projectPath", "footageDir"];
        let mut missing = vec![];
        for spec in effectcraft_engine::command_specs() {
            let Some(params) = effectcraft_engine::commands::accepted_params(spec.params) else { continue };
            let id = spec.id;
            let classified = keys_of(READS, id).is_some()
                || keys_of(WRITES, id).is_some()
                || REFUSED.contains(&id)
                || effectcraft_engine::commands::scripts::RUNS_SCRIPTS.contains(&id)
                || IMPLICIT_WRITES.contains(&id)
                || id.starts_with("renderQueue.")
                || id == "render.addOutputModule"
                || NOT_FILES.contains(&id);
            if params.iter().any(|p| FILE_KEYS.contains(&p.as_str())) && !classified {
                missing.push(format!("{id} {}", spec.params));
            }
        }
        assert!(missing.is_empty(), "add these to READS / WRITES / REFUSED or NOT_FILES:\n{}", missing.join("\n"));
        // And every listed command exists.
        for id in READS.iter().chain(WRITES).chain(MIXED_WRITES).map(|(c, _)| *c).chain(NOT_FILES.iter().copied()) {
            assert!(effectcraft_engine::command_specs().iter().any(|s| s.id == id), "{id} is not a command");
        }
    }

    /// Footage outside the roots shows nothing: a render that asked for it can't be listed by its
    /// files (#3184), or a disk cache would keep the empty picture for a session with other
    /// roots. Reads inside the roots record as the inner source tells.
    #[test]
    fn a_refused_footage_read_makes_the_render_unlisted() {
        use effectcraft_engine::render::reads::{FileStamp, ReadFile, ReadLog};
        /// Reads every file, noting it.
        struct Noting;
        impl FootageSource for Noting {
            /// Returns a 1x1 frame and notes the footage path it was asked for.
            fn frame(&self, _: ItemId, f: &Footage, _: Tick) -> Option<Arc<Image>> {
                NOTED.with(|n| n.borrow_mut().push(f.path.clone()));
                Some(Arc::new(Image::new(1, 1)))
            }
            /// Runs `f`, then reports every path noted meanwhile to `reads` as a file with a missing stamp.
            fn recording(&self, reads: &ReadLog, f: &mut dyn FnMut()) {
                f();
                for p in NOTED.with(|n| std::mem::take(&mut *n.borrow_mut())) {
                    reads.note(ReadFile::new(p, FileStamp::Missing));
                }
            }
        }
        thread_local! {
            static NOTED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
        }
        // (a fresh directory, removed when the test ends, panicking or not)
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let (inside, outside) = (base.join("in"), base.join("elsewhere"));
        for d in [&inside, &outside] {
            std::fs::create_dir_all(d).unwrap();
        }
        let c = Confinement::new(std::slice::from_ref(&inside), &[]).unwrap();
        let src = ConfinedFootage::new(Arc::new(Noting), c);
        let footage = |d: &std::path::Path| Footage { path: d.join("a.png").to_string_lossy().into_owned(), ..Default::default() };
        let read = |f: &Footage| {
            let log = ReadLog::new();
            let mut got = None;
            src.recording(&log, &mut || got = src.frame(ItemId(1), f, Tick::ZERO));
            (got.is_some(), log.into_files())
        };
        let (shown, files) = read(&footage(&inside));
        assert!(shown && files.is_complete() && files.len() == 1, "inside the roots: {files:?}");
        let (shown, files) = read(&footage(&outside));
        assert!(!shown && !files.is_complete(), "outside the roots: {files:?}");
        // The next render on the thread is not affected.
        assert!(read(&footage(&inside)).1.is_complete());
    }

    #[test]
    fn relative_and_outside_paths_of_newly_listed_commands_are_refused() {
        let base = std::env::temp_dir().join(format!("ec-confine-1243-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (read, write, outside) = (base.join("in"), base.join("out"), base.join("elsewhere"));
        for d in [&read, &write, &outside] {
            std::fs::create_dir_all(d).unwrap();
        }
        let c = Confinement::new(std::slice::from_ref(&read), std::slice::from_ref(&write)).unwrap();
        let p = |d: &std::path::Path, f: &str| d.join(f).to_string_lossy().into_owned();
        let up = format!("{}/../elsewhere/t.ectemplate", read.display());
        for (id, key) in [
            ("essential.templateInfo", "path"),
            ("file.importProject", "path"),
            ("footage.relink", "path"),
            ("layer.newModel", "path"),
            ("mediaBrowser.fileInfo", "path"),
            ("mediaBrowser.list", "path"),
            ("face.model.install", "path"),
            ("templates.create", "path"),
        ] {
            for bad in ["t.ectemplate".to_string(), "../elsewhere/t.ectemplate".to_string(), up.clone(), p(&outside, "t.ectemplate")] {
                assert!(c.check_command(id, &serde_json::json!({key: bad})).is_err(), "{id} {key}={bad}");
            }
            assert!(c.check_command(id, &serde_json::json!({key: p(&read, "t.ectemplate")})).is_ok(), "{id} in a read root");
        }
        // Batched too.
        let steps = serde_json::json!({"steps": [{"command": "essential.templateInfo", "params": {"path": "t.ectemplate"}}]});
        assert!(c.check_command("engine.batch", &steps).is_err());
        // Writes: into a write root only.
        for id in ["file.exportTimeline", "file.checkOutput"] {
            for bad in ["cut.xml".to_string(), p(&read, "cut.xml"), p(&outside, "cut.xml")] {
                assert!(c.check_command(id, &serde_json::json!({"path": bad})).is_err(), "{id} {bad}");
            }
            assert!(c.check_command(id, &serde_json::json!({"path": p(&write, "cut.xml")})).is_ok());
        }
        let create = |project: String| serde_json::json!({"path": p(&read, "t.ectemplate"), "projectPath": project});
        assert!(c.check_command("templates.create", &create(p(&read, "new.ecproj"))).is_err(), "templates.create writes projectPath");
        assert!(c.check_command("templates.create", &create(p(&write, "new.ecproj"))).is_ok());
        assert!(c.check_command("file.createProxy", &serde_json::json!({"output": "[compName]_proxy"})).is_ok(), "a template name");
        assert!(c.check_command("file.createProxy", &serde_json::json!({"output": p(&outside, "proxy.mov")})).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Installing a plug-in plants code the app runs at every start, so confinement refuses it
    /// even when the file is inside a read root (like `file.installScript`).
    #[test]
    fn installing_plugins_is_refused_while_confined() {
        let base = std::env::temp_dir().join(format!("ec-confine-plugins-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let read = base.join("in");
        std::fs::create_dir_all(&read).unwrap();
        let c = Confinement::new(std::slice::from_ref(&read), &[]).unwrap();
        let inside = read.join("a.wasm").to_string_lossy().into_owned();
        for params in [
            serde_json::json!({"path": inside}),
            serde_json::json!({"folder": read.to_string_lossy()}),
            serde_json::json!({"path": "a.wasm"}),
            serde_json::json!({"bundle": true}),
        ] {
            assert!(c.check_command("effect.plugins.install", &params).is_err(), "{params}");
        }
    }

    #[test]
    fn a_batch_step_whose_params_are_a_reference_is_refused() {
        let base = std::env::temp_dir().join(format!("ec-confine-batch-ref-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (read, write) = (base.join("in"), base.join("out"));
        for d in [&read, &write] {
            std::fs::create_dir_all(d).unwrap();
        }
        let c = Confinement::new(std::slice::from_ref(&read), std::slice::from_ref(&write)).unwrap();
        let info = read.join("t.ectemplate").to_string_lossy().into_owned();
        // Step 2's params are step 1's result, which can name any `path`.
        let steps = serde_json::json!({"steps": [
            {"command": "essential.templateInfo", "params": {"path": info}},
            {"command": "file.saveAs", "params": "$1"},
        ]});
        let e = c.check_command("engine.batch", &steps).unwrap_err().to_string();
        assert!(e.contains("must be an object"), "{e}");
        for params in [serde_json::json!("$1.manifest"), serde_json::json!(["$1"]), serde_json::json!(7)] {
            let steps = serde_json::json!({"steps": [{"command": "file.saveAs", "params": params}]});
            assert!(c.check_command("engine.batch", &steps).is_err(), "{params}");
        }
        // Object params (or none) are checked as before.
        let saved = write.join("p.ecproj").to_string_lossy().into_owned();
        let steps = serde_json::json!({"steps": [{"command": "file.saveAs", "params": {"path": saved}}, {"command": "edit.undo"}]});
        assert!(c.check_command("engine.batch", &steps).is_ok());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn a_render_folder_with_a_link_out_of_the_write_roots_is_refused() {
        let base = std::env::temp_dir().join(format!("ec-confine-seq-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (read, write, outside) = (base.join("in"), base.join("out"), base.join("elsewhere"));
        for d in [&read, &write, &outside] {
            std::fs::create_dir_all(d).unwrap();
        }
        let c = Confinement::new(std::slice::from_ref(&read), std::slice::from_ref(&write)).unwrap();
        let template = write.join("frames_[#####].png").to_string_lossy().into_owned();
        let add = serde_json::json!({"output": template});
        let queue = serde_json::json!({"items": [{"outputPath": template}]});
        assert!(c.check_command("renderQueue.add", &add).is_ok());
        assert!(c.check_lookup(&Lookup::RenderOutputs, &queue).is_ok());
        // A link inside the write root is fine.
        std::os::unix::fs::symlink(write.join("kept.png"), write.join("frames_00002.png")).unwrap();
        std::fs::write(write.join("kept.png"), b"x").unwrap();
        assert!(c.check_lookup(&Lookup::RenderOutputs, &queue).is_ok());
        // Frame 1's name leads outside: rendering would truncate that file.
        std::os::unix::fs::symlink(outside.join("victim"), write.join("frames_00001.png")).unwrap();
        assert!(c.check_command("renderQueue.add", &add).is_err());
        assert!(c.check_command("render.preRender", &add).is_err());
        let e = c.check_lookup(&Lookup::RenderOutputs, &queue).unwrap_err().to_string();
        assert!(e.contains("frames_00001.png"), "{e}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn a_render_folder_that_cannot_be_listed_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("ec-confine-seq-nolist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let write = base.join("out");
        std::fs::create_dir_all(&write).unwrap();
        let c = Confinement::new(&[], std::slice::from_ref(&write)).unwrap();
        let template = write.join("frames_[#####].png").to_string_lossy().into_owned();
        let add = serde_json::json!({"output": template});
        // Writable and searchable but not listable: a link in it could not be seen.
        std::fs::set_permissions(&write, std::fs::Permissions::from_mode(0o300)).unwrap();
        // A user who can list it anyway (root) sees no difference to test.
        if std::fs::read_dir(&write).is_err() {
            assert!(c.check_command("renderQueue.add", &add).is_err());
        }
        std::fs::set_permissions(&write, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(c.check_command("renderQueue.add", &add).is_ok());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn numbered_render_folders_with_links_out_of_the_write_roots_are_refused() {
        let base = std::env::temp_dir().join(format!("ec-confine-seq-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (write, outside) = (base.join("out"), base.join("elsewhere"));
        for d in [&write, &outside] {
            std::fs::create_dir_all(d).unwrap();
        }
        let c = Confinement::new(&[], std::slice::from_ref(&write)).unwrap();
        let template = write.join("shot_[###]/f_[#####].png").to_string_lossy().into_owned();
        let add = serde_json::json!({"output": template});
        assert!(c.check_command("renderQueue.add", &add).is_ok(), "no numbered folder exists yet");
        // A numbered folder inside the roots holding a link out is refused.
        std::fs::create_dir(write.join("shot_001")).unwrap();
        assert!(c.check_command("renderQueue.add", &add).is_ok());
        std::os::unix::fs::symlink(outside.join("victim"), write.join("shot_001/f_00001.png")).unwrap();
        assert!(c.check_command("renderQueue.add", &add).is_err());
        std::fs::remove_file(write.join("shot_001/f_00001.png")).unwrap();
        // A numbered folder that is itself a link out is refused.
        std::os::unix::fs::symlink(&outside, write.join("shot_002")).unwrap();
        assert!(c.check_command("render.preRender", &add).is_err());
        // Unrelated names are left alone.
        std::fs::remove_file(write.join("shot_002")).unwrap();
        std::os::unix::fs::symlink(&outside, write.join("shot_notes")).unwrap();
        assert!(c.check_command("renderQueue.add", &add).is_ok());
        let _ = std::fs::remove_dir_all(&base);
    }
}
