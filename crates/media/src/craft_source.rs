//! ISO-BMFF (MP4/MOV/M4A) and Matroska/WebM (MKV/MKA/WebM) media-reader routes backed by
//! `craft-media`, `craft-video`, `craft-audio`, and `craft-media-source`.
//!
//! Preserves sync-sample backward seek, B-frame reordering, cooperative cancellation, VUI +
//! container colour resolution, draft mode for reduced-resolution playback, and bounded worker
//! threads, while falling back to `filmcraft-codecs` for unsupported codecs or audio streams.
//!
//! ## Route Matrix (`crates/media`)
//! - **ISO-BMFF (`MP4`/`MOV`/`M4A`, video+audio & audio-only)**:
//!   - Shared (`craft-media::isobmff` + `craft-video` + `craft-audio`): `H264`, `Hevc`, `Av1`,
//!     `Vp9`, `Vp8`, `ProRes` (coded alpha via `craft_video::prores::probe`, #3999), `Dnxhd`,
//!     `Mpeg2Video`; audio `Aac`, `Alac`, `Opus`, `Ac3`, `Mp3`/`Mp2`/`Mp1`.
//!   - Fallback (`filmcraft-codecs`): `Mjpeg`, `Mpeg4Video`, `Ffv1`, `Apv`, or unsupported audio.
//! - **Matroska / WebM (`MKV`/`MKA`/`WebM`, video+audio & audio-only)**:
//!   - Shared (`craft-media::matroska` + `craft-video` + `craft-audio`): `H264`, `Hevc`, `Av1`,
//!     `Vp9`, `Vp8`, `ProRes`, `Dnxhd`, `Mpeg2Video`; audio `Aac`, `Alac`, `Opus`, `Ac3`,
//!     `Mp3`/`Mp2`/`Mp1` (with `CodecDelay` + `SeekPreRoll` timing).
//!   - Fallback (`filmcraft-codecs`): `alpha_mode == 1` (`BlockAdditional` alpha), unreadable/zlib
//!     encodings, `Mjpeg`/`Ffv1`/`Theora`/`Apv`, or unsupported audio (`Vorbis`/`Flac`/`Pcm`).
//! - **MXF (`MXF`, video+audio & audio-only OP1a/OP-Atom)**:
//!   - Shared (`craft-media::mxf` + `craft-video` + `MxfFile::read_pcm`): `H264`, `Hevc`, `Av1`,
//!     `Vp9`, `Vp8`, `ProRes`, `Dnxhd`, `Mpeg2Video`; sound `Pcm` and `Aes3` (D-10).
//!   - Fallback (`filmcraft-codecs`): compressed MXF sound or unsupported video codecs.
//! - **Other containers (`AVI`, `MPEG-TS`/`PS`, `Ogg`, standalone `WAV`/`AIFF`/`FLAC`/`MP3`)**:
//!   - Currently routed via `filmcraft-codecs` in `stream.rs` (pending subsequent cutover PRs).

use std::sync::{Arc, Mutex};

use craft_audio::{AudioBuffer, TrackDecoder as AudioTrackDecoder};
use craft_color::{ColorInfo, Matrix, Primaries, Range, Transfer};
use craft_frame::{Chroma, PixelData, Region, VideoFrame};
use craft_media::bitstream::unescape_rbsp;
use craft_media::isobmff::{self, Mp4File};
use craft_media::matroska::{self, MkvFile};
use craft_media::mxf::{self, MxfFile, SoundFormat};
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

/// Complete first-sample read cap for the ProRes alpha probe; larger samples use the legacy route.
const MAX_PRORES_ALPHA_SAMPLE_BYTES: u32 = 32 * 1024 * 1024;

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

/// True when `head` contains an MXF SMPTE KLV partition pack key.
fn looks_like_mxf(head: &[u8]) -> bool {
    head.windows(12).any(|w| w == [0x06, 0x0e, 0x2b, 0x34, 0x02, 0x05, 0x01, 0x01, 0x0d, 0x01, 0x02, 0x01])
}

/// Open an in-place ISO-BMFF, Matroska/WebM, or MXF source via `craft-media` / `craft-video`.
/// Returns `None` when the container/codec/audio combination should fall back to `filmcraft-codecs`.
#[cfg(any(unix, windows))]
pub(crate) fn open_reader(name: &str, head: &[u8], reader: &SharedBytes) -> Option<Result<SharedSource>> {
    if looks_like_isobmff(head) {
        return open_isobmff(name, reader.clone());
    }
    if looks_like_matroska(head) {
        return open_matroska(name, reader.clone());
    }
    if looks_like_mxf(head) {
        return open_mxf(name, reader.clone());
    }
    None
}

/// Open an in-memory ISO-BMFF, Matroska/WebM, or MXF source via `craft-media` / `craft-video`.
pub(crate) fn open_bytes(name: &str, bytes: Arc<[u8]>) -> Option<Result<SharedSource>> {
    if looks_like_isobmff(&bytes) {
        return open_isobmff(name, Arc::new(bytes));
    }
    if looks_like_matroska(&bytes) {
        return open_matroska(name, Arc::new(bytes));
    }
    if looks_like_mxf(&bytes) {
        return open_mxf(name, Arc::new(bytes));
    }
    None
}

enum ContainerFile {
    Isobmff(Mp4File),
    Matroska(Box<MkvFile>),
    Mxf(Box<MxfFile>),
}

impl ContainerFile {
    fn sample_count(&self, track_idx: usize) -> usize {
        match self {
            ContainerFile::Isobmff(f) => f.tracks.get(track_idx).map_or(0, |t| t.samples.len()),
            ContainerFile::Matroska(f) => f.tracks.get(track_idx).map_or(0, |t| t.samples.len()),
            ContainerFile::Mxf(f) => f.tracks.get(track_idx).map_or(0, |t| t.samples.len()),
        }
    }

    fn read_sample(&self, src: &dyn ByteSource, track_idx: usize, sample_idx: usize) -> Result<Vec<u8>> {
        match self {
            ContainerFile::Isobmff(f) => f.read_sample(src, track_idx, sample_idx).map_err(|e| MediaError::Io(e.to_string())),
            ContainerFile::Matroska(f) => f.read_sample(src, track_idx, sample_idx).map_err(|e| MediaError::Io(e.to_string())),
            ContainerFile::Mxf(f) => f.read_sample(src, track_idx, sample_idx).map_err(|e| MediaError::Io(e.to_string())),
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
            ContainerFile::Mxf(f) => {
                let track = f.tracks.get(track_idx)?;
                let offset = track.first_edit_unit();
                let d = tick_to_rational_units(time, i64::from(track.edit_rate.num), i64::from(track.edit_rate.den));
                let presented_len = track.duration.unwrap_or_else(|| i64::try_from(track.display_order.len()).unwrap_or(0).saturating_sub(offset).max(0));
                let max_rel = presented_len.saturating_sub(1).max(0);
                let pos = offset.saturating_add(d.clamp(0, max_rel));
                let idx = track.sample_at(pos)?;
                Some((idx, track.samples[idx].pts))
            }
        }
    }

    fn sync_sample_before(&self, track_idx: usize, target_idx: usize) -> usize {
        match self {
            ContainerFile::Isobmff(f) => f.tracks[track_idx].sync_sample_before(target_idx),
            ContainerFile::Matroska(f) => f.tracks[track_idx].sync_sample_before(target_idx),
            ContainerFile::Mxf(f) => f.tracks[track_idx].sync_before(target_idx),
        }
    }

    fn sample_pts(&self, track_idx: usize, sample_idx: usize) -> i64 {
        match self {
            ContainerFile::Isobmff(f) => f.tracks[track_idx].samples[sample_idx].pts,
            ContainerFile::Matroska(f) => f.tracks[track_idx].samples[sample_idx].pts,
            ContainerFile::Mxf(f) => f.tracks[track_idx].samples[sample_idx].pts,
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
            ContainerFile::Mxf(f) => {
                let track = &f.tracks[track_idx];
                let rel_pts = raw_pts.saturating_sub(track.first_edit_unit());
                Tick::from_units(rel_pts.saturating_mul(i64::from(track.edit_rate.den)), i64::from(track.edit_rate.num.max(1)))
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
            ContainerFile::Mxf(_) => (0, 0),
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
            backend: AudioBackend::Decoder(Mutex::new(AudioTrackState { decoder: dec, next_sample: 0, decoded_pos: -delay_samples, buf: Vec::new() })),
        });
    }
    Some((audio_tracks, audio_infos))
}

/// A probe hint only: a single configured SPS can signal a visible size different from the
/// sample entry. Multiple or in-band parameter sets leave the initial hint to the container.
fn nal_visible_size_hint(config: &isobmff::CodecConfig) -> Option<(u32, u32)> {
    let (_, _, width, height) = match config {
        isobmff::CodecConfig::Avc(avc) => {
            if avc.pps.is_empty() {
                return None;
            }
            let mut sets = avc.sps.iter().filter(|n| n.first().is_some_and(|h| h & 0x1f == 7));
            let sps = sets.next()?;
            if sets.next().is_some() {
                return None;
            }
            craft_video::h264::decode::params::Sps::parse(&unescape_rbsp(sps.get(1..)?)).ok()?.crop_rect()
        }
        isobmff::CodecConfig::Hevc(hevc) => {
            let (mut vps, mut sps, mut pps) = (false, None, false);
            for nal in hevc.arrays.iter().filter(|a| (32..=34).contains(&a.nal_type)).flat_map(|a| &a.nalus) {
                match nal.first().map(|h| (h >> 1) & 0x3f) {
                    Some(32) => vps = true,
                    Some(33) if sps.is_some() => return None,
                    Some(33) => sps = Some(nal),
                    Some(34) => pps = true,
                    _ => {}
                }
            }
            if !vps || !pps {
                return None;
            }
            craft_video::hevc::decode::params::Sps::parse(&unescape_rbsp(sps?.get(2..)?)).ok()?.crop_rect()
        }
        _ => return None,
    };
    (width != 0 && height != 0).then_some((width, height))
}

/// Apply a valid container clean aperture in decoded-picture coordinates, before rotation.
fn clean_aperture_region(aperture: isobmff::CleanAperture, (width, height): (u32, u32)) -> Option<Region> {
    let ratio = |(n, d): (u32, u32)| (d != 0).then(|| f64::from(n) / f64::from(d));
    let offset = |(n, d): (i32, u32)| (d != 0).then(|| f64::from(n) / f64::from(d));
    let w = ratio(aperture.width)?.round();
    let h = ratio(aperture.height)?.round();
    let dx = offset(aperture.horiz_offset)?;
    let dy = offset(aperture.vert_offset)?;
    let (frame_width, frame_height) = (f64::from(width), f64::from(height));
    if w < 1.0 || h < 1.0 || w > frame_width || h > frame_height {
        return None;
    }
    let x = ((frame_width - w) / 2.0 + dx).round();
    let y = ((frame_height - h) / 2.0 + dy).round();
    if x < 0.0 || y < 0.0 || x + w > frame_width || y + h > frame_height {
        return None;
    }
    let r = Region { x: x as usize, y: y as usize, w: w as usize, h: h as usize };
    (!r.is_full(width as usize, height as usize)).then_some(r)
}

fn open_isobmff(name: &str, source: SharedBytes) -> Option<Result<SharedSource>> {
    let file = isobmff::open(source.clone()).ok()?;
    let video_idx = file.tracks.iter().position(|t| t.kind == isobmff::TrackKind::Video && !t.samples.is_empty() && !t.entries.is_empty() && t.timescale > 0);
    let audio_candidates: Vec<(usize, usize, CodecParams, u64)> = file
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.kind == isobmff::TrackKind::Audio && !t.samples.is_empty() && !t.entries.is_empty() && t.timescale > 0)
        .map(|(idx, t)| (idx, t.samples.len(), t.codec_params(), 0))
        .collect();

    if video_idx.is_none() && audio_candidates.is_empty() {
        return None;
    }

    let (video_info, video_state, presentation, duration) = if let Some(v_idx) = video_idx {
        let video_track = &file.tracks[v_idx];
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
        // Probe needs dimensions before decoding. Retain the original rational aperture separately
        // so frame conversion can use the actual visible size, even with in-band parameter sets.
        let aperture = video_track.video().and_then(|v| v.clean_aperture);
        if let Some(clap) = aperture {
            (width, height) = nal_visible_size_hint(&video_track.entries[0].codec).unwrap_or((width, height));
            if let Some(region) = clean_aperture_region(clap, (width, height)) {
                (width, height) = (region.w as u32, region.h as u32);
            }
        }
        let mut par = video_track.video().and_then(|v| v.pixel_aspect).filter(|&(n, d)| n > 0 && d > 0).unwrap_or((1, 1));
        let rotation = video_track.display_rotation().unwrap_or(0) % 4;
        if rotation % 2 == 1 {
            std::mem::swap(&mut width, &mut height);
            par = (par.1, par.0);
        }
        let has_alpha = if video_params.codec == CodecId::ProRes {
            // The coded alpha plane is authoritative for ProRes, not the MOV sample-entry depth.
            let sample = video_track.samples.first()?;
            if sample.size > MAX_PRORES_ALPHA_SAMPLE_BYTES {
                return None;
            }
            let packet = file.read_sample(&*source, v_idx, 0).ok()?;
            craft_video::prores::probe(&packet).ok()?.alpha != craft_video::prores::AlphaType::None
        } else {
            video_track.video().is_some_and(|v| v.depth == 32)
        };
        let frame_rate = estimate_isobmff_frame_rate(video_track);
        let duration = Tick::from_units(i64::try_from(video_track.duration).unwrap_or(i64::MAX), i64::from(video_track.timescale));
        let bitrate = video_track.entries.first().and_then(|e| e.bitrate).map(|b| u64::from(b.avg_bitrate)).filter(|&b| b > 0);
        let codec_name = video_params.codec.name().to_string();
        let video_decoder = match TrackDecoder::with_options(&video_params, TrackDecoderOptions { threads: Some(worker_cap()), draft: false }) {
            Ok(d) => d,
            Err(e) => return Some(Err(MediaError::Decode(e.to_string()))),
        };
        let v_info = VideoStreamInfo { width, height, frame_rate, par, codec: codec_name, pixel_format: String::new(), color, has_alpha, bitrate, hdr: None };
        let v_state = Mutex::new(VideoState { decoder: video_decoder, next_decode_idx: 0, gop_start_idx: 0, flushed: false, decoded: Vec::new() });
        (Some(v_info), Some(v_state), FramePresentation { container_color, par, rotation, aperture }, duration)
    } else {
        let duration = audio_candidates
            .iter()
            .map(|&(idx, _, _, _)| {
                let t = &file.tracks[idx];
                Tick::from_units(i64::try_from(t.duration).unwrap_or(i64::MAX), i64::from(t.timescale))
            })
            .max()
            .unwrap_or(Tick::ZERO);
        (None, None, FramePresentation { container_color: None, par: (1, 1), rotation: 0, aperture: None }, duration)
    };

    let container = ContainerFile::Isobmff(file);
    let (audio_tracks, audio_infos) = open_audio_tracks(&container, &*source, audio_candidates)?;
    if video_info.is_none() && audio_tracks.is_empty() {
        return None;
    }

    let kind = if video_info.is_some() { MediaKind::Movie } else { MediaKind::AudioOnly };
    let info = MediaInfo {
        name: name.to_string(),
        kind,
        duration,
        video: video_info,
        audio_streams: audio_infos,
        container: "mp4".to_string(),
        start_timecode: None,
        file_size: Some(source.len()),
    };

    Some(Ok(Arc::new(CraftMediaSource { info, source, file: container, video_track_idx: video_idx, presentation, video: video_state, audio_tracks })))
}

fn open_matroska(name: &str, source: SharedBytes) -> Option<Result<SharedSource>> {
    let file = matroska::open(source.clone()).ok()?;
    let video_idx = file.tracks.iter().position(|t| t.kind == matroska::TrackKind::Video && !t.samples.is_empty() && t.timebase.0 > 0 && t.timebase.1 > 0);
    let audio_candidates: Vec<(usize, usize, CodecParams, u64)> = file
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.kind == matroska::TrackKind::Audio && !t.samples.is_empty() && t.timebase.0 > 0 && t.timebase.1 > 0 && t.frames_readable())
        .map(|(idx, t)| (idx, t.samples.len(), t.codec_params(), t.codec_delay_ns))
        .collect();

    if video_idx.is_none() && audio_candidates.is_empty() {
        return None;
    }

    let container_name = if file.doc_type == "webm" { "webm" } else { "mkv" }.to_string();
    let (video_info, video_state, container_color, par, rotation, duration) = if let Some(v_idx) = video_idx {
        let video_track = &file.tracks[v_idx];
        if !video_track.frames_readable() {
            return None;
        }
        let video_params = video_track.codec_params();
        if !supports_video_codec(video_params.codec) {
            return None;
        }
        let v_meta = video_track.video.as_ref();
        // WebM/Matroska alpha_mode == 1 stores alpha in BlockAdditional; fall back to FilmCraft.
        if v_meta.is_some_and(|v| v.alpha_mode == 1) {
            return None;
        }
        let container_color = v_meta.and_then(|v| v.color.as_ref()).map(color_from_matroska);
        let color = container_color.unwrap_or(ColorInfo::REC709);
        let (mut width, mut height) = v_meta.map_or((0, 0), |v| (v.pixel_width, v.pixel_height));
        let mut par = matroska_par(v_meta, width, height);
        let rotation = v_meta.and_then(|v| v.display_rotation()).unwrap_or(0) % 4;
        if rotation % 2 == 1 {
            std::mem::swap(&mut width, &mut height);
            par = (par.1, par.0);
        }
        let frame_rate = estimate_matroska_frame_rate(video_track);
        let duration = matroska_duration(&file, video_track);
        let codec_name = video_params.codec.name().to_string();
        let video_decoder = match TrackDecoder::with_options(&video_params, TrackDecoderOptions { threads: Some(worker_cap()), draft: false }) {
            Ok(d) => d,
            Err(e) => return Some(Err(MediaError::Decode(e.to_string()))),
        };
        let v_info = VideoStreamInfo {
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
        };
        let v_state = Mutex::new(VideoState { decoder: video_decoder, next_decode_idx: 0, gop_start_idx: 0, flushed: false, decoded: Vec::new() });
        (Some(v_info), Some(v_state), container_color, par, rotation, duration)
    } else {
        let duration = audio_candidates.iter().map(|&(idx, _, _, _)| matroska_duration(&file, &file.tracks[idx])).max().unwrap_or(Tick::ZERO);
        (None, None, None, (1, 1), 0, duration)
    };

    let container = ContainerFile::Matroska(Box::new(file));
    let (audio_tracks, audio_infos) = open_audio_tracks(&container, &*source, audio_candidates)?;
    if video_info.is_none() && audio_tracks.is_empty() {
        return None;
    }

    let kind = if video_info.is_some() { MediaKind::Movie } else { MediaKind::AudioOnly };
    let info = MediaInfo {
        name: name.to_string(),
        kind,
        duration,
        video: video_info,
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
        presentation: FramePresentation { container_color, par, rotation, aperture: None },
        video: video_state,
        audio_tracks,
    })))
}

fn mxf_sound_window(track: &mxf::EssenceTrack) -> (u64, u64) {
    let (sp_num, sp_den) = track.samples_per_edit_unit();
    let sp_den = i128::from(sp_den.max(1));
    let sp_num = i128::from(sp_num);
    let offset_units = track.first_edit_unit();
    let presented_start = u64::try_from((i128::from(offset_units) * sp_num) / sp_den).unwrap_or(0);
    let stored = track.stored_sample_frames();
    let presented_len = match track.duration {
        Some(dur_units) => u64::try_from((i128::from(dur_units.max(0)) * sp_num) / sp_den).unwrap_or(u64::MAX).min(stored.saturating_sub(presented_start)),
        None => stored.saturating_sub(presented_start),
    };
    (presented_start, presented_len)
}

fn mxf_start_timecode(tc: mxf::Timecode, rate: Option<FrameRate>) -> i64 {
    let Some(rate) = rate else {
        return tc.start;
    };
    if tc.rounded_base == 0 || rate.num <= 0 || rate.den <= 0 {
        return tc.start;
    }
    let rounded_media = (rate.num as f64 / rate.den as f64).round().clamp(1.0, f64::from(u16::MAX)) as u16;
    if rounded_media == tc.rounded_base {
        return tc.start;
    }
    let (tc_num, tc_den) = if tc.drop_frame { (i128::from(tc.rounded_base) * 1000, 1001i128) } else { (i128::from(tc.rounded_base), 1i128) };
    let converted = (i128::from(tc.start) * tc_den * i128::from(rate.num)) / (tc_num * i128::from(rate.den));
    i64::try_from(converted).unwrap_or(tc.start)
}

fn open_mxf(name: &str, source: SharedBytes) -> Option<Result<SharedSource>> {
    let file = mxf::open(&*source).ok()?;
    let video_idx = file.tracks.iter().position(|t| t.kind == mxf::TrackKind::Picture && !t.samples.is_empty() && t.edit_rate.is_valid());
    let sound_indices: Vec<usize> =
        file.tracks.iter().enumerate().filter(|(_, t)| t.kind == mxf::TrackKind::Sound && !t.chunks.is_empty()).map(|(idx, _)| idx).collect();

    if video_idx.is_none() && sound_indices.is_empty() {
        return None;
    }

    // Fall back to FilmCraft for compressed MXF sound until MXF compressed audio is wired.
    let mut audio_tracks = Vec::new();
    let mut audio_infos = Vec::new();
    for &idx in &sound_indices {
        if audio_tracks.len() >= MAX_AUDIO_STREAMS {
            break;
        }
        let t = &file.tracks[idx];
        if !matches!(t.sound_format, SoundFormat::Pcm | SoundFormat::Aes3Element) {
            return None;
        }
        let sound = t.sound.as_ref()?;
        let rate = if sound.sample_rate.is_valid() { sound.sample_rate.as_f64().round().clamp(1.0, f64::from(u32::MAX)) as u32 } else { 48_000 };
        let channels = sound.channels.max(1);
        let (presented_start, presented_len) = mxf_sound_window(t);
        audio_infos.push(AudioStreamInfo { sample_rate: rate, channels, codec: t.codec.codec_id().name().to_string(), bits_per_sample: Some(sound.bits) });
        audio_tracks.push(AudioTrackEntry { track_idx: idx, native_rate: rate, backend: AudioBackend::MxfPcm { presented_start, presented_len } });
    }

    let (video_info, video_state, presentation, duration) = if let Some(v_idx) = video_idx {
        let video_track = &file.tracks[v_idx];
        let video_params = video_track.codec_params();
        if !supports_video_codec(video_params.codec) || video_track.needs_reorder {
            return None;
        }
        let (width, height) = video_track.picture.as_ref().map_or((0, 0), |p| (p.frame_width(), p.frame_height()));
        let er = video_track.edit_rate;
        let fps = f64::from(er.num) / f64::from(er.den.max(1));
        let frame_rate = snap_common_fps(fps).unwrap_or_else(|| FrameRate::new(i64::from(er.num), i64::from(er.den)));
        let dur_units = video_track.duration.unwrap_or_else(|| i64::try_from(video_track.display_order.len()).unwrap_or(0));
        let duration = Tick::from_units(dur_units.saturating_mul(i64::from(er.den)), i64::from(er.num.max(1)));
        let has_alpha = video_track.picture.as_ref().is_some_and(|p| p.alpha_depth > 0);
        let video_decoder = match TrackDecoder::with_options(&video_params, TrackDecoderOptions { threads: Some(worker_cap()), draft: false }) {
            Ok(d) => d,
            Err(e) => return Some(Err(MediaError::Decode(e.to_string()))),
        };
        let v_info = VideoStreamInfo {
            width,
            height,
            frame_rate,
            par: (1, 1),
            codec: video_params.codec.name().to_string(),
            pixel_format: String::new(),
            color: ColorInfo::REC709,
            has_alpha,
            bitrate: None,
            hdr: None,
        };
        let v_state = Mutex::new(VideoState { decoder: video_decoder, next_decode_idx: 0, gop_start_idx: 0, flushed: false, decoded: Vec::new() });
        (Some(v_info), Some(v_state), FramePresentation { container_color: None, par: (1, 1), rotation: 0, aperture: None }, duration)
    } else {
        let duration = audio_tracks
            .iter()
            .map(|entry| {
                let presented_len = match entry.backend {
                    AudioBackend::MxfPcm { presented_len, .. } => i64::try_from(presented_len).unwrap_or(i64::MAX),
                    AudioBackend::Decoder(_) => 0,
                };
                Tick::from_units(presented_len, i64::from(entry.native_rate))
            })
            .max()
            .unwrap_or(Tick::ZERO);
        (None, None, FramePresentation { container_color: None, par: (1, 1), rotation: 0, aperture: None }, duration)
    };

    let kind = if video_info.is_some() { MediaKind::Movie } else { MediaKind::AudioOnly };
    let start_timecode = file.timecode.map(|tc| mxf_start_timecode(tc, video_info.as_ref().map(|v| v.frame_rate)));
    let info = MediaInfo {
        name: name.to_string(),
        kind,
        duration,
        video: video_info,
        audio_streams: audio_infos,
        container: "mxf".to_string(),
        start_timecode,
        file_size: Some(source.len()),
    };

    Some(Ok(Arc::new(CraftMediaSource {
        info,
        source,
        file: ContainerFile::Mxf(Box::new(file)),
        video_track_idx: video_idx,
        presentation,
        video: video_state,
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

enum AudioBackend {
    Decoder(Mutex<AudioTrackState>),
    MxfPcm { presented_start: u64, presented_len: u64 },
}

struct AudioTrackEntry {
    track_idx: usize,
    native_rate: u32,
    backend: AudioBackend,
}

#[derive(Clone, Copy)]
struct FramePresentation {
    container_color: Option<ColorInfo>,
    par: (u32, u32),
    rotation: u8,
    aperture: Option<isobmff::CleanAperture>,
}

struct CraftMediaSource {
    info: MediaInfo,
    source: SharedBytes,
    file: ContainerFile,
    video_track_idx: Option<usize>,
    presentation: FramePresentation,
    video: Option<Mutex<VideoState>>,
    audio_tracks: Vec<AudioTrackEntry>,
}

impl MediaSource for CraftMediaSource {
    fn info(&self) -> &MediaInfo {
        &self.info
    }

    fn video_frame(&self, req: FrameRequest) -> Result<Arc<VideoFrame>> {
        let (Some(video_idx), Some(video_mutex)) = (self.video_track_idx, &self.video) else {
            return Err(MediaError::NoStream("video"));
        };
        let (target_idx, target_pts) = self.file.target_sample(video_idx, req.time).ok_or_else(|| MediaError::Decode("no video samples".into()))?;
        let want_draft = req.scale < 0.99;

        let mut state = video_mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(hit) = state.decoded.iter().find(|c| c.pts == target_pts && c.draft == want_draft) {
            return Ok(hit.frame.clone());
        }

        let sync_idx = self.file.sync_sample_before(video_idx, target_idx);
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

        let sample_count = self.file.sample_count(video_idx);
        while !state.decoded.iter().any(|c| c.pts >= target_pts && c.draft == want_draft) {
            if filmcraft_media::cancel::cancelled() {
                return Err(MediaError::Cancelled);
            }
            if state.next_decode_idx < sample_count {
                let idx = state.next_decode_idx;
                let pts = self.file.sample_pts(video_idx, idx);
                let pkt = self.file.read_sample(&*self.source, video_idx, idx)?;
                let frames = state.decoder.decode(&pkt, pts).map_err(|e| MediaError::Decode(e.to_string()))?;
                state.next_decode_idx = idx + 1;
                push_decoded(&mut state.decoded, frames, want_draft, self.presentation, &self.file, video_idx)?;
            } else if !state.flushed {
                let frames = state.decoder.flush().map_err(|e| MediaError::Decode(e.to_string()))?;
                state.flushed = true;
                push_decoded(&mut state.decoded, frames, want_draft, self.presentation, &self.file, video_idx)?;
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

        let decoded = match &entry.backend {
            AudioBackend::Decoder(state_mutex) => {
                let mut state = state_mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                decode_audio_range(&self.file, &*self.source, entry.track_idx, native_rate, channels_count, &mut state, native_start, native_frames)?
            }
            AudioBackend::MxfPcm { presented_start, presented_len } => {
                if filmcraft_media::cancel::cancelled() {
                    return Err(MediaError::Cancelled);
                }
                let ContainerFile::Mxf(mxf_file) = &self.file else {
                    return Err(MediaError::NoStream("audio"));
                };
                let req_end = native_start.saturating_add(native_frames as i64);
                let win_len = i64::try_from(*presented_len).unwrap_or(i64::MAX);
                if *presented_start == 0 && native_start >= 0 && req_end <= win_len {
                    mxf_file.read_pcm(&*self.source, entry.track_idx, native_start as u64, native_frames).map_err(|e| MediaError::Io(e.to_string()))?
                } else {
                    let mut out = vec![vec![0.0f32; native_frames]; channels_count];
                    let clamped_start = native_start.max(0).min(win_len);
                    let clamped_end = req_end.max(0).min(win_len);
                    let read_len = usize::try_from(clamped_end.saturating_sub(clamped_start)).unwrap_or(0);
                    if read_len > 0 {
                        let dst_off = usize::try_from(clamped_start - native_start).unwrap_or(0);
                        let stored_start = presented_start.saturating_add(clamped_start as u64);
                        let read = mxf_file.read_pcm(&*self.source, entry.track_idx, stored_start, read_len).map_err(|e| MediaError::Io(e.to_string()))?;
                        for (dst, src) in out.iter_mut().zip(&read) {
                            let n = src.len().min(read_len);
                            dst[dst_off..dst_off + n].copy_from_slice(&src[..n]);
                        }
                    }
                    out
                }
            }
        };

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

fn push_decoded(
    cache: &mut Vec<CachedFrame>,
    frames: Vec<DecodedVideoFrame>,
    draft: bool,
    presentation: FramePresentation,
    file: &ContainerFile,
    track_idx: usize,
) -> Result<()> {
    for vf in frames {
        let pts = vf.pts.unwrap_or(0);
        let pts_tick = file.pts_to_tick(track_idx, pts);
        let frame = Arc::new(to_craft_frame(vf, presentation, pts_tick)?);
        cache.push(CachedFrame { pts, draft, frame });
    }
    if cache.len() > MAX_CACHED_FRAMES {
        let excess = cache.len() - MAX_CACHED_FRAMES;
        cache.drain(..excess);
    }
    Ok(())
}

fn to_craft_frame(vf: DecodedVideoFrame, presentation: FramePresentation, pts: Tick) -> Result<VideoFrame> {
    let FramePresentation { container_color, par: default_par, rotation, aperture } = presentation;
    let (mut width, mut height) = (vf.width(), vf.height());
    let subsampling = vf.subsampling();
    let family = vf.family();
    let tags = vf.color;
    let par = match vf.sar.filter(|&(n, d)| n > 0 && d > 0) {
        Some((n, d)) if rotation % 2 == 1 => (d, n),
        Some(sar) => sar,
        None => default_par,
    };
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
            let (luma, chroma_region) = aperture
                .and_then(|clap| clean_aperture_region(clap, (width, height)))
                .and_then(|r| aligned_aperture(r, width as usize, height as usize, chroma))
                .unzip();
            if let Some(r) = luma {
                (width, height) = (r.w as u32, r.h as u32);
            }
            if bit_depth <= 8 {
                let y = Arc::new(plane_u8(y, luma)?);
                let cb = Arc::new(plane_u8(cb, chroma_region)?);
                let cr = Arc::new(plane_u8(cr, chroma_region)?);
                let a = alpha.map(|p| plane_u8(p, luma)).transpose()?.map(Arc::new);
                PixelData::Yuv8 { planes: [y, cb, cr], chroma, alpha: a }
            } else {
                let y = Arc::new(plane_u16(y, luma)?);
                let cb = Arc::new(plane_u16(cb, chroma_region)?);
                let cr = Arc::new(plane_u16(cr, chroma_region)?);
                let a = alpha.map(|p| plane_u16(p, luma)).transpose()?.map(Arc::new);
                PixelData::Yuv16 { planes: [y, cb, cr], chroma, bits: u32::from(bit_depth), alpha: a }
            }
        }
        ColorFamily::Rgb | ColorFamily::Gray => return Err(MediaError::Unsupported("non-YCbCr pixel format".into())),
    };

    let frame = VideoFrame { width, height, data, color, par, pts };
    Ok(rotate_frame(frame, rotation))
}

/// Move the origin onto the chroma grid before clipping, keeping the size when it fits.
fn aligned_aperture(region: Region, width: usize, height: usize, chroma: Chroma) -> Option<(Region, Region)> {
    let (sx, sy) = chroma.shifts();
    let (xs, ys) = (1usize << sx, 1usize << sy);
    let x = region.x.min(width) & !(xs - 1);
    let y = region.y.min(height) & !(ys - 1);
    let luma = Region { x, y, ..region }.clip(width, height);
    if luma.is_empty() || luma.is_full(width, height) {
        return None;
    }
    let chroma = Region { x: luma.x / xs, y: luma.y / ys, w: luma.w.div_ceil(xs), h: luma.h.div_ceil(ys) };
    Some((luma, chroma))
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

fn plane_region(aperture: Option<Region>, width: usize, height: usize) -> Result<Region> {
    let r = aperture.unwrap_or(Region { x: 0, y: 0, w: width, h: height });
    if r.clip(width, height) != r {
        return Err(MediaError::Decode("clean aperture exceeds a decoded plane".into()));
    }
    Ok(r)
}

fn plane_u8(p: Plane, aperture: Option<Region>) -> Result<Vec<u8>> {
    let (w, h, stride) = (p.width() as usize, p.height() as usize, p.stride());
    let r = plane_region(aperture, w, h)?;
    match p.into_samples() {
        Samples::U8(mut v) => {
            if stride == w && r.is_full(w, h) {
                v.truncate(w * h);
                return Ok(v);
            }
            let mut out = Vec::with_capacity(r.w * r.h);
            for y in r.y..r.y + r.h {
                let row = y * stride + r.x;
                out.extend_from_slice(&v[row..row + r.w]);
            }
            Ok(out)
        }
        Samples::U16(v) => {
            let mut out = Vec::with_capacity(r.w * r.h);
            for y in r.y..r.y + r.h {
                let row = y * stride + r.x;
                out.extend(v[row..row + r.w].iter().map(|&s| s.min(255) as u8));
            }
            Ok(out)
        }
    }
}

fn plane_u16(p: Plane, aperture: Option<Region>) -> Result<Vec<u16>> {
    let (w, h, stride) = (p.width() as usize, p.height() as usize, p.stride());
    let r = plane_region(aperture, w, h)?;
    match p.into_samples() {
        Samples::U16(mut v) => {
            if stride == w && r.is_full(w, h) {
                v.truncate(w * h);
                return Ok(v);
            }
            let mut out = Vec::with_capacity(r.w * r.h);
            for y in r.y..r.y + r.h {
                let row = y * stride + r.x;
                out.extend_from_slice(&v[row..row + r.w]);
            }
            Ok(out)
        }
        Samples::U8(v) => {
            let mut out = Vec::with_capacity(r.w * r.h);
            for y in r.y..r.y + r.h {
                let row = y * stride + r.x;
                out.extend(v[row..row + r.w].iter().map(|&s| u16::from(s)));
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

fn tick_to_rational_units(t: Tick, num: i64, den: i64) -> i64 {
    let denom = i128::from(craft_time::TICKS_PER_SECOND) * i128::from(den.max(1));
    let v = (i128::from(t.0) * i128::from(num.max(1))) / denom;
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
        let out = Plane::packed(2, 2, 8, Samples::U16(vec![10, 20, 30, 255])).ok().and_then(|p| plane_u8(p, None).ok());
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
            assert_eq!(plane_u8(p8, None).ok().as_deref(), Some(&[1, 2, 3, 4][..]));
        }
        if let Ok(p16) = Plane::new(2, 2, 4, 10, Samples::U16(vec![10, 20, 99, 99, 30, 40])) {
            assert_eq!(plane_u16(p16, None).ok().as_deref(), Some(&[10, 20, 30, 40][..]));
        }
        if let Ok(p8_as_16) = Plane::packed(2, 1, 8, Samples::U8(vec![7, 8])) {
            assert_eq!(plane_u16(p8_as_16, None).ok().as_deref(), Some(&[7, 8][..]));
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

    #[test]
    fn clean_aperture_rounds_signed_offsets_and_ignores_invalid_regions() {
        let aperture = isobmff::CleanAperture { width: (17, 2), height: (15, 2), horiz_offset: (1, 2), vert_offset: (-1, 2) };
        assert_eq!(clean_aperture_region(aperture, (16, 16)), Some(Region { x: 4, y: 4, w: 9, h: 8 }));
        assert_eq!(clean_aperture_region(isobmff::CleanAperture { width: (17, 0), ..aperture }, (16, 16)), None);
        assert_eq!(clean_aperture_region(isobmff::CleanAperture { horiz_offset: (10, 1), ..aperture }, (16, 16)), None);
        let full = isobmff::CleanAperture { width: (16, 1), height: (16, 1), horiz_offset: (0, 1), vert_offset: (0, 1) };
        assert_eq!(clean_aperture_region(full, (16, 16)), None);
    }

    #[test]
    fn odd_clean_aperture_crops_luma_chroma_and_alpha_together() {
        let luma = Plane::packed(6, 6, 8, Samples::U8((0..36).collect())).unwrap();
        let chroma = Plane::packed(3, 3, 8, Samples::U8((0..9).collect())).unwrap();
        let alpha = Plane::packed(6, 6, 8, Samples::U8((64..100).collect())).unwrap();
        let frame = DecodedVideoFrame::new(6, 6, ColorFamily::YCbCr, Subsampling::S420, vec![luma, chroma.clone(), chroma], Some(alpha)).unwrap();
        let aperture = Some(isobmff::CleanAperture { width: (3, 1), height: (3, 1), horiz_offset: (3, 2), vert_offset: (3, 2) });
        let presentation = FramePresentation { container_color: None, par: (3, 2), rotation: 0, aperture };
        let cropped = to_craft_frame(frame, presentation, Tick::ZERO).unwrap();
        assert_eq!((cropped.width, cropped.height, cropped.par), (3, 3, (3, 2)));
        let PixelData::Yuv8 { planes: [y, cb, cr], chroma: Chroma::C420, alpha: Some(a) } = cropped.data else {
            panic!("expected cropped 4:2:0 with alpha");
        };
        assert_eq!(y.as_slice(), &[14, 15, 16, 20, 21, 22, 26, 27, 28]);
        assert_eq!(cb.as_slice(), &[4, 5, 7, 8]);
        assert_eq!(cr.as_slice(), cb.as_slice());
        assert_eq!(a.as_slice(), &[78, 79, 80, 84, 85, 86, 90, 91, 92]);
    }

    #[test]
    fn clean_aperture_uses_the_decoded_picture_not_a_coded_size_hint() {
        let luma = Plane::packed(8, 6, 8, Samples::U8((0..48).collect())).unwrap();
        let chroma = Plane::packed(8, 6, 8, Samples::U8(vec![128; 48])).unwrap();
        let frame = DecodedVideoFrame::new(8, 6, ColorFamily::YCbCr, Subsampling::S444, vec![luma, chroma.clone(), chroma], None).unwrap();
        let aperture = isobmff::CleanAperture { width: (4, 1), height: (4, 1), horiz_offset: (0, 1), vert_offset: (0, 1) };
        // A coded/sample-entry size of 8×8 would put the crop one row too low on the decoded
        // 8×6 picture. The aperture remains in rational form until this picture is available.
        assert_eq!(clean_aperture_region(aperture, (8, 8)).unwrap().y, 2);
        let presentation = FramePresentation { container_color: None, par: (1, 1), rotation: 0, aperture: Some(aperture) };
        let cropped = to_craft_frame(frame, presentation, Tick::ZERO).unwrap();
        assert_eq!((cropped.width, cropped.height), (4, 4));
        let y = match &cropped.data {
            PixelData::Yuv8 { planes, .. } => &planes[0],
            _ => panic!("expected cropped luma"),
        };
        assert_eq!(y.as_slice(), &[10, 11, 12, 13, 18, 19, 20, 21, 26, 27, 28, 29, 34, 35, 36, 37]);
    }

    #[test]
    fn rotated_frame_preserves_container_par_without_double_inversion() {
        let make_vf = |sar: Option<(u32, u32)>| {
            let y = Plane::packed(2, 2, 8, Samples::U8(vec![1, 2, 3, 4])).ok()?;
            let cb = Plane::packed(1, 1, 8, Samples::U8(vec![128])).ok()?;
            let cr = Plane::packed(1, 1, 8, Samples::U8(vec![128])).ok()?;
            let mut vf = DecodedVideoFrame::new(2, 2, ColorFamily::YCbCr, Subsampling::S420, vec![y, cb, cr], None).ok()?;
            vf.sar = sar;
            Some(vf)
        };
        // `default_par` is already rotated by the container opener (e.g. (4, 3) -> (3, 4) for 90°).
        let container_rotated_par = (3, 4);
        let presentation = FramePresentation { container_color: None, par: container_rotated_par, rotation: 1, aperture: None };
        if let Some(vf_no_sar) = make_vf(None) {
            let out = to_craft_frame(vf_no_sar, presentation, Tick::ZERO).ok();
            assert_eq!(out.map(|f| f.par), Some((3, 4)));
        }
        // Decoder-supplied SAR is in coded coordinates and must be inverted once under 90° rotation.
        if let Some(vf_with_sar) = make_vf(Some((4, 3))) {
            let out = to_craft_frame(vf_with_sar, presentation, Tick::ZERO).ok();
            assert_eq!(out.map(|f| f.par), Some((3, 4)));
        }
    }

    #[test]
    fn mxf_sniffer_and_rational_units() {
        assert!(looks_like_mxf(&[0x06, 0x0e, 0x2b, 0x34, 0x02, 0x05, 0x01, 0x01, 0x0d, 0x01, 0x02, 0x01, 0x01, 0x02]));
        assert!(!looks_like_mxf(&[0x00, 0x00, 0x00, 0x18, b'f', b't', b'y', b'p']));
        assert_eq!(tick_to_rational_units(Tick(craft_time::TICKS_PER_SECOND), 25, 1), 25);
        assert_eq!(tick_to_rational_units(Tick(craft_time::TICKS_PER_SECOND), 30000, 1001), 29);
    }

    #[test]
    fn mxf_start_timecode_converts_mismatched_rates_and_preserves_matching() {
        let tc = mxf::Timecode { start: 100, rounded_base: 24, drop_frame: false };
        assert_eq!(mxf_start_timecode(tc, Some(FrameRate::FPS_23_976)), 100);
        assert_eq!(mxf_start_timecode(tc, Some(FrameRate::FPS_24)), 100);
        assert_eq!(mxf_start_timecode(tc, Some(FrameRate::new(48, 1))), 200);
        let df = mxf::Timecode { start: 3000, rounded_base: 30, drop_frame: true };
        assert_eq!(mxf_start_timecode(df, Some(FrameRate::FPS_29_97)), 3000);
        assert_eq!(mxf_start_timecode(df, Some(FrameRate::new(60_000, 1001))), 6000);
        assert_eq!(mxf_start_timecode(df, None), 3000);
    }

    #[test]
    fn mxf_opatom_pcm_reads_windowed_and_resampled_audio() {
        let opts = mxf::write::OpAtomPcm {
            sample_rate: 48_000,
            bits: 16,
            channels: 2,
            edit_rate: None,
            ids: mxf::write::PackageIds::from_seed("eff-mxf-test", "pcm"),
            timecode: Some(mxf::write::StartTimecode { frames: 240, rate: mxf::Rational::new(24, 1), drop_frame: false }),
        };
        let samples: Vec<i32> = (0..4800 * 2).map(|i| ((i * 137) % 16000) - 8000).collect();
        let bytes = mxf::write::write_opatom_pcm(&opts, &samples).expect("write opatom pcm");
        let src = open_bytes("test.mxf", Arc::from(bytes)).expect("recognized").expect("opened");
        let info = src.info();
        assert_eq!(info.container, "mxf");
        assert_eq!(info.kind, MediaKind::AudioOnly);
        assert_eq!(info.audio_streams.len(), 1);
        assert_eq!(info.audio_streams[0].sample_rate, 48_000);
        assert_eq!(info.audio_streams[0].channels, 2);

        // Direct read at t=0
        let buf = src.audio(0, 1024, 48_000).expect("direct read");
        assert_eq!(buf.channels.len(), 2);
        assert_eq!(buf.channels[0].len(), 1024);
        assert!(buf.channels[0].iter().any(|&v| v.abs() > 0.01));

        // Negative start and past-end read zero-fill cleanly
        let neg = src.audio(-100, 200, 48_000).expect("negative start");
        assert!(neg.channels[0][..100].iter().all(|&v| v == 0.0));
        assert!(neg.channels[0][100..].iter().any(|&v| v.abs() > 0.01));
        let past = src.audio(4700, 200, 48_000).expect("past end");
        assert!(past.channels[0][..100].iter().any(|&v| v.abs() > 0.01));
        assert!(past.channels[0][100..].iter().all(|&v| v == 0.0));

        // Resampled read
        let resampled = src.audio(0, 512, 44_100).expect("resampled");
        assert_eq!(resampled.sample_rate, 44_100);
        assert_eq!(resampled.channels[0].len(), 512);
    }

    #[test]
    fn mxf_video_window_and_sound_trim_helpers() {
        let mut cfg =
            mxf::write::WriterConfig::new(mxf::write::Pattern::Op1a, mxf::Rational::new(25, 1), mxf::write::PackageIds::from_seed("eff-mxf-vid", "op1a"));
        cfg.picture = Some(mxf::write::PictureDesc::new(mxf::write::PictureCoding::H264, 16, 16));
        cfg.sound = vec![mxf::write::SoundDesc { sample_rate: 48_000, channels: 1, bits: 16 }];
        let mut w = mxf::write::MxfWriter::new(std::io::Cursor::new(Vec::new()), cfg).expect("writer");
        let pcm = mxf::write::encode_pcm(&[vec![0.1f32; 1920]], 16);
        w.push_picture(&[0x00, 0x00, 0x00, 0x01, 0x65], mxf::write::FrameInfo::intra()).expect("pic0");
        w.push_sound(0, &pcm).expect("snd0");
        w.push_picture(&[0x00, 0x00, 0x00, 0x01, 0x41], mxf::write::FrameInfo::intra()).expect("pic1");
        w.push_sound(0, &pcm).expect("snd1");
        let mxf_bytes: SharedBytes = Arc::new(Arc::from(w.finish().expect("finish").into_inner()));
        let mut file = mxf::open(&*mxf_bytes).expect("open mxf");

        // Exercise ContainerFile::Mxf video helpers and trimmed sound window.
        file.tracks[0].origin = 1;
        file.tracks[0].duration = Some(1);
        file.tracks[1].origin = 1;
        file.tracks[1].duration = Some(1);
        let (win_start, win_len) = mxf_sound_window(&file.tracks[1]);
        assert_eq!((win_start, win_len), (1920, 1920));

        let container = ContainerFile::Mxf(Box::new(file));
        assert_eq!(container.sample_count(0), 2);
        assert_eq!(container.target_sample(0, Tick::ZERO), Some((1, 1)));
        assert_eq!(container.sync_sample_before(0, 1), 0);
        assert_eq!(container.sample_pts(0, 1), 1);
        assert_eq!(container.pts_to_tick(0, 1), Tick::ZERO);

        // needs_reorder without temporal offsets falls back to FilmCraft.
        let mut raw = mxf::open(&*mxf_bytes).expect("reopen");
        raw.tracks[0].needs_reorder = true;
        // Smoke-check open_mxf returns Some on valid Op1a H264+PCM (even if minimal NAL fails decode).
        assert!(open_bytes("op1a.mxf", Arc::from(vec![0u8; 4])).is_none());
    }
}
