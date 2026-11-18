//! Project settings (colour depth, working space, working gamma, linear-light switches, time
//! display, frame count start) from [`crate::ProjectSettings`] (cases 620-634), and the renderer
//! (Video Rendering and Effects ▸ Use) from `LIST gpuG`.

use crate::container::index::{Node, Root};
use crate::{TimeDisplay, WorkingSpace};
use effectcraft_color::ColorSpace;
use effectcraft_project::{BitDepth, ProjectSettings, TimeDisplayStyle};

use crate::import::report::{Collector, LossCategory};

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

/// Video Rendering and Effects ▸ Use: `LIST gpuG` holds one `Utf8`, the id of the renderer. After
/// Effects 2026 on Windows (read-backs of projects saved with each `app.project.gpuAccelType`):
/// Mercury GPU Acceleration (CUDA) writes [`CUDA_RENDERER`] (also its default for a new project
/// there), OpenCL `be93941a-7488-4117-8a46-7e3596950307`, Mercury Software Only [`SOFTWARE_RENDERER`]; a project
/// without `gpuG` opens as Software Only. The renderer changes pixels: H.264 footage decodes with
/// softer chroma edges under Software Only (up to 128 of 255 at colour-bar edges).
pub(crate) const CUDA_RENDERER: &str = "7ee0ab59-822d-44cc-ac10-16279d041016";
pub(crate) const SOFTWARE_RENDERER: &str = "f33089e2-1ede-47c1-8a9e-b232bb1cc1a4";

/// Whether the file's renderer is a GPU one (EffectCraft's Mercury GPU Acceleration): any id but
/// Software Only's; no `gpuG` is Software Only.
fn gpu_renderer(file: Root<'_, '_>) -> bool {
    let id = file.list(b"gpuG").and_then(|g| g.child(b"Utf8")).and_then(Node::bytes);
    id.is_some_and(|id| id.trim_ascii() != SOFTWARE_RENDERER.as_bytes())
}

/// `current` with every setting the file stores replaced; a setting the file does not hold (or
/// holds in an unknown form) keeps the current value. The two linear-light switches are always
/// held: After Effects writes their chunk only when on (626, 627), so a file without it has them off. A working space EffectCraft does not have is
/// reported.
pub(crate) fn map(file: Root<'_, '_>, current: &ProjectSettings, col: &mut Collector) -> ProjectSettings {
    let s = crate::ProjectSettings::of(file);
    let nhed = file.chunk(b"nhed").and_then(Node::bytes);
    let mut out = current.clone();
    match s.bits_per_channel {
        Some(8) => out.bit_depth = BitDepth::Bpc8,
        Some(16) => out.bit_depth = BitDepth::Bpc16,
        Some(32) => out.bit_depth = BitDepth::Bpc32,
        _ => {
            // The stored code (`nhed` byte 15) is 0, 1 or 2; another one is not guessed at.
            if let Some(code) = nhed.and_then(|b| b.get(15).copied()) {
                col.lose(LossCategory::ProjectSettings, format!("colour depth code {code} (not known; the current depth is kept)"));
            }
        }
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
        Some(TimeDisplay::FeetFrames { frames_per_foot }) => {
            col.lose(
                LossCategory::ProjectSettings,
                format!("feet + frames with {frames_per_foot} frames per foot (only 16 and 40 are known; the current time display is kept)"),
            );
        }
        None => {
            // `nhed` bytes 8, 11 and 13; a header too short to hold them is not a setting.
            if let Some(&[mode, _, _, feet, ..]) = nhed.and_then(|b| b.get(8..14)) {
                col.lose(LossCategory::ProjectSettings, format!("time display code {mode}/{feet} (not known; the current time display is kept)"));
            }
        }
    }
    out.gpu_acceleration = gpu_renderer(file);
    match s.frame_start {
        Some(start) => out.frame_start = i64::from(start),
        None => {
            if let Some(code) = nhed.and_then(|b| b.get(14).copied()) {
                col.lose(LossCategory::ProjectSettings, format!("frame count start code {code} (not known; the current start is kept)"));
            }
        }
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
