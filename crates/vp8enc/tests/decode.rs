//! The encoder's frames read back by decoders that are not this crate: ffmpeg (when installed)
//! must reproduce the encoder's own reconstruction exactly, and the `image` crate's WebP decoder
//! must read the frame as a picture close to the source.

use std::path::{Path, PathBuf};
use std::process::Command;

use effectcraft_vp8enc::{Vp8Frame, Vp8Params, encode_rgba};

/// A test picture with something for every mode: flat areas, gradients, hard edges, diagonal
/// stripes, fine noise, colour.
fn picture(w: u32, h: u32, seed: u32) -> Vec<u8> {
    let mut x = seed.wrapping_mul(2654435761) | 1;
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for py in 0..h {
        for px in 0..w {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let noise = (x % 64) as i32 - 32;
            let (fx, fy) = (px as f32 / w as f32, py as f32 / h as f32);
            let (r, g, b) = if fx < 0.25 {
                (200, 40, 40) // flat
            } else if fx < 0.5 {
                ((fx * 400.0) as i32, (fy * 255.0) as i32, 128) // gradient
            } else if fx < 0.75 {
                let stripe = ((px + py) / 3) % 2 == 0; // diagonal stripes
                if stripe { (240, 240, 240) } else { (20, 30, 90) }
            } else {
                (128 + noise * 2, 100 + noise, 160 - noise) // noise
            };
            let c = |v: i32| v.clamp(0, 255) as u8;
            out.extend_from_slice(&[c(r), c(g), c(b), 255]);
        }
    }
    out
}

/// A VP8 frame wrapped as a still WebP file.
fn riff(vp8: &[u8]) -> Vec<u8> {
    let mut chunk = b"VP8 ".to_vec();
    chunk.extend_from_slice(&(vp8.len() as u32).to_le_bytes());
    chunk.extend_from_slice(vp8);
    if vp8.len() % 2 == 1 {
        chunk.push(0);
    }
    let mut f = b"RIFF".to_vec();
    f.extend_from_slice(&(4 + chunk.len() as u32).to_le_bytes());
    f.extend_from_slice(b"WEBP");
    f.extend_from_slice(&chunk);
    f
}

/// Where the tests write their files.
fn out_dir() -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/vp8enc");
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

/// Whether ffmpeg is on the path.
fn have_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success())
}

/// ffmpeg's decode of the frame as raw 4:2:0 planes at the picture size.
fn ffmpeg_yuv(frame: &Vp8Frame, name: &str) -> Vec<u8> {
    let path = out_dir().join(format!("{name}.webp"));
    std::fs::write(&path, riff(&frame.data)).expect("write");
    let out = Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(&path).args(["-f", "rawvideo", "-pix_fmt", "yuv420p", "-"]).output().expect("ffmpeg");
    assert!(out.status.success() && out.stderr.is_empty(), "ffmpeg rejected {name}: {}", String::from_utf8_lossy(&out.stderr));
    out.stdout
}

/// The encoder's reconstruction cropped to the picture, in the same layout.
fn reconstruction(frame: &Vp8Frame, w: u32, h: u32) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = Vec::with_capacity(w * h + 2 * cw * ch);
    for y in 0..h {
        for x in 0..w {
            out.push(frame.y.get(x, y));
        }
    }
    for p in [&frame.u, &frame.v] {
        for y in 0..ch {
            for x in 0..cw {
                out.push(p.get(x, y));
            }
        }
    }
    out
}

/// ffmpeg decodes the frame to exactly the encoder's own reconstruction.
#[test]
fn a_decoder_reconstructs_exactly_what_the_encoder_did() {
    if !have_ffmpeg() {
        eprintln!("ffmpeg not found: skipping the exact reconstruction check");
        return;
    }
    for (w, h, quality) in [(64, 48, 90), (33, 17, 50), (100, 75, 10), (16, 16, 100), (130, 66, 75), (1, 1, 60)] {
        let rgba = picture(w, h, w * 31 + h);
        let frame = encode_rgba(&rgba, w, h, &Vp8Params { quality, loop_filter: Some(0) }).expect("encode");
        let name = format!("exact_{w}x{h}_q{quality}");
        let got = ffmpeg_yuv(&frame, &name);
        let want = reconstruction(&frame, w, h);
        assert_eq!(got.len(), want.len(), "{name}: plane sizes");
        if let Some(i) = got.iter().zip(&want).position(|(a, b)| a != b) {
            panic!("{name}: the decoder differs from the encoder's reconstruction at byte {i} of {} ({} vs {})", got.len(), got[i], want[i]);
        }
    }
}

/// The peak signal-to-noise ratio of `b` against `a`, in dB.
fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let se: f64 = a.iter().zip(b).map(|(&x, &y)| (f64::from(x) - f64::from(y)).powi(2)).sum();
    let mse = se / a.len() as f64;
    if mse == 0.0 { f64::INFINITY } else { 10.0 * (255.0f64 * 255.0 / mse).log10() }
}

/// The `image` crate decodes it close to the quality-100 decode.
#[test]
fn the_image_crate_reads_it_back_close_to_the_source() {
    // Against the decode at quality 100, so the comparison is of the coding alone and not of
    // the colour 4:2:0 cannot hold (this picture's fine coloured stripes and noise).
    let (w, h) = (96, 64);
    let rgba = picture(w, h, 7);
    let decode = |quality| {
        let frame = encode_rgba(&rgba, w, h, &Vp8Params { quality, loop_filter: None }).expect("encode");
        let img = image::load_from_memory_with_format(&riff(&frame.data), image::ImageFormat::WebP).expect("decode").to_rgba8();
        assert_eq!(img.dimensions(), (w, h));
        img.as_raw().as_chunks::<4>().0.iter().flat_map(|c| [c[0], c[1], c[2]]).collect::<Vec<u8>>()
    };
    let best = decode(100);
    let mut last = 0.0;
    for quality in [20u8, 60, 95] {
        let q = psnr(&decode(quality), &best);
        assert!(q >= last, "quality {quality} is no closer than a lower quality: {q:.1} dB after {last:.1}");
        last = q;
    }
    assert!(last > 38.0, "quality 95: {last:.1} dB");
}

/// A higher quality gives a larger frame.
#[test]
fn higher_quality_spends_more_bytes() {
    let (w, h) = (96, 64);
    let rgba = picture(w, h, 11);
    let size = |quality| encode_rgba(&rgba, w, h, &Vp8Params { quality, loop_filter: None }).expect("encode").data.len();
    let (low, mid, high) = (size(10), size(60), size(98));
    assert!(low < mid && mid < high, "{low} {mid} {high}");
}

/// Empty, short and oversized pictures are errors.
#[test]
fn bad_sizes_are_errors() {
    assert!(encode_rgba(&[], 0, 0, &Vp8Params::default()).is_err());
    assert!(encode_rgba(&[0; 12], 2, 2, &Vp8Params::default()).is_err());
    assert!(encode_rgba(&[0; 4], 16384, 1, &Vp8Params::default()).is_err());
}

/// Prints how much of the error is the coding and how much is 4:2:0 colour.
#[test]
#[ignore = "diagnostic: prints how the error divides between the codec and 4:2:0 colour"]
fn where_the_error_is() {
    let (w, h) = (96, 64);
    let rgba = picture(w, h, 7);
    std::fs::write(out_dir().join("src.rgba"), &rgba).expect("write");
    for quality in [20u8, 60, 95] {
        let frame = encode_rgba(&rgba, w, h, &Vp8Params { quality, loop_filter: None }).expect("encode");
        let lossless = encode_rgba(&rgba, w, h, &Vp8Params { quality: 100, loop_filter: Some(0) }).expect("encode");
        let crop =
            |p: &effectcraft_vp8enc::Plane| (0..h as usize).flat_map(|y| (0..w as usize).map(move |x| (x, y))).map(|(x, y)| p.get(x, y)).collect::<Vec<u8>>();
        // The encoder's own source luma is what quality 100 reconstructs to within a step.
        println!(
            "q{quality}: {} bytes, q_index {}, lf {}, luma PSNR vs q100 recon {:.1} dB",
            frame.data.len(),
            frame.q_index,
            frame.loop_filter,
            psnr(&crop(&frame.y), &crop(&lossless.y))
        );
        std::fs::write(out_dir().join(format!("diag_q{quality}.webp")), riff(&frame.data)).expect("write");
    }
}
