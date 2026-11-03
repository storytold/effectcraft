use super::{Dialog, EffectcraftApp};

impl EffectcraftApp {
    /// Imports can succeed as commands while individual files fail before a job starts.
    pub(crate) fn report_import_errors(&mut self, command: &str, result: &serde_json::Value) {
        if !matches!(command, "file.import" | "file.importMultiple") {
            return;
        }
        let errors: Vec<&str> = result["errors"].as_array().into_iter().flatten().filter_map(serde_json::Value::as_str).collect();
        match errors.as_slice() {
            [] => {}
            [one] => self.ui.status = (*one).into(),
            [first, rest @ ..] => self.ui.status = format!("{first} (and {} more)", rest.len()),
        }
        if !errors.is_empty() {
            self.error_toast = Some(crate::ErrorToast::new(errors.join("\n"), true));
        }
    }

    /// Show the crash-recovery dialog for a previous run that didn't exit cleanly.
    pub fn offer_recovery(&mut self, r: effectcraft_engine::autosave::Recovery) {
        if r.autosave.is_some() {
            self.recovery = Some(r);
            self.dialog = Some(Dialog::Recovery);
        }
    }
}
