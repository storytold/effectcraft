//! The OpenEXR reader (#3271): where a file's bytes come from ([`ExrSource`], its identity and
//! how it is opened) and the app's side of a read ([`ReadRequest`] of the index's channels, the
//! memory one read may take, cancellation and the errors shown). The reading itself is
//! `craft-exr`'s ([`craft_exr::seek`]):
//!
//! - **No whole-file read.** A request reads the headers it is given ([`ExrIndex`]) and then
//!   exactly the bytes of the chunks it needs; a multi-part file's other parts are never read.
//! - **Rows and levels.** A request names display rows and a level (every 2^level-th row and
//!   column). With one scan line per chunk (NONE, RLE, ZIPS) a chunk is a line and the lines not
//!   needed are not read or decompressed; with larger chunks (ZIP, PIZ, DWA, tiles) whole chunks
//!   are skipped and the rows within a chunk are only skipped when converting.
//! - **Cancellable** while chunks are read ([`filmcraft_media::cancel::cancelled`]).
//! - **Bit-identical** to the high-level reader: the same decompression, each line clipped to
//!   the display window, the fill value outside the data window, `u32` samples as `f32`.

use std::io::{Read, Seek};
use std::ops::Range;
use std::sync::Arc;

use craft_exr::seek;
use effectcraft_render::reads::FileStamp;

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
    pub(crate) stamp: FileStamp,
}

/// How long a file's identity, once read, stands for it ([`recent_stamp`], #3334): every request
/// for a file's channels checked it, one or two `fs::metadata` calls each (1–7 ms on a network
/// share), several per frame. A file rewritten in place is seen at most this late; Reload Footage
/// forgets it at once ([`forget_stamp`]).
pub(crate) const STAMP_TTL: std::time::Duration = std::time::Duration::from_secs(1);

/// At most this many files' identities are remembered (the oldest checks go first).
#[cfg(not(target_arch = "wasm32"))]
const STAMPS_KEPT: usize = 4096;

#[cfg(not(target_arch = "wasm32"))]
type Stamps = std::collections::HashMap<std::path::PathBuf, (FileStamp, std::time::Instant)>;

#[cfg(not(target_arch = "wasm32"))]
fn stamps() -> std::sync::MutexGuard<'static, Stamps> {
    static STAMPS: std::sync::OnceLock<std::sync::Mutex<Stamps>> = std::sync::OnceLock::new();
    STAMPS.get_or_init(Default::default).lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// [`recent_stamp`] in the browser: no clock there (and no files, only registered bytes).
#[cfg(target_arch = "wasm32")]
pub(crate) fn recent_stamp(path: &std::path::Path) -> std::io::Result<FileStamp> {
    std::fs::metadata(path).map(|m| FileStamp::of_metadata(&m))
}

/// The identity of the file at `path` (size and modification time): as read within the last
/// [`STAMP_TTL`], else read now. A file that can't be read is asked again every time.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn recent_stamp(path: &std::path::Path) -> std::io::Result<FileStamp> {
    let now = std::time::Instant::now();
    if let Some((stamp, at)) = stamps().get(path)
        && now.saturating_duration_since(*at) < STAMP_TTL
    {
        return Ok(*stamp);
    }
    #[cfg(test)]
    crate::count_stamp(path);
    let stamp = std::fs::metadata(path).map(|m| FileStamp::of_metadata(&m))?;
    note_stamp(path, stamp, now);
    Ok(stamp)
}

/// Remember `stamp` as the identity of the file at `path` read at `at` (a file just opened).
#[cfg(not(target_arch = "wasm32"))]
fn note_stamp(path: &std::path::Path, stamp: FileStamp, at: std::time::Instant) {
    let mut s = stamps();
    if s.len() >= STAMPS_KEPT && !s.contains_key(path) {
        s.retain(|_, (_, t)| at.saturating_duration_since(*t) < STAMP_TTL);
        if s.len() >= STAMPS_KEPT {
            s.clear();
        }
    }
    s.insert(path.to_path_buf(), (stamp, at));
}

/// Forget the identity read of the file at `path` (Reload Footage: the next request reads it).
pub(crate) fn forget_stamp(path: &std::path::Path) {
    #[cfg(not(target_arch = "wasm32"))]
    stamps().remove(path);
    #[cfg(target_arch = "wasm32")]
    let _ = path;
}

/// Whether the identity of the file at `path` is remembered (tests).
#[cfg(test)]
pub(crate) fn stamp_kept(path: &std::path::Path) -> bool {
    stamps().contains_key(path)
}

impl ExrSource {
    /// Its identity (a file's size and modification time), without opening it: a file's as read
    /// within the last [`STAMP_TTL`] ([`recent_stamp`]); a localized file's as it was when its
    /// verified copy was chosen (moments ago: [`recent_stamp`] too).
    pub(crate) fn stamp(&self) -> std::io::Result<FileStamp> {
        match self {
            ExrSource::Bytes(_) => Ok(FileStamp::Virtual),
            ExrSource::File(path) => recent_stamp(path),
            ExrSource::Localized { stamp, .. } => Ok(*stamp),
        }
    }

    /// The file was just opened and found to be `stamp`: later checks within [`STAMP_TTL`] use it.
    pub(crate) fn opened_as(&self, stamp: FileStamp) {
        #[cfg(not(target_arch = "wasm32"))]
        if let ExrSource::File(path) = self {
            note_stamp(path, stamp, std::time::Instant::now());
        }
        #[cfg(target_arch = "wasm32")]
        let _ = stamp;
    }

    /// Open it: a file only when it is a regular file (a project can name `/dev/zero` or a FIFO,
    /// #2134), with its identity from the open file.
    pub(crate) fn open(&self) -> std::io::Result<Opened> {
        match self {
            ExrSource::Bytes(b) => Ok(Opened { reader: Box::new(std::io::Cursor::new(b.clone())), stamp: FileStamp::Virtual }),
            ExrSource::File(path) => {
                #[cfg(test)]
                crate::count_open(path);
                let file = std::fs::File::open(path)?;
                let meta = file.metadata()?;
                if !meta.is_file() {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{} is not a file", path.display())));
                }
                Ok(Opened { stamp: FileStamp::of_metadata(&meta), reader: Box::new(file) })
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
                    Ok(Opened { stamp: *stamp, reader: Box::new(file) })
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
    /// The most bytes of planes the read may allocate (the caller's own limit; `usize::MAX`: only
    /// the system's, [`effectcraft_render::memory::decode_limit`]).
    pub(crate) max_bytes: usize,
    /// The fixed limit the system's memory may raise ([`MAX_DECODE_BYTES`]).
    pub(crate) floor: usize,
}

impl ReadRequest {
    /// Every row of `channels` at full resolution.
    pub(crate) fn full(channels: Vec<usize>) -> ReadRequest {
        ReadRequest { channels, rows: None, level: 0, max_bytes: usize::MAX, floor: MAX_DECODE_BYTES }
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
    /// The system refused the memory the planes need just now (the reason, to show): not a file
    /// too large, but not to be asked again at once either (#3749).
    OutOfMemory(String),
    /// The request was called off.
    Cancelled,
}

/// The most bytes of planes one read allocates at least (every requested channel is a
/// display-window plane of f32): a header can ask for far more than the file holds, and a file's
/// size on disk says nothing about it. With more memory in the machine the limit is higher
/// ([`effectcraft_render::memory::decode_limit`], #3749).
#[cfg(target_pointer_width = "64")]
pub(crate) const MAX_DECODE_BYTES: usize = 8 << 30;
#[cfg(not(target_pointer_width = "64"))]
pub(crate) const MAX_DECODE_BYTES: usize = 512 << 20;

/// What a failed `craft-exr` read means here. Memory the system refused just now is
/// [`ReadError::OutOfMemory`]: the plane store doesn't remember it (another request may
/// succeed), but doesn't read it again at once nor fall back to decoding the whole file
/// either; a file too large is remembered until the file or the limit changes.
pub(crate) fn read_error(e: seek::Error) -> ReadError {
    match e {
        seek::Error::Cancelled => ReadError::Cancelled,
        seek::Error::Io(why) => ReadError::Io(why),
        seek::Error::OutOfMemory(why) => ReadError::OutOfMemory(format!("its OpenEXR channels need more memory than the system can give just now ({why})")),
        seek::Error::LimitExceeded(why) => ReadError::TooLarge(format!("its OpenEXR channels need more memory than one file may use ({why})")),
        seek::Error::Malformed(why) | seek::Error::Unsupported(why) | seek::Error::UnsupportedCompression(why) => ReadError::Unreadable(why),
        other => ReadError::Unreadable(other.to_string()),
    }
}

/// The most bytes one decompressed block of a read may take, whatever the read's limit
/// ([`MAX_DECODE_BYTES`]: 8 GiB, 512 MiB in the browser): the decompressor allocates a block
/// whole before it looks at the data, and a block is one strip of the file, so a larger one is a
/// damaged or hostile header, not a file with more channels (#3749). A refusal for it doesn't
/// depend on the limit: a request asking at least as much, refused for it before, would be again.
pub(crate) const MAX_BLOCK_BYTES: usize = MAX_DECODE_BYTES;

/// The most threads one read decompresses on (beside the caller's), and the size of a file
/// whose chunks are decompressed on the calling thread alone.
const READ_WORKERS: usize = 4;
const SMALL_FILE: u64 = 1 << 20;

/// The size of the planes of a read at `level` of a `w` × `h` display window.
#[cfg(test)]
pub(crate) fn level_size(size: (usize, usize), level: u32) -> (usize, usize) {
    seek::level_size(size, level)
}

/// Read `req` from `src` (the file `index` describes): one plane per requested channel, in
/// request order, covering the display window at the request's level (the channel's fill value
/// where the file has no samples or the request no rows). The stats are a failed read's
/// defaults: `craft-exr` reports none for it.
pub(crate) fn read_planes(src: &mut dyn ReadSeek, index: &ExrIndex, req: &ReadRequest) -> (Result<Vec<Vec<f32>>, ReadError>, ReadStats) {
    match read(src, index, req) {
        Ok((planes, stats)) => (Ok(planes), stats),
        Err(e) => (Err(e), ReadStats { chunks: vec![0; index.file.parts().len()], ..Default::default() }),
    }
}

fn gb(bytes: usize) -> f64 {
    bytes as f64 / f64::from(1u32 << 30)
}

fn too_large(w: usize, h: usize, need: usize, most: usize) -> ReadError {
    ReadError::TooLarge(format!("its {}×{} OpenEXR channels need {:.1} GB of memory, more than the {:.1} GB one file may use", w, h, gb(need), gb(most)))
}

/// Refuse a request whose planes together, those already in hand (held in memory or read back
/// from the disk cache) and those still to read, `planes` of a `w` × `h` display window, need
/// more memory than one file may use: the limit is for what a file's request ends with, not
/// for the part of it read just now ([`effectcraft_render::memory::decode_limit`], #3749).
pub(crate) fn check_total(w: usize, h: usize, planes: usize, max_bytes: usize) -> Result<(), ReadError> {
    let need = w.saturating_mul(h).saturating_mul(planes).saturating_mul(std::mem::size_of::<f32>());
    let most = max_bytes.min(effectcraft_render::memory::decode_limit(MAX_DECODE_BYTES, need));
    if need > most {
        return Err(too_large(w, h, need, most));
    }
    Ok(())
}

fn read(src: &mut dyn ReadSeek, index: &ExrIndex, req: &ReadRequest) -> Result<(Vec<Vec<f32>>, ReadStats), ReadError> {
    let (w, h) = index.size();
    let (ow, oh) = seek::level_size((w, h), req.level.min(16));
    let n = ow.checked_mul(oh).ok_or_else(|| ReadError::Unreadable("display window".into()))?;
    let need = n.saturating_mul(req.channels.len()).saturating_mul(std::mem::size_of::<f32>());
    // (the limit is decided here and handed to `craft-exr` as the read's `max_bytes`: the planes
    // it reserves, fallibly, follow it; one decompressed block stays at the fixed floor however
    // much memory the machine has, see `MAX_BLOCK_BYTES`)
    let most = req.max_bytes.min(effectcraft_render::memory::decode_limit(req.floor, need));
    if need > most {
        return Err(too_large(w, h, need, most));
    }
    let mut channels = Vec::with_capacity(req.channels.len());
    let mut fill = Vec::with_capacity(req.channels.len());
    for id in &req.channels {
        let c = index.channels.get(*id).ok_or_else(|| ReadError::Unreadable("no such channel".into()))?;
        channels.push((c.part, c.channel));
        fill.push(c.fill);
    }
    let mut request = seek::Request::new(&channels, most);
    request.fill = &fill;
    request.rows = req.rows.clone();
    request.level = req.level;
    request.max_block_bytes = Some(MAX_BLOCK_BYTES);
    // craft-exr decompresses on threads of its own, started for this read, beside the decode
    // pool the caller runs on: a few at most, and none for a small file (its chunks are
    // decompressed on the calling thread).
    request.workers = Some(if index.file.file_len() < SMALL_FILE { 0 } else { rayon::current_num_threads().min(READ_WORKERS) });
    let planes = seek::read_planes(src, &index.file, &request, &filmcraft_media::cancel::cancelled).map_err(read_error)?;
    let stats = ReadStats { chunks: planes.stats.chunks, bytes: planes.stats.bytes, lines: planes.stats.lines };
    Ok((planes.planes, stats))
}
