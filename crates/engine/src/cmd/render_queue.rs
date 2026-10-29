//! Render Queue: Composition ▸ Add to Render Queue, the queue's Render Settings / Output Module /
//! Output To / Log, Render and Stop, Notify, storage overflow folders, and the Render Settings /
//! Output Module templates (Edit ▸ Templates).
//!
//! Items are addressed by `item` (the stable id from `renderQueue.add` / `renderQueue.list`) or
//! `index` (1-based, the # column).

use effectcraft_project::render_queue::{
    AlphaMode, AudioFormat, AudioOutput, Channels, ColorDepth, CurrentOrOff, DEFAULT_SEQUENCE_TEMPLATE, DEFAULT_TEMPLATE, EffectsMode, FieldRender,
    OutputFormat, OutputModule, PostRenderAction, ProResProfile, Pulldown, RESIZE_PRESETS, RenderLog, RenderQuality, RenderQueueItem, RenderSettings,
    RenderStatus, ResizeQuality, SwitchOverride, TimeSpan, post_render_parse, without_project_extension,
};
use effectcraft_project::render_templates::{RenderTemplates, TemplateKind, TemplateSlot};
use effectcraft_time::{FrameRate, Tick};
use serde_json::{Value, json};

use super::{CommandSpec, always, b_p, bad, comp_id, f_p, has_comp, str_p};
use crate::{EngineError, Result, Session, cmd, query};

fn can_add(s: &Session) -> std::result::Result<(), String> {
    not_rendering(s)?;
    if s.comp_for_queue(&Value::Null).is_ok() { Ok(()) } else { Err("select or open a composition".into()) }
}
fn not_rendering(s: &Session) -> std::result::Result<(), String> {
    if s.is_rendering() { Err("the render queue is rendering".into()) } else { Ok(()) }
}
fn has_items(s: &Session) -> std::result::Result<(), String> {
    not_rendering(s)?;
    if s.project.render_queue.is_empty() { Err("the render queue is empty".into()) } else { Ok(()) }
}
fn has_queued(s: &Session) -> std::result::Result<(), String> {
    not_rendering(s)?;
    if s.exporter.is_none() {
        return Err("export is not available in this build".into());
    }
    if s.project.render_queue.iter().any(RenderQueueItem::is_queued) { Ok(()) } else { Err("nothing is queued".into()) }
}
fn rendering(s: &Session) -> std::result::Result<(), String> {
    if s.is_rendering() { Ok(()) } else { Err("not rendering".into()) }
}

/// Index of the item named by `item` (id) or `index` (1-based).
fn item_index(s: &Session, p: &Value, cmd: &str) -> Result<usize> {
    let q = &s.project.render_queue;
    if let Some(id) = p.get("item").and_then(Value::as_u64) {
        return q.iter().position(|i| i.id == id).ok_or_else(|| bad(cmd, format!("no render queue item {id}")));
    }
    if let Some(n) = p.get("index").and_then(Value::as_u64) {
        return (n as usize).checked_sub(1).filter(|i| *i < q.len()).ok_or_else(|| bad(cmd, format!("no render queue item #{n}")));
    }
    if q.len() == 1 {
        return Ok(0);
    }
    Err(bad(cmd, "pass `item` (id) or `index` (1-based)"))
}

fn time_p(p: &Value, k: &str) -> Option<Tick> {
    f_p(p, k).map(Tick::from_seconds_f64)
}

/// Parse an enum parameter.
fn enum_p<T>(p: &Value, k: &str, cmd: &str, parse: fn(&str) -> Option<T>, help: &str) -> Result<Option<T>> {
    match p.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(v)) => parse(v).map(Some).ok_or_else(|| bad(cmd, format!("{k}: {help}"))),
        Some(Value::Bool(b)) => parse(if *b { "on" } else { "off" }).map(Some).ok_or_else(|| bad(cmd, format!("{k}: {help}"))),
        Some(Value::Number(n)) => parse(&n.to_string()).map(Some).ok_or_else(|| bad(cmd, format!("{k}: {help}"))),
        Some(_) => Err(bad(cmd, format!("{k}: {help}"))),
    }
}

/// Apply Render Settings parameters (`template` first, then the individual settings). Any
/// individual change makes the settings "Custom".
fn apply_settings(rs: &mut RenderSettings, templates: &RenderTemplates, p: &Value, cmd: &str) -> Result<bool> {
    let mut any = false;
    if let Some(t) = str_p(p, "template") {
        *rs = templates.render_settings(t).ok_or_else(|| bad(cmd, format!("no Render Settings template “{t}”")))?;
        any = true;
    }
    let before = rs.clone();
    if let Some(q) = str_p(p, "quality") {
        rs.quality = match q.to_ascii_lowercase().as_str() {
            "best" => RenderQuality::Best,
            "draft" => RenderQuality::Draft,
            _ => return Err(bad(cmd, "quality: best|draft")),
        };
    }
    match p.get("resolution") {
        Some(Value::String(r)) => {
            rs.resolution = match r.to_ascii_lowercase().as_str() {
                "full" => 1.0,
                "half" => 0.5,
                "third" => 1.0 / 3.0,
                "quarter" => 0.25,
                _ => return Err(bad(cmd, "resolution: full|half|third|quarter|<scale>")),
            };
        }
        Some(Value::Number(n)) => {
            rs.resolution = n.as_f64().unwrap_or(1.0).clamp(0.01, 4.0);
        }
        _ => {}
    }
    let (start, mut end) = (time_p(p, "start"), time_p(p, "end"));
    if let Some(d) = time_p(p, "duration") {
        // Time Span ▸ Custom ▸ Duration: from the given (or current custom) start.
        let a = start.unwrap_or(match rs.time_span {
            TimeSpan::Custom { start, .. } => start,
            _ => Tick::ZERO,
        });
        end = Some(a + d);
    }
    if let Some(ts) = str_p(p, "timeSpan") {
        rs.time_span = match ts.to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
            "workarea" | "workareaonly" => TimeSpan::WorkArea,
            "comp" | "lengthofcomp" | "full" => TimeSpan::LengthOfComp,
            "custom" => TimeSpan::Custom { start: start.unwrap_or(Tick::ZERO), end: end.unwrap_or(Tick::from_seconds_f64(1.0)) },
            _ => return Err(bad(cmd, "timeSpan: workArea|comp|custom")),
        };
    } else if start.is_some() || end.is_some() {
        let (a, b) = match rs.time_span {
            TimeSpan::Custom { start, end } => (start, end),
            _ => (Tick::ZERO, Tick(i64::MAX / 4)),
        };
        rs.time_span = TimeSpan::Custom { start: start.unwrap_or(a), end: end.unwrap_or(b) };
    }
    if let TimeSpan::Custom { start, end } = rs.time_span
        && end <= start
    {
        return Err(bad(cmd, "end must be after start"));
    }
    if let Some(u) = str_p(p, "proxyUse") {
        rs.proxy_use = effectcraft_project::render_queue::ProxyUse::parse(u).ok_or_else(|| bad(cmd, "proxyUse: current|all|comp|none"))?;
    }
    match p.get("frameRate") {
        Some(Value::Null) => {
            rs.frame_rate = None;
        }
        Some(v) => {
            let f = v.as_f64().filter(|f| *f > 0.0 && *f <= 1000.0).ok_or_else(|| bad(cmd, "frameRate: fps > 0 or null (comp rate)"))?;
            rs.frame_rate = Some(FrameRate::from_f64(f));
        }
        None => {}
    }
    match p.get("motionBlur") {
        Some(Value::Bool(b)) => {
            rs.motion_blur = *b;
            if *b && rs.motion_blur_mode == SwitchOverride::OffForAll {
                rs.motion_blur_mode = SwitchOverride::Current;
            }
        }
        Some(_) => {
            let m = enum_p(p, "motionBlur", cmd, SwitchOverride::parse, "current|onForChecked|offForAll")?.unwrap_or_default();
            rs.motion_blur = m != SwitchOverride::OffForAll;
            rs.motion_blur_mode = m;
        }
        None => {}
    }
    if let Some(m) = enum_p(p, "frameBlending", cmd, SwitchOverride::parse, "current|onForChecked|offForAll")? {
        rs.frame_blending = m;
    }
    if let Some(f) = enum_p(p, "fieldRender", cmd, FieldRender::parse, "off|upper|lower")? {
        rs.field_render = f;
    }
    if let Some(f) = enum_p(p, "pulldown", cmd, Pulldown::parse, "off|WSSWW|SSWWW|SWWWS|WWWSS|WWSSW")? {
        rs.pulldown = f;
    }
    if let Some(e) = enum_p(p, "effects", cmd, EffectsMode::parse, "current|allOn|allOff")? {
        rs.effects = e;
    }
    if let Some(e) = enum_p(p, "solo", cmd, CurrentOrOff::parse, "current|allOff")? {
        rs.solo = e;
    }
    if let Some(e) = enum_p(p, "guideLayers", cmd, CurrentOrOff::parse, "current|allOff")? {
        rs.guide_layers = e;
    }
    if let Some(e) = enum_p(p, "colorDepth", cmd, ColorDepth::parse, "current|8|16|32")? {
        rs.color_depth = e;
    }
    if let Some(b) = b_p(p, "skipExisting") {
        rs.skip_existing = b;
    }
    if let Some(b) = b_p(p, "storageOverflow") {
        rs.storage_overflow = b;
    }
    if *rs != before {
        rs.name = "Custom".into();
        any = true;
    }
    Ok(any)
}

/// Apply a template to a module, keeping its output path (with the extension fixed).
fn apply_om_template(om: &mut OutputModule, t: &OutputModule) {
    let keep = om.output.clone();
    let old = om.format;
    *om = t.clone();
    om.output = keep;
    let f = om.format;
    om.format = old;
    om.set_format(f);
    om.channels = t.channels;
    om.name = t.name.clone();
}

/// The `format` parameter's values.
const FORMATS: &str = "format: h264|hevc|av1|prores|uncompressed|webm|png|jpeg|tiff|exr|tga|gif|wav|aiff";

/// `channels` (refusing RGBA where the format has no alpha) and `color`/`alphaMode`.
fn apply_channels_and_alpha(om: &mut OutputModule, p: &Value, cmd: &str) -> Result<()> {
    if let Some(c) = str_p(p, "channels") {
        om.channels = match c.to_ascii_lowercase().replace([' ', '+'], "").as_str() {
            "rgb" => Channels::Rgb,
            "rgba" | "rgbalpha" => Channels::Rgba,
            "alpha" | "alphaonly" => Channels::Alpha,
            _ => return Err(bad(cmd, "channels: rgb|rgba|alpha")),
        };
        if om.channels == Channels::Rgba && !om.supports_alpha() {
            let what = match om.format {
                OutputFormat::WebM => format!("{} WebM has no alpha channel (VP9 WebM has)", om.webm_codec.label()),
                f => format!("{} has no alpha channel", f.label()),
            };
            return Err(bad(cmd, what));
        }
    }
    if let Some(c) = str_p(p, "color").or(str_p(p, "alphaMode")) {
        om.alpha_mode = match c.to_ascii_lowercase().replace([' ', '(', ')'], "").as_str() {
            "straight" | "straightunmatted" => AlphaMode::Straight,
            "premultiplied" | "premultipliedmatted" | "premul" => AlphaMode::Premultiplied,
            _ => return Err(bad(cmd, "color: straight|premultiplied")),
        };
    }
    Ok(())
}

/// `crop` plus the flat dialog keys (`cropTop`… and `resizeWidth`/`resizeHeight`).
fn apply_crop(om: &mut OutputModule, roi: Option<[f64; 4]>, p: &Value, cmd: &str) -> Result<()> {
    match p.get("crop") {
        Some(Value::Bool(b)) => om.crop.enabled = *b,
        Some(Value::Object(c)) => {
            om.crop.enabled = c.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            for (k, v) in [("top", &mut om.crop.top), ("left", &mut om.crop.left), ("bottom", &mut om.crop.bottom), ("right", &mut om.crop.right)] {
                if let Some(n) = c.get(k).and_then(Value::as_i64) {
                    *v = n.clamp(-20_000, 20_000) as i32;
                }
            }
            if let Some(u) = c.get("useRoi").and_then(Value::as_bool) {
                om.crop.use_roi = u;
                if u {
                    om.crop.roi = c.get("roi").and_then(|r| serde_json::from_value::<[f64; 4]>(r.clone()).ok()).or(roi);
                    if om.crop.roi.is_none() {
                        return Err(bad(
                            cmd,
                            "Use Region of Interest: the composition has no region of interest (View ▸ Region of Interest) — pass crop.roi [x, y, w, h]",
                        ));
                    }
                }
            }
        }
        Some(Value::Null) | None => {}
        Some(_) => return Err(bad(cmd, "crop: bool or {enabled?, useRoi?, roi?: [x,y,w,h], top?, left?, bottom?, right?}")),
    }
    // Flat keys (dialog forms): cropTop/cropLeft/cropBottom/cropRight, resizeWidth/resizeHeight.
    for (k, v) in [("cropTop", &mut om.crop.top), ("cropLeft", &mut om.crop.left), ("cropBottom", &mut om.crop.bottom), ("cropRight", &mut om.crop.right)] {
        if let Some(n) = p.get(k).and_then(Value::as_f64) {
            *v = (n.round() as i64).clamp(-20_000, 20_000) as i32;
            om.crop.enabled = true;
            om.crop.use_roi = false;
        }
    }
    for (k, v) in [("resizeWidth", &mut om.resize.width), ("resizeHeight", &mut om.resize.height)] {
        if let Some(n) = p.get(k).and_then(Value::as_f64) {
            *v = (n.round() as u32).clamp(1, 30_000);
            om.resize.enabled = true;
        }
    }
    Ok(())
}

/// `resize` (bool or object).
fn apply_resize(om: &mut OutputModule, p: &Value, cmd: &str) -> Result<()> {
    match p.get("resize") {
        Some(Value::Bool(b)) => om.resize.enabled = *b,
        Some(Value::Object(r)) => {
            om.resize.enabled = r.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            if let Some(name) = r.get("preset").and_then(Value::as_str) {
                let (_, w, h) =
                    RESIZE_PRESETS.iter().find(|(n, _, _)| n.eq_ignore_ascii_case(name)).ok_or_else(|| bad(cmd, format!("no resize preset “{name}”")))?;
                (om.resize.width, om.resize.height) = (*w, *h);
                om.resize.lock_aspect = false;
            }
            if let Some(l) = r.get("lockAspect").and_then(Value::as_bool) {
                om.resize.lock_aspect = l;
            }
            if let Some(w) = r.get("width").and_then(Value::as_u64) {
                om.resize.width = (w as u32).clamp(1, 30_000);
            }
            if let Some(h) = r.get("height").and_then(Value::as_u64) {
                om.resize.height = (h as u32).clamp(1, 30_000);
            }
            if let Some(q) = r.get("quality").and_then(Value::as_str) {
                om.resize.quality = match q.to_ascii_lowercase().as_str() {
                    "low" | "bilinear" => ResizeQuality::Low,
                    "high" | "bicubic" => ResizeQuality::High,
                    _ => return Err(bad(cmd, "resize.quality: low|high")),
                };
            }
        }
        Some(Value::Null) | None => {}
        Some(_) => return Err(bad(cmd, "resize: bool or {enabled?, preset?, width?, height?, lockAspect?, quality?: low|high}")),
    }
    Ok(())
}

/// Apply Output Module parameters (`template` first). `roi`: the viewer's region of interest
/// (captured by `crop.useRoi`).
fn apply_output(om: &mut OutputModule, templates: &RenderTemplates, roi: Option<[f64; 4]>, p: &Value, cmd: &str) -> Result<bool> {
    let mut any = false;
    if let Some(t) = str_p(p, "template") {
        let t = templates.output_module(t).ok_or_else(|| bad(cmd, format!("no Output Module template “{t}”")))?;
        apply_om_template(om, &t);
        any = true;
    }
    let before = om.clone();
    if let Some(f) = str_p(p, "format") {
        let f = OutputFormat::from_name(f).ok_or_else(|| bad(cmd, FORMATS))?;
        om.set_format(f);
    }
    // Before Channels: the WebM codec decides whether alpha can be written, so an explicit
    // `channels: rgba` with `webmCodec: av1` is refused rather than dropped (#166).
    any |= apply_codec_options(om, p, cmd)?;
    apply_channels_and_alpha(om, p, cmd)?;
    if let Some(q) = f_p(p, "quality") {
        om.quality = q.clamp(1.0, 100.0) as u8;
    }
    if let Some(b) = f_p(p, "bitrate") {
        om.bitrate_kbps = b.clamp(50.0, 500_000.0) as u32;
    }
    if let Some(b) = b_p(p, "webmBitrate") {
        om.webm_bitrate = b;
    }
    if let Some(k) = p.get("keyframeInterval").and_then(Value::as_u64) {
        om.keyframe_interval = k.min(100_000) as u32;
    }
    if let Some(pp) = str_p(p, "proresProfile") {
        om.prores_profile = ProResProfile::from_name(pp).ok_or_else(|| bad(cmd, "proresProfile: proxy|lt|standard|hq|4444|4444xq"))?;
    }
    apply_crop(om, roi, p, cmd)?;
    apply_resize(om, p, cmd)?;
    if let Some(a) = p.get("audio") {
        om.audio = match a {
            Value::Bool(true) => AudioOutput::On,
            Value::Bool(false) => AudioOutput::Off,
            Value::String(s) => match s.to_ascii_lowercase().as_str() {
                "auto" => AudioOutput::Auto,
                "on" => AudioOutput::On,
                "off" => AudioOutput::Off,
                _ => return Err(bad(cmd, "audio: auto|on|off")),
            },
            _ => return Err(bad(cmd, "audio: auto|on|off")),
        };
    }
    if let Some(r) = p.get("sampleRate").and_then(Value::as_u64) {
        om.audio_sample_rate = (r as u32).clamp(8_000, 192_000);
    }
    match p.get("audioChannels") {
        Some(Value::Number(n)) => om.audio_channels = if n.as_u64() == Some(1) { 1 } else { 2 },
        Some(Value::String(c)) => {
            om.audio_channels = match c.to_ascii_lowercase().as_str() {
                "mono" | "1" => 1,
                "stereo" | "2" => 2,
                _ => return Err(bad(cmd, "audioChannels: mono|stereo")),
            }
        }
        _ => {}
    }
    if let Some(f) = enum_p(p, "audioFormat", cmd, AudioFormat::parse, "16|24|32 (float)")? {
        om.audio_format = f;
    }
    if let Some(l) = b_p(p, "loop") {
        om.gif_loop = l;
    }
    if let Some(l) = b_p(p, "includeProjectLink") {
        om.include_project_link = l;
    }
    // (Without the `.ecproj` a save dialog filtered to projects appended, #293.)
    if let Some(o) = str_p(p, "output").or(str_p(p, "path")).map(without_project_extension) {
        om.output = o.to_string();
        // Only an extension the format doesn't write changes it: `.mp4` is H.264's, HEVC's and AV1's.
        if let Some(f) = OutputFormat::from_path(o).filter(|f| f.extension() != om.format.extension() && !o.contains("[fileExtension]")) {
            if p.get("format").is_some() {
                // An explicit format wins and the extension follows it, as choosing a format in
                // the Output Module renames Output To (#154: `--format hevc --out x.mp4`).
                om.set_format(om.format);
            } else {
                // `out.mov` with an H.264 module → ProRes, like picking a file type in Output To.
                let keep = om.output.clone();
                om.set_format(f);
                om.output = keep;
            }
        }
        // The format's extension exactly once (`Comp 2.mov.mov` → `Comp 2.mov`).
        om.output = om.format.with_extension(&om.output);
    }
    if *om != before {
        // Output To alone doesn't make the module custom.
        let mut o = om.clone();
        o.output = before.output.clone();
        if o != before {
            om.name = "Custom".into();
        }
        any = true;
    }
    Ok(any)
}

/// HEVC / AV1 codec options, the WebM video codec and the Opus audio options.
fn apply_codec_options(om: &mut OutputModule, p: &Value, cmd: &str) -> Result<bool> {
    use effectcraft_project::render_queue::{CodecProfile, OpusApplication, RateControlMode, VideoCodecOptions, WebmVideoCodec};
    let mut any = false;
    if let Some(c) = str_p(p, "webmCodec") {
        om.webm_codec = match c.to_ascii_lowercase().as_str() {
            "vp9" => WebmVideoCodec::Vp9,
            "av1" => WebmVideoCodec::Av1,
            _ => return Err(bad(cmd, "webmCodec: vp9|av1")),
        };
        if !om.supports_alpha() {
            om.channels = Channels::Rgb;
        }
        any = true;
    }
    if let Some(v) = str_p(p, "profile") {
        om.codec.profile = CodecProfile::from_name(v).ok_or_else(|| bad(cmd, "profile: main|main10"))?;
        any = true;
    }
    match p.get("level") {
        Some(Value::Null) => {
            om.codec.level = None;
            any = true;
        }
        Some(v) => {
            let s = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            om.codec.level = VideoCodecOptions::parse_level(&s).ok_or_else(|| bad(cmd, "level: auto|null or a level like 4.1"))?;
            any = true;
        }
        None => {}
    }
    if let Some(v) = str_p(p, "rateControl") {
        om.codec.rate_control = RateControlMode::from_name(v).ok_or_else(|| bad(cmd, "rateControl: bitrate|quality"))?;
        any = true;
    }
    if let Some(q) = f_p(p, "quality") {
        om.codec.quality = q.clamp(1.0, 100.0) as u8;
    }
    if let Some(b) = f_p(p, "audioBitrate") {
        om.opus_bitrate_kbps = b.clamp(6.0, 510.0) as u32;
        any = true;
    }
    if let Some(a) = str_p(p, "opusApplication") {
        om.opus_application = match a.to_ascii_lowercase().as_str() {
            "audio" | "music" => OpusApplication::Audio,
            "voip" | "voice" | "speech" => OpusApplication::Voip,
            _ => return Err(bad(cmd, "opusApplication: audio|voice")),
        };
        any = true;
    }
    Ok(any)
}

/// `module` (1-based; 1 = the first output module).
fn module_p(s: &Session, i: usize, p: &Value, cmd: &str) -> Result<usize> {
    let n = p.get("module").and_then(Value::as_u64).unwrap_or(1) as usize;
    let have = s.project.render_queue[i].extra_outputs.len() + 1;
    if n == 0 || n > have {
        return Err(bad(cmd, format!("module must be 1..={have}")));
    }
    Ok(n - 1)
}

fn module_ref(it: &RenderQueueItem, m: usize) -> &OutputModule {
    if m == 0 { &it.output } else { &it.extra_outputs[m - 1] }
}

fn module_mut(it: &mut RenderQueueItem, m: usize) -> &mut OutputModule {
    if m == 0 { &mut it.output } else { &mut it.extra_outputs[m - 1] }
}

/// A finished item that is edited goes back to Queued (AE re-queues on change).
fn requeue(it: &mut RenderQueueItem) {
    if it.render && !matches!(it.status, RenderStatus::Rendering) {
        it.status = RenderStatus::Queued;
    }
}

fn item_json(s: &Session, it: &RenderQueueItem, index: usize) -> Value {
    let comp = s.project.comp(it.comp);
    let name = s.project.item(it.comp).map(|i| i.name.clone());
    let (w, h) = comp.map(|c| it.output.output_size(c, &it.settings)).unwrap_or((0, 0));
    let frames = comp.map(|c| it.settings.frame_count(c)).unwrap_or(0);
    let mut v = serde_json::to_value(it).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.insert("index".into(), json!(index + 1));
        o.insert("compName".into(), json!(name));
        o.insert("statusLabel".into(), json!(it.status.label()));
        o.insert("outputPath".into(), json!(s.resolve_output(it)));
        o.insert("renderSettingsSummary".into(), json!(it.settings.summary()));
        o.insert("outputModuleSummary".into(), json!(it.output.summary()));
        let extra: Vec<Value> = it
            .extra_outputs
            .iter()
            .map(|om| {
                let mut c = it.clone();
                c.output = om.clone();
                json!({"summary": om.summary(), "outputPath": s.resolve_output(&c)})
            })
            .collect();
        o.insert("outputModules".into(), json!(it.extra_outputs.len() + 1));
        o.insert("extraOutputs".into(), json!(extra));
        o.insert("postRenderAction".into(), json!(it.post_render.label()));
        o.insert("logLabel".into(), json!(it.log.label()));
        if let Some(p) = s.resolve_output(it) {
            o.insert("logPath".into(), json!(effectcraft_project::render_queue::log_path(&p)));
        }
        o.insert("width".into(), json!(w));
        o.insert("height".into(), json!(h));
        o.insert("frames".into(), json!(frames));
    }
    v
}

fn add(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = s.comp_for_queue(p)?;
    let mut it = RenderQueueItem::new(0, cid);
    // New items start from the Movie Default templates (Edit ▸ Templates).
    let templates = s.project.render_templates.clone();
    it.settings = templates.default_render_settings(TemplateSlot::Movie);
    it.output = templates.default_output_module(TemplateSlot::Movie);
    if let Some(f) = str_p(p, "format") {
        let f = OutputFormat::from_name(f).ok_or_else(|| bad("renderQueue.add", FORMATS))?;
        if f != it.output.format {
            it.output = OutputModule::for_format(f);
        }
    }
    apply_settings(&mut it.settings, &templates, p, "renderQueue.add")?;
    // "template" belongs to Render Settings on add, not Output Module.
    let mut output_params = p.clone();
    if let Some(object) = output_params.as_object_mut() {
        object.remove("template");
    }
    apply_output(&mut it.output, &templates, s.state.region_of_interest, &output_params, "renderQueue.add")?;
    if let Some(l) = enum_p(p, "log", "renderQueue.add", RenderLog::parse, "errorsOnly|plusSettings|plusPerFrameInfo")? {
        it.log = l;
    }
    let id = s.edit("Add to Render Queue", None, |proj, _| {
        it.id = proj.render_queue.iter().map(|i| i.id).max().unwrap_or(0) + 1;
        let id = it.id;
        proj.render_queue.push(it);
        Ok(id)
    })?;
    let idx = s.project.render_queue.len() - 1;
    let mut v = item_json(s, &s.project.render_queue[idx], idx);
    v["item"] = json!(id);
    Ok(v)
}

fn remove(s: &mut Session, p: &Value) -> Result<Value> {
    let i = item_index(s, p, "renderQueue.remove")?;
    s.edit("Remove from Render Queue", None, |proj, _| {
        proj.render_queue.remove(i);
        Ok(())
    })?;
    Ok(json!({"remaining": s.project.render_queue.len()}))
}

fn set_render(s: &mut Session, p: &Value) -> Result<Value> {
    let i = item_index(s, p, "renderQueue.setRender")?;
    let on = b_p(p, "render").unwrap_or(!s.project.render_queue[i].render);
    s.edit("Render Queue: Render", None, |proj, _| {
        let it = &mut proj.render_queue[i];
        it.render = on;
        it.status = if on { RenderStatus::Queued } else { RenderStatus::Unqueued };
        Ok(())
    })?;
    Ok(json!({"render": on}))
}

fn set_render_settings(s: &mut Session, p: &Value) -> Result<Value> {
    let i = item_index(s, p, "renderQueue.setRenderSettings")?;
    let mut rs = s.project.render_queue[i].settings.clone();
    if !apply_settings(&mut rs, &s.project.render_templates, p, "renderQueue.setRenderSettings")? {
        return Err(bad("renderQueue.setRenderSettings", "nothing to change"));
    }
    s.edit("Render Settings", None, |proj, _| {
        let it = &mut proj.render_queue[i];
        it.settings = rs;
        requeue(it);
        Ok(())
    })?;
    Ok(item_json(s, &s.project.render_queue[i], i))
}

fn set_output_module(s: &mut Session, p: &Value) -> Result<Value> {
    let i = item_index(s, p, "renderQueue.setOutputModule")?;
    let m = module_p(s, i, p, "renderQueue.setOutputModule")?;
    let mut om = module_ref(&s.project.render_queue[i], m).clone();
    let changed = apply_output(&mut om, &s.project.render_templates, s.state.region_of_interest, p, "renderQueue.setOutputModule")?;

    // When aspect is locked, an explicit height-only edit must drive the size.
    // frame_size() derives height from width; calculate the matching width from the
    // cropped render bounds before storing the updated module.
    let width_given =
        p.get("resizeWidth").is_some() || p.get("resize").and_then(Value::as_object).is_some_and(|r| r.contains_key("width") || r.contains_key("preset"));
    let height_given = p.get("resizeHeight").is_some() || p.get("resize").and_then(Value::as_object).is_some_and(|r| r.contains_key("height"));
    if om.resize.enabled && om.resize.lock_aspect && height_given && !width_given {
        let item = &s.project.render_queue[i];
        let comp = s.project.comp(item.comp).ok_or(EngineError::NoComp)?;
        let (w, h) = item.settings.output_size(comp);
        let mut crop_only = om.clone();
        crop_only.resize.enabled = false;
        let (cw, ch) = crop_only.frame_size(w, h, item.settings.effective_scale(comp));
        om.resize.width = ((om.resize.height as f64 * cw as f64 / ch as f64).round() as u32).clamp(1, 30_000);
    }
    let post = match str_p(p, "postRenderAction") {
        Some(a) => Some(post_render_parse(a).ok_or_else(|| bad("renderQueue.setOutputModule", "postRenderAction: none|import|importAndReplace|setProxy"))?),
        None => None,
    };
    if !changed && post.is_none() {
        return Err(bad("renderQueue.setOutputModule", "nothing to change"));
    }
    s.edit("Output Module Settings", None, |proj, _| {
        let it = &mut proj.render_queue[i];
        *module_mut(it, m) = om;
        if let Some(a) = post {
            it.post_render = a;
        }
        requeue(it);
        Ok(())
    })?;
    Ok(item_json(s, &s.project.render_queue[i], i))
}

fn set_output(s: &mut Session, p: &Value) -> Result<Value> {
    let i = item_index(s, p, "renderQueue.setOutput")?;
    let path = str_p(p, "path").or(str_p(p, "output")).ok_or_else(|| bad("renderQueue.setOutput", "pass `path` (file path or template)"))?;
    let m = module_p(s, i, p, "renderQueue.setOutput")?;
    let mut om = module_ref(&s.project.render_queue[i], m).clone();
    apply_output(&mut om, &s.project.render_templates, None, &json!({"output": path}), "renderQueue.setOutput")?;
    s.edit("Output To", None, |proj, _| {
        let it = &mut proj.render_queue[i];
        *module_mut(it, m) = om;
        requeue(it);
        Ok(())
    })?;
    Ok(item_json(s, &s.project.render_queue[i], i))
}

fn move_item(s: &mut Session, p: &Value) -> Result<Value> {
    let i = item_index(s, p, "renderQueue.move")?;
    let n = s.project.render_queue.len();
    let to = p.get("to").and_then(Value::as_u64).ok_or_else(|| bad("renderQueue.move", "pass `to` (1-based position)"))? as usize;
    let to = to.clamp(1, n) - 1;
    s.edit("Reorder Render Queue", None, |proj, _| {
        let it = proj.render_queue.remove(i);
        proj.render_queue.insert(to, it);
        Ok(())
    })?;
    Ok(json!({"index": to + 1}))
}

fn duplicate(s: &mut Session, p: &Value) -> Result<Value> {
    let i = item_index(s, p, "renderQueue.duplicate")?;
    let id = s.edit("Duplicate Render Item", None, |proj, _| {
        let mut it = proj.render_queue[i].clone();
        it.id = proj.render_queue.iter().map(|i| i.id).max().unwrap_or(0) + 1;
        it.render = true;
        it.status = RenderStatus::Queued;
        it.started = None;
        it.render_time = None;
        it.last_output = None;
        let id = it.id;
        proj.render_queue.insert(i + 1, it);
        Ok(id)
    })?;
    Ok(json!({"item": id, "index": i + 2}))
}

fn render(s: &mut Session, p: &Value) -> Result<Value> {
    let wait = b_p(p, "wait").unwrap_or(true);
    // Settings ▸ Project ▸ Auto-Save ▸ Save When Starting Render Queue.
    if s.prefs.auto_save.enabled
        && s.prefs.auto_save.save_on_render_start
        && s.is_dirty()
        && let Err(e) = s.autosave_now()
    {
        log::warn!("{e}");
    }
    let ids = s.start_render(wait).map_err(EngineError::Other)?;
    let items: Vec<Value> = s.project.render_queue.iter().enumerate().filter(|(_, i)| ids.contains(&i.id)).map(|(k, i)| item_json(s, i, k)).collect();
    Ok(json!({"rendering": s.is_rendering(), "items": items}))
}

fn stop(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!({"stopped": s.stop_render()}))
}

fn list(s: &mut Session, _: &Value) -> Result<Value> {
    s.poll_render();
    let items: Vec<Value> = s.project.render_queue.iter().enumerate().map(|(k, i)| item_json(s, i, k)).collect();
    Ok(json!({"items": items, "rendering": s.is_rendering(), "progress": s.render_progress()}))
}

fn formats(s: &mut Session, _: &Value) -> Result<Value> {
    let avail = s.exporter.as_ref().map(|e| e.formats()).unwrap_or_default();
    let v: Vec<Value> = OutputFormat::ALL
        .iter()
        .map(|f| {
            json!({
                "id": format!("{f:?}"),
                "label": f.label(),
                "extension": f.extension(),
                "sequence": f.is_sequence(),
                "alpha": f.supports_alpha(),
                "audio": f.supports_audio(),
                "available": avail.contains(f),
            })
        })
        .collect();
    use effectcraft_project::render_queue::{CodecProfile, VideoCodecOptions};
    let level = |l: &u8| format!("{}.{}", l / 10, l % 10);
    Ok(json!({
        "formats": v,
        "proresProfiles": ProResProfile::ALL.iter().map(|p| p.label()).collect::<Vec<_>>(),
        "codecProfiles": CodecProfile::ALL.iter().map(|p| p.label()).collect::<Vec<_>>(),
        "hevcLevels": VideoCodecOptions::HEVC_LEVELS.iter().map(level).collect::<Vec<_>>(),
        "av1Levels": VideoCodecOptions::AV1_LEVELS.iter().map(level).collect::<Vec<_>>(),
        "webmCodecs": ["vp9", "av1"],
        "opusApplications": ["audio", "voice"],
    }))
}

/// `item?|index?`, else the last item (the queue's newest entry).
fn item_or_last(s: &Session, p: &Value, cmd: &str) -> Result<usize> {
    if p.get("item").is_some() || p.get("index").is_some() {
        return item_index(s, p, cmd);
    }
    s.project.render_queue.len().checked_sub(1).ok_or_else(|| bad(cmd, "the render queue is empty"))
}

/// Composition ▸ Add Output Module: another output module on a render item (the same frames are
/// encoded once more).
fn add_output_module(s: &mut Session, p: &Value) -> Result<Value> {
    let i = item_or_last(s, p, "render.addOutputModule")?;
    let it = &s.project.render_queue[i];
    let n = it.extra_outputs.len() + 2;
    let mut om = it.output.clone();
    if let Some(f) = str_p(p, "format") {
        let f = OutputFormat::from_name(f).ok_or_else(|| bad("render.addOutputModule", FORMATS))?;
        om = OutputModule::for_format(f);
    }
    if str_p(p, "output").is_none() && str_p(p, "path").is_none() {
        // Same name with a module suffix so the files don't collide.
        om.output = match om.output.rfind('.') {
            Some(dot) => format!("{}_{n}{}", &om.output[..dot], &om.output[dot..]),
            None => format!("{}_{n}", om.output),
        };
    }
    apply_output(&mut om, &s.project.render_templates, s.state.region_of_interest, p, "render.addOutputModule")?;
    s.edit("Add Output Module", None, |proj, _| {
        let it = &mut proj.render_queue[i];
        it.extra_outputs.push(om);
        requeue(it);
        Ok(())
    })?;
    let mut v = item_json(s, &s.project.render_queue[i], i);
    v["module"] = json!(n);
    Ok(v)
}

/// Composition ▸ Pre-render…: queue the comp with a lossless-with-alpha module whose post-render
/// action imports the result and replaces the comp's uses.
fn pre_render(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = s.comp_for_queue(p)?;
    let mut it = RenderQueueItem::new(0, cid);
    let templates = s.project.render_templates.clone();
    it.settings = templates.default_render_settings(TemplateSlot::PreRender);
    it.output = templates.default_output_module(TemplateSlot::PreRender);
    it.output.output = "[compName]_prerender.[fileExtension]".into();
    apply_output(&mut it.output, &templates, s.state.region_of_interest, p, "render.preRender")?;
    it.post_render = PostRenderAction::ImportAndReplace;
    let id = s.edit("Pre-render", None, |proj, _| {
        it.id = proj.render_queue.iter().map(|i| i.id).max().unwrap_or(0) + 1;
        let id = it.id;
        proj.render_queue.push(it);
        Ok(id)
    })?;
    let idx = s.project.render_queue.len() - 1;
    let mut v = item_json(s, &s.project.render_queue[idx], idx);
    v["item"] = json!(id);
    Ok(v)
}

/// Composition ▸ Save Current Preview…: render the work area of the comp straight to a movie file.
fn save_current_preview(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("render.saveCurrentPreview", "missing `path`"))?.to_string();
    let cid = comp_id(s, p)?;
    let format = OutputFormat::from_path(&path).unwrap_or(OutputFormat::H264);
    let exporter = s.exporter.clone().ok_or_else(|| EngineError::Other("export is not available in this build".into()))?;
    if !exporter.formats().contains(&format) {
        return Err(EngineError::Other(format!("{} export is not available", format.label())));
    }
    let mut item = RenderQueueItem::new(0, cid);
    item.output = OutputModule::for_format(format);
    item.output.output = path.clone();
    let job = crate::ExportJob {
        project: &s.project,
        footage: s.footage.as_ref(),
        expr: s.expr.as_deref(),
        accel: s.accel.as_deref(),
        item: &item,
        path: &path,
        storage: s.storage_quota.as_deref(),
        label: "Save Current Preview".into(),
        nested_switches: s.prefs.general.switches_affect_nested_comps,
    };
    let r = exporter.export(&job, &mut |_, _| true).map_err(EngineError::Other)?;
    s.toast(format!("Saved preview to {}", r.path));
    Ok(serde_json::to_value(&r).unwrap_or_default())
}

// ---------------------------------------------------------------- templates (Edit ▸ Templates)

fn kind_p(p: &Value, cmd: &str) -> Result<TemplateKind> {
    str_p(p, "kind").and_then(TemplateKind::parse).ok_or_else(|| bad(cmd, "kind: renderSettings|outputModule"))
}

fn name_p<'a>(p: &'a Value, cmd: &str) -> Result<&'a str> {
    str_p(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(cmd, "pass `name`"))
}

/// Templates as JSON (lists, built-in flags, defaults per slot).
fn templates_json(t: &RenderTemplates) -> Value {
    let rs: Vec<Value> = t
        .render_settings_list()
        .iter()
        .map(|r| json!({"name": r.name, "builtin": RenderTemplates::is_builtin(TemplateKind::RenderSettings, &r.name), "settings": r}))
        .collect();
    let om: Vec<Value> = t
        .output_module_list()
        .iter()
        .map(|m| json!({"name": m.name, "builtin": RenderTemplates::is_builtin(TemplateKind::OutputModule, &m.name), "summary": format!("{} · {}", m.format.label(), m.channels.label()), "module": m}))
        .collect();
    let slots = |k: TemplateKind| -> Value {
        TemplateSlot::ALL.iter().map(|sl| (format!("{sl:?}"), json!(t.default_name(k, *sl)))).collect::<serde_json::Map<String, Value>>().into()
    };
    json!({
        "renderSettings": rs,
        "outputModules": om,
        "defaults": {"renderSettings": slots(TemplateKind::RenderSettings), "outputModules": slots(TemplateKind::OutputModule)},
        "slots": TemplateSlot::ALL.iter().map(|sl| json!({"id": format!("{sl:?}"), "label": sl.label()})).collect::<Vec<_>>(),
        "resizePresets": RESIZE_PRESETS.iter().map(|(n, w, h)| json!({"name": n, "width": w, "height": h})).collect::<Vec<_>>(),
    })
}

fn templates(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(templates_json(&s.project.render_templates))
}

/// Save a template: from an item's settings (`item`/`index`, `module`), or the slot default,
/// adjusted by `params` (the same keys as setRenderSettings / setOutputModule).
fn save_template(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "renderQueue.saveTemplate";
    let kind = kind_p(p, C)?;
    let name = name_p(p, C)?.to_string();
    if name.eq_ignore_ascii_case("custom") {
        return Err(bad(C, "“Custom” is reserved"));
    }
    let item = if p.get("item").is_some() || p.get("index").is_some() { Some(item_index(s, p, C)?) } else { None };
    let params = p.get("params").cloned().unwrap_or(Value::Null);
    let from = str_p(p, "from");
    let mut t = s.project.render_templates.clone();
    match kind {
        TemplateKind::RenderSettings => {
            let mut rs = match (item, from) {
                (Some(i), _) => s.project.render_queue[i].settings.clone(),
                (None, Some(f)) => t.render_settings(f).ok_or_else(|| bad(C, format!("no Render Settings template “{f}”")))?,
                (None, None) => t.default_render_settings(TemplateSlot::Movie),
            };
            if params.is_object() {
                apply_settings(&mut rs, &t, &params, C)?;
            }
            t.save_render_settings(&name, rs);
        }
        TemplateKind::OutputModule => {
            let mut om = match (item, from) {
                (Some(i), _) => module_ref(&s.project.render_queue[i], module_p(s, i, p, C)?).clone(),
                (None, Some(f)) => t.output_module(f).ok_or_else(|| bad(C, format!("no Output Module template “{f}”")))?,
                (None, None) => t.default_output_module(TemplateSlot::Movie),
            };
            if params.is_object() {
                apply_output(&mut om, &t, s.state.region_of_interest, &params, C)?;
            }
            // Templates name files after the composition, not a particular path.
            om.output = if om.format.is_sequence() { DEFAULT_SEQUENCE_TEMPLATE } else { DEFAULT_TEMPLATE }.into();
            t.save_output_module(&name, om);
        }
    }
    s.edit("Save Template", None, |proj, _| {
        proj.render_templates = t;
        Ok(())
    })?;
    Ok(json!({"saved": name, "templates": templates_json(&s.project.render_templates)}))
}

fn delete_template(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "renderQueue.deleteTemplate";
    let kind = kind_p(p, C)?;
    let name = name_p(p, C)?.to_string();
    let mut t = s.project.render_templates.clone();
    t.delete(kind, &name).map_err(|e| bad(C, e))?;
    s.edit("Delete Template", None, |proj, _| {
        proj.render_templates = t;
        Ok(())
    })?;
    Ok(json!({"deleted": name}))
}

fn set_template_default(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "renderQueue.setTemplateDefault";
    let kind = kind_p(p, C)?;
    let slot = str_p(p, "slot").and_then(TemplateSlot::parse).ok_or_else(|| bad(C, "slot: movie|still|preRender|movieProxy|stillProxy"))?;
    let name = name_p(p, C)?.to_string();
    let mut t = s.project.render_templates.clone();
    t.set_default(kind, slot, &name).map_err(|e| bad(C, e))?;
    s.edit("Template Default", None, |proj, _| {
        proj.render_templates = t;
        Ok(())
    })?;
    Ok(json!({"slot": format!("{slot:?}"), "name": name}))
}

/// Apply templates to an item: `renderSettings` and/or `outputModule` (to `module`).
fn apply_template(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "renderQueue.applyTemplate";
    let i = item_index(s, p, C)?;
    let m = module_p(s, i, p, C)?;
    let t = s.project.render_templates.clone();
    let rs = match str_p(p, "renderSettings") {
        Some(n) => Some(t.render_settings(n).ok_or_else(|| bad(C, format!("no Render Settings template “{n}”")))?),
        None => None,
    };
    let om = match str_p(p, "outputModule") {
        Some(n) => {
            let tpl = t.output_module(n).ok_or_else(|| bad(C, format!("no Output Module template “{n}”")))?;
            let mut om = module_ref(&s.project.render_queue[i], m).clone();
            apply_om_template(&mut om, &tpl);
            Some(om)
        }
        None => None,
    };
    if rs.is_none() && om.is_none() {
        return Err(bad(C, "pass `renderSettings` and/or `outputModule` (template names)"));
    }
    s.edit("Apply Template", None, |proj, _| {
        let it = &mut proj.render_queue[i];
        if let Some(rs) = rs {
            it.settings = rs;
        }
        if let Some(om) = om {
            *module_mut(it, m) = om;
        }
        requeue(it);
        Ok(())
    })?;
    Ok(item_json(s, &s.project.render_queue[i], i))
}

fn set_log(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "renderQueue.setLog";
    let i = item_index(s, p, C)?;
    let log = enum_p(p, "log", C, RenderLog::parse, "errorsOnly|plusSettings|plusPerFrameInfo")?.ok_or_else(|| bad(C, "pass `log`"))?;
    s.edit("Render Queue: Log", None, |proj, _| {
        proj.render_queue[i].log = log;
        Ok(())
    })?;
    Ok(item_json(s, &s.project.render_queue[i], i))
}

fn set_notify(s: &mut Session, p: &Value) -> Result<Value> {
    let on = b_p(p, "notify").or(b_p(p, "value")).unwrap_or(!s.project.render_prefs.notify);
    s.edit("Render Queue: Notify", None, |proj, _| {
        proj.render_prefs.notify = on;
        Ok(())
    })?;
    Ok(json!({"notify": on}))
}

fn set_overflow(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "renderQueue.setOverflowFolders";
    let folders: Vec<String> = match p.get("folders") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::trim).filter(|f| !f.is_empty()).map(String::from).collect(),
        Some(Value::Null) | None => vec![],
        _ => return Err(bad(C, "folders: [path, …]")),
    };
    s.edit("Storage Overflow Folders", None, |proj, _| {
        proj.render_prefs.overflow_folders = folders.clone();
        Ok(())
    })?;
    Ok(json!({"folders": s.project.render_prefs.overflow_folders}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "renderQueue.add",
            "Add to Render Queue",
            ["Composition"],
            ["Cmd+M", "Cmd+Shift+/"],
            "{comp?: id|name, template?: Render Settings template, format?: h264|hevc|av1|prores|uncompressed|webm|png|jpeg|tiff|exr|tga|gif|wav|aiff, output?: path|template, log?: errorsOnly|plusSettings|plusPerFrameInfo, quality?: best|draft|1-100 (jpeg, webm), resolution?: full|half|third|quarter|scale, proxyUse?, effects?, solo?, guideLayers?, colorDepth?, frameBlending?, fieldRender?, pulldown?, motionBlur?, timeSpan?: workArea|comp|custom, start?: s, end?: s, duration?: s, frameRate?: fps|null, skipExisting?: bool, storageOverflow?: bool, channels?: rgb|rgba|alpha, color?, bitrate?: kbps, webmBitrate?, keyframeInterval?, proresProfile?, profile?, level?, rateControl?, webmCodec?, audioBitrate?, opusApplication?, crop?, resize?, includeProjectLink?, audio?: auto|on|off, sampleRate?, audioChannels?, audioFormat?, loop?: bool (values as in setRenderSettings / setOutputModule)}",
            can_add,
            add
        ),
        cmd!("renderQueue.remove", "Remove from Render Queue", [], None, "{item?: id, index?: n}", has_items, remove),
        cmd!("renderQueue.setRender", "Render Queue: Render Checkbox", [], None, "{item?|index?, render?: bool (toggles)}", has_items, set_render),
        cmd!(
            "renderQueue.setRenderSettings",
            "Render Settings...",
            [],
            None,
            "{item?|index?, template?: name, quality?: best|draft, resolution?: full|half|third|quarter|scale, proxyUse?: current|all|comp|none, effects?: current|allOn|allOff, solo?: current|allOff, guideLayers?: current|allOff, colorDepth?: current|8|16|32, frameBlending?: current|onForChecked|offForAll, fieldRender?: off|upper|lower, pulldown?: off|WSSWW|SSWWW|SWWWS|WWWSS|WWSSW, motionBlur?: bool|current|onForChecked|offForAll, timeSpan?: workArea|comp|custom, start?: s, end?: s, duration?: s, frameRate?: fps|null, skipExisting?: bool, storageOverflow?: bool}",
            has_items,
            set_render_settings
        ),
        cmd!(
            "renderQueue.setOutputModule",
            "Output Module Settings...",
            [],
            None,
            "{item?|index?, module?: n (1 = first), template?: name, format?: h264|hevc|av1|prores|uncompressed|webm|png|jpeg|tiff|exr|tga|gif|wav|aiff, channels?: rgb|rgba|alpha, color?: straight|premultiplied, quality?: 1-100, bitrate?: kbps, webmBitrate?: bool, keyframeInterval?: frames (0 = auto), proresProfile?: proxy|lt|standard|hq|4444|4444xq, profile?: main|main10 (HEVC/AV1), level?: auto|4.1, rateControl?: bitrate|quality, webmCodec?: vp9|av1, audioBitrate?: Opus kbps, opusApplication?: audio|voice, crop?: bool|{useRoi?, roi?, top?, left?, bottom?, right?}, cropTop?, cropLeft?, cropBottom?, cropRight?, resize?: bool|{preset?, width?, height?, lockAspect?, quality?: low|high}, resizeWidth?, resizeHeight?, postRenderAction?: none|import|importAndReplace|setProxy, includeProjectLink?: bool, audio?: auto|on|off, sampleRate?, audioChannels?: mono|stereo, audioFormat?: 16|24|32, loop?: bool, output?}",
            has_items,
            set_output_module
        ),
        cmd!(
            "renderQueue.setOutput",
            "Output To...",
            [],
            None,
            "{item?|index?, module?: n, path: file path or template like [compName].[fileExtension]}",
            has_items,
            set_output
        ),
        cmd!(
            "render.addOutputModule",
            "Add Output Module",
            ["Composition"],
            None,
            "{item?|index? (default: the last item), format?, output?, channels?, quality?, bitrate?, proresProfile?, audio?}",
            has_items,
            add_output_module
        ),
        cmd!("render.preRender", "Pre-render...", ["Composition"], None, "{comp?, output?: path|template, format?}", can_add, pre_render),
        cmd!("render.saveCurrentPreview", "Save Current Preview...", ["Composition"], None, "{comp?, path}", has_comp, save_current_preview),
        cmd!("renderQueue.move", "Move in Render Queue", [], None, "{item?|index?, to: n (1-based)}", has_items, move_item),
        cmd!("renderQueue.duplicate", "Duplicate Render Item", [], None, "{item?|index?}", has_items, duplicate),
        cmd!("renderQueue.render", "Render", [], None, "{wait?: bool (default true; the UI renders in the background)}", has_queued, render),
        cmd!("renderQueue.stop", "Stop Rendering", [], None, "{}", rendering, stop),
        query!("renderQueue.list", "Render Queue Items", "{}", list),
        query!("renderQueue.templates", "Render Templates", "{}", templates),
        cmd!(
            "renderQueue.saveTemplate",
            "Save Template...",
            [],
            None,
            "{kind: renderSettings|outputModule, name, item?|index? (save from this item), from?: template to copy, module?, params?: {setRenderSettings / setOutputModule keys}}",
            always,
            save_template
        ),
        cmd!("renderQueue.deleteTemplate", "Delete Template", [], None, "{kind: renderSettings|outputModule, name}", always, delete_template),
        cmd!(
            "renderQueue.setTemplateDefault",
            "Set Template Default",
            [],
            None,
            "{kind: renderSettings|outputModule, slot: movie|still|preRender|movieProxy|stillProxy, name}",
            always,
            set_template_default
        ),
        cmd!(
            "renderQueue.applyTemplate",
            "Apply Template",
            [],
            None,
            "{item?|index?, renderSettings?: template name, outputModule?: template name, module?: n}",
            has_items,
            apply_template
        ),
        cmd!("renderQueue.setLog", "Render Queue: Log", [], None, "{item?|index?, log: errorsOnly|plusSettings|plusPerFrameInfo}", has_items, set_log),
        cmd!("renderQueue.setNotify", "Notify When Done", [], None, "{notify?: bool (toggles)}", always, set_notify),
        cmd!(
            "renderQueue.setOverflowFolders",
            "Storage Overflow Folders",
            [],
            None,
            "{folders: [path, …] (used in order when the output volume is full)}",
            always,
            set_overflow
        ),
        query!("renderQueue.formats", "Output Formats", "{}", formats),
    ]
}
