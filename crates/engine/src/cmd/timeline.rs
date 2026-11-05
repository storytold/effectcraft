//! Premiere Pro interop (`effectcraft-interchange`): File ▸ Import ▸ Adobe Premiere Pro Project /
//! Timeline Interchange… and File ▸ Export ▸ Adobe Premiere Pro Project…, through the public
//! interchange formats Premiere reads and writes. Export writes Final Cut Pro XML (`.xml`) by
//! default, which Premiere Pro imports (File ▸ Import); FCPXML, OpenTimelineIO and EDL on request.
//! The native `.prproj` format has no public specification and is not read or written.

use std::collections::HashMap;
use std::sync::Arc;

use effectcraft_interchange::{ExportOptions, ImportOptions, PrecompMode, PrerenderMode, Prerendered, TimelineFormat};
use effectcraft_project::render_queue::{AudioOutput, Channels, OutputFormat, OutputModule, ProResProfile, RenderQueueItem, TimeSpan};
use effectcraft_project::{ItemId, LayerId, LayerSource, Project};
use serde_json::{Value, json};

use super::{CommandSpec, always, b_p, bad, comp_id, f_p, has_comp, str_p};
use crate::jobs::TaskCtl;
use crate::{EngineError, Exporter, Result, Session, cmd};

mod staging;

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

/// What a pre-render needs from the session, so it can run on a background thread.
struct PrerenderCx {
    protected: crate::output_guard::Protected,
    exporter: Option<Arc<dyn Exporter>>,
    footage: Arc<dyn effectcraft_render::FootageSource>,
    expr: Option<Arc<dyn effectcraft_render::ExprHost>>,
    accel: Option<Arc<dyn effectcraft_render::Accelerator>>,
    nested_switches: bool,
}

/// Render one layer to ProRes 4444 with alpha over its in–out span; `progress` as the exporter's.
fn prerender(
    cx: &PrerenderCx,
    project: &Project,
    cid: ItemId,
    lid: LayerId,
    path: &str,
    progress: &mut dyn FnMut(u64, u64) -> bool,
) -> std::result::Result<Prerendered, String> {
    let exporter = cx.exporter.as_ref().ok_or("export is not available in this build")?;
    if !exporter.formats().contains(&OutputFormat::ProRes) {
        return Err("ProRes export is not available in this build".into());
    }
    let comp = project.comp(cid).ok_or("no composition")?;
    let l = comp.layer(lid).ok_or("no layer")?;
    let fd = comp.frame_duration();
    let start = comp.frame_rate.snap(l.in_point.max(effectcraft_time::Tick::ZERO));
    let end = l.out_point.min(comp.duration).max(start + fd);
    let q = isolate(project, cid, lid).ok_or("no composition")?;
    let mut item = RenderQueueItem::new(0, cid);
    item.settings.time_span = TimeSpan::Custom { start, end };
    item.output = OutputModule::for_format(OutputFormat::ProRes);
    item.output.channels = Channels::Rgba;
    item.output.prores_profile = ProResProfile::P4444;
    item.output.audio = AudioOutput::Off;
    item.output.output = path.to_string();
    let job = crate::ExportJob {
        project: &q,
        footage: cx.footage.as_ref(),
        expr: cx.expr.as_deref(),
        accel: cx.accel.as_deref(),
        item: &item,
        path,
        storage: None,
        label: format!("Pre-render {}", l.name),
        nested_switches: cx.nested_switches,
    };
    let r = exporter.export(&job, progress)?;
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
    s.refuse_export(&path)?;
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
    let project = s.project.clone();
    let cx = PrerenderCx {
        protected: crate::output_guard::Protected::of(s),
        exporter: s.exporter.clone(),
        footage: s.footage.clone(),
        expr: s.expr.clone(),
        accel: s.accel.clone(),
        nested_switches: s.prefs.general.switches_affect_nested_comps,
    };
    // The pre-renders can take minutes: with `background` (the export dialog) they run as a job
    // in Window ▸ Progress instead of with the window waiting (#2269).
    let wait = !b_p(p, "background").unwrap_or(false);
    s.spawn_task("exportTimeline", format!("Export {}", format.label()), wait, move |ctl| {
        let (bytes, mut result, staged) = export_timeline_work(&cx, &project, cid, &opts, &dir, &stem, ctl)?;
        if let Some(source) = cx.protected.find(std::path::Path::new(&path)) {
            return Err(source.refusal(&path, crate::output_guard::Use::Export));
        }
        result["path"] = json!(path);
        result["toast"] = json!(match format {
            TimelineFormat::Fcp7Xml => format!("Exported {path} (Final Cut Pro XML; import it in Premiere Pro with File ▸ Import)"),
            _ => format!("Exported {path} ({})", format.label()),
        });
        Ok(Box::new(move |s: &mut Session| {
            s.refuse_export(&path)?;
            staging::commit(staged, s)?;
            s.write_output(&path, &bytes)?;
            Ok(result)
        }))
    })
}

/// Pre-render what the timeline can't represent, then build the timeline: its bytes and the
/// command's result (without `path`).
fn export_timeline_work(
    cx: &PrerenderCx,
    project: &Project,
    cid: ItemId,
    opts: &ExportOptions,
    dir: &std::path::Path,
    stem: &str,
    ctl: &TaskCtl,
) -> std::result::Result<(Vec<u8>, Value, Vec<staging::Movie>), String> {
    let mut pre: HashMap<(ItemId, LayerId), Prerendered> = HashMap::new();
    let mut rendered = vec![];
    let mut errors = vec![];
    let mut staged = vec![];
    let plan = effectcraft_interchange::plan_prerender(project, cid, opts);
    if plan.len() > staging::MAX_MOVIES {
        return Err(format!("too many pre-render movies (limit {})", staging::MAX_MOVIES));
    }
    let n = plan.len() as u64;
    for (i, (c, l)) in (0u64..).zip(plan) {
        let comp_name = project.item(c).map(|it| it.name.clone()).unwrap_or_default();
        let layer_name = project.comp(c).and_then(|cc| cc.layer(l)).map(|x| x.name.clone()).unwrap_or_default();
        ctl.message(format!("Pre-render {layer_name}"));
        let file = dir.join(format!("{stem}_{}_{}_{}.mov", safe(&comp_name), safe(&layer_name), l.0)).to_string_lossy().to_string();
        // The movies are outputs too: never over footage, a proxy or the project file.
        if let Some(source) = cx.protected.find(std::path::Path::new(&file)) {
            errors.push(format!("{comp_name} ▸ {layer_name}: {}", source.refusal(&file, crate::output_guard::Use::Export)));
            continue;
        }
        // Progress over all the layers, each one's frames a share of it.
        let mut progress = |done: u64, total: u64| ctl.progress(i * 1000 + done.saturating_mul(1000) / total.max(1), n * 1000);
        let movie = staging::Movie::new(&file)?;
        match prerender(cx, project, c, l, &movie.path, &mut progress) {
            Ok(mut r) => {
                r.path = file;
                rendered.push(r.path.clone());
                pre.insert((c, l), r);
                staged.push(movie);
            }
            Err(_) if ctl.cancelled() => return Err(crate::render_queue::CANCELLED.into()),
            Err(e) => errors.push(format!("{comp_name} ▸ {layer_name}: {e}")),
        }
    }
    let out = effectcraft_interchange::export(project, cid, opts, &pre).map_err(|e| e.to_string())?;
    let mut warnings = out.warnings.clone();
    warnings.extend(errors.iter().map(|e| format!("pre-render failed: {e}")));
    let result = json!({
        "format": opts.format.id(),
        "bytes": out.bytes.len(),
        "sequences": out.sequences,
        "videoTracks": out.video_tracks,
        "audioTracks": out.audio_tracks,
        "clips": out.clips,
        "prerendered": rendered,
        "warnings": warnings,
    });
    Ok((out.bytes, result, staged))
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
            "{comp?, path (Final Cut Pro XML .xml for Premiere Pro or .fcpxml .otio .edl .aaf .omf), format?: xml|fcpxml|otio|edl|aaf|omf, prerender?: none|unsupported|all, precomps?: nest|prerender, background?: bool (pre-render and write as a job, jobs.list / jobs.wait; returns {job})}",
            has_comp,
            export_timeline
        ),
        cmd!("file.timelineFormats", "Timeline Interchange Formats", [], None, "{}", always, timeline_formats),
    ]
}
