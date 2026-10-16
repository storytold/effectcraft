//! CLI-only setup of an Essential Graphics template instance using engine commands.

use std::io::Read;

use effectcraft_engine::{
    EngineError, Session,
    project::{ItemId, LayerId},
};
use serde_json::{Map, Value, json};

use crate::{Args, Failure, usage_err};

pub(super) fn validate(cmd: &str, args: &Args) -> Result<(), Failure> {
    if args.opt("--values").is_some() && args.opt("--template").is_none() {
        return usage_err("--values requires --template");
    }
    if args.opt("--template").is_none() {
        return Ok(());
    }
    if !matches!(cmd, "render" | "render-frame" | "frame") {
        return usage_err("--template is available for render and render-frame");
    }
    if args.project.is_some() || args.flag("--demo") || args.opt("--bridge").is_some() || args.flag("--queue") || args.opt("--comp").is_some() {
        return usage_err("--template renders a new instance; it cannot be combined with --project, --demo, --bridge, --queue or --comp");
    }
    if !args.pos.is_empty() {
        return usage_err("template rendering takes --template FILE.ectemplate, without positional arguments");
    }
    if cmd == "render" && args.opt("--out").is_none() {
        return usage_err("render: --out FILE is required (or --queue)");
    }
    // Resolve existing outputs before importing or rendering can overwrite an input.
    let out = args.opt("--out").or_else(|| matches!(cmd, "render-frame" | "frame").then_some("frame.png"));
    if let Some(out) = out
        && let Ok(output) = std::fs::canonicalize(out)
    {
        for input in ["--template", "--values"] {
            if let Some(path) = args.opt(input)
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
                return Err(Failure::Error(format!("--out {out} would overwrite {input} {path}")));
            }
        }
    }
    Ok(())
}

fn values(args: &Args) -> Result<Map<String, Value>, Failure> {
    let Some(path) = args.opt("--values") else { return Ok(Map::new()) };
    // A bounded read also handles a file that grows after opening.
    const MAX_BYTES: u64 = 1024 * 1024;
    let file = std::fs::File::open(path).map_err(|e| Failure::Error(format!("cannot read values {path}: {e}")))?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| Failure::Error(format!("cannot read values {path}: {e}")))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Failure::Error(format!("values {path}: JSON exceeds 1 MiB")));
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|e| Failure::Error(format!("values {path}: invalid JSON: {e}")))?;
    match value {
        Value::Object(values) => Ok(values),
        _ => Err(Failure::Error(format!("values {path}: expected a JSON object mapping control names to values"))),
    }
}

fn execute(s: &mut Session, command: &str, params: Value) -> Result<Value, Failure> {
    s.execute_checked(command, params).map_err(|e| Failure::Error(e.to_string()))
}

// Use the same instance commands to reject invalid values before import extracts media.
fn preflight(s: &Session, path: &str, values: &Map<String, Value>) -> Result<(), Failure> {
    let bytes = s.services.read_file(path).map_err(|e| Failure::Error(format!("cannot read {path}: {e}")))?;
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
        Ok(instantiate(&mut check, source, values))
    })
    .map_err(|e| Failure::Error(e.to_string()))?
}

pub(super) fn prepare(s: &mut Session, args: &Args) -> Result<(), Failure> {
    let Some(path) = args.opt("--template") else { return Ok(()) };
    let values = values(args)?;
    if !values.is_empty() {
        preflight(s, path, &values)?;
    }
    let imported = execute(s, "essential.importTemplate", json!({"path": path, "addToComp": false}))?;
    let source = imported.get("comp").and_then(Value::as_u64).ok_or_else(|| Failure::Error("template import did not return a composition".into()))?;
    instantiate(s, source, &values)
}

fn instantiate(s: &mut Session, source: u64, values: &Map<String, Value>) -> Result<(), Failure> {
    // Duplicate and empty the source with commands: this preserves exact rational frame rate,
    // Tick duration, background, work area and renderer settings without round-tripping floats.
    let duplicate = execute(s, "project.duplicate", json!({"items": [source]}))?;
    let parent = duplicate
        .get("items")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(Value::as_u64)
        .ok_or_else(|| Failure::Error("cannot create template render composition".into()))?;
    execute(s, "comp.open", json!({"comp": parent}))?;
    let layers: Vec<u64> = s
        .project
        .comp(ItemId(parent))
        .ok_or_else(|| Failure::Error("template render composition is missing".into()))?
        .layers
        .iter()
        .map(|layer| layer.id.0)
        .collect();
    if !layers.is_empty() {
        execute(s, "layer.setSwitch", json!({"comp": parent, "layers": layers, "switch": "lock", "value": false}))?;
        execute(s, "edit.clear", json!({"comp": parent, "layers": layers}))?;
    }
    let added = execute(s, "layer.addItem", json!({"comp": parent, "item": source, "time": 0}))?;
    let instance = added.get("layer").and_then(Value::as_u64).ok_or_else(|| Failure::Error("cannot create template instance".into()))?;
    // Keep a partial final frame: layer.addItem snaps its default end to the frame grid.
    s.edit("Template Duration", None, |project, _| {
        let comp = project.comp_mut(ItemId(parent)).ok_or(EngineError::NoComp)?;
        let duration = comp.duration;
        let layer = comp.layer_mut(LayerId(instance)).ok_or_else(|| EngineError::Other("template instance is missing".into()))?;
        layer.out_point = duration;
        Ok(())
    })
    .map_err(|e| Failure::Error(e.to_string()))?;
    // Let the wrapper carry the source composition's authored motion blur.
    execute(s, "layer.setSwitch", json!({"comp": parent, "layers": [instance], "switch": "motionBlur", "value": true}))?;
    for (control, value) in values {
        execute(s, "essential.set", json!({"comp": parent, "layer": instance, "control": control, "value": value}))?;
    }
    Ok(())
}
