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
/// reads. `None` when the file isn't ISO-BMFF, its `moov` is missing, or the retained boxes total
/// more than 64 MiB, including their headers.
#[cfg(any(unix, windows))]
pub(crate) fn movie_header(path: &Path) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    const MAX_HEADER: usize = 64 << 20;
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
            let size = usize::try_from(size).ok()?;
            if out.len().checked_add(size)? > MAX_HEADER {
                return None;
            }
            f.seek(SeekFrom::Start(pos)).ok()?;
            let mut b = vec![0u8; size];
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

#[cfg(all(test, any(unix, windows)))]
mod tests {
    use std::fs::{File, OpenOptions};
    use std::io::{self, Seek, SeekFrom, Write};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::movie_header;

    struct TempMovie(PathBuf);

    impl TempMovie {
        fn new() -> io::Result<(Self, File)> {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            loop {
                let id = NEXT.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!("effectcraft-movie-header-{}-{id}.mp4", std::process::id()));
                match OpenOptions::new().write(true).create_new(true).open(&path) {
                    Ok(file) => return Ok((Self(path), file)),
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(e),
                }
            }
        }

        fn bytes(bytes: &[u8]) -> io::Result<Self> {
            let (movie, mut file) = Self::new()?;
            file.write_all(bytes)?;
            Ok(movie)
        }

        fn boxes(boxes: &[(u32, &[u8; 4])]) -> io::Result<Self> {
            let (movie, mut file) = Self::new()?;
            let mut offset = 0u64;
            for &(size, kind) in boxes {
                file.seek(SeekFrom::Start(offset))?;
                file.write_all(&size.to_be_bytes())?;
                file.write_all(kind)?;
                offset = offset.checked_add(u64::from(size)).ok_or_else(|| io::Error::other("fixture offset overflow"))?;
            }
            file.set_len(offset)?;
            Ok(movie)
        }
    }

    impl Drop for TempMovie {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn movie_header_rejects_repeated_ftyp_over_budget() -> io::Result<()> {
        let movie = TempMovie::boxes(&[(33_554_440, b"ftyp"), (33_554_440, b"ftyp"), (8, b"moov")])?;
        assert!(movie_header(&movie.0).is_none());
        Ok(())
    }

    #[test]
    fn movie_header_retained_byte_boundary() -> io::Result<()> {
        {
            let movie = TempMovie::boxes(&[(67_108_856, b"ftyp"), (8, b"moov")])?;
            let header = movie_header(&movie.0).expect("exactly 64 MiB of retained boxes is accepted");
            assert_eq!(header.len(), 67_108_864);
            assert_eq!(header.get(..8), Some(b"\x03\xff\xff\xf8ftyp".as_slice()));
            assert_eq!(header.get(67_108_856..), Some(b"\0\0\0\x08moov".as_slice()));
        }
        let movie = TempMovie::boxes(&[(67_108_857, b"ftyp"), (8, b"moov")])?;
        assert!(movie_header(&movie.0).is_none());
        let movie = TempMovie::boxes(&[(67_108_865, b"moov")])?;
        assert!(movie_header(&movie.0).is_none());
        Ok(())
    }

    #[test]
    fn movie_header_preserves_bytes_and_errors() -> io::Result<()> {
        let movie = TempMovie::bytes(b"\0\0\0\x0cftypisom\0\0\0\x0cmdatdata\0\0\0\x0cmoovinfo")?;
        assert_eq!(movie_header(&movie.0), Some(b"\0\0\0\x0cftypisom\0\0\0\x0cmoovinfo".to_vec()));

        for bytes in [b"\0\0\0\x0cmoov".as_slice(), b"\0\0\0\x08moo", b"\0\0\0\x08ftyp"] {
            let movie = TempMovie::bytes(bytes)?;
            assert!(movie_header(&movie.0).is_none());
        }
        Ok(())
    }
}
