//! `render`: a comp to a movie, an image sequence or an audio file, or the project's Render Queue.

use serde_json::json;

use crate::args::RenderArgs;
use crate::commands::{Sel, emit, session};
use crate::error::{CliError, Result};
use crate::io::Io;
use crate::template;

/// A relative `--out` is relative to the working directory, like `--project` and every other CLI
/// path. The Render Queue alone would resolve it against the project's folder.
fn from_cwd(path: &str) -> Result<String> {
    std::path::absolute(path).map(|p| p.to_string_lossy().into_owned()).map_err(|source| CliError::ResolveOut { path: path.to_owned(), source })
}

/// `render`: queue `--comp` (or the active comp) with the given settings unless `--queue`, then
/// render the queue with a progress line on stderr. Fails if any item fails.
pub fn render(a: &RenderArgs, json_out: bool, io: &mut Io) -> Result {
    use effectcraft_engine::project::render_queue::{OutputFormat, RenderStatus};
    let sel = Sel { project: a.project.as_deref(), demo: a.source.demo, empty: a.source.empty, bridge: None, gpu: a.gpu.gpu, autosave: false };
    if let (Some(path), Some(out)) = (&a.template.template, &a.out) {
        template::check_output(out, path, a.template.values.as_deref())?;
    }
    let mut s = session(&sel)?;
    if let Some(path) = &a.template.template {
        template::prepare(&mut s, path, a.template.values.as_deref())?;
    } else if let Some(p) = &a.project {
        s.execute("file.open", json!({"path": p}))?;
    } else if a.source.demo || !a.source.empty {
        s.execute("file.openDemoProject", json!({}))?;
    }
    if !a.queue {
        let Some(out) = &a.out else { return Err(crate::error::UsageError::Missing("render --out FILE").into()) };
        let out = &from_cwd(out)?;
        let mut p = json!({"output": out});
        if let Some(c) = &a.comp {
            p["comp"] = json!(c);
        }
        let ext = std::path::Path::new(out).extension().map(|e| e.to_string_lossy().to_string());
        if let Some(f) = a.format.clone().or(ext) {
            p["format"] = json!(f);
        }
        match (a.start, a.end) {
            (None, None) => p["timeSpan"] = json!(if a.work_area { "workArea" } else { "comp" }),
            (start, end) => {
                p["start"] = json!(start.unwrap_or(0.0));
                if let Some(end) = end {
                    p["end"] = json!(end);
                }
            }
        }
        if let Some(v) = a.fps {
            p["frameRate"] = json!(v);
        }
        if let Some(r) = a.resolution.as_ref().or(a.scale.as_ref()) {
            p["resolution"] = r.parse::<f64>().map(|v| json!(v)).unwrap_or(json!(r));
        }
        for (value, key) in [(&a.quality, "quality"), (&a.channels, "channels"), (&a.prores, "proresProfile"), (&a.audio, "audio")] {
            if let Some(v) = value {
                p[key] = json!(v);
            }
        }
        if let Some(v) = a.jpeg_quality {
            p["quality"] = json!(v);
        }
        if let Some(v) = a.bitrate {
            p["bitrate"] = json!(v);
            // VP9 WebM encodes by quality unless told to target the bitrate (#440).
            // The format parser decides what is WebM, so its aliases (`vp9`, `webmvp9opus`) count.
            if p["format"].as_str().and_then(OutputFormat::from_name) == Some(OutputFormat::WebM) {
                p["webmBitrate"] = json!(true);
            }
        }
        for (value, key) in
            [(&a.profile, "profile"), (&a.level, "level"), (&a.rate_control, "rateControl"), (&a.webm_codec, "webmCodec"), (&a.opus_app, "opusApplication")]
        {
            if let Some(v) = value {
                p[key] = json!(v);
            }
        }
        for (value, key) in [(a.video_quality, "quality"), (a.keyint, "keyframeInterval"), (a.audio_bitrate, "audioBitrate")] {
            if let Some(v) = value {
                p[key] = json!(v);
            }
        }
        // Render exactly this item: unqueue whatever the project's queue already holds.
        for k in 0..s.project.render_queue.len() {
            let _ = s.execute("renderQueue.setRender", json!({"index": k + 1, "render": false}));
        }
        let r = s.execute("renderQueue.add", p)?;
        note!(
            io,
            "{} → {} ({}×{}, {} frames, {})",
            r["compName"].as_str().unwrap_or("?"),
            r["outputPath"].as_str().unwrap_or("?"),
            r["width"],
            r["height"],
            r["frames"],
            r["outputModuleSummary"].as_str().unwrap_or("")
        );
    }
    let attempted: std::collections::HashSet<u64> = s.project.render_queue.iter().filter(|i| i.is_queued()).map(|i| i.id).collect();
    s.execute("renderQueue.render", json!({"wait": false}))?;
    let tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
    // `--progress`: one line per update for a program reading stderr (a terminal gets the live line).
    let lines = a.progress && !tty;
    let (mut last_line, mut last_done) = (std::time::Instant::now(), u64::MAX);
    while s.is_rendering() {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if lines
            && last_line.elapsed() >= std::time::Duration::from_millis(250)
            && let Some(p) = s.render_progress()
            && p.done != last_done
        {
            last_line = std::time::Instant::now();
            last_done = p.done;
            let left = p.remaining.map_or_else(|| "-".to_string(), |r| format!("{r:.1}"));
            note!(io, "progress {} {} {:.1} {left}", p.done, p.total, p.item_elapsed);
        }
        if tty && let Some(p) = s.render_progress() {
            let left = p.remaining.map(|r| format!(", ~{r:.1}s left")).unwrap_or_default();
            io.raw_err(format_args!(
                "\r  [{}/{}] frame {}/{}  {:.1}s{left}\x1b[K",
                (p.items_done + 1).min(p.items_total),
                p.items_total,
                p.done,
                p.total,
                p.item_elapsed
            ));
        }
        s.poll_render();
    }
    s.poll_render();
    if tty {
        note!(io, "");
    }
    let mut results = vec![];
    let mut failed = vec![];
    for it in s.project.render_queue.iter().filter(|i| attempted.contains(&i.id)) {
        let name = s.project.item(it.comp).map(|i| i.name.clone()).unwrap_or_else(|| "?".into());
        match &it.status {
            RenderStatus::Done => {
                note!(io, "done: {name} → {} in {:.2}s", it.last_output.as_deref().unwrap_or("?"), it.render_time.unwrap_or(0.0));
                results.push(json!({"comp": name, "output": it.last_output, "seconds": it.render_time}));
            }
            RenderStatus::Failed(e) => failed.push(format!("{name}: {e}")),
            other => failed.push(format!("{name}: {}", other.label())),
        }
    }
    if !failed.is_empty() {
        return Err(CliError::Items(failed.join("; ")));
    }
    if json_out {
        emit(io, &json!({"rendered": results}), true);
    }
    Ok(())
}
