//! Script files a running script loads (`#include`, `$.evalFile`): where a name resolves, whether
//! the script may read it, and the limits that keep hostile, huge or cyclic files from running
//! away.

use std::path::{Path, PathBuf};

use effectcraft_engine::Session;

use crate::runtime::{GATE, normalize, project_dir};

/// How deep loaded files may nest (a loaded file loading another…).
pub(crate) const MAX_DEPTH: usize = 32;
/// The largest single file a script may load.
pub(crate) const MAX_FILE_BYTES: usize = 16 << 20;

/// The folder of the script file `name` runs from: `None` for the Script Console and for scripts
/// run by name or from code, which have no file.
pub(crate) fn script_dir(s: &Session, name: &str) -> Option<PathBuf> {
    let parent = Path::new(name).parent().filter(|p| !p.as_os_str().is_empty())?;
    s.services.exists(name).then(|| normalize(&parent.to_string_lossy()))
}

/// Resolve `name`: absolute paths as they are; relative ones against `dir` (the loading script's
/// folder), then each of `search`. The first existing file wins; when none exists, the first
/// candidate, so the read error names it.
pub(crate) fn resolve(s: &Session, name: &str, dir: Option<&Path>, search: &[PathBuf]) -> Result<PathBuf, String> {
    if Path::new(name).is_absolute() {
        return Ok(normalize(name));
    }
    let candidates: Vec<PathBuf> = dir.into_iter().chain(search.iter().map(PathBuf::as_path)).map(|d| normalize(&d.join(name).to_string_lossy())).collect();
    candidates
        .iter()
        .find(|c| s.services.exists(&c.to_string_lossy()))
        .or(candidates.first())
        .cloned()
        .ok_or_else(|| format!("cannot find \"{name}\": the script has no folder to look in (use an absolute path or #includepath)"))
}

/// The read gate for loaded script files: files under the running script's own folder (`root`) or
/// the project's folder; anywhere with the Allow Scripts to Write Files preference.
pub(crate) fn check_read(s: &Session, path: &Path, root: Option<&Path>) -> Result<(), String> {
    if s.prefs.scripting.allow_scripts_write_files {
        return Ok(());
    }
    let project = project_dir(s).map(|d| normalize(&d));
    if root.into_iter().chain(project.as_deref()).any(|d| path.starts_with(d)) {
        return Ok(());
    }
    Err(format!("Unable to read {}: scripts can only load files from their own or the project's folder unless {GATE} is on", path.display()))
}

/// Read a script file as text (a UTF-8 byte order mark dropped), refusing files over
/// [`MAX_FILE_BYTES`].
pub(crate) fn read(s: &Session, path: &Path) -> Result<String, String> {
    let too_big = || format!("Unable to read {}: script files are limited to {} MiB", path.display(), MAX_FILE_BYTES >> 20);
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_FILE_BYTES as u64) {
        return Err(too_big());
    }
    let bytes = s.services.read_file(&path.to_string_lossy()).map_err(|e| format!("Unable to read {}: {e}", path.display()))?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(too_big());
    }
    let text = String::from_utf8_lossy(&bytes);
    Ok(text.strip_prefix('\u{feff}').unwrap_or(&text).to_string())
}
