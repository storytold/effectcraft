//! Menu entries of M14.5: Edit ▸ History, File ▸ Import Recent Footage, Replace Footage ▸ With
//! Layered Comp, Save a Copy As XML, Animation ▸ Recent Animation Presets, Composition ▸ VR
//! (Create VR Environment, Extract Cubemap), the "Assign Shortcut to …" submenus, Help ▸ Enable
//! Logging / Reveal Logging File / System Compatibility Report, and `File.execute()` for scripts.

use effectcraft_color::Label;
use effectcraft_geom::{Mat4, Vec3, vec3};
use effectcraft_project::{Comp, FootageKind, ItemId, ItemKind, LayerSource};
use effectcraft_time::{FrameRate, Tick};
use serde_json::{Value, json};

use super::{CommandSpec, always, b_p, bad, comp_id, has_comp, has_layers, has_project_selection, str_p};
use crate::{EngineError, Event, Result, Session, cmd, query};

/// Run `f` (which may run several undoable commands) as one undo step named `label`; on error
/// the project, editor state and undo history are restored.
///
/// Front-end events emitted by a failed `f` are discarded with the state. Pending-analysis
/// queues and the journal are not part of this rollback and keep their existing behaviour.
pub(crate) fn grouped<T>(s: &mut Session, label: &str, f: impl FnOnce(&mut Session) -> Result<T>) -> Result<T> {
    // The steps `f` records go to an empty stack (so the undo limit can't drop the snapshot
    // taken before them), then become one step on the real one. Keep the complete pre-`f`
    // history and editor state for a failure: rollback must not leave a partially retargeted
    // editor or a `f`-recorded redo/branch behind.
    let saved_history = s.history.clone();
    let saved = std::mem::take(&mut s.history.undo);
    let before = s.project.clone();
    let before_state = s.state.clone();
    let event_mark = s.events.len();
    let r = f(s);
    let changed = !s.history.undo.is_empty();
    let interim_revision = s.ime_revision;
    s.history.undo = saved;
    s.history.merge_key = None;
    match r {
        Ok(v) => {
            if changed {
                s.history.undo.push((label.to_string(), before));
                let levels = s.prefs.general.undo_levels.max(1) as usize;
                if s.history.undo.len() > levels {
                    let extra = s.history.undo.len() - levels;
                    s.history.undo.drain(..extra);
                }
            }
            Ok(v)
        }
        Err(e) => {
            // Snapshot the failing editor before restoring the pre-`f` state: an intermediate
            // text.edit / text change / comp switch must invalidate the pre-`f` editor token
            // instead of reviving it.
            let edited = super::text_edit::active_editor_snapshot(s);
            s.project = before;
            s.state = before_state;
            s.history = saved_history;
            s.history.merge_key = None;
            // A failed intermediate comp switch must not reach the front end through an
            // `OpenComp` event while its state has been rolled back.
            s.events.truncate(event_mark);
            s.sanitize_after_replacement(edited.as_ref());
            // As in the batch rollback, the IME revision is a high-water mark: leave the highest
            // intermediate stamp rather than rewinding to before `f`.
            s.ime_revision = s.ime_revision.max(interim_revision);
            if changed {
                s.bump();
            }
            Err(e)
        }
    }
}

fn id_of(v: &Value, key: &str) -> Result<u64> {
    v.get(key).and_then(Value::as_u64).ok_or_else(|| EngineError::Other(format!("expected `{key}` in {v}")))
}

// ---------------------------------------------------------------- Edit ▸ History

/// Edit ▸ History: `{steps: n}` undoes the last n steps (the entry n−1 of the list), `{redo: n}`
/// redoes n; without either, lists the steps (newest first).
fn history(s: &mut Session, p: &Value) -> Result<Value> {
    if let Some(n) = p.get("steps").and_then(Value::as_u64) {
        if n as usize > s.history.undo.len() {
            return Err(bad("edit.history", format!("only {} undo steps", s.history.undo.len())));
        }
        for _ in 0..n {
            s.undo();
        }
    } else if let Some(n) = p.get("redo").and_then(Value::as_u64) {
        if n as usize > s.history.redo.len() {
            return Err(bad("edit.history", format!("only {} redo steps", s.history.redo.len())));
        }
        for _ in 0..n {
            s.redo();
        }
    }
    let undo: Vec<&str> = s.history.undo.iter().rev().map(|(l, _)| l.as_str()).collect();
    let redo: Vec<&str> = s.history.redo.iter().rev().map(|(l, _)| l.as_str()).collect();
    Ok(json!({"undo": undo, "redo": redo}))
}

fn has_history(s: &Session) -> std::result::Result<(), String> {
    if s.history.undo.is_empty() && s.history.redo.is_empty() { Err("nothing to undo".into()) } else { Ok(()) }
}

// ---------------------------------------------------------------- recent footage / presets

fn import_recent(s: &mut Session, p: &Value) -> Result<Value> {
    let path = match (str_p(p, "path"), p.get("index").and_then(Value::as_u64)) {
        (Some(x), _) => x.to_string(),
        (None, Some(i)) => s.prefs.recent_footage.get(i as usize).cloned().ok_or_else(|| bad("file.importRecent", format!("no recent footage #{i}")))?,
        _ => s.prefs.recent_footage.first().cloned().ok_or_else(|| bad("file.importRecent", "no recent footage"))?,
    };
    super::file::import(s, &json!({"paths": [path]}))
}

fn has_recent_footage(s: &Session) -> std::result::Result<(), String> {
    if s.prefs.recent_footage.is_empty() { Err("no recent footage".into()) } else { Ok(()) }
}

fn clear_recent_footage(s: &mut Session, _: &Value) -> Result<Value> {
    s.prefs.recent_footage.clear();
    s.save_prefs();
    s.prefs_revision += 1;
    Ok(Value::Null)
}

fn apply_recent_preset(s: &mut Session, p: &Value) -> Result<Value> {
    let path = match (str_p(p, "path"), p.get("index").and_then(Value::as_u64)) {
        (Some(x), _) => x.to_string(),
        (None, Some(i)) => s.prefs.recent_presets.get(i as usize).cloned().ok_or_else(|| bad("anim.applyRecentPreset", format!("no recent preset #{i}")))?,
        _ => s.prefs.recent_presets.first().cloned().ok_or_else(|| bad("anim.applyRecentPreset", "no recent animation presets"))?,
    };
    let mut q = json!({"path": path});
    if let Some(l) = p.get("layers") {
        q["layers"] = l.clone();
    }
    s.execute("anim.applyPreset", q)
}

fn has_recent_presets(s: &Session) -> std::result::Result<(), String> {
    has_layers(s)?;
    if s.prefs.recent_presets.is_empty() { Err("no recent animation presets".into()) } else { Ok(()) }
}

fn clear_recent_presets(s: &mut Session, _: &Value) -> Result<Value> {
    s.prefs.recent_presets.clear();
    s.save_prefs();
    s.prefs_revision += 1;
    Ok(Value::Null)
}

// ---------------------------------------------------------------- File menu

fn save_copy_xml(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("file.saveCopyAsXml", "missing `path` (.ecprojx)"))?;
    let xml = crate::xml_project::to_xml(&s.project.to_file_json_with_view(&s.saved_view())?).map_err(EngineError::Other)?;
    s.services.write_file(path, xml.as_bytes()).map_err(|e| EngineError::Other(format!("cannot write {path}: {e}")))?;
    s.toast(format!("Saved an XML copy to {path}"));
    Ok(json!({"path": path, "bytes": xml.len()}))
}

/// File ▸ Replace Footage ▸ With Layered Comp: a layered (Photoshop) footage item becomes a
/// composition of its layers; every layer using the footage now uses the comp, and the footage
/// item goes. One undo step.
fn replace_with_layered_comp(s: &mut Session, p: &Value) -> Result<Value> {
    let item = p.get("item").and_then(Value::as_u64).map(ItemId).or_else(|| s.state.project_selection.first().copied());
    let item = item.ok_or_else(|| bad("file.replaceWithLayeredComp", "select a layered footage item"))?;
    let path = match s.project.item(item).map(|i| &i.kind) {
        Some(ItemKind::Footage(f)) if f.codec == "PSD" || f.path.to_ascii_lowercase().ends_with(".psd") => f.path.clone(),
        _ => return Err(bad("file.replaceWithLayeredComp", "the item is not layered footage (a Photoshop file)")),
    };
    let bytes = s.services.read_file(&path).map_err(|e| EngineError::Other(format!("cannot read {path}: {e}")))?;
    grouped(s, "Replace Footage", |s| {
        let (comp, _, _) = super::file::import_psd_comp(s, &path, bytes, false)?;
        let comp = ItemId(comp);
        s.edit("Replace Footage", None, |proj, st| {
            let ids: Vec<ItemId> = proj.items.keys().copied().collect();
            for cid in ids {
                if cid == comp {
                    continue;
                }
                if let Some(c) = proj.comp_mut(cid) {
                    for l in &mut c.layers {
                        if matches!(&l.source, LayerSource::Footage { item: i } if *i == item) {
                            l.source = LayerSource::Comp { item: comp };
                        }
                    }
                }
            }
            proj.items.remove(&item);
            st.project_selection = vec![comp];
            Ok(())
        })?;
        Ok(json!({"comp": comp.0}))
    })
}

/// File ▸ Replace Footage ▸ With Precomposition (also in the Project panel's context menu):
/// the footage or solid item goes into a new composition of its size, pixel aspect and frame
/// rate, placed in the same folder, and every layer in every composition that used the item now
/// uses that composition instead. The layers keep their timing, transforms, effects and masks;
/// the item stays in the project (the new composition uses it). The composition lasts as long as
/// the footage, or as long as the longest layer that showed it for stills and solids. One undo
/// step.
fn replace_with_precomp(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "file.replaceWithPrecomp";
    let item = p.get("item").and_then(Value::as_u64).map(ItemId).or_else(|| s.state.project_selection.first().copied());
    let item = item.ok_or_else(|| bad(CMD, "select a footage or solid item"))?;
    let it = s.project.item(item).ok_or_else(|| bad(CMD, "no such item"))?.clone();
    let (w, h, pixel_aspect, rate, own) = match &it.kind {
        ItemKind::Footage(f) if matches!(f.kind, FootageKind::Video | FootageKind::Still | FootageKind::Sequence) && f.has_video => {
            (f.width, f.height, f.pixel_aspect, it.frame_rate(), it.duration())
        }
        ItemKind::Solid(so) => (so.width, so.height, so.pixel_aspect, None, None),
        _ => return Err(bad(CMD, "the item is not visual footage or a solid")),
    };
    // Adjustment layers only borrow a solid for their size; they keep it.
    let is_user = |l: &effectcraft_project::Layer| {
        !l.switches.adjustment && matches!(l.source, LayerSource::Footage { item: i } | LayerSource::Solid { item: i } if i == item)
    };
    // The longest stretch of the item's own time any layer shows, and the first comp's rate.
    let mut used = Tick::ZERO;
    let mut used_rate = None;
    for (_, c) in s.project.comps() {
        for l in c.layers.iter().filter(|l| is_user(l)) {
            used = used.max(l.layer_time(l.in_point)).max(l.layer_time(l.out_point));
            used_rate.get_or_insert(c.frame_rate);
        }
    }
    let rate = rate.or(used_rate).unwrap_or(FrameRate::FPS_29_97);
    let fallback = Tick::from_seconds_f64(if s.prefs.import.still_footage == "seconds" { s.prefs.import.still_seconds } else { 10.0 });
    let dur = own.filter(|d| *d > Tick::ZERO).unwrap_or(if used > Tick::ZERO { used } else { fallback }).max(rate.frame_duration());
    let base = it.name.rsplit_once('.').map(|(a, _)| a.to_string()).unwrap_or(it.name.clone());
    let name = format!("{base} Comp");
    grouped(s, "Replace with Precomposition", |s| {
        let c = Comp { pixel_aspect, ..Comp::new(w.max(1), h.max(1), rate, dur) };
        let comp = s.edit("Replace with Precomposition", None, |proj, _| Ok(proj.add_item(&name, Label::Sandstone, it.parent, ItemKind::Comp(c.into()))))?;
        let inner = id_of(&s.execute("layer.addItem", json!({"comp": comp.0, "item": item.0, "time": 0.0, "duration": dur.seconds()}))?, "layer")?;
        let replaced = s.edit("Replace with Precomposition", None, |proj, st| {
            let mut n = 0u64;
            let ids: Vec<ItemId> = proj.items.keys().copied().filter(|c| *c != comp).collect();
            for cid in ids {
                if let Some(c) = proj.comp_mut(cid) {
                    for l in c.layers.iter_mut().filter(|l| is_user(l)) {
                        l.source = LayerSource::Comp { item: comp };
                        n += 1;
                    }
                }
            }
            st.project_selection = vec![comp];
            Ok(n)
        })?;
        Ok(json!({"comp": comp.0, "layer": inner, "replaced": replaced}))
    })
}

fn has_footage_or_solid(s: &Session) -> std::result::Result<(), String> {
    has_project_selection(s)?;
    let ok = s.state.project_selection.iter().any(|i| matches!(s.project.item(*i).map(|x| &x.kind), Some(ItemKind::Footage(_) | ItemKind::Solid(_))));
    if ok { Ok(()) } else { Err("select a footage or solid item".into()) }
}

fn has_footage(s: &Session) -> std::result::Result<(), String> {
    has_project_selection(s)?;
    let ok = s.state.project_selection.iter().any(|i| matches!(s.project.item(*i).map(|x| &x.kind), Some(ItemKind::Footage(_))));
    if ok { Ok(()) } else { Err("select a footage item".into()) }
}

/// `File.execute()` from scripts (and agents): open a file with its default application.
/// Needs Allow Scripts to Write Files and Access Network; with Warn User When Executing Files on,
/// the frontend asks first. `confirmed: true` is the frontend's answer to that question: a
/// script can't give it for itself (#1246), so a script's request always asks.
fn execute_file(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("file.executeFile", "missing `path`"))?;
    if !s.prefs.scripting.allow_scripts_write_files {
        return Err(bad(
            "file.executeFile",
            "scripts can't execute files unless Settings ▸ Scripting & Expressions ▸ Allow Scripts to Write Files and Access Network is on",
        ));
    }
    if !std::path::Path::new(path).exists() {
        return Err(bad("file.executeFile", format!("no file {path}")));
    }
    let confirmed = b_p(p, "confirmed").unwrap_or(false) && !s.running_for_script();
    if s.prefs.scripting.warn_executing_files && !confirmed {
        s.events.push(Event::Frontend { command: "app.confirmExecute".into(), params: json!({"path": path}) });
        return Ok(json!({"needsConfirmation": path}));
    }
    let url = super::layer_menu::file_url(path);
    s.events.push(Event::OpenUrl(url));
    Ok(json!({"opened": path}))
}

// ---------------------------------------------------------------- Assign Shortcut to …

/// The F10 / F11 / F12 (3D views) or Shift+F10 / F11 / F12 (workspaces) slots.
fn slot_p(p: &Value, cmd: &str, shift: bool) -> Result<String> {
    let raw = str_p(p, "slot").ok_or_else(|| bad(cmd, "missing `slot` (F10, F11 or F12)"))?;
    let k = raw.trim().trim_start_matches("Shift+").to_ascii_uppercase();
    if !matches!(k.as_str(), "F10" | "F11" | "F12") {
        return Err(bad(cmd, "slot: F10, F11 or F12"));
    }
    Ok(if shift { format!("Shift+{k}") } else { k })
}

/// Bind `keys` to the bindable `key`, taking it from whatever had it.
fn reassign(s: &mut Session, key: &str, slot: &str, cmd: &str) -> Result<Value> {
    let t = s.shortcuts();
    let b = t.find(key).cloned().ok_or_else(|| bad(cmd, format!("`{key}` can't have a shortcut")))?;
    let holders: Vec<(crate::shortcuts::Bindable, Vec<String>)> = t
        .bindables
        .iter()
        .filter(|x| x.key != key && t.keys.get(&x.key).is_some_and(|k| k.iter().any(|k| k == slot)))
        .map(|x| (x.clone(), t.keys.get(&x.key).cloned().unwrap_or_default()))
        .collect();
    let mine = t.keys.get(key).cloned().unwrap_or_default();
    let replaced: Vec<String> = holders.iter().map(|(h, _)| h.label.clone()).collect();
    for (h, keys) in &holders {
        let rest: Vec<String> = keys.iter().filter(|k| *k != slot).cloned().collect();
        s.keymaps.set(h, &rest).map_err(|e| bad(cmd, e))?;
    }
    let mut keys = vec![slot.to_string()];
    keys.extend(mine.into_iter().filter(|k| k != slot));
    let created = s.keymaps.set(&b, &keys).map_err(|e| bad(cmd, e))?;
    s.shortcuts_changed();
    if let Some(n) = &created {
        s.toast(format!("The default shortcuts can't be changed: created the preset \"{n}\""));
    }
    Ok(json!({"command": key, "keys": keys, "replaced": replaced, "preset": s.keymaps.active}))
}

/// The command id of a 3D view.
pub(crate) fn view_command(v: effectcraft_render::three_d::View3D) -> &'static str {
    use effectcraft_render::three_d::View3D::*;
    match v {
        ActiveCamera => "view.3d.activeCamera",
        Default => "view.3d.default",
        Front => "view.3d.front",
        Left => "view.3d.left",
        Top => "view.3d.top",
        Back => "view.3d.back",
        Right => "view.3d.right",
        Bottom => "view.3d.bottom",
        Custom1 => "view.3d.custom1",
        Custom2 => "view.3d.custom2",
        Custom3 => "view.3d.custom3",
    }
}

/// View ▸ Assign Shortcut to "<current 3D view>": F10, F11 or F12 switches to the view.
fn assign_view_shortcut(s: &mut Session, p: &Value) -> Result<Value> {
    let slot = slot_p(p, "view.assign3dShortcut", false)?;
    let cid = comp_id(s, p)?;
    let cur = s.state.views3d.get(&cid).map(|v| v.current).unwrap_or_default();
    reassign(s, view_command(cur), &slot, "view.assign3dShortcut")
}

/// Window ▸ Assign Shortcut to `"<workspace>"` Workspace: Shift+F10, Shift+F11 or Shift+F12.
/// The frontend fills in the current workspace.
fn assign_workspace_shortcut(s: &mut Session, p: &Value) -> Result<Value> {
    let slot = slot_p(p, "window.assignWorkspaceShortcut", true)?;
    let Some(ws) = str_p(p, "workspace") else { return super::frontend(s, "window.assignWorkspaceShortcut", p) };
    let key = crate::shortcuts::binding_key("window.workspace", &json!({"name": ws}));
    reassign(s, &key, &slot, "window.assignWorkspaceShortcut")
}

// ---------------------------------------------------------------- Help

fn log_file(s: &Session) -> std::path::PathBuf {
    crate::logging::log_path(s.config.as_ref().and_then(|c| c.dir()).as_deref())
}

fn enable_logging(s: &mut Session, p: &Value) -> Result<Value> {
    let on = b_p(p, "on").unwrap_or(!crate::logging::is_enabled());
    let file = log_file(s);
    crate::logging::set_enabled(on, file.clone()).map_err(|e| EngineError::Other(format!("cannot write {}: {e}", file.display())))?;
    s.prefs_revision += 1;
    s.toast(if on { format!("Logging to {}", file.display()) } else { "Logging off".into() });
    Ok(json!({"enabled": on, "path": file.to_string_lossy()}))
}

fn reveal_log(s: &mut Session, _: &Value) -> Result<Value> {
    // Enable Logging's file while it is on, else this run's session log.
    let file = crate::logging::current_file()
        .filter(|_| crate::logging::is_enabled())
        .or_else(crate::logging::session_file)
        .or_else(crate::logging::current_file)
        .unwrap_or_else(|| log_file(s));
    let path = file.to_string_lossy().to_string();
    let url = super::layer_menu::folder_url(&path);
    s.events.push(Event::OpenUrl(url.clone()));
    Ok(json!({"path": path, "url": url}))
}

/// Media formats File ▸ Import reads (with the desktop host's decoders).
pub const IMPORT_FORMATS: &[&str] = &[
    "MP4/MOV (H.264, HEVC, ProRes, AV1)",
    "WebM/MKV (VP8, VP9, AV1)",
    "PNG",
    "JPEG",
    "GIF",
    "WebP",
    "TIFF",
    "BMP",
    "OpenEXR",
    "Photoshop (PSD)",
    "SVG",
    "WAV",
    "AIFF",
    "MP3",
    "FLAC",
    "AAC",
    "Opus",
    "glTF/GLB",
    "OBJ",
    "STL",
    "FBX",
    "Lottie JSON",
];

/// Help ▸ System Compatibility Report: OS, CPU, memory, GPU adapter, import/export formats.
pub fn system_report(s: &Session) -> Value {
    let mem = crate::sysinfo::memory();
    let gb = |b: u64| format!("{:.1} GB", b as f64 / (1u64 << 30) as f64);
    let export: Vec<&str> = s.exporter.as_ref().map(|e| e.formats().into_iter().map(|f| f.label()).collect()).unwrap_or_default();
    let gpu = s.accel.as_ref().map(|a| a.name());
    let budgets = s.prefs.cache_budgets(mem.or(s.sys_memory));
    let mut issues = vec![];
    if gpu.is_none() {
        issues.push(match &s.accel_note {
            Some(why) => format!("No GPU compositor ({why}): rendering uses the CPU (all features still work)."),
            None => "No GPU compositor: rendering uses the CPU (all features still work).".to_string(),
        });
    }
    if mem.is_some_and(|m| m.total < 8 << 30) {
        issues.push("Less than 8 GB of memory: use lower preview resolutions for long or large comps.".to_string());
    }
    if s.exporter.is_none() {
        issues.push("Export is not available in this build.".to_string());
    }
    json!({
        "app": format!("EffectCraft {}", env!("CARGO_PKG_VERSION")),
        "os": crate::sysinfo::os_version(),
        "arch": std::env::consts::ARCH,
        "cpu": crate::sysinfo::cpu_name(),
        "cores": crate::sysinfo::cpu_cores(),
        "memory": mem.map(|m| json!({"total": gb(m.total), "available": gb(m.available)})),
        "gpu": gpu,
        "gpuNote": if gpu.is_none() { s.accel_note.clone() } else { None },
        "windowGraphics": s.window_adapter,
        "caches": {"layer": gb(budgets.layer as u64), "footage": gb(budgets.media as u64), "preview": gb(budgets.preview as u64)},
        "import": IMPORT_FORMATS,
        "export": export,
        "logging": crate::logging::is_enabled(),
        "recentWarnings": crate::logging::recent().into_iter().rev().take(20).collect::<Vec<_>>(),
        "issues": issues,
    })
}

/// The report as text (the dialog and `--` CLI output).
pub fn report_text(r: &Value) -> String {
    let st = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or("unknown").to_string();
    let list = |k: &str| r.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    let mut t = format!(
        "{}\nOperating system: {} ({})\nCPU: {} ({} logical cores)\n",
        st("app"),
        st("os"),
        st("arch"),
        st("cpu"),
        r.get("cores").and_then(Value::as_u64).unwrap_or(1)
    );
    match r.get("memory").filter(|m| !m.is_null()) {
        Some(m) => t.push_str(&format!("Memory: {} ({} available)\n", m["total"].as_str().unwrap_or("?"), m["available"].as_str().unwrap_or("?"))),
        None => t.push_str("Memory: unknown\n"),
    }
    match (r.get("gpu").and_then(Value::as_str), r.get("gpuNote").and_then(Value::as_str)) {
        (Some(gpu), _) => t.push_str(&format!("GPU: {gpu}\n")),
        (None, Some(why)) => t.push_str(&format!("GPU: none (CPU compositing): {why}\n")),
        (None, None) => t.push_str("GPU: none (CPU compositing)\n"),
    }
    if let Some(w) = r.get("windowGraphics").and_then(Value::as_str) {
        t.push_str(&format!("Window graphics: {w}\n"));
    }
    let c = &r["caches"];
    t.push_str(&format!(
        "Cache budgets: layer {}, footage {}, preview {}\n",
        c["layer"].as_str().unwrap_or("?"),
        c["footage"].as_str().unwrap_or("?"),
        c["preview"].as_str().unwrap_or("?")
    ));
    t.push_str(&format!("Import: {}\nExport: {}\n", list("import"), list("export")));
    let issues = list("issues");
    t.push_str(&if issues.is_empty() { "No compatibility issues found.".to_string() } else { format!("Issues: {issues}") });
    t
}

fn report(s: &mut Session, p: &Value) -> Result<Value> {
    let r = system_report(s);
    if !b_p(p, "quiet").unwrap_or(false) {
        s.events.push(Event::Frontend { command: "app.showReport".into(), params: json!({"title": "System Compatibility Report", "text": report_text(&r)}) });
    }
    Ok(r)
}

// ---------------------------------------------------------------- Composition ▸ VR

/// Orientation (degrees, AE order) of a camera whose +X, +Y (down) and +Z axes point along the
/// given world directions: searched over quarter turns, which is all the cube faces need.
pub(crate) fn orientation_for_axes(x: Vec3, y: Vec3, z: Vec3) -> [f64; 3] {
    let mut best = ([0.0; 3], f64::MAX);
    for a in 0..4 {
        for b in 0..4 {
            for c in 0..4 {
                let o = vec3(a as f64 * 90.0, b as f64 * 90.0, c as f64 * 90.0);
                let m = Mat4::orientation(o);
                let err = (m.apply_vec(vec3(1.0, 0.0, 0.0)) - x).length()
                    + (m.apply_vec(vec3(0.0, 1.0, 0.0)) - y).length()
                    + (m.apply_vec(vec3(0.0, 0.0, 1.0)) - z).length();
                if err < best.1 {
                    best = ([o.x, o.y, o.z], err);
                }
            }
        }
    }
    best.0
}

/// Cube faces in the VR Converter's order (+X, −X, +Y, −Y, +Z, −Z): name, forward, right and up
/// in the VR convention (X right, Y up, Z front).
pub(crate) const FACES: [(&str, [f64; 3], [f64; 3], [f64; 3]); 6] = [
    ("Right", [1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
    ("Left", [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
    ("Top", [0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
    ("Bottom", [0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
    ("Front", [0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ("Back", [0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
];
/// (column, row) of each face in the 3:2 cube map.
pub(crate) const CELLS_3X2: [(u32, u32); 6] = [(0, 0), (1, 0), (2, 0), (0, 1), (1, 1), (2, 1)];

/// VR world direction → After Effects world (Y down).
pub(crate) fn ae(v: [f64; 3]) -> Vec3 {
    vec3(v[0], -v[1], v[2])
}

/// Composition ▸ VR ▸ Create VR Environment: renders the active comp's 3D scene in every
/// direction. Six face comps (a 90° camera each at `position`, default the comp centre, looking
/// at the scene nested as a collapsed 3D precomp), a 3:2 cube map of them, and an equirectangular output comp
/// (VR Converter, cube map → equirectangular). One undo step.
fn create_vr_environment(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let src = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let name = s.project.item(cid).map(|i| i.name.clone()).unwrap_or_default();
    let size = p.get("size").and_then(Value::as_u64).unwrap_or(1024).clamp(64, 8192);
    let center = match p.get("position").and_then(Value::as_array) {
        Some(a) => [0, 1, 2].map(|i| a.get(i).and_then(Value::as_f64).unwrap_or(0.0)),
        None => [src.width as f64 / 2.0, src.height as f64 / 2.0, 0.0],
    };
    let fps = src.frame_rate.as_f64();
    let dur = src.duration.seconds();
    let r = grouped(s, "Create VR Environment", |s| {
        let mut faces = vec![];
        for (fname, fw, r, u) in FACES {
            let fc = id_of(
                &s.execute(
                    "comp.new",
                    json!({"name": format!("{name} VR {fname}"), "width": size, "height": size, "frameRate": fps, "duration": dur, "open": false}),
                )?,
                "comp",
            )?;
            let o = orientation_for_axes(ae(r), ae([-u[0], -u[1], -u[2]]), ae(fw));
            let cam =
                id_of(&s.execute("layer.newCamera", json!({"comp": fc, "name": format!("{fname} Camera"), "angleOfView": 90.0, "type": "oneNode"}))?, "layer")?;
            s.execute("prop.set", json!({"comp": fc, "layer": cam, "path": "transform/position", "value": center}))?;
            s.execute("prop.set", json!({"comp": fc, "layer": cam, "path": "transform/orientation", "value": o}))?;
            // The scene, nested as a collapsed 3D precomp whose layer maps the scene's
            // coordinates onto the face comp's unchanged (anchor = position): its 3D layers are
            // seen through the face camera and edits to the scene show in every face. Lights
            // inside a collapsed precomp do not light it (the containing comp's do), so the
            // scene's lights are copied next to it.
            let pre = id_of(&s.execute("layer.addItem", json!({"comp": fc, "item": cid.0, "time": 0.0}))?, "layer")?;
            let mid = [src.width as f64 / 2.0, src.height as f64 / 2.0, 0.0];
            s.execute("layer.setSwitch", json!({"comp": fc, "layers": [pre], "switch": "threeD", "value": true}))?;
            s.execute("layer.setSwitch", json!({"comp": fc, "layers": [pre], "switch": "collapse", "value": true}))?;
            s.execute("prop.set", json!({"comp": fc, "layer": pre, "path": "transform/anchor", "value": mid}))?;
            s.execute("prop.set", json!({"comp": fc, "layer": pre, "path": "transform/position", "value": mid}))?;
            let lights: Vec<effectcraft_project::Layer> = src.layers.iter().filter(|l| l.is_light()).cloned().collect();
            if !lights.is_empty() {
                s.edit("Create VR Environment", None, |proj, _| {
                    let mut next = proj.next_id;
                    let mut map = std::collections::BTreeMap::new();
                    let mut copies = vec![];
                    for l in &lights {
                        let mut c = l.clone();
                        super::edit::reid(&mut c, &mut next);
                        map.insert(l.id, c.id);
                        copies.push(c);
                    }
                    for c in &mut copies {
                        c.parent = c.parent.and_then(|p| map.get(&p).copied());
                    }
                    proj.next_id = next;
                    let comp = proj.comp_mut(ItemId(fc)).ok_or(EngineError::NoComp)?;
                    let at = comp.layers.len().saturating_sub(1);
                    for (k, c) in copies.into_iter().enumerate() {
                        comp.layers.insert(at + k, c);
                    }
                    Ok(())
                })?;
            }
            faces.push(fc);
        }
        let cube = id_of(
            &s.execute(
                "comp.new",
                json!({"name": format!("{name} VR Cube Map"), "width": size * 3, "height": size * 2, "frameRate": fps, "duration": dur, "open": false}),
            )?,
            "comp",
        )?;
        for (i, fc) in faces.iter().enumerate() {
            let l = id_of(&s.execute("layer.addItem", json!({"comp": cube, "item": fc, "time": 0.0}))?, "layer")?;
            let (c, rr) = CELLS_3X2[i];
            let pos = [(c as f64 + 0.5) * size as f64, (rr as f64 + 0.5) * size as f64];
            s.execute("prop.set", json!({"comp": cube, "layer": l, "path": "transform/position", "value": pos}))?;
        }
        let out = id_of(
            &s.execute(
                "comp.new",
                json!({"name": format!("{name} VR Output"), "width": size * 2, "height": size, "frameRate": fps, "duration": dur, "open": false}),
            )?,
            "comp",
        )?;
        let l = id_of(&s.execute("layer.addItem", json!({"comp": out, "item": cube, "time": 0.0}))?, "layer")?;
        s.execute("prop.set", json!({"comp": out, "layer": l, "path": "transform/scale", "value": [200.0 / 3.0, 50.0, 100.0]}))?;
        s.execute("effect.apply", json!({"comp": out, "layers": [l], "effect": "ec.vr.converter"}))?;
        s.execute("prop.set", json!({"comp": out, "layer": l, "path": "effects/#1/sourceProjection", "value": 2}))?;
        s.execute("prop.set", json!({"comp": out, "layer": l, "path": "effects/#1/targetProjection", "value": 0}))?;
        Ok(json!({"faces": faces, "cubeMap": cube, "output": out}))
    })?;
    s.open_comp(ItemId(r["output"].as_u64().unwrap_or(0)));
    Ok(r)
}

/// Composition ▸ VR ▸ Extract Cubemap: a 3:2 cube-map comp of an equirectangular comp (VR
/// Converter, equirectangular → cube map), for editing the faces. One undo step.
fn extract_cubemap(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let src = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let name = s.project.item(cid).map(|i| i.name.clone()).unwrap_or_default();
    let face = p.get("faceSize").and_then(Value::as_u64).unwrap_or((src.width / 4).max(16) as u64).clamp(16, 8192);
    let (w, h) = (face * 3, face * 2);
    let r = grouped(s, "Extract Cubemap", |s| {
        let cube = id_of(
            &s.execute(
                "comp.new",
                json!({"name": format!("{name} Cubemap"), "width": w, "height": h, "frameRate": src.frame_rate.as_f64(), "duration": src.duration.seconds(), "open": false}),
            )?,
            "comp",
        )?;
        let l = id_of(&s.execute("layer.addItem", json!({"comp": cube, "item": cid.0, "time": 0.0}))?, "layer")?;
        let scale = [w as f64 / src.width.max(1) as f64 * 100.0, h as f64 / src.height.max(1) as f64 * 100.0, 100.0];
        s.execute("prop.set", json!({"comp": cube, "layer": l, "path": "transform/scale", "value": scale}))?;
        s.execute("effect.apply", json!({"comp": cube, "layers": [l], "effect": "ec.vr.converter"}))?;
        s.execute("prop.set", json!({"comp": cube, "layer": l, "path": "effects/#1/sourceProjection", "value": 0}))?;
        s.execute("prop.set", json!({"comp": cube, "layer": l, "path": "effects/#1/targetProjection", "value": 2}))?;
        Ok(json!({"cubeMap": cube, "layer": l, "faceSize": face}))
    })?;
    s.open_comp(ItemId(r["cubeMap"].as_u64().unwrap_or(0)));
    Ok(r)
}

/// The specs for the commands in this module: history steps, recent footage and presets, Save
/// a Copy As XML, footage replacement and Execute File, 3D-view and workspace shortcuts,
/// logging and the system report, VR environment/cubemap creation and System Information.
pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("edit.history", "History", [], None, "{steps?: n (undo n steps), redo?: n}", has_history, history),
        cmd!("file.importRecent", "Import Recent Footage", [], None, "{index? | path?}", has_recent_footage, import_recent),
        cmd!("file.clearRecentFootage", "Clear Recent Footage", ["File", "Import Recent Footage"], None, "{}", has_recent_footage, clear_recent_footage),
        cmd!(
            "anim.applyRecentPreset",
            "Recent Animation Presets",
            [],
            Some("Cmd+Alt+Shift+F"),
            "{index? | path?, layers?}",
            has_recent_presets,
            apply_recent_preset
        ),
        cmd!(
            "anim.clearRecentPresets",
            "Clear Recent Presets",
            ["Animation", "Recent Animation Presets"],
            None,
            "{}",
            |s| if s.prefs.recent_presets.is_empty() { Err("no recent animation presets".into()) } else { Ok(()) },
            clear_recent_presets
        ),
        cmd!("file.saveCopyAsXml", "Save a Copy As XML...", ["File", "Save As"], None, "{path: .ecprojx}", always, save_copy_xml),
        cmd!("file.replaceWithLayeredComp", "With Layered Comp", ["File", "Replace Footage"], None, "{item?}", has_footage, replace_with_layered_comp),
        cmd!("file.replaceWithPrecomp", "Replace with Precomposition", [], None, "{item?}", has_footage_or_solid, replace_with_precomp),
        cmd!("file.executeFile", "Execute File", [], None, "{path, confirmed?}", always, execute_file),
        cmd!("view.assign3dShortcut", "Assign Shortcut to 3D View", [], None, "{slot: F10|F11|F12, comp?}", has_comp, assign_view_shortcut),
        cmd!(
            "window.assignWorkspaceShortcut",
            "Assign Shortcut to Workspace",
            [],
            None,
            "{slot: Shift+F10|Shift+F11|Shift+F12, workspace?}",
            always,
            assign_workspace_shortcut
        ),
        cmd!("help.enableLogging", "Enable Logging", ["Help"], None, "{on?}", always, enable_logging),
        cmd!("help.revealLogFile", "Reveal Logging File", ["Help"], None, "{}", always, reveal_log),
        cmd!("help.systemReport", "System Compatibility Report...", ["Help"], None, "{quiet?}", always, report),
        cmd!(
            "comp.vr.createEnvironment",
            "Create VR Environment...",
            ["Composition", "VR"],
            None,
            "{comp?, size?: face pixels (1024), position?: [x,y,z]}",
            has_comp,
            create_vr_environment
        ),
        cmd!("comp.vr.extractCubemap", "Extract Cubemap...", ["Composition", "VR"], None, "{comp?, faceSize?}", has_comp, extract_cubemap),
        query!("help.systemInfo", "System Information", "{}", |s, _| Ok(system_report(s))),
    ]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn grouped_rollback_restores_the_editor_state_and_history_after_a_transient_text_change() {
        let mut s = crate::Session::default();
        s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 1})).unwrap();
        let lid = s.execute("layer.newText", json!({"text": "abcdef"})).unwrap()["layer"].as_u64().unwrap();
        s.execute("text.edit", json!({"layer": lid, "caret": 3})).unwrap();
        let original_comp = s.active_comp_id().unwrap();
        let owner = s.state.text_edit.as_ref().unwrap().session;
        let other =
            s.execute("comp.new", json!({"name": "Other", "width": 160, "height": 90, "duration": 1, "open": false})).unwrap()["comp"].as_u64().unwrap();
        let undo = s.history.undo.len();
        s.drain_events();

        let r = super::grouped(&mut s, "Rollback", |s| -> crate::Result<()> {
            // An intermediate document edit and state switch, both undone by the error path.
            s.execute("layer.setText", json!({"layer": lid, "text": "ab"}))?;
            s.execute("comp.open", json!({"comp": other}))?;
            Err(crate::EngineError::Other("stop".into()))
        });
        assert!(r.is_err());
        assert_eq!(s.active_comp_id(), Some(original_comp), "the pre-group comp is restored");
        assert_eq!(super::super::text_edit::layer_doc(&s, effectcraft_project::LayerId(lid)).unwrap().text, "abcdef", "the document is restored");
        assert_eq!(s.history.undo.len(), undo, "the partial grouped steps are not left in history");
        assert_ne!(s.state.text_edit.as_ref().unwrap().session, owner, "a transient replacement does not revive the old IME token");
        assert!(
            !s.drain_events().iter().any(|e| matches!(e, crate::Event::OpenComp(_))),
            "the failed intermediate comp switch is not published to the front end"
        );
    }

    /// `File.execute()` opens the file as a well-formed `file://` URL (spaces, `#`, `%`).
    #[test]
    fn execute_file_opens_an_encoded_file_url() {
        let dir = std::env::temp_dir().join(format!("ec-exec-url-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("run #1 100%.txt");
        std::fs::write(&f, "x").unwrap();
        let mut s = crate::Session::default();
        s.execute("prefs.set", json!({"key": "scripting.allowScriptsWriteFiles", "value": true})).unwrap();
        s.drain_events();
        s.execute("file.executeFile", json!({"path": f.to_string_lossy(), "confirmed": true})).unwrap();
        let expected = super::super::layer_menu::file_url(&f.to_string_lossy());
        assert!(expected.ends_with("/run%20%231%20100%25.txt"), "{expected}");
        assert!(s.drain_events().iter().any(|e| matches!(e, crate::Event::OpenUrl(u) if *u == expected)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
