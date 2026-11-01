//! Project settings (File ▸ Project Settings): colour depth, working space, working gamma, the two
//! linear-light switches, the time display and the frame count start. Cases 620-634: one change
//! each against the base project 620; every field below is the only byte (or chunk) that changes.

use crate::container::chunk::AepFile;
use crate::container::footage::json_string;

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
    // After Effects 26.2.1 saves the profile data without the name (settings-c, 902): the name is
    // then the profile's own description (ICC.1 `desc`).
    json_string(json, "colorProfileName")
        .or_else(|| json_string(json, "colorProfileData").and_then(|b| base64_decode(&b)).and_then(|icc| icc_description(&icc)))
        .map(WorkingSpace::Profile)
}

/// Standard base64 (RFC 4648) with or without padding; `None` for any other character.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes().filter(|c| *c != b'=') {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6 | u32::from(v)) & 0x00ff_ffff;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

/// The profile description (ICC.1 tag `desc`): a v2 `desc` (ASCII) or a v4 `mluc` (its first
/// record, UTF-16BE). `None` when the profile or the tag is cut short.
fn icc_description(icc: &[u8]) -> Option<String> {
    let be32 = |at: usize| icc.get(at..at.checked_add(4)?).and_then(|b| b.try_into().ok()).map(u32::from_be_bytes);
    let count = be32(128)?;
    let tag = (0..count.min(1024)).find_map(|i| {
        let at = 132usize.checked_add(usize::try_from(i).ok()?.checked_mul(12)?)?;
        (icc.get(at..at.checked_add(4)?)? == b"desc").then_some(at)
    })?;
    let off = usize::try_from(be32(tag.checked_add(4)?)?).ok()?;
    let len = usize::try_from(be32(tag.checked_add(8)?)?).ok()?;
    let body = icc.get(off..off.checked_add(len)?)?;
    let text = match body.get(..4)? {
        b"desc" => {
            let n = usize::try_from(u32::from_be_bytes(body.get(8..12)?.try_into().ok()?)).ok()?;
            let ascii = body.get(12..12usize.checked_add(n)?)?;
            String::from_utf8_lossy(ascii.split(|b| *b == 0).next().unwrap_or_default()).into_owned()
        }
        b"mluc" => {
            let n = usize::try_from(u32::from_be_bytes(body.get(20..24)?.try_into().ok()?)).ok()?;
            let at = usize::try_from(u32::from_be_bytes(body.get(24..28)?.try_into().ok()?)).ok()?;
            let units: Vec<u16> = body.get(at..at.checked_add(n)?)?.as_chunks::<2>().0.iter().map(|p| u16::from_be_bytes(*p)).collect();
            String::from_utf16_lossy(&units)
        }
        _ => return None,
    };
    let text = text.trim_end_matches('\0').trim().to_owned();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_decodes_with_and_without_padding() {
        assert_eq!(base64_decode("Zm9vYmFy").as_deref(), Some(&b"foobar"[..]));
        assert_eq!(base64_decode("Zm9vYmE=").as_deref(), Some(&b"fooba"[..]));
        assert_eq!(base64_decode("Zm9vYg").as_deref(), Some(&b"foob"[..]));
        assert_eq!(base64_decode("Zm9v*"), None);
    }

    /// A profile with one tag, `desc`, holding `body`.
    fn profile(body: &[u8]) -> Vec<u8> {
        let mut icc = vec![0u8; 128];
        icc.extend_from_slice(&1u32.to_be_bytes());
        icc.extend_from_slice(b"desc");
        icc.extend_from_slice(&144u32.to_be_bytes());
        icc.extend_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes());
        icc.extend_from_slice(body);
        icc
    }

    /// A version 2 `desc` body.
    fn desc(text: &str) -> Vec<u8> {
        let mut b = b"desc\0\0\0\0".to_vec();
        b.extend_from_slice(&u32::try_from(text.len() + 1).unwrap().to_be_bytes());
        b.extend_from_slice(text.as_bytes());
        b.push(0);
        b
    }

    /// A version 4 `mluc` body with one record.
    fn mluc(text: &str) -> Vec<u8> {
        let units: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
        let mut b = b"mluc\0\0\0\0".to_vec();
        b.extend_from_slice(&1u32.to_be_bytes());
        b.extend_from_slice(&12u32.to_be_bytes());
        b.extend_from_slice(b"enUS");
        b.extend_from_slice(&u32::try_from(units.len()).unwrap().to_be_bytes());
        b.extend_from_slice(&28u32.to_be_bytes());
        b.extend_from_slice(&units);
        b
    }

    #[test]
    fn the_description_of_a_profile_is_read() {
        assert_eq!(icc_description(&profile(&desc("Rec.709 Gamma 2.4"))).as_deref(), Some("Rec.709 Gamma 2.4"));
        assert_eq!(icc_description(&profile(&mluc("sRGB IEC61966-2.1"))).as_deref(), Some("sRGB IEC61966-2.1"));
        assert_eq!(icc_description(&profile(&desc(""))), None);
    }

    #[test]
    fn a_cut_or_damaged_profile_gives_no_description() {
        let icc = profile(&mluc("Rec.709 Gamma 2.4"));
        for n in 0..icc.len() {
            let mut cut = icc.get(..n).unwrap_or_default().to_vec();
            let _ = icc_description(&cut);
            cut.iter_mut().for_each(|b| *b = 0xff);
            let _ = icc_description(&cut);
        }
        assert_eq!(icc_description(&icc[..150]), None);
        assert_eq!(icc_description(&[]), None);
    }
}
