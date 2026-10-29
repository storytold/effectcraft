//! File ▸ Scripts and ScriptUI.
//!
//! * **Installed scripts** live in the settings store under `Scripts/` (File ▸ Scripts ▸ Install
//!   Script File…) and `Scripts/ScriptUI Panels/` (Install ScriptUI Panel…), next to the
//!   settings on the desktop, so dropping `.jsx` files in those folders works too. File ▸ Scripts
//!   lists them with the bundled sample scripts and extensions ([`BUNDLED`]) and runs them by name; ScriptUI
//!   panels appear at the bottom of the Window menu and open as dockable panels
//!   (`window.scriptPanel`), running with `this` = the panel as in After Effects.
//! * **ScriptUI windows** (`scriptui.*`): list the open script windows, read their control
//!   trees, click buttons and set values like a user, close them (see [`crate::scriptui`]).
//! * **Script settings** (`script.settings.*`): After Effects' `app.settings`, string values by
//!   section and key that last between runs ([`SCRIPT_SETTINGS_FILE`]).

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, str_p};
use crate::{EngineError, Event, Result, Session, cmd, scriptui};

/// The settings-store folders of installed scripts.
pub const SCRIPTS_DIR: &str = "Scripts";
pub const PANELS_DIR: &str = "Scripts/ScriptUI Panels";
/// The ScriptUI panels open when EffectCraft last quit (settings store), reopened at launch.
pub const OPEN_PANELS_FILE: &str = "scriptui_panels.json";
/// Scripts' `app.settings` in the settings store: `{section: {key: value}}`, as After Effects
/// keeps them in its preferences.
pub const SCRIPT_SETTINGS_FILE: &str = "script_settings.json";
/// User presets of the core Ease Presets panel of v0.6.0 (#254) and where the Ease Presets
/// ScriptUI panel that replaced it keeps them: (file, section, key). The file's text moves there
/// unchanged (the panel reads the same JSON) the first time settings load.
const LEGACY_EASE_PRESETS: (&str, &str, &str) = ("ease_presets.json", "Ease Presets", "userPresets");
/// Longest section, key and value a script may store (characters / bytes).
const MAX_SETTING_NAME: usize = 256;
const MAX_SETTING_VALUE: usize = 1 << 20;

/// Scripts' `app.settings`: values by section and key.
pub type ScriptSettings = BTreeMap<String, BTreeMap<String, String>>;

/// Load scripts' settings from the settings store (a corrupt file gives none), moving user ease
/// presets saved before Ease Presets became a ScriptUI panel into its section.
pub(crate) fn load_script_settings(s: &mut Session) {
    let Some(cfg) = s.config.clone() else { return };
    s.script_settings = cfg
        .read(SCRIPT_SETTINGS_FILE)
        .and_then(|t| serde_json::from_str(&t).map_err(|e| log::warn!("{SCRIPT_SETTINGS_FILE} is not valid ({e}); scripts start without their settings")).ok())
        .unwrap_or_default();
    let (file, section, key) = LEGACY_EASE_PRESETS;
    if let Some(text) = cfg.read(file)
        && !s.script_settings.get(section).is_some_and(|m| m.contains_key(key))
    {
        s.script_settings.entry(section.into()).or_default().insert(key.into(), text);
        if let Err(e) = store_script_settings(s) {
            log::warn!("cannot move {file} into the Ease Presets panel's settings: {e}");
        }
    }
}

fn store_script_settings(s: &Session) -> Result<()> {
    let Some(cfg) = &s.config else { return Ok(()) };
    let text = serde_json::to_string_pretty(&s.script_settings).map_err(|e| EngineError::Other(format!("script settings: {e}")))?;
    cfg.write(SCRIPT_SETTINGS_FILE, &text).map_err(|e| EngineError::Other(format!("cannot save script settings: {e}")))
}

fn setting_name<'a>(p: &'a Value, k: &str, c: &str) -> Result<&'a str> {
    let v = str_p(p, k).ok_or_else(|| bad(c, format!("missing `{k}`")))?;
    if v.chars().count() > MAX_SETTING_NAME {
        return Err(bad(c, format!("`{k}` is longer than {MAX_SETTING_NAME} characters")));
    }
    Ok(v)
}

/// `script.settings.get {section, key}` → `{have, value}` (`value` "" when there is none).
fn settings_get(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "script.settings.get";
    let (section, key) = (setting_name(p, "section", C)?, setting_name(p, "key", C)?);
    let v = s.script_settings.get(section).and_then(|m| m.get(key));
    Ok(json!({"have": v.is_some(), "value": v.cloned().unwrap_or_default()}))
}

/// `script.settings.save {section, key, value}`: kept in the settings store.
fn settings_save(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "script.settings.save";
    let (section, key) = (setting_name(p, "section", C)?, setting_name(p, "key", C)?);
    let value = match p.get("value") {
        Some(Value::String(v)) => v.clone(),
        Some(v) if !v.is_null() => v.to_string(),
        _ => return Err(bad(C, "missing `value`")),
    };
    if value.len() > MAX_SETTING_VALUE {
        return Err(bad(C, format!("`value` is larger than {MAX_SETTING_VALUE} bytes")));
    }
    s.script_settings.entry(section.into()).or_default().insert(key.into(), value);
    store_script_settings(s)?;
    Ok(Value::Null)
}

/// Scripts that ship with EffectCraft (original work): (file name, ScriptUI panel, source, code).
/// `sample`s show how to script EffectCraft; `extension`s (in `extensions/`) are optional tools
/// built on the public scripting API, outside the core, like third-party scripts in After Effects.
pub const BUNDLED: &[(&str, bool, &str, &str)] = &[
    ("Create Null at Selected Layers.jsx", false, "sample", include_str!("../../scripts/Create Null at Selected Layers.jsx")),
    ("Rename Layers.jsx", false, "sample", include_str!("../../scripts/Rename Layers.jsx")),
    ("Render Queue Batch.jsx", false, "sample", include_str!("../../scripts/Render Queue Batch.jsx")),
    ("Sort Layers by In Point.jsx", false, "sample", include_str!("../../scripts/Sort Layers by In Point.jsx")),
    ("Layer Tools.jsx", true, "sample", include_str!("../../scripts/ScriptUI Panels/Layer Tools.jsx")),
    ("Ease Presets.jsx", true, "extension", include_str!("../../../../extensions/scriptui-panels/Ease Presets.jsx")),
];

/// A script File ▸ Scripts (or the Window menu, for panels) offers.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ScriptEntry {
    /// File name (`Rename Layers.jsx`): what the menus show and commands take.
    pub name: String,
    /// In the ScriptUI Panels folder: opens as a dockable panel.
    pub panel: bool,
    /// `installed`, `sample` or `extension` (bundled, see [`BUNDLED`]).
    pub source: &'static str,
}

fn is_script(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l.ends_with(".jsx") || l.ends_with(".js")
}

/// Whether `name` can name an installed script: a bare file name, never a path (names are joined
/// onto the Scripts folders of the settings store, which takes them as relative paths), and on
/// Windows none of the names [`windows_unsafe_name`] lists.
fn is_script_name(name: &str) -> bool {
    is_bare_name(name) && !(cfg!(windows) && windows_unsafe_name(name))
}

/// One path component on every platform: not empty, `.` or `..`, and without `/`, `\` or NUL.
pub(crate) fn is_bare_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

/// Names that are not ordinary file names on Windows (elsewhere they are): with `:` (a drive or a
/// stream), ending in `.` or a space (Windows drops them), or a device name, which names a device
/// in any folder and with any extension (`con.jsx`): `CON`, `PRN`, `AUX`, `NUL`, `CONIN$`,
/// `CONOUT$`, `COM0`–`COM9` and `LPT0`–`LPT9` (also with `¹²³`).
pub(crate) fn windows_unsafe_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or_default().trim_end_matches(' ').to_ascii_uppercase();
    let port = |p: &str| {
        let mut n = stem.strip_prefix(p).unwrap_or_default().chars();
        matches!((n.next(), n.next()), (Some('0'..='9' | '¹' | '²' | '³'), None))
    };
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") || port("COM") || port("LPT");
    name.contains(':') || name.ends_with(['.', ' ']) || device
}

/// Installed scripts and panels, then the bundled ones not shadowed by an installed one, by name.
pub fn scripts(s: &Session) -> Vec<ScriptEntry> {
    let mut v: Vec<ScriptEntry> = vec![];
    if let Some(cfg) = &s.config {
        for (dir, panel) in [(SCRIPTS_DIR, false), (PANELS_DIR, true)] {
            for n in cfg.list(dir).into_iter().filter(|n| is_script(n) && is_script_name(n)) {
                v.push(ScriptEntry { name: n, panel, source: "installed" });
            }
        }
    }
    for (name, panel, source, _) in BUNDLED {
        if !v.iter().any(|e| e.name == *name && e.panel == *panel) {
            v.push(ScriptEntry { name: name.to_string(), panel: *panel, source });
        }
    }
    v.sort_by(|a, b| a.panel.cmp(&b.panel).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    v
}

/// The code of an installed or bundled script / panel.
pub fn script_code(s: &Session, name: &str, panel: Option<bool>) -> Option<(bool, String)> {
    if !is_script(name) || !is_script_name(name) {
        return None;
    }
    if let Some(cfg) = &s.config {
        for (dir, is_panel) in [(SCRIPTS_DIR, false), (PANELS_DIR, true)] {
            if panel.is_some_and(|p| p != is_panel) {
                continue;
            }
            if let Some(code) = cfg.read(&format!("{dir}/{name}")) {
                return Some((is_panel, code));
            }
        }
    }
    BUNDLED.iter().find(|(n, p, ..)| *n == name && panel.is_none_or(|w| w == *p)).map(|(_, p, _, c)| (*p, c.to_string()))
}

fn install(s: &mut Session, p: &Value, panel: bool) -> Result<Value> {
    let c = if panel { "file.installScriptUIPanel" } else { "file.installScript" };
    let path = str_p(p, "path").ok_or_else(|| bad(c, "missing `path` (a .jsx / .js script)"))?;
    let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if !is_script(&name) {
        return Err(bad(c, "scripts are .jsx or .js files (.jsxbin is not supported)"));
    }
    // A name it could not be run or uninstalled by (`\` is a file-name character on Unix).
    if !is_script_name(&name) {
        return Err(bad(c, format!("`{name}` is not a script file name")));
    }
    let bytes = s.services.read_file(path).map_err(|e| EngineError::Other(format!("cannot read {path}: {e}")))?;
    let code = String::from_utf8(bytes).map_err(|_| bad(c, "the script is not UTF-8 text"))?;
    let cfg = s.config.clone().ok_or_else(|| EngineError::Other("this session has no settings folder to install scripts into".into()))?;
    let dir = if panel { PANELS_DIR } else { SCRIPTS_DIR };
    cfg.write(&format!("{dir}/{name}"), &code).map_err(|e| EngineError::Other(format!("cannot install {name}: {e}")))?;
    // After Effects asks for a restart; our menus pick it up at once.
    s.toast(if panel {
        format!("Installed ScriptUI panel {name}: find it at the bottom of the Window menu")
    } else {
        format!("Installed script {name}: find it in File ▸ Scripts")
    });
    Ok(json!({"name": name, "panel": panel}))
}

fn list(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!(scripts(s)))
}

fn uninstall(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "file.uninstallScript";
    let name = str_p(p, "name").ok_or_else(|| bad(c, "missing `name`"))?;
    if !is_script(name) || !is_script_name(name) {
        return Err(bad(c, format!("`{name}` is not a script file name")));
    }
    let cfg = s.config.clone().ok_or_else(|| bad(c, "no settings folder"))?;
    let mut removed = false;
    for dir in [SCRIPTS_DIR, PANELS_DIR] {
        let key = format!("{dir}/{name}");
        if cfg.read(&key).is_some() {
            cfg.remove(&key).map_err(|e| EngineError::Other(e.to_string()))?;
            removed = true;
        }
    }
    if removed { Ok(json!({"removed": name})) } else { Err(bad(c, format!("`{name}` is not installed"))) }
}

/// Run an installed or bundled script by name (File ▸ Scripts ▸ `<name>`). A ScriptUI panel run
/// this way gets `this` = a floating palette-like panel window. A script listed under File ▸
/// Scripts runs before a panel of the same name.
pub(crate) fn run_named(s: &mut Session, name: &str) -> Result<Value> {
    let found = script_code(s, name, Some(false)).or_else(|| script_code(s, name, None));
    let (panel, code) = found.ok_or_else(|| bad("file.runScript", format!("no installed or bundled script `{name}`")))?;
    if panel {
        return open_panel(s, &json!({"name": name}));
    }
    let result = super::file_more::run_js_named(s, &code, name)?;
    super::file_more::remember_script(s, crate::prefs::RecentScript::Name(name.to_string()));
    Ok(result)
}

/// The code a ScriptUI panel runs with: `this` is a dockable Panel titled after the script.
pub fn panel_wrapper(title: &str, code: &str) -> String {
    // On one line, so line numbers in errors stay those of the file.
    format!("(function () {{ {code}\n}}).call(__uiDockPanel({}));", Value::String(title.to_string()))
}

/// The ScriptUI panels open now (script names).
pub(crate) fn open_panel_names(s: &Session) -> Vec<String> {
    s.script_ui.windows.iter().filter(|w| w.kind == scriptui::WindowKind::Panel).map(|w| w.script.clone()).collect()
}

/// The panels open when EffectCraft last quit (`{"open": [names]}`); a missing or unreadable
/// file is none.
fn remembered_panels(s: &Session) -> Vec<String> {
    let text = s.config.as_ref().and_then(|c| c.read(OPEN_PANELS_FILE)).unwrap_or_default();
    let doc: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    doc.get("open").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).filter(|n| is_script(n)).map(str::to_string).collect()
}

/// Change the remembered panels with `f` (written only when it changed). The list is kept so
/// the next launch reopens them, as After Effects reopens the panels of its workspace; quitting
/// leaves it as it is.
fn edit_remembered_panels(s: &Session, f: impl FnOnce(&mut Vec<String>)) {
    let Some(cfg) = &s.config else { return };
    let before = remembered_panels(s);
    let mut list = before.clone();
    f(&mut list);
    if list != before
        && let Err(e) = cfg.write(OPEN_PANELS_FILE, &json!({"open": list}).to_string())
    {
        log::warn!("cannot remember the open ScriptUI panels: {e}");
    }
}

/// Forget the panels of `before` (open before a script window event) that are closed now.
pub(crate) fn forget_closed_panels(s: &Session, before: &[String]) {
    let now = open_panel_names(s);
    let closed: Vec<&String> = before.iter().filter(|n| !now.contains(n)).collect();
    if !closed.is_empty() {
        edit_remembered_panels(s, |l| l.retain(|n| !closed.contains(&n)));
    }
}

/// `window.restoreScriptPanels`: open the ScriptUI panels that were open when EffectCraft last
/// quit (frontends call it once at launch). Panels whose script is gone or fails are reported
/// and forgotten; the others still open.
fn restore_panels(s: &mut Session, _: &Value) -> Result<Value> {
    let (mut opened, mut failed) = (vec![], vec![]);
    for name in remembered_panels(s) {
        let r = match script_code(s, &name, Some(true)) {
            Some(_) => open_panel(s, &json!({"name": name})).map_err(|e| e.to_string()),
            None => Err("no such ScriptUI panel".to_string()),
        };
        match r {
            Ok(_) => opened.push(name),
            Err(error) => {
                edit_remembered_panels(s, |l| l.retain(|n| *n != name));
                failed.push(json!({"name": name, "error": error}));
            }
        }
    }
    Ok(json!({"opened": opened, "failed": failed}))
}

/// Window ▸ `<ScriptUI panel>`: run the panel script (or bring its panel forward).
fn open_panel(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "window.scriptPanel";
    let name = str_p(p, "name").ok_or_else(|| bad(c, "missing `name` (a script in the ScriptUI Panels folder)"))?.to_string();
    let title = name.trim_end_matches(".jsx").trim_end_matches(".js").to_string();
    if let Some(w) = s.script_ui.windows.iter().find(|w| w.kind == scriptui::WindowKind::Panel && w.script == name) {
        let id = w.id;
        s.events.push(Event::Frontend { command: c.into(), params: json!({"window": id}) });
        return Ok(json!({"window": id, "open": true}));
    }
    let (_, code) = script_code(s, &name, Some(true)).or_else(|| script_code(s, &name, None)).ok_or_else(|| bad(c, format!("no ScriptUI panel `{name}`")))?;
    let out = super::file_more::run_js_named(s, &panel_wrapper(&title, &code), &name)?;
    let id = s.script_ui.windows.iter().rev().find(|w| w.kind == scriptui::WindowKind::Panel && w.script == name).map(|w| w.id);
    if let Some(id) = id {
        s.events.push(Event::Frontend { command: c.into(), params: json!({"window": id}) });
    }
    if id.is_some() {
        edit_remembered_panels(s, |l| {
            if !l.contains(&name) {
                l.push(name.clone());
            }
        });
    }
    Ok(json!({"window": id, "result": out}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "file.installScript",
            "Install Script File...",
            ["File", "Scripts"],
            None,
            "{path (.jsx / .js)} → copies it to the Scripts folder; it appears in File ▸ Scripts",
            always,
            |s, p| install(s, p, false)
        ),
        cmd!(
            "file.installScriptUIPanel",
            "Install ScriptUI Panel...",
            ["File", "Scripts"],
            None,
            "{path (.jsx / .js)} → copies it to the ScriptUI Panels folder; it appears in the Window menu",
            always,
            |s, p| install(s, p, true)
        ),
        cmd!("file.uninstallScript", "Uninstall Script", [], None, "{name}", always, uninstall),
        crate::query!("file.scripts.list", "List Scripts", "{} → [{name, panel, source: installed|sample|extension}]", list),
        cmd!(
            "window.scriptPanel",
            "ScriptUI Panel",
            [],
            None,
            "{name (a script in the ScriptUI Panels folder, e.g. `Layer Tools.jsx`)} → opens it as a dockable panel",
            always,
            open_panel
        ),
        cmd!(
            "window.restoreScriptPanels",
            "Restore ScriptUI Panels",
            [],
            None,
            "{} → reopens the ScriptUI panels open when EffectCraft last quit (frontends call it at launch) → {opened: [name], failed: [{name, error}]}",
            always,
            restore_panels
        ),
        crate::query!("script.settings.get", "Get Script Setting", "{section, key} → {have, value} (app.settings.haveSetting / getSetting)", settings_get),
        cmd!(
            "script.settings.save",
            "Save Script Setting",
            [],
            None,
            "{section, key, value (text)} → kept in the settings store (app.settings.saveSetting)",
            always,
            settings_save
        ),
        cmd!(
            "script.runTasks",
            "Run Scheduled Script Tasks",
            [],
            None,
            "{} → runs the app.scheduleTask tasks that are due (frontends do it every frame) → {ran, output, errors, next (ms until the next task, or null)}",
            always,
            scriptui::run_tasks
        ),
        crate::query!("scriptui.list", "List Script Windows", "{} → [{window, title, kind: dialog|palette|window|panel, script, modal, size}]", scriptui::list),
        crate::query!(
            "scriptui.get",
            "Script Window Controls",
            "{window?: id | title} → {id, title, kind, root: {id, type, name, text, value, checked, items, selection, bounds, enabled, draw (onDraw paint list), children…}}",
            scriptui::get
        ),
        cmd!(
            "scriptui.click",
            "Click Script Window Control",
            [],
            None,
            "{window?: id | title, widget: id | \"#id\" | properties.name | text} (buttons, checkboxes, radio buttons, tabs)",
            always,
            scriptui::click
        ),
        cmd!(
            "scriptui.set",
            "Set Script Window Control",
            [],
            None,
            "{window?, widget, value: text | number | bool | item index | item text, changing?: bool (a live update: each keystroke / slider step; fires onChanging only)} (edit text, sliders, checkboxes, lists) → fires onChanging / onChange",
            always,
            scriptui::set
        ),
        cmd!(
            "scriptui.mouse",
            "Mouse Event on Script Window Control",
            [],
            None,
            "{window?, widget, type: mousedown | mouseup | mousemove | mouseover | mouseout | click (default), x?, y? (relative to the control; default its centre), button? (0 left, 1 middle, 2 right), detail? (click count), screenX?, screenY?, altKey?, ctrlKey?, metaKey?, shiftKey?} → the script's addEventListener listeners on the control and its containers receive a MouseEvent",
            always,
            scriptui::mouse
        ),
        cmd!(
            "scriptui.close",
            "Close Script Window",
            [],
            None,
            "{window?, result? (a dialog's show() returns it; default 2 = Cancel)}",
            always,
            scriptui::close
        ),
    ]
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use serde_json::{Value, json};

    use super::{BUNDLED, PANELS_DIR, SCRIPTS_DIR, is_bare_name, is_script, is_script_name, script_code, windows_unsafe_name};
    use crate::config::DirConfig;
    use crate::{EngineError, Result, ScriptRequest, Session};

    /// Stands in for the JavaScript runtime: reports the code it was asked to run.
    fn echo(_: &mut Session, r: &ScriptRequest) -> Value {
        json!({"ran": r.code})
    }

    /// A session whose settings store is a fresh `<root>/settings/` (with both Scripts folders, as
    /// after installing a script and a panel), next to a script `<root>/outside.jsx`.
    fn session_in_tmp(name: &str) -> (Session, PathBuf) {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!("ec-scripts-{}-{}-{name}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("settings").join(PANELS_DIR)).unwrap();
        std::fs::write(root.join("outside.jsx"), "outside").unwrap();
        let s = Session { config: Some(Arc::new(DirConfig::new(root.join("settings")))), script: Some(echo), ..Default::default() };
        (s, root)
    }

    fn bad_params(r: &Result<Value>) -> bool {
        matches!(r, Err(EngineError::BadParams { .. }))
    }

    /// Paths, refused on every platform.
    const PATHS: [&str; 10] = ["", ".", "..", "../x.jsx", "a/b.jsx", "/abs/x.jsx", "a\\b.jsx", "..\\x.jsx", "C:\\x.jsx", "a\0.jsx"];
    /// Ordinary file names except on Windows.
    const WINDOWS_ONLY: [&str; 8] = ["Step 1: Setup.jsx", "In:Out.jsx", "C:x.jsx", "x.jsx.", "x.jsx ", "Aux.Tools.jsx", "con.jsx", "COM1.jsx"];

    #[test]
    fn bare_names_are_one_path_component() {
        for name in ["Rename Layers.jsx", "..jsx", "a..b.js", "Äpfel ✓.jsx"].into_iter().chain(WINDOWS_ONLY) {
            assert!(is_bare_name(name), "{name:?}");
        }
        for name in PATHS {
            assert!(!is_bare_name(name), "{name:?}");
        }
    }

    #[test]
    fn windows_unsafe_names_are_drives_streams_trailing_dots_and_devices() {
        let devices = ["CON", "Con.tar.jsx", "CON .jsx", "PRN.js", "aux.jsx", "NUL.jsx", "CONIN$.jsx", "conout$.js"];
        let ports = ["COM0.jsx", "com9.js", "LPT0.jsx", "lpt9.jsx", "COM¹.jsx", "LPT³.js"];
        for name in WINDOWS_ONLY.into_iter().chain(devices).chain(ports) {
            assert!(windows_unsafe_name(name), "{name:?}");
        }
        let fine = ["console.jsx", "Concert.jsx", "NULL.jsx", "CONIN.jsx", "COM10.jsx", "LPT.jsx", "x .jsx", "..jsx"];
        for name in BUNDLED.iter().map(|b| b.0).chain(fine) {
            assert!(!windows_unsafe_name(name), "{name:?}");
        }
    }

    #[test]
    fn script_names_are_bare_file_names() {
        let more = ["..jsx", "a..b.js", "Äpfel ✓.jsx", "console.jsx", "Concert.jsx", "NULL.jsx", "COM10.jsx", "LPT.jsx"];
        for name in BUNDLED.iter().map(|b| b.0).chain(more) {
            assert!(is_script(name) && is_script_name(name), "{name}");
        }
        for name in PATHS {
            assert!(!is_script_name(name), "{name:?}");
        }
        for name in WINDOWS_ONLY {
            assert_eq!(is_script_name(name), !cfg!(windows), "{name:?}");
        }
        // Install refuses a file name the script could not then be run or uninstalled by.
        #[cfg(unix)]
        {
            let (mut s, root) = session_in_tmp("odd-name");
            let odd = root.join("a\\b.jsx");
            std::fs::write(&odd, "x").unwrap();
            assert!(bad_params(&s.execute("file.installScript", json!({"path": odd.to_string_lossy()}))));
            assert!(!root.join("settings").join(SCRIPTS_DIR).join("a\\b.jsx").exists());
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn uninstall_takes_a_script_name_not_a_path() {
        let (mut s, root) = session_in_tmp("uninstall");
        let (outside, prefs) = (root.join("outside.jsx"), root.join("settings").join("prefs.json"));
        std::fs::write(&prefs, "{}").unwrap();
        // From `Scripts/`, `../../outside.jsx` is the script next to the settings folder and
        // `../prefs.json` a settings file; an absolute path is not a script name either.
        for name in ["../../outside.jsx".to_string(), "../prefs.json".into(), outside.to_string_lossy().into_owned()] {
            let r = s.execute("file.uninstallScript", json!({"name": name}));
            assert!(outside.is_file() && prefs.is_file(), "`{name}` deleted a file outside the Scripts folders: {r:?}");
            assert!(bad_params(&r), "`{name}`: {r:?}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scripts_run_by_name_come_from_the_scripts_folders() {
        let (mut s, root) = session_in_tmp("run");
        // `Scripts/../../outside.jsx` and `Scripts/ScriptUI Panels/../../../outside.jsx` are the
        // script next to the settings folder.
        let r = s.execute("file.runScript", json!({"name": "../../outside.jsx"}));
        assert!(bad_params(&r), "{r:?}");
        let r = s.execute("window.scriptPanel", json!({"name": "../../../outside.jsx"}));
        assert!(bad_params(&r), "{r:?}");
        assert_eq!(script_code(&s, "../../outside.jsx", None), None);
        assert_eq!(script_code(&s, "../../../outside.jsx", Some(true)), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// File ▸ Scripts ▸ Recent Script Files: the ten scripts run last, newest first, kept in the
    /// settings; Cmd+Alt+Shift+D runs the first one again.
    #[test]
    fn recent_script_files_run_again() {
        use crate::prefs::{MAX_RECENT_SCRIPTS, PREFS_FILE, Prefs, RecentScript};
        let (mut s, root) = session_in_tmp("recent");
        assert_eq!(crate::menus::dynamic(&s, "recentScripts", &Default::default()), (vec![], Some("No Recent Script Files")));
        assert!(bad_params(&s.execute("file.runScript", json!({"recent": 0}))));
        assert_eq!(s.shortcuts().shortcut_of("file.runScript", &json!({"recent": 0})), Some("Cmd+Alt+Shift+D"));
        let tool = root.join("Tool.jsx");
        std::fs::write(&tool, "tool").unwrap();
        let tool = tool.to_string_lossy().into_owned();
        s.execute("file.runScript", json!({"path": tool})).unwrap();
        s.execute("file.runScript", json!({"name": "Rename Layers.jsx"})).unwrap();
        // A ScriptUI panel opened from File ▸ Scripts is not a recent script file.
        std::fs::write(root.join("settings").join(PANELS_DIR).join("Panel.jsx"), "panel").unwrap();
        s.execute("file.runScript", json!({"name": "Panel.jsx"})).unwrap();
        let (path, name) = (RecentScript::Path(tool.clone()), RecentScript::Name("Rename Layers.jsx".into()));
        assert_eq!(s.prefs.recent_scripts, [name.clone(), path.clone()]);
        let (e, _) = crate::menus::dynamic(&s, "recentScripts", &Default::default());
        assert_eq!(e.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(), ["Rename Layers.jsx", "Tool.jsx"]);
        assert_eq!((e[1].command.as_str(), &e[1].params), ("file.runScript", &json!({"recent": 1})));
        // Running one again runs that file and moves it to the top.
        assert_eq!(s.execute("file.runScript", json!({"recent": 1})).unwrap()["ran"], "tool");
        assert_eq!(s.prefs.recent_scripts, [path.clone(), name.clone()]);
        let sample = BUNDLED.iter().find(|b| b.0 == "Rename Layers.jsx").unwrap().3;
        assert_eq!(s.execute("file.runScript", json!({"recent": 1})).unwrap()["ran"], sample);
        assert!(bad_params(&s.execute("file.runScript", json!({"recent": 2}))));
        assert!(bad_params(&s.execute("file.runScript", json!({"recent": "Tool.jsx"}))));
        // A file named like a bundled script runs again as that file.
        let local = root.join("Rename Layers.jsx");
        std::fs::write(&local, "local").unwrap();
        s.execute("file.runScript", json!({"path": local.to_string_lossy()})).unwrap();
        assert_eq!(s.execute("file.runScript", json!({"recent": 0})).unwrap()["ran"], "local");
        assert_eq!(s.execute("file.runScript", json!({"recent": 1})).unwrap()["ran"], sample);
        // Ten at most, saved with the settings.
        let mut last = String::new();
        for i in 0..MAX_RECENT_SCRIPTS + 2 {
            let f = root.join(format!("Step {i}.jsonl"));
            std::fs::write(&f, "").unwrap();
            last = f.to_string_lossy().into_owned();
            s.execute("file.runScript", json!({"path": last})).unwrap();
        }
        assert_eq!(s.prefs.recent_scripts.len(), MAX_RECENT_SCRIPTS);
        assert_eq!(s.prefs.recent_scripts[0], RecentScript::Path(last));
        let saved = std::fs::read_to_string(root.join("settings").join(PREFS_FILE)).unwrap();
        assert_eq!(Prefs::from_json(&saved).recent_scripts, s.prefs.recent_scripts);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recent_script_history_uses_the_selected_script() {
        let (mut s, root) = session_in_tmp("recent-panel-collision");
        let name = "Rename Layers.jsx";
        std::fs::write(root.join("settings").join(PANELS_DIR).join(name), "panel").unwrap();
        assert!(super::scripts(&s).iter().any(|entry| entry.name == name && !entry.panel));
        assert!(script_code(&s, name, None).unwrap().0);
        let sample = BUNDLED.iter().find(|b| b.0 == name).unwrap().3;
        assert_eq!(s.execute("file.runScript", json!({"name": name})).unwrap()["ran"], sample);
        assert!(super::open_panel_names(&s).is_empty(), "the listed script runs instead of the panel");
        assert_eq!(s.prefs.recent_scripts, [crate::prefs::RecentScript::Name(name.to_string())]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recent_script_history_keeps_successful_entries_when_scripts_fail() {
        fn fail_or_echo(_: &mut Session, request: &ScriptRequest) -> Value {
            if request.code == "fail" { json!({"error": {"message": "script failed"}}) } else { json!({"ran": request.code}) }
        }
        let (mut s, root) = session_in_tmp("recent-failed");
        s.script = Some(fail_or_echo);
        let good = root.join("good.jsonl");
        std::fs::write(&good, "[]").unwrap();
        s.execute("file.runScript", json!({"path": good})).unwrap();
        let remembered = s.prefs.recent_scripts.clone();
        for (name, code) in [("parse.jsonl", "{"), ("step.jsonl", r#"{"command":"does.not.exist"}"#), ("missing-command.jsonl", "{}"), ("runtime.jsx", "fail")]
        {
            let path = root.join(name);
            std::fs::write(&path, code).unwrap();
            assert!(s.execute("file.runScript", json!({"path": path})).is_err(), "{name}");
            assert_eq!(s.prefs.recent_scripts, remembered, "{name}");
        }
        std::fs::write(root.join("settings").join(SCRIPTS_DIR).join("Failed.jsx"), "fail").unwrap();
        assert!(s.execute("file.runScript", json!({"name": "Failed.jsx"})).is_err());
        assert_eq!(s.prefs.recent_scripts, remembered);
        let _ = std::fs::remove_dir_all(root);
    }

    /// Reads relative paths from a working directory that a test can change.
    struct InFolder(std::sync::Mutex<PathBuf>);

    impl InFolder {
        fn at(&self, path: &str) -> PathBuf {
            self.0.lock().unwrap().join(path)
        }
    }

    impl crate::Services for InFolder {
        fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
            std::fs::read(self.at(path))
        }
        fn write_file(&self, path: &str, data: &[u8]) -> std::io::Result<()> {
            std::fs::write(self.at(path), data)
        }
        fn full_path(&self, path: &str) -> Option<String> {
            Some(self.at(path).to_string_lossy().into_owned())
        }
    }

    /// Reads from a folder but can't make paths absolute.
    struct NoFullPath(PathBuf);

    impl crate::Services for NoFullPath {
        fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
            std::fs::read(self.0.join(path))
        }
        fn write_file(&self, path: &str, data: &[u8]) -> std::io::Result<()> {
            std::fs::write(self.0.join(path), data)
        }
        fn full_path(&self, _path: &str) -> Option<String> {
            None
        }
    }

    /// A script whose path can't be made absolute still runs, and Recent Script Files does not
    /// keep its relative path.
    #[test]
    fn unresolved_script_paths_are_not_remembered() {
        let (mut s, root) = session_in_tmp("recent-unresolved");
        std::fs::write(root.join("Tool.jsx"), "a").unwrap();
        std::fs::write(root.join("steps.jsonl"), "[]").unwrap();
        s.services = Arc::new(NoFullPath(root.clone()));
        assert_eq!(s.execute("file.runScript", json!({"path": "Tool.jsx"})).unwrap()["ran"], "a");
        s.execute("file.runScript", json!({"path": "steps.jsonl"})).unwrap();
        assert!(s.prefs.recent_scripts.is_empty(), "{:?}", s.prefs.recent_scripts);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A script run from a relative path runs the same file again from Recent Script Files after
    /// the working directory changes, and two scripts with the same file name show their paths.
    #[test]
    fn recent_script_files_keep_the_file_that_ran() {
        let (mut s, root) = session_in_tmp("recent-relative");
        let (a, b) = (root.join("a"), root.join("b"));
        for (dir, code) in [(&a, "a"), (&b, "b")] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("Tool.jsx"), code).unwrap();
        }
        let services = Arc::new(InFolder(std::sync::Mutex::new(a.clone())));
        s.services = services.clone();
        assert_eq!(s.execute("file.runScript", json!({"path": "Tool.jsx"})).unwrap()["ran"], "a");
        *services.0.lock().unwrap() = b.clone();
        assert_eq!(s.execute("file.runScript", json!({"recent": 0})).unwrap()["ran"], "a");
        let (e, _) = crate::menus::dynamic(&s, "recentScripts", &Default::default());
        assert_eq!(e.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(), ["Tool.jsx"]);
        assert_eq!(s.execute("file.runScript", json!({"path": "Tool.jsx"})).unwrap()["ran"], "b");
        let (e, _) = crate::menus::dynamic(&s, "recentScripts", &Default::default());
        let (a, b) = (a.join("Tool.jsx").to_string_lossy().into_owned(), b.join("Tool.jsx").to_string_lossy().into_owned());
        assert_eq!(e.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(), [b.as_str(), a.as_str()]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Native script paths are kept absolute.
    #[test]
    fn native_full_paths_are_absolute() {
        use crate::Services;
        assert!(crate::FsServices.full_path("Tool.jsx").is_some_and(|p| std::path::Path::new(&p).is_absolute()));
    }

    #[test]
    fn only_scripts_are_read_or_removed() {
        // A settings file in the Scripts folder that is not a script (.jsx / .js).
        let (mut s, root) = session_in_tmp("not-a-script");
        let notes = root.join("settings").join(SCRIPTS_DIR).join("notes.txt");
        std::fs::write(&notes, "notes").unwrap();
        assert_eq!(script_code(&s, "notes.txt", None), None);
        let r = s.execute("file.uninstallScript", json!({"name": "notes.txt"}));
        assert!(notes.is_file() && bad_params(&r), "{r:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// On Unix `\` is a file-name character: a script dropped into the folder under such a name is
    /// not offered, as it could not be run or uninstalled. Names only Windows refuses stay.
    #[cfg(unix)]
    #[test]
    fn listing_skips_names_run_and_uninstall_refuse() {
        let (mut s, root) = session_in_tmp("odd-names");
        let scripts = root.join("settings").join(SCRIPTS_DIR);
        std::fs::write(scripts.join("a\\b.jsx"), "x").unwrap();
        std::fs::write(scripts.join("In:Out.jsx"), "in-out").unwrap();
        let list = s.execute("file.scripts.list", json!({})).unwrap();
        let listed = |n: &str| list.as_array().unwrap().iter().any(|e| e["name"] == n);
        assert!(!listed("a\\b.jsx") && listed("In:Out.jsx"), "{list}");
        assert_eq!(script_code(&s, "a\\b.jsx", None), None);
        assert_eq!(script_code(&s, "In:Out.jsx", None), Some((false, "in-out".to_string())));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn install_run_and_uninstall_by_name() {
        let (mut s, root) = session_in_tmp("install");
        let file = root.join("Hello.jsx");
        std::fs::write(&file, "hello").unwrap();
        s.execute("file.installScript", json!({"path": file.to_string_lossy()})).unwrap();
        s.execute("file.installScriptUIPanel", json!({"path": file.to_string_lossy()})).unwrap();
        let installed = [root.join("settings").join(SCRIPTS_DIR).join("Hello.jsx"), root.join("settings").join(PANELS_DIR).join("Hello.jsx")];
        assert!(installed.iter().all(|p| p.is_file()));
        assert_eq!(s.execute("file.runScript", json!({"name": "Hello.jsx"})).unwrap()["ran"], "hello");
        let r = s.execute("window.scriptPanel", json!({"name": "Hello.jsx"})).unwrap();
        assert!(r["result"]["ran"].as_str().is_some_and(|c| c.contains("hello")), "{r}");
        // The bundled samples still run by name.
        let sample = BUNDLED.iter().find(|b| b.0 == "Rename Layers.jsx").unwrap().3;
        assert_eq!(s.execute("file.runScript", json!({"name": "Rename Layers.jsx"})).unwrap()["ran"], sample);
        s.execute("file.uninstallScript", json!({"name": "Hello.jsx"})).unwrap();
        assert!(installed.iter().all(|p| !p.exists()));
        assert!(bad_params(&s.execute("file.uninstallScript", json!({"name": "Hello.jsx"}))), "no longer installed");
        let _ = std::fs::remove_dir_all(&root);
    }
}
