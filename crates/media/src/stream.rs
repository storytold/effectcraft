//! Movie and audio files read in place: the container's index now, single samples on demand.
//!
//! Reading a file whole before opening it made a clip cost its full size in memory (twice while
//! probing: the file's bytes plus the demuxer's copy), so a 45-minute movie took gigabytes just to
//! import. FilmCraft's reader openers (MP4/MOV, Matroska/WebM, MXF, Ogg, MPEG-PS/TS) open from a
//! random-access [`ByteReader`]; [`FileReader`] serves them positional reads from the open file.
//! Formats decoded in one go (stills, WAV, FLAC, MP3…) still read the whole file, as before.
//! Builds without a native file system (wasm32) keep using in-memory bytes.

use std::path::Path;

use filmcraft_media::SharedSource;

use crate::Result;

/// A file on disk read in place: it stays open and only the requested ranges are read.
#[cfg(any(unix, windows))]
#[derive(Debug)]
pub(crate) struct FileReader {
    file: std::fs::File,
    len: u64,
}

#[cfg(any(unix, windows))]
impl FileReader {
    pub(crate) fn open(path: &Path) -> std::io::Result<Self> {
        let file = std::fs::File::open(path)?;
        let m = file.metadata()?;
        if !m.is_file() {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{} is not a file", path.display())));
        }
        Ok(Self { file, len: m.len() })
    }
}

#[cfg(any(unix, windows))]
impl filmcraft_media::ByteReader for FileReader {
    fn len(&self) -> u64 {
        self.len
    }

    /// Positional reads: threads decoding different parts of the file share no cursor.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            std::os::unix::fs::FileExt::read_exact_at(&self.file, buf, offset)
        }
        #[cfg(windows)]
        {
            let (mut done, mut at) = (0usize, offset);
            while let Some(rest) = buf.get_mut(done..).filter(|r| !r.is_empty()) {
                match std::os::windows::fs::FileExt::seek_read(&self.file, rest, at) {
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
    }
}

#[cfg(any(unix, windows))]
fn name_of(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

/// Open a movie/audio file for the media pool, reading only what its container needs; files the
/// reader openers don't handle are read whole and opened from memory, exactly as before. `None`
/// when the file can't be opened in place (the caller reads it whole, which reports the error).
#[cfg(any(unix, windows))]
pub(crate) fn open_source(path: &Path) -> Option<Result<SharedSource>> {
    let reader = FileReader::open(path).ok()?;
    let reader: filmcraft_media::SharedReader = std::sync::Arc::new(reader);
    let opened = filmcraft_media::reader::open_reader(&name_of(path), reader, &filmcraft_codecs::reader_openers(), &filmcraft_codecs::openers());
    Some(opened.map_err(Into::into))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn open_source(_path: &Path) -> Option<Result<SharedSource>> {
    None
}

/// `path` read in place (`None` when it can't be opened, or without a native file system).
#[cfg(any(unix, windows))]
pub(crate) fn file_reader(path: &Path) -> Option<Box<dyn filmcraft_media::ByteReader>> {
    Some(Box::new(FileReader::open(path).ok()?))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn file_reader(_path: &Path) -> Option<Box<dyn filmcraft_media::ByteReader>> {
    None
}

/// The leading `ftyp` and the `moov` box of an ISO-BMFF movie on disk (all a track's display
/// matrix needs), without reading the samples: top-level box headers are walked with positional
/// reads. `None` when the file isn't ISO-BMFF or its `moov` is missing or implausibly large.
#[cfg(any(unix, windows))]
pub(crate) fn movie_header(path: &Path) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    const MAX_MOOV: u64 = 64 << 20;
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut out = Vec::new();
    let mut pos = 0u64;
    for _ in 0..4096 {
        if pos.checked_add(8)? > len {
            return None;
        }
        f.seek(SeekFrom::Start(pos)).ok()?;
        let mut h = [0u8; 16];
        f.read_exact(&mut h[..8]).ok()?;
        let kind = [h[4], h[5], h[6], h[7]];
        let mut size = u64::from(u32::from_be_bytes([h[0], h[1], h[2], h[3]]));
        if size == 1 {
            f.read_exact(&mut h[8..16]).ok()?;
            size = u64::from_be_bytes([h[8], h[9], h[10], h[11], h[12], h[13], h[14], h[15]]);
        } else if size == 0 {
            size = len - pos;
        }
        if size < 8 || pos.checked_add(size)? > len {
            return None;
        }
        if kind == *b"ftyp" || kind == *b"moov" {
            if size > MAX_MOOV {
                return None;
            }
            f.seek(SeekFrom::Start(pos)).ok()?;
            let mut b = vec![0u8; usize::try_from(size).ok()?];
            f.read_exact(&mut b).ok()?;
            out.extend_from_slice(&b);
            if kind == *b"moov" {
                return Some(out);
            }
        }
        pos += size;
    }
    None
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn movie_header(_path: &Path) -> Option<Vec<u8>> {
    None
}

/// Probing in place: the opened container when `path` is a movie/audio container a reader opener
/// handles and nothing probed before containers (stills, layered and vector documents, 3D models)
/// claims it. `None`: probe the whole file's bytes as before.
#[cfg(any(unix, windows))]
pub(crate) fn probe_source(path: &Path) -> Option<Result<SharedSource>> {
    let ext = crate::ext_of(path);
    if crate::STILL_EXTENSIONS.contains(&ext.as_str()) || crate::MODEL_EXTENSIONS.contains(&ext.as_str()) {
        return None;
    }
    let reader = FileReader::open(path).ok()?;
    let reader: filmcraft_media::SharedReader = std::sync::Arc::new(reader);
    let head = filmcraft_media::reader::read_range(&*reader, 0, filmcraft_media::reader::HEAD_LEN).ok()?;
    let name = path.to_string_lossy();
    if crate::layered::sniff(&name, &head) || image::guess_format(&head).is_ok() {
        return None;
    }
    let name = name_of(path);
    filmcraft_codecs::reader_openers().into_iter().find_map(|open| open(&name, &head, &reader)).map(|r| r.map_err(Into::into))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn probe_source(_path: &Path) -> Option<Result<SharedSource>> {
    None
}
