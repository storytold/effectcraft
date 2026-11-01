use super::config;

/// Host services (file access) injected by the frontend.
pub trait Services: Send + Sync {
    fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>>;
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
        use std::io::Read;
        let mut file = std::fs::File::open(path)?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{path} is not a file")));
        }
        let mut bytes = Vec::new();
        // A sparse file can claim far more than memory: an error, not an abort.
        bytes.try_reserve_exact(usize::try_from(meta.len()).unwrap_or(usize::MAX)).map_err(|e| std::io::Error::new(std::io::ErrorKind::OutOfMemory, e))?;
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }
    fn write_file(&self, path: &str, data: &[u8]) -> std::io::Result<()> {
        config::atomic_write(std::path::Path::new(path), data)
    }
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
}
