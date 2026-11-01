//! Round trips through the filmcraft-hevc decoder and (when installed) ffmpeg: decoded pictures must
//! equal the encoder's own reconstruction bit-exactly.

use effectcraft_hevcenc::{Encoder, EncoderConfig, Frame, Profile, RateControl};
use std::process::Command;

struct Pic {
    w: usize,
    y: Vec<u16>,
    u: Vec<u16>,
    v: Vec<u16>,
}

impl Pic {
    fn frame(&self) -> Frame<'_> {
        Frame { y: &self.y, u: &self.u, v: &self.v, y_stride: self.w, uv_stride: self.w / 2 }
    }
}

/// Smooth gradients with a few hard-edged shapes, translated by (dx, dy) samples.
fn synth(w: usize, h: usize, bd: u32, dx: i32, dy: i32) -> Pic {
    let max = ((1u32 << bd) - 1) as f64;
    let mut y = vec![0u16; w * h];
    let mut u = vec![0u16; w * h / 4];
    let mut v = vec![0u16; w * h / 4];
    let luma = |x: i32, yy: i32| -> f64 {
        let (fx, fy) = ((x - dx) as f64, (yy - dy) as f64);
        let mut l = 0.25 + 0.5 * (fx / 97.0).sin().abs() * 0.6 + 0.3 * (fy / 61.0).cos().powi(2);
        // a disc and a rectangle
        if (fx - 40.0).powi(2) + (fy - 30.0).powi(2) < 18.0f64.powi(2) {
            l = 0.85;
        }
        if (70.0..110.0).contains(&fx) && (50.0..64.0).contains(&fy) {
            l = 0.12;
        }
        // fine texture
        l += 0.03 * ((fx * 0.9).sin() * (fy * 0.7).cos());
        l.clamp(0.0, 1.0)
    };
    for yy in 0..h {
        for x in 0..w {
            y[yy * w + x] = (luma(x as i32, yy as i32) * max).round() as u16;
        }
    }
    for yy in 0..h / 2 {
        for x in 0..w / 2 {
            let (fx, fy) = ((2 * x) as i32 - dx, (2 * yy) as i32 - dy);
            let cu = 0.5 + 0.2 * ((fx as f64) / 53.0).sin();
            let cv = 0.5 + 0.2 * ((fy as f64) / 37.0).cos() * if (fx / 16 + fy / 16) % 2 == 0 { 1.0 } else { -0.5 };
            u[yy * w / 2 + x] = (cu * max).round() as u16;
            v[yy * w / 2 + x] = (cv * max).round() as u16;
        }
    }
    Pic { w, y, u, v }
}

fn psnr(a: &[u16], b: &[u16], bd: u32) -> f64 {
    let max = ((1u32 << bd) - 1) as f64;
    let mse = a.iter().zip(b).map(|(&x, &y)| (x as f64 - y as f64).powi(2)).sum::<f64>() / a.len() as f64;
    if mse == 0.0 { 99.0 } else { 10.0 * (max * max / mse).log10() }
}

struct Run {
    packets: Vec<Vec<u8>>,
    keys: Vec<bool>,
    recons: Vec<[Vec<u16>; 3]>,
    hvcc: Vec<u8>,
    sets: [Vec<u8>; 3],
    psnr_y: Vec<f64>,
}

fn encode(cfg: EncoderConfig, pics: &[Pic]) -> Run {
    let bd = cfg.profile.bit_depth() as u32;
    let mut enc = Encoder::new(cfg).unwrap();
    let mut run = Run { packets: vec![], keys: vec![], recons: vec![], hvcc: enc.hvcc(), sets: enc.parameter_sets(), psnr_y: vec![] };
    for p in pics {
        let pkt = enc.encode(&p.frame());
        let rec = enc.last_reconstruction().unwrap().clone();
        run.psnr_y.push(psnr(&p.y, &rec[0], bd));
        run.packets.push(pkt.data);
        run.keys.push(pkt.keyframe);
        run.recons.push(rec);
    }
    run
}

/// Decode with filmcraft-hevc and compare with the reconstructions.
fn check_filmcraft(run: &Run, w: usize, h: usize) {
    let mut dec = filmcraft_hevc::Decoder::from_hvcc(&run.hvcc).unwrap();
    let mut pics = Vec::new();
    for (i, p) in run.packets.iter().enumerate() {
        pics.extend(dec.decode(p, i as i64).unwrap());
    }
    pics.extend(dec.flush());
    assert_eq!(pics.len(), run.packets.len(), "decoded picture count");
    for (i, pic) in pics.iter().enumerate() {
        assert_eq!((pic.width as usize, pic.height as usize), (w, h));
        assert_eq!(pic.pts, i as i64);
        let rec = &run.recons[i];
        for (c, plane) in [&pic.y, &pic.u, &pic.v].into_iter().enumerate() {
            let (pw, ph, stride) = if c == 0 { (w, h, pic.y_stride) } else { (w / 2, h / 2, pic.uv_stride) };
            for yy in 0..ph {
                for x in 0..pw {
                    let got = plane.get(yy * stride + x);
                    let want = rec[c][yy * pw + x];
                    assert_eq!(got, want, "picture {i} plane {c} at ({x}, {yy})");
                }
            }
        }
    }
}

fn annexb(run: &Run) -> Vec<u8> {
    let mut out = Vec::new();
    for s in &run.sets {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(s);
    }
    for p in &run.packets {
        let mut i = 0;
        while i < p.len() {
            let n = u32::from_be_bytes([p[i], p[i + 1], p[i + 2], p[i + 3]]) as usize;
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(&p[i + 4..i + 4 + n]);
            i += 4 + n;
        }
    }
    out
}

fn ffmpeg_available() -> bool {
    Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// `true` when this ffmpeg/ffprobe build actually ships an HEVC decoder. Distro "free" builds
/// (e.g. Fedora's ffmpeg-free) ship without one — the tools run but cannot decode our stream, so
/// the external oracle means nothing and the test skips rather than failing on a gap in the tool.
fn hevc_decoder_available() -> bool {
    Command::new("ffmpeg")
        .args(["-hide_banner", "-decoders"])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout).lines().any(|l| {
                // Decoder rows look like ` V....D hevc ...`; match the codec name, not the text.
                // Every row of `-decoders` is a decoder, so the flags column says nothing about
                // that — its trailing `D` is "supports direct rendering method 1", which a
                // build's hevc decoder may well lack (requiring it would skip a working oracle).
                let mut it = l.split_whitespace();
                it.next().is_some() && it.next() == Some("hevc")
            })
        })
        .unwrap_or(false)
}

/// Decode the Annex-B stream with ffmpeg and require bit-exact equality and an empty stderr.
fn check_ffmpeg(run: &Run, w: usize, h: usize, bd: u32, tag: &str) {
    if !ffmpeg_available() {
        eprintln!("ffmpeg not found; skipping the ffmpeg oracle");
        return;
    }
    if !hevc_decoder_available() {
        eprintln!("ffmpeg has no HEVC decoder here; skipping the ffmpeg oracle");
        return;
    }
    let dir = std::env::temp_dir();
    let uniq = format!("{}-{}-{}", tag, std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let inp = dir.join(format!("hevcenc-{uniq}.hevc"));
    let out = dir.join(format!("hevcenc-{uniq}.yuv"));
    std::fs::write(&inp, annexb(run)).unwrap();
    let pix = if bd == 8 { "yuv420p" } else { "yuv420p10le" };
    let o = Command::new("ffmpeg")
        .args(["-v", "error", "-xerror", "-y", "-f", "hevc", "-i"])
        .arg(&inp)
        .args(["-f", "rawvideo", "-pix_fmt", pix])
        .arg(&out)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&o.stderr).to_string();
    let data = std::fs::read(&out).unwrap_or_default();
    let _ = std::fs::remove_file(&inp);
    let _ = std::fs::remove_file(&out);
    assert!(o.status.success() && stderr.trim().is_empty(), "ffmpeg failed: {stderr}");
    let bps = if bd == 8 { 1 } else { 2 };
    let frame_bytes = (w * h + 2 * (w / 2) * (h / 2)) * bps;
    assert_eq!(data.len(), frame_bytes * run.packets.len(), "ffmpeg output size");
    for (i, rec) in run.recons.iter().enumerate() {
        let mut off = i * frame_bytes;
        for (c, plane) in rec.iter().enumerate() {
            let pw = if c == 0 { w } else { w / 2 };
            for (k, &want) in plane.iter().enumerate() {
                let got = if bd == 8 { data[off] as u16 } else { u16::from_le_bytes([data[off], data[off + 1]]) };
                assert_eq!(got, want, "ffmpeg: picture {i} plane {c} at ({}, {})", k % pw, k / pw);
                off += bps;
            }
        }
    }
}

fn cfg(w: u32, h: u32, qp: u8, keyint: u32, profile: Profile) -> EncoderConfig {
    let mut c = EncoderConfig::new(w, h, 30, 1);
    c.rate = RateControl::ConstantQp(qp);
    c.keyint = keyint;
    c.profile = profile;
    c
}

#[test]
fn intra_8bit_quality_and_decoders() {
    let (w, h) = (192, 128);
    let pics = vec![synth(w, h, 8, 0, 0)];
    let run = encode(cfg(w as u32, h as u32, 22, 1, Profile::Main), &pics);
    eprintln!("intra QP22: {} bytes, PSNR-Y {:.2}", run.packets[0].len(), run.psnr_y[0]);
    assert!(run.psnr_y[0] > 35.0, "PSNR {}", run.psnr_y[0]);
    check_filmcraft(&run, w, h);
    check_ffmpeg(&run, w, h, 8, "intra8");
}

/// ffprobe reads the profile, level, colour description and timing from the parameter sets.
#[test]
fn ffprobe_stream_info() {
    if !ffmpeg_available() {
        return;
    }
    if !hevc_decoder_available() {
        eprintln!("ffmpeg has no HEVC decoder here; ffprobe cannot fully describe our stream, skipping");
        return;
    }
    let (w, h) = (130, 72);
    let pics: Vec<Pic> = (0..2).map(|i| synth(w, h, 10, i, 0)).collect();
    let mut c = cfg(w as u32, h as u32, 30, 30, Profile::Main10);
    c.fps_num = 30000;
    c.fps_den = 1001;
    c.full_range = true;
    let run = encode(c, &pics);
    let path = std::env::temp_dir().join(format!("hevcenc-probe-{}.hevc", std::process::id()));
    std::fs::write(&path, annexb(&run)).unwrap();
    let o = Command::new("ffprobe").args(["-v", "error", "-show_streams", "-of", "flat"]).arg(&path).output().unwrap();
    let _ = std::fs::remove_file(&path);
    let s = String::from_utf8_lossy(&o.stdout).to_string();
    for want in [
        "codec_name=\"hevc\"",
        "profile=\"Main 10\"",
        "width=130",
        "height=72",
        "coded_width=136",
        "coded_height=72",
        "pix_fmt=\"yuv420p10le\"",
        "level=30",
        "color_range=\"pc\"",
        "color_space=\"bt709\"",
        "color_transfer=\"bt709\"",
        "color_primaries=\"bt709\"",
        "r_frame_rate=\"30000/1001\"",
    ] {
        assert!(s.contains(want), "ffprobe output lacks {want}:\n{s}");
    }
}

#[test]
fn inter_8bit_moving_pattern() {
    let (w, h) = (160, 96);
    let pics: Vec<Pic> = (0..6).map(|i| synth(w, h, 8, 3 * i, i)).collect();
    let run = encode(cfg(w as u32, h as u32, 27, 30, Profile::Main), &pics);
    let sizes: Vec<usize> = run.packets.iter().map(|p| p.len()).collect();
    eprintln!("inter sizes {sizes:?} psnr {:?}", run.psnr_y);
    assert_eq!(run.keys, vec![true, false, false, false, false, false]);
    let p_avg = sizes[1..].iter().sum::<usize>() as f64 / 5.0;
    assert!(p_avg < sizes[0] as f64 * 0.6, "P frames {p_avg} vs I {}", sizes[0]);
    assert!(run.psnr_y.iter().all(|&p| p > 33.0));
    check_filmcraft(&run, w, h);
    check_ffmpeg(&run, w, h, 8, "inter8");
}

#[test]
fn odd_multiple_sizes_and_cropping() {
    for (w, h) in [(34usize, 18usize), (66, 38), (8, 8), (130, 72), (2, 2), (2, 70), (264, 6)] {
        let pics: Vec<Pic> = (0..3).map(|i| synth(w, h, 8, i, 2 * i)).collect();
        let run = encode(cfg(w as u32, h as u32, 30, 2, Profile::Main), &pics);
        check_filmcraft(&run, w, h);
        check_ffmpeg(&run, w, h, 8, &format!("size{w}x{h}"));
    }
}

/// White noise at QP 0: large levels, escape codes with the maximum Rice parameter.
#[test]
fn noise_low_qp() {
    let (w, h) = (48, 40);
    let mut seed = 0x1234_5678u32;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for (profile, bd) in [(Profile::Main, 8u32), (Profile::Main10, 10)] {
        let max = (1u32 << bd) - 1;
        let pics: Vec<Pic> = (0..3)
            .map(|_| {
                let mut p = synth(w, h, bd, 0, 0);
                for v in p.y.iter_mut().chain(p.u.iter_mut()).chain(p.v.iter_mut()) {
                    *v = (rnd() % (max + 1)) as u16;
                }
                p
            })
            .collect();
        let run = encode(cfg(w as u32, h as u32, 0, 2, profile), &pics);
        assert!(run.psnr_y.iter().all(|&p| p > 45.0), "{:?}", run.psnr_y);
        check_filmcraft(&run, w, h);
        check_ffmpeg(&run, w, h, bd, &format!("noise{bd}"));
    }
}

/// High QPs: strong deblocking, large skipped CUs.
#[test]
fn high_qp_deblocking() {
    let (w, h) = (200, 136);
    for (qp, profile, bd) in [(38u8, Profile::Main, 8u32), (44, Profile::Main, 8), (37, Profile::Main10, 10)] {
        let pics: Vec<Pic> = (0..5).map(|i| synth(w, h, bd, 2 * i, i)).collect();
        let run = encode(cfg(w as u32, h as u32, qp, 4, profile), &pics);
        check_filmcraft(&run, w, h);
        check_ffmpeg(&run, w, h, bd, &format!("hq{qp}"));
    }
}

/// More pictures than MaxPicOrderCntLsb (256) without an IDR, then a periodic IDR: POC wrap.
#[test]
fn long_gop_poc_wrap() {
    let (w, h) = (16, 16);
    let pics: Vec<Pic> = (0..300).map(|i| synth(w, h, 8, i % 7, i % 5)).collect();
    let run = encode(cfg(w as u32, h as u32, 30, 0, Profile::Main), &pics);
    assert_eq!(run.keys.iter().filter(|&&k| k).count(), 1);
    check_filmcraft(&run, w, h);
    check_ffmpeg(&run, w, h, 8, "pocwrap");
    let run = encode(cfg(w as u32, h as u32, 30, 260, Profile::Main), &pics);
    assert_eq!(run.keys.iter().filter(|&&k| k).count(), 2);
    check_ffmpeg(&run, w, h, 8, "pocwrap2");
}

#[test]
fn odd_dimensions_are_rejected() {
    assert!(Encoder::new(EncoderConfig::new(33, 17, 30, 1)).is_err());
}

#[test]
fn main10_round_trip() {
    let (w, h) = (96, 64);
    let pics: Vec<Pic> = (0..4).map(|i| synth(w, h, 10, 2 * i, -i)).collect();
    let run = encode(cfg(w as u32, h as u32, 24, 3, Profile::Main10), &pics);
    eprintln!("main10 psnr {:?}", run.psnr_y);
    assert!(run.psnr_y[0] > 35.0);
    check_filmcraft(&run, w, h);
    check_ffmpeg(&run, w, h, 10, "main10");
}

#[test]
fn qp_extremes() {
    let (w, h) = (64, 64);
    for qp in [0u8, 51] {
        let pics: Vec<Pic> = (0..2).map(|i| synth(w, h, 8, i, i)).collect();
        let run = encode(cfg(w as u32, h as u32, qp, 30, Profile::Main), &pics);
        check_filmcraft(&run, w, h);
        check_ffmpeg(&run, w, h, 8, &format!("qp{qp}"));
    }
}

#[test]
fn bitrate_mode_hits_target() {
    let (w, h) = (192, 128);
    let pics: Vec<Pic> = (0..30).map(|i| synth(w, h, 8, i, i / 2)).collect();
    let mut c = EncoderConfig::new(w as u32, h as u32, 30, 1);
    c.rate = RateControl::Bitrate { kbps: 400 };
    c.keyint = 15;
    let run = encode(c, &pics);
    let bits: usize = run.packets.iter().map(|p| p.len() * 8).sum();
    let kbps = bits as f64 / (pics.len() as f64 / 30.0) / 1000.0;
    eprintln!("bitrate mode: {kbps:.1} kbit/s for a 400 kbit/s target");
    assert!(kbps > 400.0 / 2.0 && kbps < 400.0 * 2.0, "{kbps}");
    check_filmcraft(&run, w, h);
}

#[test]
fn hvcc_layout() {
    let enc = Encoder::new(cfg(64, 48, 28, 30, Profile::Main10)).unwrap();
    let h = enc.hvcc();
    assert_eq!(h[0], 1);
    assert_eq!(h[1] & 31, 2); // Main 10
    assert_eq!(h[21] & 3, 3); // lengthSizeMinusOne
    assert_eq!(h[22], 3); // arrays
    let sets = enc.parameter_sets();
    assert_eq!(sets[0][0] >> 1, 32);
    assert_eq!(sets[1][0] >> 1, 33);
    assert_eq!(sets[2][0] >> 1, 34);
}

/// Speed / size / quality report on ffmpeg-generated 1080p test content:
/// `cargo test -p effectcraft-hevcenc --test roundtrip bench_1080p -- --ignored --nocapture`
/// (`HEVCENC_BENCH_SRC` selects another lavfi source).
#[test]
#[ignore]
fn bench_1080p() {
    if !ffmpeg_available() {
        return;
    }
    let (w, h, n) = (1920usize, 1080usize, 8usize);
    let src = std::env::var("HEVCENC_BENCH_SRC").unwrap_or_else(|_| "testsrc2=size=1920x1080:rate=30".into());
    let out = std::env::temp_dir().join(format!("hevcenc-bench-{}.yuv", std::process::id()));
    let st = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", &src, "-frames:v", &n.to_string(), "-pix_fmt", "yuv420p", "-f", "rawvideo"])
        .arg(&out)
        .status()
        .unwrap();
    assert!(st.success());
    let raw = std::fs::read(&out).unwrap();
    let _ = std::fs::remove_file(&out);
    let fsz = w * h * 3 / 2;
    let to16 = |s: &[u8]| s.iter().map(|&v| v as u16).collect::<Vec<_>>();
    let pics: Vec<Pic> = (0..n)
        .map(|i| {
            let f = &raw[i * fsz..(i + 1) * fsz];
            Pic { w, y: to16(&f[..w * h]), u: to16(&f[w * h..w * h * 5 / 4]), v: to16(&f[w * h * 5 / 4..]) }
        })
        .collect();
    for qp in [22u8, 32] {
        let mut enc = Encoder::new(cfg(w as u32, h as u32, qp, 30, Profile::Main)).unwrap();
        for (i, p) in pics.iter().enumerate() {
            let t = std::time::Instant::now();
            let pkt = enc.encode(&p.frame());
            let dt = t.elapsed();
            let rec = enc.last_reconstruction().unwrap();
            eprintln!(
                "QP {qp} frame {i} ({}): {:7} bytes, {:6.1} ms, PSNR Y {:.2} U {:.2} V {:.2}",
                if pkt.keyframe { "I" } else { "P" },
                pkt.data.len(),
                dt.as_secs_f64() * 1000.0,
                psnr(&p.y, &rec[0], 8),
                psnr(&p.u, &rec[1], 8),
                psnr(&p.v, &rec[2], 8)
            );
        }
    }
}
