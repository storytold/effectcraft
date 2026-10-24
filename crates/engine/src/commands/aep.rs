//! File ▸ Import ▸ After Effects Project… (`effectcraft-aep-import`) and File ▸ Export ▸ After Effects
//! Project… (`effectcraft-aep-export`).
//!
//! Adds the items of an `.aep` (or its XML form, `.aepx`) to the project in a folder named after
//! the file (`docs/aep-import.md` lists what arrives). What the importer cannot bring over (effects
//! outside its table, undecoded text and style settings, …) is returned in the report; with losses and without
//! `allowLosses` the import is refused and changes nothing. Fonts that are not installed are listed
//! in the report's `missingFonts` (not a loss: the text keeps the font name and is drawn with the
//! fallback font).

use effectcraft_aep_export::{AepExportError, ExportOptions};
use effectcraft_aep_import::{AepImportError, ImportOptions};
use serde_json::{Value, json};

use super::{CommandSpec, always, b_p, bad, str_p};
use crate::{EngineError, Result, Session, cmd};

fn import_aep(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("file.importAep", "missing `path`"))?.to_string();
    let allow_losses = b_p(p, "allowLosses").unwrap_or(false);
    let dry_run = b_p(p, "dryRun").unwrap_or(false);
    let bytes = s.services.read_file(&path).map_err(|e| EngineError::Other(format!("cannot read {path}: {e}")))?;
    let services = s.services.clone();
    let exists = move |f: &str| services.exists(f);
    // The renderer's own font lookup (family, full or PostScript name); a miss falls back.
    let has_font = |font: &str| !effectcraft_text::resolve(font, "Regular").missing;
    let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "After Effects Project".into());
    let options = ImportOptions { allow_losses, exists: &exists, has_font: &has_font, folder_name: &name };
    if dry_run {
        let report = effectcraft_aep_import::analyze_with(&bytes, &options).map_err(|e| EngineError::Other(format!("cannot import {path}: {e}")))?;
        return Ok(json!({"dryRun": true, "report": report}));
    }
    let apply_settings = b_p(p, "applySettings");
    let res = s.edit("Import After Effects Project", None, |proj, st| {
        // Into a project without compositions the file's settings come along (it is opened as a
        // project); into one with compositions only when asked (`applySettings`).
        let apply = apply_settings.unwrap_or_else(|| proj.comps().next().is_none());
        let r = effectcraft_aep_import::import(proj, &bytes, &options).map_err(|e| match e {
            AepImportError::Refused(_) => {
                EngineError::Other(format!("{e} (nothing was imported: pass `allowLosses` to import anyway, or `dryRun` for the full report)"))
            }
            other => EngineError::Other(format!("cannot import {path}: {other}")),
        })?;
        if apply {
            proj.settings = r.settings.clone();
        }
        st.project_selection = vec![r.folder];
        Ok(r)
    })?;
    if let Some(first) = res.comps.first() {
        s.open_comp(*first);
    }
    let losses = res.report.total_losses();
    s.toast(if losses == 0 {
        format!("Imported {name}")
    } else {
        format!("Imported {name} ({losses} unsupported item{} not imported)", if losses == 1 { "" } else { "s" })
    });
    Ok(json!({
        "folder": res.folder.0,
        "comps": res.comps.iter().map(|c| c.0).collect::<Vec<_>>(),
        "items": res.items.iter().map(|i| i.0).collect::<Vec<_>>(),
        "report": res.report,
    }))
}

fn export_aep(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("file.exportAep", "missing `path`"))?.to_string();
    let allow_losses = b_p(p, "allowLosses").unwrap_or(false);
    let dry_run = b_p(p, "dryRun").unwrap_or(false);
    if dry_run {
        return Ok(json!({"dryRun": true, "report": effectcraft_aep_export::analyze(&s.project)}));
    }
    // Image-sequence folders keep their stored numbering in After Effects only with their actual
    // modification time (otherwise it rescans them and warns about every gap on open).
    let services = s.services.clone();
    let folder_time = move |dir: &str| services.stamp(dir).and_then(|(_, ns)| u64::try_from(ns / 1_000_000_000).ok());
    let res = effectcraft_aep_export::export_with(&s.project, &ExportOptions { allow_losses }, &folder_time).map_err(|e| match e {
        AepExportError::Refused(_) => {
            EngineError::Other(format!("{e} (nothing was written: pass `allowLosses` to export anyway, or `dryRun` for the full report)"))
        }
        other => EngineError::Other(format!("cannot export {path}: {other}")),
    })?;
    s.services.write_file(&path, &res.bytes).map_err(|e| EngineError::Other(format!("cannot write {path}: {e}")))?;
    let losses = res.report.total_losses();
    s.toast(if losses == 0 {
        format!("Exported After Effects project to {path}")
    } else {
        format!("Exported After Effects project to {path} ({losses} unsupported item{} not exported)", if losses == 1 { "" } else { "s" })
    });
    Ok(json!({"path": path, "bytes": res.bytes.len(), "report": res.report}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "file.importAep",
            "After Effects Project...",
            ["File", "Import"],
            None,
            "{path (.aep or .aepx), allowLosses?: bool (import although parts cannot be imported), dryRun?: bool (only return the report), applySettings?: bool (take over the file's project settings; default: when the project has no compositions)}",
            always,
            import_aep
        ),
        cmd!(
            "file.exportAep",
            "After Effects Project...",
            ["File", "Export"],
            None,
            "{path (.aep), allowLosses?: bool (export although parts cannot be exported), dryRun?: bool (only return the report)}",
            always,
            export_aep
        ),
    ]
}
