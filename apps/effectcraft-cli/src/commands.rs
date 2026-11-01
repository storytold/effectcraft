//! The subcommands that drive a session: `info`, `commands`, `exec`, `run`, `props`, `get`,
//! `set`, `render-frame`, `script` and `mcp`.

use effectcraft_automation::tools::{self, Reply};
use effectcraft_automation::{Backend, McpServer};
use serde_json::{Value, json};

use crate::args::{
    BridgeArg, CommandsArgs, ExecArgs, GetArgs, GpuArg, InfoArgs, McpArgs, PropsArgs, RenderFrameArgs, RunArgs, SaveArgs, ScriptArgs, SetArgs, SourceArgs,
};
use crate::error::{CliError, CommandsFileError, Result, UsageError};
use crate::io::Io;
use crate::steps::{parse_commands, read_commands};
use crate::template;

/// Which session a command runs against.
pub struct Sel<'a> {
    pub project: Option<&'a str>,
    pub demo: bool,
    pub empty: bool,
    pub bridge: Option<&'a str>,
    pub gpu: bool,
    pub autosave: bool,
}

impl<'a> Sel<'a> {
    pub fn new(source: &'a SourceArgs, bridge: &'a BridgeArg, gpu: &GpuArg, project: Option<&'a String>) -> Self {
        Sel { project: project.map(String::as_str), demo: source.demo, empty: source.empty, bridge: bridge.bridge.as_deref(), gpu: gpu.gpu, autosave: false }
    }
}

/// A comp/layer reference: number → id, `-` → none (active), else a name / `#n`.
fn reference(s: &str) -> Option<Value> {
    match s {
        "-" | "" => None,
        _ => Some(s.parse::<u64>().map(Value::from).unwrap_or_else(|_| json!(s))),
    }
}

/// JSON if it parses, else the bare string.
fn value_arg(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or_else(|_| json!(s))
}

/// Build the backend: a bridge to the app, or a headless session with the requested project.
pub fn backend(sel: &Sel, default_demo: bool) -> Result<Backend> {
    if let Some(addr) = sel.bridge {
        return Ok(Backend::bridge(addr)?);
    }
    let mut b = Backend::headless(session(sel)?);
    if let Some(p) = sel.project {
        b.exec("file.open", json!({"path": p}))?;
    } else if sel.demo || (default_demo && !sel.empty) {
        b.exec("file.openDemoProject", json!({}))?;
    }
    Ok(b)
}

/// A wired session; `--gpu` attaches the GPU compositor (renders then follow the project's
/// renderer setting, Mercury GPU Acceleration by default).
pub fn session(sel: &Sel) -> Result<effectcraft_engine::Session> {
    let mut s = effectcraft_host::session();
    if sel.autosave {
        let dir = effectcraft_host::config_dir().ok_or(CliError::NoConfigDir)?;
        // Read only auto-save settings. Opting in must not change headless rendering or load
        // the desktop's models, shortcuts, disk cache or recovery sentinel.
        use effectcraft_engine::config::ConfigStore;
        let config = effectcraft_engine::config::DirConfig::new(dir);
        if let Some(text) = config.read(effectcraft_engine::prefs::PREFS_FILE) {
            s.prefs.auto_save = effectcraft_engine::prefs::Prefs::from_json(&text).auto_save;
        }
        s.prefs.normalize();
    }
    if sel.gpu {
        let g = effectcraft_gpu::Gpu::try_headless().map_err(CliError::NoGpu)?;
        s.accel = Some(std::sync::Arc::new(g));
    } else {
        s.accel_note = Some("headless: renders on the CPU unless started with --gpu".into());
    }
    Ok(s)
}

/// `--save` / `--save-as` after a mutating command.
fn maybe_save(b: &mut Backend, save: &SaveArgs) -> Result<Option<String>> {
    let r = if let Some(p) = &save.save_as {
        b.exec("file.saveAs", json!({"path": p}))?
    } else if save.save {
        b.exec("file.save", json!({})).map_err(|source| CliError::SaveWithoutProject { source })?
    } else {
        return Ok(None);
    };
    Ok(r.get("path").and_then(Value::as_str).map(str::to_string).or_else(|| save.save_as.clone()))
}

fn tool(b: &mut Backend, name: &str, a: Value) -> Result<Value> {
    match tools::run(b, name, &a)? {
        Reply::Json(v) => Ok(v),
        Reply::Image { info, .. } => Ok(info),
    }
}

/// Print a result: compact JSON with `--json`, else pretty JSON.
pub fn emit(io: &mut Io, v: &Value, json_out: bool) {
    if json_out {
        say!(io, "{v}");
    } else {
        say!(io, "{}", serde_json::to_string_pretty(v).unwrap_or_default());
    }
}

fn with_saved(v: Value, saved: Option<String>) -> Value {
    match saved {
        None => v,
        Some(p) => json!({"result": v, "saved": p}),
    }
}

/// `renderQueue.render` reports a failed item in its result, not as an error. Under `run`/`exec`
/// that item fails the command (#1426), as it fails `render --queue`.
fn failed_render(id: &str, r: &Value) -> Option<String> {
    if id != "renderQueue.render" {
        return None;
    }
    let failed: Vec<String> = r["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| Some(format!("{}: {}", i["compName"].as_str().unwrap_or("?"), i["status"].get("Failed")?.as_str().unwrap_or("failed"))))
        .collect();
    (!failed.is_empty()).then(|| failed.join("; "))
}

pub fn info(a: &InfoArgs, json_out: bool, io: &mut Io) -> Result {
    let mut b = backend(&Sel::new(&a.source, &a.bridge, &a.gpu, a.project.as_ref()), true)?;
    let p = tool(&mut b, "get_project", json!({}))?;
    let comp = tool(&mut b, "get_comp", json!({})).ok();
    let info = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "mode": if b.is_bridge() { "bridge" } else { "headless" },
        "commands": effectcraft_engine::command_specs().len(),
        "effects": effectcraft_engine::effects::registry().len(),
        "project": p,
        "activeComp": comp,
    });
    if json_out {
        emit(io, &info, true);
    } else {
        say!(
            io,
            "EffectCraft {} ({}), {} commands, {} effects",
            info["version"].as_str().unwrap_or(""),
            info["mode"].as_str().unwrap_or(""),
            info["commands"],
            info["effects"]
        );
        say!(io, "project: {}", p["path"].as_str().unwrap_or("(unsaved)"));
        for i in p["items"].as_array().into_iter().flatten() {
            say!(io, "  item {:>3}  {:<12} {}", i["id"], i["type"].as_str().unwrap_or(""), i["name"].as_str().unwrap_or(""));
        }
        if let Some(c) = comp {
            say!(
                io,
                "active comp {} \"{}\" {}x{} @ {} fps, {} s",
                c["id"],
                c["name"].as_str().unwrap_or(""),
                c["width"],
                c["height"],
                c["frameRate"],
                c["duration"]
            );
            for l in c["layers"].as_array().into_iter().flatten() {
                say!(io, "  #{:<3} id {:<4} {:<10} {}", l["index"], l["id"], l["type"].as_str().unwrap_or(""), l["name"].as_str().unwrap_or(""));
            }
        }
    }
    Ok(())
}

/// `commands` / `exec --list`: every engine command (id, label, shortcut, params doc; with
/// `--schemas` also each command's params JSON Schema).
fn list_commands(sel: &Sel, filter: Option<&str>, enabled: bool, schemas: bool, json_out: bool, io: &mut Io) -> Result {
    let mut b = backend(sel, false)?;
    let v = tool(&mut b, "list_commands", json!({"filter": filter, "enabled_only": enabled, "schemas": schemas}))?;
    if json_out {
        emit(io, &v, true);
    } else {
        for c in v.as_array().into_iter().flatten() {
            say!(
                io,
                "{:<36} {:<36} {:<16} {}",
                c["id"].as_str().unwrap_or(""),
                c["label"].as_str().unwrap_or(""),
                c["shortcut"].as_str().unwrap_or(""),
                c["params"].as_str().unwrap_or("")
            );
        }
    }
    Ok(())
}

pub fn commands(a: &CommandsArgs, json_out: bool, io: &mut Io) -> Result {
    let sel = Sel::new(&a.source, &a.bridge, &a.gpu, a.project.as_ref());
    list_commands(&sel, a.filter.as_deref(), a.enabled, a.schemas, json_out, io)
}

pub fn exec(a: &mut ExecArgs, json_out: bool, io: &mut Io) -> Result {
    let sel = Sel::new(&a.source, &a.bridge, &a.gpu, a.project.as_ref());
    if a.list {
        return list_commands(&sel, a.filter.as_deref(), a.enabled, a.schemas, json_out, io);
    }
    let Some(call) = a.call.take() else { return Err(UsageError::Missing("exec <command-id> [<params JSON>]").into()) };
    let mut b = backend(&sel, true)?;
    let r = b.exec(&call.id, call.params)?;
    if let Some(message) = failed_render(&call.id, &r) {
        return Err(CliError::RenderFailed(message));
    }
    let saved = maybe_save(&mut b, &a.save)?;
    emit(io, &with_saved(r, saved), json_out);
    Ok(())
}

/// The `<id> [<json>]` pairs of `run` as steps: an id followed by an argument that starts with `{`
/// is that command's params.
fn pair_steps(pairs: &[String]) -> Result<Vec<(String, Value)>> {
    let mut steps = vec![];
    let mut i = 0;
    while let Some(id) = pairs.get(i) {
        let params = match pairs.get(i + 1).filter(|p| p.trim_start().starts_with('{')) {
            Some(p) => {
                i += 1;
                serde_json::from_str(p).map_err(|source| UsageError::BadValue { id: id.clone(), source })?
            }
            None => json!({}),
        };
        i += 1;
        steps.push((id.clone(), params));
    }
    Ok(steps)
}

pub fn run_cmds(a: &RunArgs, json_out: bool, io: &mut Io) -> Result {
    let mut pairs = a.pairs.clone();
    // Windows caps a command line at 32 767 characters: long batches come from a file.
    if let Some(f) = &a.args_file {
        let text = std::fs::read_to_string(f).map_err(|source| UsageError::BadCommands(CommandsFileError::ArgsFile { path: f.clone(), source }))?;
        pairs.extend(text.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from));
    }
    if pairs.is_empty() && a.commands.is_empty() {
        return Err(UsageError::NoSteps.into());
    }
    let mut steps = pair_steps(&pairs)?;
    for source in &a.commands {
        let text = read_commands(source).map_err(UsageError::BadCommands)?;
        steps.extend(parse_commands(source, &text, steps.len()).map_err(UsageError::BadCommands)?);
    }
    if steps.is_empty() {
        return Err(UsageError::BadCommands(CommandsFileError::Empty(a.commands.join(", "))).into());
    }
    let mut b = backend(&Sel::new(&a.source, &a.bridge, &a.gpu, a.project.as_ref()), true)?;
    let mut results = vec![];
    // The step's index closes the message: a generated list repeats ids, and callers match the
    // leading `<id>: `.
    for (step, (id, params)) in steps.into_iter().enumerate() {
        let r = b.exec(&id, params).map_err(|source| CliError::Step { id: id.clone(), step, source })?;
        if let Some(message) = failed_render(&id, &r) {
            return Err(CliError::StepRender { id, step, message });
        }
        if !json_out {
            emit(io, &r, false);
        }
        results.push(json!({"command": id, "result": r}));
    }
    let saved = maybe_save(&mut b, &a.save)?;
    if json_out {
        emit(io, &with_saved(json!(results), saved), true);
    }
    Ok(())
}

pub fn props(a: &PropsArgs, json_out: bool, io: &mut Io) -> Result {
    let [comp, layer] = &a.target[..] else { return Err(UsageError::Missing("props <comp> <layer>").into()) };
    let mut b = backend(&Sel::new(&a.source, &a.bridge, &a.gpu, a.project.as_ref()), true)?;
    let args = json!({"comp": reference(comp), "layer": reference(layer), "flat": a.flat || !json_out, "time": a.time, "depth": a.depth});
    let v = tool(&mut b, "get_layer", args)?;
    if json_out {
        emit(io, &v, true);
    } else {
        say!(io, "layer {} \"{}\" ({}) at {} s", v["id"], v["name"].as_str().unwrap_or(""), v["type"].as_str().unwrap_or(""), v["time"]);
        for p in v["properties"].as_array().into_iter().flatten() {
            let mut extra = String::new();
            if let Some(k) = p.get("keys") {
                extra += &format!("  [{k} keys]");
            }
            if let Some(e) = p.get("expression").and_then(Value::as_str) {
                extra += &format!("  expr: {e}");
            }
            say!(io, "  {:<44} {:<8} {}{extra}", p["path"].as_str().unwrap_or(""), p["type"].as_str().unwrap_or(""), p["value"]);
        }
    }
    Ok(())
}

pub fn get(a: &GetArgs, json_out: bool, io: &mut Io) -> Result {
    let [comp, layer, path] = &a.target[..] else { return Err(UsageError::Missing("get <comp> <layer> <path>").into()) };
    let mut b = backend(&Sel::new(&a.source, &a.bridge, &a.gpu, a.project.as_ref()), true)?;
    let v = tool(&mut b, "get_property", json!({"comp": reference(comp), "layer": reference(layer), "path": path, "time": a.time}))?;
    emit(io, &v, json_out);
    Ok(())
}

pub fn set(a: &SetArgs, json_out: bool, io: &mut Io) -> Result {
    let (Some(comp), Some(layer), Some(path)) = (a.target.first(), a.target.get(1), a.target.get(2)) else {
        return Err(UsageError::Missing("set <comp> <layer> <path> <value>").into());
    };
    let value = a.target.get(3).map(|v| value_arg(v));
    let mut b = backend(&Sel::new(&a.source, &a.bridge, &a.gpu, a.project.as_ref()), true)?;
    let args = json!({"comp": reference(comp), "layer": reference(layer), "path": path, "value": value, "time": a.time, "expression": a.expression});
    let v = tool(&mut b, "set_property", args)?;
    let saved = maybe_save(&mut b, &a.save)?;
    emit(io, &with_saved(v, saved), json_out);
    Ok(())
}

pub fn render_frame(a: &RenderFrameArgs, json_out: bool, io: &mut Io) -> Result {
    let sel = Sel::new(&a.source, &a.bridge, &a.gpu, a.project.as_ref());
    let mut b = if let Some(path) = &a.template.template {
        template::check_output(&a.out, path, a.template.values.as_deref())?;
        let mut s = session(&sel)?;
        template::prepare(&mut s, path, a.template.values.as_deref())?;
        Backend::headless(s)
    } else {
        backend(&sel, true)?
    };
    let comp = a.comp.as_deref().and_then(reference);
    let mut time = a.time;
    let mut max_side = a.max_side.unwrap_or(0);
    if a.frame.is_some() || a.scale.is_some() {
        let c = tool(&mut b, "get_comp", json!({"comp": comp}))?;
        if let Some(f) = a.frame {
            time = Some(f / c["frameRate"].as_f64().unwrap_or(30.0));
        }
        if let Some(k) = a.scale {
            let long = c["width"].as_f64().unwrap_or(0.0).max(c["height"].as_f64().unwrap_or(0.0));
            max_side = (long * k).round().max(1.0) as u32;
        }
    }
    let t0 = std::time::Instant::now();
    let f = b.render_to_file(comp.as_ref(), time, max_side, a.transparent, &a.out)?;
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let info = json!({"path": a.out, "comp": f.comp, "time": f.time, "width": f.width, "height": f.height, "ms": (ms * 10.0).round() / 10.0});
    if json_out {
        emit(io, &info, true);
    } else {
        note!(io, "rendered {}x{} at {:.3}s in {ms:.1} ms -> {}", f.width, f.height, f.time, a.out);
    }
    Ok(())
}

/// `script FILE.jsx [project]` / `script --eval CODE`: run JavaScript against a headless session
/// (an empty project unless one is given, or `--demo`) or the running app (`--bridge`).
pub fn script(a: &ScriptArgs, json_out: bool, io: &mut Io) -> Result {
    let (code, name) = match (&a.eval, &a.script) {
        (Some(c), _) => (c.clone(), "eval".to_string()),
        (None, Some(f)) => (std::fs::read_to_string(f).map_err(|source| CliError::ReadScript { path: f.clone(), source })?, f.clone()),
        (None, None) => return Err(UsageError::Missing("script FILE.jsx [F.ecproj] | --eval CODE").into()),
    };
    let mut b = backend(&Sel::new(&a.source, &a.bridge, &a.gpu, a.project.as_ref()), false)?;
    let r = b.exec("script.run", json!({"code": code, "name": name}))?;
    let saved = maybe_save(&mut b, &a.save)?;
    if json_out {
        let mut o = r.clone();
        if let Some(p) = &saved {
            o["saved"] = json!(p);
        }
        emit(io, &o, true);
    } else {
        if let Some(out) = r["output"].as_str().filter(|o| !o.is_empty()) {
            say!(io, "{out}");
        }
        if !r["result"].is_null() && r["error"].is_null() {
            say!(io, "{}", r["result"]);
        }
        if let Some(p) = saved {
            note!(io, "saved {p}");
        }
    }
    if let Some(e) = r.get("error").filter(|e| !e.is_null()) {
        let pos = match (e["line"].as_u64(), e["column"].as_u64()) {
            (Some(l), Some(c)) => format!(":{l}:{c}"),
            (Some(l), None) => format!(":{l}"),
            _ => String::new(),
        };
        let msg = format!("{}{pos}: {}", e["file"].as_str().unwrap_or(&name), e["message"].as_str().unwrap_or("script error"));
        // In JSON the error is in the result already.
        return Err(if json_out { CliError::Reported } else { CliError::Script(msg) });
    }
    Ok(())
}

pub fn mcp(a: &McpArgs) -> Result {
    let mut sel = Sel::new(&a.source, &a.bridge, &a.gpu, a.project.as_ref());
    sel.autosave = a.autosave;
    let mut b = backend(&sel, false)?;
    if !a.automation_read_root.is_empty() || !a.automation_write_root.is_empty() {
        let roots = |r: &[String]| r.iter().map(std::path::PathBuf::from).collect::<Vec<_>>();
        b.confine(effectcraft_automation::confine::Confinement::new(&roots(&a.automation_read_root), &roots(&a.automation_write_root))?);
    }
    let server = McpServer::new(b);
    let mut server = if a.autosave {
        let root = effectcraft_host::config_dir().ok_or(CliError::NoConfigDir)?;
        server.with_autosave(&root)?
    } else {
        server
    };
    // A client that closes its end of stdout has ended the session.
    match server.serve_stdio() {
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => Err(e.into()),
        _ => Ok(()),
    }
}
