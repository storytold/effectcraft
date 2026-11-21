//! The start timecode a movie records is read into its footage (`Footage::file_timecode`).

use std::io::Cursor;

use craft_media::isobmff::{Brand, Mp4Writer, SampleEntry, TimecodeConfig, TrackConfig, WriteSample, WriterOptions};
use effectcraft_project::FootageKind;

/// A two-frame 24 fps MOV, with a timecode track from `start_frame` when given.
fn mov(entry: SampleEntry, packet: &[u8], start_frame: Option<u32>) -> Vec<u8> {
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), WriterOptions::new(Brand::Mov)).expect("writer");
    let v = w.add_track(TrackConfig::new(entry, 24)).expect("video track");
    if let Some(start) = start_frame {
        let tc = TimecodeConfig { timescale: 24, frame_duration: 1, frames_per_second: 24, ..Default::default() };
        w.add_timecode_track(v, tc, start).expect("timecode track");
    }
    for _ in 0..2 {
        w.write_sample(v, WriteSample { data: packet, duration: 1, composition_offset: 0, is_sync: true }).expect("frame");
    }
    w.finish().expect("finish").into_inner()
}

#[test]
fn a_movies_start_timecode_is_read() {
    let dir = std::env::temp_dir().join(format!("effectcraft-file-timecode-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let mut jpeg = Vec::new();
    image::RgbImage::from_pixel(16, 16, image::Rgb([200, 40, 90])).write_to(&mut Cursor::new(&mut jpeg), image::ImageFormat::Jpeg).expect("jpeg");
    // 10:00:00:00 at 24 fps, and a movie with no timecode
    for (name, start) in [("tc.mov", Some(10 * 3600 * 24)), ("none.mov", None)] {
        let p = dir.join(name);
        std::fs::write(&p, mov(SampleEntry::jpeg(16, 16), &jpeg, start)).expect("write");
        let f = effectcraft_media::probe(&p).expect("probe");
        assert_eq!(f.kind, FootageKind::Video, "{name}");
        assert_eq!(f.file_timecode, start.map(i64::from), "{name}");
        assert_eq!(f.start_timecode, None, "{name}: Interpret Footage still uses the source file timecode");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn shared_mov_preserves_recorded_start_timecode() {
    use craft_media::isobmff::FourCc;
    use craft_video::prores::{ChromaFormat, Encoder, Frame, Profile};

    let dir = tempfile::tempdir().expect("temp dir");
    let profile = Profile::P4444;
    let frame = Frame::new(16, 16, ChromaFormat::Yuv444, 16, false).expect("ProRes frame");
    let packet = Encoder::new(profile, 16, 16).encode(&frame).expect("encode ProRes");
    for (name, start) in [("tc.mov", Some(10 * 3600 * 24)), ("none.mov", None)] {
        let entry = SampleEntry::prores(FourCc(profile.fourcc()), 16, 16);
        let bytes = mov(entry, &packet, start);
        let path = dir.path().join(name);
        std::fs::write(&path, &bytes).expect("write MOV");

        let from_file = effectcraft_media::probe(&path).expect("probe file");
        let from_bytes = effectcraft_media::probe_bytes(name, bytes.into()).expect("probe bytes");
        for footage in [from_file, from_bytes] {
            assert_eq!(footage.kind, FootageKind::Video, "{name}");
            // FilmCraft labels the same sample entry "Apple ProRes 4444".
            assert_eq!(footage.codec, "ProRes", "{name}: must use the shared reader");
            assert_eq!(footage.file_timecode, start.map(i64::from), "{name}");
            assert_eq!(footage.start_timecode, None, "{name}: no Interpret Footage override");
        }
    }
}

#[test]
fn shared_mov_timecode_rebases_edl_labels_on_its_own_presentation_clock() {
    use craft_media::isobmff::{self, CodecConfig, Edit, FourCc, TrackKind};
    use craft_video::prores::{ChromaFormat, Encoder, Frame, Profile};
    use effectcraft_media::{MediaPool, probe_bytes};
    use effectcraft_time::{FrameRate, Tick};

    const START: u32 = 10 * 3600 * 24;
    const MOVIE_CLOCK: u32 = 24;
    const VIDEO_CLOCK: u32 = 48;
    const TC_CLOCK: u32 = 96;
    const UNIT_RATE: i32 = 0x0001_0000;
    let edit = |segment_duration, media_time| Edit { segment_duration, media_time, media_rate: UNIT_RATE };

    let profile = Profile::P4444;
    let mut encoder = Encoder::new(profile, 16, 16);
    let mut frame = Frame::new(16, 16, ChromaFormat::Yuv444, 16, false).expect("ProRes frame");
    let packets = [40, 120, 200].map(|luma| {
        frame.y.fill(luma << 8);
        encoder.encode(&frame).expect("encode ProRes")
    });

    // The timecode track is written independently to vary its real media edit,
    // rather than accidentally using the video's edit as a timecode adjustment.
    let make_mov = |video_edits: Vec<Edit>, tc_edits: Vec<Edit>, counter: u32, movie_clock: u32| {
        let mut opts = WriterOptions::new(Brand::Mov);
        opts.movie_timescale = movie_clock;
        let mut w = Mp4Writer::new(Cursor::new(Vec::new()), opts).expect("writer");
        let mut video_cfg = TrackConfig::new(SampleEntry::prores(FourCc(profile.fourcc()), 16, 16), VIDEO_CLOCK);
        video_cfg.edits = video_edits;
        let video = w.add_track(video_cfg).expect("video track");

        let tc = TimecodeConfig { timescale: TC_CLOCK, frame_duration: 4, frames_per_second: 24, ..Default::default() };
        let entry = SampleEntry {
            format: FourCc(*b"tmcd"),
            data_reference_index: 1,
            codec: CodecConfig::Timecode(tc),
            video: None,
            audio: None,
            bitrate: None,
        };
        let mut tc_cfg = TrackConfig::new(entry, TC_CLOCK);
        tc_cfg.edits = tc_edits;
        let tc = w.add_track(tc_cfg).expect("timecode track");
        for packet in &packets {
            w.write_sample(video, WriteSample { data: packet, duration: 2, composition_offset: 0, is_sync: true }).expect("write ProRes");
        }
        w.write_sample(tc, WriteSample { data: &counter.to_be_bytes(), duration: 12, composition_offset: 0, is_sync: true })
            .expect("write timecode");
        w.finish().expect("finish").into_inner()
    };

    let pool = MediaPool::new();
    let plain: std::sync::Arc<[u8]> = make_mov(vec![], vec![], START, MOVIE_CLOCK).into();
    let unedited = probe_bytes("plain.mov", plain.clone()).expect("probe unedited MOV");
    assert_eq!(unedited.file_timecode, Some(i64::from(START)));
    pool.add_bytes("plain.mov", plain);
    let at = |presentation_frame| Tick::from_units(presentation_frame, i64::from(MOVIE_CLOCK));
    let pixels = [0, 1, 2].map(|n| pool.frame_at(&unedited, at(n)).expect("decode unedited reference").data[0]);
    assert!(pixels[0][0] < pixels[1][0] && pixels[1][0] < pixels[2][0], "reference pictures must be distinct");

    let cases = [
        ("matched-trim.mov", vec![edit(2, 2)], vec![edit(2, 4)], START, -4, START + 1, [(START + 1, 0, 1), (START + 2, 1, 2)]),
        ("video-only-trim.mov", vec![edit(2, 2)], vec![], START + 1, 0, START + 1, [(START + 1, 0, 1), (START + 2, 1, 2)]),
        ("matched-delay.mov", vec![edit(1, -1), edit(3, 0)], vec![edit(1, -1), edit(3, 0)], START, 4, START - 1, [(START, 1, 0), (START + 1, 2, 1)]),
    ];
    for (name, video_edits, tc_edits, counter, tc_pts, expected_origin, points) in cases {
        let bytes: std::sync::Arc<[u8]> = make_mov(video_edits, tc_edits, counter, MOVIE_CLOCK).into();
        let parsed = isobmff::open(bytes.as_ref()).expect("parse generated MOV");
        let tc = parsed.tracks.iter().find(|track| track.kind == TrackKind::Timecode).expect("timecode track");
        assert_eq!(tc.presentation_pts(0), Some(tc_pts), "{name}: fixture timecode edit");
        let footage = probe_bytes(name, bytes.clone()).expect("probe edited MOV");
        assert_eq!(footage.codec, "ProRes", "{name}: must use the shared reader");
        assert_eq!(footage.native_rate.unwrap_or(footage.frame_rate), FrameRate::FPS_24);
        let origin = footage.file_timecode.expect("usable presentation timecode");
        assert_eq!(origin, i64::from(expected_origin), "{name}");
        pool.add_bytes(name, bytes);

        for (edl_label, presentation_frame, picture) in points {
            // The EDL importer subtracts this origin at the native video rate.
            let rebased = i64::from(edl_label) - origin;
            assert_eq!(rebased, presentation_frame, "{name}: EDL in-point");
            let image = pool.frame_at(&footage, at(rebased)).expect("decode rebased in-point");
            assert_eq!(image.data[0], pixels[picture], "{name}: decoded source picture");
        }
    }

    let disjoint = make_mov(vec![edit(1, 0), edit(1, 4)], vec![], START, MOVIE_CLOCK);
    let footage = probe_bytes("disjoint.mov", disjoint.into()).expect("probe disjoint MOV");
    assert_eq!(footage.codec, "ProRes", "ambiguous timecode alone must not force a decoder fallback");
    assert_eq!(footage.file_timecode, None, "a single EDL origin cannot describe disjoint edits");

    // One millisecond cannot be represented in the 96 Hz timecode track.
    // The demuxer reports zero after conversion, but the EDL origin is ambiguous.
    let fractional = make_mov(vec![], vec![edit(1, -1), edit(125, 0)], START, 1000);
    let parsed = isobmff::open(fractional.as_slice()).expect("parse fractional delay MOV");
    let tc = parsed.tracks.iter().find(|track| track.kind == TrackKind::Timecode).expect("timecode track");
    assert_eq!(tc.presentation_pts(0), Some(0), "demux rounded the empty edit");
    let footage = probe_bytes("fractional.mov", fractional.into()).expect("probe fractional delay MOV");
    assert_eq!(footage.file_timecode, None, "do not turn a fractional delay into a fabricated origin");
}
