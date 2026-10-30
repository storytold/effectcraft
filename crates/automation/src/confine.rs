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
//! folder. Commands that run code or open other applications (scripts, ScriptUI panels and the
//! events that reach them, Execute File, Reveal in Finder…) are refused while confined, as is
//! granting scripts the file and network access the user withheld.

use std::path::{Component, Path, PathBuf};

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
    "file.revealInFinder",
    "layer.openSource",
    "help.revealLogFile",
    "file.autoSave",
];

/// Commands that read files named by these parameters.
const READS: &[(&str, &[&str])] = &[
    ("file.open", &["path"]),
    ("file.openRecent", &["path"]),
    ("file.import", &["path", "paths"]),
    ("file.importMultiple", &["paths"]),
    ("file.importRecent", &["path"]),
    ("file.importLottie", &["path"]),
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
                    for p in std::iter::once(it.get("outputPath")).chain(extra).chain([it.get("logPath")]).flatten() {
                        if let Some(p) = p.as_str() {
                            self.check(p, Access::Write)?;
                        }
                    }
                }
                Ok(())
            }
        }
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
}
