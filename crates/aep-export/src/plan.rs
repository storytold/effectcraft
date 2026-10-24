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

/// No sensible coordinate, angle or percentage comes near this; larger numbers (and NaN
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
    /// A precomposition layer: the composition item it shows.
    Comp(ItemId),
    /// A footage layer: the footage item it shows.
    Footage(ItemId),
    /// A text layer and its text document.
    Text(Box<crate::text::TextPlan>),
    /// A camera (256) and its options.
    Camera(Box<crate::camlight::CameraPlan>),
    /// A spot or point light (254, 252) and its options.
    Light(Box<crate::camlight::LightPlan>),
    /// A shape layer and its items (519).
    Shape(Vec<crate::shapes::ShItem>),
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
    /// Track matte: the matte layer and the mode (`ldta@107`: 1 alpha, 2 alpha inverted, 3 luma,
    /// 4 luma inverted; 403-406).
    pub matte: Option<(LayerId, u8)>,
    pub markers: Vec<crate::markers::MarkerPlan>,
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
    pub three_d: bool,
    /// Time stretch as `numerator / denominator` (1 / 1 = 100 %, -1 / 1 = -100 %).
    pub stretch: (i32, u32),
    /// The time scale the layer's properties count in.
    pub time_scale: u32,
    pub props: Vec<PropPlan>,
    /// The layer's masks, in stack order.
    pub masks: Vec<crate::masks::MaskPlan>,
    /// The layer's effects, in stack order.
    pub fx: Vec<crate::effects::EffectPlan>,
    /// Size of the layer's source in pixels (the Anchor Point divisor).
    pub source_size: (u32, u32),
    /// An enabled Time Remap (source seconds; keys in layer time), written into the layer's
    /// `ADBE Time Remapping` record (319, 354).
    pub remap: Option<PropPlan>,
    /// The layer styles (None: none applied).
    pub styles: Option<crate::styles::StylesPlan>,
}

/// `cp` without its layers.
fn comp_header(cp: &CompPlan) -> CompPlan {
    CompPlan {
        width: cp.width,
        height: cp.height,
        tb: cp.tb,
        rate: cp.rate,
        duration_units: cp.duration_units,
        work_start: cp.work_start,
        work_end: cp.work_end,
        background: cp.background,
        pixel_aspect: cp.pixel_aspect,
        motion_blur: cp.motion_blur,
        shutter_angle: cp.shutter_angle,
        shutter_phase: cp.shutter_phase,
        display_start: cp.display_start,
        markers: Vec::new(),
        layers: Vec::new(),
    }
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
    /// Composition markers (written into the `Markers` view layer).
    pub markers: Vec<crate::markers::MarkerPlan>,
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
    Footage(Box<crate::footage::FootagePlan>),
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
    pub settings: crate::settings::SettingsPlan,
}

/// `name` cut to `max` bytes at a character boundary (after Effects stores a name of up to 255
/// characters; a solid's name lives in a 256 byte field).
pub(crate) fn cut_name(name: &str, max: usize, what: &str, col: &mut Collector) -> String {
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

pub(crate) fn label_index(l: Label) -> u8 {
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
        // The codes After Effects writes for the other modes (906, 907; the importer's table).
        BlendMode::SoftLight => 8,
        BlendMode::HardLight => 9,
        BlendMode::Darken => 10,
        BlendMode::Lighten => 11,
        BlendMode::ClassicDifference => 12,
        BlendMode::Hue => 13,
        BlendMode::Saturation => 14,
        BlendMode::Color => 15,
        BlendMode::Luminosity => 16,
        BlendMode::StencilAlpha => 17,
        BlendMode::StencilLuma => 18,
        BlendMode::SilhouetteAlpha => 19,
        BlendMode::SilhouetteLuma => 20,
        BlendMode::ClassicColorDodge => 23,
        BlendMode::ClassicColorBurn => 24,
        BlendMode::Exclusion => 25,
        BlendMode::ColorDodge => 27,
        BlendMode::ColorBurn => 28,
        BlendMode::LinearDodge => 29,
        BlendMode::LinearBurn => 30,
        BlendMode::LinearLight => 31,
        BlendMode::VividLight => 32,
        BlendMode::PinLight => 33,
        BlendMode::HardMix => 34,
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
    ProjectPlan { root, settings: crate::settings::plan(&project.settings, col) }
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
                let cp = plan_comp(cx, *id, &item.name, comp, col);
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
            ItemKind::Footage(f) => {
                if let Some(fp) = crate::footage::plan_footage(&item.name, f, col) {
                    out.push(Entry { ec: *id, name: item_name, label, plan: ItemPlan::Footage(Box::new(fp)) });
                }
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

fn plan_comp(cx: &PlanCx<'_>, comp_id: ItemId, name: &str, comp: &Comp, col: &mut Collector) -> CompPlan {
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
    let comp_markers = crate::markers::plan(&comp.markers, &tb, &format!("'{name}'"), col);
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
        markers: comp_markers,
        layers: Vec::new(),
    };
    let ids: HashSet<LayerId> = comp.layers.iter().map(|l| l.id).collect();
    for layer in &comp.layers {
        if let Some(lp) = plan_layer(cx, comp_id, name, comp, &cp, layer, &ids, col) {
            cp.layers.push(lp);
        }
    }
    resolve_parents(&mut cp, name, col);
    resolve_mattes(&mut cp, name, col);
    resolve_effect_layers(&mut cp, name, col);
    cp
}

/// An effect's layer parameter that names a layer that is not exported falls back to the owning
/// layer (After Effects' default) and is reported.
fn resolve_effect_layers(cp: &mut CompPlan, comp_name: &str, col: &mut Collector) {
    let exported: HashSet<LayerId> = cp.layers.iter().map(|l| l.ec_id).collect();
    for l in &mut cp.layers {
        for fx in &mut l.fx {
            let before = fx.layers.len();
            fx.layers.retain(|(_, p)| !matches!(p, crate::effects::LayerPick::Layer(id) if !exported.contains(id)));
            if fx.layers.len() != before {
                col.lose(
                    LossCategory::Effects,
                    format!(
                        "{comp_name}: '{}' effect '{}': a layer parameter names a layer that is not exported, the owning layer was written",
                        l.name, fx.name
                    ),
                );
            }
        }
    }
}

/// A track matte whose layer is not exported, or is the layer itself, is dropped and reported.
fn resolve_mattes(cp: &mut CompPlan, comp_name: &str, col: &mut Collector) {
    let exported: HashSet<LayerId> = cp.layers.iter().map(|l| l.ec_id).collect();
    for l in &mut cp.layers {
        if let Some((m, _)) = l.matte
            && (m == l.ec_id || !exported.contains(&m))
        {
            col.lose(LossCategory::TrackMattes, format!("{comp_name}: '{}': the track matte layer is not exported", l.name));
            l.matte = None;
        }
    }
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

#[allow(clippy::too_many_arguments)]
fn plan_layer(
    cx: &PlanCx<'_>,
    comp_id: ItemId,
    comp_name: &str,
    comp: &Comp,
    cp: &CompPlan,
    layer: &Layer,
    ids: &HashSet<LayerId>,
    col: &mut Collector,
) -> Option<LayerPlan> {
    let what = |s: &str| format!("{comp_name}: {s} '{}'", layer.name);
    // An enabled Time Remap (the importer's `timeRemap` property) is written on precomposition
    // and footage layers, the layers After Effects can remap (319, 354; a solid cannot).
    let remap_prop = layer.props.prop("timeRemap");
    let remappable = matches!(layer.source, LayerSource::Comp { .. } | LayerSource::Footage { .. });
    if remap_prop.is_some() && !remappable {
        col.lose(LossCategory::TimeRemapping, what("time remapping on a layer After Effects cannot remap is not written; layer"));
    }
    let remapped = remap_prop.is_some() && remappable;
    let (mut src, source_size) = match &layer.source {
        LayerSource::Solid { item } => match cx.project.item(*item).filter(|_| cx.ok.contains(item)).map(|i| &i.kind) {
            Some(ItemKind::Solid(s)) => (LayerSrc::Solid(*item), (s.width.clamp(1, MAX_SIDE), s.height.clamp(1, MAX_SIDE))),
            _ => {
                col.lose(LossCategory::UnsupportedLayers, what("solid layer without a solid:"));
                return None;
            }
        },
        LayerSource::Null => (LayerSrc::Null, (100, 100)),
        LayerSource::Text => {
            // A text layer has no source; it spans the composition (the importer's convention).
            let tp = crate::text::plan_text(layer, comp_name, col)?;
            (LayerSrc::Text(Box::new(tp)), (comp.width.clamp(1, MAX_SIDE), comp.height.clamp(1, MAX_SIDE)))
        }
        // A shape layer has no source; its anchor is in pixels (462), its items are planned from
        // the `contents` group.
        // (planned below, once the layer's time base is known: their keys count in it)
        LayerSource::Shape => (LayerSrc::Shape(Vec::new()), (comp.width.clamp(1, MAX_SIDE), comp.height.clamp(1, MAX_SIDE))),
        // Cameras and lights have no source; their point of interest is the Anchor Point in
        // pixels (3.12).
        LayerSource::Camera => (LayerSrc::Camera(Box::new(crate::camlight::plan_camera(layer, comp.width, &what("camera"), col))), (1, 1)),
        LayerSource::Light { kind } => (LayerSrc::Light(Box::new(crate::camlight::plan_light(layer, *kind, &what("light"), col)?)), (1, 1)),
        LayerSource::Model { .. } | LayerSource::Primitive { .. } => {
            col.lose(LossCategory::ModelLayers, what("3D layer"));
            return None;
        }
        LayerSource::Footage { item } => match cx.project.item(*item).filter(|_| cx.ok.contains(item)).map(|i| (&i.name, &i.kind)) {
            Some((name, ItemKind::Footage(f))) if let Some(fp) = crate::footage::plan_footage(name, f, &mut Collector::default()) => {
                // After Effects ends a layer of timed footage at the source's end (f1: an mp4,
                // two sequences and a WAV written 5 s long read back 1, 0.1, 0.1 and 1 s long).
                // Source time the layer reaches: its span divided by the stretch (a reversed layer reaches
                // it at its in point).
                let span = (layer.out_point.seconds() - layer.start_time.seconds()).abs().max((layer.in_point.seconds() - layer.start_time.seconds()).abs())
                    / (layer.stretch.abs() / 100.0).max(1e-9);
                // A remapped layer may run past its source (the remap holds a frame); less than a
                // composition frame past is the rounding of a clip's duration to frames.
                let frame = comp.frame_rate.frame_duration().seconds();
                if fp.seconds > 0.0 && span > fp.seconds + frame.max(1e-6) - 1e-9 && !remapped {
                    col.lose(
                        LossCategory::LayerTiming,
                        what(&format!("footage layer runs {span:.3} s, past its source's {:.3} s; After Effects ends it there:", fp.seconds)),
                    );
                }
                (LayerSrc::Footage(*item), (f.width.clamp(1, MAX_SIDE), f.height.clamp(1, MAX_SIDE)))
            }
            _ => {
                col.lose(LossCategory::UnsupportedLayers, what("footage layer whose footage is not exported:"));
                return None;
            }
        },
        LayerSource::Comp { item } => match cx.project.item(*item).filter(|_| cx.ok.contains(item)).map(|i| &i.kind) {
            Some(ItemKind::Comp(c)) if !precomp_loop(cx.project, comp_id, *item) => {
                (LayerSrc::Comp(*item), (c.width.clamp(1, MAX_SIDE), c.height.clamp(1, MAX_SIDE)))
            }
            Some(ItemKind::Comp(_)) => {
                col.lose(LossCategory::UnsupportedLayers, what("precomposition layer that closes a loop:"));
                return None;
            }
            _ => {
                col.lose(LossCategory::UnsupportedLayers, what("precomposition layer without its composition:"));
                return None;
            }
        },
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
    // Time stretch: the exact ratio `ldta@8 / @108` (018 200 %: 2 / 1, 316 50 %: 1 / 2, 317 150 %:
    // 3 / 2); in and out points relative to the start are in source time (absolute = start +
    // relative x stretch), and the layer's properties count in the composition's time scale x
    // stretch for a stretch of 100 % or more, in the composition's below (018, 317: 51 200 and
    // 38 400; 316: 25 600). Keys count source time in that time scale (641: 200 %, a key at comp
    // 1 s = source 0.5 s stored as 25 600 / 51 200; 642: 50 %, comp 1 s = source 2 s stored as
    // 51 200 / 25 600; 643: the same with the layer starting at 1 s), which is EffectCraft's layer
    // time in the layer's time scale.
    let mut stretch = (1i32, 1u32);
    let mut time_scale = cp.tb.timescale;
    if (layer.stretch - 100.0).abs() >= 1e-9 {
        // A negative stretch is a time-reversed layer: the numerator is negative (981-983) and the
        // properties count in the composition's time scale, as below 100 % (982, 983).
        let r = layer.stretch / 100.0;
        let reversed = r < 0.0;
        let (n, d) = rational(r.abs());
        let exact = r.is_finite() && r != 0.0 && (f64::from(n) / f64::from(d) - r.abs()).abs() <= 1e-9 * r.abs();
        let ts = if n >= d && !reversed { u64::from(cp.tb.timescale) * u64::from(n) / u64::from(d) } else { u64::from(cp.tb.timescale) };
        let ts_exact = n < d || reversed || (u64::from(cp.tb.timescale) * u64::from(n)) % u64::from(d) == 0;
        let num = i32::try_from(n).ok().map(|n| if reversed { -n } else { n });
        match num {
            Some(num) if exact && ts_exact && ts <= u64::from(u32::MAX) => {
                if has_bezier_keys(&layer.props, 0) {
                    col.lose(
                        LossCategory::ApproximatedKeyframes,
                        what(&format!(
                            "bezier ease on a {:.2} % stretched layer (its speed unit there is not sampled; written as stored) on layer",
                            layer.stretch
                        )),
                    );
                }
                stretch = (num, d);
                time_scale = u32::try_from(ts).unwrap_or(cp.tb.timescale);
            }
            _ => col.lose(LossCategory::TimeStretch, what(&format!("stretch {:.2} % (not an exact small ratio) on layer", layer.stretch))),
        }
    }
    // Relative times in source time.
    let unstretch = |rel: i32| -> i32 {
        if stretch == (1, 1) {
            return rel;
        }
        let v = (i64::from(rel) * i64::from(stretch.1)) as f64 / f64::from(stretch.0);
        v.round().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
    };

    // The layer's properties count in its own time scale (keys in source time units).
    let layer_cp;
    let cp: &CompPlan = if time_scale == cp.tb.timescale {
        cp
    } else {
        layer_cp = CompPlan { tb: TimeBase { timescale: time_scale, ..cp.tb }, layers: Vec::new(), ..comp_header(cp) };
        &layer_cp
    };

    if let LayerSrc::Shape(items) = &mut src
        && let Some(c) = layer.props.sub("contents")
    {
        let mut sx = crate::shapes::ShCx { cp, col };
        *items = crate::shapes::plan_contents(c, &what("shape layer"), 0, &mut sx);
    }
    // Text animators count their keys in the layer's time base too.
    if let LayerSrc::Text(tp) = &mut src {
        tp.animators = crate::textanim::plan(layer, cp, &format!("{comp_name}: '{}'", layer.name), col);
    }

    // Switches and features that are not written.
    // The 3D switch is written (`ldta@38` bit 0x04, 015); a 3D layer's camera, lights and
    // material options are not, and its X / Y rotation and orientation are reported below.
    let per_char_3d = matches!(layer.props.prop("text/perChar3d").map(|p| &p.value), Some(effectcraft_keyframe::Value::Bool(true)));
    if layer.switches.three_d && matches!(src, LayerSrc::Text(_)) && per_char_3d {
        col.lose(LossCategory::ThreeD, what("per-character 3D of the text layer is not written; 3D layer"));
    }
    // Track matte (403-408): the mode and the matte layer's id; a matte layer that is not
    // exported (or the layer itself) is reported in `resolve_mattes`.
    let matte = layer.track_matte.map(|tm| {
        let mode = match tm.kind {
            effectcraft_project::MatteKind::Alpha => 1,
            effectcraft_project::MatteKind::AlphaInverted => 2,
            effectcraft_project::MatteKind::Luma => 3,
            effectcraft_project::MatteKind::LumaInverted => 4,
        };
        (tm.layer, mode)
    });
    // Layer markers count layer time in the layer's units, as its keys do.
    let markers = crate::markers::plan(&layer.markers, &cp.tb, &what("layer"), col);
    let fx = crate::effects::plan_effects(layer, source_size, cp, comp_name, col);
    let styles = crate::styles::plan_styles(layer, comp, cp, &format!("{comp_name}: '{}'", layer.name), col);
    let is_text = matches!(src, LayerSrc::Text(_));
    let cam_light = matches!(src, LayerSrc::Camera(_) | LayerSrc::Light(_));
    let is_shape = matches!(src, LayerSrc::Shape(_));
    let masks = if is_text || is_shape {
        let n = layer.masks().map_or(0, |m| m.groups().count());
        if n > 0 {
            col.lose(LossCategory::Masks, what(&format!("{n} mask(s) on a text layer (the path unit on text layers is not verified) on")));
        }
        Vec::new()
    } else {
        crate::masks::plan_masks(layer, source_size, cp, comp_name, col)
    };
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
            // A camera's or light's point of interest is After Effects' Anchor Point.
            let id = if cam_light && kind == PropKind::Anchor { "poi" } else { kind.id() };
            let Some(p) = tr.get(id) else { continue };
            if let Some(pp) = plan_prop(kind, None, p, comp, source_size, is_text || cam_light || is_shape, cp, &layer.name, comp_name, col) {
                col.exported.properties += 1;
                props.push(pp);
            }
        }
        // A camera's or light's block holds After Effects' own Position and point of interest:
        // a value at EffectCraft's default is written too.
        if cam_light {
            for (kind, id) in [(PropKind::Position, "position"), (PropKind::Anchor, "poi")] {
                if props.iter().any(|p: &PropPlan| p.kind == kind) {
                    continue;
                }
                if let Some(effectcraft_keyframe::Value::Vec3(v)) = tr.get(id).map(|p| &p.value) {
                    let value: Vec<f64> = v.iter().map(|c| if c.is_finite() { *c } else { 0.0 }).collect();
                    props.push(PropPlan { kind, value, keys: Vec::new(), expr: None });
                    col.exported.properties += 1;
                }
            }
        }
        for node in &tr.children {
            let id = node.match_id();
            if PropKind::ALL.iter().any(|k| k.id() == id) || (cam_light && id == "poi") {
                continue;
            }
            if let Some(p) = node.as_prop()
                && is_non_default_extra(id, p)
            {
                col.lose(LossCategory::TransformProperties, format!("{comp_name}: '{}' {}", layer.name, p.name));
            }
        }
    }
    let remap = remap_prop.filter(|_| remapped).map(|p| plan_remap(p, comp, cp, &layer.name, comp_name, col));
    let mut name = cut_name(&layer.name, 255, &what("layer"), col);
    // A text layer named after its text stores no name (110, 113: After Effects shows the text).
    if let LayerSrc::Text(tp) = &src
        && tp.text.split('\r').next().is_some_and(|first| first.chars().take(64).collect::<String>() == name)
    {
        name.clear();
    }
    col.exported.layers += 1;
    match (&src, layer.switches.adjustment) {
        (LayerSrc::Null, _) => col.exported.null_layers += 1,
        (LayerSrc::Comp(_), _) => col.exported.precomp_layers += 1,
        (LayerSrc::Footage(_), _) => col.exported.footage_layers += 1,
        (LayerSrc::Text(_), _) => col.exported.text_layers += 1,
        (LayerSrc::Camera(_), _) => col.exported.cameras += 1,
        (LayerSrc::Light(_), _) => col.exported.lights += 1,
        (LayerSrc::Shape(_), _) => col.exported.shape_layers += 1,
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
        // Reversed (981): After Effects' in point is the later end (source 0 at the start).
        in_rel: unstretch(if stretch.0 < 0 { out_abs } else { in_abs }.saturating_sub(start)),
        out_rel: unstretch(if stretch.0 < 0 { in_abs } else { out_abs }.saturating_sub(start)),
        stretch,
        time_scale,
        blend,
        parent,
        matte,
        markers,
        ec_id: layer.id,
        video: layer.switches.video,
        audio: layer.switches.audio,
        solo: layer.switches.solo,
        locked: layer.switches.locked,
        shy: layer.switches.shy,
        collapse: layer.switches.collapse || is_shape,
        effects: layer.switches.effects,
        motion_blur: layer.switches.motion_blur,
        guide: layer.switches.guide,
        three_d: layer.switches.three_d || cam_light,
        props,
        masks,
        fx,
        source_size,
        remap,
        styles,
    })
}

/// Whether any property below `g` has keyframes.
/// Whether a property of the group has a key with a bezier side.
fn has_bezier_keys(g: &effectcraft_project::PropGroup, depth: usize) -> bool {
    if depth > 64 {
        return true;
    }
    g.children.iter().any(|n| match n {
        effectcraft_project::Node::Group(s) => has_bezier_keys(s, depth + 1),
        effectcraft_project::Node::Prop(p) => {
            p.keys.iter().any(|k| k.in_interp == effectcraft_keyframe::Interp::Bezier || k.out_interp == effectcraft_keyframe::Interp::Bezier)
        }
    })
}

/// Whether showing the composition `to` inside `from` closes a loop (`to` is `from`, or uses it
/// through its own precomposition layers). The walk is iterative and visits each item once.
fn precomp_loop(project: &Project, from: ItemId, to: ItemId) -> bool {
    let mut seen: HashSet<ItemId> = HashSet::new();
    let mut stack = vec![to];
    while let Some(at) = stack.pop() {
        if at == from {
            return true;
        }
        if !seen.insert(at) {
            continue;
        }
        if let Some(ItemKind::Comp(c)) = project.item(at).map(|i| &i.kind) {
            for l in &c.layers {
                if let LayerSource::Comp { item } = &l.source {
                    stack.push(*item);
                }
            }
        }
    }
    false
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
    label: Option<&str>,
    p: &Property,
    comp: &Comp,
    source: (u32, u32),
    text: bool,
    cp: &CompPlan,
    layer: &str,
    comp_name: &str,
    col: &mut Collector,
) -> Option<PropPlan> {
    // A text layer's anchor is stored in pixels, with the origin as its default (385).
    let (sw, sh) = if text { (1.0, 1.0) } else { (f64::from(source.0.max(1)), f64::from(source.1.max(1))) };
    let what = format!("{comp_name}: '{layer}' {}", label.unwrap_or(kind.match_name()));
    let dims = kind.dims();
    let default: Vec<f64> = match kind {
        PropKind::Anchor if text => vec![0.0, 0.0, 0.0],
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
        let side = |interp: Interp| -> u8 {
            match interp {
                Interp::Linear => 1,
                Interp::Hold => 3,
                Interp::Bezier => 2,
            }
        };
        let in_i = side(k.in_interp);
        let out_i = side(k.out_interp);
        let slots = if kind.spatial() || dims == 1 { 1 } else { dims };
        let ease_of = |eases: &[Ease], i: usize| -> EaseSlot {
            let e = eases.get(i).or_else(|| eases.last()).copied().unwrap_or(Ease { speed: 0.0, influence: 1.0 / 3.0 });
            // An Anchor Point stored as a fraction of the source has its spatial speed in pixels
            // per second over the source height (597, 730), as the importer reads it.
            let per = if anchor { sh } else { 1.0 };
            let speed = sane(e.speed / kind.unit() / per).unwrap_or(0.0);
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

/// The Time Remap in the file's terms: a one-component value in seconds, keys in layer time
/// with the plain 48-byte records of a one-dimensional property (319, 354). An enabled remap
/// without keys is written as one key at layer time 0 holding its value: After Effects reads a
/// remap record without keys as switched off (r1, "Freeze": `tdsb` 1 with a static value came
/// back disabled).
fn plan_remap(p: &Property, comp: &Comp, cp: &CompPlan, layer: &str, comp_name: &str, col: &mut Collector) -> PropPlan {
    // Rotation has the same layout (one component, stored as is, not spatial).
    let planned = plan_prop(PropKind::Rotation, Some("Time Remap"), p, comp, (1, 1), false, cp, layer, comp_name, col);
    col.exported.time_remaps += 1;
    let mut plan = planned.unwrap_or_else(|| PropPlan { kind: PropKind::Rotation, value: vec![0.0], keys: Vec::new(), expr: None });
    if plan.keys.is_empty() {
        plan.keys.push(KeyPlan {
            time: 0,
            value: plan.value.clone(),
            in_interp: 1,
            out_interp: 1,
            in_ease: vec![EaseSlot::default()],
            out_ease: vec![EaseSlot::default()],
            spatial_in: [0.0; 3],
            spatial_out: [0.0; 3],
            spatial_auto: false,
            roving: false,
            label: 0,
        });
        col.exported.keyframes += 1;
    }
    plan
}
