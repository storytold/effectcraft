//! Opening movie and audio files without holding them in memory.
//!
//! Containers FilmCraft can read through a [`ByteReader`] (MP4/MOV, Matroska/WebM, MXF, Ogg, MPEG
//! TS/PS) are opened from the file itself: only their index is read now, and samples are read at
//! their offsets when they are decoded. Formats FilmCraft decodes in one go (WAV, MP3, FLAC, AIFF)
//! are read whole, once, into a single buffer.

use std::sync::Arc;

use filmcraft_media::SharedSource;
use filmcraft_media::reader::{ByteReader, HEAD_LEN, MemReader, SharedReader};

use crate::{MediaError, Result};

/// The file name of `path` (FilmCraft's openers sniff by its extension).
pub(crate) fn file_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string())
}

/// A file read at any offset (positional reads, no shared cursor, so decoders on several threads
/// read at once).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct FileReader {
    #[cfg(any(unix, windows))]
    file: std::fs::File,
    #[cfg(not(any(unix, windows)))]
    file: std::sync::Mutex<std::fs::File>,
    len: u64,
}

#[cfg(not(target_arch = "wasm32"))]
impl FileReader {
    pub(crate) fn open(path: &str) -> std::io::Result<Self> {
        let file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        #[cfg(not(any(unix, windows)))]
        let file = std::sync::Mutex::new(file);
        Ok(Self { file, len })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ByteReader for FileReader {
    fn len(&self) -> u64 {
        self.len
    }
    #[cfg(unix)]
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        std::os::unix::fs::FileExt::read_exact_at(&self.file, buf, offset)
    }
    #[cfg(windows)]
    fn read_at(&self, offset: u64, mut buf: &mut [u8]) -> std::io::Result<()> {
        let mut at = offset;
        while !buf.is_empty() {
            match std::os::windows::fs::FileExt::seek_read(&self.file, buf, at) {
                Ok(0) => return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "read past end of file")),
                Ok(n) => {
                    let rest = std::mem::take(&mut buf);
                    buf = rest.get_mut(n..).unwrap_or_default();
                    at = at.saturating_add(n as u64);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = self.file.lock().unwrap_or_else(|e| e.into_inner());
        f.seek(SeekFrom::Start(offset))?;
        f.read_exact(buf)
    }
}

/// The first bytes of `r` (for sniffing).
pub(crate) fn head(r: &dyn ByteReader) -> std::io::Result<Vec<u8>> {
    filmcraft_media::reader::read_range(r, 0, HEAD_LEN)
}

/// All of `r` in one allocation: `Vec` → `Arc<[u8]>` copies, and a whole movie held twice at
/// once is what this module avoids.
pub(crate) fn read_whole(r: &dyn ByteReader) -> std::io::Result<Arc<[u8]>> {
    let n = usize::try_from(r.len()).map_err(|_| std::io::Error::new(std::io::ErrorKind::OutOfMemory, "file too large"))?;
    let mut all: Arc<[u8]> = std::iter::repeat_n(0u8, n).collect();
    let Some(buf) = Arc::get_mut(&mut all) else { return Err(std::io::Error::other("buffer is shared")) };
    r.read_at(0, buf)?;
    Ok(all)
}

/// Read a whole file into one `Arc<[u8]>` (a single allocation, no `Vec` copy).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn read_file(path: &str) -> Result<Arc<[u8]>> {
    let io = |e: std::io::Error| MediaError::Io(format!("{path}: {e}"));
    let r = FileReader::open(path).map_err(io)?;
    read_whole(&r).map_err(io)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn read_file(path: &str) -> Result<Arc<[u8]>> {
    std::fs::read(path).map(Into::into).map_err(|e| MediaError::Io(format!("{path}: {e}")))
}

/// Try FilmCraft's reader openers (containers read by their index): `None` when none of them
/// takes the file.
pub(crate) fn open_with_reader(name: &str, head: &[u8], reader: &SharedReader) -> Option<Result<SharedSource>> {
    filmcraft_codecs::reader_openers().into_iter().find_map(|o| o(name, head, reader)).map(|r| r.map_err(MediaError::from))
}

/// Open the movie or audio file `path`: from `mem` when its contents are in memory, otherwise
/// from disk through a file reader (containers keep only their index in memory), else read whole.
pub(crate) fn open_source(path: &str, mem: Option<Arc<[u8]>>) -> Result<SharedSource> {
    let name = file_name(path);
    let io = |e: std::io::Error| MediaError::Io(format!("{path}: {e}"));
    let reader: SharedReader = match &mem {
        Some(b) => Arc::new(MemReader(b.clone())),
        #[cfg(not(target_arch = "wasm32"))]
        None => Arc::new(FileReader::open(path).map_err(io)?),
        #[cfg(target_arch = "wasm32")]
        None => Arc::new(MemReader(read_file(path)?)),
    };
    let head = head(&*reader).map_err(io)?;
    if let Some(r) = open_with_reader(&name, &head, &reader) {
        return r;
    }
    let bytes = match mem {
        Some(b) => b,
        None => read_whole(&*reader).map_err(io)?,
    };
    drop(reader);
    filmcraft_codecs::open_bytes(&name, bytes).map_err(MediaError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_reader_reads_ranges_and_whole_files() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-media-open");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bytes.bin");
        let data: Vec<u8> = (0..200_000u32).map(|i| (i * 7) as u8).collect();
        std::fs::write(&p, &data).unwrap();
        let path = p.to_string_lossy();
        let r = FileReader::open(&path).unwrap();
        assert_eq!(r.len(), data.len() as u64);
        let mut b = [0u8; 5];
        r.read_at(100_000, &mut b).unwrap();
        assert_eq!(&b, &data[100_000..100_005]);
        assert_eq!(r.read_at(199_998, &mut b).unwrap_err().kind(), std::io::ErrorKind::UnexpectedEof);
        assert_eq!(head(&r).unwrap(), &data[..HEAD_LEN]);
        assert_eq!(&*read_file(&path).unwrap(), &data[..]);
        assert!(read_file(&dir.join("missing.bin").to_string_lossy()).is_err());
        let _ = std::fs::remove_file(&p);
    }
}
