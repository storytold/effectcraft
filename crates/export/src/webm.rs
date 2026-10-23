//! WebM output (VP9 video, optional VP9 alpha, Opus audio) and audio-only WAV / AIFF.
//!
//! The WebM container is written from the Matroska / WebM specifications (IETF RFC 8794 EBML,
//! the Matroska element registry and the WebM container guidelines): EBML header, Segment
//! (size patched at the end), Info, Tracks and one Cluster per render batch. Video frames are
//! SimpleBlocks; with alpha (Channels: RGB + Alpha) each frame is a BlockGroup whose
//! BlockAdditional (BlockAddID 1) carries a second VP9 frame coding the alpha channel as luma,
//! with `AlphaMode` 1 on the track, as WebM defines. VP9 comes from `effectcraft-vp9enc` (key
//! frames every two seconds, inter frames between them: SimpleBlocks carry the key flag, inter
//! BlockGroups a ReferenceBlock; the alpha stream's key frames line up with the colour stream's),
//! Opus from `effectcraft-opusenc` (CELT, 48 kHz stereo, 20 ms packets,
//! `CodecDelay` = pre-skip). WAV (RIFF, PCM or IEEE float) and AIFF (big-endian PCM, 80-bit
//! extended sample rate; AIFF-C `fl32` for float) follow their published file layouts.

use std::io::{Seek, SeekFrom, Write};

use effectcraft_project::Comp;
use effectcraft_project::render_queue::{AudioFormat, Channels};
use effectcraft_time::{TICKS_PER_SECOND, Tick};

use crate::encode::{mix, pcm_bytes};
use crate::{Cx, Report, Result, State, batch_size, io, wants_audio};

pub(crate) const OPUS_RATE: u32 = 48_000;

// Element ids (with their length markers).
pub(crate) const EBML: u32 = 0x1A45_DFA3;
pub(crate) const SEGMENT: u32 = 0x1853_8067;
pub(crate) const INFO: u32 = 0x1549_A966;
pub(crate) const TRACKS: u32 = 0x1654_AE6B;
pub(crate) const TRACK_ENTRY: u32 = 0xAE;
pub(crate) const CLUSTER: u32 = 0x1F43_B675;

pub(crate) fn id_bytes(id: u32) -> Vec<u8> {
    let b = id.to_be_bytes();
    let skip = b.iter().position(|x| *x != 0).unwrap_or(3);
    b[skip..].to_vec()
}

/// EBML variable-size integer for an element data size (shortest form).
pub(crate) fn vint(n: u64) -> Vec<u8> {
    for len in 1..=8u32 {
        let max = (1u64 << (7 * len)) - 2;
        if n <= max {
            let marked = n | (1u64 << (7 * len));
            return marked.to_be_bytes()[8 - len as usize..].to_vec();
        }
    }
    let mut v = vec![0x01];
    v.extend_from_slice(&n.to_be_bytes()[1..]);
    v
}

pub(crate) fn el(out: &mut Vec<u8>, id: u32, data: &[u8]) {
    out.extend(id_bytes(id));
    out.extend(vint(data.len() as u64));
    out.extend_from_slice(data);
}

pub(crate) fn el_uint(out: &mut Vec<u8>, id: u32, v: u64) {
    let b = v.to_be_bytes();
    let skip = b.iter().position(|x| *x != 0).unwrap_or(7);
    el(out, id, &b[skip..]);
}

pub(crate) fn el_float(out: &mut Vec<u8>, id: u32, v: f64) {
    el(out, id, &v.to_be_bytes());
}

pub(crate) fn el_str(out: &mut Vec<u8>, id: u32, s: &str) {
    el(out, id, s.as_bytes());
}

/// A block payload: track number, timestamp relative to the cluster, flags, frame.
fn block(track: u64, rel: i16, flags: u8, data: &[u8]) -> Vec<u8> {
    let mut b = vint(track);
    b.extend_from_slice(&rel.to_be_bytes());
    b.push(flags);
    b.extend_from_slice(data);
    b
}

struct Frame {
    ms: i64,
    track: u64,
    /// A key frame (VP9) or any audio packet.
    key: bool,
    /// Timestamp of the previous video frame (ReferenceBlock of inter frames).
    prev_ms: Option<i64>,
    data: Vec<u8>,
    alpha: Option<Vec<u8>>,
}

fn cluster(frames: &mut [Frame]) -> Vec<u8> {
    frames.sort_by_key(|f| (f.ms, f.track));
    let base = frames.first().map_or(0, |f| f.ms);
    let mut body = vec![];
    el_uint(&mut body, 0xE7, base.max(0) as u64);
    for f in frames.iter() {
        let rel = (f.ms - base).clamp(i16::MIN as i64, i16::MAX as i64) as i16;
        match &f.alpha {
            None => el(&mut body, 0xA3, &block(f.track, rel, if f.key { 0x80 } else { 0x00 }, &f.data)),
            Some(a) => {
                let mut g = vec![];
                el(&mut g, 0xA1, &block(f.track, rel, 0x00, &f.data));
                if let (false, Some(p)) = (f.key, f.prev_ms) {
                    // ReferenceBlock: a signed timestamp relative to this block (an inter frame).
                    let d = (p - f.ms).clamp(i16::MIN as i64, -1) as i16;
                    el(&mut g, 0xFB, &d.to_be_bytes());
                }
                let mut more = vec![];
                el_uint(&mut more, 0xEE, 1);
                el(&mut more, 0xA5, a);
                let mut adds = vec![];
                el(&mut adds, 0xA6, &more);
                el(&mut g, 0x75A1, &adds);
                el(&mut body, 0xA0, &g);
            }
        }
    }
    let mut out = vec![];
    el(&mut out, CLUSTER, &body);
    out
}

/// The Opus encoder of a WebM output: the module's channel count, bitrate and application.
pub(crate) fn opus_encoder(job: &Cx) -> effectcraft_opusenc::OpusEncoder {
    let channels = if job.output.audio_channels == 1 { 1 } else { 2 };
    let app = match job.output.opus_application {
        effectcraft_project::render_queue::OpusApplication::Audio => effectcraft_opusenc::Application::Audio,
        effectcraft_project::render_queue::OpusApplication::Voip => effectcraft_opusenc::Application::Voip,
    };
    effectcraft_opusenc::OpusEncoder::with_application(channels, job.output.opus_bitrate_kbps.clamp(6, 510) * 1000, app)
}

pub(crate) async fn webm(job: &Cx<'_>, comp: &Comp, w: u32, h: u32, st: &mut State<'_>) -> Result<Report> {
    if job.output.webm_codec == effectcraft_project::render_queue::WebmVideoCodec::Av1 {
        return crate::webm_av1::webm_av1(job, comp, w, h, st).await;
    }
    let rate = job.settings.rate(comp);
    let alpha = job.output.channels == Channels::Rgba;
    let colour_channels = if job.output.channels == Channels::Alpha { Channels::Alpha } else { Channels::Rgb };
    let quality = job.output.quality.clamp(1, 100);
    let fps = rate.as_f64().max(1.0);
    let cfg = effectcraft_vp9enc::EncoderConfig {
        width: w,
        height: h,
        quality,
        full_range: false,
        keyframe_interval: if job.output.keyframe_interval > 0 { job.output.keyframe_interval } else { (fps * 2.0).round().max(1.0) as u32 },
        target_kbps: job.output.webm_bitrate.then_some(job.output.bitrate_kbps.max(50)),
        frame_rate: fps,
        loop_filter: true,
    };
    let mut color_enc = effectcraft_vp9enc::Vp9Encoder::new(cfg.clone());
    let mut alpha_enc = alpha.then(|| effectcraft_vp9enc::Vp9Encoder::new(cfg.clone()));
    let with_audio = wants_audio(job);
    let opus_channels = if job.output.audio_channels == 1 { 1usize } else { 2 };
    let mut opus = with_audio.then(|| opus_encoder(job));
    let total = st.total;
    let frame_ms = |k: u64| -> i64 { ((k as i128 * 1000 * rate.den as i128 + rate.num as i128 / 2) / rate.num as i128) as i64 };
    let duration_ms = frame_ms(total) as f64;

    let path = job.place(job.path, 1);
    let mut file = job.create(&path)?;
    // EBML header.
    let mut head = vec![];
    let mut eb = vec![];
    el_uint(&mut eb, 0x4286, 1);
    el_uint(&mut eb, 0x42F7, 1);
    el_uint(&mut eb, 0x42F2, 4);
    el_uint(&mut eb, 0x42F3, 8);
    el_str(&mut eb, 0x4282, "webm");
    el_uint(&mut eb, 0x4287, 4);
    el_uint(&mut eb, 0x4285, 2);
    el(&mut head, EBML, &eb);
    // Segment with an 8-byte size patched at the end.
    head.extend(id_bytes(SEGMENT));
    let size_pos = head.len() as u64;
    head.extend_from_slice(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
    let seg_start = head.len() as u64;
    let mut info = vec![];
    el_uint(&mut info, 0x2AD7B1, 1_000_000);
    el_float(&mut info, 0x4489, duration_ms);
    el_str(&mut info, 0x4D80, "EffectCraft");
    el_str(&mut info, 0x5741, "EffectCraft");
    el(&mut head, INFO, &info);
    let mut tracks = vec![];
    let mut v = vec![];
    el_uint(&mut v, 0xD7, 1);
    el_uint(&mut v, 0x73C5, 1);
    el_uint(&mut v, 0x83, 1);
    el_uint(&mut v, 0x9C, 0);
    el_str(&mut v, 0x86, "V_VP9");
    el_uint(&mut v, 0x23E383, (1_000_000_000u128 * rate.den as u128 / rate.num.max(1) as u128) as u64);
    if alpha {
        el_uint(&mut v, 0x55EE, 1);
    }
    let mut video = vec![];
    el_uint(&mut video, 0xB0, w as u64);
    el_uint(&mut video, 0xBA, h as u64);
    if alpha {
        el_uint(&mut video, 0x53C0, 1);
    }
    el(&mut v, 0xE0, &video);
    el(&mut tracks, TRACK_ENTRY, &v);
    if let Some(o) = &opus {
        let mut a = vec![];
        el_uint(&mut a, 0xD7, 2);
        el_uint(&mut a, 0x73C5, 2);
        el_uint(&mut a, 0x83, 2);
        el_uint(&mut a, 0x9C, 0);
        el_str(&mut a, 0x86, "A_OPUS");
        el(&mut a, 0x63A2, &o.opus_head());
        el_uint(&mut a, 0x56AA, o.pre_skip() as u64 * 1_000_000_000 / OPUS_RATE as u64);
        el_uint(&mut a, 0x56BB, 80_000_000);
        let mut au = vec![];
        el_float(&mut au, 0xB5, OPUS_RATE as f64);
        el_uint(&mut au, 0x9F, opus_channels as u64);
        el(&mut a, 0xE1, &au);
        el(&mut tracks, TRACK_ENTRY, &a);
    }
    el(&mut head, TRACKS, &tracks);
    file.write_all(&head).map_err(io)?;
    let mut seg_len = head.len() as u64 - seg_start;

    // Audio: mixed in blocks, encoded in 20 ms packets.
    let (span_start, span_end) = job.settings.span(comp);
    let mut cursor = span_start.to_units_floor(OPUS_RATE as i64);
    let mut pcm_left: Vec<f32> = vec![];
    let mut packets = 0u64;
    let mut audio_until = |until: Tick, frames: &mut Vec<Frame>, finish: bool| {
        let Some(enc) = opus.as_mut() else { return };
        let end = until.to_units_floor(OPUS_RATE as i64);
        if end > cursor {
            let n = (end - cursor) as usize;
            let start = Tick(((cursor as i128 * TICKS_PER_SECOND as i128) / OPUS_RATE as i128) as i64);
            pcm_left.extend(mix(job, start, n, OPUS_RATE));
            cursor = end;
        }
        let fs = effectcraft_opusenc::OpusEncoder::FRAME_SIZE * opus_channels;
        if finish && !pcm_left.is_empty() {
            // Pad the last packet (pre-skip + padding cover the encoder delay).
            let pad = (fs - pcm_left.len() % fs) % fs + fs;
            pcm_left.extend(std::iter::repeat_n(0.0, pad));
        }
        while pcm_left.len() >= fs {
            let pkt = enc.encode_float(&pcm_left[..fs]);
            pcm_left.drain(..fs);
            frames.push(Frame { ms: packets as i64 * 20, track: 2, key: true, prev_ms: None, data: pkt, alpha: None });
            packets += 1;
        }
    };

    let batch = batch_size();
    let mut i = 0;
    while i < total {
        let end = (i + batch).min(total);
        // Frames render in parallel; the VP9 streams (inter-coded) encode in order, colour and
        // alpha side by side.
        let pixels: Vec<Vec<u8>> =
            job.frames(comp, (i..end).collect(), |_, img| job.pixels(&img, comp, if alpha { Channels::Rgba } else { colour_channels }, w, h)).await;
        let mut frames: Vec<Frame> = Vec::with_capacity(pixels.len());
        for (j, px) in pixels.iter().enumerate() {
            let (color, a) = rayon::join(
                || {
                    let (y, u, vv) = effectcraft_vp9enc::rgba_to_yuv420(px, w, h, false);
                    color_enc.encode_frame(&y, &u, &vv, false)
                },
                || {
                    alpha_enc.as_mut().map(|ea| {
                        let (y, u, vv) = effectcraft_vp9enc::alpha_to_yuv420(px, w, h);
                        ea.encode_frame(&y, &u, &vv, false).data
                    })
                },
            );
            let k = i + j as u64;
            frames.push(Frame { ms: frame_ms(k), track: 1, key: color.key, prev_ms: (k > 0).then(|| frame_ms(k - 1)), data: color.data, alpha: a });
        }
        drop(pixels);
        let t_end = if end >= total { span_end } else { job.settings.frame_time(comp, end) };
        audio_until(t_end, &mut frames, end >= total);
        let c = cluster(&mut frames);
        file.write_all(&c).map_err(io)?;
        seg_len += c.len() as u64;
        st.advance(end - i)?;
        i = end;
    }
    if total == 0 {
        let mut frames = vec![];
        audio_until(span_end, &mut frames, true);
        if !frames.is_empty() {
            let c = cluster(&mut frames);
            file.write_all(&c).map_err(io)?;
            seg_len += c.len() as u64;
        }
    }
    // Patch the Segment size.
    let mut size = [0u8; 8];
    size[0] = 0x01;
    size[1..].copy_from_slice(&seg_len.to_be_bytes()[1..]);
    file.seek(SeekFrom::Start(size_pos)).map_err(io)?;
    file.write_all(&size).map_err(io)?;
    file.seek(SeekFrom::End(0)).map_err(io)?;
    let bytes = file.finish()?;
    Ok(Report {
        path,
        frames: 0,
        width: w,
        height: h,
        seconds: 0.0,
        bytes,
        audio: with_audio,
        video_encoder: Some("software"),
        hardware_fallback_reason: None,
        log: None,
        overflow: vec![],
    })
}

// ---------------------------------------------------------------- audio-only outputs

fn ieee_extended(v: f64) -> [u8; 10] {
    // 80-bit IEEE 754 extended precision (AIFF COMM sample rate).
    let mut out = [0u8; 10];
    if v <= 0.0 {
        return out;
    }
    let e = v.log2().floor() as i32;
    let mant = (v / 2f64.powi(e) * (1u64 << 63) as f64) as u64;
    out[..2].copy_from_slice(&((e + 16383) as u16).to_be_bytes());
    out[2..].copy_from_slice(&mant.to_be_bytes());
    out
}

/// WAV or AIFF: the comp's audio over the render span (Audio Output: sample rate, mono/stereo,
/// 16/24-bit integer or 32-bit float; plain AIFF has no float, so 32-bit float is AIFF-C `fl32`).
pub(crate) fn audio_file(job: &Cx, comp: &Comp, aiff: bool, st: &mut State) -> Result<Report> {
    let sr = job.output.audio_sample_rate.clamp(8_000, 192_000);
    let channels = if job.output.audio_channels == 1 { 1u16 } else { 2 };
    let fmt = job.output.audio_format;
    let bytes_per_sample: u16 = match fmt {
        AudioFormat::S16 => 2,
        AudioFormat::S24 => 3,
        AudioFormat::F32 => 4,
    };
    let (span_start, span_end) = job.settings.span(comp);
    let start = span_start.to_units_floor(sr as i64);
    let end = span_end.to_units_floor(sr as i64);
    let n = (end - start).max(0) as usize;
    let mut pcm = Vec::with_capacity(n * (channels * bytes_per_sample) as usize);
    // Mix in one-second blocks, reporting progress in frames.
    let block = sr as usize;
    let mut done = 0usize;
    let total_frames = st.total.max(1);
    let mut reported = 0u64;
    while done < n {
        let k = block.min(n - done);
        let sample = start + done as i64;
        let t = Tick::from_units(sample, i64::from(sr));
        let t = if t.to_units_floor(i64::from(sr)) < sample { Tick(t.0.saturating_add(1)) } else { t };
        pcm.extend(pcm_bytes(&mix(job, t, k, sr), fmt, aiff));
        done += k;
        let now = (done as u64 * total_frames / n.max(1) as u64).min(total_frames);
        if now > reported && st.total > 0 {
            st.advance(now - reported)?;
            reported = now;
        }
    }
    if reported < st.total {
        st.advance(st.total - reported)?;
    }
    let block_align = channels * bytes_per_sample;
    let frames = (pcm.len() / block_align as usize) as u32;
    let mut out = vec![];
    if aiff {
        // AIFF-C (Apple, 1991) for float: a format version chunk, and the COMM chunk names the
        // compression type and its Pascal-string name, padded to an even length.
        let float = fmt == AudioFormat::F32;
        let mut comm = vec![];
        comm.extend_from_slice(&channels.to_be_bytes());
        comm.extend_from_slice(&frames.to_be_bytes());
        comm.extend_from_slice(&(bytes_per_sample * 8).to_be_bytes());
        comm.extend_from_slice(&ieee_extended(sr as f64));
        let mut fver = vec![];
        if float {
            const NAME: &[u8] = b"32-bit floating point";
            comm.extend_from_slice(b"fl32");
            comm.push(NAME.len() as u8);
            comm.extend_from_slice(NAME);
            if (1 + NAME.len()) % 2 == 1 {
                comm.push(0);
            }
            fver.extend_from_slice(b"FVER");
            fver.extend_from_slice(&4u32.to_be_bytes());
            // AIFC Version 1 (May 23, 1990, 2:40 pm).
            fver.extend_from_slice(&0xA280_5140u32.to_be_bytes());
        }
        let ssnd_len = 8 + pcm.len();
        out.extend_from_slice(b"FORM");
        out.extend_from_slice(&((4 + fver.len() + 8 + comm.len() + 8 + ssnd_len + ssnd_len % 2) as u32).to_be_bytes());
        out.extend_from_slice(if float { b"AIFC" } else { b"AIFF" });
        out.extend_from_slice(&fver);
        out.extend_from_slice(b"COMM");
        out.extend_from_slice(&(comm.len() as u32).to_be_bytes());
        out.extend_from_slice(&comm);
        out.extend_from_slice(b"SSND");
        out.extend_from_slice(&(ssnd_len as u32).to_be_bytes());
        out.extend_from_slice(&[0; 8]);
        out.extend_from_slice(&pcm);
        if ssnd_len % 2 == 1 {
            out.push(0);
        }
    } else {
        // WAVE_FORMAT_PCM (1) or WAVE_FORMAT_IEEE_FLOAT (3).
        let tag: u16 = if fmt == AudioFormat::F32 { 3 } else { 1 };
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&((36 + pcm.len() + pcm.len() % 2) as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&sr.to_le_bytes());
        out.extend_from_slice(&(sr * block_align as u32).to_le_bytes());
        out.extend_from_slice(&block_align.to_le_bytes());
        out.extend_from_slice(&(bytes_per_sample * 8).to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
        out.extend_from_slice(&pcm);
        if pcm.len() % 2 == 1 {
            out.push(0);
        }
    }
    let path = job.place(job.path, out.len() as u64);
    let mut file = job.create(&path)?;
    file.write_all(&out).map_err(io)?;
    let bytes = file.finish()?;
    Ok(Report {
        path,
        frames: 0,
        width: 0,
        height: 0,
        seconds: 0.0,
        bytes,
        audio: true,
        video_encoder: None,
        hardware_fallback_reason: None,
        log: None,
        overflow: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vints_and_ids() {
        assert_eq!(vint(0), vec![0x80]);
        assert_eq!(vint(126), vec![0xFE]);
        assert_eq!(vint(127), vec![0x40, 0x7F]);
        assert_eq!(id_bytes(0xA3), vec![0xA3]);
        assert_eq!(id_bytes(EBML), vec![0x1A, 0x45, 0xDF, 0xA3]);
        assert_eq!(ieee_extended(44100.0), [0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]);
        assert_eq!(ieee_extended(48000.0)[..4], [0x40, 0x0E, 0xBB, 0x80]);
    }
}
