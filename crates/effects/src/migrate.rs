//! Bringing effect instances saved by earlier versions up to date with the registry.
//!
//! Effect instances are ordinary property groups saved with their parameters' display names
//! and UI hints, so a project written before a parameter was renamed, regrouped, added or had
//! its popup reordered still carries the old shape. [`upgrade_instance`] (run when a project is
//! opened) refreshes each instance from its [`EffectSpec`]:
//!
//! - parameters whose id changed are found through [`PARAM_ID_ALIASES`] and renamed;
//! - parameters moved into a twirl-down group (`group/param` ids) are moved there;
//! - display names and UI hints (ranges, popup options) are refreshed, and popup values are
//!   remapped **by label** when the option list changed order ([`POPUP_LABEL_ALIASES`] covers
//!   renamed labels);
//! - parameters added since are created with their defaults.
//!
//! Animation, expressions and values are kept.

use effectcraft_keyframe::Value;
use effectcraft_project::build::Ids;
use effectcraft_project::{Node, ParamUi, PropGroup, Property};

use crate::{EffectSpec, ParamSpec, default_value, group_name, layer_source_id};

/// Parameter ids renamed since they were first saved: `(effect id, old id, new id)`. The new id
/// is a spec id (it may include twirl-down group segments, `group/param`).
pub const PARAM_ID_ALIASES: &[(&str, &str, &str)] = &[
    // Color Correction: Change to Color's single Tolerance became the Tolerance group's Hue.
    ("ec.color.changetocolor", "tolerance", "toleranceGroup/hue"),
    // Roto Brush & Refine Edge: propagation controls got their own twirl-down; motion blur and
    // decontamination settings are nested under Refine Edge Matte.
    ("ec.matte.rotobrush", "rotoBrushMatte/searchRadius", "rotoBrushPropagation/searchRadius"),
    ("ec.matte.rotobrush", "rotoBrushMatte/motionThreshold", "rotoBrushPropagation/motionThreshold"),
    ("ec.matte.rotobrush", "rotoBrushMatte/motionDamping", "rotoBrushPropagation/motionDamping"),
    ("ec.matte.rotobrush", "rotoBrushMatte/viewSearchRegion", "rotoBrushPropagation/viewSearchRegion"),
    ("ec.matte.rotobrush", "refineEdgeMatte/motionBlurSamples", "refineEdgeMatte/motionBlur/motionBlurSamples"),
    ("ec.matte.rotobrush", "refineEdgeMatte/shutterAngle", "refineEdgeMatte/motionBlur/shutterAngle"),
    ("ec.matte.rotobrush", "refineEdgeMatte/higherQuality", "refineEdgeMatte/motionBlur/higherQuality"),
    ("ec.matte.rotobrush", "refineEdgeMatte/decontaminationAmount", "refineEdgeMatte/decontamination/decontaminationAmount"),
    ("ec.matte.rotobrush", "refineEdgeMatte/extendWhereSmoothed", "refineEdgeMatte/decontamination/extendWhereSmoothed"),
    ("ec.matte.rotobrush", "refineEdgeMatte/increaseDecontaminationRadius", "refineEdgeMatte/decontamination/increaseDecontaminationRadius"),
    ("ec.matte.rotobrush", "refineEdgeMatte/viewDecontaminationMap", "refineEdgeMatte/decontamination/viewDecontaminationMap"),
    // Keying: Inner/Outer Key's single additional masks became Additional Foreground /
    // Background 1 (of 10).
    ("ec.key.innerouter", "additionalForeground", "additionalForeground/foreground1"),
    ("ec.key.innerouter", "additionalBackground", "additionalBackground/background1"),
    // Generate: Radio Waves' growth speed was called Velocity; Velocity now moves the wave.
    ("ec.generate.radiowaves", "velocity", "waveMotion/expansion"),
];

/// Popup option labels renamed since they were first saved:
/// `(effect id, param id, old label, new label)`.
pub const POPUP_LABEL_ALIASES: &[(&str, &str, &str, &str)] = &[
    // Blur & Sharpen, Channel.
    ("ec.blur.smart", "mode", "Edge Overlay", "Overlay Edge"),
    ("ec.channel.minimax", "direction", "Horizontal Only", "Just Horizontal"),
    ("ec.channel.minimax", "direction", "Vertical Only", "Just Vertical"),
    ("ec.channel.minimax", "operation", "Minimum then Maximum", "Minimum Then Maximum"),
    ("ec.channel.minimax", "operation", "Maximum then Minimum", "Maximum Then Minimum"),
    ("ec.channel.combiner", "to", "Lightness", "Lightness Only"),
    ("ec.channel.combiner", "to", "Hue", "Hue Only"),
    ("ec.channel.combiner", "to", "Saturation", "Saturation Only"),
    // Distort.
    ("ec.distort.turbulentdisplace", "pinning", "Pin All Edges", "Pin All"),
    ("ec.distort.turbulentdisplace", "pinning", "Pin Horizontal Edges", "Pin Horizontal"),
    ("ec.distort.turbulentdisplace", "pinning", "Pin Vertical Edges", "Pin Vertical"),
    ("ec.distort.reshape", "elasticity", "Normal", "Absolutely Normal"),
    ("ec.distort.reshape", "elasticity", "Above Normal", "Above Average"),
    ("ec.distort.smear", "elasticity", "Normal", "Absolutely Normal"),
    ("ec.distort.smear", "elasticity", "Above Normal", "Above Average"),
    ("ec.distort.liquify", "tool", "warp", "Warp"),
    ("ec.distort.liquify", "tool", "turbulence", "Turbulence"),
    ("ec.distort.liquify", "tool", "twirlClockwise", "Twirl Clockwise"),
    ("ec.distort.liquify", "tool", "twirlCounterclockwise", "Twirl Counterclockwise"),
    ("ec.distort.liquify", "tool", "pucker", "Pucker"),
    ("ec.distort.liquify", "tool", "bloat", "Bloat"),
    ("ec.distort.liquify", "tool", "shiftPixels", "Shift Pixels"),
    ("ec.distort.liquify", "tool", "reflection", "Reflection"),
    ("ec.distort.liquify", "tool", "clone", "Clone"),
    ("ec.distort.liquify", "tool", "reconstruction", "Reconstruction"),
    ("ec.distort.liquify", "tool", "freeze", "Freeze"),
    ("ec.distort.liquify", "tool", "thaw", "Thaw"),
    // Keying, Matte, Obsolete.
    ("ec.key.colordifference", "view", "Source Only", "Source"),
    ("ec.key.colordifference", "view", "Matte Only", "Corrected Matte"),
    // Simulation.
    ("ec.sim.carddance", "rowsColumns", "Columns Follow Rows", "Columns Follows Rows"),
    ("ec.sim.particleplayground", "persistentPropertyMapper/mapRedTo", "X Velocity", "X Speed"),
    ("ec.sim.particleplayground", "persistentPropertyMapper/mapRedTo", "Y Velocity", "Y Speed"),
    ("ec.sim.particleplayground", "persistentPropertyMapper/mapGreenTo", "X Velocity", "X Speed"),
    ("ec.sim.particleplayground", "persistentPropertyMapper/mapGreenTo", "Y Velocity", "Y Speed"),
    ("ec.sim.particleplayground", "persistentPropertyMapper/mapBlueTo", "X Velocity", "X Speed"),
    ("ec.sim.particleplayground", "persistentPropertyMapper/mapBlueTo", "Y Velocity", "Y Speed"),
    // Generate.
    ("ec.generate.fractal", "setChoice", "Mandelbrot Over Inverse", "Mandelbrot Over Julia"),
    ("ec.generate.fractal", "setChoice", "Mandelbrot Inverse Over Mandelbrot", "Mandelbrot Inverse Over Julia"),
    ("ec.generate.fractal", "fractalColor/palette", "Lightness Bands", "Lightness Gradient"),
    ("ec.generate.fractal", "fractalColor/palette", "Hue Bands", "Hue Wheel"),
    ("ec.generate.fractal", "fractalColor/palette", "Hue/Lightness Bands", "Hue Wheel"),
    ("ec.generate.fractal", "highQualitySettings/samplingMethod", "Edge Detect", "Edge Detect-Fast-May Miss Pixels"),
    ("ec.generate.fractal", "highQualitySettings/samplingMethod", "Every Pixel", "Brute Force-Slow-Every Pixel"),
    ("ec.generate.cellpattern", "cellPattern", "Mixed Crystals", "Mixed Crystals HQ"),
    ("ec.generate.cellpattern", "cellPattern", "Dots", "Bubbles"),
    // Noise & Grain, Time.
    ("ec.noise.matchgrain", "application/blendingMode", "Normal", "Add"),
    ("ec.time.timedifference", "alphaChannel", "Lightness of Result", "Lightness Of Result"),
    ("ec.time.timedifference", "alphaChannel", "Max of Result", "Max Of Result"),
    // Immersive Video: VR Converter's layouts.
    ("ec.vr.converter", "sourceProjection", "Equirectangular", "Equirectangular 2:1"),
    ("ec.vr.converter", "sourceProjection", "Cube-map 3:2", "Cube-map Pano2VR 3:2"),
    ("ec.vr.converter", "sourceProjection", "Cube-map 6:1", "Cube-map GearVR 6:1"),
    ("ec.vr.converter", "sourceProjection", "Sphere Map", "Sphere-map"),
    ("ec.vr.converter", "sourceProjection", "2D Rectilinear", "2D Source"),
    ("ec.vr.converter", "targetProjection", "Equirectangular", "Equirectangular 2:1"),
    ("ec.vr.converter", "targetProjection", "Cube-map 3:2", "Cube-map Pano2VR 3:2"),
    ("ec.vr.converter", "targetProjection", "Cube-map 6:1", "Cube-map GearVR 6:1"),
    ("ec.vr.converter", "targetProjection", "Sphere Map", "Sphere-map"),
    ("ec.vr.converter", "targetProjection", "2D Rectilinear", "2D Source"),
];

/// Effect display names used by earlier versions: `(old name, effect id)`. [`crate::lookup`]
/// resolves them so scripts and saved presets naming the old effect still work.
pub const EFFECT_NAME_ALIASES: &[(&str, &str)] = &[
    // Keying, Matte, Obsolete.
    ("mocha shape", "ec.obsolete.mochashape"),
    // Immersive Video.
    ("VR Sphere To Plane", "ec.vr.spheretoplane"),
];

/// The value a parameter added since a project was saved takes in that project when it must
/// differ from the parameter's default to reproduce how the effect rendered before the parameter
/// existed (Brightness & Contrast's Use Legacy, Photo Filter's Custom filter, …).
pub fn legacy_default(effect: &str, param: &str) -> Option<Value> {
    Some(match (effect, param) {
        ("ec.color.brightnesscontrast", "useLegacy") => Value::Bool(true),
        // Distort: earlier Ripples were symmetric, Displacement Map kept the layer bounds and
        // Magnify sampled smoothly.
        ("ec.distort.ripple", "conversion") => Value::Enum(1),
        ("ec.distort.displacementmap", "expandOutput") => Value::Bool(false),
        ("ec.distort.magnify", "scaling") => Value::Enum(1),
        ("ec.color.photofilter", "filter") => Value::Enum(crate::color_fx::PHOTO_FILTER_CUSTOM),
        ("ec.color.colorbalance", "preserveLuminosity") => Value::Bool(false),
        ("ec.color.leavecolor", "matchColors") => Value::Enum(1),
        ("ec.color.changetocolor", "change") => Value::Enum(3),
        ("ec.color.changetocolor", "changeBy") => Value::Enum(1),
        ("ec.color.changetocolor", "toleranceGroup/lightness" | "toleranceGroup/saturation") => Value::Scalar(100.0),
        ("ec.color.shadowhighlight", "moreOptions/blackClip" | "moreOptions/whiteClip") => Value::Scalar(0.0),
        // Generate: the shape generators used to composite Normal over the layer, with
        // independent width and height.
        ("ec.generate.fourcolor" | "ec.generate.checkerboard" | "ec.generate.grid" | "ec.generate.circle", "blendingMode") => Value::Enum(1),
        ("ec.generate.checkerboard" | "ec.generate.grid", "sizeFrom") => Value::Enum(2),
        // Noise & Grain: Fractal / Turbulent Noise used to clip, halve each octave's weight and
        // size, and draw the noise alone; grain was added; Remove Grain filtered channels apart.
        ("ec.noise.fractal", "overflow") => Value::Enum(0),
        ("ec.noise.fractal", "subSettings/subInfluence" | "subSettings/subScaling") => Value::Scalar(50.0),
        ("ec.noise.fractal" | "ec.noise.turbulent", "blendingMode") => Value::Enum(crate::noise3::BLEND_NONE),
        ("ec.noise.turbulent", "evolutionOptions/turbulenceFactor") => Value::Scalar(0.0),
        ("ec.noise.addgrain", "application/blendingMode") => Value::Enum(2),
        ("ec.noise.removegrain", "noiseReductionSettings/mode") => Value::Enum(1),
        // Audio: the Compressor's release was always manual.
        ("ec.audio.compressor", "autoRelease") => Value::Bool(false),
        // Keying: Advanced Spill Suppressor's Ultra mode had no hue tolerance.
        ("ec.key.advancedspill", "ultraSettings/tolerance") => Value::Scalar(100.0),
        // Transition: Block Dissolve's blocks had hard edges.
        ("ec.transition.blockdissolve", "softEdges") => Value::Bool(false),
        _ => return None,
    })
}

/// Categories whose parameters users edit per instance (Dropdown Menu Control's items, slider
/// ranges): their stored names and UI hints are kept.
const USER_EDITED_CATEGORIES: &[&str] = &["Expression Controls"];

fn take_prop(g: &mut PropGroup, match_id: &str) -> Option<Property> {
    let i = g.children.iter().position(|c| matches!(c, Node::Prop(p) if p.match_id == match_id))?;
    match g.children.remove(i) {
        Node::Prop(p) => Some(p),
        Node::Group(_) => None,
    }
}

/// Remove the property at spec id `id` (`a/b/param`) from `g`.
fn take_at(g: &mut PropGroup, id: &str) -> Option<Property> {
    match id.split_once('/') {
        None => take_prop(g, id),
        Some((first, rest)) => take_at(g.sub_mut(first)?, rest),
    }
}

fn prop_at<'a>(g: &'a mut PropGroup, id: &str) -> Option<&'a mut Property> {
    match id.split_once('/') {
        None => g.get_mut(id),
        Some((first, rest)) => prop_at(g.sub_mut(first)?, rest),
    }
}

fn group_at<'a>(g: &'a mut PropGroup, ids: &mut Ids, path: &str) -> Option<&'a mut PropGroup> {
    let (first, rest) = match path.split_once('/') {
        Some((a, b)) => (a, Some(b)),
        None => (path, None),
    };
    if g.sub(first).is_none() {
        g.children.push(ids.group(first, group_name(first)).into());
    }
    let sub = g.sub_mut(first)?;
    match rest {
        Some(r) => group_at(sub, ids, r),
        None => Some(sub),
    }
}

/// Remap one stored popup value from `old` option labels to `new` ones.
fn remap_enum(v: &mut Value, old: &[String], new: &[String], fx: &str, pid: &str) {
    let Value::Enum(i) = v else { return };
    let Some(label) = old.get(*i as usize) else { return };
    let label = POPUP_LABEL_ALIASES.iter().find(|(e, p, o, _)| *e == fx && *p == pid && o == label).map(|(_, _, _, n)| *n).unwrap_or(label);
    if let Some(j) = new.iter().position(|n| n == label) {
        *i = j as u32;
    }
}

/// Refresh a stored property from its spec: name, UI hints, popup values by label.
fn refresh(pr: &mut Property, ps: &ParamSpec, spec: &EffectSpec) {
    if USER_EDITED_CATEGORIES.contains(&spec.category) {
        return;
    }
    pr.name = ps.name.to_string();
    match (&pr.ui, &ps.ui) {
        (ParamUi::Popup { options: old }, ParamUi::Popup { options: new }) => {
            if old != new {
                let old = old.clone();
                remap_enum(&mut pr.value, &old, new, spec.id, ps.id);
                for k in &mut pr.keys {
                    remap_enum(&mut k.value, &old, new, spec.id, ps.id);
                }
            }
            pr.ui = ps.ui.clone();
        }
        // A parameter that became internal (superseded by a new one) is hidden.
        (_, ParamUi::Hidden) => pr.ui = ParamUi::Hidden,
        // A path typed as text, or set only by commands, that got a Choose… button (Apply Color
        // LUT's LUT, the OCIO files, #514).
        (ParamUi::Hidden | ParamUi::Text, ParamUi::File { .. }) => pr.ui = ps.ui.clone(),
        // An internal point that became a control (Liquify's Distortion Mesh Offset, #1194).
        (ParamUi::Hidden, ParamUi::Point) => pr.ui = ParamUi::Point,
        // Same kind of value: take the spec's hints (units, ranges).
        (a, b) if std::mem::discriminant(a) == std::mem::discriminant(b) || is_numeric_ui(a) && is_numeric_ui(b) => pr.ui = ps.ui.clone(),
        _ => {}
    }
}

fn is_numeric_ui(u: &ParamUi) -> bool {
    matches!(u, ParamUi::Number | ParamUi::Slider { .. } | ParamUi::Percent | ParamUi::Angle | ParamUi::Pixels)
}

/// Bring the saved instance `g` of `spec` up to date (see the module docs). `layer_size` sizes
/// point defaults of added parameters. Returns whether anything changed.
pub fn upgrade_instance(spec: &EffectSpec, g: &mut PropGroup, ids: &mut Ids, layer_size: [f64; 2]) -> bool {
    let before = g.clone();
    // Card Wipe's Back Layer was a None / Self popup; it is a layer parameter now, with "Self"
    // kept in the hidden Back Layer Is Self switch.
    let mut back_self = None;
    if spec.id == "ec.transition.cardwipe"
        && let Some(pr) = g.get_mut("backLayer")
        && matches!(pr.ui, ParamUi::Popup { .. })
    {
        back_self = Some(pr.value.as_enum() == 1);
        pr.value = Value::Layer(None);
        pr.keys.clear();
        pr.ui = ParamUi::Layer;
    }
    for ps in &spec.params {
        // Locate the stored property: at its spec path, under an old id, or at top level
        // before it moved into a twirl-down group.
        let found = prop_at(g, ps.id).is_some();
        if !found {
            let moved = PARAM_ID_ALIASES.iter().filter(|(e, _, new)| *e == spec.id && *new == ps.id).find_map(|(_, old, _)| take_at(g, old)).or_else(|| {
                let leaf = ps.id.rsplit('/').next().unwrap_or(ps.id);
                // Not when the top-level leaf is an old id of another parameter (Radio
                // Waves' old `velocity` is the new `waveMotion/expansion`).
                let aliased = PARAM_ID_ALIASES.iter().any(|(e, old, _)| *e == spec.id && *old == leaf);
                (leaf != ps.id && !aliased && !spec.params.iter().any(|q| q.id == leaf)).then(|| take_prop(g, leaf)).flatten()
            });
            let (path, leaf) = ps.id.rsplit_once('/').map(|(p, l)| (Some(p), l)).unwrap_or((None, ps.id));
            let pr = match moved {
                Some(mut pr) => {
                    pr.match_id = leaf.to_string();
                    pr
                }
                None => {
                    let v = legacy_default(spec.id, ps.id).unwrap_or_else(|| default_value(ps, layer_size));
                    let mut pr = Property::new(ids.alloc(), leaf, ps.name, v).with_ui(ps.ui.clone());
                    pr.spatial = matches!(ps.ui, ParamUi::Point);
                    pr.hold_only |= matches!(ps.ui, ParamUi::Checkbox | ParamUi::Popup { .. } | ParamUi::Layer);
                    pr
                }
            };
            match path.and_then(|path| group_at(g, ids, path)) {
                Some(sub) => sub.children.push(pr.into()),
                None => g.children.push(pr.into()),
            }
        }
        if let Some(pr) = prop_at(g, ps.id) {
            refresh(pr, ps, spec);
        }
        // Layer parameters' source companions.
        let sid = layer_source_id(ps.id);
        if matches!(ps.ui, ParamUi::Layer) && !spec.params.iter().any(|q| q.id == sid) && prop_at(g, &sid).is_none() {
            let (path, leaf) = ps.id.rsplit_once('/').map(|(p, l)| (Some(p), l)).unwrap_or((None, ps.id));
            // Companions of grouped layer parameters were once stored at top level.
            let src = take_prop(g, &sid).map(|mut p| {
                p.match_id = layer_source_id(leaf);
                p
            });
            let src = src.unwrap_or_else(|| {
                let mut src = Property::new(ids.alloc(), &layer_source_id(leaf), &format!("{} Source", ps.name), Value::Enum(2)).with_ui(ParamUi::Hidden);
                src.hold_only = true;
                src
            });
            match path.and_then(|path| group_at(g, ids, path)) {
                Some(sub) => sub.children.push(src.into()),
                None => g.children.push(src.into()),
            }
        }
    }
    if let Some(on) = back_self
        && let Some(pr) = g.get_mut("backSelf")
    {
        pr.value = Value::Bool(on);
    }
    // Refresh twirl-down group names.
    fn names(g: &mut PropGroup) {
        for c in &mut g.children {
            if let Node::Group(sg) = c {
                let n = group_name(&sg.match_id);
                if n != sg.match_id {
                    sg.name = n.to_string();
                }
                names(sg);
            }
        }
    }
    if !USER_EDITED_CATEGORIES.contains(&spec.category) && spec.params.iter().any(|p| p.id.contains('/')) {
        names(g);
    }
    *g != before
}

/// Bring an effect instance saved before project schema 2 into effect space (#227).
///
/// Layers without a source rectangle (shape, text) have comp-sized effect bounds centred on
/// their origin. Effects used to measure positions from the origin, so the default "layer
/// centre" sat at the comp's bottom-right corner; they now measure from the bounds' top-left,
/// as After Effects does. `d` is the bounds' half size: positions the instance holds move by it
/// so they keep their place in the comp (point parameters with their keyframes, Paint stroke
/// paths and clone positions, Puppet mesh seeds and pin rest points, Liquify and Roto Brush
/// strokes). A point parameter still at its default (`layer_size`) and not animated stays: it
/// meant the layer centre and now is. Expression controls keep their values (what they mean is
/// up to the expressions reading them), and expressions are not rewritten.
///
/// `clone_d(source)`: the shift of a Paint clone stroke's source layer (`None` = this layer),
/// whose effect space its Clone Position is in.
pub fn to_effect_space(spec: &EffectSpec, g: &mut PropGroup, layer_size: [f64; 2], d: [f64; 2], clone_d: &dyn Fn(Option<u64>) -> [f64; 2]) {
    if spec.id.starts_with("ec.control.") {
        return;
    }
    let defaults: Vec<u64> = spec
        .params
        .iter()
        .filter(|ps| matches!(ps.ui, ParamUi::Point | ParamUi::Point3))
        .filter_map(|ps| g.prop(ps.id).filter(|pr| pr.keys.is_empty() && same_point(&pr.value, &default_value(ps, layer_size))).map(|pr| pr.uid))
        .collect();
    let at = Shift { d, clone_d, defaults: &defaults, effect: spec.id };
    at.group(g);
}

/// What [`to_effect_space`] moves, and by how much.
struct Shift<'a> {
    d: [f64; 2],
    clone_d: &'a dyn Fn(Option<u64>) -> [f64; 2],
    /// Point parameters left at their defaults.
    defaults: &'a [u64],
    effect: &'a str,
}

impl Shift<'_> {
    fn group(&self, g: &mut PropGroup) {
        // A Paint clone stroke's Clone Position is in its source layer's effect space.
        let clone_d = g.get("clone_source").map(|s| (self.clone_d)(s.value.as_layer())).unwrap_or(self.d);
        for n in &mut g.children {
            match n {
                Node::Group(sg) => self.group(sg),
                Node::Prop(pr) => {
                    let d = if pr.match_id == "clone_position" { clone_d } else { self.d };
                    let kind = match (&pr.ui, pr.match_id.as_str()) {
                        (ParamUi::Point | ParamUi::Point3, _) if !self.defaults.contains(&pr.uid) => Some(Data::Point),
                        (ParamUi::Path, _) => Some(Data::Point),
                        (ParamUi::Hidden, "seed" | "rest") if self.effect == crate::puppet::ID => Some(Data::Point),
                        (ParamUi::Hidden, "distortionMesh") if self.effect == "ec.distort.liquify" => Some(Data::Liquify),
                        (ParamUi::Hidden, crate::roto::STROKES) if self.effect == crate::roto::ID => Some(Data::Roto),
                        _ => None,
                    };
                    if let Some(kind) = kind.filter(|_| d != [0.0; 2]) {
                        shift_value(&mut pr.value, kind, d);
                        for k in &mut pr.keys {
                            shift_value(&mut k.value, kind, d);
                        }
                    }
                }
            }
        }
    }
}

/// How a value holds positions.
#[derive(Clone, Copy)]
enum Data {
    /// A point (2D or 3D: x and y move) or a path (its vertices; tangents are relative).
    Point,
    /// Liquify's Distortion Mesh text (`distort4::LiquifyStroke` lines).
    Liquify,
    /// Roto Brush's Strokes JSON ([`effectcraft_track::roto::RotoData`]).
    Roto,
}

fn shift_value(v: &mut Value, kind: Data, d: [f64; 2]) {
    let add = |p: &mut [f64; 2]| *p = [p[0] + d[0], p[1] + d[1]];
    match (kind, v) {
        (Data::Point, Value::Vec2(p)) => add(p),
        (Data::Point, Value::Vec3(p)) => *p = [p[0] + d[0], p[1] + d[1], p[2]],
        (Data::Point, Value::Path(sp)) => sp.vertices.iter_mut().for_each(add),
        (Data::Liquify, Value::Str(s)) => {
            // Lines that don't read as strokes are kept as they are.
            let lines: Vec<String> = s
                .lines()
                .map(|l| match crate::distort4::LiquifyStroke::parse(l) {
                    Some(mut st) => {
                        st.points.iter_mut().for_each(add);
                        st.to_line()
                    }
                    None => l.to_string(),
                })
                .collect();
            *s = lines.join("\n");
        }
        (Data::Roto, Value::Str(s)) => {
            // Unreadable data is kept as it is.
            if let Ok(mut r) = serde_json::from_str::<effectcraft_track::roto::RotoData>(s) {
                r.strokes.iter_mut().flat_map(|st| st.points.iter_mut()).for_each(add);
                *s = r.to_json();
            }
        }
        _ => {}
    }
}

/// Two point values equal up to float noise.
fn same_point(a: &Value, b: &Value) -> bool {
    let (a, b) = (a.components(), b.components());
    a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-6)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{find, instantiate};

    #[test]
    fn upgrade_is_a_no_op_on_fresh_instances() {
        let mut next = 1;
        for spec in crate::registry() {
            let mut g = instantiate(spec, &mut Ids(&mut next), spec.name, [100.0, 50.0]);
            assert!(!upgrade_instance(spec, &mut g, &mut Ids(&mut next), [100.0, 50.0]), "{} changed on upgrade", spec.id);
        }
    }

    #[test]
    fn upgrade_adds_missing_params_refreshes_names_and_remaps_popups() {
        let spec = find("ec.blur.gaussian").unwrap();
        let mut next = 1;
        let mut g = instantiate(spec, &mut Ids(&mut next), "Gaussian Blur", [100.0, 50.0]);
        // An old save: a missing parameter, a stale name, a popup stored with options in
        // another order.
        let removed = spec.params.last().unwrap().id;
        g.children.retain(|c| c.match_id() != removed);
        let popup = spec.params.iter().find(|p| matches!(p.ui, ParamUi::Popup { .. })).expect("Gaussian Blur has a popup");
        let ParamUi::Popup { options } = &popup.ui else { panic!("not a popup") };
        {
            let pr = g.get_mut(popup.id).unwrap();
            pr.name = "Old Name".into();
            let mut rev = options.clone();
            rev.reverse();
            pr.ui = ParamUi::Popup { options: rev };
            pr.value = Value::Enum(0); // = the last option in the current order
        }
        assert!(upgrade_instance(spec, &mut g, &mut Ids(&mut next), [100.0, 50.0]));
        assert!(g.get(removed).is_some());
        let pr = g.get(popup.id).unwrap();
        assert_eq!(pr.name, popup.name);
        assert_eq!(pr.value, Value::Enum(options.len() as u32 - 1));
        assert_eq!(pr.ui, popup.ui);
    }

    /// #514: Apply Color LUT's LUT was hidden (set only by commands) and the OCIO files were
    /// plain text; old projects get the Choose… file parameter, keeping the path.
    #[test]
    fn hidden_and_text_paths_become_file_parameters() {
        for (fx, pid, old) in [("ec.utility.applylut", "lut", ParamUi::Hidden), ("ec.color.ociofile", "file", ParamUi::Text)] {
            let mut next = 1;
            let (spec, mut g) = instance(fx, &mut next);
            let pr = g.get_mut(pid).unwrap();
            pr.ui = old;
            pr.value = Value::Str("/luts/look.cube".into());
            assert!(upgrade_instance(spec, &mut g, &mut Ids(&mut next), [100.0, 100.0]), "{fx}");
            let pr = g.get(pid).unwrap();
            assert!(matches!(&pr.ui, ParamUi::File { filters } if filters.iter().any(|f| f == "cube")), "{fx}: {:?}", pr.ui);
            assert_eq!(pr.value, Value::Str("/luts/look.cube".into()));
        }
    }

    /// #1194: Liquify's Distortion Mesh Offset was hidden; old projects show it, keeping its value.
    #[test]
    fn a_hidden_point_that_became_a_control_is_shown() {
        let mut next = 1;
        let (spec, mut g) = instance("ec.distort.liquify", &mut next);
        let pr = g.get_mut("distortionMeshOffset").unwrap();
        pr.ui = ParamUi::Hidden;
        pr.value = Value::Vec2([20.0, -5.0]);
        assert!(upgrade_instance(spec, &mut g, &mut Ids(&mut next), [100.0, 100.0]));
        let pr = g.get("distortionMeshOffset").unwrap();
        assert_eq!((&pr.ui, &pr.value), (&ParamUi::Point, &Value::Vec2([20.0, -5.0])));
    }

    #[test]
    fn upgrade_moves_params_into_twirl_down_groups() {
        let spec = find(crate::warp_stab::ID).unwrap();
        let mut next = 1;
        let mut g = instantiate(spec, &mut Ids(&mut next), "Warp Stabilizer", [100.0, 50.0]);
        // Pretend Smoothness was saved at top level with a custom value.
        let mut pr = take_at(&mut g, "stabilization/smoothness").unwrap();
        pr.value = Value::Scalar(12.0);
        g.children.push(pr.into());
        assert!(upgrade_instance(spec, &mut g, &mut Ids(&mut next), [100.0, 50.0]));
        assert!(g.get("smoothness").is_none());
        assert_eq!(prop_at(&mut g, "stabilization/smoothness").unwrap().value, Value::Scalar(12.0));
    }

    #[test]
    fn card_wipe_back_layer_popup_becomes_a_layer_parameter() {
        let spec = find("ec.transition.cardwipe").unwrap();
        let mut next = 1;
        for (old, self_) in [(1, true), (0, false)] {
            let mut g = instantiate(spec, &mut Ids(&mut next), "Card Wipe", [100.0, 50.0]);
            take_prop(&mut g, "backSelf");
            let pr = g.get_mut("backLayer").unwrap();
            pr.ui = ParamUi::Popup { options: vec!["None".into(), "Self".into()] };
            pr.value = Value::Enum(old);
            assert!(upgrade_instance(spec, &mut g, &mut Ids(&mut next), [100.0, 50.0]));
            assert_eq!(g.get("backLayer").unwrap().value, Value::Layer(None));
            assert_eq!(g.get("backLayer").unwrap().ui, ParamUi::Layer);
            assert_eq!(g.get("backSelf").unwrap().value, Value::Bool(self_));
        }
    }

    #[test]
    fn particle_playground_mapper_targets_remap_by_label() {
        let spec = find("ec.sim.particleplayground").unwrap();
        let mut next = 1;
        let mut g = instantiate(spec, &mut Ids(&mut next), "Particle Playground", [100.0, 50.0]);
        let old: Vec<String> =
            ["None", "Red", "Green", "Blue", "Kinetic Friction", "Scale", "X", "Y", "X Speed", "Y Speed", "X Force", "Y Force", "Opacity", "Mass"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        let pr = prop_at(&mut g, "persistentPropertyMapper/mapRedTo").unwrap();
        pr.ui = ParamUi::Popup { options: old };
        pr.value = Value::Enum(8);
        assert!(upgrade_instance(spec, &mut g, &mut Ids(&mut next), [100.0, 50.0]));
        assert_eq!(prop_at(&mut g, "persistentPropertyMapper/mapRedTo").unwrap().value, Value::Enum(15), "X Speed");
    }

    #[test]
    fn selective_color_ranges_move_into_details() {
        let spec = find("ec.color.selectivecolor").unwrap();
        let mut next = 1;
        let mut g = instantiate(spec, &mut Ids(&mut next), "Selective Color", [100.0, 50.0]);
        let mut pr = take_at(&mut g, "details/reds/redsCyan").unwrap();
        pr.value = Value::Scalar(40.0);
        g.children.push(pr.into());
        assert!(upgrade_instance(spec, &mut g, &mut Ids(&mut next), [100.0, 50.0]));
        assert!(g.get("redsCyan").is_none());
        assert_eq!(prop_at(&mut g, "details/reds/redsCyan").unwrap().value, Value::Scalar(40.0));
        assert_eq!(g.sub("details").unwrap().sub("reds").unwrap().name, "Reds");
    }

    #[test]
    fn inner_outer_key_additional_masks_move_into_their_groups() {
        let spec = find("ec.key.innerouter").unwrap();
        let mut next = 1;
        let mut g = instantiate(spec, &mut Ids(&mut next), "Inner/Outer Key", [100.0, 50.0]);
        let mut pr = take_at(&mut g, "additionalForeground/foreground1").unwrap();
        pr.match_id = "additionalForeground".into();
        pr.value = Value::Enum(3);
        g.children.push(pr.into());
        assert!(upgrade_instance(spec, &mut g, &mut Ids(&mut next), [100.0, 50.0]));
        assert!(g.get("additionalForeground").is_none());
        assert_eq!(prop_at(&mut g, "additionalForeground/foreground1").unwrap().value, Value::Enum(3));
    }

    #[test]
    fn match_grain_viewing_modes_remap_by_label() {
        let spec = find("ec.noise.matchgrain").unwrap();
        let mut next = 1;
        let mut g = instantiate(spec, &mut Ids(&mut next), "Match Grain", [100.0, 50.0]);
        let pr = g.get_mut("viewingMode").unwrap();
        pr.ui = ParamUi::Popup { options: vec!["Final Output".into(), "Noise Samples".into(), "Blending Matte".into()] };
        pr.value = Value::Enum(0);
        assert!(upgrade_instance(spec, &mut g, &mut Ids(&mut next), [100.0, 50.0]));
        assert_eq!(g.get("viewingMode").unwrap().value, Value::Enum(4), "Final Output");
    }

    /// The shift a 200 × 100 shape layer's effect positions get (#227): half its bounds.
    const D: [f64; 2] = [100.0, 50.0];
    const SIZE: [f64; 2] = [200.0, 100.0];

    fn instance(id: &str, next: &mut u64) -> (&'static EffectSpec, PropGroup) {
        let spec = find(id).unwrap();
        (spec, instantiate(spec, &mut Ids(next), spec.name, SIZE))
    }

    fn own(_: Option<u64>) -> [f64; 2] {
        D
    }

    #[test]
    fn effect_points_move_into_effect_space_unless_left_at_their_default() {
        let mut next = 1;
        // Twirl: a set centre (with keyframes) moves; Bulge's default centre stays.
        let (spec, mut twirl) = instance("ec.distort.twirl", &mut next);
        let c = twirl.get_mut("center").unwrap();
        c.value = Value::Vec2([-30.0, -10.0]);
        let key = |t: i64, v: Value| {
            let mut k = effectcraft_keyframe::Keyframe::new(Default::default(), v);
            k.time.0 = t;
            k
        };
        c.keys = vec![key(0, Value::Vec2([0.0, 0.0])), key(1000, Value::Vec2([5.0, -5.0]))];
        to_effect_space(spec, &mut twirl, SIZE, D, &own);
        let c = twirl.get("center").unwrap();
        assert_eq!(c.value, Value::Vec2([70.0, 40.0]));
        assert_eq!(c.keys.iter().map(|k| k.value.clone()).collect::<Vec<_>>(), vec![Value::Vec2([100.0, 50.0]), Value::Vec2([105.0, 45.0])]);
        let (spec, mut bulge) = instance("ec.distort.bulge", &mut next);
        let before = bulge.clone();
        to_effect_space(spec, &mut bulge, SIZE, D, &own);
        assert_eq!(bulge, before, "the default centre now means the layer centre");
        // Expression controls keep their values.
        let (spec, mut ctl) = instance("ec.control.point", &mut next);
        let pr = ctl.props().find(|p| p.ui == ParamUi::Point).unwrap().uid;
        ctl.find_mut(pr).unwrap().value = Value::Vec2([5.0, 5.0]);
        to_effect_space(spec, &mut ctl, SIZE, D, &own);
        assert_eq!(ctl.find(pr).unwrap().value, Value::Vec2([5.0, 5.0]));
    }

    #[test]
    fn puppet_paint_liquify_and_roto_data_move_into_effect_space() {
        let mut next = 1;
        // Puppet: mesh seed, pin rest and Position.
        let (spec, mut puppet) = instance(crate::puppet::ID, &mut next);
        let mut mesh = crate::puppet::mesh_group(&mut Ids(&mut next), "Mesh 1", [1.0, 2.0], &crate::puppet::MeshOpts::default());
        let pin = crate::puppet::pin_group(&mut Ids(&mut next), "Puppet Pin 1", crate::puppet::PinKind::Position, [3.0, 4.0]);
        mesh.sub_mut("deform").unwrap().children.push(pin.into());
        puppet.children.push(mesh.into());
        to_effect_space(spec, &mut puppet, SIZE, D, &own);
        let mesh = puppet.sub("mesh").unwrap();
        assert_eq!(mesh.get("seed").unwrap().value, Value::Vec2([101.0, 52.0]));
        let pin = mesh.sub("deform").unwrap().sub("pin").unwrap();
        assert_eq!(pin.get("rest").unwrap().value, Value::Vec2([103.0, 54.0]));
        assert_eq!(pin.get("position").unwrap().value, Value::Vec2([103.0, 54.0]));

        // Paint: the stroke path, its transform, and a clone position in its source layer's space.
        let (spec, mut paint) = instance(crate::paint::ID, &mut next);
        let s = crate::paint::StrokeSpec {
            kind: crate::paint::StrokeKind::Clone,
            points: vec![[0.0, 0.0], [10.0, 0.0]],
            clone_source: Some(7),
            clone_position: [20.0, 20.0],
            ..Default::default()
        };
        paint.children.push(crate::paint::stroke_group(&mut Ids(&mut next), "Clone 1", &s).into());
        to_effect_space(spec, &mut paint, SIZE, D, &|src| if src == Some(7) { [0.0; 2] } else { D });
        let st = paint.sub("clone").unwrap();
        let Value::Path(path) = &st.get("path").unwrap().value else { panic!("no path") };
        assert_eq!(path.vertices, vec![[100.0, 50.0], [110.0, 50.0]]);
        assert_eq!(st.sub("transform").unwrap().get("position").unwrap().value, Value::Vec2([100.0, 50.0]));
        assert_eq!(st.sub("stroke_options").unwrap().get("clone_position").unwrap().value, Value::Vec2([20.0, 20.0]), "a footage source");

        // Liquify: stroke points move, the clone offset (relative) doesn't; unknown lines stay.
        let (spec, mut liquify) = instance("ec.distort.liquify", &mut next);
        let line =
            crate::distort4::LiquifyStroke { tool: 0, size: 64.0, pressure: 50.0, jitter: 0.0, clone_offset: [3.0, 3.0], points: vec![[1.0, 1.0]] }.to_line();
        liquify.get_mut("distortionMesh").unwrap().value = Value::Str(format!(
            "{line}
not a stroke"
        ));
        to_effect_space(spec, &mut liquify, SIZE, D, &own);
        let Value::Str(text) = &liquify.get("distortionMesh").unwrap().value else { panic!("no mesh") };
        let moved = crate::distort4::LiquifyStroke { points: vec![[101.0, 51.0]], ..crate::distort4::LiquifyStroke::parse(&line).unwrap() };
        assert_eq!(
            text,
            &format!(
                "{}
not a stroke",
                moved.to_line()
            )
        );

        // Roto Brush strokes.
        let (spec, mut roto) = instance(crate::roto::ID, &mut next);
        let data = effectcraft_track::roto::RotoData {
            base: Some(0),
            span: [0, 10],
            strokes: vec![effectcraft_track::roto::Stroke { kind: effectcraft_track::roto::StrokeKind::Fg, frame: 0, radius: 5.0, points: vec![[2.0, 2.0]] }],
        };
        roto.get_mut(crate::roto::STROKES).unwrap().value = Value::Str(data.to_json());
        to_effect_space(spec, &mut roto, SIZE, D, &own);
        let Value::Str(text) = &roto.get(crate::roto::STROKES).unwrap().value else { panic!("no strokes") };
        assert_eq!(effectcraft_track::roto::RotoData::from_json(text).strokes[0].points, vec![[102.0, 52.0]]);
    }

    #[test]
    fn layers_with_a_source_rectangle_keep_their_effect_positions() {
        let mut next = 1;
        let (spec, mut twirl) = instance("ec.distort.twirl", &mut next);
        twirl.get_mut("center").unwrap().value = Value::Vec2([3.0, 4.0]);
        let before = twirl.clone();
        to_effect_space(spec, &mut twirl, SIZE, [0.0; 2], &|_| [0.0; 2]);
        assert_eq!(twirl, before);
    }
}
