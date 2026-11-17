//! What a movie's Dolby Vision configuration says, as the footage information shows it, and
//! whether its video can be decoded.

use serde::{Deserialize, Serialize};

use crate::Footage;

/// The codec the importer names a video stream it reads no codec in (craft-media's
/// `CodecConfig::Unknown`, as FilmCraft, which decodes movies, labels it): none of its decoders
/// takes it, so the footage shows no picture.
pub const UNREAD_CODEC: &str = "unknown";

impl Footage {
    /// The importer read no codec in the footage's video ([`UNREAD_CODEC`]), so this build can't
    /// decode it. Import warnings and the footage information go by this one answer, so they
    /// change together once the decoder reads the stream.
    pub fn codec_unread(&self) -> bool {
        self.codec == UNREAD_CODEC
    }
}

/// The Dolby Vision configuration of a movie's video (its `dvcC`, `dvvC` or `dvwC` box), read when
/// probing ([`crate::Footage::dolby_vision`]). It only describes the file: the footage is decoded
/// as the stream's base layer, and the Dolby Vision metadata (RPUs, an enhancement layer) is not
/// applied, so it never changes the footage's pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DolbyVisionInfo {
    /// The Dolby Vision profile (5, 7, 8, 9, 10, 20, …).
    pub profile: u8,
    pub level: u8,
    /// What the base layer is compatible with, `dv_bl_signal_compatibility_id` in Dolby's
    /// "Dolby Vision Profiles and Levels": 0 none, 1 HDR10, 2 SDR, 4 HLG, 6 Ultra HD Blu-ray.
    pub compatibility: u8,
}

impl DolbyVisionInfo {
    /// The base layer the compatibility ID names ("HLG"); `None` for 0 (no compatible base layer)
    /// and for the IDs Dolby reserves or doesn't list.
    pub fn base_layer(self) -> Option<&'static str> {
        match self.compatibility {
            1 => Some("HDR10"),
            2 => Some("SDR"),
            4 => Some("HLG"),
            6 => Some("Ultra HD Blu-ray"),
            _ => None,
        }
    }

    /// "Dolby Vision 8.4 (HLG base layer)": the profile and compatibility ID, the way Dolby numbers
    /// them (8.1, 8.2, 8.4), and the base layer that ID names.
    pub fn label(self) -> String {
        let base = match (self.base_layer(), self.compatibility) {
            (Some(name), _) => format!("{name} base layer"),
            (None, 0) => "no compatible base layer".to_string(),
            (None, id) => format!("base layer compatibility ID {id}"),
        };
        format!("Dolby Vision {}.{} ({base})", self.profile, self.compatibility)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dv(profile: u8, compatibility: u8) -> DolbyVisionInfo {
        DolbyVisionInfo { profile, level: 4, compatibility }
    }

    /// Each compatibility ID Dolby lists names its base layer; 0 says there is none, and an ID
    /// Dolby reserves or doesn't list is shown as the number it is.
    #[test]
    fn the_label_names_the_base_layer_of_each_compatibility_id() {
        let rows = [
            (dv(8, 1), "Dolby Vision 8.1 (HDR10 base layer)"),
            (dv(8, 2), "Dolby Vision 8.2 (SDR base layer)"),
            (dv(8, 4), "Dolby Vision 8.4 (HLG base layer)"),
            (dv(7, 6), "Dolby Vision 7.6 (Ultra HD Blu-ray base layer)"),
            (dv(5, 0), "Dolby Vision 5.0 (no compatible base layer)"),
            (dv(8, 3), "Dolby Vision 8.3 (base layer compatibility ID 3)"),
            (dv(8, 9), "Dolby Vision 8.9 (base layer compatibility ID 9)"),
            (dv(127, 15), "Dolby Vision 127.15 (base layer compatibility ID 15)"),
        ];
        for (info, want) in rows {
            assert_eq!(info.label(), want, "{info:?}");
        }
    }

    /// Only the codec name the importer gives a stream it read no codec in says the video can't be
    /// decoded; a named codec, or none at all (stills, data), doesn't.
    #[test]
    fn only_the_unread_codec_name_means_the_video_cannot_be_decoded() {
        let with = |codec: &str| Footage { codec: codec.into(), ..Footage::default() };
        assert!(with(UNREAD_CODEC).codec_unread());
        for codec in ["HEVC", "H.264 (High)", "", "Unknown", "unknown video"] {
            assert!(!with(codec).codec_unread(), "{codec:?}");
        }
    }

    /// A project saved before footage recorded Dolby Vision loads with none, and footage without
    /// it is saved without the field (the file stays as earlier builds wrote it).
    #[test]
    fn footage_without_dolby_vision_keeps_the_earlier_format() {
        let plain = Footage { path: "/m/clip.mov".into(), codec: "HEVC".into(), ..Footage::default() };
        let text = serde_json::to_string(&plain).unwrap();
        assert!(!text.contains("dolby_vision"), "{text}");
        let back: Footage = serde_json::from_str(&text).unwrap();
        assert_eq!(back, plain);
        assert_eq!(back.dolby_vision, None);
    }

    /// The configuration round-trips through the project file.
    #[test]
    fn dolby_vision_round_trips_through_the_project_file() {
        let f = Footage { path: "/m/iphone.mov".into(), dolby_vision: Some(DolbyVisionInfo { profile: 8, level: 4, compatibility: 4 }), ..Footage::default() };
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["dolby_vision"], serde_json::json!({"profile": 8, "level": 4, "compatibility": 4}));
        let back: Footage = serde_json::from_value(v).unwrap();
        assert_eq!(back, f);
    }
}
