//! Read-only destination review, shared by interactive and control-channel hosts.

use serde_json::{Map, Value, json};

use crate::{Job, Result, io};

const FORMAT: &str = "effectcraft.output-approval.v1";
#[cfg(not(target_arch = "wasm32"))]
const MAX_COLLISIONS: usize = 100_000;
#[cfg(not(target_arch = "wasm32"))]
const MAX_TOKEN_BYTES: usize = 64 * 1024 * 1024;

/// A host retains `authorization` with its queue/settings snapshot, shows the collisions
/// once, and supplies the unchanged value to `export_authorized` after acceptance.
#[derive(Clone, Debug)]
pub struct OutputPreflight {
    pub collisions: Vec<String>,
    /// Canonical candidate write paths, for detecting conflicts across queued outputs.
    pub destinations: Vec<String>,
    pub authorization: Value,
}

/// Inspect exactly the paths the exporter can write, including sequence frame numbers
/// and configured overflow alternatives. Inspection creates no files or directories.
pub fn preflight(job: &Job<'_>) -> Result<OutputPreflight> {
    let (targets, collisions, destinations) = collect(job)?;
    Ok(OutputPreflight { collisions, destinations, authorization: json!({"format": FORMAT, "targets": targets}) })
}

#[cfg(not(target_arch = "wasm32"))]
fn collect(job: &Job<'_>) -> Result<(Map<String, Value>, Vec<String>, Vec<String>)> {
    let mut targets = Map::new();
    let mut collisions = Vec::new();
    let mut bytes = serde_json::to_vec(&json!({"format": FORMAT, "targets": {}})).map_err(io)?.len();
    let mut destinations = std::collections::BTreeSet::new();
    if job.sink.is_none() {
        visit_paths(job, |path, skipped| {
            // Skipped files need neither write access nor approval. Omitting them from
            // the token also prevents a later disappearance from authorizing a write.
            if skipped {
                return Ok(());
            }
            let (key, entry, existing) = crate::pending::inspect(path).map_err(io)?;
            if targets.contains_key(&key) {
                return Ok(());
            }
            if existing && collisions.len() >= MAX_COLLISIONS {
                return Err(io("export preflight exceeds 100000 existing outputs"));
            }
            let size = serde_json::to_vec(&entry)
                .map_err(io)?
                .len()
                .checked_add(serde_json::to_vec(&key).map_err(io)?.len())
                // The colon and comma; the initial envelope already includes braces.
                .and_then(|size| size.checked_add(2))
                .ok_or_else(|| io("export preflight metadata size overflow"))?;
            bytes = bytes.checked_add(size).filter(|n| *n <= MAX_TOKEN_BYTES).ok_or_else(|| io("export preflight metadata exceeds 64 MiB"))?;
            if existing {
                collisions.push(path.to_string());
            }
            let resolved = entry.get("resolved").and_then(Value::as_str).ok_or_else(|| io("invalid inspected destination"))?;
            destinations.insert(resolved.to_string());
            targets.insert(key, entry);
            Ok(())
        })?;
    }
    Ok((targets, collisions, destinations.into_iter().collect()))
}

#[cfg(target_arch = "wasm32")]
fn collect(job: &Job<'_>) -> Result<(Map<String, Value>, Vec<String>, Vec<String>)> {
    if job.sink.is_none() {
        return Err(io("web exports require an output sink"));
    }
    Ok((Map::new(), Vec::new(), Vec::new()))
}

pub(crate) fn validate(job: &Job<'_>, authorization: Option<&Value>) -> Result<()> {
    if job.sink.is_some() {
        return Ok(());
    }
    if let Some(authorization) = authorization {
        token_targets(authorization).map_err(io)?;
    }
    visit_paths(job, |path, skipped| {
        if skipped {
            return Ok(());
        }
        #[cfg(not(target_arch = "wasm32"))]
        return crate::pending::validate_approval(path, authorization).map_err(io);
        #[cfg(target_arch = "wasm32")]
        Err(io(format!("web exports require an output sink: {path}")))
    })
}

pub(crate) fn token_targets(authorization: &Value) -> std::io::Result<&Map<String, Value>> {
    if authorization.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err(std::io::Error::other("invalid export authorization; run preflight again"));
    }
    authorization.get("targets").and_then(Value::as_object).ok_or_else(|| std::io::Error::other("invalid export authorization targets"))
}

fn visit_paths(job: &Job<'_>, mut visit: impl FnMut(&str, bool) -> Result<()>) -> Result<()> {
    let comp = job.project.comp(job.comp).ok_or(crate::ExportError::NoComp)?;
    let frames = if job.output.format.is_sequence() { job.settings.frame_count(comp) } else { 1 };
    let alternatives = if job.settings.storage_overflow && job.options.storage.is_some() { job.options.overflow.len() } else { 0 };
    let first = job.settings.first_frame(comp);
    for frame in 0..frames {
        let path = if job.output.format.is_sequence() {
            let frame = i64::try_from(frame).ok().and_then(|frame| first.checked_add(frame)).ok_or_else(|| io("export frame number overflow"))?;
            crate::render_queue::sequence_path(job.path, frame)
        } else {
            job.path.to_string()
        };
        let skipped = job.output.format.is_sequence() && job.settings.skip_existing && crate::out::exists(None, &path);
        visit(&path, skipped)?;
        if alternatives > 0 {
            let name = std::path::Path::new(&path).file_name().ok_or_else(|| io("export output has no filename"))?;
            for directory in &job.options.overflow {
                let alternate = std::path::Path::new(directory).join(name);
                let alternate = alternate.to_str().ok_or_else(|| io("non-Unicode overflow output path"))?;
                visit(alternate, skipped)?;
            }
        }
    }
    Ok(())
}
