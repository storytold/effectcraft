//! Copied keyframes as After Effects keyframe text, for the system clipboard (#1908). Other
//! applications read this tab-separated format (AE2Blend brings it into Blender). Like After
//! Effects it covers one layer: its copied properties, keyframed ones as one row per key (frame
//! number in comp time, then the value) and static ones as a single row with no frame. Easing
//! and tangents aren't part of the format; EffectCraft's own paste keeps them.

use effectcraft_project::{AutoOrient, Comp, GroupKind, ItemKind, Layer, LayerId, LayerSource, Node, ParamUi, Project, PropGroup, Property, Uid};

use crate::{KeyRef, keyframe_text_effects};

/// Transform properties with the unit their columns are labelled with and, for vectors, the
/// third component a 2D layer shows. X / Y / Z Position (Separate Dimensions) have no unit.
const TRANSFORM: &[(&str, &str, Option<f64>)] = &[
    ("poi", "pixels", Some(0.0)),
    ("anchor", "pixels", Some(0.0)),
    ("position", "pixels", Some(0.0)),
    ("positionX", "", None),
    ("positionY", "", None),
    ("positionZ", "", None),
    ("scale", "percent", Some(100.0)),
    ("orientation", "degrees", None),
    ("rotationX", "degrees", None),
    ("rotationY", "degrees", None),
    ("rotation", "degrees", None),
    ("opacity", "percent", None),
];

/// How a copied property is written: its block's header line, column labels and values.
struct Channel {
    header: String,
    labels: Vec<String>,
    values: Values,
}

/// How a property's value becomes the numbers in a row.
#[derive(Clone, Copy)]
enum Values {
    Plain,
    /// A 2D layer's Transform vector: the third component is the one the layer shows (it may
    /// keep another from pasted 3D keys).
    Flat(f64),
    /// RGBA 0–1 as alpha, red, green, blue 0–255.
    Argb,
    /// A popup's 0-based choice as After Effects' 1-based one.
    OneBased,
    Scale(f64),
}

impl Values {
    fn row(self, mut v: Vec<f64>) -> Vec<f64> {
        match (self, v.as_slice()) {
            (Values::Flat(z), _) => {
                if let Some(c) = v.get_mut(2) {
                    *c = z;
                }
                v
            }
            (Values::Argb, [r, g, b, a]) => [a, r, g, b].map(|c| c * 255.0).to_vec(),
            (Values::OneBased, _) => v.into_iter().map(|c| c + 1.0).collect(),
            (Values::Scale(s), _) => v.into_iter().map(|c| c * s).collect(),
            _ => v,
        }
    }
}

/// The text for copied keys (layer times) and unanimated properties. `None` unless they all come
/// from one layer and at least one property can be written.
pub(crate) fn text(project: &Project, comp: &Comp, keys: &[KeyRef], statics: &[(LayerId, Uid)]) -> Option<String> {
    let lid = keys.first().map(|k| k.layer).or_else(|| statics.first().map(|s| s.0))?;
    if keys.iter().any(|k| k.layer != lid) || statics.iter().any(|s| s.0 != lid) {
        return None;
    }
    let layer = comp.layer(lid)?;
    let fps = comp.frame_rate.as_f64();
    let mut blocks = String::new();
    layer.props.walk("", &mut |_, pr| {
        let mut times: Vec<_> = keys.iter().filter(|k| k.prop == pr.uid).map(|k| k.time).collect();
        times.sort();
        times.dedup();
        let rows: Vec<(String, Vec<f64>)> = if !times.is_empty() {
            let frame = |t| num(layer.comp_time(t).seconds() * fps);
            times.into_iter().filter_map(|t| Some((frame(t), pr.keys.iter().find(|k| k.time == t)?.value.components()))).collect()
        } else if statics.iter().any(|s| s.1 == pr.uid) {
            vec![(String::new(), pr.value.components())]
        } else {
            return;
        };
        let Some(ch) = channel(layer, pr).filter(|ch| !ch.labels.is_empty()) else { return };
        if rows.iter().any(|(_, v)| v.is_empty()) {
            return;
        }
        blocks.push_str(&format!("{}\n\tFrame\t{}\t\n", ch.header, ch.labels.join("\t")));
        for (frame, values) in rows {
            let values: Vec<String> = ch.values.row(values).into_iter().map(num).collect();
            blocks.push_str(&format!("\t{frame}\t{}\t\n", values.join("\t")));
        }
        blocks.push('\n');
    });
    if blocks.is_empty() {
        return None;
    }
    // After Effects writes a text layer's source as 1 × 1. Other layers without a source of
    // their own (shapes, cameras) take the comp's size, and all of them its pixel aspect.
    let (w, h) = match (&layer.source, effectcraft_render::eval::source_size(project, layer)) {
        (LayerSource::Text, _) => (1, 1),
        (_, (0, 0)) => (comp.width, comp.height),
        (_, size) => size,
    };
    let par = match layer.source {
        LayerSource::Footage { item } | LayerSource::Solid { item } | LayerSource::Comp { item } => match project.item(item).map(|i| &i.kind) {
            Some(ItemKind::Footage(f)) => f.pixel_aspect,
            Some(ItemKind::Solid(s)) => s.pixel_aspect,
            Some(ItemKind::Comp(c)) => c.pixel_aspect,
            _ => comp.pixel_aspect,
        },
        _ => comp.pixel_aspect,
    };
    // The layout of current After Effects: a `Layer` line before the layer's properties (parsers
    // such as ae-keyframes reject properties without one), and a tab after every column.
    Some(format!(
        "Adobe After Effects 9.0 Keyframe Data\n\n\tUnits Per Second\t{}\n\tSource Width\t{w}\n\tSource Height\t{h}\n\tSource Pixel Aspect Ratio\t{}\n\tComp Pixel Aspect Ratio\t{}\n\nLayer\n{blocks}\nEnd of Keyframe Data\n",
        num(fps),
        num(par),
        num(comp.pixel_aspect)
    ))
}

/// How the layer's property `pr` is written, by where it sits in the layer: `None` for
/// properties the text doesn't carry.
fn channel(layer: &Layer, pr: &Property) -> Option<Channel> {
    let chain = layer.props.node_chain(pr.uid)?;
    match chain.first().and_then(|n| n.as_group()).map(|g| g.match_id.as_str()) {
        None if pr.match_id == "timeRemap" => Some(Channel { header: "Time Remap".into(), labels: labels("seconds", 1), values: Values::Plain }),
        Some("transform") => transform(layer, pr),
        Some("effects") => effect(&chain, pr),
        Some("masks" | "text" | "contents" | "cameraOptions" | "lightOptions" | "materialOptions") => nested(&chain, pr),
        _ => None,
    }
}

/// Indexed groups by match id, with the kind After Effects names them by: it writes them by kind
/// and place (`Mask #2`, `Group #1`), not by name.
const INSTANCES: &[(&str, &str)] = &[
    ("mask", "Mask"),
    ("animator", "Animator"),
    ("rangeSelector", "Range Selector"),
    ("wigglySelector", "Wiggly Selector"),
    ("expressionSelector", "Expression Selector"),
    ("group", "Group"),
    ("rect", "Rectangle Path"),
    ("ellipse", "Ellipse Path"),
    ("star", "Polystar Path"),
    ("path", "Path"),
    ("fill", "Fill"),
    ("stroke", "Stroke"),
    ("gfill", "Gradient Fill"),
    ("gstroke", "Gradient Stroke"),
    ("trim", "Trim Paths"),
    ("repeater", "Repeater"),
    ("round", "Round Corners"),
    ("offset", "Offset Paths"),
    ("pucker", "Pucker & Bloat"),
    ("twist", "Twist"),
    ("zigzag", "Zig Zag"),
    ("wiggle", "Wiggle Paths"),
    ("merge", "Merge Paths"),
];

/// A property under Masks, Text, Contents or Camera / Light / Material Options: the group's
/// name, then each group down to the property. Instances go by kind and place, and a text
/// selector's own properties carry their place too (`Start #1`).
fn nested(chain: &[&Node], pr: &Property) -> Option<Channel> {
    let groups: Vec<&PropGroup> = chain.iter().filter_map(|n| n.as_group()).collect();
    let parent = groups.last()?;
    let (labels, values) = columns(&parent.match_id, &pr.match_id, pr, false)?;
    let mut header = groups.first()?.name.clone();
    for pair in groups.windows(2) {
        let [outer, g] = pair else { continue };
        match INSTANCES.iter().find(|(id, _)| *id == g.match_id).filter(|_| g.is_instance()) {
            Some((_, kind)) => header.push_str(&format!("\t{kind} #{}", place(outer, g.uid)?)),
            None => header.push_str(&format!("\t{}", g.name)),
        }
    }
    if parent.match_id.ends_with("Selector") {
        header.push_str(&format!("\t{} #{}", pr.name, place(parent, pr.uid)?));
    } else {
        header.push_str(&format!("\t{}", pr.name));
    }
    Some(Channel { header, labels, values })
}

/// An effect property: `Effects`, the effect's name (not a custom one) and place on the layer,
/// then the property's name and After Effects index. Properties in twirl-down groups are
/// written without their group, as After Effects does.
fn effect(chain: &[&Node], pr: &Property) -> Option<Channel> {
    let [Node::Group(effects), Node::Group(fx), ..] = chain else { return None };
    let GroupKind::Effect { effect } = &fx.kind else { return None };
    let name = effectcraft_effects::find(effect).map_or(fx.name.as_str(), |spec| spec.name);
    let path: String = chain.iter().skip(2).filter_map(|n| n.as_group()).map(|g| format!("{}/", g.match_id)).chain([pr.match_id.clone()]).collect();
    let (labels, values) = columns(effect, &path, pr, true)?;
    let header = format!("Effects\t{name} #{}\t{} #{}", place(effects, fx.uid)?, pr.name, effect_index(effect, fx, pr.uid)?);
    Some(Channel { header, labels, values })
}

/// After Effects' index of an effect property: the effect's hidden input layer is #1, and a
/// twirl-down group takes an index where it opens and another where it closes. The hidden
/// source choice EffectCraft keeps beside a layer parameter has none. Where After Effects
/// numbers a parameter otherwise ([`keyframe_text_effects::INDICES`]), counting goes on from it.
fn effect_index(effect: &str, fx: &PropGroup, uid: Uid) -> Option<usize> {
    fn count(effect: &str, g: &PropGroup, prefix: &str, uid: Uid, n: &mut usize) -> bool {
        for c in &g.children {
            match c {
                Node::Prop(p) if is_layer_source(g, p) => {}
                Node::Prop(p) => {
                    let path = format!("{prefix}{}", p.match_id);
                    *n = keyframe_text_effects::INDICES.iter().find(|(e, q, _)| *e == effect && *q == path).map_or(*n + 1, |a| a.2);
                    if p.uid == uid {
                        return true;
                    }
                }
                Node::Group(sub) => {
                    *n += 1;
                    if count(effect, sub, &format!("{prefix}{}/", sub.match_id), uid, n) {
                        return true;
                    }
                    *n += 1;
                }
            }
        }
        false
    }
    let mut n = 1;
    count(effect, fx, "", uid, &mut n).then_some(n)
}

fn is_layer_source(g: &PropGroup, p: &Property) -> bool {
    p.ui == ParamUi::Hidden && g.props().any(|l| l.ui == ParamUi::Layer && effectcraft_effects::layer_source_id(&l.match_id) == p.match_id)
}

/// 1-based position of a child in its group.
fn place(group: &PropGroup, uid: Uid) -> Option<usize> {
    group.children.iter().position(|c| c.uid() == uid).map(|i| i + 1)
}

/// How After Effects writes a property differently from what its editor suggests ([`by_ui`]).
#[derive(Clone, Copy)]
enum Exception {
    /// Not written.
    Skip,
    Unit(&'static str),
    Values(Values),
}

/// Properties After Effects writes differently from what their editor suggests, by the effect
/// or group they're directly in and their match id.
const EXCEPTIONS: &[(&str, &str, Exception)] = &[
    // Path Options' Path picks a mask, which After Effects doesn't write.
    ("pathOptions", "path", Exception::Skip),
    ("mask", "feather", Exception::Unit("pixels")),
    ("mask", "expansion", Exception::Unit("pixels")),
    ("cameraOptions", "zoom", Exception::Unit("pixels")),
    ("cameraOptions", "focusDistance", Exception::Unit("pixels")),
    ("cameraOptions", "aperture", Exception::Unit("pixels")),
    ("cameraOptions", "highlightGain", Exception::Unit("")),
    ("cameraOptions", "highlightSaturation", Exception::Unit("")),
    // After Effects keeps the threshold as 0–1.
    ("cameraOptions", "highlightThreshold", Exception::Values(Values::Scale(1.0 / 255.0))),
    ("lightOptions", "shadowDiffusion", Exception::Unit("pixels")),
    // Off, On, Only: written from 0.
    ("materialOptions", "castsShadows", Exception::Values(Values::Plain)),
    ("materialOptions", "acceptsShadows", Exception::Values(Values::Plain)),
];

/// Column labels and values for a property in `scope`, the effect (by id) or group (by match
/// id) it's directly in, at `path` there (an effect property's groups, then its match id): [`by_ui`]
/// unless [`EXCEPTIONS`] or, in effects, [`keyframe_text_effects::UNITS`] say otherwise.
fn columns(scope: &str, path: &str, pr: &Property, in_effect: bool) -> Option<(Vec<String>, Values)> {
    let (by_editor, values) = by_ui(pr, in_effect)?;
    let exception = EXCEPTIONS.iter().find(|(s, id, _)| *s == scope && *id == path).map(|e| e.2);
    let effect_unit = || keyframe_text_effects::UNITS.iter().find(|(e, p, _)| in_effect && *e == scope && *p == path).map(|u| Exception::Unit(u.2));
    Some(match exception.or_else(effect_unit) {
        None => (by_editor, values),
        Some(Exception::Skip) => return None,
        Some(Exception::Unit(unit)) => (labels(unit, pr.value.dims()), values),
        Some(Exception::Values(values)) => (by_editor, values),
    })
}

/// Column labels and values for a property, by how it's edited. After Effects labels most 0–100
/// sliders (opacities) as percent, and ±100 ones too outside effects (text selector amounts),
/// and other sliders, checkboxes and popups with no unit. [`keyframe_text_effects::UNITS`] lists
/// the effect parameters After Effects labels otherwise.
fn by_ui(pr: &Property, in_effect: bool) -> Option<(Vec<String>, Values)> {
    let dims = pr.value.dims();
    Some(match &pr.ui {
        ParamUi::Color => (["alpha ", "red ", "green ", "blue "].map(String::from).to_vec(), Values::Argb),
        ParamUi::Popup { .. } => (labels("", dims), Values::OneBased),
        ParamUi::Slider { min, max, .. } if *max == 100.0 && (*min == 0.0 || (*min == -100.0 && !in_effect)) => (labels("percent", dims), Values::Plain),
        ParamUi::Percent => (labels("percent", dims), Values::Plain),
        ParamUi::Angle => (labels("degrees", dims), Values::Plain),
        ParamUi::Point | ParamUi::Point3 => (labels("pixels", dims), Values::Plain),
        ParamUi::Number | ParamUi::Slider { .. } | ParamUi::Pixels | ParamUi::Checkbox => (labels("", dims), Values::Plain),
        _ => return None,
    })
}

fn transform(layer: &Layer, pr: &Property) -> Option<Channel> {
    let (id, unit, z) = TRANSFORM.iter().find(|(id, ..)| *id == pr.match_id)?;
    if !active(layer, id) || (pr.three_d_only && !layer.is_3d()) {
        return None;
    }
    // After Effects labels only the first of Orientation's three columns, and names Z Rotation
    // plain Rotation (as AE2Blend expects).
    let labels = if *id == "orientation" { vec![format!("X {unit}")] } else { labels(unit, pr.value.dims()) };
    let values = match z {
        Some(z) if !layer.is_3d() => Values::Flat(*z),
        _ => Values::Plain,
    };
    Some(Channel { header: format!("Transform\t{}", pr.name), labels, values })
}

/// Column labels: the unit, or one per axis (`X pixels`, `Y pixels`; `X ` with no unit, as After
/// Effects writes a shape's Size).
fn labels(unit: &str, dims: usize) -> Vec<String> {
    match dims {
        0 => vec![],
        1 => vec![unit.to_string()],
        _ => ["X", "Y", "Z"].iter().take(dims).map(|axis| format!("{axis} {unit}")).collect(),
    }
}

/// Whether the layer uses this Transform property: one-node cameras (and lights not aimed at
/// it) have no Point of Interest, and Separate Dimensions replaces Position with X / Y / Z
/// Position.
fn active(layer: &Layer, id: &str) -> bool {
    match id {
        "poi" => !(layer.is_camera() || layer.is_light()) || layer.auto_orient == AutoOrient::TowardsPointOfInterest,
        "position" => layer.transform().is_none_or(|tr| tr.get("positionX").is_none()),
        _ => true,
    }
}

/// A number with at most three decimals and no trailing zeros (`23.976`, `960`, `-0.5`).
fn num(v: f64) -> String {
    let s = format!("{:.3}", if v.is_finite() { v } else { 0.0 });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}
