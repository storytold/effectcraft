//! The export report: what was written (counts) and everything that was not, per category.
//!
//! Plain data (`serde`), so it round-trips through JSON: the engine command returns it and agents
//! read it. Nothing the exporter leaves out is missing from it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Most example lines kept per loss category (the count is always exact); the same as the import
/// report's.
pub const MAX_DETAILS: usize = crate::import::MAX_DETAILS;

/// Why a piece of the project did not arrive in the `.aep`, or arrived changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LossCategory {
    /// Effects on layers that are not written (not in the table, a parameter
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
    /// Shape layer contents that are not written.
    ShapeLayers,
    /// Camera settings that are not written, Auto-Orient Along Path or Towards Camera included.
    Cameras,
    /// Light settings that are not written, a spot light not oriented towards its point of
    /// interest included.
    Lights,
    /// 3D model and primitive layers (skipped as a whole).
    ModelLayers,
    /// Footage items that are not written (a file type without a prototype, a layered file's
    /// layer, a sequence with gaps or at a fractional rate), and items in a folder loop.
    FootageItems,
    /// Layers whose source is not exported (footage that is not written, a precomposition that is
    /// missing or closes a loop, other).
    UnsupportedLayers,
    /// Blend modes After Effects has no code for (written as Normal).
    BlendModes,
    /// Track mattes.
    TrackMattes,
    /// 3D features that are not written (per-character 3D of a text layer, Material Options other
    /// than the defaults; the layer's 3D switch itself is written).
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
    /// Composition options that are not exported (guides, essential graphics, advanced renderer),
    /// and settings that change rendered frames but are written at After Effects' defaults:
    /// Preserve Frame Rate or Preserve Resolution switched on, motion blur samples per frame and
    /// adaptive sample limit other than 16 and 128.
    CompositionOptions,
    /// Names longer than the format's name field (the name is kept in full where verified), and
    /// item comments (not written).
    Names,
    /// Layer timing After Effects does not keep: a layer of a clip, sequence or sound that runs
    /// past the end of its source (After Effects ends it there).
    LayerTiming,
    /// Project settings that are not written: a working space other than None, sRGB and Rec.709
    /// Gamma 2.4, the OCIO
    /// colour engine, HDR handling and output space, a frame count start other than 0 and 1.
    ProjectSettings,
    /// Time remapping on a layer After Effects cannot remap (a solid, a text): not written.
    TimeRemapping,
    /// Layer-style data that is not written: a style or Layer Styles switched off, edited
    /// gradients, keys on colours, points, popups and checkboxes, Knockout.
    LayerStyles,
    /// Layer switches and options that are not written: Wireframe quality, Bicubic sampling,
    /// frame blending, Preserve Transparency, Auto-Orient on a layer other than a camera or light
    /// (theirs is reported as [`Cameras`](Self::Cameras) / [`Lights`](Self::Lights)), an
    /// environment layer, locked markers.
    LayerSwitches,
    /// Render queue items (the render queue is not written).
    RenderQueue,
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
            LossCategory::LayerSwitches => "layer switches",
            LossCategory::RenderQueue => "render queue",
        }
    }
}

/// One category of things that were not exported: the import report's loss entry with the
/// export categories.
pub type Loss = crate::import::Loss<LossCategory>;

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
    "The render queue (its items are reported), render templates and project settings other than colour depth, working space (None, sRGB or Rec.709 Gamma 2.4), working gamma, the two linear-light switches, time display and frame count start are not exported.",
    "Composition options other than size, frame rate, duration, work area, background, pixel aspect, shutter, motion blur switch and display start are written at After Effects' defaults: other values of preserve frame rate, preserve resolution, motion blur samples and adaptive sample limit are reported; draft 3D, hide shy, the viewer resolution, the poster time and the composition's frame blending switch (layer frame blending is reported, and no layer blends without it) are not exported. The background is stored with 8 bits per channel.",
    "Footage is linked, not embedded: png, jpeg, OpenEXR, TIFF, Targa, Photoshop, Illustrator and EPS stills, png, OpenEXR and jpeg sequences, mp4 and mov clips and WAV audio are written with their path, size, duration and frame rate (a conformed clip rate too); the rest of their interpretation (alpha, colour management, fields, loops) is the default.",
    "Layer comments, a 3D layer's X / Y rotation, orientation and material options (each reported when it is not at its default), parallel and ambient lights, and shape items outside groups, rectangles, ellipses, polystars, paths, fills, strokes, gradients, trim paths, round corners, merge and offset paths and repeaters (wiggle, twist, zig-zag, pucker and bloat, ...) are not written.",
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
        Loss::total(&self.losses)
    }

    /// True when nothing was lost.
    pub fn is_lossless(&self) -> bool {
        self.losses.is_empty()
    }

    /// The loss count of a category.
    pub fn count(&self, category: LossCategory) -> usize {
        Loss::count_of(&self.losses, category)
    }

    /// `effects: 3, masks: 1` (empty when lossless).
    pub fn summary(&self) -> String {
        Loss::summary(&self.losses, LossCategory::label)
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
        Loss::record(&mut self.losses, category, move || detail.into());
    }

    pub fn finish(self) -> ExportReport {
        ExportReport {
            exported: self.exported,
            losses: self.losses.into_values().collect(),
            limitations: LIMITATIONS.iter().map(|s| (*s).to_string()).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A report with losses in two categories, as the collector finishes it.
    fn collected() -> ExportReport {
        let mut col = Collector::default();
        col.exported.layers = 2;
        col.lose(LossCategory::RoundedTimes, "time 1");
        col.lose(LossCategory::Effects, String::from("fx 1"));
        col.lose(LossCategory::Effects, "fx 2");
        let mut report = col.finish();
        report.limitations.truncate(1);
        report.limitations[0] = "limit".into();
        report
    }

    /// The JSON shape agents read: field names, category spelling, order and the loss entries.
    #[test]
    fn the_json_shape_is_pinned() {
        let json = serde_json::to_string(&collected()).unwrap();
        assert_eq!(
            json,
            r#"{"exported":{"folders":0,"compositions":0,"solids":0,"layers":2,"solidLayers":0,"nullLayers":0,"adjustmentLayers":0,"properties":0,"keyframes":0,"expressions":0,"masks":0,"precompLayers":0,"effects":0,"footage":0,"footageLayers":0,"textLayers":0,"markers":0,"cameras":0,"lights":0,"shapeLayers":0,"timeRemaps":0,"textAnimators":0,"layerStyles":0},"losses":[{"category":"effects","count":2,"details":["fx 1","fx 2"],"unlisted":0},{"category":"rounded_times","count":1,"details":["time 1"],"unlisted":0}],"limitations":["limit"]}"#
        );
        let back: ExportReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back, collected());
    }

    /// The queries read the losses.
    #[test]
    fn the_queries_read_the_losses() {
        let report = collected();
        assert_eq!((report.total_losses(), report.is_lossless()), (3, false));
        assert_eq!((report.count(LossCategory::Effects), report.count(LossCategory::Lights)), (2, 0));
        assert_eq!(report.summary(), "effects: 2, rounded times: 1");
        let empty = Collector::default().finish();
        assert_eq!((empty.total_losses(), empty.is_lossless(), empty.summary()), (0, true, String::new()));
    }

    /// Details stop at the cap.
    #[test]
    fn details_stop_at_the_cap() {
        let mut col = Collector::default();
        for i in 0..MAX_DETAILS + 5 {
            col.lose(LossCategory::Masks, format!("mask {i}"));
        }
        let report = col.finish();
        assert_eq!(report.count(LossCategory::Masks), MAX_DETAILS + 5);
        assert_eq!(report.losses[0].details.len(), MAX_DETAILS);
    }
}
