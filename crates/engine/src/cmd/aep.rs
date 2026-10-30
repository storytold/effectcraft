//! File ▸ Import ▸ After Effects Project… (`effectcraft_aep::import`).
//!
//! Adds the items of an `.aep` (or its XML form, `.aepx`) to the project in a folder named after
//! the file (`docs/aep-import.md` lists what arrives). What the importer cannot bring over (effects
//! outside its table, undecoded text and style settings, …) is returned in the report; with losses and without
//! `allowLosses` the import is refused and changes nothing. Fonts that are not installed are listed
//! in the report's `missingFonts` (not a loss: the text keeps the font name and is drawn with the
//! fallback font).

use effectcraft_aep::import::{AepImportError, ImportOptions};
use effectcraft_project::{FootageKind, ItemKind};
use serde_json::{Value, json};

use super::{CommandSpec, always, b_p, bad, str_p};
use crate::{EngineError, Result, Session, cmd};

fn import_aep(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("file.importAep", "missing `path`"))?.to_string();
    let allow_losses = b_p(p, "allowLosses").unwrap_or(false);
    let dry_run = b_p(p, "dryRun").unwrap_or(false);
    let bytes = s.services.read_file(&path).map_err(|e| EngineError::Other(format!("cannot read {path}: {e}")))?;
    // Footage paths stored in the file reach the file system only through the document-path policy
    // (`doc_path`): relative ones are taken from the file's folder, paths to other computers are
    // never looked at (the footage arrives missing).
    let services = s.services.clone();
    let doc = path.clone();
    let exists = move |f: &str| crate::doc_path::resolve(f, Some(&doc)).ok().and_then(|p| p.to_str().map(|p| services.exists(p))).unwrap_or(false);
    // The renderer's own font lookup (family, full or PostScript name); a miss falls back.
    let has_font = |font: &str| !effectcraft_text::resolve(font, "Regular").missing;
    let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "After Effects Project".into());
    // Into a project without compositions the file's settings come along (it is opened as a
    // project); into one with compositions only when asked (`applySettings`). Settings that are not
    // taken over cannot be lost, so the dry run and the refusal leave them out.
    let apply = b_p(p, "applySettings").unwrap_or_else(|| s.project.comps().next().is_none());
    let options = ImportOptions { allow_losses, exists: &exists, has_font: &has_font, folder_name: &name, settings: apply };
    if dry_run {
        let report = effectcraft_aep::import::analyze_with(&bytes, &options).map_err(|e| EngineError::Other(format!("cannot import {path}: {e}")))?;
        return Ok(json!({"dryRun": true, "report": report}));
    }
    let importer = s.importer.clone();
    let res = s.edit("Import After Effects Project", None, |proj, st| {
        let r = effectcraft_aep::import::import(proj, &bytes, &options).map_err(|e| match e {
            AepImportError::Refused(_) => {
                EngineError::Other(format!("{e} (nothing was imported: pass `allowLosses` to import anyway, or `dryRun` for the full report)"))
            }
            other => EngineError::Other(format!("cannot import {path}: {other}")),
        })?;
        if apply {
            proj.settings = r.settings.clone();
        }
        // The new footage keeps the file as its origin: resolved against it, a refused path missing.
        let created: std::collections::HashSet<_> = r.items.iter().copied().collect();
        crate::doc_path::resolve_footage_of(proj, |id| created.contains(&id), Some(&path));
        // A video file that is there says whether it carries sound, and it is right when the
        // project file's stored sample rate is not (the file changed since, or the project is not
        // After Effects's own). The path is the one the document-path policy resolved above; a
        // refused or missing one is not read and keeps what the project file says.
        if let Some(importer) = &importer {
            for item in proj.items.values_mut().filter(|i| created.contains(&i.id)) {
                if let ItemKind::Footage(f) = &mut item.kind
                    && f.kind == FootageKind::Video
                    && !f.missing
                    && let Ok(probed) = importer.probe(&f.path)
                {
                    f.has_audio = probed.has_audio;
                }
            }
        }
        st.project_selection = vec![r.folder];
        Ok(r)
    })?;
    let created: std::collections::HashSet<_> = res.items.iter().copied().collect();
    s.stored_paths.add_items(&s.project, |id| created.contains(&id), Some(&path));
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

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "file.importAep",
        "After Effects Project...",
        ["File", "Import"],
        None,
        "{path (.aep or .aepx), allowLosses?: bool (import although parts cannot be imported), dryRun?: bool (only return the report), applySettings?: bool (take over the file's project settings; default: when the project has no compositions)}",
        always,
        import_aep
    )]
}
