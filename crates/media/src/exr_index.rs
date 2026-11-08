//! [`ExrIndex`]: what an OpenEXR file holds, read from its headers and offset tables only
//! (#3271): the parts, every channel by its plane name, where each chunk lies and which rows it
//! holds, the Cryptomatte manifests and the channels its picture is made of.
//!
//! An index is a few kilobytes and is kept per file ([`get`]): the channel popups,
//! `layer.channels`, the Cryptomatte layer lists, import and the planning of a read use it
//! and never decode pixels. The reader ([`crate::exr_read`]) fetches the chunks it lists.
//!
//! Everything is validated before anything is sized by it: the display window
//! ([`crate::exr_channels::MAX_DISPLAY_PIXELS`]), the chunk count (eight bytes of offset table
//! each, so never more than the file can hold), and every offset (inside the chunk area; a chunk
//! with a bad offset is never read).

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex, PoisonError};

use effectcraft_raster::AuxChannels;
use effectcraft_raster::channels3d::BACKGROUND_DEPTH;
use effectcraft_render::reads::FileStamp;
use exr::meta::MetaData;
use exr::meta::attribute::{AttributeValue, IntegerBounds, Text};

use crate::exr_channels::{MAX_DISPLAY_PIXELS, Rgb, channel_name, is_depth, parse_manifest, part_name, picture_names};
use crate::exr_read::{ExrSource, ReadSeek};

/// One channel of the file, in file order (parts in order, each part's channels as its header
/// lists them): the order of [`AuxChannels::channels`] when every channel is read.
#[derive(Clone, Debug)]
pub(crate) struct IndexChannel {
    /// The plane name (`part.channel`, see [`channel_name`]).
    pub(crate) name: String,
    /// Its part and its position in the part's channel list.
    pub(crate) part: usize,
    pub(crate) channel: usize,
    /// The value outside the part's data window (the background depth for depth channels).
    pub(crate) fill: f32,
}

/// One chunk of a part, in offset-table order.
#[derive(Clone, Copy, Debug)]
pub(crate) struct IndexChunk {
    /// Where it starts in the file, and how many bytes to read for it (0: its offset is not
    /// valid, it is never read).
    pub(crate) offset: u64,
    pub(crate) len: u64,
    /// Its first row in the part's data window and how many rows it holds.
    pub(crate) y: i64,
    pub(crate) rows: usize,
    /// Of the full-resolution level (tiled files also store reduced levels).
    pub(crate) largest: bool,
}

/// One part of the file.
#[derive(Clone, Debug)]
pub(crate) struct IndexPart {
    /// The part's data window position in the display window, and the data window's size.
    pub(crate) offset: (i64, i64),
    pub(crate) layer: (usize, usize),
    pub(crate) chunks: Vec<IndexChunk>,
}

/// What an OpenEXR file holds ([`get`]).
pub(crate) struct ExrIndex {
    pub(crate) meta: MetaData,
    /// The file's identity when indexed: a read of another identity indexes it again.
    pub(crate) stamp: FileStamp,
    /// The first part's display window: every plane covers it.
    pub(crate) display: IntegerBounds,
    pub(crate) parts: Vec<IndexPart>,
    pub(crate) channels: Vec<IndexChannel>,
    /// How the `image` crate's decoder would show the file (indices into `channels`).
    pub(crate) rgb: Rgb,
    /// A file the decoder refuses ([`Rgb::None`]): the channels its picture is made of
    /// ([`picture_names`]: red, green, blue and alpha by name) and every channel answering to
    /// one of those names (lookups ignore case).
    pub(crate) picture: Option<(Vec<usize>, [String; 3], Option<String>)>,
    /// The names only (planes empty), manifests and aliases: for the channel lists and for
    /// resolving what effects declare. Never handed to an effect.
    pub(crate) names: Arc<AuxChannels>,
}

impl ExrIndex {
    /// The display window's size.
    pub(crate) fn size(&self) -> (usize, usize) {
        (self.display.size.0, self.display.size.1)
    }

    /// The channels the picture is made of: the image crate's choice ([`Rgb::Planes`]) or the
    /// layered rule's. `None`: the planes can't make it (decode with the `image` crate).
    pub(crate) fn picture_ids(&self) -> Option<Vec<usize>> {
        match self.rgb {
            Rgb::Planes([r, g, b], a) => Some([Some(r), Some(g), Some(b), a].into_iter().flatten().collect()),
            Rgb::None => self.picture.as_ref().map(|(ids, _, _)| ids.clone()),
            Rgb::Unsupported => None,
        }
    }

    /// The channels named `names` (exact plane names, each once, in file order).
    pub(crate) fn ids_named<'a>(&self, names: impl IntoIterator<Item = &'a str>) -> Vec<usize> {
        let names: std::collections::HashSet<&str> = names.into_iter().collect();
        self.channels.iter().enumerate().filter(|(_, c)| names.contains(c.name.as_str())).map(|(i, _)| i).collect()
    }
}

/// Why a file has no index.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IndexError {
    /// Not an OpenEXR file this reader takes (decoders may still try the whole file).
    Unreadable,
    /// An OpenEXR file whose compression can't be decoded (the reason, to show).
    Unsupported(&'static str),
    /// The file can't be opened or read.
    Io,
}

/// Bytes counted as they are read (the offset tables start where the headers end).
struct Counted<'r> {
    inner: &'r mut dyn Read,
    read: u64,
}

impl Read for Counted<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read = self.read.saturating_add(n as u64);
        Ok(n)
    }
}

/// The headers of the file in `src` as the high-level reader takes them (headers read strictly,
/// then validated leniently: the files that opened before open now), checked before anything is
/// sized by them, and the offset tables: the headers, the tables and where the chunks start.
/// Read through one buffer (attribute names are read a byte at a time); the bytes taken from it
/// are counted, so the chunk area's start is exact.
fn headers_and_tables(src: &mut dyn ReadSeek, len: u64) -> Result<(MetaData, exr::meta::OffsetTables, u64), IndexError> {
    let strict = {
        let mut buffered = std::io::BufReader::with_capacity(64 << 10, &mut *src);
        let mut counted = Counted { inner: &mut buffered, read: 0 };
        match MetaData::read_from_buffered(&mut counted, true).and_then(|meta| MetaData::validate(&meta.headers, false).map(|_| meta)) {
            Ok(meta) => {
                let header_end = counted.read;
                check(&meta, len, header_end)?;
                let tables = MetaData::read_offset_tables(&mut exr::io::PeekRead::new(&mut counted), &meta.headers).map_err(|_| IndexError::Unreadable)?;
                Some((meta, tables, counted.read))
            }
            Err(_) => None,
        }
    };
    match strict {
        Some(parsed) => Ok(parsed),
        None => {
            // (an HTJ2K file fails here: say why)
            src.seek(SeekFrom::Start(0)).map_err(|_| IndexError::Io)?;
            match MetaData::read_from_buffered(std::io::BufReader::new(&mut *src), false).ok().as_ref().and_then(unsupported) {
                Some(why) => Err(IndexError::Unsupported(why)),
                None => Err(IndexError::Unreadable),
            }
        }
    }
}

/// Refuse headers this reader doesn't take or that ask for more than the file can hold.
fn check(meta: &MetaData, len: u64, header_end: u64) -> Result<(), IndexError> {
    if let Some(why) = unsupported(meta) {
        return Err(IndexError::Unsupported(why));
    }
    let first = meta.headers.first().ok_or(IndexError::Unreadable)?;
    // Deep data isn't read (the whole file was refused before).
    if meta.headers.iter().any(|h| h.deep) {
        return Err(IndexError::Unreadable);
    }
    let display = first.shared_attributes.display_window;
    // Each plane covers the display window: a 60000² window over a 1×1 data window was a
    // multi-gigabyte allocation per channel (#2141).
    display.size.0.checked_mul(display.size.1).filter(|n| *n > 0 && *n <= MAX_DISPLAY_PIXELS).ok_or(IndexError::Unreadable)?;
    // Eight bytes of offset table per chunk: a header can't claim more chunks than that.
    let chunks: usize = meta.headers.iter().map(|h| h.chunk_count).fold(0, usize::saturating_add);
    if (chunks as u64).saturating_mul(8) > len.saturating_sub(header_end) {
        return Err(IndexError::Unreadable);
    }
    Ok(())
}

/// [`crate::exr_channels::unsupported_compression`] of parsed headers.
pub(crate) fn unsupported(meta: &MetaData) -> Option<&'static str> {
    use exr::compression::Compression;
    meta.headers
        .iter()
        .any(|h| matches!(h.compression, Compression::HTJ2K32 | Compression::HTJ2K256))
        .then_some("OpenEXR HTJ2K compression isn't supported yet: re-save the file with ZIP, PIZ or DWAA compression")
}

/// Index the OpenEXR file in `src` (`len` bytes, identity `stamp`), reading its headers and
/// offset tables only.
pub(crate) fn build(src: &mut dyn ReadSeek, len: u64, stamp: FileStamp) -> Result<ExrIndex, IndexError> {
    src.seek(SeekFrom::Start(0)).map_err(|_| IndexError::Io)?;
    let (meta, tables, chunks_start) = headers_and_tables(&mut *src, len)?;
    let first = meta.headers.first().ok_or(IndexError::Unreadable)?;
    let display = first.shared_attributes.display_window;
    // Where each chunk ends: at the next chunk's start, or at the end of the file.
    let mut starts: Vec<u64> = tables.iter().flatten().copied().filter(|o| *o >= chunks_start && *o < len).collect();
    starts.sort_unstable();
    starts.dedup();
    let end_of = |offset: u64| match starts.binary_search(&offset) {
        Ok(i) => starts.get(i + 1).copied().unwrap_or(len),
        Err(_) => offset,
    };
    let mut parts = Vec::with_capacity(meta.headers.len());
    for (pi, h) in meta.headers.iter().enumerate() {
        let table = tables.get(pi).ok_or(IndexError::Unreadable)?;
        // A chunk is at most its block's bytes plus its own small header (part number, position
        // and sizes): never read more for one.
        let most = (h.max_block_byte_size() as u64).saturating_add(64);
        let mut list = Vec::with_capacity(h.chunk_count);
        for (i, tile) in h.blocks_increasing_y_order().enumerate() {
            let bounds = h.get_absolute_block_pixel_coordinates(tile.location).map_err(|_| IndexError::Unreadable)?;
            let offset = table.get(i).copied().unwrap_or(0);
            let valid = offset >= chunks_start && offset < len;
            let bytes = if valid { end_of(offset).saturating_sub(offset).min(most) } else { 0 };
            list.push(IndexChunk {
                offset,
                len: bytes,
                y: i64::from(bounds.position.1),
                rows: bounds.size.1,
                largest: tile.location.is_largest_resolution_level(),
            });
        }
        let pos = h.own_attributes.layer_position;
        parts.push(IndexPart {
            offset: (i64::from(pos.0) - i64::from(display.position.0), i64::from(pos.1) - i64::from(display.position.1)),
            layer: (h.layer_size.0, h.layer_size.1),
            chunks: list,
        });
    }
    let mut channels = vec![];
    let mut aliases = vec![];
    for (pi, h) in meta.headers.iter().enumerate() {
        let prefix = part_name(h);
        for (ci, ch) in h.channels.list.iter().enumerate() {
            let stored = ch.name.to_string();
            let name = channel_name(prefix.as_deref(), &stored);
            // A stored `layer.channel` name inside a part named otherwise is also found as stored
            // (`Crypto_Material00.R` in part `Crypto_Material`); bare names (`R`) are not.
            if prefix.is_some() && name != stored && stored.contains('.') {
                aliases.push((stored, name.clone()));
            }
            let fill = if is_depth(&name) { BACKGROUND_DEPTH } else { 0.0 };
            channels.push(IndexChannel { name, part: pi, channel: ci, fill });
        }
    }
    let manifests = manifests(&meta);
    let rgb = rgb_of(&meta, &channels);
    let mut names = AuxChannels::new(display.size.0 as u32, display.size.1 as u32, 1.0);
    names.channels = channels.iter().map(|c| (c.name.clone(), vec![])).collect();
    names.manifests = manifests;
    names.aliases = aliases;
    let picture = picture_names(&names).map(|(rgb, a)| {
        // Planes are looked up by name ignoring case, so every channel that could answer is read.
        let ids = channels
            .iter()
            .enumerate()
            .filter(|(_, c)| rgb.iter().chain(&a).any(|k| k.eq_ignore_ascii_case(&c.name)))
            .map(|(i, _)| i)
            .collect();
        (ids, rgb, a)
    });
    Ok(ExrIndex { meta, stamp, display, parts, channels, rgb, picture, names: Arc::new(names) })
}

/// The Cryptomatte manifests of the headers (`cryptomatte/<key>/name` and `…/manifest`), sorted.
fn manifests(meta: &MetaData) -> Vec<(String, Vec<(String, u32)>)> {
    let mut crypto: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    let mut collect = |other: &HashMap<Text, AttributeValue>| {
        for (k, v) in other {
            let key = k.to_string();
            let Some(rest) = key.strip_prefix("cryptomatte/") else { continue };
            let Some((id, field)) = rest.split_once('/') else { continue };
            let AttributeValue::Text(t) = v else { continue };
            let e = crypto.entry(id.to_string()).or_default();
            match field {
                "name" => e.0 = Some(t.to_string()),
                "manifest" => e.1 = Some(t.to_string()),
                _ => {}
            }
        }
    };
    // (the image's attributes are the first part's shared ones, as the decoder reports them)
    if let Some(first) = meta.headers.first() {
        collect(&first.shared_attributes.other);
    }
    for h in meta.headers.iter() {
        collect(&h.own_attributes.other);
    }
    let mut out: Vec<(String, Vec<(String, u32)>)> =
        crypto.into_values().filter_map(|(name, manifest)| Some((name?, parse_manifest(&manifest?)))).collect();
    out.sort();
    out
}

/// How the `image` crate's decoder would show the file: the first part with exact `R`, `G` and
/// `B` channels (alpha `A` when it has one), unless its planes can't stand in for it (a
/// depth-named channel, filled with the background depth outside the data window, or a part
/// with its own display window).
fn rgb_of(meta: &MetaData, channels: &[IndexChannel]) -> Rgb {
    let Some(display) = meta.headers.first().map(|h| h.shared_attributes.display_window) else { return Rgb::None };
    for (pi, h) in meta.headers.iter().enumerate() {
        let find = |c: &str| h.channels.find_index_of_channel(&Text::from(c));
        let (Some(r), Some(g), Some(b)) = (find("R"), find("G"), find("B")) else { continue };
        let id = |ci: usize| channels.iter().position(|c| c.part == pi && c.channel == ci);
        let picked = [Some(r), Some(g), Some(b), find("A")];
        let ids: Vec<Option<usize>> = picked.iter().map(|c| c.and_then(id)).collect();
        let (Some(Some(r)), Some(Some(g)), Some(Some(b))) = (ids.first(), ids.get(1), ids.get(2)) else { return Rgb::Unsupported };
        let a = ids.get(3).copied().flatten();
        let depth = [Some(*r), Some(*g), Some(*b), a].iter().flatten().any(|i| channels.get(*i).is_some_and(|c| is_depth(&c.name)));
        if depth || h.shared_attributes.display_window != display {
            return Rgb::Unsupported;
        }
        return Rgb::Planes([*r, *g, *b], a);
    }
    Rgb::None
}

/// At most this many files are indexed (a few kilobytes each); the oldest goes first.
const INDEXED_FILES: usize = 4096;

/// Indexed files by path.
struct Indexes {
    map: HashMap<String, Arc<ExrIndex>>,
    order: VecDeque<String>,
}

fn indexes() -> &'static Mutex<Indexes> {
    static INDEXES: std::sync::OnceLock<Mutex<Indexes>> = std::sync::OnceLock::new();
    INDEXES.get_or_init(|| Mutex::new(Indexes { map: HashMap::new(), order: VecDeque::new() }))
}

fn lock() -> std::sync::MutexGuard<'static, Indexes> {
    indexes().lock().unwrap_or_else(PoisonError::into_inner)
}

/// Keep `index` as the index of `path`.
pub(crate) fn keep(path: &str, index: Arc<ExrIndex>) {
    let mut ix = lock();
    if ix.map.insert(path.to_string(), index).is_none() {
        ix.order.push_back(path.to_string());
    }
    while ix.map.len() > INDEXED_FILES {
        let Some(old) = ix.order.pop_front() else { break };
        ix.map.remove(&old);
    }
}

/// The index of the file at `path` (read from `source` the first time; kept until
/// [`forget`]). Header-only users take it as it is; a read checks the file's identity and
/// indexes it again when it changed.
pub(crate) fn get(path: &str, source: &ExrSource) -> Result<Arc<ExrIndex>, IndexError> {
    if let Some(ix) = lock().map.get(path).cloned() {
        return Ok(ix);
    }
    let mut opened = source.open().map_err(|e| {
        log::debug!("media: {path}: {e}");
        IndexError::Io
    })?;
    let ix = Arc::new(build(&mut *opened.reader, opened.len, opened.stamp)?);
    keep(path, ix.clone());
    Ok(ix)
}

/// The index of `path` if it is kept.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn cached(path: &str) -> Option<Arc<ExrIndex>> {
    lock().map.get(path).cloned()
}

/// Drop the index of `path` (the file changed on disk).
pub(crate) fn forget(path: &str) {
    let mut ix = lock();
    if ix.map.remove(path).is_some() {
        ix.order.retain(|p| p != path);
    }
}
