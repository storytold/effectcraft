//! File ▸ Import / Export ▸ After Effects Project… from the menu: what the conversion cannot
//! carry over is shown first (the command's dry run), and the user chooses to cancel or to go on
//! with those losses (`allowLosses`). Cancel changes nothing; a conversion without losses runs at
//! once. Agents running `file.importAep` / `file.exportAep` with `engine.execute` are never asked:
//! they pass `allowLosses` or get the refusal.
//!
//! Automation ids: `dialog.aepLosses.continue`, `dialog.aepLosses.cancel`.

use crate::i18n::tr;
use egui::vec2;
use serde_json::{Value, json};

use crate::theme::Tokens;
use crate::{Dialog, EffectcraftApp};

/// The conversion waiting on the prompt: its command, params and the dry run's report.
#[derive(Clone, Debug, Default)]
pub struct Pending {
    pub command: Option<(String, Value, Value)>,
}

/// How many detail lines of a loss category the prompt shows.
const DETAILS: usize = 3;

/// Whether command `id` with `params` needs the prompt first. When it does, the prompt opens
/// holding the command and the caller must not run it. `Err` when the dry run fails (the file
/// cannot be read or is not a project), with the message.
pub fn guard(app: &mut EffectcraftApp, id: &str, params: &Value) -> Result<bool, String> {
    if !matches!(id, "file.importAep" | "file.exportAep")
        || params.get("allowLosses") == Some(&Value::Bool(true))
        || params.get("dryRun") == Some(&Value::Bool(true))
    {
        return Ok(false);
    }
    let mut dry = params.as_object().cloned().unwrap_or_default();
    dry.insert("dryRun".into(), Value::Bool(true));
    let r = app.session.execute(id, Value::Object(dry)).map_err(|e| e.to_string())?;
    let report = r.get("report").cloned().unwrap_or(Value::Null);
    if report["losses"].as_array().is_none_or(|l| l.is_empty()) {
        return Ok(false);
    }
    app.dialog_state.aep_losses.command = Some((id.to_string(), params.clone(), report));
    app.dialog = Some(Dialog::AepLosses);
    Ok(true)
}

/// A loss category (`snake_case` in the report) as words: `layer_styles` → `Layer styles`.
fn category(c: &str) -> String {
    let words = c.replace('_', " ");
    let mut chars = words.chars();
    chars.next().map(|f| f.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// The report's losses as lines: one per category with its count, and a few of its details.
pub fn lines(report: &Value) -> Vec<(String, Vec<String>)> {
    let Some(losses) = report["losses"].as_array() else { return vec![] };
    losses
        .iter()
        .map(|l| {
            let head = format!("{} ({})", category(l["category"].as_str().unwrap_or("other")), l["count"].as_u64().unwrap_or(0));
            let details = l["details"].as_array().map(|d| d.iter().filter_map(Value::as_str).take(DETAILS).map(str::to_string).collect()).unwrap_or_default();
            (head, details)
        })
        .collect()
}

pub fn show(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let Some((id, _, report)) = app.dialog_state.aep_losses.command.clone() else {
        app.dialog = None;
        return;
    };
    let import = id == "file.importAep";
    let title = if import { "Import After Effects Project" } else { "Export After Effects Project" };
    let mut answer: Option<bool> = None;
    super::dialogs::modal(ctx, title, vec2(520.0, 340.0), t, |ui| {
        let text = if import {
            tr("Some parts of this project cannot be imported exactly (left out or changed):")
        } else {
            tr("Some parts of this project cannot be exported exactly (left out or changed):")
        };
        ui.label(egui::RichText::new(text).color(t.tab_text_active));
        ui.add_space(6.0);
        egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
            for (head, details) in lines(&report) {
                ui.label(egui::RichText::new(head).color(t.tab_text_active));
                for d in details {
                    ui.label(egui::RichText::new(format!("    {d}")).color(t.text_dim).size(11.0));
                }
            }
        });
        ui.add_space(14.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = if import { tr("Import Anyway") } else { tr("Export Anyway") };
            let r = ui.button(label);
            app.auto.add("dialog.aepLosses.continue", r.rect, label);
            if r.clicked() {
                answer = Some(true);
            }
            let r = ui.button(tr("Cancel"));
            app.auto.add("dialog.aepLosses.cancel", r.rect, "Cancel");
            if r.clicked() {
                answer = Some(false);
            }
        });
    });
    if answer.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        answer = Some(false);
    }
    if let Some(go) = answer {
        finish(app, ctx, go);
    }
}

/// The prompt's answer: Cancel (`false`) drops the conversion, Import / Export Anyway runs it
/// with `allowLosses`.
fn finish(app: &mut EffectcraftApp, ctx: &egui::Context, go: bool) {
    app.dialog = None;
    let Some((id, params, _)) = app.dialog_state.aep_losses.command.take() else { return };
    if go {
        // An error is already in the status bar.
        let _ = crate::menus::run_engine(app, ctx, &id, with_losses(&params));
    }
}

/// `params` with the user's consent to the losses the prompt listed.
fn with_losses(params: &Value) -> Value {
    let mut p = params.as_object().cloned().unwrap_or_default();
    p.insert("allowLosses".into(), json!(true));
    Value::Object(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_reads_as_categories_with_counts_and_details() {
        let report = json!({"losses": [{"category": "layer_styles", "count": 4, "details": ["a", "b", "c", "d"]}, {"category": "effects", "count": 1}]});
        let l = lines(&report);
        assert_eq!(l[0], ("Layer styles (4)".to_string(), vec!["a".to_string(), "b".into(), "c".into()]));
        assert_eq!(l[1], ("Effects (1)".to_string(), vec![]));
        assert!(lines(&json!({})).is_empty());
    }

    #[test]
    fn agents_and_consented_calls_are_not_asked() {
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        assert_eq!(guard(&mut app, "file.importAep", &json!({"path": "x.aep", "allowLosses": true})), Ok(false));
        assert_eq!(guard(&mut app, "file.importAep", &json!({"path": "x.aep", "dryRun": true})), Ok(false));
        assert_eq!(guard(&mut app, "file.open", &json!({"path": "x.aep"})), Ok(false));
        // `false` is no consent: the dry run still runs (and fails first here).
        assert!(guard(&mut app, "file.importAep", &json!({"path": "/no/such/file.aep", "allowLosses": false, "dryRun": false})).is_err());
        // A file that cannot be read is an error, not a prompt.
        assert!(guard(&mut app, "file.importAep", &json!({"path": "/no/such/file.aep"})).is_err());
        assert!(app.dialog.is_none());
    }

    #[test]
    fn cancel_changes_nothing_and_continue_exports_with_the_losses() {
        let ctx = egui::Context::default();
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        crate::theme::install(&ctx, &app.tokens, crate::i18n::language(&app));
        // Rec. 2020 has no After Effects counterpart: the export reports it.
        std::sync::Arc::make_mut(&mut app.session.project).settings.working_space = Some(effectcraft_engine::color::ColorSpace::Rec2020);
        let dir = TempDir::new();
        let path = dir.0.join("out.aep");
        let params = json!({"path": path.to_string_lossy()});
        assert_eq!(guard(&mut app, "file.exportAep", &params), Ok(true));
        assert_eq!(app.dialog, Some(Dialog::AepLosses));
        crate::panels::dialogs::press_key(&mut app, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
        assert!(app.dialog.is_none() && app.dialog_state.aep_losses.command.is_none());
        assert!(!path.exists(), "Cancel wrote nothing");
        assert_eq!(guard(&mut app, "file.exportAep", &params), Ok(true));
        finish(&mut app, &ctx, true);
        assert!(app.dialog.is_none() && path.is_file(), "Export Anyway wrote the file");
    }

    #[test]
    fn a_false_consent_still_asks() {
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        std::sync::Arc::make_mut(&mut app.session.project).settings.working_space = Some(effectcraft_engine::color::ColorSpace::Rec2020);
        let dir = TempDir::new();
        let params = json!({"path": dir.0.join("out.aep").to_string_lossy(), "allowLosses": false, "dryRun": false});
        assert_eq!(guard(&mut app, "file.exportAep", &params), Ok(true));
        assert_eq!(app.dialog, Some(Dialog::AepLosses));
    }

    /// A folder of its own for one test, removed afterwards.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new() -> Self {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("ec-aep-losses-{}-{nanos}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            assert!(std::fs::read_dir(&dir).unwrap().next().is_none(), "a fresh folder");
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_lossy_import_asks_and_import_anyway_imports() {
        // The combined effects project of the test corpus (opt-in, the folder named by
        // `EFFECTCRAFT_AEP_FIXTURES`) has effects outside the importer's table.
        let path = std::env::var_os("EFFECTCRAFT_AEP_FIXTURES").filter(|d| !d.is_empty()).map(|d| std::path::PathBuf::from(d).join("effects.aep"));
        let Some(path) = path.filter(|p| p.is_file()) else {
            println!("SKIPPED: fixture effects.aep missing");
            return;
        };
        let ctx = egui::Context::default();
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        crate::theme::install(&ctx, &app.tokens, crate::i18n::language(&app));
        let params = json!({"path": path.to_string_lossy()});
        assert_eq!(guard(&mut app, "file.importAep", &params), Ok(true));
        assert_eq!(app.dialog, Some(Dialog::AepLosses));
        assert!(app.session.project.items.is_empty(), "the dry run imported nothing");
        finish(&mut app, &ctx, true);
        assert!(app.dialog.is_none() && !app.session.project.items.is_empty(), "Import Anyway imported the project");
    }
}
