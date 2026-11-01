//! The frame exchange file: pixels shared between EffectCraft and a helper without going
//! through the control pipe. EffectCraft creates it (in `/dev/shm` when there is one, so it
//! stays in memory, else the temp folder) and both sides read and write it at the offsets the
//! messages name. Plain file I/O, no `unsafe`: the OS page cache makes it shared memory in
//! practice, and a helper is free to map it.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::protocol::ImageRef;

/// An open frame exchange file.
pub struct Exchange {
    file: File,
    path: PathBuf,
    /// Remove the file when dropped (the side that created it).
    owned: bool,
}

impl Exchange {
    /// Create a new, empty exchange file (EffectCraft's side).
    pub fn create() -> std::io::Result<Exchange> {
        static N: AtomicU64 = AtomicU64::new(0);
        let shm = Path::new("/dev/shm");
        let dir = if cfg!(target_os = "linux") && shm.is_dir() { shm.to_path_buf() } else { std::env::temp_dir() };
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let path = dir.join(format!("effectcraft-helper-{}-{}-{nanos}.frames", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let file = OpenOptions::new().read(true).write(true).create_new(true).open(&path)?;
        Ok(Exchange { file, path, owned: true })
    }

    /// Open an exchange file EffectCraft created (the helper's side).
    pub fn open(path: &Path) -> std::io::Result<Exchange> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        Ok(Exchange { file, path: path.to_path_buf(), owned: false })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Remove the file's name once both sides have it open (Unix keeps the open file usable),
    /// so nothing is left behind if either side dies.
    pub fn unlink(&mut self) {
        if cfg!(unix) && self.owned && std::fs::remove_file(&self.path).is_ok() {
            self.owned = false;
        }
    }

    /// Write `pixels` as the image `at` describes.
    pub fn write(&mut self, at: &ImageRef, pixels: &[[f32; 4]]) -> Result<(), String> {
        if at.pixels() != Some(pixels.len()) {
            return Err(format!("image is {}×{} but has {} pixels", at.width, at.height, pixels.len()));
        }
        self.file.seek(SeekFrom::Start(at.offset)).map_err(io)?;
        self.file.write_all(bytemuck::cast_slice(pixels)).map_err(io)
    }

    /// Read the image `at` describes into `out` (which must have its size).
    pub fn read_into(&mut self, at: &ImageRef, out: &mut [[f32; 4]]) -> Result<(), String> {
        if at.pixels() != Some(out.len()) {
            return Err(format!("image is {}×{} but the buffer has {} pixels", at.width, at.height, out.len()));
        }
        self.file.seek(SeekFrom::Start(at.offset)).map_err(io)?;
        self.file.read_exact(bytemuck::cast_slice_mut(out)).map_err(io)
    }

    /// Read the image `at` describes.
    pub fn read(&mut self, at: &ImageRef) -> Result<Vec<[f32; 4]>, String> {
        let n = at.pixels().ok_or_else(|| format!("image {}×{} is too large", at.width, at.height))?;
        let mut out = vec![[0.0f32; 4]; n];
        self.read_into(at, &mut out)?;
        Ok(out)
    }
}

fn io(e: std::io::Error) -> String {
    format!("frame exchange: {e}")
}

impl Drop for Exchange {
    fn drop(&mut self) {
        if self.owned {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_sides_see_the_same_pixels() {
        let mut a = Exchange::create().unwrap();
        let mut b = Exchange::open(a.path()).unwrap();
        let at = ImageRef { offset: 64, width: 2, height: 1, origin: [0.0; 2], scale: 1.0 };
        a.write(&at, &[[0.1, 0.2, 0.3, 0.4], [1.0, 2.0, 3.0, 4.0]]).unwrap();
        assert_eq!(b.read(&at).unwrap()[1], [1.0, 2.0, 3.0, 4.0]);
        assert!(b.write(&at, &[[0.0; 4]]).is_err(), "size mismatch is an error");
        let p = a.path().to_path_buf();
        a.unlink();
        drop(a);
        drop(b);
        assert!(!p.exists());
    }
}
