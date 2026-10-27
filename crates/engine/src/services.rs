use super::*;

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
}

/// Native filesystem.
pub struct FsServices;

impl Services for FsServices {
    fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
        std::fs::read(path)
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
