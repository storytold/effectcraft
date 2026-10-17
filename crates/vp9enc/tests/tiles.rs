//! Tile columns coded in parallel: frames wide enough for several tile columns round-trip through
//! FilmCraft's VP9 decoder exactly (decoded frame = the encoder's reconstruction), lossless stays
//! lossless across tile edges, and the output is the same on every run.

use effectcraft_vp9enc::{EncoderConfig, Vp9Encoder};
use filmcraft_vp9::{Decoder, Picture, Plane};

fn plane_u8(p: &Plane) -> Vec<u8> {
    match p {
        Plane::U8(v) => v.clone(),
        Plane::U16(v) => v.iter().map(|x| *x as u8).collect(),
    }
}

fn crop(p: &[u8], stride: usize, w: usize, h: usize) -> Vec<u8> {
    (0..h).flat_map(|y| p[y * stride..y * stride + w].to_vec()).collect()
}

fn planes_of(p: &Picture, w: usize, h: usize) -> [Vec<u8>; 3] {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    [crop(&plane_u8(&p.y), p.y_stride, w, h), crop(&plane_u8(&p.u), p.uv_stride, cw, ch), crop(&plane_u8(&p.v), p.uv_stride, cw, ch)]
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let mse: f64 = a.iter().zip(b).map(|(x, y)| (*x as f64 - *y as f64).powi(2)).sum::<f64>() / a.len() as f64;
    if mse == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() }
}

/// Frame `t` of a moving sequence: gradient, texture and squares translating by a fractional
/// amount per frame, plus a little noise.
fn moving(w: usize, h: usize, t: usize) -> [Vec<u8>; 3] {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let (dx, dy) = (1.75 * t as f64, 0.6 * t as f64);
    let mut s = 1234u32 + t as u32 * 77;
    let mut rnd = || {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        (s >> 29) as f64
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
            y[yy * w + x] = (lum(x as f64 - dx, yy as f64 - dy) + rnd() - 3.5).round().clamp(0.0, 255.0) as u8;
        }
    }
    let u = (0..cw * ch).map(|i| (100.0 + 40.0 * (((i % cw) as f64 * 2.0 - dx) / w as f64)).round() as u8).collect();
    let v = (0..cw * ch).map(|i| (150.0 - 30.0 * (((i / cw) as f64 * 2.0 - dy) / h as f64)).round() as u8).collect();
    [y, u, v]
}

/// Encode `n` frames, decode each and compare with the reconstruction: (frames, total bytes, mean
/// luma PSNR).
fn run(cfg: EncoderConfig, n: usize) -> (Vec<Vec<u8>>, usize, f64) {
    let (w, h) = (cfg.width as usize, cfg.height as usize);
    let mut e = Vp9Encoder::new(cfg);
    let mut d = Decoder::new();
    let (mut frames, mut db) = (vec![], 0.0);
    for t in 0..n {
        let src = moving(w, h, t);
        let f = e.encode_frame(&src[0], &src[1], &src[2], false);
        let mut pics = d.decode(&f.data, t as i64).unwrap_or_else(|err| panic!("{w}x{h} frame {t}: {err:?}"));
        assert_eq!(pics.len(), 1);
        let got = planes_of(&pics.remove(0), w, h);
        let rec = e.reconstruction().expect("reconstruction");
        for k in 0..3 {
            assert!(got[k] == rec[k], "{w}x{h} frame {t} (key {}): plane {k} differs from the reconstruction", f.key);
        }
        db += psnr(&got[0], &src[0]);
        frames.push(f.data);
    }
    let bytes = frames.iter().map(Vec::len).sum();
    (frames, bytes, db / n as f64)
}

#[test]
fn several_tile_columns_decode_exactly() {
    // 1024 px: 16 superblocks, four tile columns; 1000 px: the last column ends inside a
    // superblock; 520 px: two columns.
    for (w, h) in [(1024u32, 136u32), (1000, 72), (520, 40)] {
        let cfg = EncoderConfig { width: w, height: h, quality: 75, keyframe_interval: 4, ..Default::default() };
        let (_, bytes, db) = run(cfg, 6);
        eprintln!("{w}x{h} q75: {bytes} B, {db:.2} dB");
        assert!(db > 33.0, "{w}x{h}: {db:.2} dB");
    }
}

#[test]
fn lossless_across_tile_edges() {
    let (w, h) = (1000usize, 40usize);
    let src = moving(w, h, 3);
    let mut e = Vp9Encoder::new(EncoderConfig { width: w as u32, height: h as u32, quality: 100, ..Default::default() });
    let f = e.encode_yuv420(&src[0], &src[1], &src[2]);
    let pics = Decoder::new().decode(&f, 0).expect("decode");
    assert_eq!(planes_of(&pics[0], w, h), src);
}

#[test]
fn output_is_deterministic() {
    let cfg = EncoderConfig { width: 1024, height: 72, quality: 70, keyframe_interval: 3, ..Default::default() };
    let a = run(cfg.clone(), 4).0;
    let b = run(cfg, 4).0;
    assert!(a == b, "two runs gave different bytes");
}

/// Size, quality and speed at 720p (run with `--ignored --nocapture`).
#[test]
#[ignore]
fn bench_720p_tiles() {
    let (w, h) = (1280usize, 720usize);
    let cfg = EncoderConfig { width: w as u32, height: h as u32, quality: 80, keyframe_interval: 0, ..Default::default() };
    let t0 = std::time::Instant::now();
    let (_, bytes, db) = run(cfg, 8);
    eprintln!("720p q80, 8 frames: {bytes} B, {db:.3} dB, {:.0} ms (incl. decode)", t0.elapsed().as_secs_f64() * 1000.0);
    let mut e = Vp9Encoder::new(EncoderConfig { width: w as u32, height: h as u32, quality: 80, keyframe_interval: 0, ..Default::default() });
    let frames: Vec<_> = (0..8).map(|t| moving(w, h, t)).collect();
    let t0 = std::time::Instant::now();
    for f in &frames {
        e.encode_frame(&f[0], &f[1], &f[2], false);
    }
    eprintln!("encode only: {:.0} ms / frame", t0.elapsed().as_secs_f64() * 1000.0 / 8.0);
}
