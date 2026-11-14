//! The frame exchange file: pixels shared between EffectCraft and a helper without going
//! through the control pipe. EffectCraft creates it (in `/dev/shm` when there is one, so it
//! stays in memory, else the temp folder; on Windows as a temporary file) and both sides read
//! and write it at the offsets the messages name. Plain positional file I/O (`pread` and
//! `pwrite`, `seek_read` and `seek_write`), no `unsafe`: the OS page cache makes it shared
//! memory in practice, and a helper is free to map it.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::protocol::ImageRef;

/// `FILE_ATTRIBUTE_TEMPORARY` (winnt.h), defined here to avoid a Windows API dependency.
#[cfg(windows)]
const FILE_ATTRIBUTE_TEMPORARY: u32 = 0x100;

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
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create_new(true);
        // Frames are the user's pixels and /dev/shm is shared: only this user may read them
        // (the helper runs as the same user).
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        // Windows: a temporary file stays in the cache instead of being written out to disk
        // where the system can manage it, which is what a frame exchange wants.
        #[cfg(windows)]
        std::os::windows::fs::OpenOptionsExt::attributes(&mut opts, FILE_ATTRIBUTE_TEMPORARY);
        let file = opts.open(&path)?;
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

    /// The byte range of the image `at` describes, checked against overflow and [`MAX_EXTENT`].
    fn range(at: &ImageRef) -> Result<(u64, u64), String> {
        let bytes = at.bytes().ok_or_else(|| format!("image {}×{} is too large", at.width, at.height))?;
        let end = at
            .offset
            .checked_add(bytes)
            .filter(|&e| e <= MAX_EXTENT)
            .ok_or_else(|| format!("image at offset {} would take the frame exchange file past its 64 GiB limit", at.offset))?;
        Ok((at.offset, end))
    }

    /// Write `pixels` as the image `at` describes. Positional I/O on Unix and Windows: no shared
    /// cursor, so `&self` (the fallback for other targets seeks; they can't run helpers).
    pub fn write(&self, at: &ImageRef, pixels: &[[f32; 4]]) -> Result<(), String> {
        if at.pixels() != Some(pixels.len()) {
            return Err(format!("image is {}×{} but has {} pixels", at.width, at.height, pixels.len()));
        }
        let (start, _) = Self::range(at)?;
        write_all_at(&self.file, bytemuck::cast_slice(pixels), start).map_err(io)
    }

    /// Read the image `at` describes into `out` (which must have its size).
    pub fn read_into(&self, at: &ImageRef, out: &mut [[f32; 4]]) -> Result<(), String> {
        if at.pixels() != Some(out.len()) {
            return Err(format!("image is {}×{} but the buffer has {} pixels", at.width, at.height, out.len()));
        }
        let (start, _) = Self::range(at)?;
        read_exact_at(&self.file, bytemuck::cast_slice_mut(out), start).map_err(io)
    }

    /// Read the image `at` describes.
    pub fn read(&self, at: &ImageRef) -> Result<Vec<[f32; 4]>, String> {
        let n = at.pixels().ok_or_else(|| format!("image {}×{} is too large", at.width, at.height))?;
        // Look at the file before allocating: a hostile ref must not cost a huge buffer.
        let (_, end) = Self::range(at)?;
        let len = self.file.metadata().map_err(io)?.len();
        if end > len {
            return Err(format!("frame exchange: the image ends at byte {end} but the file has {len}"));
        }
        let mut out = vec![[0.0f32; 4]; n];
        self.read_into(at, &mut out)?;
        Ok(out)
    }
}

/// Upper bound for the end of any image in the exchange file; keeps a hostile offset from
/// growing the file. Each process has its own file; the images it holds at once are stacked in
/// it: the outermost render's frame at 0, then its `input_at` / `layer_input` answers, and a
/// nested render starts above the images the render around it still uses. All of them share this
/// limit, so in 32 bpc one process holds about 128 images of 7680×4320 at once, or 16 of
/// 16384×16384.
const MAX_EXTENT: u64 = 1 << 36;

#[cfg(unix)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
}

#[cfg(unix)]
fn write_all_at(file: &File, buf: &[u8], offset: u64) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, buf, offset)
}

/// `seek_read` may return less than asked: loop; a zero before the end is end of file.
#[cfg(windows)]
fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    while !buf.is_empty() {
        match std::os::windows::fs::FileExt::seek_read(file, buf, offset) {
            Ok(0) => return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "failed to fill whole buffer")),
            Ok(n) => {
                let rest = buf.get_mut(n..).ok_or_else(|| std::io::Error::other("read returned more than asked"))?;
                buf = rest;
                offset = offset.checked_add(n as u64).ok_or_else(|| std::io::Error::other("offset overflow"))?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// `seek_write` may write less than asked: loop; a zero is an error.
#[cfg(windows)]
fn write_all_at(file: &File, mut buf: &[u8], mut offset: u64) -> std::io::Result<()> {
    while !buf.is_empty() {
        match std::os::windows::fs::FileExt::seek_write(file, buf, offset) {
            Ok(0) => return Err(std::io::Error::new(std::io::ErrorKind::WriteZero, "failed to write whole buffer")),
            Ok(n) => {
                let rest = buf.get(n..).ok_or_else(|| std::io::Error::other("write returned more than asked"))?;
                buf = rest;
                offset = offset.checked_add(n as u64).ok_or_else(|| std::io::Error::other("offset overflow"))?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Other targets (wasm has no helpers to run anyway): seek and transfer through `&File`.
#[cfg(not(any(unix, windows)))]
fn read_exact_at(mut file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    use std::io::{Read, Seek, SeekFrom};
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(buf)
}

#[cfg(not(any(unix, windows)))]
fn write_all_at(mut file: &File, buf: &[u8], offset: u64) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom, Write};
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(buf)
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

    fn px(n: usize, seed: f32) -> Vec<[f32; 4]> {
        (0..n).map(|i| [seed + i as f32, seed * 2.0, seed * 3.0, 1.0]).collect()
    }

    fn at(offset: u64, width: u32, height: u32) -> ImageRef {
        ImageRef { offset, width, height, origin: [0.0; 2], scale: 1.0 }
    }

    #[test]
    fn two_images_at_different_offsets_do_not_overwrite_each_other() {
        let x = Exchange::create().unwrap();
        let (a, b) = (at(0, 4, 2), at(128, 3, 3));
        let (pa, pb) = (px(8, 1.0), px(9, 100.0));
        x.write(&a, &pa).unwrap();
        x.write(&b, &pb).unwrap();
        assert_eq!(x.read(&a).unwrap(), pa);
        assert_eq!(x.read(&b).unwrap(), pb);
        // Rewriting one leaves the other alone.
        let pa2 = px(8, -5.0);
        x.write(&a, &pa2).unwrap();
        assert_eq!(x.read(&a).unwrap(), pa2);
        assert_eq!(x.read(&b).unwrap(), pb);
    }

    #[test]
    fn an_image_ending_exactly_at_the_end_of_the_file_reads_back() {
        let x = Exchange::create().unwrap();
        let r = at(1000, 5, 5);
        let p = px(25, 7.0);
        x.write(&r, &p).unwrap();
        assert_eq!(x.file.metadata().unwrap().len(), 1000 + 25 * 16);
        assert_eq!(x.read(&r).unwrap(), p);
        let mut out = vec![[0.0; 4]; 25];
        x.read_into(&r, &mut out).unwrap();
        assert_eq!(out, p);
    }

    #[test]
    fn a_truncated_file_is_an_error_not_a_hang() {
        let x = Exchange::create().unwrap();
        x.write(&at(0, 2, 2), &px(4, 1.0)).unwrap();
        // One pixel past the end, and wholly past the end.
        assert!(x.read(&at(0, 2, 3)).is_err());
        assert!(x.read(&at(48, 2, 1)).is_err());
        assert!(x.read(&at(4096, 1, 1)).is_err());
        // The same through the buffer variant (which has no length check of its own).
        let mut out = vec![[0.0; 4]; 6];
        assert!(x.read_into(&at(0, 2, 3), &mut out).is_err());
        let mut one = vec![[0.0; 4]; 1];
        assert!(x.read_into(&at(4096, 1, 1), &mut one).is_err());
    }

    #[test]
    fn a_hostile_offset_is_an_error() {
        let x = Exchange::create().unwrap();
        let p = px(4, 1.0);
        for off in [u64::MAX, u64::MAX - 15, u64::MAX - 63, MAX_EXTENT, MAX_EXTENT - 63, i64::MAX as u64] {
            let r = at(off, 2, 2);
            assert!(x.write(&r, &p).is_err(), "write at {off}");
            assert!(x.read(&r).is_err(), "read at {off}");
            let mut out = vec![[0.0; 4]; 4];
            assert!(x.read_into(&r, &mut out).is_err(), "read_into at {off}");
        }
        // An image of the maximum size at a hostile offset must not allocate 4 GiB first.
        assert!(x.read(&at(u64::MAX, 16384, 16384)).is_err());
        assert_eq!(x.file.metadata().unwrap().len(), 0, "a rejected write leaves the file alone");
    }

    #[test]
    fn a_large_image_round_trips_whole() {
        // A whole-image round trip of a large frame (the short-transfer loop only splits at
        // 4 GiB on Windows, too large for a unit test).
        let x = Exchange::create().unwrap();
        let r = at(64, 1024, 512);
        let p = px(1024 * 512, 3.0);
        x.write(&r, &p).unwrap();
        assert!(x.read(&r).unwrap() == p);
    }

    #[cfg(windows)]
    #[test]
    fn the_windows_file_is_temporary_and_goes_away_with_its_owner() {
        use std::os::windows::fs::MetadataExt;
        let a = Exchange::create().unwrap();
        let b = Exchange::open(a.path()).unwrap();
        assert_ne!(std::fs::metadata(a.path()).unwrap().file_attributes() & FILE_ATTRIBUTE_TEMPORARY, 0);
        let r = at(0, 2, 1);
        b.write(&r, &px(2, 9.0)).unwrap();
        assert_eq!(a.read(&r).unwrap(), px(2, 9.0));
        let p = a.path().to_path_buf();
        drop(b);
        assert!(p.exists());
        drop(a);
        assert!(!p.exists());
    }

    /// The previous implementation, for the timing comparison only.
    fn old_write(file: &mut File, at: &ImageRef, pixels: &[[f32; 4]]) -> Result<(), String> {
        use std::io::{Seek, SeekFrom, Write};
        file.seek(SeekFrom::Start(at.offset)).map_err(io)?;
        file.write_all(bytemuck::cast_slice(pixels)).map_err(io)
    }

    fn old_read_into(file: &mut File, at: &ImageRef, out: &mut [[f32; 4]]) -> Result<(), String> {
        use std::io::{Read, Seek, SeekFrom};
        file.seek(SeekFrom::Start(at.offset)).map_err(io)?;
        file.read_exact(bytemuck::cast_slice_mut(out)).map_err(io)
    }

    /// `cargo test -p effectcraft-helper-host --release frames::tests::timing -- --ignored --nocapture`
    #[test]
    #[ignore = "timing only"]
    fn timing_of_seek_and_read_against_positional_io() {
        use std::time::Instant;
        let r = at(0, 1280, 720);
        let p = px(1280 * 720, 1.0);
        let mut out = vec![[0.0f32; 4]; p.len()];
        for round in 0..3 {
            let mut old = Exchange::create().unwrap();
            let t = Instant::now();
            for _ in 0..50 {
                old_write(&mut old.file, &r, &p).unwrap();
                old_read_into(&mut old.file, &r, &mut out).unwrap();
            }
            let old_ms = t.elapsed().as_secs_f64() * 1000.0;
            let new = Exchange::create().unwrap();
            let t = Instant::now();
            for _ in 0..50 {
                new.write(&r, &p).unwrap();
                new.read_into(&r, &mut out).unwrap();
            }
            let new_ms = t.elapsed().as_secs_f64() * 1000.0;
            println!("round {round}: 50 x (write + read) of 1280x720 RGBA f32: seek+rw {old_ms:.1} ms, positional {new_ms:.1} ms");
        }
    }

    #[test]
    fn both_sides_see_the_same_pixels() {
        let a = Exchange::create().unwrap();
        let b = Exchange::open(a.path()).unwrap();
        // Owner-only: frames are the user's pixels and /dev/shm is shared.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(a.path()).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let at = ImageRef { offset: 64, width: 2, height: 1, origin: [0.0; 2], scale: 1.0 };
        a.write(&at, &[[0.1, 0.2, 0.3, 0.4], [1.0, 2.0, 3.0, 4.0]]).unwrap();
        assert_eq!(b.read(&at).unwrap()[1], [1.0, 2.0, 3.0, 4.0]);
        assert!(b.write(&at, &[[0.0; 4]]).is_err(), "size mismatch is an error");
        let p = a.path().to_path_buf();
        let mut a = a;
        a.unlink();
        drop(a);
        drop(b);
        assert!(!p.exists());
    }
}
