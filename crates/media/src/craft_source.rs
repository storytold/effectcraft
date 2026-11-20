//! ISO-BMFF (MP4/MOV) and Matroska/WebM media-reader routes backed by `craft-media`,
//! `craft-video`, `craft-audio`, and `craft-media-source`.
//!
//! Preserves sync-sample backward seek, B-frame reordering, cooperative cancellation, VUI +
//! container colour resolution, draft mode for reduced-resolution playback, and bounded worker
//! threads, while falling back to `filmcraft-codecs` for unsupported codecs or audio streams.

use std::sync::{Arc, Mutex};

use craft_audio::{AudioBuffer, TrackDecoder as AudioTrackDecoder};
use craft_color::{ColorInfo, Matrix, Primaries, Range, Transfer};
use craft_frame::{Chroma, PixelData, VideoFrame};
use craft_media::isobmff::{self, Mp4File};
use craft_media::matroska::{self, MkvFile};
use craft_media::{ByteSource, CodecId, CodecParams};
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

fn supports_video_codec(codec: CodecId) -> bool {
    matches!(codec, CodecId::H264 | CodecId::Hevc | CodecId::Av1 | CodecId::Vp9 | CodecId::Vp8 | CodecId::ProRes | CodecId::Dnxhd | CodecId::Mpeg2Video)
}

/// True when `head` begins with a plausible ISO-BMFF top-level box.
fn looks_like_isobmff(head: &[u8]) -> bool {
    if head.len() < 8 {
        return false;
    }
    matches!(&head[4..8], b"ftyp" | b"moov" | b"mdat" | b"free" | b"skip" | b"wide" | b"pnot" | b"uuid")
}

/// True when `head` begins with an EBML header (Matroska / WebM).
fn looks_like_matroska(head: &[u8]) -> bool {
    head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3])
}

/// Open an in-place ISO-BMFF or Matroska/WebM source via `craft-media` / `craft-video`.
/// Returns `None` when the container/codec/audio combination should fall back to `filmcraft-codecs`.
#[cfg(any(unix, windows))]
pub(crate) fn open_reader(name: &str, head: &[u8], reader: &SharedBytes) -> Option<Result<SharedSource>> {
    if looks_like_isobmff(head) {
        return open_isobmff(name, reader.clone());
    }
    if looks_like_matroska(head) {
        return open_matroska(name, reader.clone());
    }
    None
}

/// Open an in-memory ISO-BMFF or Matroska/WebM source via `craft-media` / `craft-video`.
pub(crate) fn open_bytes(name: &str, bytes: Arc<[u8]>) -> Option<Result<SharedSource>> {
    if looks_like_isobmff(&bytes) {
        return open_isobmff(name, Arc::new(bytes));
    }
    if looks_like_matroska(&bytes) {
        return open_matroska(name, Arc::new(bytes));
    }
    None
}

enum ContainerFile {
    Isobmff(Mp4File),
    Matroska(Box<MkvFile>),
}

impl ContainerFile {
    fn sample_count(&self, track_idx: usize) -> usize {
        match self {
            ContainerFile::Isobmff(f) => f.tracks.get(track_idx).map_or(0, |t| t.samples.len()),
            ContainerFile::Matroska(f) => f.tracks.get(track_idx).map_or(0, |t| t.samples.len()),
        }
    }

    fn read_sample(&self, src: &dyn ByteSource, track_idx: usize, sample_idx: usize) -> Result<Vec<u8>> {
        match self {
            ContainerFile::Isobmff(f) => f.read_sample(src, track_idx, sample_idx).map_err(|e| MediaError::Io(e.to_string())),
            ContainerFile::Matroska(f) => f.read_sample(src, track_idx, sample_idx).map_err(|e| MediaError::Io(e.to_string())),
        }
    }

    fn target_sample(&self, track_idx: usize, time: Tick) -> Option<(usize, i64)> {
        match self {
            ContainerFile::Isobmff(f) => {
                let track = f.tracks.get(track_idx)?;
                let t_track = tick_to_units(time, track.timescale);
                let idx = track.sample_at_presentation_time(t_track)?;
                Some((idx, track.samples[idx].pts))
            }
            ContainerFile::Matroska(f) => {
                let track = f.tracks.get(track_idx)?;
                let t_track = tick_to_matroska_units(time, track.timebase);
                let idx = track.sample_at_pts(t_track)?;
                Some((idx, track.samples[idx].pts))
            }
        }
    }

    fn sync_sample_before(&self, track_idx: usize, target_idx: usize) -> usize {
        match self {
            ContainerFile::Isobmff(f) => f.tracks[track_idx].sync_sample_before(target_idx),
            ContainerFile::Matroska(f) => f.tracks[track_idx].sync_sample_before(target_idx),
        }
    }

    fn sample_pts(&self, track_idx: usize, sample_idx: usize) -> i64 {
        match self {
            ContainerFile::Isobmff(f) => f.tracks[track_idx].samples[sample_idx].pts,
            ContainerFile::Matroska(f) => f.tracks[track_idx].samples[sample_idx].pts,
        }
    }

    fn pts_to_tick(&self, track_idx: usize, raw_pts: i64) -> Tick {
        match self {
            ContainerFile::Isobmff(f) => {
                let track = &f.tracks[track_idx];
                let pts_units = raw_pts.saturating_add(track.edit_offset);
                Tick::from_units(pts_units, i64::from(track.timescale.max(1)))
            }
            ContainerFile::Matroska(f) => {
                let track = &f.tracks[track_idx];
                let ns = track.ticks_to_ns(raw_pts);
                Tick::from_units(ns, 1_000_000_000)
            }
        }
    }

    fn audio_seek_info(&self, track_idx: usize, start: i64, native_rate: u32) -> (usize, i64) {
        match self {
            ContainerFile::Isobmff(f) => {
                let track = &f.tracks[track_idx];
                let t_track = (i128::from(start.max(0)) * i128::from(track.timescale)) / i128::from(native_rate);
                let sample_idx = track.sample_at_presentation_time(i64::try_from(t_track).unwrap_or(0)).unwrap_or(0);
                // Back up a few packets for AAC/Opus overlap/preroll.
                let seek_idx = sample_idx.saturating_sub(2);
                let sample_pts = track.presentation_pts(seek_idx).unwrap_or(0);
                let decoded_pos = i64::try_from((i128::from(sample_pts) * i128::from(native_rate)) / i128::from(track.timescale.max(1))).unwrap_or(0);
                (seek_idx, decoded_pos)
            }
            ContainerFile::Matroska(f) => {
                let track = &f.tracks[track_idx];
                let delay_ns = i128::from(track.codec_delay_ns);
                let preroll_ns = i128::from(track.seek_pre_roll_ns.max(80_000_000));
                let target_ns = (i128::from(start.max(0)) * 1_000_000_000) / i128::from(native_rate) + delay_ns;
                let seek_ns = (target_ns - preroll_ns).max(0);
                let t_track = track.ns_to_ticks(i64::try_from(seek_ns).unwrap_or(0));
                let sample_idx = track.sample_at_pts(t_track).unwrap_or(0);
                let seek_idx = sample_idx.saturating_sub(2);
                let sample_pts = track.samples.get(seek_idx).map_or(0, |s| s.pts);
                let sample_ns = i128::from(track.ticks_to_ns(sample_pts)) - delay_ns;
                let decoded_pos = i64::try_from((sample_ns * i128::from(native_rate)) / 1_000_000_000).unwrap_or(0);
                (seek_idx, decoded_pos)
            }
        }
    }
}

fn open_audio_tracks<I>(container: &ContainerFile, source: &dyn ByteSource, candidates: I) -> Option<(Vec<AudioTrackEntry>, Vec<AudioStreamInfo>)>
where
    I: IntoIterator<Item = (usize, usize, CodecParams, u64)>,
{
    let mut audio_tracks = Vec::new();
    let mut audio_infos = Vec::new();
    for (idx, sample_count, params, codec_delay_ns) in candidates {
        if audio_tracks.len() >= MAX_AUDIO_STREAMS {
            break;
        }
        let n = AUDIO_LOOKAHEAD.min(sample_count);
        let mut lookahead = Vec::with_capacity(n);
        for s_idx in 0..n {
            let pkt = container.read_sample(source, idx, s_idx).ok()?;
            lookahead.push(pkt);
        }
        let refs: Vec<&[u8]> = lookahead.iter().map(Vec::as_slice).collect();
        let dec = AudioTrackDecoder::new(&params, &refs).ok()?;
        let native_rate = dec.sample_rate().max(1);
        let channels = u32::from(dec.layout().channels()).max(1);
        let delay_samples = i64::try_from((i128::from(codec_delay_ns) * i128::from(native_rate)) / 1_000_000_000).unwrap_or(0);
        audio_infos.push(AudioStreamInfo {
            sample_rate: native_rate,
            channels,
            codec: params.codec.name().to_string(),
            bits_per_sample: params.pcm.map(|p| u32::from(p.bits)),
        });
        audio_tracks.push(AudioTrackEntry {
            track_idx: idx,
            native_rate,
            state: Mutex::new(AudioTrackState { decoder: dec, next_sample: 0, decoded_pos: -delay_samples, buf: Vec::new() }),
        });
    }
    Some((audio_tracks, audio_infos))
}

fn open_isobmff(name: &str, source: SharedBytes) -> Option<Result<SharedSource>> {
    let file = isobmff::open(source.clone()).ok()?;
    let video_idx =
        file.tracks.iter().position(|t| t.kind == isobmff::TrackKind::Video && !t.samples.is_empty() && !t.entries.is_empty() && t.timescale > 0)?;
    let video_track = &file.tracks[video_idx];
    let video_params = video_track.codec_params();
    if !supports_video_codec(video_params.codec) {
        return None;
    }

    let container_color = video_track.video().and_then(|v| v.color.as_ref()).map(color_from_isobmff);
    let color = container_color.unwrap_or(ColorInfo::REC709);
    let (mut width, mut height) = video_track
        .video()
        .map(|v| (u32::from(v.width), u32::from(v.height)))
        .filter(|&(w, h)| w > 0 && h > 0)
        .unwrap_or((video_track.width, video_track.height));
    let mut par = video_track.video().and_then(|v| v.pixel_aspect).filter(|&(n, d)| n > 0 && d > 0).unwrap_or((1, 1));
    let rotation = video_track.display_rotation().unwrap_or(0) % 4;
    if rotation % 2 == 1 {
        std::mem::swap(&mut width, &mut height);
        par = (par.1, par.0);
    }
    let has_alpha = video_track.video().is_some_and(|v| v.depth == 32);
    let frame_rate = estimate_isobmff_frame_rate(video_track);
    let duration = Tick::from_units(i64::try_from(video_track.duration).unwrap_or(i64::MAX), i64::from(video_track.timescale));
    let bitrate = video_track.entries.first().and_then(|e| e.bitrate).map(|b| u64::from(b.avg_bitrate)).filter(|&b| b > 0);
    let codec_name = video_params.codec.name().to_string();

    let audio_candidates: Vec<(usize, usize, CodecParams, u64)> = file
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.kind == isobmff::TrackKind::Audio && !t.samples.is_empty() && !t.entries.is_empty() && t.timescale > 0)
        .map(|(idx, t)| (idx, t.samples.len(), t.codec_params(), 0))
        .collect();

    let container = ContainerFile::Isobmff(file);
    let (audio_tracks, audio_infos) = open_audio_tracks(&container, &*source, audio_candidates)?;

    let video_decoder = match TrackDecoder::with_options(&video_params, TrackDecoderOptions { threads: Some(worker_cap()), draft: false }) {
        Ok(d) => d,
        Err(e) => return Some(Err(MediaError::Decode(e.to_string()))),
    };

    let info = MediaInfo {
        name: name.to_string(),
        kind: MediaKind::Movie,
        duration,
        video: Some(VideoStreamInfo { width, height, frame_rate, par, codec: codec_name, pixel_format: String::new(), color, has_alpha, bitrate, hdr: None }),
        audio_streams: audio_infos,
        container: "mp4".to_string(),
        start_timecode: None,
        file_size: Some(source.len()),
    };

    Some(Ok(Arc::new(CraftMediaSource {
        info,
        source,
        file: container,
        video_track_idx: video_idx,
        container_color,
        par,
        rotation,
        video: Mutex::new(VideoState { decoder: video_decoder, next_decode_idx: 0, gop_start_idx: 0, flushed: false, decoded: Vec::new() }),
        audio_tracks,
    })))
}

fn open_matroska(name: &str, source: SharedBytes) -> Option<Result<SharedSource>> {
    let file = matroska::open(source.clone()).ok()?;
    let video_idx = file.tracks.iter().position(|t| t.kind == matroska::TrackKind::Video && !t.samples.is_empty() && t.timebase.0 > 0 && t.timebase.1 > 0)?;
    let video_track = &file.tracks[video_idx];
    if !video_track.frames_readable() {
        return None;
    }
    let video_params = video_track.codec_params();
    if !supports_video_codec(video_params.codec) {
        return None;
    }

    let video_info = video_track.video.as_ref();
    // WebM/Matroska alpha_mode == 1 stores alpha in BlockAdditional; fall back to FilmCraft.
    if video_info.is_some_and(|v| v.alpha_mode == 1) {
        return None;
    }
    let container_color = video_info.and_then(|v| v.color.as_ref()).map(color_from_matroska);
    let color = container_color.unwrap_or(ColorInfo::REC709);
    let (mut width, mut height) = video_info.map_or((0, 0), |v| (v.pixel_width, v.pixel_height));
    let mut par = matroska_par(video_info, width, height);
    let rotation = video_info.and_then(|v| v.display_rotation()).unwrap_or(0) % 4;
    if rotation % 2 == 1 {
        std::mem::swap(&mut width, &mut height);
        par = (par.1, par.0);
    }
    let frame_rate = estimate_matroska_frame_rate(video_track);
    let duration = matroska_duration(&file, video_track);
    let codec_name = video_params.codec.name().to_string();
    let container_name = if file.doc_type == "webm" { "webm" } else { "mkv" }.to_string();

    let audio_candidates: Vec<(usize, usize, CodecParams, u64)> = file
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.kind == matroska::TrackKind::Audio && !t.samples.is_empty() && t.timebase.0 > 0 && t.timebase.1 > 0 && t.frames_readable())
        .map(|(idx, t)| (idx, t.samples.len(), t.codec_params(), t.codec_delay_ns))
        .collect();

    let container = ContainerFile::Matroska(Box::new(file));
    let (audio_tracks, audio_infos) = open_audio_tracks(&container, &*source, audio_candidates)?;

    let video_decoder = match TrackDecoder::with_options(&video_params, TrackDecoderOptions { threads: Some(worker_cap()), draft: false }) {
        Ok(d) => d,
        Err(e) => return Some(Err(MediaError::Decode(e.to_string()))),
    };

    let info = MediaInfo {
        name: name.to_string(),
        kind: MediaKind::Movie,
        duration,
        video: Some(VideoStreamInfo {
            width,
            height,
            frame_rate,
            par,
            codec: codec_name,
            pixel_format: String::new(),
            color,
            has_alpha: false,
            bitrate: None,
            hdr: None,
        }),
        audio_streams: audio_infos,
        container: container_name,
        start_timecode: None,
        file_size: Some(source.len()),
    };

    Some(Ok(Arc::new(CraftMediaSource {
        info,
        source,
        file: container,
        video_track_idx: video_idx,
        container_color,
        par,
        rotation,
        video: Mutex::new(VideoState { decoder: video_decoder, next_decode_idx: 0, gop_start_idx: 0, flushed: false, decoded: Vec::new() }),
        audio_tracks,
    })))
}

struct CachedFrame {
    pts: i64,
    draft: bool,
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
    /// Planar: one `Vec<f32>` per channel.
    buf: Vec<Vec<f32>>,
}

struct AudioTrackEntry {
    track_idx: usize,
    native_rate: u32,
    state: Mutex<AudioTrackState>,
}

struct CraftMediaSource {
    info: MediaInfo,
    source: SharedBytes,
    file: ContainerFile,
    video_track_idx: usize,
    container_color: Option<ColorInfo>,
    par: (u32, u32),
    rotation: u8,
    video: Mutex<VideoState>,
    audio_tracks: Vec<AudioTrackEntry>,
}

impl MediaSource for CraftMediaSource {
    fn info(&self) -> &MediaInfo {
        &self.info
    }

    fn video_frame(&self, req: FrameRequest) -> Result<Arc<VideoFrame>> {
        let (target_idx, target_pts) = self.file.target_sample(self.video_track_idx, req.time).ok_or_else(|| MediaError::Decode("no video samples".into()))?;
        let want_draft = req.scale < 0.99;

        let mut state = self.video.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(hit) = state.decoded.iter().find(|c| c.pts == target_pts && c.draft == want_draft) {
            return Ok(hit.frame.clone());
        }

        let sync_idx = self.file.sync_sample_before(self.video_track_idx, target_idx);
        let draft_changed = state.decoder.draft() != want_draft;
        if draft_changed {
            state.decoder.set_draft(want_draft);
        }
        // Backward seek, jump to a later GOP, or draft/full-quality mode switch: reset decoder to
        // the target sync sample so full-quality frames never reuse draft-decoded references.
        if draft_changed || target_idx < state.next_decode_idx || sync_idx > state.gop_start_idx {
            state.decoder.reset().map_err(|e| MediaError::Decode(e.to_string()))?;
            state.decoded.clear();
            state.next_decode_idx = sync_idx;
            state.gop_start_idx = sync_idx;
            state.flushed = false;
        }

        let sample_count = self.file.sample_count(self.video_track_idx);
        while !state.decoded.iter().any(|c| c.pts >= target_pts && c.draft == want_draft) {
            if filmcraft_media::cancel::cancelled() {
                return Err(MediaError::Cancelled);
            }
            if state.next_decode_idx < sample_count {
                let idx = state.next_decode_idx;
                let pts = self.file.sample_pts(self.video_track_idx, idx);
                let pkt = self.file.read_sample(&*self.source, self.video_track_idx, idx)?;
                let frames = state.decoder.decode(&pkt, pts).map_err(|e| MediaError::Decode(e.to_string()))?;
                state.next_decode_idx = idx + 1;
                push_decoded(&mut state.decoded, frames, want_draft, self.container_color, self.par, self.rotation, &self.file, self.video_track_idx)?;
            } else if !state.flushed {
                let frames = state.decoder.flush().map_err(|e| MediaError::Decode(e.to_string()))?;
                state.flushed = true;
                push_decoded(&mut state.decoded, frames, want_draft, self.container_color, self.par, self.rotation, &self.file, self.video_track_idx)?;
                break;
            } else {
                break;
            }
        }

        if let Some(exact) = state.decoded.iter().find(|c| c.pts == target_pts && c.draft == want_draft) {
            return Ok(exact.frame.clone());
        }
        // Fallback: latest decoded frame at or before target_pts, or earliest available.
        let best = state
            .decoded
            .iter()
            .filter(|c| c.draft == want_draft && c.pts <= target_pts)
            .max_by_key(|c| c.pts)
            .or_else(|| state.decoded.iter().find(|c| c.draft == want_draft))
            .ok_or_else(|| MediaError::Decode("video decoder produced no frame".into()))?;
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

        let mut state = entry.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let decoded = decode_audio_range(&self.file, &*self.source, entry.track_idx, native_rate, channels_count, &mut state, native_start, native_frames)?;

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
    file: &ContainerFile,
    source: &dyn ByteSource,
    track_idx: usize,
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
        let (seek_idx, decoded_pos) = file.audio_seek_info(track_idx, start, native_rate);
        state.decoder.reset();
        state.next_sample = seek_idx;
        state.decoded_pos = decoded_pos;
        for ch in &mut state.buf {
            ch.clear();
        }
    }

    if state.buf.len() != channels_count {
        state.buf = vec![Vec::new(); channels_count];
    }

    let sample_count = file.sample_count(track_idx);
    while state.decoded_pos + state.buf.first().map_or(0, Vec::len) as i64 <= end && state.next_sample < sample_count {
        if filmcraft_media::cancel::cancelled() {
            return Err(MediaError::Cancelled);
        }
        let idx = state.next_sample;
        let pkt = file.read_sample(source, track_idx, idx)?;
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

#[allow(clippy::too_many_arguments)]
fn push_decoded(
    cache: &mut Vec<CachedFrame>,
    frames: Vec<DecodedVideoFrame>,
    draft: bool,
    container_color: Option<ColorInfo>,
    par: (u32, u32),
    rotation: u8,
    file: &ContainerFile,
    track_idx: usize,
) -> Result<()> {
    for vf in frames {
        let pts = vf.pts.unwrap_or(0);
        let pts_tick = file.pts_to_tick(track_idx, pts);
        let frame = Arc::new(to_craft_frame(vf, container_color, par, rotation, pts_tick)?);
        cache.push(CachedFrame { pts, draft, frame });
    }
    if cache.len() > MAX_CACHED_FRAMES {
        let excess = cache.len() - MAX_CACHED_FRAMES;
        cache.drain(..excess);
    }
    Ok(())
}

fn to_craft_frame(vf: DecodedVideoFrame, container_color: Option<ColorInfo>, default_par: (u32, u32), rotation: u8, pts: Tick) -> Result<VideoFrame> {
    let width = vf.width();
    let height = vf.height();
    let subsampling = vf.subsampling();
    let family = vf.family();
    let tags = vf.color;
    let raw_par = vf.sar.filter(|&(n, d)| n > 0 && d > 0).unwrap_or(default_par);
    let par = if rotation % 2 == 1 { (raw_par.1, raw_par.0) } else { raw_par };
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
        ColorFamily::Rgb | ColorFamily::Gray => return Err(MediaError::Unsupported("non-YCbCr pixel format".into())),
    };

    let frame = VideoFrame { width, height, data, color, par, pts };
    Ok(rotate_frame(frame, rotation))
}

fn rotate_plane<T: Copy + Default>(src: &[T], w: usize, h: usize, turns: u8) -> Vec<T> {
    if w == 0 || h == 0 || src.len() < w * h {
        return src.to_vec();
    }
    let mut dst = vec![T::default(); w * h];
    match turns % 4 {
        1 => {
            for y in 0..h {
                for x in 0..w {
                    dst[x * h + (h - 1 - y)] = src[y * w + x];
                }
            }
        }
        2 => {
            for y in 0..h {
                for x in 0..w {
                    dst[(h - 1 - y) * w + (w - 1 - x)] = src[y * w + x];
                }
            }
        }
        3 => {
            for y in 0..h {
                for x in 0..w {
                    dst[(w - 1 - x) * h + y] = src[y * w + x];
                }
            }
        }
        _ => return src.to_vec(),
    }
    dst
}

fn upsample_horiz<T: Copy>(src: &[T], cw: usize, ch: usize, full_w: usize) -> Vec<T> {
    let mut out = Vec::with_capacity(full_w * ch);
    for y in 0..ch {
        let row = &src[y * cw..(y + 1) * cw];
        for x in 0..full_w {
            out.push(row[(x / 2).min(cw.saturating_sub(1))]);
        }
    }
    out
}

fn rotate_frame(mut frame: VideoFrame, turns: u8) -> VideoFrame {
    let turns = turns % 4;
    if turns == 0 {
        return frame;
    }
    let (w, h) = (frame.width as usize, frame.height as usize);
    let (nw, nh) = if turns % 2 == 1 { (frame.height, frame.width) } else { (frame.width, frame.height) };
    let data = match frame.data {
        PixelData::Yuv8 { planes: [y, cb, cr], chroma, alpha } => {
            let (cw, ch, out_chroma) = match chroma {
                Chroma::C420 => (w.div_ceil(2), h.div_ceil(2), Chroma::C420),
                Chroma::C422 if turns % 2 == 1 => (w, h, Chroma::C444),
                Chroma::C422 => (w.div_ceil(2), h, Chroma::C422),
                Chroma::C444 => (w, h, Chroma::C444),
            };
            let (cb_r, cr_r) = if chroma == Chroma::C422 && turns % 2 == 1 {
                let src_cw = w.div_ceil(2);
                (rotate_plane(&upsample_horiz(&cb, src_cw, h, w), w, h, turns), rotate_plane(&upsample_horiz(&cr, src_cw, h, w), w, h, turns))
            } else {
                (rotate_plane(&cb, cw, ch, turns), rotate_plane(&cr, cw, ch, turns))
            };
            PixelData::Yuv8 {
                planes: [Arc::new(rotate_plane(&y, w, h, turns)), Arc::new(cb_r), Arc::new(cr_r)],
                chroma: out_chroma,
                alpha: alpha.map(|a| Arc::new(rotate_plane(&a, w, h, turns))),
            }
        }
        PixelData::Yuv16 { planes: [y, cb, cr], chroma, bits, alpha } => {
            let (cw, ch, out_chroma) = match chroma {
                Chroma::C420 => (w.div_ceil(2), h.div_ceil(2), Chroma::C420),
                Chroma::C422 if turns % 2 == 1 => (w, h, Chroma::C444),
                Chroma::C422 => (w.div_ceil(2), h, Chroma::C422),
                Chroma::C444 => (w, h, Chroma::C444),
            };
            let (cb_r, cr_r) = if chroma == Chroma::C422 && turns % 2 == 1 {
                let src_cw = w.div_ceil(2);
                (rotate_plane(&upsample_horiz(&cb, src_cw, h, w), w, h, turns), rotate_plane(&upsample_horiz(&cr, src_cw, h, w), w, h, turns))
            } else {
                (rotate_plane(&cb, cw, ch, turns), rotate_plane(&cr, cw, ch, turns))
            };
            PixelData::Yuv16 {
                planes: [Arc::new(rotate_plane(&y, w, h, turns)), Arc::new(cb_r), Arc::new(cr_r)],
                chroma: out_chroma,
                bits,
                alpha: alpha.map(|a| Arc::new(rotate_plane(&a, w, h, turns))),
            }
        }
        other => other,
    };
    frame.width = nw;
    frame.height = nh;
    frame.data = data;
    frame
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
        Samples::U16(v) => {
            let mut out = Vec::with_capacity(w * h);
            for y in 0..h {
                let row = y * stride;
                out.extend(v[row..row + w].iter().map(|&s| s.min(255) as u8));
            }
            Ok(out)
        }
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
        Samples::U8(v) => {
            let mut out = Vec::with_capacity(w * h);
            for y in 0..h {
                let row = y * stride;
                out.extend(v[row..row + w].iter().map(|&s| u16::from(s)));
            }
            Ok(out)
        }
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

fn color_from_matroska(c: &matroska::Color) -> ColorInfo {
    ColorInfo {
        matrix: c.matrix_coefficients.and_then(|m| u8::try_from(m).ok()).and_then(Matrix::from_code).unwrap_or(Matrix::Bt709),
        transfer: c.transfer_characteristics.and_then(|t| u8::try_from(t).ok()).and_then(Transfer::from_code).unwrap_or(Transfer::Bt709),
        primaries: c.primaries.and_then(|p| u8::try_from(p).ok()).and_then(primaries_from_code).unwrap_or(Primaries::Bt709),
        range: if c.full_range() { Range::Full } else { Range::Limited },
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

fn matroska_par(video: Option<&matroska::VideoInfo>, width: u32, height: u32) -> (u32, u32) {
    let Some(v) = video else { return (1, 1) };
    if v.display_unit == 0
        && let (Some(dw), Some(dh)) = (v.display_width, v.display_height)
        && dw > 0
        && dh > 0
        && width > 0
        && height > 0
        && (dw != width || dh != height)
    {
        return (dw * height, dh * width);
    }
    (1, 1)
}

fn matroska_duration(file: &MkvFile, track: &matroska::Track) -> Tick {
    if let Some(d) = file.info.duration
        && d.is_finite()
        && d > 0.0
    {
        let ns = (d * file.info.timestamp_scale as f64).round() as i64;
        return Tick::from_units(ns, 1_000_000_000);
    }
    let last_end = track.samples.last().map_or(0, |s| s.pts.saturating_add(i64::try_from(s.duration).unwrap_or(0)));
    Tick::from_units(track.ticks_to_ns(last_end), 1_000_000_000)
}

fn tick_to_units(t: Tick, timescale: u32) -> i64 {
    let v = (i128::from(t.0) * i128::from(timescale)) / i128::from(craft_time::TICKS_PER_SECOND);
    i64::try_from(v).unwrap_or(if v < 0 { i64::MIN } else { i64::MAX })
}

fn tick_to_matroska_units(t: Tick, timebase: (u64, u64)) -> i64 {
    let denom = i128::from(craft_time::TICKS_PER_SECOND) * i128::from(timebase.0.max(1));
    let v = (i128::from(t.0) * i128::from(timebase.1)) / denom;
    i64::try_from(v).unwrap_or(if v < 0 { i64::MIN } else { i64::MAX })
}

fn snap_common_fps(fps: f64) -> Option<FrameRate> {
    if !fps.is_finite() || fps <= 0.0 {
        return None;
    }
    FrameRate::COMMON
        .into_iter()
        .map(|r| (r, (fps / r.as_f64() - 1.0).abs()))
        .filter(|&(_, off)| off <= 0.005)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(r, _)| r)
}

fn estimate_isobmff_frame_rate(track: &isobmff::Track) -> FrameRate {
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
    snap_common_fps(fps).unwrap_or_else(|| FrameRate::new(i64::from(track.timescale), i64::from(median)))
}

fn estimate_matroska_frame_rate(track: &matroska::Track) -> FrameRate {
    if let Some(ns) = track.default_duration_ns
        && ns > 0
    {
        let fps = 1_000_000_000.0 / ns as f64;
        return snap_common_fps(fps).unwrap_or_else(|| FrameRate::new(1_000_000_000, i64::try_from(ns).unwrap_or(i64::MAX)));
    }
    if track.samples.len() >= 2 {
        let mut pts: Vec<i64> = track.samples.iter().map(|s| s.pts).collect();
        pts.sort_unstable();
        let mut deltas: Vec<i64> = pts.windows(2).map(|w| w[1] - w[0]).filter(|&d| d > 0).collect();
        if !deltas.is_empty() {
            deltas.sort_unstable();
            let median_ticks = deltas[deltas.len() / 2];
            let median_ns = track.ticks_to_ns(median_ticks);
            if median_ns > 0 {
                let fps = 1_000_000_000.0 / median_ns as f64;
                return snap_common_fps(fps).unwrap_or_else(|| FrameRate::new(1_000_000_000, median_ns));
            }
        }
    }
    FrameRate::FPS_30
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_cap_is_bounded() {
        let cap = worker_cap();
        assert!((1..=MAX_WORKERS).contains(&cap), "cap={cap}");
    }

    #[test]
    fn vui_overrides_container_color_and_falls_back_when_unspecified() {
        let container = color_from_isobmff(&isobmff::ColorInfo::Nclx { primaries: 9, transfer: 16, matrix: 9, full_range: false });
        assert_eq!(container.primaries, Primaries::Bt2020);
        assert_eq!(container.transfer, Transfer::Pq);
        assert_eq!(container.matrix, Matrix::Bt2020Ncl);
        assert_eq!(container.range, Range::Limited);

        // Unspecified VUI (2) falls back to container color.
        let resolved = resolve_color(ColorTags::UNSPECIFIED, Some(container));
        assert_eq!(resolved, container);

        // Explicit VUI overrides container color.
        let vui = ColorTags { primaries: 1, transfer: 1, matrix: 1, full_range: true };
        let resolved = resolve_color(vui, Some(container));
        assert_eq!(resolved.primaries, Primaries::Bt709);
        assert_eq!(resolved.transfer, Transfer::Bt709);
        assert_eq!(resolved.matrix, Matrix::Bt709);
        assert_eq!(resolved.range, Range::Full);
    }

    #[test]
    fn matroska_color_and_sniffer_preserve_policies() {
        assert!(looks_like_matroska(&[0x1A, 0x45, 0xDF, 0xA3, 0x01, 0x00]));
        assert!(!looks_like_matroska(&[0x00, 0x00, 0x00, 0x18, b'f', b't', b'y', b'p']));
        assert!(looks_like_isobmff(&[0x00, 0x00, 0x00, 0x18, b'f', b't', b'y', b'p']));

        let mkv_hdr =
            matroska::Color { matrix_coefficients: Some(9), transfer_characteristics: Some(16), primaries: Some(9), range: Some(1), ..Default::default() };
        let c = color_from_matroska(&mkv_hdr);
        assert_eq!(c.primaries, Primaries::Bt2020);
        assert_eq!(c.transfer, Transfer::Pq);
        assert_eq!(c.matrix, Matrix::Bt2020Ncl);
        assert_eq!(c.range, Range::Limited);
    }

    #[test]
    fn rotate_frame_swaps_dimensions_and_orients_planes() {
        let frame = VideoFrame {
            width: 4,
            height: 2,
            data: PixelData::Yuv8 {
                planes: [Arc::new(vec![1, 2, 3, 4, 5, 6, 7, 8]), Arc::new(vec![10, 20]), Arc::new(vec![30, 40])],
                chroma: Chroma::C420,
                alpha: Some(Arc::new(vec![11, 12, 13, 14, 15, 16, 17, 18])),
            },
            color: ColorInfo::REC709,
            par: (1, 1),
            pts: Tick::ZERO,
        };
        let rot90 = rotate_frame(frame.clone(), 1);
        assert_eq!((rot90.width, rot90.height), (2, 4));
        assert!(matches!(rot90.data, PixelData::Yuv8 { alpha: Some(_), .. }));
        if let PixelData::Yuv8 { planes: [y, ..], alpha: Some(a), .. } = &rot90.data {
            assert_eq!(&**y, &[5, 1, 6, 2, 7, 3, 8, 4]);
            assert_eq!(&**a, &[15, 11, 16, 12, 17, 13, 18, 14]);
        }
        let rot360 = rotate_frame(rot90, 3);
        assert_eq!((rot360.width, rot360.height), (4, 2));
        if let PixelData::Yuv8 { planes: [y, ..], .. } = &rot360.data {
            assert_eq!(&**y, &[1, 2, 3, 4, 5, 6, 7, 8]);
        }
    }

    #[test]
    fn plane_u8_handles_8bit_u16_storage_as_av1_produces() {
        let out = Plane::packed(2, 2, 8, Samples::U16(vec![10, 20, 30, 255])).ok().and_then(|p| plane_u8(p).ok());
        assert_eq!(out.as_deref(), Some(&[10, 20, 30, 255][..]));
    }

    #[test]
    fn rotate_frame_covers_180_270_c422_and_yuv16() {
        let f16 = VideoFrame {
            width: 4,
            height: 2,
            data: PixelData::Yuv16 {
                planes: [Arc::new(vec![1, 2, 3, 4, 5, 6, 7, 8]), Arc::new(vec![10, 20, 30, 40]), Arc::new(vec![50, 60, 70, 80])],
                chroma: Chroma::C422,
                bits: 10,
                alpha: Some(Arc::new(vec![100; 8])),
            },
            color: ColorInfo::REC709,
            par: (1, 1),
            pts: Tick::ZERO,
        };
        let r180 = rotate_frame(f16.clone(), 2);
        assert_eq!((r180.width, r180.height), (4, 2));
        if let PixelData::Yuv16 { planes: [y, ..], chroma, .. } = &r180.data {
            assert_eq!(*chroma, Chroma::C422);
            assert_eq!(&**y, &[8, 7, 6, 5, 4, 3, 2, 1]);
        }
        let r270 = rotate_frame(f16, 1);
        assert_eq!((r270.width, r270.height), (2, 4));
        if let PixelData::Yuv16 { chroma, .. } = &r270.data {
            assert_eq!(*chroma, Chroma::C444);
        }
    }

    #[test]
    fn plane_stride_and_container_helpers_cover_variants() {
        if let Ok(p8) = Plane::new(2, 2, 4, 8, Samples::U8(vec![1, 2, 9, 9, 3, 4])) {
            assert_eq!(plane_u8(p8).ok().as_deref(), Some(&[1, 2, 3, 4][..]));
        }
        if let Ok(p16) = Plane::new(2, 2, 4, 10, Samples::U16(vec![10, 20, 99, 99, 30, 40])) {
            assert_eq!(plane_u16(p16).ok().as_deref(), Some(&[10, 20, 30, 40][..]));
        }
        if let Ok(p8_as_16) = Plane::packed(2, 1, 8, Samples::U8(vec![7, 8])) {
            assert_eq!(plane_u16(p8_as_16).ok().as_deref(), Some(&[7, 8][..]));
        }

        let nclc = color_from_isobmff(&isobmff::ColorInfo::Nclc { primaries: 1, transfer: 1, matrix: 1 });
        assert_eq!(nclc, ColorInfo::REC709);
        let icc = color_from_isobmff(&isobmff::ColorInfo::Icc { kind: isobmff::FourCc(*b"prof"), profile: Vec::new() });
        assert_eq!(icc, ColorInfo::REC709);

        let v = matroska::VideoInfo { display_width: Some(1280), display_height: Some(720), display_unit: 0, ..Default::default() };
        assert_eq!(matroska_par(Some(&v), 640, 720), (1280 * 720, 720 * 640));
        assert_eq!(matroska_par(None, 640, 360), (1, 1));
        assert_eq!(snap_common_fps(23.976), Some(FrameRate::FPS_23_976));
        assert_eq!(snap_common_fps(f64::NAN), None);
        assert_eq!(tick_to_matroska_units(Tick(craft_time::TICKS_PER_SECOND), (1, 1000)), 1000);
        assert_eq!(tick_to_units(Tick(craft_time::TICKS_PER_SECOND), 90000), 90000);
    }
}
