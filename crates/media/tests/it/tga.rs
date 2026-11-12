//! Tiny synthetic Targa files exercise import without external fixtures or codecs.

use std::path::Path;
use std::sync::Arc;

use effectcraft_media::{MediaPool, is_importable, probe_bytes};
use effectcraft_project::{AlphaMode, Footage, FootageKind};
use effectcraft_time::Tick;

/// A classic 2×2 TGA without a footer. True-colour pixels are stored BGR(A); RLE
/// packets repeat each row's colour twice. `pixels` is always in top-down RGBA order.
fn tga(pixels: [[u8; 4]; 4], alpha: bool, rle: bool, top: bool) -> Vec<u8> {
    let mut bytes = vec![0; 18];
    bytes[2] = if rle { 10 } else { 2 };
    bytes[12] = 2;
    bytes[14] = 2;
    bytes[16] = if alpha { 32 } else { 24 };
    bytes[17] = if top { 0x20 } else { 0 } | if alpha { 8 } else { 0 };
    let rows = if top { [[0, 1], [2, 3]] } else { [[2, 3], [0, 1]] };
    for row in rows {
        if rle {
            assert_eq!(pixels[row[0]], pixels[row[1]]);
            bytes.push(0x81); // A two-pixel run.
        }
        for &i in row.iter().take(if rle { 1 } else { 2 }) {
            let [r, g, b, a] = pixels[i];
            bytes.extend_from_slice(&[b, g, r]);
            if alpha {
                bytes.push(a);
            }
        }
    }
    bytes
}

#[test]
fn tga_raw_and_rle_preserve_colours_orientation_and_alpha() {
    for alpha in [false, true] {
        for rle in [false, true] {
            for top in [false, true] {
                let pixels = if rle {
                    [[255, 0, 0, 128], [255, 0, 0, 128], [0, 0, 255, 255], [0, 0, 255, 255]]
                } else {
                    [[255, 0, 0, 128], [0, 255, 0, 255], [0, 0, 255, 64], [255, 255, 255, 0]]
                };
                let path = if top { "/frame.TGA" } else { "/frame.tga" };
                assert!(is_importable(Path::new(path)));
                let bytes: Arc<[u8]> = tga(pixels, alpha, rle, top).into();
                let footage = probe_bytes(path, bytes.clone()).unwrap();
                assert_eq!(footage.kind, FootageKind::Still);
                assert_eq!((footage.width, footage.height), (2, 2));
                assert_eq!(footage.codec, "TGA");
                assert_eq!(footage.alpha, if alpha { AlphaMode::Straight } else { AlphaMode::Ignore });
                assert!(footage.has_video && !footage.has_audio);
                let pool = MediaPool::new();
                pool.add_bytes(path, bytes);
                let image = pool.frame_at(&footage, Tick::ZERO).unwrap();
                for (actual, [r, g, b, a]) in image.data.iter().zip(pixels) {
                    let a = if alpha { f32::from(a) / 255.0 } else { 1.0 };
                    let expected = [f32::from(r) / 255.0 * a, f32::from(g) / 255.0 * a, f32::from(b) / 255.0 * a, a];
                    assert!(actual.iter().zip(expected).all(|(a, b)| (a - b).abs() < 1e-6), "{actual:?} != {expected:?}");
                }
            }
        }
    }
}

#[test]
fn tga_sequence_decodes_each_frame_from_memory() {
    let pool = MediaPool::new();
    pool.set_read_ahead(false);
    let paths = ["/shot_0001.tga", "/shot_0002.tga"];
    let colours = [[255, 0, 0, 255], [0, 0, 255, 255]];
    let mut footage = None;
    for (path, colour) in paths.iter().zip(colours) {
        let bytes: Arc<[u8]> = tga([colour; 4], false, true, true).into();
        footage.get_or_insert_with(|| probe_bytes(path, bytes.clone()).unwrap());
        pool.add_bytes(path, bytes);
    }
    let mut footage = Footage { kind: FootageKind::Sequence, sequence: paths.iter().map(|s| s.to_string()).collect(), ..footage.unwrap() };
    footage.sync_sequence_duration();
    for (n, expected) in [[1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]].into_iter().enumerate() {
        let image = pool.frame_at(&footage, footage.frame_rate.tick_of(n as i64)).unwrap();
        assert_eq!(image.get(0, 0), expected);
    }
}

#[test]
fn malformed_tga_returns_contextual_errors() {
    for bytes in [vec![], vec![0; 17], vec![0; 18]] {
        let error = probe_bytes("/broken.tga", bytes.into()).unwrap_err().to_string();
        assert!(error.contains("/broken.tga"), "{error}");
    }
    let mut bytes = tga([[255, 0, 0, 255]; 4], true, false, true);
    bytes.truncate(19); // Valid metadata, truncated pixel data.
    let bytes: Arc<[u8]> = bytes.into();
    let footage = probe_bytes("/truncated.tga", bytes.clone()).unwrap();
    let pool = MediaPool::new();
    pool.add_bytes(&footage.path, bytes);
    let error = pool.frame_at(&footage, Tick::ZERO).unwrap_err().to_string();
    assert!(error.contains("/truncated.tga"), "{error}");
}

#[test]
fn a_recognised_signature_takes_precedence_over_the_tga_extension() {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(3, 2).write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    let bytes: Arc<[u8]> = bytes.into_inner().into();
    let footage = probe_bytes("/actually-png.tga", bytes.clone()).unwrap();
    assert_eq!(footage.codec, "PNG");
    let pool = MediaPool::new();
    pool.add_bytes(&footage.path, bytes);
    let image = pool.frame_at(&footage, Tick::ZERO).unwrap();
    assert_eq!((image.width, image.height), (3, 2));
}
