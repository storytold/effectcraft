use super::{Dialog, EffectcraftApp};

impl EffectcraftApp {
    /// Open the project at `path`: through [`crate::Hooks::open_project`] when the host has one (it
    /// opens the project itself, later), else with the `file.open` command. Every open by path
    /// goes through here or through `file.open` / `file.openRecent`, which come here.
    pub fn open_project_path(&mut self, path: &str) -> Result<serde_json::Value, String> {
        if let Some(open) = self.hooks.open_project.as_ref() {
            open(path);
            return Ok(serde_json::json!({"path": path, "pending": true}));
        }
        let r = self.session.execute("file.open", serde_json::json!({"path": path})).map_err(|e| e.to_string());
        if let Err(e) = &r {
            self.ui.status = e.clone();
        }
        r
    }

    /// Show the crash-recovery dialog for a previous run that didn't exit cleanly.
    pub fn offer_recovery(&mut self, r: effectcraft_engine::autosave::Recovery) {
        if r.autosave.is_some() {
            self.recovery = Some(r);
            self.dialog = Some(Dialog::Recovery);
        }
    }
}
