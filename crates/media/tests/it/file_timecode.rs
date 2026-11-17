//! The start timecode a movie records is read into its footage (`Footage::file_timecode`).

use std::io::Cursor;

use craft_media::isobmff::{Brand, Mp4Writer, SampleEntry, TimecodeConfig, TrackConfig, WriteSample, WriterOptions};
use effectcraft_project::FootageKind;

/// A two-frame 24 fps Motion JPEG MOV, with a timecode track from `start_frame` when given.
fn mov(start_frame: Option<u32>) -> Vec<u8> {
    let mut jpeg = Vec::new();
    image::RgbImage::from_pixel(16, 16, image::Rgb([200, 40, 90])).write_to(&mut Cursor::new(&mut jpeg), image::ImageFormat::Jpeg).expect("jpeg");
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), WriterOptions::new(Brand::Mov)).expect("writer");
    let v = w.add_track(TrackConfig::new(SampleEntry::jpeg(16, 16), 24)).expect("video track");
    if let Some(start) = start_frame {
        let tc = TimecodeConfig { timescale: 24, frame_duration: 1, frames_per_second: 24, ..Default::default() };
        w.add_timecode_track(v, tc, start).expect("timecode track");
    }
    for _ in 0..2 {
        w.write_sample(v, WriteSample { data: &jpeg, duration: 1, composition_offset: 0, is_sync: true }).expect("frame");
    }
    w.finish().expect("finish").into_inner()
}

#[test]
fn a_movies_start_timecode_is_read() {
    let dir = std::env::temp_dir().join(format!("effectcraft-file-timecode-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    // 10:00:00:00 at 24 fps, and a movie with no timecode
    for (name, start) in [("tc.mov", Some(10 * 3600 * 24)), ("none.mov", None)] {
        let p = dir.join(name);
        std::fs::write(&p, mov(start)).expect("write");
        let f = effectcraft_media::probe(&p).expect("probe");
        assert_eq!(f.kind, FootageKind::Video, "{name}");
        assert_eq!(f.file_timecode, start.map(i64::from), "{name}");
        assert_eq!(f.start_timecode, None, "{name}: Interpret Footage still uses the source file timecode");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
