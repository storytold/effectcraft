//! The OpenEXR reader (#3271): only the chunks a request needs, fetched by the offset table in
//! file order (part by part as the file stores them), decompressed with the `exr` crate's block
//! API on the thread pool the caller runs on (the footage decode threads), and only the
//! requested channels converted, straight into their display-window planes.
//!
//! - **No whole-file read.** A request reads the headers it is given ([`ExrIndex`]) and then
//!   exactly the bytes of the chunks it needs; a multi-part file's other parts are never read.
//! - **Rows and levels.** A request names display rows and a level (every 2^level-th row and
//!   column). With one scan line per chunk (NONE, RLE, ZIPS) a chunk is a line and the lines not
//!   needed are not read or decompressed; with larger chunks (ZIP, PIZ, DWA, tiles) whole chunks
//!   are skipped and the rows within a chunk are only skipped when converting.
//! - **Cancellable** between batches of chunks ([`filmcraft_media::cancel::cancelled`]).
//! - **Bit-identical** to the high-level reader: the same decompression, and each line placed
//!   as `exr_channels`' planes reader placed it (clipped to the display window, the fill value
//!   outside the data window, `u32` samples as `f32`).

use std::io::{Read, Seek, SeekFrom};
use std::ops::Range;
use std::sync::Arc;

use effectcraft_render::reads::FileStamp;
use exr::block::UncompressedBlock;
use exr::block::chunk::Chunk;
use exr::meta::attribute::SampleType;
use rayon::prelude::*;

use crate::exr_index::ExrIndex;

/// A source to read an OpenEXR file from: in-memory bytes or a file on disk.
pub(crate) trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

/// Where an OpenEXR file's bytes come from.
#[derive(Clone, Debug)]
pub(crate) enum ExrSource {
    /// Registered in memory (`MediaPool::add_bytes`, web builds, tests).
    Bytes(Arc<[u8]>),
    /// A file, opened for each read and read in place.
    File(std::path::PathBuf),
    /// A network file read from its verified local copy ([`crate::localize`]): it opens the
    /// copy, but its identity (and every entry made of it) is the original's.
    Localized { original: std::path::PathBuf, copy: std::path::PathBuf, stamp: FileStamp },
}

/// An opened [`ExrSource`].
pub(crate) struct Opened {
    pub(crate) reader: Box<dyn ReadSeek>,
    pub(crate) len: u64,
    pub(crate) stamp: FileStamp,
}

impl ExrSource {
    /// Its identity now (a file's size and modification time), without opening it.
    pub(crate) fn stamp(&self) -> std::io::Result<FileStamp> {
        match self {
            ExrSource::Bytes(_) => Ok(FileStamp::Virtual),
            ExrSource::File(path) | ExrSource::Localized { original: path, .. } => std::fs::metadata(path).map(|m| FileStamp::of_metadata(&m)),
        }
    }

    /// Open it: a file only when it is a regular file (a project can name `/dev/zero` or a FIFO,
    /// #2134), with its identity from the open file.
    pub(crate) fn open(&self) -> std::io::Result<Opened> {
        match self {
            ExrSource::Bytes(b) => Ok(Opened { len: b.len() as u64, reader: Box::new(std::io::Cursor::new(b.clone())), stamp: FileStamp::Virtual }),
            ExrSource::File(path) => {
                #[cfg(test)]
                crate::count_open(path);
                let file = std::fs::File::open(path)?;
                let meta = file.metadata()?;
                if !meta.is_file() {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{} is not a file", path.display())));
                }
                Ok(Opened { len: meta.len(), stamp: FileStamp::of_metadata(&meta), reader: Box::new(file) })
            }
            ExrSource::Localized { original, copy, stamp } => {
                let local = || -> std::io::Result<Opened> {
                    #[cfg(test)]
                    crate::count_open(copy);
                    let file = std::fs::File::open(copy)?;
                    let meta = file.metadata()?;
                    // (the copy must still be complete: its size is the original's)
                    let complete = matches!(stamp, FileStamp::Native { len, .. } if *len == meta.len());
                    if !meta.is_file() || !complete {
                        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{}: not a complete copy", copy.display())));
                    }
                    Ok(Opened { len: meta.len(), stamp: *stamp, reader: Box::new(file) })
                };
                // (a copy evicted or damaged meanwhile: the share)
                local().or_else(|_| ExrSource::File(original.clone()).open())
            }
        }
    }
}

/// What to read: channels (indices into [`ExrIndex::channels`]), display rows, and a level.
#[derive(Clone, Debug)]
pub(crate) struct ReadRequest {
    pub(crate) channels: Vec<usize>,
    /// Rows of the display window (full resolution); `None`: all.
    pub(crate) rows: Option<Range<i64>>,
    /// Every 2^level-th row and column of the display window (0: full resolution).
    pub(crate) level: u32,
    /// The most bytes of planes the read may allocate.
    pub(crate) max_bytes: usize,
}

impl ReadRequest {
    /// Every row of `channels` at full resolution.
    pub(crate) fn full(channels: Vec<usize>) -> ReadRequest {
        ReadRequest { channels, rows: None, level: 0, max_bytes: MAX_DECODE_BYTES }
    }
}

/// What a read did, for measurements and tests.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ReadStats {
    /// Chunks fetched, per part.
    pub(crate) chunks: Vec<usize>,
    /// Bytes of chunks read from the source.
    pub(crate) bytes: u64,
    /// Scan lines decompressed (rows of the chunks fetched).
    pub(crate) lines: usize,
}

/// Why a read gave no planes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ReadError {
    /// A chunk can't be decompressed, or the file doesn't hold it.
    Unreadable(String),
    /// Reading the file failed just now (a seek or read error, no memory for a chunk): another
    /// read may succeed.
    Io(String),
    /// The planes need more memory than one read may allocate (the reason, to show).
    TooLarge(String),
    /// The request was called off.
    Cancelled,
}

/// The most bytes of planes one read allocates (every requested channel is a display-window
/// plane of f32): a header can ask for far more than the file holds.
#[cfg(target_pointer_width = "64")]
pub(crate) const MAX_DECODE_BYTES: usize = 8 << 30;
#[cfg(not(target_pointer_width = "64"))]
pub(crate) const MAX_DECODE_BYTES: usize = 512 << 20;

/// At most this many chunks, or bytes of them, are read before they are decompressed (and the
/// request is checked for being called off). The first batch takes at most [`FIRST_BATCH_BYTES`]
/// and each one after twice the one before, so decoding starts early.
const BATCH_CHUNKS: usize = 256;
const BATCH_BYTES: u64 = 64 << 20;
const FIRST_BATCH_BYTES: u64 = 1 << 20;

/// The raw chunks of a batch: their bytes one after another, and each chunk's part and bytes.
#[derive(Default)]
struct Raw {
    data: Vec<u8>,
    chunks: Vec<(usize, Range<usize>)>,
}

/// Samples of one line placed in a plane.
struct Placed {
    /// The requested channel's position in the request.
    slot: usize,
    /// The first sample's index in the plane.
    start: usize,
    values: Vec<f32>,
}

/// The size of the planes of a read at `level` of a `w` × `h` display window.
pub(crate) fn level_size((w, h): (usize, usize), level: u32) -> (usize, usize) {
    let stride = 1usize << level.min(16);
    (w.div_ceil(stride), h.div_ceil(stride))
}

/// Read `req` from `src` (the file `index` describes): one plane per requested channel, in
/// request order, covering the display window at the request's level (the channel's fill value
/// where the file has no samples or the request no rows).
pub(crate) fn read_planes(src: &mut dyn ReadSeek, index: &ExrIndex, req: &ReadRequest) -> (Result<Vec<Vec<f32>>, ReadError>, ReadStats) {
    let mut stats = ReadStats { chunks: vec![0; index.parts.len()], ..Default::default() };
    let planes = read_into(src, index, req, &mut stats);
    (planes, stats)
}

fn read_into(src: &mut dyn ReadSeek, index: &ExrIndex, req: &ReadRequest, stats: &mut ReadStats) -> Result<Vec<Vec<f32>>, ReadError> {
    let level = req.level.min(16);
    let stride = 1i64 << level;
    let (w, h) = index.size();
    let (ow, oh) = level_size((w, h), level);
    let n = ow.checked_mul(oh).ok_or_else(|| ReadError::Unreadable("display window".into()))?;
    let need = n.saturating_mul(req.channels.len()).saturating_mul(std::mem::size_of::<f32>());
    let most = req.max_bytes.min(MAX_DECODE_BYTES);
    if need > most {
        let gb = |b: usize| b as f64 / f64::from(1u32 << 30);
        return Err(ReadError::TooLarge(format!(
            "its {}×{} OpenEXR channels need {:.1} GB of memory, more than the {:.1} GB one file may use",
            w,
            h,
            gb(need),
            gb(most)
        )));
    }
    // Per part: the plane slot of each of its channels (`None`: not requested).
    let mut slots: Vec<Vec<Option<usize>>> = index.meta.headers.iter().map(|h| vec![None; h.channels.list.len()]).collect();
    let mut planes = Vec::with_capacity(req.channels.len());
    for (slot, id) in req.channels.iter().enumerate() {
        let c = index.channels.get(*id).ok_or_else(|| ReadError::Unreadable("no such channel".into()))?;
        if let Some(s) = slots.get_mut(c.part).and_then(|p| p.get_mut(c.channel)) {
            *s = Some(slot);
        }
        planes.push(vec![c.fill; n]);
    }
    let rows = req.rows.clone().unwrap_or(0..h as i64);
    let (r0, r1) = (rows.start.max(0), rows.end.min(h as i64));
    // A display row is read: in the rows, on the level's grid.
    let wanted = move |ty: i64| ty >= r0 && ty < r1 && ty.rem_euclid(stride) == 0;
    // A chunk holding display rows `a..b` has a row read.
    let any_wanted = move |a: i64, b: i64| {
        let first = a.max(r0);
        let first = first + (stride - first.rem_euclid(stride)) % stride;
        first < b.min(r1)
    };
    // The chunks to read, in file order.
    let mut jobs: Vec<(usize, u64, u64)> = vec![];
    for (pi, part) in index.parts.iter().enumerate() {
        if !slots.get(pi).is_some_and(|s| s.iter().any(Option::is_some)) {
            continue;
        }
        for c in &part.chunks {
            let ty0 = c.y.saturating_add(part.offset.1);
            if !c.largest || !any_wanted(ty0, ty0.saturating_add(c.rows as i64)) {
                continue;
            }
            if c.len == 0 {
                return Err(ReadError::Unreadable("a chunk offset outside the file".into()));
            }
            jobs.push((pi, c.offset, c.len));
        }
    }
    jobs.sort_by_key(|(_, offset, _)| *offset);
    let geometry = Geometry { size: (w, h), out: (ow, oh), stride, wanted: &wanted };
    let mut pos: Option<u64> = None;
    let mut rest = jobs.as_slice();
    let mut raw = Raw::default();
    let mut most = FIRST_BATCH_BYTES;
    loop {
        if filmcraft_media::cancel::cancelled() {
            return Err(ReadError::Cancelled);
        }
        // The next batch is read on this thread while the one read before decompresses and
        // converts on this pool's other threads (a network read overlaps the decoding).
        let (batch, more) = rest.split_at(batch_len(rest, most));
        rest = more;
        most = most.saturating_mul(2).min(BATCH_BYTES);
        let decode = std::mem::take(&mut raw);
        let (next, done) = rayon::join(
            || read_batch(&mut *src, batch, &mut pos, stats),
            || -> Vec<Result<(Vec<Placed>, usize), String>> {
                let data = &decode.data;
                decode
                    .chunks
                    .par_iter()
                    .map(|(pi, r)| chunk_lines(index, *pi, data.get(r.clone()).unwrap_or_default(), slots.get(*pi).map_or(&[][..], Vec::as_slice), &geometry))
                    .collect()
            },
        );
        // Placed plane by plane, the planes in parallel.
        let mut by_slot: Vec<Vec<Placed>> = (0..planes.len()).map(|_| vec![]).collect();
        for d in done {
            let (placed, lines) = d.map_err(ReadError::Unreadable)?;
            stats.lines = stats.lines.saturating_add(lines);
            for p in placed {
                if let Some(s) = by_slot.get_mut(p.slot) {
                    s.push(p);
                }
            }
        }
        planes.par_iter_mut().zip(by_slot).for_each(|(plane, placed)| {
            for p in placed {
                let end = p.start.saturating_add(p.values.len());
                if let Some(dst) = plane.get_mut(p.start..end) {
                    dst.copy_from_slice(&p.values);
                }
            }
        });
        raw = next?;
        if raw.chunks.is_empty() {
            break;
        }
    }
    Ok(planes)
}

/// How many of `jobs` (chunks in file order) the next batch takes: up to [`BATCH_CHUNKS`] chunks
/// or `most` bytes, at least one.
fn batch_len(jobs: &[(usize, u64, u64)], most: u64) -> usize {
    let mut take = 0;
    let mut bytes = 0u64;
    while let Some((_, _, len)) = jobs.get(take) {
        if take > 0 && (take >= BATCH_CHUNKS || bytes.saturating_add(*len) > most) {
            break;
        }
        bytes = bytes.saturating_add(*len);
        take += 1;
    }
    take
}

/// The raw bytes of the chunks `batch` (part, offset, length; in file order): chunks that follow
/// each other in the file are read in one go (one request to a network share, not one per
/// chunk), with one seek per gap (`pos`: where the source stands). A chunk the file ends in
/// keeps the bytes there are.
fn read_batch(src: &mut dyn ReadSeek, batch: &[(usize, u64, u64)], pos: &mut Option<u64>, stats: &mut ReadStats) -> Result<Raw, ReadError> {
    let total = batch.iter().map(|(_, _, len)| *len).fold(0u64, u64::saturating_add);
    let mut raw = Raw::default();
    raw.data.try_reserve_exact(usize::try_from(total).map_err(|_| ReadError::Unreadable("chunk size".into()))?).map_err(|e| ReadError::Io(e.to_string()))?;
    let mut i = 0;
    while let Some((_, start, _)) = batch.get(i) {
        // A run of chunks, each starting where the one before ends.
        let (start, mut end, mut j) = (*start, *start, i);
        while let Some((_, offset, len)) = batch.get(j)
            && *offset == end
        {
            end = offset.saturating_add(*len);
            j += 1;
        }
        if *pos != Some(start) {
            src.seek(SeekFrom::Start(start)).map_err(|e| ReadError::Io(e.to_string()))?;
        }
        let at = raw.data.len();
        (&mut *src).take(end - start).read_to_end(&mut raw.data).map_err(|e| ReadError::Io(e.to_string()))?;
        let got = raw.data.len() - at;
        *pos = Some(start.saturating_add(got as u64));
        for (pi, offset, len) in batch.get(i..j).unwrap_or_default() {
            let a = usize::try_from(offset - start).map_or(raw.data.len(), |o| at.saturating_add(o)).min(raw.data.len());
            let b = usize::try_from(*len).map_or(raw.data.len(), |l| a.saturating_add(l)).min(raw.data.len());
            raw.chunks.push((*pi, a..b));
            if let Some(c) = stats.chunks.get_mut(*pi) {
                *c += 1;
            }
        }
        stats.bytes = stats.bytes.saturating_add(got as u64);
        i = j.max(i + 1);
    }
    Ok(raw)
}

/// The planes' geometry for [`chunk_lines`].
struct Geometry<'w> {
    /// The display window's size, and the planes' (at the level).
    size: (usize, usize),
    out: (usize, usize),
    stride: i64,
    wanted: &'w (dyn Fn(i64) -> bool + Sync),
}

/// Decompress the chunk `buf` of part `pi` and convert the lines of the requested channels
/// (`slots`: each channel's plane slot): the samples to place, and the scan lines decompressed.
/// A chunk that names another part than its offset-table slot's is not placed.
fn chunk_lines(index: &ExrIndex, pi: usize, buf: &[u8], slots: &[Option<usize>], g: &Geometry) -> Result<(Vec<Placed>, usize), String> {
    let meta = &index.meta;
    let mut read = buf;
    let chunk = Chunk::read(&mut read, meta).map_err(|e| e.to_string())?;
    if chunk.layer_index != pi {
        return Ok((vec![], 0));
    }
    let block = UncompressedBlock::decompress_chunk(chunk, meta, false).map_err(|e| e.to_string())?;
    let (Some(header), Some(part)) = (meta.headers.get(pi), index.parts.get(pi)) else { return Ok((vec![], 0)) };
    let lines = block.index.pixel_size.1;
    let ((lw, lh), (w, h)) = (part.layer, g.size);
    let (ow, _) = g.out;
    let mut out = vec![];
    let (mut f16s, mut f32s, mut u32s) = (vec![], vec![], vec![]);
    for line in block.lines(&header.channels) {
        let Some(Some(slot)) = slots.get(line.location.channel).copied() else { continue };
        let Some(kind) = header.channels.list.get(line.location.channel).map(|c| c.sample_type) else { continue };
        let (x0, y, count) = (line.location.position.0, line.location.position.1, line.location.sample_count);
        // The samples of data-window row `y`, columns `x0..x0 + count`, at display row `ty`
        // from display column `tx0`; clipped to the display window.
        if y >= lh || x0.checked_add(count).is_none_or(|end| end > lw) {
            continue;
        }
        let Some(ty) = usize::try_from(y as i64 + part.offset.1).ok().filter(|ty| *ty < h) else { continue };
        if !(g.wanted)(ty as i64) {
            continue;
        }
        let tx0 = x0 as i64 + part.offset.0;
        let skip = usize::try_from(-tx0).unwrap_or(0).min(count);
        let tx = usize::try_from(tx0.max(0)).unwrap_or(usize::MAX);
        let take = (count - skip).min(w.saturating_sub(tx));
        if take == 0 {
            continue;
        }
        let samples: Vec<f32> = match kind {
            SampleType::F16 => {
                f16s.resize(count, exr::prelude::f16::ZERO);
                line.read_samples_into_slice(&mut f16s).map_err(|e| e.to_string())?;
                // (the slice conversion runs the same per-value conversion, in vector steps)
                let mut out = vec![0.0; take];
                if let Some(src) = f16s.get(skip..skip + take) {
                    half::slice::HalfFloatSliceExt::convert_to_f32_slice(src, &mut out);
                }
                out
            }
            SampleType::F32 => {
                f32s.resize(count, 0.0);
                line.read_samples_into_slice(&mut f32s).map_err(|e| e.to_string())?;
                f32s.iter().skip(skip).take(take).copied().collect()
            }
            SampleType::U32 => {
                u32s.resize(count, 0);
                line.read_samples_into_slice(&mut u32s).map_err(|e| e.to_string())?;
                u32s.iter().skip(skip).take(take).map(|s| *s as f32).collect()
            }
        };
        if g.stride == 1 {
            let Some(start) = ty.checked_mul(w).and_then(|row| row.checked_add(tx)) else { continue };
            out.push(Placed { slot, start, values: samples });
            continue;
        }
        // A reduced level: every stride-th column of the row, on the level's grid.
        let stride = g.stride as usize;
        let first = tx.div_ceil(stride) * stride;
        let values: Vec<f32> = samples.iter().enumerate().filter(|(j, _)| (tx + j) % stride == 0).map(|(_, v)| *v).collect();
        if values.is_empty() {
            continue;
        }
        let Some(start) = (ty / stride).checked_mul(ow).and_then(|row| row.checked_add(first / stride)) else { continue };
        out.push(Placed { slot, start, values });
    }
    Ok((out, lines))
}
