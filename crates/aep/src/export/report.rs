//! The export report: what was written (counts) and everything that was not, per category.
//!
//! Plain data (`serde`), so it round-trips through JSON: the engine command returns it and agents
//! read it. Nothing the exporter leaves out is missing from it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Most example lines kept per loss category (the count is always exact).
pub const MAX_DETAILS: usize = 20;

/// Why a piece of the project did not arrive in the `.aep`, or arrived changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LossCategory {
    /// Effects on layers that are not written (not in the table, switched off, a parameter
    /// without a counterpart, an expression on a parameter).
    Effects,
    /// Mask data that is not written: a disabled mask or one without a usable path, keys on Mask
    /// Opacity or Expansion, expressions on mask properties, the lock, mask motion blur other than
    /// Same As Layer, Linear feather falloff, RotoBezier and variable-width feather points.
    Masks,
    /// Layer and composition markers.
    Markers,
    /// Text layer features that are not written (animators, Source Text keys, strokes, ...), and
    /// text layers without a text document.
    TextLayers,
    /// Shape layers (skipped as a whole).
    ShapeLayers,
    /// Camera layers (skipped as a whole).
    Cameras,
    /// Light layers (skipped as a whole).
    Lights,
    /// 3D model and primitive layers (skipped as a whole).
    ModelLayers,
    /// Footage items that are not written (a file type without a prototype, a layered file's
    /// layer, a sequence with gaps or at a fractional rate), and items in a folder loop.
    FootageItems,
    /// Layers whose source is not exported (footage that is not written, a precomposition that is
    /// missing or closes a loop, other).
    UnsupportedLayers,
    /// Blend modes other than Normal, Multiply, Add, Screen, Overlay and Difference.
    BlendModes,
    /// Track mattes.
    TrackMattes,
    /// 3D features that are not written (per-character 3D of a text layer; the layer's 3D switch
    /// itself is written).
    ThreeD,
    /// A time stretch that is not an exact small ratio (the layer is written unstretched).
    TimeStretch,
    /// Transform properties other than Anchor Point, Position, Scale, Rotation and Opacity.
    TransformProperties,
    /// Expression on a property that is not exported.
    Expressions,
    /// Keyframes written in a simpler form (see the detail lines).
    ApproximatedKeyframes,
    /// A value that is not a finite number or beyond the range of the format (default written).
    InvalidValues,
    /// Frame rate that the format cannot hold exactly, composition size or duration out of range.
    CompositionSettings,
    /// Parent links to a layer that was not exported.
    ParentLinks,
    /// Times rounded to the composition's time base.
    RoundedTimes,
    /// Composition options that are not exported (guides, essential graphics, advanced renderer).
    CompositionOptions,
    /// Names longer than the format's name field (the name is kept in full where verified).
    Names,
    /// Layer timing After Effects does not keep: a layer of a clip, sequence or sound that runs
    /// past the end of its source (After Effects ends it there).
    LayerTiming,
    /// Project settings that are not written: a working space other than None and sRGB, the OCIO
    /// colour engine, HDR handling and output space, a frame count start other than 0 and 1.
    ProjectSettings,
    /// Time remapping on a layer After Effects cannot remap (a solid, a text): not written.
    TimeRemapping,
    /// Layer-style data that is not written: a style or Layer Styles switched off, edited
    /// gradients, keys on colours, points, popups and checkboxes, Knockout.
    LayerStyles,
}

impl LossCategory {
    /// A short readable name.
    pub fn label(self) -> &'static str {
        match self {
            LossCategory::Effects => "effects",
            LossCategory::Masks => "masks",
            LossCategory::Markers => "markers",
            LossCategory::TextLayers => "text layers",
            LossCategory::ShapeLayers => "shape layers",
            LossCategory::Cameras => "cameras",
            LossCategory::Lights => "lights",
            LossCategory::ModelLayers => "3D model layers",
            LossCategory::FootageItems => "footage items",
            LossCategory::UnsupportedLayers => "unsupported layers",
            LossCategory::BlendModes => "blend modes",
            LossCategory::TrackMattes => "track mattes",
            LossCategory::ThreeD => "3D layers",
            LossCategory::TimeStretch => "time stretch",
            LossCategory::TransformProperties => "transform properties",
            LossCategory::Expressions => "expressions",
            LossCategory::ApproximatedKeyframes => "approximated keyframes",
            LossCategory::InvalidValues => "invalid values",
            LossCategory::CompositionSettings => "composition settings",
            LossCategory::ParentLinks => "parent links",
            LossCategory::RoundedTimes => "rounded times",
            LossCategory::CompositionOptions => "composition options",
            LossCategory::Names => "names",
            LossCategory::LayerTiming => "layer timing",
            LossCategory::ProjectSettings => "project settings",
            LossCategory::TimeRemapping => "time remapping",
            LossCategory::LayerStyles => "layer styles",
        }
    }
}

/// One category of things that were not exported.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Loss {
    pub category: LossCategory,
    /// How many occurrences (exact).
    pub count: usize,
    /// Up to [`MAX_DETAILS`] example lines.
    pub details: Vec<String>,
}

/// What was written to the file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Exported {
    pub folders: usize,
    pub compositions: usize,
    pub solids: usize,
    /// All layers.
    pub layers: usize,
    pub solid_layers: usize,
    pub null_layers: usize,
    pub adjustment_layers: usize,
    /// Transform properties written with a value that differs from the default.
    pub properties: usize,
    /// Keyframes written.
    pub keyframes: usize,
    /// Expressions written (source text and enable switch).
    pub expressions: usize,
    /// Layer masks written.
    #[serde(default)]
    pub masks: usize,
    /// Precomposition layers (a composition shown as a layer).
    #[serde(default)]
    pub precomp_layers: usize,
    /// Effect instances written.
    #[serde(default)]
    pub effects: usize,
    /// Footage items (files) written.
    #[serde(default)]
    pub footage: usize,
    /// Footage layers written.
    #[serde(default)]
    pub footage_layers: usize,
    /// Text layers written.
    #[serde(default)]
    pub text_layers: usize,
    /// Layer and composition markers written.
    #[serde(default)]
    pub markers: usize,
    /// Camera layers written.
    #[serde(default)]
    pub cameras: usize,
    /// Light layers written.
    #[serde(default)]
    pub lights: usize,
    /// Shape layers written.
    #[serde(default)]
    pub shape_layers: usize,
    /// Layers written with an enabled Time Remap.
    #[serde(default)]
    pub time_remaps: usize,
    /// Text animators written.
    #[serde(default)]
    pub text_animators: usize,
    /// Layer styles written (one per style on a layer).
    #[serde(default)]
    pub layer_styles: usize,
}

/// Standing limitations of the exporter (what it cannot write at all), listed in every report.
pub const LIMITATIONS: &[&str] = &[
    "The file is built from a generated template project: the render queue, render templates and project settings other than colour depth, working space (None or sRGB), working gamma, the two linear-light switches, time display and frame count start are not exported.",
    "Composition options other than size, frame rate, duration, work area, background, pixel aspect, shutter and display start are not exported (draft 3D, preserve frame rate/resolution, hide shy, frame blending, resolution).",
    "Footage is linked, not embedded: png, jpeg, OpenEXR, TIFF, Targa, Photoshop, Illustrator and EPS stills, png, OpenEXR and jpeg sequences, mp4 and mov clips and WAV audio are written with their path, size, duration and frame rate (a conformed clip rate too); the rest of their interpretation (alpha, colour management, fields, loops) is the default.",
    "Frame blending, layer comments, a 3D layer's X / Y rotation, orientation and material options, parallel and ambient lights, and shape items other than groups, rectangles, ellipses, paths, fills and strokes are not written.",
    "Mask colours are not written: After Effects shows its own colour for every exported mask.",
];

/// The result of planning/writing an `.aep`: counts of what was written, the losses per category
/// and the standing limitations of the exporter.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReport {
    pub exported: Exported,
    /// Only categories that occurred, in category order.
    pub losses: Vec<Loss>,
    pub limitations: Vec<String>,
}

impl ExportReport {
    /// The sum of all loss counts.
    pub fn total_losses(&self) -> usize {
        self.losses.iter().map(|l| l.count).sum()
    }

    /// True when nothing was lost.
    pub fn is_lossless(&self) -> bool {
        self.losses.is_empty()
    }

    /// The loss count of a category.
    pub fn count(&self, category: LossCategory) -> usize {
        self.losses.iter().find(|l| l.category == category).map_or(0, |l| l.count)
    }

    /// `effects: 3, masks: 1` (empty when lossless).
    pub fn summary(&self) -> String {
        self.losses.iter().map(|l| format!("{}: {}", l.category.label(), l.count)).collect::<Vec<_>>().join(", ")
    }
}

/// Collects losses while planning.
#[derive(Default)]
pub(crate) struct Collector {
    pub exported: Exported,
    losses: BTreeMap<LossCategory, Loss>,
}

impl Collector {
    /// Records one loss.
    pub fn lose(&mut self, category: LossCategory, detail: impl Into<String>) {
        let loss = self.losses.entry(category).or_insert_with(|| Loss { category, count: 0, details: Vec::new() });
        loss.count = loss.count.saturating_add(1);
        if loss.details.len() < MAX_DETAILS {
            loss.details.push(detail.into());
        }
    }

    pub fn finish(self) -> ExportReport {
        ExportReport {
            exported: self.exported,
            losses: self.losses.into_values().collect(),
            limitations: LIMITATIONS.iter().map(|s| (*s).to_string()).collect(),
        }
    }
}
