use super::{Dialog, EffectcraftApp};

impl EffectcraftApp {
    pub(crate) fn push_error_toast(&mut self, toast: crate::ErrorToast) {
        if self.error_toasts.len() >= crate::ErrorToast::MAX_PENDING {
            // Preserve the visible notification and summarize the pending backlog.
            let reports = self.error_toasts.drain(1..).fold(0usize, |sum, toast| sum.saturating_add(toast.reports));
            self.error_toasts.push_back(crate::ErrorToast::omitted(reports));
        }
        self.error_toasts.push_back(toast);
    }

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
            self.ui.status = crate::ErrorToast::bounded_text([self.ui.status.as_str()]);
            self.push_error_toast(crate::ErrorToast::new(errors.iter().copied(), Some(&self.ui.status)));
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
