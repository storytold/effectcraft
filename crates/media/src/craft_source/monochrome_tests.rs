use super::*;
use crate::stream::{self, MediaSource as OpenedSource};

#[test]
fn shared_monochrome_av1_retains_luma_on_backward_seek() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let bytes: Arc<[u8]> = Arc::from(include_bytes!("../../tests/fixtures/monochrome-av1.mkv").as_slice());
    let OpenedSource::Craft(source) = stream::open_bytes("monochrome-av1.mkv", bytes)? else {
        panic!("expected the shared AV1 reader");
    };
    for i in [2u8, 0, 1] {
        let pts = Tick::from_units(i64::from(i), 25);
        let frame = source.video_frame(FrameRequest { time: pts, scale: 1.0 })?;
        assert_eq!((frame.width, frame.height, frame.pts, frame.color.range), (16, 12, pts, Range::Full));
        let PixelData::Yuv8 { planes: [y, cb, cr], chroma: Chroma::C420, alpha: None } = &frame.data else {
            panic!("expected 8-bit luma with neutral chroma");
        };
        let expected: Vec<u8> = (0..12).flat_map(|row| (0..16).map(move |col| 16 + 64 * i + col + 2 * row)).collect();
        assert_eq!(y.as_slice(), expected);
        assert_eq!(cb.as_slice(), [128; 48]);
        assert_eq!(cr.as_slice(), cb.as_slice());
        let rgba = frame.to_rgba8()?;
        for (pixel, &sample) in rgba.as_chunks::<4>().0.iter().zip(&expected) {
            assert_eq!(*pixel, [sample, sample, sample, 255]);
        }
    }
    Ok(())
}

#[test]
fn monochrome_10_bit_preserves_aperture_rotation_alpha_and_layout() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let y = Plane::packed(5, 5, 10, Samples::U16((64..89).collect()))?;
    let alpha = Plane::packed(5, 5, 10, Samples::U16((900..925).collect()))?;
    let mut decoded = DecodedVideoFrame::new(5, 5, ColorFamily::Gray, Subsampling::S444, vec![y], Some(alpha))?;
    decoded.sar = Some((3, 2));
    let aperture = isobmff::CleanAperture { width: (3, 1), height: (3, 1), horiz_offset: (0, 1), vert_offset: (0, 1) };
    let presentation = FramePresentation { container_color: Some(ColorInfo::REC709), par: (1, 1), rotation: 1, aperture: Some(aperture) };
    let pts = Tick::from_units(4, 25);
    let frame = to_craft_frame(decoded, presentation, pts)?;
    assert_eq!((frame.width, frame.height, frame.par, frame.pts), (3, 3, (2, 3), pts));
    assert_eq!(frame.color, ColorInfo::REC709);
    let PixelData::Yuv16 { planes: [y, cb, cr], chroma: Chroma::C420, bits: 10, alpha: Some(alpha) } = &frame.data else {
        panic!("expected 10-bit luma and alpha");
    };
    assert_eq!(y.as_slice(), [80, 75, 70, 81, 76, 71, 82, 77, 72]);
    assert_eq!(alpha.as_slice(), [916, 911, 906, 917, 912, 907, 918, 913, 908]);
    assert_eq!(cb.as_slice(), [512; 4]);
    assert_eq!(cr.as_slice(), cb.as_slice());
    Ok(())
}
