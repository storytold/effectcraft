//! Shared helpers for the SILK/hybrid round-trip tests: synthetic signals, encoding, decoding
//! with FilmCraft's spec-derived decoder, delay-searched SNR and an Ogg Opus writer for ffmpeg.
#![allow(dead_code)]

use effectcraft_opusenc::OpusEncoder;

pub const SR: f64 = 48_000.0;
pub const FRAME: usize = OpusEncoder::FRAME_SIZE;

/// Two-pole resonator state.
struct Reson {
    y1: f64,
    y2: f64,
}

impl Reson {
    fn run(&mut self, x: f64, f: f64, bw: f64) -> f64 {
        let r = (-std::f64::consts::PI * bw / SR).exp();
        let a1 = 2.0 * r * (2.0 * std::f64::consts::PI * f / SR).cos();
        let a2 = -r * r;
        let g = 1.0 - r;
        let y = g * x + a1 * self.y1 + a2 * self.y2;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// Speech-like test signal: a glottal pulse train (100–200 Hz, gliding pitch) through three
/// formant resonators whose frequencies change per syllable, with syllable envelopes, short
/// pauses and a little aspiration noise.
pub fn speech(len: usize, seed: u32) -> Vec<f32> {
    let vowels: [[f64; 3]; 5] = [[730.0, 1090.0, 2440.0], [270.0, 2290.0, 3010.0], [530.0, 1840.0, 2480.0], [570.0, 840.0, 2410.0], [300.0, 870.0, 2240.0]];
    let syl = (0.24 * SR) as usize;
    let mut out = Vec::with_capacity(len);
    let mut res = [Reson { y1: 0.0, y2: 0.0 }, Reson { y1: 0.0, y2: 0.0 }, Reson { y1: 0.0, y2: 0.0 }];
    let mut phase = 0.0f64;
    let mut s = seed;
    let mut prev_glottal = 0.0;
    for n in 0..len {
        let t = n as f64 / SR;
        let k = n / syl;
        let pos = (n % syl) as f64 / syl as f64;
        // Every fourth syllable is a pause.
        let pause = k % 4 == 3;
        let env = if pause { 0.0 } else { (std::f64::consts::PI * pos).sin().powf(0.7) };
        let f0 = 100.0 + 50.0 * (1.0 + (2.0 * std::f64::consts::PI * 0.9 * t).sin()) + 15.0 * ((k * 7) % 5) as f64 - 30.0 * pos;
        phase += f0 / SR;
        if phase >= 1.0 {
            phase -= 1.0;
        }
        // Rosenberg-like glottal flow; its derivative excites the vocal tract.
        let g = if phase < 0.4 {
            0.5 * (1.0 - (std::f64::consts::PI * phase / 0.4).cos())
        } else if phase < 0.56 {
            (std::f64::consts::FRAC_PI_2 * (phase - 0.4) / 0.16).cos()
        } else {
            0.0
        };
        let dg = (g - prev_glottal) * 40.0;
        prev_glottal = g;
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = ((s >> 8) as f64 / (1u32 << 24) as f64 - 0.5) * 0.02;
        let v = vowels[(k * 3 + 1) % 5];
        let w = vowels[(k * 3 + 4) % 5];
        let mix = pos;
        let mut x = dg + noise;
        let mut y = 0.0;
        for (i, r) in res.iter_mut().enumerate() {
            let f = v[i] * (1.0 - mix) + w[i] * mix;
            y += r.run(x, f, 60.0 + 40.0 * i as f64) * [1.0, 0.6, 0.3][i];
        }
        x = y * env * 1.2;
        out.push(x as f32);
    }
    let peak = out.iter().fold(0f32, |m, v| m.max(v.abs()));
    let g = 0.5 / peak.max(1e-9);
    out.iter().map(|v| v * g).collect()
}

/// Music-like test signal: three-note chords of harmonic tones (six partials each), changing
/// every half second with soft attacks and releases.
pub fn music(len: usize) -> Vec<f32> {
    let chords: [[f64; 3]; 4] = [[261.63, 329.63, 392.0], [220.0, 277.18, 329.63], [174.61, 220.0, 261.63], [196.0, 246.94, 293.66]];
    let seg = (0.5 * SR) as usize;
    (0..len)
        .map(|n| {
            let t = n as f64 / SR;
            let k = n / seg;
            let pos = (n % seg) as f64 / seg as f64;
            let env = (pos * 20.0).min(1.0) * ((1.0 - pos) * 10.0).min(1.0);
            let mut x = 0.0;
            for (j, &f) in chords[k % 4].iter().enumerate() {
                for h in 1..=6 {
                    x += (2.0 * std::f64::consts::PI * f * h as f64 * t + j as f64).sin() / h as f64;
                }
            }
            (0.12 * env * x) as f32
        })
        .collect()
}

pub fn interleave(planar: &[Vec<f32>]) -> Vec<f32> {
    let c = planar.len();
    let len = planar[0].len();
    let mut pcm = vec![0f32; len * c];
    for (ch, p) in planar.iter().enumerate() {
        for (i, &v) in p.iter().enumerate() {
            pcm[i * c + ch] = v;
        }
    }
    pcm
}

/// Encodes planar input (padding the tail so the encoder delay is flushed).
pub fn encode(mut enc: OpusEncoder, planar: &[Vec<f32>]) -> (OpusEncoder, Vec<Vec<u8>>) {
    let c = planar.len();
    let len = planar[0].len();
    let total = len + enc.pre_skip() as usize + 16;
    let frames = total.div_ceil(FRAME);
    let mut pcm = interleave(planar);
    pcm.resize(frames * FRAME * c, 0.0);
    let packets = pcm.chunks(FRAME * c).map(|f| enc.encode_float(f)).collect();
    (enc, packets)
}

/// Decodes with filmcraft-opus, drops the pre-skip and returns planar channels (full length).
pub fn decode(head: &[u8], packets: &[Vec<u8>]) -> Vec<Vec<f32>> {
    let mut dec = filmcraft_opus::Decoder::new(head).expect("OpusHead");
    dec.set_trim_pre_skip(true);
    let mut out: Vec<Vec<f32>> = vec![Vec::new(); dec.channels()];
    for p in packets {
        let planes = dec.decode(Some(p)).expect("decode");
        for (o, p) in out.iter_mut().zip(planes) {
            o.extend_from_slice(&p);
        }
    }
    out
}

/// Waveform SNR (dB) of `dec[i + shift]` against `orig[i]`, excluding the first and last 20 ms.
pub fn snr_at(orig: &[f32], dec: &[f32], shift: isize) -> f64 {
    let (mut s, mut e) = (0f64, 0f64);
    for i in FRAME..orig.len() - FRAME {
        let j = i as isize + shift;
        let d = if j >= 0 && (j as usize) < dec.len() { dec[j as usize] as f64 } else { 0.0 };
        s += (orig[i] as f64).powi(2);
        e += (orig[i] as f64 - d).powi(2);
    }
    10.0 * (s / e.max(1e-30)).log10()
}

/// Best SNR over residual delays of ±`range` samples: (SNR dB, delay).
pub fn snr_search(orig: &[f32], dec: &[f32], range: isize) -> (f64, isize) {
    (-range..=range).map(|d| (snr_at(orig, dec, d), d)).fold((f64::MIN, 0), |a, b| if b.0 > a.0 { b } else { a })
}

/// Segmental SNR (dB): mean over 20 ms segments holding signal (each clamped to [-10, 40]).
pub fn seg_snr(orig: &[f32], dec: &[f32], shift: isize) -> f64 {
    let mut acc = 0f64;
    let mut n = 0;
    let mut i = FRAME;
    while i + FRAME <= orig.len() - FRAME {
        let (mut s, mut e) = (0f64, 0f64);
        for k in i..i + FRAME {
            let j = k as isize + shift;
            let d = if j >= 0 && (j as usize) < dec.len() { dec[j as usize] as f64 } else { 0.0 };
            s += (orig[k] as f64).powi(2);
            e += (orig[k] as f64 - d).powi(2);
        }
        if s / FRAME as f64 > 1e-5 {
            acc += (10.0 * (s / e.max(1e-30)).log10()).clamp(-10.0, 40.0);
            n += 1;
        }
        i += FRAME;
    }
    acc / n.max(1) as f64
}

/// Average bitrate (bits per second) of a packet sequence of 20 ms packets.
pub fn bitrate(packets: &[Vec<u8>]) -> f64 {
    packets.iter().map(|p| p.len() * 8).sum::<usize>() as f64 / (packets.len() as f64 * 0.02)
}

/// Low-pass filtered copy (FFT brick wall) used to compare band-limited modes fairly.
pub fn lowpass(x: &[f32], fc: f64) -> Vec<f32> {
    let n = x.len().next_power_of_two();
    let mut re: Vec<f64> = x.iter().map(|&v| v as f64).collect();
    re.resize(n, 0.0);
    let mut im = vec![0f64; n];
    fft(&mut re, &mut im, false);
    for k in 0..n {
        let f = (k.min(n - k)) as f64 * SR / n as f64;
        if f > fc {
            re[k] = 0.0;
            im[k] = 0.0;
        }
    }
    fft(&mut re, &mut im, true);
    re[..x.len()].iter().map(|&v| (v / n as f64) as f32).collect()
}

pub fn fft(re: &mut [f64], im: &mut [f64], inverse: bool) {
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
        let ang = if inverse { 2.0 } else { -2.0 } * std::f64::consts::PI / len as f64;
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
    out.extend_from_slice(&0x4543_4F50u32.to_le_bytes());
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.push(lacing.len() as u8);
    out.extend_from_slice(&lacing);
    out.extend_from_slice(packet);
    let crc = ogg_crc(&out[start..]);
    out[start + 22..start + 26].copy_from_slice(&crc.to_le_bytes());
}

/// An Ogg Opus file holding `packets` for `len` samples of audio.
pub fn ogg_opus(enc: &OpusEncoder, packets: &[Vec<u8>], len: usize) -> Vec<u8> {
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
