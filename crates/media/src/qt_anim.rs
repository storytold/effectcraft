//! QuickTime Animation (`rle `) inside a `.mov`.
//!
//! After Effects' "Millions of Colors+" export is this codec at 32 bits per pixel: 8-bit alpha
//! followed by 8-bit RGB ([Apple QuickTime File Format Specification](https://developer.apple.com/library/archive/documentation/QuickTime/QTFF/QTFFChap3/qtff3.html),
//! the sample-description depth field: 32 means the image has an alpha channel). 24-bit is opaque
//! RGB and 16-bit is RGB555. Paletted depths (1, 2, 4 and 8) are refused rather than guessed.
//!
//! A frame is a big-endian chunk: a 4-byte size, a 2-byte header, then run-length encoded lines
//! (the layout published as the Animation codec data format). Header `0` paints from the first
//! line; a header with bit 3 set names the first line and how many lines change. Each line starts
//! with a skip count stored as one more than the pixels to leave alone (`0` ends the frame). A
//! signed code then copies pixels (`> 0`), repeats one pixel (`< -1`), skips again (`0`) or ends
//! the line (`-1`). Pixels a frame does not write stay as they were, so a later frame can update
//! only the lines that moved.
//!
//! FilmCraft's MOV reader does not know this fourcc, so a file whose video sample is `rle ` is
//! opened here instead. Audio in the same file is not decoded.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use effectcraft_project::{AlphaMode, Footage, FootageKind};
use effectcraft_time::{FrameRate, Tick};
use filmcraft_media::{FrameRequest, MediaInfo, MediaKind, MediaSource, SharedSource};

use crate::{MediaError, Result};

const MAX_EDGE: u32 = 16_384;
const MAX_SAMPLES: usize = 1_000_000;
const MAX_SAMPLE: usize = 64 << 20;

struct Sample {
    offset: u64,
    size: usize,
    /// Start of this sample, in [`Tick`]s.
    start: Tick,
    duration: Tick,
}

struct Movie {
    path: PathBuf,
    width: u32,
    height: u32,
    depth: u16,
    rate: FrameRate,
    samples: Vec<Sample>,
    info: MediaInfo,
    /// The picture after the last decoded sample. Animation frames patch this in place.
    decoded: Mutex<Decoded>,
}

struct Decoded {
    index: Option<usize>,
    rgba: Vec<u8>,
}

/// `Some` when `path` is a QuickTime movie whose video is Animation. `Ok(None)` for anything else.
pub(crate) fn probe(path: &Path) -> Result<Option<Footage>> {
    let Some(movie) = read_movie(path)? else { return Ok(None) };
    Ok(Some(footage_of(path, &movie)))
}

/// Open an Animation movie. `Ok(None)` when the file is not one.
pub(crate) fn open(path: &Path) -> Result<Option<SharedSource>> {
    Ok(read_movie(path)?.map(|m| Arc::new(m) as SharedSource))
}

fn footage_of(path: &Path, movie: &Movie) -> Footage {
    let duration = movie.samples.last().map(|s| Tick(s.start.0.saturating_add(s.duration.0))).unwrap_or(Tick::ZERO);
    Footage {
        path: path.to_string_lossy().into_owned(),
        kind: FootageKind::Video,
        width: movie.width,
        height: movie.height,
        pixel_aspect: 1.0,
        frame_rate: movie.rate,
        native_rate: None,
        duration,
        has_video: true,
        has_audio: false,
        alpha: if movie.depth == 32 { AlphaMode::Straight } else { AlphaMode::Ignore },
        premul_color: [0.0; 3],
        loop_count: 1,
        codec: format!("Animation ({}-bit)", movie.depth),
        missing: false,
        sequence: Vec::new(),
        color_profile: None,
        ..Default::default()
    }
}

impl MediaSource for Movie {
    fn info(&self) -> &MediaInfo {
        &self.info
    }

    fn video_frame(&self, req: FrameRequest) -> filmcraft_media::Result<Arc<filmcraft_frame::VideoFrame>> {
        let index = self.sample_at(req.time).min(self.samples.len().saturating_sub(1));
        let mut guard = self.decoded.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.index != Some(index) {
            let from = match guard.index {
                Some(have) if have < index => have + 1,
                _ => {
                    guard.rgba.clear();
                    0
                }
            };
            if guard.rgba.len() != (self.width as usize).saturating_mul(self.height as usize).saturating_mul(4) {
                let n = (self.width as usize).saturating_mul(self.height as usize).saturating_mul(4);
                guard.rgba = vec![0; n];
            }
            for i in from..=index {
                let sample = read_sample(&self.path, &self.samples[i]).map_err(filmcraft_media::MediaError::Decode)?;
                decode_frame(&sample, self.width, self.height, self.depth, &mut guard.rgba).map_err(filmcraft_media::MediaError::Decode)?;
            }
            guard.index = Some(index);
        }
        let pts = self.samples.get(index).map(|s| filmcraft_time::Tick(s.start.0)).unwrap_or(filmcraft_time::Tick::ZERO);
        Ok(Arc::new(filmcraft_frame::VideoFrame::rgba8(self.width, self.height, guard.rgba.clone()).with_pts(pts)))
    }

    fn audio(&self, _start: i64, frames: usize, sample_rate: u32) -> filmcraft_media::Result<filmcraft_frame::AudioBuffer> {
        Ok(filmcraft_frame::AudioBuffer::silence(sample_rate, 2, frames))
    }
}

impl Movie {
    fn sample_at(&self, time: filmcraft_time::Tick) -> usize {
        let t = time.0.max(0);
        self.samples.iter().position(|s| t < s.start.0.saturating_add(s.duration.0)).unwrap_or(self.samples.len().saturating_sub(1))
    }
}

fn read_movie(path: &Path) -> Result<Option<Movie>> {
    let ext = crate::ext_of(path);
    if ext != "mov" && ext != "mp4" && ext != "m4v" && ext != "qt" {
        return Ok(None);
    }
    let file = std::fs::File::open(path).map_err(|e| MediaError::Io(format!("{}: {e}", path.display())))?;
    let len = file.metadata().map_err(|e| MediaError::Io(format!("{}: {e}", path.display())))?.len();
    let Some(parsed) = parse_animation(&file, len).map_err(|e| MediaError::Decode(format!("{}: {e}", path.display())))? else {
        return Ok(None);
    };
    let (width, height, depth, timescale, samples) = parsed;
    if samples.is_empty() {
        return Err(MediaError::Decode(format!("{}: Animation movie has no frames", path.display())));
    }
    let dur = samples.first().map(|s| s.duration.0).filter(|d| *d > 0).unwrap_or(Tick::from_units(1, 30).0);
    let rate = rate_of(timescale, samples.first().map(|s| s.duration).unwrap_or(Tick(dur)));
    let duration = samples.last().map(|s| filmcraft_time::Tick(s.start.0.saturating_add(s.duration.0))).unwrap_or(filmcraft_time::Tick::ZERO);
    let info = MediaInfo {
        name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        kind: MediaKind::Movie,
        duration,
        video: Some(filmcraft_media::VideoStreamInfo {
            width,
            height,
            frame_rate: filmcraft_time::FrameRate::from_f64(rate.num as f64 / rate.den.max(1) as f64),
            par: (1, 1),
            codec: format!("Animation ({}-bit)", depth),
            pixel_format: if depth == 32 { "ARGB 8-bit".into() } else { format!("RGB {depth}-bit") },
            color: filmcraft_color::ColorInfo::SRGB_FULL,
            has_alpha: depth == 32,
            bitrate: None,
        }),
        audio: None,
        container: "QuickTime".into(),
        start_timecode: None,
        file_size: Some(len),
    };
    Ok(Some(Movie { path: path.to_path_buf(), width, height, depth, rate, samples, info, decoded: Mutex::new(Decoded { index: None, rgba: Vec::new() }) }))
}

fn rate_of(timescale: u32, sample: Tick) -> FrameRate {
    let seconds = sample.seconds();
    if seconds > 0.0 && timescale > 0 {
        FrameRate::from_f64(1.0 / seconds)
    } else {
        FrameRate::FPS_30
    }
}

fn read_at(file: &std::fs::File, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
    }
    #[cfg(windows)]
    {
        let (mut done, mut at) = (0usize, offset);
        while let Some(rest) = buf.get_mut(done..).filter(|r| !r.is_empty()) {
            match std::os::windows::fs::FileExt::seek_read(file, rest, at) {
                Ok(0) => return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "read past end of file")),
                Ok(n) => {
                    done = done.saturating_add(n);
                    at = at.saturating_add(n as u64);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (file, offset, buf);
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "positional read"))
    }
}

fn read_sample(path: &Path, sample: &Sample) -> std::result::Result<Vec<u8>, String> {
    if sample.size > MAX_SAMPLE {
        return Err("Animation frame is larger than 64 MiB".into());
    }
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; sample.size];
    read_at(&file, sample.offset, &mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}

fn parse_animation(file: &std::fs::File, len: u64) -> std::result::Result<Option<(u32, u32, u16, u32, Vec<Sample>)>, String> {
    let Some(moov) = read_moov(file, len) else { return Ok(None) };
    let Some((width, height, depth, timescale, chunks, stsc, sizes, durations)) = parse_moov(&moov)? else { return Ok(None) };
    if depth != 16 && depth != 24 && depth != 32 {
        return Err(format!("Animation at {depth} bits per pixel is paletted; 16, 24 and 32-bit are the depths this reads"));
    }
    if width == 0 || height == 0 || width > MAX_EDGE || height > MAX_EDGE {
        return Err(format!("Animation frame size {width}×{height} is outside 1…{MAX_EDGE}"));
    }
    let samples = layout_samples(timescale, &chunks, &stsc, &sizes, &durations)?;
    Ok(Some((width, height, depth, timescale, samples)))
}

/// The `moov` payload, or `None` when this is not a QuickTime / ISO-BMFF file.
fn read_moov(file: &std::fs::File, len: u64) -> Option<Vec<u8>> {
    let mut pos = 0u64;
    let mut steps = 0u32;
    while pos + 8 <= len && steps < 4096 {
        steps += 1;
        let mut h = [0u8; 16];
        read_at(file, pos, &mut h[..8]).ok()?;
        let mut size = u64::from(u32::from_be_bytes([h[0], h[1], h[2], h[3]]));
        let kind = [h[4], h[5], h[6], h[7]];
        let header = if size == 1 {
            read_at(file, pos + 8, &mut h[8..16]).ok()?;
            size = u64::from_be_bytes([h[8], h[9], h[10], h[11], h[12], h[13], h[14], h[15]]);
            16u64
        } else {
            8u64
        };
        if size == 0 {
            size = len.saturating_sub(pos);
        }
        if size < header || pos.checked_add(size).is_none_or(|end| end > len) {
            return None;
        }
        if kind == *b"moov" {
            if size > (64 << 20) {
                return None;
            }
            let mut buf = vec![0u8; (size - header) as usize];
            read_at(file, pos + header, &mut buf).ok()?;
            return Some(buf);
        }
        pos += size;
    }
    None
}

fn parse_moov(moov: &[u8]) -> std::result::Result<Option<(u32, u32, u16, u32, Vec<u64>, Vec<Stsc>, Vec<usize>, Vec<(u32, u32)>)>, String> {
    let mut video = None;
    for (kind, payload) in boxes(moov) {
        if kind == *b"trak"
            && let Some(v) = parse_trak(payload)?
        {
            video = Some(v);
            break;
        }
    }
    Ok(video)
}

fn parse_trak(trak: &[u8]) -> std::result::Result<Option<(u32, u32, u16, u32, Vec<u64>, Vec<Stsc>, Vec<usize>, Vec<(u32, u32)>)>, String> {
    let mut timescale = 0u32;
    let mut stsd = None;
    let mut stts = None;
    let mut stsc = None;
    let mut stsz = None;
    let mut chunks = None;
    for (kind, payload) in boxes(trak) {
        if kind == *b"mdia" {
            for (k, p) in boxes(payload) {
                if k == *b"mdhd" {
                    timescale = mdhd_timescale(p)?;
                }
                if k == *b"minf" {
                    for (k2, p2) in boxes(p) {
                        if k2 == *b"stbl" {
                            for (k3, p3) in boxes(p2) {
                                match &k3 {
                                    b"stsd" => stsd = Some(p3),
                                    b"stts" => stts = Some(p3),
                                    b"stsc" => stsc = Some(p3),
                                    b"stsz" => stsz = Some(p3),
                                    b"stco" => chunks = Some(chunk_offsets(p3, false)?),
                                    b"co64" => chunks = Some(chunk_offsets(p3, true)?),
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let Some(stsd) = stsd else { return Ok(None) };
    let Some((w, h, depth)) = animation_entry(stsd)? else { return Ok(None) };
    let chunks = chunks.ok_or("Animation track has no chunk offsets")?;
    let stsc = parse_stsc(stsc.ok_or("Animation track has no sample-to-chunk table")?)?;
    let sizes = parse_stsz(stsz.ok_or("Animation track has no sample sizes")?)?;
    let durations = parse_stts(stts.ok_or("Animation track has no time-to-sample table")?)?;
    Ok(Some((w, h, depth, timescale.max(1), chunks, stsc, sizes, durations)))
}

fn mdhd_timescale(p: &[u8]) -> std::result::Result<u32, String> {
    let version = *p.first().ok_or("truncated mdhd")?;
    let at = if version == 1 { 4 + 8 + 8 } else { 4 + 4 + 4 };
    Ok(u32b(p, at).unwrap_or(1).max(1))
}

fn animation_entry(stsd: &[u8]) -> std::result::Result<Option<(u32, u32, u16)>, String> {
    let count = u32b(stsd, 4).unwrap_or(0) as usize;
    let mut at = 8usize;
    for _ in 0..count.min(32) {
        let size = u32b(stsd, at).ok_or("truncated sample description")? as usize;
        if size < 16 || at.checked_add(size).is_none_or(|e| e > stsd.len()) {
            return Err("sample description runs past stsd".into());
        }
        let entry = stsd.get(at..at + size).ok_or("truncated sample description")?;
        let fourcc = entry.get(4..8).ok_or("truncated sample description")?;
        if fourcc == b"rle " {
            // Video sample description: after the 16-byte entry header, width is at byte 32 and
            // height at 34, depth at 82, of the entry (Apple QTFF image description).
            let width = u16b(entry, 32).ok_or("Animation sample description has no width")? as u32;
            let height = u16b(entry, 34).ok_or("Animation sample description has no height")? as u32;
            let depth = u16b(entry, 82).unwrap_or(32);
            return Ok(Some((width, height, depth)));
        }
        at += size;
    }
    Ok(None)
}

struct Stsc {
    first_chunk: u32,
    per_chunk: u32,
}

fn parse_stsc(p: &[u8]) -> std::result::Result<Vec<Stsc>, String> {
    let n = u32b(p, 4).unwrap_or(0) as usize;
    if n > MAX_SAMPLES {
        return Err("sample-to-chunk table is too long".into());
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let at = 8 + i * 12;
        out.push(Stsc { first_chunk: u32b(p, at).unwrap_or(1).max(1), per_chunk: u32b(p, at + 4).unwrap_or(1).max(1) });
    }
    if out.is_empty() {
        out.push(Stsc { first_chunk: 1, per_chunk: 1 });
    }
    Ok(out)
}

fn parse_stsz(p: &[u8]) -> std::result::Result<Vec<usize>, String> {
    let uniform = u32b(p, 4).unwrap_or(0);
    let n = u32b(p, 8).unwrap_or(0) as usize;
    if n > MAX_SAMPLES {
        return Err("too many Animation frames".into());
    }
    if uniform > 0 {
        return Ok(vec![uniform as usize; n]);
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let size = u32b(p, 12 + i * 4).unwrap_or(0) as usize;
        if size > MAX_SAMPLE {
            return Err("an Animation frame is larger than 64 MiB".into());
        }
        out.push(size);
    }
    Ok(out)
}

fn parse_stts(p: &[u8]) -> std::result::Result<Vec<(u32, u32)>, String> {
    let n = u32b(p, 4).unwrap_or(0) as usize;
    if n > MAX_SAMPLES {
        return Err("time-to-sample table is too long".into());
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let at = 8 + i * 8;
        out.push((u32b(p, at).unwrap_or(0), u32b(p, at + 4).unwrap_or(1).max(1)));
    }
    Ok(out)
}

fn chunk_offsets(p: &[u8], wide: bool) -> std::result::Result<Vec<u64>, String> {
    let n = u32b(p, 4).unwrap_or(0) as usize;
    if n > MAX_SAMPLES {
        return Err("too many Animation chunks".into());
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let off = if wide {
            let at = 8 + i * 8;
            u64::from_be_bytes(p.get(at..at + 8).ok_or("truncated co64")?.try_into().map_err(|_| "truncated co64")?)
        } else {
            u64::from(u32b(p, 8 + i * 4).ok_or("truncated stco")?)
        };
        out.push(off);
    }
    Ok(out)
}

fn layout_samples(timescale: u32, chunks: &[u64], stsc: &[Stsc], sizes: &[usize], durations: &[(u32, u32)]) -> std::result::Result<Vec<Sample>, String> {
    let mut sample_chunk = Vec::with_capacity(sizes.len());
    let mut chunk_i = 0usize;
    while chunk_i < chunks.len() && sample_chunk.len() < sizes.len() {
        let per = stsc.iter().rev().find(|e| (e.first_chunk as usize) <= chunk_i + 1).map(|e| e.per_chunk).unwrap_or(1);
        for _ in 0..per {
            if sample_chunk.len() >= sizes.len() {
                break;
            }
            sample_chunk.push(chunk_i);
        }
        chunk_i += 1;
    }
    if sample_chunk.len() != sizes.len() {
        return Err("Animation sample tables do not cover every frame".into());
    }
    let mut cursor = vec![0u64; chunks.len()];
    let mut out = Vec::with_capacity(sizes.len());
    let mut time = 0i64;
    let mut dur_i = 0usize;
    let mut dur_left = durations.first().map(|d| d.0).unwrap_or(0);
    for (n, (size, chunk)) in sizes.iter().zip(sample_chunk).enumerate() {
        let base = *chunks.get(chunk).ok_or("a frame points at a missing chunk")?;
        let off = base.saturating_add(*cursor.get(chunk).ok_or("a frame points at a missing chunk")?);
        if let Some(slot) = cursor.get_mut(chunk) {
            *slot = slot.saturating_add(*size as u64);
        }
        while dur_left == 0 {
            dur_i += 1;
            dur_left = durations.get(dur_i).map(|d| d.0).unwrap_or(1);
        }
        dur_left -= 1;
        let units = durations.get(dur_i).map(|d| d.1).unwrap_or(1);
        let duration = Tick::from_units(units as i64, timescale as i64);
        out.push(Sample { offset: off, size: *size, start: Tick(time), duration });
        time = time.saturating_add(duration.0);
        let _ = n;
    }
    Ok(out)
}

fn boxes(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 8 <= data.len() && out.len() < 100_000 {
        let size32 = u32b(data, pos).unwrap_or(0) as u64;
        let kind = [data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]];
        let (size, header) = if size32 == 1 {
            let wide = data.get(pos + 8..pos + 16).map(|b| u64::from_be_bytes(b.try_into().unwrap_or([0; 8]))).unwrap_or(0);
            (wide, 16usize)
        } else if size32 == 0 {
            (data.len().saturating_sub(pos) as u64, 8)
        } else {
            (size32, 8)
        };
        if size < header as u64 || pos.saturating_add(size as usize) > data.len() {
            break;
        }
        let payload = data.get(pos + header..pos + size as usize).unwrap_or(&[]);
        out.push((kind, payload));
        pos = pos.saturating_add(size as usize);
    }
    out
}

fn u32b(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}
fn u16b(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

/// Paint one Animation chunk onto `rgba` (straight RGBA, top row first). Unwritten pixels stay.
fn decode_frame(sample: &[u8], width: u32, height: u32, depth: u16, rgba: &mut [u8]) -> std::result::Result<(), String> {
    let need = (width as usize).checked_mul(height as usize).and_then(|n| n.checked_mul(4)).ok_or("frame is too large")?;
    if rgba.len() < need {
        return Err("frame buffer is short".into());
    }
    let data = chunk_body(sample);
    if data.len() < 2 {
        return Err("Animation frame is truncated".into());
    }
    let header = u16::from_be_bytes([data[0], data[1]]);
    let (mut line, lines, mut i) = if header & 0x0008 != 0 {
        if data.len() < 10 {
            return Err("Animation frame header is truncated".into());
        }
        let start = u16::from_be_bytes([data[2], data[3]]) as u32;
        let count = u16::from_be_bytes([data[6], data[7]]) as u32;
        (start, count, 10usize)
    } else {
        (0u32, height, 2usize)
    };
    let bpp = match depth {
        16 => 2,
        24 => 3,
        32 => 4,
        _ => return Err(format!("Animation depth {depth} is not supported")),
    };
    for _ in 0..lines {
        if line >= height {
            break;
        }
        let Some(skip) = data.get(i).copied() else { break };
        i += 1;
        if skip == 0 {
            break;
        }
        let mut x = (skip as u32).saturating_sub(1);
        loop {
            let Some(code_b) = data.get(i).copied() else { return Err("Animation line is truncated".into()) };
            i += 1;
            let code = code_b as i8;
            if code == -1 {
                break;
            }
            if code == 0 {
                let Some(more) = data.get(i).copied() else { return Err("Animation skip is truncated".into()) };
                i += 1;
                x = x.saturating_add((more as u32).saturating_sub(1));
                continue;
            }
            let (count, repeat) = if code > 0 { (code as u32, false) } else { ((-(code as i16)) as u32, true) };
            if repeat {
                let px = data.get(i..i + bpp).ok_or("Animation repeat pixel is truncated")?;
                i += bpp;
                for _ in 0..count {
                    if x < width {
                        put(rgba, width, x, line, depth, px);
                    }
                    x = x.saturating_add(1);
                }
            } else {
                for _ in 0..count {
                    let px = data.get(i..i + bpp).ok_or("Animation pixel run is truncated")?;
                    i += bpp;
                    if x < width {
                        put(rgba, width, x, line, depth, px);
                    }
                    x = x.saturating_add(1);
                }
            }
        }
        line = line.saturating_add(1);
    }
    Ok(())
}

fn chunk_body(sample: &[u8]) -> &[u8] {
    if sample.len() >= 6 {
        let size = u32::from_be_bytes([sample[0], sample[1], sample[2], sample[3]]) as usize;
        if size == sample.len() {
            return &sample[4..];
        }
    }
    sample
}

fn put(rgba: &mut [u8], width: u32, x: u32, y: u32, depth: u16, px: &[u8]) {
    let at = (y as usize).saturating_mul(width as usize).saturating_add(x as usize).saturating_mul(4);
    let Some(dst) = rgba.get_mut(at..at + 4) else { return };
    match depth {
        32 => {
            dst[0] = px[1];
            dst[1] = px[2];
            dst[2] = px[3];
            dst[3] = px[0];
        }
        24 => {
            dst[0] = px[0];
            dst[1] = px[1];
            dst[2] = px[2];
            dst[3] = 255;
        }
        16 => {
            let v = u16::from_be_bytes([px[0], px[1]]);
            let expand = |c: u16| {
                let c = (c as u8) & 0x1f;
                (c << 3) | (c >> 2)
            };
            dst[0] = expand((v >> 10) & 0x1f);
            dst[1] = expand((v >> 5) & 0x1f);
            dst[2] = expand(v & 0x1f);
            dst[3] = 255;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be16(n: u16) -> [u8; 2] {
        n.to_be_bytes()
    }

    /// One full-frame 32-bit line: skip nothing, copy `argb` pixels, end the line.
    fn line32(argb: &[[u8; 4]]) -> Vec<u8> {
        let mut o = vec![1u8, argb.len() as u8];
        for p in argb {
            o.extend_from_slice(p);
        }
        o.push(0xff);
        o
    }

    fn chunk(body: &[u8]) -> Vec<u8> {
        let mut c = ((body.len() + 4) as u32).to_be_bytes().to_vec();
        c.extend_from_slice(body);
        c
    }

    fn box_of(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut o = (payload.len() as u32 + 8).to_be_bytes().to_vec();
        o.extend_from_slice(kind);
        o.extend_from_slice(payload);
        o
    }

    fn video_entry(w: u16, h: u16, depth: u16) -> Vec<u8> {
        let mut e = Vec::new();
        e.extend_from_slice(&[0, 0, 0, 0]);
        e.extend_from_slice(b"rle ");
        e.extend_from_slice(&[0; 6]);
        e.extend_from_slice(&be16(1));
        e.extend_from_slice(&be16(0));
        e.extend_from_slice(&be16(0));
        e.extend_from_slice(b"appl");
        e.extend_from_slice(&0u32.to_be_bytes());
        e.extend_from_slice(&0x0000_0400u32.to_be_bytes());
        e.extend_from_slice(&be16(w));
        e.extend_from_slice(&be16(h));
        e.extend_from_slice(&0x0048_0000u32.to_be_bytes());
        e.extend_from_slice(&0x0048_0000u32.to_be_bytes());
        e.extend_from_slice(&0u32.to_be_bytes());
        e.extend_from_slice(&be16(1));
        e.extend_from_slice(&[0; 32]);
        e.extend_from_slice(&be16(depth));
        e.extend_from_slice(&(-1i16).to_be_bytes());
        let n = e.len() as u32;
        e[..4].copy_from_slice(&n.to_be_bytes());
        e
    }

    fn mov(w: u16, h: u16, depth: u16, frames: &[Vec<u8>]) -> Vec<u8> {
        let timescale = 30u32;
        let mut mdat_payload = Vec::new();
        let mut offsets = Vec::new();
        let mut sizes = Vec::new();
        let mdat_data_at = 8 + 8; // ftyp (empty-ish) then mdat header. computed after ftyp.
        let ftyp = box_of(b"ftyp", b"qt  ");
        let data_at = ftyp.len() + 8;
        for frame in frames {
            offsets.push((data_at + mdat_payload.len()) as u32);
            sizes.push(frame.len() as u32);
            mdat_payload.extend_from_slice(frame);
        }
        let mdat = box_of(b"mdat", &mdat_payload);
        let stsd_payload = {
            let entry = video_entry(w, h, depth);
            let mut p = vec![0, 0, 0, 0, 0, 0, 0, 1];
            p.extend(entry);
            p
        };
        let stts = {
            let mut p = vec![0, 0, 0, 0, 0, 0, 0, 1];
            p.extend_from_slice(&(frames.len() as u32).to_be_bytes());
            p.extend_from_slice(&1u32.to_be_bytes());
            box_of(b"stts", &p)
        };
        let stsc = box_of(b"stsc", &[0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1]);
        let stsz = {
            let mut p = vec![0, 0, 0, 0, 0, 0, 0, 0];
            p.extend_from_slice(&(frames.len() as u32).to_be_bytes());
            for s in &sizes {
                p.extend_from_slice(&s.to_be_bytes());
            }
            box_of(b"stsz", &p)
        };
        let stco = {
            let mut p = vec![0, 0, 0, 0];
            p.extend_from_slice(&(offsets.len() as u32).to_be_bytes());
            for o in &offsets {
                p.extend_from_slice(&o.to_be_bytes());
            }
            box_of(b"stco", &p)
        };
        let stbl = {
            let mut p = box_of(b"stsd", &stsd_payload);
            p.extend(stts);
            p.extend(stsc);
            p.extend(stsz);
            p.extend(stco);
            box_of(b"stbl", &p)
        };
        let minf = box_of(b"minf", &stbl);
        let mdhd = {
            let mut p = vec![0; 4];
            p.extend_from_slice(&0u32.to_be_bytes());
            p.extend_from_slice(&0u32.to_be_bytes());
            p.extend_from_slice(&timescale.to_be_bytes());
            p.extend_from_slice(&(frames.len() as u32).to_be_bytes());
            p.extend_from_slice(&be16(0x55c4));
            p.extend_from_slice(&be16(0));
            box_of(b"mdhd", &p)
        };
        let mdia = {
            let mut p = mdhd;
            p.extend(minf);
            box_of(b"mdia", &p)
        };
        let trak = box_of(b"trak", &mdia);
        let moov = box_of(b"moov", &trak);
        let mut file = ftyp;
        file.extend(mdat);
        file.extend(moov);
        let _ = mdat_data_at;
        file
    }

    fn pixel(frame: &filmcraft_frame::VideoFrame, x: usize, y: usize) -> [u8; 4] {
        let filmcraft_frame::PixelData::Rgba8(d) = &frame.data else { panic!("rgba") };
        let i = (y * frame.width as usize + x) * 4;
        [d[i], d[i + 1], d[i + 2], d[i + 3]]
    }

    #[test]
    fn thirty_two_bit_animation_keeps_alpha_and_patches_later_frames() {
        let red = [0x00, 0xff, 0x00, 0x00];
        let blue = [0xff, 0x00, 0x00, 0xff];
        let green = [0xff, 0x00, 0xff, 0x00];
        let mut full = vec![0, 0];
        full.extend(line32(&[red, blue]));
        full.extend(line32(&[green, green]));
        let mut patch = Vec::new();
        patch.extend_from_slice(&be16(0x0008));
        patch.extend_from_slice(&be16(0));
        patch.extend_from_slice(&be16(0));
        patch.extend_from_slice(&be16(1));
        patch.extend_from_slice(&be16(0));
        patch.extend(line32(&[[0xff, 0xff, 0xff, 0xff], [0xff, 0xff, 0xff, 0xff]]));
        let bytes = mov(2, 2, 32, &[chunk(&full), chunk(&patch)]);
        let path = std::env::temp_dir().join(format!("ec-anim-{}.mov", std::process::id()));
        std::fs::write(&path, &bytes).unwrap();
        let footage = probe(&path).unwrap().expect("animation movie");
        assert_eq!(footage.codec, "Animation (32-bit)");
        assert_eq!(footage.alpha, AlphaMode::Straight);
        assert_eq!((footage.width, footage.height), (2, 2));
        let src = open(&path).unwrap().expect("open");
        let a = src.video_frame(FrameRequest::full(filmcraft_time::Tick::ZERO)).unwrap();
        assert_eq!(pixel(&a, 0, 0), [255, 0, 0, 0]);
        assert_eq!(pixel(&a, 1, 0), [0, 0, 255, 255]);
        assert_eq!(pixel(&a, 0, 1), [0, 255, 0, 255]);
        let b = src.video_frame(FrameRequest::full(filmcraft_time::Tick(Tick::from_units(1, 30).0))).unwrap();
        assert_eq!(pixel(&b, 0, 0), [255, 255, 255, 255]);
        assert_eq!(pixel(&b, 0, 1), [0, 255, 0, 255], "the line the second frame does not write stays");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_plain_mov_is_not_claimed() {
        let path = std::env::temp_dir().join(format!("ec-not-anim-{}.mov", std::process::id()));
        std::fs::write(&path, b"not a quicktime file at all!!").unwrap();
        assert!(probe(&path).unwrap().is_none());
        let _ = std::fs::remove_file(&path);
    }
}
