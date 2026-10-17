//! Compositions → timeline documents (File ▸ Export ▸ Adobe Premiere Pro Project…, which writes
//! Final Cut Pro XML that Premiere Pro imports; FCPXML, OTIO and EDL on request).

use std::collections::HashMap;

use effectcraft_keyframe::{Interp, Value};
use effectcraft_project::{Comp, Footage, FootageKind, ItemId, ItemKind, Layer, LayerId, LayerSource, Project, Property};
use effectcraft_time::{FrameRate, Tick};
use filmcraft_geom::Vec2;
use filmcraft_media::{AudioStreamInfo, Generator, MediaInfo, MediaKind, VideoStreamInfo};
use filmcraft_project as fp;
use serde::{Deserialize, Serialize};

use crate::{Error, Result, TimelineFormat, par_fraction, rate_out, tick_out};

/// Which layers are pre-rendered to media (ProRes 4444 with alpha) instead of exported as clips.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PrerenderMode {
    /// Nothing is rendered: layers only EffectCraft can draw are left out (with a warning), and
    /// effects, masks, parenting… of footage layers are dropped.
    None,
    /// Layers a timeline cannot represent (text, shapes, effects, masks, 3D, parenting, track
    /// mattes, expressions, time remapping…) are pre-rendered.
    #[default]
    Unsupported,
    /// Every visual layer is pre-rendered (what you see is what Premiere shows).
    All,
}

impl PrerenderMode {
    pub fn parse(s: &str) -> Option<PrerenderMode> {
        match s.to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
            "none" | "off" | "false" | "no" => Some(PrerenderMode::None),
            "unsupported" | "renderedonly" | "auto" | "true" | "yes" => Some(PrerenderMode::Unsupported),
            "all" | "everything" => Some(PrerenderMode::All),
            _ => None,
        }
    }
}

/// How precomposition layers are exported.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PrecompMode {
    /// As nested sequences (their layers become clips too).
    #[default]
    Nest,
    /// Pre-rendered like other rendered-only layers.
    Prerender,
}

/// Export options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ExportOptions {
    pub format: TimelineFormat,
    pub prerender: PrerenderMode,
    pub precomps: PrecompMode,
    /// Write media paths relative to this folder where the format allows (OTIO, EDL).
    pub relative_to: Option<String>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions { format: TimelineFormat::Fcp7Xml, prerender: PrerenderMode::Unsupported, precomps: PrecompMode::Nest, relative_to: None }
    }
}

/// A layer pre-rendered by the caller: a comp-sized movie of the layer alone (transform baked in)
/// covering the layer's in to out point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prerendered {
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub frame_rate: FrameRate,
    pub duration: Tick,
}

/// The written document.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ExportOutput {
    pub bytes: Vec<u8>,
    pub warnings: Vec<String>,
    /// Sequences written (the comp and nested precomps).
    pub sequences: usize,
    pub video_tracks: usize,
    pub audio_tracks: usize,
    pub clips: usize,
    /// Layers referenced as pre-rendered media.
    pub prerendered: usize,
}

/// How one layer is exported.
#[derive(Clone, Debug, PartialEq)]
enum Plan {
    /// A clip of its footage / a colour matte, with Motion and Opacity.
    Native,
    /// A nested sequence of its precomp.
    Nest,
    /// Pre-rendered media (reason).
    Render(String),
    /// Audio only (no picture).
    AudioOnly,
    /// Left out (reason, or empty for silently skipped layers such as cameras).
    Skip(String),
}

/// Why a layer can't be a plain clip (None = it can), regardless of the mode.
fn unsupported(project: &Project, comp: &Comp, l: &Layer, format: TimelineFormat) -> Option<String> {
    let kind = match &l.source {
        LayerSource::Text => Some("text layer"),
        LayerSource::Shape => Some("shape layer"),
        LayerSource::Model { .. } | LayerSource::Primitive { .. } => Some("3D model layer"),
        LayerSource::Solid { item } => match project.item(*item).map(|i| &i.kind) {
            Some(ItemKind::Solid(s)) if s.width != comp.width || s.height != comp.height => Some("solid smaller or larger than the frame"),
            Some(ItemKind::Solid(_)) if format != TimelineFormat::Fcp7Xml => Some("solid (the format has no colour mattes)"),
            _ => None,
        },
        _ => None,
    };
    if let Some(k) = kind {
        return Some(k.into());
    }
    if l.effects().is_some_and(|e| !e.children.is_empty()) {
        return Some("effects".into());
    }
    if l.masks().is_some_and(|m| !m.children.is_empty()) {
        return Some("masks".into());
    }
    if l.track_matte.is_some() {
        return Some("track matte".into());
    }
    if l.parent.is_some() {
        return Some("parenting".into());
    }
    if l.is_3d() {
        return Some("3D layer".into());
    }
    if l.props.sub("layerStyles").is_some_and(|g| !g.children.is_empty()) {
        return Some("layer styles".into());
    }
    if l.props.get("timeRemap").is_some_and(|p| p.is_animated()) {
        return Some("time remapping".into());
    }
    let mut expr = false;
    if let Some(t) = l.transform() {
        t.walk("", &mut |_, p| expr |= p.has_expression());
    }
    if expr {
        return Some("expressions".into());
    }
    if l.blend_mode != effectcraft_color::BlendMode::Normal && !fp::effect::BLEND_MODES.iter().any(|m| m.eq_ignore_ascii_case(l.blend_mode.label())) {
        return Some(format!("blend mode {}", l.blend_mode.label()));
    }
    // Premiere's Basic Motion in the interchange formats has one (uniform) scale.
    if let Some(s) = l.transform().and_then(|t| t.get("scale"))
        && std::iter::once(&s.value).chain(s.keys.iter().map(|k| &k.value)).any(|v| {
            let a = v.as_vec3();
            (a[0] - a[1]).abs() > 1e-9
        })
    {
        return Some("non-uniform scale".into());
    }
    if l.preserve_transparency {
        return Some("preserve underlying transparency".into());
    }
    None
}

fn plan(project: &Project, comp: &Comp, l: &Layer, opts: &ExportOptions) -> Plan {
    if l.switches.guide {
        return Plan::Skip(String::new());
    }
    match &l.source {
        LayerSource::Camera | LayerSource::Light { .. } | LayerSource::Null => return Plan::Skip(String::new()),
        LayerSource::Footage { item } => match project.item(*item).map(|i| &i.kind) {
            Some(ItemKind::Footage(f)) if f.kind == FootageKind::Audio || !f.has_video => return Plan::AudioOnly,
            Some(ItemKind::Footage(f)) if f.kind == FootageKind::Data => return Plan::Skip("data footage has no picture".into()),
            _ => {}
        },
        _ => {}
    }
    if l.switches.adjustment {
        return Plan::Skip("adjustment layer (its effects apply to the layers below)".into());
    }
    let why = unsupported(project, comp, l, opts.format);
    match opts.prerender {
        PrerenderMode::All => Plan::Render("pre-render all layers".into()),
        PrerenderMode::Unsupported => match (&l.source, why) {
            (_, Some(w)) => Plan::Render(w),
            (LayerSource::Comp { .. }, None) if opts.precomps == PrecompMode::Prerender => Plan::Render("precomp".into()),
            (LayerSource::Comp { .. }, None) => Plan::Nest,
            _ => Plan::Native,
        },
        PrerenderMode::None => match (&l.source, why) {
            (LayerSource::Text | LayerSource::Shape | LayerSource::Model { .. } | LayerSource::Primitive { .. }, Some(w)) => Plan::Skip(w),
            (LayerSource::Solid { .. }, Some(w)) => Plan::Skip(w),
            (LayerSource::Comp { .. }, _) => Plan::Nest,
            _ => Plan::Native,
        },
    }
}

/// Why `layer` would be pre-rendered under `opts` (None when it exports as a clip or is left out).
pub fn prerender_reason(project: &Project, comp: ItemId, layer: LayerId, opts: &ExportOptions) -> Option<String> {
    let c = project.comp(comp)?;
    match plan(project, c, c.layer(layer)?, opts) {
        Plan::Render(r) => Some(r),
        _ => None,
    }
}

/// The layers the caller must pre-render before [`export`]: `(comp, layer)` pairs, nested
/// precomps included when they are exported as nested sequences.
pub fn plan_prerender(project: &Project, comp: ItemId, opts: &ExportOptions) -> Vec<(ItemId, LayerId)> {
    let mut out = vec![];
    let mut seen = vec![];
    fn walk(p: &Project, cid: ItemId, opts: &ExportOptions, out: &mut Vec<(ItemId, LayerId)>, seen: &mut Vec<ItemId>) {
        if seen.contains(&cid) {
            return;
        }
        seen.push(cid);
        let Some(c) = p.comp(cid) else { return };
        for l in &c.layers {
            match plan(p, c, l, opts) {
                Plan::Render(_) => out.push((cid, l.id)),
                Plan::Nest => {
                    if let LayerSource::Comp { item } = l.source {
                        walk(p, item, opts, out, seen);
                    }
                }
                _ => {}
            }
        }
    }
    walk(project, comp, opts, &mut out, &mut seen);
    out
}

/// Export composition `comp` (with the precomps it nests) as a timeline document.
pub fn export(project: &Project, comp: ItemId, opts: &ExportOptions, prerendered: &HashMap<(ItemId, LayerId), Prerendered>) -> Result<ExportOutput> {
    project.comp(comp).ok_or(Error::NoComp(comp.0))?;
    let name = project.item(comp).map(|i| i.name.clone()).unwrap_or_else(|| "Composition".into());
    let mut ex = Exporter {
        p: project,
        opts,
        pre: prerendered,
        fp: fp::Project::new(&name),
        out: ExportOutput::default(),
        seqs: HashMap::new(),
        media: HashMap::new(),
        bins: HashMap::new(),
        stack: vec![],
    };
    let seq = ex.sequence(comp).ok_or(Error::NoComp(comp.0))?;
    let fc_opts = filmcraft_interchange::ExportOptions { relative_to: opts.relative_to.clone(), name: Some(name), ..Default::default() };
    let (bytes, report) = filmcraft_interchange::export(&ex.fp, seq, opts.format.fc(), &fc_opts)?;
    for e in report.entries {
        let m = if e.count > 1 { format!("{} (×{})", e.message, e.count) } else { e.message };
        ex.warn(m);
    }
    ex.out.bytes = bytes;
    Ok(ex.out)
}

struct Exporter<'a> {
    p: &'a Project,
    opts: &'a ExportOptions,
    pre: &'a HashMap<(ItemId, LayerId), Prerendered>,
    fp: fp::Project,
    out: ExportOutput,
    seqs: HashMap<ItemId, fp::ItemId>,
    /// Media items by (path or solid key).
    media: HashMap<String, fp::ItemId>,
    bins: HashMap<ItemId, fp::BinId>,
    stack: Vec<ItemId>,
}

/// Clip timing derived from a layer.
struct Timing {
    start: Tick,
    duration: Tick,
    source_in: Tick,
    speed: f64,
    reverse: bool,
}

fn timing(l: &Layer, comp: &Comp) -> Option<Timing> {
    let a = l.in_point.max(Tick::ZERO);
    let b = l.out_point.min(comp.duration);
    if b <= a {
        return None;
    }
    let reverse = l.stretch < 0.0;
    let speed = if l.stretch.abs() < 1e-9 { 1.0 } else { 100.0 / l.stretch.abs() };
    let (la, lb) = (l.layer_time(a), l.layer_time(b));
    Some(Timing { start: a, duration: b - a, source_in: la.min(lb), speed, reverse })
}

impl Exporter<'_> {
    fn warn(&mut self, m: impl Into<String>) {
        let m = m.into();
        if !self.out.warnings.contains(&m) {
            self.out.warnings.push(m);
        }
    }

    /// The bin mirroring a Project panel folder.
    fn bin(&mut self, folder: Option<ItemId>) -> Option<fp::BinId> {
        let f = folder?;
        if let Some(b) = self.bins.get(&f) {
            return Some(*b);
        }
        let it = self.p.item(f)?;
        let parent = self.bin(it.parent);
        let b = self.fp.add_bin(&it.name, parent);
        self.bins.insert(f, b);
        Some(b)
    }

    fn footage_item(&mut self, item: ItemId, f: &Footage) -> fp::ItemId {
        let path = if f.kind == FootageKind::Sequence { f.sequence.first().cloned().unwrap_or_else(|| f.path.clone()) } else { f.path.clone() };
        if let Some(id) = self.media.get(&path) {
            return *id;
        }
        let it = self.p.item(item);
        let name = it.map(|i| i.name.clone()).unwrap_or_else(|| path.clone());
        let kind = match f.kind {
            FootageKind::Audio => MediaKind::AudioOnly,
            FootageKind::Still => MediaKind::Still,
            FootageKind::Sequence => MediaKind::ImageSequence,
            _ if !f.has_video => MediaKind::AudioOnly,
            _ => MediaKind::Movie,
        };
        let duration = if f.kind == FootageKind::Still { Tick::from_seconds_f64(3600.0) } else { f.duration };
        let info = media_info(
            &name,
            kind,
            duration,
            f.has_video.then(|| (f.width, f.height, f.frame_rate, f.pixel_aspect, f.codec.clone(), f.alpha != effectcraft_project::AlphaMode::Ignore)),
            f.has_audio,
        );
        let mut clip = media_clip(fp::MediaRef::File { path: path.clone() }, info);
        clip.offline = f.missing;
        let bin = self.bin(it.and_then(|i| i.parent));
        let id = self.fp.add_item(&name, fp::Label::Iris, fp::ItemKind::Media(clip), bin);
        self.media.insert(path, id);
        id
    }

    fn prerendered_item(&mut self, name: &str, r: &Prerendered) -> fp::ItemId {
        if let Some(id) = self.media.get(&r.path) {
            return *id;
        }
        let info = media_info(name, MediaKind::Movie, r.duration, Some((r.width, r.height, r.frame_rate, 1.0, "Apple ProRes 4444".into(), true)), false);
        let clip = media_clip(fp::MediaRef::File { path: r.path.clone() }, info);
        let bin = match self.bins.get(&ItemId(u64::MAX)) {
            Some(b) => Some(*b),
            None => {
                let b = self.fp.add_bin("Pre-rendered Layers", None);
                self.bins.insert(ItemId(u64::MAX), b);
                Some(b)
            }
        };
        let id = self.fp.add_item(name, fp::Label::Violet, fp::ItemKind::Media(clip), bin);
        self.media.insert(r.path.clone(), id);
        id
    }

    fn solid_item(&mut self, item: ItemId, color: [f32; 3], comp: &Comp) -> fp::ItemId {
        let key = format!("solid:{}:{}x{}", item.0, comp.width, comp.height);
        if let Some(id) = self.media.get(&key) {
            return *id;
        }
        let it = self.p.item(item);
        let name = it.map(|i| i.name.clone()).unwrap_or_else(|| "Color Matte".into());
        let info = media_info(
            &name,
            MediaKind::Synthetic,
            Tick::from_seconds_f64(3600.0),
            Some((comp.width, comp.height, comp.frame_rate, 1.0, String::new(), false)),
            false,
        );
        let clip = media_clip(fp::MediaRef::Generator(Generator::ColorMatte { color: [color[0], color[1], color[2], 1.0] }), info);
        let bin = self.bin(it.and_then(|i| i.parent));
        let id = self.fp.add_item(&name, fp::Label::Rose, fp::ItemKind::Media(clip), bin);
        self.media.insert(key, id);
        id
    }

    /// The sequence for a comp (created once); None if `cid` is not a composition.
    fn sequence(&mut self, cid: ItemId) -> Option<fp::ItemId> {
        if let Some(s) = self.seqs.get(&cid) {
            return Some(*s);
        }
        let comp = self.p.comp_arc(cid)?;
        let it = self.p.item(cid);
        let name = it.map(|i| i.name.clone()).unwrap_or_default();
        let settings = fp::SequenceSettings {
            width: comp.width,
            height: comp.height,
            frame_rate: rate_out(comp.frame_rate),
            par: par_fraction(comp.pixel_aspect),
            preset: "Custom".into(),
            ..Default::default()
        };
        let bin = if self.stack.is_empty() { None } else { self.bin(it.and_then(|i| i.parent)) };
        let sid = self.fp.new_sequence(&name, settings, 0, 0, bin);
        self.seqs.insert(cid, sid);
        self.stack.push(cid);
        self.out.sequences += 1;
        let rate = comp.frame_rate;
        let mut video: Vec<fp::Track> = vec![];
        let mut audio: Vec<fp::Track> = vec![];
        for l in comp.layers.iter().rev() {
            let pl = plan(self.p, &comp, l, self.opts);
            let Some(tm) = timing(l, &comp) else { continue };
            let link = self.fp.alloc_id();
            let mut has_video_clip = false;
            let clip = match &pl {
                Plan::Skip(why) => {
                    if !why.is_empty() {
                        self.warn(format!("layer \"{}\" was left out ({why})", l.name));
                    }
                    None
                }
                Plan::AudioOnly => None,
                Plan::Render(why) => match self.pre.get(&(cid, l.id)).cloned() {
                    Some(r) => {
                        let item = self.prerendered_item(&format!("{} (pre-rendered)", l.name), &r);
                        self.track_item(item, &l.name, fp::TrackKind::Video, &tm, rate).map(|mut ti| {
                            ti.source_in = filmcraft_time::Tick::ZERO;
                            ti.speed = 1.0;
                            ti.reverse = false;
                            self.out.prerendered += 1;
                            ti
                        })
                    }
                    None => {
                        self.warn(format!("layer \"{}\" was left out ({why}; pre-rendering is off or unavailable)", l.name));
                        None
                    }
                },
                Plan::Native | Plan::Nest => {
                    if self.opts.prerender == PrerenderMode::None
                        && let Some(w) = unsupported(self.p, &comp, l, self.opts.format)
                    {
                        self.warn(format!("layer \"{}\": {w} not exported", l.name));
                    }
                    let item = match &l.source {
                        LayerSource::Footage { item } => match self.p.item(*item).map(|i| &i.kind) {
                            Some(ItemKind::Footage(f)) => Some(self.footage_item(*item, &f.clone())),
                            _ => None,
                        },
                        LayerSource::Solid { item } => match self.p.item(*item).map(|i| &i.kind) {
                            Some(ItemKind::Solid(s)) => Some(self.solid_item(*item, s.color, &comp)),
                            _ => None,
                        },
                        LayerSource::Comp { item } if !self.stack.contains(item) => self.sequence(*item),
                        _ => None,
                    };
                    item.and_then(|item| {
                        let mut ti = self.track_item(item, &l.name, fp::TrackKind::Video, &tm, rate)?;
                        self.motion(&mut ti, l, &comp, &tm);
                        if let Some(h) = l.props.get("timeRemap").filter(|p| !p.is_animated()) {
                            ti.frame_hold = Some(tick_out(Tick::from_seconds_f64(h.value.as_f64())));
                        }
                        Some(ti)
                    })
                }
            };
            if let Some(mut ti) = clip {
                ti.enabled = l.switches.video;
                ti.link = Some(link);
                let mut t = fp::Track::new(fp::TrackId(self.fp.alloc_id()), fp::TrackKind::Video, format!("Video {}", video.len() + 1));
                t.items.push(ti);
                video.push(t);
                has_video_clip = true;
                self.out.clips += 1;
            }
            // Sound: footage and precomps with audio.
            let sound = match &l.source {
                LayerSource::Footage { item } => match self.p.item(*item).map(|i| &i.kind) {
                    Some(ItemKind::Footage(f)) if f.has_audio => Some(self.footage_item(*item, &f.clone())),
                    _ => None,
                },
                LayerSource::Comp { item } if self.p.comp(*item).is_some_and(|c| comp_has_audio(self.p, c)) && !self.stack.contains(item) => {
                    self.sequence(*item)
                }
                _ => None,
            };
            if let Some(item) = sound
                && !matches!(pl, Plan::Skip(_))
                && let Some(mut ti) = self.track_item(item, &l.name, fp::TrackKind::Audio, &tm, rate)
            {
                ti.enabled = l.switches.audio;
                ti.link = has_video_clip.then_some(link);
                self.levels(&mut ti, l, &tm);
                let mut t = fp::Track::new(fp::TrackId(self.fp.alloc_id()), fp::TrackKind::Audio, format!("Audio {}", audio.len() + 1));
                t.items.push(ti);
                audio.push(t);
                self.out.clips += 1;
            }
        }
        self.out.video_tracks += video.len();
        self.out.audio_tracks += audio.len();
        if video.is_empty() {
            video.push(fp::Track::new(fp::TrackId(self.fp.alloc_id()), fp::TrackKind::Video, "Video 1".into()));
        }
        let markers: Vec<fp::Marker> = comp
            .markers
            .iter()
            .map(|m| fp::Marker {
                id: fp::MarkerId(self.fp.alloc_id()),
                start: tick_out(m.time),
                duration: tick_out(m.duration),
                name: if m.chapter.is_empty() { m.comment.clone() } else { m.chapter.clone() },
                comment: if m.chapter.is_empty() { String::new() } else { m.comment.clone() },
                kind: if m.chapter.is_empty() { fp::MarkerKind::Comment } else { fp::MarkerKind::Chapter },
                color: fp::Label::Green,
            })
            .collect();
        if let Some(seq) = self.fp.sequence_mut(sid) {
            seq.video_tracks = video;
            seq.audio_tracks = audio;
            seq.markers = markers;
            seq.start_timecode = rate.frame_at(comp.display_start);
            seq.work_area = None;
        }
        self.stack.pop();
        Some(sid)
    }

    fn track_item(&mut self, item: fp::ItemId, name: &str, kind: fp::TrackKind, tm: &Timing, rate: FrameRate) -> Option<fp::TrackItem> {
        let range = filmcraft_time::TimeRange::new(tick_out(tm.source_in), tick_out(Tick((tm.duration.0 as f64 * tm.speed).round() as i64)));
        let mut ti = self.fp.make_track_item(item, kind, tick_out(tm.start), range, rate_out(rate))?;
        ti.name = name.to_string();
        ti.duration = tick_out(tm.duration);
        ti.source_in = tick_out(tm.source_in);
        ti.speed = tm.speed;
        ti.reverse = tm.reverse;
        Some(ti)
    }

    /// Transform → Motion + Opacity (clip-time keyframes).
    fn motion(&mut self, ti: &mut fp::TrackItem, l: &Layer, comp: &Comp, tm: &Timing) {
        let Some(tr) = l.transform() else { return };
        let to_clip = |t: Tick| tick_out(l.comp_time(t) - tm.start);
        let (sw, sh) = match &l.source {
            LayerSource::Footage { item } | LayerSource::Comp { item } | LayerSource::Solid { item } => {
                self.p.item(*item).and_then(|i| i.dimensions()).unwrap_or((comp.width, comp.height))
            }
            _ => (comp.width, comp.height),
        };
        let center = |x: f64, y: f64| -> fp::ParamValue { fp::ParamValue::Vec2(Vec2 { x, y }) };
        let Some(mut m) = effect_or_new(ti, "motion") else { return };
        if let Some(p) = tr.get("position") {
            let def = [comp.width as f64 / 2.0, comp.height as f64 / 2.0];
            if p.is_animated() || differs(&p.value, def) {
                m.params.insert(
                    "position".into(),
                    param_out(p, &to_clip, |v| {
                        let a = v.as_vec3();
                        center(a[0], a[1])
                    }),
                );
            }
        }
        if let Some(p) = tr.get("anchor") {
            let def = [sw as f64 / 2.0, sh as f64 / 2.0];
            if p.is_animated() || differs(&p.value, def) {
                m.params.insert(
                    "anchor".into(),
                    param_out(p, &to_clip, |v| {
                        let a = v.as_vec3();
                        center(a[0], a[1])
                    }),
                );
            }
        }
        if let Some(p) = tr.get("scale") {
            let uniform = std::iter::once(&p.value).chain(p.keys.iter().map(|k| &k.value)).all(|v| {
                let a = v.as_vec3();
                (a[0] - a[1]).abs() < 1e-9
            });
            m.params.insert("scale".into(), param_out(p, &to_clip, |v| fp::ParamValue::Float(v.as_vec3()[1])));
            if !uniform {
                m.params.insert("scale_width".into(), param_out(p, &to_clip, |v| fp::ParamValue::Float(v.as_vec3()[0])));
                m.params.insert("uniform_scale".into(), fp::Param::new(fp::ParamValue::Bool(false)));
            }
        }
        if let Some(p) = tr.get("rotation") {
            m.params.insert("rotation".into(), param_out(p, &to_clip, |v| fp::ParamValue::Float(v.as_f64())));
        }
        set_effect(ti, m);
        if let Some(p) = tr.get("opacity")
            && let Some(mut o) = effect_or_new(ti, "opacity")
        {
            o.params.insert("opacity".into(), param_out(p, &to_clip, |v| fp::ParamValue::Float(v.as_f64())));
            if l.blend_mode != effectcraft_color::BlendMode::Normal
                && let Some(i) = fp::effect::BLEND_MODES.iter().position(|m| m.eq_ignore_ascii_case(l.blend_mode.label()))
            {
                o.params.insert("blend".into(), fp::Param::new(fp::ParamValue::Choice(i as u32)));
            }
            set_effect(ti, o);
        }
    }

    fn levels(&mut self, ti: &mut fp::TrackItem, l: &Layer, tm: &Timing) {
        let Some(p) = l.props.prop("audio/levels") else { return };
        let to_clip = |t: Tick| tick_out(l.comp_time(t) - tm.start);
        if !p.is_animated() && p.value.as_vec2() == [0.0, 0.0] {
            return;
        }
        if (p.value.as_vec2()[0] - p.value.as_vec2()[1]).abs() > 1e-9 {
            self.warn("separate left/right audio levels were exported as the left level");
        }
        let Some(mut v) = effect_or_new(ti, "volume") else { return };
        v.params.insert("level".into(), param_out(p, &to_clip, |v| fp::ParamValue::Float(v.as_vec2()[0])));
        set_effect(ti, v);
    }
}

fn comp_has_audio(p: &Project, c: &Comp) -> bool {
    c.layers.iter().any(|l| match &l.source {
        LayerSource::Footage { item } => matches!(p.item(*item).map(|i| &i.kind), Some(ItemKind::Footage(f)) if f.has_audio),
        LayerSource::Comp { item } => p.comp(*item).is_some_and(|c| comp_has_audio(p, c)),
        _ => false,
    })
}

fn differs(v: &Value, def: [f64; 2]) -> bool {
    let a = v.as_vec3();
    (a[0] - def[0]).abs() > 1e-6 || (a[1] - def[1]).abs() > 1e-6
}

/// The clip's `id` effect, or a new instance of it (None if the registry has no such effect).
fn effect_or_new(ti: &fp::TrackItem, id: &str) -> Option<fp::EffectInstance> {
    ti.effect(id).cloned().or_else(|| fp::find_effect(id).map(|e| e.instance()))
}

fn set_effect(ti: &mut fp::TrackItem, e: fp::EffectInstance) {
    match ti.effects.iter_mut().find(|x| x.effect == e.effect) {
        Some(x) => *x = e,
        None => ti.effects.push(e),
    }
}

/// A property → a FilmCraft parameter (layer-time keys → clip-time keys).
fn param_out(p: &Property, to_clip: &impl Fn(Tick) -> filmcraft_time::Tick, conv: impl Fn(&Value) -> fp::ParamValue) -> fp::Param {
    let mut out = fp::Param::new(conv(&p.value));
    for k in &p.keys {
        let mut fk = fp::Keyframe::new(to_clip(k.time), conv(&k.value));
        fk.interp = match k.out_interp {
            Interp::Linear => fp::Interpolation::Linear,
            Interp::Hold => fp::Interpolation::Hold,
            Interp::Bezier => fp::Interpolation::Bezier,
        };
        out.keyframes.push(fk);
    }
    out.keyframes.sort_by_key(|k| k.time);
    if let Some(k) = out.keyframes.first() {
        out.value = k.value.clone();
    }
    out
}

type VideoSpec = (u32, u32, FrameRate, f64, String, bool);

fn media_info(name: &str, kind: MediaKind, duration: Tick, video: Option<VideoSpec>, audio: bool) -> MediaInfo {
    MediaInfo {
        name: name.into(),
        kind,
        duration: tick_out(duration),
        video: video.map(|(width, height, rate, par, codec, has_alpha)| VideoStreamInfo {
            width,
            height,
            frame_rate: rate_out(rate),
            par: par_fraction(par),
            codec,
            pixel_format: String::new(),
            color: Default::default(),
            has_alpha,
            bitrate: None,
            hdr: None,
        }),
        audio_streams: audio
            .then(|| AudioStreamInfo { sample_rate: 48_000, channels: 2, codec: "pcm".into(), bits_per_sample: Some(16) })
            .into_iter()
            .collect(),
        container: String::new(),
        start_timecode: None,
        file_size: None,
    }
}

fn media_clip(media: fp::MediaRef, info: MediaInfo) -> fp::MediaClip {
    fp::MediaClip { media, info, interpret: Default::default(), mark_in: None, mark_out: None, markers: vec![], offline: false, proxy: None, identity: None }
}
