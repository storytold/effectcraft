//! Programmatic control of the running app (agents, tests, MCP bridge).
//!
//! Transport-agnostic: a transport thread (TCP in the desktop app) sends [`ControlRequest`]s; the
//! UI thread handles them between frames and replies with `{"ok":…, "result"|"error":…}`.
//!
//! Methods:
//! - `engine.execute {command, params}` / `ui.menu.invoke {id, params}`: run any engine or UI command
//! - `engine.commands`: list engine commands; `ui.menu.list`: menu tree with enablement
//! - `ui.inspect`: UI state + summary; `ui.elements {prefix?}`: registered widgets (id, label, rect)
//! - `ui.set {tool?, workspace?, theme?, focused?, viewer?: {zoom, res, pan, grid, …}, timeline?: {pps, start, graphEditor, showModes}}`
//! - `ui.panel.show {panel}` / `ui.panel.close {panel}`
//! - `ui.click {id | x,y, button?, count?, modifiers?}` / `ui.move {x,y}` / `ui.scroll {x,y,dx,dy}`
//! - `ui.drag {from:{id|x,y}, to:{id|x,y}, steps?, modifiers?}`: synthetic press-move-release
//! - `ui.key {key, command?, shift?, alt?, ctrl?}` / `ui.type {text}`
//! - `ui.viewer.locate {x, y}`: screen point of a comp pixel; `ui.viewer.hit {x, y}`: comp pixel at a screen point
//! - `ui.timeline.locate {layer, time?}`: screen point of a layer bar (at a comp time)
//! - `ui.playback {action: play|stop|toggle}`
//! - `ui.screenshot {path?, panel?, id?}`: PNG of the window (or one panel / element)
//! - `render.frame {comp?, time?, max_side?, path?, base64?}`: PNG of a comp frame rendered by the session
//! - `ui.resize {width, height}` / `ui.focus` / `app.quit {force?}` (a modified project asks to save first unless `force`)
//!
//! The full reference with examples is `docs/control-protocol.md`.

use std::sync::mpsc::Sender;

use serde_json::{Value, json};

use crate::EffectcraftApp;
use crate::dock::PanelKind;
use crate::state::{Resolution, Tool};

/// Prefix marking errors that may resolve after another frame.
pub const RETRY: &str = "\u{1}";

pub struct ControlRequest {
    pub method: String,
    pub params: Value,
    pub reply: Sender<Value>,
}

impl ControlRequest {
    pub fn new(method: impl Into<String>, params: Value) -> (Self, std::sync::mpsc::Receiver<Value>) {
        let (tx, rx) = std::sync::mpsc::channel();
        (Self { method: method.into(), params, reply: tx }, rx)
    }
}

pub enum Outcome {
    Done(Value),
    /// Try again on a later frame (e.g. the element is not on screen yet).
    Retry(String),
    /// Reply once queued synthetic input has been processed.
    AfterInput,
    Screenshot {
        path: Option<String>,
        crop: Option<[f32; 4]>,
    },
    /// Reply once background jobs end (a `wait: true` command whose job runs in a worker).
    AwaitJobs(JobWaiter),
}

/// A `wait: true` engine command whose job was sent to the session's offload (the browser's
/// Web Workers) instead of blocking the UI thread: the reply goes out when the job ends.
pub struct JobWaiter {
    pub command: String,
    pub params: Value,
    /// The command's own result (with `wait: false`).
    pub result: Value,
    /// Ids of the jobs it started (`jobs.list` ids: `render`, `track`, `roto`…).
    pub jobs: Vec<String>,
    /// Length of [`effectcraft_engine::Session::offload_log`] when it started.
    pub log_len: usize,
}

/// Commands that block with `wait` (and its default).
fn waits(command: &str, params: &Value) -> bool {
    let default = command == "renderQueue.render";
    params.get("wait").and_then(Value::as_bool).unwrap_or(default)
}

/// Run an engine command. A waiting command (`wait: true`) whose job the session offloads
/// runs with `wait: false` instead, and the reply waits for the job ([`Outcome::AwaitJobs`]),
/// so the browser's UI keeps drawing while it runs.
fn execute_engine(app: &mut EffectcraftApp, id: &str, params: Value) -> Result<Outcome, String> {
    if !app.session.offloads() || !waits(id, &params) {
        return app.session.execute_checked(id, params).map(ok).map_err(|e| e.to_string());
    }
    let before: Vec<String> = app.session.jobs().into_iter().map(|j| j.id).collect();
    let log_len = app.session.offload_log.len();
    let mut p = params.clone();
    p["wait"] = json!(false);
    let result = app.session.execute_checked(id, p).map_err(|e| e.to_string())?;
    let jobs: Vec<String> = app.session.jobs().into_iter().map(|j| j.id).filter(|j| !before.contains(j)).collect();
    if jobs.is_empty() {
        return Ok(ok(result));
    }
    Ok(Outcome::AwaitJobs(JobWaiter { command: id.to_string(), params, result, jobs, log_len }))
}

impl JobWaiter {
    /// Whether its jobs are still running.
    pub fn pending(&self, app: &EffectcraftApp) -> bool {
        app.session.jobs().iter().any(|j| self.jobs.contains(&j.id))
    }

    /// The reply: what the blocking command would have returned (`running: false`, the final
    /// progress, render items and analysis status), or the job's error.
    pub fn finish(self, app: &mut EffectcraftApp) -> Value {
        let s = &mut app.session;
        s.poll_render();
        let errors: Vec<String> = s.offload_log.iter().skip(self.log_len).filter_map(|(_, e)| e.clone()).collect();
        if let Some(e) = errors.first() {
            return json!({"ok": false, "error": e});
        }
        let mut r = self.result;
        if r.get("running").is_some() {
            r["running"] = json!(false);
        }
        if self.command == "renderQueue.render" {
            let ids: Vec<Value> = r["items"].as_array().map(|a| a.iter().map(|i| i["id"].clone()).collect()).unwrap_or_default();
            if let Ok(list) = s.execute("renderQueue.list", json!({})) {
                r["items"] =
                    json!(list["items"].as_array().map(|a| a.iter().filter(|i| ids.contains(&i["id"])).cloned().collect::<Vec<_>>()).unwrap_or_default());
            }
            r["rendering"] = json!(s.is_rendering());
        }
        let prefix = self.command.split('.').next().unwrap_or("");
        if matches!(prefix, "warp" | "camera" | "roto")
            && r.get("layer").is_some()
            && let Ok(st) = s.execute(&format!("{prefix}.status"), json!({"layer": r["layer"], "effect": r["effect"]}))
        {
            r["status"] = st;
        }
        if r.get("progress").is_some() {
            r["progress"] = Value::Null;
        }
        json!({"ok": true, "result": r})
    }
}

fn ok(v: Value) -> Outcome {
    Outcome::Done(json!({"ok": true, "result": v}))
}
fn err(e: impl std::fmt::Display) -> Outcome {
    Outcome::Done(json!({"ok": false, "error": e.to_string()}))
}

fn modifiers(p: &Value) -> egui::Modifiers {
    let m = p.get("modifiers").unwrap_or(p);
    let b = |k: &str| m.get(k).and_then(Value::as_bool).unwrap_or(false);
    egui::Modifiers { alt: b("alt"), ctrl: b("ctrl"), shift: b("shift"), mac_cmd: cfg!(target_os = "macos") && b("command"), command: b("command") }
}

/// Resolve a point from `{id}` (element centre) or `{x, y}`.
fn point(app: &EffectcraftApp, p: &Value) -> Result<egui::Pos2, String> {
    if let Some(id) = p.get("id").and_then(Value::as_str) {
        let e = app.auto.find(id).ok_or_else(|| format!("{RETRY}no element `{id}` (see ui.elements)"))?;
        let fx = p.get("fx").and_then(Value::as_f64).unwrap_or(0.5) as f32;
        let fy = p.get("fy").and_then(Value::as_f64).unwrap_or(0.5) as f32;
        return Ok(egui::pos2(e.rect[0] + e.rect[2] * fx, e.rect[1] + e.rect[3] * fy));
    }
    let x = p.get("x").and_then(Value::as_f64).ok_or("need `id` or `x`,`y`")?;
    let y = p.get("y").and_then(Value::as_f64).ok_or("need `y`")?;
    Ok(egui::pos2(x as f32, y as f32))
}

pub fn handle(app: &mut EffectcraftApp, ctx: &egui::Context, req: &ControlRequest) -> Outcome {
    let p = &req.params;
    let s = |k: &str| p.get(k).and_then(Value::as_str);
    match req.method.as_str() {
        "engine.execute" | "ui.menu.invoke" => {
            let Some(id) = s("command").or(s("id")) else { return err("missing `command`") };
            let params = p.get("params").cloned().filter(|v| !v.is_null()).unwrap_or(json!({}));
            // `engine.execute` runs engine commands exactly as headless (never opens a dialog, e.g.
            // `comp.new {}` creates a default comp); UI-only ids (`tool.*`, `view.*`, `window.*`,
            // `playback.*` …) and `ui.menu.invoke` go through the menu dispatcher like a click.
            let r = if req.method == "engine.execute" && effectcraft_engine::find_command(id).is_some() {
                execute_engine(app, id, params)
            } else {
                crate::menus::invoke(app, ctx, id, params).map(ok)
            };
            match r {
                Ok(o) => o,
                Err(e) => {
                    app.ui.status = e.clone();
                    err(e)
                }
            }
        }
        "engine.commands" => match app.session.execute("command.list", json!({"filter": p.get("filter"), "enabledOnly": p.get("enabledOnly")})) {
            Ok(v) => ok(v),
            Err(e) => err(e),
        },
        "ui.menu.list" => ok(serde_json::to_value(crate::menus::menu_items(app)).unwrap_or_default()),
        "ui.inspect" => ok(inspect(app, ctx)),
        "ui.elements" => {
            let prefix = s("prefix").unwrap_or("");
            ok(serde_json::to_value(app.auto.query(prefix)).unwrap_or_default())
        }
        "ui.set" => {
            if let Some(t) = s("tool") {
                match Tool::from_name(t) {
                    Some(t) => app.ui.tool = t,
                    None => return err(format!("unknown tool `{t}`")),
                }
            }
            if let Some(w) = s("workspace") {
                match crate::dock::WORKSPACES.iter().find(|x| x.eq_ignore_ascii_case(w)) {
                    Some(w) => app.set_workspace(w),
                    None => return err(format!("unknown workspace `{w}`")),
                }
            }
            if let Some(th) = s("theme") {
                match crate::theme::ThemeKind::from_name(th) {
                    Some(k) => app.set_theme(ctx, k),
                    None => return err("unknown theme (dark, darker, light)"),
                }
            }
            if let Some(f) = s("focused").and_then(PanelKind::from_name) {
                app.ui.focused = f;
            }
            if let Some(v) = p.get("viewer") {
                if let Some(z) = v.get("zoom") {
                    app.ui.viewer.zoom = z.as_f64().map(|z| z as f32);
                }
                if let Some(r) = v.get("res").and_then(Value::as_str) {
                    app.ui.viewer.res = Resolution::ALL.into_iter().find(|x| x.label().eq_ignore_ascii_case(r)).unwrap_or(app.ui.viewer.res);
                }
                if let Some(Value::Array(a)) = v.get("pan") {
                    app.ui.viewer.pan = [a.first().and_then(Value::as_f64).unwrap_or(0.0) as f32, a.get(1).and_then(Value::as_f64).unwrap_or(0.0) as f32];
                }
                for (k, slot) in [
                    ("grid", &mut app.ui.viewer.grid),
                    ("rulers", &mut app.ui.viewer.rulers),
                    ("safeMargins", &mut app.ui.viewer.safe_margins),
                    ("transparencyGrid", &mut app.ui.viewer.transparency_grid),
                ] {
                    if let Some(b) = v.get(k).and_then(Value::as_bool) {
                        *slot = b;
                    }
                }
            }
            if let Some(tl) = p.get("timeline") {
                if let Some(v) = tl.get("pps") {
                    app.ui.timeline.pps = v.as_f64();
                }
                if let Some(v) = tl.get("start").and_then(Value::as_f64) {
                    app.ui.timeline.start = v;
                }
                if let Some(v) = tl.get("graphEditor").and_then(Value::as_bool) {
                    app.ui.timeline.graph_editor = v;
                }
                if let Some(v) = tl.get("showModes").and_then(Value::as_bool) {
                    app.ui.timeline.show_modes = v;
                }
                if let Some(Value::Array(a)) = tl.get("openLayers") {
                    app.ui.timeline.open_layers = a.iter().filter_map(Value::as_u64).collect();
                }
                if let Some(Value::Array(a)) = tl.get("openGroups") {
                    app.ui.timeline.open_groups = a.iter().filter_map(Value::as_u64).collect();
                }
            }
            if let Some(b) = p.get("menuBar").and_then(Value::as_bool) {
                app.ui.show_menu_bar = b;
            }
            // Project panel optional columns, e.g. ["type", "duration", "path", "comment"].
            if let Some(Value::Array(a)) = p.get("projectColumns") {
                let mut cols = vec![];
                for k in a.iter().filter_map(Value::as_str) {
                    crate::panels::project::set_column(&mut cols, k, true);
                }
                app.ui.project_columns = cols;
            }
            // Wiggler / Smoother / Motion Sketch settings (fields of `AnimToolsState`).
            if let Some(Value::Object(m)) = p.get("animTools") {
                let mut cur = serde_json::to_value(&app.ui.anim_tools).unwrap_or_default();
                for (k, v) in m {
                    cur[k] = v.clone();
                }
                match serde_json::from_value(cur) {
                    Ok(v) => app.ui.anim_tools = v,
                    Err(e) => return err(format!("animTools: {e}")),
                }
            }
            if let Some(b) = p.get("home").and_then(Value::as_bool) {
                app.ui.start_screen = b;
            }
            // Home screen tab: "home", "templates" or "learn".
            if let Some(tab) = p.get("homeTab").and_then(Value::as_str) {
                app.ui.home_learn = tab == "learn";
                app.ui.home_templates = tab == "templates";
            }
            ok(Value::Null)
        }
        "ui.panel.show" | "ui.panel.close" => {
            let Some(panel) = s("panel").and_then(PanelKind::from_name) else { return err("unknown `panel`") };
            if req.method == "ui.panel.show" {
                app.show_panel(panel);
            } else {
                app.ui.dock.close(panel);
            }
            ok(Value::Null)
        }
        "ui.click" | "ui.move" => {
            let pos = match point(app, p) {
                Ok(p) => p,
                Err(e) if e.starts_with(RETRY) => return Outcome::Retry(e.trim_start_matches(RETRY).to_string()),
                Err(e) => return err(e),
            };
            let m = modifiers(p);
            // egui keeps the modifiers from `ModifiersChanged` (not the ones on a pointer event)
            // and the synthetic events arrive one per frame: hold them for the whole gesture.
            app.synthetic.push(egui::Event::ModifiersChanged(m));
            app.synthetic.push(egui::Event::PointerMoved(pos));
            if req.method == "ui.click" {
                let button = match s("button") {
                    Some("right") => egui::PointerButton::Secondary,
                    Some("middle") => egui::PointerButton::Middle,
                    _ => egui::PointerButton::Primary,
                };
                let count = p.get("count").and_then(Value::as_u64).unwrap_or(1);
                for _ in 0..count {
                    app.synthetic.push(egui::Event::PointerButton { pos, button, pressed: true, modifiers: m });
                    app.synthetic.push(egui::Event::PointerButton { pos, button, pressed: false, modifiers: m });
                }
            }
            app.synthetic.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
            Outcome::AfterInput
        }
        "ui.drag" => {
            let (Some(from), Some(to)) = (p.get("from"), p.get("to")) else { return err("need `from` and `to`") };
            let (a, b) = match (point(app, from), point(app, to)) {
                (Ok(a), Ok(b)) => (a, b),
                (Err(e), _) | (_, Err(e)) if e.starts_with(RETRY) => return Outcome::Retry(e.trim_start_matches(RETRY).to_string()),
                (Err(e), _) | (_, Err(e)) => return err(e),
            };
            let steps = p.get("steps").and_then(Value::as_u64).unwrap_or(12).max(2);
            let m = modifiers(p);
            app.synthetic.push(egui::Event::ModifiersChanged(m));
            app.synthetic.push(egui::Event::PointerMoved(a));
            app.synthetic.push(egui::Event::PointerButton { pos: a, button: egui::PointerButton::Primary, pressed: true, modifiers: m });
            for i in 1..=steps {
                let f = i as f32 / steps as f32;
                app.synthetic.push(egui::Event::PointerMoved(a + (b - a) * f));
            }
            app.synthetic.push(egui::Event::PointerButton { pos: b, button: egui::PointerButton::Primary, pressed: false, modifiers: m });
            app.synthetic.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
            Outcome::AfterInput
        }
        "ui.scroll" => {
            let pos = match point(app, p) {
                Ok(p) => p,
                Err(e) => return err(e),
            };
            let dx = p.get("dx").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let dy = p.get("dy").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            app.synthetic.push(egui::Event::PointerMoved(pos));
            app.synthetic.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(dx, dy),
                modifiers: modifiers(p),
                phase: egui::TouchPhase::Move,
            });
            app.synthetic.push(egui::Event::PointerButton {
                pos: egui::pos2(-100.0, -100.0),
                button: egui::PointerButton::Extra1,
                pressed: false,
                modifiers: Default::default(),
            });
            Outcome::AfterInput
        }
        "ui.key" => {
            let Some(name) = s("key") else { return err("missing `key`") };
            let Some((mut m, key)) = crate::menus::parse_shortcut(name) else { return err(format!("unknown key `{name}`")) };
            let extra = modifiers(p);
            m |= extra;
            app.synthetic.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: m });
            app.synthetic.push(egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: m });
            Outcome::AfterInput
        }
        "ui.type" => {
            let Some(text) = s("text") else { return err("missing `text`") };
            app.synthetic.push(egui::Event::Text(text.to_string()));
            app.synthetic.push(egui::Event::Key { key: egui::Key::F35, physical_key: None, pressed: false, repeat: false, modifiers: Default::default() });
            Outcome::AfterInput
        }
        "ui.viewer.locate" => {
            let (Some(x), Some(y)) = (p.get("x").and_then(Value::as_f64), p.get("y").and_then(Value::as_f64)) else {
                return err("need `x`, `y` (comp pixels)");
            };
            match crate::panels::viewer::comp_to_screen(ctx, [x as f32, y as f32]) {
                Some(pt) => ok(json!({"x": pt.x, "y": pt.y})),
                None => Outcome::Retry("the composition viewer is not visible".into()),
            }
        }
        "ui.viewer.hit" => {
            let pos = match point(app, p) {
                Ok(p) => p,
                Err(e) => return err(e),
            };
            match crate::panels::viewer::screen_to_comp(ctx, pos) {
                Some(c) => ok(json!({"x": c[0], "y": c[1]})),
                None => err("the composition viewer is not visible"),
            }
        }
        "ui.timeline.locate" => {
            let Some(l) = p.get("layer").and_then(Value::as_u64) else { return err("need `layer` (id)") };
            match crate::panels::timeline::locate(app, l, p.get("time").and_then(Value::as_f64)) {
                Some((x, y)) => ok(json!({"x": x, "y": y})),
                None => Outcome::Retry("layer not visible in the timeline".into()),
            }
        }
        "ui.playback" => {
            let now = ctx.input(|i| i.time);
            match s("action").unwrap_or("toggle") {
                "play" => app.play(now),
                "stop" => app.stop(),
                "status" => {}
                _ => app.toggle_play(now),
            }
            ok(
                json!({"playing": app.playback.playing, "time": app.session.time().seconds(), "audio": app.audio.is_some(), "levelsDb": app.meter.level_db, "peaksDb": app.meter.peak_db}),
            )
        }
        "ui.screenshot" => {
            let crop = s("panel").and_then(PanelKind::from_name).and_then(|pk| app.auto.find(&format!("panel.{}", pk.id())).map(|e| e.rect));
            let crop = crop.or_else(|| s("id").and_then(|id| app.auto.find(id).map(|e| e.rect)));
            // Bring the window on screen so a frame is presented, without taking keyboard focus.
            app.raise_for_control(ctx);
            Outcome::Screenshot { path: s("path").map(str::to_string), crop }
        }
        "render.frame" => render_frame(app, p),
        "ui.resize" => {
            let w = p.get("width").and_then(Value::as_f64).unwrap_or(1600.0) as f32;
            let h = p.get("height").and_then(Value::as_f64).unwrap_or(1000.0) as f32;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w, h)));
            ok(Value::Null)
        }
        "ui.focus" => {
            // Explicit request: activate the app and take keyboard focus.
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            ok(Value::Null)
        }
        "app.quit" => {
            // A modified project asks to save first unless `force: true`.
            if p.get("force").and_then(Value::as_bool).unwrap_or(false) {
                app.dialog_state.unsaved.quitting = true;
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            ok(Value::Null)
        }
        m => err(format!("unknown method `{m}`")),
    }
}

/// `render.frame {comp?, time?, max_side?, path?, base64?}`: render a comp frame through the session
/// (independent of the viewer zoom/resolution) and return it as PNG, written to `path` (default a
/// temp file) or inline as base64 when `base64: true`.
fn render_frame(app: &EffectcraftApp, p: &Value) -> Outcome {
    let s = &app.session;
    let cid = match s.resolve_comp(p.get("comp")) {
        Ok(c) => c,
        Err(e) => return err(e),
    };
    let t = p.get("time").or(p.get("seconds")).and_then(Value::as_f64).map(effectcraft_time::Tick::from_seconds_f64).unwrap_or_else(|| s.time());
    let max_side = p.get("max_side").or(p.get("maxSide")).and_then(Value::as_u64).unwrap_or(0) as u32;
    let transparent = p.get("transparent").and_then(Value::as_bool).unwrap_or(false);
    let (w, h, rgba) = match s.render_rgba8_alpha(cid, t, max_side, transparent) {
        Ok(r) => r,
        Err(e) => return err(e),
    };
    let png = match encode_png(&rgba, w, h) {
        Ok(p) => p,
        Err(e) => return err(e),
    };
    let mut out = json!({"comp": cid.0, "time": t.seconds(), "width": w, "height": h});
    // (the web has no file system: inline unless a path is given)
    let inline = p.get("base64").and_then(Value::as_bool).unwrap_or(cfg!(target_arch = "wasm32") && p.get("path").is_none());
    if inline {
        out["png"] = json!(base64(&png));
        return ok(out);
    }
    let path = p
        .get("path")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("effectcraft-frame-{}.png", std::process::id())).to_string_lossy().to_string());
    match std::fs::write(&path, png) {
        Ok(()) => {
            out["path"] = json!(path);
            ok(out)
        }
        Err(e) => err(format!("cannot write {path}: {e}")),
    }
}

/// Standard base64 (RFC 4648, padded).
pub fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

pub fn inspect(app: &EffectcraftApp, ctx: &egui::Context) -> Value {
    let size = ctx.content_rect().size();
    json!({
        "window": [size.x, size.y],
        "pixelsPerPoint": ctx.pixels_per_point(),
        "fps": app.fps,
        "ui": app.ui,
        "playing": app.playback.playing,
        "time": app.session.time().seconds(),
        "activeComp": app.session.state.active_comp.map(|i| i.0),
        "selectedLayers": app.session.state.selected_layers.iter().map(|c| c.0).collect::<Vec<_>>(),
        "selectedKeys": app.session.state.selected_keys.len(),
        "elements": app.auto.previous.len(),
        "dialog": app.dialog.map(|d| format!("{d:?}")),
        "renderMs": app.frames.last_ms.lock().map(|v| *v).unwrap_or(0.0),
    })
}

/// Save a screenshot (optionally cropped to a rect in points) as PNG; returns the reply JSON.
pub fn save_screenshot(ctx: &egui::Context, image: &egui::ColorImage, path: Option<&str>, crop: Option<[f32; 4]>) -> Value {
    let ppp = ctx.pixels_per_point();
    let [w, h] = image.size;
    let (x0, y0, cw, ch) = match crop {
        Some([x, y, cw, ch]) => {
            let x0 = ((x * ppp) as usize).min(w);
            let y0 = ((y * ppp) as usize).min(h);
            (x0, y0, ((cw * ppp) as usize).min(w - x0), ((ch * ppp) as usize).min(h - y0))
        }
        None => (0, 0, w, h),
    };
    let mut rgba = Vec::with_capacity(cw * ch * 4);
    for y in y0..y0 + ch {
        for x in x0..x0 + cw {
            let c = image.pixels[y * w + x];
            rgba.extend_from_slice(&[c.r(), c.g(), c.b(), 255]);
        }
    }
    if path.is_none() && cfg!(target_arch = "wasm32") {
        // no file system on the web: inline PNG
        return match encode_png(&rgba, cw as u32, ch as u32) {
            Ok(png) => json!({"ok": true, "result": {"png": base64(&png), "width": cw, "height": ch}}),
            Err(e) => json!({"ok": false, "error": e}),
        };
    }
    let path = path.map(str::to_string).unwrap_or_else(|| std::env::temp_dir().join("effectcraft-screenshot.png").to_string_lossy().to_string());
    match encode_png(&rgba, cw as u32, ch as u32) {
        Ok(png) => match std::fs::write(&path, png) {
            Ok(()) => json!({"ok": true, "result": {"path": path, "width": cw, "height": ch}}),
            Err(e) => json!({"ok": false, "error": e.to_string()}),
        },
        Err(e) => json!({"ok": false, "error": e}),
    }
}

/// Minimal PNG encoder (stored deflate blocks, no compression dependency).
pub fn encode_png(rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    fn chunk(out: &mut Vec<u8>, ty: &[u8], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut c = Vec::with_capacity(4 + data.len());
        c.extend_from_slice(ty);
        c.extend_from_slice(data);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crc(&c).to_be_bytes());
    }
    fn crc(d: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for &b in d {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
            }
        }
        !c
    }
    let mut raw = Vec::with_capacity((w as usize * 4 + 1) * h as usize);
    for y in 0..h as usize {
        raw.push(0);
        raw.extend_from_slice(&rgba[y * w as usize * 4..(y + 1) * w as usize * 4]);
    }
    // zlib stream with stored blocks
    let mut z = vec![0x78, 0x01];
    for (i, block) in raw.chunks(65535).enumerate() {
        let last = (i + 1) * 65535 >= raw.len();
        z.push(last as u8);
        z.extend_from_slice(&(block.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        z.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_rfc4648_vectors() {
        for (i, o) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(super::base64(i.as_bytes()), o);
        }
    }
}
