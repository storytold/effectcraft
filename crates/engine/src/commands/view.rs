//! View menu commands that change the project (guides, each comp's Resolution) or
//! headless-relevant editor state (the region of interest).

use effectcraft_project::{Guide, Resolution};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, comp_id, f_p, has_comp, str_p};
use crate::{EngineError, Result, Session, cmd};

fn has_guides(s: &Session) -> std::result::Result<(), String> {
    has_comp(s)?;
    if s.active_comp().is_some_and(|c| !c.guides.is_empty()) { Ok(()) } else { Err("the composition has no guides".into()) }
}

fn add_guide(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let vertical = match str_p(p, "orientation").unwrap_or("vertical") {
        "vertical" | "v" => true,
        "horizontal" | "h" => false,
        o => return Err(bad("view.addGuide", format!("orientation must be vertical or horizontal, not `{o}`"))),
    };
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let position = f_p(p, "position").unwrap_or(if vertical { comp.width as f64 / 2.0 } else { comp.height as f64 / 2.0 });
    let n = s.edit("Add Guide", None, |proj, _| {
        let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        c.guides.push(Guide { vertical, position });
        Ok(c.guides.len())
    })?;
    Ok(json!({"guides": n}))
}

fn clear_guides(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    s.edit("Clear Guides", None, |proj, _| {
        proj.comp_mut(cid).ok_or(EngineError::NoComp)?.guides.clear();
        Ok(())
    })?;
    Ok(Value::Null)
}

fn export_guides(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("view.exportGuides", "missing `path`"))?;
    let comp = s.project.comp(comp_id(s, p)?).ok_or(EngineError::NoComp)?;
    let doc = json!({"effectcraftGuides": 1, "width": comp.width, "height": comp.height, "guides": comp.guides});
    let text = serde_json::to_string_pretty(&doc).unwrap_or_default();
    s.services.write_file(path, text.as_bytes()).map_err(|e| EngineError::Other(format!("cannot write {path}: {e}")))?;
    Ok(json!({"path": path, "guides": comp.guides.len()}))
}

fn import_guides(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("view.importGuides", "missing `path`"))?;
    let cid = comp_id(s, p)?;
    let bytes = s.services.read_file(path).map_err(|e| EngineError::Other(format!("cannot read {path}: {e}")))?;
    let doc: Value = serde_json::from_slice(&bytes).map_err(|e| bad("view.importGuides", format!("not a guides file: {e}")))?;
    let guides: Vec<Guide> = serde_json::from_value(doc.get("guides").cloned().unwrap_or(json!([]))).map_err(|e| bad("view.importGuides", e.to_string()))?;
    let n = guides.len();
    s.edit("Import Guides", None, |proj, _| {
        proj.comp_mut(cid).ok_or(EngineError::NoComp)?.guides.extend(guides);
        Ok(())
    })?;
    Ok(json!({"imported": n}))
}

fn region_of_interest(s: &mut Session, p: &Value) -> Result<Value> {
    s.state.region_of_interest = match p.get("rect") {
        Some(Value::Array(a)) if a.len() == 4 => {
            let r = [0, 1, 2, 3].map(|i| a[i].as_f64().unwrap_or(0.0));
            if r[2] <= 0.0 || r[3] <= 0.0 {
                return Err(bad("view.setRegionOfInterest", "width and height must be positive"));
            }
            Some(r)
        }
        _ => None,
    };
    Ok(json!(s.state.region_of_interest))
}

/// View ▸ Resolution (and the Composition panel's Resolution popup): the comp's Resolution, saved
/// with the project but no undo step ([`Session::set_resolution`]).
fn set_resolution(s: &mut Session, p: &Value, r: Resolution) -> Result<Value> {
    let cid = comp_id(s, p)?;
    s.set_resolution(cid, r)?;
    Ok(json!({"comp": cid.0, "resolution": r.label(), "factor": r.factor()}))
}

/// View ▸ Resolution ▸ Custom: `factor` renders every n-th pixel; without one the frontend asks
/// for it (the Custom Resolution dialog).
fn custom_resolution(s: &mut Session, p: &Value) -> Result<Value> {
    match f_p(p, "factor") {
        Some(n) => set_resolution(s, p, Resolution::from_factor(n.round().clamp(1.0, 40.0) as u64)),
        None if p.get("factor").is_some_and(|v| !v.is_null()) => Err(bad("view.res.custom", "factor: a number from 1 to 40")),
        None => super::frontend(s, "view.res.custom", p),
    }
}

fn view_layout(s: &mut Session, p: &Value) -> Result<Value> {
    let n = p.get("views").and_then(Value::as_u64).ok_or_else(|| bad("view.layout", "views: 1|2|4"))?;
    if !matches!(n, 1 | 2 | 4) {
        return Err(bad("view.layout", "views: 1|2|4"));
    }
    s.state.view_layout = n as u8;
    Ok(json!({"views": n}))
}

fn share_view_options(s: &mut Session, p: &Value) -> Result<Value> {
    s.state.share_view_options = super::b_p(p, "value").unwrap_or(!s.state.share_view_options);
    Ok(json!(s.state.share_view_options))
}

/// Settings ▸ 3D ▸ Extended Viewer: 3D views (custom views, or Draft 3D) show the pasteboard
/// around the comp frame with the layers that reach onto it.
fn extended_viewer(s: &mut Session, p: &Value) -> Result<Value> {
    let v = super::b_p(p, "value").unwrap_or(!s.prefs.three_d.extended_viewer);
    if v != s.prefs.three_d.extended_viewer {
        s.prefs.three_d.extended_viewer = v;
        s.prefs_changed();
        s.save_prefs();
    }
    Ok(json!(v))
}

/// A `view.res.*` command setting one [`Resolution`].
macro_rules! res {
    ($id:literal, $label:literal, [$($m:literal),*], $sc:expr, $r:ident) => {
        cmd!($id, $label, [$($m),*], $sc, "{comp?}", has_comp, |s, p| set_resolution(s, p, Resolution::$r))
    };
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("view.addGuide", "Add Guide...", ["View"], None, "{orientation?: vertical|horizontal, position? (comp px)}", has_comp, add_guide),
        cmd!("view.clearGuides", "Clear Guides", ["View"], None, "{comp?}", has_guides, clear_guides),
        cmd!("view.importGuides", "Import Guides...", ["View"], None, "{path}", has_comp, import_guides),
        cmd!("view.exportGuides", "Export Guides...", ["View"], None, "{path}", has_guides, export_guides),
        res!("view.res.full", "Full", ["View", "Resolution"], Some("Cmd+J"), Full),
        res!("view.res.half", "Half", ["View", "Resolution"], Some("Cmd+Shift+J"), Half),
        res!("view.res.third", "Third", ["View", "Resolution"], None, Third),
        res!("view.res.quarter", "Quarter", ["View", "Resolution"], Some("Cmd+Alt+Shift+J"), Quarter),
        cmd!("view.res.custom", "Custom...", ["View", "Resolution"], None, "{factor?: 1..40 (render every n-th pixel), comp?}", has_comp, custom_resolution),
        // The Composition panel's Resolution popup (not in the View menu).
        res!("view.res.auto", "Resolution: Auto", [], None, Auto),
        cmd!("view.layout", "Switch View Layout", [], None, "{views: 1|2|4}", has_comp, view_layout),
        cmd!("view.shareViewOptions", "Share View Options", ["View", "Switch View Layout"], None, "{value?}", has_comp, share_view_options),
        cmd!("view.extendedViewer", "Extended Viewer", [], None, "{value?}", always, extended_viewer),
        cmd!("view.setRegionOfInterest", "Region of Interest", [], None, "{rect?: [x, y, w, h] | null}", always, region_of_interest),
    ]
}
