//! The frame exchange file: pixels shared between EffectCraft and a helper without going
//! through the control pipe. EffectCraft creates it (in `/dev/shm` when there is one, so it
//! stays in memory, else the temp folder) and both sides read and write it at the offsets the
//! messages name. Plain file I/O, no `unsafe`: the OS page cache makes it shared memory in
//! practice, and a helper is free to map it. Large images are written and read in a few slices
//! at once: one thread copies through the page cache at ~2 GB/s (~18 ms for a 1080p frame on
//! Windows), several at ~4 ms.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::protocol::ImageRef;

/// Images at least this large are copied in [`SLICES`] parallel slices.
const PARALLEL_BYTES: usize = 4 << 20;
const SLICES: usize = 4;

/// An open frame exchange file.
pub struct Exchange {
    file: File,
    /// More handles to the file for parallel slices, opened on first use. Windows serialises
    /// I/O on one synchronous handle, so each slice needs its own; Unix positioned I/O doesn't.
    #[cfg(windows)]
    more: Vec<File>,
    path: PathBuf,
    /// Remove the file when dropped (the side that created it).
    owned: bool,
    /// The file is known to be at least this long ([`Exchange::reserve`] skips asking: the
    /// question waits for reads in flight on Windows).
    reserved: u64,
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
        let file = opts.open(&path)?;
        Ok(Exchange {
            file,
            #[cfg(windows)]
            more: vec![],
            path,
            owned: true,
            reserved: 0,
        })
    }

    /// Open an exchange file EffectCraft created (the helper's side).
    pub fn open(path: &Path) -> std::io::Result<Exchange> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        Ok(Exchange {
            file,
            #[cfg(windows)]
            more: vec![],
            path: path.to_path_buf(),
            owned: false,
            reserved: 0,
        })
    }

    /// More handles to the same file (for reading on another thread while this one is busy).
    pub fn reader(&self) -> std::io::Result<Exchange> {
        // (Windows: a handle of its own, as a clone shares the file object, whose synchronous
        // I/O is serialised; Unix: the name may be gone, and positional I/O doesn't serialise)
        #[cfg(windows)]
        let file = OpenOptions::new().read(true).open(&self.path)?;
        #[cfg(not(windows))]
        let file = self.file.try_clone()?;
        Ok(Exchange {
            file,
            #[cfg(windows)]
            more: vec![],
            path: self.path.clone(),
            owned: false,
            reserved: 0,
        })
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
        let bytes: &[u8] = bytemuck::cast_slice(pixels);
        let step = slice_len(at, bytes.len())?;
        let handles = self.handles(bytes.len().div_ceil(step)).map_err(io)?;
        in_parallel(bytes.chunks(step).zip(handles).enumerate(), |(i, (part, f))| write_at(f, part, at.offset + (i * step) as u64))
    }

    /// Make the file long enough to hold the image `at` describes, without writing it (zeros where
    /// nothing was written yet): the frame of an effect that ignores its input isn't copied in,
    /// but the helper still reads or maps it there.
    pub fn reserve(&mut self, at: &ImageRef) -> Result<(), String> {
        let n = at.pixels().ok_or_else(|| format!("image {}×{} is too large", at.width, at.height))?;
        let end = (n as u64).checked_mul(16).and_then(|b| b.checked_add(at.offset)).ok_or("frame exchange: image past the end of a file")?;
        if end <= self.reserved {
            return Ok(());
        }
        if self.file.metadata().map_err(io)?.len() < end {
            self.file.set_len(end).map_err(io)?;
        }
        self.reserved = end;
        Ok(())
    }

    /// Read the image `at` describes into `out` (which must have its size).
    pub fn read_into(&mut self, at: &ImageRef, out: &mut [[f32; 4]]) -> Result<(), String> {
        if at.pixels() != Some(out.len()) {
            return Err(format!("image is {}×{} but the buffer has {} pixels", at.width, at.height, out.len()));
        }
        let bytes: &mut [u8] = bytemuck::cast_slice_mut(out);
        let step = slice_len(at, bytes.len())?;
        let handles = self.handles(bytes.len().div_ceil(step)).map_err(io)?;
        in_parallel(bytes.chunks_mut(step).zip(handles).enumerate(), |(i, (part, f))| read_at(f, part, at.offset + (i * step) as u64))
    }

    /// Read only the rectangle `bounds` (`[x0, y0, x1, y1]`, clamped to the image) of the image
    /// `at` describes into `out`, and make the rest of `out` transparent: a helper that drew into
    /// part of the frame only (a particle system's few puffs) costs a fraction of a full read.
    /// With `clear` false only the rows of `bounds` are read and nothing is cleared: the caller
    /// treats the rest as transparent itself.
    pub fn read_bounds_into(&mut self, at: &ImageRef, out: &mut [[f32; 4]], bounds: [u32; 4], clear: bool) -> Result<(), String> {
        if at.pixels() != Some(out.len()) {
            return Err(format!("image is {}×{} but the buffer has {} pixels", at.width, at.height, out.len()));
        }
        // The whole image's byte range, so no offset inside the band below can overflow.
        slice_len(at, std::mem::size_of_val(out))?;
        let (w, h) = (at.width as usize, at.height as usize);
        let x0 = (bounds[0] as usize).min(w);
        let x1 = (bounds[2] as usize).clamp(x0, w);
        let y0 = (bounds[1] as usize).min(h);
        let y1 = (bounds[3] as usize).clamp(y0, h);
        // (empty: a fully transparent result)
        if x0 == x1 || y0 == y1 {
            if clear {
                out.fill([0.0; 4]);
            }
            return Ok(());
        }
        let (above, rest) = out.split_at_mut(y0 * w);
        let (band, below) = rest.split_at_mut((y1 - y0) * w);
        if clear {
            above.fill([0.0; 4]);
            below.fill([0.0; 4]);
        }
        // Whole rows, one contiguous range in a few parallel slices: far fewer reads than row by
        // row (a read per row cost more than the copying), then the columns outside cleared.
        let bytes: &mut [u8] = bytemuck::cast_slice_mut(band);
        let step = slice_len(at, bytes.len())?;
        let handles = self.handles(bytes.len().div_ceil(step)).map_err(io)?;
        let start = at.offset + (y0 * w * 16) as u64;
        in_parallel(bytes.chunks_mut(step).zip(handles).enumerate(), |(i, (part, f))| read_at(f, part, start + (i * step) as u64))?;
        if clear && (x0 > 0 || x1 < w) {
            for row in band.chunks_mut(w) {
                row.get_mut(..x0).unwrap_or_default().fill([0.0; 4]);
                row.get_mut(x1..).unwrap_or_default().fill([0.0; 4]);
            }
        }
        Ok(())
    }

    /// `n` handles to the file, one per slice.
    fn handles(&mut self, n: usize) -> std::io::Result<Vec<&File>> {
        #[cfg(windows)]
        {
            while self.more.len() + 1 < n {
                self.more.push(OpenOptions::new().read(true).write(true).open(&self.path)?);
            }
            Ok(std::iter::once(&self.file).chain(&self.more).take(n).collect())
        }
        #[cfg(not(windows))]
        Ok(vec![&self.file; n])
    }

    /// Read the image `at` describes.
    pub fn read(&mut self, at: &ImageRef) -> Result<Vec<[f32; 4]>, String> {
        let n = at.pixels().ok_or_else(|| format!("image {}×{} is too large", at.width, at.height))?;
        let mut out = vec![[0.0f32; 4]; n];
        self.read_into(at, &mut out)?;
        Ok(out)
    }
}

/// The slice size for copying the `len` bytes of `at`: whole below [`PARALLEL_BYTES`], else in
/// [`SLICES`]. The whole range is checked first, so no slice's offset can overflow.
fn slice_len(at: &ImageRef, len: usize) -> Result<usize, String> {
    if at.offset.checked_add(len as u64).is_none() {
        return Err(format!("image offset {} is out of range", at.offset));
    }
    Ok(if len < PARALLEL_BYTES { len.max(1) } else { len.div_ceil(SLICES) })
}

/// Run `f` on every item, on scoped threads when there is more than one.
fn in_parallel<T: Send>(items: impl Iterator<Item = T>, f: impl Fn(T) -> std::io::Result<()> + Sync) -> Result<(), String> {
    let mut items: Vec<T> = items.collect();
    if items.len() <= 1 {
        return items.pop().map_or(Ok(()), &f).map_err(io);
    }
    std::thread::scope(|s| {
        let jobs: Vec<_> = items.into_iter().map(|t| s.spawn(|| f(t))).collect();
        // Join every job, keeping the first error: one left unjoined that panicked would make
        // `scope` itself panic.
        jobs.into_iter().map(|j| j.join().unwrap_or_else(|_| Err(std::io::Error::other("copy thread panicked")))).fold(Ok(()), Result::and)
    })
    .map_err(io)
}

#[cfg(windows)]
fn write_at(f: &File, mut buf: &[u8], mut pos: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = f.seek_write(buf, pos)?;
        if n == 0 {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        buf = &buf[n..];
        pos += n as u64;
    }
    Ok(())
}

#[cfg(windows)]
fn read_at(f: &File, mut buf: &mut [u8], mut pos: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = f.seek_read(buf, pos)?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        buf = &mut std::mem::take(&mut buf)[n..];
        pos += n as u64;
    }
    Ok(())
}

#[cfg(unix)]
fn write_at(f: &File, buf: &[u8], pos: u64) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(f, buf, pos)
}

#[cfg(unix)]
fn read_at(f: &File, buf: &mut [u8], pos: u64) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(f, buf, pos)
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
        // A large image goes in parallel slices: same pixels back.
        let big = ImageRef { offset: 64 * 3, width: 1023, height: 1025, origin: [0.0; 2], scale: 1.0 };
        let px: Vec<[f32; 4]> = (0..1023 * 1025).map(|i| [i as f32, 1.0, 2.0, 3.0]).collect();
        assert!(px.len() * 16 >= PARALLEL_BYTES);
        a.write(&big, &px).unwrap();
        assert_eq!(b.read(&big).unwrap(), px);
        assert_eq!(a.read(&at).unwrap()[1], [1.0, 2.0, 3.0, 4.0], "the small image below is untouched");
        // An offset whose image would end past the last byte offset is refused before any slice
        // (its later slices would otherwise wrap to low offsets in release builds).
        let wraps = ImageRef { offset: u64::MAX - 16, ..big };
        assert!(a.write(&wraps, &px).is_err_and(|e| e.contains("out of range")));
        assert!(b.read(&wraps).is_err_and(|e| e.contains("out of range")));
        let mut sink = vec![[0.0f32; 4]; px.len()];
        assert!(b.read_bounds_into(&wraps, &mut sink, [0, 1000, 1, 1025], true).is_err_and(|e| e.contains("out of range")));
        // Bounds: only that rectangle is read and the rest of the buffer is made transparent,
        // whatever it held; bounds past the image are clamped, empty ones clear everything.
        let mut out = vec![[9.0f32; 4]; px.len()];
        a.read_bounds_into(&big, &mut out, [100, 200, 900, 1000], true).unwrap();
        for (i, (o, p)) in out.iter().zip(&px).enumerate() {
            let (x, y) = (i % 1023, i / 1023);
            let inside = (100..900).contains(&x) && (200..1000).contains(&y);
            assert_eq!(*o, if inside { *p } else { [0.0; 4] }, "pixel {x},{y}");
        }
        out.fill([9.0; 4]);
        a.read_bounds_into(&big, &mut out, [1000, 1020, 5000, 9000], true).unwrap();
        assert_eq!(out[1024 * 1023 + 1022], px[1024 * 1023 + 1022]);
        assert_eq!(out[1024 * 1023 + 999], [0.0; 4]);
        out.fill([9.0; 4]);
        a.read_bounds_into(&big, &mut out, [500, 500, 500, 900], true).unwrap();
        assert!(out.iter().all(|o| *o == [0.0; 4]));
        assert!(a.read_bounds_into(&at, &mut out, [0, 0, 1, 1], true).is_err(), "size mismatch is an error");
        let p = a.path().to_path_buf();
        a.unlink();
        drop(a);
        drop(b);
        assert!(!p.exists());
    }

    /// A failed slice doesn't stop the others being joined: a later one that panics is reported as
    /// an error, not as a panic out of the thread scope.
    #[test]
    fn every_copy_thread_is_joined() {
        let r = super::in_parallel(0..3, |i| match i {
            0 => Err(std::io::Error::other("first")),
            1 => panic!("copy slice"),
            _ => Ok(()),
        });
        assert!(r.unwrap_err().contains("first"));
    }
}
