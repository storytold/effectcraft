//! Android's app-private storage: `Projects/`, `Exports/` and `config/` below the
//! `AndroidApp::internal_data_path` directory.
//!
//! Android has no system file picker in v1 (rfd has no backend there), so saves, render outputs
//! and settings default to private storage: no permissions, no Storage Access Framework
//! round-trip, though invisible to the Files app (the in-app browser can additionally reach
//! shared storage once "All files access" is granted). Pure path logic, unit-tested on the host;
//! the handle comes from `run_android` (`apps/effectcraft/src/lib.rs`).

use std::path::{Path, PathBuf};

/// The storage folders below the app-private base directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Saved `.ecproj`/`.ecprojx` projects (the Open Project prompt lists these).
    pub projects: PathBuf,
    /// Render output, frame stills and other saved files.
    pub exports: PathBuf,
    /// Settings, auto-save and crash-recovery state (`session.config`).
    pub config: PathBuf,
}

/// Split the app-private `base` into its folders.
pub fn layout(base: &Path) -> Layout {
    Layout { projects: base.join("Projects"), exports: base.join("Exports"), config: base.join("config") }
}

/// Create the folders (`Err` names the first failure; the app then keeps that feature off
/// rather than crashing).
pub fn ensure(layout: &Layout) -> Result<(), String> {
    for dir in [&layout.projects, &layout.exports, &layout.config] {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.to_string_lossy()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_base(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("ec-android-storage-{tag}-{}", std::process::id()))
    }

    #[test]
    fn layout_splits_the_base_into_projects_exports_and_config() {
        let base = Path::new("/data/data/ai.storyteller.effectcraft/files");
        assert_eq!(layout(base), Layout { projects: base.join("Projects"), exports: base.join("Exports"), config: base.join("config") });
    }

    #[test]
    fn ensure_creates_every_folder() {
        let base = temp_base("ok");
        let _ = std::fs::remove_dir_all(&base);
        let l = layout(&base);
        ensure(&l).unwrap();
        assert!(l.projects.is_dir() && l.exports.is_dir() && l.config.is_dir());
        // Re-running over existing folders is fine.
        ensure(&l).unwrap();
        let _ = std::fs::remove_dir_all(&base);
    }
}
