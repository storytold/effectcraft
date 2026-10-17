//! MCP resources (docs/mcp.md "Resources"): the fixed catalog resources and the URI templates
//! `effectcraft://comp/{comp}`, `effectcraft://layer/{comp}/{layer}`,
//! `effectcraft://frame/{comp}/{time}` and `effectcraft://command/{id}`. Path segments are
//! percent-decoded after splitting, so a name containing `/` is written `%2F`.

use serde_json::{Value, json};

/// The project overview and active composition (same as doc_inspect).
pub const DOCUMENT_URI: &str = "effectcraft://document";
/// The command catalog (same as command_list).
pub const COMMANDS_URI: &str = "effectcraft://commands";

const SCHEME: &str = "effectcraft://";
/// Default and maximum longest side of a frame resource.
const FRAME_MAX_SIDE: u32 = 960;
const FRAME_MAX_SIDE_LIMIT: u32 = 8192;

/// What a resource URI names.
#[derive(Debug, PartialEq)]
pub enum Target {
    Document,
    Commands,
    Comp(Value),
    Layer { comp: Value, layer: Value, flat: bool },
    Frame { comp: Value, time: f64, max_side: u32 },
    Command(String),
}

/// Why a URI can't be read.
#[derive(Debug, PartialEq)]
pub enum UriError {
    /// Not one of ours (JSON-RPC -32002).
    NotFound(String),
    /// One of ours, malformed (JSON-RPC -32602).
    Invalid(String),
}

impl Target {
    /// The tool that reads it and its arguments.
    pub fn tool(&self) -> (&'static str, Value) {
        match self {
            Target::Document => ("doc_inspect", json!({})),
            Target::Commands => ("command_list", json!({})),
            Target::Comp(c) => ("get_comp", json!({"comp": c})),
            Target::Layer { comp, layer, flat } => ("get_layer", json!({"comp": comp, "layer": layer, "flat": flat})),
            Target::Frame { comp, time, max_side } => ("render_preview", json!({"comp": comp, "time": time, "max_side": max_side})),
            Target::Command(id) => ("describe_command", json!({"command": id})),
        }
    }

    /// Whether the content follows the project (subscribers are told when it changes).
    pub fn follows_project(&self) -> bool {
        !matches!(self, Target::Commands | Target::Command(_))
    }
}

/// `resources/list`.
pub fn list() -> Value {
    json!([
        {"uri": DOCUMENT_URI, "name": "document", "title": "Project", "description": "The project overview and the active composition (same as doc_inspect).", "mimeType": "application/json"},
        {"uri": COMMANDS_URI, "name": "commands", "title": "Command catalog", "description": "Every engine command with id, label, menu, shortcut and params (same as command_list).", "mimeType": "application/json"},
    ])
}

/// `resources/templates/list`; `titles` for clients that know `title` (2025-06-18+).
pub fn templates(titles: bool) -> Value {
    let t = |uri: &str, name: &str, title: &str, description: &str, mime: &str| {
        let mut v = json!({"uriTemplate": uri, "name": name, "description": description, "mimeType": mime});
        if titles && let Some(o) = v.as_object_mut() {
            o.insert("title".into(), json!(title));
        }
        v
    };
    json!([
        t(
            "effectcraft://comp/{comp}",
            "comp",
            "Composition",
            "A composition (get_comp): settings and layers. `comp` is an item id or a percent-encoded name.",
            "application/json"
        ),
        t(
            "effectcraft://layer/{comp}/{layer}",
            "layer",
            "Layer",
            "A layer's property tree (get_layer); append `?flat=true` for the flat list. `layer` is an id, `#n` (percent-encoded `%23n`) or a percent-encoded name.",
            "application/json"
        ),
        t(
            "effectcraft://frame/{comp}/{time}",
            "frame",
            "Frame",
            "A rendered frame as PNG (render_preview) at comp time `time` in seconds, longest side 960 (`?max_side=N`, 0 = full size).",
            "image/png"
        ),
        t("effectcraft://command/{id}", "command", "Command", "One engine command in full (describe_command): params doc and JSON Schema.", "application/json"),
    ])
}

/// Parse a resource URI.
pub fn parse(uri: &str) -> Result<Target, UriError> {
    let not_found = || UriError::NotFound(format!("resource not found: {uri}"));
    let rest = uri.strip_prefix(SCHEME).ok_or_else(not_found)?;
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (kind, tail) = match path.split_once('/') {
        Some((k, t)) => (k, Some(t)),
        None => (path, None),
    };
    let query = parse_query(query).ok_or_else(|| UriError::Invalid(format!("bad query in {uri}")))?;
    let segments = match tail {
        None => vec![],
        Some(t) => t.split('/').map(decode).collect::<Option<Vec<_>>>().ok_or_else(|| UriError::Invalid(format!("bad percent-encoding in {uri}")))?,
    };
    let allow = |keys: &[&str]| match query.iter().find(|(k, _)| !keys.contains(&k.as_str())) {
        Some((k, _)) => Err(UriError::Invalid(format!("unknown query parameter `{k}` in {uri}"))),
        None => Ok(()),
    };
    let q = |k: &str| query.iter().rev().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
    let shape = |shape: &str| UriError::Invalid(format!("expected {shape}, got {uri}"));
    if segments.iter().any(String::is_empty) {
        return Err(shape(&format!("{SCHEME}{kind}/… with non-empty segments")));
    }
    match (kind, segments.as_slice()) {
        ("document" | "commands", _) => {
            if tail.is_some() {
                return Err(not_found());
            }
            allow(&[])?;
            Ok(if kind == "document" { Target::Document } else { Target::Commands })
        }
        ("comp", [comp]) => {
            allow(&[])?;
            Ok(Target::Comp(reference(comp)))
        }
        ("layer", [comp, layer]) => {
            allow(&["flat"])?;
            let flat = match q("flat") {
                None | Some("false" | "0") => false,
                Some("true" | "1" | "") => true,
                Some(v) => return Err(UriError::Invalid(format!("flat must be true or false, got `{v}`"))),
            };
            Ok(Target::Layer { comp: reference(comp), layer: reference(layer), flat })
        }
        ("frame", [comp, time]) => {
            allow(&["max_side"])?;
            let time = time.parse::<f64>().ok().filter(|t| t.is_finite()).ok_or_else(|| UriError::Invalid(format!("time must be seconds, got `{time}`")))?;
            let max_side = match q("max_side") {
                None => FRAME_MAX_SIDE,
                Some(v) => v
                    .parse::<u32>()
                    .ok()
                    .filter(|n| *n <= FRAME_MAX_SIDE_LIMIT)
                    .ok_or_else(|| UriError::Invalid(format!("max_side must be 0-{FRAME_MAX_SIDE_LIMIT}, got `{v}`")))?,
            };
            Ok(Target::Frame { comp: reference(comp), time, max_side })
        }
        ("command", [id]) => {
            allow(&[])?;
            Ok(Target::Command(id.clone()))
        }
        ("comp", _) => Err(shape("effectcraft://comp/{comp}")),
        ("layer", _) => Err(shape("effectcraft://layer/{comp}/{layer}")),
        ("frame", _) => Err(shape("effectcraft://frame/{comp}/{time}")),
        ("command", _) => Err(shape("effectcraft://command/{id}")),
        _ => Err(not_found()),
    }
}

/// An item / layer reference: a number is an id, anything else a name (or `#n`).
fn reference(s: &str) -> Value {
    s.parse::<u64>().map_or_else(|_| json!(s), |n| json!(n))
}

/// `a=1&b=2` (values percent-decoded); `None` when malformed.
fn parse_query(q: &str) -> Option<Vec<(String, String)>> {
    if q.is_empty() {
        return Some(vec![]);
    }
    q.split('&')
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            Some((decode(k)?, decode(v)?))
        })
        .collect()
}

/// Percent-decode a URI component (RFC 3986). `None` for a truncated or non-hex escape or
/// bytes that aren't UTF-8.
pub fn decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        if c == b'%' {
            let hex = b.get(i + 1..i + 3)?;
            if !hex.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            out.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
            i += 3;
        } else {
            out.push(c);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Percent-encode a path segment (everything but RFC 3986 unreserved characters).
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &c in s.as_bytes() {
        if c.is_ascii_alphanumeric() || matches!(c, b'-' | b'.' | b'_' | b'~') {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{c:02X}"));
        }
    }
    out
}
