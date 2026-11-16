//! [`ExrIndex`]: what an OpenEXR file holds, read from its headers and offset tables only
//! (#3271): the parts, every channel by its plane name, the Cryptomatte manifests and the
//! channels its picture is made of.
//!
//! The headers and offset tables are read and checked by `craft-exr`
//! ([`craft_exr::seek::Index`]), which also knows where each chunk lies; this index adds the
//! app's view of the file: the plane names and aliases, the manifests, the picture's channels and
//! the file's identity. An index is a few kilobytes and is kept per file ([`get`]): the channel
//! popups, `layer.channels`, the Cryptomatte layer lists, import and the planning of a read use
//! it and never decode pixels. The reader ([`crate::exr_read`]) fetches the chunks it lists.
//!
//! Everything is validated before anything is sized by it: the display window
//! ([`crate::exr_channels::MAX_DISPLAY_PIXELS`]), the data windows (four times that), the blocks,
//! the chunk count (eight bytes of offset table each, so never more than the file can hold), and
//! every offset (inside the chunk area; a chunk with a bad offset is never read).

use std::collections::{HashMap, VecDeque};
use std::io::SeekFrom;
use std::sync::{Arc, Mutex, PoisonError};

use craft_exr::channels::ColorLayout;
use craft_exr::decode::{Limits, PartInfo, Window};
use craft_exr::seek;
use effectcraft_raster::AuxChannels;
use effectcraft_raster::channels3d::BACKGROUND_DEPTH;
use effectcraft_render::reads::FileStamp;

use crate::exr_channels::{MAX_DISPLAY_PIXELS, channel_name, is_depth, parse_manifest, part_name};
use crate::exr_read::{ExrSource, Opened, ReadSeek};

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

/// A file's main picture: its channels (indices into [`ExrIndex::channels`]) in the layout's
/// order, `R`, `G`, `B` (`A`) or `Y` (`A`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MainPicture {
    pub(crate) layout: ColorLayout,
    pub(crate) ids: Vec<usize>,
}

impl MainPicture {
    /// The red, green, blue and alpha channels (a `Y` picture is grey: `Y` three times).
    pub(crate) fn rgba(&self) -> Option<([usize; 3], Option<usize>)> {
        let id = |i: usize| self.ids.get(i).copied();
        match self.layout {
            ColorLayout::Rgb | ColorLayout::Rgba => Some(([id(0)?, id(1)?, id(2)?], id(3))),
            ColorLayout::Y | ColorLayout::YA => Some(([id(0)?; 3], id(1))),
        }
    }

    /// It has an alpha channel.
    pub(crate) fn has_alpha(&self) -> bool {
        matches!(self.layout, ColorLayout::Rgba | ColorLayout::YA)
    }
}

/// What an OpenEXR file holds ([`get`]).
pub(crate) struct ExrIndex {
    /// The headers and chunk tables, as `craft-exr` reads them.
    pub(crate) file: seek::Index,
    /// The file's identity when indexed: a read of another identity indexes it again.
    pub(crate) stamp: FileStamp,
    /// The first part's display window: every plane covers it.
    pub(crate) display: Window,
    pub(crate) channels: Vec<IndexChannel>,
    /// The file's main picture ([`craft_exr::channels::main_picture`]), as After Effects shows
    /// it: `None` for a file of named layers only (it shows black; effects reach its layers by
    /// name).
    pub(crate) picture: Option<MainPicture>,
    /// The names only (planes empty), manifests and aliases: for the channel lists and for
    /// resolving what effects declare. Never handed to an effect.
    pub(crate) names: Arc<AuxChannels>,
}

impl ExrIndex {
    /// The display window's size.
    pub(crate) fn size(&self) -> (usize, usize) {
        (self.display.width as usize, self.display.height as usize)
    }

    /// The channels the picture is made of, in file order (none for a file without a main
    /// picture).
    pub(crate) fn picture_ids(&self) -> Vec<usize> {
        let mut ids = self.picture.as_ref().map(|p| p.ids.clone()).unwrap_or_default();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Every channel but the picture's (what [`crate::exr_planes`] means by all channels: the
    /// picture is kept converted, by the media pool, not as planes too).
    pub(crate) fn aux_ids(&self) -> Vec<usize> {
        let picture = self.picture_ids();
        (0..self.channels.len()).filter(|i| !picture.contains(i)).collect()
    }

    /// The channels named `names` (exact plane names, each once, in file order).
    pub(crate) fn ids_named<'a>(&self, names: impl IntoIterator<Item = &'a str>) -> Vec<usize> {
        let names: std::collections::HashSet<&str> = names.into_iter().collect();
        self.channels.iter().enumerate().filter(|(_, c)| names.contains(c.name.as_str())).map(|(i, _)| i).collect()
    }

    /// About the bytes it holds: `craft-exr`'s index (its chunks and header facts), the channels
    /// and the manifests.
    pub(crate) fn bytes(&self) -> usize {
        let names: usize = self.channels.iter().map(|c| c.name.len() * 2 + 64).sum();
        let manifests: usize = self.names.manifests.iter().map(|(l, m)| l.len() + m.iter().map(|(n, _)| n.len() + 32).sum::<usize>()).sum();
        self.file.bytes().saturating_add(names).saturating_add(manifests.saturating_mul(2))
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

/// Why an HTJ2K file (OpenEXR 3.4's High-Throughput JPEG 2000) isn't read.
pub(crate) const HTJ2K: &str = "OpenEXR HTJ2K compression isn't supported yet: re-save the file with ZIP, PIZ or DWAA compression";

/// The most pixels of a part's data window (the display window's limit, four times over: data
/// windows may reach past it).
const MAX_DATA_PIXELS: usize = 4 * MAX_DISPLAY_PIXELS;

/// What `craft-exr` may index: data windows of at most [`MAX_DATA_PIXELS`], of any width or
/// height (the display window is checked against [`MAX_DISPLAY_PIXELS`] in [`build`]).
/// `craft-exr` bounds every block itself (256 MiB uncompressed).
fn limits() -> Limits {
    let mut limits = Limits::none();
    limits.max_pixels = MAX_DATA_PIXELS as u64;
    limits
}

/// The headers and offset tables of the OpenEXR file in `src`, read and checked by `craft-exr`.
fn index_file(src: &mut dyn ReadSeek) -> Result<seek::Index, seek::Error> {
    src.seek(SeekFrom::Start(0)).map_err(|e| seek::Error::Io(e.to_string()))?;
    // (an index is shared by every request for the file: building one is never called off)
    seek::Index::build(src, &limits(), &|| false)
}

/// The headers of the OpenEXR `bytes` are safe to hand to a decoder: they can be read and no
/// part's data window or block is larger than this reader takes (#3271 review). A file this
/// reader doesn't cover (deep parts, HTJ2K) is left to the decoder; damaged headers, a part too
/// large and a failed read are refused.
pub(crate) fn safe_to_decode(bytes: &[u8]) -> bool {
    matches!(index_file(&mut std::io::Cursor::new(bytes)), Ok(_) | Err(seek::Error::Unsupported(_) | seek::Error::UnsupportedCompression(_)))
}

/// The channel list of `part` as the `exr` crate's validation took it (#3271): sorted by name,
/// every sampling factor non-zero and dividing the data window's position and size. The headers
/// are read leniently, and the crate's decompressors divide by the sampling of every channel of
/// a chunk, requested or not (where panics abort, in the browser, that would end the app).
fn channels_valid(part: &PartInfo) -> bool {
    let w = part.data_window;
    let divides = |s: usize, pos: i32, size: u32| i64::try_from(s).ok().filter(|s| *s > 0).is_some_and(|s| i64::from(pos) % s == 0 && i64::from(size) % s == 0);
    let sorted = part.channels.windows(2).all(|pair| pair.first().zip(pair.get(1)).is_some_and(|(a, b)| a.name.as_bytes() <= b.name.as_bytes()));
    sorted && part.channels.iter().all(|c| divides(c.sampling.0, w.x, w.width) && divides(c.sampling.1, w.y, w.height))
}

/// Index the OpenEXR file in `src` (identity `stamp`), reading its headers and offset tables
/// only.
pub(crate) fn build(src: &mut dyn ReadSeek, stamp: FileStamp) -> Result<ExrIndex, IndexError> {
    let file = index_file(src).map_err(|e| match e {
        seek::Error::UnsupportedCompression(name) if name.contains("HTJ2K") => IndexError::Unsupported(HTJ2K),
        seek::Error::Io(_) => IndexError::Io,
        _ => IndexError::Unreadable,
    })?;
    let parts = file.parts();
    // Deep data isn't read (the whole file was refused before).
    if parts.iter().any(|p| p.deep || !channels_valid(p)) {
        return Err(IndexError::Unreadable);
    }
    let display = file.display();
    // Each plane covers the display window: a 60000² window over a 1×1 data window was a
    // multi-gigabyte allocation per channel (#2141).
    (display.width as usize).checked_mul(display.height as usize).filter(|n| *n > 0 && *n <= MAX_DISPLAY_PIXELS).ok_or(IndexError::Unreadable)?;
    let manifests = manifests(parts);
    let mut channels = vec![];
    let mut aliases = vec![];
    for (pi, p) in parts.iter().enumerate() {
        let prefix = part_name(p);
        for (ci, ch) in p.channels.iter().enumerate() {
            let stored = ch.name.clone();
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
    let picture = main_picture(parts, &channels);
    // The picture's channels are colour (0 outside the data window, as After Effects and the
    // `image` crate show it), even in a part whose name reads as depth (`depth.R`).
    for id in picture.iter().flat_map(|p| &p.ids) {
        if let Some(c) = channels.get_mut(*id) {
            c.fill = 0.0;
        }
    }
    let mut names = AuxChannels::new(display.width, display.height, 1.0);
    names.channels = channels.iter().map(|c| (c.name.clone(), vec![])).collect();
    names.manifests = manifests;
    names.aliases = aliases;
    Ok(ExrIndex { file, stamp, display, channels, picture, names: Arc::new(names) })
}

/// The Cryptomatte manifests of the headers (`cryptomatte/<key>/name` and `…/manifest`, in any
/// part), sorted.
fn manifests(parts: &[PartInfo]) -> Vec<(String, Vec<(String, u32)>)> {
    let mut crypto: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    for (key, value) in parts.iter().flat_map(|p| &p.text_attributes) {
        let Some(rest) = key.strip_prefix("cryptomatte/") else { continue };
        let Some((id, field)) = rest.split_once('/') else { continue };
        let e = crypto.entry(id.to_string()).or_default();
        match field {
            "name" => e.0 = Some(value.clone()),
            "manifest" => e.1 = Some(value.clone()),
            _ => {}
        }
    }
    let mut out: Vec<(String, Vec<(String, u32)>)> = crypto.into_values().filter_map(|(name, manifest)| Some((name?, parse_manifest(&manifest?)))).collect();
    out.sort();
    out
}

/// The file's main picture by the OpenEXR convention, as After Effects shows it
/// ([`craft_exr::channels::main_picture`]: the unprefixed `R`, `G`, `B` (`A`) of the first part
/// that has them, else the first unprefixed `Y` (`A`)), as channel ids. The parts' channels are
/// matched as stored (a part's own `R`, not the `part.R` plane name).
fn main_picture(parts: &[PartInfo], channels: &[IndexChannel]) -> Option<MainPicture> {
    let (part, layout, picked) = craft_exr::channels::main_picture(parts.iter().map(|p| p.channels.iter().map(|c| c.name.as_str())))?;
    let ids = picked.iter().map(|ci| channels.iter().position(|c| c.part == part && c.channel == *ci)).collect::<Option<Vec<usize>>>()?;
    Some(MainPicture { layout, ids })
}

/// At most this many files are indexed, and their indexes take at most [`INDEXED_BYTES`]; the
/// oldest goes first.
const INDEXED_FILES: usize = 4096;
const INDEXED_BYTES: usize = 64 << 20;

/// Indexed files by path.
struct Indexes {
    map: HashMap<String, Arc<ExrIndex>>,
    order: VecDeque<String>,
    bytes: usize,
}

fn indexes() -> &'static Mutex<Indexes> {
    static INDEXES: std::sync::OnceLock<Mutex<Indexes>> = std::sync::OnceLock::new();
    INDEXES.get_or_init(|| Mutex::new(Indexes { map: HashMap::new(), order: VecDeque::new(), bytes: 0 }))
}

fn lock() -> std::sync::MutexGuard<'static, Indexes> {
    indexes().lock().unwrap_or_else(PoisonError::into_inner)
}

/// Keep `index` as the index of `path`, dropping the oldest beyond the bounds (the newest stays).
pub(crate) fn keep(path: &str, index: Arc<ExrIndex>) {
    let mut ix = lock();
    let bytes = index.bytes();
    match ix.map.insert(path.to_string(), index) {
        Some(old) => ix.bytes = ix.bytes.saturating_sub(old.bytes()),
        None => ix.order.push_back(path.to_string()),
    }
    ix.bytes = ix.bytes.saturating_add(bytes);
    for _ in 0..=ix.order.len() {
        if (ix.map.len() <= INDEXED_FILES && ix.bytes <= INDEXED_BYTES) || ix.map.len() <= 1 {
            break;
        }
        let Some(old) = ix.order.pop_front() else { break };
        if old == path {
            ix.order.push_back(old);
            continue;
        }
        if let Some(gone) = ix.map.remove(&old) {
            ix.bytes = ix.bytes.saturating_sub(gone.bytes());
        }
    }
}

/// The index of the file at `path` (read from `source` the first time; kept until
/// [`forget`]). A file whose identity changed since (size, modification time; checked at most
/// once per [`crate::exr_read::STAMP_TTL`], #3334) is indexed again; a read checks it again on
/// the file it opens.
pub(crate) fn get(path: &str, source: &ExrSource) -> Result<Arc<ExrIndex>, IndexError> {
    get_opened(path, source).map(|(ix, _)| ix)
}

/// [`get`], with the file left open when indexing it opened it: a decode that follows reads from
/// it instead of opening the file again (one open per new frame, #3271).
pub(crate) fn get_opened(path: &str, source: &ExrSource) -> Result<(Arc<ExrIndex>, Option<Opened>), IndexError> {
    let kept = lock().map.get(path).cloned();
    if let Some(ix) = kept {
        match source.stamp() {
            Ok(stamp) if stamp == ix.stamp => return Ok((ix, None)),
            Ok(_) => {}
            Err(e) => {
                log::debug!("media: {path}: {e}");
                return Err(IndexError::Io);
            }
        }
    }
    let mut opened = source.open().map_err(|e| {
        log::debug!("media: {path}: {e}");
        IndexError::Io
    })?;
    // (the identity just read: the next checks within the stamp's lifetime use it)
    source.opened_as(opened.stamp);
    let ix = Arc::new(build(&mut *opened.reader, opened.stamp)?);
    keep(path, ix.clone());
    Ok((ix, Some(opened)))
}

/// The index of `path` if it is kept.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn cached(path: &str) -> Option<Arc<ExrIndex>> {
    lock().map.get(path).cloned()
}

/// Drop the index of `path` (the file changed on disk), and the identity last read of it.
pub(crate) fn forget(path: &str) {
    crate::exr_read::forget_stamp(std::path::Path::new(path));
    let mut ix = lock();
    if let Some(gone) = ix.map.remove(path) {
        ix.bytes = ix.bytes.saturating_sub(gone.bytes());
        ix.order.retain(|p| p != path);
    }
}
