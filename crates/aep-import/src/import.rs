//! The walk over the project: items first, then the layers of every composition.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use effectcraft_aep::{
    BlendMode as AepBlend, CompHeader, Item as AepItem, ItemKind as AepKind, Layer as AepLayer, LayerHeader, LayerType, LightType, Project as AepProject,
    PropNode,
};
use effectcraft_aep::{MatteMode, Quality as AepQuality, Rational, Renderer as AepRenderer, TrackMatte as AepMatte};
use effectcraft_color::{BlendMode, Label};
use effectcraft_keyframe::Value;
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{
    AutoOrient, Comp, Footage, FootageKind, ItemId, ItemKind, Layer, LayerId, LayerSource, LightKind, MatteKind, Project, Quality, Solid, TrackMatte,
};
use effectcraft_time::Tick;

use crate::ImportOptions;
use crate::camera::{import_camera_options, import_light_options};
use crate::effects::{LayerCtx, PendingLayerRef, import_effects, resolve_layer_refs};
use crate::markers::{import_markers, import_time_remap, insert_time_remap};
use crate::masks::import_masks;
use crate::props::{AnchorUnit, import_transform};
use crate::report::{Collector, ImportReport, LossCategory};
use crate::shapes::import_shapes;
use crate::styles::import_layer_styles;
use crate::text::{report_text_extras, text_doc};
use crate::text_anim::import_animators;
use crate::time::{TimeMap, frame_rate};

/// What a layer's source id refers to.
#[derive(Clone, Debug)]
struct Source {
    item: ItemId,
    name: String,
    kind: SourceKind,
    width: u32,
    height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceKind {
    Solid,
    Footage,
    Comp,
}

/// A composition whose layers are still to be built.
struct PendingComp<'f, 'a> {
    id: ItemId,
    item: AepItem<'f, 'a>,
    map: TimeMap,
}

struct Ctx<'o> {
    sources: HashMap<u32, Source>,
    /// Which comps use which comps as precomps (to refuse loops).
    uses: HashMap<ItemId, Vec<ItemId>>,
    created: Vec<ItemId>,
    col: Collector,
    options: &'o ImportOptions<'o>,
    /// Sequence file names the import may still create (all sequences together).
    sequence_budget: u32,
}

pub(crate) fn run(project: &mut Project, view: &AepProject<'_, '_>, options: &ImportOptions<'_>) -> (ItemId, Vec<ItemId>, Vec<ItemId>, ImportReport) {
    let mut cx =
        Ctx { sources: HashMap::new(), uses: HashMap::new(), created: Vec::new(), col: Collector::default(), options, sequence_budget: MAX_SEQUENCE_FILES };
    let root = project.add_item(options.folder_name, Label::None, None, ItemKind::Folder);
    cx.created.push(root);

    let (null_only, _) = null_only_sources(view);
    let mut pending: Vec<PendingComp<'_, '_>> = Vec::new();
    let mut order: Vec<ItemId> = Vec::new();

    // Pass 1: the item tree, depth-first in file order, without recursion.
    let mut stack: Vec<(AepItem<'_, '_>, ItemId)> = view.items().map(|i| (i, root)).collect();
    stack.reverse();
    while let Some((item, parent)) = stack.pop() {
        let name = item.display_name().unwrap_or_default();
        let label = item_label(&item);
        match item.kind() {
            Some(AepKind::Folder) => {
                let name = if name.is_empty() { "Folder".to_string() } else { name };
                let id = project.add_item(&name, label, Some(parent), ItemKind::Folder);
                cx.created.push(id);
                cx.col.imported.folders += 1;
                let mut kids: Vec<_> = item.children().map(|k| (k, id)).collect();
                kids.reverse();
                stack.extend(kids);
            }
            Some(AepKind::Composition) => {
                let name = if name.is_empty() { "Composition".to_string() } else { name };
                let Some(comp_view) = item.comp() else { continue };
                let Some(header) = comp_view.header() else {
                    cx.col.lose(LossCategory::DamagedData, format!("composition '{name}': header unreadable, not imported"));
                    continue;
                };
                let tb = header.time_base;
                let Some(rate) = frame_rate(tb.timescale, tb.frame_duration) else {
                    cx.col.lose(LossCategory::DamagedData, format!("composition '{name}': time base unreadable, not imported"));
                    continue;
                };
                let map = TimeMap { timescale: tb.timescale, frame_duration: tb.frame_duration, rate };
                let comp = new_comp(&header, comp_view.renderer(), &map);
                let id = project.add_item(&name, label, Some(parent), ItemKind::Comp(Arc::new(comp)));
                cx.created.push(id);
                cx.col.imported.compositions += 1;
                order.push(id);
                if let Some(aep_id) = item.id() {
                    cx.sources
                        .insert(aep_id, Source { item: id, name, kind: SourceKind::Comp, width: u32::from(header.width), height: u32::from(header.height) });
                }
                pending.push(PendingComp { id, item, map });
            }
            Some(AepKind::Footage) => import_footage(project, &mut cx, &item, name, label, parent, &null_only),
            Some(AepKind::Other(code)) => cx.col.lose(LossCategory::UnsupportedItems, format!("'{name}': item type {code}")),
            None => cx.col.lose(LossCategory::DamagedData, "a project item without a readable type".to_string()),
        }
    }

    // Pass 2: the layers.
    for p in &pending {
        build_layers(project, &mut cx, view, p);
    }

    let Ctx { created, col, .. } = cx;
    (root, order, created, col.finish())
}

fn item_label(item: &AepItem<'_, '_>) -> Label {
    item.label().and_then(|i| Label::ALL.get(usize::from(i)).copied()).unwrap_or(Label::None)
}

/// The ids of solid items that only null layers use (a null object's hidden placeholder solid,
/// which has no counterpart in the project: the null layer has no item). Also the ids used by any
/// other layer.
fn null_only_sources(view: &AepProject<'_, '_>) -> (HashSet<u32>, HashSet<u32>) {
    let (mut by_null, mut by_other) = (HashSet::new(), HashSet::new());
    for item in view.all_items() {
        let Some(comp) = item.comp() else { continue };
        for layer in comp.layers() {
            let Some(h) = layer.header() else { continue };
            let Some(src) = h.source_id else { continue };
            if h.is_null_object() {
                by_null.insert(src);
            } else {
                by_other.insert(src);
            }
        }
    }
    let only = by_null.difference(&by_other).copied().collect();
    (only, by_other)
}

fn new_comp(h: &CompHeader, renderer: Option<AepRenderer>, map: &TimeMap) -> Comp {
    let duration = map.units(i64::from(h.duration_units), map.timescale);
    let mut comp = Comp::new(u32::from(h.width), u32::from(h.height), map.rate, duration);
    let start = map.units(i64::from(h.work_area_start_units), map.timescale);
    let end = h.work_area_end_units.map_or(duration, |e| map.units(i64::from(e), map.timescale));
    comp.work_area = if start >= Tick::ZERO && end > start { (start, end) } else { (Tick::ZERO, duration) };
    comp.background = h.background.map(|c| f32::from(c) / 255.0);
    comp.pixel_aspect = h.pixel_aspect.to_f64().filter(|v| v.is_finite() && *v > 0.0).unwrap_or(1.0);
    comp.shutter_angle = f64::from(h.shutter_angle);
    // The stored phase; a header without one centres the shutter on the frame.
    comp.shutter_phase = h.shutter_phase.map_or(-comp.shutter_angle / 2.0, f64::from);
    if let Some(start) = h.display_start.filter(|r| r.num >= 0 && r.den > 0) {
        comp.display_start = map.ticks(start.num, start.den);
    }
    comp.enable_motion_blur = h.motion_blur;
    if renderer.is_some_and(|r| r.match_name == "ADBE Calder") {
        comp.renderer = effectcraft_project::Renderer::Advanced3D;
    }
    comp
}

fn import_footage(project: &mut Project, cx: &mut Ctx<'_>, item: &AepItem<'_, '_>, name: String, label: Label, parent: ItemId, null_only: &HashSet<u32>) {
    let Some(f) = item.footage() else {
        cx.col.lose(LossCategory::DamagedData, format!("footage '{name}': data unreadable, not imported"));
        return;
    };
    let aep_id = item.id();
    // An item without a stored name shows its file's name, a sequence `shot_[0101-0103].exr`
    // (126, 603-607).
    let name = if name.is_empty() { default_footage_name(item, &f).unwrap_or_else(|| "Footage".to_string()) } else { name };
    if f.is_solid {
        // A null object's placeholder solid is not an item here.
        if aep_id.is_some_and(|i| null_only.contains(&i)) {
            if let Some(i) = aep_id {
                cx.sources.insert(i, Source { item: ItemId(0), name, kind: SourceKind::Solid, width: f.width, height: f.height });
            }
            return;
        }
        let rgb = f.solid_rgb().unwrap_or([0.0; 3]);
        let solid = Solid { color: rgb, width: f.width, height: f.height, pixel_aspect: 1.0 };
        let id = project.add_item(&name, label, Some(parent), ItemKind::Solid(solid));
        cx.created.push(id);
        cx.col.imported.solids += 1;
        if let Some(i) = aep_id {
            cx.sources.insert(i, Source { item: id, name, kind: SourceKind::Solid, width: f.width, height: f.height });
        }
        return;
    }
    let mut path = f.path.clone().unwrap_or_default();
    // An image sequence's alias is its folder; its files are named by the stored numbering
    // (353, 603). The first file stands for the sequence, as in EffectCraft.
    let mut sequence = Vec::new();
    if let Some(seq) = item.sequence_info().filter(|s| !path.is_empty() && s.end >= s.start) {
        let frames = (seq.end - seq.start).saturating_add(1);
        let sep = if path.contains('\\') { '\\' } else { '/' };
        let folder = path.trim_end_matches(['\\', '/']).to_string();
        if frames > MAX_SEQUENCE_FRAMES || frames > cx.sequence_budget {
            // Not expanded: the item keeps its first file (a still standing for the sequence).
            cx.col.lose(
                LossCategory::DamagedData,
                format!("footage '{name}': a sequence of {frames} frames is longer than the import expands; only its first file is linked"),
            );
            path = format!("{folder}{sep}{}", seq.file_name(seq.start));
        } else {
            cx.sequence_budget -= frames;
            sequence = (seq.start..=seq.end).map(|n| format!("{folder}{sep}{}", seq.file_name(n))).collect();
        }
        if let Some(first) = sequence.first() {
            path = first.clone();
        }
    }
    let present = !path.is_empty() && (cx.options.exists)(&path);
    if !present {
        let shown = if path.is_empty() { format!("{name} (no path stored)") } else { path.clone() };
        cx.col.missing_footage.push(shown);
    }
    let kind = if sequence.is_empty() { footage_kind(&path) } else { FootageKind::Sequence };
    let mut footage = Footage {
        path,
        kind,
        width: f.width,
        height: f.height,
        has_video: kind != FootageKind::Audio,
        has_audio: kind == FootageKind::Audio,
        missing: !present,
        sequence,
        ..Footage::default()
    };
    // The native frame rate and the stored duration of sequences and clips (352, 603, 606).
    if matches!(kind, FootageKind::Sequence | FootageKind::Video) && f.native_frame_rate > 0 {
        footage.frame_rate = effectcraft_time::FrameRate::new(i64::from(f.native_frame_rate), 1);
    }
    if kind == FootageKind::Sequence {
        footage.sync_sequence_duration();
    } else if let Some(d) = f.duration.filter(|d| d.num > 0 && d.den > 0) {
        footage.duration = cx_ticks(d.num, d.den);
        // A clip conformed to another rate (645: 25 native frames at 30 fps): the stored duration
        // counts native frames, which now last 1 / conform s each.
        let conform_rate = item.conform_frame_rate().unwrap_or(0);
        if kind == FootageKind::Video && conform_rate > 0 && f.native_frame_rate > 0 && conform_rate != f.native_frame_rate {
            let native = effectcraft_time::FrameRate::new(i64::from(f.native_frame_rate), 1);
            let conform = effectcraft_time::FrameRate::new(i64::from(conform_rate), 1);
            if let Some(frames) = d.num.checked_mul(i64::from(f.native_frame_rate)).map(|n| n / d.den.max(1)) {
                footage.native_rate = Some(native);
                footage.frame_rate = conform;
                footage.duration = conform.tick_of(frames);
            }
        }
    }
    let id = project.add_item(&name, label, Some(parent), ItemKind::Footage(footage));
    cx.created.push(id);
    cx.col.imported.footage += 1;
    if let Some(i) = aep_id {
        cx.sources.insert(i, Source { item: id, name, kind: SourceKind::Footage, width: f.width, height: f.height });
    }
}

/// The name After Effects shows for a footage item that stores none: the file's name, or for a
/// sequence the name with the frame range (`shot_[0101-0103].exr`).
fn default_footage_name(item: &AepItem<'_, '_>, f: &effectcraft_aep::Footage) -> Option<String> {
    if let Some(s) = item.sequence_info() {
        let w = usize::try_from(s.digits.min(16)).unwrap_or(0);
        return Some(format!("{}[{:0w$}-{:0w$}]{}", s.prefix, s.start, s.end, s.suffix));
    }
    let path = f.path.as_deref()?;
    path.rsplit(['\\', '/']).next().filter(|n| !n.is_empty()).map(str::to_string)
}

/// The longest image sequence that is imported as a list of files.
const MAX_SEQUENCE_FRAMES: u32 = 100_000;
/// The most sequence file names one import creates (all sequences together).
const MAX_SEQUENCE_FILES: u32 = 1_000_000;

/// `num / den` seconds as ticks (saturating).
fn cx_ticks(num: i64, den: i64) -> Tick {
    let t = i128::from(num).saturating_mul(i128::from(effectcraft_time::TICKS_PER_SECOND)) / i128::from(den.max(1));
    Tick(i64::try_from(t).unwrap_or(i64::MAX))
}

/// The footage kind by file extension (the duration and frame rate are not decoded).
fn footage_kind(path: &str) -> FootageKind {
    let ext = path.rsplit('.').next().map(str::to_ascii_lowercase).unwrap_or_default();
    match ext.as_str() {
        "mov" | "mp4" | "m4v" | "avi" | "mxf" | "mkv" | "webm" | "mpg" | "mpeg" | "wmv" | "flv" | "r3d" => FootageKind::Video,
        "wav" | "mp3" | "aac" | "aif" | "aiff" | "flac" | "ogg" | "opus" | "m4a" => FootageKind::Audio,
        _ => FootageKind::Still,
    }
}

fn blend(mode: AepBlend) -> Option<BlendMode> {
    Some(match mode {
        AepBlend::Normal => BlendMode::Normal,
        AepBlend::Add => BlendMode::Add,
        AepBlend::Multiply => BlendMode::Multiply,
        AepBlend::Screen => BlendMode::Screen,
        AepBlend::Overlay => BlendMode::Overlay,
        AepBlend::Difference => BlendMode::Difference,
        AepBlend::Other(code) => blend_code(code)?,
    })
}

/// The blend mode codes beyond the reader's six (`ldta` byte 99): 906 (set by name: Soft Light 8,
/// Hard Light 9, Darken 10, Lighten 11, Exclusion 25, Color Dodge 27, Color Burn 28) and 907
/// (each code rendered by After Effects over a test image and matched against every EffectCraft
/// mode: the named mode was the only one within 1/255 on average inside the patch). Codes 23 / 24
/// render like 27 / 28 on that image; they are taken as the classic variants, which After Effects
/// keeps beside the newer ones (E). Not imported: 1, 21 and 22 (Alpha Add and Luminescent
/// Premul render alike on opaque layers), 3 (Dissolve and Dancing Dissolve share it, 906 sweep)
/// and codes above 34 (not sampled).
fn blend_code(code: u8) -> Option<BlendMode> {
    Some(match code {
        8 => BlendMode::SoftLight,
        9 => BlendMode::HardLight,
        10 => BlendMode::Darken,
        11 => BlendMode::Lighten,
        12 => BlendMode::ClassicDifference,
        13 => BlendMode::Hue,
        14 => BlendMode::Saturation,
        15 => BlendMode::Color,
        16 => BlendMode::Luminosity,
        17 => BlendMode::StencilAlpha,
        18 => BlendMode::StencilLuma,
        19 => BlendMode::SilhouetteAlpha,
        20 => BlendMode::SilhouetteLuma,
        23 => BlendMode::ClassicColorDodge,
        24 => BlendMode::ClassicColorBurn,
        25 => BlendMode::Exclusion,
        27 => BlendMode::ColorDodge,
        28 => BlendMode::ColorBurn,
        29 => BlendMode::LinearDodge,
        30 => BlendMode::LinearBurn,
        31 => BlendMode::LinearLight,
        32 => BlendMode::VividLight,
        33 => BlendMode::PinLight,
        34 => BlendMode::HardMix,
        _ => return None,
    })
}

/// `start + offset * stretch` of two stored times, exact and unreduced when they share a
/// denominator (so frames stay frames), reduced otherwise.
fn absolute(start: Rational, offset: Rational, stretch: Rational, reduced: Option<Rational>) -> Option<(i64, i64)> {
    if start.den == offset.den && start.den != 0 && stretch.den == 1 {
        let num = i128::from(start.num).checked_add(i128::from(offset.num).checked_mul(i128::from(stretch.num))?)?;
        return Some((i64::try_from(num).ok()?, start.den));
    }
    reduced.map(|r| (r.num, r.den))
}

fn build_layers(project: &mut Project, cx: &mut Ctx<'_>, view: &AepProject<'_, '_>, p: &PendingComp<'_, '_>) {
    let Some(comp_view) = p.item.comp() else { return };
    let Some(mut comp) = project.comp(p.id).cloned() else { return };
    let comp_name = project.item(p.id).map(|i| i.name.clone()).unwrap_or_default();
    let tm = p.map;
    let comp_size = (comp.width, comp.height);
    // aep layer id -> project layer id, and the stored parent links, resolved at the end.
    let mut ids: HashMap<u32, LayerId> = HashMap::new();
    let mut skipped: HashMap<u32, String> = HashMap::new();
    let mut parents: Vec<(u32, LayerId, String, u32)> = Vec::new();
    let mut mattes: Vec<PendingMatte> = Vec::new();
    // Effect layer parameters naming another layer.
    let mut layer_refs: Vec<PendingLayerRef> = Vec::new();
    // The id of the layer above in the file (the matte of the older "layer above" form).
    let mut above: Option<u32> = None;

    for layer in comp_view.layers() {
        let Some(h) = layer.header() else {
            cx.col.lose(LossCategory::DamagedData, format!("{comp_name}: a layer header is unreadable, layer not imported"));
            above = None;
            continue;
        };
        let layer_above = above.replace(h.id);
        let matte = layer.track_matte();
        let aep_name = layer.name();
        let src = h.source_id.and_then(|s| cx.sources.get(&s)).cloned();
        // A text layer without a name of its own is shown with its text, as in After Effects.
        let text_name = || {
            (h.layer_type == LayerType::Text)
                .then(|| layer.text_document())
                .flatten()
                .map(|d| crate::text::first_paragraph(&d.raw_text))
                .filter(|t| !t.trim().is_empty())
        };
        let name = aep_name.clone().or_else(|| src.as_ref().map(|s| s.name.clone())).or_else(text_name).unwrap_or_else(|| "Layer".to_string());
        let skip = |cx: &mut Ctx<'_>, skipped: &mut HashMap<u32, String>, category: LossCategory, what: &str| {
            cx.col.lose(category, format!("{comp_name}: {what} '{name}'"));
            skipped.insert(h.id, what.to_string());
        };
        let is_text = h.layer_type == LayerType::Text;
        let is_shape = h.layer_type == LayerType::Shape;
        let is_camera = h.layer_type == LayerType::Camera;
        // Spot and point lights are sampled (252, 254, 255, 343-345), parallel and ambient lights
        // too (701, 702); environment lights are not.
        let light = match h.light_type() {
            Some(LightType::Spot) => Some(LightKind::Spot),
            Some(LightType::Point) => Some(LightKind::Point),
            Some(LightType::Parallel) => Some(LightKind::Parallel),
            Some(LightType::Ambient) => Some(LightKind::Ambient),
            _ => None,
        };
        match h.layer_type {
            LayerType::Av | LayerType::Text | LayerType::Shape | LayerType::Camera => {}
            LayerType::Light if light.is_some() => {}
            LayerType::Light => {
                let code = match h.light_type() {
                    Some(LightType::Other(c)) => c,
                    _ => 0,
                };
                skip(cx, &mut skipped, LossCategory::Lights, &format!("light of type {code} (not sampled)"));
                continue;
            }
            LayerType::Other(code) => {
                skip(cx, &mut skipped, LossCategory::UnsupportedLayers, &format!("layer of type {code}"));
                continue;
            }
        }
        // The source.
        let is_null = h.is_null_object();
        // A text layer has no source; it spans the composition (as a new text layer does).
        let text_size = (comp.width.max(1), comp.height.max(1));
        let (source, size) = match (&src, is_null) {
            _ if is_camera => (LayerSource::Camera, text_size),
            _ if light.is_some() => (LayerSource::Light { kind: light.unwrap_or_default() }, text_size),
            _ if is_text => (LayerSource::Text, text_size),
            // A shape layer has no source either and spans the composition.
            _ if is_shape => (LayerSource::Shape, text_size),
            (Some(s), true) => (LayerSource::Null, (s.width.max(1), s.height.max(1))),
            (None, true) => (LayerSource::Null, (100, 100)),
            (Some(s), false) => match s.kind {
                SourceKind::Solid => (LayerSource::Solid { item: s.item }, (s.width.max(1), s.height.max(1))),
                SourceKind::Footage => (LayerSource::Footage { item: s.item }, (s.width.max(1), s.height.max(1))),
                SourceKind::Comp => {
                    if would_loop(&cx.uses, p.id, s.item) {
                        skip(cx, &mut skipped, LossCategory::UnsupportedLayers, "precomposition loop, layer");
                        continue;
                    }
                    cx.uses.entry(p.id).or_default().push(s.item);
                    (LayerSource::Comp { item: s.item }, (s.width.max(1), s.height.max(1)))
                }
            },
            (None, false) => {
                skip(cx, &mut skipped, LossCategory::UnsupportedLayers, "layer without a usable source:");
                continue;
            }
        };
        let mut l: Layer = build::layer(project, &comp, &name, source.clone(), size, None);
        l.name = name.clone();
        l.label = Label::ALL.get(usize::from(h.label)).copied().unwrap_or(Label::None);
        apply_timing(&mut l, &h, &tm);
        apply_switches(&mut l, &h);
        match blend(h.blend_mode) {
            Some(b) => l.blend_mode = b,
            // Cameras and lights have no blend mode (their byte is 0, 251).
            None if is_camera || light.is_some() => {}
            None => cx.col.lose(LossCategory::BlendModes, format!("{comp_name}: '{name}' uses blend mode code {}", h.blend_mode.code())),
        }
        let what = format!("{comp_name}: '{name}'");
        l.markers = import_markers(layer.markers(), layer.chunk(), &what, &tm, &mut cx.col);
        if let Some(node) = import_time_remap(&layer, &what, &tm, &mut Ids(&mut project.next_id), &mut cx.col) {
            insert_time_remap(&mut l.props, node);
        }
        end_at_source(project, &mut l, comp.frame_rate.frame_duration());
        import_layer_styles(&layer, &mut l, &comp.global_light, &mut Ids(&mut project.next_id), &tm, &what, &mut cx.col);
        // Text and shape layers store their anchor in pixels (385, 462); cameras and lights their
        // point of interest.
        let anchor = if is_camera || light.is_some() {
            AnchorUnit::PointOfInterest
        } else if is_text || is_shape {
            AnchorUnit::Pixels
        } else {
            AnchorUnit::Source(size.0, size.1)
        };
        if let Some(tr) = l.props.sub_mut("transform") {
            import_transform(&layer, &name, comp_size, anchor, &tm, tr, &mut cx.col);
        }
        if is_camera || light.is_some() {
            // Byte 38 bit 0x40: a two-node camera and every light aim at the point of interest; a
            // one-node camera does not (251, 256).
            l.auto_orient = if h.is_auto_oriented() { AutoOrient::TowardsPointOfInterest } else { AutoOrient::Off };
            if matches!(light, Some(LightKind::Point | LightKind::Ambient)) {
                // A point or ambient light has no point of interest here.
                l.auto_orient = AutoOrient::Off;
            }
        }
        if is_camera && let Some(options) = l.props.sub_mut("cameraOptions") {
            import_camera_options(&layer, &what, comp.width, &tm, options, &mut cx.col);
        }
        if light.is_some()
            && let Some(options) = l.props.sub_mut("lightOptions")
        {
            import_light_options(&layer, &what, &tm, options, &mut cx.col);
        }
        if is_shape && let Some(contents) = l.props.sub_mut("contents") {
            import_shapes(&layer, &what, &tm, &mut Ids(&mut project.next_id), contents, &mut cx.col);
        }
        if is_text {
            import_source_text(&mut l, &layer, &what, cx);
            import_text_animators(&mut l, &layer, &what, &tm, &mut Ids(&mut project.next_id), &mut cx.col);
        }
        if let Some(masks) = l.props.sub_mut("masks") {
            // On a text or shape layer, which has no source, the mask box is stored in layer
            // pixels (705, 706), so the scale is 1.
            let mask_scale = if is_text || is_shape { (1, 1) } else { size };
            import_masks(&layer, &comp_name, &name, mask_scale, !(is_text || is_shape), &tm, &mut build::Ids(&mut project.next_id), masks, &mut cx.col);
        }
        if let Some(fx) = l.props.sub_mut("effects") {
            let lcx = LayerCtx { comp: &comp_name, layer: &name, aep_id: h.id, ec_id: l.id, size, tm: &tm };
            import_effects(view, &layer, &lcx, &mut Ids(&mut project.next_id), fx, &mut layer_refs, &mut cx.col);
        }
        if let Some(parent) = h.parent_id {
            parents.push((h.id, l.id, name.clone(), parent));
        }
        ids.insert(h.id, l.id);
        if let Some(m) = matte {
            mattes.push(PendingMatte { layer: l.id, name: name.clone(), matte: m, above: layer_above });
        }
        let counts = &mut cx.col.imported;
        counts.layers += 1;
        match &source {
            LayerSource::Null => counts.null_layers += 1,
            LayerSource::Comp { .. } => counts.precomp_layers += 1,
            LayerSource::Footage { .. } => counts.footage_layers += 1,
            LayerSource::Text => counts.text_layers += 1,
            LayerSource::Shape => counts.shape_layers += 1,
            LayerSource::Camera => counts.cameras += 1,
            LayerSource::Light { .. } => counts.lights += 1,
            _ if h.is_adjustment() => counts.adjustment_layers += 1,
            _ => counts.solid_layers += 1,
        }
        comp.layers.push(l);
    }

    resolve_parents(&mut comp, &parents, &ids, &skipped, &comp_name, &mut cx.col);
    resolve_mattes(&mut comp, &mattes, &ids, &skipped, &comp_name, &mut cx.col);
    resolve_layer_refs(&mut comp, &layer_refs, &ids, &mut cx.col);
    // Global Light is one setting per composition here: the layers' stored values are adopted.
    effectcraft_project::styles::sync_comp(&mut comp);

    if let Some(secl) = p.item.chunk().child_list(b"SecL") {
        comp.markers = import_markers(comp_view.markers(), secl, &comp_name, &tm, &mut cx.col);
    }
    if let Some(item) = project.item_mut(p.id) {
        item.kind = ItemKind::Comp(Arc::new(comp));
    }
}

/// The Source Text of a text layer from its text document, and the reports of what is not
/// imported.
fn import_source_text(l: &mut Layer, layer: &AepLayer<'_, '_>, what: &str, cx: &mut Ctx<'_>) {
    match layer.text_document() {
        Some(doc) => {
            let value = text_doc(&doc, what, cx.options.has_font, &mut cx.col);
            match l.props.prop_mut("text/sourceText") {
                Some(p) => p.value = Value::Text(Box::new(value)),
                None => cx.col.lose(LossCategory::DamagedData, format!("{what}: the text layer has no Source Text property, the text is not imported")),
            }
        }
        None => cx.col.lose(LossCategory::DamagedData, format!("{what}: the text document is unreadable, the layer is imported with empty text")),
    }
    report_text_extras(layer, what, &mut cx.col);
}

/// The text animators of a text layer into its `text/animators` group.
fn import_text_animators(l: &mut Layer, layer: &AepLayer<'_, '_>, what: &str, tm: &TimeMap, ids: &mut Ids<'_>, col: &mut Collector) {
    let Some(PropNode::Group(text)) = layer.properties().and_then(|p| p.get("ADBE Text Properties")) else { return };
    let Some(PropNode::Group(animators)) = text.get("ADBE Text Animators") else { return };
    if animators.entries().is_empty() {
        return;
    }
    match l.props.group_mut("text/animators") {
        Some(target) => import_animators(&animators, what, tm, ids, target, col),
        None => col.lose(LossCategory::TextLayers, format!("{what}: the text layer has no animator group, its text animators are not imported")),
    }
}

/// Whether adding `to` as a precomp of `from` would close a loop.
fn would_loop(uses: &HashMap<ItemId, Vec<ItemId>>, from: ItemId, to: ItemId) -> bool {
    if from == to {
        return true;
    }
    let mut seen = HashSet::new();
    let mut stack = vec![to];
    while let Some(c) = stack.pop() {
        if c == from {
            return true;
        }
        if seen.insert(c) {
            stack.extend(uses.get(&c).into_iter().flatten().copied());
        }
    }
    false
}

fn apply_timing(l: &mut Layer, h: &LayerHeader, tm: &TimeMap) {
    // The exact ratio `stretch / stretch_den` (50 % is 1/2, -100 % is -1/1); a zero or unreadable
    // one is 100 %.
    let stretch = h.stretch_ratio().filter(|r| r.num != 0).unwrap_or(Rational { num: 1, den: 1 });
    l.stretch = stretch.to_f64().map_or(100.0, |r| r * 100.0);
    l.start_time = tm.ticks(h.start.num, h.start.den);
    let abs = |offset: Rational, reduced: Option<Rational>| absolute(h.start, offset, stretch, reduced).map_or(Tick::ZERO, |(n, d)| tm.ticks(n, d));
    // A negative stretch reverses the layer (981-983): the stored in point is then the later
    // end (`start + in * stretch`, 5 s for 0 s of source at -100 %), the out point the earlier.
    let (a, b) = (abs(h.in_relative, h.in_point()), abs(h.out_relative, h.out_point()));
    l.in_point = a.min(b);
    l.out_point = a.max(b);
}

/// After Effects ends a layer of timed footage (a clip, a sequence, a sound) at its source's end
/// when it opens a project, whatever out point the file stores (export test f1: layers written
/// 5 s long over a 1 s clip, a 3-frame sequence and a 1 s WAV read back 1, 0.1 and 1 s long; R31
/// of aep-export.md). A remapped or looped layer keeps its out point, and so does one that runs
/// less than a composition frame past (`frame`): a clip's duration need not be whole frames.
fn end_at_source(project: &Project, l: &mut Layer, frame: Tick) {
    let LayerSource::Footage { item } = &l.source else { return };
    if l.props.prop("timeRemap").is_some() {
        return;
    }
    let Some(ItemKind::Footage(f)) = project.item(*item).map(|i| &i.kind) else { return };
    if !matches!(f.kind, FootageKind::Video | FootageKind::Sequence | FootageKind::Audio) || f.loop_count > 1 || f.duration <= Tick::ZERO {
        return;
    }
    let stretch = l.stretch / 100.0;
    if !(stretch.is_finite() && stretch > 0.0) {
        return;
    }
    let span = (f.duration.0 as f64 * stretch).round();
    if !span.is_finite() || span.abs() > 9.0e18 {
        return;
    }
    let end = Tick(l.start_time.0.saturating_add(span as i64));
    if l.out_point.0.saturating_sub(end.0) >= frame.0.max(1) {
        l.out_point = end.max(l.in_point);
    }
}

fn apply_switches(l: &mut Layer, h: &LayerHeader) {
    let s = &mut l.switches;
    s.video = h.is_video_enabled();
    s.audio = h.is_audio_enabled();
    s.solo = h.is_solo();
    s.locked = h.is_locked();
    s.shy = h.is_shy();
    s.collapse = h.collapses_transformations();
    s.effects = h.is_effects_enabled();
    s.motion_blur = h.is_motion_blur();
    s.adjustment = h.is_adjustment();
    s.three_d = h.is_three_d();
    s.guide = h.is_guide();
    s.quality = match h.quality {
        AepQuality::Draft => Quality::Draft,
        AepQuality::Best | AepQuality::Other(_) => Quality::Best,
    };
}

/// A track matte read from a layer header, resolved once every layer of the composition exists.
struct PendingMatte {
    layer: LayerId,
    name: String,
    matte: AepMatte,
    /// The id of the layer directly above in the file.
    above: Option<u32>,
}

/// Sets the track mattes (402-408: mode at `ldta` byte 107, matte layer id at byte 160). The
/// matte layer can be anywhere in the composition, also below its target (407). A matte with no
/// layer id is the form of After Effects before 23.0, where the matte is always the layer directly
/// above: that layer is used. The matte layer's video switch is imported as stored (After Effects
/// switches it off when the matte is set, 403; EffectCraft, like After Effects, still uses a
/// hidden layer as a matte).
fn resolve_mattes(comp: &mut Comp, mattes: &[PendingMatte], ids: &HashMap<u32, LayerId>, skipped: &HashMap<u32, String>, comp_name: &str, col: &mut Collector) {
    for m in mattes {
        let kind = match m.matte.mode {
            MatteMode::Alpha => MatteKind::Alpha,
            MatteMode::AlphaInverted => MatteKind::AlphaInverted,
            MatteMode::Luma => MatteKind::Luma,
            MatteMode::LumaInverted => MatteKind::LumaInverted,
            MatteMode::Other(code) => {
                col.lose(LossCategory::TrackMattes, format!("{comp_name}: '{}' has a track matte of unknown mode {code}", m.name));
                continue;
            }
        };
        let Some(source) = m.matte.layer.or(m.above) else {
            col.lose(LossCategory::TrackMattes, format!("{comp_name}: '{}' has a track matte but no layer above it", m.name));
            continue;
        };
        let Some(target) = ids.get(&source).copied() else {
            let why = skipped.get(&source).map_or("a layer that does not exist", |s| s.as_str());
            col.lose(LossCategory::TrackMattes, format!("{comp_name}: '{}' has {why} as its track matte", m.name));
            continue;
        };
        if target == m.layer {
            col.lose(LossCategory::TrackMattes, format!("{comp_name}: '{}' is its own track matte", m.name));
            continue;
        }
        if let Some(l) = comp.layer_mut(m.layer) {
            l.track_matte = Some(TrackMatte { layer: target, kind });
            col.imported.track_mattes += 1;
        }
    }
}

/// Sets the parent links that point at an imported layer, without loops.
fn resolve_parents(
    comp: &mut Comp,
    parents: &[(u32, LayerId, String, u32)],
    ids: &HashMap<u32, LayerId>,
    skipped: &HashMap<u32, String>,
    comp_name: &str,
    col: &mut Collector,
) {
    let mut links: HashMap<LayerId, LayerId> = HashMap::new();
    for (_, layer, name, parent) in parents {
        match ids.get(parent) {
            Some(target) => {
                links.insert(*layer, *target);
            }
            None => {
                let why = skipped.get(parent).map_or("a layer that does not exist", |s| s.as_str());
                col.lose(LossCategory::ParentLinks, format!("{comp_name}: '{name}' has {why} as its parent"));
            }
        }
    }
    // Drop every link that leads into a loop (including a layer parented to itself).
    let looped: Vec<LayerId> = links
        .keys()
        .copied()
        .filter(|start| {
            let mut at = *start;
            for _ in 0..=links.len() {
                match links.get(&at) {
                    Some(next) if next == start => return true,
                    Some(next) => at = *next,
                    None => return false,
                }
            }
            false
        })
        .collect();
    for l in &looped {
        links.remove(l);
        let name = comp.layer(*l).map(|x| x.name.clone()).unwrap_or_default();
        col.lose(LossCategory::ParentLinks, format!("{comp_name}: '{name}' has a parent loop"));
    }
    for (layer, parent) in links {
        if let Some(l) = comp.layer_mut(layer) {
            l.parent = Some(parent);
        }
    }
}
