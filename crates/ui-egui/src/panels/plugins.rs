//! Effect ▸ Plug-ins ▸ Manage Plug-ins… (`plugin.manage`): the plug-ins installed through the
//! shared plug-in store, installing one from a file, a GitHub repository or a catalog, and the
//! confirmation every install goes through (permissions, kind, source, SHA-256, and whether a
//! catalog you use lists it). Also the form a plug-in command's parameters are asked in.
//!
//! Everything goes through the `plugin.*` commands. Downloads (a repository, a catalog) run on a
//! worker thread, so the window never blocks; the bundle they fetch is held by the engine for the
//! install that follows its confirmation, so nothing is downloaded twice. Plug-in panels
//! (`ui_show`) aren't shown yet.

use std::sync::{Arc, Mutex, PoisonError};

use effectcraft_engine::plugin::craft_plugin::catalog::Catalog;
use effectcraft_engine::plugin::craft_plugin::{Manifest, ParamKind};
use effectcraft_engine::plugin::{self, Candidate};
use egui::vec2;
use serde_json::{Map, Value, json};

use crate::EffectcraftApp;
use crate::i18n::tr;

/// What a worker thread brought back.
#[derive(Clone, Debug)]
enum Job {
    /// A bundle downloaded for review, with the `plugin.install` parameters that named it.
    Bundle(Result<(Value, Candidate), String>),
    /// The catalogs read: (URL, catalog and what was left out of it, or why it couldn't be read).
    Catalogs(Vec<(String, Result<(Catalog, Vec<String>), String>)>),
}

/// The window's state (kept in the dialog state).
#[derive(Clone, Debug, Default)]
pub struct PluginsUi {
    pub open: bool,
    /// `owner/repo` or `owner/repo@tag`, as typed.
    pub repo: String,
    /// A catalog URL to add.
    pub catalog: String,
    /// The packages of the catalogs, as `plugin.catalog.list` reports them.
    pub packages: Vec<Value>,
    pub warnings: Vec<String>,
    /// The install waiting for its confirmation: the `plugin.install` parameters and what the
    /// review said.
    pub review: Option<(Value, Value)>,
    /// The parameters of a plug-in command being asked for: plug-in id, command id, values.
    pub form: Option<(String, String, Map<String, Value>)>,
    /// What the worker is doing, and its result once done.
    busy: Option<(String, Arc<Mutex<Option<Job>>>)>,
    /// The last message (a failure or what was done).
    pub status: String,
}

/// Open the window (`plugin.manage`), with the installed plug-ins read again.
pub(crate) fn open(app: &mut EffectcraftApp) {
    app.dialog_state.plugins.open = true;
    if let Err(e) = app.session.execute("plugin.list", json!({})) {
        app.dialog_state.plugins.status = e.to_string();
    }
}

/// Check the bundle `params` names and show the install confirmation (`plugin.install` from a menu,
/// a picked file, a finished download). Nothing is installed here.
pub(crate) fn review(app: &mut EffectcraftApp, params: Value) -> Result<Value, String> {
    let mut request = params.as_object().cloned().unwrap_or_default();
    request.remove("review");
    let mut ask = request.clone();
    ask.insert("review".into(), json!(true));
    request.remove("sha256");
    let ui = &mut app.dialog_state.plugins;
    match app.session.execute("plugin.install", Value::Object(ask)) {
        Ok(info) => {
            ui.open = true;
            ui.review = Some((Value::Object(request), info.clone()));
            Ok(info)
        }
        Err(e) => {
            ui.open = true;
            ui.status = e.to_string();
            Err(e.to_string())
        }
    }
}

/// `plugin.run` of a command with parameters and none given: ask for them first. Whether the
/// form opened.
pub(crate) fn open_params(app: &mut EffectcraftApp, params: &Value) -> bool {
    if params.get("params").is_some() {
        return false;
    }
    let Some(id) = params.get("id").and_then(Value::as_str) else { return false };
    let Some(p) = app.session.plugins.get(id) else { return false };
    let Some(c) = p.manifest.command(params.get("command").and_then(Value::as_str)) else { return false };
    if !c.params.iter().any(|f| f.is_field()) {
        return false;
    }
    let values: Map<String, Value> = c.params.iter().filter(|f| f.is_field()).map(|f| (f.name.clone(), f.default_value())).collect();
    app.dialog_state.plugins.form = Some((p.manifest.id.clone(), c.id.clone(), values));
    true
}

/// Start a download of the bundle `request` names (`repo`, or `catalog` + `id`) on a worker.
fn download(app: &mut EffectcraftApp, ctx: &egui::Context, what: String, request: Value) {
    let catalogs = app.session.plugins.catalogs().to_vec();
    let mut http = plugin::network(&app.session);
    spawn(app, ctx, what, move || Job::Bundle(plugin::download(&request, &catalogs, http.as_mut()).map(|c| (request, c))));
}

/// Read every catalog added, on a worker.
fn read_catalogs(app: &mut EffectcraftApp, ctx: &egui::Context) {
    let urls = app.session.plugins.catalogs().to_vec();
    let mut http = plugin::network(&app.session);
    spawn(app, ctx, tr("Reading the plug-in catalogs…").to_string(), move || {
        Job::Catalogs(urls.into_iter().map(|u| (u.clone(), plugin::craft_plugin::fetch::fetch_catalog(http.as_mut(), &u))).collect())
    });
}

/// Run `job` on a worker thread (in place where there are none, the web build).
fn spawn(app: &mut EffectcraftApp, ctx: &egui::Context, what: String, job: impl FnOnce() -> Job + Send + 'static) {
    let ui = &mut app.dialog_state.plugins;
    if ui.busy.is_some() {
        return;
    }
    let slot: Arc<Mutex<Option<Job>>> = Arc::default();
    let out = slot.clone();
    let repaint = ctx.clone();
    let work = move || {
        let done = job();
        *out.lock().unwrap_or_else(PoisonError::into_inner) = Some(done);
        repaint.request_repaint();
    };
    if cfg!(target_arch = "wasm32") {
        work();
    } else if let Err(e) = std::thread::Builder::new().name("ec-plugins".into()).spawn(work) {
        ui.status = format!("{}: {e}", tr("Cannot start the download"));
        return;
    }
    ui.status.clear();
    ui.busy = Some((what, slot));
}

/// Take the worker's result when it is done.
fn poll(app: &mut EffectcraftApp) {
    let done = app.dialog_state.plugins.busy.as_ref().and_then(|(_, slot)| slot.lock().unwrap_or_else(PoisonError::into_inner).take());
    let Some(job) = done else { return };
    app.dialog_state.plugins.busy = None;
    match job {
        Job::Bundle(Ok((request, candidate))) => {
            let sha256 = plugin::craft_plugin::sha256_hex(&candidate.bytes);
            plugin::hold(&mut app.session, &sha256, request.clone(), candidate);
            let mut ask = request.as_object().cloned().unwrap_or_default();
            ask.insert("sha256".into(), json!(sha256));
            let _ = review(app, Value::Object(ask));
        }
        Job::Bundle(Err(e)) => app.dialog_state.plugins.status = e,
        Job::Catalogs(found) => {
            let ui_rows: Vec<(Vec<Value>, Vec<String>)> = found
                .iter()
                .map(|(url, r)| match r {
                    Ok((cat, warnings)) => (plugin::catalog_rows(&app.session, url, cat), warnings.iter().map(|w| format!("{url}: {w}")).collect()),
                    Err(e) => (vec![], vec![format!("{url}: {e}")]),
                })
                .collect();
            let ui = &mut app.dialog_state.plugins;
            ui.packages = ui_rows.iter().flat_map(|(rows, _)| rows.iter().cloned()).collect();
            ui.warnings = ui_rows.into_iter().flat_map(|(_, w)| w).collect();
        }
    }
}

/// Run a command and keep its failure (or `done`) as the window's status.
fn run(app: &mut EffectcraftApp, ctx: &egui::Context, id: &str, params: Value, done: &str) -> Option<Value> {
    match crate::menus::run_engine(app, ctx, id, params) {
        Ok(v) => {
            app.dialog_state.plugins.status = done.to_string();
            Some(v)
        }
        Err(e) => {
            app.dialog_state.plugins.status = e;
            None
        }
    }
}

/// Draw the window, the install confirmation and the parameter form (each when open).
pub(crate) fn show(app: &mut EffectcraftApp, ctx: &egui::Context) {
    poll(app);
    if app.dialog_state.plugins.open {
        window(app, ctx);
    }
    if app.dialog_state.plugins.review.is_some() {
        confirmation(app, ctx);
    }
    if app.dialog_state.plugins.form.is_some() {
        form(app, ctx);
    }
}

/// A short description of where a plug-in comes from (`source` of `plugin.list`).
fn source_text(source: &Value) -> String {
    let text = |k: &str| source.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    match source.get("type").and_then(Value::as_str) {
        Some("github") => format!("GitHub {} {}", text("repo"), text("tag")),
        Some("catalog") => format!("{} ({})", text("package"), text("catalog")),
        Some("file") => text("path"),
        _ => String::new(),
    }
}

/// Something a button of the window asked for, run once the window is drawn.
type Action = Box<dyn FnOnce(&mut EffectcraftApp, &egui::Context)>;

fn window(app: &mut EffectcraftApp, ctx: &egui::Context) {
    let mut open = true;
    let mut action: Option<Action> = None;
    egui::Window::new(tr("Plug-ins")).id(egui::Id::unique("plugins-window")).open(&mut open).default_size(vec2(560.0, 480.0)).show(ctx, |ui| {
        let busy = app.dialog_state.plugins.busy.as_ref().map(|(what, _)| what.clone());
        installed(app, ui, &mut action);
        ui.separator();
        install_sources(app, ui, busy.is_some(), &mut action);
        ui.separator();
        catalogs(app, ui, busy.is_some(), &mut action);
        let t = app.tokens;
        if let Some(what) = &busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(what);
            });
        }
        if !app.dialog_state.plugins.status.is_empty() {
            ui.label(egui::RichText::new(&app.dialog_state.plugins.status).color(t.text_dim));
        }
    });
    if !open {
        app.dialog_state.plugins.open = false;
    }
    if let Some(act) = action {
        act(app, ctx);
    }
}

/// The installed plug-ins: Remove, and a button per command of a command plug-in.
fn installed(app: &mut EffectcraftApp, ui: &mut egui::Ui, action: &mut Option<Action>) {
    let t = app.tokens;
    egui::ScrollArea::vertical().max_height(220.0).id_salt("plugins-installed").show(ui, |ui| {
        if app.session.plugins.list().is_empty() {
            ui.label(egui::RichText::new(tr("No plug-ins are installed.")).color(t.text_dim));
        }
        for p in app.session.plugins.list().to_vec() {
            ui.horizontal(|ui| installed_row(app, ui, &p, action));
        }
        for problem in app.session.plugins.problems() {
            ui.label(egui::RichText::new(problem).color(t.text_dim).size(11.0));
        }
        if !app.session.plugins.restart_needed().is_empty() {
            ui.label(egui::RichText::new(tr("Restart EffectCraft to finish updating or removing effect plug-ins.")).color(t.text_dim));
        }
    });
}

fn installed_row(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &plugin::Plugin, action: &mut Option<Action>) {
    let t = app.tokens;
    let m = &p.manifest;
    ui.label(egui::RichText::new(format!("{} {}", m.name, m.version)).color(t.tab_text_active));
    ui.label(egui::RichText::new(format!("{} · {}", m.kind, source_text(&p.source))).color(t.text_dim).size(11.0));
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let id = m.id.clone();
        let r = ui.button(tr("Remove"));
        app.auto.add(&format!("plugins.remove.{id}"), r.rect, tr("Remove"));
        if r.clicked() {
            *action = Some(Box::new(move |app: &mut EffectcraftApp, ctx: &egui::Context| {
                let _ = run(app, ctx, "plugin.remove", json!({"id": id}), tr("Plug-in removed"));
            }));
        }
        if !p.is_command() {
            return;
        }
        for c in m.commands.iter().rev() {
            let (id, command) = (m.id.clone(), c.id.clone());
            let r = ui.button(c.label.as_str());
            app.auto.add(&format!("plugins.run.{id}.{command}"), r.rect, &c.label);
            if r.clicked() {
                *action = Some(Box::new(move |app: &mut EffectcraftApp, ctx: &egui::Context| {
                    if let Err(e) = crate::menus::invoke(app, ctx, "plugin.run", json!({"id": id, "command": command})) {
                        app.dialog_state.plugins.status = e;
                    }
                }));
            }
        }
    });
}

/// Install from a file or a GitHub repository.
fn install_sources(app: &mut EffectcraftApp, ui: &mut egui::Ui, busy: bool, action: &mut Option<Action>) {
    ui.heading(tr("Install"));
    let r = ui.button(tr("From File..."));
    app.auto.add("plugins.installFile", r.rect, tr("From File..."));
    if r.clicked() {
        *action = Some(Box::new(|app: &mut EffectcraftApp, ctx: &egui::Context| {
            if let Err(e) = crate::menus::invoke(app, ctx, "plugin.install", json!({})) {
                app.dialog_state.plugins.status = e;
            }
        }));
    }
    ui.horizontal(|ui| {
        ui.label(tr("GitHub repository:"));
        let r = ui.add(egui::TextEdit::singleline(&mut app.dialog_state.plugins.repo).hint_text("owner/repo@tag").desired_width(220.0));
        app.auto.add("plugins.repo", r.rect, tr("GitHub repository:"));
        let spec = app.dialog_state.plugins.repo.trim().to_string();
        let r = ui.add_enabled(!busy && !spec.is_empty(), egui::Button::new(tr("Review...")));
        app.auto.add("plugins.reviewRepo", r.rect, tr("Review..."));
        if r.clicked() {
            *action = Some(Box::new(move |app: &mut EffectcraftApp, ctx: &egui::Context| {
                download(app, ctx, format!("{} {spec}…", tr("Downloading")), json!({"repo": spec}))
            }));
        }
    });
}

/// The catalogs added, adding one, and their packages.
fn catalogs(app: &mut EffectcraftApp, ui: &mut egui::Ui, busy: bool, action: &mut Option<Action>) {
    ui.heading(tr("Catalogs"));
    for url in app.session.plugins.catalogs().to_vec() {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&url).size(11.0));
            if ui.small_button(tr("Remove")).clicked() {
                *action = Some(Box::new(move |app: &mut EffectcraftApp, ctx: &egui::Context| {
                    let _ = run(app, ctx, "plugin.catalog.add", json!({"url": url, "remove": true}), "");
                }));
            }
        });
    }
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut app.dialog_state.plugins.catalog).hint_text("https://…/index.json").desired_width(300.0));
        app.auto.add("plugins.catalogUrl", r.rect, tr("Add Catalog"));
        let url = app.dialog_state.plugins.catalog.trim().to_string();
        let r = ui.add_enabled(!url.is_empty(), egui::Button::new(tr("Add Catalog")));
        app.auto.add("plugins.addCatalog", r.rect, tr("Add Catalog"));
        if r.clicked() {
            *action = Some(Box::new(move |app: &mut EffectcraftApp, ctx: &egui::Context| {
                if run(app, ctx, "plugin.catalog.add", json!({"url": url}), "").is_some() {
                    app.dialog_state.plugins.catalog.clear();
                    read_catalogs(app, ctx);
                }
            }));
        }
        let r = ui.add_enabled(!busy && !app.session.plugins.catalogs().is_empty(), egui::Button::new(tr("Refresh")));
        app.auto.add("plugins.refreshCatalogs", r.rect, tr("Refresh"));
        if r.clicked() {
            *action = Some(Box::new(read_catalogs));
        }
    });
    let t = app.tokens;
    egui::ScrollArea::vertical().max_height(160.0).id_salt("plugins-catalog").show(ui, |ui| {
        for pkg in app.dialog_state.plugins.packages.clone() {
            ui.horizontal(|ui| package_row(app, ui, &pkg, busy, action));
        }
        for w in &app.dialog_state.plugins.warnings {
            ui.label(egui::RichText::new(w).color(t.text_dim).size(11.0));
        }
    });
}

fn package_row(app: &mut EffectcraftApp, ui: &mut egui::Ui, pkg: &Value, busy: bool, action: &mut Option<Action>) {
    let t = app.tokens;
    let text = |k: &str| pkg.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    ui.label(egui::RichText::new(format!("{} {}", text("name"), text("version"))).color(t.tab_text_active));
    ui.label(egui::RichText::new(text("description")).color(t.text_dim).size(11.0));
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let label = if pkg.get("installed").is_some_and(|v| !v.is_null()) { tr("Reinstall...") } else { tr("Review...") };
        let r = ui.add_enabled(!busy, egui::Button::new(label));
        app.auto.add(&format!("plugins.review.{}", text("id")), r.rect, label);
        if r.clicked() {
            let request = json!({"catalog": text("catalog"), "id": text("id")});
            let what = format!("{} {}…", tr("Downloading"), text("name"));
            *action = Some(Box::new(move |app: &mut EffectcraftApp, ctx: &egui::Context| download(app, ctx, what, request)));
        }
    });
}

/// Every install's confirmation: what the plug-in may do, where it comes from and its SHA-256.
fn confirmation(app: &mut EffectcraftApp, ctx: &egui::Context) {
    let t = app.tokens;
    let Some((request, info)) = app.dialog_state.plugins.review.clone() else { return };
    let text = |k: &str| info.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let mut answer: Option<bool> = None;
    super::dialogs::modal(ctx, tr("Install Plug-in"), vec2(520.0, 380.0), &t, |ui| {
        let line = |ui: &mut egui::Ui, head: &str, value: String| {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(head).color(t.text_dim));
                ui.label(egui::RichText::new(value).color(t.tab_text_active));
            });
        };
        line(ui, tr("Plug-in:"), format!("{} {} ({})", text("name"), text("version"), text("id")));
        if !text("author").is_empty() {
            line(ui, tr("Author:"), text("author"));
        }
        line(ui, tr("Kind:"), text("kind"));
        line(ui, tr("Source:"), source_text(&info["source"]));
        line(ui, "SHA-256:", text("sha256"));
        let perms = &info["permissions"];
        let access = perms.get("access").and_then(Value::as_str).unwrap_or("read");
        let access = if access == "write" { tr("read and change the project") } else { tr("read the project") };
        line(ui, tr("It may:"), access.to_string());
        let commands: Vec<&str> = perms.get("commands").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
        if text("kind") == "command" {
            let allowed = if commands.is_empty() { tr("any command plug-ins may run").to_string() } else { commands.join(", ") };
            line(ui, tr("Commands:"), allowed);
        }
        if perms.get("saveFile").and_then(Value::as_bool) == Some(true) {
            line(ui, tr("Files:"), tr("may offer files to save").to_string());
        }
        if info["inCatalog"] == json!(false) {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(tr("This plug-in is not in any catalog you use. Install it only if you trust its author.")).color(t.tab_text_active));
        }
        if let Some(old) = info["replaces"].as_str() {
            line(ui, tr("Replaces version:"), old.to_string());
        }
        if info["permissionsChanged"] == json!(true) {
            ui.label(egui::RichText::new(tr("This version asks for different permissions.")).color(t.tab_text_active));
        }
        if info["stored"] == json!(false) {
            ui.label(egui::RichText::new(tr("It is installed for this session only.")).color(t.text_dim));
        }
        ui.add_space(12.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.button(tr("Install"));
            app.auto.add("dialog.pluginInstall.install", r.rect, tr("Install"));
            if r.clicked() {
                answer = Some(true);
            }
            let r = ui.button(tr("Cancel"));
            app.auto.add("dialog.pluginInstall.cancel", r.rect, tr("Cancel"));
            if r.clicked() {
                answer = Some(false);
            }
        });
    });
    match answer {
        Some(true) => {
            app.dialog_state.plugins.review = None;
            let mut p = request.as_object().cloned().unwrap_or_default();
            p.insert("sha256".into(), info["sha256"].clone());
            if let Some(done) = run(app, ctx, "plugin.install", Value::Object(p), tr("Plug-in installed"))
                && done["restartNeeded"] == json!(true)
            {
                app.dialog_state.plugins.status = tr("Plug-in installed: restart EffectCraft to use this version").to_string();
            }
        }
        Some(false) => app.dialog_state.plugins.review = None,
        None => {}
    }
}

/// One field of a plug-in command's form.
fn field(ui: &mut egui::Ui, f: &plugin::craft_plugin::Param, value: &mut Value) {
    match f.kind {
        ParamKind::Boolean => {
            let mut on = value.as_bool().unwrap_or(false);
            if ui.checkbox(&mut on, "").changed() {
                *value = json!(on);
            }
        }
        ParamKind::Number => {
            let mut n = value.as_f64().unwrap_or(0.0);
            let mut drag = egui::DragValue::new(&mut n).speed(0.1);
            if let (Some(lo), Some(hi)) = (f.min, f.max) {
                drag = drag.range(lo..=hi);
            }
            if ui.add(drag).changed() {
                *value = json!(n);
            }
        }
        ParamKind::Slider => {
            let mut n = value.as_f64().unwrap_or(0.0);
            let (lo, hi) = (f.min.unwrap_or(0.0), f.max.unwrap_or(1.0));
            if ui.add(egui::Slider::new(&mut n, lo..=hi)).changed() {
                *value = json!(n);
            }
        }
        ParamKind::Select | ParamKind::List => {
            let current = value.as_str().unwrap_or_default().to_string();
            craft_ui::ComboBox::from_id_salt(("plugin-param", f.name.as_str())).selected_text(current.as_str()).show_ui(ui, |ui| {
                for o in &f.options {
                    if ui.selectable_label(*o == current, o.as_str()).clicked() {
                        *value = json!(o);
                    }
                }
            });
        }
        // Text and colours (`#RRGGBB`) are typed.
        _ => {
            let mut s = value.as_str().unwrap_or_default().to_string();
            if ui.text_edit_singleline(&mut s).changed() {
                *value = json!(s);
            }
        }
    }
}

/// The parameters of a plug-in command, asked for before it runs.
fn form(app: &mut EffectcraftApp, ctx: &egui::Context) {
    let t = app.tokens;
    let Some((id, command, mut values)) = app.dialog_state.plugins.form.clone() else { return };
    let Some(m): Option<Manifest> = app.session.plugins.get(&id).map(|p| p.manifest.clone()) else {
        app.dialog_state.plugins.form = None;
        return;
    };
    let Some(c) = m.command(Some(command.as_str())).cloned() else {
        app.dialog_state.plugins.form = None;
        return;
    };
    let mut answer: Option<bool> = None;
    let title = format!("{}: {}", m.name, c.label.trim_end_matches(['…', '.']));
    super::dialogs::modal(ctx, &title, vec2(420.0, 300.0), &t, |ui| {
        egui::Grid::new("plugin-params").num_columns(2).show(ui, |ui| {
            for f in &c.params {
                if !f.is_field() {
                    if !f.label.is_empty() {
                        ui.label(egui::RichText::new(&f.label).color(t.text_dim));
                        ui.end_row();
                    }
                    continue;
                }
                ui.label(f.title());
                let v = values.entry(f.name.clone()).or_insert_with(|| f.default_value());
                field(ui, f, v);
                ui.end_row();
            }
        });
        ui.add_space(12.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.button(tr("OK"));
            app.auto.add("dialog.pluginParams.ok", r.rect, tr("OK"));
            if r.clicked() {
                answer = Some(true);
            }
            let r = ui.button(tr("Cancel"));
            app.auto.add("dialog.pluginParams.cancel", r.rect, tr("Cancel"));
            if r.clicked() {
                answer = Some(false);
            }
        });
    });
    match answer {
        Some(true) => {
            app.dialog_state.plugins.form = None;
            let _ = run(app, ctx, "plugin.run", json!({"id": id, "command": command, "params": Value::Object(values)}), "");
        }
        Some(false) => app.dialog_state.plugins.form = None,
        None => app.dialog_state.plugins.form = Some((id, command, values)),
    }
}
