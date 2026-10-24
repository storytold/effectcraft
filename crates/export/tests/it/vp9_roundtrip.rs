//! The VP9 intra encoder (`effectcraft-vp9enc`) round-trips through FilmCraft's VP9 decoder (and
//! ffmpeg as an external oracle when installed): sizes, lossless exactness, PSNR.

use effectcraft_vp9enc::{EncoderConfig, Vp9Encoder};
use filmcraft_vp9::{Decoder, Plane};

fn plane_u8(p: &Plane) -> Vec<u8> {
    match p {
        Plane::U8(v) => v.clone(),
        Plane::U16(v) => v.iter().map(|x| *x as u8).collect(),
    }
}

/// Synthetic planes: gradients, noise and sharp edges.
fn source(w: usize, h: usize, kind: u32) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut s = 99u32 + kind;
    let mut rnd = || {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        (s >> 24) as u8
    };
    let y: Vec<u8> = (0..w * h)
        .map(|i| {
            let (x, yy) = (i % w, i / w);
            match kind {
                0 => ((x * 255) / w.max(1)) as u8 / 2 + ((yy * 255) / h.max(1)) as u8 / 2,
                1 => rnd(),
                _ => {
                    if (x / 7 + yy / 5) % 2 == 0 {
                        30
                    } else {
                        220
                    }
                }
            }
        })
        .collect();
    let u: Vec<u8> = (0..cw * ch).map(|i| (64 + (i % cw) * 128 / cw.max(1)) as u8).collect();
    let v: Vec<u8> = (0..cw * ch).map(|i| (200 - (i / cw) * 100 / ch.max(1)) as u8).collect();
    (y, u, v)
}

fn decode(frame: &[u8]) -> filmcraft_vp9::Picture {
    let mut d = Decoder::new();
    let mut pics = d.decode(frame, 0).expect("decode");
    assert_eq!(pics.len(), 1);
    pics.remove(0)
}

fn crop(p: &[u8], stride: usize, w: usize, h: usize) -> Vec<u8> {
    (0..h).flat_map(|y| p[y * stride..y * stride + w].to_vec()).collect()
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let mse: f64 = a.iter().zip(b).map(|(x, y)| (*x as f64 - *y as f64).powi(2)).sum::<f64>() / a.len() as f64;
    if mse == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() }
}

#[test]
fn lossless_is_exact_for_odd_sizes() {
    for (w, h) in [(1, 1), (33, 17), (64, 64), (200, 120), (7, 70)] {
        for kind in 0..3 {
            let (y, u, v) = source(w, h, kind);
            let mut e = Vp9Encoder::new(EncoderConfig { width: w as u32, height: h as u32, quality: 100, full_range: false, ..Default::default() });
            let f = e.encode_yuv420(&y, &u, &v);
            let p = decode(&f);
            assert_eq!((p.width, p.height), (w as u32, h as u32));
            let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
            assert_eq!(crop(&plane_u8(&p.y), p.y_stride, w, h), y, "{w}x{h} kind {kind}: luma");
            assert_eq!(crop(&plane_u8(&p.u), p.uv_stride, cw, ch), u, "{w}x{h} kind {kind}: u");
            assert_eq!(crop(&plane_u8(&p.v), p.uv_stride, cw, ch), v, "{w}x{h} kind {kind}: v");
        }
    }
}

#[test]
fn lossy_quality_and_size() {
    let (w, h) = (200, 120);
    for kind in [0, 2] {
        let (y, u, v) = source(w, h, kind);
        let mut last = 0usize;
        for q in [30u8, 60, 80, 95] {
            let mut e = Vp9Encoder::new(EncoderConfig { width: w as u32, height: h as u32, quality: q, full_range: false, ..Default::default() });
            let f = e.encode_yuv420(&y, &u, &v);
            let p = decode(&f);
            let got = crop(&plane_u8(&p.y), p.y_stride, w, h);
            let db = psnr(&got, &y);
            let min = match q {
                30 => 20.0,
                60 => 28.0,
                80 => 32.0,
                _ => 38.0,
            };
            assert!(db > min, "kind {kind} q{q}: PSNR {db:.1} dB, {} bytes", f.len());
            assert!(f.len() >= last, "higher quality, more bytes");
            last = f.len();
        }
    }
}

#[test]
fn wide_frames_use_tile_columns() {
    // 4160 px > 64 superblocks: two tile columns are required.
    let (w, h) = (4160, 16);
    let (y, u, v) = source(w, h, 0);
    let mut e = Vp9Encoder::new(EncoderConfig { width: w as u32, height: h as u32, quality: 90, full_range: false, ..Default::default() });
    let f = e.encode_yuv420(&y, &u, &v);
    let p = decode(&f);
    assert!(psnr(&crop(&plane_u8(&p.y), p.y_stride, w, h), &y) > 35.0);
}

#[test]
fn ffmpeg_decodes_our_stream() {
    let (w, h) = (96, 64);
    let (y, u, v) = source(w, h, 0);
    let mut e = Vp9Encoder::new(EncoderConfig { width: w as u32, height: h as u32, quality: 100, full_range: false, ..Default::default() });
    let frame = e.encode_yuv420(&y, &u, &v);
    // IVF container (one frame).
    let mut ivf = Vec::new();
    ivf.extend_from_slice(b"DKIF");
    ivf.extend_from_slice(&0u16.to_le_bytes());
    ivf.extend_from_slice(&32u16.to_le_bytes());
    ivf.extend_from_slice(b"VP90");
    ivf.extend_from_slice(&(w as u16).to_le_bytes());
    ivf.extend_from_slice(&(h as u16).to_le_bytes());
    ivf.extend_from_slice(&30u32.to_le_bytes());
    ivf.extend_from_slice(&1u32.to_le_bytes());
    ivf.extend_from_slice(&1u32.to_le_bytes());
    ivf.extend_from_slice(&0u32.to_le_bytes());
    ivf.extend_from_slice(&(frame.len() as u32).to_le_bytes());
    ivf.extend_from_slice(&0u64.to_le_bytes());
    ivf.extend_from_slice(&frame);
    let dir = std::env::temp_dir().join(format!("effectcraft-vp9-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.ivf");
    std::fs::write(&path, &ivf).unwrap();
    let out = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(&path)
        .args(["-f", "rawvideo", "-pix_fmt", "yuv420p", "-"])
        .output();
    let Ok(out) = out else {
        eprintln!("ffmpeg not found: skipping oracle");
        return;
    };
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(&out.stdout[..w * h], &y[..], "ffmpeg luma matches (lossless)");
    assert_eq!(&out.stdout[w * h..w * h + u.len()], &u[..]);
}

// ---------------------------------------------------------------- inter frames

/// Frame `t` of a moving sequence: a diagonal gradient with a sinusoidal texture and bright
/// squares, all translating by a fractional amount per frame (`kind` 1 adds noise that changes
/// every frame).
fn moving(w: usize, h: usize, t: usize, kind: u32) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let (dx, dy) = (1.75 * t as f64, 0.6 * t as f64);
    let mut s = 1234u32 + t as u32 * 77;
    let mut rnd = || {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        (s >> 28) as f64
    };
    let lum = |x: f64, y: f64| -> f64 {
        let g = 40.0 + 120.0 * (x / w as f64) + 50.0 * (y / h as f64);
        let tex = 18.0 * ((x * 0.31).sin() * (y * 0.23).cos());
        let sq = if (x - 20.0).rem_euclid(64.0) < 18.0 && (y - 10.0).rem_euclid(48.0) < 14.0 { 45.0 } else { 0.0 };
        g + tex + sq
    };
    let mut y = vec![0u8; w * h];
    for yy in 0..h {
        for x in 0..w {
            let n = if kind == 1 { rnd() - 7.5 } else { 0.0 };
            y[yy * w + x] = (lum(x as f64 - dx, yy as f64 - dy) + n).round().clamp(0.0, 255.0) as u8;
        }
    }
    let u: Vec<u8> = (0..cw * ch).map(|i| (100.0 + 40.0 * (((i % cw) as f64 * 2.0 - dx) / w as f64)).round() as u8).collect();
    let v: Vec<u8> = (0..cw * ch).map(|i| (150.0 - 30.0 * (((i / cw) as f64 * 2.0 - dy) / h as f64)).round() as u8).collect();
    (y, u, v)
}

fn planes_of(p: &filmcraft_vp9::Picture, w: usize, h: usize) -> [Vec<u8>; 3] {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    [crop(&plane_u8(&p.y), p.y_stride, w, h), crop(&plane_u8(&p.u), p.uv_stride, cw, ch), crop(&plane_u8(&p.v), p.uv_stride, cw, ch)]
}

/// Encode `n` frames and decode them with one decoder: every decoded frame must equal the
/// encoder's reconstruction. Returns (total bytes, mean luma PSNR vs the source).
fn run_sequence(cfg: EncoderConfig, n: usize, kind: u32) -> (usize, f64) {
    let (w, h) = (cfg.width as usize, cfg.height as usize);
    let mut e = Vp9Encoder::new(cfg);
    let mut d = Decoder::new();
    let (mut bytes, mut db) = (0usize, 0.0);
    for t in 0..n {
        let (y, u, v) = moving(w, h, t, kind);
        let f = e.encode_frame(&y, &u, &v, false);
        bytes += f.data.len();
        let kind_s = if f.key { "key" } else { "inter" };
        let mut pics = d.decode(&f.data, t as i64).unwrap_or_else(|err| panic!("{w}x{h} frame {t} ({kind_s}): {err:?}"));
        assert_eq!(pics.len(), 1);
        let got = planes_of(&pics.remove(0), w, h);
        let rec = e.reconstruction().expect("reconstruction");
        for k in 0..3 {
            if got[k] != rec[k] {
                let first = got[k].iter().zip(&rec[k]).position(|(a, b)| a != b).unwrap_or(0);
                let pw = if k == 0 { w } else { w.div_ceil(2) };
                panic!("{w}x{h} frame {t} ({kind_s}, q{} lf{}): plane {k} differs first at ({}, {})", f.q_idx, f.filter_level, first % pw, first / pw);
            }
        }
        db += psnr(&got[0], &y);
    }
    (bytes, db / n as f64)
}

#[test]
fn inter_frames_decode_exactly() {
    for (w, h) in [(64, 64), (96, 72), (33, 17), (130, 66)] {
        for (q, kind) in [(30u8, 0u32), (70, 1), (92, 0), (100, 0)] {
            let cfg = EncoderConfig { width: w as u32, height: h as u32, quality: q, keyframe_interval: 6, ..Default::default() };
            run_sequence(cfg, 8, kind);
        }
    }
}

#[test]
fn inter_frames_with_tile_columns_decode_exactly() {
    let cfg = EncoderConfig { width: 4160, height: 24, quality: 80, keyframe_interval: 0, ..Default::default() };
    run_sequence(cfg, 3, 0);
}

#[test]
fn inter_psnr_rises_with_bitrate() {
    let (w, h) = (160, 96);
    let mut last = (0usize, 0.0f64);
    let mut rows = vec![];
    for kbps in [60u32, 150, 400, 1200] {
        let cfg = EncoderConfig { width: w, height: h, quality: 60, target_kbps: Some(kbps), frame_rate: 30.0, keyframe_interval: 30, ..Default::default() };
        let (bytes, db) = run_sequence(cfg, 20, 0);
        rows.push(format!("{kbps} kbps target: {:.0} kbps actual, {db:.2} dB", bytes as f64 * 8.0 * 30.0 / 20.0 / 1000.0));
        assert!(bytes > last.0 && db > last.1, "{rows:#?}");
        last = (bytes, db);
    }
    eprintln!("{rows:#?}");
}

#[test]
fn inter_frames_are_much_smaller_than_intra() {
    let (w, h) = (192u32, 108u32);
    let mut report = vec![];
    for q in [50u8, 75, 90] {
        let intra = EncoderConfig { width: w, height: h, quality: q, keyframe_interval: 1, ..Default::default() };
        let inter = EncoderConfig { keyframe_interval: 0, ..intra.clone() };
        let (bi, pi) = run_sequence(intra, 24, 0);
        let (bp, pp) = run_sequence(inter, 24, 0);
        report.push(format!("q{q}: intra {bi} B {pi:.2} dB, inter {bp} B {pp:.2} dB, ratio {:.2}", bi as f64 / bp as f64));
        assert!(pp > pi - 1.0, "{report:#?}");
        assert!(bi as f64 >= 3.0 * bp as f64, "{report:#?}");
    }
    eprintln!("{report:#?}");
}

/// Encode `n` frames of `moving(.., kind)`: (frames, the encoder's reconstruction of each).
fn encode_moving(cfg: EncoderConfig, n: usize, kind: u32) -> (Vec<Vec<u8>>, Vec<[Vec<u8>; 3]>) {
    let (w, h) = (cfg.width as usize, cfg.height as usize);
    let mut e = Vp9Encoder::new(cfg);
    (0..n)
        .map(|t| {
            let (y, u, v) = moving(w, h, t, kind);
            let f = e.encode_frame(&y, &u, &v, false);
            (f.data, e.reconstruction().expect("recon"))
        })
        .unzip()
}

/// Decode `frames` with ffmpeg (an IVF file) and check every frame against `recon`, with nothing
/// on ffmpeg's error log. Skipped without ffmpeg.
fn ffmpeg_matches(w: usize, h: usize, frames: &[Vec<u8>], recon: &[[Vec<u8>; 3]], name: &str) {
    let mut ivf = Vec::new();
    ivf.extend_from_slice(b"DKIF");
    ivf.extend_from_slice(&0u16.to_le_bytes());
    ivf.extend_from_slice(&32u16.to_le_bytes());
    ivf.extend_from_slice(b"VP90");
    ivf.extend_from_slice(&(w as u16).to_le_bytes());
    ivf.extend_from_slice(&(h as u16).to_le_bytes());
    ivf.extend_from_slice(&30u32.to_le_bytes());
    ivf.extend_from_slice(&1u32.to_le_bytes());
    ivf.extend_from_slice(&(frames.len() as u32).to_le_bytes());
    ivf.extend_from_slice(&0u32.to_le_bytes());
    for (t, f) in frames.iter().enumerate() {
        ivf.extend_from_slice(&(f.len() as u32).to_le_bytes());
        ivf.extend_from_slice(&(t as u64).to_le_bytes());
        ivf.extend_from_slice(f);
    }
    let dir = std::env::temp_dir().join(format!("effectcraft-vp9-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.ivf");
    std::fs::write(&path, &ivf).unwrap();
    let out = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(&path)
        .args(["-f", "rawvideo", "-pix_fmt", "yuv420p", "-"])
        .output();
    let Ok(out) = out else {
        eprintln!("ffmpeg not found: skipping oracle");
        return;
    };
    assert!(out.status.success() && out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
    let fsz = w * h + 2 * w.div_ceil(2) * h.div_ceil(2);
    assert_eq!(out.stdout.len(), fsz * frames.len());
    for (t, r) in recon.iter().enumerate() {
        let frame = &out.stdout[t * fsz..(t + 1) * fsz];
        assert_eq!(&frame[..w * h], &r[0][..], "ffmpeg luma of frame {t}");
        assert_eq!(&frame[w * h..w * h + r[1].len()], &r[1][..], "ffmpeg U of frame {t}");
    }
}

#[test]
fn ffmpeg_decodes_inter_frames_like_the_encoder() {
    let (w, h) = (96usize, 64usize);
    let cfg = EncoderConfig { width: w as u32, height: h as u32, quality: 75, keyframe_interval: 5, ..Default::default() };
    let (frames, recon) = encode_moving(cfg, 7, 0);
    ffmpeg_matches(w, h, &frames, &recon, "inter");
}

/// A frame whose data would end in a byte that reads as a superframe marker (0b110xxxxx) gets a
/// zero byte after it; otherwise decoders take its tail for a superframe index and reject it
/// (#161: 3 of 60 frames of a WebM failed to decode). Such frames still decode exactly.
#[test]
fn frames_never_end_in_a_superframe_marker() {
    let marker = |b: u8| b & 0xe0 == 0xc0;
    let mut padded = None;
    for (w, h) in [(96usize, 64usize), (130, 66), (160, 96)] {
        for q in [40u8, 60, 75, 90] {
            let cfg = EncoderConfig { width: w as u32, height: h as u32, quality: q, keyframe_interval: 12, ..Default::default() };
            let (frames, recon) = encode_moving(cfg, 24, 1);
            for (t, f) in frames.iter().enumerate() {
                assert!(!f.last().copied().is_some_and(marker), "{w}x{h} q{q} frame {t} ends in {:#x}", f[f.len() - 1]);
            }
            let needed = frames.iter().any(|f| f.len() >= 2 && f[f.len() - 1] == 0 && marker(f[f.len() - 2]));
            if needed && padded.is_none() {
                padded = Some((w, h, q, frames, recon));
            }
        }
    }
    let Some((w, h, q, frames, recon)) = padded else { panic!("no frame needed the padding: widen the search") };
    // The padded frames decode exactly, with FilmCraft's decoder and with ffmpeg.
    let mut d = Decoder::new();
    for (t, (f, r)) in frames.iter().zip(&recon).enumerate() {
        let mut pics = d.decode(f, t as i64).unwrap_or_else(|e| panic!("{w}x{h} q{q} frame {t}: {e:?}"));
        assert_eq!(planes_of(&pics.remove(0), w, h), *r, "{w}x{h} q{q} frame {t}");
    }
    ffmpeg_matches(w, h, &frames, &recon, "marker");
}

/// Encoder speed on 720p (run with `--ignored --nocapture`).
#[test]
#[ignore]
fn bench_720p() {
    let (w, h) = (1280usize, 720usize);
    let mut e = Vp9Encoder::new(EncoderConfig { width: w as u32, height: h as u32, quality: 80, keyframe_interval: 0, ..Default::default() });
    for t in 0..6 {
        let (y, u, v) = moving(w, h, t, 0);
        let t0 = std::time::Instant::now();
        let f = e.encode_frame(&y, &u, &v, false);
        eprintln!("frame {t} ({}): {} B in {:.0} ms", if f.key { "key" } else { "inter" }, f.data.len(), t0.elapsed().as_secs_f64() * 1000.0);
    }
}
