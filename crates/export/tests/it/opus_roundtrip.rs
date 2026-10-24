//! Round trip of `effectcraft-opusenc` through FilmCraft's spec-derived Opus decoder, plus an
//! ffmpeg oracle (skipped when ffmpeg is not installed) reading an Ogg Opus file written here.

use std::process::Command;

use effectcraft_opusenc::OpusEncoder;

const SR: f32 = 48_000.0;
const FRAME: usize = OpusEncoder::FRAME_SIZE;

/// Encodes interleaved `pcm`, padding the tail so the encoder's look-ahead is flushed.
fn encode(pcm: &[f32], channels: u8, bitrate: u32) -> (OpusEncoder, Vec<Vec<u8>>) {
    let c = channels as usize;
    let mut enc = OpusEncoder::new(channels, bitrate);
    let total = pcm.len() / c + enc.pre_skip() as usize;
    let frames = total.div_ceil(FRAME);
    let mut padded = pcm.to_vec();
    padded.resize(frames * FRAME * c, 0.0);
    let packets = padded.chunks(FRAME * c).map(|f| enc.encode_float(f)).collect();
    (enc, packets)
}

/// Decodes with filmcraft-opus, drops the pre-skip and returns planar channels of `len` samples.
fn decode(head: &[u8], packets: &[Vec<u8>], len: usize) -> Vec<Vec<f32>> {
    let mut dec = filmcraft_opus::Decoder::new(head).expect("OpusHead");
    dec.set_trim_pre_skip(true);
    let mut out: Vec<Vec<f32>> = vec![Vec::new(); dec.channels()];
    for p in packets {
        let planes = dec.decode(Some(p)).expect("decode");
        for (o, p) in out.iter_mut().zip(planes) {
            o.extend_from_slice(&p);
        }
    }
    for o in &mut out {
        assert!(o.len() >= len, "decoded {} < {len}", o.len());
        o.truncate(len);
    }
    out
}

fn roundtrip(planar: &[Vec<f32>], bitrate: u32) -> (Vec<Vec<u8>>, Vec<Vec<f32>>) {
    let c = planar.len();
    let len = planar[0].len();
    let mut pcm = vec![0f32; len * c];
    for (ch, p) in planar.iter().enumerate() {
        for (i, &v) in p.iter().enumerate() {
            pcm[i * c + ch] = v;
        }
    }
    let (enc, packets) = encode(&pcm, c as u8, bitrate);
    let decoded = decode(&enc.opus_head(), &packets, len);
    (packets, decoded)
}

/// SNR in dB over the steady part (the first and last 20 ms are excluded).
fn snr(orig: &[f32], dec: &[f32]) -> f64 {
    let (a, b) = (FRAME, orig.len() - FRAME);
    let mut s = 0f64;
    let mut e = 0f64;
    for i in a..b {
        s += (orig[i] as f64).powi(2);
        e += (orig[i] as f64 - dec[i] as f64).powi(2);
    }
    10.0 * (s / e.max(1e-30)).log10()
}

fn sine(freq: f32, amp: f32, len: usize) -> Vec<f32> {
    (0..len).map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / SR).sin()).collect()
}

fn chirp(f0: f32, f1: f32, amp: f32, len: usize) -> Vec<f32> {
    let dur = len as f32 / SR;
    let k = (f1 - f0) / dur;
    (0..len)
        .map(|i| {
            let t = i as f32 / SR;
            amp * (2.0 * std::f32::consts::PI * (f0 * t + 0.5 * k * t * t)).sin()
        })
        .collect()
}

fn noise(amp: f32, len: usize, seed: u32) -> Vec<f32> {
    let mut s = seed;
    (0..len)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            amp * ((s >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0)
        })
        .collect()
}

/// Energy (dB) of `x` in each of a few octave bands up to 16 kHz, from a Hann-windowed FFT
/// averaged over 2048-sample blocks.
fn band_energies(x: &[f32]) -> Vec<f64> {
    const N: usize = 2048;
    let edges = [100.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];
    let mut acc = vec![0f64; edges.len() - 1];
    let mut start = N;
    while start + N <= x.len() - N {
        let mut re: Vec<f64> = (0..N).map(|i| x[start + i] as f64 * (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / N as f64).cos())).collect();
        let mut im = vec![0f64; N];
        fft(&mut re, &mut im);
        for k in 1..N / 2 {
            let f = k as f64 * SR as f64 / N as f64;
            if let Some(b) = edges.windows(2).position(|w| f >= w[0] && f < w[1]) {
                acc[b] += re[k] * re[k] + im[k] * im[k];
            }
        }
        start += N;
    }
    acc.iter().map(|e| 10.0 * (e + 1e-20).log10()).collect()
}

fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f64::consts::PI / len as f64;
        for s in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (wr, wi) = ((ang * k as f64).cos(), (ang * k as f64).sin());
                let (a, b) = (s + k, s + k + len / 2);
                let tr = re[b] * wr - im[b] * wi;
                let ti = re[b] * wi + im[b] * wr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}

#[test]
fn sine_440_mono() {
    let x = sine(440.0, 0.5, 48_000);
    for rate in [64_000, 128_000] {
        let (packets, d) = roundtrip(std::slice::from_ref(&x), rate);
        assert!(packets.iter().all(|p| p.len() == rate as usize / 400));
        let s = snr(&x, &d[0]);
        eprintln!("sine 440 Hz mono @ {} kb/s: SNR {s:.1} dB", rate / 1000);
        assert!(s > 20.0, "sine SNR {s:.1} dB at {rate}");
    }
}

#[test]
fn chirp_mono() {
    let x = chirp(100.0, 10_000.0, 0.4, 96_000);
    for rate in [64_000, 128_000] {
        let (_, d) = roundtrip(std::slice::from_ref(&x), rate);
        let s = snr(&x, &d[0]);
        eprintln!("chirp 100 Hz-10 kHz mono @ {} kb/s: SNR {s:.1} dB", rate / 1000);
        assert!(s > 15.0, "chirp SNR {s:.1} dB at {rate}");
    }
}

#[test]
fn white_noise_mono() {
    let x = noise(0.3, 48_000, 12345);
    for rate in [64_000, 128_000] {
        let (_, d) = roundtrip(std::slice::from_ref(&x), rate);
        let s = snr(&x, &d[0]);
        let (eo, ed) = (band_energies(&x), band_energies(&d[0]));
        let err: Vec<f32> = x.iter().zip(&d[0]).map(|(a, b)| a - b).collect();
        let ee = band_energies(&err);
        let octave_snr: Vec<f64> = eo.iter().zip(&ee).map(|(a, b)| a - b).collect();
        let worst = eo.iter().zip(&ed).map(|(a, b)| (a - b).abs()).fold(0.0, f64::max);
        let min_octave = octave_snr.iter().copied().fold(f64::MAX, f64::min);
        eprintln!(
            "white noise mono @ {} kb/s: full-band SNR {s:.1} dB, octave SNRs 100 Hz-16 kHz {:.1?} dB, worst octave energy error {worst:.2} dB",
            rate / 1000,
            octave_snr
        );
        // Full-band noise loses everything above 20 kHz (CELT's last band ends there) and spends
        // few bits per coefficient on 16-20 kHz, so the full-band waveform SNR is low; the band
        // energies must match closely and the octaves below 16 kHz must be waveform-coded.
        assert!(worst < 1.5, "band energy error {worst:.2} dB at {rate}: {eo:?} vs {ed:?}");
        assert!(s > 0.5, "noise SNR {s:.1} dB at {rate}");
        assert!(min_octave > if rate >= 128_000 { 10.0 } else { 3.0 }, "octave SNRs {octave_snr:?} at {rate}");
    }
}

#[test]
fn lowpass_noise_mono() {
    // White noise through a 4-pole low-pass at about 2 kHz: a dense spectrum the encoder can
    // afford to code as a waveform.
    let w = noise(0.8, 48_000, 999);
    let a = (-2.0 * std::f32::consts::PI * 2_000.0 / SR).exp();
    let mut st = [0f32; 4];
    let x: Vec<f32> = w
        .iter()
        .map(|&v| {
            let mut y = v;
            for s in st.iter_mut() {
                *s = (1.0 - a) * y + a * *s;
                y = *s;
            }
            y
        })
        .collect();
    for rate in [64_000, 128_000] {
        let (_, d) = roundtrip(std::slice::from_ref(&x), rate);
        let s = snr(&x, &d[0]);
        eprintln!("low-pass noise mono @ {} kb/s: SNR {s:.1} dB", rate / 1000);
        assert!(s > 15.0, "low-pass noise SNR {s:.1} dB at {rate}");
    }
}

#[test]
fn silence_stays_silent() {
    let x = vec![0f32; 48_000];
    let (packets, d) = roundtrip(&[x.clone(), x], 128_000);
    assert!(packets.iter().all(|p| p.len() == 3), "silence frames are 3-byte packets");
    for ch in &d {
        let peak = ch.iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(peak < 1e-6, "peak {peak}");
    }
}

#[test]
fn stereo_distinct_channels() {
    let l = sine(440.0, 0.5, 48_000);
    let r = chirp(200.0, 6_000.0, 0.3, 48_000);
    let (packets, d) = roundtrip(&[l.clone(), r.clone()], 256_000);
    assert!(packets.iter().all(|p| p[0] == 0xFC && p.len() == 640));
    let (sl, sr) = (snr(&l, &d[0]), snr(&r, &d[1]));
    eprintln!("stereo @ 256 kb/s: L (sine) SNR {sl:.1} dB, R (chirp) SNR {sr:.1} dB");
    assert!(sl > 20.0 && sr > 15.0, "stereo SNR L {sl:.1} R {sr:.1}");
    // No leakage of the chirp into the left channel (2-4 kHz octave: chirp only).
    let (el, er) = (band_energies(&d[0]), band_energies(&r));
    assert!(el[4] < er[4] - 30.0, "chirp energy leaked into L: {} vs {}", el[4], er[4]);
}

#[test]
fn stereo_128k_total() {
    let l = sine(1000.0, 0.4, 48_000);
    let r = sine(330.0, 0.4, 48_000);
    let (_, d) = roundtrip(&[l.clone(), r.clone()], 128_000);
    let (sl, sr) = (snr(&l, &d[0]), snr(&r, &d[1]));
    eprintln!("stereo @ 128 kb/s: L SNR {sl:.1} dB, R SNR {sr:.1} dB");
    assert!(sl > 15.0 && sr > 15.0, "stereo SNR L {sl:.1} R {sr:.1}");
}

#[test]
fn signal_after_silence_and_loud_transient() {
    // Silence, then a burst at full scale, then silence again: energy prediction must recover.
    let mut x = vec![0f32; 24_000];
    x.extend(sine(2_000.0, 0.99, 24_000));
    x.extend(vec![0f32; 24_000]);
    let (_, d) = roundtrip(std::slice::from_ref(&x), 96_000);
    let s = snr(&x[24_000 + 2 * FRAME..48_000], &d[0][24_000 + 2 * FRAME..48_000]);
    eprintln!("burst after silence @ 96 kb/s: SNR {s:.1} dB");
    assert!(s > 15.0, "burst SNR {s:.1}");
    let tail_peak = d[0][48_000 + 2 * FRAME..].iter().fold(0f32, |m, v| m.max(v.abs()));
    assert!(tail_peak < 1e-3, "tail peak {tail_peak}");
}

// ---- Ogg Opus (RFC 3533 + RFC 7845) for the ffmpeg oracle ----

fn ogg_crc(data: &[u8]) -> u32 {
    let mut crc = 0u32;
    for &b in data {
        crc ^= (b as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ 0x04C1_1DB7 } else { crc << 1 };
        }
    }
    crc
}

fn ogg_page(out: &mut Vec<u8>, packet: &[u8], header_type: u8, granule: u64, seq: u32) {
    let mut lacing = vec![255u8; packet.len() / 255];
    lacing.push((packet.len() % 255) as u8);
    let start = out.len();
    out.extend_from_slice(b"OggS");
    out.push(0);
    out.push(header_type);
    out.extend_from_slice(&granule.to_le_bytes());
    out.extend_from_slice(&0x4543_4F50u32.to_le_bytes()); // serial
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // CRC placeholder
    out.push(lacing.len() as u8);
    out.extend_from_slice(&lacing);
    out.extend_from_slice(packet);
    let crc = ogg_crc(&out[start..]);
    out[start + 22..start + 26].copy_from_slice(&crc.to_le_bytes());
}

fn ogg_opus(enc: &OpusEncoder, packets: &[Vec<u8>], len: usize) -> Vec<u8> {
    let mut out = Vec::new();
    ogg_page(&mut out, &enc.opus_head(), 0x02, 0, 0);
    let vendor = b"effectcraft-opusenc";
    let mut tags = b"OpusTags".to_vec();
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0u32.to_le_bytes());
    ogg_page(&mut out, &tags, 0, 0, 1);
    let pre_skip = enc.pre_skip() as u64;
    for (i, p) in packets.iter().enumerate() {
        let last = i + 1 == packets.len();
        let granule = if last { pre_skip + len as u64 } else { ((i + 1) * FRAME) as u64 };
        ogg_page(&mut out, p, if last { 0x04 } else { 0 }, granule, i as u32 + 2);
    }
    out
}

#[test]
fn ogg_crc_known_value() {
    // CRC-32 (poly 0x04C11DB7, no reflection, init 0) of "123456789".
    assert_eq!(ogg_crc(b"123456789"), 0x89A1_897F);
}

#[test]
fn ffmpeg_oracle_decodes_ogg_opus() {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("ffmpeg not installed; skipping the oracle test");
        return;
    }
    for (channels, rate) in [(1u8, 96_000u32), (2, 192_000)] {
        let len = 48_000;
        let planar: Vec<Vec<f32>> =
            if channels == 1 { vec![chirp(150.0, 9_000.0, 0.4, len)] } else { vec![sine(440.0, 0.5, len), chirp(150.0, 9_000.0, 0.3, len)] };
        let c = channels as usize;
        let mut pcm = vec![0f32; len * c];
        for (ch, p) in planar.iter().enumerate() {
            for (i, &v) in p.iter().enumerate() {
                pcm[i * c + ch] = v;
            }
        }
        let (enc, packets) = encode(&pcm, channels, rate);
        let path = std::env::temp_dir().join(format!("effectcraft-opusenc-{}-{channels}.opus", std::process::id()));
        std::fs::write(&path, ogg_opus(&enc, &packets, len)).unwrap();
        let out = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&path)
            .args(["-f", "f32le", "-ac", &channels.to_string(), "-ar", "48000", "-"])
            .output()
            .expect("run ffmpeg");
        let _ = std::fs::remove_file(&path);
        assert!(out.status.success(), "ffmpeg failed: {}", String::from_utf8_lossy(&out.stderr));
        let dec: Vec<f32> = out.stdout.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        assert_eq!(dec.len(), len * c, "ffmpeg must honour pre-skip and the end granule");
        for ch in 0..c {
            let d: Vec<f32> = (0..len).map(|i| dec[i * c + ch]).collect();
            let s = snr(&planar[ch], &d);
            eprintln!("ffmpeg {channels} ch @ {} kb/s, channel {ch}: SNR {s:.1} dB", rate / 1000);
            assert!(s > 15.0, "ffmpeg-decoded SNR {s:.1} dB (channel {ch})");
        }
    }
}
