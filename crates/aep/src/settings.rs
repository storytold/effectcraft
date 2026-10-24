//! Project settings (File ▸ Project Settings): colour depth, working space, working gamma, the two
//! linear-light switches, the time display and the frame count start. Cases 620-634: one change
//! each against the base project 620; every field below is the only byte (or chunk) that changes.

use crate::chunk::AepFile;
use crate::footage::json_string;

/// The time display style (`nhed` byte 8, feet + frames byte 11, film type byte 13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeDisplay {
    /// Timecode (the base, 620): byte 8 = 0.
    Timecode,
    /// Frames (629): byte 8 = 1.
    Frames,
    /// Feet + frames (633, 634): byte 8 = 1 and byte 11 = 1; byte 13 is the frames per foot,
    /// 16 for 35 mm (633) and 40 for 16 mm (634).
    FeetFrames { frames_per_foot: u8 },
}

/// The working colour space (`cpid` and the JSON `Utf8` after `PwCs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkingSpace {
    /// No colour management (623): `cpid` is 16 bytes of `0xff` and the JSON is `{}`.
    None,
    /// A colour profile, named as After Effects lists it (`baseColorProfile/colorProfileName`):
    /// `sRGB IEC61966-2.1` in the base (620), `Rec.709 Gamma 2.4` in 624. The profile data next to
    /// the name is kept as stored.
    Profile(String),
}

/// The project settings that are decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSettings {
    /// Bits per channel: 8, 16 or 32 (`nhed` byte 15 = 0 / 1 / 2: 621, 622, 620; the same byte
    /// is `nnhd` byte 24). `None` for another value.
    pub bits_per_channel: Option<u8>,
    /// The working space; `None` when the chunks are missing or unreadable.
    pub working_space: Option<WorkingSpace>,
    /// Working gamma 2.4 (`dwga` `u32` @0 = 1, 625) or 2.2 (0, 620).
    pub working_gamma_24: Option<bool>,
    /// Linearize Working Space: a 1-byte `lnrp` chunk holding 1 exists (626, 632).
    pub linearize: bool,
    /// Blend Colors Using 1.0 Gamma: a 1-byte `lnrb` chunk holding 1 exists (627, 632).
    pub blend_linear: bool,
    /// Compensate for Scene-referred Profiles (`acer` = 1 in the base, 0 in 628).
    pub scene_compensation: Option<bool>,
    /// The time display style.
    pub time_display: Option<TimeDisplay>,
    /// Frame count start: `nhed` byte 14 holds 2 for a count from 0 (620) and 1 for a count from
    /// 1 (630); this maps 2 to 0 and 1 to 1, anything else to `None`.
    pub frame_start: Option<u8>,
}

impl ProjectSettings {
    /// Reads the settings from the top-level chunks.
    pub fn read(file: &AepFile<'_>) -> ProjectSettings {
        let nhed = file.chunk(b"nhed").and_then(|c| c.bytes());
        let byte = |i: usize| nhed.and_then(|b| b.get(i).copied());
        let bits_per_channel = byte(15).and_then(|v| match v {
            0 => Some(8),
            1 => Some(16),
            2 => Some(32),
            _ => None,
        });
        let time_display = match (byte(8), byte(11), byte(13)) {
            (Some(0), _, _) => Some(TimeDisplay::Timecode),
            (Some(1), Some(0), _) => Some(TimeDisplay::Frames),
            (Some(1), Some(1), Some(fpf)) => Some(TimeDisplay::FeetFrames { frames_per_foot: fpf }),
            _ => None,
        };
        let frame_start = byte(14).and_then(|v| match v {
            2 => Some(0),
            1 => Some(1),
            _ => None,
        });
        let flag = |id: &[u8; 4]| file.chunk(id).and_then(|c| c.bytes()).and_then(|b| b.first().copied()) == Some(1);
        let working_gamma_24 = file.chunk(b"dwga").and_then(|c| c.bytes()).and_then(|b| b.get(..4)).map(|b| b != [0, 0, 0, 0]);
        let scene_compensation = file.chunk(b"acer").and_then(|c| c.bytes()).and_then(|b| b.first().copied()).map(|v| v != 0);
        ProjectSettings {
            bits_per_channel,
            working_space: working_space(file),
            working_gamma_24,
            linearize: flag(b"lnrp"),
            blend_linear: flag(b"lnrb"),
            scene_compensation,
            time_display,
            frame_start,
        }
    }
}

/// The working space: the `Utf8` right after `PwCs`.
fn working_space(file: &AepFile<'_>) -> Option<WorkingSpace> {
    let at = file.chunks.iter().position(|c| c.is(b"PwCs"))?;
    let json = file.chunks.get(at.checked_add(1)?).filter(|c| c.is(b"Utf8"))?.bytes()?;
    let trimmed: Vec<u8> = json.iter().copied().filter(|b| !b.is_ascii_whitespace() && *b != 0).collect();
    if trimmed == b"{}" {
        return Some(WorkingSpace::None);
    }
    json_string(json, "colorProfileName").map(WorkingSpace::Profile)
}
