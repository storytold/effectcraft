//! Multichannel audio downmix (#903) against ffmpeg as an external oracle. Fixtures: ffmpeg with
//! one sine per channel, written to `target/fixtures/media`; tests skip when ffmpeg is missing.

use std::f64::consts::PI;
use std::path::{Path, PathBuf};
use std::process::Command;

use effectcraft_media::{MediaPool, probe};
use effectcraft_time::Tick;

const RATE: u32 = 48_000;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fixtures/media")
}

/// A fixture with `layout`'s channels `names` (join order) each carrying its own sine, encoded
/// with `codec_args`. `None` when ffmpeg is missing or cannot make it.
fn fixture(name: &str, layout: &str, names: &[&str], freqs: &[u32], codec_args: &[&str]) -> Option<PathBuf> {
    if !Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success()) {
        eprintln!("ffmpeg not found: skipping");
        return None;
    }
    let out = fixtures().join(name);
    if out.exists() {
        return Some(out);
    }
    std::fs::create_dir_all(fixtures()).ok()?;
    let ext = out.extension()?.to_string_lossy().to_string();
    let tmp = out.with_file_name(format!("tmp-{}-{name}", std::process::id())).with_extension(ext);
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-y"]);
    let mut map = String::new();
    for (i, (n, f)) in names.iter().zip(freqs).enumerate() {
        cmd.args(["-f", "lavfi", "-i", &format!("sine=frequency={f}:sample_rate={RATE}:duration=2")]);
        map.push_str(&format!("[{i}]"));
        let _ = n;
    }
    let post = if name.contains("loud") { ",volume=8:precision=float,alimiter=limit=1:level=disabled" } else { "" };
    let joins: Vec<String> = names.iter().enumerate().map(|(i, n)| format!("{i}.0-{n}")).collect();
    cmd.args(["-filter_complex", &format!("{map}join=inputs={}:channel_layout={layout}:map={}{}[a]", names.len(), joins.join("|"), post), "-map", "[a]"]);
    cmd.args(codec_args).arg(&tmp);
    let ok = cmd.status().ok()?.success();
    if !ok {
        eprintln!("ffmpeg could not make {name}");
        return None;
    }
    std::fs::rename(&tmp, &out).ok()?;
    Some(out)
}

/// Amplitude (peak) of the sine at `freq` in one channel of interleaved stereo.
fn amp(s: &[f32], ch: usize, freq: u32) -> f64 {
    let (mut re, mut im, mut n) = (0.0, 0.0, 0usize);
    for (i, f) in s.as_chunks::<2>().0.iter().enumerate() {
        let w = 2.0 * PI * f64::from(freq) * i as f64 / f64::from(RATE);
        re += f64::from(f[ch]) * w.cos();
        im += f64::from(f[ch]) * w.sin();
        n += 1;
    }
    2.0 * (re * re + im * im).sqrt() / n.max(1) as f64
}

const K: f64 = std::f64::consts::FRAC_1_SQRT_2;

struct Case {
    file: &'static str,
    layout: &'static str,
    names: &'static [&'static str],
    codec: &'static [&'static str],
    /// Expected `[left, right]` gain of each channel (the same order as `names`).
    gains: Vec<[f64; 2]>,
    /// Allowed deviation of a channel's amplitude (lossy codecs).
    tol: f64,
}

const FREQS: [u32; 8] = [300, 500, 700, 900, 1100, 1300, 1500, 1700];

fn five_one() -> (&'static [&'static str], Vec<[f64; 2]>) {
    let k = 1.0 / (1.0 + 2.0 * K);
    (&["FL", "FR", "FC", "LFE", "BL", "BR"], vec![[k, 0.0], [0.0, k], [k * K, k * K], [0.0, 0.0], [k * K, 0.0], [0.0, k * K]])
}

fn seven_one() -> (&'static [&'static str], Vec<[f64; 2]>) {
    let k = 1.0 / (1.0 + 3.0 * K);
    (
        &["FL", "FR", "FC", "LFE", "BL", "BR", "SL", "SR"],
        vec![[k, 0.0], [0.0, k], [k * K, k * K], [0.0, 0.0], [k * K, 0.0], [0.0, k * K], [k * K, 0.0], [0.0, k * K]],
    )
}

fn cases() -> Vec<Case> {
    let (n51, g51) = five_one();
    let (n71, g71) = seven_one();
    let k5 = 1.0 / (1.0 + 2.0 * K);
    let kq = 1.0 / (1.0 + K);
    let k61 = 1.0 / (1.5 + 2.0 * K);
    vec![
        Case { file: "dm51.m4a", layout: "5.1", names: n51, codec: &["-c:a", "aac", "-b:a", "384k"], gains: g51.clone(), tol: 0.15 },
        Case {
            file: "dm61.m4a",
            layout: "6.1",
            names: &["FL", "FR", "FC", "LFE", "BC", "SL", "SR"],
            codec: &["-c:a", "aac", "-b:a", "448k"],
            gains: vec![[k61, 0.0], [0.0, k61], [k61 * K, k61 * K], [0.0, 0.0], [k61 / 2.0, k61 / 2.0], [k61 * K, 0.0], [0.0, k61 * K]],
            tol: 0.15,
        },
        Case { file: "dm71.m4a", layout: "7.1", names: n71, codec: &["-c:a", "aac", "-b:a", "512k"], gains: g71.clone(), tol: 0.15 },
        Case { file: "dm51.ac3.mp4", layout: "5.1", names: n51, codec: &["-c:a", "ac3", "-b:a", "640k"], gains: g51.clone(), tol: 0.15 },
        Case { file: "dm51.wav", layout: "5.1", names: n51, codec: &["-c:a", "pcm_s16le"], gains: g51.clone(), tol: 0.02 },
        Case { file: "dm51.flac", layout: "5.1", names: n51, codec: &["-c:a", "flac"], gains: g51, tol: 0.02 },
        Case { file: "dm71.wav", layout: "7.1", names: n71, codec: &["-c:a", "pcm_s16le"], gains: g71.clone(), tol: 0.02 },
        Case { file: "dm71.flac", layout: "7.1", names: n71, codec: &["-c:a", "flac"], gains: g71, tol: 0.02 },
        Case {
            file: "dm50.wav",
            layout: "5.0",
            names: &["FL", "FR", "FC", "BL", "BR"],
            codec: &["-c:a", "pcm_s16le"],
            gains: vec![[k5, 0.0], [0.0, k5], [k5 * K, k5 * K], [k5 * K, 0.0], [0.0, k5 * K]],
            tol: 0.02,
        },
        Case {
            file: "dmquad.wav",
            layout: "quad",
            names: &["FL", "FR", "BL", "BR"],
            codec: &["-c:a", "pcm_s16le"],
            gains: vec![[kq, 0.0], [0.0, kq], [kq * K, 0.0], [0.0, kq * K]],
            tol: 0.02,
        },
    ]
}

/// Our stereo mix of `p` over one second, from 0.2 s in.
fn ours(p: &Path) -> Vec<f32> {
    let f = probe(p).expect("probe");
    MediaPool::new().audio_samples(&f, Tick::from_seconds_f64(0.2), RATE as usize, RATE)
}

/// ffmpeg's own `-ac 2` downmix of the same second, interleaved. Written as 16-bit integers:
/// swresample only normalises the matrix (to keep full scale from clipping) for integer output.
fn ffmpeg_ac2(p: &Path) -> Vec<f32> {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-ss", "0.2", "-t", "1", "-i"])
        .arg(p)
        .args(["-ac", "2", "-ar", "48000", "-f", "s16le", "-"])
        .output()
        .expect("run ffmpeg");
    out.stdout.as_chunks::<2>().0.iter().map(|b| f32::from(i16::from_le_bytes(*b)) / 32768.0).collect()
}

fn rms(s: &[f32]) -> f64 {
    (s.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / s.len().max(1) as f64).sqrt()
}

fn peak(s: &[f32]) -> f64 {
    s.iter().fold(0.0f64, |m, v| m.max(f64::from(v.abs())))
}

/// Each channel carries its own sine at amplitude 0.5·√... (ffmpeg's `sine` is −24 dBFS RMS,
/// peak ≈ 0.125). Every channel must reach each side at the BS.775 gain, and the overall level
/// must match ffmpeg's `-ac 2`.
#[test]
fn each_channel_reaches_stereo_at_its_bs775_gain() {
    for c in cases() {
        let freqs: Vec<u32> = FREQS.iter().copied().take(c.names.len()).collect();
        let Some(p) = fixture(c.file, c.layout, c.names, &freqs, c.codec) else { continue };
        check(&c, &p);
    }
}

/// `p`, or a copy whose AAC sample entry names all `channels` of its PCE layout: ffmpeg 4.2 writes
/// 2 there for a PCE layout (8.1 the real count), and FilmCraft decodes as many channels as the
/// sample entry names, so the other five of a 6.1 would never reach the mix.
fn whole_pce_entry(p: &Path, channels: usize) -> PathBuf {
    let mut bytes = std::fs::read(p).expect("read");
    let Some((asc, 2)) = aac_entry(&bytes) else { return p.to_path_buf() };
    if channels <= 2 || filmcraft_aac::AudioSpecificConfig::parse(&asc).map(|a| a.channel_config).ok() != Some(0) {
        return p.to_path_buf();
    }
    // the `mp4a` entry's channelcount follows its 6 reserved bytes, data reference index and
    // 8 reserved bytes
    let at = bytes.windows(4).position(|w| w == b"mp4a").expect("an mp4a sample entry") + 4 + 6 + 2 + 8;
    bytes[at..at + 2].copy_from_slice(&(channels as u16).to_be_bytes());
    eprintln!("{}: its sample entry names 2 of {channels} channels (ffmpeg 4.2): reading a copy naming all", p.display());
    let out = p.with_extension("entry.m4a");
    std::fs::write(&out, bytes).expect("write");
    out
}

/// The AudioSpecificConfig and sample entry channel count of an MP4's first AAC track.
fn aac_entry(bytes: &[u8]) -> Option<(Vec<u8>, u32)> {
    let file = filmcraft_isobmff::open(bytes).ok()?;
    file.tracks.iter().flat_map(|t| &t.entries).find_map(|e| match &e.codec {
        filmcraft_isobmff::CodecConfig::Aac(a) => Some((a.asc.clone(), e.audio.as_ref()?.channels)),
        _ => None,
    })
}

fn check(c: &Case, p: &Path) {
    let p = &whole_pce_entry(p, c.names.len());
    let s = ours(p);
    let freqs: Vec<u32> = FREQS.iter().copied().take(c.names.len()).collect();
    let sine = 0.125 * 1.0; // peak of ffmpeg's sine source (0.125 = 2^-3)
    for (i, (name, g)) in c.names.iter().zip(&c.gains).enumerate() {
        for (side, gain) in g.iter().enumerate() {
            let got = amp(&s, side, freqs[i]);
            let want = sine * gain;
            assert!((got - want).abs() <= sine * c.tol.max(0.02), "{}: {name} -> {} is {got:.4}, expected {want:.4}", c.file, ["L", "R"][side]);
        }
    }
    let theirs = ffmpeg_ac2(p);
    let (a, b) = (rms(&s), rms(&theirs));
    let db = 20.0 * (a / b).log10();
    assert!(db.abs() < 1.0, "{}: level {db:.2} dB off ffmpeg -ac 2 (rms {a:.4} vs {b:.4})", c.file);
    let pdb = 20.0 * (peak(&s) / peak(&theirs)).log10();
    assert!(pdb.abs() < 1.5, "{}: peak {pdb:.2} dB off ffmpeg -ac 2", c.file);
}

/// Full-scale in every channel must not clip.
#[test]
fn loud_surround_does_not_clip() {
    let Some(p) = fixture("dm51_loud.wav", "5.1", &["FL", "FR", "FC", "LFE", "BL", "BR"], &[300; 6], &["-c:a", "pcm_s16le"]) else { return };
    let s = ours(&p);
    assert!(peak(&s) <= 1.001, "peak {}", peak(&s));
    assert!(peak(&s) > 0.5, "audible, peak {}", peak(&s));
}

/// Mono and stereo footage keep their level (mono on both sides).
#[test]
fn mono_and_stereo_keep_their_level() {
    let Some(p) = fixture("dm10.wav", "mono", &["FC"], &[440], &["-c:a", "pcm_s16le"]) else { return };
    let s = ours(&p);
    assert!((amp(&s, 0, 440) - 0.125).abs() < 0.003 && (amp(&s, 1, 440) - 0.125).abs() < 0.003);
    let Some(p) = fixture("dm20.wav", "stereo", &["FL", "FR"], &[440, 660], &["-c:a", "pcm_s16le"]) else { return };
    let s = ours(&p);
    assert!((amp(&s, 0, 440) - 0.125).abs() < 0.003 && amp(&s, 1, 440) < 0.003);
    assert!((amp(&s, 1, 660) - 0.125).abs() < 0.003 && amp(&s, 0, 660) < 0.003);
}

/// Odd channel counts (3, 4, 7, 9, 12, 16) decode to finite stereo without panicking.
#[test]
fn hostile_channel_counts_do_not_panic() {
    for (n, layout) in [(3, "3.0"), (4, "quad"), (7, "6.1"), (9, "9 channels"), (12, "12 channels"), (16, "16 channels")] {
        let names: Vec<String> = (0..n).map(|i| format!("{i}")).collect();
        // channel names for unnamed layouts are DEFAULT-ordered by position
        let _ = names;
        let file = format!("dmn{n}.wav");
        let args = ["-f", "lavfi", "-i", &format!("sine=frequency=440:sample_rate={RATE}:duration=1")];
        if !Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success()) {
            return;
        }
        let out = fixtures().join(&file);
        std::fs::create_dir_all(fixtures()).expect("mkdir");
        if !out.exists() {
            let tmp = fixtures().join(format!("tmp-{}-{file}", std::process::id()));
            let af = format!("pan={layout}|{}", (0..n).map(|i| format!("c{i}=c0")).collect::<Vec<_>>().join("|"));
            let ok =
                Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-y"]).args(args).args(["-af", &af, "-c:a", "pcm_s16le"]).arg(&tmp).status();
            if !ok.is_ok_and(|s| s.success()) {
                eprintln!("ffmpeg cannot make {n} channels: skipping");
                continue;
            }
            std::fs::rename(&tmp, &out).expect("rename");
        }
        let Ok(f) = probe(&out) else { continue };
        let s = MediaPool::new().audio_samples(&f, Tick::ZERO, 4800, RATE);
        assert_eq!(s.len(), 9600, "{n} channels");
        assert!(s.iter().all(|v| v.is_finite() && v.abs() <= 1.001), "{n} channels");
    }
}
