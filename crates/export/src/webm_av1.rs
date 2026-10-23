//! WebM with AV1 video (`V_AV1`, CodecPrivate = the `av1C` record, per the AV1-in-Matroska
//! codec mapping) and Opus audio. Frames are rendered in parallel batches and encoded in order
//! (AV1 inter frames reference the previous frame); each batch becomes one Cluster whose
//! SimpleBlocks carry the key-frame flag only on AV1 key frames (the Cues-less layout of the
//! VP9 writer: players seek by scanning clusters).

use std::io::{Seek, SeekFrom, Write};

use effectcraft_project::Comp;
use effectcraft_project::render_queue::Channels;
use effectcraft_time::{TICKS_PER_SECOND, Tick};

use crate::encode::VideoEncoder;
use crate::encode::mix;
use crate::webm::{CLUSTER, EBML, INFO, OPUS_RATE, SEGMENT, TRACK_ENTRY, TRACKS, el, el_float, el_str, el_uint, id_bytes, opus_encoder, vint};
use crate::{Cx, Report, Result, State, batch_size, io, wants_audio};

struct Block {
    ms: i64,
    track: u64,
    key: bool,
    data: Vec<u8>,
}

fn cluster(blocks: &mut [Block]) -> Vec<u8> {
    blocks.sort_by_key(|b| (b.ms, b.track));
    let base = blocks.first().map_or(0, |b| b.ms);
    let mut body = vec![];
    el_uint(&mut body, 0xE7, base.max(0) as u64);
    for b in blocks.iter() {
        let rel = (b.ms - base).clamp(i16::MIN as i64, i16::MAX as i64) as i16;
        let mut p = vint(b.track);
        p.extend_from_slice(&rel.to_be_bytes());
        p.push(if b.key { 0x80 } else { 0x00 });
        p.extend_from_slice(&b.data);
        el(&mut body, 0xA3, &p);
    }
    let mut out = vec![];
    el(&mut out, CLUSTER, &body);
    out
}

pub(crate) async fn webm_av1(job: &Cx<'_>, comp: &Comp, w: u32, h: u32, st: &mut State<'_>) -> Result<Report> {
    let rate = job.settings.rate(comp);
    let mut venc = crate::hevc_av1::Av1::new(w, h, rate, job.output)?;
    let with_audio = wants_audio(job);
    let opus_channels = if job.output.audio_channels == 1 { 1usize } else { 2 };
    let mut opus = with_audio.then(|| opus_encoder(job));
    let total = st.total;
    let frame_ms = |k: u64| -> i64 { ((k as i128 * 1000 * rate.den as i128 + rate.num as i128 / 2) / rate.num as i128) as i64 };
    let duration_ms = frame_ms(total) as f64;

    let path = job.place(job.path, 1);
    let mut file = job.create(&path)?;
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
    el_str(&mut v, 0x86, "V_AV1");
    el(&mut v, 0x63A2, &venc.av1c());
    el_uint(&mut v, 0x23E383, (1_000_000_000u128 * rate.den as u128 / rate.num.max(1) as u128) as u64);
    let mut video = vec![];
    el_uint(&mut video, 0xB0, w as u64);
    el_uint(&mut video, 0xBA, h as u64);
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

    let (span_start, span_end) = job.settings.span(comp);
    let mut cursor = span_start.to_units_floor(OPUS_RATE as i64);
    let mut pcm_left: Vec<f32> = vec![];
    let mut packets = 0u64;
    let mut audio_until = |until: Tick, blocks: &mut Vec<Block>, finish: bool| -> Result<()> {
        let Some(enc) = opus.as_mut() else { return Ok(()) };
        let end = until.to_units_floor(OPUS_RATE as i64);
        if end > cursor {
            let n = (end - cursor) as usize;
            let start = Tick(((cursor as i128 * TICKS_PER_SECOND as i128) / OPUS_RATE as i128) as i64);
            pcm_left.extend(mix(job, start, n, OPUS_RATE)?);
            cursor = end;
        }
        let fs = effectcraft_opusenc::OpusEncoder::FRAME_SIZE * opus_channels;
        if finish && !pcm_left.is_empty() {
            let pad = (fs - pcm_left.len() % fs) % fs + fs;
            pcm_left.extend(std::iter::repeat_n(0.0, pad));
        }
        while pcm_left.len() >= fs {
            let pkt = enc.encode_float(&pcm_left[..fs]);
            pcm_left.drain(..fs);
            blocks.push(Block { ms: packets as i64 * 20, track: 2, key: true, data: pkt });
            packets += 1;
        }
        Ok(())
    };

    let batch = batch_size();
    let mut i = 0;
    let flush_blocks = |blocks: &mut Vec<Block>, file: &mut crate::out::Out, seg_len: &mut u64| -> Result<()> {
        if !blocks.is_empty() {
            let c = cluster(blocks);
            file.write_all(&c).map_err(io)?;
            *seg_len += c.len() as u64;
            blocks.clear();
        }
        Ok(())
    };
    let mut blocks = vec![];
    while i < total {
        let end = (i + batch).min(total);
        // RGB, or the alpha matte alone (Channels: Alpha); AV1 in WebM carries no alpha track.
        let channels = if job.output.channels == Channels::Alpha { Channels::Alpha } else { Channels::Rgb };
        let frames: Vec<Vec<u8>> = job.frames(comp, (i..end).collect(), |_, img| job.pixels(&img, comp, channels, w, h)).await;
        for (j, px) in frames.iter().enumerate() {
            let k = i + j as u64;
            for p in venc.encode(px, k)? {
                blocks.push(Block { ms: frame_ms(k), track: 1, key: p.key, data: p.data });
            }
        }
        let t_end = if end >= total { span_end } else { job.settings.frame_time(comp, end) };
        audio_until(t_end, &mut blocks, end >= total)?;
        flush_blocks(&mut blocks, &mut file, &mut seg_len)?;
        st.advance(end - i)?;
        i = end;
    }
    if total == 0 {
        audio_until(span_end, &mut blocks, true)?;
        flush_blocks(&mut blocks, &mut file, &mut seg_len)?;
    }
    let mut size = [0u8; 8];
    size[0] = 0x01;
    size[1..].copy_from_slice(&seg_len.to_be_bytes()[1..]);
    file.seek(SeekFrom::Start(size_pos)).map_err(io)?;
    file.write_all(&size).map_err(io)?;
    file.seek(SeekFrom::End(0)).map_err(io)?;
    let bytes = file.finish()?;
    Ok(Report { path, frames: 0, width: w, height: h, seconds: 0.0, bytes, audio: with_audio, log: None, overflow: vec![] })
}
