//! A still's Exif orientation (a camera held sideways, a mirrored scan) is applied on import: the
//! footage has the size the picture is seen at, and its frames are the picture the right way up.

use std::sync::Arc;

use effectcraft_media::{MediaPool, probe_bytes};
use effectcraft_project::FootageKind;
use effectcraft_time::Tick;

const RED: [f32; 3] = [1.0, 0.0, 0.0];
const GREEN: [f32; 3] = [0.0, 1.0, 0.0];
const BLUE: [f32; 3] = [0.0, 0.0, 1.0];
const WHITE: [f32; 3] = [1.0, 1.0, 1.0];

/// A 32×16 JPEG with red, green / blue, white quarters, tagged with Exif orientation `tag`
/// (`None`: no Exif at all).
fn quartered_jpeg(tag: Option<u8>) -> Vec<u8> {
    let img = image::RgbImage::from_fn(32, 16, |x, y| {
        let c = match (x < 16, y < 8) {
            (true, true) => RED,
            (false, true) => GREEN,
            (true, false) => BLUE,
            (false, false) => WHITE,
        };
        image::Rgb(c.map(|v| (v * 255.0) as u8))
    });
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 100).encode_image(&img).expect("encode");
    let Some(tag) = tag else { return jpeg };
    // APP1 "Exif": a big-endian TIFF header and one IFD holding Orientation (0x0112, SHORT).
    let mut app1 = vec![0xFF, 0xE1, 0x00, 0x22];
    app1.extend_from_slice(b"Exif\0\0MM\0\x2A\0\0\0\x08");
    app1.extend_from_slice(&[0x00, 0x01, 0x01, 0x12, 0x00, 0x03, 0x00, 0x00, 0x00, 0x01, 0x00, tag, 0x00, 0x00]);
    app1.extend_from_slice(&[0x00; 4]);
    // (Straight after the SOI marker.)
    jpeg.splice(2..2, app1);
    jpeg
}

/// The footage size and the colours of its four corners (top-left, top-right, bottom-left,
/// bottom-right), as imported.
fn imported(tag: Option<u8>) -> ((u32, u32), [[f32; 3]; 4]) {
    let name = format!("oriented-{tag:?}.jpg");
    let bytes: Arc<[u8]> = quartered_jpeg(tag).into();
    let f = probe_bytes(&name, bytes.clone()).expect("probe");
    assert_eq!(f.kind, FootageKind::Still);
    let pool = MediaPool::new();
    pool.add_bytes(&name, bytes);
    let img = pool.frame_at(&f, Tick::ZERO).expect("frame");
    assert_eq!((img.width, img.height), (f.width, f.height), "orientation {tag:?}: the frame has the probed size");
    let (w, h) = (img.width as usize, img.height as usize);
    let at = |x: usize, y: usize| {
        let p = img.data[y * w + x];
        [p[0], p[1], p[2]]
    };
    ((f.width, f.height), [at(2, 2), at(w - 3, 2), at(2, h - 3), at(w - 3, h - 3)])
}

#[test]
fn exif_orientation_is_applied_to_stills() {
    let (landscape, portrait) = ((32, 16), (16, 32));
    let cases = [
        (None, landscape, [RED, GREEN, BLUE, WHITE]),
        (Some(1), landscape, [RED, GREEN, BLUE, WHITE]),
        (Some(2), landscape, [GREEN, RED, WHITE, BLUE]),
        (Some(3), landscape, [WHITE, BLUE, GREEN, RED]),
        (Some(4), landscape, [BLUE, WHITE, RED, GREEN]),
        (Some(5), portrait, [RED, BLUE, GREEN, WHITE]),
        (Some(6), portrait, [BLUE, RED, WHITE, GREEN]),
        (Some(7), portrait, [WHITE, GREEN, BLUE, RED]),
        (Some(8), portrait, [GREEN, WHITE, RED, BLUE]),
        // (Not an orientation: the picture as stored.)
        (Some(0), landscape, [RED, GREEN, BLUE, WHITE]),
        (Some(9), landscape, [RED, GREEN, BLUE, WHITE]),
    ];
    for (tag, size, corners) in cases {
        let (got_size, got) = imported(tag);
        assert_eq!(got_size, size, "orientation {tag:?}: size");
        for (i, (g, want)) in got.iter().zip(corners).enumerate() {
            let near = g.iter().zip(want).all(|(a, b)| (a - b).abs() < 0.2);
            assert!(near, "orientation {tag:?}: corner {i} is {g:?}, expected {want:?}");
        }
    }
}
