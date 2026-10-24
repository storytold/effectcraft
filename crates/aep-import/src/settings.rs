//! Project settings (colour depth, working space, working gamma, linear-light switches, time
//! display, frame count start) from [`effectcraft_aep::ProjectSettings`] (cases 620-634).

use effectcraft_aep::{AepFile, TimeDisplay, WorkingSpace};
use effectcraft_color::ColorSpace;
use effectcraft_project::{BitDepth, ProjectSettings, TimeDisplayStyle};

use crate::report::{Collector, LossCategory};

/// The working spaces whose After Effects name is sampled (620, 624) and that EffectCraft has.
pub(crate) fn color_space(name: &str) -> Option<ColorSpace> {
    match name {
        "sRGB IEC61966-2.1" => Some(ColorSpace::Srgb),
        // The second name is the one After Effects shows for the profile EffectCraft's exporter
        // writes (its description tag) once it re-saves such a project.
        "Rec.709 Gamma 2.4" | "Rec.709 Gamma 2.4 (EffectCraft)" => Some(ColorSpace::Rec709),
        _ => None,
    }
}

/// `current` with every setting the file stores replaced; a setting the file does not hold (or
/// holds in an unknown form) keeps the current value. A working space EffectCraft does not have is
/// reported.
pub(crate) fn map(file: &AepFile<'_>, current: &ProjectSettings, col: &mut Collector) -> ProjectSettings {
    let s = effectcraft_aep::ProjectSettings::read(file);
    let mut out = current.clone();
    match s.bits_per_channel {
        Some(8) => out.bit_depth = BitDepth::Bpc8,
        Some(16) => out.bit_depth = BitDepth::Bpc16,
        Some(32) => out.bit_depth = BitDepth::Bpc32,
        _ => {}
    }
    match &s.working_space {
        Some(WorkingSpace::None) => out.working_space = None,
        Some(WorkingSpace::Profile(name)) => match color_space(name) {
            Some(cs) => {
                out.working_space = Some(cs);
                // After Effects renders in the working space (902: an sRGB PNG step 128 is 135 in
                // a Rec.709 Gamma 2.4 project, a grey 0.5 solid 128, in the Render Queue output
                // and in the captures), EffectCraft converts to `output_space` (sRGB when unset).
                // sRGB stays unset, which is the same.
                out.output_space = (cs != ColorSpace::Srgb).then_some(cs);
            }
            None => col.lose(LossCategory::ProjectSettings, format!("working space '{name}' (no EffectCraft counterpart; the current one is kept)")),
        },
        None => {}
    }
    if let Some(g24) = s.working_gamma_24 {
        out.working_gamma = if g24 { 2.4 } else { 2.2 };
    }
    out.linearize = s.linearize;
    out.blend_linear = s.blend_linear;
    match s.time_display {
        Some(TimeDisplay::Timecode) => out.time_display = TimeDisplayStyle::Timecode,
        Some(TimeDisplay::Frames) => out.time_display = TimeDisplayStyle::Frames,
        Some(TimeDisplay::FeetFrames { frames_per_foot: 16 }) => out.time_display = TimeDisplayStyle::Feet35,
        Some(TimeDisplay::FeetFrames { frames_per_foot: 40 }) => out.time_display = TimeDisplayStyle::Feet16,
        _ => {}
    }
    if let Some(start) = s.frame_start {
        out.frame_start = i64::from(start);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rec709_under_both_names() {
        assert_eq!(color_space("Rec.709 Gamma 2.4"), Some(ColorSpace::Rec709));
        assert_eq!(color_space("Rec.709 Gamma 2.4 (EffectCraft)"), Some(ColorSpace::Rec709));
        assert_eq!(color_space("Rec.709 Gamma 2.4 (other)"), None);
    }
}
