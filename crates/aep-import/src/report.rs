//! The import report: what was imported (counts) and everything that was not, per category.
//!
//! The report is plain data (`serde`), so it round-trips through JSON: the engine command returns it
//! and agents read it. Nothing the importer skips is left out of it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Most example lines kept per loss category (the count is always exact).
pub const MAX_DETAILS: usize = 20;

/// Why a piece of the file did not arrive in the project, or arrived changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LossCategory {
    /// Effects on layers that are not imported (by match name), and parameters of imported
    /// effects that are not.
    Effects,
    /// Mask data that is not imported: a mask whose path cannot be used (none, unreadable, empty,
    /// not finite) and an unknown mask mode (imported as Add).
    Masks,
    /// Shape layers (skipped as a whole).
    ShapeLayers,
    /// Parts of text layers that are not imported: text animators, Path Options and More Options
    /// with set values, Source Text keyframes and expressions, undecoded character and paragraph
    /// fields that differ from the default, unknown capitalisation or justification codes, control
    /// characters of unknown meaning. (The layers themselves are imported.)
    TextLayers,
    /// Camera layers (skipped as a whole).
    Cameras,
    /// Light layers (skipped as a whole).
    Lights,
    /// Layer styles (drop shadow, glow, stroke, ...) with set values.
    LayerStyles,
    /// Layer and composition markers.
    Markers,
    /// Blend modes other than Normal, Multiply, Add, Screen, Overlay and Difference.
    BlendModes,
    /// Stored transform properties other than Anchor Point, Position, Scale, Rotation and Opacity
    /// (3D rotations, orientation, separated position dimensions).
    TransformProperties,
    /// Time remapping.
    TimeRemapping,
    /// Keyframe data that was imported in a simplified form (see the detail lines).
    ApproximatedKeyframes,
    /// Parent links to a layer that was not imported (or that would form a loop).
    ParentLinks,
    /// Track mattes that are not imported: an unknown matte mode, a matte layer that was not
    /// imported or does not exist, a layer matted by itself.
    TrackMattes,
    /// Layers that could not be imported for another reason (unknown layer type, missing or
    /// unsupported source, precomposition loop).
    UnsupportedLayers,
    /// Project items of a kind that is not imported.
    UnsupportedItems,
    /// Data that could not be read (damaged headers, unreadable keyframes).
    DamagedData,
    /// Project settings without an EffectCraft counterpart (a working space other than None,
    /// sRGB and Rec. 709).
    ProjectSettings,
}

impl LossCategory {
    /// A short readable name.
    pub fn label(self) -> &'static str {
        match self {
            LossCategory::Effects => "effects",
            LossCategory::Masks => "masks",
            LossCategory::ShapeLayers => "shape layers",
            LossCategory::TextLayers => "text layer features",
            LossCategory::LayerStyles => "layer styles",
            LossCategory::Cameras => "cameras",
            LossCategory::Lights => "lights",
            LossCategory::Markers => "markers",
            LossCategory::BlendModes => "blend modes",
            LossCategory::TransformProperties => "transform properties",
            LossCategory::TimeRemapping => "time remapping",
            LossCategory::ApproximatedKeyframes => "approximated keyframes",
            LossCategory::ParentLinks => "parent links",
            LossCategory::TrackMattes => "track mattes",
            LossCategory::UnsupportedLayers => "unsupported layers",
            LossCategory::UnsupportedItems => "unsupported items",
            LossCategory::DamagedData => "damaged data",
            LossCategory::ProjectSettings => "project settings",
        }
    }
}

/// One category of things that were not imported.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Loss {
    pub category: LossCategory,
    /// How many occurrences (exact).
    pub count: usize,
    /// Up to [`MAX_DETAILS`] example lines.
    pub details: Vec<String>,
}

/// What arrived in the project.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Imported {
    pub folders: usize,
    pub compositions: usize,
    pub solids: usize,
    /// Footage items (files), present or missing.
    pub footage: usize,
    /// All layers.
    pub layers: usize,
    pub solid_layers: usize,
    pub null_layers: usize,
    pub adjustment_layers: usize,
    pub footage_layers: usize,
    pub precomp_layers: usize,
    /// Text layers (with their Source Text).
    #[serde(default)]
    pub text_layers: usize,
    /// Keyframes on the imported properties.
    pub keyframes: usize,
    /// Expressions (kept as source text with their enable switch).
    pub expressions: usize,
    /// Layer masks.
    #[serde(default)]
    pub masks: usize,
    /// Track mattes (the layer's matte source and mode).
    #[serde(default)]
    pub track_mattes: usize,
    /// Layer and composition markers.
    #[serde(default)]
    pub markers: usize,
    /// Layers with Time Remapping.
    #[serde(default)]
    pub time_remaps: usize,
    /// Shape layers.
    #[serde(default)]
    pub shape_layers: usize,
    /// Shape contents items (groups, paths, paints, operators).
    #[serde(default)]
    pub shape_items: usize,
    /// Camera layers.
    #[serde(default)]
    pub cameras: usize,
    /// Light layers (spot and point).
    #[serde(default)]
    pub lights: usize,
    /// Text animators (with their range selectors).
    #[serde(default)]
    pub text_animators: usize,
    /// Effect instances (with their parameters; the parameters that could not be imported are
    /// in [`LossCategory::Effects`]).
    #[serde(default)]
    pub effects: usize,
    /// Layer styles (Drop Shadow, Stroke, ...), one per style on a layer.
    #[serde(default)]
    pub layer_styles: usize,
}

/// The result of reading an `.aep`: counts of what was imported, the losses per category, the
/// footage files that could not be found and the standing limitations of the reader.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub imported: Imported,
    /// Only categories that occurred, in category order.
    pub losses: Vec<Loss>,
    /// Footage files whose path does not exist here; the items are kept as missing-footage
    /// placeholders (relink them with File > Replace Footage / Relink).
    pub missing_footage: Vec<String>,
    /// Fonts (PostScript names) that the text uses and that are not installed here. The text keeps
    /// the name and is drawn with the fallback font until the font is installed; this is not a
    /// loss.
    #[serde(default)]
    pub missing_fonts: Vec<String>,
    /// What the file reader cannot see at all, so it can be neither imported nor counted.
    pub limitations: Vec<String>,
}

impl ImportReport {
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

/// Collects losses while importing.
#[derive(Default)]
pub(crate) struct Collector {
    pub imported: Imported,
    losses: BTreeMap<LossCategory, Loss>,
    pub missing_footage: Vec<String>,
    pub missing_fonts: Vec<String>,
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

    pub fn finish(self) -> ImportReport {
        ImportReport {
            imported: self.imported,
            losses: self.losses.into_values().collect(),
            missing_footage: self.missing_footage,
            missing_fonts: self.missing_fonts,
            limitations: LIMITATIONS.iter().map(|s| (*s).to_string()).collect(),
        }
    }
}

/// Parts of a project that the container reader does not decode. They are never imported and, as
/// the reader cannot see them, never counted either.
pub const LIMITATIONS: &[&str] = &[
    "Frame blending, 3D material options, audio levels, guides are not decoded and not imported.",
    "Mask colour, lock, feather falloff, motion blur and RotoBezier are not decoded: imported masks get the default colour, unlocked, Smooth falloff, motion blur Same As Layer and no RotoBezier.",
    "Layer styles are not decoded: a layer with set layer-style values is reported (layer styles), the styles are not imported.",
    "Text: per-character 3D, text on a path and several style slots of unknown meaning are not decoded (stored values that differ from the default are reported as text layer features).",
    "Footage that is not a solid is linked by its stored path; its duration and frame rate are not decoded (use Reload Footage after the files are found).",
    "The per-effect on/off switch is not decoded: imported effects are switched on.",
];
