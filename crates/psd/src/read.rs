//! Big-endian byte reader.

use crate::{Error, Result};

#[derive(Clone)]
pub struct Reader<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }
    pub fn at(data: &'a [u8], pos: usize) -> Self {
        Reader { data, pos }
    }
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(Error::Truncated);
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        let b = self.bytes(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    pub fn i16(&mut self) -> Result<i16> {
        Ok(self.u16()? as i16)
    }
    pub fn u32(&mut self) -> Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }
    pub fn u64(&mut self) -> Result<u64> {
        let b = self.bytes(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_be_bytes(a))
    }
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }
    pub fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_bits(self.u64()?))
    }
    /// A big-endian double where NaN and ±infinity read as 0 (they are never meaningful in a file).
    pub fn f64_finite(&mut self) -> Result<f64> {
        let v = self.f64()?;
        Ok(if v.is_finite() { v } else { 0.0 })
    }
    pub fn tag(&mut self) -> Result<[u8; 4]> {
        let b = self.bytes(4)?;
        Ok([b[0], b[1], b[2], b[3]])
    }
    /// A length field: 4 bytes in PSD, 8 bytes in PSB (`wide`).
    pub fn length(&mut self, wide: bool) -> Result<usize> {
        let v = if wide { self.u64()? } else { self.u32()? as u64 };
        usize::try_from(v).map_err(|_| Error::Invalid("length".into()))
    }
    /// A Pascal string padded so that length byte + text is a multiple of `pad`.
    pub fn pascal(&mut self, pad: usize) -> Result<String> {
        let n = self.u8()? as usize;
        let s = self.bytes(n)?;
        let total = 1 + n;
        let padded = total.div_ceil(pad) * pad;
        self.skip(padded - total)?;
        // Pascal strings are in the system (Mac Roman) encoding; ASCII covers the common case.
        Ok(decode_mac_roman(s))
    }
}

/// Decode the legacy Pascal names in PSD and PSB.
pub(crate) fn decode_mac_roman(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&byte| {
            // Keep the ordinary space EffectCraft used for this byte in legacy Pascal names.
            if byte == 0xca { ' ' } else { craft_codepage::windows_byte(craft_codepage::MAC_ROMAN_CODE_PAGE, byte) }
        })
        .collect()
}
