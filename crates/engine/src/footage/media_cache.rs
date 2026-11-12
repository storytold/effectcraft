//! The media cache (Settings ▸ Disk ▸ Database and Cache Folder): audio waveform summaries kept
//! on disk so a footage file is summarised once, not in every session.
//!
//! A summary is stored as `Peaks/<hash>_<bins>.ecpk` (raw little-endian `f32` quadruples
//! `[min L, max L, min R, max R]` per bin), named by a hash of the footage path, its size and
//! modification time, so an edited file is summarised again.

use std::path::{Path, PathBuf};

/// The peak file of footage `path` at `bins` bins per second in the media cache `folder`
/// (`None` when the file can't be inspected).
pub fn peaks_path(folder: &Path, path: &str, bins: u32) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let meta = std::fs::metadata(path).ok()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (path, meta.len(), meta.modified().ok()).hash(&mut h);
    Some(folder.join("Peaks").join(format!("{:016x}_{bins}.ecpk", h.finish())))
}

/// Read a stored summary.
pub fn load_peaks(file: &Path) -> Option<Vec<[f32; 4]>> {
    let b = std::fs::read(file).ok()?;
    if b.len() % 16 != 0 {
        return None;
    }
    let f = |c: &[u8]| f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
    Some(b.as_chunks::<16>().0.iter().map(|c| [f(&c[0..4]), f(&c[4..8]), f(&c[8..12]), f(&c[12..16])]).collect())
}

/// Store a summary (atomically: written beside, then renamed).
pub fn store_peaks(file: &Path, peaks: &[[f32; 4]]) -> std::io::Result<()> {
    if let Some(d) = file.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut b = Vec::with_capacity(peaks.len() * 16);
    for p in peaks {
        for v in p {
            b.extend_from_slice(&v.to_le_bytes());
        }
    }
    crate::config::atomic_write(file, &b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peaks_round_trip_and_follow_the_file() {
        let d = std::env::temp_dir().join(format!("ec-media-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let src = d.join("a.wav");
        std::fs::write(&src, b"one").unwrap();
        let p = peaks_path(&d, src.to_str().unwrap(), 200).unwrap();
        assert!(p.starts_with(d.join("Peaks")));
        let peaks = vec![[-0.5, 0.5, -0.25, 0.25], [0.0, 0.1, -0.1, 0.0]];
        store_peaks(&p, &peaks).unwrap();
        assert_eq!(load_peaks(&p).unwrap(), peaks);
        // A changed file gets another name.
        std::fs::write(&src, b"longer").unwrap();
        assert_ne!(peaks_path(&d, src.to_str().unwrap(), 200).unwrap(), p);
        let _ = std::fs::remove_dir_all(&d);
    }
}
