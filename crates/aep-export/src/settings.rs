//! Project settings (`docs/aep-format.md` 3.12a, cases 620-634): colour depth, working space
//! (None or the template's sRGB), working gamma, the two linear-light switches, time display and
//! frame count start, written into the template's top-level chunks.

use effectcraft_aep::{AepFile, Chunk};
use effectcraft_color::ColorSpace;
use effectcraft_project::{BitDepth, ColorEngine, HdrMode, ProjectSettings, TimeDisplayStyle};

use crate::report::{Collector, LossCategory};
use crate::tree::{R, data_mut, put_u8, tpl};

/// The settings in the file's terms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SettingsPlan {
    /// `nhed` byte 15: 0 / 1 / 2 for 8 / 16 / 32 bits per channel.
    pub depth: u8,
    /// No working space (623); otherwise the template's sRGB is kept.
    pub no_working_space: bool,
    /// Rec.709 Gamma 2.4, written with a profile authored here (`icc.rs`, R49).
    pub rec709: bool,
    /// `dwga` byte 0: 1 for a working gamma of 2.4.
    pub gamma_24: bool,
    pub linearize: bool,
    pub blend_linear: bool,
    /// `nhed` bytes 8, 11, 13: frames, feet + frames, frames per foot.
    pub frames: bool,
    pub feet: Option<u8>,
    /// `nhed` byte 14: 2 for a count from 0, 1 for a count from 1.
    pub frame_start: u8,
}

pub(crate) fn plan(s: &ProjectSettings, col: &mut Collector) -> SettingsPlan {
    let depth = match s.bit_depth {
        BitDepth::Bpc8 => 0,
        BitDepth::Bpc16 => 1,
        BitDepth::Bpc32 => 2,
    };
    let (no_working_space, rec709) = match s.working_space {
        None => (true, false),
        Some(ColorSpace::Srgb) => (false, false),
        Some(ColorSpace::Rec709) => (false, true),
        Some(other) => {
            col.lose(
                LossCategory::ProjectSettings,
                format!("the working space {other:?} is written as sRGB (only None, sRGB and Rec.709 Gamma 2.4 are written)"),
            );
            (false, false)
        }
    };
    if s.color_engine != ColorEngine::Adobe {
        col.lose(LossCategory::ProjectSettings, "the OCIO colour engine is not written".to_string());
    }
    if s.hdr != HdrMode::Clip {
        col.lose(LossCategory::ProjectSettings, "the HDR handling is not written".to_string());
    }
    // After Effects renders in the working space (902): an output space equal to the working
    // space is its normal case (the importer sets it so for Rec.709 Gamma 2.4); another one has
    // no place in the file.
    if s.output_space.is_some_and(|o| Some(o) != s.working_space) {
        col.lose(LossCategory::ProjectSettings, "an output space other than the working space is not written".to_string());
    }
    let (frames, feet) = match s.time_display {
        TimeDisplayStyle::Timecode => (false, None),
        TimeDisplayStyle::Frames => (true, None),
        TimeDisplayStyle::Feet35 => (true, Some(16)),
        TimeDisplayStyle::Feet16 => (true, Some(40)),
    };
    let frame_start = match s.frame_start {
        0 => 2,
        1 => 1,
        other => {
            col.lose(LossCategory::ProjectSettings, format!("a frame count start of {other} is written as 0 (only 0 and 1 exist)"));
            2
        }
    };
    SettingsPlan {
        depth,
        no_working_space,
        rec709,
        gamma_24: s.working_gamma_24(),
        linearize: s.linearize,
        blend_linear: s.blend_linear,
        frames,
        feet,
        frame_start,
    }
}

/// Writes `p` into the template's top-level chunks.
pub(crate) fn emit(file: &mut AepFile<'static>, p: &SettingsPlan) -> R<()> {
    let top = &mut file.chunks;
    let header =
        |top: &mut Vec<Chunk<'static>>, id: &[u8; 4]| -> R<usize> { top.iter().position(|c| c.is(id)).ok_or_else(|| tpl("template header chunk missing")) };
    // nhed and its copy nnhd (byte offsets per field: 620-634).
    for (id, at_depth, at_display, at_feet, at_fpf, at_start) in [(b"nhed", 15, 8, 11, 13, 14), (b"nnhd", 24, 8, 11, 19, 20)] {
        let i = header(top, id)?;
        let d = data_mut(top.get_mut(i).ok_or_else(|| tpl("header"))?)?;
        put_u8(d, at_depth, p.depth)?;
        put_u8(d, at_display, u8::from(p.frames))?;
        put_u8(d, at_feet, u8::from(p.feet.is_some()))?;
        put_u8(d, at_fpf, p.feet.unwrap_or(16))?;
        put_u8(d, at_start, p.frame_start)?;
    }
    {
        let i = header(top, b"dwga")?;
        put_u8(data_mut(top.get_mut(i).ok_or_else(|| tpl("dwga"))?)?, 0, u8::from(p.gamma_24))?;
    }
    // Blend Colors Using 1.0 Gamma (lnrb) and Linearize Working Space (lnrp): one-byte chunks
    // right after cpid, in that order (626, 627, 632).
    top.retain(|c| !c.is(b"lnrb") && !c.is(b"lnrp"));
    let cpid = header(top, b"cpid")?;
    let mut at = cpid.saturating_add(1);
    if p.blend_linear {
        top.insert(at, Chunk::data(*b"lnrb", vec![1u8]));
        at = at.saturating_add(1);
    }
    if p.linearize {
        top.insert(at, Chunk::data(*b"lnrp", vec![1u8]));
    }
    if p.rec709 {
        // 624: `cpid` is the profile ID, the JSON after PwCs holds the profile's data and the
        // working space's name; the profile is our own (R49).
        let icc = crate::icc::rec709_gamma24();
        *top.get_mut(cpid).ok_or_else(|| tpl("cpid"))? = Chunk::data(*b"cpid", crate::icc::profile_id(&icc).to_vec());
        let pw = header(top, b"PwCs")?;
        let json = top.get_mut(pw.saturating_add(1)).filter(|c| c.is(b"Utf8")).ok_or_else(|| tpl("template without the working-space JSON"))?;
        let text = format!(
            "{{\"baseColorProfile\":{{\"colorProfileData\":\"{}\",\"colorProfileName\":\"{}\"}},\"baseProfileType\":2}}",
            crate::icc::base64(&icc),
            crate::icc::REC709_NAME
        );
        *json = Chunk::data(*b"Utf8", text.into_bytes());
    }
    if p.no_working_space {
        // 623: cpid all 0xff, the working-space JSON after PwCs is `{}`.
        *top.get_mut(cpid).ok_or_else(|| tpl("cpid"))? = Chunk::data(*b"cpid", vec![0xffu8; 16]);
        let pw = header(top, b"PwCs")?;
        let json = top.get_mut(pw.saturating_add(1)).filter(|c| c.is(b"Utf8")).ok_or_else(|| tpl("template without the working-space JSON"))?;
        *json = Chunk::data(*b"Utf8", b"{}".to_vec());
    }
    Ok(())
}
