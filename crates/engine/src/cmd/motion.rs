//! File ▸ Import ▸ Apple Motion… and File ▸ Export ▸ Apple Motion Project… (`effectcraft-motion`).

use std::collections::HashMap;

use effectcraft_motion::ClipRef;
use effectcraft_project::Footage;
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, comp_id, has_comp, str_p};
use crate::{EngineError, Result, Session, cmd};

fn export_motion(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("file.exportMotion", "missing `path`"))?.to_string();
    let cid = comp_id(s, p)?;
    let dir = std::path::Path::new(&path).parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
    let res = effectcraft_motion::export_comp(&s.project, cid, &dir).map_err(|e| EngineError::Other(e.to_string()))?;
    let bytes = res.xml.into_bytes();
    s.services.write_file(&path, &bytes).map_err(|e| EngineError::Other(format!("cannot write {path}: {e}")))?;
    let n = res.warnings.len();
    s.toast(if n == 0 {
        format!("Exported Motion project to {path}")
    } else {
        format!("Exported Motion project to {path} ({n} warning{})", if n == 1 { "" } else { "s" })
    });
    Ok(json!({"path": path, "bytes": bytes.len(), "warnings": res.warnings}))
}

/// The files a clip may be: next to the Motion document (`relativeURL`), then its absolute path.
fn candidates(c: &ClipRef, dir: &str) -> Vec<String> {
    let mut out = vec![];
    if let Some(rel) = &c.relative {
        out.push(std::path::Path::new(dir).join(rel).to_string_lossy().to_string());
    }
    if let Some(p) = &c.path {
        out.push(p.clone());
    }
    out
}

fn import_motion(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("file.importMotion", "missing `path`"))?.to_string();
    let bytes = s.services.read_file(&path).map_err(|e| EngineError::Other(format!("cannot read {path}: {e}")))?;
    let fp = std::path::Path::new(&path);
    let dir = fp.parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
    let stem = fp.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Motion".into());
    // Find and probe the media before the edit (probing needs the session).
    let clips = effectcraft_motion::clip_refs(&bytes).map_err(|e| EngineError::Other(e.to_string()))?;
    let mut found: HashMap<String, Footage> = HashMap::new();
    for c in &clips {
        // A one-frame clip is that image alone: `shot_0001.png` and `shot_0002.png` used as two
        // stills must not each become the `shot_[0001-0002]` sequence.
        let probe = |f: &str| if c.still { s.probe_file(f) } else { s.probe_footage(f) };
        let probed = candidates(c, &dir).into_iter().filter(|f| s.services.exists(f)).find_map(|f| probe(&f).ok());
        if let Some(f) = probed {
            found.insert(c.id.clone(), f);
        }
    }
    let resolve = |c: &ClipRef| found.get(&c.id).cloned();
    let res = s.edit("Import Motion Project", None, |proj, st| {
        let r = effectcraft_motion::import(proj, &bytes, &stem, &resolve).map_err(|e| EngineError::Other(e.to_string()))?;
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
        cmd!("file.exportMotion", "Apple Motion Project...", ["File", "Export"], None, "{comp?, path (.motn)}", has_comp, export_motion),
        cmd!("file.importMotion", "Apple Motion...", ["File", "Import"], None, "{path (.motn, .motr, .moti, .moef or .mogn)}", always, import_motion),
    ]
}
