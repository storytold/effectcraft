//! RBSP bit writer (7.2, 9.2 Exp-Golomb) and NAL unit encapsulation with emulation prevention (7.3.1).

/// MSB-first bit writer.
#[derive(Default)]
pub struct BitWriter {
    buf: Vec<u8>,
    acc: u64,
    nbits: u32,
}

impl BitWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Write the `n` (<= 32) low bits of `v`.
    #[inline]
    pub fn put(&mut self, v: u32, n: u32) {
        debug_assert!(n <= 32);
        if n == 0 {
            return;
        }
        let v = (v as u64) & ((1u64 << n) - 1);
        self.acc = (self.acc << n) | v;
        self.nbits += n;
        while self.nbits >= 8 {
            self.nbits -= 8;
            self.buf.push((self.acc >> self.nbits) as u8);
        }
        self.acc &= (1u64 << self.nbits) - 1;
    }

    #[inline]
    pub fn bit(&mut self, b: u32) {
        self.put(b & 1, 1);
    }

    pub fn flag(&mut self, b: bool) {
        self.put(b as u32, 1);
    }

    /// ue(v).
    pub fn ue(&mut self, v: u32) {
        let x = v as u64 + 1;
        let len = 64 - x.leading_zeros(); // bits of x
        // len - 1 leading zeros, then x in len bits
        self.put(0, len - 1);
        if len > 32 {
            self.put((x >> 32) as u32, len - 32);
            self.put(x as u32, 32);
        } else {
            self.put(x as u32, len);
        }
    }

    /// se(v).
    pub fn se(&mut self, v: i32) {
        let k = if v > 0 { 2 * v as u32 - 1 } else { (-(v as i64) as u32) * 2 };
        self.ue(k);
    }

    pub fn is_aligned(&self) -> bool {
        self.nbits == 0
    }

    /// rbsp_trailing_bits(): stop bit and zero alignment.
    pub fn trailing(&mut self) {
        self.bit(1);
        self.align_zero();
    }

    pub fn align_zero(&mut self) {
        if self.nbits > 0 {
            self.put(0, 8 - self.nbits);
        }
    }

    pub fn into_bytes(mut self) -> Vec<u8> {
        self.align_zero();
        self.buf
    }
}

/// Build a NAL unit (2-byte header, `nuh_layer_id` 0, `TemporalId` 0) from an RBSP, inserting
/// emulation prevention bytes.
pub fn nal_unit(nal_type: u8, rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rbsp.len() + rbsp.len() / 64 + 4);
    out.push(nal_type << 1);
    out.push(1);
    let mut zeros = 0;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        if b == 0 {
            zeros += 1;
        } else {
            zeros = 0;
        }
    }
    // cabac_zero_words / trailing zero bytes never occur: every RBSP ends with the stop bit.
    out
}

/// Size of `data` with emulation prevention bytes inserted, starting after a non-zero byte (WPP
/// substreams and the slice header each end in one: the alignment bit is 1).
pub fn escaped_len(data: &[u8]) -> usize {
    let (mut n, mut zeros) = (data.len(), 0);
    for &b in data {
        if zeros >= 2 && b <= 3 {
            n += 1;
            zeros = 0;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    n
}

pub const NAL_TRAIL_R: u8 = 1;
pub const NAL_IDR_N_LP: u8 = 20;
pub const NAL_VPS: u8 = 32;
pub const NAL_SPS: u8 = 33;
pub const NAL_PPS: u8 = 34;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_golomb() {
        let mut w = BitWriter::new();
        w.ue(0); // 1
        w.ue(1); // 010
        w.ue(2); // 011
        w.ue(3); // 00100
        w.se(-1); // ue(2) = 011
        w.trailing();
        // 1 010 011 00100 011 + stop bit: 1010 0110 0100 0111
        assert_eq!(w.into_bytes(), vec![0xa6, 0x47]);
    }

    #[test]
    fn emulation_prevention() {
        let raw = [0, 0, 1, 0, 0, 0, 0, 0, 4];
        let n = nal_unit(1, &raw);
        assert_eq!(&n[2..], &[0, 0, 3, 1, 0, 0, 3, 0, 0, 3, 0, 4]);
        assert_eq!(escaped_len(&raw), n.len() - 2);
    }
}
