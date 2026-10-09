//! Commands behind the Progress, Media Browser, Metadata and Lumetri Scopes panels.

use effectcraft_project::ItemId;
use effectcraft_raster::Image;
use effectcraft_raster::scopes::{self, ColorStandard, ScopeKind, ScopeOpts};
use effectcraft_render::RenderOpts;
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::{CommandSpec, always, b_p, bad, f_p, str_p};
use crate::{EngineError, Result, Session, cmd, media_browser as mb, query};

// ---------------------------------------------------------------- Progress

fn jobs_list(s: &mut Session, _: &Value) -> Result<Value> {
    s.poll_jobs();
    Ok(json!({"running": s.jobs(), "finished": s.job_log}))
}

fn jobs_cancel(s: &mut Session, p: &Value) -> Result<Value> {
    let id = str_p(p, "job").unwrap_or("all");
    Ok(json!({"cancelled": s.cancel_job(id)}))
}

fn jobs_wait(s: &mut Session, _: &Value) -> Result<Value> {
    s.wait_jobs();
    Ok(json!({"finished": s.job_log}))
}

// ---------------------------------------------------------------- Media Browser

fn browser(s: &Session) -> Result<std::sync::Arc<dyn mb::Browser>> {
    s.media_browser().ok_or_else(|| EngineError::Other("the Media Browser is not available in this build".into()))
}

fn browser_dir(s: &Session, b: &dyn mb::Browser, p: &Value) -> String {
    str_p(p, "path").map(str::to_string).or_else(|| s.state.media_browser.folder.clone()).unwrap_or_else(|| b.home())
}

fn browser_list(s: &mut Session, p: &Value) -> Result<Value> {
    let b = browser(s)?;
    let dir = browser_dir(s, b.as_ref(), p);
    let only = b_p(p, "importableOnly").unwrap_or(s.state.media_browser.importable_only);
    let entries = b.list(&dir, only).map_err(EngineError::Other)?;
    let places: Vec<Value> = b.places().into_iter().map(|(name, path)| json!({"name": name, "path": path})).collect();
    let actions: Vec<Value> = b.actions().into_iter().map(|(id, label)| json!({"id": id, "label": label})).collect();
    Ok(json!({
        "path": dir,
        "parent": b.parent(&dir),
        "entries": entries,
        "favorites": s.state.media_browser.favorites,
        "places": places,
        "actions": actions,
    }))
}

fn browser_enabled(s: &Session) -> std::result::Result<(), String> {
    if s.media_browser().is_some() { Ok(()) } else { Err("the Media Browser is not available in this build".into()) }
}

fn browser_go(s: &mut Session, p: &Value) -> Result<Value> {
    let b = browser(s)?;
    let dir = match str_p(p, "path") {
        Some("..") => s.state.media_browser.folder.clone().and_then(|d| b.parent(&d)).unwrap_or_else(|| b.home()),
        Some(d) => d.to_string(),
        None => b.home(),
    };
    if let Some(v) = b_p(p, "importableOnly") {
        s.state.media_browser.importable_only = v;
    }
    // Validate before switching.
    b.list(&dir, false).map_err(EngineError::Other)?;
    s.state.media_browser.folder = Some(dir.clone());
    browser_list(s, &json!({"path": dir}))
}

/// A browser-specific toolbar action (web: Open Folder…, Add Files…).
fn browser_action(s: &mut Session, p: &Value) -> Result<Value> {
    let b = browser(s)?;
    let id = str_p(p, "action").ok_or_else(|| bad("mediaBrowser.action", "missing `action`"))?;
    b.action(id).map_err(|e| bad("mediaBrowser.action", e))
}

fn browser_favorite(s: &mut Session, p: &Value, add: bool) -> Result<Value> {
    let b = browser(s)?;
    let dir = browser_dir(s, b.as_ref(), p);
    let favs = &mut s.state.media_browser.favorites;
    favs.retain(|f| f != &dir);
    if add {
        favs.push(dir);
    }
    Ok(json!({"favorites": favs}))
}

fn browser_import(s: &mut Session, p: &Value) -> Result<Value> {
    let paths = p.get("paths").or(p.get("path")).cloned().ok_or_else(|| bad("mediaBrowser.import", "missing `paths`"))?;
    // Files the browser still has to read (the web app's opened folders): imported when they
    // arrive.
    let list: Vec<String> = match &paths {
        Value::String(one) => vec![one.clone()],
        Value::Array(a) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        _ => vec![],
    };
    if !browser(s)?.fetch(&list, p) {
        return Ok(json!({"pending": true, "paths": list}));
    }
    let target = s.active_comp_id();
    let mut r = s.execute("file.import", json!({"paths": paths}))?;
    // Dragged into the Timeline or the Composition viewer: into the comp, where they were dropped.
    let errors = super::file::add_to_comp(s, &r, target, p);
    if !errors.is_empty()
        && let Some(e) = r.get_mut("errors").and_then(Value::as_array_mut)
    {
        e.extend(errors.into_iter().map(Value::from));
    }
    Ok(r)
}

fn file_info(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("mediaBrowser.fileInfo", "missing `path`"))?;
    let (created, modified, size) = mb::file_dates(path);
    let mut v = json!({"path": path, "kind": mb::kind_of(path), "size": size, "created": created.map(mb::iso_date), "modified": modified.map(mb::iso_date)});
    if matches!(mb::kind_of(path), Some("video" | "image" | "audio" | "model"))
        && let Ok(f) = s.probe_footage(path)
    {
        v["footage"] = mb::footage_metadata(&f);
    }
    Ok(v)
}

// ---------------------------------------------------------------- Metadata

fn item_ref(s: &Session, p: &Value) -> Option<ItemId> {
    match p.get("item") {
        Some(Value::Number(n)) => n.as_u64().map(ItemId),
        Some(Value::String(name)) => s.project.items.values().find(|i| &i.name == name).map(|i| i.id),
        _ => s.state.project_selection.first().copied(),
    }
}

fn metadata(s: &mut Session, p: &Value) -> Result<Value> {
    let project = mb::project_metadata(&s.project, s.path.as_deref());
    let item = match item_ref(s, p) {
        Some(id) => Some(mb::item_metadata(&s.project, s.project.item(id).ok_or_else(|| bad("item.metadata", format!("no item {}", id.0)))?)),
        None => None,
    };
    Ok(json!({"project": project, "item": item}))
}

fn project_comment(s: &mut Session, p: &Value) -> Result<Value> {
    let text = str_p(p, "comment").ok_or_else(|| bad("project.setProjectComment", "missing `comment`"))?.to_string();
    s.edit("Project Comment", super::merge_p(p), |proj, _| {
        proj.settings.comment = text;
        Ok(())
    })?;
    Ok(Value::Null)
}

// ---------------------------------------------------------------- Lumetri Scopes

/// The comp frame the scopes read (the viewer's image: over the comp background), at most
/// `max_side` pixels on the long side.
pub fn scope_frame(s: &Session, comp: ItemId, t: Tick, max_side: u32) -> Option<Image> {
    let c = s.project.comp(comp)?;
    let long = c.width.max(c.height).max(1) as f64;
    let scale = (max_side as f64 / long).min(1.0);
    let mut img = s.render(comp, t, RenderOpts { scale, ..Default::default() });
    let bg = c.background;
    for p in &mut img.data {
        let k = 1.0 - p[3].clamp(0.0, 1.0);
        *p = [p[0] + bg[0] * k, p[1] + bg[1] * k, p[2] + bg[2] * k, 1.0];
    }
    Some(img)
}

/// Scope options from parameters.
pub fn scope_opts(p: &Value) -> std::result::Result<(ScopeKind, ScopeOpts), String> {
    let kind = match str_p(p, "scope") {
        Some(k) => ScopeKind::from_name(k).ok_or_else(|| format!("unknown scope `{k}`; one of {}", ScopeKind::ALL.map(|k| k.name()).join(", ")))?,
        None => ScopeKind::WaveformRgb,
    };
    let standard = match str_p(p, "standard") {
        Some(x) => ColorStandard::from_name(x).ok_or_else(|| format!("standard: rec601|rec709|rec2020, not `{x}`"))?,
        None => ColorStandard::Rec709,
    };
    let size = f_p(p, "size").unwrap_or(256.0).clamp(16.0, 1024.0) as u32;
    Ok((kind, ScopeOpts { standard, float: b_p(p, "float").unwrap_or(false), clamp: b_p(p, "clamp").unwrap_or(true), width: size, height: size }))
}

fn scopes_analyze(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = super::comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let t = super::time_p(s, p, Some(comp));
    let (kind, o) = scope_opts(p).map_err(|e| bad("scopes.analyze", e))?;
    let img = scope_frame(s, cid, t, 512).ok_or(EngineError::NoComp)?;
    let sc = scopes::compute(&img, kind, &o);
    // Signal statistics of the frame.
    let n = img.data.len().max(1) as f64;
    let mut mean = [0.0f64; 3];
    let (mut ymin, mut ymax, mut ysum) = (f64::MAX, f64::MIN, 0.0);
    for px in &img.data {
        for c in 0..3 {
            mean[c] += px[c] as f64 / n;
        }
        let y = o.standard.ycbcr([px[0], px[1], px[2]])[0] as f64;
        ymin = ymin.min(y);
        ymax = ymax.max(y);
        ysum += y;
    }
    let mut v = json!({
        "scope": kind.name(),
        "width": sc.width,
        "height": sc.height,
        "peaks": [sc.peak(0), sc.peak(1), sc.peak(2)],
        "totals": [sc.total(0), sc.total(1), sc.total(2)],
        "mean": mean,
        "luma": {"min": ymin, "max": ymax, "mean": ysum / n},
    });
    if kind == ScopeKind::Histogram {
        v["histogram"] = json!(scopes::histogram_bars(&sc));
    }
    Ok(v)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        query!("jobs.list", "Background Jobs", "{}", jobs_list),
        cmd!("jobs.cancel", "Cancel Job", [], None, "{job?: render|track|maskTrack|warp|camera|roto|task:<n>|all}", always, jobs_cancel),
        query!("jobs.wait", "Wait for Background Jobs", "{}", jobs_wait),
        query!("mediaBrowser.list", "List Folder", "{path?, importableOnly?}", browser_list),
        cmd!("mediaBrowser.go", "Go to Folder", [], None, "{path?: folder | \"..\", importableOnly?}", browser_enabled, browser_go),
        cmd!("mediaBrowser.addFavorite", "Add to Favorites", [], None, "{path?}", browser_enabled, |s, p| browser_favorite(s, p, true)),
        cmd!("mediaBrowser.removeFavorite", "Remove from Favorites", [], None, "{path?}", browser_enabled, |s, p| browser_favorite(s, p, false)),
        cmd!(
            "mediaBrowser.import",
            "Import",
            [],
            None,
            "{paths, addToComp?, time?, index?, position? (where the layers go, as in layer.addItem)}",
            browser_enabled,
            browser_import
        ),
        cmd!(
            "mediaBrowser.action",
            "Media Browser Action",
            [],
            None,
            "{action}: an action from mediaBrowser.list's `actions` (web: openFolder, addFiles, uploadFolder)",
            browser_enabled,
            browser_action
        ),
        query!("mediaBrowser.fileInfo", "File Info", "{path}", file_info),
        query!("item.metadata", "Metadata", "{item?: id|name}", metadata),
        cmd!("project.setProjectComment", "Project Comment", [], None, "{comment}", always, project_comment),
        query!(
            "scopes.analyze",
            "Lumetri Scopes",
            "{comp?, time?|frame?, scope?: waveformRgb|waveformLuma|waveformYc|vectorscopeYuv|vectorscopeHls|histogram|paradeRgb|paradeYuv, standard?: rec601|rec709|rec2020, float?, clamp?, size?}",
            scopes_analyze
        ),
    ]
}
