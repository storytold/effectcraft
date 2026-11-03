//! CLI-only setup of an Essential Graphics template instance using engine commands.

use std::io::Read;

use effectcraft_engine::{
    EngineError, Session,
    project::{ItemId, LayerId, essential::EgKind},
};
use serde_json::{Map, Value, json};

use crate::error::TemplateError;

type Result<T = ()> = std::result::Result<T, TemplateError>;

/// Refuse an `--out` that is the template or the values file, before importing or rendering can
/// overwrite an input. Whether `--template` may be combined with other options, and what it needs,
/// is the command line's (`args`).
pub(super) fn check_output(out: &str, template: &str, values: Option<&str>) -> Result {
    let Ok(output) = std::fs::canonicalize(out) else { return Ok(()) };
    for (input, path) in [("--template", Some(template)), ("--values", values)] {
        if let Some(path) = path
            && std::fs::canonicalize(path).is_ok_and(|path| {
                if path == output {
                    return true;
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    // Distinct canonical paths can still name the same file through hard links.
                    if let (Ok(input), Ok(output)) = (std::fs::metadata(&path), std::fs::metadata(&output)) {
                        return input.dev() == output.dev() && input.ino() == output.ino();
                    }
                }
                false
            })
        {
            return Err(TemplateError::OutIsInput { out: out.to_owned(), input, path: path.to_owned() });
        }
    }
    Ok(())
}

fn values(path: Option<&str>) -> Result<Map<String, Value>> {
    let Some(path) = path else { return Ok(Map::new()) };
    // A bounded read also handles a file that grows after opening.
    const MAX_BYTES: u64 = 1024 * 1024;
    let read_err = |source| TemplateError::ReadValues { path: path.to_owned(), source };
    let file = std::fs::File::open(path).map_err(read_err)?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(read_err)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(TemplateError::ValuesTooBig(path.to_owned()));
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|source| TemplateError::ValuesJson { path: path.to_owned(), source })?;
    match value {
        Value::Object(values) => Ok(values),
        _ => Err(TemplateError::ValuesNotAnObject(path.to_owned())),
    }
}

fn execute(s: &mut Session, command: &str, params: Value) -> Result<Value> {
    Ok(s.execute_checked(command, params)?)
}

// Use the same instance commands to reject invalid values before import extracts media.
fn preflight(s: &Session, path: &str, values: &Map<String, Value>) -> Result {
    let bytes = s.services.read_file(path).map_err(|e| TemplateError::ReadTemplate { path: path.to_owned(), message: e.to_string() })?;
    effectcraft_engine::guard::guarded("template value validation", || {
        let bad = |msg: String| EngineError::BadParams { cmd: "essential.importTemplate".into(), msg };
        let (manifest, mut project, _) = effectcraft_engine::commands::essential::read_template(&bytes).map_err(bad)?;
        let source = manifest.get("comp").and_then(Value::as_u64).ok_or_else(|| bad("manifest has no `comp`".into()))?;
        let base = s.project.next_id;
        let source = source.checked_add(base).ok_or_else(|| bad("template composition ID overflows the import offset".into()))?;
        effectcraft_engine::project::essential::offset_ids(&mut project, base);
        project.fix_next_id();
        let mut check = Session::new();
        check.project = project.into();
        Ok(instantiate(&mut check, source, values, false))
    })
    .map_err(|e| TemplateError::Engine(EngineError::Other(e.to_string())))?
}

pub(super) fn prepare(s: &mut Session, path: &str, values_path: Option<&str>) -> Result {
    let values = values(values_path)?;
    if !values.is_empty() {
        preflight(s, path, &values)?;
    }
    let imported = execute(s, "essential.importTemplate", json!({"path": path, "addToComp": false}))?;
    let source = imported.get("comp").and_then(Value::as_u64).ok_or(TemplateError::NoImportedComp)?;
    instantiate(s, source, &values, true)
}

/// The file a Media Replacement control is given: a path, or `{path|file|media: path}`.
fn media_path(value: &Value) -> Option<&str> {
    match value {
        Value::String(path) => Some(path.as_str()),
        Value::Object(m) => ["path", "file", "media"].iter().find_map(|k| m.get(*k)).and_then(Value::as_str),
        _ => None,
    }
}

/// Whether the template's control of that name (looked up as `essential.set` does) is a Media Replacement.
fn is_media_control(s: &Session, source: u64, name: &str) -> bool {
    let Some(eg) = s.project.comp(ItemId(source)).and_then(|c| c.essential.as_ref()) else { return false };
    let all = eg.flat();
    let control = all.iter().find(|c| c.name == name).or_else(|| all.iter().find(|c| c.name.eq_ignore_ascii_case(name)));
    control.is_some_and(|c| matches!(c.kind, EgKind::Media { .. }))
}

fn instantiate(s: &mut Session, source: u64, values: &Map<String, Value>, import_media: bool) -> Result {
    // Duplicate and empty the source with commands: this preserves exact rational frame rate,
    // Tick duration, background, work area and renderer settings without round-tripping floats.
    let duplicate = execute(s, "project.duplicate", json!({"items": [source]}))?;
    let parent = duplicate.get("items").and_then(Value::as_array).and_then(|items| items.first()).and_then(Value::as_u64).ok_or(TemplateError::NoRenderComp)?;
    execute(s, "comp.open", json!({"comp": parent}))?;
    let layers: Vec<u64> = s.project.comp(ItemId(parent)).ok_or(TemplateError::RenderCompMissing)?.layers.iter().map(|layer| layer.id.0).collect();
    if !layers.is_empty() {
        execute(s, "layer.setSwitch", json!({"comp": parent, "layers": layers, "switch": "lock", "value": false}))?;
        execute(s, "edit.clear", json!({"comp": parent, "layers": layers}))?;
    }
    let added = execute(s, "layer.addItem", json!({"comp": parent, "item": source, "time": 0}))?;
    let instance = added.get("layer").and_then(Value::as_u64).ok_or(TemplateError::NoInstance)?;
    // Keep a partial final frame: layer.addItem snaps its default end to the frame grid. Give the
    // wrapper the source's exact name, not Duplicate's numbered one, for `[compName]` outputs.
    s.edit("Template Duration", None, |project, _| {
        let name = project.item(ItemId(source)).ok_or_else(|| EngineError::Other("template composition is missing".into()))?.name.clone();
        project.item_mut(ItemId(parent)).ok_or_else(|| EngineError::Other("template render composition is missing".into()))?.name = name;
        let comp = project.comp_mut(ItemId(parent)).ok_or(EngineError::NoComp)?;
        let duration = comp.duration;
        let layer = comp.layer_mut(LayerId(instance)).ok_or_else(|| EngineError::Other("template instance is missing".into()))?;
        layer.out_point = duration;
        Ok(())
    })
    .map_err(|e| TemplateError::Engine(EngineError::Other(e.to_string())))?;
    // Let the wrapper carry the source composition's authored motion blur.
    execute(s, "layer.setSwitch", json!({"comp": parent, "layers": [instance], "switch": "motionBlur", "value": true}))?;
    for (control, value) in values {
        if is_media_control(s, source, control) {
            let path = media_path(value).ok_or_else(|| TemplateError::MediaNeedsPath(control.clone()))?;
            // The preflight session cannot import files; the real run reports a bad path.
            if !import_media {
                continue;
            }
            let imported = execute(s, "file.import", json!({"paths": [path]}))?;
            if let Some(err) = imported.get("errors").and_then(Value::as_array).and_then(|e| e.first()) {
                return Err(TemplateError::MediaImport { control: control.clone(), path: path.to_owned(), error: err.clone() });
            }
            let item = imported
                .get("items")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(Value::as_u64)
                .ok_or_else(|| TemplateError::MediaNotFootage { control: control.clone(), path: path.to_owned() })?;
            execute(s, "essential.set", json!({"comp": parent, "layer": instance, "control": control, "item": item}))?;
            continue;
        }
        execute(s, "essential.set", json!({"comp": parent, "layer": instance, "control": control, "value": value}))?;
    }
    Ok(())
}
