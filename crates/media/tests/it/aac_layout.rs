//! Multichannel AAC reaches stereo by where each channel plays, whatever order its channel
//! configuration decodes in (#1701). Fixtures are tones encoded with FilmCraft's AAC encoder (no
//! ffmpeg needed, so nothing is skipped), with the configuration rewritten where the coded
//! elements fit another: configurations 11 and 12 code the same elements as the encoder's 7- and
//! 8-channel output, and a program config element can name any of them.

use std::f64::consts::PI;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;

use effectcraft_media::{MediaPool, probe, probe_bytes};
use effectcraft_time::Tick;
use filmcraft_aac::{AudioSpecificConfig, Encoder, EncoderConfig, ProgramConfig};

const RATE: u32 = 48_000;
/// Tone frequencies by channel; an LFE gets [`LFE_TONE`], which its element can carry.
const FREQS: [u32; 8] = [300, 500, 700, 900, 1100, 1300, 1500, 1700];
const LFE_TONE: u32 = 60;
const AMP: f32 = 0.25;
const K: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// A channel's content, by where it plays.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Ch {
    L,
    R,
    C,
    Lfe,
    Ls,
    Rs,
    Cs,
}

impl Ch {
    /// ITU-R BS.775 gains into `[left, right]`, before normalisation.
    fn gains(self) -> [f64; 2] {
        match self {
            Ch::L => [1.0, 0.0],
            Ch::R => [0.0, 1.0],
            Ch::C => [K, K],
            Ch::Lfe => [0.0, 0.0],
            Ch::Ls => [K, 0.0],
            Ch::Rs => [0.0, K],
            Ch::Cs => [0.5, 0.5],
        }
    }
}

struct Case {
    name: &'static str,
    /// What the encoder's input channels carry (its element order: C, L R, …, LFE last).
    content: &'static [Ch],
    /// The AudioSpecificConfig written to the container instead of the encoder's, if any.
    asc: Option<AudioSpecificConfig>,
}

fn lc(config: u8) -> Option<AudioSpecificConfig> {
    Some(AudioSpecificConfig::lc(3, config))
}

fn pce(front: Vec<(bool, u8)>, side: Vec<(bool, u8)>, back: Vec<(bool, u8)>, lfe: Vec<u8>, comment: &[u8]) -> Option<AudioSpecificConfig> {
    let pce = ProgramConfig { object_type: 1, sf_index: 3, front, side, back, lfe, comment: comment.to_vec(), ..Default::default() };
    Some(AudioSpecificConfig { pce: Some(pce), ..AudioSpecificConfig::lc(3, 0) })
}

use Ch::*;

fn cases() -> Vec<Case> {
    vec![
        Case { name: "config3", content: &[C, L, R], asc: None },
        Case { name: "config4", content: &[C, L, R, Cs], asc: None },
        Case { name: "config5", content: &[C, L, R, Ls, Rs], asc: None },
        Case { name: "config6", content: &[C, L, R, Ls, Rs, Lfe], asc: None },
        // configuration 7 as ffmpeg writes 7.1: the second and third pairs are surrounds
        Case { name: "config7", content: &[C, L, R, Ls, Rs, Ls, Rs, Lfe], asc: None },
        Case { name: "config11", content: &[C, L, R, Ls, Rs, Cs, Lfe], asc: lc(11) },
        Case { name: "config12", content: &[C, L, R, Ls, Rs, Ls, Rs, Lfe], asc: lc(12) },
        // the encoder's own 6.1: a PCE listing the centre first
        Case { name: "pce61-centre-first", content: &[C, L, R, Ls, Rs, Cs, Lfe], asc: None },
        // ffmpeg's 6.1 PCE: front pair and centre, a side SCE, a back pair and SCE, no LFE
        // element, its channels in ffmpeg's order L R C LFE Cs Ls Rs. The decoder places the
        // encoder's SCE 0 in the front SCE, pair 0 front, pair 1 back, SCE 1 in the back SCE (tag
        // 1) and the LFE in the side SCE left over (tag 5).
        Case {
            name: "pce61-ffmpeg",
            content: &[C, L, R, Cs, Ls, Rs, Lfe],
            asc: pce(vec![(true, 0), (false, 0)], vec![(false, 5)], vec![(true, 1), (false, 1)], vec![], b"Lavc62.28.102"),
        },
    ]
}

fn freq(content: &[Ch], i: usize) -> u32 {
    if content.get(i) == Some(&Lfe) { LFE_TONE } else { FREQS[i] }
}

/// Two seconds of `content`'s tones encoded as AAC: the AudioSpecificConfig and access units.
fn encode(content: &[Ch], adts: bool) -> (Vec<u8>, Vec<Vec<u8>>) {
    let n = content.len();
    let mut cfg = EncoderConfig::cbr(RATE, n, 96_000 * n as u32);
    cfg.adts = adts;
    let mut enc = Encoder::new(cfg).expect("encoder");
    let planes: Vec<Vec<f32>> =
        (0..n).map(|c| (0..2 * RATE).map(|i| AMP * (2.0 * PI * f64::from(freq(content, c)) * f64::from(i) / f64::from(RATE)).sin() as f32).collect()).collect();
    let refs: Vec<&[f32]> = planes.iter().map(Vec::as_slice).collect();
    let mut units = enc.encode(&refs);
    units.extend(enc.flush());
    (enc.audio_specific_config(), units)
}

fn mp4(asc: Vec<u8>, channels: u32, units: &[Vec<u8>]) -> Vec<u8> {
    mp4_after(None, asc, channels, units)
}

/// As [`mp4`], after an audio track with the configuration and channel count `empty` and no
/// samples, if any.
fn mp4_after(empty: Option<(Vec<u8>, u32)>, asc: Vec<u8>, channels: u32, units: &[Vec<u8>]) -> Vec<u8> {
    use filmcraft_isobmff::{Brand, Mp4Writer, SampleEntry, TrackConfig, WriteSample, WriterOptions};
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), WriterOptions::new(Brand::Mp4)).expect("writer");
    if let Some((asc, channels)) = empty {
        w.add_track(TrackConfig::new(SampleEntry::aac(asc, channels, RATE), RATE)).expect("empty track");
    }
    let t = w.add_track(TrackConfig::new(SampleEntry::aac(asc, channels, RATE), RATE)).expect("track");
    for u in units {
        w.write_sample(t, WriteSample { data: u, duration: 1024, composition_offset: 0, is_sync: true }).expect("sample");
    }
    w.finish().expect("finish").into_inner()
}

fn mkv(asc: Vec<u8>, channels: u32, units: &[Vec<u8>]) -> Vec<u8> {
    mkv_after(None, asc, channels, units)
}

/// As [`mkv`], after an audio track with the configuration and channel count `empty` and no
/// frames, if any.
fn mkv_after(empty: Option<(Vec<u8>, u32)>, asc: Vec<u8>, channels: u32, units: &[Vec<u8>]) -> Vec<u8> {
    use filmcraft_matroska::{MkvWriter, MuxOptions, TrackKind, TrackSpec};
    let track = |asc: Vec<u8>, channels: u32| {
        let mut t = TrackSpec::new(TrackKind::Audio, "A_AAC");
        t.codec_private = asc;
        t.audio = Some((f64::from(RATE), u64::from(channels), None));
        t
    };
    let tracks: Vec<TrackSpec> = empty.into_iter().map(|(asc, n)| track(asc, n)).chain([track(asc, channels)]).collect();
    let played = tracks.len() - 1;
    let mut w = MkvWriter::new(Cursor::new(Vec::new()), tracks, MuxOptions::default()).expect("writer");
    let frame_ns = 1_000_000_000 * 1024 / i64::from(RATE);
    for (i, u) in units.iter().enumerate() {
        w.write_frame(played, i as i64 * frame_ns, true, u, Some(frame_ns as u64)).expect("frame");
    }
    w.finish().expect("finish").into_inner()
}

/// The fixture written to disk as `name`.
fn write(name: &str, bytes: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("effectcraft-aac-layout-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let p = dir.join(name);
    std::fs::write(&p, bytes).expect("write");
    p
}

/// Amplitude of the tone at `freq` in one side of interleaved stereo.
fn amp(s: &[f32], side: usize, freq: u32) -> f64 {
    let (mut re, mut im, mut n) = (0.0, 0.0, 0usize);
    for (i, f) in s.as_chunks::<2>().0.iter().enumerate() {
        let w = 2.0 * PI * f64::from(freq) * i as f64 / f64::from(RATE);
        re += f64::from(f[side]) * w.cos();
        im += f64::from(f[side]) * w.sin();
        n += 1;
    }
    2.0 * (re * re + im * im).sqrt() / n.max(1) as f64
}

/// Stereo of one second from 0.5 s in, through the media pool as footage reads it.
fn mix(pool: &MediaPool, footage: &effectcraft_project::Footage) -> Vec<f32> {
    pool.audio_samples(footage, Tick::from_seconds_f64(0.5), RATE as usize, RATE)
}

/// Each channel of `content` reaches each side at its BS.775 gain, normalised by the larger
/// side's sum (an LFE nowhere).
fn check(label: &str, content: &[Ch], s: &[f32]) {
    let raw: Vec<[f64; 2]> = content.iter().map(|c| c.gains()).collect();
    let norm = raw.iter().map(|g| g[0]).sum::<f64>().max(raw.iter().map(|g| g[1]).sum::<f64>()).max(1.0);
    for (i, (ch, g)) in content.iter().zip(&raw).enumerate() {
        for side in 0..2 {
            let (got, want) = (amp(s, side, freq(content, i)), f64::from(AMP) * g[side] / norm);
            assert!(
                (got - want).abs() <= f64::from(AMP) * 0.05,
                "{label}: {ch:?} ({} Hz) -> {} is {got:.4}, expected {want:.4}",
                freq(content, i),
                ["L", "R"][side]
            );
        }
    }
}

/// The LFE tone survives encoding, so its absence from the mix means it was left out.
fn lfe_is_coded(case: &Case, bytes: Vec<u8>) {
    if !case.content.contains(&Lfe) {
        return;
    }
    let src = filmcraft_codecs::open_bytes("lfe.mp4", bytes.into()).expect("open");
    let buf = src.audio(24_000, RATE as usize, RATE).expect("decode");
    let best = buf.channels.iter().map(|ch| amp(&ch.iter().flat_map(|&v| [v, v]).collect::<Vec<_>>(), 0, LFE_TONE)).fold(0.0, f64::max);
    assert!(best > 0.8 * f64::from(AMP), "{}: the LFE tone is coded ({best:.4})", case.name);
}

#[test]
fn every_aac_layout_mixes_by_speaker_in_mp4() {
    for case in cases() {
        let (asc, units) = encode(case.content, false);
        let asc = case.asc.as_ref().map_or(asc, AudioSpecificConfig::to_bytes);
        let bytes = mp4(asc, case.content.len() as u32, &units);
        lfe_is_coded(&case, bytes.clone());
        let p = write(&format!("{}.mp4", case.name), &bytes);
        check(case.name, case.content, &mix(&MediaPool::new(), &probe(&p).expect("probe")));
        // in memory (`add_bytes`) as well as in place
        let (path, bytes): (String, Arc<[u8]>) = (format!("/mem/{}.mp4", case.name), bytes.into());
        let pool = MediaPool::new();
        pool.add_bytes(&path, bytes.clone());
        check(&format!("{} in memory", case.name), case.content, &mix(&pool, &probe_bytes(&path, bytes).expect("probe")));
    }
}

#[test]
fn matroska_aac_layouts_mix_by_speaker() {
    for case in cases().into_iter().filter(|c| matches!(c.name, "config12" | "pce61-centre-first" | "pce61-ffmpeg")) {
        let (asc, units) = encode(case.content, false);
        let asc = case.asc.as_ref().map_or(asc, AudioSpecificConfig::to_bytes);
        let p = write(&format!("{}.mkv", case.name), &mkv(asc, case.content.len() as u32, &units));
        check(case.name, case.content, &mix(&MediaPool::new(), &probe(&p).expect("probe")));
    }
}

/// ADTS signals configurations 1 to 7 only, which the channel count tells apart.
#[test]
fn adts_aac_layouts_mix_by_speaker() {
    for case in cases().into_iter().filter(|c| c.asc.is_none() && c.content.len() != 7) {
        let (_, units) = encode(case.content, true);
        let p = write(&format!("{}.aac", case.name), &units.concat());
        check(case.name, case.content, &mix(&MediaPool::new(), &probe(&p).expect("probe")));
    }
}

/// Audio plays from the first track FilmCraft can play, so an audio track with no samples before
/// it lends it no configuration. Each case decodes in another order than the empty track's
/// configuration would (FilmCraft reorders 12 to WAVE / SMPTE but not 7, and 11 but not a PCE).
#[test]
fn an_empty_audio_track_lends_its_configuration_to_no_other() {
    for (name, empty) in [("config12", 7), ("pce61-centre-first", 11)] {
        let case = cases().into_iter().find(|c| c.name == name).expect("case");
        let n = case.content.len() as u32;
        let empty = Some((AudioSpecificConfig::lc(3, empty).to_bytes(), n));
        let (asc, units) = encode(case.content, false);
        let asc = case.asc.as_ref().map_or(asc, AudioSpecificConfig::to_bytes);
        let p = write(&format!("{name}-after-empty.mp4"), &mp4_after(empty.clone(), asc.clone(), n, &units));
        check(&format!("{name} MP4"), case.content, &mix(&MediaPool::new(), &probe(&p).expect("probe")));
        let p = write(&format!("{name}-after-empty.mkv"), &mkv_after(empty, asc, n, &units));
        check(&format!("{name} Matroska"), case.content, &mix(&MediaPool::new(), &probe(&p).expect("probe")));
    }
}

/// A transport stream plays its first audio stream and mixes by that stream's configuration,
/// whatever follows it. ADTS of configuration 7 decodes in element order, LATM of configuration
/// 12 reordered to WAVE / SMPTE; LATM of configuration 7 names 7 channels, which mix as
/// configuration 7's rather than as 7 channels by count. (FilmCraft decodes LATM of configuration
/// 12 as 12 channels, so it isn't played here.)
#[test]
fn mpeg_aac_mixes_by_the_stream_played() {
    let case = cases().into_iter().find(|c| c.name == "config7").expect("case");
    let (_, adts) = encode(case.content, true);
    let (_, raw) = encode(case.content, false);
    for (label, latm_first, latm_config) in [("ADTS before LATM", false, 12), ("LATM before ADTS", true, 7)] {
        let (adts_pid, latm_pid) = if latm_first { (0x102, 0x101) } else { (0x101, 0x102) };
        let mut ts = Ts::default();
        let mut streams = [(0x0F, adts_pid), (0x11, latm_pid)];
        streams.sort_by_key(|s| s.1);
        ts.psi(&streams);
        for (i, (a, r)) in adts.iter().zip(&raw).enumerate() {
            let pts = 1920 * i as i64;
            let mut units = [(adts_pid, pes(pts, a)), (latm_pid, pes(pts, &loas(latm_config, r)))];
            units.sort_by_key(|u| u.0);
            for (pid, unit) in units {
                ts.pes(pid, &unit);
            }
        }
        let p = write(&format!("latm-first-{latm_first}.ts"), &ts.out);
        check(label, case.content, &mix(&MediaPool::new(), &probe(&p).expect("probe")));
    }
}

/// A LOAS frame of `payload` with its StreamMuxConfig: AAC LC, 48 kHz, channel configuration
/// `config`.
fn loas(config: u32, payload: &[u8]) -> Vec<u8> {
    let mut bits = Bits::default();
    // useSameStreamMux, audioMuxVersion, allStreamsSameTimeFraming, numSubFrames, numProgram, numLayer
    bits.put(0b001, 3);
    bits.put(0, 13);
    // AudioSpecificConfig and GASpecificConfig
    bits.put(2, 5);
    bits.put(3, 4);
    bits.put(config, 4);
    bits.put(0, 3);
    // frameLengthType, latmBufferFullness, otherDataPresent, crcCheckPresent
    bits.put(0, 3);
    bits.put(0xFF, 8);
    bits.put(0, 2);
    let mut len = payload.len();
    while len >= 255 {
        bits.put(255, 8);
        len -= 255;
    }
    bits.put(len as u32, 8);
    for &b in payload {
        bits.put(u32::from(b), 8);
    }
    let body = bits.finish();
    let mut v = vec![0x56, 0xE0 | (body.len() >> 8) as u8, body.len() as u8];
    v.extend(body);
    v
}

/// A big-endian bit writer.
#[derive(Default)]
struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn put(&mut self, v: u32, n: u32) {
        self.acc = self.acc << n | u64::from(v);
        self.n += n;
        while self.n >= 8 {
            self.n -= 8;
            self.out.push((self.acc >> self.n) as u8);
        }
    }
    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.put(0, 8 - self.n);
        }
        self.out
    }
}

/// An audio PES packet (stream id 0xC0) presented at `pts` (90 kHz).
fn pes(pts: i64, payload: &[u8]) -> Vec<u8> {
    let t = [0x21 | ((pts >> 29) & 0x0E) as u8, (pts >> 22) as u8, ((pts >> 14) & 0xFE) as u8 | 1, (pts >> 7) as u8, ((pts << 1) & 0xFE) as u8 | 1];
    let len = 8 + payload.len();
    let mut v = vec![0, 0, 1, 0xC0, (len >> 8) as u8, len as u8, 0x80, 0x80, 5];
    v.extend(t);
    v.extend_from_slice(payload);
    v
}

/// A transport stream of one program (its PMT on PID 0x100), PES cut into 188-byte packets.
#[derive(Default)]
struct Ts {
    out: Vec<u8>,
    cc: std::collections::HashMap<u16, u8>,
}

impl Ts {
    fn packet(&mut self, pid: u16, start: bool, payload: &[u8]) -> usize {
        let cc = self.cc.entry(pid).or_insert(0);
        let mut p = vec![0x47, u8::from(start) << 6 | (pid >> 8) as u8, pid as u8];
        let n = payload.len().min(184);
        if n < 184 {
            let stuffing = 183 - n;
            p.extend([0x30 | *cc, stuffing as u8]);
            if stuffing > 0 {
                p.push(0);
                p.extend(std::iter::repeat_n(0xFF, stuffing - 1));
            }
        } else {
            p.push(0x10 | *cc);
        }
        *cc = (*cc + 1) & 15;
        p.extend_from_slice(&payload[..n]);
        self.out.extend(p);
        n
    }
    fn section(&mut self, pid: u16, body: &[u8]) {
        let mut s = vec![0];
        s.extend_from_slice(body);
        s.extend(crc32_mpeg(body).to_be_bytes());
        self.packet(pid, true, &s);
    }
    /// The PAT and a PMT of `streams` (stream type, PID).
    fn psi(&mut self, streams: &[(u8, u16)]) {
        self.section(0, &[0x00, 0xB0, 13, 0, 1, 0xC1, 0, 0, 0, 1, 0xE1, 0x00]);
        let mut pmt = vec![0x02, 0xB0, 0, 0, 1, 0xC1, 0, 0, 0xE1, 0x01, 0xF0, 0];
        for &(kind, pid) in streams {
            pmt.extend([kind, 0xE0 | (pid >> 8) as u8, pid as u8, 0xF0, 0]);
        }
        pmt[2] = (pmt.len() + 1) as u8;
        self.section(0x100, &pmt);
    }
    fn pes(&mut self, pid: u16, data: &[u8]) {
        let (mut rest, mut start) = (data, true);
        while !rest.is_empty() {
            let n = self.packet(pid, start, rest);
            rest = &rest[n..];
            start = false;
        }
    }
}

/// CRC-32 of PSI sections (MSB first, polynomial 0x04C11DB7, initial value all ones).
fn crc32_mpeg(data: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &b in data {
        crc ^= u32::from(b) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 { crc << 1 ^ 0x04C1_1DB7 } else { crc << 1 };
        }
    }
    crc
}
