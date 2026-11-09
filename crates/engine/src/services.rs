use super::config;

/// The largest PhotoCraft bundle [`Services::read_pcraft`] reads.
pub const PCRAFT_MAX_INPUT: u64 = 512 << 20;

/// The error for a PhotoCraft bundle over [`PCRAFT_MAX_INPUT`].
pub fn pcraft_too_large() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, "PhotoCraft bundle exceeds the 512 MiB input limit")
}

/// Host services (file access) injected by the frontend.
pub trait Services: Send + Sync {
    fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>>;
    /// A PhotoCraft bundle's bytes: like [`Services::read_file`], but a file over
    /// [`PCRAFT_MAX_INPUT`] is an error. Other files read with `read_file` have no such limit.
    fn read_pcraft(&self, path: &str) -> std::io::Result<Vec<u8>> {
        if self.stamp(path).is_some_and(|(len, _)| len > PCRAFT_MAX_INPUT) {
            return Err(pcraft_too_large());
        }
        let bytes = self.read_file(path)?;
        if bytes.len() as u64 > PCRAFT_MAX_INPUT {
            return Err(pcraft_too_large());
        }
        Ok(bytes)
    }
    /// [`Services::read_file`] for a copy that must not load an unbounded file: more than `max`
    /// bytes is an error. The native one opens the file once, reads its size from that handle and
    /// reads through a [`std::io::Read::take`] limit, so a file grown or swapped after the check
    /// still cannot read past `max`.
    fn read_file_bounded(&self, path: &str, max: u64) -> std::io::Result<Vec<u8>> {
        let bytes = self.read_file(path)?;
        if bytes.len() as u64 > max {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{path} is larger than the {max}-byte limit")));
        }
        Ok(bytes)
    }
    fn write_file(&self, path: &str, data: &[u8]) -> std::io::Result<()>;
    /// Whether a file exists (the background footage check after open).
    fn exists(&self, path: &str) -> bool {
        std::path::Path::new(path).exists()
    }
    /// A file's size and modification time (nanoseconds since 1970), to notice that it changed
    /// on disk (Settings ▸ Import ▸ Reload Footage Changed on Disk); `None` when it can't be read.
    fn stamp(&self, path: &str) -> Option<(u64, u128)> {
        let meta = std::fs::metadata(path).ok()?;
        let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
        Some((meta.len(), modified.as_nanos()))
    }
    /// Keep a file the app made for itself (footage extracted from a template): like
    /// [`Services::write_file`], creating its folder, but never offered as a download (web: it
    /// goes to browser storage only).
    fn store_file(&self, path: &str, data: &[u8]) -> std::io::Result<()> {
        if let Some(d) = std::path::Path::new(path).parent().filter(|d| !d.as_os_str().is_empty()) {
            let _ = std::fs::create_dir_all(d);
        }
        self.write_file(path, data)
    }
    /// The files in folder `dir` (full paths; no folders, no hidden files), for image-sequence
    /// detection on import. Empty when it can't be read.
    fn list_dir(&self, dir: &str) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(if dir.is_empty() { "." } else { dir }) else { return vec![] };
        rd.filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()) && !e.file_name().to_string_lossy().starts_with('.'))
            .map(|e| e.path().to_string_lossy().to_string())
            .collect()
    }
    /// Whether `path` is a folder (dropped or picked folders import their files).
    fn is_dir(&self, path: &str) -> bool {
        std::path::Path::new(path).is_dir()
    }
    /// `path` as a list of recent files keeps it (File ▸ Scripts ▸ Recent Script Files): a
    /// relative path is made absolute against the working directory, so it opens the same file
    /// after the working directory changes. `None` when the path can't be made absolute (the
    /// working directory can't be read); the script is then not added to the list.
    fn full_path(&self, path: &str) -> Option<String> {
        std::path::absolute(path).ok().map(|p| p.to_string_lossy().into_owned())
    }
}

/// Native filesystem.
pub struct FsServices;

impl Services for FsServices {
    /// Refuses what isn't a regular file: a project can name `/dev/zero` or a FIFO, and reading
    /// those never ends (#2134).
    fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
        read_regular_file(path, None)
    }
    /// Checks the size before reserving the buffer, and stops reading a file that grows past
    /// the limit while it is read.
    fn read_pcraft(&self, path: &str) -> std::io::Result<Vec<u8>> {
        read_regular_file(path, Some(PCRAFT_MAX_INPUT))
    }
    fn read_file_bounded(&self, path: &str, max: u64) -> std::io::Result<Vec<u8>> {
        use std::io::Read;
        let file = std::fs::File::open(path)?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{path} is not a file")));
        }
        if meta.len() > max {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{path} is larger than the {max}-byte limit")));
        }
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(usize::try_from(meta.len()).unwrap_or(usize::MAX)).map_err(|e| std::io::Error::new(std::io::ErrorKind::OutOfMemory, e))?;
        // The size above came from this handle, and the read is capped too: a file that grows
        // after the check still cannot read past `max`. Saturating, so `u64::MAX` cannot overflow.
        file.take(max.saturating_add(1)).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > max {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{path} is larger than the {max}-byte limit")));
        }
        Ok(bytes)
    }
    fn write_file(&self, path: &str, data: &[u8]) -> std::io::Result<()> {
        config::atomic_write(std::path::Path::new(path), data)
    }
}

/// A regular file's bytes; with `limit`, a file larger than `limit` bytes is an error.
fn read_regular_file(path: &str, limit: Option<u64>) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{path} is not a file")));
    }
    if limit.is_some_and(|max| meta.len() > max) {
        return Err(pcraft_too_large());
    }
    let mut bytes = Vec::new();
    // A sparse file can claim far more than memory: an error, not an abort.
    bytes.try_reserve_exact(usize::try_from(meta.len()).unwrap_or(usize::MAX)).map_err(|e| std::io::Error::new(std::io::ErrorKind::OutOfMemory, e))?;
    file.take(limit.map_or(u64::MAX, |max| max + 1)).read_to_end(&mut bytes)?;
    if limit.is_some_and(|max| bytes.len() as u64 > max) {
        return Err(pcraft_too_large());
    }
    Ok(bytes)
}

/// Imports media files into the project (implemented by the media layer).
pub trait Importer: Send + Sync {
    /// Probe one file and return footage metadata. Image sequences are the engine's
    /// ([`sequence`]): it groups numbered stills and probes their first file here.
    fn probe(&self, path: &str) -> std::result::Result<effectcraft_project::Footage, String>;
    /// A footage file's bytes are now at `path` (extracted from a template): hosts whose media
    /// layer reads from memory (the browser) register them; file-based ones need nothing.
    fn register(&self, _path: &str, _data: &[u8]) {}
    /// An image sequence's other frames (its first was probed) are footage now: hosts whose
    /// media layer reads from memory (the browser) register them; file-based ones need nothing.
    fn add_frames(&self, _paths: &[String]) {}
}

#[cfg(test)]
mod tests {
    use super::{FsServices, Services};

    /// #2134: a project can name a folder, `/dev/zero` or a FIFO; reading those is an error.
    #[test]
    fn fs_services_read_only_regular_files() {
        let dir = std::env::temp_dir();
        assert!(FsServices.read_file(&dir.to_string_lossy()).is_err());
        #[cfg(unix)]
        assert!(FsServices.read_file("/dev/zero").is_err());
        let f = dir.join(format!("ec-services-read-{}.txt", std::process::id()));
        std::fs::write(&f, b"data").unwrap();
        assert_eq!(FsServices.read_file(&f.to_string_lossy()).unwrap(), b"data");
        let _ = std::fs::remove_file(f);
    }

    /// Templates and dotLottie files are ZIP archives too: only PhotoCraft reads have the
    /// 512 MiB input limit. The length is set without writing the data.
    #[test]
    fn only_photocraft_reads_have_the_input_limit() {
        let f = std::env::temp_dir().join(format!("ec-services-large-{}.zip", std::process::id()));
        std::fs::write(&f, b"PK\x03\x04").unwrap();
        std::fs::OpenOptions::new().write(true).open(&f).unwrap().set_len(super::PCRAFT_MAX_INPUT + 1).unwrap();
        let path = f.to_string_lossy();
        let error = FsServices.read_pcraft(&path).unwrap_err();
        assert!(error.to_string().contains("512 MiB input limit"), "{error}");
        let bytes = FsServices.read_file(&path).unwrap();
        assert_eq!(bytes.len() as u64, super::PCRAFT_MAX_INPUT + 1);
        assert!(bytes.starts_with(b"PK\x03\x04"));
        drop(bytes);
        let _ = std::fs::remove_file(f);
    }

    /// The limit is inclusive and cannot overflow: `u64::MAX` reads the file, one byte too many
    /// is an error, and nothing past it is read.
    #[test]
    fn fs_services_bounded_read_stops_at_the_limit() {
        let f = std::env::temp_dir().join(format!("ec-services-bounded-{}.txt", std::process::id()));
        std::fs::write(&f, b"data").unwrap();
        let p = f.to_string_lossy().to_string();
        assert_eq!(FsServices.read_file_bounded(&p, u64::MAX).unwrap(), b"data");
        assert_eq!(FsServices.read_file_bounded(&p, 4).unwrap(), b"data");
        assert!(FsServices.read_file_bounded(&p, 3).is_err());
        let _ = std::fs::remove_file(f);
    }
}
