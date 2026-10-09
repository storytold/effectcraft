//! Premiere Pro interop (`effectcraft-interchange`): File ▸ Import ▸ Adobe Premiere Pro Project /
//! Timeline Interchange… and File ▸ Export ▸ Adobe Premiere Pro Project…, through the public
//! interchange formats Premiere reads and writes. Export writes Final Cut Pro XML (`.xml`) by
//! default, which Premiere Pro imports (File ▸ Import); FCPXML, OpenTimelineIO and EDL on request.
//! The native `.prproj` format has no public specification and is not read or written.

use std::collections::HashMap;

use effectcraft_interchange::{ExportOptions, ImportOptions, PrecompMode, PrerenderMode, Prerendered, TimelineFormat};
use effectcraft_project::render_queue::{AudioOutput, Channels, OutputFormat, OutputModule, ProResProfile, RenderQueueItem, TimeSpan};
use effectcraft_project::{ItemId, LayerId, LayerSource, Project};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, comp_id, f_p, has_comp, str_p};
use crate::{EngineError, Result, Session, cmd};

fn format_p(p: &Value, cmd: &str) -> Result<Option<TimelineFormat>> {
    match str_p(p, "format") {
        None | Some("") | Some("auto") => Ok(None),
        Some(f) => TimelineFormat::parse(f).map(Some).ok_or_else(|| bad(cmd, format!("unknown format `{f}` (xml, fcpxml, otio, edl, aaf, omf)"))),
    }
}

fn import_timeline(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "file.importTimeline";
    let path = str_p(p, "path").ok_or_else(|| bad(CMD, "missing `path`"))?.to_string();
    if path.to_ascii_lowercase().ends_with(".prproj") {
        return Err(bad(
            CMD,
            "native Premiere Pro projects (.prproj) are not supported: in Premiere Pro use File ▸ Export ▸ Final Cut Pro XML… and import the .xml",
        ));
    }
    let format = format_p(p, CMD)?;
    let bytes = s.services.read_file(&path).map_err(|e| EngineError::Other(format!("cannot read {path}: {e}")))?;
    let fp = std::path::Path::new(&path);
    let opts = ImportOptions {
        base_dir: fp.parent().map(|d| d.to_string_lossy().to_string()).filter(|d| !d.is_empty()),
        name: fp.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Timeline".into()),
        edl_frame_rate: f_p(p, "edlFrameRate"),
    };
    let importer = s.importer.clone();
    let services = s.services.clone();
    let rate = s.sequence_rate();
    let mut probe = move |m: &str| -> Option<effectcraft_project::Footage> {
        if !services.exists(m) {
            return None;
        }
        crate::sequence::probe_path(importer.as_deref()?, services.as_ref(), m, rate).ok()
    };
    let res = s.edit("Import Timeline", None, |proj, st| {
        let r = effectcraft_interchange::import(proj, &bytes, format, &opts, &mut probe).map_err(|e| EngineError::Other(e.to_string()))?;
        st.project_selection = r.comps.clone();
        Ok(r)
    })?;
    if let Some(c) = res.comps.first() {
        s.open_comp(*c);
    }
    let n = res.missing.len();
    s.toast(match n {
        0 => format!("Imported {}", opts.name),
        _ => format!("Imported {} ({n} missing file{})", opts.name, if n == 1 { "" } else { "s" }),
    });
    Ok(json!({
        "format": res.format.map(|f| f.id()),
        "folder": res.folder.0,
        "comps": res.comps.iter().map(|c| c.0).collect::<Vec<_>>(),
        "allComps": res.all_comps.iter().map(|c| c.0).collect::<Vec<_>>(),
        "items": res.items.iter().map(|c| c.0).collect::<Vec<_>>(),
        "missing": res.missing,
        "warnings": res.warnings,
    }))
}

/// File-name-safe text.
fn safe(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    if t.is_empty() { "layer".into() } else { t }
}

/// A copy of the project where comp `cid` shows layer `lid` alone (cameras and lights kept, so 3D
/// layers render as they look in the comp).
fn isolate(project: &Project, cid: ItemId, lid: LayerId) -> Option<Project> {
    let mut q = project.clone();
    let c = q.comp_mut(cid)?;
    for l in &mut c.layers {
        if l.id == lid {
            l.switches.solo = false;
            l.switches.video = true;
        } else if !matches!(l.source, LayerSource::Camera | LayerSource::Light { .. }) {
            l.switches.video = false;
            l.switches.audio = false;
            l.switches.solo = false;
        }
    }
    Some(q)
}

/// Render one layer to ProRes 4444 with alpha over its in–out span.
fn prerender(s: &Session, cid: ItemId, lid: LayerId, path: &str) -> std::result::Result<Prerendered, String> {
    let exporter = s.exporter.clone().ok_or("export is not available in this build")?;
    if !exporter.formats().contains(&OutputFormat::ProRes) {
        return Err("ProRes export is not available in this build".into());
    }
    let comp = s.project.comp(cid).ok_or("no composition")?;
    let l = comp.layer(lid).ok_or("no layer")?;
    let fd = comp.frame_duration();
    let start = comp.frame_rate.snap(l.in_point.max(effectcraft_time::Tick::ZERO));
    let end = l.out_point.min(comp.duration).max(start + fd);
    let q = isolate(&s.project, cid, lid).ok_or("no composition")?;
    let mut item = RenderQueueItem::new(0, cid);
    item.settings.time_span = TimeSpan::Custom { start, end };
    item.output = OutputModule::for_format(OutputFormat::ProRes);
    item.output.channels = Channels::Rgba;
    item.output.prores_profile = ProResProfile::P4444;
    item.output.audio = AudioOutput::Off;
    item.output.output = path.to_string();
    let job = crate::ExportJob {
        project: &q,
        footage: s.footage.as_ref(),
        expr: s.expr.as_deref(),
        accel: s.accel.as_deref(),
        item: &item,
        path,
        storage: None,
        label: format!("Pre-render {}", l.name),
        nested_switches: s.prefs.general.switches_affect_nested_comps,
    };
    let r = exporter.export(&job, &mut |_, _| true)?;
    Ok(Prerendered { path: r.path, width: comp.width, height: comp.height, frame_rate: comp.frame_rate, duration: end - start })
}

fn export_timeline(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "file.exportTimeline";
    let mut path = str_p(p, "path").ok_or_else(|| bad(CMD, "missing `path`"))?.to_string();
    let format = format_p(p, CMD)?.or_else(|| TimelineFormat::from_path(&path)).unwrap_or(TimelineFormat::Fcp7Xml);
    if TimelineFormat::from_path(&path).is_none() || path.to_ascii_lowercase().ends_with(".prproj") {
        if let Some(stem) = path.strip_suffix(".prproj").map(str::to_string) {
            path = stem;
        }
        path = format!("{path}.{}", format.extension());
    }
    let cid = comp_id(s, p)?;
    let prerender_mode = match str_p(p, "prerender") {
        None => match p.get("prerender").and_then(Value::as_bool) {
            Some(false) => PrerenderMode::None,
            _ => PrerenderMode::Unsupported,
        },
        Some(m) => PrerenderMode::parse(m).ok_or_else(|| bad(CMD, "prerender: none|unsupported|all"))?,
    };
    let precomps = match str_p(p, "precomps") {
        None | Some("nest") => PrecompMode::Nest,
        Some("prerender") => PrecompMode::Prerender,
        Some(_) => return Err(bad(CMD, "precomps: nest|prerender")),
    };
    let fp = std::path::Path::new(&path);
    let dir = fp.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let stem = fp.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Timeline".into());
    let relative_to = Some(dir.to_string_lossy().to_string()).filter(|d| !d.is_empty());
    let opts = ExportOptions { format, prerender: prerender_mode, precomps, relative_to };
    // Pre-render what the timeline can't represent.
    let mut pre: HashMap<(ItemId, LayerId), Prerendered> = HashMap::new();
    let mut rendered = vec![];
    let mut errors = vec![];
    for (c, l) in effectcraft_interchange::plan_prerender(&s.project, cid, &opts) {
        let comp_name = s.project.item(c).map(|i| i.name.clone()).unwrap_or_default();
        let layer_name = s.project.comp(c).and_then(|cc| cc.layer(l)).map(|x| x.name.clone()).unwrap_or_default();
        let file = dir.join(format!("{stem}_{}_{}_{}.mov", safe(&comp_name), safe(&layer_name), l.0)).to_string_lossy().to_string();
        match prerender(s, c, l, &file) {
            Ok(r) => {
                rendered.push(r.path.clone());
                pre.insert((c, l), r);
            }
            Err(e) => errors.push(format!("{comp_name} ▸ {layer_name}: {e}")),
        }
    }
    let out = effectcraft_interchange::export(&s.project, cid, &opts, &pre).map_err(|e| EngineError::Other(e.to_string()))?;
    s.services.write_file(&path, &out.bytes).map_err(|e| EngineError::Other(format!("cannot write {path}: {e}")))?;
    let mut warnings = out.warnings.clone();
    warnings.extend(errors.iter().map(|e| format!("pre-render failed: {e}")));
    s.toast(match format {
        TimelineFormat::Fcp7Xml => format!("Exported {path} (Final Cut Pro XML; import it in Premiere Pro with File ▸ Import)"),
        _ => format!("Exported {path} ({})", format.label()),
    });
    Ok(json!({
        "path": path,
        "format": format.id(),
        "bytes": out.bytes.len(),
        "sequences": out.sequences,
        "videoTracks": out.video_tracks,
        "audioTracks": out.audio_tracks,
        "clips": out.clips,
        "prerendered": rendered,
        "warnings": warnings,
    }))
}

/// The formats, for agents and the export dialog.
fn timeline_formats(_: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!(
        TimelineFormat::ALL
            .iter()
            .map(|f| json!({"id": f.id(), "label": f.label(), "extension": f.extension(), "import": true, "export": true}))
            .collect::<Vec<_>>()
    ))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "file.importTimeline",
            "Adobe Premiere Pro Project...",
            ["File", "Import"],
            None,
            "{path (.xml Final Cut Pro XML from Premiere Pro or .fcpxml .otio .edl .aaf .omf), format?: auto|xml|fcpxml|otio|edl|aaf|omf, edlFrameRate?: number}",
            always,
            import_timeline
        ),
        cmd!(
            "file.exportTimeline",
            "Adobe Premiere Pro Project...",
            ["File", "Export"],
            None,
            "{comp?, path (Final Cut Pro XML .xml for Premiere Pro or .fcpxml .otio .edl .aaf .omf), format?: xml|fcpxml|otio|edl|aaf|omf, prerender?: none|unsupported|all, precomps?: nest|prerender}",
            has_comp,
            export_timeline
        ),
        cmd!("file.timelineFormats", "Timeline Interchange Formats", [], None, "{}", always, timeline_formats),
    ]
}
