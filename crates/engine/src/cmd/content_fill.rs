//! Content-Aware Fill (Window ▸ Content-Aware Fill, Layer ▸ New ▸ Content-Aware Fill Layer):
//! fill the transparent area of a layer (what its masks cut away) across the work area or the
//! whole layer, render the result as a PNG sequence and add it as a "Fill" layer above the
//! source, like After Effects. The algorithms are `effectcraft_raster::inpaint` (Object:
//! flow-guided propagation + PatchMatch; Surface: propagation + one synthesised fill carried by
//! the flow; Edge Blend: membrane fill). It runs as a background job (Window ▸ Progress); in
//! the browser the job goes to a job worker ([`FillPlan`], `offload::WorkerJob::ContentFill`),
//! whose own WebGPU device renders the layer's frames in passes, and the files come back to
//! browser storage.

use std::sync::Arc;

use effectcraft_color::Label;
use effectcraft_project::{Footage, FootageKind, ItemId, ItemKind, LayerId, LayerSource, build};
use effectcraft_raster::Image;
use effectcraft_raster::inpaint::{FillInput, FillMethod, FillOpts, dilate, fill_sequence};
use effectcraft_render::{Accelerator, EvalCtx, ExprHost, FootageSource, LayerCache, Renderer};
use effectcraft_time::Tick;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::scene_detect::LayerFrames;
use super::{CommandSpec, always, b_p, bad, f_p, has_layers, layer_p, str_p};
use crate::jobs::Apply;
use crate::{EngineError, Result, Session, cmd};

/// Content-Aware Fill panel settings (serde, in [`crate::EditorState`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FillSettings {
    /// `object`, `surface` or `edgeBlend`.
    pub method: String,
    /// `workArea` or `entire` (the layer's duration).
    pub range: String,
    /// Grow the fill area by this many pixels.
    pub alpha_expansion: f64,
    /// `none`, `subtle`, `moderate`, `strong`.
    pub lighting_correction: String,
    /// Reference frame: a layer of the comp and the comp time of the frame to use.
    pub reference_layer: Option<LayerId>,
    pub reference_time: Option<f64>,
}

impl Default for FillSettings {
    fn default() -> Self {
        FillSettings {
            method: "object".into(),
            range: "workArea".into(),
            alpha_expansion: 0.0,
            lighting_correction: "none".into(),
            reference_layer: None,
            reference_time: None,
        }
    }
}

fn lighting(s: &str) -> Option<f32> {
    Some(match s {
        "none" | "off" => 0.0,
        "subtle" => 0.35,
        "moderate" => 0.65,
        "strong" => 1.0,
        _ => return None,
    })
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "contentFill.set";
    let mut st = s.state.content_fill.clone();
    if let Some(m) = str_p(p, "method") {
        st.method = FillMethod::from_name(m).ok_or_else(|| bad(c, "method: object|surface|edgeBlend"))?.name().into();
    }
    if let Some(r) = str_p(p, "range") {
        st.range = match r {
            "workArea" | "work area" => "workArea".into(),
            "entire" | "entireDuration" | "layer" => "entire".into(),
            _ => return Err(bad(c, "range: workArea|entire")),
        };
    }
    if let Some(a) = f_p(p, "alphaExpansion") {
        st.alpha_expansion = a.clamp(0.0, 100.0);
    }
    if let Some(l) = str_p(p, "lightingCorrection") {
        lighting(l).ok_or_else(|| bad(c, "lightingCorrection: none|subtle|moderate|strong"))?;
        st.lighting_correction = l.into();
    }
    match p.get("referenceLayer") {
        Some(Value::Null) => st.reference_layer = None,
        Some(v) => {
            let comp = s.active_comp().ok_or(EngineError::NoComp)?;
            st.reference_layer = Some(super::resolve_layer(comp, v).ok_or_else(|| bad(c, format!("no layer {v}")))?);
            st.reference_time = Some(f_p(p, "referenceTime").unwrap_or(s.time().seconds()));
        }
        None => {
            if let Some(t) = f_p(p, "referenceTime") {
                st.reference_time = Some(t);
            }
        }
    }
    s.state.content_fill = st.clone();
    Ok(serde_json::to_value(st).unwrap_or_default())
}

/// A layer's masked source at comp time `t` on a `w × h` canvas in layer pixels (straight colour
/// with alpha 1 where kept), and the hole (`true` where the alpha is under one half).
fn masked_frame(r: &Renderer, ctx: &EvalCtx, layer: &effectcraft_project::Layer, w: u32, h: u32) -> (Image, Vec<bool>) {
    let mut img = Image::new(w, h);
    let mut hole = vec![true; (w * h) as usize];
    if let Some(buf) = r.layer_input(ctx, layer, 0) {
        let sc = if buf.scale > 0.0 { buf.scale } else { 1.0 };
        for y in 0..h {
            for x in 0..w {
                let (bx, by) = ((x as f64 + 0.5) * sc - buf.offset[0], (y as f64 + 0.5) * sc - buf.offset[1]);
                let p = if (sc - 1.0).abs() < 1e-9 { buf.img.get(bx.floor() as i64, by.floor() as i64) } else { buf.img.sample_bilinear(bx, by) };
                let i = (y * w + x) as usize;
                if p[3] >= 0.5 {
                    img.data[i] = [p[0] / p[3], p[1] / p[3], p[2] / p[3], 1.0];
                    hole[i] = false;
                }
            }
        }
    }
    (img, hole)
}

/// Encode a straight-alpha frame (alpha 1) as 8-bit PNG.
fn png(img: &Image) -> std::result::Result<Vec<u8>, String> {
    let rgba: Vec<u8> = img.data.iter().flat_map(|p| [p[0], p[1], p[2], p[3]].map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)).collect();
    super::comp_more::encode_png(&rgba, img.width, img.height)
}

fn default_dir(s: &Session, name: &str) -> String {
    let base = s.path.as_deref().and_then(|p| std::path::Path::new(p).parent().map(|d| d.join("Fill"))).unwrap_or_else(|| {
        // (the browser has no temporary folder: its storage)
        if cfg!(target_arch = "wasm32") { std::path::PathBuf::from("/Fill") } else { std::env::temp_dir().join("effectcraft-fill") }
    });
    let safe: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let mut k = 1;
    loop {
        let d = base.join(format!("{safe}_Fill_{k}"));
        let ds = d.to_string_lossy().to_string();
        if !s.services.exists(&ds) && !s.services.exists(&format!("{ds}/fill_00000.png")) {
            return ds;
        }
        k += 1;
    }
}

/// A fill job, detached from the session (it also travels to a browser job worker).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FillPlan {
    pub comp: ItemId,
    pub layer: LayerId,
    /// The source layer's name.
    pub name: String,
    /// The layer's size (the frames' size).
    pub width: u32,
    pub height: u32,
    /// Comp times of the filled frames.
    pub times: Vec<Tick>,
    /// `object`, `surface` or `edgeBlend`.
    pub method: String,
    pub lighting: f32,
    /// Alpha expansion (px).
    pub expand: usize,
    /// Reference frame: (layer, comp time).
    pub reference: Option<(LayerId, Tick)>,
    /// Folder of the PNG sequence.
    pub dir: String,
    /// Working memory the fill may use, in bytes (0: [`DEFAULT_FILL_MEMORY`]). A range that needs
    /// more is filled in windows of frames ([`fill_window`]).
    #[serde(default)]
    pub memory: u64,
}

/// Working memory a fill needs per pixel of each frame it holds: the frame and its hole, and in
/// the fill the copies, membrane fills, flows and results it keeps for every frame (about 115
/// bytes measured, rounded up).
const FILL_BYTES_PER_PIXEL: u64 = 128;

/// The fill's working memory when the plan names none (2 GiB).
pub const DEFAULT_FILL_MEMORY: u64 = 2 << 30;

/// How many frames one fill window holds at most: as many as `memory` bytes (0: the default)
/// allow for `width` × `height` frames. `None` when not even two frames fit (a window needs a
/// frame to carry the fill on from): filling a whole range at once held every frame several
/// times over, ten seconds of 1080p needed about 70 GB, and the app was killed for running out
/// of memory.
pub fn fill_window(width: u32, height: u32, memory: u64) -> Option<usize> {
    let budget = if memory > 0 { memory } else { DEFAULT_FILL_MEMORY };
    let per_frame = (u64::from(width) * u64::from(height)).saturating_mul(FILL_BYTES_PER_PIXEL).max(1);
    let frames = usize::try_from(budget / per_frame).unwrap_or(usize::MAX);
    (frames >= 2).then_some(frames)
}

/// The error for a fill whose frames are too large for even a two-frame window.
fn window_error(w: u32, h: u32) -> String {
    format!("Content-Aware Fill can't hold two {w}×{h} frames in the memory it may use: close other applications or fill a smaller layer")
}

/// The working memory a fill started in this session may use: half of what the system has
/// available when that is known (0, the default, when it isn't).
fn fill_memory(s: &Session) -> u64 {
    s.sys_memory.filter(|m| m.available > 1).map_or(0, |m| m.available / 2)
}

/// The windows a fill of `count` frames runs in, `window` frames each, in order: first the one
/// holding the reference frame `anchor`, then those after it, then those before it. Neighbouring
/// windows share a frame, which the window filled first carries into the next (`backward`: the
/// shared frame is the window's last, carried back from the window after it).
fn fill_windows(count: usize, window: usize, anchor: usize) -> Vec<Window> {
    let window = window.max(2);
    let end = anchor.saturating_add(window / 2 + 1).min(count).max(window.min(count));
    let start = end.saturating_sub(window);
    let mut out = vec![Window { start, end, backward: false }];
    let mut s = end;
    while s < count {
        let a = s - 1;
        let b = a.saturating_add(window).min(count);
        out.push(Window { start: a, end: b, backward: false });
        s = b;
    }
    let mut e = start;
    while e > 0 {
        let b = e + 1;
        let a = b.saturating_sub(window);
        out.push(Window { start: a, end: b, backward: true });
        e = a;
    }
    out
}

/// One window of a fill: frames `start..end`; see [`fill_windows`].
#[derive(Clone, Copy, Debug, PartialEq)]
struct Window {
    start: usize,
    end: usize,
    backward: bool,
}

/// The fill holds every frame as float RGBA (16 bytes a pixel), its hole mask (1) and the filled
/// copy (16) until it finishes (#1789): when even that is more than `available`, the error to
/// report up front, instead of running out of memory partway through.
fn memory_shortfall(frames: usize, w: u32, h: u32, available: u64) -> Option<String> {
    let need = (frames as u64).saturating_mul(u64::from(w)).saturating_mul(u64::from(h)).saturating_mul(33);
    (need > available).then(|| {
        let gb = |b: u64| b as f64 / 1e9;
        format!(
            "Content-Aware Fill of {frames} frames at {w}×{h} needs at least {:.1} GB of memory, and {:.1} GB is free: fill a shorter range (set the work area and choose Work Area) or close other applications",
            gb(need),
            gb(available)
        )
    })
}

/// The frames a fill has written so far: removed when it's dropped before [`Written::keep`]
/// (the fill failed, was cancelled or was dropped), so a fill leaves all of its sequence or
/// none of it.
struct Written<'a> {
    services: &'a dyn crate::Services,
    paths: Vec<String>,
    keep: bool,
}

impl Written<'_> {
    /// The fill finished: its frames stay.
    fn keep(mut self) {
        self.keep = true;
    }
}

impl Drop for Written<'_> {
    fn drop(&mut self) {
        if !self.keep {
            for p in &self.paths {
                let _ = self.services.remove_file(p);
            }
        }
    }
}

/// Run a fill: the layer's frames (rendered in passes with a job worker's GPU), the fill, the
/// PNG sequence written into `plan.dir` (`Services::store_file`). Returns the files.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_plan(
    plan: &FillPlan,
    project: &effectcraft_project::Project,
    footage: &dyn FootageSource,
    expr: Option<&dyn ExprHost>,
    cache: &LayerCache,
    services: &dyn crate::Services,
    accel: Option<&dyn Accelerator>,
    progress: &mut dyn FnMut(u64, u64) -> bool,
    message: &mut dyn FnMut(&str),
) -> std::result::Result<Vec<String>, String> {
    message("Reading frames");
    let (cid, w, h) = (plan.comp, plan.width, plan.height);
    let comp = project.comp(cid).ok_or("composition missing")?;
    let layer = comp.layer(plan.layer).ok_or("layer missing")?;
    let mut r = Renderer::new(project, footage, crate::offload::analysis_opts(accel));
    r.expr = expr;
    r.cache = Some(cache);
    let r = &r;
    let ctx0 = EvalCtx { expr, ..EvalCtx::new(project, cid, comp, Tick::ZERO) };
    let times = &plan.times;
    let count = times.len();
    if count == 0 {
        return Err("the range holds no whole frame to fill".into());
    }
    let n = count as u64;
    // The reference frame (a complete picture of one frame), at the filled frame nearest it.
    let mut reference = None;
    if let Some((rl, rt)) = plan.reference
        && let Some(l) = comp.layer(rl)
    {
        let (img, hole) = crate::offload::in_passes(accel, |a| masked_frame(&r.with_accel(a), &ctx0.at(rt), l, w, h)).await;
        if hole.iter().all(|b| !*b) {
            let k = times.iter().enumerate().min_by_key(|(_, t)| (t.0 - rt.0).abs()).map(|(k, _)| k).unwrap_or(0);
            reference = Some((k, img));
        }
    }
    let opts = FillOpts { method: FillMethod::from_name(&plan.method).unwrap_or_default(), lighting: plan.lighting, ..Default::default() };
    // A range too long to hold at once is filled in windows, outward from the reference frame.
    // Each window starts (or, going back, ends) with a frame its neighbour filled, as a
    // reference frame, so the fill carries on across them both ways.
    let window = fill_window(w, h, plan.memory).ok_or_else(|| window_error(w, h))?;
    let windows = fill_windows(count, window, reference.as_ref().map_or(0, |(k, _)| *k));
    let last = windows.len().saturating_sub(1);
    let dir = &plan.dir;
    let mut paths: Vec<Option<String>> = vec![None; count];
    let mut files = Written { services, paths: vec![], keep: false };
    let (mut forward, mut back): (Option<Image>, Option<Image>) = (None, None);
    let mut any_hole = false;
    let (mut read, mut filled, mut written) = (0u64, 0u64, 0u64);
    for (wi, win) in windows.iter().enumerate() {
        let Window { start, end, backward } = *win;
        let len = end - start;
        // The frame shared with the window filled before, and where it sits in this one.
        let carried = if wi == 0 {
            None
        } else if backward {
            back.take().map(|img| (len - 1, img))
        } else {
            forward.take().map(|img| (0, img))
        };
        let mut frames = Vec::with_capacity(len);
        let mut holes = Vec::with_capacity(len);
        let mut references = vec![];
        for i in 0..len {
            if let Some((ci, img)) = &carried
                && *ci == i
            {
                holes.push(vec![false; w as usize * h as usize]);
                frames.push(img.clone());
                references.push((i, img.clone()));
                continue;
            }
            let Some(t) = times.get(start + i) else { break };
            let (img, hole) = crate::offload::in_passes(accel, |a| masked_frame(&r.with_accel(a), &ctx0.at(*t), layer, w, h)).await;
            holes.push(dilate(&hole, w as usize, h as usize, plan.expand));
            frames.push(img);
            read += 1;
            if !progress(read.min(n), n * 3) {
                return Err(crate::render_queue::CANCELLED.into());
            }
        }
        any_hole |= holes.iter().any(|h| h.iter().any(|b| *b));
        // (checked before anything is written when the range fits in one window)
        if wi == last && !any_hole {
            return Err("the layer has no transparent area to fill: draw a mask (Subtract) around what to remove".into());
        }
        if let Some((k, img)) = &reference
            && (start..end).contains(k)
            && carried.as_ref().is_none_or(|(ci, _)| start + ci != *k)
        {
            references.push((k - start, img.clone()));
        }
        message("Filling");
        let (f0, l) = (filled, len as u64);
        let input = FillInput { frames, holes, references };
        let out = fill_sequence(&input, &opts, &mut |d, t| progress(n + (f0 + d as u64 * l / t.max(1) as u64).min(n), n * 3))
            .ok_or(crate::render_queue::CANCELLED)?;
        filled += l;
        drop(input);
        message("Writing the fill layer");
        for (i, img) in out.iter().enumerate() {
            // The shared frame was written by the window that filled it.
            if carried.as_ref().is_some_and(|(ci, _)| *ci == i) {
                continue;
            }
            let k = start + i;
            let path = std::path::Path::new(dir).join(format!("fill_{k:05}.png")).to_string_lossy().to_string();
            services.store_file(&path, &png(img)?).map_err(|e| format!("{path}: {e}"))?;
            files.paths.push(path.clone());
            if let Some(slot) = paths.get_mut(k) {
                *slot = Some(path);
            }
            written += 1;
            if !progress(2 * n + written.min(n), n * 3) {
                return Err(crate::render_queue::CANCELLED.into());
            }
        }
        // The window holding the reference carries both ways; the others in their direction.
        if wi == 0 || backward {
            back = out.first().cloned();
        }
        if !backward {
            forward = out.into_iter().last();
        }
    }
    files.keep();
    Ok(paths.into_iter().flatten().collect())
}

/// Add the fill layer of a finished plan (one undo step).
pub(crate) fn apply_plan(s: &mut Session, plan: &FillPlan, paths: Vec<String>) -> Result<Value> {
    let importer = s.importer.clone();
    add_fill_layer(s, plan.comp, plan.layer, &plan.name, paths, (plan.width, plan.height), plan.times[0], plan.times.len(), importer)
}

/// The fill `contentFill.generate` would run with parameters `p` (they override the panel's
/// settings).
pub(crate) fn make_plan(s: &mut Session, p: &Value) -> Result<FillPlan> {
    let c = "contentFill.generate";
    let (cid, lid) = layer_p(s, p, c)?;
    // Parameters override the panel settings.
    let mut st = s.state.content_fill.clone();
    {
        let saved = s.state.content_fill.clone();
        let mut q = p.clone();
        if let Some(o) = q.as_object_mut() {
            o.retain(|k, _| ["method", "range", "alphaExpansion", "lightingCorrection", "referenceLayer", "referenceTime"].contains(&k.as_str()));
        }
        set(s, &q)?;
        std::mem::swap(&mut st, &mut s.state.content_fill);
        s.state.content_fill = saved;
    }
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?.clone();
    if !layer.source.is_av() || layer.source.item().is_none() {
        return Err(bad(c, "Content-Aware Fill needs a footage, solid or precomp layer"));
    }
    let (w, h) = effectcraft_render::source_size(&s.project, &layer);
    if w == 0 || h == 0 {
        return Err(bad(c, "the layer has no pixels"));
    }
    let fr = comp.frame_rate;
    let (a, b) =
        if st.range == "entire" { (layer.in_point, layer.out_point) } else { (comp.work_area.0.max(layer.in_point), comp.work_area.1.min(layer.out_point)) };
    let (a, b) = (a.max(Tick::ZERO), b.min(comp.duration));
    if b <= a {
        return Err(bad(c, "the range doesn't overlap the layer"));
    }
    let (fa, fb) = (fr.frame_at(fr.snap_nearest(a)), fr.frame_at(b - Tick(1)));
    let times: Vec<Tick> = (fa..=fb).map(|f| fr.tick_of(f)).collect();
    if times.is_empty() {
        return Err(bad(c, "the range holds no whole frame to fill"));
    }
    // The fill holds one window of frames at a time.
    let window = fill_window(w, h, fill_memory(s)).ok_or_else(|| bad(c, window_error(w, h)))?;
    if let Some(e) = crate::sysinfo::memory().and_then(|m| memory_shortfall(times.len().min(window), w, h, m.available)) {
        return Err(bad(c, e));
    }
    let opts = FillOpts {
        method: FillMethod::from_name(&st.method).unwrap_or_default(),
        lighting: lighting(&st.lighting_correction).unwrap_or(0.0),
        ..Default::default()
    };
    let expand = st.alpha_expansion.round().max(0.0) as usize;
    let reference = st.reference_layer.filter(|r| comp.layer(*r).is_some()).map(|r| (r, Tick::from_seconds_f64(st.reference_time.unwrap_or(0.0))));
    let dir = str_p(p, "outputDir").map(str::to_string).unwrap_or_else(|| default_dir(s, &layer.name));
    let plan = FillPlan {
        comp: cid,
        layer: lid,
        name: layer.name.clone(),
        width: w,
        height: h,
        times,
        method: opts.method.name().to_string(),
        lighting: opts.lighting,
        expand,
        reference,
        dir,
        memory: fill_memory(s),
    };
    Ok(plan)
}

fn generate(s: &mut Session, p: &Value) -> Result<Value> {
    let plan = make_plan(s, p)?;
    let (cid, lid, (w, h)) = (plan.comp, plan.layer, (plan.width, plan.height));
    let wait = b_p(p, "wait").unwrap_or(false);
    // The browser: a job worker (its own GPU), the page keeps running.
    if !wait && s.offloads() {
        s.offload_analysis(crate::offload::WorkerJob::ContentFill { plan: Box::new(plan) }).map_err(EngineError::Other)?;
        return Ok(json!({"job": "contentFill", "started": true, "worker": true}));
    }
    let lf = LayerFrames::new(s, cid, lid);
    let services = s.services.clone();
    let importer = s.importer.clone();
    let name = plan.name.clone();
    s.spawn_task("contentFill", format!("Content-Aware Fill: {name}"), wait, move |ctl| {
        let paths = effectcraft_render::passes::block_on(run_plan(
            &plan,
            &lf.project,
            lf.footage.as_ref(),
            lf.expr.as_deref(),
            &lf.cache,
            services.as_ref(),
            None,
            &mut |d, t| ctl.progress(d, t),
            &mut |m| ctl.message(m),
        ))?;
        let apply: Apply = Box::new(move |s: &mut Session| add_fill_layer(s, cid, lid, &name, paths, (w, h), plan.times[0], plan.times.len(), importer));
        Ok(apply)
    })
}

/// Import the rendered sequence and put it in a layer above the source.
#[allow(clippy::too_many_arguments)]
fn add_fill_layer(
    s: &mut Session,
    cid: ItemId,
    lid: LayerId,
    name: &str,
    paths: Vec<String>,
    size: (u32, u32),
    start: Tick,
    count: usize,
    importer: Option<Arc<dyn crate::Importer>>,
) -> Result<Value> {
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let src = comp.layer(lid).ok_or_else(|| bad("contentFill.generate", "the source layer is gone"))?.clone();
    let fr = comp.frame_rate;
    let dur = fr.tick_of(count as i64);
    let mut footage = importer.and_then(|i| i.probe(&paths[0]).ok()).filter(|f| f.has_video).unwrap_or_else(|| Footage {
        path: paths[0].clone(),
        kind: FootageKind::Sequence,
        width: size.0,
        height: size.1,
        has_video: true,
        codec: "PNG".into(),
        ..Default::default()
    });
    footage.kind = FootageKind::Sequence;
    footage.sequence = paths.clone();
    footage.frame_rate = fr;
    footage.duration = dur;
    let (lname, item, layer) = s.edit("Content-Aware Fill", None, |proj, st| {
        let folder = proj.folder_named("Fill").unwrap_or_else(|| proj.add_item("Fill", Label::Yellow, None, ItemKind::Folder));
        let n = proj.items.values().filter(|i| i.parent == Some(folder)).count() + 1;
        let lname = format!("Fill {n} [{name}]");
        let iid = proj.add_item(&lname, Label::Lavender, Some(folder), ItemKind::Footage(footage));
        let mut l = build::layer(proj, &comp, &lname, LayerSource::Footage { item: iid }, size, Some(dur));
        l.start_time = start;
        l.in_point = start;
        l.out_point = (start + dur).min(comp.duration);
        l.parent = src.parent;
        l.switches.three_d = src.switches.three_d;
        if let (Some(dst), Some(t)) = (l.props.sub_mut("transform"), src.transform()) {
            let mut tr = t.clone();
            tr.reassign_uids(&mut proj.next_id);
            *dst = tr;
        }
        let id = l.id;
        let cm = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let at = cm.layers.iter().position(|x| x.id == lid).unwrap_or(0);
        cm.layers.insert(at, l);
        st.selected_layers = vec![id];
        Ok((lname, iid, id))
    })?;
    Ok(json!({"layer": layer.0, "item": item.0, "name": lname, "frames": count, "files": paths}))
}

fn open_panel(s: &mut Session, p: &Value) -> Result<Value> {
    // Parameters given: generate straight away (agents); else show the panel.
    if p.as_object().is_some_and(|o| !o.is_empty()) {
        return generate(s, p);
    }
    s.events.push(crate::Event::Frontend { command: "window.panel".into(), params: json!({"panel": "contentAwareFill"}) });
    Ok(json!({"frontend": "window.panel"}))
}

pub fn specs() -> Vec<CommandSpec> {
    const P: &str = "{layer?, method?: object|surface|edgeBlend, range?: workArea|entire, alphaExpansion? (px), lightingCorrection?: none|subtle|moderate|strong, referenceLayer?, referenceTime? (s), outputDir?, wait?: bool}";
    vec![
        cmd!(
            "contentFill.set",
            "Content-Aware Fill Settings",
            [],
            None,
            "{method?: object|surface|edgeBlend, range?: workArea|entire, alphaExpansion? (px), lightingCorrection?: none|subtle|moderate|strong, referenceLayer?: layer|null, referenceTime? (s)}",
            always,
            set
        ),
        CommandSpec {
            id: "contentFill.generate",
            label: "Generate Fill Layer",
            menu: &[],
            shortcut: None,
            alt_shortcuts: &[],
            params: P,
            enabled: has_layers,
            run: generate,
            journal: true,
        },
        CommandSpec {
            id: "layer.newContentAwareFill",
            label: "Content-Aware Fill Layer...",
            menu: &["Layer", "New"],
            shortcut: None,
            alt_shortcuts: &[],
            params: P,
            enabled: always,
            run: open_panel,
            journal: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::{Window, fill_windows, memory_shortfall};

    /// Windows run outward from the reference frame, so its fill reaches the frames before it
    /// too (they used to start at frame 0, and a reference only reached its own window and the
    /// later ones). Each frame is in a window, and neighbours share one frame.
    #[test]
    fn fill_windows_run_outward_from_the_reference() {
        let w = |start, end, backward| Window { start, end, backward };
        assert_eq!(fill_windows(12, 3, 0), vec![w(0, 3, false), w(2, 5, false), w(4, 7, false), w(6, 9, false), w(8, 11, false), w(10, 12, false)]);
        assert_eq!(fill_windows(12, 3, 7), vec![w(6, 9, false), w(8, 11, false), w(10, 12, false), w(4, 7, true), w(2, 5, true), w(0, 3, true)]);
        assert_eq!(fill_windows(12, 3, 11), vec![w(9, 12, false), w(7, 10, true), w(5, 8, true), w(3, 6, true), w(1, 4, true), w(0, 2, true)]);
        assert_eq!(fill_windows(5, 30, 2), vec![w(0, 5, false)], "a range that fits is one window");
        assert_eq!(fill_windows(1, 2, 0), vec![w(0, 1, false)]);
        for (count, window, anchor) in [(12, 3, 7), (100, 7, 50), (9, 2, 4), (40, 9, 39), (3, usize::MAX, 1)] {
            let ws = fill_windows(count, window, anchor);
            assert!(ws[0].start <= anchor && anchor < ws[0].end, "{ws:?}");
            assert!(ws.iter().all(|x| x.end - x.start <= window.max(2) && x.start < x.end && x.end <= count), "{ws:?}");
            let mut seen = vec![false; count];
            for x in &ws {
                seen[x.start..x.end].iter_mut().for_each(|b| *b = true);
            }
            assert!(seen.iter().all(|b| *b), "{count} {window} {anchor}: {ws:?}");
        }
    }

    /// #1789: 10 s of 1080p30 (300 frames) needs at least 20 GB, which 4 GB free can't hold.
    #[test]
    fn a_fill_that_cannot_fit_in_memory_is_refused_up_front() {
        let e = memory_shortfall(300, 1920, 1080, 4_000_000_000).expect("refused");
        assert!(e.contains("300 frames") && e.contains("Work Area"), "{e}");
        assert_eq!(memory_shortfall(30, 1920, 1080, 4_000_000_000), None, "a second of it fits");
        // Hostile sizes saturate instead of overflowing.
        assert!(memory_shortfall(usize::MAX, u32::MAX, u32::MAX, u64::MAX - 1).is_some());
    }
}
