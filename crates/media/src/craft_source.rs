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
//! - **AVI / MPEG-TS / MPEG-PS**:
//!   - Shared (`craft-media::avi`, `craft-media::mpegts` + `craft-video` + `craft-audio`):
//!     `H264`, `Hevc`, `Mpeg2Video`, `Av1`, `Vp9`, `Vp8`, `ProRes`, `Dnxhd`; audio `Aac`,
//!     `Ac3`/`Eac3`, `Mp3`/`Mp2`/`Mp1`, `Pcm`.
//!   - Fallback (`filmcraft-codecs`): unsupported codecs (`Mjpeg`, `Mpeg4Video`, `Ffv1`, `Theora`,
//!     `Apv`, raw RGB AVI) or unsupported audio.
//! - **Other containers & standalone audio**:
//!   - Shared (`craft_audio::Decoder` with `decode-native`): standalone `WAV`, `AIFF`, `MP3`/`MP2`/`MP1`,
//!     `AAC` (ADTS), `AC-3`, and `Ogg` (`Opus`).
//!   - Fallback (`filmcraft-codecs`): `Ogg Vorbis`, `FLAC`, `CAF`, or unsupported WAV/AIFF
//!     encodings.

use std::sync::{Arc, Mutex};

use craft_audio::{AudioBuffer, TrackDecoder as AudioTrackDecoder};
use craft_color::{ColorInfo, Matrix, Primaries, Range, Transfer};
use craft_frame::{Chroma, PixelData, Region, VideoFrame};
use craft_media::avi::{self, AviFile};
use craft_media::bitstream::unescape_rbsp;
use craft_media::isobmff::{self, Mp4File};
use craft_media::matroska::{self, MkvFile};
use craft_media::mpegts;
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
const AUDIO_SEEK_BACKUP_PACKETS: usize = 2;

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

/// Open an in-place ISO-BMFF, Matroska/WebM, MXF, AVI, MPEG-TS/PS, or standalone audio source via `craft-media` / `craft-video` / `craft-audio`.
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
    if avi::sniff(head) {
        return open_avi(name, reader.clone());
    }
    if let Some(fmt) = mpegts::sniff(head) {
        return open_mpeg(name, fmt, reader.clone());
    }
    open_standalone_audio(name, head, reader.clone())
}

/// Open an in-memory ISO-BMFF, Matroska/WebM, MXF, AVI, MPEG-TS/PS, or standalone audio source via `craft-media` / `craft-video` / `craft-audio`.
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
    if avi::sniff(&bytes) {
        return open_avi(name, Arc::new(bytes));
    }
    if let Some(fmt) = mpegts::sniff(&bytes) {
        return open_mpeg(name, fmt, Arc::new(bytes));
    }
    #[cfg(any(unix, windows))]
    {
        let head_len = <[u8]>::len(&bytes).min(64);
        let head = bytes[..head_len].to_vec();
        open_standalone_audio(name, &head, Arc::new(bytes))
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

enum ContainerFile {
    Isobmff(Mp4File),
    Matroska(Box<MkvFile>),
    Avi(Box<AviFile>),
    Mpeg { file: Box<mpegts::File>, origin_pts: i64 },
    Mxf(Box<MxfFile>),
}

impl ContainerFile {
    fn sample_count(&self, track_idx: usize) -> usize {
        match self {
            ContainerFile::Isobmff(f) => f.tracks.get(track_idx).map_or(0, |t| t.samples.len()),
            ContainerFile::Matroska(f) => f.tracks.get(track_idx).map_or(0, |t| t.samples.len()),
            ContainerFile::Avi(f) => f.streams.get(track_idx).map_or(0, |s| s.chunks.len()),
            ContainerFile::Mpeg { file, .. } => file.streams.get(track_idx).map_or(0, |s| s.units.len()),
            ContainerFile::Mxf(f) => f.tracks.get(track_idx).map_or(0, |t| t.samples.len()),
        }
    }

    fn read_sample(&self, src: &dyn ByteSource, track_idx: usize, sample_idx: usize) -> Result<Vec<u8>> {
        match self {
            ContainerFile::Isobmff(f) => f.read_sample(src, track_idx, sample_idx).map_err(|e| MediaError::Io(e.to_string())),
            ContainerFile::Matroska(f) => f.read_sample(src, track_idx, sample_idx).map_err(|e| MediaError::Io(e.to_string())),
            ContainerFile::Avi(f) => f.read_chunk(src, track_idx, sample_idx).map_err(|e| MediaError::Io(e.to_string())),
            ContainerFile::Mpeg { file, .. } => file.read_unit(src, track_idx, sample_idx).map_err(|e| MediaError::Io(e.to_string())),
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
            ContainerFile::Avi(f) => {
                let s = f.streams.get(track_idx)?;
                if s.chunks.is_empty() || s.rate == 0 || s.scale == 0 {
                    return None;
                }
                let max_idx = i64::try_from(s.chunks.len() - 1).unwrap_or(0);
                let d = tick_to_rational_units(time, i64::from(s.rate), i64::from(s.scale)).clamp(0, max_idx);
                let idx = usize::try_from(d).ok()?;
                Some((idx, d))
            }
            ContainerFile::Mpeg { file, origin_pts } => {
                let s = file.streams.get(track_idx)?;
                let t_90k = tick_to_units(time, 90_000).saturating_add(*origin_pts);
                let (idx, pts) = s
                    .units
                    .iter()
                    .enumerate()
                    .filter_map(|(i, u)| u.pts.map(|p| (i, p)))
                    .filter(|&(_, p)| p <= t_90k)
                    .max_by_key(|&(_, p)| p)
                    .or_else(|| s.units.iter().enumerate().find_map(|(i, u)| u.pts.map(|p| (i, p))))?;
                Some((idx, pts))
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
            ContainerFile::Avi(f) => {
                let s = &f.streams[track_idx];
                let i = target_idx.min(s.chunks.len().saturating_sub(1));
                if s.keyframes_known { (0..=i).rev().find(|&k| s.chunks[k].key).unwrap_or(0) } else { 0 }
            }
            ContainerFile::Mpeg { file, .. } => {
                let s = &file.streams[track_idx];
                let i = target_idx.min(s.units.len().saturating_sub(1));
                (0..=i).rev().find(|&k| s.units[k].key).unwrap_or(0)
            }
            ContainerFile::Mxf(f) => f.tracks[track_idx].sync_before(target_idx),
        }
    }

    fn sample_pts(&self, track_idx: usize, sample_idx: usize) -> i64 {
        match self {
            ContainerFile::Isobmff(f) => f.tracks[track_idx].samples[sample_idx].pts,
            ContainerFile::Matroska(f) => f.tracks[track_idx].samples[sample_idx].pts,
            ContainerFile::Avi(_) => i64::try_from(sample_idx).unwrap_or(0),
            ContainerFile::Mpeg { file, .. } => file.streams[track_idx].units[sample_idx].pts.unwrap_or(0),
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
            ContainerFile::Avi(f) => {
                let s = &f.streams[track_idx];
                Tick::from_units(raw_pts.saturating_mul(i64::from(s.scale.max(1))), i64::from(s.rate.max(1)))
            }
            ContainerFile::Mpeg { origin_pts, .. } => Tick::from_units(raw_pts.saturating_sub(*origin_pts), 90_000),
            ContainerFile::Mxf(f) => {
                let track = &f.tracks[track_idx];
                let rel_pts = raw_pts.saturating_sub(track.first_edit_unit());
                Tick::from_units(rel_pts.saturating_mul(i64::from(track.edit_rate.den)), i64::from(track.edit_rate.num.max(1)))
            }
        }
    }

    fn audio_packet_is_stamped(&self, track_idx: usize, sample_idx: usize) -> bool {
        match self {
            Self::Isobmff(_) | Self::Avi(_) | Self::Mpeg { .. } | Self::Mxf(_) => true,
            Self::Matroska(f) => f.tracks[track_idx].samples.get(sample_idx).is_some_and(|s| s.lace == 0 || s.duration > 0),
        }
    }

    fn audio_seek_info(&self, track_idx: usize, audio_starts: &[i64], seek_preroll_samples: i64, start: i64) -> (usize, i64) {
        let target = start.saturating_sub(seek_preroll_samples).max(0);
        let sample_idx = audio_starts.partition_point(|&s| s <= target).saturating_sub(1);
        let mut seek_idx = sample_idx.saturating_sub(AUDIO_SEEK_BACKUP_PACKETS);
        while seek_idx > 0 && !self.audio_packet_is_stamped(track_idx, seek_idx) {
            seek_idx -= 1;
        }
        let decoded_pos = audio_starts.get(seek_idx).copied().unwrap_or(0);
        (seek_idx, decoded_pos)
    }

    fn audio_silent_prefix(&self, track_idx: usize, start: i64, frames: usize, sample_rate: u32) -> usize {
        let Self::Isobmff(file) = self else { return 0 };
        let Some((leading_duration, media_time)) = file.tracks.get(track_idx).and_then(|track| unit_rate_media_edit(&track.edits)) else {
            return 0;
        };
        if leading_duration == 0 || media_time == 0 || file.timescale == 0 {
            return 0;
        }

        // The net packet offset includes the trim and can put decoder preroll inside
        // the empty edit. Use the empty edit's duration on the output clock instead.
        let first_presented = (u128::from(leading_duration) * u128::from(sample_rate)).div_ceil(u128::from(file.timescale)) as i128;
        usize::try_from((first_presented - i128::from(start)).max(0)).unwrap_or(usize::MAX).min(frames)
    }
}

/// Where the container puts one audio packet, for [`contiguous_starts`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PacketTime {
    /// Its timestamp, in output samples.
    stamp: i64,
    /// Whether `stamp` is the packet's own. A Matroska frame after the first in a laced block
    /// without a duration repeats the block's timestamp.
    stamped: bool,
    /// Samples it decodes to, when known.
    samples: Option<i64>,
}

/// Packet start positions in output samples: each packet starts where the previous one ends,
/// unless its own timestamp is more than `tolerance` samples from there (a real gap or overlap in
/// the recording) or the previous packet's length is unknown; then it starts at its timestamp.
///
/// Container timestamps are coarser than a sample. Matroska stores milliseconds, and a remux to MP4
/// (OBS's) turns that rounding into uneven sample durations: a 1024-sample AAC frame at 48 kHz
/// lasts 21.33 ms, so frames are stamped at 0, 1008, 2064, 3072… Placing each decoded packet at its
/// stamp overwrote the end of one packet and left a hole of silence after another, a click every
/// few packets: crackling throughout playback and export (`storytold/effectcraft#1546`,
/// `storytold/filmcraft#359`).
fn contiguous_starts(packets: impl IntoIterator<Item = PacketTime>, tolerance: i64) -> Vec<i64> {
    let tolerance = tolerance.max(0).unsigned_abs();
    let mut next = None;
    packets.into_iter().map(|p| place_packet(p, &mut next, tolerance)).collect()
}

fn packet_start(packet: PacketTime, next: Option<i64>, tolerance: u64) -> i64 {
    match next {
        Some(n) if !packet.stamped || n.abs_diff(packet.stamp) <= tolerance => n,
        _ => packet.stamp,
    }
}

fn place_packet(packet: PacketTime, next: &mut Option<i64>, tolerance: u64) -> i64 {
    let start = packet_start(packet, *next, tolerance);
    *next = packet.samples.and_then(|n| start.checked_add(n));
    start
}

struct OpusPrefix {
    len: usize,
    next: Option<i64>,
    tolerance: u64,
    min_pending_stamp: i64,
}

struct AudioStarts {
    // For Opus, unvisited entries still hold raw container stamps. Resolve in place
    // as packets arrive, reusing this table for the finished timeline.
    positions: Vec<i64>,
    opus: Option<OpusPrefix>,
}

impl AudioStarts {
    fn complete(positions: Vec<i64>) -> Self {
        Self { positions, opus: None }
    }

    fn opus(positions: Vec<i64>, tolerance: i64) -> Self {
        // Open resolves exactly the bounded lookahead before anyone can seek.
        let min_pending_stamp = positions.iter().skip(AUDIO_LOOKAHEAD).copied().min().unwrap_or(i64::MIN);
        let opus = (!positions.is_empty()).then_some(OpusPrefix { len: 0, next: None, tolerance: tolerance.max(0).unsigned_abs(), min_pending_stamp });
        Self { positions, opus }
    }

    fn start(&self, file: &ContainerFile, track: usize, idx: usize) -> Option<i64> {
        let stamp = *self.positions.get(idx)?;
        let Some(prefix) = &self.opus else { return Some(stamp) };
        if idx < prefix.len {
            return Some(stamp);
        }
        debug_assert_eq!(idx, prefix.len);
        (idx == prefix.len).then(|| {
            let packet = PacketTime { stamp, stamped: file.audio_packet_is_stamped(track, idx), samples: None };
            packet_start(packet, prefix.next, prefix.tolerance)
        })
    }

    fn observe(&mut self, file: &ContainerFile, track: usize, idx: usize, packet: &[u8]) {
        let Some(prefix) = &mut self.opus else { return };
        if idx != prefix.len {
            return;
        }
        let timing =
            PacketTime { stamp: self.positions[idx], stamped: file.audio_packet_is_stamped(track, idx), samples: opus_packet_samples(packet).map(i64::from) };
        self.positions[idx] = place_packet(timing, &mut prefix.next, prefix.tolerance);
        prefix.len += 1;
        if prefix.len == self.positions.len() {
            self.opus = None;
        }
    }

    fn seek_proves_first_packet(&self, target: i64) -> bool {
        let Some(prefix) = &self.opus else { return false };
        // A normal read that starts before the very first packet looks like a
        // backward seek. It need not read the suffix if the ordinary seek
        // would back up to packet 0 anyway. Stamped starts can only move by the
        // timing tolerance; unstamped continuation moves forward from next.
        prefix.min_pending_stamp > target
            && prefix.min_pending_stamp.abs_diff(target) > prefix.tolerance
            && prefix.next.is_none_or(|next| next > target)
            && self.positions.get(AUDIO_SEEK_BACKUP_PACKETS + 1..prefix.len).is_some_and(|p| p.iter().all(|&start| start > target))
    }

    fn finish_for_seek(&mut self, file: &ContainerFile, source: &dyn ByteSource, track: usize) -> Result<()> {
        // The exact recurrence can carry a timing phase across arbitrarily many
        // rounded or laced packets. A first discontinuous seek may still have to
        // index the unread suffix; normal open and sequential decode never do this.
        while let Some(idx) = self.opus.as_ref().map(|prefix| prefix.len) {
            if filmcraft_media::cancel::cancelled() {
                return Err(MediaError::Cancelled);
            }
            // Do not commit a guessed duration after an I/O error; the next
            // request can retry this packet without re-reading the valid prefix.
            let packet = file.read_sample(source, track, idx)?;
            self.observe(file, track, idx, &packet);
        }
        Ok(())
    }
}

fn fixed_packet_samples(dec: &mut AudioTrackDecoder, codec: CodecId, lookahead: &[Vec<u8>]) -> Option<i64> {
    if !matches!(codec, CodecId::Aac | CodecId::HeAac | CodecId::Mp1 | CodecId::Mp2 | CodecId::Mp3 | CodecId::Ac3 | CodecId::Eac3) {
        return None;
    }
    let mut frame_len: Option<usize> = None;
    for pkt in lookahead {
        let Ok(block) = dec.decode(pkt) else {
            dec.reset();
            return None;
        };
        let n = block.channels.first().map_or(0, Vec::len);
        if n == 0 {
            dec.reset();
            return None;
        }
        match frame_len {
            None => frame_len = Some(n),
            Some(prev) if prev != n => {
                dec.reset();
                return None;
            }
            Some(_) => {}
        }
    }
    dec.reset();
    frame_len.and_then(|n| i64::try_from(n).ok())
}

/// Duration of an Opus packet in 48 kHz samples from its TOC (RFC 6716 §3.1); `None` if invalid.
fn opus_packet_samples(pkt: &[u8]) -> Option<u32> {
    let &toc = pkt.first()?;
    let config = toc >> 3;
    let frame = match config {
        0..=11 => [480, 960, 1920, 2880][(config & 3) as usize],
        12..=15 => [480, 960][(config & 1) as usize],
        _ => [120, 240, 480, 960][(config & 3) as usize],
    };
    let frames = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => (*pkt.get(1)? & 0x3F) as u32,
    };
    let n = frame * frames;
    (n > 0 && n <= 5760).then_some(n)
}

fn compute_audio_starts(container: &ContainerFile, track_idx: usize, codec: CodecId, native_rate: u32, fixed_samples: Option<i64>) -> AudioStarts {
    let rate = i128::from(native_rate);
    match container {
        ContainerFile::Isobmff(f) => {
            let Some(track) = f.tracks.get(track_idx) else { return AudioStarts::complete(Vec::new()) };
            let ts = i128::from(track.timescale.max(1));
            let stamps: Vec<i64> = (0..track.samples.len()).map(|i| isobmff_audio_position(track, i, native_rate)).collect();
            let unit = i64::try_from((rate + ts - 1) / ts).unwrap_or(i64::MAX);
            if codec == CodecId::Opus {
                return AudioStarts::opus(stamps, unit.saturating_mul(2).saturating_add(1));
            }
            let Some(frame) = fixed_samples else {
                return AudioStarts::complete(stamps);
            };
            let tolerance = (frame / 2).max(unit.saturating_mul(2).saturating_add(1));
            let packets = stamps.into_iter().map(|stamp| PacketTime { stamp, stamped: true, samples: Some(frame) });
            AudioStarts::complete(contiguous_starts(packets, tolerance))
        }
        ContainerFile::Matroska(f) => {
            let Some(track) = f.tracks.get(track_idx) else { return AudioStarts::complete(Vec::new()) };
            // Indexed PTS already subtracts CodecDelay rounded to the container clock.
            // Translate between that clock and the warm path's whole-frame delay.
            let tick_ns = i128::from(f.info.timestamp_scale.max(1));
            let delay_ns = i128::from(track.codec_delay_ns);
            let indexed_delay_ns = ((delay_ns + tick_ns / 2) / tick_ns) * tick_ns;
            let delay_frames = audio_frames_from_ns(delay_ns, native_rate);
            let stamp_for = |pts: i64| -> i64 {
                let ns = i128::from(track.ticks_to_ns(pts)) + indexed_delay_ns;
                i64::try_from(audio_frames_from_ns(ns, native_rate) - delay_frames).unwrap_or(0)
            };
            let tick_samples = i64::try_from(i128::from(track.ticks_to_ns(2)) * rate / 1_000_000_000).unwrap_or(i64::MAX).saturating_add(1);
            if codec == CodecId::Opus {
                return AudioStarts::opus(track.samples.iter().map(|s| stamp_for(s.pts)).collect(), tick_samples);
            }
            let Some(frame) = fixed_samples else {
                return AudioStarts::complete(track.samples.iter().map(|s| stamp_for(s.pts)).collect());
            };
            let packets = track.samples.iter().map(|s| PacketTime { stamp: stamp_for(s.pts), stamped: s.lace == 0 || s.duration > 0, samples: Some(frame) });
            AudioStarts::complete(contiguous_starts(packets, (frame / 2).max(tick_samples)))
        }
        ContainerFile::Avi(f) => {
            let Some(s) = f.streams.get(track_idx) else { return AudioStarts::complete(Vec::new()) };
            let den = i128::from(s.rate.max(1));
            let scale = i128::from(s.scale.max(1));
            if s.sample_size > 0 {
                let mut acc = 0i64;
                return AudioStarts::complete(
                    s.chunks
                        .iter()
                        .map(|c| {
                            let start = acc;
                            acc = acc.saturating_add(i64::from(c.size / s.sample_size.max(1)));
                            start
                        })
                        .collect(),
                );
            }
            let stamps: Vec<i64> = (0..s.chunks.len()).map(|i| i64::try_from(i128::from(i as i64) * scale * rate / den).unwrap_or(0)).collect();
            let Some(frame) = fixed_samples else {
                return AudioStarts::complete(stamps);
            };
            AudioStarts::complete(contiguous_starts(stamps.into_iter().map(|stamp| PacketTime { stamp, stamped: true, samples: Some(frame) }), frame / 2))
        }
        ContainerFile::Mpeg { file, origin_pts } => {
            let Some(s) = file.streams.get(track_idx) else { return AudioStarts::complete(Vec::new()) };
            let mut last_stamp = 0i64;
            let stamps: Vec<i64> = s
                .units
                .iter()
                .map(|u| {
                    if let Some(p) = u.pts {
                        last_stamp = i64::try_from(i128::from(p.saturating_sub(*origin_pts)) * rate / 90_000).unwrap_or(0);
                    }
                    last_stamp
                })
                .collect();
            let Some(frame) = fixed_samples else {
                return AudioStarts::complete(stamps);
            };
            AudioStarts::complete(contiguous_starts(
                s.units.iter().zip(stamps).map(|(u, stamp)| PacketTime { stamp, stamped: u.pts.is_some(), samples: Some(frame) }),
                frame / 2,
            ))
        }
        ContainerFile::Mxf(_) => AudioStarts::complete(Vec::new()),
    }
}

fn audio_frames_from_ns(ns: i128, rate: u32) -> i128 {
    (ns * i128::from(rate)) / 1_000_000_000
}

/// The decoded buffer starts on the presentation timeline, including negative trimmed samples.
/// Using the same signed position on open and seek preserves edit-list trim and leading silence.
fn isobmff_audio_position(track: &isobmff::Track, sample_idx: usize, native_rate: u32) -> i64 {
    let pts = track.presentation_pts(sample_idx).unwrap_or(0);
    let scaled = (i128::from(pts) * i128::from(native_rate)) / i128::from(track.timescale.max(1));
    i64::try_from(scaled).unwrap_or(if scaled < 0 { i64::MIN } else { i64::MAX })
}

fn open_audio_tracks<I>(container: &ContainerFile, source: &dyn ByteSource, candidates: I) -> Option<(Vec<AudioTrackEntry>, Vec<AudioStreamInfo>)>
where
    I: IntoIterator<Item = (usize, usize, CodecParams, u64)>,
{
    let mut audio_tracks = Vec::new();
    let mut audio_infos = Vec::new();
    for (idx, sample_count, params, _codec_delay_ns) in candidates {
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
        let mut dec = AudioTrackDecoder::new(&params, &refs).ok()?;
        let native_rate = dec.sample_rate().max(1);
        let channels = u32::from(dec.layout().channels()).max(1);
        let codec = dec.codec();
        let fixed_samples = fixed_packet_samples(&mut dec, codec, &lookahead);
        let mut audio_starts = compute_audio_starts(container, idx, codec, native_rate, fixed_samples);
        if codec == CodecId::Opus {
            for (s_idx, pkt) in lookahead.iter().enumerate() {
                audio_starts.observe(container, idx, s_idx, pkt);
            }
        }
        let seek_preroll_samples = match container {
            ContainerFile::Matroska(f) if codec == CodecId::Opus => {
                let preroll_ns = i128::from(f.tracks[idx].seek_pre_roll_ns.max(80_000_000));
                i64::try_from(preroll_ns * i128::from(native_rate) / 1_000_000_000).unwrap_or(0)
            }
            _ => 0,
        };
        let initial_pos = audio_starts.start(container, idx, 0).unwrap_or(0);
        audio_infos.push(AudioStreamInfo {
            sample_rate: native_rate,
            channels,
            codec: params.codec.name().to_string(),
            bits_per_sample: params.pcm.map(|p| u32::from(p.bits)),
        });
        audio_tracks.push(AudioTrackEntry {
            track_idx: idx,
            native_rate,
            backend: AudioBackend::Decoder {
                seek_preroll_samples,
                state: Mutex::new(AudioTrackState { decoder: dec, audio_starts, next_sample: 0, decoded_pos: initial_pos, buf: Vec::new() }),
            },
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

/// A single unit-rate media span, optionally preceded by one empty span.
fn unit_rate_media_edit(edits: &[isobmff::Edit]) -> Option<(u64, i64)> {
    const UNIT_RATE: i32 = 0x0001_0000;
    let (leading_duration, media) = match edits {
        [media] => (0, media),
        [empty, media] if empty.media_time == -1 && empty.segment_duration > 0 && empty.media_rate == UNIT_RATE => (empty.segment_duration, media),
        _ => return None,
    };
    (media.media_time >= 0 && media.segment_duration != 0 && media.media_rate == UNIT_RATE).then_some((leading_duration, media.media_time))
}

/// A scalar EDL origin cannot describe dwell, disjoint edits or respeeding.
fn continuous_edit_mapping(track: &isobmff::Track, movie_timescale: u32) -> Option<(i64, i64)> {
    if track.edits.is_empty() {
        return (track.edit_offset == 0).then_some((0, 0));
    }
    let (leading_duration, media_time) = unit_rate_media_edit(&track.edits)?;
    if movie_timescale == 0 || track.timescale == 0 {
        return None;
    }
    // Check the raw movie-to-media conversion before the demuxer's rounded
    // offset can make a fractional delay look like an integral frame.
    let lead = i128::from(leading_duration) * i128::from(track.timescale);
    let movie_clock = i128::from(movie_timescale);
    if lead % movie_clock != 0 {
        return None;
    }
    let offset = i64::try_from(lead / movie_clock - i128::from(media_time)).ok()?;
    (offset == track.edit_offset).then_some((media_time, offset))
}

/// The EDL origin is the timecode at presentation time zero, not the raw
/// counter at the start of the timecode sample. Its own track's edit and media
/// clock determine that sample's position; the video may have a different edit.
fn isobmff_timecode_origin(file: &Mp4File, video_idx: usize, video_rate: FrameRate) -> Option<i64> {
    let video = file.tracks.get(video_idx)?;
    continuous_edit_mapping(video, file.timescale)?;
    let mut links = video.references.iter().filter(|(kind, _)| kind.0 == *b"tmcd").flat_map(|(_, ids)| ids);
    let linked = links.next();
    if links.next().is_some() {
        return None;
    }

    let mut tracks = file.tracks.iter().filter(|track| track.kind == isobmff::TrackKind::Timecode && linked.is_none_or(|id| track.id == *id));
    let track = tracks.next()?;
    if tracks.next().is_some() {
        return None;
    }
    let [entry] = track.entries.as_slice() else { return None };
    let isobmff::CodecConfig::Timecode(tc) = &entry.codec else { return None };
    let [sample] = track.samples.as_slice() else { return None };
    let (media_start, timecode_offset) = continuous_edit_mapping(track, file.timescale)?;
    if track.timescale == 0 || tc.timescale == 0 || tc.frame_duration == 0 || video_rate.num <= 0 || video_rate.den <= 0 {
        return None;
    }
    if !track.edits.is_empty() && !(sample.pts..sample.pts.checked_add(i64::from(sample.duration))?).contains(&media_start) {
        return None;
    }

    // EDL subtracts the origin in native video frames; do not invent a
    // rounding or clock-conversion policy for nonmatching or fractional origins.
    let num = i128::from(tc.timescale);
    let den = i128::from(tc.frame_duration);
    if num * i128::from(video_rate.den) != den * i128::from(video_rate.num)
        || video.samples.iter().any(|sample| i128::from(sample.duration) * num != i128::from(video.timescale) * den)
    {
        return None;
    }
    let delta = (i128::from(sample.pts) + i128::from(timecode_offset)) * num;
    let divisor = i128::from(track.timescale) * den;
    if delta % divisor != 0 {
        return None;
    }
    i64::try_from(i128::from(tc.start_frame?) - delta / divisor).ok().filter(|&origin| origin >= 0)
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

    let start_timecode = video_idx.zip(video_info.as_ref()).and_then(|(idx, info)| isobmff_timecode_origin(&file, idx, info.frame_rate));
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
        start_timecode,
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

struct ContainerVideoPart {
    track_idx: usize,
    info: VideoStreamInfo,
    state: Mutex<VideoState>,
}

fn finish_container_source(
    name: &str,
    container_name: &str,
    source: SharedBytes,
    container: ContainerFile,
    video: Option<ContainerVideoPart>,
    duration: Tick,
    audio_candidates: Vec<(usize, usize, CodecParams, u64)>,
) -> Option<Result<SharedSource>> {
    let (audio_tracks, audio_infos) = open_audio_tracks(&container, &*source, audio_candidates)?;
    if video.is_none() && audio_tracks.is_empty() {
        return None;
    }
    let kind = if video.is_some() { MediaKind::Movie } else { MediaKind::AudioOnly };
    let (video_track_idx, video_info, video_state) = match video {
        Some(v) => (Some(v.track_idx), Some(v.info), Some(v.state)),
        None => (None, None, None),
    };
    let info = MediaInfo {
        name: name.to_string(),
        kind,
        duration,
        video: video_info,
        audio_streams: audio_infos,
        container: container_name.to_string(),
        start_timecode: None,
        file_size: Some(source.len()),
    };
    Some(Ok(Arc::new(CraftMediaSource {
        info,
        source,
        file: container,
        video_track_idx,
        presentation: FramePresentation { container_color: None, par: (1, 1), rotation: 0, aperture: None },
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
    let Some(numerator) = (i128::from(tc.start) * tc_den).checked_mul(i128::from(rate.num)) else {
        return tc.start;
    };
    let denominator = tc_num * i128::from(rate.den);
    // Match FilmCraft's nearest-frame rounding for recorded nonnegative labels.
    // Keep the shared reader's existing truncation for negative counts.
    let round_up = numerator >= 0 && numerator % denominator >= denominator - denominator / 2;
    let converted = numerator / denominator + i128::from(round_up);
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
                    AudioBackend::Decoder { .. } => 0,
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

fn default_video_info(width: u32, height: u32, frame_rate: FrameRate, codec: &CodecParams) -> VideoStreamInfo {
    VideoStreamInfo {
        width,
        height,
        frame_rate,
        par: (1, 1),
        codec: codec.codec.name().to_string(),
        pixel_format: String::new(),
        color: ColorInfo::REC709,
        has_alpha: false,
        bitrate: None,
        hdr: None,
    }
}

fn open_avi(name: &str, source: SharedBytes) -> Option<Result<SharedSource>> {
    let file = avi::open(&*source).ok()?;
    let video_idx = file.streams.iter().position(|s| s.kind == avi::StreamKind::Video && !s.chunks.is_empty() && s.rate > 0 && s.scale > 0);
    let audio_candidates: Vec<(usize, usize, CodecParams, u64)> = file
        .streams
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind == avi::StreamKind::Audio && !s.chunks.is_empty())
        .map(|(idx, s)| (idx, s.chunks.len(), s.codec_params(), 0))
        .collect();

    if video_idx.is_none() && audio_candidates.is_empty() {
        return None;
    }

    let (video, duration) = if let Some(v_idx) = video_idx {
        let video_stream = &file.streams[v_idx];
        let video_params = video_stream.codec_params();
        if !supports_video_codec(video_params.codec) {
            return None;
        }
        let (width, height) = video_stream.video.as_ref().map_or((0, 0), |v| (v.width.unsigned_abs(), v.height.unsigned_abs()));
        let fps = f64::from(video_stream.rate) / f64::from(video_stream.scale.max(1));
        let frame_rate = snap_common_fps(fps).unwrap_or_else(|| FrameRate::new(i64::from(video_stream.rate), i64::from(video_stream.scale)));
        let total_units = i64::from(video_stream.length.max(video_stream.chunks.len() as u32));
        let duration = Tick::from_units(total_units.saturating_mul(i64::from(video_stream.scale)), i64::from(video_stream.rate.max(1)));
        let video_decoder = match TrackDecoder::with_options(&video_params, TrackDecoderOptions { threads: Some(worker_cap()), draft: false }) {
            Ok(d) => d,
            Err(e) => return Some(Err(MediaError::Decode(e.to_string()))),
        };
        let info = default_video_info(width, height, frame_rate, &video_params);
        let state = Mutex::new(VideoState { decoder: video_decoder, next_decode_idx: 0, gop_start_idx: 0, flushed: false, decoded: Vec::new() });
        (Some(ContainerVideoPart { track_idx: v_idx, info, state }), duration)
    } else {
        let duration = audio_candidates
            .iter()
            .map(|&(idx, _, _, _)| {
                let s = &file.streams[idx];
                Tick::from_units(i64::from(s.length).saturating_mul(i64::from(s.scale.max(1))), i64::from(s.rate.max(1)))
            })
            .max()
            .unwrap_or(Tick::ZERO);
        (None, duration)
    };

    finish_container_source(name, "avi", source, ContainerFile::Avi(Box::new(file)), video, duration, audio_candidates)
}

fn estimate_mpeg_frame_rate_pts(pts: impl IntoIterator<Item = i64>) -> FrameRate {
    let mut pts: Vec<i64> = pts.into_iter().take(64).collect();
    pts.sort_unstable();
    let mut deltas: Vec<i64> = pts.windows(2).map(|w| w[1] - w[0]).filter(|&d| d > 0).collect();
    if deltas.is_empty() {
        return FrameRate::FPS_30;
    }
    deltas.sort_unstable();
    let median = deltas[deltas.len() / 2];
    snap_common_fps(90_000.0 / median as f64).unwrap_or(FrameRate::FPS_30)
}

fn estimate_mpeg_frame_rate(units: &[mpegts::Unit]) -> FrameRate {
    estimate_mpeg_frame_rate_pts(units.iter().filter_map(|u| u.pts))
}

fn open_mpeg(name: &str, fmt: mpegts::Format, source: SharedBytes) -> Option<Result<SharedSource>> {
    let file = mpegts::open(&*source).ok()?;
    let origin_pts = file.streams.iter().filter_map(|s| s.pts_range().map(|(lo, _)| lo)).min().unwrap_or(0);
    let video_idx = file.streams.iter().position(|s| s.kind() == mpegts::Kind::Video && !s.units.is_empty());
    let audio_candidates: Vec<(usize, usize, CodecParams, u64)> = file
        .streams
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind() == mpegts::Kind::Audio && !s.units.is_empty())
        .map(|(idx, s)| (idx, s.units.len(), s.codec_params(), 0))
        .collect();

    if video_idx.is_none() && audio_candidates.is_empty() {
        return None;
    }

    let (video, duration) = if let Some(v_idx) = video_idx {
        let video_stream = &file.streams[v_idx];
        let video_params = video_stream.codec_params();
        if !supports_video_codec(video_params.codec) {
            return None;
        }
        let mut video_decoder = match TrackDecoder::with_options(&video_params, TrackDecoderOptions { threads: Some(worker_cap()), draft: false }) {
            Ok(d) => d,
            Err(e) => return Some(Err(MediaError::Decode(e.to_string()))),
        };
        let mut decoded_dims = (video_params.width.unwrap_or(0), video_params.height.unwrap_or(0));
        if decoded_dims.0 == 0 || decoded_dims.1 == 0 {
            let start_u = video_stream.units.iter().position(|u| u.key).unwrap_or(0);
            for u_idx in start_u..video_stream.units.len().min(start_u + 16) {
                if let Ok(pkt) = file.read_unit(&*source, v_idx, u_idx)
                    && let Ok(frames) = video_decoder.decode(&pkt, video_stream.units[u_idx].pts.unwrap_or(0))
                    && let Some(vf) = frames.first()
                {
                    decoded_dims = (vf.width(), vf.height());
                    break;
                }
            }
            let _ = video_decoder.reset();
        }
        let frame_rate = estimate_mpeg_frame_rate(&video_stream.units);
        let duration = video_stream.pts_range().map_or(Tick::ZERO, |(_, hi)| Tick::from_units(hi.saturating_sub(origin_pts), 90_000));
        let info = default_video_info(decoded_dims.0, decoded_dims.1, frame_rate, &video_params);
        let state = Mutex::new(VideoState { decoder: video_decoder, next_decode_idx: 0, gop_start_idx: 0, flushed: false, decoded: Vec::new() });
        (Some(ContainerVideoPart { track_idx: v_idx, info, state }), duration)
    } else {
        let duration = audio_candidates
            .iter()
            .filter_map(|&(idx, _, _, _)| file.streams[idx].pts_range().map(|(_, hi)| Tick::from_units(hi.saturating_sub(origin_pts), 90_000)))
            .max()
            .unwrap_or(Tick::ZERO);
        (None, duration)
    };

    finish_container_source(name, fmt.name(), source, ContainerFile::Mpeg { file: Box::new(file), origin_pts }, video, duration, audio_candidates)
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
    audio_starts: AudioStarts,
    next_sample: usize,
    /// Sample index (in native-rate frames) of `buf[0]` relative to track start (0).
    decoded_pos: i64,
    /// Planar: one `Vec<f32>` per channel.
    buf: Vec<Vec<f32>>,
}

enum AudioBackend {
    Decoder { seek_preroll_samples: i64, state: Mutex<AudioTrackState> },
    MxfPcm { presented_start: u64, presented_len: u64 },
}

struct AudioTrackEntry {
    track_idx: usize,
    native_rate: u32,
    backend: AudioBackend,
}

#[cfg(test)]
impl AudioTrackEntry {
    fn decoder_parts(&mut self) -> (i64, &mut AudioTrackState) {
        match &mut self.backend {
            AudioBackend::Decoder { seek_preroll_samples, state } => (*seek_preroll_samples, state.get_mut().unwrap()),
            AudioBackend::MxfPcm { .. } => panic!("expected decoder audio backend"),
        }
    }
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
        let silent_frames = self.file.audio_silent_prefix(entry.track_idx, start, frames, sample_rate);
        let audible_frames = frames - silent_frames;
        if audible_frames == 0 {
            return Ok(AudioBuffer::new(sample_rate, channels_count as u16, frames));
        }

        let audible_start = i64::try_from(i128::from(start) + silent_frames as i128).unwrap_or(i64::MAX);
        let native_rate = entry.native_rate;
        let (native_start, native_frames) = native_audio_window(audible_start, audible_frames, native_rate, sample_rate);

        let decoded = match &entry.backend {
            AudioBackend::Decoder { seek_preroll_samples, state: state_mutex } => {
                let mut state = state_mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                decode_audio_range(
                    &self.file,
                    &*self.source,
                    entry.track_idx,
                    native_rate,
                    channels_count,
                    *seek_preroll_samples,
                    &mut state,
                    native_start,
                    native_frames,
                )?
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

        let mut channels =
            if sample_rate == native_rate { decoded } else { resample_audio(&decoded, channels_count, native_rate, sample_rate, audible_frames) };
        if silent_frames > 0 {
            for channel in &mut channels {
                channel.resize(frames, 0.0);
                channel.rotate_right(silent_frames);
            }
        }
        Ok(AudioBuffer { sample_rate, channels })
    }
}

#[cfg(any(unix, windows))]
struct SharedBytesSource(SharedBytes);

#[cfg(any(unix, windows))]
impl ByteSource for SharedBytesSource {
    fn len(&self) -> u64 {
        self.0.len()
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        self.0.read_at(offset, buf)
    }
}

#[cfg(any(unix, windows))]
struct StandaloneAudioState {
    decoder: craft_audio::Decoder<'static>,
    decoded_pos: i64,
    buf: Vec<Vec<f32>>,
}

#[cfg(any(unix, windows))]
struct StandaloneAudioSource {
    info: MediaInfo,
    native_rate: u32,
    state: Mutex<StandaloneAudioState>,
}

#[cfg(any(unix, windows))]
impl MediaSource for StandaloneAudioSource {
    fn info(&self) -> &MediaInfo {
        &self.info
    }

    fn video_frame(&self, _req: FrameRequest) -> Result<Arc<VideoFrame>> {
        Err(MediaError::NoStream("video"))
    }

    fn audio(&self, start: i64, frames: usize, sample_rate: u32) -> Result<AudioBuffer> {
        self.audio_stream(0, start, frames, sample_rate)
    }

    fn audio_stream(&self, stream: usize, start: i64, frames: usize, sample_rate: u32) -> Result<AudioBuffer> {
        if stream != 0 {
            return Err(MediaError::NoStream("audio"));
        }
        let channels_count = self.info.audio_streams.first().map_or(2, |s| s.channels as usize);
        if frames == 0 || sample_rate == 0 {
            return Ok(AudioBuffer::new(sample_rate, channels_count as u16, frames));
        }
        let native_rate = self.native_rate;
        let (native_start, native_frames, phase) = if sample_rate == native_rate {
            (start, frames, 0)
        } else {
            let scaled = i128::from(start) * i128::from(native_rate);
            let divisor = i128::from(sample_rate);
            let Ok(origin) = i64::try_from(scaled.div_euclid(divisor)) else {
                // An unrepresentable source position cannot address this stream.
                return Ok(AudioBuffer::new(sample_rate, channels_count as u16, frames));
            };
            // Reuse the existing window size, but not its truncating start calculation.
            (origin, native_audio_window(0, frames, native_rate, sample_rate).1, scaled.rem_euclid(divisor) as u64)
        };

        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let decoded = decode_standalone_range(native_rate, channels_count, &mut state, native_start, native_frames)?;
        if sample_rate == native_rate {
            return Ok(AudioBuffer { sample_rate, channels: decoded });
        }
        // Negative output positions are silence, not interpolation into frame zero.
        let first_output = if start < 0 { usize::try_from(start.unsigned_abs()).unwrap_or(frames).min(frames) } else { 0 };
        Ok(AudioBuffer { sample_rate, channels: resample_standalone_audio(&decoded, native_rate, sample_rate, frames, phase, first_output) })
    }
}

/// Step the fractional input position as an exact integer quotient and remainder;
/// rounding happens only when interpolating a sample, independent of the window.
#[cfg(any(unix, windows))]
fn resample_standalone_audio(decoded: &[Vec<f32>], native_rate: u32, sample_rate: u32, frames: usize, phase: u64, first_output: usize) -> Vec<Vec<f32>> {
    let mut out = vec![vec![0.0; frames]; decoded.len()];
    let divisor = u64::from(sample_rate);
    let first = first_output as u128 * u128::from(native_rate) + u128::from(phase);
    let Ok(first_index) = usize::try_from(first / u128::from(divisor)) else { return out };
    let first_remainder = (first % u128::from(divisor)) as u64;
    let whole_step = (native_rate / sample_rate) as usize;
    let remainder_step = u64::from(native_rate % sample_rate);
    let inverse_rate = f64::from(sample_rate).recip();
    for (samples, dst) in decoded.iter().zip(&mut out) {
        let (mut index, mut remainder) = (first_index, first_remainder);
        for sample in dst.iter_mut().skip(first_output) {
            let Some(&a) = samples.get(index) else { break };
            let b = samples.get(index + 1).copied().unwrap_or(a);
            let t = (remainder as f64 * inverse_rate) as f32;
            *sample = a + (b - a) * t;
            remainder += remainder_step;
            let carry = remainder >= divisor;
            if carry {
                remainder -= divisor;
            }
            let Some(next) = index.checked_add(whole_step + carry as usize) else { break };
            index = next;
        }
    }
    out
}

#[cfg(any(unix, windows))]
fn decode_standalone_range(native_rate: u32, channels_count: usize, state: &mut StandaloneAudioState, start: i64, frames: usize) -> Result<Vec<Vec<f32>>> {
    let end = start.saturating_add(frames as i64);
    if end <= 0 {
        return Ok(vec![vec![0.0f32; frames]; channels_count]);
    }

    let buf_len = state.buf.first().map_or(0, Vec::len) as i64;
    let buf_end = state.decoded_pos.saturating_add(buf_len);
    if start < state.decoded_pos || start > buf_end + i64::from(native_rate) * 2 {
        let target = u64::try_from(start.max(0)).unwrap_or(0);
        let actual = state.decoder.seek(target).map_err(|e| MediaError::Decode(e.to_string()))?;
        state.decoded_pos = i64::try_from(actual).unwrap_or(i64::MAX);
        for ch in &mut state.buf {
            ch.clear();
        }
    }

    if state.buf.len() != channels_count {
        state.buf = vec![Vec::new(); channels_count];
    }

    while state.decoded_pos + state.buf.first().map_or(0, Vec::len) as i64 <= end {
        if filmcraft_media::cancel::cancelled() {
            return Err(MediaError::Cancelled);
        }
        let Some(block) = state.decoder.next_block().map_err(|e| MediaError::Decode(e.to_string()))? else {
            break;
        };
        for (c, ch) in state.buf.iter_mut().enumerate() {
            if let Some(src_ch) = block.channels.get(c) {
                ch.extend_from_slice(src_ch);
            }
        }
    }

    Ok(drain_audio_window(&mut state.buf, &mut state.decoded_pos, channels_count, start, frames))
}

fn native_audio_window(start: i64, frames: usize, native_rate: u32, sample_rate: u32) -> (i64, usize) {
    if sample_rate == native_rate {
        return (start, frames);
    }
    let s0 = (i128::from(start) * i128::from(native_rate)) / i128::from(sample_rate);
    let len = ((frames as u128 * u128::from(native_rate)).div_ceil(u128::from(sample_rate)) + 2) as usize;
    (i64::try_from(s0).unwrap_or(0), len)
}

fn drain_audio_window(buf: &mut [Vec<f32>], decoded_pos: &mut i64, channels_count: usize, start: i64, frames: usize) -> Vec<Vec<f32>> {
    let mut out = vec![vec![0.0f32; frames]; channels_count];
    let avail = buf.first().map_or(0, Vec::len);
    for i in 0..frames {
        let pos = start.saturating_add(i as i64);
        if pos >= *decoded_pos {
            let off = usize::try_from(pos.saturating_sub(*decoded_pos)).unwrap_or(usize::MAX);
            if off < avail {
                for (c, out_ch) in out.iter_mut().enumerate() {
                    out_ch[i] = buf[c][off];
                }
            }
        }
    }
    let end = start.saturating_add(frames as i64);
    let drop_to = usize::try_from(end.saturating_sub(*decoded_pos).max(0)).unwrap_or(usize::MAX);
    let trim = drop_to.min(avail);
    if trim > 0 {
        for ch in buf {
            ch.drain(..trim);
        }
        *decoded_pos = decoded_pos.saturating_add(trim as i64);
    }
    out
}

fn resample_audio(decoded: &[Vec<f32>], channels_count: usize, native_rate: u32, sample_rate: u32, frames: usize) -> Vec<Vec<f32>> {
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
    resampled
}

#[cfg(any(unix, windows))]
fn open_standalone_audio(name: &str, head: &[u8], source: SharedBytes) -> Option<Result<SharedSource>> {
    let ext = std::path::Path::new(name).extension().and_then(|e| e.to_str());
    let container = craft_audio::detect_container(head, ext)?;
    let container_name = match container {
        craft_audio::Container::Wav => "wav",
        craft_audio::Container::Aiff => "aiff",
        craft_audio::Container::MpegAudio => "mp3",
        craft_audio::Container::Adts => "aac",
        craft_audio::Container::Ac3 => "ac3",
        craft_audio::Container::Ogg => "ogg",
        _ => return None,
    };

    let dec = craft_audio::Decoder::open(SharedBytesSource(source.clone()), ext, &craft_audio::DecodeOptions::default()).ok()?;
    let audio_info = if dec.length_known() {
        dec.info().clone()
    } else {
        craft_audio::Decoder::open(SharedBytesSource(source.clone()), ext, &craft_audio::DecodeOptions::default()).ok()?.probe().ok()?
    };

    let native_rate = audio_info.sample_rate.max(1);
    let channels = u32::from(audio_info.channels).max(1);
    let duration = Tick::from_units(i64::try_from(audio_info.frames).unwrap_or(i64::MAX), i64::from(native_rate));
    let bits_per_sample = match audio_info.sample_format {
        craft_audio::SampleFormat::Int8 => Some(8),
        craft_audio::SampleFormat::Int16 => Some(16),
        craft_audio::SampleFormat::Int24 => Some(24),
        craft_audio::SampleFormat::Int32 | craft_audio::SampleFormat::Float32 => Some(32),
        craft_audio::SampleFormat::Float64 => Some(64),
        craft_audio::SampleFormat::Compressed | _ => None,
    };
    let info = MediaInfo {
        name: name.to_string(),
        kind: MediaKind::AudioOnly,
        duration,
        video: None,
        audio_streams: vec![AudioStreamInfo { sample_rate: native_rate, channels, codec: audio_info.codec.name().to_string(), bits_per_sample }],
        container: container_name.to_string(),
        start_timecode: None,
        file_size: Some(source.len()),
    };

    Some(Ok(Arc::new(StandaloneAudioSource { info, native_rate, state: Mutex::new(StandaloneAudioState { decoder: dec, decoded_pos: 0, buf: Vec::new() }) })))
}

#[allow(clippy::too_many_arguments)]
fn decode_audio_range(
    file: &ContainerFile,
    source: &dyn ByteSource,
    track_idx: usize,
    native_rate: u32,
    channels_count: usize,
    seek_preroll_samples: i64,
    state: &mut AudioTrackState,
    start: i64,
    frames: usize,
) -> Result<Vec<Vec<f32>>> {
    let end = start.saturating_add(frames as i64);
    if end <= 0 {
        return Ok(vec![vec![0.0f32; frames]; channels_count]);
    }

    let buf_len = state.buf.first().map_or(0, Vec::len) as i64;
    let buf_end = state.decoded_pos.saturating_add(buf_len);
    let moves_backward = start < state.decoded_pos;
    if moves_backward || start > buf_end.saturating_add(i64::from(native_rate) * 2) {
        let target = start.saturating_sub(seek_preroll_samples).max(0);
        let (seek_idx, decoded_pos) = if moves_backward && state.next_sample == 0 && state.audio_starts.seek_proves_first_packet(target) {
            (0, state.decoded_pos)
        } else {
            state.audio_starts.finish_for_seek(file, source, track_idx)?;
            file.audio_seek_info(track_idx, &state.audio_starts.positions, seek_preroll_samples, start)
        };
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
    while state.next_sample < sample_count {
        if filmcraft_media::cancel::cancelled() {
            return Err(MediaError::Cancelled);
        }
        let idx = state.next_sample;
        let buf_len = state.buf.first().map_or(0, Vec::len) as i64;
        let buf_end = state.decoded_pos.saturating_add(buf_len);
        let pk_start = if buf_len > 0 && !file.audio_packet_is_stamped(track_idx, idx) {
            buf_end
        } else {
            state.audio_starts.start(file, track_idx, idx).unwrap_or(buf_end)
        };
        if pk_start >= end {
            break;
        }
        let pkt = file.read_sample(source, track_idx, idx)?;
        state.audio_starts.observe(file, track_idx, idx, &pkt);
        state.next_sample = idx + 1;
        let block = state.decoder.decode(&pkt).map_err(|e| MediaError::Decode(e.to_string()))?;
        let buf_len = state.buf.first().map_or(0, Vec::len) as i64;
        if buf_len == 0 {
            state.decoded_pos = pk_start;
        } else {
            let buf_end = state.decoded_pos.saturating_add(buf_len);
            if pk_start > buf_end {
                let gap = usize::try_from(pk_start - buf_end).unwrap_or(0);
                for ch in &mut state.buf {
                    ch.resize(ch.len() + gap, 0.0);
                }
            } else if pk_start < buf_end {
                if pk_start <= state.decoded_pos {
                    for ch in &mut state.buf {
                        ch.clear();
                    }
                    state.decoded_pos = pk_start;
                } else {
                    let overlap = usize::try_from(buf_end - pk_start).unwrap_or(0);
                    for ch in &mut state.buf {
                        let keep = ch.len().saturating_sub(overlap);
                        ch.truncate(keep);
                    }
                }
            }
        }
        for (c, ch) in state.buf.iter_mut().enumerate() {
            if let Some(src_ch) = block.channels.get(c) {
                ch.extend_from_slice(src_ch);
            }
        }
    }

    Ok(drain_audio_window(&mut state.buf, &mut state.decoded_pos, channels_count, start, frames))
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

    // One 20 ms mono packet from craft-audio's generated tone.opus test fixture.
    // Re-mux it into ordinary small audio-only files without an external encoder.
    const OPUS_TONE: &[u8] = b"\x78\x82\x01\xB7\x6C\x7E\x40\xE6\x00\x00\x0A\xBE\x9A\xA7\xBE\xFF\xB6\x85\xD7\x0A\x71\xE0\xDD\
        \x6B\xA0\xE3\xC6\xC5\xC3\x32\xC6\x64\x1E\x6E\x63\x52\x3A\x45\x0C\x8A\x53\x8F\x44\x18\xB0\x8C\
        \x64\xAB\xEC\x98\x96\x79\x6C\x54\x53\x30\x55\x4E\x82\xE4\x16\xCA\x59\x49\xD8\xC7\x09\x5D\x59";

    struct OpusPayloadReads {
        bytes: Vec<u8>,
        offsets: Vec<u64>,
        packets: Mutex<Vec<usize>>,
        fail_once_at: Mutex<Option<usize>>,
    }

    impl ByteSource for OpusPayloadReads {
        fn len(&self) -> u64 {
            self.bytes.len() as u64
        }

        fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
            if buf.len() == OPUS_TONE.len()
                && let Ok(index) = self.offsets.binary_search(&offset)
            {
                self.packets.lock().unwrap().push(index);
                let mut failure = self.fail_once_at.lock().unwrap();
                if *failure == Some(index) {
                    *failure = None;
                    return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "test transient packet read"));
                }
            }
            self.bytes.as_slice().read_at(offset, buf)
        }
    }

    const OPUS_RATE: u32 = 48_000;
    const OPUS_FRAME: usize = 960;
    const OPUS_PACKETS: usize = 192;
    const OPUS_GAP_AT: usize = 96;
    const OPUS_GAP: usize = 9_600;
    const OPUS_LEAD: usize = 4_800;
    const OPUS_CODEC_DELAY_NS: i64 = 6_500_000;
    const OPUS_CODEC_DELAY_FRAMES: i64 = 312;

    struct OpusFixture {
        bytes: Vec<u8>,
        file: ContainerFile,
        offsets: Vec<u64>,
        params: CodecParams,
        delay: u64,
    }

    fn opus_test_config() -> isobmff::OpusConfig {
        isobmff::OpusConfig { output_channels: 1, pre_skip: OPUS_CODEC_DELAY_FRAMES as u16, input_sample_rate: OPUS_RATE, ..Default::default() }
    }

    fn generated_opus_matroska(lead: usize, laced: bool) -> OpusFixture {
        use std::io::Cursor;

        let mut track = matroska::TrackSpec::new(matroska::TrackKind::Audio, "A_OPUS");
        track.audio = Some((f64::from(OPUS_RATE), 1, None));
        track.codec_private = opus_test_config().opus_head();
        track.codec_delay_ns = OPUS_CODEC_DELAY_NS as u64;
        track.seek_pre_roll_ns = 80_000_000;
        track.default_duration_ns = Some(20_000_000);
        let options = if laced {
            matroska::MuxOptions { lacing: matroska::LacingMode::FixedOrEbml, max_lace_frames: 6, cluster_duration_ns: 10_000_000_000, ..Default::default() }
        } else {
            matroska::MuxOptions::default()
        };
        let mut mux = matroska::MkvWriter::new(Cursor::new(Vec::new()), vec![track], options).unwrap();
        let lead_ns = lead as i64 * 1_000_000_000 / i64::from(OPUS_RATE);
        for idx in 0..OPUS_PACKETS {
            let gap_ns = if idx >= OPUS_GAP_AT { 200_000_000 } else { 0 };
            let jitter = if laced || idx % 2 == 0 { 0 } else { 1_000_000 };
            mux.write_frame(0, idx as i64 * 20_000_000 + lead_ns + gap_ns + jitter - OPUS_CODEC_DELAY_NS, true, OPUS_TONE, None).unwrap();
        }
        let bytes = mux.finish().unwrap().into_inner();
        let mut file = matroska::open(bytes.as_slice()).unwrap();
        let track = &mut file.tracks[0];
        if laced {
            // The writer needs a duration to group real laces. Exercise the
            // durationless index representation by clearing it after demuxing.
            assert_eq!(track.samples[AUDIO_LOOKAHEAD].lace, 2);
            assert_eq!(track.samples[145].lace, 1);
            let mut anchor = 0;
            for packet in &mut track.samples {
                if packet.lace == 0 {
                    anchor = packet.pts;
                } else {
                    packet.pts = anchor;
                }
                packet.duration = 0;
            }
        }
        let offsets = track.samples.iter().map(|s| s.offset).collect::<Vec<_>>();
        let params = track.codec_params();
        OpusFixture { bytes, file: ContainerFile::Matroska(Box::new(file)), offsets, params, delay: OPUS_CODEC_DELAY_NS as u64 }
    }

    fn generated_opus_mp4(lead: usize) -> OpusFixture {
        use std::io::Cursor;

        let entry = isobmff::SampleEntry {
            format: isobmff::FourCc(*b"Opus"),
            data_reference_index: 1,
            codec: isobmff::CodecConfig::Opus(opus_test_config()),
            video: None,
            audio: Some(isobmff::AudioParams { channels: 1, sample_size: 16, sample_rate: f64::from(OPUS_RATE), ..Default::default() }),
            bitrate: None,
        };
        let mut mux = isobmff::Mp4Writer::new(Cursor::new(Vec::new()), isobmff::WriterOptions::new(isobmff::Brand::Mp4)).unwrap();
        mux.add_track(isobmff::TrackConfig::new(entry, OPUS_RATE)).unwrap();
        for idx in 0..OPUS_PACKETS {
            let duration = (if idx + 1 == OPUS_GAP_AT { OPUS_FRAME + OPUS_GAP } else { OPUS_FRAME }) as u32;
            mux.write_sample(0, isobmff::WriteSample { data: OPUS_TONE, duration, composition_offset: lead as i32, is_sync: true }).unwrap();
        }
        let bytes = mux.finish().unwrap().into_inner();
        let file = isobmff::open(bytes.as_slice()).unwrap();
        let track = &file.tracks[0];
        let offsets = track.samples.iter().map(|s| s.offset).collect::<Vec<_>>();
        let params = track.codec_params();
        OpusFixture { bytes, file: ContainerFile::Isobmff(file), offsets, params, delay: 0 }
    }

    fn assert_opus_early_and_backward(
        file: &ContainerFile,
        source: &OpusPayloadReads,
        params: &CodecParams,
        delay: u64,
        expected_positions: &[i64],
        lead: usize,
    ) -> Vec<Vec<f32>> {
        let (mut tracks, _) = open_audio_tracks(file, source, [(0, OPUS_PACKETS, params.clone(), delay)]).unwrap();
        assert_eq!(*source.packets.lock().unwrap(), (0..AUDIO_LOOKAHEAD).collect::<Vec<_>>());

        let first = expected_positions[0];
        assert_eq!(tracks[0].decoder_parts().1.decoded_pos, first);
        let (mut reference, _) = open_audio_tracks(file, &source.bytes, [(0, OPUS_PACKETS, params.clone(), delay)]).unwrap();
        reference[0].decoder_parts().1.audio_starts = AudioStarts::complete(expected_positions.to_vec());

        let decode = |track: &mut AudioTrackEntry, src: &dyn ByteSource, start, frames| {
            let (preroll, state) = track.decoder_parts();
            decode_audio_range(file, src, 0, OPUS_RATE, 1, preroll, state, start, frames).unwrap()
        };
        // A first read before the initial packet can be ordinary leading silence
        // or negative pre-roll. Neither should trigger the cold suffix scan.
        let early_start = if lead == 0 { first - 128 } else { 0 };
        let silent_frames = (first - early_start) as usize;
        let silent = decode(&mut tracks[0], source, early_start, silent_frames);
        assert!(silent[0].iter().all(|&s| s == 0.0));
        assert_eq!(*source.packets.lock().unwrap(), (0..AUDIO_LOOKAHEAD).collect::<Vec<_>>());
        assert_eq!(silent, decode(&mut reference[0], &source.bytes, early_start, silent_frames));
        let early_frames = silent_frames + 12 * OPUS_FRAME;
        let early = decode(&mut tracks[0], source, early_start, early_frames);
        assert_eq!(early, decode(&mut reference[0], &source.bytes, early_start, early_frames));
        assert!(early[0].iter().all(|s| s.is_finite()));
        assert!(early[0].iter().any(|s| s.abs() > 1e-5), "real Opus decoding, not a silent fallback");
        {
            let reads = source.packets.lock().unwrap();
            assert!(reads.iter().any(|&i| i >= AUDIO_LOOKAHEAD), "exercise the incremental frontier");
            assert!(reads.iter().all(|&i| i < 14) && reads.len() <= AUDIO_LOOKAHEAD + 14, "{reads:?}");
        }

        // A backward read after real decode must still use the exact seek index.
        let backward_start = first + 2 * OPUS_FRAME as i64;
        let backward = decode(&mut tracks[0], source, backward_start, OPUS_FRAME);
        assert_eq!(backward, decode(&mut reference[0], &source.bytes, backward_start, OPUS_FRAME));
        let (_, state) = tracks[0].decoder_parts();
        assert_eq!(state.audio_starts.positions.as_slice(), expected_positions);
        assert!(state.audio_starts.opus.is_none());
        early
    }

    fn assert_opus_first_cold_seek_and_retry(
        file: &ContainerFile,
        source: &OpusPayloadReads,
        params: CodecParams,
        delay: u64,
        expected_positions: &[i64],
        laced: bool,
        start: i64,
    ) {
        let first = expected_positions[0];
        let decode = |track: &mut AudioTrackEntry, src: &dyn ByteSource, start, frames| {
            let (preroll, state) = track.decoder_parts();
            decode_audio_range(file, src, 0, OPUS_RATE, 1, preroll, state, start, frames).unwrap()
        };
        // A genuinely random first request is not the leading-silence shortcut.
        let (mut fresh, _) = open_audio_tracks(file, source, [(0, OPUS_PACKETS, params.clone(), delay)]).unwrap();
        let (mut eager, _) = open_audio_tracks(file, &source.bytes, [(0, OPUS_PACKETS, params, delay)]).unwrap();
        eager[0].decoder_parts().1.audio_starts = AudioStarts::complete(expected_positions.to_vec());
        if laced {
            assert_eq!(file.audio_seek_info(0, expected_positions, fresh[0].decoder_parts().0, start).0, 144);
        }
        let transient_packet = 64;
        *source.fail_once_at.lock().unwrap() = Some(transient_packet);
        {
            let (preroll, state) = fresh[0].decoder_parts();
            let error = decode_audio_range(file, source, 0, OPUS_RATE, 1, preroll, state, start, 3 * OPUS_FRAME).unwrap_err();
            assert!(matches!(error, MediaError::Io(_)));
            assert_eq!(state.audio_starts.opus.as_ref().unwrap().len, transient_packet);
            assert_eq!((state.next_sample, state.decoded_pos), (0, first));
        }
        let cold = decode(&mut fresh[0], source, start, 3 * OPUS_FRAME);
        assert_eq!(cold, decode(&mut eager[0], &source.bytes, start, 3 * OPUS_FRAME));
        assert!(cold[0].iter().any(|s| s.abs() > 1e-5));
        let (_, state) = fresh[0].decoder_parts();
        assert_eq!(state.audio_starts.positions.as_slice(), expected_positions);
        assert!(state.audio_starts.opus.is_none());
    }

    #[test]
    fn opus_open_and_first_sequential_read_do_not_scan_tail_packets() {
        assert_eq!(opus_packet_samples(OPUS_TONE), Some(OPUS_FRAME as u32));
        let mut unlaced_matroska_audio = None;
        let cases = [(false, 0, false), (true, 0, false), (false, OPUS_LEAD, false), (true, OPUS_LEAD, false), (true, OPUS_LEAD, true)];
        for (matroska_file, lead, laced) in cases {
            let fixture = if matroska_file { generated_opus_matroska(lead, laced) } else { generated_opus_mp4(lead) };
            let OpusFixture { bytes, file, offsets, params, delay } = fixture;
            assert_eq!(offsets.len(), OPUS_PACKETS);
            assert!(offsets.windows(2).all(|w| w[0] < w[1]));
            // Start counting after container metadata indexing. This isolates payload reads by the app
            // opener, independent of how the demuxer peeks at block headers or buffers the file.
            let source = OpusPayloadReads { bytes, offsets, packets: Mutex::new(Vec::new()), fail_once_at: Mutex::new(None) };
            // Independent eager placement oracle: fixed 20 ms packets, an explicit 200 ms gap,
            // no drift from alternating Matroska stamp jitter, and delay applied once.
            let delay_frames = if matroska_file { OPUS_CODEC_DELAY_FRAMES } else { 0 };
            let expected_positions: Vec<_> = (0..OPUS_PACKETS)
                .map(|i| {
                    let gap = if i >= OPUS_GAP_AT { OPUS_GAP } else { 0 };
                    (lead + i * OPUS_FRAME + gap) as i64 - delay_frames
                })
                .collect();
            let early = assert_opus_early_and_backward(&file, &source, &params, delay, &expected_positions, lead);
            if laced {
                assert_eq!(&early, unlaced_matroska_audio.as_ref().unwrap(), "durationless laces must place the same real audio as stamped packets");
            } else if matroska_file && lead == OPUS_LEAD {
                unlaced_matroska_audio = Some(early);
            }
            let start = (lead + 151 * OPUS_FRAME + OPUS_GAP) as i64 - delay_frames;
            assert_opus_first_cold_seek_and_retry(&file, &source, params, delay, &expected_positions, laced, start);
        }
    }

    #[test]
    fn worker_cap_is_bounded() {
        let cap = worker_cap();
        assert!((1..=MAX_WORKERS).contains(&cap), "cap={cap}");
    }

    /// Generated AAC permits unequal track/sample clocks, unlike the MOV writer's PCM tracks.
    #[test]
    fn mov_aac_edit_positions_saturate_with_unequal_clocks() {
        use craft_audio::aac::{Encoder, EncoderConfig};
        use isobmff::{Brand, Edit, Mp4Writer, SampleEntry, TrackConfig, WriteSample, WriterOptions};

        const RATE: u32 = 48_000;
        let mut encoder = Encoder::new(EncoderConfig::cbr(RATE, 1, 64_000)).unwrap();
        let tone: Vec<f32> = (0..2048).map(|i| (i as f32 * 0.13).sin() * 0.25).collect();
        let mut packets = encoder.encode(&[&tone]);
        packets.extend(encoder.flush());
        assert!(!packets.is_empty());
        for (offset, expected) in [(10i64, 480i64), (-10, -480), (i64::MAX / 2, i64::MAX), (-i64::MAX / 2, i64::MIN)] {
            let mut writer = Mp4Writer::new(std::io::Cursor::new(Vec::new()), WriterOptions::new(Brand::Mov)).unwrap();
            let mut config = TrackConfig::new(SampleEntry::aac(encoder.audio_specific_config(), 1, RATE), 1_000);
            if offset > 0 {
                config.edits.push(Edit { segment_duration: offset as u64, media_time: -1, media_rate: 0x0001_0000 });
            }
            config.edits.push(Edit { segment_duration: 100, media_time: (-offset).max(0), media_rate: 0x0001_0000 });
            let track_idx = writer.add_track(config).expect("AAC accepts unequal media and sample clocks");
            for packet in &packets {
                writer.write_sample(track_idx, WriteSample { data: packet, duration: 21, composition_offset: 0, is_sync: true }).unwrap();
            }
            let bytes = writer.finish().unwrap().into_inner();
            let mov = isobmff::open(bytes.as_slice()).unwrap();
            let track = &mov.tracks[0];
            assert_eq!((track.timescale, track.edit_offset, track.presentation_pts(0)), (1_000, offset, Some(offset)));
            let params = track.codec_params();
            assert_eq!((params.codec, params.sample_rate), (CodecId::Aac, Some(RATE)));
            // Check the conversion itself: a decode error substituted with silence cannot pass.
            assert_eq!(isobmff_audio_position(track, 0, RATE), expected);
            let file = ContainerFile::Isobmff(mov);
            let (mut tracks, infos) = open_audio_tracks(&file, &bytes, [(0, packets.len(), params, 0)]).expect("shared AAC source");
            assert_eq!(infos[0].sample_rate, RATE);
            let (preroll, state) = tracks[0].decoder_parts();
            assert_eq!(state.audio_starts.start(&file, 0, 0), Some(expected));
            for start in [0, 6_000, 0] {
                let decoded = decode_audio_range(&file, &bytes, 0, RATE, 1, preroll, state, start, 64).expect("the result-returning shared read must succeed");
                assert_eq!(decoded[0].len(), 64);
                if expected == i64::MAX || expected == i64::MIN {
                    assert!(decoded[0].iter().all(|&v| v == 0.0), "out-of-range edits have no audible samples at {start}");
                }
            }
        }
    }

    /// Original PCM generated in memory to exercise an empty edit followed by a trim.
    #[test]
    fn mov_pcm_trim_after_empty_edit_preserves_leading_silence() {
        use isobmff::{Brand, Edit, Mp4Writer, PcmConfig, SampleEntry, TrackConfig, WriteSample, WriterOptions};

        const RATE: u32 = 48_000;
        const DELAY_MS: u64 = 100;
        const DELAY: i64 = 4_800;
        let samples: Vec<i16> = (0..12_000).map(|i| 1_000 + (i % 257) as i16 * 32).collect();
        for trim in [2_400i64, 4_800, 9_600] {
            let mut writer = Mp4Writer::new(std::io::Cursor::new(Vec::new()), WriterOptions::new(Brand::Mov)).unwrap();
            let pcm = PcmConfig { bits: 16, float: false, big_endian: false, signed: true, channels: 1, sample_rate: f64::from(RATE) };
            let mut config = TrackConfig::new(SampleEntry::pcm(pcm), RATE);
            config.edits = vec![
                Edit { segment_duration: DELAY_MS, media_time: -1, media_rate: 0x0001_0000 },
                Edit { segment_duration: 40, media_time: trim, media_rate: 0x0001_0000 },
            ];
            let track = writer.add_track(config).unwrap();
            for chunk in samples.chunks(256) {
                let data: Vec<u8> = chunk.iter().flat_map(|sample| sample.to_le_bytes()).collect();
                writer.write_sample(track, WriteSample { data: &data, duration: chunk.len() as u32, composition_offset: 0, is_sync: true }).unwrap();
            }
            let bytes = writer.finish().unwrap().into_inner();
            let file = isobmff::open(bytes.as_slice()).unwrap();
            assert_eq!(file.timescale, 1_000);
            assert_eq!(file.tracks[0].edit_offset, DELAY - trim);

            let source = open_bytes("edited-pcm.mov", bytes.into()).expect("shared MOV route").unwrap();
            let sample = |index: i64| f32::from(samples[index as usize]) / 32_768.0;
            let expected = [0.0, 0.0, sample(trim), sample(trim + 1)];
            let read = |start, frames, rate| source.audio(start, frames, rate).expect("direct shared PCM read").channels.remove(0);
            assert_eq!(read(DELAY - 2, 4, RATE), expected, "first read, trim {trim}");
            assert_eq!(read(0, DELAY as usize, RATE), vec![0.0; DELAY as usize], "backward seek, trim {trim}");
            assert_eq!(read(DELAY, 2, RATE), expected[2..], "read from first presented sample, trim {trim}");
            assert_eq!(read(DELAY * 2 - 4, 5, RATE * 2), [0.0, 0.0, 0.0, 0.0, sample(trim)], "resampled read, trim {trim}");
            assert_eq!(read(4_408, 3, 44_100), [0.0, 0.0, sample(trim)], "nonintegral resampling ratio, trim {trim}");
        }
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
    fn mxf_start_timecode_rounds_fractional_recorded_frames() {
        // Three frames at 25 fps fall at 3.6 frames on a 30 fps track.
        let tc = mxf::Timecode { start: 3, rounded_base: 25, drop_frame: false };
        assert_eq!(mxf_start_timecode(tc, Some(FrameRate::FPS_30)), 4);
        // Two frames at 24 fps fall exactly halfway between frames 2 and 3.
        let half = mxf::Timecode { start: 2, rounded_base: 24, ..tc };
        assert_eq!(mxf_start_timecode(half, Some(FrameRate::FPS_30)), 3);
        let integral = mxf::Timecode { start: 5, ..tc };
        assert_eq!(mxf_start_timecode(integral, Some(FrameRate::FPS_30)), 6);

        let negative = mxf::Timecode { start: -100, ..tc };
        assert_eq!(mxf_start_timecode(negative, Some(FrameRate::FPS_30)), -120);
        let negative_fraction = mxf::Timecode { start: -3, ..tc };
        assert_eq!(mxf_start_timecode(negative_fraction, Some(FrameRate::FPS_30)), -3);
        let fallback = mxf::Timecode { start: i64::MAX - 1, ..tc };
        assert_eq!(mxf_start_timecode(fallback, Some(FrameRate::FPS_50)), i64::MAX - 1);
        let too_large = mxf::Timecode { start: i64::MAX, ..tc };
        assert_eq!(mxf_start_timecode(too_large, Some(FrameRate::FPS_50)), i64::MAX);
        let extreme = mxf::Timecode { drop_frame: true, ..too_large };
        assert_eq!(mxf_start_timecode(extreme, Some(FrameRate::new(i64::MAX, 1))), i64::MAX);
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
        cfg.picture = Some(mxf::write::PictureDesc::new(mxf::write::PictureCoding::Avc { profile_idc: 66, intra: false }, 16, 16));
        cfg.sound = vec![mxf::write::SoundDesc { sample_rate: 48_000, channels: 1, bits: 16 }];
        let mut w = mxf::write::MxfWriter::new(std::io::Cursor::new(Vec::new()), cfg).expect("writer");
        let pcm = mxf::write::encode_pcm(&[vec![0.1f32; 1920]], 16);
        w.push_picture(&[0x00, 0x00, 0x00, 0x01, 0x65], mxf::write::FrameInfo::intra()).expect("pic0");
        w.push_sound(0, &pcm).expect("snd0");
        w.push_picture(&[0x00, 0x00, 0x00, 0x01, 0x41], mxf::write::FrameInfo::intra()).expect("pic1");
        w.push_sound(0, &pcm).expect("snd1");
        let raw_bytes = w.finish().expect("finish").into_inner();
        let arc_bytes: Arc<[u8]> = Arc::from(raw_bytes.clone());
        let mxf_bytes: SharedBytes = Arc::new(arc_bytes);
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
        assert_eq!(container.sync_sample_before(0, 1), 1);
        assert_eq!(container.sample_pts(0, 1), 1);
        assert_eq!(container.pts_to_tick(0, 1), Tick::ZERO);

        // Exercise open_mxf video+sound path on valid Op1a Avc+PCM.
        let src = open_bytes("op1a.mxf", Arc::from(raw_bytes)).expect("recognized").expect("opened");
        assert_eq!(src.info().container, "mxf");
        assert_eq!(src.info().audio_streams.len(), 1);
        assert!(open_bytes("op1a.mxf", Arc::from(vec![0u8; 4])).is_none());
    }

    #[test]
    fn standalone_wav_opens_seeks_and_resamples_via_craft_audio() {
        // Minimal 48 kHz stereo 16-bit PCM WAV with 4800 frames (0.1s).
        let frames = 4800u32;
        let data_bytes = frames * 4;
        let mut wav = Vec::with_capacity(44 + data_bytes as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&48_000u32.to_le_bytes());
        wav.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_bytes.to_le_bytes());
        for i in 0..frames * 2 {
            let s = (((i as i32 * 257) % 16000) - 8000) as i16;
            wav.extend_from_slice(&s.to_le_bytes());
        }

        let wav = Arc::<[u8]>::from(wav);
        let src = open_bytes("tone.wav", wav.clone()).expect("recognized").expect("opened");
        let info = src.info();
        assert_eq!(info.container, "wav");
        assert_eq!(info.kind, MediaKind::AudioOnly);
        assert_eq!(info.audio_streams[0].sample_rate, 48_000);
        assert_eq!(info.audio_streams[0].bits_per_sample, Some(16));

        let head = src.audio(0, 512, 48_000).expect("head");
        assert_eq!(head.channels[0].len(), 512);
        let tail = src.audio(4000, 512, 48_000).expect("forward seek");
        assert!(tail.channels[0].iter().any(|&v| v.abs() > 0.01));
        let back = src.audio(0, 512, 48_000).expect("backward seek");
        assert_eq!(head.channels[0], back.channels[0]);
        let neg = src.audio(-50, 100, 48_000).expect("negative start");
        assert!(neg.channels[0][..50].iter().all(|&v| v == 0.0));
        let resampled = src.audio(0, 256, 44_100).expect("resampled");
        assert_eq!(resampled.sample_rate, 44_100);
        assert_eq!(resampled.channels[0].len(), 256);

        let cold = open_bytes("tone.wav", wav).expect("recognized").expect("opened");
        let alone = cold.audio(1, 1, 44_100).expect("single output frame");
        // Output frame 1 is at input frame 160/147. The left channel's
        // input frames 1 and 2 contain -7486 and -6972 in signed 16-bit PCM.
        let expected = (-7486.0 + (13.0 / 147.0) * 514.0) / 32768.0;
        assert!((alone.channels[0][0] - expected).abs() < 1e-7, "absolute fractional phase");
        for (single, block) in alone.channels.iter().zip(&resampled.channels) {
            assert_eq!(single[0], block[1], "reading a frame alone must match a longer block");
        }
        let pre_roll = cold.audio(-1, 3, 44_100).expect("negative output window");
        for (shifted, block) in pre_roll.channels.iter().zip(&resampled.channels) {
            assert_eq!(shifted[0], 0.0, "silence before frame zero");
            assert_eq!(&shifted[1..], &block[..2], "crossing frame zero must preserve phase");
        }
        // Frame 3675 is exactly native frame 4000, where the left ramp is zero.
        let crossing = cold.audio(3670, 6, 44_100).expect("cross a zero sample");
        let zero = cold.audio(3675, 1, 44_100).expect("read the zero sample alone");
        assert_eq!(crossing.channels[0][5], 0.0, "a fractional start must not leak into an integral sample");
        assert_eq!(crossing.channels[0][5], zero.channels[0][0]);

        let after_end = cold.audio(4410, 2, 44_100).expect("after EOF");
        assert!(after_end.channels.iter().flatten().all(|&sample| sample == 0.0));
        let out_of_range = cold.audio(i64::MAX / 2, 1, 1).expect("unrepresentable source origin");
        assert!(out_of_range.channels.iter().flatten().all(|&sample| sample == 0.0), "an unreachable origin must not replay frame zero");
    }

    /// 1024-sample frames at 48 kHz stamped to the nearest millisecond, then back in samples.
    fn ms_stamps(count: i64) -> Vec<i64> {
        (0..count).map(|k| (k * 1024 * 1000 + 24_000) / 48_000 * 48).collect()
    }

    fn timed(stamps: &[i64], samples: Option<i64>) -> Vec<PacketTime> {
        stamps.iter().map(|&stamp| PacketTime { stamp, stamped: true, samples }).collect()
    }

    #[test]
    fn rounded_stamps_give_back_to_back_packets() {
        let stamps = ms_stamps(200);
        assert_eq!(&stamps[..4], [0, 1008, 2064, 3072]);
        let starts = contiguous_starts(timed(&stamps, Some(1024)), 512);
        assert_eq!(starts, (0..200).map(|k| k * 1024).collect::<Vec<_>>());
    }

    #[test]
    fn real_gaps_and_overlaps_resynchronise() {
        // A 100 ms gap after packet 3, then a packet stamped 600 samples early.
        let mut stamps: Vec<i64> = (0..8).map(|k| k * 1024).collect();
        for s in &mut stamps[4..] {
            *s += 4800;
        }
        stamps[7] -= 600;
        let starts = contiguous_starts(timed(&stamps, Some(1024)), 512);
        assert_eq!(starts, [0, 1024, 2048, 3072, 8896, 9920, 10944, 11368]);
        // Within the tolerance: runs on.
        stamps[7] += 200;
        assert_eq!(contiguous_starts(timed(&stamps, Some(1024)), 512)[7], 11968);
    }

    #[test]
    fn unstamped_packets_follow_on_and_unknown_lengths_resynchronise() {
        let packets = [0, 1024, 1024, 1024, 4096].map(|stamp| PacketTime { stamp, stamped: true, samples: Some(1024) });
        let mut laced = packets;
        laced[2].stamped = false;
        laced[3].stamped = false;
        assert_eq!(contiguous_starts(laced, 512), [0, 1024, 2048, 3072, 4096]);
        let mut unknown = timed(&[0, 1024, 2100, 3124], Some(1024));
        unknown[1].samples = None;
        assert_eq!(contiguous_starts(unknown, 512), [0, 1024, 2100, 3124]);
    }

    #[test]
    fn hostile_values_stay_finite() {
        let packets = [
            PacketTime { stamp: i64::MAX, stamped: true, samples: Some(i64::MAX) },
            PacketTime { stamp: i64::MIN, stamped: false, samples: Some(-5) },
            PacketTime { stamp: 0, stamped: true, samples: Some(i64::MIN) },
            PacketTime { stamp: 7, stamped: true, samples: None },
        ];
        assert_eq!(contiguous_starts(packets, i64::MIN).len(), 4);
        assert_eq!(contiguous_starts(packets, i64::MAX).len(), 4);
        assert!(contiguous_starts([], 512).is_empty());
    }

    #[test]
    fn avi_and_mpeg_helpers_and_in_memory_route() {
        assert_eq!(tick_to_rational_units(Tick(craft_time::TICKS_PER_SECOND), 24, 1), 24);
        assert_eq!(tick_to_rational_units(Tick(-1), 24, 1), 0);
        assert_eq!(estimate_mpeg_frame_rate_pts([0, 3000, 6000]), FrameRate::FPS_30);
        assert_eq!(estimate_mpeg_frame_rate(&[]), FrameRate::FPS_30);

        let streams = vec![avi::StreamSpec::Video(avi::VideoSpec::new(*b"H264", 16, 16, 24, 1)), avi::StreamSpec::Audio(avi::AudioSpec::pcm(48_000, 2, 16))];
        let mut w = avi::AviWriter::new(std::io::Cursor::new(Vec::new()), streams, avi::MuxOptions::default()).expect("avi writer");
        let pcm = vec![0x10u8; 2000 * 4];
        w.write_chunk(0, &[0x00, 0x00, 0x00, 0x01, 0x65], true).expect("v0");
        w.write_chunk(1, &pcm, true).expect("a0");
        w.write_chunk(0, &[0x00, 0x00, 0x00, 0x01, 0x41], false).expect("v1");
        w.write_chunk(1, &pcm, true).expect("a1");
        let raw = w.finish().expect("finish").into_inner();
        let avi_file = avi::open(raw.as_slice()).expect("open avi");
        let avi_container = ContainerFile::Avi(Box::new(avi_file));
        assert_eq!(avi_container.sample_count(0), 2);
        assert_eq!(avi_container.target_sample(0, Tick::ZERO), Some((0, 0)));
        assert_eq!(avi_container.sync_sample_before(0, 1), 0);
        assert_eq!(avi_container.sample_pts(0, 1), 1);
        assert!(avi_container.pts_to_tick(0, 1) > Tick::ZERO);

        let src = open_bytes("test.avi", Arc::from(raw)).expect("recognized").expect("opened");
        assert_eq!(src.info().container, "avi");
        assert_eq!(src.info().audio_streams[0].sample_rate, 48_000);
        let audio = src.audio(0, 512, 48_000).expect("avi audio");
        assert_eq!(audio.channels[0].len(), 512);

        // Audio-only AVI route.
        let mut wa =
            avi::AviWriter::new(std::io::Cursor::new(Vec::new()), vec![avi::StreamSpec::Audio(avi::AudioSpec::pcm(48_000, 1, 16))], avi::MuxOptions::default())
                .expect("audio avi writer");
        wa.write_chunk(0, &pcm_s16le_bytes(4_000, 1024), true).expect("a0");
        let audio_avi =
            open_bytes("audio.avi", Arc::from(wa.finish().expect("finish audio avi").into_inner())).expect("recognized audio avi").expect("opened audio avi");
        assert_eq!(audio_avi.info().kind, MediaKind::AudioOnly);
        assert_eq!(audio_avi.audio(0, 256, 48_000).expect("audio avi read").channels[0].len(), 256);

        // MPEG-TS route and ContainerFile::Mpeg helpers.
        let mut ts = mpegts::mux::Muxer::new(Vec::new(), mpegts::mux::Config::default(), &[mpegts::mux::Stream::new(0x100, mpegts::mux::Codec::H264)])
            .expect("ts muxer");
        ts.write_frame(0, &mpegts::mux::Frame { data: &[0x00, 0x00, 0x00, 0x01, 0x65, 0x88], pts: 90_000, dts: None, key: true }).expect("ts f0");
        ts.write_frame(0, &mpegts::mux::Frame { data: &[0x00, 0x00, 0x00, 0x01, 0x41, 0x9A], pts: 93_000, dts: None, key: false }).expect("ts f1");
        ts.write_frame(0, &mpegts::mux::Frame { data: &[0x00, 0x00, 0x00, 0x01, 0x41, 0x9A], pts: 96_000, dts: None, key: false }).expect("ts f2");
        ts.write_frame(0, &mpegts::mux::Frame { data: &[0x00, 0x00, 0x00, 0x01, 0x41, 0x9A], pts: 99_000, dts: None, key: false }).expect("ts f3");
        let ts_bytes = ts.finish().expect("finish ts");
        let ts_file = mpegts::open(ts_bytes.as_slice()).expect("open ts");
        let ts_container = ContainerFile::Mpeg { file: Box::new(ts_file), origin_pts: 90_000 };
        assert!(ts_container.target_sample(0, Tick::ZERO).is_some());
        let _ = ts_container.sync_sample_before(0, 1);
        assert_eq!(ts_container.sample_pts(0, 0), 90_000);
        assert_eq!(ts_container.pts_to_tick(0, 90_000), Tick::ZERO);
        let ts_src = open_bytes("test.ts", Arc::from(ts_bytes)).expect("recognized ts").expect("opened ts");
        assert_eq!(ts_src.info().container, "MPEG-2 TS");
    }

    fn pcm_s16le_bytes(sample: i16, count: usize) -> Vec<u8> {
        std::iter::repeat_n(sample.to_le_bytes(), count).flatten().collect()
    }

    #[test]
    fn matroska_durationless_lace_variable_pcm_preserves_all_blocks() {
        use std::io::Cursor;

        let mut track = matroska::TrackSpec::new(matroska::TrackKind::Audio, "A_PCM/INT/LIT");
        track.audio = Some((48_000.0, 1, Some(16)));
        let mut mux = matroska::MkvWriter::new(Cursor::new(Vec::new()), vec![track], matroska::MuxOptions::default()).unwrap();
        let blocks = [(8_000i16, 120usize), (-16_000i16, 200usize), (24_000i16, 80usize), (4_000i16, 100usize)];
        let pts_ns = [0i64, 2_000_000, 4_000_000, 8_333_333];
        for ((val, len), pts) in blocks.iter().zip(pts_ns) {
            mux.write_frame(0, pts, true, &pcm_s16le_bytes(*val, *len), None).unwrap();
        }
        let bytes = mux.finish().unwrap().into_inner();
        let mut mkv = matroska::open(bytes.as_slice()).unwrap();
        // Simulate a durationless lace for blocks 0..3 sharing block 0's timestamp.
        let base_pts = mkv.tracks[0].samples[0].pts;
        for (lace, s) in mkv.tracks[0].samples[..3].iter_mut().enumerate() {
            s.pts = base_pts;
            s.duration = 0;
            s.lace = lace as u16;
        }
        let params = mkv.tracks[0].codec_params();
        let file = ContainerFile::Matroska(Box::new(mkv));
        let (mut tracks, _) = open_audio_tracks(&file, &bytes, [(0, 4, params.clone(), 0)]).unwrap();

        let decode = |track: &mut AudioTrackEntry, start: i64, frames: usize| {
            let (preroll, state) = track.decoder_parts();
            decode_audio_range(&file, &bytes, 0, 48_000, 1, preroll, state, start, frames).unwrap()
        };

        let full = decode(&mut tracks[0], 0, 500);
        assert_eq!(full[0].len(), 500);
        // Each block's samples remain intact at their cumulative offsets (0..120, 120..320, 320..400, 400..500).
        assert!((full[0][0] - (8_000.0 / 32_768.0)).abs() < 1e-4);
        assert!((full[0][120] - (-16_000.0 / 32_768.0)).abs() < 1e-4);
        assert!((full[0][320] - (24_000.0 / 32_768.0)).abs() < 1e-4);
        assert!((full[0][400] - (4_000.0 / 32_768.0)).abs() < 1e-4);

        // Chunked sequential reads and a backward seek into the lace must match the full read.
        let (mut chunked_tracks, _) = open_audio_tracks(&file, &bytes, [(0, 4, params, 0)]).unwrap();
        let mut chunked = Vec::new();
        for (start, frames) in [(0, 150), (150, 200), (350, 150)] {
            chunked.extend_from_slice(&decode(&mut chunked_tracks[0], start, frames)[0]);
        }
        assert_eq!(chunked, full[0]);

        let rewound = decode(&mut tracks[0], 200, 250);
        assert_eq!(rewound[0], full[0][200..450]);
    }

    #[test]
    fn narrow_read_includes_overlapping_packet_like_wider_read() {
        use std::io::Cursor;

        let mut track = matroska::TrackSpec::new(matroska::TrackKind::Audio, "A_PCM/INT/LIT");
        track.audio = Some((48_000.0, 1, Some(16)));
        let mut mux = matroska::MkvWriter::new(Cursor::new(Vec::new()), vec![track], matroska::MuxOptions::default()).unwrap();
        mux.write_frame(0, 0, true, &pcm_s16le_bytes(8_000, 1024), None).unwrap();
        mux.write_frame(0, 10_666_667, true, &pcm_s16le_bytes(-16_000, 1024), None).unwrap();
        let bytes = mux.finish().unwrap().into_inner();
        let mkv = matroska::open(bytes.as_slice()).unwrap();
        let params = mkv.tracks[0].codec_params();
        let file = ContainerFile::Matroska(Box::new(mkv));

        let open_with_overlap = || {
            let (mut tracks, _) = open_audio_tracks(&file, &bytes, [(0, 2, params.clone(), 0)]).unwrap();
            tracks[0].decoder_parts().1.audio_starts = AudioStarts::complete(vec![0, 512]);
            tracks
        };
        let decode = |track: &mut AudioTrackEntry, start: i64, frames: usize| {
            let (preroll, state) = track.decoder_parts();
            decode_audio_range(&file, &bytes, 0, 48_000, 1, preroll, state, start, frames).unwrap()
        };

        let mut wide_tracks = open_with_overlap();
        let wide = decode(&mut wide_tracks[0], 0, 1536);

        // Narrow read [0, 600) ends before packet 0's 1024-sample tail, yet must include packet 1 at 512.
        let mut narrow_tracks = open_with_overlap();
        let narrow = decode(&mut narrow_tracks[0], 0, 600);
        assert_eq!(narrow[0], wide[0][..600]);
        assert!((narrow[0][0] - (8_000.0 / 32_768.0)).abs() < 1e-4);
        assert!((narrow[0][512] - (-16_000.0 / 32_768.0)).abs() < 1e-4);

        // Chunked reads and a backward seek over the same interval must also match.
        let mut chunked_tracks = open_with_overlap();
        let mut chunked = Vec::new();
        for (start, frames) in [(0, 300), (300, 300)] {
            chunked.extend_from_slice(&decode(&mut chunked_tracks[0], start, frames)[0]);
        }
        assert_eq!(chunked, wide[0][..600]);

        let rewound = decode(&mut wide_tracks[0], 0, 600);
        assert_eq!(rewound[0], wide[0][..600]);
    }
}
