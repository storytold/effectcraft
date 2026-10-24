//! Layer masks: path, mode, invert, feather, opacity and expansion, with keyframes and
//! expressions, in EffectCraft's units.
//!
//! Units (`docs/aep-format.md` 3.4 and 3.8): the path is stored as `f32` points normalised inside a
//! box that is itself normalised to the **layer source** size (090 on a 1920x1080 solid, 309 on a
//! 100x100 solid), so vertices become layer pixels with the origin at the layer's top left, as the
//! project's masks are; a tangent is the handle minus the vertex (232). Feather and expansion are
//! pixels, opacity a fraction (percent here). The path keyframes (64-byte records, 093) carry time,
//! interpolation, label, roving and the ease at byte 24 (599, 700: EffectCraft's path at 0.5 / 1 /
//! 1.5 s within 0.5 px of After Effects'); only speed 0 is sampled, another speed is imported as
//! progress per second and reported.

use effectcraft_aep::{Layer as AepLayer, MaskAtom, MaskMode as AepMode, PropNode, ShapePath as AepPath, ValueSource};
use effectcraft_keyframe::{Ease, Interp, Keyframe, ShapePath, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{GroupKind, MaskMode, Node, PropGroup as EcGroup};

use crate::props::{Kind, LIMIT, import_expression, import_keys, interp};
use crate::report::{Collector, LossCategory};
use crate::time::TimeMap;

/// The most vertices one mask path may have (a traced mask has a few hundred). Larger paths are
/// reported as damaged data and the mask is not imported.
pub(crate) const MAX_VERTICES: usize = 65_536;

fn mode(m: AepMode) -> Option<MaskMode> {
    Some(match m {
        AepMode::None => MaskMode::None,
        AepMode::Add => MaskMode::Add,
        AepMode::Subtract => MaskMode::Subtract,
        AepMode::Intersect => MaskMode::Intersect,
        AepMode::Lighten => MaskMode::Lighten,
        AepMode::Darken => MaskMode::Darken,
        AepMode::Difference => MaskMode::Difference,
        AepMode::Other(_) => return None,
    })
}

/// A stored path in layer pixels (`size` is the layer's source size), or why it cannot be used.
/// The path of a mask whose path was never set: the source rectangle, clockwise from the top left
/// as After Effects reads it back (900: `[0,0] [0,1080] [1920,1080] [1920,0]`), no tangents.
fn source_rectangle(size: (u32, u32)) -> ShapePath {
    let (w, h) = (f64::from(size.0), f64::from(size.1));
    ShapePath {
        vertices: vec![[0.0, 0.0], [0.0, h], [w, h], [w, 0.0]],
        in_tangents: vec![[0.0, 0.0]; 4],
        out_tangents: vec![[0.0, 0.0]; 4],
        closed: true,
        feather: Vec::new(),
    }
}

pub(crate) fn to_path(s: &AepPath, size: (u32, u32)) -> Result<ShapePath, &'static str> {
    if !s.points.len().is_multiple_of(3) {
        return Err("its point count is not a multiple of three");
    }
    let n = s.vertex_count();
    if n == 0 {
        return Err("its path has no vertices");
    }
    if n > MAX_VERTICES {
        return Err("its path has too many vertices");
    }
    let vertices = s.vertices((f64::from(size.0), f64::from(size.1)));
    let ok = |p: [f64; 2]| p.iter().all(|c| c.is_finite() && c.abs() <= LIMIT);
    let mut path = ShapePath {
        vertices: Vec::with_capacity(n),
        in_tangents: Vec::with_capacity(n),
        out_tangents: Vec::with_capacity(n),
        closed: !s.open,
        feather: Vec::new(),
    };
    for v in &vertices {
        let (i, o) = (v.in_tangent(), v.out_tangent());
        if !(ok(v.position) && ok(i) && ok(o)) {
            return Err("its path holds a number that is not valid");
        }
        path.vertices.push(v.position);
        path.in_tangents.push(i);
        path.out_tangents.push(o);
    }
    Ok(path)
}

/// Imports the masks of `layer` into its `masks` group. `size` is the layer's source size (1 x 1
/// on a layer without a source, whose mask boxes are pixels); `source` says whether the layer has
/// a source (a mask without a stored path is then its rectangle, 900).
#[allow(clippy::too_many_arguments)]
pub(crate) fn import_masks(
    layer: &AepLayer<'_, '_>,
    comp: &str,
    layer_name: &str,
    size: (u32, u32),
    source: bool,
    tm: &TimeMap,
    ids: &mut Ids<'_>,
    target: &mut EcGroup,
    col: &mut Collector,
) {
    for (i, atom) in layer.mask_atoms().iter().enumerate() {
        let shown = atom.mask.name.clone().unwrap_or_else(|| format!("Mask {}", i.saturating_add(1)));
        let what = format!("{comp}: '{layer_name}' {shown}");
        if let Some(g) = import_mask(atom, i, &shown, &what, size, source, tm, ids, col) {
            target.children.push(Node::Group(g));
            col.imported.masks = col.imported.masks.saturating_add(1);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn import_mask(
    atom: &MaskAtom<'_, '_>,
    index: usize,
    name: &str,
    what: &str,
    size: (u32, u32),
    source: bool,
    tm: &TimeMap,
    ids: &mut Ids<'_>,
    col: &mut Collector,
) -> Option<EcGroup> {
    let m = &atom.mask;
    // The path: the first shape (the static path, or the shape of the first key). A mask atom
    // without a Mask Path entry is a mask whose path was never set: After Effects reads it back as
    // the layer's source rectangle (900; 901 renders it shrunk by its expansion).
    let unset = atom.path_shapes.is_empty() && atom.atom.get("ADBE Mask Shape").is_none();
    let first = match atom.path_shapes.first() {
        Some(Some(s)) => to_path(s, size),
        Some(None) => Err("its path is unreadable"),
        None if unset && source => Ok(source_rectangle(size)),
        None if unset => Err("it has no path (the default path of a layer without a source is not sampled)"),
        None => Err("it has no path"),
    };
    let first = match first {
        Ok(p) => p,
        Err(why) => {
            col.lose(LossCategory::Masks, format!("{what}: not imported, {why}"));
            return None;
        }
    };
    let mask_mode = match m.mode {
        Some(code) => mode(code).unwrap_or_else(|| {
            let shown = match code {
                AepMode::Other(c) => c,
                _ => 0,
            };
            col.lose(LossCategory::Masks, format!("{what}: mode code {shown} is not known, imported as Add"));
            MaskMode::Add
        }),
        None => {
            col.lose(LossCategory::DamagedData, format!("{what}: mode and invert switch unreadable, Add used"));
            MaskMode::Add
        }
    };
    let color = build::MASK_COLORS.get(index % build::MASK_COLORS.len()).copied().unwrap_or([0xb1, 0xb4, 0x4f]);
    let mut g = build::mask(ids, name, first, mask_mode, color);
    if let GroupKind::Mask { inverted, .. } = &mut g.kind {
        *inverted = m.inverted.unwrap_or(false);
    }
    // The path keyframes.
    if let (Some(path), Some(prop)) = (atom.path, g.get_mut("path")) {
        if m.path_animated {
            import_path_keys(&atom.path_shapes, &path, what, size, tm, prop, col);
        }
        import_expression(&path, prop, col);
    }
    // Feather, opacity and expansion.
    let feather = m.feather;
    set_static(&mut g, "feather", Value::Vec2(feather.value), feather.source, |v| v.components().iter().all(|c| *c >= 0.0), what, "feather", col);
    let opacity = m.opacity_percent;
    set_static(
        &mut g,
        "opacity",
        Value::Scalar(opacity.value),
        opacity.source,
        |v| v.components().iter().all(|c| (0.0..=100.0).contains(c)),
        what,
        "opacity",
        col,
    );
    let expansion = m.expansion;
    set_static(&mut g, "expansion", Value::Scalar(expansion.value), expansion.source, |_| true, what, "expansion", col);
    for (match_name, id, kind) in [
        ("ADBE Mask Feather", "feather", Kind::MaskFeather),
        ("ADBE Mask Opacity", "opacity", Kind::MaskOpacity),
        ("ADBE Mask Offset", "expansion", Kind::MaskExpansion),
    ] {
        let Some(PropNode::Property(node)) = atom.atom.get(match_name) else { continue };
        let Some(prop) = g.get_mut(id) else { continue };
        if node.is_animated() {
            import_keys(&node, kind, what, match_name, tm, prop, col);
        }
        import_expression(&node, prop, col);
    }
    Some(g)
}

/// Sets a static mask value when it is valid; otherwise the default stays and it is reported.
#[allow(clippy::too_many_arguments)]
fn set_static(g: &mut EcGroup, id: &str, value: Value, source: ValueSource, valid: impl Fn(&Value) -> bool, what: &str, field: &str, col: &mut Collector) {
    match source {
        ValueSource::Default | ValueSource::Keyframed => {}
        ValueSource::Unreadable => col.lose(LossCategory::DamagedData, format!("{what}: {field} unreadable, default used")),
        ValueSource::Stored => {
            if crate::props::sane(&value) && valid(&value) {
                if let Some(p) = g.get_mut(id) {
                    p.value = value;
                }
            } else {
                col.lose(LossCategory::DamagedData, format!("{what}: {field} holds a number that is not valid, default used"));
            }
        }
    }
}

/// The path keyframes: key `i` of the path property with shape `i` of `omks` (`shapes`); `size`
/// scales the stored box (the source size for masks, 1 x 1 for shape paths, whose box is pixels).
pub(crate) fn import_path_keys(
    shapes: &[Option<AepPath>],
    path: &effectcraft_aep::Property<'_, '_>,
    what: &str,
    size: (u32, u32),
    tm: &TimeMap,
    prop: &mut effectcraft_project::Property,
    col: &mut Collector,
) {
    let Some(list) = path.keys() else {
        col.lose(LossCategory::DamagedData, format!("{what}: path keyframes unreadable, the first shape is static"));
        return;
    };
    let records: Vec<_> = list.keyframes().collect();
    if records.len() != shapes.len() {
        col.lose(LossCategory::DamagedData, format!("{what}: {} path keyframes but {} shapes, the first shape is static", records.len(), shapes.len()));
        return;
    }
    let scale = path.time_scale().filter(|s| *s != 0).unwrap_or(tm.timescale);
    let mut keys: Vec<Keyframe> = Vec::new();
    for (rec, shape) in records.iter().zip(shapes) {
        let Some(units) = rec.time_units() else {
            col.lose(LossCategory::DamagedData, format!("{what}: unreadable path keyframe"));
            continue;
        };
        let path = match shape.as_ref().ok_or("its shape is unreadable").and_then(|s| to_path(s, size)) {
            Ok(p) => p,
            Err(why) => {
                col.lose(LossCategory::DamagedData, format!("{what}: a path keyframe was dropped, {why}"));
                continue;
            }
        };
        let mut key = Keyframe::new(tm.units(i64::from(units), scale), Value::Path(path));
        key.in_interp = interp(rec.interpolation_in(), col, what);
        key.out_interp = interp(rec.interpolation_out(), col, what);
        if key.in_interp == Interp::Bezier || key.out_interp == Interp::Bezier {
            // The ease at byte 24 (599, 700): influences as fractions; a speed of 0 is exact, the
            // unit of another speed is not verified (taken as progress per second).
            match rec.ease().filter(|e| [e.in_speed, e.in_influence, e.out_speed, e.out_influence].iter().all(|v| v.is_finite())) {
                Some(e) => {
                    if key.in_interp == Interp::Bezier {
                        key.in_ease = vec![Ease { speed: e.in_speed, influence: e.in_influence.clamp(0.001, 1.0) }];
                    }
                    if key.out_interp == Interp::Bezier {
                        key.out_ease = vec![Ease { speed: e.out_speed, influence: e.out_influence.clamp(0.001, 1.0) }];
                    }
                    if (key.in_interp == Interp::Bezier && e.in_speed != 0.0) || (key.out_interp == Interp::Bezier && e.out_speed != 0.0) {
                        col.lose(LossCategory::ApproximatedKeyframes, format!("{what}: path key ease speed imported unverified (only speed 0 is sampled)"));
                    }
                }
                None => {
                    col.lose(LossCategory::ApproximatedKeyframes, format!("{what}: bezier path key read as linear (its ease is unreadable)"));
                    key.in_interp = if key.in_interp == Interp::Bezier { Interp::Linear } else { key.in_interp };
                    key.out_interp = if key.out_interp == Interp::Bezier { Interp::Linear } else { key.out_interp };
                }
            }
        }
        key.roving = rec.is_roving().unwrap_or(false);
        key.label = rec.label().filter(|l| *l <= 16).unwrap_or(0);
        keys.push(key);
    }
    if keys.is_empty() {
        return;
    }
    keys.sort_by_key(|k| k.time);
    let before = keys.len();
    keys.dedup_by_key(|k| k.time);
    if keys.len() != before {
        col.lose(LossCategory::DamagedData, format!("{what}: path keyframes at the same time"));
    }
    // The first key's incoming and the last key's outgoing ease mean nothing.
    if let Some(first) = keys.first_mut() {
        first.in_ease.clear();
    }
    if let Some(last) = keys.last_mut() {
        last.out_ease.clear();
    }
    col.imported.keyframes = col.imported.keyframes.saturating_add(keys.len());
    if let Some(first) = keys.first() {
        prop.value = first.value.clone();
    }
    prop.keys = keys;
}
