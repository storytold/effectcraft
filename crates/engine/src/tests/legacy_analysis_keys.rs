//! Warp Stabilizer and 3D Camera Tracker analyses saved by an earlier release keep matching
//! their footage after an edit that doesn't change it (#3456), and a real change still clears them.

/// The footage text analysis keys are hashed from, as 0.23 saved it. A new `Footage` field must
/// not change it (or every saved analysis is dropped again).
#[test]
fn footage_signature_text_is_pinned() {
    use effectcraft_project::{Footage, FootageKind};
    let f = Footage {
        path: "clip.mp4".into(),
        kind: FootageKind::Video,
        width: 160,
        height: 120,
        invert_alpha: true,
        page: 2,
        placed_page: true,
        ..Footage::default()
    };
    let fields = r#"path: "clip.mp4", kind: Video, width: 160, height: 120, pixel_aspect: 1.0, frame_rate: FrameRate { num: 30, den: 1 }, native_rate: None, duration: Tick(0), has_video: false, has_audio: false, alpha: Straight, premul_color: [0.0, 0.0, 0.0], loop_count: 1, codec: "", missing: false, sequence: [], color_profile: None, layer: None, fields: Off, invert_alpha: true, linear_light: false, preserve_rgb: false, cineon: CineonSettings { log_conversion: false, converted_black: 0.0, converted_white: 1023.0, black: 95.0, white: 685.0, gamma: 1.7, rolloff: 20.0, units: TenBit }, data: None, page: 2, alphabetical: false, start_timecode: None, file_timecode: None, markers: []"#;
    assert_eq!(crate::warp::footage_text(&f, true), format!("Footage {{ {fields}, placed_page: true }}"));
    assert_eq!(crate::warp::footage_text(&f, false), format!("Footage {{ {fields} }}"));
}
