//! MCP server: JSON-RPC 2.0, one message per line (the MCP stdio transport). Implements
//! tools and JSON resources. The stdio loop handles progress and cancellation for long renders.

use std::io::{BufRead, Write};
use std::path::Path;

use serde_json::{Value, json};

use crate::autosave::AutoSave;
use crate::tools::{self, Reply};
use crate::{Backend, Error, base64};

/// Protocol revisions we speak, newest first. We answer with the client's if we know it.
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Visualab Effects is an After Effects-class motion graphics compositor. Everything is an engine command: command_list discovers ids and params, command_run {id, params} runs them (undoable; command_batch runs several in order). Typical flow: command_run comp.new -> command_run layer.newText / layer.newSolid (returns the layer id) -> set_property / add_keyframe -> command_run effect.apply -> render_preview to look at the result. Inspect with doc_inspect, get_project, get_comp and get_layer (every property node carries its `path`, e.g. `transform/position`, `effects/#1/blurriness`). Times are seconds. In bridge mode (app started with `--control <port>`) screenshot and the ui_* tools show and operate the live window.";

// JSON-RPC error codes.
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;
const RESOURCE_NOT_FOUND: i64 = -32002;

pub struct McpServer {
    backend: Backend,
    autosave: Option<AutoSave>,
}

impl McpServer {
    pub fn new(backend: Backend) -> Self {
        Self { backend, autosave: None }
    }

    /// Opt in to durable headless checkpoints in a separate folder per server.
    /// `root` is the platform config directory; settings are read by the caller.
    pub fn with_autosave(mut self, root: &Path) -> crate::Result<Self> {
        let s = self.backend.session().ok_or_else(|| crate::Error::BadArgs("--autosave is for headless MCP; the bridged app owns its auto-saves".into()))?;
        s.autosave.background = false;
        let autosave = AutoSave::new(root, &s.prefs).map_err(|e| crate::Error::Other(format!("MCP auto-save: {e}")))?;
        s.autosave_folder_override = Some(autosave.folder().to_path_buf());
        self.autosave = Some(autosave);
        Ok(self)
    }

    fn autosave_info(&self) -> Value {
        self.autosave.as_ref().map(AutoSave::info).unwrap_or_else(|| json!({"enabled": false}))
    }

    fn checkpoint(&mut self, finish: bool) -> std::io::Result<()> {
        match (&mut self.autosave, self.backend.session()) {
            (Some(a), Some(s)) => a.checkpoint(s, finish),
            _ => Ok(()),
        }
    }

    pub fn backend(&mut self) -> &mut Backend {
        &mut self.backend
    }

    /// Handle one decoded message (request, notification or batch); `None` when no reply is due.
    pub fn handle(&mut self, msg: &Value) -> Option<Value> {
        if let Value::Array(batch) = msg {
            if batch.is_empty() {
                return Some(error(Value::Null, INVALID_REQUEST, "empty batch"));
            }
            let out: Vec<Value> = batch.iter().filter_map(|m| self.handle(m)).collect();
            return (!out.is_empty()).then_some(Value::Array(out));
        }
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            // A response from the client (we send no requests) or garbage.
            return msg
                .get("id")
                .filter(|_| msg.get("result").is_none() && msg.get("error").is_none())
                .map(|id| error(id.clone(), INVALID_REQUEST, "missing `method`"));
        };
        let Some(id) = msg.get("id").cloned() else {
            return None; // notification (notifications/initialized, notifications/cancelled, ...)
        };
        let params = msg.get("params").cloned().unwrap_or(json!({}));
        Some(match self.request(method, &params) {
            Ok(mut result) => {
                modern_result_fields(method, &params, &mut result);
                json!({"jsonrpc": "2.0", "id": id, "result": result})
            }
            Err((code, m)) => error(id, code, &m),
        })
    }

    /// Handle one line of input; `None` when no reply is due.
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let reply = match serde_json::from_str::<Value>(line) {
            Ok(msg) => self.handle(&msg)?,
            Err(e) => error(Value::Null, PARSE_ERROR, &format!("parse error: {e}")),
        };
        Some(reply.to_string())
    }

    /// Serve until EOF on `input`. Input is read on its own thread so a long render can report
    /// progress, be cancelled and let other requests through while it runs (`long_job`). The
    /// reader thread is detached: when writing a reply fails, the server returns at once instead
    /// of waiting for a read that may never complete.
    pub fn serve(&mut self, input: impl BufRead + Send + 'static, mut output: impl Write) -> std::io::Result<()> {
        let (tx, inbox) = std::sync::mpsc::channel::<Option<String>>();
        // A read error ends the input like EOF and is returned once the queued requests are answered.
        let read_error = std::sync::Arc::new(std::sync::Mutex::new(None::<std::io::Error>));
        let reader_error = read_error.clone();
        std::thread::Builder::new().name("mcp-input".into()).spawn(move || {
            for line in input.lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(e) => {
                        *reader_error.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(e);
                        break;
                    }
                };
                if tx.send(Some(line)).is_err() {
                    return;
                }
            }
            let _ = tx.send(None);
        })?;
        let served = (|| {
            while let Ok(Some(line)) = inbox.recv() {
                let long = serde_json::from_str::<Value>(line.trim()).ok().and_then(|m| crate::long_job::long_call(self, &m));
                if let Some(call) = long {
                    if !self.run_long(call, &inbox, &mut output)? {
                        break;
                    }
                    continue;
                }
                if let Some(reply) = self.handle_line(&line) {
                    output.write_all(reply.as_bytes())?;
                    output.write_all(b"\n")?;
                    output.flush()?;
                }
            }
            match read_error.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
                Some(e) => Err(e),
                None => Ok(()),
            }
        })();
        match (served, self.checkpoint(true)) {
            (r, Ok(())) => r,
            (Ok(()), Err(e)) => Err(e),
            (Err(transport), Err(save)) => Err(std::io::Error::other(format!("{transport}; {save}"))),
        }
    }

    /// Serve on stdin/stdout (the MCP stdio transport).
    pub fn serve_stdio(&mut self) -> std::io::Result<()> {
        self.serve(std::io::BufReader::new(std::io::stdin()), std::io::stdout().lock())
    }

    fn request(&mut self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
        match method {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
                let version = PROTOCOL_VERSIONS.iter().find(|v| **v == asked).copied().unwrap_or(PROTOCOL_VERSIONS.first().copied().unwrap_or("2025-06-18"));
                let mode = if self.backend.is_bridge() { "bridge" } else { "headless" };
                let instructions = if self.autosave.is_some() {
                    format!("{INSTRUCTIONS} Headless auto-save and recovery: {}", self.autosave_info())
                } else {
                    INSTRUCTIONS.to_string()
                };
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}, "resources": {"listChanged": false, "subscribe": false}},
                    "serverInfo": {"name": "effectcraft", "title": format!("Visualab Effects ({mode})"), "version": env!("CARGO_PKG_VERSION")},
                    "instructions": instructions,
                    "_meta": {"effectcraftAutoSave": self.autosave_info()},
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => {
                let tools: Vec<Value> = tools::available(self.backend.is_bridge()).map(|t| t.descriptor()).collect();
                Ok(json!({"tools": tools}))
            }
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).ok_or((INVALID_PARAMS, "missing tool `name`".to_string()))?;
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                if tools::available(self.backend.is_bridge()).all(|t| t.name != name) {
                    let hint = if tools::find(name).is_some() { format!(" ({})", crate::backend::NEED_BRIDGE) } else { String::new() };
                    return Err((INVALID_PARAMS, format!("unknown tool `{name}`{hint}")));
                }
                if let Some(m) = tools::find(name).and_then(|t| t.unknown_arg(&args)) {
                    return Err((INVALID_PARAMS, m));
                }
                let mut result = call_result(self.guarded(name, &args));
                // Also checkpoint a partially successful tool that returned an error. The
                // project is durable before the client receives its reply (even if it kills
                // the process immediately afterwards).
                if let Err(e) = self.checkpoint(false) {
                    let _ = writeln!(std::io::stderr(), "{e}");
                    if let Some(content) = result.get_mut("content").and_then(Value::as_array_mut) {
                        content.push(json!({"type": "text", "text": format!("Warning: {e}. Use save_project with a writable path before disconnecting.")}));
                    }
                }
                if self.autosave.is_some()
                    && let Some(obj) = result.as_object_mut()
                {
                    let meta = obj.entry("_meta").or_insert_with(|| json!({}));
                    if let Some(meta) = meta.as_object_mut() {
                        meta.insert("effectcraftAutoSave".into(), self.autosave_info());
                    }
                }
                if self.autosave.is_some()
                    && name == "get_project"
                    && result["isError"] == false
                    && let Some(text) = result.get_mut("content").and_then(Value::as_array_mut).and_then(|c| c.first_mut()).and_then(|c| c.get_mut("text"))
                    && let Some(summary) = text.as_str().and_then(|t| serde_json::from_str::<Value>(t).ok())
                {
                    let mut summary = summary;
                    summary["autosave"] = self.autosave_info();
                    *text = Value::String(summary.to_string());
                }
                Ok(result)
            }
            "resources/list" => Ok(json!({"resources": [
                {"uri": DOCUMENT_URI, "name": "document", "title": "Project", "description": "The project overview and the active composition (same as doc_inspect).", "mimeType": "application/json"},
                {"uri": COMMANDS_URI, "name": "commands", "title": "Command catalog", "description": "Every engine command with id, label, menu, shortcut and params (same as command_list).", "mimeType": "application/json"},
            ]})),
            "resources/templates/list" => Ok(json!({"resourceTemplates": []})),
            "resources/read" => {
                let uri = params.get("uri").and_then(Value::as_str).ok_or((INVALID_PARAMS, "missing `uri`".to_string()))?;
                let tool = match uri {
                    DOCUMENT_URI => "doc_inspect",
                    COMMANDS_URI => "command_list",
                    _ => return Err((RESOURCE_NOT_FOUND, format!("resource not found: {uri}"))),
                };
                match self.guarded(tool, &json!({})) {
                    Ok(Reply::Json(v)) => Ok(json!({"contents": [{"uri": uri, "mimeType": "application/json", "text": v.to_string()}]})),
                    Ok(Reply::Image { .. }) => Err((INTERNAL_ERROR, "unexpected image".into())),
                    Err(e) => Err((INTERNAL_ERROR, e.to_string())),
                }
            }
            "prompts/list" => Ok(json!({"prompts": []})),
            m => Err((METHOD_NOT_FOUND, format!("method not found: {m}"))),
        }
    }
}

/// A `tools/call` result: tool failures are reported in-band (`isError`) so the model can react.
pub fn call_result(r: Result<Reply, Error>) -> Value {
    match r {
        Ok(Reply::Json(v)) => {
            json!({"content": [{"type": "text", "text": v.to_string()}], "isError": v.get("failed").and_then(Value::as_u64).is_some_and(|n| n > 0)})
        }
        Ok(Reply::Image { png, info }) => json!({
            "content": [
                {"type": "image", "data": base64::encode(&png), "mimeType": "image/png"},
                {"type": "text", "text": info.to_string()},
            ],
            "isError": false,
        }),
        Err(e) => json!({"content": [{"type": "text", "text": e.to_string()}], "isError": true}),
    }
}

impl McpServer {
    /// Run a tool with panics caught: a panic becomes an error result and the session keeps
    /// serving (docs/mcp.md).
    pub(crate) fn guarded(&mut self, name: &str, args: &Value) -> Result<Reply, Error> {
        let backend = &mut self.backend;
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tools::run(backend, name, args))).unwrap_or_else(|p| {
            let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".into());
            Err(Error::Other(format!("internal error: {msg}")))
        })
    }
}

/// The craft catalog resources (docs/mcp.md).
const DOCUMENT_URI: &str = "effectcraft://document";
const COMMANDS_URI: &str = "effectcraft://commands";

/// Whether a request declares MCP 2026-07-28 or later in its `_meta` (a "modern" client, which
/// negotiates per request instead of through `initialize`).
pub(crate) fn is_modern(params: &Value) -> bool {
    params.pointer("/_meta/io.modelcontextprotocol~1protocolVersion").and_then(Value::as_str).is_some_and(|v| v >= "2026-07-28")
}

/// Modern clients reject list/read results without `resultType`, `ttlMs` and `cacheScope`; those
/// fields are added for them. Sessions that negotiated an older revision through `initialize` keep
/// the legacy shape. The tool and resource lists never change while the server runs (cacheable
/// for ten minutes); reads follow the project, so they are never cached.
pub(crate) fn modern_result_fields(method: &str, params: &Value, result: &mut Value) {
    let ttl_ms = match method {
        "tools/list" | "resources/list" | "resources/templates/list" | "prompts/list" => Some(600_000),
        "resources/read" => Some(0),
        "tools/call" => None,
        _ => return,
    };
    let Some(obj) = result.as_object_mut().filter(|_| is_modern(params)) else { return };
    obj.insert("resultType".into(), json!("complete"));
    if let Some(ttl_ms) = ttl_ms {
        obj.insert("ttlMs".into(), json!(ttl_ms));
        obj.insert("cacheScope".into(), json!("private"));
    }
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}
