use std::io::Cursor;

use craft_video::ffv1::{self, Colorspace, Encoder, EncoderConfig};

use super::*;
use crate::stream::{self, MediaSource as OpenedSource};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

const WIDTH: u32 = 5;
const HEIGHT: u32 = 3;

fn rgba(frame: u8, x: u8, y: u8) -> [u8; 4] {
    [15 + 55 * frame + x, 59 + 3 * y, 191 + x + y, 255]
}

#[test]
fn shared_ffv1_rgb8_keeps_opaque_pixels_across_backward_seek() -> TestResult {
    let mut raw = ffv1::Frame { width: WIDTH, height: HEIGHT, colorspace: Colorspace::Rgb, bit_depth: 8, chroma_shift: (0, 0), planes: vec![Vec::new(); 3] };
    let encoder = Encoder::new(&raw, EncoderConfig::default())?;
    let mut track = matroska::TrackSpec::new(matroska::TrackKind::Video, "V_FFV1");
    track.codec_private = encoder.config_record();
    track.video_size = Some((WIDTH, HEIGHT));
    track.default_duration_ns = Some(40_000_000);
    let mut writer = matroska::MkvWriter::new(Cursor::new(Vec::new()), vec![track], matroska::MuxOptions::default())?;

    // Original per-channel ramps, encoded and muxed in memory.
    let mut expected = Vec::new();
    for n in 0u8..3 {
        let pixels: Vec<[u8; 4]> = (0..HEIGHT as u8).flat_map(|y| (0..WIDTH as u8).map(move |x| rgba(n, x, y))).collect();
        for (c, plane) in raw.planes.iter_mut().enumerate() {
            *plane = pixels.iter().map(|pixel| u16::from(pixel[c])).collect();
        }
        writer.write_frame(0, i64::from(n) * 40_000_000, true, &encoder.encode(&raw)?, Some(40_000_000))?;
        expected.push(pixels.into_flattened());
    }
    let bytes = writer.finish()?.into_inner();
    let OpenedSource::Craft(source) = stream::open_bytes("rgb-ffv1.mkv", bytes.into())? else {
        panic!("expected the shared FFV1 reader");
    };
    for n in [2, 0, 1] {
        let pts = Tick::from_units(n as i64, 25);
        let frame = source.video_frame(FrameRequest { time: pts, scale: 1.0 })?;
        assert_eq!((frame.width, frame.height, frame.pts, frame.color.range), (WIDTH, HEIGHT, pts, Range::Full));
        let PixelData::Rgba8(pixels) = &frame.data else { panic!("expected packed RGBA8") };
        assert_eq!(pixels.as_slice(), expected[n]);
    }
    Ok(())
}

fn presentation() -> FramePresentation {
    FramePresentation { container_color: Some(ColorInfo::SRGB_FULL), par: (1, 1), rotation: 0, aperture: None }
}

#[test]
fn packed_rgb_keeps_stride_aperture_rotation_and_straight_alpha() -> TestResult {
    let make_plane = |base: u8| {
        let samples: Vec<u8> = (0u8..3).flat_map(|y| [base + y * 4, base + y * 4 + 1, base + y * 4 + 2, 250]).collect();
        Plane::new(3, 3, 4, 8, Samples::U8(samples))
    };
    let mut decoded =
        DecodedVideoFrame::new(3, 3, ColorFamily::Rgb, Subsampling::S444, vec![make_plane(10)?, make_plane(30)?, make_plane(50)?], Some(make_plane(0)?))?;
    decoded.sar = Some((3, 2));
    let aperture = isobmff::CleanAperture { width: (1, 1), height: (3, 1), horiz_offset: (0, 1), vert_offset: (0, 1) };
    let presentation = FramePresentation { aperture: Some(aperture), rotation: 1, ..presentation() };
    let pts = Tick::from_units(4, 25);
    let frame = to_craft_frame(decoded, presentation, pts)?;
    assert_eq!((frame.width, frame.height, frame.par, frame.pts), (3, 1, (2, 3), pts));
    assert_eq!(frame.color, ColorInfo::SRGB_FULL);
    let PixelData::Rgba8(pixels) = &frame.data else { panic!("expected packed RGBA8") };
    assert_eq!(pixels.as_slice(), [19, 39, 59, 9, 15, 35, 55, 5, 11, 31, 51, 1]);
    Ok(())
}

#[test]
fn limited_range_high_depth_rgb_remains_unsupported() -> TestResult {
    let channel = Plane::packed(1, 1, 10, Samples::U16(vec![900]))?;
    let decoded = DecodedVideoFrame::new(1, 1, ColorFamily::Rgb, Subsampling::S444, vec![channel; 3], None)?;
    let color = ColorInfo { range: Range::Limited, ..ColorInfo::SRGB_FULL };
    let presentation = FramePresentation { container_color: Some(color), ..presentation() };
    assert!(matches!(to_craft_frame(decoded, presentation, Tick::ZERO), Err(MediaError::Unsupported(_))));
    Ok(())
}
