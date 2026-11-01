//! EffectCraft composition → Motion document.

use std::collections::HashMap;

use effectcraft_color::BlendMode;
use effectcraft_project::{Comp, Footage, FootageKind, GroupKind, ItemId, ItemKind, Layer, LayerSource, Project, PropGroup, Property};
use effectcraft_time::{FrameRate, Tick};

use crate::curve::{Channel, channel_of};
use crate::filters::{FilterOut, export_effect};
use crate::xml::{Writer, num};
use crate::{MAX_DEPTH, MotionError, fmt_time, rate_fields};

/// Factory ids we declare (Motion's factory UUIDs for these object kinds).
const FACTORY_IMAGE: u32 = 1;
const FACTORY_GENERATOR: u32 = 2;
const FACTORY_FILTER: u32 = 3;
const FACTORIES: [(u32, &str, &str); 3] = [
    (FACTORY_IMAGE, "66fc0d6af6a911d6a7a7000393670732", "Image"),
    (FACTORY_GENERATOR, "615c4bf6406511d88802000a95af90f2", "Generator"),
    (FACTORY_FILTER, "deca4859b16011d7a12d0003936f6f92", "ProPlugin Filter"),
];
const COLOR_SOLID_UUID: &str = "C18E8B62-C24F-4F8D-8597-858675A40BBC";

/// An exported Motion document and what could not be expressed.
#[derive(Clone, Debug)]
pub struct ExportResult {
    pub xml: String,
    pub warnings: Vec<String>,
}

struct Clip<'a> {
    id: u64,
    name: String,
    footage: &'a Footage,
}

struct Ex<'a> {
    project: &'a Project,
    rate: FrameRate,
    size: (u32, u32),
    doc_dir: String,
    next_id: u64,
    clips: Vec<Clip<'a>>,
    clip_ids: HashMap<ItemId, u64>,
    warnings: Vec<String>,
}

impl<'a> Ex<'a> {
    fn warn(&mut self, w: impl Into<String>) {
        let w = w.into();
        if !self.warnings.contains(&w) {
            self.warnings.push(w);
        }
    }

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn clip(&mut self, item: ItemId) -> Option<(u64, &'a Footage)> {
        let project = self.project;
        let it = project.item(item)?;
        let ItemKind::Footage(f) = &it.kind else { return None };
        if let Some(id) = self.clip_ids.get(&item) {
            return Some((*id, f));
        }
        let id = self.id();
        self.clip_ids.insert(item, id);
        self.clips.push(Clip { id, name: it.name.clone(), footage: f });
        Some((id, f))
    }
}

/// `%`-encode a path for a `file://` URL.
fn url_path(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    for b in p.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `path` relative to `dir` when it lies inside it.
fn relative_to(path: &str, dir: &str) -> Option<String> {
    if dir.is_empty() {
        return None;
    }
    let dir = dir.trim_end_matches(['/', '\\']);
    let rest = path.strip_prefix(dir)?;
    let rest = rest.strip_prefix(['/', '\\'])?;
    (!rest.is_empty()).then(|| rest.replace('\\', "/"))
}

fn timing(w: &mut Writer, tin: Tick, out: Tick, offset: Tick) {
    w.empty("timing", &[("in", fmt_time(tin)), ("out", fmt_time(out.max(tin))), ("offset", fmt_time(offset))]);
}

/// A scalar parameter: a constant, or a curve.
fn channel(w: &mut Writer, name: &str, id: u32, ch: &Channel, default: f64) {
    let base = [("name", name.to_string()), ("id", id.to_string()), ("flags", "16".to_string())];
    if ch.keys.is_empty() {
        let mut a = base.to_vec();
        a.push(("default", num(default)));
        a.push(("value", num(ch.value)));
        w.empty("parameter", &a);
        return;
    }
    w.open("parameter", &base);
    w.open("curve", &[("type", "1".into()), ("default", num(default)), ("value", num(ch.value))]);
    w.text("numberOfKeypoints", &ch.keys.len().to_string());
    let last = ch.keys.len().saturating_sub(1);
    for (i, k) in ch.keys.iter().enumerate() {
        let mut a = vec![];
        if let Some(c) = k.code {
            a.push(("interpolation", c.to_string()));
        }
        a.push(("flags", "32".to_string()));
        w.open("keypoint", &a);
        w.text("time", &fmt_time(Tick::from_seconds_f64(k.t)));
        w.text("value", &num(k.v));
        if let Some((t, v)) = k.tin.filter(|_| i > 0) {
            w.text("inputTangentTime", &num(t));
            w.text("inputTangentValue", &num(v));
        }
        if let Some((t, v)) = k.tout.filter(|_| i < last) {
            w.text("outputTangentTime", &num(t));
            w.text("outputTangentValue", &num(v));
        }
        w.close("keypoint");
    }
    w.close("curve");
    w.close("parameter");
}

fn vector(w: &mut Writer, name: &str, id: u32, chs: &[(&str, u32, Channel, f64)]) {
    w.open("parameter", &[("name", name.to_string()), ("id", id.to_string()), ("flags", "16".to_string())]);
    for (n, i, ch, d) in chs {
        channel(w, n, *i, ch, *d);
    }
    w.close("parameter");
}

fn prop<'p>(g: &'p PropGroup, m: &str) -> Option<&'p Property> {
    g.get(m)
}

/// The Properties ▸ Transform and Blending parameters of a layer. `anchor_center` is the
/// layer-space point of Motion's anchor (0, 0); `extra` writes more Properties children.
fn properties(ex: &mut Ex, w: &mut Writer, l: &Layer, anchor_center: [f64; 2], extra: &dyn Fn(&mut Writer)) {
    let (pw, ph) = (ex.size.0 as f64 / 2.0, ex.size.1 as f64 / 2.0);
    let deg = 180.0 / std::f64::consts::PI;
    let three_d = l.switches.three_d;
    w.open("parameter", &[("name", "Properties".into()), ("id", "1".into()), ("flags", "16".into())]);
    if let Some(t) = l.props.sub("transform") {
        for p in t.children.iter().filter_map(|c| if let effectcraft_project::Node::Prop(p) = c { Some(p) } else { None }) {
            if p.expr.as_ref().is_some_and(|e| e.enabled && !e.text.trim().is_empty()) {
                ex.warn(format!("{}: expressions are not exported (the {} value without its expression is)", l.name, p.name));
            }
        }
        let zero = Channel::constant(0.0);
        let pos = match (prop(t, "positionX"), prop(t, "positionY")) {
            (Some(x), Some(y)) => [
                channel_of(x, 0, 1.0, pw),
                channel_of(y, 0, -1.0, ph),
                prop(t, "positionZ").filter(|_| three_d).map(|z| channel_of(z, 0, -1.0, 0.0)).unwrap_or(zero.clone()),
            ],
            _ => match prop(t, "position") {
                Some(p) => [channel_of(p, 0, 1.0, pw), channel_of(p, 1, -1.0, ph), if three_d { channel_of(p, 2, -1.0, 0.0) } else { zero.clone() }],
                None => [zero.clone(), zero.clone(), zero.clone()],
            },
        };
        let rot = [
            prop(t, "rotationX").filter(|_| three_d).map(|p| channel_of(p, 0, deg, 0.0)).unwrap_or(zero.clone()),
            prop(t, "rotationY").filter(|_| three_d).map(|p| channel_of(p, 0, -deg, 0.0)).unwrap_or(zero.clone()),
            prop(t, "rotation").map(|p| channel_of(p, 0, -deg, 0.0)).unwrap_or(zero.clone()),
        ];
        if three_d && prop(t, "orientation").is_some_and(|o| !o.keys.is_empty() || o.value.components().iter().any(|v| *v != 0.0)) {
            ex.warn(format!("{}: Orientation is not exported (use X/Y/Z Rotation)", l.name));
        }
        let one = Channel::constant(1.0);
        let scale = match prop(t, "scale") {
            Some(p) => [channel_of(p, 0, 100.0, 0.0), channel_of(p, 1, 100.0, 0.0), if three_d { channel_of(p, 2, 100.0, 0.0) } else { one.clone() }],
            None => [one.clone(), one.clone(), one.clone()],
        };
        let anchor = match prop(t, "anchor") {
            Some(p) => [
                channel_of(p, 0, 1.0, anchor_center[0]),
                channel_of(p, 1, -1.0, anchor_center[1]),
                if three_d { channel_of(p, 2, -1.0, 0.0) } else { zero.clone() },
            ],
            None => [zero.clone(), zero.clone(), zero.clone()],
        };
        let xyz = |c: [Channel; 3], d: f64| -> Vec<(&'static str, u32, Channel, f64)> {
            let [x, y, z] = c;
            vec![("X", 1, x, d), ("Y", 2, y, d), ("Z", 3, z, d)]
        };
        w.open("parameter", &[("name", "Transform".into()), ("id", "100".into()), ("flags", "16".into())]);
        vector(w, "Position", 101, &xyz(pos, 0.0));
        vector(w, "Rotation", 109, &xyz(rot, 0.0));
        vector(w, "Scale", 105, &xyz(scale, 1.0));
        vector(w, "Anchor Point", 106, &xyz(anchor, 0.0));
        w.close("parameter");
        let opacity = prop(t, "opacity").map(|p| channel_of(p, 0, 100.0, 0.0)).unwrap_or(one);
        w.open("parameter", &[("name", "Blending".into()), ("id", "200".into()), ("flags", "16".into())]);
        channel(w, "Opacity", 202, &opacity, 1.0);
        w.close("parameter");
    }
    extra(w);
    w.close("parameter");
}

/// Motion filters for a layer's effects (unsupported effects become warnings).
fn effect_filters(ex: &mut Ex, l: &Layer) -> Vec<FilterOut> {
    let mut out = vec![];
    let Some(fx) = l.props.sub("effects") else { return out };
    for c in &fx.children {
        let effectcraft_project::Node::Group(g) = c else { continue };
        if !matches!(g.kind, GroupKind::Effect { .. }) {
            continue;
        }
        match export_effect(g) {
            Some(f) => out.push(f),
            None => ex.warn(format!("{}: effect `{}` has no Motion filter we can write (skipped)", l.name, g.name)),
        }
    }
    out
}

fn write_filter(ex: &mut Ex, w: &mut Writer, f: &FilterOut, span: (Tick, Tick, Tick)) {
    let id = ex.id();
    w.open(
        "filter",
        &[
            ("name", f.name.clone()),
            ("id", id.to_string()),
            ("factoryID", FACTORY_FILTER.to_string()),
            ("pluginUUID", f.uuid.to_string()),
            ("pluginVersion", "1".to_string()),
            ("pluginName", f.plugin.to_string()),
        ],
    );
    timing(w, span.0, span.1, span.2);
    if !f.enabled {
        w.text("enabled", "0");
    }
    for (name, pid, ch) in &f.params {
        channel(w, name, *pid, ch, ch.value);
    }
    if let Some(m) = &f.mix {
        channel(w, "Mix", crate::filters::MIX_ID, m, 1.0);
    }
    w.close("filter");
}

/// Motion's inclusive out frame from an exclusive out point.
fn last_frame(ex: &Ex, l: &Layer) -> Tick {
    Tick(l.out_point.0.saturating_sub(ex.rate.frame_duration().0)).max(l.in_point)
}

fn layer_warnings(ex: &mut Ex, l: &Layer) {
    if l.parent.is_some() {
        ex.warn(format!("{}: parenting is not exported (Motion parents by grouping)", l.name));
    }
    if l.track_matte.is_some() {
        ex.warn(format!("{}: track mattes are not exported", l.name));
    }
    if l.blend_mode != BlendMode::Normal {
        ex.warn(format!("{}: blend mode {:?} is not exported", l.name, l.blend_mode));
    }
    if (l.stretch - 100.0).abs() > 1e-9 {
        ex.warn(format!("{}: time stretch is not exported", l.name));
    }
    if l.props.sub("masks").is_some_and(|m| !m.children.is_empty()) {
        ex.warn(format!("{}: masks are not exported", l.name));
    }
    if l.props.get("timeRemap").is_some() {
        ex.warn(format!("{}: time remapping is not exported", l.name));
    }
    if l.props.sub("layerStyles").is_some_and(|m| !m.children.is_empty()) {
        ex.warn(format!("{}: layer styles are not exported", l.name));
    }
}

/// The layers of `comp` inside the group being written, then the group's filters (from the
/// comp's adjustment layers).
fn layers(ex: &mut Ex, w: &mut Writer, comp: &Comp, stack: &mut Vec<ItemId>) {
    let mut group_filters: Vec<(FilterOut, (Tick, Tick, Tick))> = vec![];
    let mut seen_content = false;
    for l in &comp.layers {
        if !l.switches.video {
            ex.warn(format!("{}: hidden layers are not exported", l.name));
            continue;
        }
        if l.switches.adjustment {
            if seen_content {
                ex.warn(format!("{}: an adjustment layer below other layers is exported as a group filter (it then affects the whole group)", l.name));
            }
            let span = (l.in_point, last_frame(ex, l), l.start_time);
            // The adjustment layer's opacity is the filters' Mix.
            let mix = l
                .props
                .sub("transform")
                .and_then(|t| t.get("opacity"))
                .map(|p| channel_of(p, 0, 100.0, 0.0))
                .filter(|m| m.animated() || (m.value - 1.0).abs() > 1e-9);
            for mut f in effect_filters(ex, l) {
                f.mix = mix.clone();
                group_filters.push((f, span));
            }
            continue;
        }
        seen_content = true;
        layer_warnings(ex, l);
        match &l.source {
            LayerSource::Comp { item } => group(ex, w, l, *item, stack),
            LayerSource::Solid { item } => solid(ex, w, l, *item, comp),
            LayerSource::Footage { item } => image(ex, w, l, *item),
            other => ex.warn(format!("{}: {} layers are not exported", l.name, other.type_name().to_lowercase())),
        }
    }
    // Filters apply top to bottom: the bottom adjustment layer's effects first.
    for (f, span) in group_filters.iter().rev() {
        write_filter(ex, w, f, *span);
    }
}

fn group(ex: &mut Ex, w: &mut Writer, l: &Layer, item: ItemId, stack: &mut Vec<ItemId>) {
    let project = ex.project;
    let Some(inner) = project.comp(item) else { return };
    if stack.contains(&item) || stack.len() >= MAX_DEPTH {
        ex.warn(format!("{}: nested compositions too deep or cyclic (skipped)", l.name));
        return;
    }
    if (inner.width, inner.height) != ex.size {
        ex.warn(format!("{}: a nested composition of another size is exported at the project size", l.name));
    }
    let id = ex.id();
    w.open("layer", &[("name", l.name.clone()), ("id", id.to_string())]);
    stack.push(item);
    layers(ex, w, inner, stack);
    stack.pop();
    timing(w, l.in_point, last_frame(ex, l), l.start_time);
    let (cw, ch) = (inner.width as f64, inner.height as f64);
    properties(ex, w, l, [cw / 2.0, ch / 2.0], &|_| {});
    w.open("parameter", &[("name", "Object".into()), ("id", "2".into()), ("flags", "16".into())]);
    channel(w, "Fixed Width", 302, &Channel::constant(cw), cw);
    channel(w, "Fixed Height", 303, &Channel::constant(ch), ch);
    w.close("parameter");
    for f in effect_filters(ex, l) {
        write_filter(ex, w, &f, (l.in_point, last_frame(ex, l), l.start_time));
    }
    w.close("layer");
}

fn object_filters(ex: &mut Ex, w: &mut Writer, l: &Layer) {
    for f in effect_filters(ex, l) {
        write_filter(ex, w, &f, (l.in_point, last_frame(ex, l), l.start_time));
    }
}

fn solid(ex: &mut Ex, w: &mut Writer, l: &Layer, item: ItemId, comp: &Comp) {
    let Some(ItemKind::Solid(s)) = ex.project.item(item).map(|i| &i.kind) else { return };
    let s = s.clone();
    if (s.width, s.height) != (comp.width, comp.height) {
        ex.warn(format!("{}: a solid smaller or larger than the composition becomes a full-frame Color Solid", l.name));
    }
    let id = ex.id();
    w.open(
        "scenenode",
        &[
            ("name", l.name.clone()),
            ("id", id.to_string()),
            ("factoryID", FACTORY_GENERATOR.to_string()),
            ("pluginUUID", COLOR_SOLID_UUID.to_string()),
            ("pluginVersion", "0".to_string()),
            ("pluginName", "Color Solid".to_string()),
            ("version", "5".to_string()),
        ],
    );
    timing(w, l.in_point, last_frame(ex, l), l.start_time);
    properties(ex, w, l, [s.width as f64 / 2.0, s.height as f64 / 2.0], &|_| {});
    w.open("parameter", &[("name", "Object".into()), ("id", "2".into()), ("flags", "16".into())]);
    w.open("parameter", &[("name", "Color Solid".into()), ("id", "1".into()), ("flags", "16".into())]);
    let [r, g, b] = s.color;
    vector(
        w,
        "Color",
        1,
        &[("Red", 1, Channel::constant(r as f64), 0.0), ("Green", 2, Channel::constant(g as f64), 0.0), ("Blue", 3, Channel::constant(b as f64), 1.0)],
    );
    w.close("parameter");
    w.close("parameter");
    object_filters(ex, w, l);
    w.close("scenenode");
}

fn image(ex: &mut Ex, w: &mut Writer, l: &Layer, item: ItemId) {
    let Some((clip, f)) = ex.clip(item) else { return };
    if f.kind == FootageKind::Audio || !f.has_video {
        ex.warn(format!("{}: audio is not exported", l.name));
        return;
    }
    let (fw, fh) = (f.width as f64, f.height as f64);
    let id = ex.id();
    w.open("scenenode", &[("name", l.name.clone()), ("id", id.to_string()), ("factoryID", FACTORY_IMAGE.to_string()), ("version", "5".to_string())]);
    timing(w, l.in_point, last_frame(ex, l), l.start_time);
    properties(ex, w, l, [fw / 2.0, fh / 2.0], &|w: &mut Writer| {
        w.open("parameter", &[("name", "Media".into()), ("id", "324".into()), ("flags", "16".into())]);
        w.empty("parameter", &[("name", "Source Media".into()), ("id", "300".into()), ("flags", "16".into()), ("value", clip.to_string())]);
        w.close("parameter");
    });
    w.open("parameter", &[("name", "Object".into()), ("id", "2".into()), ("flags", "16".into())]);
    channel(w, "Width", 313, &Channel::constant(fw), fw);
    channel(w, "Height", 314, &Channel::constant(fh), fh);
    w.close("parameter");
    object_filters(ex, w, l);
    w.close("scenenode");
}

fn clip_xml(ex: &Ex, w: &mut Writer, c: &Clip) {
    let f = c.footage;
    w.open("clip", &[("name", c.name.clone()), ("id", c.id.to_string())]);
    let path = if f.kind == FootageKind::Sequence { f.sequence.first().cloned().unwrap_or_else(|| f.path.clone()) } else { f.path.clone() };
    if let Some(rel) = relative_to(&path, &ex.doc_dir) {
        w.text("relativeURL", &url_path(&rel));
    }
    let abs = path.replace('\\', "/");
    let abs = if abs.starts_with('/') { abs } else { format!("/{abs}") };
    w.text("pathURL", &format!("file://localhost{}", url_path(&abs)));
    let still = matches!(f.kind, FootageKind::Still);
    let secs = if still { 1.0 / ex.rate.as_f64().max(1e-9) } else { f.duration.seconds() };
    let frames = if still { 1 } else { ex.rate.frame_at(f.duration).max(1) };
    w.text("missingWidth", &f.width.to_string());
    w.text("missingHeight", &f.height.to_string());
    w.text("missingDuration", &num(secs));
    w.text("creationDuration", &frames.to_string());
    timing(w, Tick::ZERO, Tick(f.duration.0.saturating_sub(ex.rate.frame_duration().0)).max(Tick::ZERO), Tick::ZERO);
    w.open("parameter", &[("name", "Object".into()), ("id", "2".into()), ("flags", "16".into())]);
    channel(w, "Pixel Aspect Ratio", 104, &Channel::constant(f.pixel_aspect), 1.0);
    channel(w, "Frame Rate", 107, &Channel::constant(f.frame_rate.as_f64()), 0.0);
    channel(w, "Fixed Width", 114, &Channel::constant(f.width as f64), f.width as f64);
    channel(w, "Fixed Height", 115, &Channel::constant(f.height as f64), f.height as f64);
    w.close("parameter");
    w.close("clip");
}

/// Export composition `comp` as a Motion project. `doc_dir` is the folder the document will be
/// written to (media inside it gets a relative URL too).
pub fn export_comp(project: &Project, comp: ItemId, doc_dir: &str) -> Result<ExportResult, MotionError> {
    let item = project.item(comp).ok_or_else(|| MotionError::Invalid(format!("no item {}", comp.0)))?;
    let c = project.comp(comp).ok_or_else(|| MotionError::Invalid(format!("item {} is not a composition", comp.0)))?;
    let mut ex = Ex {
        project,
        rate: c.frame_rate,
        size: (c.width, c.height),
        doc_dir: doc_dir.to_string(),
        next_id: 10_000,
        clips: vec![],
        clip_ids: HashMap::new(),
        warnings: vec![],
    };
    if (c.pixel_aspect - 1.0).abs() > 1e-9 {
        ex.warn("non-square pixels are exported as square");
    }
    if !c.markers.is_empty() {
        ex.warn("composition markers are not exported");
    }
    // Everything goes in one group named after the composition (Motion objects live in groups).
    let mut body = Writer::at_depth(1);
    let root = ex.id();
    body.open("layer", &[("name", item.name.clone()), ("id", root.to_string())]);
    let mut stack = vec![comp];
    layers(&mut ex, &mut body, c, &mut stack);
    timing(&mut body, Tick::ZERO, Tick(c.duration.0.saturating_sub(c.frame_rate.frame_duration().0)).max(Tick::ZERO), Tick::ZERO);
    body.close("layer");
    let footage_id = ex.id();
    let mut w = Writer::new();
    w.raw("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE ozxmlscene>\n<ozml version=\"5.2\">\n\n");
    w.text("displayversion", "5.0.2");
    for (id, uuid, desc) in FACTORIES {
        w.open("factory", &[("id", id.to_string()), ("uuid", uuid.to_string())]);
        w.text("description", desc);
        w.text("manufacturer", "Apple");
        w.text("version", "1");
        w.close("factory");
    }
    w.open("scene", &[]);
    w.open("sceneSettings", &[]);
    let (fps, ntsc) = rate_fields(c.frame_rate);
    w.text("width", &c.width.to_string());
    w.text("height", &c.height.to_string());
    w.text("duration", &c.frame_rate.frame_at(c.duration).max(1).to_string());
    w.text("frameRate", &num(fps));
    w.text("NTSC", if ntsc { "1" } else { "0" });
    w.text("channels", "4");
    w.text("pixelAspectRatio", "1");
    let [r, g, b] = c.background;
    w.empty("backgroundColor", &[("red", num(r as f64)), ("green", num(g as f64)), ("blue", num(b as f64)), ("alpha", "1".into())]);
    w.close("sceneSettings");
    w.empty("timeRange", &[("offset", fmt_time(Tick::ZERO)), ("duration", fmt_time(c.duration))]);
    w.empty("playRange", &[("offset", fmt_time(Tick::ZERO)), ("duration", fmt_time(c.duration))]);
    w.raw(&body.finish());
    w.open("footage", &[("name", "Media Layer".into()), ("id", footage_id.to_string())]);
    for clip in &ex.clips {
        clip_xml(&ex, &mut w, clip);
    }
    w.close("footage");
    w.close("scene");
    w.raw("</ozml>\n");
    Ok(ExportResult { xml: w.finish(), warnings: ex.warnings })
}
