//! The effect table shared by the importer (`effectcraft-aep-import`) and the exporter
//! (`effectcraft-aep-export`): an After Effects effect (its match name) and its parameters (by
//! their index in `parT`) map to an EffectCraft effect, a parameter id and a unit conversion.
//! Plain data; the evidence for each row is in `docs/aep-format.md` 3.7 and the case numbers in
//! the comments. The importer reads a file through it, the exporter writes the inverse.

use crate::effects::ParamKind;
use ParamKind as K;

/// How a stored parameter value becomes an EffectCraft value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Conv {
    /// A colour (ARGB 0..255 or the reader's ARGB fractions) to opaque RGBA 0..1.
    Color,
    /// A point (fraction of the layer size) to layer pixels.
    Point,
    /// A slider shown as a percent here: the stored value is a fraction of `valid_max`.
    Percent { valid_max: f64 },
    /// A slider whose stored number is used as it is (`valid_max` checks the scale).
    Plain { valid_max: f64 },
    /// A slider whose stored number times `factor` is the value here (`valid_max` checks the
    /// scale).
    Factor { valid_max: f64, factor: f64 },
    /// An angle in degrees.
    Angle,
    /// A popup, mapped by item name; `default` is After Effects' default (1-based) for a fresh
    /// effect, used when the file's default slot cannot be trusted. `items` are After Effects'
    /// items as the corpus stores them (`pdnm`): used when a file has no `pdnm` for the popup
    /// and its item count (`pard` 60) matches.
    Popup { default: u32, items: &'static [&'static str] },
    /// A checkbox (stored 1.0 when on).
    Checkbox,
    /// A layer (the `tdpi` id of a layer of the same composition, 0 = None; 904 E22-E24, 220,
    /// 320, 321). `own` says what the owning layer itself becomes: itself (Layer Control) or
    /// EffectCraft's unset layer (effects whose unset layer means "this layer").
    LayerRef { own: bool },
    /// A parameter that holds no value to import (Levels (Individual Controls)' histogram).
    Skip,
}

// After Effects' popup items as the corpus stores them (`pdnm`).
/// 368.
pub const RAMP_SHAPE: &[&str] = &["Linear Ramp", "Radial Ramp"];
/// 389.
pub const EXPOSURE_CHANNELS: &[&str] = &["Master", "Individual Channels"];
/// 409.
pub const INVERT_CHANNEL: &[&str] = &[
    "RGB",
    "Red",
    "Green",
    "Blue",
    "(-",
    "HLS",
    "Hue",
    "Lightness",
    "Saturation",
    "(-",
    "YIQ",
    "Luminance",
    "In Phase Chrominance",
    "Quadrature Chrominance",
    "(-",
    "Alpha",
];
/// 412, 417, 427.
pub const BLUR_DIMENSIONS: &[&str] = &["Horizontal and Vertical", "Horizontal", "Vertical"];
/// 529.
pub const TIMEWARP_METHOD: &[&str] = &["Whole Frames", "Frame Mix", "Pixel Motion"];
/// 529.
pub const TIMEWARP_ADJUST: &[&str] = &["Speed", "Source Frame"];
/// 427.
pub const GLOW_BASED_ON: &[&str] = &["Alpha Channel", "Color Channels"];
/// 427.
pub const GLOW_COMPOSITE: &[&str] = &["On Top", "Behind", "None"];
/// 427.
pub const GLOW_OPERATION: &[&str] = &[
    "None",
    "Normal",
    "Add",
    "Multiply",
    "Dissolve",
    "Screen",
    "Overlay",
    "Soft Light",
    "Hard Light",
    "Darken",
    "Lighten",
    "Difference",
    "Hue",
    "Saturation",
    "Color",
    "Luminosity",
    "Color Dodge",
    "Color Burn",
    "Exclusion",
    "Stencil Alpha",
    "Stencil Luma",
    "Silhouette Alpha",
    "Silhouette Luma",
    "Luminescent Premultiply",
    "Alpha Add",
];
/// 427.
pub const GLOW_COLORS: &[&str] = &["Original Colors", "A & B Colors", "Arbitrary Map"];
/// 427.
pub const GLOW_LOOPING: &[&str] = &["Sawtooth A>B", "Sawtooth B>A", "Triangle A>B>A", "Triangle B>A>B"];
/// 443.
pub const HUE_SAT_CHANNEL: &[&str] = &["Master", "Reds", "Yellows", "Greens", "Cyans", "Blues", "Magentas"];
/// 511.
pub const LEVELS_CHANNEL: &[&str] = &["RGB", "Red", "Green", "Blue", "Alpha"];
/// 511.
pub const LEVELS_CLIP: &[&str] = &["On", "Off", "Off for 32 bpc Color"];

/// One mapped parameter.
#[derive(Clone, Copy, Debug)]
pub struct ParamMap {
    /// The index in `parT` (0 is the input layer).
    pub index: usize,
    /// The parameter type the file must declare.
    pub kind: ParamKind,
    /// The EffectCraft parameter id.
    pub target: &'static str,
    pub conv: Conv,
}

/// Values of an effect that are not ordinary parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extra {
    None,
    /// Hue/Saturation's master values in the Channel Range data (444-446).
    HueSaturation,
    /// Levels' values in the Histogram data (512-516).
    Levels,
    /// Curves' five curves (961-964, `CurvesData`).
    Curves,
    /// Mesh Warp's Distortion Mesh (965, 985, `MeshData`).
    MeshWarp,
}

/// One mapped effect.
#[derive(Clone, Copy, Debug)]
pub struct EffectMap {
    pub match_name: &'static str,
    /// The EffectCraft effect id.
    pub effect: &'static str,
    pub params: &'static [ParamMap],
    pub extra: Extra,
}

const fn pm(index: usize, kind: ParamKind, target: &'static str, conv: Conv) -> ParamMap {
    ParamMap { index, kind, target, conv }
}

const fn fx(match_name: &'static str, effect: &'static str, params: &'static [ParamMap]) -> EffectMap {
    EffectMap { match_name, effect, params, extra: Extra::None }
}

/// 712.
pub const RADIAL_TYPE: &[&str] = &["Spin", "Zoom"];
/// 712.
pub const RADIAL_AA: &[&str] = &["Low", "High"];
/// 712 (the fresh default is item 6, Scratch).
pub const CC_RADIAL_TYPE: &[&str] = &["Straight Zoom", "Fading Zoom", "Centered Zoom", "(-", "Rotate", "Scratch", "Rotate Fading"];
/// 712.
pub const LUMA_KEY_TYPE: &[&str] = &["Key Out Brighter", "Key Out Darker", "Key Out Similar", "Key Out Dissimilar"];
/// 712.
pub const CHOKER_VIEW: &[&str] = &["Final Output", "Matte"];
/// 712.
pub const SAMPLING: &[&str] = &["Bilinear", "Bicubic"];

// After Effects' popup items of the batch-6 effects as 903 stores them (`pdnm`).
/// 903.
pub const TURB_DISPLACEMENT: &[&str] = &[
    "Turbulent",
    "Bulge",
    "Twist",
    "(-",
    "Turbulent Smoother",
    "Bulge Smoother",
    "Twist Smoother",
    "(-",
    "Vertical Displacement",
    "Horizontal Displacement",
    "Cross Displacement",
];
/// 903.
pub const TURB_PINNING: &[&str] = &[
    "None",
    "(-",
    "Pin All",
    "Pin Horizontal",
    "Pin Vertical",
    "Pin Left",
    "Pin Right",
    "Pin Top",
    "Pin Bottom",
    "(-",
    "Pin All Locked",
    "Pin Horizontal Locked",
    "Pin Vertical Locked",
    "Pin Left Locked",
    "Pin Right Locked",
    "Pin Top Locked",
    "Pin Bottom Locked",
];
/// 903.
pub const FRACTAL_TYPE: &[&str] = &[
    "Basic",
    "(-",
    "Turbulent Smooth",
    "Turbulent Basic",
    "Turbulent Sharp",
    "(-",
    "Dynamic",
    "Dynamic Progressive",
    "Dynamic Twist",
    "(-",
    "Max",
    "Smeary",
    "Swirly",
    "Rocky",
    "Cloudy",
    "Terrain",
    "Subscale",
    "Small Bumps",
    "Strings",
    "Threads",
];
/// 903.
pub const NOISE_TYPE: &[&str] = &["Block", "Linear", "Soft Linear", "Spline"];
/// 903.
pub const OVERFLOW: &[&str] = &["Clip", "Soft Clamp", "Wrap Back", "Allow HDR Results"];
/// 903 (Fractal Noise).
pub const FRACTAL_BLEND: &[&str] = &[
    "None",
    "Normal",
    "(-",
    "Add",
    "Multiply",
    "Screen",
    "Overlay",
    "Soft Light",
    "Hard Light",
    "(-",
    "Color Dodge",
    "Color Burn",
    "(-",
    "Darken",
    "Lighten",
    "Difference",
    "Exclusion",
    "(-",
    "Hue",
    "Saturation",
    "Luminosity",
];
/// 903 (4-Color Gradient).
pub const FOUR_COLOR_BLEND: &[&str] = &[
    "None",
    "Normal",
    "(-",
    "Add",
    "Multiply",
    "Screen",
    "Overlay",
    "Soft Light",
    "Hard Light",
    "(-",
    "Color Dodge",
    "Color Burn",
    "(-",
    "Darken",
    "Lighten",
    "Difference",
    "Exclusion",
    "(-",
    "Hue",
    "Saturation",
    "Color",
    "Luminosity",
];
/// 903 (Grid).
pub const GRID_BLEND: &[&str] = &[
    "None",
    "Normal",
    "Stencil Alpha",
    "(-",
    "Add",
    "Multiply",
    "Screen",
    "Overlay",
    "Soft Light",
    "Hard Light",
    "(-",
    "Color Dodge",
    "Color Burn",
    "(-",
    "Darken",
    "Lighten",
    "Difference",
    "Exclusion",
    "(-",
    "Hue",
    "Saturation",
    "Color",
    "Luminosity",
];
/// 903.
pub const FAST_ZOOM: &[&str] = &["Standard", "Brightest", "Darkest"];
/// 903.
pub const GRID_SIZE_FROM: &[&str] = &["Corner Point", "Width Slider", "Width & Height Sliders"];
/// 903.
pub const KEY_VIEW: &[&str] = &["Final Output", "Source Only", "Matte Only"];
/// 903.
pub const MATCH_COLORS: &[&str] = &["Using RGB", "Using Hue", "Using Chroma"];
/// 903.
pub const KEY_OPERATION: &[&str] = &["Key Colors", "Keep Colors"];
/// 903.
pub const WIPE: &[&str] = &["Clockwise", "Counterclockwise", "Both"];
/// 903.
pub const BEND_PRESTART: &[&str] = &["None", "Static", "Bend", "Mirror"];
/// 903.
pub const CHANGE_VIEW: &[&str] = &["Corrected Layer", "Color Correction Mask"];
/// 903.
pub const EXTRACT_CHANNEL: &[&str] = &["Luminance", "Red", "Green", "Blue", "Alpha"];
/// 903.
pub const NOISE_HLS: &[&str] = &["Uniform", "Squared", "Grain"];
/// 903.
pub const SCATTER_GRAIN: &[&str] = &["Both", "Horizontal", "Vertical"];
/// 903.
pub const SWEEP_SHAPE: &[&str] = &["Linear", "Smooth", "Sharp"];
/// 903.
pub const SWEEP_RECEPTION: &[&str] = &["Add", "Composite", "Cutout"];
/// 903.
pub const DISPLACE_USE: &[&str] = &["Red", "Green", "Blue", "Alpha", "Luminance", "Hue", "Lightness", "Saturation", "Full", "Half", "Off"];
/// 903.
pub const DISPLACE_BEHAVIOR: &[&str] = &["Center Map", "Stretch Map to Fit", "Tile Map"];
/// 903.
pub const MATTE_USE: &[&str] = &["Red Channel", "Green Channel", "Blue Channel", "Alpha Channel", "Luminance", "Hue", "Lightness", "Saturation", "Full", "Off"];
/// 903.
pub const SHUTTER_CONTROL: &[&str] = &["Manual", "Automatic"];

/// The effects the importer converts.
pub const EFFECTS: &[EffectMap] = &[
    // Tint (356-364): Map Black To, Map White To, Amount to Tint; 4 = Swap Colors (a button).
    fx(
        "ADBE Tint",
        "ec.color.tint",
        &[pm(1, K::Color, "black", Conv::Color), pm(2, K::Color, "white", Conv::Color), pm(3, K::FixedSlider, "amount", Conv::Percent { valid_max: 100.0 })],
    ),
    // Gradient Ramp (368-383): 8 = Swap Colors (a button).
    fx(
        "ADBE Ramp",
        "ec.generate.gradientramp",
        &[
            pm(1, K::Point, "start", Conv::Point),
            pm(2, K::Color, "startColor", Conv::Color),
            pm(3, K::Point, "end", Conv::Point),
            pm(4, K::Color, "endColor", Conv::Color),
            pm(5, K::Popup, "shape", Conv::Popup { default: 1, items: RAMP_SHAPE }),
            pm(6, K::FixedSlider, "scatter", Conv::Plain { valid_max: 512.0 }),
            pm(7, K::FixedSlider, "blend", Conv::Percent { valid_max: 1.0 }),
        ],
    ),
    // Exposure (389-397): 1 Channels; Master 2-6, Red 7-11, Green 12-16, Blue 17-21, each
    // [group start, Exposure, Offset, Gamma Correction, group end]; 22 Bypass Linear Light
    // Conversion. Valid maxima: Exposure 100 (stops), Offset 2, Gamma Correction 10.
    fx(
        "ADBE Exposure2",
        "ec.color.exposure",
        &[
            pm(1, K::Popup, "channels", Conv::Popup { default: 1, items: EXPOSURE_CHANNELS }),
            pm(3, K::FloatSlider, "master/exposure", EXPOSURE),
            pm(4, K::FloatSlider, "master/offset", OFFSET),
            pm(5, K::FloatSlider, "master/gamma", GAMMA),
            pm(8, K::FloatSlider, "red/redExposure", EXPOSURE),
            pm(9, K::FloatSlider, "red/redOffset", OFFSET),
            pm(10, K::FloatSlider, "red/redGamma", GAMMA),
            pm(13, K::FloatSlider, "green/greenExposure", EXPOSURE),
            pm(14, K::FloatSlider, "green/greenOffset", OFFSET),
            pm(15, K::FloatSlider, "green/greenGamma", GAMMA),
            pm(18, K::FloatSlider, "blue/blueExposure", EXPOSURE),
            pm(19, K::FloatSlider, "blue/blueOffset", OFFSET),
            pm(20, K::FloatSlider, "blue/blueGamma", GAMMA),
            pm(22, K::Checkbox, "bypassLinearLight", Conv::Checkbox),
        ],
    ),
    // Invert (409-411): Channel (16 items with separators), Blend With Original (percent).
    fx(
        "ADBE Invert",
        "ec.channel.invert",
        &[pm(1, K::Popup, "channel", Conv::Popup { default: 1, items: INVERT_CHANNEL }), pm(2, K::FixedSlider, "blend", Conv::Plain { valid_max: 100.0 })],
    ),
    // Fast Box Blur (412-416).
    fx(
        "ADBE Box Blur2",
        "ec.blur.fastbox",
        &[
            pm(1, K::FixedSlider, "radius", Conv::Plain { valid_max: 30000.0 }),
            pm(2, K::IntSlider, "iterations", Conv::Plain { valid_max: 50.0 }),
            pm(3, K::Popup, "dimensions", Conv::Popup { default: 1, items: BLUR_DIMENSIONS }),
            pm(4, K::Checkbox, "repeatEdge", Conv::Checkbox),
        ],
    ),
    // Gaussian Blur (214, 216, 300, 301, 417-419).
    fx(
        "ADBE Gaussian Blur 2",
        "ec.blur.gaussian",
        &[
            pm(1, K::FloatSlider, "blurriness", Conv::Plain { valid_max: 30000.0 }),
            pm(2, K::Popup, "dimensions", Conv::Popup { default: 1, items: BLUR_DIMENSIONS }),
            pm(3, K::Checkbox, "repeatEdge", Conv::Checkbox),
        ],
    ),
    // Drop Shadow (080, 420-426).
    fx(
        "ADBE Drop Shadow",
        "ec.perspective.dropshadow",
        &[
            pm(1, K::Color, "color", Conv::Color),
            pm(2, K::FixedSlider, "opacity", Conv::Percent { valid_max: 255.0 }),
            pm(3, K::Angle, "direction", Conv::Angle),
            pm(4, K::FixedSlider, "distance", Conv::Plain { valid_max: 4000.0 }),
            pm(5, K::FixedSlider, "softness", Conv::Plain { valid_max: 30000.0 }),
            pm(6, K::Checkbox, "shadowOnly", Conv::Checkbox),
        ],
    ),
    // Timewarp (529-533): Speed and Source Frame are plain numbers (valid max 5 000); the
    // tuning, motion blur, matte and crop parameters are not mapped (reported when stored).
    fx(
        "ADBE Timewarp",
        "ec.time.timewarp",
        &[
            pm(1, K::Popup, "method", Conv::Popup { default: 3, items: TIMEWARP_METHOD }),
            pm(2, K::Popup, "adjustTimeBy", Conv::Popup { default: 1, items: TIMEWARP_ADJUST }),
            pm(3, K::FixedSlider, "speed", Conv::Plain { valid_max: 5000.0 }),
            pm(4, K::FixedSlider, "sourceFrame", Conv::Plain { valid_max: 5000.0 }),
        ],
    ),
    // Glow (427-441).
    fx(
        "ADBE Glo2",
        "ec.stylize.glow",
        &[
            pm(1, K::Popup, "based", Conv::Popup { default: 2, items: GLOW_BASED_ON }),
            pm(2, K::FixedSlider, "threshold", Conv::Percent { valid_max: 255.0 }),
            pm(3, K::FixedSlider, "radius", Conv::Plain { valid_max: 1000.0 }),
            pm(4, K::FixedSlider, "intensity", Conv::Plain { valid_max: 255.0 }),
            pm(5, K::Popup, "operation", Conv::Popup { default: 2, items: GLOW_COMPOSITE }),
            pm(6, K::Popup, "glowOperation", Conv::Popup { default: 3, items: GLOW_OPERATION }),
            pm(7, K::Popup, "colors", Conv::Popup { default: 1, items: GLOW_COLORS }),
            pm(8, K::Popup, "colorLooping", Conv::Popup { default: 3, items: GLOW_LOOPING }),
            pm(9, K::FixedSlider, "colorLoops", Conv::Plain { valid_max: 127.0 }),
            pm(10, K::Angle, "colorPhase", Conv::Angle),
            pm(11, K::FixedSlider, "abMidpoint", Conv::Factor { valid_max: GLOW_MIDPOINT_MAX, factor: 100.0 }),
            pm(12, K::Color, "colorA", Conv::Color),
            pm(13, K::Color, "colorB", Conv::Color),
            pm(14, K::Popup, "glowDimensions", Conv::Popup { default: 1, items: BLUR_DIMENSIONS }),
        ],
    ),
    // Fill (079, 210, 215, 302, 303, 307): 1 Fill Mask (a path reference, reported when set).
    fx(
        "ADBE Fill",
        "ec.generate.fill",
        &[
            pm(2, K::Checkbox, "allMasks", Conv::Checkbox),
            pm(3, K::Color, "color", Conv::Color),
            pm(4, K::Checkbox, "invert", Conv::Checkbox),
            pm(5, K::FixedSlider, "horizontalFeather", Conv::Plain { valid_max: 999.0 }),
            pm(6, K::FixedSlider, "verticalFeather", Conv::Plain { valid_max: 999.0 }),
            pm(7, K::FixedSlider, "opacity", Conv::Percent { valid_max: 1.0 }),
        ],
    ),
    // Hue/Saturation (443-450): the master values are arbitrary data (parameter 2).
    EffectMap {
        match_name: "ADBE HUE SATURATION",
        effect: "ec.color.huesaturation",
        params: &[
            pm(1, K::Popup, "channelControl", Conv::Popup { default: 1, items: HUE_SAT_CHANNEL }),
            pm(6, K::Checkbox, "colorize", Conv::Checkbox),
            pm(7, K::Angle, "colorizeHue", Conv::Angle),
            pm(8, K::FixedSlider, "colorizeSaturation", Conv::Plain { valid_max: 100.0 }),
            pm(9, K::FixedSlider, "colorizeLightness", Conv::Plain { valid_max: 100.0 }),
        ],
        extra: Extra::HueSaturation,
    },
    // Levels (511-516): the values are arbitrary data (parameter 2).
    EffectMap {
        match_name: "ADBE Easy Levels2",
        effect: "ec.color.levels",
        params: &[
            pm(1, K::Popup, "channel", Conv::Popup { default: 1, items: LEVELS_CHANNEL }),
            pm(8, K::Popup, "clipToOutputBlack", Conv::Popup { default: 3, items: LEVELS_CLIP }),
            pm(9, K::Popup, "clipToOutputWhite", Conv::Popup { default: 3, items: LEVELS_CLIP }),
        ],
        extra: Extra::Levels,
    },
    // Curves (961-964, FORMAT S.1): 1 Channel only picks the curve shown (UI), 2 the curves
    // themselves (arbitrary data, always stored).
    EffectMap { match_name: "ADBE CurvesCustom", effect: "ec.color.curves", params: &[pm(1, K::Popup, "", Conv::Skip)], extra: Extra::Curves },
    // Mesh Warp (965, 985, FORMAT S.2): Rows, Columns, Quality are integer sliders (records only
    // when not at the default), 4 the Distortion Mesh (arbitrary data, always stored).
    EffectMap {
        match_name: "ADBE MESH WARP",
        effect: "ec.distort.meshwarp",
        params: &[
            pm(1, K::IntSlider, "rows", Conv::Plain { valid_max: 31.0 }),
            pm(2, K::IntSlider, "columns", Conv::Plain { valid_max: 31.0 }),
            pm(3, K::IntSlider, "quality", Conv::Plain { valid_max: 10.0 }),
        ],
        extra: Extra::MeshWarp,
    },
    // Expression controls (070-077, 217-221, 310).
    // Directional Blur (711): Direction in degrees, Blur Length in pixels (valid max 1 000).
    fx(
        "ADBE Motion Blur",
        "ec.blur.directional",
        &[pm(1, K::Angle, "direction", Conv::Angle), pm(2, K::FixedSlider, "length", Conv::Plain { valid_max: 1000.0 })],
    ),
    // Radial Blur (712, 713): 1 is a hidden entry without data; 6 Random Seed has no EffectCraft
    // parameter (reported when stored).
    fx(
        "ADBE Radial Blur",
        "ec.blur.radial",
        &[
            pm(2, K::FixedSlider, "amount", Conv::Plain { valid_max: 1000.0 }),
            pm(3, K::Point, "center", Conv::Point),
            pm(4, K::Popup, "type", Conv::Popup { default: 1, items: RADIAL_TYPE }),
            pm(5, K::Popup, "antialiasing", Conv::Popup { default: 1, items: RADIAL_AA }),
        ],
    ),
    // CC Radial Blur (712, 714).
    fx(
        "CC Radial Blur",
        "ec.blur.ccradial",
        &[
            pm(1, K::Popup, "type", Conv::Popup { default: 6, items: CC_RADIAL_TYPE }),
            pm(2, K::FixedSlider, "amount", Conv::Plain { valid_max: 250.0 }),
            pm(3, K::FixedSlider, "quality", Conv::Plain { valid_max: 100.0 }),
            pm(4, K::Point, "center", Conv::Point),
        ],
    ),
    // Venetian Blinds (712, 715).
    fx(
        "ADBE Venetian Blinds",
        "ec.transition.venetian",
        &[
            pm(1, K::FixedSlider, "completion", Conv::Plain { valid_max: 100.0 }),
            pm(2, K::Angle, "direction", Conv::Angle),
            pm(3, K::FixedSlider, "width", Conv::Plain { valid_max: 32000.0 }),
            pm(4, K::FixedSlider, "feather", Conv::Plain { valid_max: 32000.0 }),
        ],
    ),
    // Channel Blur (712, 716): 5 Edge Behavior is the Repeat Edge Pixels checkbox (on by default).
    fx(
        "ADBE Channel Blur",
        "ec.blur.channel",
        &[
            pm(1, K::FixedSlider, "redBlurriness", Conv::Plain { valid_max: 32767.0 }),
            pm(2, K::FixedSlider, "greenBlurriness", Conv::Plain { valid_max: 32767.0 }),
            pm(3, K::FixedSlider, "blueBlurriness", Conv::Plain { valid_max: 32767.0 }),
            pm(4, K::FixedSlider, "alphaBlurriness", Conv::Plain { valid_max: 32767.0 }),
            pm(5, K::Checkbox, "repeatEdge", Conv::Checkbox),
            pm(6, K::Popup, "dimensions", Conv::Popup { default: 1, items: BLUR_DIMENSIONS }),
        ],
    ),
    // Black & White (712, 717): six float sliders (valid -200..300), Tint, Tint Color.
    fx(
        "ADBE Black&White",
        "ec.color.blackwhite",
        &[
            pm(1, K::FloatSlider, "reds", Conv::Plain { valid_max: 300.0 }),
            pm(2, K::FloatSlider, "yellows", Conv::Plain { valid_max: 300.0 }),
            pm(3, K::FloatSlider, "greens", Conv::Plain { valid_max: 300.0 }),
            pm(4, K::FloatSlider, "cyans", Conv::Plain { valid_max: 300.0 }),
            pm(5, K::FloatSlider, "blues", Conv::Plain { valid_max: 300.0 }),
            pm(6, K::FloatSlider, "magentas", Conv::Plain { valid_max: 300.0 }),
            pm(7, K::Checkbox, "tint", Conv::Checkbox),
            pm(8, K::Color, "tintColor", Conv::Color),
        ],
    ),
    // Tritone (712, 718).
    fx(
        "ADBE Tritone",
        "ec.color.tritone",
        &[
            pm(1, K::Color, "highlights", Conv::Color),
            pm(2, K::Color, "midtones", Conv::Color),
            pm(3, K::Color, "shadows", Conv::Color),
            pm(4, K::FloatSlider, "blend", Conv::Plain { valid_max: 100.0 }),
        ],
    ),
    // Luma Key (712, 719): threshold and tolerance 0..255, Edge Thin an integer to 5.
    fx(
        "ADBE Luma Key",
        "ec.key.luma",
        &[
            pm(1, K::Popup, "keyType", Conv::Popup { default: 2, items: LUMA_KEY_TYPE }),
            pm(2, K::IntSlider, "threshold", Conv::Plain { valid_max: 255.0 }),
            pm(3, K::IntSlider, "tolerance", Conv::Plain { valid_max: 255.0 }),
            pm(4, K::IntSlider, "edgeThin", Conv::Plain { valid_max: 5.0 }),
            pm(5, K::FixedSlider, "edgeFeather", Conv::Plain { valid_max: 100.0 }),
        ],
    ),
    // Simple Choker (712, 720, 721).
    fx(
        "ADBE Simple Choker",
        "ec.matte.simplechoker",
        &[pm(1, K::Popup, "view", Conv::Popup { default: 1, items: CHOKER_VIEW }), pm(2, K::FixedSlider, "chokeMatte", Conv::Plain { valid_max: 100.0 })],
    ),
    // Matte Choker (712, 722): the gray level softnesses are fractions (valid max 1, default 0.1
    // and 1), percentages here.
    fx(
        "ADBE Matte Choker",
        "ec.matte.mattechoker",
        &[
            pm(1, K::FixedSlider, "geometricSoftness1", Conv::Plain { valid_max: 100.0 }),
            pm(2, K::IntSlider, "choke1", Conv::Plain { valid_max: 127.0 }),
            pm(3, K::FixedSlider, "grayLevelSoftness1", Conv::Percent { valid_max: 1.0 }),
            pm(4, K::FixedSlider, "geometricSoftness2", Conv::Plain { valid_max: 100.0 }),
            pm(5, K::IntSlider, "choke2", Conv::Plain { valid_max: 127.0 }),
            pm(6, K::FixedSlider, "grayLevelSoftness2", Conv::Percent { valid_max: 1.0 }),
            pm(7, K::IntSlider, "iterations", Conv::Plain { valid_max: 100.0 }),
        ],
    ),
    // Transform (712, 723).
    fx(
        "ADBE Geometry2",
        "ec.distort.transform",
        &[
            pm(1, K::Point, "anchor", Conv::Point),
            pm(2, K::Point, "position", Conv::Point),
            pm(3, K::Checkbox, "uniform", Conv::Checkbox),
            pm(4, K::FixedSlider, "scaleHeight", Conv::Plain { valid_max: 30000.0 }),
            pm(5, K::FixedSlider, "scaleWidth", Conv::Plain { valid_max: 30000.0 }),
            pm(6, K::FixedSlider, "skew", Conv::Plain { valid_max: 70.0 }),
            pm(7, K::Angle, "skewAxis", Conv::Angle),
            pm(8, K::Angle, "rotation", Conv::Angle),
            pm(9, K::FixedSlider, "opacity", Conv::Plain { valid_max: 100.0 }),
            pm(10, K::Checkbox, "useCompositionShutterAngle", Conv::Checkbox),
            pm(11, K::FixedSlider, "shutterAngle", Conv::Plain { valid_max: 360.0 }),
            pm(12, K::Popup, "sampling", Conv::Popup { default: 1, items: SAMPLING }),
        ],
    ),
    // Brightness & Contrast (903, 904 E01).
    fx(
        "ADBE Brightness & Contrast 2",
        "ec.color.brightnesscontrast",
        &[
            pm(1, K::FixedSlider, "brightness", Conv::Plain { valid_max: 150.0 }),
            pm(2, K::FixedSlider, "contrast", Conv::Plain { valid_max: 100.0 }),
            pm(3, K::Checkbox, "useLegacy", Conv::Checkbox),
        ],
    ),
    // Turbulent Displace (903, 904 E02); 14 Antialiasing for Best Quality has no counterpart.
    fx(
        "ADBE Turbulent Displace",
        "ec.distort.turbulentdisplace",
        &[
            pm(1, K::Popup, "displacement", Conv::Popup { default: 1, items: TURB_DISPLACEMENT }),
            pm(2, K::FixedSlider, "amount", Conv::Plain { valid_max: 10000.0 }),
            pm(3, K::FixedSlider, "size", Conv::Plain { valid_max: 1000.0 }),
            pm(4, K::Point, "offset", Conv::Point),
            pm(5, K::FixedSlider, "complexity", Conv::Plain { valid_max: 10.0 }),
            pm(6, K::Angle, "evolution", Conv::Angle),
            pm(8, K::Checkbox, "evolutionOptions/cycleEvolution", Conv::Checkbox),
            pm(9, K::IntSlider, "evolutionOptions/cycle", Conv::Plain { valid_max: 88.0 }),
            pm(10, K::IntSlider, "evolutionOptions/randomSeed", Conv::Plain { valid_max: 100000.0 }),
            pm(12, K::Popup, "pinning", Conv::Popup { default: 3, items: TURB_PINNING }),
            pm(13, K::Checkbox, "resizeLayer", Conv::Checkbox),
        ],
    ),
    // Fractal Noise (903, 904 E03).
    fx(
        "ADBE Fractal Noise",
        "ec.noise.fractal",
        &[
            pm(1, K::Popup, "fractalType", Conv::Popup { default: 1, items: FRACTAL_TYPE }),
            pm(2, K::Popup, "noiseType", Conv::Popup { default: 3, items: NOISE_TYPE }),
            pm(3, K::Checkbox, "invert", Conv::Checkbox),
            pm(4, K::FixedSlider, "contrast", Conv::Plain { valid_max: 10000.0 }),
            pm(5, K::FixedSlider, "brightness", Conv::Plain { valid_max: 10000.0 }),
            pm(6, K::Popup, "overflow", Conv::Popup { default: 4, items: OVERFLOW }),
            pm(8, K::Angle, "transform/rotation", Conv::Angle),
            pm(9, K::Checkbox, "transform/uniformScaling", Conv::Checkbox),
            pm(10, K::FixedSlider, "transform/scale", Conv::Plain { valid_max: 10000.0 }),
            pm(11, K::FixedSlider, "transform/scaleWidth", Conv::Plain { valid_max: 10000.0 }),
            pm(12, K::FixedSlider, "transform/scaleHeight", Conv::Plain { valid_max: 10000.0 }),
            pm(13, K::Point, "transform/offset", Conv::Point),
            pm(14, K::Checkbox, "transform/perspectiveOffset", Conv::Checkbox),
            pm(16, K::FixedSlider, "complexity", Conv::Plain { valid_max: 20.0 }),
            pm(18, K::FixedSlider, "subSettings/subInfluence", Conv::Plain { valid_max: 10000.0 }),
            pm(19, K::FixedSlider, "subSettings/subScaling", Conv::Plain { valid_max: 10000.0 }),
            pm(20, K::Angle, "subSettings/subRotation", Conv::Angle),
            pm(21, K::Point, "subSettings/subOffset", Conv::Point),
            pm(22, K::Checkbox, "subSettings/centerSubscale", Conv::Checkbox),
            pm(24, K::Angle, "evolution", Conv::Angle),
            pm(26, K::Checkbox, "evolutionOptions/cycleEvolution", Conv::Checkbox),
            pm(27, K::IntSlider, "evolutionOptions/cycle", Conv::Plain { valid_max: 88.0 }),
            pm(28, K::IntSlider, "evolutionOptions/seed", Conv::Plain { valid_max: 100000.0 }),
            pm(30, K::FixedSlider, "opacity", Conv::Plain { valid_max: 100.0 }),
            pm(31, K::Popup, "blendingMode", Conv::Popup { default: 2, items: FRACTAL_BLEND }),
        ],
    ),
    // Beam (903, 904 E04): Length, Time and Softness are 0..1 fractions, percentages here.
    fx(
        "ADBE Laser",
        "ec.generate.beam",
        &[
            pm(1, K::Point, "startPoint", Conv::Point),
            pm(2, K::Point, "endPoint", Conv::Point),
            pm(3, K::FixedSlider, "length", Conv::Percent { valid_max: 1.0 }),
            pm(4, K::FixedSlider, "time", Conv::Percent { valid_max: 1.0 }),
            pm(5, K::FixedSlider, "startThickness", Conv::Plain { valid_max: 4000.0 }),
            pm(6, K::FixedSlider, "endThickness", Conv::Plain { valid_max: 4000.0 }),
            pm(7, K::FixedSlider, "softness", Conv::Percent { valid_max: 1.0 }),
            pm(8, K::Color, "insideColor", Conv::Color),
            pm(9, K::Color, "outsideColor", Conv::Color),
            pm(10, K::Checkbox, "perspective3d", Conv::Checkbox),
            pm(11, K::Checkbox, "compositeOnOriginal", Conv::Checkbox),
        ],
    ),
    // CC Radial Fast Blur (903, 904 E05).
    fx(
        "CC Radial Fast Blur",
        "ec.blur.ccradialfast",
        &[
            pm(1, K::Point, "center", Conv::Point),
            pm(2, K::FixedSlider, "amount", Conv::Plain { valid_max: 100.0 }),
            pm(3, K::Popup, "zoom", Conv::Popup { default: 1, items: FAST_ZOOM }),
        ],
    ),
    // Noise (903, 904 E06).
    fx(
        "ADBE Noise2",
        "ec.noise.noise",
        &[
            pm(1, K::FixedSlider, "amount", Conv::Plain { valid_max: 100.0 }),
            pm(2, K::Checkbox, "color", Conv::Checkbox),
            pm(3, K::Checkbox, "clip", Conv::Checkbox),
        ],
    ),
    // Levels (Individual Controls) (903, 904 E07): 1 Channel only picks the histogram shown, 2 the histogram itself (no value).
    fx(
        "ADBE Pro Levels2",
        "ec.color.levelsic",
        &[
            pm(2, K::ArbitraryData, "", Conv::Skip),
            pm(4, K::FloatSlider, "rgb/rgbInBlack", Conv::Plain { valid_max: 10000.0 }),
            pm(5, K::FloatSlider, "rgb/rgbInWhite", Conv::Plain { valid_max: 10000.0 }),
            pm(6, K::FloatSlider, "rgb/rgbGamma", Conv::Plain { valid_max: 5.0 }),
            pm(7, K::FloatSlider, "rgb/rgbOutBlack", Conv::Plain { valid_max: 10000.0 }),
            pm(8, K::FloatSlider, "rgb/rgbOutWhite", Conv::Plain { valid_max: 10000.0 }),
            pm(11, K::FloatSlider, "red/redInBlack", Conv::Plain { valid_max: 10000.0 }),
            pm(12, K::FloatSlider, "red/redInWhite", Conv::Plain { valid_max: 10000.0 }),
            pm(13, K::FloatSlider, "red/redGamma", Conv::Plain { valid_max: 5.0 }),
            pm(14, K::FloatSlider, "red/redOutBlack", Conv::Plain { valid_max: 10000.0 }),
            pm(15, K::FloatSlider, "red/redOutWhite", Conv::Plain { valid_max: 10000.0 }),
            pm(18, K::FloatSlider, "green/greenInBlack", Conv::Plain { valid_max: 10000.0 }),
            pm(19, K::FloatSlider, "green/greenInWhite", Conv::Plain { valid_max: 10000.0 }),
            pm(20, K::FloatSlider, "green/greenGamma", Conv::Plain { valid_max: 5.0 }),
            pm(21, K::FloatSlider, "green/greenOutBlack", Conv::Plain { valid_max: 10000.0 }),
            pm(22, K::FloatSlider, "green/greenOutWhite", Conv::Plain { valid_max: 10000.0 }),
            pm(25, K::FloatSlider, "blue/blueInBlack", Conv::Plain { valid_max: 10000.0 }),
            pm(26, K::FloatSlider, "blue/blueInWhite", Conv::Plain { valid_max: 10000.0 }),
            pm(27, K::FloatSlider, "blue/blueGamma", Conv::Plain { valid_max: 5.0 }),
            pm(28, K::FloatSlider, "blue/blueOutBlack", Conv::Plain { valid_max: 10000.0 }),
            pm(29, K::FloatSlider, "blue/blueOutWhite", Conv::Plain { valid_max: 10000.0 }),
            pm(32, K::FloatSlider, "alpha/alphaInBlack", Conv::Plain { valid_max: 10000.0 }),
            pm(33, K::FloatSlider, "alpha/alphaInWhite", Conv::Plain { valid_max: 10000.0 }),
            pm(34, K::FloatSlider, "alpha/alphaGamma", Conv::Plain { valid_max: 5.0 }),
            pm(35, K::FloatSlider, "alpha/alphaOutBlack", Conv::Plain { valid_max: 10000.0 }),
            pm(36, K::FloatSlider, "alpha/alphaOutWhite", Conv::Plain { valid_max: 10000.0 }),
            pm(38, K::Popup, "clipToOutputBlack", Conv::Popup { default: 3, items: LEVELS_CLIP }),
            pm(39, K::Popup, "clipToOutputWhite", Conv::Popup { default: 3, items: LEVELS_CLIP }),
        ],
    ),
    // Corner Pin (903, 904 E08).
    fx(
        "ADBE Corner Pin",
        "ec.distort.cornerpin",
        &[pm(1, K::Point, "ul", Conv::Point), pm(2, K::Point, "ur", Conv::Point), pm(3, K::Point, "ll", Conv::Point), pm(4, K::Point, "lr", Conv::Point)],
    ),
    // Grid (903, 904 E09).
    fx(
        "ADBE Grid",
        "ec.generate.grid",
        &[
            pm(1, K::Point, "anchor", Conv::Point),
            pm(2, K::Popup, "sizeFrom", Conv::Popup { default: 1, items: GRID_SIZE_FROM }),
            pm(3, K::Point, "corner", Conv::Point),
            pm(4, K::FixedSlider, "width", Conv::Plain { valid_max: 4000.0 }),
            pm(5, K::FixedSlider, "height", Conv::Plain { valid_max: 4000.0 }),
            pm(6, K::FixedSlider, "border", Conv::Plain { valid_max: 2000.0 }),
            pm(8, K::FixedSlider, "feather/featherWidth", Conv::Plain { valid_max: 400.0 }),
            pm(9, K::FixedSlider, "feather/featherHeight", Conv::Plain { valid_max: 400.0 }),
            pm(11, K::Checkbox, "invertGrid", Conv::Checkbox),
            pm(12, K::Color, "color", Conv::Color),
            pm(13, K::FixedSlider, "opacity", Conv::Plain { valid_max: 100.0 }),
            pm(14, K::Popup, "blendingMode", Conv::Popup { default: 1, items: GRID_BLEND }),
        ],
    ),
    // Sharpen (903, 904 E10).
    fx("ADBE Sharpen", "ec.blur.sharpen", &[pm(1, K::IntSlider, "amount", Conv::Plain { valid_max: 4000.0 })]),
    // Linear Color Key (903, 904 E11); 1 Preview has no data.
    fx(
        "ADBE Linear Color Key2",
        "ec.key.linearcolor",
        &[
            pm(2, K::Popup, "view", Conv::Popup { default: 1, items: KEY_VIEW }),
            pm(3, K::Color, "keyColor", Conv::Color),
            pm(4, K::Popup, "matchColors", Conv::Popup { default: 1, items: MATCH_COLORS }),
            pm(5, K::FixedSlider, "tolerance", Conv::Plain { valid_max: 100.0 }),
            pm(6, K::FixedSlider, "softness", Conv::Plain { valid_max: 100.0 }),
            pm(7, K::Popup, "keyOperation", Conv::Popup { default: 1, items: KEY_OPERATION }),
        ],
    ),
    // Radial Wipe (903, 904 E12).
    fx(
        "ADBE Radial Wipe",
        "ec.transition.radialwipe",
        &[
            pm(1, K::FixedSlider, "completion", Conv::Plain { valid_max: 100.0 }),
            pm(2, K::Angle, "startAngle", Conv::Angle),
            pm(3, K::Point, "center", Conv::Point),
            pm(4, K::Popup, "wipe", Conv::Popup { default: 1, items: WIPE }),
            pm(5, K::FixedSlider, "feather", Conv::Plain { valid_max: 32000.0 }),
        ],
    ),
    // Remove Color Matting (903, 904 E13).
    fx(
        "ADBE Remove Color Matting",
        "ec.channel.removecolormatting",
        &[pm(1, K::Color, "backgroundColor", Conv::Color), pm(2, K::Checkbox, "clipHdr", Conv::Checkbox)],
    ),
    // CC Bend It (903, 904 E14): Bend is a slider (valid max 250) in After Effects; 5 Distort has no counterpart.
    fx(
        "CC Bend It",
        "ec.distort.ccbendit",
        &[
            pm(1, K::FixedSlider, "bend", Conv::Plain { valid_max: 250.0 }),
            pm(2, K::Point, "start", Conv::Point),
            pm(3, K::Point, "end", Conv::Point),
            pm(4, K::Popup, "renderPrestart", Conv::Popup { default: 3, items: BEND_PRESTART }),
        ],
    ),
    // CC Power Pin (903, 904 E15): Perspective is a 0..1 fraction.
    fx(
        "CC Power Pin",
        "ec.distort.ccpowerpin",
        &[
            pm(1, K::Point, "topLeft", Conv::Point),
            pm(2, K::Point, "topRight", Conv::Point),
            pm(3, K::Point, "bottomLeft", Conv::Point),
            pm(4, K::Point, "bottomRight", Conv::Point),
            pm(5, K::FixedSlider, "perspective", Conv::Percent { valid_max: 1.0 }),
            pm(6, K::Checkbox, "unstretch", Conv::Checkbox),
            pm(8, K::FixedSlider, "expandTop", Conv::Plain { valid_max: 200.0 }),
            pm(9, K::FixedSlider, "expandLeft", Conv::Plain { valid_max: 200.0 }),
            pm(10, K::FixedSlider, "expandRight", Conv::Plain { valid_max: 200.0 }),
            pm(11, K::FixedSlider, "expandBottom", Conv::Plain { valid_max: 200.0 }),
        ],
    ),
    // 4-Color Gradient (903, 904 E16).
    fx(
        "ADBE 4ColorGradient",
        "ec.generate.fourcolor",
        &[
            pm(2, K::Point, "positionsColors/point1", Conv::Point),
            pm(3, K::Color, "positionsColors/color1", Conv::Color),
            pm(4, K::Point, "positionsColors/point2", Conv::Point),
            pm(5, K::Color, "positionsColors/color2", Conv::Color),
            pm(6, K::Point, "positionsColors/point3", Conv::Point),
            pm(7, K::Color, "positionsColors/color3", Conv::Color),
            pm(8, K::Point, "positionsColors/point4", Conv::Point),
            pm(9, K::Color, "positionsColors/color4", Conv::Color),
            pm(11, K::FixedSlider, "blend", Conv::Plain { valid_max: 10000.0 }),
            pm(12, K::FixedSlider, "jitter", Conv::Plain { valid_max: 500.0 }),
            pm(13, K::FixedSlider, "opacity", Conv::Plain { valid_max: 100.0 }),
            pm(14, K::Popup, "blendingMode", Conv::Popup { default: 1, items: FOUR_COLOR_BLEND }),
        ],
    ),
    // Change Color (903, 904 E17).
    fx(
        "ADBE Change Color",
        "ec.color.changecolor",
        &[
            pm(1, K::Popup, "view", Conv::Popup { default: 1, items: CHANGE_VIEW }),
            pm(2, K::FixedSlider, "hueTransform", Conv::Plain { valid_max: 1800.0 }),
            pm(3, K::FixedSlider, "lightnessTransform", Conv::Plain { valid_max: 100.0 }),
            pm(4, K::FixedSlider, "saturationTransform", Conv::Plain { valid_max: 100.0 }),
            pm(5, K::Color, "colorToChange", Conv::Color),
            pm(6, K::FixedSlider, "matchingTolerance", Conv::Plain { valid_max: 100.0 }),
            pm(7, K::FixedSlider, "matchingSoftness", Conv::Plain { valid_max: 100.0 }),
            pm(8, K::Popup, "matchColors", Conv::Popup { default: 1, items: MATCH_COLORS }),
            pm(9, K::Checkbox, "invertColorCorrectionMask", Conv::Checkbox),
        ],
    ),
    // Extract (903, 904 E18); 1 Histogram has no data.
    fx(
        "ADBE Extract",
        "ec.key.extract",
        &[
            pm(2, K::Popup, "channel", Conv::Popup { default: 1, items: EXTRACT_CHANNEL }),
            pm(3, K::IntSlider, "blackPoint", Conv::Plain { valid_max: 255.0 }),
            pm(4, K::IntSlider, "whitePoint", Conv::Plain { valid_max: 255.0 }),
            pm(5, K::IntSlider, "blackSoftness", Conv::Plain { valid_max: 255.0 }),
            pm(6, K::IntSlider, "whiteSoftness", Conv::Plain { valid_max: 255.0 }),
            pm(7, K::Checkbox, "invert", Conv::Checkbox),
        ],
    ),
    // Noise HLS (903, 904 E19).
    fx(
        "ADBE Noise HLS2",
        "ec.noise.noisehls",
        &[
            pm(1, K::Popup, "noise", Conv::Popup { default: 1, items: NOISE_HLS }),
            pm(2, K::FloatSlider, "hue", Conv::Plain { valid_max: 1000.0 }),
            pm(3, K::FloatSlider, "lightness", Conv::Plain { valid_max: 1000.0 }),
            pm(4, K::FloatSlider, "saturation", Conv::Plain { valid_max: 1000.0 }),
            pm(5, K::FixedSlider, "grainSize", Conv::Plain { valid_max: 10.0 }),
            pm(6, K::Angle, "noisePhase", Conv::Angle),
        ],
    ),
    // Scatter (903, 904 E20).
    fx(
        "ADBE Scatter",
        "ec.stylize.scatter",
        &[
            pm(1, K::FixedSlider, "amount", Conv::Plain { valid_max: 1000.0 }),
            pm(2, K::Popup, "grain", Conv::Popup { default: 1, items: SCATTER_GRAIN }),
            pm(3, K::Checkbox, "randomizeEveryFrame", Conv::Checkbox),
        ],
    ),
    // CC Light Sweep (903, 904 E21).
    fx(
        "CC Light Sweep",
        "ec.generate.cclightsweep",
        &[
            pm(1, K::Point, "center", Conv::Point),
            pm(2, K::Angle, "direction", Conv::Angle),
            pm(3, K::Popup, "shape", Conv::Popup { default: 3, items: SWEEP_SHAPE }),
            pm(4, K::FixedSlider, "width", Conv::Plain { valid_max: 4000.0 }),
            pm(5, K::FixedSlider, "sweepIntensity", Conv::Plain { valid_max: 500.0 }),
            pm(6, K::FixedSlider, "edgeIntensity", Conv::Plain { valid_max: 500.0 }),
            pm(7, K::FixedSlider, "edgeThickness", Conv::Plain { valid_max: 20.0 }),
            pm(8, K::Color, "lightColor", Conv::Color),
            pm(9, K::Popup, "lightReceptionMode", Conv::Popup { default: 1, items: SWEEP_RECEPTION }),
        ],
    ),
    // Compound Blur (903, 904 E22): an unset Blur Layer is the layer itself in both programs.
    fx(
        "ADBE Compound Blur",
        "ec.blur.compound",
        &[
            pm(1, K::Layer, "blurLayer", Conv::LayerRef { own: false }),
            pm(2, K::FixedSlider, "maximumBlur", Conv::Plain { valid_max: 4000.0 }),
            pm(3, K::Checkbox, "stretchMapToFit", Conv::Checkbox),
            pm(4, K::Checkbox, "invertBlur", Conv::Checkbox),
        ],
    ),
    // Displacement Map (903, 904 E23).
    fx(
        "ADBE Displacement Map",
        "ec.distort.displacementmap",
        &[
            pm(1, K::Layer, "displacementMapLayer", Conv::LayerRef { own: false }),
            pm(2, K::Popup, "useForHorizontal", Conv::Popup { default: 1, items: DISPLACE_USE }),
            pm(3, K::FixedSlider, "maxHorizontal", Conv::Plain { valid_max: 32000.0 }),
            pm(4, K::Popup, "useForVertical", Conv::Popup { default: 2, items: DISPLACE_USE }),
            pm(5, K::FixedSlider, "maxVertical", Conv::Plain { valid_max: 32000.0 }),
            pm(6, K::Popup, "displacementMapBehavior", Conv::Popup { default: 1, items: DISPLACE_BEHAVIOR }),
            pm(7, K::Checkbox, "wrapPixelsAround", Conv::Checkbox),
            pm(8, K::Checkbox, "expandOutput", Conv::Checkbox),
        ],
    ),
    // Set Matte (903, 904 E24): an unset Take Matte From Layer is the layer itself.
    fx(
        "ADBE Set Matte3",
        "ec.channel.setmatte",
        &[
            pm(1, K::Layer, "takeMatteFromLayer", Conv::LayerRef { own: false }),
            pm(2, K::Popup, "takeMatteFrom", Conv::Popup { default: 4, items: MATTE_USE }),
            pm(3, K::Checkbox, "invertMatte", Conv::Checkbox),
            pm(4, K::Checkbox, "stretchMatteToFit", Conv::Checkbox),
            pm(5, K::Checkbox, "compositeMatteWithOriginal", Conv::Checkbox),
            pm(6, K::Checkbox, "premultiplyMatteLayer", Conv::Checkbox),
        ],
    ),
    // Gaussian Blur (Legacy) (903).
    fx(
        "ADBE Gaussian Blur",
        "ec.obsolete.gaussianlegacy",
        &[
            pm(1, K::FixedSlider, "blurriness", Conv::Plain { valid_max: 30000.0 }),
            pm(2, K::Popup, "blurDimensions", Conv::Popup { default: 1, items: BLUR_DIMENSIONS }),
        ],
    ),
    // Pixel Motion Blur (903).
    fx(
        "ADBE OFMotionBlur",
        "ec.time.pixelmotionblur",
        &[
            pm(1, K::Popup, "shutterControl", Conv::Popup { default: 1, items: SHUTTER_CONTROL }),
            pm(2, K::FloatSlider, "shutterAngle", Conv::Plain { valid_max: 5000.0 }),
            pm(3, K::IntSlider, "shutterSamples", Conv::Plain { valid_max: 256.0 }),
            pm(4, K::FloatSlider, "vectorDetail", Conv::Plain { valid_max: 100.0 }),
        ],
    ),
    // Layer Control (076, 220, 320, 321): the default is the owning layer, None is unset.
    fx("ADBE Layer Control", "ec.control.layer", &[pm(1, K::Layer, "layer", Conv::LayerRef { own: true })]),
    fx("ADBE Slider Control", "ec.control.slider", &[pm(1, K::FloatSlider, "slider", Conv::Plain { valid_max: 1_000_000.0 })]),
    fx("ADBE Angle Control", "ec.control.angle", &[pm(1, K::Angle, "angle", Conv::Angle)]),
    fx("ADBE Checkbox Control", "ec.control.checkbox", &[pm(1, K::Checkbox, "checkbox", Conv::Checkbox)]),
    fx("ADBE Color Control", "ec.control.color", &[pm(1, K::Color, "color", Conv::Color)]),
    fx("ADBE Point Control", "ec.control.point", &[pm(1, K::Point, "point", Conv::Point)]),
];

/// Exposure's three sliders, by their valid maximum in the file (389).
pub const EXPOSURE: Conv = Conv::Plain { valid_max: 100.0 };
pub const OFFSET: Conv = Conv::Plain { valid_max: 2.0 };
pub const GAMMA: Conv = Conv::Plain { valid_max: 10.0 };
/// The valid maximum of Glow's A & B Midpoint (16.16 of 0.99, 427).
pub const GLOW_MIDPOINT_MAX: f64 = 0.990_005_493_164_062_5;

/// The key a popup item is matched by between the two programs: trimmed, lower case, and
/// Set Matte's "Red Channel" ... "Alpha Channel" are EffectCraft's "Red" ... "Alpha" (904 E24).
pub fn popup_key(item: &str) -> String {
    let s = item.trim().to_ascii_lowercase();
    s.strip_suffix(" channel").map(str::to_string).unwrap_or(s)
}

/// The table row of an After Effects match name.
pub fn by_match_name(match_name: &str) -> Option<&'static EffectMap> {
    EFFECTS.iter().find(|m| m.match_name == match_name)
}

/// The table row of an EffectCraft effect id.
pub fn by_effect(effect: &str) -> Option<&'static EffectMap> {
    EFFECTS.iter().find(|m| m.effect == effect)
}

/// The EffectCraft parameters Hue/Saturation's master values (`Extra::HueSaturation`) go to,
/// in the order of the data (hue, saturation, lightness: 444-446).
pub const HUE_SATURATION_MASTER: [&str; 3] = ["hue", "saturation", "lightness"];

/// The EffectCraft parameters Levels' RGB channel values (`Extra::Levels`) go to, in the order of
/// the data (input black, input white, gamma, output black, output white: 512-516).
pub const LEVELS_RGB: [&str; 5] = ["inBlack", "inWhite", "gamma", "outBlack", "outWhite"];
