//! The After Effects version a project is written for (`head` bytes 4 to 7) and the version of the
//! After Effects that wrote it (`svap`).
//!
//! Both hold the same packed `u32` (see `docs/aep-format.md`, section 1, "Version"). A project saved
//! normally has the same word in both; File ▸ Save a Copy As 25.x / 24.x of After Effects 2026 keeps
//! `svap` (the 2026 version) and writes the older version into `head`. The XMP packet after the
//! container names the application of the first save (`CreatorTool`) and is no guide to either.

use std::fmt;

use crate::container::bytes::be_u32;
use crate::container::chunk::AepFile;
use crate::container::index::{Root, TreeIndex};

/// An After Effects version as packed in `head` / `svap`: `26.2.1x2` is major 26, minor 2, bug-fix
/// 1, build 2 (the form of the scripting API's `app.version`).
///
/// Bit layout of the big-endian word (bit 31 first), decoded from the samples (the 2026 word
/// `0x0f11_0e02` is the version the saving After Effects reports, `26.2.1x2`; the down-saves give
/// `0x0f0b_0661` and `0x0f03_0641`, read as `25.6.0x97` and `24.6.0x65`):
///
/// | bits | field |
/// |---|---|
/// | 31-26 | not decoded (`000011` in every sample) |
/// | 25-24, 21-19 | major (high two bits, then low three bits) |
/// | 23-22 | not decoded (`00` in every sample) |
/// | 18-15 | minor |
/// | 14-11 | bug-fix |
/// | 10-8 | stage (6 in every sample) |
/// | 7-0 | build |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AeVersion {
    pub major: u8,
    pub minor: u8,
    pub bugfix: u8,
    /// The release stage (6 in every sample; not shown in [`AeVersion::text`]).
    pub stage: u8,
    pub build: u8,
    /// The word as stored.
    pub raw: u32,
}

impl AeVersion {
    /// Decodes a stored word; `None` for a word that holds no version (zero, all ones, or major 0:
    /// After Effects itself refuses a file whose `head` version is zero, "[0.0x0]").
    pub fn from_word(raw: u32) -> Option<Self> {
        if raw == 0 || raw == u32::MAX {
            return None;
        }
        let [b0, b1, b2, b3] = raw.to_be_bytes();
        let major = ((b0 & 0x03) << 3) | ((b1 >> 3) & 0x07);
        if major == 0 {
            return None;
        }
        Some(Self { major, minor: ((b1 & 0x07) << 1) | (b2 >> 7), bugfix: (b2 >> 3) & 0x0f, stage: b2 & 0x07, build: b3, raw })
    }

    /// `26.2.1x2`.
    pub fn text(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for AeVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}x{}", self.major, self.minor, self.bugfix, self.build)
    }
}

impl AepFile<'_> {
    /// The version the file is written for: `head` bytes 4 to 7. After Effects 2026 writes its own
    /// version there, and the older version for File ▸ Save a Copy As 25.x / 24.x. `None` when the
    /// chunk is missing, short or holds no version.
    pub fn file_version(&self) -> Option<AeVersion> {
        Root::from(self).file_version()
    }

    /// The version of the After Effects that wrote the file: `svap` bytes 0 to 3 (the same word in a
    /// down-saved copy as in a normal save). `None` when the chunk is missing (After Effects opens such
    /// a file), short or holds no version.
    pub fn saved_by_version(&self) -> Option<AeVersion> {
        Root::from(self).saved_by_version()
    }
}

impl TreeIndex<'_> {
    /// [`AepFile::file_version`] of the indexed file.
    pub fn file_version(&self) -> Option<AeVersion> {
        Root::from(self).file_version()
    }

    /// [`AepFile::saved_by_version`] of the indexed file.
    pub fn saved_by_version(&self) -> Option<AeVersion> {
        Root::from(self).saved_by_version()
    }
}

impl Root<'_, '_> {
    pub(crate) fn file_version(self) -> Option<AeVersion> {
        AeVersion::from_word(be_u32(self.chunk(b"head")?.bytes()?, 4)?)
    }

    pub(crate) fn saved_by_version(self) -> Option<AeVersion> {
        AeVersion::from_word(be_u32(self.chunk(b"svap")?.bytes()?, 0)?)
    }
}

#[cfg(test)]
mod tests {
    use super::AeVersion;

    #[test]
    fn decodes_the_sampled_words() {
        let v = AeVersion::from_word(0x0f11_0e02).unwrap();
        assert_eq!((v.major, v.minor, v.bugfix, v.stage, v.build), (26, 2, 1, 6, 2));
        assert_eq!(v.text(), "26.2.1x2");
        assert_eq!(AeVersion::from_word(0x0f0b_0661).unwrap().text(), "25.6.0x97");
        assert_eq!(AeVersion::from_word(0x0f03_0641).unwrap().text(), "24.6.0x65");
    }

    #[test]
    fn a_word_without_a_version_is_none() {
        assert_eq!(AeVersion::from_word(0), None);
        assert_eq!(AeVersion::from_word(u32::MAX), None);
        // Major 0 (only the undecoded high bits set).
        assert_eq!(AeVersion::from_word(0x0c00_0000), None);
    }

    #[test]
    fn every_field_takes_its_extremes() {
        let v = AeVersion::from_word(0x03ff_ffff).unwrap();
        assert_eq!((v.major, v.minor, v.bugfix, v.stage, v.build), (31, 15, 15, 7, 255));
        let v = AeVersion::from_word(0x0000_0800 | 0x0008_0000).unwrap();
        assert_eq!((v.major, v.minor, v.bugfix, v.build), (1, 0, 1, 0));
    }
}
