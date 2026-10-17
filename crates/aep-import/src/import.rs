//! The walk over the project: items first, then the layers of every composition.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use effectcraft_aep::{
    BlendMode as AepBlend, CompHeader, Item as AepItem, ItemKind as AepKind, Layer as AepLayer, LayerHeader, LayerType, Project as AepProject,
};
use effectcraft_aep::{PropNode, Quality as AepQuality, Rational, Renderer as AepRenderer};
use effectcraft_color::{BlendMode, Label};
use effectcraft_project::build;
use effectcraft_project::{Comp, Footage, FootageKind, ItemId, ItemKind, Layer, LayerId, LayerSource, Project, Quality, Solid};
use effectcraft_time::Tick;

use crate::ImportOptions;
use crate::props::import_transform;
use crate::report::{Collector, ImportReport, LossCategory};
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
}

pub(crate) fn run(project: &mut Project, view: &AepProject<'_, '_>, options: &ImportOptions<'_>) -> (ItemId, Vec<ItemId>, Vec<ItemId>, ImportReport) {
    let mut cx = Ctx { sources: HashMap::new(), uses: HashMap::new(), created: Vec::new(), col: Collector::default(), options };
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
        build_layers(project, &mut cx, p);
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
    let name = if name.is_empty() { "Footage".to_string() } else { name };
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
    let path = f.path.clone().unwrap_or_default();
    let present = !path.is_empty() && (cx.options.exists)(&path);
    if !present {
        let shown = if path.is_empty() { format!("{name} (no path stored)") } else { path.clone() };
        cx.col.missing_footage.push(shown);
    }
    let kind = footage_kind(&path);
    let footage = Footage {
        path,
        kind,
        width: f.width,
        height: f.height,
        has_video: kind != FootageKind::Audio,
        has_audio: kind == FootageKind::Audio,
        missing: !present,
        ..Footage::default()
    };
    let id = project.add_item(&name, label, Some(parent), ItemKind::Footage(footage));
    cx.created.push(id);
    cx.col.imported.footage += 1;
    if let Some(i) = aep_id {
        cx.sources.insert(i, Source { item: id, name, kind: SourceKind::Footage, width: f.width, height: f.height });
    }
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
        AepBlend::Other(_) => return None,
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

fn build_layers(project: &mut Project, cx: &mut Ctx<'_>, p: &PendingComp<'_, '_>) {
    let Some(comp_view) = p.item.comp() else { return };
    let Some(mut comp) = project.comp(p.id).cloned() else { return };
    let comp_name = project.item(p.id).map(|i| i.name.clone()).unwrap_or_default();
    let tm = p.map;
    let comp_size = (comp.width, comp.height);
    // aep layer id -> project layer id, and the stored parent links, resolved at the end.
    let mut ids: HashMap<u32, LayerId> = HashMap::new();
    let mut skipped: HashMap<u32, String> = HashMap::new();
    let mut parents: Vec<(u32, LayerId, String, u32)> = Vec::new();

    for layer in comp_view.layers() {
        let Some(h) = layer.header() else {
            cx.col.lose(LossCategory::DamagedData, format!("{comp_name}: a layer header is unreadable, layer not imported"));
            continue;
        };
        let aep_name = layer.name();
        let src = h.source_id.and_then(|s| cx.sources.get(&s)).cloned();
        let name = aep_name.clone().or_else(|| src.as_ref().map(|s| s.name.clone())).unwrap_or_else(|| "Layer".to_string());
        let skip = |cx: &mut Ctx<'_>, skipped: &mut HashMap<u32, String>, category: LossCategory, what: &str| {
            cx.col.lose(category, format!("{comp_name}: {what} '{name}'"));
            skipped.insert(h.id, what.to_string());
        };
        match h.layer_type {
            LayerType::Av => {}
            LayerType::Text => {
                skip(cx, &mut skipped, LossCategory::TextLayers, "text layer");
                continue;
            }
            LayerType::Shape => {
                skip(cx, &mut skipped, LossCategory::ShapeLayers, "shape layer");
                continue;
            }
            LayerType::Camera => {
                skip(cx, &mut skipped, LossCategory::Cameras, "camera");
                continue;
            }
            LayerType::Light => {
                skip(cx, &mut skipped, LossCategory::Lights, "light");
                continue;
            }
            LayerType::Other(code) => {
                skip(cx, &mut skipped, LossCategory::UnsupportedLayers, &format!("layer of type {code}"));
                continue;
            }
        }
        // The source.
        let is_null = h.is_null_object();
        let (source, size) = match (&src, is_null) {
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
            None => cx.col.lose(LossCategory::BlendModes, format!("{comp_name}: '{name}' uses blend mode code {}", h.blend_mode.code())),
        }
        report_layer_content(cx, &layer, &comp_name, &name, &tm);
        if let Some(tr) = l.props.sub_mut("transform") {
            import_transform(&layer, &name, comp_size, size, &tm, tr, &mut cx.col);
        }
        if let Some(parent) = h.parent_id {
            parents.push((h.id, l.id, name.clone(), parent));
        }
        ids.insert(h.id, l.id);
        let counts = &mut cx.col.imported;
        counts.layers += 1;
        match &source {
            LayerSource::Null => counts.null_layers += 1,
            LayerSource::Comp { .. } => counts.precomp_layers += 1,
            LayerSource::Footage { .. } => counts.footage_layers += 1,
            _ if h.is_adjustment() => counts.adjustment_layers += 1,
            _ => counts.solid_layers += 1,
        }
        comp.layers.push(l);
    }

    resolve_parents(&mut comp, &parents, &ids, &skipped, &comp_name, &mut cx.col);

    for m in comp_view.markers() {
        cx.col.lose(LossCategory::Markers, format!("{comp_name}: composition marker at {} s '{}'", m.seconds.unwrap_or(0.0), m.comment));
    }
    if let Some(item) = project.item_mut(p.id) {
        item.kind = ItemKind::Comp(Arc::new(comp));
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
    // The exact ratio `stretch / stretch_den` (50 % is 1/2); a zero or unreadable one is 100 %.
    let stretch = h.stretch_ratio().filter(|r| r.num != 0).unwrap_or(Rational { num: 1, den: 1 });
    l.stretch = stretch.to_f64().map_or(100.0, |r| r * 100.0);
    l.start_time = tm.ticks(h.start.num, h.start.den);
    let abs = |offset: Rational, reduced: Option<Rational>| absolute(h.start, offset, stretch, reduced).map_or(Tick::ZERO, |(n, d)| tm.ticks(n, d));
    l.in_point = abs(h.in_relative, h.in_point());
    l.out_point = abs(h.out_relative, h.out_point()).max(l.in_point);
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

/// Effects, masks, markers and time remapping of an imported layer are counted, not imported.
fn report_layer_content(cx: &mut Ctx<'_>, layer: &AepLayer<'_, '_>, comp: &str, name: &str, tm: &TimeMap) {
    for fx in layer.effects() {
        let shown = fx.name().unwrap_or_else(|| fx.match_name().to_string());
        cx.col.lose(LossCategory::Effects, format!("{comp}: '{name}' has the effect '{shown}' ({})", fx.match_name()));
    }
    for (i, mask) in layer.masks().iter().enumerate() {
        let shown = mask.name.clone().unwrap_or_else(|| format!("Mask {}", i + 1));
        cx.col.lose(LossCategory::Masks, format!("{comp}: '{name}' has '{shown}'"));
    }
    for m in layer.markers() {
        let at = m.seconds.unwrap_or_else(|| tm.units(i64::from(m.time_units), tm.timescale).seconds());
        cx.col.lose(LossCategory::Markers, format!("{comp}: '{name}' has a marker at {at} s '{}'", m.comment));
    }
    if let Some(PropNode::Property(p)) = layer.properties().and_then(|g| g.get("ADBE Time Remapping"))
        && (p.is_animated() || p.flags() == Some(1))
    {
        cx.col.lose(LossCategory::TimeRemapping, format!("{comp}: '{name}'"));
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
