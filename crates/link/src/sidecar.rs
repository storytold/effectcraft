//! The sidecar project of a link: `<link_id>.ecproj` next to `<link_id>.mov`.
//!
//! FilmCraft reopens a linked clip with `link.attach`. When EffectCraft has no comp bound to the
//! link, the attach loads this file. It must hold the latest edits, or the second hand-off shows
//! the comp as it was before the first one. So the live link writes it a short time after the
//! last change ([`SETTLE`]), the app writes it on close, and Back writes it after the render.
//!
//! The write follows the same rules as the result file (see `result::check_result_target`):
//!
//! - the name is `<link_id>.ecproj`, in the directory of the binding's `<link_id>.mov`;
//! - the directory must exist. Only `link.open` creates it;
//! - a sidecar that exists must be a regular file, never a symlink;
//! - the bytes go to `<link_id>.ecproj.part` (created new, never through a symlink) and then
//!   replace the sidecar with one rename, so a crash leaves the old sidecar or the new one.
//!
//! Writes are ordered. Each call takes a sequence number before it serializes, and a write whose
//! number is older than the last one on disk is dropped. A slow background write that started
//! before the close can never overwrite the close write.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use effectcraft_project::Project;

/// Quiet time after the last change before the sidecar is written.
pub const SETTLE: Duration = Duration::from_secs(2);

/// Why a sidecar was not written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarError {
    pub link_id: String,
    pub message: String,
}

impl std::fmt::Display for SidecarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "link {}: {}", self.link_id, self.message)
    }
}

impl std::error::Error for SidecarError {}

static SEQ: AtomicU64 = AtomicU64::new(0);
/// Sequence number of the newest write that reached the disk, per sidecar path.
static WRITTEN: Mutex<BTreeMap<PathBuf, u64>> = Mutex::new(BTreeMap::new());

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Take a place in the write order. Call this before the snapshot is serialized.
pub fn ticket() -> u64 {
    SEQ.fetch_add(1, Ordering::SeqCst) + 1
}

/// Write the sidecar of every link `project` binds. `ticket` comes from [`ticket`]. The project is
/// serialized once. One failed link does not stop the others. A write that a newer one already
/// replaced is skipped and counts as done.
pub fn write_all(project: &Project, ticket: u64) -> Vec<SidecarError> {
    if project.craft_links.is_empty() {
        return Vec::new();
    }
    let text = project.to_json();
    if text.is_empty() {
        let message = "the project could not be saved".to_string();
        return project.craft_links.keys().map(|link_id| SidecarError { link_id: link_id.clone(), message: message.clone() }).collect();
    }
    let mut written = lock(&WRITTEN);
    let mut errors = Vec::new();
    for (link_id, binding) in &project.craft_links {
        let outcome = target(link_id, &binding.result_path).and_then(|path| {
            if written.get(&path).is_some_and(|newest| *newest > ticket) {
                return Ok(());
            }
            write_atomic(&path, link_id, text.as_bytes())?;
            written.insert(path, ticket);
            Ok(())
        });
        if let Err(message) = outcome {
            errors.push(SidecarError { link_id: link_id.clone(), message });
        }
    }
    errors
}

/// The sidecar path for a link, or why this link may not write one.
fn target(link_id: &str, result_path: &str) -> Result<PathBuf, String> {
    let result = crate::result::check_result_name(link_id, result_path).map_err(|e| e.message)?;
    let Some(dir) = result.parent().filter(|dir| dir.is_dir()) else {
        return Err("the link directory does not exist; open the link again".to_string());
    };
    let sidecar = dir.join(format!("{link_id}.ecproj"));
    match std::fs::symlink_metadata(&sidecar) {
        Ok(meta) if !meta.is_file() => Err(format!("{} is not a regular file", sidecar.display())),
        _ => Ok(sidecar),
    }
}

/// Part file, then rename. The part is removed first, so a leftover (or a symlink planted at
/// that name) is replaced, never followed.
pub(crate) fn write_atomic(sidecar: &Path, link_id: &str, bytes: &[u8]) -> Result<(), String> {
    let Some(dir) = sidecar.parent() else {
        return Err("the sidecar path has no directory".to_string());
    };
    let part = dir.join(format!("{link_id}.ecproj.part"));
    match std::fs::remove_file(&part) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("could not clear {}: {e}", part.display())),
    }
    let written = std::fs::OpenOptions::new().write(true).create_new(true).open(&part).and_then(|mut file| {
        file.write_all(bytes)?;
        file.sync_data()
    });
    if let Err(e) = written {
        let _ = std::fs::remove_file(&part);
        return Err(format!("could not write {}: {e}", part.display()));
    }
    std::fs::rename(&part, sidecar).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        format!("could not replace {}: {e}", sidecar.display())
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Arc;

    use effectcraft_engine::Session;
    use effectcraft_engine::color::Label;
    use effectcraft_project::{Comp, CraftLinkBinding, ItemKind};
    use effectcraft_time::FrameRate;
    use serde_json::json;

    use super::*;

    const LINK: &str = "77777777-7777-4777-8777-777777777777";

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ec-sidecar-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn session(dir: &Path, comp_name: &str) -> Session {
        let mut session = Session::default();
        let rate = FrameRate::new(24, 1);
        let comp = session.project_mut().add_item(comp_name, Label::Sandstone, None, ItemKind::Comp(Arc::new(Comp::new(64, 36, rate, rate.tick_of(1)))));
        session.project_mut().craft_links.insert(
            LINK.into(),
            CraftLinkBinding {
                link_id: LINK.into(),
                comp,
                source: json!({"app": "filmcraft", "doc_id": "0", "item": "1"}),
                result_path: dir.join(format!("{LINK}.mov")).to_string_lossy().into_owned(),
                epoch: 0,
            },
        );
        session
    }

    fn names_in(path: &Path) -> Vec<String> {
        let project = Project::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
        project.comps().filter_map(|(id, _)| project.item(*id).map(|item| item.name.clone())).collect()
    }

    #[test]
    fn the_sidecar_is_replaced_atomically_and_leaves_no_part() {
        let dir = scratch("atomic");
        let sidecar = dir.join(format!("{LINK}.ecproj"));
        std::fs::write(&sidecar, b"old").unwrap();
        let session = session(&dir, "Fresh");
        assert!(write_all(&session.project, ticket()).is_empty());
        assert_eq!(names_in(&sidecar), ["Fresh"]);
        assert!(!dir.join(format!("{LINK}.ecproj.part")).exists(), "the part file stays after the rename");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_sidecar_is_refused_and_its_target_is_untouched() {
        let dir = scratch("symlink");
        let victim = dir.join("victim.txt");
        std::fs::write(&victim, b"keep").unwrap();
        std::os::unix::fs::symlink(&victim, dir.join(format!("{LINK}.ecproj"))).unwrap();
        let session = session(&dir, "Fresh");
        let errors = write_all(&session.project, ticket());
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_part_file_is_replaced_not_followed() {
        let dir = scratch("part-link");
        let victim = dir.join("victim.txt");
        std::fs::write(&victim, b"keep").unwrap();
        std::os::unix::fs::symlink(&victim, dir.join(format!("{LINK}.ecproj.part"))).unwrap();
        let session = session(&dir, "Fresh");
        assert!(write_all(&session.project, ticket()).is_empty());
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep");
        assert_eq!(names_in(&dir.join(format!("{LINK}.ecproj"))), ["Fresh"]);
    }

    #[test]
    fn a_missing_directory_is_never_created() {
        let dir = scratch("nodir").join("not-made");
        let session = session(&dir, "Fresh");
        let errors = write_all(&session.project, ticket());
        assert_eq!(errors.len(), 1);
        assert!(!dir.exists(), "the write created a directory it does not own");
    }

    #[test]
    fn a_result_path_with_another_name_writes_nothing() {
        let dir = scratch("name");
        let mut session = session(&dir, "Fresh");
        let bad = dir.join("elsewhere.mov").to_string_lossy().into_owned();
        session.project_mut().craft_links.get_mut(LINK).unwrap().result_path = bad;
        assert_eq!(write_all(&session.project, ticket()).len(), 1);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }

    #[test]
    fn an_older_write_never_replaces_a_newer_one() {
        let dir = scratch("order");
        let old = ticket();
        let new = ticket();
        let sidecar = dir.join(format!("{LINK}.ecproj"));
        assert!(write_all(&session(&dir, "Newer").project, new).is_empty());
        assert!(write_all(&session(&dir, "Older").project, old).is_empty());
        assert_eq!(names_in(&sidecar), ["Newer"]);
    }
}
