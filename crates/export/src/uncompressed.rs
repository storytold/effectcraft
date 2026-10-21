//! Uncompressed QuickTime video (`raw `): 8-bit RGB ("Millions of Colors", depth 24) or ARGB
//! ("Millions of Colors+", depth 32), one frame per sample, muxed by FilmCraft's QuickTime writer
//! next to PCM audio like ProRes.
//!
//! Layout from the QuickTime File Format specification (Apple, "Video Sample Description" and
//! "Uncompressed RGB" formats): pixels are stored top to bottom, components big-endian in
//! A R G B / R G B order, and each row is padded to an even number of bytes (only depth 24 at odd
//! widths needs it).

use filmcraft_isobmff::{CodecConfig, FourCc, SampleEntry, VideoParams};

use crate::encode::{Packet, VideoEncoder};
use crate::{ExportError, Result};

pub(crate) struct Raw {
    w: u32,
    h: u32,
    alpha: bool,
}

impl Raw {
    /// `alpha`: write ARGB (depth 32); otherwise RGB (depth 24).
    pub(crate) fn new(w: u32, h: u32, alpha: bool) -> Result<Raw> {
        if w == 0 || h == 0 || w > u16::MAX as u32 || h > u16::MAX as u32 {
            return Err(ExportError::Unsupported(format!("uncompressed QuickTime frames must be 1–65535 px, not {w}×{h}")));
        }
        Ok(Raw { w, h, alpha })
    }

    fn depth(&self) -> u16 {
        if self.alpha { 32 } else { 24 }
    }

    /// Bytes per stored row (even).
    fn row_bytes(&self) -> usize {
        let n = self.w as usize * if self.alpha { 4 } else { 3 };
        n + (n & 1)
    }

    /// The `raw ` video sample description payload (after the 8-byte box header).
    fn description(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(78);
        b.extend_from_slice(&[0; 6]); // reserved
        b.extend_from_slice(&1u16.to_be_bytes()); // data reference index
        b.extend_from_slice(&[0; 4]); // version, revision level
        b.extend_from_slice(&[0; 4]); // vendor
        b.extend_from_slice(&0u32.to_be_bytes()); // temporal quality
        b.extend_from_slice(&1024u32.to_be_bytes()); // spatial quality: codecLosslessQuality
        b.extend_from_slice(&(self.w as u16).to_be_bytes());
        b.extend_from_slice(&(self.h as u16).to_be_bytes());
        b.extend_from_slice(&0x0048_0000u32.to_be_bytes()); // 72 dpi
        b.extend_from_slice(&0x0048_0000u32.to_be_bytes());
        b.extend_from_slice(&0u32.to_be_bytes()); // data size
        b.extend_from_slice(&1u16.to_be_bytes()); // frames per sample
        let name = b"None";
        let mut nb = [0u8; 32];
        nb[0] = name.len() as u8;
        nb[1..=name.len()].copy_from_slice(name);
        b.extend_from_slice(&nb);
        b.extend_from_slice(&self.depth().to_be_bytes());
        b.extend_from_slice(&(-1i16).to_be_bytes()); // no colour table
        b
    }
}

impl VideoEncoder for Raw {
    fn sample_entry(&self) -> SampleEntry {
        let fourcc = FourCc(*b"raw ");
        SampleEntry {
            format: fourcc,
            data_reference_index: 1,
            // Written verbatim; `video` makes the track a video track with the right size.
            codec: CodecConfig::Unknown { fourcc, raw: self.description() },
            video: Some(VideoParams { width: self.w as u16, height: self.h as u16, depth: self.depth(), ..Default::default() }),
            audio: None,
            bitrate: None,
        }
    }

    fn encode(&mut self, rgba: &[u8], _index: u64) -> Result<Vec<Packet>> {
        let (w, h) = (self.w as usize, self.h as usize);
        let row = self.row_bytes();
        let size = row.checked_mul(h).ok_or_else(|| ExportError::Encode("frame too large".into()))?;
        let mut data = vec![0u8; size];
        for (y, out) in data.chunks_exact_mut(row).enumerate() {
            let src = rgba.get(y * w * 4..(y + 1) * w * 4).ok_or_else(|| ExportError::Encode("short frame".into()))?;
            for (p, o) in src.as_chunks::<4>().0.iter().zip(out.chunks_exact_mut(if self.alpha { 4 } else { 3 })) {
                if self.alpha {
                    o.copy_from_slice(&[p[3], p[0], p[1], p[2]]);
                } else {
                    o.copy_from_slice(&p[..3]);
                }
            }
        }
        Ok(vec![Packet { data, key: true, cto: 0 }])
    }

    fn flush(&mut self) -> Result<Vec<Packet>> {
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_rows_are_padded_to_even_bytes() {
        let mut r = Raw::new(3, 2, false).unwrap();
        let rgba: Vec<u8> = (0..24).collect();
        let p = r.encode(&rgba, 0).unwrap();
        assert_eq!(p[0].data, [0, 1, 2, 4, 5, 6, 8, 9, 10, 0, 12, 13, 14, 16, 17, 18, 20, 21, 22, 0]);
        assert_eq!(r.sample_entry().video.unwrap().depth, 24);
    }

    #[test]
    fn argb_moves_alpha_first() {
        let mut r = Raw::new(1, 1, true).unwrap();
        assert_eq!(r.encode(&[10, 20, 30, 40], 0).unwrap()[0].data, [40, 10, 20, 30]);
        let e = r.sample_entry();
        let CodecConfig::Unknown { raw, .. } = e.codec else { panic!("raw entry") };
        assert_eq!(raw.len(), 78);
        assert_eq!(&raw[74..76], &32u16.to_be_bytes());
    }

    #[test]
    fn hostile_sizes_are_errors() {
        assert!(Raw::new(0, 1, false).is_err());
        assert!(Raw::new(70_000, 1, true).is_err());
        assert!(Raw::new(2, 2, true).unwrap().encode(&[0; 4], 0).is_err());
    }
}
