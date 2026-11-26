//! File ▸ Import / Export ▸ Adobe After Effects Project… from the menu: what the conversion cannot
//! carry over is shown first (the command's dry run), and the user chooses to cancel or to go on
//! with those losses (`allowLosses`). Cancel changes nothing; a conversion without losses runs at
//! once. Agents running `file.importAep` / `file.exportAep` with `engine.execute` are never asked:
//! they pass `allowLosses` or get the refusal. After Effects projects dropped or picked together
//! (File ▸ Import ▸ File…) are asked about one at a time: the others wait in the queue until the
//! prompt is answered, and Cancel skips only the one it asks about.
//!
//! Automation ids: `dialog.aepLosses.continue`, `dialog.aepLosses.cancel`.

use crate::i18n::{tr, tr_args};
use egui::vec2;
use serde_json::{Value, json};

use crate::theme::Tokens;
use crate::{Dialog, EffectcraftApp};

/// The conversion waiting on the prompt: its command, params and the dry run's report.
#[derive(Clone, Debug, Default)]
pub struct Pending {
    pub command: Option<(String, Value, Value)>,
    /// After Effects projects dropped or picked with it, imported once it is answered.
    pub queue: Vec<String>,
}

/// Whether the prompt is showing with a conversion waiting on it.
pub fn asking(app: &EffectcraftApp) -> bool {
    app.dialog == Some(Dialog::AepLosses) && app.dialog_state.aep_losses.command.is_some()
}

/// The projects of a prompt another dialog replaced (Save changes? on quitting) before it was
/// answered: its import's file, then its queue. Taken, so they are tried again instead of waiting
/// for an answer that never comes (a replaced export is dropped: it wrote nothing).
pub fn take_unanswered(app: &mut EffectcraftApp) -> Vec<String> {
    if app.dialog == Some(Dialog::AepLosses) {
        return Vec::new();
    }
    let losses = &mut app.dialog_state.aep_losses;
    let Some((id, params, _)) = losses.command.take() else { return std::mem::take(&mut losses.queue) };
    let mut paths: Vec<String> = params.get("path").and_then(Value::as_str).filter(|_| id == "file.importAep").map(str::to_string).into_iter().collect();
    paths.append(&mut losses.queue);
    paths
}

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
    // An unanswered prompt's projects wait behind this one.
    let unanswered = take_unanswered(app);
    app.dialog_state.aep_losses.queue.extend(unanswered);
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

/// The report's losses as lines: one per category with how many occurrences are left out or
/// changed, then every detail line the report keeps and "… and N more" for the occurrences none
/// describes (#3365).
pub fn lines(report: &Value) -> Vec<(String, Vec<String>)> {
    let Some(losses) = report["losses"].as_array() else { return vec![] };
    losses
        .iter()
        .map(|l| {
            let count = l["count"].as_u64().unwrap_or(0);
            let head = tr_args("{}: {} left out or changed", &[&category(l["category"].as_str().unwrap_or("other")), &count]);
            let mut details: Vec<String> =
                l["details"].as_array().map(|d| d.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
            // An export report has no `unlisted`: each of its lines is one occurrence.
            let listed = u64::try_from(details.len()).unwrap_or(u64::MAX);
            let unlisted = l["unlisted"].as_u64().unwrap_or_else(|| count.saturating_sub(listed));
            if unlisted > 0 {
                details.push(tr_args("… and {} more", &[&unlisted]));
            }
            (head, details)
        })
        .collect()
}

/// Draw the prompt for the conversion waiting on it: its losses by category, Cancel and Import /
/// Export Anyway (Escape cancels).
pub fn show(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let Some((id, _, report)) = app.dialog_state.aep_losses.command.clone() else {
        app.dialog = None;
        return;
    };
    let import = id == "file.importAep";
    let title = if import { "Import Adobe After Effects Project" } else { "Export Adobe After Effects Project" };
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
    let Some((id, params, _)) = app.dialog_state.aep_losses.command.take() else {
        app.dialog = None;
        return;
    };
    let mut errors = Vec::new();
    if go {
        // The prompt stays up while the conversion runs: a question queued by a footage import
        // that finished meanwhile (Interpret Footage for unlabeled alpha, drained by `run_engine`)
        // leaves a note instead of opening a dialog the next queued prompt would replace.
        if let Err(e) = crate::menus::run_engine(app, ctx, &id, with_losses(&params)) {
            errors.push(e);
        }
    }
    app.dialog = None;
    // The projects dropped or picked with this one: Cancel skips only this one.
    let queue = std::mem::take(&mut app.dialog_state.aep_losses.queue);
    errors.extend(crate::menus::import_aep_projects(app, queue));
    if !errors.is_empty() {
        app.ui.status = errors.join("; ");
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
        assert_eq!(l[0], ("Layer styles: 4 left out or changed".to_string(), vec!["a".to_string(), "b".into(), "c".into(), "d".into()]));
        assert_eq!(l[1], ("Effects: 1 left out or changed".to_string(), vec!["… and 1 more".to_string()]));
        assert!(lines(&json!({})).is_empty());
    }

    /// #3365: every detail line shows, then how many occurrences have none: the report's
    /// `unlisted` (an effect line can describe many instances), else the lines past the count.
    #[test]
    fn every_detail_shows_and_the_rest_is_counted() {
        let details: Vec<String> = (0..20).map(|i| format!("d{i}")).collect();
        let l = lines(&json!({"losses": [{"category": "masks", "count": 27, "details": details}]}));
        assert_eq!(l[0].1.len(), 21);
        assert_eq!(l[0].1[20], "… and 7 more");
        let l = lines(&json!({"losses": [{"category": "effects", "count": 50, "details": ["X, and 44 more like it"], "unlisted": 5}]}));
        assert_eq!(l[0].1, ["X, and 44 more like it", "… and 5 more"]);
        let l = lines(&json!({"losses": [{"category": "effects", "count": 45, "details": ["X, and 44 more like it"], "unlisted": 0}]}));
        assert_eq!(l[0].1, ["X, and 44 more like it"]);
    }

    /// Projects dropped or picked together wait only behind a prompt that shows; with none
    /// showing they are tried at once (a missing file is an error naming it).
    #[test]
    fn projects_wait_only_behind_a_showing_prompt() {
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        app.dialog_state.aep_losses.command = Some(("file.importAep".into(), json!({"path": "/a/One.aep"}), json!({})));
        app.dialog = Some(Dialog::AepLosses);
        assert!(crate::menus::import_aep_projects(&mut app, vec!["/no/such/Two.aep".into(), "/no/such/Three.aep".into()]).is_empty());
        assert_eq!(app.dialog_state.aep_losses.queue, ["/no/such/Two.aep", "/no/such/Three.aep"]);

        // Cancel: the queued ones are tried next (both missing here, so both are reported).
        let ctx = egui::Context::default();
        finish(&mut app, &ctx, false);
        assert!(app.dialog_state.aep_losses.queue.is_empty());
        assert!(app.ui.status.contains("Two.aep") && app.ui.status.contains("Three.aep"), "{}", app.ui.status);
    }

    /// A prompt another dialog replaced (Save changes? on quitting) holds nothing back: the next
    /// import tries its project and queue first, then the new one.
    #[test]
    fn a_replaced_prompt_holds_nothing_back() {
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        app.dialog_state.aep_losses.command = Some(("file.importAep".into(), json!({"path": "/no/such/One.aep"}), json!({})));
        app.dialog_state.aep_losses.queue = vec!["/no/such/Two.aep".into()];
        app.dialog = Some(Dialog::UnsavedChanges);
        let errors = crate::menus::import_aep_projects(&mut app, vec!["/no/such/Three.aep".into()]);
        let named = |n: &str| errors.iter().position(|e| e.contains(n));
        assert_eq!((named("One.aep"), named("Two.aep"), named("Three.aep")), (Some(0), Some(1), Some(2)), "{errors:?}");
        assert!(app.dialog_state.aep_losses.command.is_none() && app.dialog_state.aep_losses.queue.is_empty());
        assert_eq!(app.dialog, Some(Dialog::UnsavedChanges), "the other dialog stays");
    }

    /// A new prompt keeps an unanswered one's projects: they wait behind it.
    #[test]
    fn a_new_prompt_keeps_an_unanswered_ones_projects() {
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        app.dialog_state.aep_losses.command = Some(("file.importAep".into(), json!({"path": "/a/One.aep"}), json!({})));
        app.dialog_state.aep_losses.queue = vec!["/a/Two.aep".into()];
        app.dialog = Some(Dialog::UnsavedChanges);
        // Rec. 2020 has no After Effects counterpart: exporting asks.
        std::sync::Arc::make_mut(&mut app.session.project).settings.working_space = Some(effectcraft_engine::color::ColorSpace::Rec2020);
        let dir = TempDir::new();
        assert_eq!(guard(&mut app, "file.exportAep", &json!({"path": dir.0.join("out.aep").to_string_lossy()})), Ok(true));
        assert_eq!(app.dialog, Some(Dialog::AepLosses));
        assert_eq!(app.dialog_state.aep_losses.queue, ["/a/One.aep", "/a/Two.aep"]);
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
            assert!(std::env::var_os("EFFECTCRAFT_AEP_FIXTURES_REQUIRED").is_none_or(|v| v.is_empty()), "fixture effects.aep missing");
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
