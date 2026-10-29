//! After Effects-compatible match names for our property tree and effects, so common scripts
//! (`layer.property("ADBE Transform Group").property("ADBE Position")`,
//! `layer.Effects.addProperty("ADBE Gaussian Blur 2")`) work unchanged.
//!
//! The names are the stable identifiers After Effects documents for scripting (Scripting Guide
//! ▸ match names). Properties without a documented equivalent keep our own match id
//! (`perChar3d`, `ec.blur.bilateral`), which `property()` and `addProperty()` accept too.

use effectcraft_project::{LayerSource, Node};

/// Match name of a layer (its root property group).
pub fn layer(source: &LayerSource) -> &'static str {
    match source {
        LayerSource::Text => "ADBE Text Layer",
        LayerSource::Shape => "ADBE Vector Layer",
        LayerSource::Camera => "ADBE Camera Layer",
        LayerSource::Light { .. } => "ADBE Light Layer",
        _ => "ADBE AV Layer",
    }
}

/// Top-level groups of a layer.
const LAYER_GROUPS: &[(&str, &str)] = &[
    ("transform", "ADBE Transform Group"),
    ("masks", "ADBE Mask Parade"),
    ("effects", "ADBE Effect Parade"),
    ("text", "ADBE Text Properties"),
    ("contents", "ADBE Root Vectors Group"),
    ("materialOptions", "ADBE Material Options Group"),
    ("cameraOptions", "ADBE Camera Options Group"),
    ("lightOptions", "ADBE Light Options Group"),
    ("audio", "ADBE Audio Group"),
    ("layerStyles", "ADBE Layer Styles"),
    ("motionTrackers", "ADBE MTrackers"),
    ("timeRemap", "ADBE Time Remapping"),
];

const TRANSFORM: &[(&str, &str)] = &[
    ("anchor", "ADBE Anchor Point"),
    // Cameras and lights store the Point of Interest where AVLayers keep the Anchor Point.
    ("poi", "ADBE Anchor Point"),
    ("position", "ADBE Position"),
    ("positionX", "ADBE Position_0"),
    ("positionY", "ADBE Position_1"),
    ("positionZ", "ADBE Position_2"),
    ("scale", "ADBE Scale"),
    ("orientation", "ADBE Orientation"),
    ("rotationX", "ADBE Rotate X"),
    ("rotationY", "ADBE Rotate Y"),
    ("rotation", "ADBE Rotate Z"),
    ("opacity", "ADBE Opacity"),
];

const MASK: &[(&str, &str)] =
    &[("path", "ADBE Mask Shape"), ("feather", "ADBE Mask Feather"), ("opacity", "ADBE Mask Opacity"), ("expansion", "ADBE Mask Offset")];

const TEXT: &[(&str, &str)] = &[
    ("sourceText", "ADBE Text Document"),
    ("pathOptions", "ADBE Text Path Options"),
    ("moreOptions", "ADBE Text More Options"),
    ("animators", "ADBE Text Animators"),
];

// Match names of the six properties below Text ▸ Path Options, as named by
// After Effects' scripting API (rather than EffectCraft's internal ids).
const TEXT_PATH_OPTIONS: &[(&str, &str)] = &[
    ("path", "ADBE Text Path"),
    ("reversePath", "ADBE Text Reverse Path"),
    ("perpendicular", "ADBE Text Perpendicular To Path"),
    ("forceAlignment", "ADBE Text Force Align Path"),
    ("firstMargin", "ADBE Text First Margin"),
    ("lastMargin", "ADBE Text Last Margin"),
];

// A stroke's Dashes, as After Effects' scripting API names them (#815).
const STROKE_DASHES: &[(&str, &str)] = &[
    ("dash", "ADBE Vector Stroke Dash 1"),
    ("gap", "ADBE Vector Stroke Gap 1"),
    ("dash2", "ADBE Vector Stroke Dash 2"),
    ("gap2", "ADBE Vector Stroke Gap 2"),
    ("dash3", "ADBE Vector Stroke Dash 3"),
    ("gap3", "ADBE Vector Stroke Gap 3"),
    ("offset", "ADBE Vector Stroke Offset"),
];

/// The Dashes child id (`dash`, `gap2`, `offset`…) for an After Effects match name.
pub fn stroke_dash(name: &str) -> Option<&'static str> {
    STROKE_DASHES.iter().find(|(_, m)| *m == name).map(|(id, _)| *id)
}

const TEXT_ANIMATOR: &[(&str, &str)] = &[("selectors", "ADBE Text Selectors"), ("properties", "ADBE Text Animator Properties")];

const TEXT_SELECTORS: &[(&str, &str)] =
    &[("rangeSelector", "ADBE Text Selector"), ("wigglySelector", "ADBE Text Wiggly Selector"), ("expressionSelector", "ADBE Text Expressible Selector")];

const TEXT_RANGE_SELECTOR: &[(&str, &str)] = &[("start", "ADBE Text Percent Start"), ("end", "ADBE Text Percent End"), ("offset", "ADBE Text Percent Offset")];

const CAMERA: &[(&str, &str)] = &[
    ("zoom", "ADBE Camera Zoom"),
    ("dof", "ADBE Camera Depth of Field"),
    ("focusDistance", "ADBE Camera Focus Distance"),
    ("aperture", "ADBE Camera Aperture"),
    ("blurLevel", "ADBE Camera Blur Level"),
];

const LIGHT: &[(&str, &str)] = &[
    ("intensity", "ADBE Light Intensity"),
    ("color", "ADBE Light Color"),
    ("coneAngle", "ADBE Light Cone Angle"),
    ("coneFeather", "ADBE Light Cone Feather 2"),
    ("falloff", "ADBE Light Falloff Type"),
    ("radius", "ADBE Light Falloff Start"),
    ("falloffDistance", "ADBE Light Falloff Distance"),
    ("castsShadows", "ADBE Light Casts Shadows"),
    ("shadowDarkness", "ADBE Light Shadow Darkness"),
    ("shadowDiffusion", "ADBE Light Shadow Diffusion"),
];

const MATERIAL: &[(&str, &str)] = &[
    ("castsShadows", "ADBE Casts Shadows"),
    ("lightTransmission", "ADBE Light Transmission"),
    ("acceptsShadows", "ADBE Accepts Shadows"),
    ("acceptsLights", "ADBE Accepts Lights"),
    ("ambient", "ADBE Ambient Coefficient"),
    ("diffuse", "ADBE Diffuse Coefficient"),
    ("specularIntensity", "ADBE Specular Coefficient"),
    ("specularShininess", "ADBE Shininess Coefficient"),
    ("metal", "ADBE Metal Coefficient"),
];

/// Shape items (Contents ▸ Add): our match id → AE match name.
pub const SHAPE_ITEMS: &[(&str, &str)] = &[
    ("group", "ADBE Vector Group"),
    ("rect", "ADBE Vector Shape - Rect"),
    ("ellipse", "ADBE Vector Shape - Ellipse"),
    ("star", "ADBE Vector Shape - Star"),
    ("path", "ADBE Vector Shape - Group"),
    ("fill", "ADBE Vector Graphic - Fill"),
    ("stroke", "ADBE Vector Graphic - Stroke"),
    ("gfill", "ADBE Vector Graphic - G-Fill"),
    ("gstroke", "ADBE Vector Graphic - G-Stroke"),
    ("merge", "ADBE Vector Filter - Merge"),
    ("offset", "ADBE Vector Filter - Offset"),
    ("pucker", "ADBE Vector Filter - PB"),
    ("repeater", "ADBE Vector Filter - Repeater"),
    ("round", "ADBE Vector Filter - RC"),
    ("trim", "ADBE Vector Filter - Trim"),
    ("twist", "ADBE Vector Filter - Twist"),
    ("wiggle", "ADBE Vector Filter - Roughen"),
    ("zigzag", "ADBE Vector Filter - Zigzag"),
];

/// Properties inside shape items: (item match id, property match id, AE match name).
const SHAPE_PROPS: &[(&str, &str, &str)] = &[
    ("group", "blend", "ADBE Vector Blend Mode"),
    ("group", "contents", "ADBE Vectors Group"),
    ("group", "transform", "ADBE Vector Transform Group"),
    ("transform", "anchor", "ADBE Vector Anchor"),
    ("transform", "position", "ADBE Vector Position"),
    ("transform", "scale", "ADBE Vector Scale"),
    ("transform", "skew", "ADBE Vector Skew"),
    ("transform", "skewAxis", "ADBE Vector Skew Axis"),
    ("transform", "rotation", "ADBE Vector Rotation"),
    ("transform", "opacity", "ADBE Vector Group Opacity"),
    ("rect", "direction", "ADBE Vector Shape Direction"),
    ("rect", "size", "ADBE Vector Rect Size"),
    ("rect", "position", "ADBE Vector Rect Position"),
    ("rect", "roundness", "ADBE Vector Rect Roundness"),
    ("ellipse", "direction", "ADBE Vector Shape Direction"),
    ("ellipse", "size", "ADBE Vector Ellipse Size"),
    ("ellipse", "position", "ADBE Vector Ellipse Position"),
    ("star", "direction", "ADBE Vector Shape Direction"),
    ("star", "type", "ADBE Vector Star Type"),
    ("star", "points", "ADBE Vector Star Points"),
    ("star", "position", "ADBE Vector Star Position"),
    ("star", "rotation", "ADBE Vector Star Rotation"),
    ("star", "innerRadius", "ADBE Vector Star Inner Radius"),
    ("star", "outerRadius", "ADBE Vector Star Outer Radius"),
    ("star", "innerRoundness", "ADBE Vector Star Inner Roundess"),
    ("star", "outerRoundness", "ADBE Vector Star Outer Roundess"),
    ("path", "direction", "ADBE Vector Shape Direction"),
    ("path", "path", "ADBE Vector Shape"),
    ("fill", "blend", "ADBE Vector Blend Mode"),
    ("fill", "composite", "ADBE Vector Composite Order"),
    ("fill", "rule", "ADBE Vector Fill Rule"),
    ("fill", "color", "ADBE Vector Fill Color"),
    ("fill", "opacity", "ADBE Vector Fill Opacity"),
    ("stroke", "blend", "ADBE Vector Blend Mode"),
    ("stroke", "composite", "ADBE Vector Composite Order"),
    ("stroke", "color", "ADBE Vector Stroke Color"),
    ("stroke", "opacity", "ADBE Vector Stroke Opacity"),
    ("stroke", "width", "ADBE Vector Stroke Width"),
    ("stroke", "cap", "ADBE Vector Stroke Line Cap"),
    ("stroke", "join", "ADBE Vector Stroke Line Join"),
    ("stroke", "miter", "ADBE Vector Stroke Miter Limit"),
    ("stroke", "dashes", "ADBE Vector Stroke Dashes"),
    ("stroke", "taper", "ADBE Vector Stroke Taper"),
    ("stroke", "wave", "ADBE Vector Stroke Wave"),
    ("trim", "start", "ADBE Vector Trim Start"),
    ("trim", "end", "ADBE Vector Trim End"),
    ("trim", "offset", "ADBE Vector Trim Offset"),
    ("trim", "mode", "ADBE Vector Trim Type"),
    ("repeater", "copies", "ADBE Vector Repeater Copies"),
    ("repeater", "offset", "ADBE Vector Repeater Offset"),
    ("repeater", "composite", "ADBE Vector Repeater Order"),
    ("repeater", "transform", "ADBE Vector Repeater Transform"),
    ("round", "radius", "ADBE Vector RoundCorner Radius"),
    ("pucker", "amount", "ADBE Vector PuckerBloat Amount"),
    ("twist", "angle", "ADBE Vector Twist Angle"),
    ("twist", "center", "ADBE Vector Twist Center"),
    ("merge", "mode", "ADBE Vector Merge Type"),
    ("wiggle", "size", "ADBE Vector Roughen Size"),
    ("wiggle", "detail", "ADBE Vector Roughen Detail"),
    ("wiggle", "points", "ADBE Vector Roughen Points"),
    ("wiggle", "speed", "ADBE Vector Temporal Freq"),
    ("wiggle", "correlation", "ADBE Vector Correlation"),
    ("wiggle", "phase", "ADBE Vector Temporal Phase"),
    ("wiggle", "spatialPhase", "ADBE Vector Spatial Phase"),
    ("wiggle", "seed", "ADBE Vector Random Seed"),
];

/// Effects: our effect id → AE match name (the documented ones; the rest keep our id).
pub const EFFECTS: &[(&str, &str)] = &[
    ("ec.blur.gaussian", "ADBE Gaussian Blur 2"),
    ("ec.blur.fastbox", "ADBE Box Blur2"),
    ("ec.blur.directional", "ADBE Motion Blur"),
    ("ec.blur.radial", "ADBE Radial Blur"),
    ("ec.blur.cameralens", "ADBE Camera Lens Blur"),
    ("ec.blur.channel", "ADBE Channel Blur"),
    ("ec.blur.compound", "ADBE Compound Blur"),
    ("ec.blur.sharpen", "ADBE Sharpen"),
    ("ec.blur.smart", "ADBE Smart Blur"),
    ("ec.blur.unsharp", "ADBE Unsharp Mask2"),
    ("ec.channel.invert", "ADBE Invert"),
    ("ec.channel.setmatte", "ADBE Set Matte3"),
    ("ec.channel.setchannels", "ADBE Set Channels"),
    ("ec.channel.shiftchannels", "ADBE Shift Channels"),
    ("ec.channel.minimax", "ADBE Minimax"),
    ("ec.channel.blend", "ADBE Blend"),
    ("ec.channel.arithmetic", "ADBE Arithmetic"),
    ("ec.channel.calculations", "ADBE Calculations"),
    ("ec.channel.solidcomposite", "ADBE Solid Composite"),
    ("ec.color.brightnesscontrast", "ADBE Brightness & Contrast 2"),
    ("ec.color.curves", "ADBE CurvesCustom"),
    ("ec.color.levels", "ADBE Easy Levels2"),
    ("ec.color.levelsic", "ADBE Pro Levels2"),
    ("ec.color.huesaturation", "ADBE HUE SATURATION"),
    ("ec.color.exposure", "ADBE Exposure2"),
    ("ec.color.tint", "ADBE Tint"),
    ("ec.color.tritone", "ADBE Tritone"),
    ("ec.color.blackwhite", "ADBE Black&White"),
    ("ec.color.channelmixer", "ADBE CHANNEL MIXER"),
    ("ec.color.colorbalance", "ADBE Color Balance 2"),
    ("ec.color.colorbalancehls", "ADBE Color Balance (HLS)"),
    ("ec.color.changecolor", "ADBE Change Color"),
    ("ec.color.changetocolor", "ADBE Change To Color"),
    ("ec.color.colorama", "APC Colorama"),
    ("ec.color.leavecolor", "ADBE Leave Color"),
    ("ec.color.photofilter", "ADBE Photo Filter"),
    ("ec.color.vibrance", "ADBE Vibrance"),
    ("ec.color.lumetri", "ADBE Lumetri"),
    ("ec.color.shadowhighlight", "ADBE Shadow/Highlight"),
    ("ec.color.selectivecolor", "ADBE Selective Color"),
    ("ec.color.gammapedestalgain", "ADBE Gamma/Pedestal/Gain"),
    ("ec.color.equalize", "ADBE Equalize"),
    ("ec.color.autocontrast", "ADBE AutoContrast"),
    ("ec.color.autolevels", "ADBE AutoLevels"),
    ("ec.color.autocolor", "ADBE AutoColor"),
    ("ec.control.slider", "ADBE Slider Control"),
    ("ec.control.angle", "ADBE Angle Control"),
    ("ec.control.checkbox", "ADBE Checkbox Control"),
    ("ec.control.color", "ADBE Color Control"),
    ("ec.control.point", "ADBE Point Control"),
    ("ec.control.point3d", "ADBE Point3D Control"),
    ("ec.control.layer", "ADBE Layer Control"),
    ("ec.control.dropdown", "ADBE Dropdown Control"),
    ("ec.distort.transform", "ADBE Geometry2"),
    ("ec.distort.cornerpin", "ADBE Corner Pin"),
    ("ec.distort.bulge", "ADBE Bulge"),
    ("ec.distort.mirror", "ADBE Mirror"),
    ("ec.distort.offset", "ADBE Offset"),
    ("ec.distort.ripple", "ADBE Ripple"),
    ("ec.distort.spherize", "ADBE Spherize"),
    ("ec.distort.twirl", "ADBE Twirl"),
    ("ec.distort.wavewarp", "ADBE Wave Warp"),
    ("ec.distort.turbulentdisplace", "ADBE Turbulent Displace"),
    ("ec.distort.displacementmap", "ADBE Displacement Map"),
    ("ec.distort.meshwarp", "ADBE MESH WARP"),
    ("ec.distort.polar", "ADBE Polar Coordinates"),
    ("ec.distort.opticscompensation", "ADBE Optics Compensation"),
    ("ec.distort.magnify", "ADBE Magnify"),
    ("ec.distort.warpstabilizer", "ADBE SubspaceStabilizer"),
    ("ec.distort.puppet", "ADBE FreePin3"),
    ("ec.generate.fill", "ADBE Fill"),
    ("ec.generate.gradientramp", "ADBE Ramp"),
    ("ec.generate.fourcolor", "ADBE 4ColorGradient"),
    ("ec.generate.stroke", "ADBE Stroke"),
    ("ec.generate.grid", "ADBE Grid"),
    ("ec.generate.checkerboard", "ADBE Checkerboard"),
    ("ec.generate.circle", "ADBE Circle"),
    ("ec.generate.ellipse", "ADBE Ellipse"),
    ("ec.generate.cellpattern", "ADBE Cell Pattern"),
    ("ec.generate.lensflare", "ADBE Lens Flare"),
    ("ec.generate.vegas", "ADBE Vegas"),
    ("ec.generate.writeon", "ADBE Write On"),
    ("ec.generate.audiospectrum", "ADBE AudSpect"),
    ("ec.generate.audiowaveform", "ADBE AudWave"),
    ("ec.generate.beam", "ADBE Laser"),
    ("ec.generate.radiowaves", "ADBE Radio Waves"),
    ("ec.generate.scribble", "ADBE Scribble Fill"),
    ("ec.generate.paintbucket", "ADBE Paint Bucket"),
    ("ec.key.colorkey", "ADBE Color Key"),
    ("ec.key.luma", "ADBE Luma Key"),
    ("ec.key.linearcolor", "ADBE Linear Color Key2"),
    ("ec.key.colorrange", "ADBE Color Range"),
    ("ec.key.colordifference", "ADBE Color Difference Key"),
    ("ec.key.extract", "ADBE Extract"),
    ("ec.key.spill", "ADBE Spill2"),
    ("ec.matte.simplechoker", "ADBE Simple Choker"),
    ("ec.matte.mattechoker", "ADBE Matte Choker"),
    ("ec.noise.noise", "ADBE Noise"),
    ("ec.noise.fractal", "ADBE Fractal Noise"),
    ("ec.noise.median", "ADBE Median"),
    ("ec.noise.addgrain", "VISINF Grain Implant"),
    ("ec.noise.dustscratches", "ADBE Dust & Scratches"),
    ("ec.noise.turbulent", "ADBE Turbulent Noise"),
    ("ec.perspective.dropshadow", "ADBE Drop Shadow"),
    ("ec.perspective.radialshadow", "ADBE Radial Shadow"),
    ("ec.perspective.bevelalpha", "ADBE Bevel Alpha"),
    ("ec.perspective.beveledges", "ADBE Bevel Edges"),
    ("ec.perspective.3dglasses", "ADBE 3D Glasses2"),
    ("ec.stylize.glow", "ADBE Glo2"),
    ("ec.stylize.mosaic", "ADBE Mosaic"),
    ("ec.stylize.posterize", "ADBE Posterize"),
    ("ec.stylize.threshold", "ADBE Threshold2"),
    ("ec.stylize.findedges", "ADBE Find Edges"),
    ("ec.stylize.emboss", "ADBE Emboss"),
    ("ec.stylize.coloremboss", "ADBE Color Emboss"),
    ("ec.stylize.roughenedges", "ADBE Roughen Edges"),
    ("ec.stylize.scatter", "ADBE Scatter"),
    ("ec.stylize.strobe", "ADBE Strobe"),
    ("ec.stylize.motiontile", "ADBE Tile"),
    ("ec.stylize.texturize", "ADBE Texturize"),
    ("ec.stylize.cartoon", "ADBE Cartoonify"),
    ("ec.stylize.brushstrokes", "ADBE Brush Strokes"),
    ("ec.text.numbers", "ADBE Numbers2"),
    ("ec.text.timecode", "ADBE Timecode"),
    ("ec.time.echo", "ADBE Echo"),
    ("ec.time.posterizetime", "ADBE Posterize Time"),
    ("ec.time.timedisplacement", "ADBE Time Displacement"),
    ("ec.time.timedifference", "ADBE Time Difference"),
    ("ec.time.pixelmotionblur", "ADBE Pixel Motion Blur"),
    ("ec.time.timewarp", "ADBE Timewarp"),
    ("ec.transition.linearwipe", "ADBE Linear Wipe"),
    ("ec.transition.radialwipe", "ADBE Radial Wipe"),
    ("ec.transition.venetian", "ADBE Venetian Blinds"),
    ("ec.transition.blockdissolve", "ADBE Block Dissolve"),
    ("ec.transition.gradientwipe", "ADBE Gradient Wipe"),
    ("ec.transition.iriswipe", "ADBE Iris Wipe"),
    ("ec.utility.applylut", "ADBE Apply Color LUT2"),
    ("ec.utility.cineon", "ADBE Cineon Converter2"),
    ("ec.utility.growbounds", "ADBE Grow Bounds"),
];

fn find<'a>(t: &'a [(&str, &'a str)], k: &str) -> Option<&'a str> {
    t.iter().find(|(a, _)| *a == k).map(|(_, b)| *b)
}

/// AE match name of an effect id (falls back to the id).
pub fn effect(id: &str) -> String {
    find(EFFECTS, id).map(str::to_string).unwrap_or_else(|| id.to_string())
}

/// Our effect id for an AE effect match name.
pub fn effect_from_match(m: &str) -> Option<&'static str> {
    EFFECTS.iter().find(|(_, b)| b.eq_ignore_ascii_case(m)).map(|(a, _)| *a)
}

/// Our shape item kind (`layer.addShapeItem`) for an AE match name, our match id or a display
/// name (`Rectangle`, `Fill`, `Trim Paths`).
pub fn shape_kind(name: &str) -> Option<&'static str> {
    if let Some((k, _)) = SHAPE_ITEMS.iter().find(|(k, m)| m.eq_ignore_ascii_case(name) || k.eq_ignore_ascii_case(name)) {
        return Some(k);
    }
    let n: String = name.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_ascii_lowercase();
    Some(match n.as_str() {
        "rectangle" | "rectanglepath" => "rect",
        "ellipsepath" => "ellipse",
        "polystar" | "polystarpath" => "star",
        "polygon" => "polygon",
        "group" | "vectorgroup" => "group",
        "fill" => "fill",
        "stroke" => "stroke",
        "gradientfill" => "gfill",
        "gradientstroke" => "gstroke",
        "trimpaths" | "trim" => "trim",
        "repeater" => "repeater",
        "roundcorners" => "round",
        "offsetpaths" => "offset",
        "puckerbloat" | "puckerandbloat" => "pucker",
        "twist" => "twist",
        "zigzag" => "zigzag",
        "wigglepaths" => "wiggle",
        "mergepaths" => "merge",
        _ => return None,
    })
}

/// Effects whose documented parameter match names are not numbered in display order:
/// (effect id, param match id, number). Fill lists All Masks second but it is `-0007`. Params of
/// these effects that After Effects doesn't have are numbered after the listed ones (#1405).
const PARAM_NUMBERS: &[(&str, &str, usize)] = &[
    ("ec.generate.fill", "fillMask", 1),
    ("ec.generate.fill", "color", 2),
    ("ec.generate.fill", "horizontalFeather", 3),
    ("ec.generate.fill", "verticalFeather", 4),
    ("ec.generate.fill", "opacity", 5),
    ("ec.generate.fill", "invert", 6),
    ("ec.generate.fill", "allMasks", 7),
    ("ec.generate.stroke", "path", 1),
    ("ec.generate.stroke", "color", 2),
    ("ec.generate.stroke", "brushSize", 3),
    ("ec.generate.stroke", "brushHardness", 4),
    ("ec.generate.stroke", "opacity", 5),
    ("ec.generate.stroke", "spacing", 6),
    ("ec.generate.stroke", "paintStyle", 7),
    ("ec.generate.stroke", "start", 8),
    ("ec.generate.stroke", "end", 9),
    ("ec.generate.stroke", "allMasks", 10),
    ("ec.generate.stroke", "strokeSequentially", 11),
    // -0001 is a hidden parameter and -0003 Channel Range, which we don't have; the per-channel
    // controls are ours (After Effects edits the selected channel with the Master ones).
    ("ec.color.huesaturation", "channelControl", 2),
    ("ec.color.huesaturation", "hue", 4),
    ("ec.color.huesaturation", "saturation", 5),
    ("ec.color.huesaturation", "lightness", 6),
    ("ec.color.huesaturation", "colorize", 7),
    ("ec.color.huesaturation", "colorizeHue", 8),
    ("ec.color.huesaturation", "colorizeSaturation", 9),
    ("ec.color.huesaturation", "colorizeLightness", 10),
];

/// The `-NNNN` number of the param `id` at 0-based `pos` among `fx`'s `params`.
fn param_number(fx: &str, params: &[Node], id: &str, pos: usize) -> usize {
    let listed = |p: &str| PARAM_NUMBERS.iter().find(|(f, q, _)| *f == fx && *q == p).map(|(_, _, n)| *n);
    if let Some(n) = listed(id) {
        return n;
    }
    let Some(last) = PARAM_NUMBERS.iter().filter(|(f, _, _)| *f == fx).map(|(_, _, n)| *n).max() else {
        return pos + 1;
    };
    let unlisted_before = params.iter().take(pos).filter(|c| listed(c.match_id()).is_none()).count();
    last + unlisted_before + 1
}

/// Match name of the node at the end of `chain` (layer root's children → … → node). `effect`
/// params are numbered `<effect match name>-0001…` in parameter order, like After Effects.
pub fn node(chain: &[&Node]) -> String {
    let Some(last) = chain.last() else { return String::new() };
    let id = last.match_id();
    let parent = chain.len().checked_sub(2).map(|i| chain[i]);
    let grand = chain.len().checked_sub(3).map(|i| chain[i]);
    let top = chain.first().map(|n| n.match_id()).unwrap_or("");
    let own = || id.to_string();
    if chain.len() == 1 {
        return find(LAYER_GROUPS, id).map(str::to_string).unwrap_or_else(own);
    }
    let pid = parent.map(|p| p.match_id()).unwrap_or("");
    match top {
        "transform" if chain.len() == 2 => find(TRANSFORM, id).map(str::to_string).unwrap_or_else(own),
        "masks" if chain.len() == 2 => "ADBE Mask Atom".into(),
        "masks" if chain.len() == 3 => find(MASK, id).map(str::to_string).unwrap_or_else(own),
        "effects" if chain.len() == 2 => effect(id),
        "effects" if chain.len() == 3 => {
            let fx = parent.and_then(Node::as_group);
            match fx {
                Some(g) => {
                    let pos = g.children.iter().position(|c| c.uid() == last.uid()).unwrap_or(0);
                    let i = param_number(&g.match_id, &g.children, id, pos);
                    format!("{}-{i:04}", effect(&g.match_id))
                }
                None => own(),
            }
        }
        "text" if chain.len() == 2 => find(TEXT, id).map(str::to_string).unwrap_or_else(own),
        "text" if pid == "pathOptions" => find(TEXT_PATH_OPTIONS, id).map(str::to_string).unwrap_or_else(own),
        "text" if pid == "animators" => "ADBE Text Animator".into(),
        "text" if pid == "selectors" => find(TEXT_SELECTORS, id).map(str::to_string).unwrap_or_else(own),
        "text" if pid == "rangeSelector" => find(TEXT_RANGE_SELECTOR, id).map(str::to_string).unwrap_or_else(own),
        "text" if grand.map(|g| g.match_id()) == Some("animators") => find(TEXT_ANIMATOR, id).map(str::to_string).unwrap_or_else(own),
        "cameraOptions" => find(CAMERA, id).map(str::to_string).unwrap_or_else(own),
        "lightOptions" => find(LIGHT, id).map(str::to_string).unwrap_or_else(own),
        "materialOptions" => find(MATERIAL, id).map(str::to_string).unwrap_or_else(own),
        "audio" if id == "levels" => "ADBE Audio Levels".into(),
        "contents" => {
            // Items sit in a `contents` list; their properties under the item.
            if pid == "contents" {
                if id == "star" || id == "polygon" {
                    return "ADBE Vector Shape - Star".into();
                }
                return find(SHAPE_ITEMS, id).map(str::to_string).unwrap_or_else(own);
            }
            if let Some((_, _, m)) = SHAPE_PROPS.iter().find(|(item, prop, _)| *item == pid && *prop == id) {
                return m.to_string();
            }
            if pid == "dashes" {
                return find(STROKE_DASHES, id).map(str::to_string).unwrap_or_else(own);
            }
            own()
        }
        _ => own(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_names_round_trip() {
        for (id, m) in EFFECTS {
            assert_eq!(effect(id), *m);
            assert_eq!(effect_from_match(m), Some(*id));
        }
        assert_eq!(effect("ec.blur.ccvector"), "ec.blur.ccvector");
        let mut ids: Vec<&str> = EFFECTS.iter().map(|(a, _)| *a).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), EFFECTS.len(), "duplicate effect ids");
    }

    #[test]
    fn shape_kinds() {
        assert_eq!(shape_kind("ADBE Vector Shape - Rect"), Some("rect"));
        assert_eq!(shape_kind("Trim Paths"), Some("trim"));
        assert_eq!(shape_kind("ADBE Vector Graphic - Fill"), Some("fill"));
        assert_eq!(shape_kind("nope"), None);
    }

    #[test]
    fn text_range_selector_names() {
        let n = |m: &str| Node::from(effectcraft_project::PropGroup::new(1, m, m));
        let (text, animators, animator, selectors, range, start, end, offset) =
            (n("text"), n("animators"), n("animator"), n("selectors"), n("rangeSelector"), n("start"), n("end"), n("offset"));
        assert_eq!(node(&[&text, &animators, &animator, &selectors, &range]), "ADBE Text Selector");
        assert_eq!(node(&[&text, &animators, &animator, &selectors, &range, &start]), "ADBE Text Percent Start");
        assert_eq!(node(&[&text, &animators, &animator, &selectors, &range, &end]), "ADBE Text Percent End");
        assert_eq!(node(&[&text, &animators, &animator, &selectors, &range, &offset]), "ADBE Text Percent Offset");
    }
}
