//! The boolean entropy encoder (RFC 6386 §7) and the cost of coding a bool.

use std::sync::OnceLock;

/// Writes bools at 8-bit probabilities, as §7.3 describes: `bottom` and `range` are the coding
/// interval, `bit_count` the shifts left before a byte of `bottom` is ready.
pub(crate) struct BoolWriter {
    out: Vec<u8>,
    range: u32,
    bottom: u32,
    bit_count: i32,
}

impl BoolWriter {
    pub(crate) fn new() -> BoolWriter {
        BoolWriter { out: Vec::new(), range: 255, bottom: 0, bit_count: 24 }
    }

    /// A bool whose probability of being false is `prob / 256`.
    pub(crate) fn put(&mut self, prob: u8, bit: bool) {
        let split = 1 + (((self.range - 1) * u32::from(prob)) >> 8);
        if bit {
            self.bottom = self.bottom.wrapping_add(split);
            self.range -= split;
        } else {
            self.range = split;
        }
        while self.range < 128 {
            self.range <<= 1;
            if self.bottom & (1 << 31) != 0 {
                self.carry();
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

    /// An unsigned `bits`-wide literal, high bit first, each bit at probability 1/2.
    pub(crate) fn literal(&mut self, bits: u32, value: u32) {
        for b in (0..bits).rev() {
            self.put(128, (value >> b) & 1 == 1);
        }
    }

    /// The bytes written, the interval's remainder appended (§7.3, flush).
    pub(crate) fn finish(mut self) -> Vec<u8> {
        let mut c = self.bit_count;
        let mut v = self.bottom;
        if c < 32 && v & (1 << (32 - c)) != 0 {
            self.carry();
        }
        v <<= c & 7;
        c >>= 3;
        while c > 0 {
            v <<= 8;
            c -= 1;
        }
        for _ in 0..4 {
            self.out.push((v >> 24) as u8);
            v <<= 8;
        }
        self.out
    }

    /// Adds one to the bytes already written. The coded value is always less than one, so the
    /// carry never runs off the front.
    fn carry(&mut self) {
        for b in self.out.iter_mut().rev() {
            if *b == 255 {
                *b = 0;
            } else {
                *b += 1;
                return;
            }
        }
    }
}

/// Bits a bool costs at `prob`: −log2 of its probability.
pub(crate) fn cost(prob: u8, bit: bool) -> f32 {
    static TABLE: OnceLock<[f32; 256]> = OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0f32; 256];
        for (p, c) in t.iter_mut().enumerate() {
            *c = -((p.max(1) as f32) / 256.0).log2();
        }
        t
    });
    let p = if bit { 256 - usize::from(prob) } else { usize::from(prob) };
    t[p.min(255)]
}

/// Writes `leaf` of a tree in the RFC's array form (§8.1): even entries are pairs of branches, a
/// positive entry the index of a deeper pair, a non-positive one the leaf `-entry`. `probs` are
/// indexed by half the pair's index. Returns false when the tree has no such leaf.
pub(crate) fn put_tree(w: &mut BoolWriter, tree: &[i8], probs: &[u8], leaf: u8) -> bool {
    let mut path = Vec::with_capacity(12);
    if !find(tree, 0, leaf, &mut path, 0) {
        return false;
    }
    for (node, bit) in path {
        w.put(probs.get(node / 2).copied().unwrap_or(128), bit);
    }
    true
}

/// Bits [`put_tree`] would spend.
pub(crate) fn tree_cost(tree: &[i8], probs: &[u8], leaf: u8) -> f32 {
    let mut path = Vec::with_capacity(12);
    if !find(tree, 0, leaf, &mut path, 0) {
        return 0.0;
    }
    path.iter().map(|&(node, bit)| cost(probs.get(node / 2).copied().unwrap_or(128), bit)).sum()
}

fn find(tree: &[i8], node: usize, leaf: u8, path: &mut Vec<(usize, bool)>, depth: usize) -> bool {
    if depth > 16 {
        return false;
    }
    for bit in [false, true] {
        let Some(&entry) = tree.get(node + usize::from(bit)) else { return false };
        path.push((node, bit));
        let hit = if entry > 0 { find(tree, entry as usize, leaf, path, depth + 1) } else { (-i16::from(entry)) as u8 == leaf };
        if hit {
            return true;
        }
        path.pop();
    }
    false
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The decoder of §7.3, to read back what the writer wrote.
    pub(crate) struct BoolReader<'a> {
        data: &'a [u8],
        pos: usize,
        value: u32,
        range: u32,
        bit_count: u32,
    }

    impl<'a> BoolReader<'a> {
        pub(crate) fn new(data: &'a [u8]) -> Self {
            let byte = |i: usize| u32::from(data.get(i).copied().unwrap_or(0));
            BoolReader { data, pos: 2, value: (byte(0) << 8) | byte(1), range: 255, bit_count: 0 }
        }
        pub(crate) fn get(&mut self, prob: u8) -> bool {
            let split = 1 + (((self.range - 1) * u32::from(prob)) >> 8);
            let big = split << 8;
            let bit = if self.value >= big {
                self.range -= split;
                self.value -= big;
                true
            } else {
                self.range = split;
                false
            };
            while self.range < 128 {
                self.value <<= 1;
                self.range <<= 1;
                self.bit_count += 1;
                if self.bit_count == 8 {
                    self.bit_count = 0;
                    self.value |= u32::from(self.data.get(self.pos).copied().unwrap_or(0));
                    self.pos += 1;
                }
            }
            bit
        }
    }

    #[test]
    fn bools_read_back() {
        let mut w = BoolWriter::new();
        let mut want = Vec::new();
        let mut x = 12345u32;
        for i in 0..20000 {
            x = x.wrapping_mul(1103515245).wrapping_add(12345);
            let prob = ((x >> 16) % 255 + 1) as u8;
            let bit = (x >> 8) % 256 >= u32::from(prob) || i % 97 == 0;
            w.put(prob, bit);
            want.push((prob, bit));
        }
        let data = w.finish();
        let mut r = BoolReader::new(&data);
        for (i, (prob, bit)) in want.into_iter().enumerate() {
            assert_eq!(r.get(prob), bit, "bool {i}");
        }
    }

    #[test]
    fn trees_write_their_leaves() {
        // kf_ymode_tree (§11.2): B_PRED = 0, DC = 100, V = 101, H = 110, TM = 111.
        const TREE: [i8; 8] = [-4, 2, 4, 6, 0, -1, -2, -3];
        for leaf in 0..5u8 {
            let mut w = BoolWriter::new();
            assert!(put_tree(&mut w, &TREE, &[145, 156, 163, 128], leaf));
            let data = w.finish();
            let mut r = BoolReader::new(&data);
            let read = if !r.get(145) {
                4
            } else if !r.get(156) {
                u8::from(r.get(163))
            } else {
                2 + u8::from(r.get(128))
            };
            assert_eq!(read, leaf);
        }
        assert!(!put_tree(&mut BoolWriter::new(), &TREE, &[128; 4], 9));
    }

    #[test]
    fn costs_are_information() {
        assert!((cost(128, false) - 1.0).abs() < 1e-6);
        assert!((cost(192, false) - 0.415).abs() < 1e-3);
        assert!((cost(192, true) - 2.0).abs() < 1e-6);
    }
}
