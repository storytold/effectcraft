//! Motion document → EffectCraft compositions.

use std::collections::HashMap;
use std::sync::Arc;

use effectcraft_color::Label;
use effectcraft_keyframe::{TextDoc, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Comp, Footage, FootageKind, ItemId, ItemKind, Layer, LayerSource, Node as PropNode, ParamUi, Project, Property, Solid};
use effectcraft_time::{FrameRate, Tick};
use roxmltree::{Document, Node};

use crate::curve::{Channel, Mapped, merge, read_channel};
use crate::filters::{FilterIn, import_filter};
use crate::xml::{attr_f64, child, child_f64, child_text, children, param, param_path};
use crate::{MAX_DEPTH, MotionError, parse_time, scene_rate};

/// A media file the document references (a `clip` in the scene's media list).
#[derive(Clone, Debug, PartialEq)]
pub struct ClipRef {
    /// The clip's id in the document.
    pub id: String,
    pub name: String,
    /// Absolute path from `pathURL` (percent-decoded, `file://` removed).
    pub path: Option<String>,
    /// Path relative to the Motion document, from `relativeURL`.
    pub relative: Option<String>,
    pub width: u32,
    pub height: u32,
    /// Seconds.
    pub duration: f64,
    /// A single image (one frame long).
    pub still: bool,
}

#[derive(Clone, Debug)]
pub struct ImportResult {
    /// The new top-level composition.
    pub comp: ItemId,
    /// Every item created (compositions, footage, solids, folder).
    pub items: Vec<ItemId>,
    pub warnings: Vec<String>,
}

fn document(text: &str) -> Result<Document<'_>, MotionError> {
    let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let doc = Document::parse_with_options(text, opts).map_err(|e| MotionError::Parse(e.to_string()))?;
    if doc.root_element().tag_name().name() != "ozml" {
        return Err(MotionError::Parse(format!("root element is <{}>, not <ozml>", doc.root_element().tag_name().name())));
    }
    Ok(doc)
}

fn utf8(bytes: &[u8]) -> Result<&str, MotionError> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    std::str::from_utf8(bytes).map_err(|e| MotionError::Parse(format!("not UTF-8 text: {e}")))
}

/// Decode `%XX` escapes (invalid escapes are kept as written).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        if c == b'%'
            && let Some(hex) = b.get(i + 1..i + 3)
            && let Some(v) = std::str::from_utf8(hex).ok().and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(c);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A local file path from a `file://` URL (`file://localhost/Users/…`, `file:///Users/…`).
fn file_url_path(url: &str) -> Option<String> {
    let rest = url.trim().strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    rest.starts_with('/').then(|| percent_decode(rest))
}

fn scene<'a, 'i>(doc: &'a Document<'i>) -> Result<Node<'a, 'i>, MotionError> {
    child(doc.root_element(), "scene").ok_or_else(|| MotionError::Parse("no <scene> element".into()))
}

fn read_clip(c: Node) -> Option<ClipRef> {
    let id = c.attribute("id")?.to_string();
    let duration = child_f64(c, "missingDuration").unwrap_or(0.0).max(0.0);
    let frames = child_f64(c, "creationDuration").unwrap_or(0.0);
    Some(ClipRef {
        id,
        name: c.attribute("name").unwrap_or("Media").to_string(),
        path: child_text(c, "pathURL").and_then(file_url_path),
        // Drop zone placeholders give a bare file name instead of a URL.
        relative: child_text(c, "relativeURL")
            .filter(|r| !r.is_empty())
            .or_else(|| child_text(c, "pathURL").filter(|p| !p.is_empty() && !p.contains("://")))
            .map(percent_decode),
        width: child_f64(c, "missingWidth").unwrap_or(1920.0).clamp(1.0, 65_535.0) as u32,
        height: child_f64(c, "missingHeight").unwrap_or(1080.0).clamp(1.0, 65_535.0) as u32,
        duration,
        still: frames <= 1.0,
    })
}

/// The media files a Motion document references, so the caller can find and probe them before
/// [`import`].
pub fn clip_refs(bytes: &[u8]) -> Result<Vec<ClipRef>, MotionError> {
    let text = utf8(bytes)?;
    let doc = document(text)?;
    let sc = scene(&doc)?;
    Ok(children(sc, "footage").flat_map(|f| children(f, "clip")).filter_map(read_clip).collect())
}

struct Cx<'a> {
    rate: FrameRate,
    size: (u32, u32),
    duration: Tick,
    background: [f32; 3],
    factories: HashMap<String, String>,
    clips: HashMap<String, ClipRef>,
    footage: HashMap<String, ItemId>,
    resolve: &'a dyn Fn(&ClipRef) -> Option<Footage>,
    items: Vec<ItemId>,
    folder: Option<ItemId>,
    name: String,
    warnings: Vec<String>,
}

impl Cx<'_> {
    fn warn(&mut self, w: impl Into<String>) {
        let w = w.into();
        if !self.warnings.contains(&w) {
            self.warnings.push(w);
        }
    }

    fn folder(&mut self, project: &mut Project) -> ItemId {
        if let Some(f) = self.folder {
            return f;
        }
        let f = project.add_item(&format!("{} Media", self.name), Label::None, None, ItemKind::Folder);
        self.items.push(f);
        self.folder = Some(f);
        f
    }

    fn kind_of(&self, n: Node) -> String {
        n.attribute("factoryID").and_then(|id| self.factories.get(id)).cloned().unwrap_or_default()
    }

    fn comp(&self) -> Comp {
        let mut c = Comp::new(self.size.0, self.size.1, self.rate, self.duration);
        c.background = self.background;
        c
    }

    fn footage_item(&mut self, project: &mut Project, clip_id: &str) -> Option<(ItemId, (u32, u32))> {
        let clip = self.clips.get(clip_id)?.clone();
        if let Some(item) = self.footage.get(clip_id).copied() {
            let size = match project.item(item).map(|i| &i.kind) {
                Some(ItemKind::Footage(f)) => (f.width, f.height),
                _ => (clip.width, clip.height),
            };
            return Some((item, size));
        }
        let foot = (self.resolve)(&clip).unwrap_or_else(|| {
            self.warnings.push(format!("media `{}` was not found; it is imported as missing footage", clip.name));
            Footage {
                path: clip.path.clone().or(clip.relative.clone()).unwrap_or_else(|| clip.name.clone()),
                kind: if clip.still { FootageKind::Still } else { FootageKind::Video },
                width: clip.width,
                height: clip.height,
                pixel_aspect: 1.0,
                frame_rate: self.rate,
                duration: if clip.still { Tick::ZERO } else { Tick::from_seconds_f64(clip.duration) },
                has_video: true,
                missing: true,
                ..Default::default()
            }
        });
        let size = (foot.width.max(1), foot.height.max(1));
        let folder = self.folder(project);
        let item = project.add_item(&clip.name, Label::Lavender, Some(folder), ItemKind::Footage(foot));
        self.items.push(item);
        self.footage.insert(clip_id.to_string(), item);
        Some((item, size))
    }
}

/// Import a Motion document into `project` as a new composition. `resolve` locates and probes
/// a referenced media file (`None` = not found: the footage is imported as missing).
pub fn import(project: &mut Project, bytes: &[u8], fallback_name: &str, resolve: &dyn Fn(&ClipRef) -> Option<Footage>) -> Result<ImportResult, MotionError> {
    let text = utf8(bytes)?;
    let doc = document(text)?;
    let sc = scene(&doc)?;
    let settings = child(sc, "sceneSettings").ok_or_else(|| MotionError::Parse("no <sceneSettings>".into()))?;
    let w = child_f64(settings, "width").unwrap_or(1920.0).round().clamp(1.0, 30_000.0) as u32;
    let h = child_f64(settings, "height").unwrap_or(1080.0).round().clamp(1.0, 30_000.0) as u32;
    let rate = scene_rate(child_f64(settings, "frameRate").unwrap_or(30.0), child_text(settings, "NTSC") == Some("1"));
    let duration = child_f64(settings, "duration")
        .map(|f| rate.tick_of(f.round().clamp(1.0, 1e9) as i64))
        .or_else(|| child(sc, "timeRange").and_then(|t| t.attribute("duration")).and_then(|d| parse_time(d, rate)))
        .filter(|d| d.0 > 0)
        .unwrap_or_else(|| rate.tick_of(300));
    let background = child(settings, "backgroundColor")
        .map(|b| {
            let c = |k: &str| attr_f64(b, k).unwrap_or(0.0).clamp(0.0, 1.0) as f32;
            [c("red"), c("green"), c("blue")]
        })
        .unwrap_or([0.0; 3]);
    let factories =
        children(doc.root_element(), "factory").filter_map(|f| Some((f.attribute("id")?.to_string(), child_text(f, "description")?.to_string()))).collect();
    let clips = children(sc, "footage").flat_map(|f| children(f, "clip")).filter_map(read_clip).map(|c| (c.id.clone(), c)).collect();
    let name = fallback_name.to_string();
    let mut cx = Cx {
        rate,
        size: (w, h),
        duration,
        background,
        factories,
        clips,
        footage: HashMap::new(),
        resolve,
        items: vec![],
        folder: None,
        name: name.clone(),
        warnings: vec![],
    };
    for n in children(sc, "scenenode") {
        if cx.kind_of(n) == "Rig" {
            cx.warn("rigs and their widgets (published parameters) are not imported");
        }
    }
    if children(sc, "audio").next().is_some() {
        cx.warn("audio tracks are not imported");
    }
    let groups: Vec<Node> = sc.children().filter(|c| is_group(*c) && !empty_group(*c)).collect();
    // One plain group holding everything becomes the composition itself.
    let comp = match groups.as_slice() {
        [g] if plain_group(*g, &cx) => build_group(&mut cx, project, *g, 0),
        _ => {
            let mut comp = cx.comp();
            for g in &groups {
                if let Some(l) = group_layer(&mut cx, project, &comp, *g, 0) {
                    comp.layers.push(l);
                }
            }
            comp
        }
    };
    let id = project.add_item(&name, Label::Sandstone, None, ItemKind::Comp(Arc::new(comp)));
    cx.items.push(id);
    project.fix_next_id();
    Ok(ImportResult { comp: id, items: cx.items, warnings: cx.warnings })
}

fn is_group(n: Node) -> bool {
    n.is_element() && matches!(n.tag_name().name(), "layer" | "group")
}

/// A group with no objects, groups or filters (Motion leaves these behind).
fn empty_group(n: Node) -> bool {
    !n.children().any(|c| c.is_element() && matches!(c.tag_name().name(), "layer" | "group" | "scenenode" | "filter"))
}

/// A group with no transform, opacity, timing or behaviors of its own.
fn plain_group(n: Node, cx: &Cx) -> bool {
    let props = param(n, "Properties");
    let animated_or_set = |path: &[&str]| props.and_then(|p| param_path(p, path)).is_some();
    let timing_full = child(n, "timing").is_none_or(|t| {
        let tin = t.attribute("in").and_then(|v| parse_time(v, cx.rate)).unwrap_or(Tick::ZERO);
        let tout = t.attribute("out").and_then(|v| parse_time(v, cx.rate)).unwrap_or(Tick(-1));
        tin == Tick::ZERO && (tout < tin || tout.0.saturating_add(cx.rate.frame_duration().0) >= cx.duration.0)
    });
    !animated_or_set(&["Transform"]) && !animated_or_set(&["Blending", "Opacity"]) && children(n, "behavior").next().is_none() && timing_full
}

/// In, out and start of an object from its `timing` element (Motion's out is the last frame).
fn timing(cx: &Cx, n: Node) -> (Tick, Tick, Tick) {
    let full = (Tick::ZERO, cx.duration, Tick::ZERO);
    let Some(t) = child(n, "timing") else { return full };
    let get = |k: &str| t.attribute(k).and_then(|v| parse_time(v, cx.rate));
    let tin = get("in").unwrap_or(Tick::ZERO);
    let offset = get("offset").unwrap_or(Tick::ZERO);
    let out = match get("out") {
        Some(o) if o >= tin => Tick(o.0.saturating_add(cx.rate.frame_duration().0)),
        _ => cx.duration,
    };
    (tin, out.max(tin), offset)
}

fn apply_timing(cx: &Cx, n: Node, l: &mut Layer) {
    let (tin, out, start) = timing(cx, n);
    l.in_point = tin;
    l.out_point = out;
    l.start_time = start;
}

/// The three channels (`X`, `Y`, `Z`) of a vector parameter, `fallback` where absent.
fn xyz(cx: &mut Cx, p: Option<Node>, fallback: f64) -> [Channel; 3] {
    let rate = cx.rate;
    let mut warn = |w: String| cx.warn(w);
    let ch = |name: &str, warn: &mut dyn FnMut(String)| {
        p.and_then(|p| param(p, name)).map(|c| read_channel(c, rate, fallback, warn)).unwrap_or(Channel::constant(fallback))
    };
    [ch("X", &mut warn), ch("Y", &mut warn), ch("Z", &mut warn)]
}

fn scalar_prop(p: &mut Property, ch: &Channel, scale: f64, offset: f64, warn: &mut dyn FnMut(String)) {
    let (v, keys) = merge(&[Mapped { ch, scale, offset }], &|v| Value::Scalar(v.first().copied().unwrap_or(0.0)), false, warn);
    p.value = v;
    p.keys = keys;
}

fn vec3(v: &[f64]) -> Value {
    Value::Vec3([v.first().copied().unwrap_or(0.0), v.get(1).copied().unwrap_or(0.0), v.get(2).copied().unwrap_or(0.0)])
}

/// Transform and opacity of an object or group onto `l`. `anchor_center` is the layer-space
/// point Motion's anchor (0, 0) refers to.
fn read_transform(cx: &mut Cx, project: &mut Project, n: Node, l: &mut Layer, anchor_center: [f64; 2]) {
    for b in children(n, "behavior") {
        let name = b.attribute("name").unwrap_or("Behavior");
        cx.warn(format!("{}: behavior `{name}` is not imported (behaviors are procedural; convert them to keyframes in Motion first)", l.name));
    }
    if children(n, "mask").next().is_some() {
        cx.warn(format!("{}: masks are not imported", l.name));
    }
    let Some(props) = param(n, "Properties") else { return };
    let tr = param(props, "Transform");
    let (pw, ph) = (cx.size.0 as f64 / 2.0, cx.size.1 as f64 / 2.0);
    let deg = 180.0 / std::f64::consts::PI;
    let pos = xyz(cx, tr.and_then(|t| param(t, "Position")), 0.0);
    let rot = xyz(cx, tr.and_then(|t| param(t, "Rotation")), 0.0);
    let scale = xyz(cx, tr.and_then(|t| param(t, "Scale")), 1.0);
    let anchor = xyz(cx, tr.and_then(|t| param(t, "Anchor Point")), 0.0);
    if tr.and_then(|t| param(t, "Shear")).is_some_and(|s| ["X", "Y"].iter().any(|c| param(s, c).and_then(|p| attr_f64(p, "value")).is_some_and(|v| v != 0.0))) {
        cx.warn(format!("{}: shear is not imported", l.name));
    }
    let opacity = param_path(props, &["Blending", "Opacity"]).or_else(|| param(props, "Opacity"));
    let mut warns = vec![];
    let opacity = opacity.map(|o| read_channel(o, cx.rate, 1.0, &mut |w| warns.push(w))).unwrap_or(Channel::constant(1.0));
    // Motion's blend mode numbers are not documented; 0 is Normal and a group's default 34
    // passes its layers' own modes through.
    if let Some(bm) = param_path(props, &["Blending", "Blend Mode"]).and_then(|b| attr_f64(b, "value"))
        && bm != 0.0
        && bm != 34.0
    {
        warns.push(format!("{}: Motion blend mode {bm} is not imported (Normal is used)", l.name));
    }
    let three_d = pos[2].animated() || pos[2].value != 0.0 || rot[..2].iter().any(|c| c.animated() || c.value != 0.0);
    if three_d {
        l.switches.three_d = true;
    }
    let mut next = project.next_id;
    let mut warn = |w: String| warns.push(w);
    if let Some(t) = l.props.sub_mut("transform") {
        let pmap = [(1.0, pw), (-1.0, ph), (-1.0, 0.0)];
        if pos.iter().any(Channel::animated) {
            // Separate dimensions keep each channel's own keyframes.
            let base: Vec<f64> = pos.iter().zip(pmap).map(|(c, (s, o))| s * c.value + o).collect();
            if let Some(p) = t.get_mut("position") {
                p.value = vec3(&base);
            }
            let at = t.children.iter().position(|c| c.match_id() == "position").map(|i| i + 1).unwrap_or(t.children.len());
            let dims = [("positionX", "X Position"), ("positionY", "Y Position"), ("positionZ", "Z Position")];
            for (d, ((m, name), (s, o))) in dims.iter().zip(pmap).enumerate().rev() {
                let mut pr = Property::new(next, m, name, Value::Scalar(0.0)).with_ui(ParamUi::Number);
                next += 1;
                pr.three_d_only = d == 2;
                if let Some(ch) = pos.get(d) {
                    scalar_prop(&mut pr, ch, s, o, &mut warn);
                }
                t.children.insert(at, PropNode::Prop(pr));
            }
        } else if let Some(p) = t.get_mut("position") {
            p.value = vec3(&pos.iter().zip(pmap).map(|(c, (s, o))| s * c.value + o).collect::<Vec<_>>());
        }
        if let Some(p) = t.get_mut("anchor") {
            let [ax, ay, az] = &anchor;
            let chs = [
                Mapped { ch: ax, scale: 1.0, offset: anchor_center[0] },
                Mapped { ch: ay, scale: -1.0, offset: anchor_center[1] },
                Mapped { ch: az, scale: -1.0, offset: 0.0 },
            ];
            (p.value, p.keys) = merge(&chs, &vec3, true, &mut warn);
        }
        if let Some(p) = t.get_mut("scale") {
            let chs: Vec<Mapped> = scale.iter().map(|ch| Mapped { ch, scale: 100.0, offset: 0.0 }).collect();
            (p.value, p.keys) = merge(&chs, &vec3, false, &mut warn);
        }
        // Y up → Y down keeps X rotations and mirrors Y and Z rotations.
        for (m, ch, s) in [("rotationX", &rot[0], deg), ("rotationY", &rot[1], -deg), ("rotation", &rot[2], -deg)] {
            if let Some(p) = t.get_mut(m) {
                scalar_prop(p, ch, s, 0.0, &mut warn);
            }
        }
        if let Some(p) = t.get_mut("opacity") {
            scalar_prop(p, &opacity, 100.0, 0.0, &mut warn);
        }
    }
    project.next_id = next;
    for w in warns {
        cx.warn(w);
    }
}

/// Effects of `filter` children, added to `l`'s Effects (filters on an object).
fn object_filters(cx: &mut Cx, project: &mut Project, n: Node, l: &mut Layer, size: [f64; 2]) {
    let mut next = project.next_id;
    let mut warns = vec![];
    for f in children(n, "filter") {
        let mut ids = Ids(&mut next);
        let fin_ = FilterIn { node: f, rate: cx.rate, size };
        let fx = import_filter(&fin_, &mut ids, &mut |w| warns.push(w));
        if !fx.is_empty() && fin_.mix(&mut |w| warns.push(w)).is_some_and(|m| m.animated() || (m.value - 1.0).abs() > 1e-9) {
            warns.push(format!(
                "{}: the Mix of filter `{}` is not imported (EffectCraft effects have no mix)",
                l.name,
                f.attribute("name").unwrap_or("Filter")
            ));
        }
        let (fin, fout, _) = timing(cx, f);
        let (lin, lout, _) = timing(cx, n);
        if fin > lin || fout < lout {
            warns.push(format!("{}: filter `{}` applies for the whole layer, not only its own time range", l.name, f.attribute("name").unwrap_or("Filter")));
        }
        if let Some(g) = l.props.sub_mut("effects") {
            g.children.extend(fx.into_iter().map(PropNode::Group));
        }
    }
    project.next_id = next;
    for w in warns {
        cx.warn(w);
    }
}

/// An adjustment layer per filter of a group: filters process the group's composite, which is
/// what an adjustment layer above the group's contents does, for the filter's own time range.
fn filter_layers(cx: &mut Cx, project: &mut Project, comp: &Comp, n: Node) -> Vec<Layer> {
    let mut out = vec![];
    let size = [comp.width as f64, comp.height as f64];
    for f in children(n, "filter") {
        let name = f.attribute("name").unwrap_or("Filter").to_string();
        let mut next = project.next_id;
        let mut warns = vec![];
        let fin = FilterIn { node: f, rate: cx.rate, size };
        let fx = import_filter(&fin, &mut Ids(&mut next), &mut |w| warns.push(w));
        let mix = fin.mix(&mut |w| warns.push(w));
        project.next_id = next;
        for w in std::mem::take(&mut warns) {
            cx.warn(w);
        }
        if fx.is_empty() {
            continue;
        }
        let folder = cx.folder(project);
        let solid = Solid { color: [1.0, 1.0, 1.0], width: comp.width, height: comp.height, pixel_aspect: 1.0 };
        let item = project.add_item(&format!("{name} Adjustment"), Label::None, Some(folder), ItemKind::Solid(solid));
        cx.items.push(item);
        let mut l = build::layer(project, comp, &name, LayerSource::Solid { item }, (comp.width, comp.height), None);
        l.switches.adjustment = true;
        apply_timing(cx, f, &mut l);
        // Mix blends filtered and unfiltered images: the adjustment layer's opacity.
        if let Some(mix) = &mix
            && let Some(p) = l.props.sub_mut("transform").and_then(|t| t.get_mut("opacity"))
        {
            let (v, keys) =
                merge(&[Mapped { ch: mix, scale: 100.0, offset: 0.0 }], &|v| Value::Scalar(v.first().copied().unwrap_or(0.0)), false, &mut |w| warns.push(w));
            p.value = v;
            p.keys = keys;
        }
        for w in std::mem::take(&mut warns) {
            cx.warn(w);
        }
        if let Some(g) = l.props.sub_mut("effects") {
            g.children.extend(fx.into_iter().map(PropNode::Group));
        }
        out.push(l);
    }
    // Motion applies filters top to bottom; the lowest adjustment layer applies first.
    out.reverse();
    out
}

/// The layers and filters of a group as a composition.
fn build_group(cx: &mut Cx, project: &mut Project, n: Node, depth: usize) -> Comp {
    let mut comp = cx.comp();
    let mut layers = vec![];
    for c in n.children().filter(|c| c.is_element()) {
        match c.tag_name().name() {
            "layer" | "group" if !empty_group(c) => {
                if depth + 1 >= MAX_DEPTH {
                    cx.warn("groups nested too deeply are not imported");
                    continue;
                }
                if let Some(l) = group_layer(cx, project, &comp, c, depth + 1) {
                    layers.push(l);
                }
            }
            "scenenode" => {
                if let Some(l) = object_layer(cx, project, &comp, c) {
                    layers.push(l);
                }
            }
            _ => {}
        }
    }
    let mut all = filter_layers(cx, project, &comp, n);
    all.extend(layers);
    comp.layers = all;
    comp
}

/// A group as a precomposition layer.
fn group_layer(cx: &mut Cx, project: &mut Project, parent: &Comp, n: Node, depth: usize) -> Option<Layer> {
    let name = n.attribute("name").unwrap_or("Group").to_string();
    let inner = build_group(cx, project, n, depth);
    let folder = cx.folder(project);
    let item = project.add_item(&name, Label::Sandstone, Some(folder), ItemKind::Comp(Arc::new(inner)));
    cx.items.push(item);
    let (w, h) = cx.size;
    let mut l = build::layer(project, parent, &name, LayerSource::Comp { item }, (w, h), None);
    apply_timing(cx, n, &mut l);
    read_transform(cx, project, n, &mut l, [w as f64 / 2.0, h as f64 / 2.0]);
    Some(l)
}

/// The id of the clip an image object shows: the first numeric `value=` under Properties that
/// names a clip (Motion 5 nests it in Media ▸ Source Media; older files have Media directly).
fn source_clip(cx: &Cx, n: Node) -> Option<String> {
    let props = param(n, "Properties")?;
    let mut stack = vec![(props, 0usize)];
    while let Some((p, d)) = stack.pop() {
        for c in children(p, "parameter") {
            if let Some(v) = c.attribute("value")
                && matches!(c.attribute("name"), Some("Media" | "Source Media"))
                && cx.clips.contains_key(v)
            {
                return Some(v.to_string());
            }
            if d < 4 {
                stack.push((c, d + 1));
            }
        }
    }
    None
}

fn object_layer(cx: &mut Cx, project: &mut Project, comp: &Comp, n: Node) -> Option<Layer> {
    let name = n.attribute("name").unwrap_or("Object").to_string();
    let kind = cx.kind_of(n);
    let (source, size, anchor_center) = match kind.as_str() {
        "Image" => {
            let Some(clip) = source_clip(cx, n) else {
                cx.warn(format!("{name}: image object without media (skipped)"));
                return None;
            };
            let (item, (w, h)) = cx.footage_item(project, &clip)?;
            (LayerSource::Footage { item }, (w, h), [w as f64 / 2.0, h as f64 / 2.0])
        }
        "Generator" if n.attribute("pluginName") == Some("Color Solid") => {
            let c = param_path(n, &["Object", "Color Solid", "Color"]);
            let ch = |k: &str| c.and_then(|c| param(c, k)).and_then(|p| attr_f64(p, "value")).unwrap_or(0.0).clamp(0.0, 1.0) as f32;
            let solid = Solid { color: [ch("Red"), ch("Green"), ch("Blue")], width: comp.width, height: comp.height, pixel_aspect: 1.0 };
            let folder = cx.folder(project);
            let item = project.add_item(&name, Label::Red, Some(folder), ItemKind::Solid(solid));
            cx.items.push(item);
            (LayerSource::Solid { item }, (comp.width, comp.height), [comp.width as f64 / 2.0, comp.height as f64 / 2.0])
        }
        "Text" => (LayerSource::Text, (comp.width, comp.height), [0.0, 0.0]),
        "" => {
            cx.warn(format!("{name}: object of an unknown kind (skipped)"));
            return None;
        }
        other => {
            let what = if other == "Generator" { format!("generator `{}`", n.attribute("pluginName").unwrap_or("?")) } else { other.to_lowercase() };
            cx.warn(format!("{name}: {what} objects are not imported"));
            return None;
        }
    };
    let mut l = build::layer(project, comp, &name, source.clone(), size, None);
    apply_timing(cx, n, &mut l);
    if source == LayerSource::Text {
        read_text(cx, n, &mut l);
    }
    read_transform(cx, project, n, &mut l, anchor_center);
    object_filters(cx, project, n, &mut l, [size.0 as f64, size.1 as f64]);
    Some(l)
}

/// The characters, and the font and size of the first style, of a text object.
fn read_text(cx: &mut Cx, n: Node, l: &mut Layer) {
    let text = child(n, "text").map(|t| t.text().unwrap_or("").replace("\r\n", "\r").replace('\n', "\r")).unwrap_or_default();
    let mut doc = TextDoc::plain(&text);
    if let Some(style) = child(n, "style") {
        if let Some(font) = child_text(style, "font").filter(|f| !f.is_empty()) {
            doc.font = font.to_string();
        }
        if let Some(size) = param(style, "Size").and_then(|p| attr_f64(p, "value")).filter(|s| *s > 0.0) {
            doc.size = size;
        }
    }
    if let Some(p) = l.props.sub_mut("text").and_then(|t| t.get_mut("sourceText")) {
        p.value = Value::Text(Box::new(doc));
    }
    cx.warn(format!("{}: text styles and text behaviors are simplified (font and size of the first style)", l.name));
}
