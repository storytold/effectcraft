use std::io::Cursor;

use craft_video::ffv1::{self, Colorspace, Encoder, EncoderConfig};

use super::*;
use crate::stream::{self, MediaSource as OpenedSource};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn assert_samples(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    for (i, (&got, &want)) in actual.iter().zip(expected).enumerate() {
        assert!((got - want).abs() < 1e-6, "sample {i}: expected {want}, got {got}");
    }
}

#[test]
fn full_range_rgb_ffv1_keeps_wide_samples_across_backward_seek() -> TestResult {
    const WIDTH: u32 = 5;
    const HEIGHT: u32 = 3;
    let mut raw = ffv1::Frame { width: WIDTH, height: HEIGHT, colorspace: Colorspace::Rgb, bit_depth: 10, chroma_shift: (0, 0), planes: vec![Vec::new(); 3] };
    let encoder = Encoder::new(&raw, EncoderConfig::default())?;
    let mut track = matroska::TrackSpec::new(matroska::TrackKind::Video, "V_FFV1");
    track.codec_private = encoder.config_record();
    track.video_size = Some((WIDTH, HEIGHT));
    track.default_duration_ns = Some(40_000_000);
    let mut writer = matroska::MkvWriter::new(Cursor::new(Vec::new()), vec![track], matroska::MuxOptions::default())?;

    let mut expected = Vec::new();
    for n in 0u16..3 {
        let codes: Vec<[u16; 3]> = (0..WIDTH as u16 * HEIGHT as u16).map(|p| [256 + 96 * n + p, 500 + p, 910 - p]).collect();
        for (c, plane) in raw.planes.iter_mut().enumerate() {
            *plane = codes.iter().map(|pixel| pixel[c]).collect();
        }
        writer.write_frame(0, i64::from(n) * 40_000_000, true, &encoder.encode(&raw)?, Some(40_000_000))?;
        // The existing Rec.709 fallback uses the sRGB preview curve. All fixture codes are
        // above its linear segment, and adjacent codes must survive the full conversion.
        expected.push(
            codes
                .into_iter()
                .flat_map(|pixel| {
                    let [r, g, b] = pixel.map(|c| ((f32::from(c) / 1023.0 + 0.055) / 1.055).powf(2.4));
                    [r, g, b, 1.0]
                })
                .collect::<Vec<_>>(),
        );
    }
    let bytes = writer.finish()?.into_inner();
    let OpenedSource::Craft(source) = stream::open_bytes("wide-rgb-ffv1.mkv", bytes.into())? else {
        panic!("expected the shared FFV1 reader");
    };
    for n in [2, 0, 1] {
        let pts = Tick::from_units(n as i64, 25);
        let frame = source.video_frame(FrameRequest { time: pts, scale: 1.0 })?;
        assert_eq!((frame.width, frame.height, frame.pts), (WIDTH, HEIGHT, pts));
        assert_eq!((frame.color.range, frame.color.transfer), (Range::Full, Transfer::Linear));
        let PixelData::RgbaF32(pixels) = &frame.data else { panic!("expected linear floating-point RGBA") };
        assert_samples(pixels, &expected[n]);
    }
    Ok(())
}

#[test]
fn full_range_wide_rgb_crops_rotates_and_premultiplies() -> TestResult {
    let plane = |rows| Plane::new(3, 2, 4, 10, Samples::U16(rows));
    let red = plane(vec![8, 1023, 11, 999, 12, 1023, 13, 999])?;
    let green = plane(vec![20, 512, 21, 999, 22, 256, 23, 999])?;
    let blue = plane(vec![24, 1, 25, 999, 26, 1023, 27, 999])?;
    let alpha = plane(vec![0, 512, 0, 999, 0, 0, 0, 999])?;
    let mut decoded = DecodedVideoFrame::new(3, 2, ColorFamily::Rgb, Subsampling::S444, vec![red, green, blue], Some(alpha))?;
    decoded.sar = Some((5, 4));
    let aperture = isobmff::CleanAperture { width: (1, 1), height: (2, 1), horiz_offset: (0, 1), vert_offset: (0, 1) };
    let color = ColorInfo { transfer: Transfer::Linear, ..ColorInfo::SRGB_FULL };
    let presentation = FramePresentation { container_color: Some(color), par: (1, 1), rotation: 1, aperture: Some(aperture) };
    let pts = Tick::from_units(4, 25);
    let frame = to_craft_frame(decoded, presentation, pts)?;
    assert_eq!((frame.width, frame.height, frame.par, frame.pts, frame.color), (2, 1, (4, 5), pts, color));
    let PixelData::RgbaF32(pixels) = &frame.data else { panic!("expected linear floating-point RGBA") };
    let half = 512.0 / 1023.0;
    assert_samples(pixels, &[0.0, 0.0, 0.0, 0.0, half, half * half, half / 1023.0, half]);
    Ok(())
}
