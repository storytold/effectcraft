//! Boolean (arithmetic) encoder: the inverse of the VP9 boolean decoder (spec §9.2), written as
//! the classic carry-propagating range coder described in RFC 6386 §7.3 (VP8 and VP9 share the
//! bool coder).

pub struct BoolEncoder {
    out: Vec<u8>,
    range: u32,
    bottom: u32,
    bit_count: i32,
    /// Start of the run of 0xFF bytes at the end of `out` (`out.len()` when it ends in another
    /// byte). A carry only touches that run and the byte before it.
    ff_start: usize,
}

impl Default for BoolEncoder {
    fn default() -> Self {
        BoolEncoder::new()
    }
}

impl BoolEncoder {
    pub fn new() -> Self {
        BoolEncoder { out: Vec::new(), range: 255, bottom: 0, bit_count: 24, ff_start: 0 }
    }

    fn push_byte(&mut self, b: u8) {
        if b != 255 {
            self.ff_start = self.out.len() + 1;
        }
        self.out.push(b);
    }

    /// Carry into the bytes written so far: the trailing 0xFF run becomes 0x00 and the byte
    /// before it grows by one (a carry out of the first byte is dropped). Each 0xFF byte is
    /// cleared once, so a long run costs O(1) per byte instead of a rescan for every carry.
    fn add_one(&mut self) {
        let end = self.out.len();
        let start = self.ff_start.min(end);
        if let Some(run) = self.out.get_mut(start..) {
            run.fill(0);
        }
        self.ff_start = end;
        if let Some(i) = start.checked_sub(1)
            && let Some(b) = self.out.get_mut(i)
        {
            *b = b.wrapping_add(1);
            if *b == 255 {
                // The grown byte joins the 0xFF bytes before it.
                let mut s = i;
                while s > 0 && self.out.get(s - 1) == Some(&255) {
                    s -= 1;
                }
                self.ff_start = s;
            }
        }
    }

    /// Write `bit` with probability `prob`/256 of it being 0.
    #[inline]
    pub fn write(&mut self, bit: bool, prob: u8) {
        let split = 1 + (((self.range - 1) * prob as u32) >> 8);
        if bit {
            self.bottom = self.bottom.wrapping_add(split);
            self.range -= split;
        } else {
            self.range = split;
        }
        while self.range < 128 {
            self.range <<= 1;
            if self.bottom & (1 << 31) != 0 {
                self.add_one();
            }
            self.bottom <<= 1;
            self.bit_count -= 1;
            if self.bit_count == 0 {
                self.push_byte((self.bottom >> 24) as u8);
                self.bottom &= (1 << 24) - 1;
                self.bit_count = 8;
            }
        }
    }

    pub fn literal(&mut self, v: u32, bits: u32) {
        for i in (0..bits).rev() {
            self.write((v >> i) & 1 != 0, 128);
        }
    }

    /// Flush (RFC 6386 §7.3) and return the bytes.
    pub fn finish(mut self) -> Vec<u8> {
        let mut c = self.bit_count;
        let mut v = self.bottom;
        if v & (1u32 << (32 - c)) != 0 {
            self.add_one();
        }
        v <<= c & 7;
        c >>= 3;
        while {
            c -= 1;
            c >= 0
        } {
            v <<= 8;
        }
        for _ in 0..4 {
            self.push_byte((v >> 24) as u8);
            v <<= 8;
        }
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A straightforward decoder per VP9 spec §9.2.
    struct Dec<'a> {
        d: &'a [u8],
        pos: usize,
        value: u64,
        bits: i32,
        range: u32,
    }
    impl<'a> Dec<'a> {
        fn new(d: &'a [u8]) -> Self {
            let mut x = Dec { d, pos: 0, value: 0, bits: 0, range: 255 };
            x.fill();
            x
        }
        fn fill(&mut self) {
            while self.bits <= 48 {
                let b = if self.pos < self.d.len() { self.d[self.pos] } else { 0 };
                self.pos += 1;
                self.value |= (b as u64) << (56 - self.bits);
                self.bits += 8;
            }
        }
        fn read(&mut self, p: u8) -> bool {
            let split = 1 + (((self.range - 1) * p as u32) >> 8);
            let big = (split as u64) << 56;
            let bit = if self.value >= big {
                self.range -= split;
                self.value -= big;
                true
            } else {
                self.range = split;
                false
            };
            while self.range < 128 {
                self.range <<= 1;
                self.value <<= 1;
                self.bits -= 1;
            }
            self.fill();
            bit
        }
    }

    #[test]
    fn round_trip() {
        let mut e = BoolEncoder::new();
        let mut seq = vec![];
        let mut s = 12345u32;
        for i in 0..20000 {
            s = s.wrapping_mul(1103515245).wrapping_add(12345);
            let p = ((s >> 8) % 255 + 1) as u8;
            let bit = (s >> 20) % 256 >= p as u32 || i % 97 == 0;
            seq.push((bit, p));
            e.write(bit, p);
        }
        let d = e.finish();
        let mut dec = Dec::new(&d);
        for (i, (b, p)) in seq.into_iter().enumerate() {
            assert_eq!(dec.read(p), b, "bit {i}");
        }
    }

    #[test]
    fn all_ones_carry() {
        let mut e = BoolEncoder::new();
        for _ in 0..5000 {
            e.write(true, 1);
        }
        let d = e.finish();
        let mut dec = Dec::new(&d);
        for _ in 0..5000 {
            assert!(dec.read(1));
        }
    }

    /// The encoder as it was before the carry run was tracked: a carry rescans the bytes.
    struct Scan {
        out: Vec<u8>,
        range: u32,
        bottom: u32,
        bit_count: i32,
    }
    impl Scan {
        fn add_one(&mut self) {
            let mut i = self.out.len();
            while i > 0 {
                i -= 1;
                if self.out[i] == 255 {
                    self.out[i] = 0;
                } else {
                    self.out[i] += 1;
                    return;
                }
            }
        }
        fn write(&mut self, bit: bool, prob: u8) {
            let split = 1 + (((self.range - 1) * prob as u32) >> 8);
            if bit {
                self.bottom = self.bottom.wrapping_add(split);
                self.range -= split;
            } else {
                self.range = split;
            }
            while self.range < 128 {
                self.range <<= 1;
                if self.bottom & (1 << 31) != 0 {
                    self.add_one();
                }
                self.bottom <<= 1;
                self.bit_count -= 1;
                if self.bit_count == 0 {
                    self.out.push((self.bottom >> 24) as u8);
                    self.bottom &= (1 << 24) - 1;
                    self.bit_count = 8;
                }
            }
        }
    }

    #[test]
    fn carry_run_tracking_matches_the_rescan() {
        let mut seed = 99u32;
        for case in 0..300 {
            let mut fast = BoolEncoder::new();
            let mut slow = Scan { out: vec![], range: 255, bottom: 0, bit_count: 24 };
            // Skewed probabilities make long 0xFF runs and carries likely.
            let extreme = case % 3;
            for _ in 0..4000 {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let p = match extreme {
                    0 => (seed >> 8) as u8 | 1,
                    1 => 1 + (seed >> 8) as u8 % 4,
                    _ => 255 - (seed >> 8) as u8 % 4,
                };
                let bit = (seed >> 24) % 256 >= p as u32 || (seed >> 3).is_multiple_of(11);
                fast.write(bit, p);
                slow.write(bit, p);
                assert_eq!(fast.out, slow.out, "case {case}");
            }
        }
    }
}
