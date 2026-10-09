//! File menu, part two: folders, close, save a copy, import placeholders/solids, New Comp from
//! Selection, Dependencies (collect / consolidate / remove unused / reduce / find missing),
//! scripts, footage interpretation, replace/reload footage, reveal in Finder.

use std::collections::{BTreeMap, BTreeSet};

use effectcraft_color::Label;
use effectcraft_project::{AlphaMode, Comp, Footage, FootageKind, ItemId, ItemKind, LayerSource, Project, Solid};
use effectcraft_time::{FrameRate, Tick};
use serde_json::{Value, json};

use super::{CommandSpec, always, b_p, bad, f_p, has_comp, has_project_selection, str_p};
use crate::sequence::SequenceOptions;
use crate::{EngineError, Result, Session, cmd};

fn has_footage_selection(s: &Session) -> std::result::Result<(), String> {
    if selected_footage(s).is_empty() { Err("select footage in the Project panel".into()) } else { Ok(()) }
}

fn has_interpretation(s: &Session) -> std::result::Result<(), String> {
    has_footage_selection(s)?;
    if s.state.interpretation.is_none() { Err("no interpretation remembered".into()) } else { Ok(()) }
}

fn has_items(s: &Session) -> std::result::Result<(), String> {
    if s.project.items.is_empty() { Err("the project is empty".into()) } else { Ok(()) }
}

fn has_comp_selection(s: &Session) -> std::result::Result<(), String> {
    if s.state.project_selection.iter().any(|i| s.project.comp(*i).is_some()) { Ok(()) } else { Err("select compositions in the Project panel".into()) }
}

/// Selected project items that are footage (or the source of the selected layer).
fn selected_footage(s: &Session) -> Vec<ItemId> {
    let mut v: Vec<ItemId> =
        s.state.project_selection.iter().copied().filter(|i| matches!(s.project.item(*i).map(|x| &x.kind), Some(ItemKind::Footage(_)))).collect();
    if v.is_empty()
        && let Some(c) = s.active_comp()
    {
        v = s
            .state
            .selected_layers
            .iter()
            .filter_map(|l| c.layer(*l))
            .filter_map(|l| if let LayerSource::Footage { item } = l.source { Some(item) } else { None })
            .collect();
    }
    v
}

fn items_p(s: &Session, p: &Value) -> Vec<ItemId> {
    match p.get("items").or(p.get("item")) {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_u64).map(ItemId).collect(),
        Some(Value::Number(n)) => n.as_u64().map(ItemId).into_iter().collect(),
        _ => selected_footage(s),
    }
}

fn new_folder(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_p(p, "name").unwrap_or("Untitled Folder").to_string();
    let parent = p.get("parent").and_then(Value::as_u64).map(ItemId).filter(|i| s.project.item(*i).is_some_and(|x| x.is_folder()));
    let id = s.edit("New Folder", None, |proj, st| {
        let id = proj.add_item(&name, Label::Yellow, parent, ItemKind::Folder);
        st.project_selection = vec![id];
        Ok(id)
    })?;
    Ok(json!({"item": id.0}))
}

fn close(s: &mut Session, p: &Value) -> Result<Value> {
    s.execute("comp.close", p.clone())
}

fn close_project(s: &mut Session, _: &Value) -> Result<Value> {
    s.replace_project(Project::default(), None);
    Ok(Value::Null)
}

fn save_copy(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("file.saveCopy", "missing `path`"))?;
    let json = super::file::file_text(s, path)?;
    s.services.write_file(path, json.as_bytes()).map_err(|e| EngineError::Other(format!("cannot write {path}: {e}")))?;
    Ok(json!({"path": path, "bytes": json.len()}))
}

fn rate_of(p: &Value) -> FrameRate {
    f_p(p, "frameRate").map(FrameRate::from_f64).unwrap_or(FrameRate::FPS_29_97)
}

fn placeholder_footage(p: &Value) -> Footage {
    let w = p.get("width").and_then(Value::as_u64).unwrap_or(1920) as u32;
    let h = p.get("height").and_then(Value::as_u64).unwrap_or(1080) as u32;
    Footage {
        path: String::new(),
        kind: FootageKind::Video,
        width: w,
        height: h,
        pixel_aspect: 1.0,
        frame_rate: rate_of(p),
        native_rate: None,
        duration: Tick::from_seconds_f64(f_p(p, "duration").unwrap_or(30.0)),
        has_video: true,
        has_audio: false,
        alpha: AlphaMode::Straight,
        premul_color: [0.0; 3],
        loop_count: 1,
        codec: "Placeholder".into(),
        missing: true,
        sequence: vec![],
        color_profile: None,
        ..Default::default()
    }
}

fn import_placeholder(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_p(p, "name").unwrap_or("Placeholder").to_string();
    let f = placeholder_footage(p);
    let id = s.edit("Import Placeholder", None, |proj, st| {
        let id = proj.add_item(&name, Label::Aqua, None, ItemKind::Footage(f));
        st.project_selection = vec![id];
        Ok(id)
    })?;
    Ok(json!({"item": id.0}))
}

fn solid_from(p: &Value, s: &Session) -> Solid {
    let (cw, ch) = s.active_comp().map(|c| (c.width, c.height)).unwrap_or((1920, 1080));
    let color = match p.get("color") {
        Some(Value::String(h)) => effectcraft_color::Rgba::from_hex(h).map(|c| [c.r, c.g, c.b]),
        Some(Value::Array(a)) => Some([0, 1, 2].map(|i| a.get(i).and_then(Value::as_f64).unwrap_or(0.0) as f32)),
        _ => None,
    }
    .unwrap_or([0.5, 0.5, 0.5]);
    Solid {
        color,
        width: p.get("width").and_then(Value::as_u64).map(|v| v as u32).unwrap_or(cw).max(1),
        height: p.get("height").and_then(Value::as_u64).map(|v| v as u32).unwrap_or(ch).max(1),
        pixel_aspect: 1.0,
    }
}

fn import_solid(s: &mut Session, p: &Value) -> Result<Value> {
    let so = solid_from(p, s);
    let name = str_p(p, "name").unwrap_or("Solid").to_string();
    let id = s.edit("Import Solid", None, |proj, st| {
        let folder = proj.folder_named("Solids").unwrap_or_else(|| proj.add_item("Solids", Label::Yellow, None, ItemKind::Folder));
        let id = proj.add_item(&name, Label::Red, Some(folder), ItemKind::Solid(so));
        st.project_selection = vec![id];
        Ok(id)
    })?;
    Ok(json!({"item": id.0}))
}

/// File ▸ New Comp from Selection. Each selected item becomes a composition of its size, pixel
/// aspect, frame rate and duration holding it as a layer; with `single`, one composition holds
/// them all (first selected on top), with the settings of item `dimensionsFrom` (an index into
/// the selection, default 0), as long as the longest item, or with `sequence` the layers one
/// after another (Sequence Layers: `overlap`, `overlapDuration` seconds, `transition`).
/// `duration`: seconds for stills (default: Settings ▸ Import ▸ Still Footage when it is a
/// duration, else 10 s).
/// `addToRenderQueue` queues the new compositions. One undo step.
fn new_comp_from_selection(s: &mut Session, p: &Value) -> Result<Value> {
    let items: Vec<ItemId> = s.state.project_selection.iter().copied().filter(|i| s.project.item(*i).is_some_and(|x| !x.is_folder())).collect();
    if items.is_empty() {
        return Err(bad("file.newCompFromSelection", "select footage in the Project panel"));
    }
    let single = b_p(p, "single").unwrap_or(false) && items.len() > 1;
    let from = p.get("dimensionsFrom").and_then(Value::as_u64).unwrap_or(0) as usize;
    if single && from >= items.len() {
        return Err(bad("file.newCompFromSelection", format!("`dimensionsFrom` must be below {}", items.len())));
    }
    let still = f_p(p, "duration")
        .or((s.prefs.import.still_footage == "seconds").then_some(s.prefs.import.still_seconds))
        .filter(|d| d.is_finite() && *d > 0.0)
        .unwrap_or(10.0);
    // Layer ▸ Keyframe Assistant ▸ Sequence Layers' parameters.
    let sequence = b_p(p, "sequence").unwrap_or(false).then(|| {
        json!({
            "overlap": b_p(p, "overlap").unwrap_or(false),
            "duration": f_p(p, "overlapDuration").filter(|d| d.is_finite() && *d >= 0.0).unwrap_or(1.0),
            "transition": str_p(p, "transition").unwrap_or("off"),
        })
    });
    let queue = b_p(p, "addToRenderQueue").unwrap_or(false);
    let proj = s.project.clone();
    let duration = |i: ItemId| proj.item(i).and_then(|it| it.duration()).filter(|d| *d > Tick::ZERO).unwrap_or(Tick::from_seconds_f64(still));
    let groups: Vec<Vec<ItemId>> = if single { vec![items.clone()] } else { items.iter().map(|i| vec![*i]).collect() };
    super::app_more::grouped(s, "New Comp from Selection", |s| {
        let mut made = vec![];
        for g in groups {
            let lead = g.get(if single { from } else { 0 }).and_then(|i| proj.item(*i)).ok_or(EngineError::NoComp)?;
            let (w, h) = lead.dimensions().unwrap_or((1920, 1080));
            let rate = lead.frame_rate().unwrap_or(FrameRate::FPS_29_97);
            let pixel_aspect = match &lead.kind {
                ItemKind::Footage(f) => f.pixel_aspect,
                ItemKind::Solid(so) => so.pixel_aspect,
                ItemKind::Comp(c) => c.pixel_aspect,
                ItemKind::Folder => 1.0,
            };
            let overlap =
                sequence.as_ref().filter(|q| q["overlap"] == true).map_or(Tick::ZERO, |q| Tick::from_seconds_f64(q["duration"].as_f64().unwrap_or(1.0)));
            let dur = match &sequence {
                Some(_) if g.len() > 1 => g.iter().map(|i| duration(*i)).fold(Tick::ZERO, |a, d| a + d) - Tick(overlap.0.saturating_mul(g.len() as i64 - 1)),
                _ => g.iter().map(|i| duration(*i)).max().unwrap_or(Tick::from_seconds_f64(still)),
            }
            .max(rate.frame_duration());
            let name = lead.name.rsplit_once('.').map(|(a, _)| a.to_string()).unwrap_or(lead.name.clone());
            let c = Comp { pixel_aspect, ..Comp::new(w.max(1), h.max(1), rate, dur) };
            let cid = s.edit("New Comp from Selection", None, |proj, st| {
                let cid = proj.add_item(&name, Label::Sandstone, None, ItemKind::Comp(c.into()));
                st.project_selection = vec![cid];
                Ok(cid)
            })?;
            s.open_comp(cid);
            // Added bottom first, so the first selected ends up on top.
            let mut layers = vec![];
            for item in g.iter().rev() {
                layers.push(s.execute("layer.addItem", json!({"comp": cid.0, "item": item.0, "duration": still}))?["layer"].clone());
            }
            layers.reverse();
            if let Some(q) = &sequence
                && layers.len() > 1
            {
                let mut q = q.clone();
                q["comp"] = json!(cid.0);
                q["layers"] = json!(layers);
                s.execute("layer.sequence", q)?;
            }
            if queue {
                s.execute("renderQueue.add", json!({"comp": cid.0}))?;
            }
            made.push(cid.0);
        }
        Ok(json!({"comps": made}))
    })
}

// ---------------------------------------------------------------- dependencies

/// Items used (transitively) by `roots`.
fn used_items(proj: &Project, roots: &[ItemId]) -> BTreeSet<ItemId> {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<ItemId> = roots.to_vec();
    while let Some(i) = stack.pop() {
        if !seen.insert(i) {
            continue;
        }
        if let Some(c) = proj.comp(i) {
            stack.extend(c.layers.iter().filter_map(|l| l.source.item()));
        }
        // Parent folders stay so the structure is kept.
        if let Some(parent) = proj.item(i).and_then(|x| x.parent) {
            stack.push(parent);
        }
    }
    seen
}

fn referenced_sources(proj: &Project) -> BTreeSet<ItemId> {
    proj.comps().flat_map(|(_, c)| c.layers.iter().filter_map(|l| l.source.item())).collect()
}

fn remove_unused(s: &mut Session, _: &Value) -> Result<Value> {
    let used = referenced_sources(&s.project);
    let n = s.edit("Remove Unused Footage", None, |proj, _| {
        let before = proj.items.len();
        proj.items.retain(|id, it| !matches!(it.kind, ItemKind::Footage(_) | ItemKind::Solid(_)) || used.contains(id));
        Ok(before - proj.items.len())
    })?;
    s.sanitize_state();
    s.toast(format!("Removed {n} unused footage item(s)"));
    Ok(json!({"removed": n}))
}

fn consolidate(s: &mut Session, _: &Value) -> Result<Value> {
    // Footage with the same file and interpretation collapses into the first such item.
    let mut first: BTreeMap<String, ItemId> = BTreeMap::new();
    let mut remap: BTreeMap<ItemId, ItemId> = BTreeMap::new();
    for (id, it) in &s.project.items {
        if let ItemKind::Footage(f) = &it.kind
            && !f.path.is_empty()
        {
            let key = serde_json::to_string(f).unwrap_or_default();
            match first.get(&key) {
                Some(keep) => {
                    remap.insert(*id, *keep);
                }
                None => {
                    first.insert(key, *id);
                }
            }
        }
    }
    let n = remap.len();
    s.edit("Consolidate All Footage", None, |proj, _| {
        let ids: Vec<ItemId> = proj.comps().map(|(id, _)| *id).collect();
        for cid in ids {
            if let Some(c) = proj.comp_mut(cid) {
                for l in &mut c.layers {
                    if let LayerSource::Footage { item } = &mut l.source
                        && let Some(k) = remap.get(item)
                    {
                        *item = *k;
                    }
                }
            }
        }
        proj.items.retain(|id, _| !remap.contains_key(id));
        Ok(())
    })?;
    s.sanitize_state();
    s.toast(format!("Consolidated {n} duplicate footage item(s)"));
    Ok(json!({"removed": n}))
}

fn reduce(s: &mut Session, _: &Value) -> Result<Value> {
    let roots: Vec<ItemId> = s.state.project_selection.iter().copied().filter(|i| s.project.comp(*i).is_some()).collect();
    let keep = used_items(&s.project, &roots);
    let n = s.edit("Reduce Project", None, |proj, _| {
        let before = proj.items.len();
        proj.items.retain(|id, _| keep.contains(id));
        Ok(before - proj.items.len())
    })?;
    s.sanitize_state();
    s.toast(format!("Removed {n} item(s)"));
    Ok(json!({"removed": n}))
}

fn find_missing(s: &mut Session, p: &Value) -> Result<Value> {
    let what = str_p(p, "what").unwrap_or("footage");
    match what {
        "footage" => {
            let ids: Vec<ItemId> = s.project.items.values().filter(|i| matches!(&i.kind, ItemKind::Footage(f) if f.missing)).map(|i| i.id).collect();
            s.state.project_selection = ids.clone();
            s.toast(format!("{} missing footage item(s)", ids.len()));
            Ok(json!({"items": ids.iter().map(|i| i.0).collect::<Vec<_>>()}))
        }
        "effects" => {
            let mut found = vec![];
            for (cid, c) in s.project.comps() {
                for l in &c.layers {
                    for g in l.effects().into_iter().flat_map(|fx| fx.groups()) {
                        if let effectcraft_project::GroupKind::Effect { effect } = &g.kind
                            && crate::effects::find(effect).is_none()
                        {
                            found.push(json!({"comp": cid.0, "layer": l.id.0, "effect": effect}));
                        }
                    }
                }
            }
            s.toast(format!("{} missing effect(s)", found.len()));
            Ok(json!({"effects": found}))
        }
        "fonts" => {
            let families = crate::text_families();
            let mut found = vec![];
            for (cid, c) in s.project.comps() {
                for l in &c.layers {
                    if let Some(pr) = l.props.prop("text/sourceText")
                        && let Some(doc) = pr.value.as_text()
                        && !families.iter().any(|f| f.eq_ignore_ascii_case(&doc.font))
                    {
                        found.push(json!({"comp": cid.0, "layer": l.id.0, "font": doc.font}));
                    }
                }
            }
            s.toast(format!("{} layer(s) with missing fonts", found.len()));
            Ok(json!({"fonts": found}))
        }
        w => Err(bad("file.findMissing", format!("what: footage|effects|fonts, not `{w}`"))),
    }
}

fn collect_files(s: &mut Session, p: &Value) -> Result<Value> {
    let folder = str_p(p, "folder").ok_or_else(|| bad("file.collectFiles", "missing `folder`"))?.trim_end_matches('/').to_string();
    // Collect Files makes its folder (and the footage folder inside it).
    #[cfg(not(target_arch = "wasm32"))]
    std::fs::create_dir_all(format!("{folder}/(Footage)")).map_err(|e| EngineError::Other(format!("cannot create {folder}: {e}")))?;
    let mut proj = (*s.project).clone();
    let mut copied = 0;
    let mut errors = vec![];
    let mut used_names = BTreeSet::new();
    for it in proj.items.values_mut() {
        let ItemKind::Footage(f) = &mut it.kind else { continue };
        if f.path.is_empty() {
            continue;
        }
        let files: Vec<String> = if f.sequence.is_empty() { vec![f.path.clone()] } else { f.sequence.clone() };
        let mut new_paths = vec![];
        for src in &files {
            let mut name = std::path::Path::new(src).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "footage".into());
            let mut k = 2;
            while !used_names.insert(name.clone()) {
                name = format!("{k}_{name}");
                k += 1;
            }
            let dst = format!("{folder}/(Footage)/{name}");
            match s.services.read_file(src).and_then(|bytes| s.services.write_file(&dst, &bytes)) {
                Ok(()) => copied += 1,
                Err(e) => errors.push(format!("{src}: {e}")),
            }
            new_paths.push(dst);
        }
        if f.sequence.is_empty() {
            f.path = new_paths.remove(0);
        } else {
            f.path = new_paths.first().cloned().unwrap_or_default();
            f.sequence = new_paths;
        }
    }
    let stem = s
        .path
        .as_deref()
        .and_then(|p| std::path::Path::new(p).file_stem())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "Untitled Project".into());
    let out = format!("{folder}/{stem}.ecproj");
    s.services.write_file(&out, proj.to_json().as_bytes()).map_err(|e| EngineError::Other(format!("cannot write {out}: {e}")))?;
    s.toast(format!("Collected {copied} file(s) into {folder}"));
    Ok(json!({"project": out, "copied": copied, "errors": errors}))
}

// ---------------------------------------------------------------- scripts

/// File ▸ Scripts ▸ Run Script File…: `.jsx` / `.js` files run as JavaScript against the After
/// Effects-style object model ([`Session::script`]); anything else is a command script: a JSON
/// array or JSON lines of `{"command": id, "params": {…}}` (also accepts the control channel's
/// `{"method": "engine.execute", "params": {"id", "params"}}`).
fn run_script(s: &mut Session, p: &Value) -> Result<Value> {
    // File ▸ Scripts ▸ <installed or sample script>.
    if let Some(name) = str_p(p, "name")
        && p.get("path").is_none()
        && p.get("steps").is_none()
    {
        let name = name.to_string();
        return super::scripts::run_named(s, &name);
    }
    if let Some(path) = str_p(p, "path")
        && p.get("steps").is_none()
        && is_js_script(path)
    {
        if is_jsxbin(path) {
            return Err(bad("file.runScript", JSXBIN));
        }
        let bytes = s.services.read_file(path).map_err(|e| EngineError::Other(format!("cannot read {path}: {e}")))?;
        let code = String::from_utf8_lossy(&bytes).to_string();
        let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
        let out = run_js(s, &code, path, false)?;
        return report_js(s, &name, out);
    }
    let steps: Vec<Value> = match (p.get("steps"), str_p(p, "path")) {
        (Some(Value::Array(a)), _) => a.clone(),
        (_, Some(path)) => {
            let bytes = s.services.read_file(path).map_err(|e| EngineError::Other(format!("cannot read {path}: {e}")))?;
            let text = String::from_utf8_lossy(&bytes).to_string();
            match serde_json::from_str::<Value>(&text) {
                Ok(Value::Array(a)) => a,
                _ => text
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !l.starts_with("//") && !l.starts_with('#'))
                    .map(|l| serde_json::from_str(l).map_err(|e| bad("file.runScript", format!("{e}: {l}"))))
                    .collect::<Result<Vec<Value>>>()?,
            }
        }
        _ => return Err(bad("file.runScript", "missing `path` (or inline `steps`)")),
    };
    let mut results = vec![];
    for (i, step) in steps.iter().enumerate() {
        let (id, params) = match (step.get("command").or(step.get("id")).and_then(Value::as_str), step.get("method").and_then(Value::as_str)) {
            (Some(id), _) => (id.to_string(), step.get("params").cloned().unwrap_or(json!({}))),
            (None, Some("engine.execute")) => {
                let pp = step.get("params").cloned().unwrap_or(json!({}));
                (pp.get("id").and_then(Value::as_str).unwrap_or_default().to_string(), pp.get("params").cloned().unwrap_or(json!({})))
            }
            _ => return Err(bad("file.runScript", format!("step {}: needs `command`", i + 1))),
        };
        if id == "file.runScript" || id == "script.run" {
            return Err(bad("file.runScript", "scripts can't run scripts"));
        }
        let r = s.execute(&id, params).map_err(|e| EngineError::Other(format!("step {} ({id}): {e}", i + 1)))?;
        results.push(r);
    }
    Ok(json!({"steps": results.len(), "results": results}))
}

/// `.jsx` / `.js` (and `.jsxbin`, rejected) are JavaScript; `.json` / `.jsonl` / other files are
/// command scripts.
fn is_js_script(path: &str) -> bool {
    let l = path.to_ascii_lowercase();
    l.ends_with(".jsx") || l.ends_with(".js") || is_jsxbin(path)
}

/// Compiled `.jsxbin` scripts are an undocumented binary encoding, not JavaScript.
const JSXBIN: &str = "binary .jsxbin scripts are not supported: run the .jsx source";

fn is_jsxbin(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".jsxbin")
}

/// A script error becomes a toast and a command error.
fn report_js(s: &mut Session, name: &str, out: Value) -> Result<Value> {
    if let Some(e) = out.get("error").filter(|e| !e.is_null()) {
        let line = e.get("line").and_then(Value::as_u64).map(|l| format!(" (line {l})")).unwrap_or_default();
        let msg = e.get("message").and_then(Value::as_str).unwrap_or("script error");
        s.events.push(crate::Event::Toast { message: format!("{name}{line}: {msg}"), error: true });
        return Err(EngineError::Other(format!("{name}{line}: {msg}")));
    }
    Ok(out)
}

/// Run a script's code under its file name (installed / sample scripts, ScriptUI panels).
pub(crate) fn run_js_named(s: &mut Session, code: &str, name: &str) -> Result<Value> {
    let out = run_js(s, code, name, false)?;
    report_js(s, name, out)
}

/// Run JavaScript through the session's scripting engine.
fn run_js(s: &mut Session, code: &str, name: &str, console: bool) -> Result<Value> {
    let runner = s.script.ok_or_else(|| EngineError::Other("JavaScript scripting is not available in this build".into()))?;
    Ok(runner(s, &crate::ScriptRequest { code, name, console }))
}

/// `script.run`: run JavaScript (the After Effects scripting object model: `app.project`, comps,
/// layers, properties…) and report `{ok, result, output, error}`; script errors are reported in
/// `error` (with line and column), not as a command failure.
fn script_run(s: &mut Session, p: &Value) -> Result<Value> {
    let code = str_p(p, "code").ok_or_else(|| bad("script.run", "missing `code` (JavaScript)"))?.to_string();
    let name = str_p(p, "name").unwrap_or("script").to_string();
    // A compiled script (named `.jsxbin`, or its text starts with the format's header) would only
    // fail to parse at 1:1.
    if is_jsxbin(&name) || code.trim_start_matches('\u{feff}').trim_start().starts_with("@JSXBIN@") {
        return Err(bad("script.run", JSXBIN));
    }
    run_js(s, &code, &name, super::b_p(p, "console").unwrap_or(false))
}

// ---------------------------------------------------------------- footage

/// Interpret Footage settings parsed from command parameters (every field optional).
#[derive(Clone, Debug, Default)]
pub(crate) struct Interpretation {
    rate: Option<FrameRate>,
    /// Back to the file's own rate (Use frame rate from file).
    native: bool,
    alpha: Option<AlphaMode>,
    /// Alpha ▸ Guess: decide Straight / Premultiplied (and the matte colour) from a frame.
    guess: bool,
    matte: Option<[f32; 3]>,
    invert_alpha: Option<bool>,
    loops: Option<u32>,
    par: Option<f64>,
    fields: Option<effectcraft_project::FieldOrder>,
    profile: Option<Option<effectcraft_project::ColorSpace>>,
    linear: Option<bool>,
    /// Image sequences: Start Frame (`Some(None)` = the first file's number).
    start: Option<Option<i64>>,
}

impl Interpretation {
    pub(crate) fn parse(p: &Value, cmd: &str) -> Result<Interpretation> {
        let mut it = Interpretation { rate: f_p(p, "frameRate").map(FrameRate::from_f64), ..Default::default() };
        if str_p(p, "frameRate") == Some("file") {
            it.native = true;
        }
        match str_p(p, "alpha") {
            Some("straight") => it.alpha = Some(AlphaMode::Straight),
            Some("premultiplied") => it.alpha = Some(AlphaMode::Premultiplied),
            Some("ignore") => it.alpha = Some(AlphaMode::Ignore),
            Some("guess") => it.guess = true,
            Some(a) => return Err(bad(cmd, format!("alpha: straight|premultiplied|ignore|guess, not `{a}`"))),
            None => {}
        }
        it.guess |= super::b_p(p, "guessAlpha").unwrap_or(false);
        it.matte = match p.get("matteColor").or(p.get("premulColor")) {
            None => None,
            Some(v) => Some(
                effectcraft_keyframe::Value::Color([0.0, 0.0, 0.0, 1.0])
                    .coerce_json(v)
                    .map(|c| {
                        let c = c.as_color();
                        [c[0], c[1], c[2]]
                    })
                    .ok_or_else(|| bad(cmd, "matteColor: #hex or [r, g, b] (0..1)"))?,
            ),
        };
        it.invert_alpha = super::b_p(p, "invertAlpha");
        it.loops = p.get("loop").and_then(Value::as_u64).map(|v| v.clamp(1, 9999) as u32);
        it.par = f_p(p, "pixelAspect").filter(|x| *x > 0.0);
        it.fields = match str_p(p, "fields") {
            Some(f) => Some(effectcraft_project::FieldOrder::parse(f).ok_or_else(|| bad(cmd, "fields: off|upper|lower"))?),
            None => None,
        };
        // Color ▸ Assign Profile: a colour space id, or "auto"/"none" for the file's own (sRGB).
        it.profile = match str_p(p, "colorProfile") {
            Some("auto" | "none" | "") => Some(None),
            Some(c) => match effectcraft_project::ColorSpace::parse(c) {
                Some(cs) => Some(Some(cs)),
                None => return Err(bad(cmd, format!("colorProfile: srgb|rec709|rec2020|p3|auto, not `{c}`"))),
            },
            None => None,
        };
        it.linear = super::b_p(p, "linearLight");
        // Start Frame: a frame number, or "file" for the first file's.
        it.start = match p.get("startFrame") {
            None | Some(Value::Null) => None,
            Some(Value::String(f)) if f == "file" => Some(None),
            Some(v) => Some(Some(
                v.as_f64()
                    .filter(|x| x.is_finite() && x.abs() <= 1.0e9)
                    .map(|x| x.round() as i64)
                    .ok_or_else(|| bad(cmd, "startFrame: a frame number (up to 1e9), or \"file\""))?,
            )),
        };
        Ok(it)
    }

    pub(crate) fn apply(&self, f: &mut Footage, guessed: Option<(AlphaMode, [f32; 3])>) {
        let before = f.frame_rate;
        self.apply_settings(f, guessed);
        // A sequence's length is its frames (Start Frame decides how many).
        if f.kind == FootageKind::Sequence && !f.sequence.is_empty() {
            f.sync_sequence_duration();
            return;
        }
        // Conforming keeps the frames and changes how long they last (12 frames at 30 fps are
        // 0.4 s; conformed to 12 fps, 1 s). Stills have no frame rate of their own.
        let (old, new) = (before.as_f64(), f.frame_rate.as_f64());
        if matches!(f.kind, FootageKind::Video | FootageKind::Sequence) && old > 0.0 && new > 0.0 && (old - new).abs() > 1e-9 {
            let frames = (f.duration.seconds() * old).round() as i64;
            f.duration = f.frame_rate.tick_of(frames);
        }
    }

    fn apply_settings(&self, f: &mut Footage, guessed: Option<(AlphaMode, [f32; 3])>) {
        if self.native {
            if let Some(n) = f.native_rate.take() {
                f.frame_rate = n;
            }
        } else if let Some(r) = self.rate {
            if f.native_rate.is_none() {
                f.native_rate = Some(f.frame_rate);
            }
            f.frame_rate = r;
        }
        if let Some(a) = self.alpha {
            f.alpha = a;
        }
        if let Some((a, m)) = guessed {
            f.alpha = a;
            f.premul_color = m;
        }
        if let Some(m) = self.matte {
            f.premul_color = m;
        }
        if let Some(v) = self.invert_alpha {
            f.invert_alpha = v;
        }
        if let Some(l) = self.loops {
            f.loop_count = l;
        }
        if let Some(x) = self.par {
            f.pixel_aspect = x;
        }
        if let Some(x) = self.fields {
            f.fields = x;
        }
        if let Some(c) = self.profile {
            f.color_profile = c;
        }
        if let Some(v) = self.linear {
            f.linear_light = v;
        }
        if f.kind == FootageKind::Sequence
            && let Some(start) = self.start
        {
            // The first file's own number needn't be stored (it follows the files).
            let first = f.sequence.first().and_then(|p| effectcraft_project::sequence::frame_number(p));
            f.start_frame = start.filter(|n| Some(*n) != first);
        }
    }
}

/// Interpret Footage ▸ Alpha ▸ Guess: read the first frame as straight colour and decide
/// whether the file is premultiplied (every partly transparent pixel's colour within its alpha
/// of a black or white matte) or straight. `None` when the frame has no partial alpha.
pub(crate) fn guess_alpha(s: &Session, item: ItemId, f: &Footage) -> Option<(AlphaMode, [f32; 3])> {
    let probe = Footage { alpha: AlphaMode::Straight, invert_alpha: false, ..f.clone() };
    let img = s.footage.frame(item, &probe, Tick::ZERO)?;
    let (mut partial, mut black, mut white) = (0usize, true, true);
    let eps = 1.5 / 255.0;
    for px in img.data.iter() {
        let a = px[3];
        if a <= eps || a >= 1.0 - eps {
            continue;
        }
        partial += 1;
        for c in &px[..3] {
            // The file's colour (the decoder premultiplied it as straight).
            let file = c / a;
            black &= file <= a + eps;
            white &= file >= 1.0 - a - eps;
        }
    }
    if partial == 0 {
        return None;
    }
    Some(if black {
        (AlphaMode::Premultiplied, [0.0; 3])
    } else if white {
        (AlphaMode::Premultiplied, [1.0; 3])
    } else {
        (AlphaMode::Straight, f.premul_color)
    })
}

fn interpret(s: &mut Session, p: &Value) -> Result<Value> {
    let items = items_p(s, p);
    if items.is_empty() {
        return Err(bad("file.interpretFootage", "select footage"));
    }
    let it = Interpretation::parse(p, "file.interpretFootage")?;
    let guesses: Vec<Option<(AlphaMode, [f32; 3])>> = items
        .iter()
        .map(|i| match (it.guess, s.project.item(*i).map(|x| &x.kind)) {
            (true, Some(ItemKind::Footage(f))) => guess_alpha(s, *i, f),
            _ => None,
        })
        .collect();
    s.edit("Interpret Footage", None, |proj, _| {
        for (i, g) in items.iter().zip(&guesses) {
            let Some(ItemKind::Footage(f)) = proj.item_mut(*i).map(|x| &mut x.kind) else { continue };
            let old = f.duration;
            it.apply(f, *g);
            let new = f.duration;
            if new == old {
                continue;
            }
            // Layers that ran to the footage's end still do (After Effects re-times them too).
            let ids: Vec<ItemId> = proj.comps().map(|(id, _)| *id).collect();
            for cid in ids {
                for l in proj.comp_mut(cid).into_iter().flat_map(|c| c.layers.iter_mut()) {
                    if l.source.item() == Some(*i) && (l.stretch - 100.0).abs() < 1e-9 && l.out_point == l.start_time + old {
                        l.out_point = l.start_time + new;
                    }
                }
            }
        }
        Ok(())
    })?;
    let out: Vec<Value> = items
        .iter()
        .filter_map(|i| match s.project.item(*i).map(|x| &x.kind) {
            Some(ItemKind::Footage(f)) => Some(json!({
                "item": i.0, "alpha": format!("{:?}", f.alpha), "matteColor": f.premul_color, "invertAlpha": f.invert_alpha,
                "fields": f.fields.label(), "pixelAspect": f.pixel_aspect, "loop": f.loop_count, "frameRate": f.frame_rate.as_f64(),
                "linearLight": f.linear_light, "startFrame": f.first_frame_number(),
                "frames": f.sequence_frames(),
            })),
            _ => None,
        })
        .collect();
    Ok(json!({"items": out}))
}

fn remember_interpretation(s: &mut Session, p: &Value) -> Result<Value> {
    let item = *items_p(s, p).first().ok_or_else(|| bad("file.rememberInterpretation", "select footage"))?;
    if let Some(ItemKind::Footage(f)) = s.project.item(item).map(|x| &x.kind) {
        s.state.interpretation = Some(f.clone());
    }
    Ok(Value::Null)
}

fn apply_interpretation(s: &mut Session, p: &Value) -> Result<Value> {
    let src = s.state.interpretation.clone().ok_or_else(|| bad("file.applyInterpretation", "no interpretation remembered"))?;
    let items = items_p(s, p);
    s.edit("Apply Interpretation", None, |proj, _| {
        for i in &items {
            if let Some(ItemKind::Footage(f)) = proj.item_mut(*i).map(|x| &mut x.kind) {
                f.alpha = src.alpha;
                f.premul_color = src.premul_color;
                f.pixel_aspect = src.pixel_aspect;
                f.loop_count = src.loop_count;
                f.fields = src.fields;
                f.invert_alpha = src.invert_alpha;
                f.linear_light = src.linear_light;
                f.color_profile = src.color_profile;
                if src.native_rate.is_some() {
                    if f.native_rate.is_none() {
                        f.native_rate = Some(f.frame_rate);
                    }
                    f.frame_rate = src.frame_rate;
                }
            }
        }
        Ok(())
    })?;
    Ok(json!({"items": items.len()}))
}

fn replace_footage(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("file.replaceFootage", "missing `path`"))?.to_string();
    let item = *items_p(s, p).first().ok_or_else(|| bad("file.replaceFootage", "select footage"))?;
    let importer = s.importer.clone().ok_or_else(|| EngineError::Other("media import is not available in this build".into()))?;
    // A numbered still brings its image sequence, as on import (`sequence: false` for one frame).
    let opts = SequenceOptions::from_params(p);
    let src = crate::sequence::source_of(s.services.as_ref(), &path, opts);
    let f = crate::sequence::probe(importer.as_ref(), &src, s.sequence_rate()).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    let name = src.name();
    s.edit("Replace Footage", None, |proj, _| {
        let it = proj.item_mut(item).ok_or_else(|| bad("file.replaceFootage", "no such item"))?;
        it.name = name;
        it.kind = ItemKind::Footage(f);
        retarget_layers(proj, item, |_| LayerSource::Footage { item });
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Point every layer using `item` at a new source kind.
fn retarget_layers(proj: &mut Project, item: ItemId, src: impl Fn(&LayerSource) -> LayerSource) {
    let ids: Vec<ItemId> = proj.comps().map(|(id, _)| *id).collect();
    for cid in ids {
        if let Some(c) = proj.comp_mut(cid) {
            for l in &mut c.layers {
                if l.source.item() == Some(item) {
                    l.source = src(&l.source);
                }
            }
        }
    }
}

fn replace_with_placeholder(s: &mut Session, p: &Value) -> Result<Value> {
    let item = *items_p(s, p).first().ok_or_else(|| bad("file.replaceWithPlaceholder", "select footage"))?;
    let base = match s.project.item(item).map(|x| &x.kind) {
        Some(ItemKind::Footage(f)) => json!({"width": f.width, "height": f.height, "frameRate": f.frame_rate.as_f64(), "duration": f.duration.seconds()}),
        _ => json!({}),
    };
    let mut merged = base;
    if let (Some(m), Some(pm)) = (merged.as_object_mut(), p.as_object()) {
        for (k, v) in pm {
            m.insert(k.clone(), v.clone());
        }
    }
    let f = placeholder_footage(&merged);
    s.edit("Replace Footage", None, |proj, _| {
        let it = proj.item_mut(item).ok_or_else(|| bad("file.replaceWithPlaceholder", "no such item"))?;
        it.kind = ItemKind::Footage(f);
        retarget_layers(proj, item, |_| LayerSource::Footage { item });
        Ok(())
    })?;
    Ok(Value::Null)
}

fn replace_with_solid(s: &mut Session, p: &Value) -> Result<Value> {
    let item = *items_p(s, p).first().ok_or_else(|| bad("file.replaceWithSolid", "select footage"))?;
    let mut so = solid_from(p, s);
    if let Some((w, h)) = s.project.item(item).and_then(|x| x.dimensions())
        && p.get("width").is_none()
    {
        so.width = w.max(1);
        so.height = h.max(1);
    }
    s.edit("Replace Footage", None, |proj, _| {
        let it = proj.item_mut(item).ok_or_else(|| bad("file.replaceWithSolid", "no such item"))?;
        it.kind = ItemKind::Solid(so);
        retarget_layers(proj, item, |_| LayerSource::Solid { item });
        Ok(())
    })?;
    Ok(Value::Null)
}

fn reload(s: &mut Session, p: &Value) -> Result<Value> {
    let items = items_p(s, p);
    if s.importer.is_none() {
        return Err(EngineError::Other("media import is not available in this build".into()));
    }
    let mut updates = vec![];
    for i in &items {
        if let Some(ItemKind::Footage(f)) = s.project.item(*i).map(|x| &x.kind) {
            // An image sequence picks up frames added to (or removed from) its run.
            let mut nf = match s.probe_footage(&f.path) {
                Ok(n) => n,
                Err(_) => Footage { missing: true, ..f.clone() },
            };
            // Keep the interpretation.
            nf.alpha = f.alpha;
            nf.loop_count = f.loop_count;
            nf.pixel_aspect = f.pixel_aspect;
            if nf.kind == FootageKind::Sequence && f.kind == FootageKind::Sequence {
                nf.frame_rate = f.frame_rate;
                nf.native_rate = f.native_rate;
                nf.alphabetical = f.alphabetical;
                nf.start_frame = f.start_frame;
                nf.sync_sequence_duration();
            }
            updates.push((*i, nf));
        }
    }
    let n = updates.len();
    s.edit("Reload Footage", None, |proj, _| {
        for (i, f) in updates {
            if let Some(it) = proj.item_mut(i) {
                it.kind = ItemKind::Footage(f);
            }
        }
        Ok(())
    })?;
    s.events.push(crate::Event::PurgeCaches);
    Ok(json!({"reloaded": n}))
}

fn reveal_in_finder(s: &mut Session, p: &Value) -> Result<Value> {
    let item = *items_p(s, p).first().ok_or_else(|| bad("file.revealInFinder", "select footage"))?;
    let path = match s.project.item(item).map(|x| &x.kind) {
        Some(ItemKind::Footage(f)) if !f.path.is_empty() => f.path.clone(),
        _ => return Err(bad("file.revealInFinder", "the item is not a file")),
    };
    let url = super::layer_menu::folder_url(&path);
    s.events.push(crate::Event::OpenUrl(url.clone()));
    Ok(json!({"url": url}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("project.newFolder", "New Folder", ["File", "New"], Some("Cmd+Alt+Shift+N"), "{name?, parent?}", always, new_folder),
        cmd!("file.close", "Close", ["File"], Some("Cmd+W"), "{comp?}", has_comp, close),
        cmd!("file.closeProject", "Close Project", ["File"], None, "{}", always, close_project),
        cmd!("file.saveCopy", "Save a Copy...", ["File", "Save As"], None, "{path}", always, save_copy),
        cmd!("file.importMultiple", "Multiple Files...", ["File", "Import"], Some("Cmd+Alt+I"), "{paths: [string]}", always, |s, p| s
            .execute("file.import", p.clone())),
        cmd!(
            "file.importPlaceholder",
            "Placeholder...",
            ["File", "Import"],
            None,
            "{name?, width?, height?, frameRate?, duration? (s)}",
            always,
            import_placeholder
        ),
        cmd!("file.importSolid", "Solid...", ["File", "Import"], None, "{name?, color?, width?, height?}", always, import_solid),
        cmd!(
            "file.newCompFromSelection",
            "New Comp from Selection...",
            ["File"],
            Some("Cmd+Alt+\\"),
            "{duration? (s, for stills), single?: bool (one comp for all), dimensionsFrom?: index, sequence?: bool, overlap?: bool, overlapDuration? (s), transition?: off|dissolveFront|crossDissolve, addToRenderQueue?: bool} → {comps}",
            has_project_selection,
            new_comp_from_selection
        ),
        cmd!("file.collectFiles", "Collect Files...", ["File", "Dependencies"], None, "{folder}", has_items, collect_files),
        cmd!("file.consolidateFootage", "Consolidate All Footage", ["File", "Dependencies"], None, "{}", has_items, consolidate),
        cmd!("file.removeUnusedFootage", "Remove Unused Footage", ["File", "Dependencies"], None, "{}", has_items, remove_unused),
        cmd!(
            "file.reduceProject",
            "Reduce Project",
            ["File", "Dependencies"],
            None,
            "{} (keeps the selected comps and what they use)",
            has_comp_selection,
            reduce
        ),
        cmd!("file.findMissing", "Find Missing", [], None, "{what: footage|effects|fonts}", has_items, find_missing),
        cmd!(
            "footage.check",
            "Check Footage",
            [],
            None,
            "{items?: [id], wait?} — look for every footage file (in the background unless `wait`) and flag missing items",
            has_items,
            crate::footage_check::command
        ),
        cmd!(
            "file.runScript",
            "Run Script File...",
            ["File", "Scripts"],
            None,
            "{path (.jsx/.js JavaScript, or a .jsonl/.json command script) | name (an installed or sample script, see file.scripts.list) | steps: [{command, params}]}",
            always,
            run_script
        ),
        cmd!(
            "script.run",
            "Run Script",
            [],
            None,
            "{code (JavaScript: app, app.project, CompItem, Layer, Property… like After Effects scripting), name?, console? (Script Console context)} → {ok, result, output, error: {message, line, column} | null}",
            always,
            script_run
        ),
        cmd!(
            "file.interpretFootage",
            "Main...",
            ["File", "Interpret Footage"],
            Some("Cmd+Alt+G"),
            "{items?, frameRate?: fps|\"file\", alpha?: straight|premultiplied|ignore|guess, guessAlpha?, matteColor?, invertAlpha?, loop?, pixelAspect?, fields?: off|upper|lower, colorProfile?: srgb|rec709|rec2020|p3|auto, linearLight?, startFrame?: number|\"file\" (image sequences: the frame number at the footage's first frame)}",
            has_footage_selection,
            interpret
        ),
        cmd!(
            "file.rememberInterpretation",
            "Remember Interpretation",
            ["File", "Interpret Footage"],
            None,
            "{item?}",
            has_footage_selection,
            remember_interpretation
        ),
        cmd!(
            "file.applyInterpretation",
            "Apply Interpretation",
            ["File", "Interpret Footage"],
            Some("Cmd+Alt+V"),
            "{items?}",
            has_interpretation,
            apply_interpretation
        ),
        cmd!(
            "file.replaceFootage",
            "File...",
            ["File", "Replace Footage"],
            Some("Cmd+H"),
            "{path, item?, sequence?: bool (default true: a numbered still brings its image sequence), alphabetical?: bool}",
            has_footage_selection,
            replace_footage
        ),
        cmd!(
            "file.replaceWithPlaceholder",
            "Placeholder...",
            ["File", "Replace Footage"],
            None,
            "{item?, width?, height?, frameRate?, duration?}",
            has_footage_selection,
            replace_with_placeholder
        ),
        cmd!(
            "file.replaceWithSolid",
            "Solid...",
            ["File", "Replace Footage"],
            None,
            "{item?, color?, width?, height?}",
            has_footage_selection,
            replace_with_solid
        ),
        cmd!("file.reloadFootage", "Reload Footage", ["File"], Some("Cmd+Alt+L"), "{items?}", has_footage_selection, reload),
        cmd!("file.revealInFinder", "Reveal in Finder", ["File"], None, "{item?}", has_footage_selection, reveal_in_finder),
    ]
}
