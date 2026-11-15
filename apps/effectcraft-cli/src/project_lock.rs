//! One saving run at a time per project file.
//!
//! A `--save` run reads the project, edits it and writes the whole file back, so two at once would
//! silently drop the edits of whichever saved first (#2851). Each run holds an OS lock from before
//! its read until it exits; the next one waits, then reads what the first one saved. This is how
//! filmcraft-cli does it (storytold/filmcraft#3926).
//!
//! The lock is on a file next to the project, not on the project: a save replaces the project's
//! inode, and on Windows a locked file can't be read by other processes. The OS drops the lock when
//! the process ends, however it ends, so a crash leaves nothing stale. The file itself stays, since
//! removing it would let one run wait on the old file while another locks a new one.

use crate::{Args, Failure};
use std::fs::{File, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Most bytes of the project's file name kept in the lock's name; the rest of the 255-byte limit
/// is left for the leading dot and `.lock`.
const MAX_NAME_BYTES: usize = 200;

fn lock_path(project: &Path) -> PathBuf {
    let name = project.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    project.with_file_name(format!(".{}.lock", &name[..name.floor_char_boundary(MAX_NAME_BYTES)]))
}

/// Do two paths name one existing file?
fn same_file(a: &str, b: &str) -> bool {
    a == b || matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b)
}

/// The path this run saves over, when that is the file it opens. `--save-as` somewhere else replaces
/// that file whatever it held, so there is no edit of its to lose.
///
/// The lock goes by this path as written, not by where a symlink points: runs that name the
/// project the same way share one lock.
///
/// A missing project is an error here, before anything else, the same one opening it would give.
/// If the run went on, another one could create the project before it is read, and this run would
/// edit and save it with no lock. Checking first also means the paths compared below exist, so
/// `canonicalize` tells exactly whether they are one file, however they are spelt. A project that
/// exists but isn't a file gets no lock file beside it, and opening it reports why.
fn saved_in_place(a: &Args) -> Result<Option<&Path>, String> {
    // `--bridge` with a project is a usage error, which `backend` reports
    let Some(project) = a.project.as_deref().filter(|_| a.opt("--bridge").is_none()) else { return Ok(None) };
    match Path::new(project).metadata() {
        Ok(meta) if meta.is_file() => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(format!("cannot read {project}: {e}")),
        _ => return Ok(None),
    }
    match a.opt("--save-as") {
        Some(target) if same_file(target, project) => Ok(Some(Path::new(target))),
        None if a.flag("--save") => Ok(Some(Path::new(project))),
        _ => Ok(None),
    }
}

/// Lock `project`, calling `waiting` first if another run holds the lock.
fn lock(project: &Path, waiting: impl FnOnce()) -> std::io::Result<File> {
    let file = File::options().create(true).truncate(false).read(true).write(true).open(lock_path(project))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            waiting();
            file.lock()?;
        }
        Err(TryLockError::Error(e)) => return Err(e),
    }
    Ok(file)
}

/// Take the project's lock if this run saves over the file it opens, or fail if the project is
/// missing. Call it before opening the project and keep the result until the save is done. Blocks
/// while another run holds the lock, which is fine here: nothing else is running yet. A lock that
/// can't be had (a read-only folder, a file system without locks) is a warning and not an error, so
/// that saving works where it did.
pub(crate) fn for_save(a: &Args) -> Result<Option<File>, Failure> {
    let Some(project) = saved_in_place(a).map_err(Failure::Error)? else { return Ok(None) };
    let shown = project.display();
    let waiting = || {
        let _ = writeln!(std::io::stderr(), "effectcraft-cli: waiting for another run saving {shown}");
    };
    match lock(project, waiting) {
        Ok(file) => Ok(Some(file)),
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "effectcraft-cli: warning: can't lock {shown} ({e}); another run saving it now could overwrite these edits");
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Args {
        Args::parse(list.iter().map(|s| s.to_string()).collect()).unwrap()
    }

    #[test]
    fn the_lock_sits_next_to_the_project() {
        assert_eq!(lock_path(Path::new("/a/b/p.ecproj")), Path::new("/a/b/.p.ecproj.lock"));
        assert_eq!(lock_path(Path::new("p.ecproj")), Path::new(".p.ecproj.lock"));
    }

    #[test]
    fn a_long_project_name_still_gets_a_lock_name_the_file_system_takes() {
        for name in ["a".repeat(255), "é".repeat(127)] {
            let lock = lock_path(Path::new(&name));
            let lock = lock.to_str().unwrap();
            assert!(lock.len() <= 255, "{} bytes", lock.len());
            assert!(lock.ends_with(".lock"));
        }
    }

    /// The moment between this check and opening the project can't be hit on demand, so this pins
    /// the decision that closes it: a run never goes on to open a project that wasn't there, however
    /// its paths are spelt and whatever it saves.
    #[test]
    fn a_missing_project_stops_the_run_before_anything_opens_it() {
        let name = format!("ec-cli-gone-{}.ecproj", std::process::id());
        let gone = std::env::temp_dir().join(&name);
        let gone = gone.to_str().unwrap();
        let alias = std::env::temp_dir().join(".").join(&name);
        let alias = alias.to_str().unwrap();
        let dotted = format!("./{name}");
        // in a folder that doesn't exist either, so no part of either spelling can be resolved
        let new_dir = std::env::temp_dir().join(format!("ec-cli-new-dir-{}", std::process::id()));
        let in_new_dir = new_dir.join("p.ecproj");
        let in_new_dir = in_new_dir.to_str().unwrap();
        let in_new_dir_alias = new_dir.join(".").join("p.ecproj");
        let in_new_dir_alias = in_new_dir_alias.to_str().unwrap();
        for (project, list) in [
            (gone, &["exec", "--project", gone, "--save"][..]),
            (gone, &["exec", gone, "--save"]),
            (gone, &["exec", "--project", gone, "--save-as", gone]),
            (gone, &["exec", "--project", gone, "--save-as", alias]),
            (name.as_str(), &["exec", "--project", &name, "--save-as", &dotted]),
            (in_new_dir, &["exec", "--project", in_new_dir, "--save-as", in_new_dir_alias]),
            (gone, &["exec", "--project", gone, "--save-as", "other.ecproj"]),
            (gone, &["exec", "--project", gone]),
        ] {
            let error = saved_in_place(&args(list)).unwrap_err();
            assert_eq!(error.split_once(": ").map(|(head, _)| head), Some(format!("cannot read {project}").as_str()), "{list:?}: {error}");
        }
        // a project with --bridge is a usage error, which `backend` reports; no project, no lock
        assert_eq!(saved_in_place(&args(&["exec", "--bridge", "9877", "--project", gone, "--save"])), Ok(None));
        assert_eq!(saved_in_place(&args(&["exec", "--demo", "--save-as", gone])), Ok(None));
    }
}
