//! Plug-ins installed through the shared plug-in runtime, `craft-plugin` (RFC-0046): bundles
//! (`.craftplugin`) installed from a file, a catalog or a GitHub repository's releases into the
//! user's `Plug-ins` folder, in two kinds:
//!
//! - **command** plug-ins (the shared kind): WebAssembly modules that act on the project only
//!   through the same commands as the menus, the CLI, the control channel and MCP, inside
//!   craft-plugin's sandbox. This module is EffectCraft's side of it: which commands a plug-in may
//!   call ([`policy`], [`refusal`]), one undo step per run (rolled back when the run fails),
//!   read-only access checked after every call, and the plug-ins' settings (kept in the settings
//!   folder).
//! - **effect** plug-ins (EffectCraft's own kind): the WebAssembly effect modules of
//!   `docs/plugins.md` (`ec_render`, ABI 1 and 2) packed in a bundle. They load through the host's
//!   [`PluginLoader`](crate::PluginLoader), exactly as a `.wasm` in the Plug-ins folder or a Plug-in
//!   Library package does; the bundle only adds the shared manifest, the SHA-256 checks and the
//!   GitHub installation.
//!
//! The Plug-in Library ([`crate::library`]) keeps its own catalog format and folder
//! (`Plug-ins/Library`), unchanged; loose `.wasm` files in `Plug-ins` still load at start-up. The
//! shared store's folders (`Plug-ins/<32 hex digits>/`) sit beside them and neither reads the
//! other's. The `plugin.*` commands are in `cmd::plugins`; see `docs/plugins.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

pub use craft_plugin;
use craft_plugin::bundle::{self, Bundle};
use craft_plugin::fetch::{self, Fetch};
use craft_plugin::{API_VERSION, AppInfo, Instance, KIND_COMMAND, KindApi, Limits, Manifest, Policy, RunOutput, UiChange, catalog, github};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::cmd::bad;
use crate::effects::plugin::{MIN_WASM_API_VERSION, WASM_API_VERSION};
use crate::{EngineError, Event, Result, Session};

/// EffectCraft's own kind: a WebAssembly effect module (`docs/plugins.md`) in a bundle.
pub const KIND_EFFECT: &str = "effect";

/// EffectCraft as manifests name it: command plug-ins of every shared command ABI version, and
/// effect plug-ins of the WebAssembly effect ABIs this build loads; only manifests that name
/// `effectcraft` in `apps`.
pub const APP: AppInfo<'static> = AppInfo {
    id: "effectcraft",
    version: env!("CARGO_PKG_VERSION"),
    api: 1..=API_VERSION,
    kinds: &[KIND_COMMAND, KIND_EFFECT],
    accept_legacy: false,
    kind_api: &[KindApi { kind: KIND_EFFECT, api: MIN_WASM_API_VERSION..=WASM_API_VERSION }],
};

/// What one plug-in may keep in its settings (keys and JSON values, in bytes), and its longest key.
pub const MAX_SETTINGS_BYTES: usize = 64 * 1024;
pub const MAX_SETTING_KEY: usize = 128;
/// Catalogs a person may add.
pub const MAX_CATALOGS: usize = 32;
/// The settings-store entry that keeps plug-ins' settings and the catalogs added.
pub const PLUGIN_SETTINGS_FILE: &str = "plugin_settings.json";
/// Messages a run may show (`notify`), and their length in characters.
const MAX_NOTES: usize = 20;
const MAX_NOTE_CHARS: usize = 1000;

/// EffectCraft's commands no plug-in may call, on top of [`craft_plugin::ALWAYS_DENIED`] (plug-ins,
/// Undo and Redo, `file.*`, `app.*`, `pref.*`, `window.*`). A plug-in gets no path, no file, no
/// network, no other project, no settings, no undo history and no other code, and a run is already
/// one undo step, so nothing that nests commands either.
const DENIED: &[&str] = &[
    // Settings (EffectCraft's are `prefs.*`), shortcuts, playback and render settings.
    "prefs.*",
    "shortcuts.*",
    "playback.*",
    "render.*",
    "renderQueue.*",
    "storage.*",
    // The undo history and the command journal.
    "edit.history",
    "edit.history.*",
    "edit.purge",
    "edit.purgeUndo",
    "journal.*",
    // The clipboard belongs to the session (and the system clipboard to the person).
    "edit.copy",
    "edit.copy*",
    "edit.cut",
    "edit.paste*",
    "keys.copy",
    "keys.paste",
    "effect.copy",
    "effect.paste",
    "comp.copyFrame",
    // Other code: scripts, ScriptUI, batches, other plug-ins, effect helpers, trained models.
    "script.*",
    "scriptui.*",
    "engine.*",
    "library.*",
    "effect.plugins.*",
    "effect.helpers.*",
    "roto.model.*",
    "face.model.*",
    "jobs.*",
    // Files and folders on disk, other applications, the web.
    "mediaBrowser.*",
    "templates.*",
    "help.*",
    "learn.*",
    "edit.editOriginal",
    "layer.openSource",
    "footage.relink",
    "comp.saveFrameAs",
    "comp.saveFrameAsPsd",
    "comp.saveFrameAsExr",
    "comp.savePreset",
    "comp.deletePreset",
    "anim.savePreset",
    "anim.applyRecentPreset",
    "anim.clearRecentPresets",
    "anim.reveal",
    "essential.exportTemplate",
    "essential.importTemplate",
    "essential.templateInfo",
    "view.exportGuides",
    "view.importGuides",
    "camera.exportBlender",
    "keys.rpfCameraImport",
];

/// Commands whose `path` names a file (elsewhere `path` is a property path, `transform/position`):
/// they may run with the content itself (`preset`, `item`), not with a file.
const PATH_IS_A_FILE: &[&str] = &["anim.applyPreset", "layer.newModel"];

/// Parameters that name files or folders on disk in any command.
const FILE_KEYS: &[&str] = &["paths", "file", "files", "dir", "directory", "projectPath", "footageDir", "output", "outputs"];

/// The rules every plug-in run follows: [`craft_plugin::ALWAYS_DENIED`] and [`DENIED`].
pub fn policy() -> &'static Policy {
    static POLICY: OnceLock<Policy> = OnceLock::new();
    POLICY.get_or_init(|| Policy::new(DENIED, &[]))
}

/// Why plug-in `m` may not call `command` with `params`, before it runs: a refused command, one
/// outside its manifest's `permissions.commands`, or parameters that name a file.
pub fn refusal(m: &Manifest, command: &str, params: &Value) -> Option<String> {
    if let Err(why) = craft_plugin::check_call(m, policy(), command) {
        return Some(why);
    }
    let path_file = PATH_IS_A_FILE.contains(&command) && params.get("path").is_some_and(|p| !p.is_null());
    if path_file || names_a_file(params, 0) {
        return Some(format!("plug-ins can't name files (`{command}` was given a file or folder): pass the content itself"));
    }
    None
}

/// Whether `params` hold one of [`FILE_KEYS`] at any depth.
fn names_a_file(params: &Value, depth: usize) -> bool {
    // serde_json refuses nesting past 128, so nothing deeper reaches here.
    depth < 128
        && match params {
            Value::Object(m) => m.iter().any(|(k, v)| FILE_KEYS.contains(&k.as_str()) || names_a_file(v, depth + 1)),
            Value::Array(a) => a.iter().any(|v| names_a_file(v, depth + 1)),
            _ => false,
        }
}

/// An installed plug-in.
#[derive(Clone, Debug)]
pub struct Plugin {
    pub manifest: Manifest,
    /// The SHA-256 of its bundle (lower-case hex).
    pub sha256: String,
    /// Where it came from: `{type: file, path}`, `{type: catalog, catalog, package}` or
    /// `{type: github, repo, tag, asset}` (craft-plugin's `store::Source`).
    pub source: Value,
    /// Kept in the user's `Plug-ins` folder; otherwise for this session only.
    pub stored: bool,
    wasm: Arc<Vec<u8>>,
}

impl Plugin {
    /// What `plugin.list` says about it.
    pub fn describe(&self) -> Value {
        let mut v = serde_json::to_value(&self.manifest).unwrap_or_else(|_| json!({"id": self.manifest.id}));
        v["kind"] = json!(self.manifest.kind);
        v["sha256"] = json!(self.sha256);
        v["source"] = self.source.clone();
        v["stored"] = json!(self.stored);
        v
    }

    /// Whether it is a command plug-in (one `plugin.run` runs).
    pub fn is_command(&self) -> bool {
        self.manifest.kind == KIND_COMMAND
    }
}

/// A bundle checked and shown for review (`plugin.install {review: true}`), kept so the install
/// that follows doesn't download it again.
struct Pending {
    sha256: String,
    /// The `plugin.install` parameters that named it.
    request: Value,
    candidate: Candidate,
}

/// What [`PLUGIN_SETTINGS_FILE`] keeps.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Stored {
    /// Each plug-in's settings, by plug-in id.
    #[serde(default)]
    settings: BTreeMap<String, Map<String, Value>>,
    /// Catalogs (`index.json` URLs) the person added.
    #[serde(default)]
    catalogs: Vec<String>,
}

/// The session's plug-ins.
#[derive(Default)]
pub struct Runtime {
    /// The `Plug-ins` folder the list was read from (`Some(None)`: read, and there is none).
    opened: Option<Option<PathBuf>>,
    #[cfg(not(target_arch = "wasm32"))]
    store: Option<craft_plugin::store::Store>,
    plugins: Vec<Plugin>,
    /// Plug-in folders that couldn't be read, or plug-ins this version can't run.
    problems: Vec<String>,
    stored: Stored,
    /// HTTPS for catalogs and GitHub releases; `None`: the system's `curl` when the session has a
    /// `Plug-ins` folder, else no network (tests set their own).
    pub fetch: Option<Box<dyn Fetch + Send + Sync>>,
    pending: Option<Pending>,
    /// The last run (plug-in id, command id, params).
    pub last: Option<(String, String, Value)>,
    /// Effect plug-ins whose effects this session registered: id → version.
    loaded: BTreeMap<String, String>,
    /// Effect plug-ins updated or removed after they loaded: the change applies at the next start.
    restart: BTreeSet<String>,
}

impl Runtime {
    /// The installed plug-ins (as last read: commands call [`ensure`] first).
    pub fn list(&self) -> &[Plugin] {
        &self.plugins
    }

    /// Why plug-in folders couldn't be used.
    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    /// Plug-in `id` (ignoring case, as the store does).
    pub fn get(&self, id: &str) -> Option<&Plugin> {
        self.plugins.iter().find(|p| p.manifest.id.eq_ignore_ascii_case(id))
    }

    /// Whether plug-ins are kept in the user's folder (else for this session only).
    pub fn has_store(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        return self.store.is_some();
        #[cfg(target_arch = "wasm32")]
        false
    }

    /// The catalogs added (`plugin.catalog.add`).
    pub fn catalogs(&self) -> &[String] {
        &self.stored.catalogs
    }

    /// Effect plug-ins whose update or removal applies at the next start.
    pub fn restart_needed(&self) -> &BTreeSet<String> {
        &self.restart
    }
}

/// Read the installed plug-ins when the session's `Plug-ins` folder changed (or was never read).
pub fn ensure(s: &mut Session) {
    let root = s.plugins_dir();
    if s.plugins.opened.as_ref() == Some(&root) {
        return;
    }
    s.plugins.opened = Some(root.clone());
    #[cfg(not(target_arch = "wasm32"))]
    {
        s.plugins.store = root.map(craft_plugin::store::Store::new);
    }
    reload(s);
}

/// Read the installed plug-ins and load the effects of effect plug-ins (the app runs it at
/// start-up). Returns what `plugin.list` reports.
pub fn load_installed(s: &mut Session) -> Value {
    ensure(s);
    json!({"plugins": s.plugins.plugins.len(), "problems": s.plugins.problems})
}

/// Read the store again (the session's own plug-ins stay).
fn reload(s: &mut Session) {
    #[cfg(target_arch = "wasm32")]
    let _ = s;
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(store) = &s.plugins.store else { return };
        let listing = store.list_for(&APP);
        s.plugins.plugins.retain(|p| !p.stored);
        s.plugins.problems = listing.problems.into_iter().map(|p| format!("{}: {}", p.dir.display(), p.error)).collect();
        s.plugins.problems.extend(listing.not_runnable.into_iter().map(|(i, why)| format!("{}: {why}", i.manifest.name)));
        for (i, _) in listing.runnable {
            let source = serde_json::to_value(&i.record.source).unwrap_or(Value::Null);
            let origin = i.dir.join(format!("{}.{}", i.record.sha256, bundle::BUNDLE_EXTENSION)).to_string_lossy().to_string();
            let plugin = Plugin { manifest: i.manifest, sha256: i.record.sha256, source, stored: true, wasm: Arc::new(i.bundle.wasm) };
            if plugin.manifest.kind == KIND_EFFECT
                && let Err(why) = load_effect(s, &plugin, &origin)
            {
                s.plugins.problems.push(format!("{}: {why}", plugin.manifest.name));
                continue;
            }
            s.plugins.plugins.retain(|p| !p.manifest.id.eq_ignore_ascii_case(&plugin.manifest.id));
            s.plugins.plugins.push(plugin);
        }
    }
}

/// Register an effect plug-in's effect through the host's loader, unless this session already
/// did (effects can't be unregistered: a new version then waits for the next start).
fn load_effect(s: &mut Session, p: &Plugin, origin: &str) -> std::result::Result<(), String> {
    let id = &p.manifest.id;
    if let Some(v) = s.plugins.loaded.get(id) {
        if *v != p.manifest.version {
            s.plugins.restart.insert(id.clone());
        }
        return Ok(());
    }
    if crate::effects::find(id).is_some() {
        // Registered from elsewhere (a loose `.wasm`, the Plug-in Library): the first one stays.
        s.plugins.restart.insert(id.clone());
        return Ok(());
    }
    let loader = s.plugin_loader.ok_or("WebAssembly effect plug-ins are not available in this build")?;
    loader(&p.wasm, origin)?;
    s.plugins.loaded.insert(id.clone(), p.manifest.version.clone());
    Ok(())
}

/// A fetcher for sessions without network access: `file://` catalogs still read.
struct NoNetwork;

impl Fetch for NoNetwork {
    fn get(&mut self, url: &str, _max: usize) -> std::result::Result<Option<Vec<u8>>, String> {
        let shown: String = url.chars().take(120).collect();
        Err(format!("there is no network access here to read {shown}: install the plug-in from a file, or in the EffectCraft app"))
    }
}

/// HTTPS through the system's `curl`, as the Plug-in Library downloads ([`crate::library`]): only
/// `https://` on GitHub's hosts and GitHub's API, also after redirects.
struct Curl {
    /// Where temporary downloads go (the `Plug-ins` folder).
    tmp: PathBuf,
}

impl Fetch for Curl {
    fn get(&mut self, url: &str, max: usize) -> std::result::Result<Option<Vec<u8>>, String> {
        if !catalog::is_api_url(url) {
            catalog::check_url(url, None)?;
        }
        let got = crate::library::curl(url, max as u64, &self.tmp)?;
        if got.status == 404 {
            return Ok(None);
        }
        if got.too_big {
            return Err(format!("{} is larger than {} KB", url.chars().take(120).collect::<String>(), max / 1024));
        }
        if !got.ok {
            return Err(format!("download failed: {}", got.stderr));
        }
        // where it ended stays on GitHub's API for an API request, on its download hosts otherwise
        catalog::check_redirect_for(url, &got.end)?;
        Ok(got.body)
    }
}

/// The HTTPS client a background download of this session uses: the system's `curl` when the
/// session has a `Plug-ins` folder (downloads pass through it), else none.
pub fn network(s: &Session) -> Box<dyn Fetch + Send + Sync> {
    match s.plugins_dir() {
        Some(tmp) if !cfg!(target_arch = "wasm32") => Box::new(Curl { tmp }),
        _ => Box::new(NoNetwork),
    }
}

/// Run `f` with the session's HTTPS client ([`Runtime::fetch`], else [`network`]).
fn with_fetch<T>(s: &mut Session, f: impl FnOnce(&mut dyn Fetch) -> T) -> T {
    if let Some(http) = s.plugins.fetch.as_deref_mut() {
        return f(http);
    }
    let mut http = network(s);
    f(http.as_mut())
}

// ---------- settings ----------

/// Read plug-ins' settings and catalogs from the settings store (a damaged entry gives none).
pub(crate) fn load_settings(s: &mut Session) {
    let Some(cfg) = s.config.clone() else { return };
    s.plugins.stored = cfg
        .read(PLUGIN_SETTINGS_FILE)
        .and_then(|t| serde_json::from_str(&t).map_err(|e| log::warn!("{PLUGIN_SETTINGS_FILE} is not valid ({e}); plug-ins start without their settings")).ok())
        .unwrap_or_default();
}

/// Write plug-ins' settings and catalogs to the settings store (nothing without one).
fn save_settings(s: &Session) -> std::result::Result<(), String> {
    let Some(cfg) = s.config.as_ref() else { return Ok(()) };
    let text = serde_json::to_string_pretty(&s.plugins.stored).map_err(|e| e.to_string())?;
    cfg.write(PLUGIN_SETTINGS_FILE, &text).map_err(|e| format!("cannot save plug-in settings: {e}"))
}

/// Store (`Some`) or forget (`None`) plug-in `id`'s setting `key` in `all`, within
/// [`MAX_SETTINGS_BYTES`] and [`MAX_SETTING_KEY`].
fn set_setting(all: &mut BTreeMap<String, Map<String, Value>>, id: &str, key: &str, value: Option<&str>) -> std::result::Result<(), String> {
    let Some(text) = value else {
        if let Some(mine) = all.get_mut(id) {
            mine.remove(key);
            if mine.is_empty() {
                all.remove(id);
            }
        }
        return Ok(());
    };
    if key.is_empty() || key.chars().count() > MAX_SETTING_KEY || key.chars().any(char::is_control) {
        return Err(format!("a setting's key must be 1–{MAX_SETTING_KEY} characters without control characters"));
    }
    let too_big = || format!("a plug-in's settings are limited to {} KB", MAX_SETTINGS_BYTES / 1024);
    if key.len() + text.len() > MAX_SETTINGS_BYTES {
        return Err(too_big());
    }
    let v: Value = serde_json::from_str(text).map_err(|e| format!("a setting must be JSON: {e}"))?;
    let others: usize = all.get(id).map_or(0, |mine| mine.iter().filter(|(k, _)| k.as_str() != key).map(|(k, v)| k.len() + v.to_string().len()).sum());
    if others + key.len() + v.to_string().len() > MAX_SETTINGS_BYTES {
        return Err(too_big());
    }
    all.entry(id.to_string()).or_default().insert(key.to_string(), v);
    Ok(())
}

// ---------- running ----------

/// The sandbox's view of the session: every call is checked by [`refusal`], and one that asks the
/// frontend for something (a dialog, a panel, a URL, the clipboard) or, from a read-only plug-in,
/// changes the project is put back and refused.
struct EngineHost<'s> {
    s: &'s mut Session,
    manifest: &'s Manifest,
    notes: Vec<String>,
    settings_changed: bool,
}

/// Events a plug-in's call may not leave behind: the frontend would open a dialog or a panel,
/// a web page, or put something on the system clipboard.
fn asks_the_frontend(e: &Event) -> bool {
    matches!(e, Event::Frontend { .. } | Event::OpenUrl(_) | Event::CopyImage { .. })
}

impl craft_plugin::Host for EngineHost<'_> {
    fn call(&mut self, command: &str, params: &str) -> std::result::Result<String, String> {
        let params: Value = serde_json::from_str(params).map_err(|e| format!("the params for `{command}` aren't JSON: {e}"))?;
        if let Some(why) = refusal(self.manifest, command, &params) {
            return Err(why);
        }
        let s = &mut *self.s;
        let (project, history, state) = (s.project.clone(), s.history.clone(), s.state.clone());
        let (events, journal) = (s.events.len(), s.journal.len());
        let r = s.execute_checked(command, params);
        let asks = s.events.get(events..).is_some_and(|new| new.iter().any(asks_the_frontend));
        let changed = !Arc::ptr_eq(&project, &s.project);
        let refused = if asks {
            Some(format!("`{command}` opens a dialog, a panel or another application, which plug-ins can't"))
        } else if changed {
            craft_plugin::check_read_only(self.manifest, command, true).err()
        } else {
            None
        };
        if let Some(why) = refused {
            // Put the call back: the project, its history and the selection as they were, and
            // nothing of it journaled or shown.
            let edited = crate::commands::text_edit::active_editor_snapshot(s);
            s.project = project;
            s.history = history;
            s.state = state;
            s.events.truncate(events);
            s.journal.truncate(journal);
            s.sanitize_after_replacement(edited.as_ref());
            if changed {
                s.bump();
            }
            return Err(why);
        }
        r.map(|v| v.to_string()).map_err(|e| e.to_string())
    }

    fn selection(&mut self) -> String {
        let ids: Vec<u64> = self.s.state.selected_layers.iter().map(|l| l.0).collect();
        json!(ids).to_string()
    }

    fn notify(&mut self, message: &str) {
        if self.notes.len() < MAX_NOTES {
            self.notes.push(message.chars().take(MAX_NOTE_CHARS).collect());
        }
    }

    fn log(&mut self, line: &str) {
        log::info!("plug-in {}: {line}", self.manifest.id);
    }

    fn setting(&mut self, key: &str) -> Option<String> {
        Some(self.s.plugins.stored.settings.get(&self.manifest.id)?.get(key)?.to_string())
    }

    fn set_setting(&mut self, key: &str, value: Option<&str>) -> std::result::Result<(), String> {
        set_setting(&mut self.s.plugins.stored.settings, &self.manifest.id, key, value)?;
        self.settings_changed = true;
        Ok(())
    }
}

/// The undo step a plug-in run makes: `Name`, or `Name: Command` for a plug-in with several.
fn step_label(m: &Manifest, c: &craft_plugin::PluginCommand) -> String {
    if m.commands.len() > 1 { format!("{}: {}", m.name, c.label.trim_end_matches(['…', '.'])) } else { m.name.clone() }
}

/// Run plug-in `id`'s `command` (default: its first) with `params` as one undo step named after
/// the plug-in. A run that fails (a trap, an exhausted budget, a refusal it passes on, a non-zero
/// status) leaves the project, selection and undo history as they were.
pub fn run(s: &mut Session, id: &str, command: Option<&str>, params: &Value, fuel: Option<u64>) -> Result<Value> {
    let cmd = "plugin.run";
    ensure(s);
    let p = s.plugins.get(id).cloned().ok_or_else(|| bad(cmd, format!("no plug-in `{id}` is installed (see plugin.list)")))?;
    let m = &p.manifest;
    if !p.is_command() {
        return Err(bad(cmd, format!("{} is an effect plug-in: apply its effect from Effects & Presets (effect.apply)", m.name)));
    }
    let api = m.target(&APP).map_err(|e| bad(cmd, format!("{} can't run here: {e}", m.name)))?.api();
    let c = m.command(command).ok_or_else(|| bad(cmd, format!("{} has no command `{}`", m.name, command.unwrap_or_default())))?;
    let params = c.resolve_params(params).map_err(|e| bad(cmd, e))?;
    let mut limits = Limits::default();
    if let Some(f) = fuel {
        limits.fuel = f.clamp(1000, limits.fuel);
    }
    let mut instance = Instance::new(&p.wasm, &limits).map_err(|e| failed(&m.name, e))?;
    let selection: Vec<u64> = s.state.selected_layers.iter().map(|l| l.0).collect();
    let input = json!({"api": api, "command": c.id, "params": params, "selection": selection}).to_string();
    s.plugins.last = Some((m.id.clone(), c.id.clone(), params));
    let label = step_label(m, c);

    // The run's commands record undo steps of their own; they fold into one below, as
    // `engine.batch`'s do. A failed run puts everything back, as an atomic batch does.
    let (project, history, state) = (s.project.clone(), s.history.clone(), s.state.clone());
    let (events, journal) = (s.events.len(), s.journal.len());
    let mut host = EngineHost { s: &mut *s, manifest: m, notes: Vec::new(), settings_changed: false };
    let ran: std::result::Result<RunOutput, craft_plugin::PluginError> = instance.call(&input, &mut host);
    let notes = std::mem::take(&mut host.notes);
    let settings_changed = host.settings_changed;
    if settings_changed && let Err(e) = save_settings(s) {
        log::warn!("plug-in {}: {e}", m.id);
    }
    let out = match ran {
        Ok(out) => out,
        Err(e) => {
            let edited = crate::commands::text_edit::active_editor_snapshot(s);
            let changed = !Arc::ptr_eq(&project, &s.project);
            s.project = project;
            s.history = history;
            s.state = state;
            s.events.truncate(events);
            s.journal.truncate(journal);
            s.sanitize_after_replacement(edited.as_ref());
            if changed {
                s.bump();
            }
            return Err(failed(&m.name, e));
        }
    };
    crate::cmd::fold_undo_steps(s, &project, &label);
    for n in &notes {
        s.toast(format!("{}: {n}", m.name));
    }
    let output = serde_json::from_str::<Value>(&out.output).unwrap_or(Value::String(out.output));
    // Panels (`ui_show`) aren't shown in EffectCraft yet: say so rather than drop it silently.
    let panel = matches!(out.ui, Some(UiChange::Show(_))).then_some("EffectCraft doesn't show plug-in panels yet");
    Ok(json!({"plugin": m.id, "command": c.id, "output": output, "notes": notes, "logs": out.logs, "calls": out.calls, "fuel": out.fuel, "panel": panel}))
}

/// A plug-in error as the engine reports it.
fn failed(name: &str, e: impl std::fmt::Display) -> EngineError {
    EngineError::Other(format!("{name}: {e}"))
}

// ---------- installing ----------

/// A bundle obtained for installing: its bytes, where it came from and, for a repository,
/// whether a catalog in use lists it.
#[derive(Clone)]
pub struct Candidate {
    pub bytes: Vec<u8>,
    pub source: Value,
    /// `Some(true)` from a catalog; for a repository, whether a catalog in use lists it.
    pub in_catalog: Option<bool>,
}

impl std::fmt::Debug for Candidate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Candidate").field("bytes", &self.bytes.len()).field("source", &self.source).field("in_catalog", &self.in_catalog).finish()
    }
}

/// Whether `p` names a source that is downloaded (`repo`, or `catalog` + `id`).
pub fn is_download(p: &Value) -> bool {
    let has = |k: &str| p.get(k).and_then(Value::as_str).is_some_and(|t| !t.trim().is_empty());
    has("repo") || has("catalog")
}

/// Download the bundle `p` names (`repo`: `owner/repo[@tag]`, or `catalog` + `id`, and optionally
/// `version`) over `f`. `catalogs` are the catalogs in use, to tell whether a repository is in
/// one. Needs no session, so the app can run it on a worker thread.
pub fn download(p: &Value, catalogs: &[String], f: &mut dyn Fetch) -> std::result::Result<Candidate, String> {
    let text = |k: &str| p.get(k).and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty());
    if let Some(spec) = text("repo") {
        let repo = github::parse_repo_spec(spec)?;
        let plan = fetch::resolve_repo(f, &repo)?;
        let bytes = fetch::download_repo(f, &plan)?;
        let source = json!({"type": "github", "repo": plan.repo.slug(), "tag": plan.repo.tag.clone().unwrap_or_default(), "asset": plan.asset});
        let in_catalog = catalogs.iter().any(|url| {
            fetch::fetch_catalog(f, url)
                .is_ok_and(|(cat, _)| cat.packages.iter().any(|pkg| pkg.repo_ref().is_some_and(|r| r.same_repo(&repo.owner, &repo.name))))
        });
        return Ok(Candidate { bytes, source, in_catalog: Some(in_catalog) });
    }
    if let Some(url) = text("catalog") {
        let id = text("id").ok_or("`id` required with `catalog` (a package of the catalog)")?;
        let (cat, _) = fetch::fetch_catalog(f, url)?;
        let package = cat.package(id).ok_or_else(|| format!("{url} has no package `{id}`"))?;
        let release = match text("version") {
            Some(v) => package.release(v).ok_or_else(|| format!("{id} has no version {v} in {url}"))?,
            None => package.latest_for(&APP).ok_or_else(|| format!("{id} has no release for EffectCraft {}", APP.version))?,
        };
        release.target(&package.kind, &APP)?;
        let bytes = fetch::download_release(f, url, package, release)?;
        let manifest = Bundle::decode(&bytes, bundle::MAX_BUNDLE_BYTES).and_then(|b| Manifest::parse(&b.manifest))?;
        package.check_manifest(release, &manifest)?;
        return Ok(Candidate { bytes, source: json!({"type": "catalog", "catalog": url, "package": package.id}), in_catalog: Some(true) });
    }
    Err("pass `repo` (owner/repo or owner/repo@tag) or `catalog` with `id`".into())
}

/// Read the bundle `p` names: `path`, `data` (base64), `repo` (`owner/repo[@tag]`) or `catalog` +
/// `id` (and optionally `version`).
pub(crate) fn obtain(s: &mut Session, cmd: &str, p: &Value) -> Result<Candidate> {
    let text = |k: &str| p.get(k).and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty());
    let max = bundle::MAX_BUNDLE_BYTES;
    if let Some(path) = text("path") {
        let bytes = s.services.read_file_bounded(path, max as u64).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
        if bytes.len() > max {
            return Err(bad(cmd, format!("{path} is larger than {} MB", max >> 20)));
        }
        return Ok(Candidate { bytes, source: json!({"type": "file", "path": path}), in_catalog: None });
    }
    if let Some(data) = text("data") {
        if data.len() > max / 3 * 4 + 4 {
            return Err(bad(cmd, format!("the bundle is larger than {} MB", max >> 20)));
        }
        let bytes = effectcraft_track::roto::rle::base64_decode(data).ok_or_else(|| bad(cmd, "`data` isn't base64"))?;
        let name = text("name").unwrap_or("data");
        return Ok(Candidate { bytes, source: json!({"type": "file", "path": name}), in_catalog: None });
    }
    if is_download(p) {
        let catalogs = s.plugins.stored.catalogs.clone();
        return with_fetch(s, |f| download(p, &catalogs, f)).map_err(|e| bad(cmd, e));
    }
    Err(bad(cmd, "pass `path`, `data` (base64), `repo` (owner/repo or owner/repo@tag) or `catalog` with `id`"))
}

/// Read and check a bundle as EffectCraft runs it: a command plug-in's module against the
/// shared sandbox, an effect plug-in's through the host's previewer (it must load, and register
/// the effect the manifest names). The manifest, or why it can't be used.
fn open_bundle(s: &Session, bytes: &[u8]) -> std::result::Result<(Bundle, Manifest), String> {
    let b = Bundle::decode(bytes, bundle::MAX_BUNDLE_BYTES)?;
    let kind = Manifest::parse(&b.manifest)?.kind;
    let manifest = if kind == KIND_EFFECT { b.check_with(&effect_check(s))? } else { b.check()? };
    // Not installed yet: the confirmation needs to know before anything is written.
    manifest.target(&APP).map_err(|e| format!("{} can't be installed: {e}", manifest.name))?;
    Ok((b, manifest))
}

/// The module check of effect plug-ins: the WebAssembly effect module loads (rendered once on a
/// one-pixel frame, never registered) and its own manifest has the bundle's id.
fn effect_check(s: &Session) -> impl Fn(&Manifest, &[u8]) -> std::result::Result<(), String> + use<> {
    let previewer = s.plugin_previewer;
    move |m: &Manifest, wasm: &[u8]| {
        let preview = previewer.ok_or("WebAssembly effect plug-ins are not available in this build")?;
        let (effect, _) = preview(wasm, 1, 1, &[[0.0; 4]], &[])?;
        let effect_id = effect.get("id").and_then(Value::as_str).unwrap_or_default();
        if effect_id != m.id {
            return Err(format!("its module registers the effect `{effect_id}`, not `{}` as its manifest says", m.id));
        }
        Ok(())
    }
}

/// What an install confirmation shows about `c`: the manifest's permissions, kind and commands,
/// the source, the SHA-256, whether a catalog lists it and what it replaces. An error when the
/// bundle isn't a plug-in EffectCraft can run.
pub(crate) fn review(s: &mut Session, cmd: &str, c: &Candidate) -> Result<(Manifest, Value)> {
    ensure(s);
    let (_, manifest) = open_bundle(s, &c.bytes).map_err(|e| bad(cmd, e))?;
    let old = s.plugins.get(&manifest.id);
    let mut v = serde_json::to_value(&manifest).unwrap_or_else(|_| json!({"id": manifest.id}));
    v["kind"] = json!(manifest.kind);
    v["sha256"] = json!(bundle::sha256_hex(&c.bytes));
    v["source"] = c.source.clone();
    v["inCatalog"] = json!(c.in_catalog);
    v["replaces"] = json!(old.map(|o| &o.manifest.version));
    v["permissionsChanged"] = json!(old.is_some_and(|o| craft_plugin::permissions::changed(&o.manifest, &manifest)));
    v["stored"] = json!(s.plugins.has_store());
    Ok((manifest, v))
}

/// Install `c` (already reviewed): into the user's folder when the session has one, else for the
/// session. Replaces an installed plug-in with the same id; its settings stay. An effect plug-in's
/// effect is registered at once, or at the next start when an earlier version is loaded.
pub(crate) fn install(s: &mut Session, cmd: &str, c: Candidate) -> Result<Plugin> {
    let (manifest, _) = review(s, cmd, &c)?;
    let sha256 = bundle::sha256_hex(&c.bytes);
    #[cfg(not(target_arch = "wasm32"))]
    let installed = match &s.plugins.store {
        Some(store) => {
            let source: craft_plugin::Source = serde_json::from_value(c.source.clone()).map_err(|e| bad(cmd, e.to_string()))?;
            let check = effect_check(s);
            let check_module = move |m: &Manifest, wasm: &[u8]| if m.kind == KIND_EFFECT { check(m, wasm) } else { bundle::check_command_module(m, wasm) };
            let (i, _) = store.install_with_for(&APP, &c.bytes, source, &check_module).map_err(|e| bad(cmd, e))?;
            let origin = i.dir.join(format!("{}.{}", i.record.sha256, bundle::BUNDLE_EXTENSION)).to_string_lossy().to_string();
            Some((Plugin { manifest: i.manifest, sha256: i.record.sha256, source: c.source.clone(), stored: true, wasm: Arc::new(i.bundle.wasm) }, origin))
        }
        None => None,
    };
    #[cfg(target_arch = "wasm32")]
    let installed: Option<(Plugin, String)> = None;
    let (plugin, origin) = match installed {
        Some(found) => found,
        None => {
            let (b, _) = open_bundle(s, &c.bytes).map_err(|e| bad(cmd, e))?;
            let origin = c.source.get("path").and_then(Value::as_str).unwrap_or("plug-in bundle").to_string();
            (Plugin { manifest, sha256, source: c.source, stored: false, wasm: Arc::new(b.wasm) }, origin)
        }
    };
    if plugin.manifest.kind == KIND_EFFECT {
        load_effect(s, &plugin, &origin).map_err(|e| bad(cmd, format!("{} didn't load: {e}", plugin.manifest.name)))?;
    }
    Ok(keep(s, plugin))
}

/// Put `plugin` in the session's list, in place of one with the same id.
fn keep(s: &mut Session, plugin: Plugin) -> Plugin {
    let list = &mut s.plugins.plugins;
    match list.iter_mut().find(|p| p.manifest.id.eq_ignore_ascii_case(&plugin.manifest.id)) {
        Some(slot) => *slot = plugin.clone(),
        None => list.push(plugin.clone()),
    }
    plugin
}

/// Keep `c`, named by the install parameters `request`, for the install that follows its review
/// (one at a time).
pub fn hold(s: &mut Session, sha256: &str, request: Value, candidate: Candidate) {
    s.plugins.pending = Some(Pending { sha256: sha256.into(), request, candidate });
}

/// The bundle reviewed last, when it is `sha256` and `request` named it.
pub(crate) fn take_held(s: &mut Session, sha256: &str, request: &Value) -> Option<Candidate> {
    let held = s.plugins.pending.take_if(|h| h.sha256.eq_ignore_ascii_case(sha256) && h.request == *request)?;
    Some(held.candidate)
}

/// Remove plug-in `id` and its settings; whether it was installed. An effect plug-in's effect
/// stays registered until the next start.
pub(crate) fn remove(s: &mut Session, cmd: &str, id: &str) -> Result<bool> {
    ensure(s);
    let Some(p) = s.plugins.get(id).cloned() else { return Ok(false) };
    #[cfg(not(target_arch = "wasm32"))]
    if p.stored
        && let Some(store) = &s.plugins.store
    {
        store.remove(&p.manifest.id).map_err(|e| bad(cmd, e))?;
    }
    #[cfg(target_arch = "wasm32")]
    let _ = cmd;
    s.plugins.plugins.retain(|x| !x.manifest.id.eq_ignore_ascii_case(id));
    if s.plugins.loaded.contains_key(&p.manifest.id) {
        s.plugins.restart.insert(p.manifest.id.clone());
    }
    if s.plugins.stored.settings.remove(&p.manifest.id).is_some()
        && let Err(e) = save_settings(s)
    {
        log::warn!("plug-in {}: {e}", p.manifest.id);
    }
    if s.plugins.last.as_ref().is_some_and(|(last, ..)| last.eq_ignore_ascii_case(id)) {
        s.plugins.last = None;
    }
    Ok(true)
}

/// A catalog's packages EffectCraft can run, with their newest release for it, as
/// `plugin.catalog.list` reports them.
pub fn catalog_rows(s: &Session, url: &str, cat: &catalog::Catalog) -> Vec<Value> {
    cat.packages
        .iter()
        .filter_map(|pkg| {
            let release = pkg.latest_for(&APP)?;
            let installed = s.plugins.get(&pkg.id).map(|p| p.manifest.version.clone());
            Some(json!({
                "catalog": url, "id": pkg.id, "name": pkg.name, "version": release.version, "author": pkg.author,
                "description": pkg.description, "license": pkg.license, "repo": pkg.repo, "kind": pkg.kind,
                "sha256": release.sha256, "installed": installed,
            }))
        })
        .collect()
}

/// The catalog `url`'s packages EffectCraft can run, and what was left out of it.
pub(crate) fn catalog_packages(s: &mut Session, url: &str) -> std::result::Result<(Vec<Value>, Vec<String>), String> {
    let (cat, warnings) = with_fetch(s, |f| fetch::fetch_catalog(f, url))?;
    Ok((catalog_rows(s, url, &cat), warnings))
}

/// Whether `url` is a catalog address EffectCraft reads (`https://` on GitHub's hosts, or
/// `file://`), or why not.
pub(crate) fn check_catalog_url(url: &str) -> std::result::Result<(), String> {
    catalog::check_url(url, None).map(|_| ())
}

/// Add (or, with `remove`, drop) catalog `url`; the catalogs now.
pub(crate) fn set_catalog(s: &mut Session, url: &str, remove: bool) -> std::result::Result<Vec<String>, String> {
    let list = &mut s.plugins.stored.catalogs;
    if remove {
        list.retain(|u| u != url);
    } else if !list.iter().any(|u| u == url) {
        check_catalog_url(url)?;
        if list.len() >= MAX_CATALOGS {
            return Err(format!("at most {MAX_CATALOGS} catalogs"));
        }
        list.push(url.to_string());
    }
    let now = list.clone();
    save_settings(s)?;
    Ok(now)
}

/// Where plug-in `p` has a newer release: the install parameters that get it and its version (a
/// tag, for a repository). `None` when it is up to date or came from a file.
pub(crate) fn update_for(s: &mut Session, p: &Plugin) -> std::result::Result<Option<(Value, String)>, String> {
    let text = |k: &str| p.source.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    match p.source.get("type").and_then(Value::as_str) {
        Some("catalog") => {
            let (url, id) = (text("catalog"), text("package"));
            let (cat, _) = with_fetch(s, |f| fetch::fetch_catalog(f, &url))?;
            let release = cat.package(&id).and_then(|pkg| pkg.latest_for(&APP));
            Ok(release
                .filter(|r| bundle::newer(&r.version, &p.manifest.version))
                .map(|r| (json!({"catalog": url, "id": id, "version": r.version}), r.version.clone())))
        }
        Some("github") => {
            let mut repo = github::parse_repo_spec(&text("repo"))?;
            repo.tag = None;
            let plan = with_fetch(s, |f| fetch::resolve_repo(f, &repo))?;
            let tag = plan.repo.tag.clone().unwrap_or_default();
            Ok((tag != text("tag")).then(|| (json!({"repo": format!("{}@{tag}", plan.repo.slug())}), tag)))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests;
