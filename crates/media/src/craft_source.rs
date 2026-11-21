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
//! - **Other containers & standalone audio**:
//!   - Shared (`craft_audio::Decoder` with `decode-native`): standalone `WAV`, `AIFF`, `MP3`/`MP2`/`MP1`,
//!     `AAC` (ADTS), `AC-3`, and `Ogg` (`Opus`).
//!   - Fallback (`filmcraft-codecs`): `MXF`, `AVI`, `MPEG-TS`/`PS`, `Ogg Vorbis`, `FLAC`, `CAF`, or
//!     unsupported WAV/AIFF encodings.

use std::sync::{Arc, Mutex};

use craft_audio::{AudioBuffer, TrackDecoder as AudioTrackDecoder};
use craft_color::{ColorInfo, Matrix, Primaries, Range, Transfer};
use craft_frame::{Chroma, PixelData, Region, VideoFrame};
use craft_media::bitstream::unescape_rbsp;
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

/// Open an in-place ISO-BMFF, Matroska/WebM, or standalone audio source via `craft-media` / `craft-audio`.
/// Returns `None` when the container/codec/audio combination should fall back to `filmcraft-codecs`.
#[cfg(any(unix, windows))]
pub(crate) fn open_reader(name: &str, head: &[u8], reader: &SharedBytes) -> Option<Result<SharedSource>> {
    if looks_like_isobmff(head) {
        return open_isobmff(name, reader.clone());
    }
    if looks_like_matroska(head) {
        return open_matroska(name, reader.clone());
    }
    open_standalone_audio(name, head, reader.clone())
}

/// Open an in-memory ISO-BMFF, Matroska/WebM, or standalone audio source via `craft-media` / `craft-audio`.
pub(crate) fn open_bytes(name: &str, bytes: Arc<[u8]>) -> Option<Result<SharedSource>> {
    if looks_like_isobmff(&bytes) {
        return open_isobmff(name, Arc::new(bytes));
    }
    if looks_like_matroska(&bytes) {
        return open_matroska(name, Arc::new(bytes));
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

    fn audio_packet_is_stamped(&self, track_idx: usize, sample_idx: usize) -> bool {
        match self {
            Self::Isobmff(_) => true,
            Self::Matroska(f) => f.tracks[track_idx].samples.get(sample_idx).is_some_and(|s| s.lace == 0 || s.duration > 0),
        }
    }

    fn audio_seek_info(audio_starts: &[i64], seek_preroll_samples: i64, start: i64) -> (usize, i64) {
        let target = start.saturating_sub(seek_preroll_samples).max(0);
        let sample_idx = audio_starts.partition_point(|&s| s <= target).saturating_sub(1);
        let seek_idx = sample_idx.saturating_sub(AUDIO_SEEK_BACKUP_PACKETS);
        let decoded_pos = audio_starts.get(seek_idx).copied().unwrap_or(0);
        (seek_idx, decoded_pos)
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
        let timing = PacketTime {
            stamp: self.positions[idx],
            stamped: file.audio_packet_is_stamped(track, idx),
            samples: opus_packet_samples(packet).map(i64::from),
        };
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
            let stamps: Vec<i64> = (0..track.samples.len())
                .map(|i| {
                    let pts = track.presentation_pts(i).unwrap_or(0);
                    i64::try_from(i128::from(pts) * rate / ts).unwrap_or(0)
                })
                .collect();
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
    }
}

fn audio_frames_from_ns(ns: i128, rate: u32) -> i128 {
    (ns * i128::from(rate)) / 1_000_000_000
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
            seek_preroll_samples,
            state: Mutex::new(AudioTrackState { decoder: dec, audio_starts, next_sample: 0, decoded_pos: initial_pos, buf: Vec::new() }),
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

struct AudioTrackEntry {
    track_idx: usize,
    native_rate: u32,
    seek_preroll_samples: i64,
    state: Mutex<AudioTrackState>,
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
        let (native_start, native_frames) = native_audio_window(start, frames, native_rate, sample_rate);

        let mut state = entry.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let decoded = decode_audio_range(
            &self.file,
            &*self.source,
            entry.track_idx,
            native_rate,
            channels_count,
            entry.seek_preroll_samples,
            &mut state,
            native_start,
            native_frames,
        )?;

        if sample_rate == native_rate {
            return Ok(AudioBuffer { sample_rate, channels: decoded });
        }

        Ok(AudioBuffer { sample_rate, channels: resample_audio(&decoded, channels_count, native_rate, sample_rate, frames) })
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
        let (native_start, native_frames) = native_audio_window(start, frames, native_rate, sample_rate);

        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let decoded = decode_standalone_range(native_rate, channels_count, &mut state, native_start, native_frames)?;
        if sample_rate == native_rate {
            return Ok(AudioBuffer { sample_rate, channels: decoded });
        }
        Ok(AudioBuffer { sample_rate, channels: resample_audio(&decoded, channels_count, native_rate, sample_rate, frames) })
    }
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
            let off = (pos - *decoded_pos) as usize;
            if off < avail {
                for (c, out_ch) in out.iter_mut().enumerate() {
                    out_ch[i] = buf[c][off];
                }
            }
        }
    }
    let end = start.saturating_add(frames as i64);
    let drop_to = (end - *decoded_pos).max(0) as usize;
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
    if moves_backward || start > buf_end + i64::from(native_rate) * 2 {
        let target = start.saturating_sub(seek_preroll_samples).max(0);
        let (seek_idx, decoded_pos) = if moves_backward && state.next_sample == 0 && state.audio_starts.seek_proves_first_packet(target) {
            (0, state.decoded_pos)
        } else {
            state.audio_starts.finish_for_seek(file, source, track_idx)?;
            ContainerFile::audio_seek_info(&state.audio_starts.positions, seek_preroll_samples, start)
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
    while state.decoded_pos + state.buf.first().map_or(0, Vec::len) as i64 <= end && state.next_sample < sample_count {
        if filmcraft_media::cancel::cancelled() {
            return Err(MediaError::Cancelled);
        }
        let idx = state.next_sample;
        let pk_start = state.audio_starts.start(file, track_idx, idx).unwrap_or_else(|| state.decoded_pos + state.buf.first().map_or(0, Vec::len) as i64);
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

    #[test]
    fn opus_open_and_first_sequential_read_do_not_scan_tail_packets() {
        use std::io::Cursor;

        const RATE: u32 = 48_000;
        const FRAME: usize = 960;
        const PACKETS: usize = 192;
        const GAP_AT: usize = 96;
        const GAP: usize = 9_600;
        const LEAD: usize = 4_800;
        const CODEC_DELAY_NS: i64 = 6_500_000;
        const CODEC_DELAY_FRAMES: i64 = 312;

        let opus = isobmff::OpusConfig { output_channels: 1, pre_skip: CODEC_DELAY_FRAMES as u16, input_sample_rate: RATE, ..Default::default() };
        assert_eq!(opus_packet_samples(OPUS_TONE), Some(FRAME as u32));
        for (matroska_file, lead) in [(false, 0), (true, 0), (false, LEAD), (true, LEAD)] {
            let (bytes, file, offsets, params, delay) = if matroska_file {
                let mut track = matroska::TrackSpec::new(matroska::TrackKind::Audio, "A_OPUS");
                track.audio = Some((f64::from(RATE), 1, None));
                track.codec_private = opus.opus_head();
                track.codec_delay_ns = CODEC_DELAY_NS as u64;
                track.seek_pre_roll_ns = 80_000_000;
                track.default_duration_ns = Some(20_000_000);
                let mut mux = matroska::MkvWriter::new(Cursor::new(Vec::new()), vec![track], matroska::MuxOptions::default()).unwrap();
                for idx in 0..PACKETS {
                    let gap_ns = if idx >= GAP_AT { 200_000_000 } else { 0 };
                    let jitter = if idx % 2 == 0 { 0 } else { 1_000_000 };
                    let lead_ns = if lead == 0 { 0 } else { 100_000_000 };
                    mux.write_frame(0, idx as i64 * 20_000_000 + lead_ns + gap_ns + jitter - CODEC_DELAY_NS, true, OPUS_TONE, None).unwrap();
                }
                let bytes = mux.finish().unwrap().into_inner();
                let file = matroska::open(bytes.as_slice()).unwrap();
                let track = &file.tracks[0];
                let offsets = track.samples.iter().map(|s| s.offset).collect::<Vec<_>>();
                let params = track.codec_params();
                (bytes, ContainerFile::Matroska(Box::new(file)), offsets, params, CODEC_DELAY_NS as u64)
            } else {
                let entry = isobmff::SampleEntry {
                    format: isobmff::FourCc(*b"Opus"),
                    data_reference_index: 1,
                    codec: isobmff::CodecConfig::Opus(opus.clone()),
                    video: None,
                    audio: Some(isobmff::AudioParams { channels: 1, sample_size: 16, sample_rate: f64::from(RATE), ..Default::default() }),
                    bitrate: None,
                };
                let mut mux = isobmff::Mp4Writer::new(Cursor::new(Vec::new()), isobmff::WriterOptions::new(isobmff::Brand::Mp4)).unwrap();
                mux.add_track(isobmff::TrackConfig::new(entry, RATE)).unwrap();
                for idx in 0..PACKETS {
                    let duration = (if idx + 1 == GAP_AT { FRAME + GAP } else { FRAME }) as u32;
                    mux.write_sample(0, isobmff::WriteSample { data: OPUS_TONE, duration, composition_offset: lead as i32, is_sync: true }).unwrap();
                }
                let bytes = mux.finish().unwrap().into_inner();
                let file = isobmff::open(bytes.as_slice()).unwrap();
                let track = &file.tracks[0];
                let offsets = track.samples.iter().map(|s| s.offset).collect::<Vec<_>>();
                let params = track.codec_params();
                (bytes, ContainerFile::Isobmff(file), offsets, params, 0)
            };
            assert_eq!(offsets.len(), PACKETS);
            assert!(offsets.windows(2).all(|w| w[0] < w[1]));
            // Start counting after container metadata indexing. This isolates payload reads by the app
            // opener, independent of how the demuxer peeks at block headers or buffers the file.
            let source = OpusPayloadReads { bytes, offsets, packets: Mutex::new(Vec::new()), fail_once_at: Mutex::new(None) };
            let (mut tracks, _) = open_audio_tracks(&file, &source, [(0, PACKETS, params.clone(), delay)]).unwrap();
            assert_eq!(*source.packets.lock().unwrap(), (0..AUDIO_LOOKAHEAD).collect::<Vec<_>>());

            // Independent eager placement oracle: fixed 20 ms packets, an explicit 200 ms gap,
            // no drift from alternating Matroska stamp jitter, and delay applied once.
            let delay_frames = if matroska_file { CODEC_DELAY_FRAMES } else { 0 };
            let expected_positions: Vec<_> = (0..PACKETS).map(|i| (lead + i * FRAME + if i >= GAP_AT { GAP } else { 0 }) as i64 - delay_frames).collect();
            let first = expected_positions[0];
            assert_eq!(tracks[0].state.get_mut().unwrap().decoded_pos, first);
            let (mut reference, _) = open_audio_tracks(&file, &source.bytes, [(0, PACKETS, params.clone(), delay)]).unwrap();
            reference[0].state.get_mut().unwrap().audio_starts = AudioStarts::complete(expected_positions.clone());

            let decode = |track: &mut AudioTrackEntry, src: &dyn ByteSource, start, frames| {
                decode_audio_range(&file, src, 0, RATE, 1, track.seek_preroll_samples, track.state.get_mut().unwrap(), start, frames).unwrap()
            };
            // A first read before the initial packet can be ordinary leading silence
            // or negative pre-roll. Neither should trigger the cold suffix scan.
            let early_start = if lead == 0 { first - 128 } else { 0 };
            let silent_frames = (first - early_start) as usize;
            let silent = decode(&mut tracks[0], &source, early_start, silent_frames);
            assert!(silent[0].iter().all(|&s| s == 0.0));
            assert_eq!(*source.packets.lock().unwrap(), (0..AUDIO_LOOKAHEAD).collect::<Vec<_>>());
            assert_eq!(silent, decode(&mut reference[0], &source.bytes, early_start, silent_frames));
            let early_frames = silent_frames + 12 * FRAME;
            let early = decode(&mut tracks[0], &source, early_start, early_frames);
            assert_eq!(early, decode(&mut reference[0], &source.bytes, early_start, early_frames));
            assert!(early[0].iter().all(|s| s.is_finite()));
            assert!(early[0].iter().any(|s| s.abs() > 1e-5), "real Opus decoding, not a silent fallback");
            {
                let reads = source.packets.lock().unwrap();
                assert!(reads.iter().any(|&i| i >= AUDIO_LOOKAHEAD), "exercise the incremental frontier");
                assert!(reads.iter().all(|&i| i < 14) && reads.len() <= AUDIO_LOOKAHEAD + 14, "{reads:?}");
            }

            // A backward read after real decode must still use the exact seek index.
            let backward_start = first + 2 * FRAME as i64;
            let backward = decode(&mut tracks[0], &source, backward_start, FRAME);
            assert_eq!(backward, decode(&mut reference[0], &source.bytes, backward_start, FRAME));
            let state = tracks[0].state.get_mut().unwrap();
            assert_eq!(state.audio_starts.positions, expected_positions);
            assert!(state.audio_starts.opus.is_none());

            // A genuinely random first request is not the leading-silence shortcut.
            let (mut fresh, _) = open_audio_tracks(&file, &source, [(0, PACKETS, params.clone(), delay)]).unwrap();
            let (mut eager, _) = open_audio_tracks(&file, &source.bytes, [(0, PACKETS, params, delay)]).unwrap();
            eager[0].state.get_mut().unwrap().audio_starts = AudioStarts::complete(expected_positions.clone());
            let start = (lead + 150 * FRAME + GAP) as i64 - delay_frames;
            let transient_packet = 64;
            *source.fail_once_at.lock().unwrap() = Some(transient_packet);
            {
                let entry = &mut fresh[0];
                let preroll = entry.seek_preroll_samples;
                let state = entry.state.get_mut().unwrap();
                let error = decode_audio_range(&file, &source, 0, RATE, 1, preroll, state, start, 3 * FRAME).unwrap_err();
                assert!(matches!(error, MediaError::Io(_)));
                assert_eq!(state.audio_starts.opus.as_ref().unwrap().len, transient_packet);
                assert_eq!((state.next_sample, state.decoded_pos), (0, first));
            }
            let cold = decode(&mut fresh[0], &source, start, 3 * FRAME);
            assert_eq!(cold, decode(&mut eager[0], &source.bytes, start, 3 * FRAME));
            assert!(cold[0].iter().any(|s| s.abs() > 1e-5));
            let state = fresh[0].state.get_mut().unwrap();
            assert_eq!(state.audio_starts.positions, expected_positions);
            assert!(state.audio_starts.opus.is_none());
        }
    }

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

        let src = open_bytes("tone.wav", Arc::from(wav)).expect("recognized").expect("opened");
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
}
