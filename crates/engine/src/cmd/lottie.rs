//! File ▸ Export ▸ Lottie JSON… and File ▸ Import ▸ Lottie… (`effectcraft-lottie`).

use effectcraft_lottie::{ExportOptions, ImportedImage};
use serde_json::{Value, json};

use super::{CommandSpec, always, b_p, bad, comp_id, has_comp, str_p};
use crate::{EngineError, Result, Session, cmd};

fn export_lottie(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("file.exportLottie", "missing `path`"))?.to_string();
    let cid = comp_id(s, p)?;
    let opts = ExportOptions { include_expressions: b_p(p, "includeExpressions").unwrap_or(false), text_as_shapes: b_p(p, "textAsShapes").unwrap_or(false) };
    let services = s.services.clone();
    let read = move |f: &str| services.read_file(f).ok();
    let res = effectcraft_lottie::export_comp(&s.project, cid, &opts, &read).map_err(|e| EngineError::Other(e.to_string()))?;
    let dot = path.to_ascii_lowercase().ends_with(".lottie");
    let bytes = if dot {
        let stem = std::path::Path::new(&path).file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        effectcraft_lottie::to_dotlottie(&res, &stem)
    } else {
        res.to_string_compact().into_bytes()
    };
    s.services.write_file(&path, &bytes).map_err(|e| EngineError::Other(format!("cannot write {path}: {e}")))?;
    let n = res.warnings.len();
    s.toast(if n == 0 { format!("Exported Lottie to {path}") } else { format!("Exported Lottie to {path} ({n} warning{})", if n == 1 { "" } else { "s" }) });
    Ok(json!({"path": path, "bytes": bytes.len(), "warnings": res.warnings}))
}

fn import_lottie(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("file.importLottie", "missing `path`"))?.to_string();
    let bytes = s.services.read_file(&path).map_err(|e| EngineError::Other(format!("cannot read {path}: {e}")))?;
    let fp = std::path::Path::new(&path);
    let dir = fp.parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
    let stem = fp.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Lottie".into());
    let services = s.services.clone();
    // Embedded images are written next to the Lottie file (`<name>_<asset id>.<ext>`).
    let mut store = |img: &ImportedImage| -> Option<String> {
        let safe: String = img.id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
        let file = std::path::Path::new(&dir).join(format!("{stem}_{safe}.{}", img.ext)).to_string_lossy().to_string();
        services.write_file(&file, &img.bytes).ok().map(|_| file)
    };
    let res = s.edit("Import Lottie", None, |proj, st| {
        let r = effectcraft_lottie::import(proj, &bytes, &stem, &dir, &mut store).map_err(|e| EngineError::Other(e.to_string()))?;
        st.project_selection = vec![r.comp];
        Ok(r)
    })?;
    s.open_comp(res.comp);
    if !res.warnings.is_empty() {
        s.toast(format!("Imported {path} ({} warnings)", res.warnings.len()));
    }
    Ok(json!({"comp": res.comp.0, "items": res.items.iter().map(|i| i.0).collect::<Vec<_>>(), "warnings": res.warnings}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "file.exportLottie",
            "Lottie JSON...",
            ["File", "Export"],
            None,
            "{comp?, path (.json or .lottie), includeExpressions?: bool, textAsShapes?: bool}",
            has_comp,
            export_lottie
        ),
        cmd!("file.importLottie", "Lottie...", ["File", "Import"], None, "{path (.json or .lottie)}", always, import_lottie),
    ]
}
