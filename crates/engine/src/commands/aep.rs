//! File ▸ Import ▸ After Effects Project… (`effectcraft-aep-import`).
//!
//! Adds the items of an `.aep` to the project in a folder named after the file. What the importer
//! cannot bring over (effects, masks, text, shapes, cameras, lights, markers, …) is returned in the
//! report; with losses and without `allowLosses` the import is refused and changes nothing.

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
    if dry_run {
        let report = effectcraft_aep_import::analyze(&bytes, &exists).map_err(|e| EngineError::Other(format!("cannot import {path}: {e}")))?;
        return Ok(json!({"dryRun": true, "report": report}));
    }
    let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "After Effects Project".into());
    let options = ImportOptions { allow_losses, exists: &exists, folder_name: &name };
    let res = s.edit("Import After Effects Project", None, |proj, st| {
        let r = effectcraft_aep_import::import(proj, &bytes, &options).map_err(|e| match e {
            AepImportError::Refused(_) => {
                EngineError::Other(format!("{e} (nothing was imported: pass `allowLosses` to import anyway, or `dryRun` for the full report)"))
            }
            other => EngineError::Other(format!("cannot import {path}: {other}")),
        })?;
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

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "file.importAep",
        "After Effects Project...",
        ["File", "Import"],
        None,
        "{path (.aep), allowLosses?: bool (import although parts cannot be imported), dryRun?: bool (only return the report)}",
        always,
        import_aep
    )]
}
