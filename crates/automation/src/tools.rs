//! The tool catalogue: names, descriptions, JSON input schemas and implementations. Shared by the
//! MCP server (`tools/list`, `tools/call`) and the CLI's one-shot subcommands, so both behave the
//! same. Every tool maps onto engine commands (`Session::execute`) or control-channel methods.

use serde_json::{Map, Value, json};

use crate::backend::NEED_BRIDGE;
use crate::{Backend, Error, Result, recompress_png};

/// A tool's output.
pub enum Reply {
    Json(Value),
    /// A PNG plus a JSON description of it (comp, time, size, path).
    Image {
        png: Vec<u8>,
        info: Value,
    },
}

pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    /// Only listed (and usable) when bridged to the running app.
    pub bridge_only: bool,
    schema: fn() -> Value,
    run: fn(&mut Backend, &Value) -> Result<Reply>,
}

impl ToolDef {
    /// The JSON Schema of the tool's arguments.
    pub fn input_schema(&self) -> Value {
        (self.schema)()
    }
    /// The MCP `Tool` object, with a title and annotations (docs/mcp.md).
    pub fn descriptor(&self) -> Value {
        let title = title_of(self.name);
        let read_only = READ_ONLY.contains(&self.name);
        let files = WRITES_FILES.contains(&self.name);
        json!({
            "name": self.name,
            "title": title,
            "description": self.description,
            "inputSchema": self.input_schema(),
            "annotations": {
                "title": title,
                "readOnlyHint": read_only,
                "destructiveHint": !read_only && !files,
                "idempotentHint": read_only || files,
                "openWorldHint": false,
            },
        })
    }

    /// The first argument this tool doesn't take, as a message naming the accepted ones.
    pub fn unknown_arg(&self, args: &Value) -> Option<String> {
        let Some(m) = args.as_object() else { return Some("arguments must be an object".into()) };
        let schema = self.input_schema();
        let props = schema.get("properties")?.as_object()?;
        let bad = m.keys().find(|k| !props.contains_key(*k))?;
        let accepted: Vec<&str> = props.keys().map(String::as_str).collect();
        Some(format!("unknown argument \"{bad}\" for {}; expected: {}", self.name, accepted.join(", ")))
    }
}

/// Tools that only read.
const READ_ONLY: &[&str] = &[
    "command_list",
    "list_commands",
    "describe_command",
    "doc_inspect",
    "render_preview",
    "get_project",
    "get_comp",
    "get_layer",
    "get_property",
    "list_effects",
    "list_fonts",
    "get_state",
    "ui_inspect",
    "ui_elements",
];
/// Tools that write files without changing the project.
const WRITES_FILES: &[&str] = &["save_project", "render_frame", "screenshot"];

/// `get_project` → "Get project".
fn title_of(name: &str) -> String {
    let s = name.replace('_', " ");
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// All tools (headless + bridge-only).
pub fn catalogue() -> &'static [ToolDef] {
    TOOLS
}

pub fn find(name: &str) -> Option<&'static ToolDef> {
    TOOLS.iter().find(|t| t.name == name)
}

/// The tools available on a backend (bridge-only tools are hidden in headless mode).
pub fn available(bridge: bool) -> impl Iterator<Item = &'static ToolDef> {
    TOOLS.iter().filter(move |t| bridge || !t.bridge_only)
}

/// Run a tool by name.
pub fn run(b: &mut Backend, name: &str, args: &Value) -> Result<Reply> {
    let t = find(name).ok_or_else(|| Error::BadArgs(format!("unknown tool `{name}`")))?;
    if t.bridge_only && !b.is_bridge() {
        return Err(Error::BadArgs(NEED_BRIDGE.into()));
    }
    let empty = json!({});
    (t.run)(b, if args.is_null() { &empty } else { args })
}

// ------------------------------------------------------------------ schema fragments

fn schema(props: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": required, "additionalProperties": false})
}
fn comp_s() -> Value {
    json!({"type": ["integer", "string"], "description": "Composition item id or name; omit for the active comp."})
}
fn layer_s() -> Value {
    json!({"type": ["integer", "string"], "description": "Layer id (from get_comp), \"#n\" (1-based index from the top) or layer name."})
}
fn path_s() -> Value {
    json!({"type": "string", "description": "Property path from get_layer: match ids joined by `/`, `#n` for the nth child, or `@uid`. E.g. `transform/position`, `transform/opacity`, `transform/scale`, `transform/rotation`, `effects/#1/blurriness`, `masks/#1/feather`, `@57`."})
}
fn value_s() -> Value {
    json!({"description": "Number for 1-D properties (50), array for vectors ([960,540]), \"#rrggbb\" or [r,g,b,a] (0-1) for colours, bool for checkboxes, string for text/menus."})
}

// ------------------------------------------------------------------ helpers

/// Build a params object from the present, non-null entries.
fn obj(entries: &[(&str, Option<&Value>)]) -> Value {
    let mut m = Map::new();
    for (k, v) in entries {
        if let Some(v) = v.filter(|v| !v.is_null()) {
            m.insert(k.to_string(), v.clone());
        }
    }
    Value::Object(m)
}

fn get<'a>(a: &'a Value, k: &str) -> Option<&'a Value> {
    a.get(k).filter(|v| !v.is_null())
}

fn need<'a>(a: &'a Value, k: &str) -> Result<&'a Value> {
    get(a, k).ok_or_else(|| Error::BadArgs(format!("missing `{k}`")))
}

fn json_reply(v: Value) -> Result<Reply> {
    Ok(Reply::Json(v))
}

/// Accept params given as a JSON string too (some clients stringify nested objects).
fn params_of(v: Option<&Value>) -> Result<Value> {
    match v {
        None => Ok(json!({})),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(json!({})),
        Some(Value::String(s)) => serde_json::from_str(s).map_err(|e| Error::BadArgs(format!("`params` is not valid JSON: {e}"))),
        Some(v) => Ok(v.clone()),
    }
}

// ------------------------------------------------------------------ engine tools

fn list_commands(b: &mut Backend, a: &Value) -> Result<Reply> {
    let schemas = get(a, "schemas").and_then(Value::as_bool).unwrap_or(false).then_some(json!(true));
    let r = b.exec("command.list", obj(&[("filter", get(a, "filter")), ("enabledOnly", get(a, "enabled_only")), ("schemas", schemas.as_ref())]))?;
    // Drop empty fields to keep the listing compact.
    let list: Vec<Value> = r
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|mut c| {
            if let Some(o) = c.as_object_mut() {
                o.retain(|_, v| !(v.is_null() || v.as_array().is_some_and(Vec::is_empty) || v.as_str() == Some("{}")));
            }
            c
        })
        .collect();
    json_reply(json!(list))
}

fn execute_command(b: &mut Backend, a: &Value) -> Result<Reply> {
    let id = get(a, "command").or(get(a, "id")).and_then(Value::as_str).ok_or_else(|| Error::BadArgs("missing `command` (e.g. `comp.new`)".into()))?;
    let params = params_of(get(a, "params"))?;
    json_reply(b.exec(id, params)?)
}

fn describe_command(b: &mut Backend, a: &Value) -> Result<Reply> {
    let id = get(a, "command").or(get(a, "id")).and_then(Value::as_str).ok_or_else(|| Error::BadArgs("missing `command` (e.g. `layer.newText`)".into()))?;
    json_reply(b.exec("command.describe", json!({"command": id}))?)
}

fn list_effects(b: &mut Backend, a: &Value) -> Result<Reply> {
    json_reply(b.exec("effect.list", obj(&[("filter", get(a, "filter"))]))?)
}

fn list_fonts(b: &mut Backend, a: &Value) -> Result<Reply> {
    json_reply(b.exec("text.fonts", obj(&[("query", get(a, "query")), ("rescan", get(a, "rescan"))]))?)
}

/// `effect.apply` on one layer, then set parameters on the new instance by param id: one batch,
/// so one undo step, and a failing value leaves nothing applied.
fn add_effect(b: &mut Backend, a: &Value) -> Result<Reply> {
    let (layer, effect, comp) = (need(a, "layer")?, need(a, "effect")?, get(a, "comp"));
    let count = |t: &Value| effects_group(t).and_then(|g| g.get("children")).and_then(Value::as_array).map_or(0, Vec::len);
    let before = b.exec("layer.tree", obj(&[("layer", Some(layer)), ("comp", comp)]))?;
    let index = count(&before) + 1;
    let mut p = obj(&[("effect", Some(effect)), ("comp", comp)]);
    p["layers"] = json!([layer]);
    let mut steps = vec![json!({"command": "effect.apply", "params": p})];
    let prefix = format!("effects/#{index}");
    let mut set = vec![];
    if let Some(vals) = get(a, "values").and_then(Value::as_object) {
        for (k, v) in vals {
            let path = format!("{prefix}/{k}");
            steps.push(
                json!({"command": "prop.set", "params": obj(&[("layer", Some(layer)), ("comp", comp), ("path", Some(&json!(path))), ("value", Some(v))])}),
            );
            set.push(path);
        }
    }
    // The undo step is named like effect.apply's own ("Apply Gaussian Blur").
    let name = effect.as_str().map(|e| effectcraft_engine::effects::lookup(e).map_or(e, |s| s.name));
    let label = name.map(|n| json!(format!("Apply {n}")));
    b.exec("engine.batch", obj(&[("steps", Some(&json!(steps))), ("label", label.as_ref())]))?;
    let tree = b.exec("layer.tree", obj(&[("layer", Some(layer)), ("comp", comp)]))?;
    let inst = effects_group(&tree).and_then(|g| g.get("children")).and_then(Value::as_array).and_then(|c| c.get(index - 1)).cloned();
    let mut params = vec![];
    if let Some(i) = &inst {
        flatten(i, &mut params);
    }
    json_reply(json!({"layer": tree["id"], "index": index, "path": prefix, "name": inst.as_ref().map(|i| i["name"].clone()), "set": set, "params": params}))
}

/// The `effects` group of a `layer.tree` reply.
fn effects_group(tree: &Value) -> Option<&Value> {
    tree["properties"]["children"].as_array()?.iter().find(|c| c["path"] == "effects")
}

fn get_state(b: &mut Backend, _: &Value) -> Result<Reply> {
    let state = b.exec("editor.state", json!({}))?;
    let caps = b.exec("app.capabilities", json!({}))?;
    json_reply(json!({"state": state, "app": caps}))
}

fn run_script(b: &mut Backend, a: &Value) -> Result<Reply> {
    let code = get(a, "code").and_then(Value::as_str).ok_or_else(|| Error::BadArgs("missing `code` (JavaScript)".into()))?;
    let mut p = json!({"code": code});
    if let Some(n) = get(a, "name") {
        p["name"] = n.clone();
    }
    json_reply(b.exec("script.run", p)?)
}

fn get_project(b: &mut Backend, _: &Value) -> Result<Reply> {
    json_reply(b.exec("project.summary", json!({}))?)
}

fn get_comp(b: &mut Backend, a: &Value) -> Result<Reply> {
    json_reply(b.exec("comp.info", obj(&[("comp", get(a, "comp"))]))?)
}

/// Flatten a `layer.tree` property tree into `[{path, type, value, keys?, expression?}]`.
fn flatten(node: &Value, out: &mut Vec<Value>) {
    // A group cut off by `depth` stays visible (instead of vanishing from the flat list).
    if node.get("truncated").and_then(Value::as_bool) == Some(true) {
        out.push(json!({"path": node["path"], "type": "group", "name": node["name"], "truncated": true}));
        return;
    }
    match node.get("children").and_then(Value::as_array) {
        Some(ch) => ch.iter().for_each(|c| flatten(c, out)),
        None => {
            let mut o = json!({"path": node["path"], "type": node["type"], "value": node["value"]});
            if let Some(k) = node.get("keys").and_then(Value::as_array) {
                o["keys"] = json!(k.len());
            }
            if let Some(e) = node.get("expression") {
                o["expression"] = e.clone();
            }
            out.push(o);
        }
    }
}

fn get_layer(b: &mut Backend, a: &Value) -> Result<Reply> {
    let p = obj(&[("layer", Some(need(a, "layer")?)), ("comp", get(a, "comp")), ("depth", get(a, "depth")), ("time", get(a, "time"))]);
    let mut r = b.exec("layer.tree", p)?;
    if get(a, "flat").and_then(Value::as_bool) == Some(true) {
        let mut props = vec![];
        flatten(&r["properties"], &mut props);
        r["properties"] = json!(props);
    }
    json_reply(r)
}

fn get_property(b: &mut Backend, a: &Value) -> Result<Reply> {
    let p = obj(&[("layer", Some(need(a, "layer")?)), ("path", Some(need(a, "path")?)), ("comp", get(a, "comp")), ("time", get(a, "time"))]);
    json_reply(b.exec("prop.get", p)?)
}

fn set_property(b: &mut Backend, a: &Value) -> Result<Reply> {
    let (layer, path, comp, time) = (need(a, "layer")?, need(a, "path")?, get(a, "comp"), get(a, "time"));
    let (value, expr) = (get(a, "value"), get(a, "expression"));
    if value.is_none() && expr.is_none() {
        return Err(Error::BadArgs("give `value` and/or `expression`".into()));
    }
    let base = [("layer", Some(layer)), ("path", Some(path)), ("comp", comp)];
    if let Some(v) = value {
        let mut p = obj(&base);
        p["value"] = v.clone();
        match time {
            Some(t) => {
                p["time"] = t.clone();
                b.exec("prop.addKey", p)?;
            }
            None => {
                b.exec("prop.set", p)?;
            }
        }
    }
    if let Some(e) = expr {
        let mut p = obj(&base);
        p["expression"] = e.clone();
        b.exec("prop.setExpression", p)?;
    }
    json_reply(b.exec("prop.get", obj(&[("layer", Some(layer)), ("path", Some(path)), ("comp", comp), ("time", time)]))?)
}

fn add_keyframe(b: &mut Backend, a: &Value) -> Result<Reply> {
    let (layer, path, comp) = (need(a, "layer")?, need(a, "path")?, get(a, "comp"));
    let keys: Vec<Value> = match get(a, "keys") {
        Some(Value::Array(k)) => k.clone(),
        Some(_) => return Err(Error::BadArgs("`keys` must be an array of {time, value?}".into())),
        None => vec![obj(&[("time", Some(need(a, "time")?)), ("value", get(a, "value"))])],
    };
    if keys.is_empty() {
        return Err(Error::BadArgs("no keys".into()));
    }
    let base = [("layer", Some(layer)), ("path", Some(path)), ("comp", comp)];
    for k in &keys {
        let mut p = obj(&base);
        p["time"] = need(k, "time")?.clone();
        if let Some(v) = get(k, "value") {
            p["value"] = v.clone();
        }
        b.exec("prop.addKey", p)?;
    }
    let info = b.exec("prop.get", obj(&base))?;
    if let Some(interp) = get(a, "interpolation").and_then(Value::as_str) {
        // Key selection works on the active comp.
        if let Some(c) = comp {
            b.exec("comp.open", json!({"comp": c}))?;
        }
        let sel: Vec<Value> = keys.iter().map(|k| json!({"layer": info["layer"], "prop": info["uid"], "time": k["time"]})).collect();
        b.exec("keys.select", json!({"keys": sel}))?;
        match interp.to_ascii_lowercase().as_str() {
            "easyease" | "ease" => b.exec("keys.easyEase", json!({}))?,
            "easyeasein" => b.exec("keys.easyEase", json!({"which": "in"}))?,
            "easyeaseout" => b.exec("keys.easyEase", json!({"which": "out"}))?,
            i @ ("linear" | "bezier" | "hold") => b.exec("keys.interpolation", json!({"interpolation": i}))?,
            other => return Err(Error::BadArgs(format!("unknown interpolation `{other}` (linear, bezier, hold, easyEase, easyEaseIn, easyEaseOut)"))),
        };
        return json_reply(b.exec("prop.get", obj(&base))?);
    }
    json_reply(info)
}

fn render_frame(b: &mut Backend, a: &Value) -> Result<Reply> {
    let max_side = get(a, "max_side").and_then(Value::as_u64).unwrap_or(960) as u32;
    let transparent = get(a, "transparent").and_then(Value::as_bool).unwrap_or(false);
    let f = b.render_with(get(a, "comp"), get(a, "time").and_then(Value::as_f64), max_side, transparent)?;
    let mut info = json!({"comp": f.comp, "time": f.time, "width": f.width, "height": f.height});
    if let Some(p) = get(a, "path").and_then(Value::as_str) {
        std::fs::write(p, &f.png).map_err(|e| Error::Other(format!("cannot write {p}: {e}")))?;
        info["path"] = json!(p);
    }
    if get(a, "inline").and_then(Value::as_bool) == Some(false) {
        return json_reply(info);
    }
    Ok(Reply::Image { png: f.png, info })
}

fn open_project(b: &mut Backend, a: &Value) -> Result<Reply> {
    if let Some(p) = get(a, "path") {
        b.exec("file.open", json!({"path": p}))?;
    } else if get(a, "demo").and_then(Value::as_bool) == Some(true) {
        b.exec("file.openDemoProject", json!({}))?;
    } else if get(a, "new").and_then(Value::as_bool) == Some(true) {
        b.exec("file.newProject", json!({}))?;
    } else {
        return Err(Error::BadArgs("give `path` (.ecproj), `demo: true` or `new: true`".into()));
    }
    get_project(b, &json!({}))
}

fn save_project(b: &mut Backend, a: &Value) -> Result<Reply> {
    match get(a, "path") {
        Some(p) => b.exec("file.saveAs", json!({"path": p}))?,
        None => b.exec("file.save", json!({}))?,
    };
    let s = b.exec("project.summary", json!({}))?;
    json_reply(json!({"path": s["path"], "dirty": s["dirty"]}))
}

fn history(b: &mut Backend, a: &Value, id: &str) -> Result<Reply> {
    let steps = get(a, "steps").and_then(Value::as_u64).unwrap_or(1).max(1);
    let mut done = 0;
    for _ in 0..steps {
        match b.exec(id, json!({})) {
            Ok(_) => done += 1,
            Err(e) if done == 0 => return Err(e),
            Err(_) => break,
        }
    }
    let s = b.exec("project.summary", json!({}))?;
    json_reply(json!({"steps": done, "undoStack": s["undo"]}))
}

fn undo(b: &mut Backend, a: &Value) -> Result<Reply> {
    history(b, a, "edit.undo")
}

fn redo(b: &mut Backend, a: &Value) -> Result<Reply> {
    history(b, a, "edit.redo")
}

fn script_ui_tool(b: &mut Backend, a: &Value) -> Result<Reply> {
    let action = get(a, "action").and_then(Value::as_str).unwrap_or("list");
    let id = match action {
        "list" => "scriptui.list",
        "get" => "scriptui.get",
        "click" => "scriptui.click",
        "set" => "scriptui.set",
        "close" => "scriptui.close",
        other => return Err(Error::BadArgs(format!("unknown action `{other}` (list | get | click | set | close)"))),
    };
    let p = obj(&[("window", get(a, "window")), ("widget", get(a, "widget")), ("value", get(a, "value")), ("result", get(a, "result"))]);
    json_reply(b.exec(id, p)?)
}

fn history_tool(b: &mut Backend, a: &Value) -> Result<Reply> {
    if let Some(g) = get(a, "goto") {
        let p = match g {
            Value::String(id) if id.parse::<u64>().is_err() => json!({"id": id}),
            Value::String(n) => json!({"index": n.parse::<u64>().unwrap_or(0)}),
            v => json!({"index": v}),
        };
        b.exec("edit.history.goto", p)?;
    }
    json_reply(b.exec("edit.history.list", json!({}))?)
}

fn batch_tool(b: &mut Backend, a: &Value) -> Result<Reply> {
    let steps = need(a, "steps")?;
    let steps = match steps {
        Value::String(_) => params_of(Some(steps))?,
        v => v.clone(),
    };
    json_reply(b.exec("engine.batch", obj(&[("steps", Some(&steps)), ("label", get(a, "label")), ("atomic", get(a, "atomic"))]))?)
}

// ------------------------------------------------------------------ bridge-only tools

fn screenshot(b: &mut Backend, a: &Value) -> Result<Reply> {
    let max_side = get(a, "max_side").and_then(Value::as_u64).unwrap_or(1600) as u32;
    let keep = get(a, "path").and_then(Value::as_str).map(str::to_string);
    let path =
        keep.clone().unwrap_or_else(|| std::env::temp_dir().join(format!("effectcraft-mcp-shot-{}.png", std::process::id())).to_string_lossy().into_owned());
    b.control("ui.screenshot", obj(&[("path", Some(&json!(path))), ("panel", get(a, "panel")), ("id", get(a, "id"))]))?;
    let bytes = std::fs::read(&path).map_err(|e| Error::Other(format!("screenshot: {e}")))?;
    if keep.is_none() {
        let _ = std::fs::remove_file(&path);
    }
    let (png, w, h) = recompress_png(&bytes, max_side)?;
    Ok(Reply::Image { png, info: json!({"width": w, "height": h, "path": keep}) })
}

fn passthrough(b: &mut Backend, method: &str, a: &Value) -> Result<Reply> {
    json_reply(b.control(method, a.clone())?)
}

fn ui_inspect(b: &mut Backend, a: &Value) -> Result<Reply> {
    passthrough(b, "ui.inspect", a)
}
fn ui_elements(b: &mut Backend, a: &Value) -> Result<Reply> {
    passthrough(b, "ui.elements", a)
}
fn ui_click(b: &mut Backend, a: &Value) -> Result<Reply> {
    passthrough(b, "ui.click", a)
}
fn ui_drag(b: &mut Backend, a: &Value) -> Result<Reply> {
    passthrough(b, "ui.drag", a)
}
fn ui_key(b: &mut Backend, a: &Value) -> Result<Reply> {
    passthrough(b, "ui.key", a)
}
fn ui_type(b: &mut Backend, a: &Value) -> Result<Reply> {
    passthrough(b, "ui.type", a)
}
fn ui_set(b: &mut Backend, a: &Value) -> Result<Reply> {
    passthrough(b, "ui.set", a)
}
fn control(b: &mut Backend, a: &Value) -> Result<Reply> {
    let m = need(a, "method")?.as_str().ok_or_else(|| Error::BadArgs("`method` must be a string".into()))?;
    let p = params_of(get(a, "params"))?;
    json_reply(b.control(m, p)?)
}

// ------------------------------------------------------------------ the catalogue

static TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "list_commands",
        description: "List engine commands: id, label, menu path, shortcut, `params` doc (e.g. `{name?, width?, height?, frameRate?, duration? (s)}`) and whether it can run now (`why` if not). Every menu item, shortcut and panel gesture maps to one of these ids; run them with execute_command. Narrow with `filter` (e.g. `layer.new`, `keys`, `effect`).",
        bridge_only: false,
        schema: || {
            schema(
                json!({
                    "filter": {"type": "string", "description": "Only commands whose id or label contains this text (case-insensitive)."},
                    "enabled_only": {"type": "boolean", "description": "Only commands that can run right now."},
                    "schemas": {"type": "boolean", "description": "Also return each command's parameter JSON Schema (`schema`)."}
                }),
                &[],
            )
        },
        run: list_commands,
    },
    ToolDef {
        name: "describe_command",
        description: "One command in full: id, label, menu path, shortcut, the `params` doc, a JSON Schema of its parameters (`schema`: properties, required, additionalProperties) and whether it can run now (`why` if not). Use before execute_command when unsure of the parameters; execute_command rejects unknown keys.",
        bridge_only: false,
        schema: || {
            schema(
                json!({"command": {"type": "string", "description": "Command id, e.g. `layer.newText`."}, "id": {"type": "string", "description": "Alias of `command`."}}),
                &[],
            )
        },
        run: describe_command,
    },
    ToolDef {
        name: "execute_command",
        description: "Run any engine command by id with JSON params; edits are undoable. Examples: comp.new {\"name\":\"Main\",\"width\":1920,\"height\":1080,\"frameRate\":30,\"duration\":5}; layer.newText {\"text\":\"Hello\",\"size\":120,\"fill\":\"#ffffff\"} (returns {\"layer\":id}); layer.newSolid {\"color\":\"#3366ff\"}; layer.newShape; effect.apply {\"layer\":3,\"effect\":\"Gaussian Blur\"}; time.set {\"time\":2.5}; layer.select {\"layers\":[3]}. Layers are referenced by id, \"#n\" or name; times are seconds.",
        bridge_only: false,
        schema: || {
            schema(
                json!({
                    "command": {"type": "string", "description": "Command id, e.g. `comp.new`, `layer.newText`, `effect.apply` (see list_commands)."},
                    "id": {"type": "string", "description": "Alias of `command`."},
                    "params": {"type": "object", "description": "Command parameters as documented by list_commands.", "additionalProperties": true}
                }),
                &[],
            )
        },
        run: execute_command,
    },
    ToolDef {
        name: "run_script",
        description: "Run JavaScript with an After Effects-style scripting object model (app, app.project, items.addComp, comp.layers.addSolid/addText/addShape/addNull/addCamera/addLight, layer.property(\"ADBE Transform Group\").property(\"ADBE Position\").setValueAtTime(t, v), Effects.addProperty(\"ADBE Gaussian Blur 2\"), expressions, markers, renderQueue…). Every edit is an undoable engine command; app.beginUndoGroup/endUndoGroup make one undo step. Returns {ok, result (last expression value), output (writeLn/$.writeln/alert text), error: {message, line, column} | null}. File access is limited to the project folder unless the user allows scripts to write files.",
        bridge_only: false,
        schema: || {
            schema(
                json!({
                    "code": {"type": "string", "description": "JavaScript source, e.g. `var c = app.project.items.addComp(\"Main\", 1920, 1080, 1, 5, 30); c.layers.addText(\"Hi\"); c.numLayers`."},
                    "name": {"type": "string", "description": "Script name used in error messages (default `script`)."}
                }),
                &["code"],
            )
        },
        run: run_script,
    },
    ToolDef {
        name: "get_project",
        description: "Project overview: file path, dirty flag, bit depth, every item (id, name, type Composition/Footage/Solid/Folder, size, duration, layer count), the active comp id, current time (s), selected layers and the undo stack.",
        bridge_only: false,
        schema: || schema(json!({}), &[]),
        run: get_project,
    },
    ToolDef {
        name: "get_comp",
        description: "A composition: id, name, size, frame rate, duration, work area, background and its layers top to bottom (id, index, name, type, in/out/start seconds, blend mode, parent, track matte, switches). Layer ids feed get_layer, get_property/set_property and commands.",
        bridge_only: false,
        schema: || schema(json!({"comp": comp_s()}), &[]),
        run: get_comp,
    },
    ToolDef {
        name: "get_layer",
        description: "A layer's property tree (transform, masks, effects, text, shape contents, ...): every node has a `path` usable with get_property/set_property/add_keyframe, plus uid, match id, name, type, value at `time`, keyframes and expression. `flat: true` returns just a list of {path, type, value, keys, expression} (compact).",
        bridge_only: false,
        schema: || {
            schema(
                json!({
                    "layer": layer_s(), "comp": comp_s(),
                    "time": {"type": "number", "description": "Comp time in seconds for values (default: current time)."},
                    "depth": {"type": "integer", "description": "Group depth to expand (default all)."},
                    "flat": {"type": "boolean", "description": "Return a flat list of properties instead of the tree."}
                }),
                &["layer"],
            )
        },
        run: get_layer,
    },
    ToolDef {
        name: "get_property",
        description: "Read one property: type, value at `time` (comp seconds; default the current time), whether it is animated, its keyframes (time, value, in/out interpolation) and expression.",
        bridge_only: false,
        schema: || {
            schema(
                json!({"layer": layer_s(), "path": path_s(), "comp": comp_s(), "time": {"type": "number", "description": "Comp time in seconds."}}),
                &["layer", "path"],
            )
        },
        run: get_property,
    },
    ToolDef {
        name: "set_property",
        description: "Set a property. Without `time`: sets the static value (or a key at the current time if already animated). With `time`: adds/replaces a keyframe there, turning the stopwatch on. `expression` sets an After Effects-style JS expression (\"\" removes it). Returns the property as get_property does. Examples: {\"layer\":3,\"path\":\"transform/opacity\",\"value\":50}; {\"layer\":\"Title\",\"path\":\"transform/position\",\"value\":[200,540],\"time\":0}; {\"layer\":3,\"path\":\"transform/rotation\",\"expression\":\"time*90\"}.",
        bridge_only: false,
        schema: || {
            schema(
                json!({
                    "layer": layer_s(), "path": path_s(), "value": value_s(), "comp": comp_s(),
                    "time": {"type": "number", "description": "Keyframe time in seconds (layer time; equals comp time unless the layer is offset/stretched)."},
                    "expression": {"type": "string", "description": "Expression text; empty string removes the expression."}
                }),
                &["layer", "path"],
            )
        },
        run: set_property,
    },
    ToolDef {
        name: "add_keyframe",
        description: "Add keyframes to a property (turns the stopwatch on): one with `time` (+ optional `value`, default the current value) or many with `keys: [{time, value}]`, then optionally set their `interpolation`: linear, bezier, hold, easyEase, easyEaseIn, easyEaseOut. Example: {\"layer\":1,\"path\":\"transform/position\",\"keys\":[{\"time\":0,\"value\":[0,540]},{\"time\":2,\"value\":[1920,540]}],\"interpolation\":\"easyEase\"}.",
        bridge_only: false,
        schema: || {
            schema(
                json!({
                    "layer": layer_s(), "path": path_s(), "comp": comp_s(),
                    "time": {"type": "number", "description": "Key time in seconds (layer time)."},
                    "value": value_s(),
                    "keys": {"type": "array", "items": {"type": "object", "properties": {"time": {"type": "number"}, "value": value_s()}, "required": ["time"]}, "description": "Several keys at once."},
                    "interpolation": {"type": "string", "enum": ["linear", "bezier", "hold", "easyEase", "easyEaseIn", "easyEaseOut"]}
                }),
                &["layer", "path"],
            )
        },
        run: add_keyframe,
    },
    ToolDef {
        name: "add_effect",
        description: "Apply an effect to a layer and optionally set its parameters in one undoable call. `effect` is an id (`ec.blur.gaussian`) or After Effects name (`Gaussian Blur`; see list_effects); `values` maps parameter ids to values, e.g. {\"blurriness\": 12}. One undo step; if a value fails, nothing is applied. Returns the instance `path` (`effects/#n`, for set_property / add_keyframe) and its parameters with their paths and current values.",
        bridge_only: false,
        schema: || {
            schema(
                json!({
                    "layer": layer_s(), "comp": comp_s(),
                    "effect": {"type": "string", "description": "Effect id or name, e.g. `Gaussian Blur`, `Glow`, `Curves`, `ec.color.levels`."},
                    "values": {"type": "object", "description": "Parameter id -> value (ids are in the returned `params` paths), e.g. {\"blurriness\": 12}.", "additionalProperties": true}
                }),
                &["layer", "effect"],
            )
        },
        run: add_effect,
    },
    ToolDef {
        name: "list_effects",
        description: "The effect registry: id, After Effects name, category, GPU and 32-bpc support. Narrow with `filter` (id, name or category text). Apply with add_effect.",
        bridge_only: false,
        schema: || schema(json!({"filter": {"type": "string", "description": "Only effects whose id, name or category contains this text."}}), &[]),
        run: list_effects,
    },
    ToolDef {
        name: "list_fonts",
        description: "The font families text layers can use, bundled and installed on this machine: family, styles, origin (bundled / system / user) and the name in the font's own language. Narrow with `query`; `rescan` picks up fonts installed since launch. Set one with execute_command `layer.setText {\"layer\", \"font\", \"style\"}`.",
        bridge_only: false,
        schema: || {
            schema(
                json!({
                    "query": {"type": "string", "description": "Only families whose name (English or own-language) contains this text."},
                    "rescan": {"type": "boolean", "description": "Look for fonts installed since launch first."}
                }),
                &[],
            )
        },
        run: list_fonts,
    },
    ToolDef {
        name: "get_state",
        description: "Where things stand: the editor state (active comp, current time, selected layers/keys/items, open comps, tool, work area) plus what this build can do (`app`: version, command and effect counts, GPU effects, import/export formats, After Effects parity summary). Cheap; call it to re-orient after a series of edits.",
        bridge_only: false,
        schema: || schema(json!({}), &[]),
        run: get_state,
    },
    ToolDef {
        name: "render_frame",
        description: "Render a composition frame (default: the active comp at the current time) and return it as a PNG image, longest side `max_side` (default 960, 0 = full size). `path` also writes the PNG to disk; `inline: false` returns only the metadata. Use it to check your work.",
        bridge_only: false,
        schema: || {
            schema(
                json!({
                    "comp": comp_s(),
                    "time": {"type": "number", "description": "Comp time in seconds (default: current time)."},
                    "max_side": {"type": "integer", "description": "Longest side in pixels (default 960; 0 = full comp size)."},
                    "path": {"type": "string", "description": "Also write the PNG here."},
                    "inline": {"type": "boolean", "description": "Include the image in the reply (default true)."},
                    "transparent": {"type": "boolean", "description": "Keep the frame's alpha (RGBA PNG, as an RGB + Alpha render writes it) instead of compositing over the comp's background colour."}
                }),
                &[],
            )
        },
        run: render_frame,
    },
    ToolDef {
        name: "open_project",
        description: "Open a project: `path` to an .ecproj file, `demo: true` for the built-in demo project, or `new: true` for an empty project. Returns the project overview.",
        bridge_only: false,
        schema: || {
            schema(json!({"path": {"type": "string", "description": "Path of an .ecproj file."}, "demo": {"type": "boolean"}, "new": {"type": "boolean"}}), &[])
        },
        run: open_project,
    },
    ToolDef {
        name: "save_project",
        description: "Save the project to its current file, or to `path` (Save As, .ecproj).",
        bridge_only: false,
        schema: || schema(json!({"path": {"type": "string", "description": "Destination .ecproj path (Save As)."}}), &[]),
        run: save_project,
    },
    ToolDef {
        name: "undo",
        description: "Undo the last `steps` edits (default 1). Returns the remaining undo stack.",
        bridge_only: false,
        schema: || schema(json!({"steps": {"type": "integer", "minimum": 1}}), &[]),
        run: undo,
    },
    ToolDef {
        name: "redo",
        description: "Redo `steps` undone edits (default 1).",
        bridge_only: false,
        schema: || schema(json!({"steps": {"type": "integer", "minimum": 1}}), &[]),
        run: redo,
    },
    ToolDef {
        name: "history",
        description: "The branching undo history (History panel): without arguments lists every state {index, id, label, parent, depth (0 = working line, >0 = branch), current, future}; undoing and then editing keeps the undone states as a branch. With `goto` (an index or id from the listing) jumps to that state, on any branch, without losing the others.",
        bridge_only: false,
        schema: || schema(json!({"goto": {"type": ["integer", "string"], "description": "State index or id to jump to (edit.history.goto)."}}), &[]),
        run: history_tool,
    },
    ToolDef {
        name: "script_ui",
        description: "Drive ScriptUI windows that scripts opened (dialogs, palettes, dockable panels). action `list` (default): open windows; `get`: a window's control tree (type, name, text, value, checked, items, selection, bounds); `click`: press a button / toggle a checkbox / pick a radio button; `set`: type text, move a slider, pick a list item (index or text); `close`: close a window (`result` for a dialog's show(), default 2 = Cancel). Controls by id, \"#id\", properties.name or text. A dialog's script resumes when the dialog closes.",
        bridge_only: false,
        schema: || {
            schema(
                json!({
                    "action": {"type": "string", "enum": ["list", "get", "click", "set", "close"]},
                    "window": {"type": ["integer", "string"], "description": "Window id or title (optional when one is open)."},
                    "widget": {"type": ["integer", "string"], "description": "Control id, \"#id\", properties.name or text."},
                    "value": {"description": "For `set`: text, number, bool, item index or item text."},
                    "result": {"type": "integer", "description": "For `close`: what the dialog's show() returns."}
                }),
                &[],
            )
        },
        run: script_ui_tool,
    },
    ToolDef {
        name: "screenshot",
        description: "Screenshot of the live app window, or one panel (`Composition`, `Timeline`, `EffectControls`, `Project`, ...) or element id, as PNG (longest side `max_side`, default 1600). Use it to check what the UI looks like after an action.",
        bridge_only: true,
        schema: || {
            schema(
                json!({
                    "panel": {"type": "string", "description": "Panel to crop to."},
                    "id": {"type": "string", "description": "Element id (from ui_elements) to crop to."},
                    "max_side": {"type": "integer"},
                    "path": {"type": "string", "description": "Keep the full-size PNG at this path."}
                }),
                &[],
            )
        },
        run: screenshot,
    },
    ToolDef {
        name: "ui_inspect",
        description: "UI state: window size, tool, workspace, panels, viewer and timeline view state, playback, active comp, selection, open dialog, render time.",
        bridge_only: true,
        schema: || schema(json!({}), &[]),
        run: ui_inspect,
    },
    ToolDef {
        name: "ui_elements",
        description: "Interactive elements drawn in the last frame: id, label, rect [x,y,w,h] in points. Stable ids such as `tools.Selection`, `panel.Timeline`, `panel.tab.EffectControls`, `project.item.<id>`, `viewer.comp`, `timeline.layer.<id>.bar`, `timeline.prop.<uid>.stopwatch`, `timeline.cti`, `effects.item.<name>`. Filter with `prefix`.",
        bridge_only: true,
        schema: || schema(json!({"prefix": {"type": "string", "description": "Only ids starting with this, e.g. `tools.`, `timeline.layer.`."}}), &[]),
        run: ui_elements,
    },
    ToolDef {
        name: "ui_click",
        description: "Click an element by `id` (from ui_elements) or at `x`,`y` (points). `button`: left|right|middle, `count`: 2 for double-click, `modifiers`: {shift, alt, command, ctrl}.",
        bridge_only: true,
        schema: || {
            schema(
                json!({
                    "id": {"type": "string"}, "x": {"type": "number"}, "y": {"type": "number"},
                    "button": {"type": "string", "enum": ["left", "right", "middle"]},
                    "count": {"type": "integer"}, "modifiers": {"type": "object"}
                }),
                &[],
            )
        },
        run: ui_click,
    },
    ToolDef {
        name: "ui_drag",
        description: "Press-move-release from `from` to `to`; each is {\"id\": element} (add fx/fy 0-1 for a spot inside it) or {\"x\",\"y\"}. Moves layers in the viewer, drags layer bars/keyframes/the CTI (`timeline.cti`), scrubs values, draws shapes with a shape tool.",
        bridge_only: true,
        schema: || {
            schema(
                json!({"from": {"type": "object"}, "to": {"type": "object"}, "steps": {"type": "integer"}, "modifiers": {"type": "object"}}),
                &["from", "to"],
            )
        },
        run: ui_drag,
    },
    ToolDef {
        name: "ui_key",
        description: "Press a key or shortcut: `Space` (play), `F9` (Easy Ease), `Cmd+Z`, `Cmd+N`, `PageDown`, `Escape`, `Enter` (`Cmd` = Ctrl off macOS).",
        bridge_only: true,
        schema: || schema(json!({"key": {"type": "string"}}), &["key"]),
        run: ui_key,
    },
    ToolDef {
        name: "ui_type",
        description: "Type text into the focused field (text layer editing, search boxes, value fields).",
        bridge_only: true,
        schema: || schema(json!({"text": {"type": "string"}}), &["text"]),
        run: ui_type,
    },
    ToolDef {
        name: "ui_set",
        description: "Set UI state directly: tool, workspace, theme (dark|darker|light|studioDark|studioLight|classic), focused panel, viewer {zoom, res, pan, grid, rulers, safeMargins, transparencyGrid}, timeline {pps, start, graphEditor, showModes, openLayers, openGroups}, menuBar, home.",
        bridge_only: true,
        schema: || {
            schema(
                json!({
                    "tool": {"type": "string"}, "workspace": {"type": "string"}, "theme": {"type": "string"}, "focused": {"type": "string"},
                    "viewer": {"type": "object"}, "timeline": {"type": "object"}, "menuBar": {"type": "boolean"}, "home": {"type": "boolean"}
                }),
                &[],
            )
        },
        run: ui_set,
    },
    ToolDef {
        name: "control",
        description: "Call any control-channel method (docs/control-protocol.md) with its params, e.g. ui.menu.list, ui.menu.invoke {id}, ui.panel.show {panel}, ui.scroll, ui.viewer.locate {x,y}, ui.timeline.locate {layer,time}, ui.playback {action}, ui.resize.",
        bridge_only: true,
        schema: || schema(json!({"method": {"type": "string"}, "params": {"type": "object", "additionalProperties": true}}), &["method"]),
        run: control,
    },
    ToolDef {
        name: "batch",
        description: "Run several engine commands in one call as ONE undo step (engine.batch). Later steps can use earlier results: a string param \"$N\" or \"$N.key\" is step N's result (1-based), e.g. create a layer, then key and style it: {\"steps\":[{\"command\":\"layer.newText\",\"params\":{\"text\":\"Hi\",\"name\":\"Title\"}},{\"command\":\"prop.addKey\",\"params\":{\"layer\":\"$1.layer\",\"path\":\"transform/opacity\",\"time\":0,\"value\":0}},{\"command\":\"effect.apply\",\"params\":{\"layer\":\"$1.layer\",\"effect\":\"Glow\"}}]}. Returns {steps, results}. If a step fails the whole batch is rolled back (unless atomic: false) and the error names the step.",
        bridge_only: false,
        schema: || {
            schema(
                json!({
                    "steps": {"type": "array", "items": {"type": "object", "properties": {"command": {"type": "string"}, "params": {"type": "object", "additionalProperties": true}}, "required": ["command"]}},
                    "label": {"type": "string", "description": "Undo step name (default Batch)."},
                    "atomic": {"type": "boolean", "description": "Roll everything back when a step fails (default true)."}
                }),
                &["steps"],
            )
        },
        run: batch_tool,
    },
    ToolDef {
        name: "command_list",
        description: "List engine commands: id, label, menu path, shortcut, `params` doc (e.g. `{name?, width?, height?, frameRate?, duration? (s)}`) and whether it can run now (`why` if not). Every menu item, shortcut and panel gesture maps to one of these ids; run them with execute_command. Narrow with `filter` (e.g. `layer.new`, `keys`, `effect`).",
        bridge_only: false,
        schema: || {
            schema(
                json!({
                    "filter": {"type": "string", "description": "Only commands whose id or label contains this text (case-insensitive)."},
                    "enabled_only": {"type": "boolean", "description": "Only commands that can run right now."},
                    "schemas": {"type": "boolean", "description": "Also return each command's parameter JSON Schema (`schema`)."}
                }),
                &[],
            )
        },
        run: list_commands,
    },
    ToolDef {
        name: "command_run",
        description: "Run any engine command by id with JSON params; edits are undoable. Examples: comp.new {\"name\":\"Main\",\"width\":1920,\"height\":1080,\"frameRate\":30,\"duration\":5}; layer.newText {\"text\":\"Hello\",\"size\":120,\"fill\":\"#ffffff\"} (returns {\"layer\":id}); layer.newSolid {\"color\":\"#3366ff\"}; layer.newShape; effect.apply {\"layer\":3,\"effect\":\"Gaussian Blur\"}; time.set {\"time\":2.5}; layer.select {\"layers\":[3]}. Layers are referenced by id, \"#n\" or name; times are seconds.",
        bridge_only: false,
        schema: || {
            schema(
                json!({
                    "id": {"type": "string", "description": "Command id, e.g. `comp.new`, `layer.newText`, `effect.apply` (see list_commands)."},
                    "params": {"type": "object", "description": "Command parameters as documented by list_commands.", "additionalProperties": true}
                }),
                &["id"],
            )
        },
        run: execute_command,
    },
    ToolDef {
        name: "render_preview",
        description: "Render a composition frame (default: the active comp at the current time) and return it as a PNG image, longest side `max_side` (default 960, 0 = full size). Use it to check your work.",
        bridge_only: false,
        schema: || {
            schema(
                json!({
                    "comp": comp_s(),
                    "time": {"type": "number", "description": "Comp time in seconds (default: current time)."},
                    "max_side": {"type": "integer", "description": "Longest side in pixels (default 960; 0 = full comp size)."},
                    "transparent": {"type": "boolean", "description": "Keep the frame's alpha (RGBA PNG, as an RGB + Alpha render writes it) instead of compositing over the comp's background colour."}
                }),
                &[],
            )
        },
        run: render_frame,
    },
    ToolDef {
        name: "command_batch",
        description: "Run commands in order, returning each result or error. Stop at the first error by default; stop_on_error: false continues. Each edit has its own undo step; use batch for an atomic undo group.",
        bridge_only: false,
        schema: || {
            schema(
                json!({"steps": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}, "params": {"type": "object"}}, "required": ["id"], "additionalProperties": false}}, "stop_on_error": {"type": "boolean"}}),
                &["steps"],
            )
        },
        run: command_batch,
    },
    ToolDef {
        name: "doc_inspect",
        description: "Inspect the project overview and active composition (null when no composition is active).",
        bridge_only: false,
        schema: || schema(json!({}), &[]),
        run: doc_inspect,
    },
];

fn doc_inspect(b: &mut Backend, _: &Value) -> Result<Reply> {
    let project = b.exec("project.summary", json!({}))?;
    let comp = b.exec("comp.info", json!({})).ok();
    json_reply(json!({"project": project, "activeComp": comp}))
}

fn command_batch(b: &mut Backend, a: &Value) -> Result<Reply> {
    let steps = need(a, "steps")?.as_array().ok_or_else(|| Error::BadArgs("steps must be an array".into()))?;
    let stop = get(a, "stop_on_error").and_then(Value::as_bool).unwrap_or(true);
    let mut results = Vec::new();
    let (mut completed, mut failed) = (0, 0);
    for step in steps {
        match execute_command(b, step) {
            Ok(Reply::Json(result)) => {
                completed += 1;
                results.push(json!({"ok": true, "result": result}));
            }
            Ok(Reply::Image { .. }) => return Err(Error::Other("unexpected command image".into())),
            Err(e) => {
                failed += 1;
                results.push(json!({"ok": false, "error": e.to_string()}));
                if stop {
                    break;
                }
            }
        }
    }
    json_reply(json!({"completed": completed, "failed": failed, "results": results}))
}
