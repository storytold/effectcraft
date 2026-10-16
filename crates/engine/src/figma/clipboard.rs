//! One-time FigCraft clipboard transfer. Native layers and bounded embedded bitmap assets; no network reads.

use std::borrow::Cow;

use serde::Deserialize;
use serde_json::{Value, json};

use super::{Source, build, map};
use crate::commands::{CommandSpec, always, bad, str_p};
use crate::{Result, Session};

const COMMAND: &str = "edit.pasteFigCraft";

fn paste_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let stamp = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    #[cfg(not(target_arch = "wasm32"))]
    let process = std::process::id();
    #[cfg(target_arch = "wasm32")]
    let process = 0;
    format!("{process}-{stamp}-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

#[path = "clipboard_assets.rs"]
mod assets;

const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_CARRIER_BYTES: usize = 128 * 1024 * 1024;
const MAX_ENCODED_BYTES: usize = MAX_BYTES.div_ceil(3) * 4;
const METADATA_PREFIX: &str = "<metadata id=\"figcraft-clipboard-v1\"";
const METADATA_START: &str = "<metadata id=\"figcraft-clipboard-v1\">";

/// Recognize damaged marked content too, so Paste never falls through to SVG or an older copy.
pub fn is_clipboard_text(text: &str) -> bool {
    text.contains(METADATA_PREFIX) || (text.trim_start().starts_with('{') && text.contains("\"format\"") && text.contains("\"figcraft.clipboard\""))
}

fn payload_json(text: &str) -> Result<Cow<'_, str>> {
    if !text.contains(METADATA_PREFIX) {
        return Ok(Cow::Borrowed(text));
    }
    if text.len() > MAX_CARRIER_BYTES {
        return Err(bad(COMMAND, "FigCraft SVG clipboard exceeds 128 MiB"));
    }
    if !crate::vector::is_svg_text(text) {
        return Err(bad(COMMAND, "FigCraft metadata must be inside an SVG clipboard"));
    }
    let mut markers = text.match_indices(METADATA_PREFIX);
    let (index, _) = markers.next().ok_or_else(|| bad(COMMAND, "missing FigCraft clipboard metadata"))?;
    if markers.next().is_some() {
        return Err(bad(COMMAND, "duplicate FigCraft clipboard metadata"));
    }
    let content = text.get(index..).and_then(|s| s.strip_prefix(METADATA_START)).ok_or_else(|| bad(COMMAND, "malformed FigCraft clipboard metadata"))?;
    let (encoded, _) = content.split_once("</metadata>").ok_or_else(|| bad(COMMAND, "unterminated FigCraft clipboard metadata"))?;
    if encoded.len() > MAX_ENCODED_BYTES {
        return Err(bad(COMMAND, "encoded FigCraft clipboard exceeds the 64 MiB payload limit"));
    }
    let bytes = super::fetch::base64_decode(encoded).map_err(|e| bad(COMMAND, format!("invalid FigCraft clipboard base64: {e}")))?;
    if bytes.len() > MAX_BYTES {
        return Err(bad(COMMAND, "FigCraft clipboard exceeds 64 MiB"));
    }
    // The existing decoder also accepts data URLs and loose padding; this carrier is canonical
    // standard base64, so damaged encoding cannot silently become a different JSON payload.
    if effectcraft_track::roto::rle::base64_encode(&bytes) != encoded {
        return Err(bad(COMMAND, "invalid FigCraft clipboard base64 encoding"));
    }
    String::from_utf8(bytes).map(Cow::Owned).map_err(|_| bad(COMMAND, "FigCraft clipboard metadata is not UTF-8"))
}

// Bound recursion before serde builds a Value; node arrays add two JSON levels per node.
fn bounded_json(text: &str) -> Result<()> {
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for byte in text.bytes() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'{' | b'[' => {
                    depth = depth.saturating_add(1);
                    if depth > 192 {
                        return Err(bad(COMMAND, "clipboard JSON nesting exceeds 192 levels"));
                    }
                }
                b'}' | b']' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    Ok(())
}

fn validate_tree(value: &Value, depth: usize, count: &mut usize, geometry_entry: bool) -> Result<()> {
    if depth > 192 {
        return Err(bad(COMMAND, "clipboard JSON nesting exceeds 192 levels"));
    }
    match value {
        Value::Object(object) => {
            if object.contains_key("type") && object.contains_key("id") {
                *count = count.saturating_add(1);
                if *count > 20_000 {
                    return Err(bad(COMMAND, "clipboard contains more than 20000 nodes"));
                }
            }
            for (key, child) in object {
                if matches!(key.as_str(), "images" | "pdf" | "png" | "pdfPath" | "payloadPath" | "payloadPaths" | "url" | "apiBase" | "token" | "$ref")
                    || (key == "path" && !(geometry_entry && child.is_string()))
                {
                    return Err(bad(COMMAND, format!("external or embedded assets are not supported: `{key}`")));
                }
                if matches!(key.as_str(), "fillGeometry" | "strokeGeometry") {
                    let entries = child.as_array().ok_or_else(|| bad(COMMAND, "path geometry must be an array"))?;
                    for entry in entries {
                        validate_tree(entry, depth + 2, count, true)?;
                    }
                } else {
                    validate_tree(child, depth + 1, count, false)?;
                }
            }
        }
        Value::Array(values) => {
            for child in values {
                validate_tree(child, depth + 1, count, false)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_nodes(nodes: &[Value], depth: usize) -> Result<()> {
    if depth > 64 {
        return Err(bad(COMMAND, "clipboard node nesting exceeds 64 levels"));
    }
    for node in nodes {
        if node.get("id").and_then(Value::as_str).is_none_or(str::is_empty) || node.get("type").and_then(Value::as_str).is_none_or(str::is_empty) {
            return Err(bad(COMMAND, "every node needs a nonempty string id and type"));
        }
        if let Some(children) = node.get("children") {
            let children = children.as_array().ok_or_else(|| bad(COMMAND, "node children must be an array"))?;
            if !children.is_empty() {
                validate_nodes(children, depth + 1)?;
            }
        }
    }
    Ok(())
}

fn paste(s: &mut Session, p: &Value) -> Result<Value> {
    let text = str_p(p, "clipboard").ok_or_else(|| bad(COMMAND, "missing `clipboard` JSON text"))?;
    let text = payload_json(text)?;
    if text.len() > MAX_BYTES {
        return Err(bad(COMMAND, "FigCraft clipboard exceeds 64 MiB"));
    }
    bounded_json(&text)?;
    let mut decoder = serde_json::Deserializer::from_str(&text);
    decoder.disable_recursion_limit();
    let payload = Value::deserialize(&mut decoder).map_err(|e| bad(COMMAND, format!("invalid FigCraft clipboard JSON: {e}")))?;
    decoder.end().map_err(|e| bad(COMMAND, format!("invalid FigCraft clipboard JSON: {e}")))?;
    let object = payload.as_object().ok_or_else(|| bad(COMMAND, "clipboard must be an object"))?;
    if object.keys().any(|key| !matches!(key.as_str(), "format" | "version" | "nodes" | "warnings" | "assets")) {
        return Err(bad(COMMAND, "clipboard accepts only format, version, nodes, warnings and assets"));
    }
    if payload.get("format").and_then(Value::as_str) != Some("figcraft.clipboard") || !matches!(payload.get("version").and_then(Value::as_u64), Some(1 | 2)) {
        return Err(bad(COMMAND, "expected figcraft.clipboard version 1 or 2"));
    }
    if payload.get("version").and_then(Value::as_u64) == Some(1) && payload.get("assets").is_some() {
        return Err(bad(COMMAND, "embedded assets require clipboard version 2"));
    }
    let nodes =
        payload.get("nodes").and_then(Value::as_array).filter(|a| !a.is_empty()).ok_or_else(|| bad(COMMAND, "clipboard nodes must be a nonempty array"))?;
    let mut count = 0;
    for node in nodes {
        validate_tree(node, 0, &mut count, false)?;
    }
    validate_nodes(nodes, 0)?;
    let mut nodes = nodes.clone();
    let empty_assets = json!({});
    let asset_value = if payload.get("version").and_then(Value::as_u64) == Some(2) { Some(payload.get("assets").unwrap_or(&empty_assets)) } else { None };
    let images = assets::prepare(asset_value, &mut nodes)?;
    let mut warnings = match payload.get("warnings") {
        None => vec![],
        Some(Value::Array(values)) => {
            values.iter().map(|v| v.as_str().map(str::to_string).ok_or_else(|| bad(COMMAND, "warnings must be strings"))).collect::<Result<Vec<_>>>()?
        }
        _ => return Err(bad(COMMAND, "warnings must be an array")),
    };
    let target = str_p(p, "target").unwrap_or("auto");
    if !matches!(target, "auto" | "newComp" | "activeComp") {
        return Err(bad(COMMAND, "target must be auto, newComp or activeComp"));
    }
    let active = match p.get("comp") {
        Some(c) => Some(s.resolve_comp(Some(c))?),
        None => s.active_comp_id(),
    };
    let all_frames =
        nodes.iter().all(|n| matches!(n.get("type").and_then(Value::as_str), Some("FRAME" | "COMPONENT" | "INSTANCE" | "COMPONENT_SET" | "SECTION")));
    let into = if target == "activeComp" || (target == "auto" && !all_frames) { active } else { None };
    if target == "activeComp" && into.is_none() {
        return Err(bad(COMMAND, "no composition is open"));
    }
    if all_frames && into.is_none() {
        for node in &nodes {
            if node.get("figcraft").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            let name = node.get("name").and_then(Value::as_str).unwrap_or("Frame");
            let rounded = node.get("cornerRadius").and_then(Value::as_f64).is_some_and(|r| r > 0.0)
                || node.get("rectangleCornerRadii").and_then(Value::as_array).is_some_and(|r| r.iter().any(|v| v.as_f64().is_some_and(|r| r > 0.0)));
            if rounded && node.get("clipsContent").and_then(Value::as_bool) == Some(true) {
                warnings.push(format!("{name}: rounded clipping of the root frame is not applied to the new composition"));
            }
            if node.get("visible").and_then(Value::as_bool) == Some(false) {
                warnings.push(format!("{name}: a hidden root frame becomes a visible composition"));
            }
            if node.get("blendMode").and_then(Value::as_str).is_some_and(|mode| !matches!(mode, "NORMAL" | "PASS_THROUGH")) {
                warnings.push(format!("{name}: the root frame blend mode is not applied to the new composition"));
            }
        }
    }
    let target = match into.and_then(|id| s.project.comp(id)) {
        Some(comp) => map::Target::ActiveComp { width: comp.width, height: comp.height },
        None => map::Target::NewComp,
    };
    let mut plan = map::plan(&nodes, &map::Options { target });
    for comp in &mut plan.comps {
        if comp.figma_id.is_empty()
            && let Some(name) = comp.name.strip_suffix(" (Figma)")
        {
            comp.name = format!("{name} (FigCraft)");
        }
    }
    for note in &mut plan.notes {
        *note = note.replace("stay in Figma (it became the composition)", "are not applied to the new composition");
    }
    warnings.append(&mut plan.notes);
    warnings.extend(
        plan.rasters
            .iter()
            .filter(|r| !images.contains_key(&r.node_id))
            .map(|r| format!("{}: omitted ({}); this paste keeps native editable layers only", r.name, r.reason)),
    );
    if plan.counts().0 == 0 && !plan.rasters.iter().any(|r| images.contains_key(&r.node_id)) {
        return Err(bad(COMMAND, format!("clipboard has no supported native layers. {}", warnings.join("; "))));
    }
    plan.notes = warnings.clone();
    // Only generated base64 entries reach the shared builder; caller paths and URLs were rejected.
    let source = Source { nodes: vec![], images, rest: None, label: format!("FigCraft-{}-{}", paste_id(), s.revision) };
    let (frame_rate, duration) = into.and_then(|id| s.project.comp(id)).map(|comp| (comp.frame_rate.as_f64(), comp.duration.seconds())).unwrap_or((30.0, 10.0));
    let settings = build::Settings { origin: "FigCraft", undo_label: "Paste from FigCraft", frame_rate, duration, into };
    let mut result = build::run(s, plan, &source, 1.0, &settings)?;
    result["warnings"] = json!(warnings);
    if !warnings.is_empty() {
        let preview = warnings.iter().take(5).map(|warning| warning.chars().take(240).collect::<String>()).collect::<Vec<_>>().join("; ");
        s.toast(format!("Pasted from FigCraft with {} warning(s): {preview}", warnings.len()));
    }
    Ok(result)
}

pub(super) fn spec() -> CommandSpec {
    CommandSpec {
        id: COMMAND,
        label: "Paste from FigCraft",
        menu: &[],
        shortcut: None,
        alt_shortcuts: &[],
        params: "{clipboard (FigCraft SVG carrier up to 128 MiB, or version 1/2 JSON text up to 64 MiB), target?: auto|newComp|activeComp, comp?} → {comp, layers, native, warnings, notes}",
        enabled: always,
        run: paste,
        journal: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_project::{ItemId, LayerSource};

    #[test]
    fn independent_pastes_have_distinct_asset_ids() {
        assert_ne!(paste_id(), paste_id());
    }

    fn rectangle() -> Value {
        json!({"id":"rect", "type":"RECTANGLE", "name":"Editable rectangle", "size":{"x":40,"y":20},
            "absoluteTransform":[[1,0,10],[0,1,15]], "children":[], "fills":[{"type":"SOLID","color":{"r":1,"g":0,"b":0,"a":1}}]})
    }

    fn envelope(nodes: Vec<Value>) -> Value {
        json!({"format":"figcraft.clipboard", "version":1,"nodes":nodes,"warnings":[]})
    }

    fn params(payload: &Value) -> Value {
        json!({"clipboard":payload.to_string()})
    }

    fn carrier(payload: &Value) -> String {
        let encoded = effectcraft_track::roto::rle::base64_encode(payload.to_string().as_bytes());
        format!("<svg xmlns=\"http://www.w3.org/2000/svg\"><path d=\"M0 0L10 10\"/>{METADATA_START}{encoded}</metadata></svg>")
    }

    #[test]
    fn normal_copy_svg_metadata_pastes_native_vector_paths() {
        let node = json!({"id":"vector","type":"VECTOR","name":"Native triangle","size":{"x":20,"y":20},
            "absoluteTransform":[[1,0,0],[0,1,0]],
            "fillGeometry":[{"path":"M0 0L20 0L10 20Z","windingRule":"NONZERO"}],
            "strokeGeometry":[{"path":"M0 0L20 0L10 20Z","windingRule":"NONZERO"}],
            "fills":[{"type":"SOLID","color":{"r":1,"g":0,"b":0,"a":1}}]});
        let payload = envelope(vec![node]);
        for clipboard in [payload.to_string(), carrier(&payload)] {
            assert!(is_clipboard_text(&clipboard));
            let mut s = Session::default();
            let result = s.execute_checked(COMMAND, json!({"clipboard":clipboard})).unwrap();
            assert_eq!(result["native"], 1);
            assert_eq!(result["images"], 0);
            let layer = &s.active_comp().unwrap().layers[0];
            assert!(matches!(layer.source, LayerSource::Shape));
            assert_eq!(layer.name, "Native triangle");
            let mut paths = vec![];
            layer.props.walk("", &mut |_, prop| {
                if let Some(path) = prop.value.as_path() {
                    paths.push(path.clone());
                }
            });
            assert_eq!(paths.len(), 1);
            assert_eq!(paths[0].vertices, vec![[0.0, 0.0], [20.0, 0.0], [10.0, 20.0]]);
            assert!(paths[0].closed);
        }
        assert!(!is_clipboard_text("<svg><metadata id=\"other\">ordinary SVG</metadata></svg>"));
    }

    #[test]
    fn malformed_or_oversized_svg_metadata_changes_nothing() {
        let payload = envelope(vec![rectangle()]);
        let valid = carrier(&payload);
        let duplicate = valid.replace("</svg>", &format!("{METADATA_START}e30=</metadata></svg>"));
        let bad_base64 = format!("<svg>{METADATA_START}%%%not-base64</metadata></svg>");
        let bad_padding = format!("<svg>{METADATA_START}e30===</metadata></svg>");
        let bad_utf8 = format!("<svg>{METADATA_START}/w==</metadata></svg>");
        let oversized_encoding = format!("<svg>{METADATA_START}{}</metadata></svg>", "A".repeat(MAX_ENCODED_BYTES + 4));
        let oversized_decoded = format!("<svg>{METADATA_START}{}</metadata></svg>", "A".repeat(MAX_ENCODED_BYTES));
        let oversized_carrier = format!("<svg>{}{METADATA_START}e30=</metadata></svg>", " ".repeat(MAX_CARRIER_BYTES));
        for clipboard in [
            duplicate,
            bad_base64,
            bad_padding,
            bad_utf8,
            oversized_encoding,
            oversized_decoded,
            oversized_carrier,
            valid.replace("</metadata>", ""),
            valid.replace(METADATA_START, &format!("{METADATA_PREFIX} extra=\"bad\">")),
        ] {
            assert!(is_clipboard_text(&clipboard));
            let mut s = Session::default();
            assert!(s.execute_checked(COMMAND, json!({"clipboard":clipboard})).is_err());
            assert!(s.project.items.is_empty());
            assert!(s.history.undo.is_empty());
            assert_eq!(s.revision, 0);
        }
    }

    #[test]
    fn geometry_path_exception_does_not_allow_asset_paths_elsewhere() {
        for extra in [
            json!({"path":"/private/asset.svg"}),
            json!({"fillGeometry":[{"extra":{"path":"/private/asset.svg"}}]}),
            json!({"fillGeometry":[{"path":{"path":"/private/asset.svg"}}]}),
            json!({"strokeGeometry":[{"pdfPath":"/private/asset.pdf"}]}),
        ] {
            let mut node = rectangle();
            node.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            let mut s = Session::default();
            assert!(s.execute_checked(COMMAND, params(&envelope(vec![node]))).is_err());
            assert!(s.project.items.is_empty());
        }
    }

    #[test]
    fn figcraft_paste_creates_native_layers_and_one_undo_step() {
        let mut s = Session::default();
        let mut payload = envelope(vec![json!({"id":"frame", "type":"FRAME", "name":"Card", "size":{"x":200,"y":100},
            "absoluteTransform":[[1,0,0],[0,1,0]], "children":[rectangle(),
            {"id":"text", "type":"TEXT", "name":"Title", "characters":"Hello", "size":{"x":100,"y":30},
             "absoluteTransform":[[1,0,20],[0,1,40]], "style":{"fontFamily":"sans-serif","fontSize":20},
             "fills":[{"type":"SOLID","color":{"r":0,"g":0,"b":0,"a":1}}]}]})]);
        payload["warnings"] = json!(["Test: missing font substituted"]);
        let result = s.execute_checked(COMMAND, params(&payload)).unwrap();
        let comp_id = ItemId(result["comp"].as_u64().unwrap());
        let comp = s.project.comp(comp_id).unwrap();
        assert_eq!((comp.width, comp.height), (200, 100));
        assert!(comp.layers.iter().any(|l| matches!(l.source, LayerSource::Shape) && l.props.sub("contents").unwrap().groups().count() > 0));
        assert!(comp.layers.iter().any(|l| matches!(l.source, LayerSource::Text)));
        assert!(comp.layers.iter().all(|l| l.comment.starts_with("FigCraft ")));
        assert_eq!(result["images"], 0);
        assert_eq!(result["warnings"][0], "Test: missing font substituted");
        assert_eq!(s.history.undo.len(), 1);
        assert_eq!(s.history.undo[0].0, "Paste from FigCraft");
        s.execute_checked("edit.undo", json!({})).unwrap();
        assert!(s.project.items.is_empty());
        s.execute_checked("edit.redo", json!({})).unwrap();
        assert!(s.project.comp(comp_id).is_some());
        assert_eq!(s.active_comp_id(), Some(comp_id));
    }

    #[test]
    fn figcraft_paste_rejects_bad_envelopes_and_asset_inputs_atomically() {
        let mut s = Session::default();
        for payload in [
            json!({"format":"figcraft.clipboard","version":3,"nodes":[rectangle()]}),
            json!({"format":"figcraft.clipboard","version":1,"nodes":[rectangle()],"images":{"rect":{"pdfPath":"/not/read"}}}),
            envelope(vec![json!({"id":"bad","type":"FRAME","children":"wrong"})]),
            envelope(vec![json!({"id":"bad","type":"RECTANGLE","url":"https://invalid.example/"})]),
            envelope(vec![json!({"id":"bad","type":"WIDGET"})]),
            envelope(vec![]),
        ] {
            assert!(s.execute_checked(COMMAND, params(&payload)).is_err(), "{payload}");
            assert!(s.project.items.is_empty());
            assert!(s.history.undo.is_empty());
            assert_eq!(s.revision, 0);
        }
        assert!(s.execute_checked(COMMAND, json!({"clipboard":"x".repeat(MAX_BYTES + 1)})).is_err());
        assert!(s.execute_checked(COMMAND, json!({"clipboard":"{".repeat(193)})).is_err());
        let too_many = envelope(vec![json!({"id":"n","type":"GROUP"}); 20_001]);
        assert!(s.execute_checked(COMMAND, params(&too_many)).unwrap_err().to_string().contains("20000"));
        assert!(s.project.items.is_empty());
        assert!(s.history.undo.is_empty());
    }

    #[test]
    fn figcraft_paste_omits_unsupported_nodes_with_visible_warning_and_no_footage() {
        let mut s = Session::default();
        let result =
            s.execute_checked(COMMAND, params(&envelope(vec![rectangle(), json!({"id":"widget", "type":"WIDGET", "name":"Unsupported widget"})]))).unwrap();
        assert_eq!(result["images"], 0);
        assert!(result["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("Unsupported widget: omitted")));
        assert!(s.events.iter().any(|event| matches!(event, crate::Event::Toast { message, .. } if message.contains("warning"))));
        assert!(s.project.items.values().all(|item| !matches!(item.kind, effectcraft_project::ItemKind::Footage(_))));
    }

    #[test]
    fn figcraft_paste_accepts_64_node_levels_and_rejects_more() {
        let mut node = rectangle();
        for level in 0..64 {
            node = json!({"id":format!("group{level}"),"type":"GROUP","name":"Group","children":[node]});
        }
        let mut s = Session::default();
        let result = s.execute_checked(COMMAND, params(&envelope(vec![node.clone()]))).unwrap();
        assert!(result["warnings"].as_array().unwrap().is_empty(), "{result}");
        assert!(s.active_comp().unwrap().layers.iter().any(|layer| matches!(layer.source, LayerSource::Shape)));
        assert_eq!(result["native"], 65);
        let too_deep = json!({"id":"extra","type":"GROUP","children":[node]});
        assert!(s.execute_checked(COMMAND, params(&envelope(vec![too_deep]))).is_err());
    }

    #[test]
    fn figcraft_paste_reports_root_frame_appearance_limits() {
        let mut s = Session::default();
        let payload = envelope(vec![json!({"id":"frame","type":"FRAME","name":"Card","size":{"x":100,"y":100},
            "cornerRadius":12,"clipsContent":true,"visible":false,"blendMode":"MULTIPLY","children":[rectangle()]})]);
        let result = s.execute_checked(COMMAND, params(&payload)).unwrap();
        let warnings = result["warnings"].to_string();
        assert!(warnings.contains("rounded clipping"));
        assert!(warnings.contains("hidden root frame"));
        assert!(warnings.contains("blend mode"));
    }

    #[test]
    fn figcraft_paste_does_not_count_brackets_inside_text_as_nesting() {
        let mut s = Session::default();
        let text = "{[\\\"".repeat(200);
        let payload = envelope(vec![json!({"id":"text","type":"TEXT","name":"Text","characters":text,
            "size":{"x":100,"y":30},"style":{"fontSize":20}})]);
        assert!(s.execute_checked(COMMAND, params(&payload)).is_ok());
    }

    #[test]
    fn figcraft_paste_into_existing_comp_inherits_its_timing_for_nested_comps() {
        let mut s = Session::default();
        s.execute_checked("comp.new", json!({"name":"Main","width":320,"height":180,"frameRate":24,"duration":20})).unwrap();
        let payload = envelope(vec![json!({"id":"frame","type":"FRAME","name":"Nested","size":{"x":100,"y":100},"children":[rectangle()]})]);
        let mut request = params(&payload);
        request["target"] = json!("activeComp");
        let result = s.execute_checked(COMMAND, request).unwrap();
        let active = s.project.comp(ItemId(result["comp"].as_u64().unwrap())).unwrap();
        let layer = &active.layers[0];
        let LayerSource::Comp { item } = layer.source else { panic!("expected a precomp") };
        let nested = s.project.comp(item).unwrap();
        assert_eq!(nested.frame_rate.as_f64(), 24.0);
        assert_eq!(nested.duration.seconds(), 20.0);
        assert_eq!(layer.out_point.seconds(), 20.0);
        assert_eq!(nested.layers[0].out_point.seconds(), 20.0);
    }

    #[test]
    fn figcraft_paste_redo_keeps_an_existing_active_comp() {
        let mut s = Session::default();
        s.execute_checked("comp.new", json!({"name":"Existing"})).unwrap();
        let existing = s.active_comp_id().unwrap();
        let payload = envelope(vec![json!({"id":"frame","type":"FRAME","name":"Pasted","size":{"x":100,"y":100},"children":[rectangle()]})]);
        let result = s.execute_checked(COMMAND, params(&payload)).unwrap();
        let pasted = ItemId(result["comp"].as_u64().unwrap());
        assert_eq!(s.active_comp_id(), Some(pasted));
        s.execute_checked("edit.undo", json!({})).unwrap();
        assert_eq!(s.active_comp_id(), Some(existing));
        s.execute_checked("edit.redo", json!({})).unwrap();
        assert!(s.project.comp(pasted).is_some());
        assert_eq!(s.active_comp_id(), Some(existing));
    }

    #[test]
    fn figcraft_paste_new_canvas_includes_stroke_and_shadow_overflow() {
        let node = json!({"id":"line","type":"LINE","name":"Thick line","size":{"x":100,"y":0},
            "absoluteTransform":[[1,0,10],[0,1,20]],"absoluteBoundingBox":{"x":10,"y":20,"width":100,"height":0},
            "absoluteRenderBounds":{"x":4,"y":14,"width":116,"height":28},
            "strokeWeight":8,"strokes":[{"type":"SOLID","color":{"r":1,"g":0,"b":0,"a":1}}],
            "effects":[{"type":"DROP_SHADOW","radius":4,"offset":{"x":6,"y":6},"color":{"r":0,"g":0,"b":0,"a":1}}]});
        let mut s = Session::default();
        s.execute_checked(COMMAND, params(&envelope(vec![node.clone()]))).unwrap();
        let comp = s.active_comp().unwrap();
        assert_eq!((comp.width, comp.height), (116, 28));
        assert_eq!(comp.layers[0].props.prop("transform/position").unwrap().value.as_vec3(), [56.0, 6.0, 0.0]);
        let active = map::plan(&[node], &map::Options { target: map::Target::ActiveComp { width: 1000, height: 500 } });
        assert_eq!(active.comps[0].layers[0].position, [500.0, 250.0]);

        for bounds in [
            json!({"x":20,"y":20,"width":1,"height":1}),
            json!({"x":0,"y":0,"width":0,"height":0}),
            json!({"x":1e308,"y":0,"width":1e308,"height":10}),
            json!({"x":"bad","y":0,"width":1,"height":1}),
        ] {
            let mut rectangle = rectangle();
            rectangle["absoluteBoundingBox"] = json!({"x":10,"y":15,"width":40,"height":20});
            rectangle["absoluteRenderBounds"] = bounds;
            let plan = map::plan(&[rectangle], &map::Options { target: map::Target::NewComp });
            assert_eq!((plan.comps[0].width, plan.comps[0].height), (40, 20));
        }
    }
}
