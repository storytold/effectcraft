//! Software MP4/MOV H.264 route backed by `craft-media`, `craft-video`, `craft-audio`, and
//! `craft-media-source`.
//!
//! Preserves sync-sample backward seek, B-frame reordering, cooperative cancellation, VUI +
//! container colour resolution, draft mode for reduced-resolution playback, and bounded worker
//! threads.

use std::sync::{Arc, Mutex};

use craft_audio::{AudioBuffer, TrackDecoder as AudioTrackDecoder};
use craft_color::{ColorInfo, Matrix, Primaries, Range, Transfer};
use craft_frame::{Chroma, PixelData, VideoFrame};
use craft_media::isobmff::{self, Mp4File, Track, TrackKind};
use craft_media::{ByteSource, CodecId};
use craft_media_source::{
    AudioStreamInfo, FrameRequest, MAX_AUDIO_STREAMS, MediaError, MediaInfo, MediaKind, MediaSource, Result, SharedBytes, SharedSource, VideoStreamInfo,
};
use craft_time::{FrameRate, Tick};
use craft_video::frame::{ColorFamily, ColorTags, Plane, Samples, Subsampling, VideoFrame as DecodedVideoFrame};
use craft_video::{TrackDecoder, TrackDecoderOptions};

/// Maximum worker threads per `TrackDecoder` instance.
const MAX_WORKERS: usize = 8;

/// Maximum decoded frames retained in the per-source GOP window cache.
const MAX_CACHED_FRAMES: usize = 32;

/// Maximum audio lookahead packets inspected when initializing `craft_audio::TrackDecoder`.
const AUDIO_LOOKAHEAD: usize = 8;

fn worker_cap() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, MAX_WORKERS)
}

/// True when `head` begins with a plausible ISO-BMFF top-level box.
fn looks_like_isobmff(head: &[u8]) -> bool {
    if head.len() < 8 {
        return false;
    }
    matches!(&head[4..8], b"ftyp" | b"moov" | b"mdat" | b"free" | b"skip" | b"wide" | b"pnot" | b"uuid")
}

/// Open an in-place MP4/MOV H.264 source via `craft-media` / `craft-video`.
/// Returns `None` when the file is not ISO-BMFF, has no H.264 video track, or has an audio track
/// whose codec is not yet handled by `craft-audio`.
pub(crate) fn open_reader(name: &str, head: &[u8], reader: &SharedBytes) -> Option<Result<SharedSource>> {
    if !looks_like_isobmff(head) {
        return None;
    }
    open_byte_source(name, reader.clone())
}

/// Open an in-memory MP4/MOV H.264 source via `craft-media` / `craft-video`.
pub(crate) fn open_bytes(name: &str, bytes: Arc<[u8]>) -> Option<Result<SharedSource>> {
    if !looks_like_isobmff(&bytes) {
        return None;
    }
    open_byte_source(name, Arc::new(bytes))
}

fn open_byte_source(name: &str, source: SharedBytes) -> Option<Result<SharedSource>> {
    let file = isobmff::open(source.clone()).ok()?;
    let video_idx = file.tracks.iter().position(|t| t.kind == TrackKind::Video && !t.samples.is_empty() && !t.entries.is_empty() && t.timescale > 0)?;
    let video_track = &file.tracks[video_idx];
    let video_params = video_track.codec_params();
    if video_params.codec != CodecId::H264 {
        return None;
    }

    // Collect playable audio tracks and ensure `craft-audio` can decode every one of them.
    let mut audio_tracks = Vec::new();
    let mut audio_infos = Vec::new();
    for (idx, track) in file.tracks.iter().enumerate() {
        if audio_tracks.len() >= MAX_AUDIO_STREAMS {
            break;
        }
        if track.kind != TrackKind::Audio || track.samples.is_empty() || track.entries.is_empty() || track.timescale == 0 {
            continue;
        }
        let params = track.codec_params();
        let mut lookahead = Vec::with_capacity(AUDIO_LOOKAHEAD.min(track.samples.len()));
        for s_idx in 0..AUDIO_LOOKAHEAD.min(track.samples.len()) {
            let Ok(pkt) = file.read_sample(&*source, idx, s_idx) else {
                return None;
            };
            lookahead.push(pkt);
        }
        let refs: Vec<&[u8]> = lookahead.iter().map(Vec::as_slice).collect();
        let Ok(dec) = AudioTrackDecoder::new(&params, &refs) else {
            return None;
        };
        let native_rate = dec.sample_rate().max(1);
        let channels = u32::from(dec.layout().channels()).max(1);
        audio_infos.push(AudioStreamInfo {
            sample_rate: native_rate,
            channels,
            codec: params.codec.name().to_string(),
            bits_per_sample: params.pcm.map(|p| u32::from(p.bits)),
        });
        audio_tracks.push(AudioTrackEntry {
            track_idx: idx,
            native_rate,
            state: Mutex::new(AudioTrackState { decoder: dec, next_sample: 0, decoded_pos: 0, buf: Vec::new() }),
        });
    }

    let video_decoder = match TrackDecoder::with_options(&video_params, TrackDecoderOptions { threads: Some(worker_cap()), draft: false }) {
        Ok(d) => d,
        Err(e) => return Some(Err(MediaError::Decode(e.to_string()))),
    };

    let container_color = video_track.video().and_then(|v| v.color.as_ref()).map(color_from_isobmff);
    let color = container_color.unwrap_or(ColorInfo::REC709);
    let (width, height) = video_track
        .video()
        .map(|v| (u32::from(v.width), u32::from(v.height)))
        .filter(|&(w, h)| w > 0 && h > 0)
        .unwrap_or((video_track.width, video_track.height));
    let par = video_track.video().and_then(|v| v.pixel_aspect).filter(|&(n, d)| n > 0 && d > 0).unwrap_or((1, 1));
    let frame_rate = estimate_frame_rate(video_track);
    let duration = Tick::from_units(i64::try_from(video_track.duration).unwrap_or(i64::MAX), i64::from(video_track.timescale));
    let bitrate = video_track.entries.first().and_then(|e| e.bitrate).map(|b| u64::from(b.avg_bitrate)).filter(|&b| b > 0);

    let info = MediaInfo {
        name: name.to_string(),
        kind: MediaKind::Movie,
        duration,
        video: Some(VideoStreamInfo {
            width,
            height,
            frame_rate,
            par,
            codec: "H.264".to_string(),
            pixel_format: "yuv420p".to_string(),
            color,
            has_alpha: false,
            bitrate,
            hdr: None,
        }),
        audio_streams: audio_infos,
        container: "mp4".to_string(),
        start_timecode: None,
        file_size: Some(source.len()),
    };

    Some(Ok(Arc::new(Mp4H264Source {
        info,
        source,
        file,
        video_track_idx: video_idx,
        container_color,
        par,
        video: Mutex::new(VideoState { decoder: video_decoder, next_decode_idx: 0, gop_start_idx: 0, flushed: false, decoded: Vec::new() }),
        audio_tracks,
    })))
}

struct CachedFrame {
    pts: i64,
    frame: Arc<VideoFrame>,
}

struct VideoState {
    decoder: TrackDecoder,
    next_decode_idx: usize,
    gop_start_idx: usize,
    flushed: bool,
    decoded: Vec<CachedFrame>,
}

struct AudioTrackState {
    decoder: AudioTrackDecoder,
    next_sample: usize,
    /// Sample index (in native-rate frames) of `buf[0]` relative to track start (0).
    decoded_pos: i64,
    /// Interleaved or planar? Planar: one `Vec<f32>` per channel.
    buf: Vec<Vec<f32>>,
}

struct AudioTrackEntry {
    track_idx: usize,
    native_rate: u32,
    state: Mutex<AudioTrackState>,
}

struct Mp4H264Source {
    info: MediaInfo,
    source: SharedBytes,
    file: Mp4File,
    video_track_idx: usize,
    container_color: Option<ColorInfo>,
    par: (u32, u32),
    video: Mutex<VideoState>,
    audio_tracks: Vec<AudioTrackEntry>,
}

impl MediaSource for Mp4H264Source {
    fn info(&self) -> &MediaInfo {
        &self.info
    }

    fn video_frame(&self, req: FrameRequest) -> Result<Arc<VideoFrame>> {
        let track = &self.file.tracks[self.video_track_idx];
        let t_track = tick_to_units(req.time, track.timescale);
        let target_idx = track.sample_at_presentation_time(t_track).ok_or_else(|| MediaError::Decode("no video samples".into()))?;
        let target_pts = track.samples[target_idx].pts;

        let mut state = self.video.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(hit) = state.decoded.iter().find(|c| c.pts == target_pts) {
            return Ok(hit.frame.clone());
        }

        let sync_idx = track.sync_sample_before(target_idx);
        // Backward seek or jump to a later GOP: reset the decoder to the target sync sample.
        if target_idx < state.next_decode_idx || sync_idx > state.gop_start_idx {
            state.decoder.reset().map_err(|e| MediaError::Decode(e.to_string()))?;
            state.decoded.clear();
            state.next_decode_idx = sync_idx;
            state.gop_start_idx = sync_idx;
            state.flushed = false;
        }

        let want_draft = req.scale < 0.99;
        if state.decoder.draft() != want_draft {
            state.decoder.set_draft(want_draft);
        }

        while !state.decoded.iter().any(|c| c.pts >= target_pts) {
            if filmcraft_media::cancel::cancelled() {
                return Err(MediaError::Cancelled);
            }
            if state.next_decode_idx < track.samples.len() {
                let idx = state.next_decode_idx;
                let sample = track.samples[idx];
                let pkt = self.file.read_sample(&*self.source, self.video_track_idx, idx).map_err(|e| MediaError::Io(e.to_string()))?;
                let frames = state.decoder.decode(&pkt, sample.pts).map_err(|e| MediaError::Decode(e.to_string()))?;
                state.next_decode_idx = idx + 1;
                push_decoded(&mut state.decoded, frames, self.container_color, self.par, track.timescale, track.edit_offset)?;
            } else if !state.flushed {
                let frames = state.decoder.flush().map_err(|e| MediaError::Decode(e.to_string()))?;
                state.flushed = true;
                push_decoded(&mut state.decoded, frames, self.container_color, self.par, track.timescale, track.edit_offset)?;
                break;
            } else {
                break;
            }
        }

        if let Some(exact) = state.decoded.iter().find(|c| c.pts == target_pts) {
            return Ok(exact.frame.clone());
        }
        // Fallback: latest decoded frame at or before target_pts, or earliest available.
        let best = state
            .decoded
            .iter()
            .filter(|c| c.pts <= target_pts)
            .max_by_key(|c| c.pts)
            .or_else(|| state.decoded.first())
            .ok_or_else(|| MediaError::Decode("H.264 decoder produced no frame".into()))?;
        Ok(best.frame.clone())
    }

    fn audio(&self, start: i64, frames: usize, sample_rate: u32) -> Result<AudioBuffer> {
        self.audio_stream(0, start, frames, sample_rate)
    }

    fn audio_stream(&self, stream: usize, start: i64, frames: usize, sample_rate: u32) -> Result<AudioBuffer> {
        let entry = self.audio_tracks.get(stream).ok_or(MediaError::NoStream("audio"))?;
        if sample_rate == 0 {
            return Err(MediaError::Decode("invalid audio sample rate 0".into()));
        }
        let channels_count = self.info.audio_streams.get(stream).map_or(2, |s| s.channels as usize);
        if frames == 0 {
            return Ok(AudioBuffer::new(sample_rate, channels_count as u16, 0));
        }

        let native_rate = entry.native_rate;
        let (native_start, native_frames) = if sample_rate == native_rate {
            (start, frames)
        } else {
            let s0 = (i128::from(start) * i128::from(native_rate)) / i128::from(sample_rate);
            let len = ((frames as u128 * u128::from(native_rate)).div_ceil(u128::from(sample_rate)) + 2) as usize;
            (i64::try_from(s0).unwrap_or(0), len)
        };

        let track = &self.file.tracks[entry.track_idx];
        let mut state = entry.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let decoded =
            decode_audio_range(&self.file, &*self.source, entry.track_idx, track, native_rate, channels_count, &mut state, native_start, native_frames)?;

        if sample_rate == native_rate {
            return Ok(AudioBuffer { sample_rate, channels: decoded });
        }

        let mut resampled = vec![vec![0.0f32; frames]; channels_count];
        let ratio = f64::from(native_rate) / f64::from(sample_rate);
        for (c, out_ch) in resampled.iter_mut().enumerate() {
            let in_ch = &decoded[c];
            if in_ch.is_empty() {
                continue;
            }
            for (i, dst) in out_ch.iter_mut().enumerate() {
                let pos = i as f64 * ratio;
                let idx = pos.floor() as usize;
                let frac = (pos - idx as f64) as f32;
                let s0 = in_ch.get(idx).copied().unwrap_or(0.0);
                let s1 = in_ch.get(idx + 1).copied().unwrap_or(s0);
                *dst = s0 + (s1 - s0) * frac;
            }
        }
        Ok(AudioBuffer { sample_rate, channels: resampled })
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_audio_range(
    file: &Mp4File,
    source: &dyn ByteSource,
    track_idx: usize,
    track: &Track,
    native_rate: u32,
    channels_count: usize,
    state: &mut AudioTrackState,
    start: i64,
    frames: usize,
) -> Result<Vec<Vec<f32>>> {
    let mut out = vec![vec![0.0f32; frames]; channels_count];
    let end = start.saturating_add(frames as i64);
    if end <= 0 {
        return Ok(out);
    }

    let buf_len = state.buf.first().map_or(0, Vec::len) as i64;
    let buf_end = state.decoded_pos.saturating_add(buf_len);
    if start < state.decoded_pos || start > buf_end + i64::from(native_rate) * 2 {
        // Seek to the sample at or before `start` in track timescale.
        let t_track = (i128::from(start.max(0)) * i128::from(track.timescale)) / i128::from(native_rate);
        let sample_idx = track.sample_at_presentation_time(i64::try_from(t_track).unwrap_or(0)).unwrap_or(0);
        // Back up a few packets for AAC/Opus overlap/preroll.
        let seek_idx = sample_idx.saturating_sub(2);
        state.decoder.reset();
        state.next_sample = seek_idx;
        let sample_pts = track.presentation_pts(seek_idx).unwrap_or(0);
        state.decoded_pos = i64::try_from((i128::from(sample_pts) * i128::from(native_rate)) / i128::from(track.timescale.max(1))).unwrap_or(0);
        for ch in &mut state.buf {
            ch.clear();
        }
    }

    if state.buf.len() != channels_count {
        state.buf = vec![Vec::new(); channels_count];
    }

    while state.decoded_pos + state.buf.first().map_or(0, Vec::len) as i64 <= end && state.next_sample < track.samples.len() {
        if filmcraft_media::cancel::cancelled() {
            return Err(MediaError::Cancelled);
        }
        let idx = state.next_sample;
        let pkt = file.read_sample(source, track_idx, idx).map_err(|e| MediaError::Io(e.to_string()))?;
        state.next_sample = idx + 1;
        let block = state.decoder.decode(&pkt).map_err(|e| MediaError::Decode(e.to_string()))?;
        for (c, ch) in state.buf.iter_mut().enumerate() {
            if let Some(src_ch) = block.channels.get(c) {
                ch.extend_from_slice(src_ch);
            }
        }
    }

    let avail = state.buf.first().map_or(0, Vec::len);
    for i in 0..frames {
        let pos = start.saturating_add(i as i64);
        if pos >= state.decoded_pos {
            let off = (pos - state.decoded_pos) as usize;
            if off < avail {
                for (c, out_ch) in out.iter_mut().enumerate() {
                    out_ch[i] = state.buf[c][off];
                }
            }
        }
    }

    // Trim already-consumed audio older than `end` minus a small safety margin.
    let drop_to = (end - state.decoded_pos).max(0) as usize;
    let trim = drop_to.min(avail);
    if trim > 0 {
        for ch in &mut state.buf {
            ch.drain(..trim);
        }
        state.decoded_pos = state.decoded_pos.saturating_add(trim as i64);
    }

    Ok(out)
}

fn push_decoded(
    cache: &mut Vec<CachedFrame>,
    frames: Vec<DecodedVideoFrame>,
    container_color: Option<ColorInfo>,
    par: (u32, u32),
    timescale: u32,
    edit_offset: i64,
) -> Result<()> {
    for vf in frames {
        let pts = vf.pts.unwrap_or(0);
        let frame = Arc::new(to_craft_frame(vf, container_color, par, timescale, edit_offset)?);
        cache.push(CachedFrame { pts, frame });
    }
    if cache.len() > MAX_CACHED_FRAMES {
        let excess = cache.len() - MAX_CACHED_FRAMES;
        cache.drain(..excess);
    }
    Ok(())
}

fn to_craft_frame(vf: DecodedVideoFrame, container_color: Option<ColorInfo>, default_par: (u32, u32), timescale: u32, edit_offset: i64) -> Result<VideoFrame> {
    let width = vf.width();
    let height = vf.height();
    let subsampling = vf.subsampling();
    let family = vf.family();
    let tags = vf.color;
    let pts_units = vf.pts.unwrap_or(0).saturating_add(edit_offset);
    let pts = Tick::from_units(pts_units, i64::from(timescale.max(1)));
    let par = vf.sar.filter(|&(n, d)| n > 0 && d > 0).unwrap_or(default_par);
    let color = resolve_color(tags, container_color);
    let bit_depth = vf.bit_depth();
    let (planes, alpha) = vf.into_planes();

    let data = match family {
        ColorFamily::YCbCr => {
            let chroma = match subsampling {
                Subsampling::S420 => Chroma::C420,
                Subsampling::S422 => Chroma::C422,
                Subsampling::S444 => Chroma::C444,
                _ => return Err(MediaError::Unsupported("unsupported YCbCr subsampling".into())),
            };
            let mut iter = planes.into_iter();
            let (Some(y), Some(cb), Some(cr)) = (iter.next(), iter.next(), iter.next()) else {
                return Err(MediaError::Decode("missing YUV planes".into()));
            };
            if bit_depth <= 8 {
                let y = Arc::new(plane_u8(y)?);
                let cb = Arc::new(plane_u8(cb)?);
                let cr = Arc::new(plane_u8(cr)?);
                let a = alpha.map(plane_u8).transpose()?.map(Arc::new);
                PixelData::Yuv8 { planes: [y, cb, cr], chroma, alpha: a }
            } else {
                let y = Arc::new(plane_u16(y)?);
                let cb = Arc::new(plane_u16(cb)?);
                let cr = Arc::new(plane_u16(cr)?);
                let a = alpha.map(plane_u16).transpose()?.map(Arc::new);
                PixelData::Yuv16 { planes: [y, cb, cr], chroma, bits: u32::from(bit_depth), alpha: a }
            }
        }
        ColorFamily::Rgb | ColorFamily::Gray => return Err(MediaError::Unsupported("non-YCbCr H.264 pixel format".into())),
    };

    Ok(VideoFrame { width, height, data, color, par, pts })
}

fn plane_u8(p: Plane) -> Result<Vec<u8>> {
    let (w, h, stride) = (p.width() as usize, p.height() as usize, p.stride());
    match p.into_samples() {
        Samples::U8(mut v) => {
            if stride == w {
                v.truncate(w * h);
                return Ok(v);
            }
            let mut out = Vec::with_capacity(w * h);
            for y in 0..h {
                let row = y * stride;
                out.extend_from_slice(&v[row..row + w]);
            }
            Ok(out)
        }
        Samples::U16(_) => Err(MediaError::Decode("expected 8-bit plane".into())),
    }
}

fn plane_u16(p: Plane) -> Result<Vec<u16>> {
    let (w, h, stride) = (p.width() as usize, p.height() as usize, p.stride());
    match p.into_samples() {
        Samples::U16(mut v) => {
            if stride == w {
                v.truncate(w * h);
                return Ok(v);
            }
            let mut out = Vec::with_capacity(w * h);
            for y in 0..h {
                let row = y * stride;
                out.extend_from_slice(&v[row..row + w]);
            }
            Ok(out)
        }
        Samples::U8(_) => Err(MediaError::Decode("expected 16-bit plane".into())),
    }
}

fn resolve_color(vui: ColorTags, container: Option<ColorInfo>) -> ColorInfo {
    let base = container.unwrap_or(ColorInfo::REC709);
    let matrix = Matrix::from_code(vui.matrix).unwrap_or(base.matrix);
    let transfer = Transfer::from_code(vui.transfer).unwrap_or(base.transfer);
    let primaries = primaries_from_code(vui.primaries).unwrap_or(base.primaries);
    let range = if vui.full_range { Range::Full } else { base.range };
    ColorInfo { matrix, transfer, primaries, range }
}

fn color_from_isobmff(c: &isobmff::ColorInfo) -> ColorInfo {
    match c {
        isobmff::ColorInfo::Nclx { primaries, transfer, matrix, full_range } => ColorInfo {
            matrix: u8::try_from(*matrix).ok().and_then(Matrix::from_code).unwrap_or(Matrix::Bt709),
            transfer: u8::try_from(*transfer).ok().and_then(Transfer::from_code).unwrap_or(Transfer::Bt709),
            primaries: u8::try_from(*primaries).ok().and_then(primaries_from_code).unwrap_or(Primaries::Bt709),
            range: if *full_range { Range::Full } else { Range::Limited },
        },
        isobmff::ColorInfo::Nclc { primaries, transfer, matrix } => ColorInfo {
            matrix: u8::try_from(*matrix).ok().and_then(Matrix::from_code).unwrap_or(Matrix::Bt709),
            transfer: u8::try_from(*transfer).ok().and_then(Transfer::from_code).unwrap_or(Transfer::Bt709),
            primaries: u8::try_from(*primaries).ok().and_then(primaries_from_code).unwrap_or(Primaries::Bt709),
            range: Range::Limited,
        },
        isobmff::ColorInfo::Icc { .. } => ColorInfo::REC709,
    }
}

fn primaries_from_code(c: u8) -> Option<Primaries> {
    match c {
        1 => Some(Primaries::Bt709),
        5 => Some(Primaries::Bt601_625),
        6 => Some(Primaries::Bt601_525),
        9 => Some(Primaries::Bt2020),
        11 | 12 => Some(Primaries::P3D65),
        _ => None,
    }
}

fn tick_to_units(t: Tick, timescale: u32) -> i64 {
    let v = (i128::from(t.0) * i128::from(timescale)) / i128::from(craft_time::TICKS_PER_SECOND);
    i64::try_from(v).unwrap_or(if v < 0 { i64::MIN } else { i64::MAX })
}

fn estimate_frame_rate(track: &Track) -> FrameRate {
    let ts = i128::from(track.timescale);
    if ts <= 0 || track.samples.is_empty() {
        return FrameRate::FPS_30;
    }
    let mut durs: Vec<u32> = track.samples.iter().map(|s| s.duration).filter(|&d| d > 0).collect();
    if durs.is_empty() {
        return FrameRate::FPS_30;
    }
    durs.sort_unstable();
    let median = durs[durs.len() / 2];
    for r in FrameRate::COMMON {
        if ts * i128::from(r.den) == i128::from(median) * i128::from(r.num) {
            return r;
        }
    }
    let fps = track.timescale as f64 / f64::from(median);
    FrameRate::COMMON
        .into_iter()
        .map(|r| (r, (fps / r.as_f64() - 1.0).abs()))
        .filter(|&(_, off)| off <= 0.005)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(r, _)| r)
        .unwrap_or_else(|| FrameRate::new(track.timescale, median))
}
