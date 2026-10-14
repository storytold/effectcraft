//! The plan: the EffectCraft project reduced to exactly the values the `.aep` can hold, in the
//! file's units, with everything that cannot be written already reported. Pure data; the chunks
//! are built from it in `emit`.

use std::collections::{BTreeMap, HashSet};

use effectcraft_color::{BlendMode, Label};
use effectcraft_keyframe::{Ease, Interp, Keyframe};
use effectcraft_project::{Comp, ItemId, ItemKind, Layer, LayerId, LayerSource, Project, Property, Quality, Renderer};
use effectcraft_time::{FrameRate, Tick};

use crate::report::{Collector, LossCategory};
use crate::timebase::{TimeBase, time_base};

/// No coordinate, angle or percentage of a real project comes near this; larger numbers (and NaN
/// or infinity) are written as the default.
pub(crate) const LIMIT: f64 = 1e9;
/// The largest composition or solid side After Effects accepts.
pub(crate) const MAX_SIDE: u32 = 30_000;
/// The longest composition After Effects accepts (3 hours).
const MAX_SECONDS: i64 = 3 * 3600;
/// Deepest folder nesting that is exported.
const MAX_FOLDER_DEPTH: usize = 64;

/// The five Transform properties that are exported, in the file's canonical order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PropKind {
    Anchor,
    Position,
    Scale,
    Rotation,
    Opacity,
}

impl PropKind {
    pub const ALL: [PropKind; 5] = [PropKind::Anchor, PropKind::Position, PropKind::Scale, PropKind::Rotation, PropKind::Opacity];

    /// The match name in the file.
    pub fn match_name(self) -> &'static str {
        match self {
            PropKind::Anchor => "ADBE Anchor Point",
            PropKind::Position => "ADBE Position",
            PropKind::Scale => "ADBE Scale",
            PropKind::Rotation => "ADBE Rotate Z",
            PropKind::Opacity => "ADBE Opacity",
        }
    }

    /// The property id in the project.
    pub fn id(self) -> &'static str {
        match self {
            PropKind::Anchor => "anchor",
            PropKind::Position => "position",
            PropKind::Scale => "scale",
            PropKind::Rotation => "rotation",
            PropKind::Opacity => "opacity",
        }
    }

    /// Components stored in a value.
    pub fn dims(self) -> usize {
        match self {
            PropKind::Anchor | PropKind::Position | PropKind::Scale => 3,
            PropKind::Rotation | PropKind::Opacity => 1,
        }
    }

    /// Anchor and Position have motion paths (the spatial key template).
    pub fn spatial(self) -> bool {
        matches!(self, PropKind::Anchor | PropKind::Position)
    }

    /// The factor from the project's unit to the stored one (percent to fraction).
    fn unit(self) -> f64 {
        match self {
            PropKind::Scale | PropKind::Opacity => 100.0,
            _ => 1.0,
        }
    }
}

/// One side of a key's temporal ease: speed in stored units per second, influence 0..1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EaseSlot {
    pub speed: f64,
    pub influence: f64,
}

impl Default for EaseSlot {
    fn default() -> Self {
        EaseSlot { speed: 0.0, influence: 0.0 }
    }
}

/// A keyframe in the file's units.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct KeyPlan {
    /// In the composition's time units (layer time).
    pub time: i32,
    pub value: Vec<f64>,
    /// 1 linear, 2 bezier, 3 hold.
    pub in_interp: u8,
    pub out_interp: u8,
    /// One slot per component (plain template) or one slot (spatial template).
    pub in_ease: Vec<EaseSlot>,
    pub out_ease: Vec<EaseSlot>,
    pub spatial_in: [f64; 3],
    pub spatial_out: [f64; 3],
    pub spatial_auto: bool,
    pub roving: bool,
    pub label: u8,
}

/// A property that differs from its default: a static value, keys, or an expression.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PropPlan {
    pub kind: PropKind,
    pub value: Vec<f64>,
    pub keys: Vec<KeyPlan>,
    pub expr: Option<(String, bool)>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LayerSrc {
    Solid(ItemId),
    Null,
}

/// A layer in the file's units.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LayerPlan {
    pub name: String,
    pub src: LayerSrc,
    pub adjustment: bool,
    pub label: u8,
    /// 1 draft, 2 best.
    pub quality: u8,
    pub start: i32,
    pub in_rel: i32,
    pub out_rel: i32,
    pub blend: u8,
    pub parent: Option<LayerId>,
    pub ec_id: LayerId,
    pub video: bool,
    pub audio: bool,
    pub solo: bool,
    pub locked: bool,
    pub shy: bool,
    pub collapse: bool,
    pub effects: bool,
    pub motion_blur: bool,
    pub guide: bool,
    pub props: Vec<PropPlan>,
    /// Size of the layer's source in pixels (the Anchor Point divisor).
    pub source_size: (u32, u32),
}

/// A composition in the file's units.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CompPlan {
    pub width: u16,
    pub height: u16,
    pub tb: TimeBase,
    pub rate: FrameRate,
    pub duration_units: u32,
    pub work_start: u32,
    /// `None` = the whole composition (`0xffffffff`).
    pub work_end: Option<u32>,
    pub background: [u8; 3],
    pub pixel_aspect: (u32, u32),
    pub motion_blur: bool,
    pub shutter_angle: u32,
    pub shutter_phase: i32,
    pub display_start: u32,
    pub layers: Vec<LayerPlan>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SolidPlan {
    pub color: [f32; 3],
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ItemPlan {
    Folder(Vec<Entry>),
    Comp(Box<CompPlan>),
    Solid(SolidPlan),
}

/// A project item with its name and label.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Entry {
    pub ec: ItemId,
    pub name: String,
    pub label: u8,
    pub plan: ItemPlan,
}

/// The whole project: the items at the root.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProjectPlan {
    pub root: Vec<Entry>,
}

/// `name` cut to `max` bytes at a character boundary (after Effects stores a name of up to 255
/// characters; a solid's name lives in a 256 byte field).
fn cut_name(name: &str, max: usize, what: &str, col: &mut Collector) -> String {
    if name.len() <= max {
        return name.to_string();
    }
    let mut end = max;
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    col.lose(LossCategory::Names, format!("{what}: name of {} bytes cut to {end}", name.len()));
    name.get(..end).unwrap_or_default().to_string()
}

/// A number that can be written, or `None`.
fn sane(v: f64) -> Option<f64> {
    (v.is_finite() && v.abs() <= LIMIT).then_some(v)
}

fn label_index(l: Label) -> u8 {
    Label::ALL.iter().position(|x| *x == l).and_then(|i| u8::try_from(i).ok()).unwrap_or(0)
}

fn blend_code(mode: BlendMode) -> Option<u8> {
    Some(match mode {
        BlendMode::Normal => 2,
        BlendMode::Add => 4,
        BlendMode::Multiply => 5,
        BlendMode::Screen => 6,
        BlendMode::Overlay => 7,
        BlendMode::Difference => 26,
        _ => return None,
    })
}

/// Natural, case-insensitive order of names (digit runs compare as numbers), as After Effects
/// sorts the project panel and stores the items.
pub(crate) fn natural_key(name: &str) -> Vec<(bool, String, u128)> {
    let mut out: Vec<(bool, String, u128)> = Vec::new();
    let mut chars = name.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            let mut n: u128 = 0;
            let mut s = String::new();
            while let Some(&d) = chars.peek() {
                if !d.is_ascii_digit() {
                    break;
                }
                n = n.saturating_mul(10).saturating_add(u128::from(d as u8 - b'0'));
                s.push(d);
                chars.next();
            }
            out.push((true, s, n));
        } else {
            let mut s = String::new();
            while let Some(&d) = chars.peek() {
                if d.is_ascii_digit() {
                    break;
                }
                s.extend(d.to_lowercase());
                chars.next();
            }
            out.push((false, s, 0));
        }
    }
    out
}

fn sort_entries(v: &mut [Entry]) {
    v.sort_by(|a, b| {
        let ka = natural_key(&a.name);
        let kb = natural_key(&b.name);
        let cmp = |x: &(bool, String, u128), y: &(bool, String, u128)| match (x.0, y.0) {
            (true, true) => x.2.cmp(&y.2).then_with(|| x.1.cmp(&y.1)),
            (false, false) => x.1.cmp(&y.1),
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
        };
        for (x, y) in ka.iter().zip(kb.iter()) {
            let c = cmp(x, y);
            if c != std::cmp::Ordering::Equal {
                return c;
            }
        }
        ka.len().cmp(&kb.len()).then_with(|| a.ec.0.cmp(&b.ec.0))
    });
}

/// Plans the whole project.
pub(crate) fn plan(project: &Project, col: &mut Collector) -> ProjectPlan {
    // Items whose chain of folders reaches the root (no loop, not nested absurdly deep).
    let mut ok: HashSet<ItemId> = HashSet::new();
    for (id, item) in &project.items {
        let mut at = item.parent.filter(|p| project.items.contains_key(p));
        let mut steps = 0usize;
        let reachable = loop {
            match at {
                None => break true,
                Some(p) if p == *id => break false,
                Some(p) => {
                    steps += 1;
                    if steps > MAX_FOLDER_DEPTH {
                        break false;
                    }
                    at = project.items.get(&p).and_then(|i| i.parent).filter(|q| project.items.contains_key(q));
                }
            }
        };
        if reachable {
            ok.insert(*id);
        } else {
            col.lose(
                LossCategory::FootageItems,
                format!("'{}' is in a folder loop or nested deeper than {MAX_FOLDER_DEPTH} levels and was not exported", item.name),
            );
        }
    }
    // Children per folder, project order (creation order = id order).
    let mut by_parent: BTreeMap<Option<ItemId>, Vec<ItemId>> = BTreeMap::new();
    for (id, item) in &project.items {
        if ok.contains(id) {
            by_parent.entry(item.parent.filter(|p| project.items.contains_key(p))).or_default().push(*id);
        }
    }
    let cx = PlanCx { project, by_parent: &by_parent, ok: &ok };
    let root = plan_children(&cx, None, col);
    ProjectPlan { root }
}

struct PlanCx<'a> {
    project: &'a Project,
    by_parent: &'a BTreeMap<Option<ItemId>, Vec<ItemId>>,
    ok: &'a HashSet<ItemId>,
}

fn plan_children(cx: &PlanCx<'_>, parent: Option<ItemId>, col: &mut Collector) -> Vec<Entry> {
    let project = cx.project;
    let mut out = Vec::new();
    for id in cx.by_parent.get(&parent).map(Vec::as_slice).unwrap_or(&[]) {
        let Some(item) = project.item(*id) else { continue };
        let label = label_index(item.label);
        let item_name = cut_name(&item.name, 255, "item", col);
        match &item.kind {
            ItemKind::Folder => {
                col.exported.folders += 1;
                let kids = plan_children(cx, Some(*id), col);
                out.push(Entry { ec: *id, name: item_name, label, plan: ItemPlan::Folder(kids) });
            }
            ItemKind::Comp(comp) => {
                let cp = plan_comp(cx, &item.name, comp, col);
                col.exported.compositions += 1;
                out.push(Entry { ec: *id, name: item_name, label, plan: ItemPlan::Comp(Box::new(cp)) });
            }
            ItemKind::Solid(s) => {
                let width = clamp_side(s.width, &item.name, col);
                let height = clamp_side(s.height, &item.name, col);
                let mut color = [0f32; 3];
                for (c, v) in color.iter_mut().zip(s.color) {
                    *c = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
                }
                col.exported.solids += 1;
                out.push(Entry { ec: *id, name: item_name, label, plan: ItemPlan::Solid(SolidPlan { color, width, height }) });
            }
            ItemKind::Footage(_) => {
                col.lose(LossCategory::FootageItems, format!("footage '{}'", item.name));
            }
        }
    }
    sort_entries(&mut out);
    out
}

fn clamp_side(v: u32, what: &str, col: &mut Collector) -> u32 {
    if (1..=MAX_SIDE).contains(&v) {
        v
    } else {
        col.lose(LossCategory::CompositionSettings, format!("'{what}': size {v} is outside 1 to {MAX_SIDE}, written as {}", v.clamp(1, MAX_SIDE)));
        v.clamp(1, MAX_SIDE)
    }
}

/// The best fraction `n / d` of `v` with a denominator up to 1000 (continued fractions).
fn rational(v: f64) -> (u32, u32) {
    if !(v.is_finite() && v > 0.0) {
        return (1, 1);
    }
    let (mut h0, mut h1, mut k0, mut k1) = (0i64, 1i64, 1i64, 0i64);
    let mut x = v;
    for _ in 0..24 {
        let a = x.floor();
        if a > 1e6 {
            break;
        }
        let a = a as i64;
        let (h2, k2) = (a * h1 + h0, a * k1 + k0);
        if k2 > 1000 || h2 > 1_000_000 {
            break;
        }
        (h0, h1, k0, k1) = (h1, h2, k1, k2);
        if (h1 as f64 / k1 as f64 - v).abs() < 1e-9 {
            break;
        }
        let frac = x - a as f64;
        if frac < 1e-12 {
            break;
        }
        x = 1.0 / frac;
    }
    match (u32::try_from(h1), u32::try_from(k1)) {
        (Ok(n), Ok(d)) if n > 0 && d > 0 => (n, d),
        _ => (1, 1),
    }
}

fn plan_comp(cx: &PlanCx<'_>, name: &str, comp: &Comp, col: &mut Collector) -> CompPlan {
    let rate = comp.frame_rate;
    let tb = time_base(rate).unwrap_or_else(|| {
        col.lose(LossCategory::CompositionSettings, format!("'{name}': frame rate {} cannot be stored, 25 fps written", rate.label()));
        TimeBase { frame_duration: 1024, timescale: 25600 }
    });
    let rate = if time_base(rate).is_some() { rate } else { FrameRate::new(25, 1) };
    let width = clamp_side(comp.width, name, col);
    let height = clamp_side(comp.height, name, col);
    let max_frames = MAX_SECONDS.saturating_mul(rate.num) / rate.den.max(1);
    let mut frames = tb.frames(comp.duration, rate);
    if frames < 1 || frames > max_frames {
        let fixed = frames.clamp(1, max_frames.max(1));
        col.lose(LossCategory::CompositionSettings, format!("'{name}': duration of {frames} frames is outside 1 frame to 3 hours, written as {fixed} frames"));
        frames = fixed;
    }
    let fd = i64::from(tb.frame_duration);
    let duration_units = u32::try_from(frames.saturating_mul(fd)).unwrap_or(u32::MAX);
    let ws = tb.frames(comp.work_area.0, rate).clamp(0, frames - 1);
    let we = tb.frames(comp.work_area.1, rate).clamp(ws + 1, frames);
    let whole = ws == 0 && we == frames;
    if (comp.work_area.0 != rate.tick_of(ws) && comp.work_area.0 != Tick::ZERO) || (comp.work_area.1 != rate.tick_of(we) && comp.work_area.1 != comp.duration) {
        col.lose(LossCategory::RoundedTimes, format!("'{name}': work area moved to whole frames ({ws} to {we})"));
    }
    let mut background = [0u8; 3];
    for (b, v) in background.iter_mut().zip(comp.background) {
        *b = if v.is_finite() { (v.clamp(0.0, 1.0) * 255.0).round() as u8 } else { 0 };
    }
    let pa = sane(comp.pixel_aspect).filter(|v| *v > 0.0).unwrap_or(1.0);
    let (pn, pd) = rational(pa);
    if (f64::from(pn) / f64::from(pd) - pa).abs() > 1e-4 {
        col.lose(LossCategory::CompositionSettings, format!("'{name}': pixel aspect {pa} written as {pn}/{pd}"));
    }
    if matches!(comp.renderer, Renderer::Advanced3D) {
        col.lose(LossCategory::CompositionOptions, format!("'{name}': the Advanced 3D renderer is written as Classic 3D"));
    }
    for m in &comp.markers {
        col.lose(LossCategory::Markers, format!("{name}: composition marker at {:.3} s '{}'", m.time.seconds(), m.comment));
    }
    if !comp.guides.is_empty() {
        col.lose(LossCategory::CompositionOptions, format!("'{name}': {} guides", comp.guides.len()));
    }
    if comp.essential.is_some() {
        col.lose(LossCategory::CompositionOptions, format!("'{name}': Essential Graphics"));
    }
    let (ds, _) = tb.units(comp.display_start);
    let display_start = u32::try_from(ds.max(0)).unwrap_or(0);
    let shutter_angle = sane(comp.shutter_angle).map_or(180, |v| v.round().clamp(0.0, 720.0) as u32);
    let shutter_phase = sane(comp.shutter_phase).map_or(0, |v| v.round().clamp(-720.0, 720.0) as i32);

    let mut cp = CompPlan {
        width: u16::try_from(width).unwrap_or(u16::MAX),
        height: u16::try_from(height).unwrap_or(u16::MAX),
        tb,
        rate,
        duration_units,
        work_start: u32::try_from(ws.saturating_mul(fd)).unwrap_or(0),
        work_end: if whole { None } else { u32::try_from(we.saturating_mul(fd)).ok() },
        background,
        pixel_aspect: (pn, pd),
        motion_blur: comp.enable_motion_blur,
        shutter_angle,
        shutter_phase,
        display_start,
        layers: Vec::new(),
    };
    let ids: HashSet<LayerId> = comp.layers.iter().map(|l| l.id).collect();
    for layer in &comp.layers {
        if let Some(lp) = plan_layer(cx, name, comp, &cp, layer, &ids, col) {
            cp.layers.push(lp);
        }
    }
    resolve_parents(&mut cp, name, col);
    cp
}

/// Parent links to a layer that was not exported, and links that lead into a loop, are cut.
fn resolve_parents(cp: &mut CompPlan, comp_name: &str, col: &mut Collector) {
    let exported: HashSet<LayerId> = cp.layers.iter().map(|l| l.ec_id).collect();
    let links: std::collections::HashMap<LayerId, LayerId> = cp.layers.iter().filter_map(|l| l.parent.map(|p| (l.ec_id, p))).collect();
    for l in &mut cp.layers {
        let Some(p) = l.parent else { continue };
        let mut cut = !exported.contains(&p);
        if !cut {
            // Follow the chain; a chain longer than the number of links is a loop.
            let mut at = p;
            for _ in 0..=links.len() {
                if at == l.ec_id {
                    cut = true;
                    break;
                }
                match links.get(&at) {
                    Some(next) => at = *next,
                    None => break,
                }
            }
            if !cut && at == l.ec_id {
                cut = true;
            }
        }
        if cut {
            col.lose(LossCategory::ParentLinks, format!("{comp_name}: '{}' has a parent that was not exported or forms a loop", l.name));
            l.parent = None;
        }
    }
}

fn plan_layer(cx: &PlanCx<'_>, comp_name: &str, comp: &Comp, cp: &CompPlan, layer: &Layer, ids: &HashSet<LayerId>, col: &mut Collector) -> Option<LayerPlan> {
    let what = |s: &str| format!("{comp_name}: {s} '{}'", layer.name);
    let (src, source_size) = match &layer.source {
        LayerSource::Solid { item } => match cx.project.item(*item).filter(|_| cx.ok.contains(item)).map(|i| &i.kind) {
            Some(ItemKind::Solid(s)) => (LayerSrc::Solid(*item), (s.width.clamp(1, MAX_SIDE), s.height.clamp(1, MAX_SIDE))),
            _ => {
                col.lose(LossCategory::UnsupportedLayers, what("solid layer without a solid:"));
                return None;
            }
        },
        LayerSource::Null => (LayerSrc::Null, (100, 100)),
        LayerSource::Text => {
            col.lose(LossCategory::TextLayers, what("text layer"));
            return None;
        }
        LayerSource::Shape => {
            col.lose(LossCategory::ShapeLayers, what("shape layer"));
            return None;
        }
        LayerSource::Camera => {
            col.lose(LossCategory::Cameras, what("camera"));
            return None;
        }
        LayerSource::Light { .. } => {
            col.lose(LossCategory::Lights, what("light"));
            return None;
        }
        LayerSource::Model { .. } | LayerSource::Primitive { .. } => {
            col.lose(LossCategory::ModelLayers, what("3D layer"));
            return None;
        }
        LayerSource::Footage { .. } => {
            col.lose(LossCategory::UnsupportedLayers, what("footage layer"));
            return None;
        }
        LayerSource::Comp { .. } => {
            col.lose(LossCategory::UnsupportedLayers, what("precomposition layer"));
            return None;
        }
    };
    let tb = cp.tb;
    let mut rounded = false;
    let mut units = |t: Tick, col: &mut Collector, what: &str| -> i32 {
        let (u, inexact) = tb.units(t);
        rounded |= inexact;
        i32::try_from(u).unwrap_or_else(|_| {
            col.lose(LossCategory::InvalidValues, format!("{comp_name}: '{}' {what} is out of range, written as 0", layer.name));
            0
        })
    };
    let start = units(layer.start_time, col, "start time");
    let in_abs = units(layer.in_point, col, "in point");
    let out_abs = units(layer.out_point, col, "out point");
    let stretch_ok = (layer.stretch - 100.0).abs() < 1e-9;
    if !stretch_ok {
        col.lose(LossCategory::TimeStretch, what(&format!("stretch {:.2} % on layer", layer.stretch)));
    }

    // Switches and features that are not written.
    if layer.switches.three_d {
        col.lose(LossCategory::ThreeD, what("3D layer"));
    }
    if layer.track_matte.is_some() {
        col.lose(LossCategory::TrackMattes, what("track matte on"));
    }
    if !layer.markers.is_empty() {
        col.lose(LossCategory::Markers, what(&format!("{} marker(s) on", layer.markers.len())));
    }
    if let Some(fx) = layer.effects() {
        for e in &fx.children {
            col.lose(LossCategory::Effects, format!("{comp_name}: '{}' has the effect '{}'", layer.name, e.name()));
        }
    }
    if let Some(masks) = layer.masks() {
        for m in &masks.children {
            col.lose(LossCategory::Masks, format!("{comp_name}: '{}' has '{}'", layer.name, m.name()));
        }
    }
    let blend = match blend_code(layer.blend_mode) {
        Some(c) => c,
        None => {
            col.lose(LossCategory::BlendModes, what(&format!("blend mode {} on", layer.blend_mode.label())));
            2
        }
    };
    let parent = layer.parent.filter(|p| ids.contains(p));

    let mut props = Vec::new();
    if let Some(tr) = layer.transform() {
        for kind in PropKind::ALL {
            let Some(p) = tr.get(kind.id()) else { continue };
            if let Some(pp) = plan_prop(kind, p, comp, source_size, cp, &layer.name, comp_name, col) {
                col.exported.properties += 1;
                props.push(pp);
            }
        }
        for node in &tr.children {
            let id = node.match_id();
            if PropKind::ALL.iter().any(|k| k.id() == id) {
                continue;
            }
            if let Some(p) = node.as_prop()
                && is_non_default_extra(id, p)
            {
                col.lose(LossCategory::TransformProperties, format!("{comp_name}: '{}' {}", layer.name, p.name));
            }
        }
    }
    let name = cut_name(&layer.name, 255, &what("layer"), col);
    col.exported.layers += 1;
    match (&src, layer.switches.adjustment) {
        (LayerSrc::Null, _) => col.exported.null_layers += 1,
        (_, true) => col.exported.adjustment_layers += 1,
        _ => col.exported.solid_layers += 1,
    }
    Some(LayerPlan {
        name,
        src,
        adjustment: layer.switches.adjustment,
        label: label_index(layer.label),
        quality: if layer.switches.quality == Quality::Draft { 1 } else { 2 },
        start,
        in_rel: in_abs.saturating_sub(start),
        out_rel: out_abs.saturating_sub(start),
        blend,
        parent,
        ec_id: layer.id,
        video: layer.switches.video,
        audio: layer.switches.audio,
        solo: layer.switches.solo,
        locked: layer.switches.locked,
        shy: layer.switches.shy,
        collapse: layer.switches.collapse,
        effects: layer.switches.effects,
        motion_blur: layer.switches.motion_blur,
        guide: layer.switches.guide,
        props,
        source_size,
    })
}

/// Whether a transform property outside the five carries something (a keyed or changed value).
fn is_non_default_extra(id: &str, p: &Property) -> bool {
    if !p.keys.is_empty() || p.has_expression() {
        return true;
    }
    let comps = p.value.components();
    match id {
        "orientation" | "rotation_x" | "rotation_y" | "x_rotation" | "y_rotation" | "orient" => comps.iter().any(|c| c.abs() > 1e-9),
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_prop(
    kind: PropKind,
    p: &Property,
    comp: &Comp,
    source: (u32, u32),
    cp: &CompPlan,
    layer: &str,
    comp_name: &str,
    col: &mut Collector,
) -> Option<PropPlan> {
    let (sw, sh) = (f64::from(source.0.max(1)), f64::from(source.1.max(1)));
    let what = format!("{comp_name}: '{layer}' {}", kind.match_name());
    let dims = kind.dims();
    let default: Vec<f64> = match kind {
        PropKind::Anchor => vec![0.5, 0.5, 0.0],
        PropKind::Position => vec![f64::from(comp.width) / 2.0, f64::from(comp.height) / 2.0, 0.0],
        PropKind::Scale => vec![1.0, 1.0, 1.0],
        PropKind::Rotation => vec![0.0],
        PropKind::Opacity => vec![1.0],
    };
    // The project's value in the stored unit.
    let to_file = |c: &[f64], what: &str, col: &mut Collector| -> Vec<f64> {
        let mut out = Vec::with_capacity(dims);
        for i in 0..dims {
            let raw = c.get(i).copied();
            let fallback = if kind == PropKind::Scale { 100.0 } else { 0.0 };
            let v = raw.and_then(sane).or(if raw.is_none() { Some(fallback) } else { None });
            let v = match v {
                Some(v) => v,
                None => {
                    col.lose(LossCategory::InvalidValues, format!("{what}: a value that is not a valid number, the default was written"));
                    default.get(i).copied().map(|d| d * kind.unit()).unwrap_or(0.0)
                }
            };
            let stored = match kind {
                PropKind::Anchor => match i {
                    0 => v / sw,
                    1 => v / sh,
                    _ => v,
                },
                PropKind::Scale | PropKind::Opacity => v / 100.0,
                _ => v,
            };
            out.push(stored);
        }
        out
    };
    let static_value = to_file(&p.value.components(), &what, col);
    let expr = p.expr.as_ref().filter(|e| !e.text.is_empty()).map(|e| (e.text.clone(), e.enabled));
    if expr.is_some() {
        col.exported.expressions += 1;
    }
    let is_default = static_value.iter().zip(&default).all(|(a, b)| (a - b).abs() < 1e-12);
    if p.keys.is_empty() {
        if is_default && expr.is_none() {
            return None;
        }
        return Some(PropPlan { kind, value: static_value, keys: Vec::new(), expr });
    }
    // Keys, sorted and unique in time units.
    let mut keys: Vec<(i64, &Keyframe)> = Vec::with_capacity(p.keys.len());
    for k in &p.keys {
        let (u, inexact) = cp.tb.units(k.time);
        if inexact {
            col.lose(LossCategory::RoundedTimes, format!("{what}: key at {:.4} s moved to the time base", k.time.seconds()));
        }
        keys.push((u, k));
    }
    keys.sort_by_key(|(u, _)| *u);
    let before = keys.len();
    keys.dedup_by_key(|(u, _)| *u);
    if keys.len() != before {
        col.lose(LossCategory::ApproximatedKeyframes, format!("{what}: keys at the same time were merged"));
    }
    let anchor = kind == PropKind::Anchor;
    let mut plans = Vec::with_capacity(keys.len());
    let all: Vec<Keyframe> = keys.iter().map(|(_, k)| (*k).clone()).collect();
    for (i, (u, k)) in keys.iter().enumerate() {
        let Ok(time) = i32::try_from(*u) else {
            col.lose(LossCategory::InvalidValues, format!("{what}: key time out of range, key dropped"));
            continue;
        };
        let value = to_file(&k.value.components(), &what, col);
        let side = |interp: Interp, col: &mut Collector| -> u8 {
            match interp {
                Interp::Linear => 1,
                Interp::Hold => 3,
                Interp::Bezier => {
                    if anchor {
                        col.lose(
                            LossCategory::ApproximatedKeyframes,
                            format!("{what}: bezier ease of an Anchor Point key is written as linear (its ease units are not decoded)"),
                        );
                        1
                    } else {
                        2
                    }
                }
            }
        };
        let in_i = side(k.in_interp, col);
        let out_i = side(k.out_interp, col);
        let slots = if kind.spatial() || dims == 1 { 1 } else { dims };
        let ease_of = |eases: &[Ease], i: usize| -> EaseSlot {
            let e = eases.get(i).or_else(|| eases.last()).copied().unwrap_or(Ease { speed: 0.0, influence: 1.0 / 3.0 });
            let speed = sane(e.speed / kind.unit()).unwrap_or(0.0);
            let influence = if e.influence.is_finite() { e.influence.clamp(0.001, 1.0) } else { 1.0 / 3.0 };
            EaseSlot { speed, influence }
        };
        let in_ease: Vec<EaseSlot> = (0..slots).map(|s| if in_i == 2 { ease_of(&k.in_ease, s) } else { EaseSlot::default() }).collect();
        let out_ease: Vec<EaseSlot> = (0..slots).map(|s| if out_i == 2 { ease_of(&k.out_ease, s) } else { EaseSlot::default() }).collect();
        let (mut tin, mut tout) = if kind.spatial() { effectcraft_keyframe::spatial_tangents(&all, i) } else { ([0.0; 3], [0.0; 3]) };
        for t in tin.iter_mut().chain(tout.iter_mut()) {
            if sane(*t).is_none() {
                *t = 0.0;
            }
        }
        if anchor {
            for t in [&mut tin, &mut tout] {
                t[0] /= sw;
                t[1] /= sh;
            }
        }
        plans.push(KeyPlan {
            time,
            value,
            in_interp: in_i,
            out_interp: out_i,
            in_ease,
            out_ease,
            spatial_in: tin,
            spatial_out: tout,
            spatial_auto: k.spatial_auto,
            roving: k.roving,
            label: k.label.min(16),
        });
        col.exported.keyframes += 1;
    }
    if plans.is_empty() {
        return None;
    }
    let value = plans.first().map(|k| k.value.clone()).unwrap_or(static_value);
    Some(PropPlan { kind, value, keys: plans, expr })
}
