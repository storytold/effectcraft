//! File access confinement for automation (`effectcraft-cli mcp --automation-read-root DIR
//! --automation-write-root DIR`, as in PhotoCraft): an agent may only read files under a read
//! root (or a write root) and only write under a write root.
//!
//! Every engine command an agent runs ([`crate::Backend::exec`], also each step of an
//! `engine.batch`) is checked before it runs: its file parameters must be absolute paths inside
//! the roots (symbolic links resolved, no `..`), any other absolute path in its parameters too,
//! and commands that write to a file they are not given (Save, Increment and Save, rendering the
//! Render Queue) are checked against where they would write. Commands that run code or open
//! other applications (scripts, Execute File, Reveal in Finder…) are refused while confined.

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

/// Commands that run code, act outside the project or open other applications.
const REFUSED: &[&str] = &[
    // Bitmap assets can follow a sibling-directory symlink outside the project write root.
    "edit.pasteFigCraft",
    "script.run",
    "file.runScript",
    "file.installScript",
    "file.installScriptUIPanel",
    "file.uninstallScript",
    "file.executeFile",
    "file.revealInFinder",
    "layer.openSource",
    "help.revealLogFile",
    "file.autoSave",
    // Stores a Figma access token in the settings folder.
    "figma.setToken",
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
    ("file.importFigma", &["payloadPath"]),
];

/// Commands that write files named by these parameters.
const WRITES: &[(&str, &[&str])] = &[
    ("file.save", &["path"]),
    ("file.saveAs", &["path"]),
    ("file.saveCopy", &["path"]),
    ("file.saveCopyAsXml", &["path"]),
    ("file.collectFiles", &["folder"]),
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
];

/// Commands that write where they are not told: the project's file, the Render Queue's outputs.
/// (`file.importFigma` writes its rendered images to a folder beside the project's file.)
const IMPLICIT_WRITES: &[&str] = &["file.save", "file.incrementAndSave", "file.importFigma", "renderQueue.render", "render.preRender"];

/// What must be looked up before an implicitly writing command may run.
#[derive(Debug, PartialEq, Eq)]
pub enum Lookup {
    /// The open project's file (`project.summary` → `path`).
    ProjectFile,
    /// Every planned image destination (`file.importFigma` with `dryRun`).
    FigmaAssets,
    /// Every output of the queued Render Queue items (`renderQueue.list`).
    RenderOutputs,
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
    pub fn check_command(&self, id: &str, params: &Value) -> Result<Option<Lookup>> {
        if REFUSED.contains(&id) {
            return Err(Error::BadArgs(format!("`{id}` is not available while file access is confined (--automation-read-root/--automation-write-root)")));
        }
        if id == "engine.batch" {
            for step in params.get("steps").and_then(Value::as_array).into_iter().flatten() {
                let Some(inner) = step.get("command").or_else(|| step.get("id")).and_then(Value::as_str) else { continue };
                let inner_params = step.get("params").cloned().unwrap_or(Value::Null);
                let named = keys_of(READS, inner).or_else(|| keys_of(WRITES, inner)).unwrap_or(&[]);
                // A step's `$N` reference is resolved only as the batch runs: never for a file.
                if named.iter().filter_map(|k| inner_params.get(*k)).any(has_reference) {
                    return Err(Error::BadArgs(format!("`{inner}` in a batch: file paths can't be `$N` references while file access is confined")));
                }
                if self.check_command(inner, &inner_params)?.is_some() {
                    return Err(Error::BadArgs(format!("`{inner}` writes to its project's or queue's files: run it on its own while file access is confined")));
                }
            }
            return Ok(None);
        }
        let (named, access) = match (keys_of(WRITES, id), keys_of(READS, id)) {
            (Some(k), _) => (k, Access::Write),
            (None, Some(k)) => (k, Access::Read),
            (None, None) if id.starts_with("renderQueue.") || id == "prefs.set" => (&[][..], Access::Write),
            (None, None) => (&[][..], Access::Read),
        };
        let mut given = false;
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
                    Some(s) if id == "render.preRender" && !looks_like_path(s) => {}
                    Some(s) => {
                        self.check(s, access)?;
                    }
                    None => return Err(Error::BadArgs(format!("`{id}`: `{key}` must be a path"))),
                }
            }
        }
        // Any other absolute path the command is given.
        let mut other = vec![];
        absolute_strings(params, named, &mut other);
        for s in other {
            self.check(&s, access)?;
        }
        // An entry of a recent list by index is a path the agent didn't give.
        if matches!(id, "file.openRecent" | "file.importRecent" | "anim.applyRecentPreset") && !given {
            return Err(Error::BadArgs(format!("`{id}`: give `path` while file access is confined")));
        }
        if IMPLICIT_WRITES.contains(&id) && !(given && id == "file.save") {
            if id == "file.importFigma" {
                return Ok(Some(Lookup::FigmaAssets));
            }
            let project = matches!(id, "file.save" | "file.incrementAndSave");
            return Ok(Some(if project { Lookup::ProjectFile } else { Lookup::RenderOutputs }));
        }
        Ok(None)
    }

    /// Check what a [`Lookup`] found: `project.summary` or `renderQueue.list`'s result.
    pub fn check_lookup(&self, lookup: &Lookup, found: &Value) -> Result<()> {
        match lookup {
            Lookup::ProjectFile => match found.get("path").and_then(Value::as_str) {
                // Increment and Save writes beside it.
                Some(p) => self.check(p, Access::Write).map(drop),
                None => Err(Error::BadArgs("the project has no file yet: give `path` (Save As) inside an automation write root".into())),
            },
            Lookup::FigmaAssets => {
                let paths = found
                    .get("assetPaths")
                    .and_then(Value::as_array)
                    .ok_or_else(|| Error::BadArgs("Figma import plan did not report its asset paths; update the connected app".into()))?;
                for path in paths {
                    let path = path.as_str().ok_or_else(|| Error::BadArgs("Figma asset path must be a string".into()))?;
                    self.check(path, Access::Write)?;
                }
                Ok(())
            }
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
        // A dangling link or unreadable entry exists even when canonicalize fails.
        // Never treat it as a new file beneath an otherwise allowed parent.
        match at.symlink_metadata() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            _ => return None,
        }
        rest.push(at.file_name()?.to_os_string());
        at = at.parent()?;
    }
}

#[cfg(test)]
mod figma_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn figma_checks_actual_asset_destinations_not_just_the_project() {
        let base = std::env::temp_dir().join(format!("ec-figma-confinement-{}", std::process::id()));
        let write = base.join("write");
        let outside = base.join("outside");
        std::fs::create_dir_all(&write).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let confinement = Confinement::new(&[], std::slice::from_ref(&write)).unwrap();
        let lookup = confinement.check_command("file.importFigma", &json!({})).unwrap().unwrap();
        let plan = |path: PathBuf| json!({"path": write.join("Project.ecproj"), "assetPaths": [path]});
        assert!(confinement.check_lookup(&lookup, &plan(write.join("Project Figma Assets/import/image.png"))).is_ok());
        assert!(confinement.check_lookup(&lookup, &plan(outside.join("image.png"))).is_err());
        assert!(confinement.check_lookup(&lookup, &json!({"path": write.join("Project.ecproj")})).is_err());
        assert!(confinement.check_lookup(&lookup, &json!({"assetPaths": []})).is_ok());
        #[cfg(unix)]
        {
            let assets = write.join("Project Figma Assets");
            std::os::unix::fs::symlink(&outside, &assets).unwrap();
            assert!(confinement.check_lookup(&lookup, &plan(assets.join("import/image.png"))).is_err());
            std::fs::remove_file(&assets).unwrap();
            std::fs::create_dir_all(assets.join("import")).unwrap();
            std::os::unix::fs::symlink(outside.join("target.png"), assets.join("import/image.png")).unwrap();
            assert!(confinement.check_lookup(&lookup, &plan(assets.join("import/image.png"))).is_err());
        }
        std::fs::remove_dir_all(base).unwrap();
    }
}

#[cfg(all(test, unix))]
mod clipboard_tests {
    use super::*;

    #[test]
    fn clipboard_paste_cannot_follow_an_asset_directory_outside_write_roots() {
        let base = std::env::temp_dir().join(format!("ec-clipboard-confinement-{}", std::process::id()));
        let (allowed, outside) = (base.join("allowed"), base.join("outside"));
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let assets = allowed.join("Project Figma Assets");
        std::os::unix::fs::symlink(&outside, &assets).unwrap();
        let policy = Confinement::new(&[], std::slice::from_ref(&allowed)).unwrap();
        let project = allowed.join("Project.ecproj");
        assert!(policy.check_lookup(&Lookup::ProjectFile, &serde_json::json!({"path": project})).is_ok());
        assert!(policy.check(assets.to_str().unwrap(), Access::Write).is_err());
        let error = policy.check_command("edit.pasteFigCraft", &serde_json::json!({"clipboard":"{}"})).unwrap_err();
        assert!(error.to_string().contains("not available while file access is confined"));
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        std::fs::remove_dir_all(base).unwrap();
    }
}
