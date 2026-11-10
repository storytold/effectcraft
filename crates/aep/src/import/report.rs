//! The import report: what was imported (counts) and everything that was not, per category.
//!
//! The report is plain data (`serde`), so it round-trips through JSON: the engine command returns it
//! and agents read it. Nothing the importer skips is left out of it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::container::AepFile;

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
    /// Parts of shape layers that are not imported: contents items and properties outside the
    /// imported set, groups nested too deep, material options. (The layers are imported.)
    ShapeLayers,
    /// Parts of text layers that are not imported: wiggly and expression selectors and the other
    /// animator entries outside the imported set, Path Options and More Options
    /// with set values, Source Text keyframes and expressions, undecoded character and paragraph
    /// fields that differ from the default, unknown capitalisation or justification codes, control
    /// characters of unknown meaning. (The layers themselves are imported.)
    TextLayers,
    /// Camera layers that are not imported (none at present: cameras are imported).
    Cameras,
    /// Light layers of a light type that is not sampled (the other lights are imported).
    Lights,
    /// Layer styles (drop shadow, glow, stroke, ...) with set values.
    LayerStyles,
    /// Marker data read in another form: a cue type of unknown code (read as an event cue).
    Markers,
    /// Blend mode codes outside the known ones (906, 907; the layer keeps Normal).
    BlendModes,
    /// Stored transform properties other than Anchor Point, Position, Scale, Rotation and Opacity
    /// (3D rotations, orientation, separated position dimensions).
    TransformProperties,
    /// Time remapping that is not imported: a Time Remap property without a readable keyframe.
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
    /// sRGB and Rec. 709) or stored as an unknown code (colour depth, time display, frames per
    /// foot, frame count start).
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
    /// The After Effects version the file is written for and the one that wrote it.
    #[serde(default)]
    pub ae_version: AeVersionInfo,
}

/// The After Effects versions the importer is tested with (the major of the version a file is
/// written for): files written for 24.x, 25.x and 26.x.
pub const TESTED_MAJORS: std::ops::RangeInclusive<u8> = 24..=26;

/// The After Effects versions of a file, decoded from the container (never from the XMP packet,
/// which names the application of the first save).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AeVersionInfo {
    /// The version the file is written for (`head`): `26.2.1x2` for a file saved by After Effects
    /// 26.2.1, `25.6.0x97` for a copy that After Effects 2026 saved as 25.x. `unknown` when the file
    /// does not say.
    pub file_version: String,
    /// The version of the After Effects that wrote the file (`svap`): `26.2.1x2` for that copy too.
    /// `unknown` when the file does not say.
    pub saved_by: String,
    /// Whether the importer is tested with files written for this version ([`TESTED_MAJORS`]).
    /// When not, what is read is still imported, and a line in
    /// [`ImportReport::limitations`] says the version is not tested.
    pub tested: bool,
}

impl Default for AeVersionInfo {
    fn default() -> Self {
        Self { file_version: UNKNOWN.to_string(), saved_by: UNKNOWN.to_string(), tested: false }
    }
}

/// The text of a version the file does not state.
pub const UNKNOWN: &str = "unknown";

impl AeVersionInfo {
    /// Reads the versions of a parsed file.
    pub fn of(file: &AepFile<'_>) -> Self {
        let file_version = file.file_version();
        Self {
            file_version: file_version.map_or_else(|| UNKNOWN.to_string(), |v| v.text()),
            saved_by: file.saved_by_version().map_or_else(|| UNKNOWN.to_string(), |v| v.text()),
            tested: file_version.is_some_and(|v| TESTED_MAJORS.contains(&v.major)),
        }
    }

    /// The limitation line for a version that is not tested; `None` for a tested one.
    pub fn limitation(&self) -> Option<String> {
        (!self.tested).then(|| {
            format!(
                "The file is written for After Effects {} (saved by {}); the importer is tested with files written for After Effects {} to {} only. What it reads is imported, but this version is not tested.",
                self.file_version,
                self.saved_by,
                TESTED_MAJORS.start(),
                TESTED_MAJORS.end()
            )
        })
    }
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
    /// The effects with no counterpart, by match name: the first instance's report line and how
    /// many instances there were. They get a line each in [`Collector::finish`], so one that
    /// comes after the example lines are used up is still named (#3365).
    unmapped_effects: BTreeMap<String, (String, usize)>,
}

/// How many different unmapped effects get a line of their own (a file names at most this many).
const MAX_UNMAPPED_EFFECTS: usize = 100;

impl Collector {
    /// Records one loss.
    pub fn lose(&mut self, category: LossCategory, detail: impl Into<String>) {
        let loss = self.losses.entry(category).or_insert_with(|| Loss { category, count: 0, details: Vec::new() });
        loss.count = loss.count.saturating_add(1);
        if loss.details.len() < MAX_DETAILS {
            loss.details.push(detail.into());
        }
    }

    /// Records one instance of an effect with no counterpart. The loss is counted now; its line is
    /// made in [`Collector::finish`], one per effect with the number of instances, so that many
    /// other losses cannot hide which effects were dropped.
    pub fn lose_unmapped_effect(&mut self, match_name: &str, detail: String) {
        let loss = self.losses.entry(LossCategory::Effects).or_insert_with(|| Loss { category: LossCategory::Effects, count: 0, details: Vec::new() });
        loss.count = loss.count.saturating_add(1);
        if let Some(seen) = self.unmapped_effects.get_mut(match_name) {
            seen.1 = seen.1.saturating_add(1);
        } else if self.unmapped_effects.len() < MAX_UNMAPPED_EFFECTS {
            self.unmapped_effects.insert(match_name.to_string(), (detail, 1));
        }
        // (Past the limit an effect is only counted: its name is not worth a line of its own.)
    }

    pub fn finish(mut self) -> ImportReport {
        if let Some(loss) = self.losses.get_mut(&LossCategory::Effects) {
            let named = std::mem::take(&mut self.unmapped_effects).into_values().map(|(first, n)| match n {
                1 => first,
                _ => format!("{first}, and {} more like it", n - 1),
            });
            loss.details.splice(0..0, named);
        }
        ImportReport {
            imported: self.imported,
            losses: self.losses.into_values().collect(),
            missing_footage: self.missing_footage,
            missing_fonts: self.missing_fonts,
            limitations: LIMITATIONS.iter().map(|s| (*s).to_string()).collect(),
            ae_version: AeVersionInfo::default(),
        }
    }
}

/// Parts of a project that the container reader does not decode. They are never imported and, as
/// the reader cannot see them, never counted either.
pub const LIMITATIONS: &[&str] = &[
    "Frame blending, 3D material options, audio levels, guides are not decoded and not imported.",
    "Mask colour, lock, feather falloff, motion blur and RotoBezier are not decoded: imported masks get the default colour, unlocked, Smooth falloff, motion blur Same As Layer and no RotoBezier.",
    "Text: per-character 3D, text on a path and several style slots of unknown meaning are not decoded (stored values that differ from the default are reported as text layer features).",
    "Footage that is not a solid is linked by its stored path with its stored frame rate and duration; the importer never opens the files (use Reload Footage after the files are found).",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// #3365: with other losses filling the 20 example lines, an effect with no counterpart still
    /// gets its own line, with how many instances were lost. The category's count is unchanged.
    #[test]
    fn every_unmapped_effect_is_named_however_many_other_losses_there_are() {
        let mut col = Collector::default();
        for i in 0..30 {
            col.lose(LossCategory::Effects, format!("C: 'L{i}': parameter P not imported"));
        }
        for i in 0..25 {
            col.lose_unmapped_effect("CC Cylinder", format!("C: 'Cyl{i}' has the effect 'CC Cylinder' (CC Cylinder)"));
        }
        col.lose_unmapped_effect("Pseudo/", "C: 'D' has the effect 'Dropdown' (Pseudo/)".to_string());
        let report = col.finish();
        let effects = report.losses.iter().find(|l| l.category == LossCategory::Effects).unwrap();
        assert_eq!(effects.count, 56, "30 parameters, 25 cylinders and 1 dropdown");
        let named = |what: &str| effects.details.iter().filter(|d| d.contains(what)).collect::<Vec<_>>();
        assert_eq!(named("(CC Cylinder)").len(), 1, "{:?}", effects.details);
        assert!(named("(CC Cylinder)")[0].contains("24 more"), "{:?}", named("(CC Cylinder)"));
        // One instance stays the single line it was.
        assert_eq!(named("(Pseudo/)"), ["C: 'D' has the effect 'Dropdown' (Pseudo/)"]);
        assert!(effects.details.len() <= MAX_DETAILS + 2, "the examples stay bounded");
    }

    /// A file with more than [`MAX_UNMAPPED_EFFECTS`] different unmapped effects names that many
    /// and counts the rest, whatever room the example lines have.
    #[test]
    fn at_most_the_limit_of_unmapped_effects_are_named() {
        let mut col = Collector::default();
        for i in 0..=MAX_UNMAPPED_EFFECTS {
            col.lose_unmapped_effect(&format!("Effect {i}"), format!("C: 'L' has the effect 'Effect {i}' (Effect {i})"));
        }
        let report = col.finish();
        let effects = report.losses.iter().find(|l| l.category == LossCategory::Effects).unwrap();
        assert_eq!(effects.count, MAX_UNMAPPED_EFFECTS + 1, "every instance is counted");
        assert_eq!(effects.details.len(), MAX_UNMAPPED_EFFECTS, "{} lines", effects.details.len());
    }
}
