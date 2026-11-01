//! Probabilities: the frame's adaptable probability table (one flat array laid out in the order
//! the compressed header updates them), a symbol recorder that codes every boolean decision
//! against either a fixed probability or an entry of that table, bit-cost estimates, and the
//! per-frame forward updates (spec 6.3: `diff_update_prob` with `decode_term_subexp` /
//! `inv_remap_prob`, and `update_mv_prob`).
//!
//! Tiles are first recorded, then the encoder counts how often each table entry coded a 0 or a
//! 1, picks updates that pay for themselves, writes them in the compressed header and finally
//! entropy codes the recorded symbols with the updated probabilities.

use crate::bool::BoolEncoder;
use crate::tables::{
    DEFAULT_COEF_PROBS, DEFAULT_INTER_MODE_PROBS, DEFAULT_IS_INTER_PROB, DEFAULT_MV_BITS_PROB, DEFAULT_MV_CLASS_PROBS, DEFAULT_MV_CLASS0_BIT_PROB,
    DEFAULT_MV_CLASS0_FR_PROBS, DEFAULT_MV_FR_PROBS, DEFAULT_MV_JOINT_PROBS, DEFAULT_MV_SIGN_PROB, DEFAULT_PARTITION_PROBS, DEFAULT_SINGLE_REF_PROB,
    DEFAULT_SKIP_PROB, PARETO_TABLE, inv_map_table,
};

/// Coefficient probabilities `[ptype][ref][band][ctx][3]` for 4×4 transforms (band 0 uses
/// contexts 0..3 only).
pub const COEF: usize = 0;
pub const SKIP: usize = 432;
pub const INTER_MODE: usize = SKIP + 3;
pub const IS_INTER: usize = INTER_MODE + 21;
pub const SINGLE_REF: usize = IS_INTER + 4;
pub const PARTITION: usize = SINGLE_REF + 10;
pub const MV_JOINT: usize = PARTITION + 48;
pub const MV_SIGN: usize = MV_JOINT + 3;
pub const MV_CLASS: usize = MV_SIGN + 2;
pub const MV_CLASS0_BIT: usize = MV_CLASS + 20;
pub const MV_BITS: usize = MV_CLASS0_BIT + 2;
pub const MV_CLASS0_FR: usize = MV_BITS + 20;
pub const MV_FR: usize = MV_CLASS0_FR + 12;
pub const NPROBS: usize = MV_FR + 6;

/// Index of the first of the three coefficient probabilities of a context.
pub fn coef_index(ptype: usize, inter: bool, band: usize, ctx: usize) -> usize {
    COEF + (((ptype * 2 + inter as usize) * 6 + band) * 6 + ctx) * 3
}

#[derive(Clone)]
pub struct Probs(pub [u8; NPROBS]);

impl Probs {
    /// The default probabilities (setup_past_independence / key frames).
    pub fn defaults() -> Probs {
        let mut p = [128u8; NPROBS];
        p[COEF..COEF + 432].copy_from_slice(&DEFAULT_COEF_PROBS);
        p[SKIP..SKIP + 3].copy_from_slice(&DEFAULT_SKIP_PROB);
        p[INTER_MODE..INTER_MODE + 21].copy_from_slice(&DEFAULT_INTER_MODE_PROBS);
        p[IS_INTER..IS_INTER + 4].copy_from_slice(&DEFAULT_IS_INTER_PROB);
        p[SINGLE_REF..SINGLE_REF + 10].copy_from_slice(&DEFAULT_SINGLE_REF_PROB);
        p[PARTITION..PARTITION + 48].copy_from_slice(&DEFAULT_PARTITION_PROBS);
        p[MV_JOINT..MV_JOINT + 3].copy_from_slice(&DEFAULT_MV_JOINT_PROBS);
        p[MV_SIGN..MV_SIGN + 2].copy_from_slice(&DEFAULT_MV_SIGN_PROB);
        p[MV_CLASS..MV_CLASS + 20].copy_from_slice(&DEFAULT_MV_CLASS_PROBS);
        p[MV_CLASS0_BIT..MV_CLASS0_BIT + 2].copy_from_slice(&DEFAULT_MV_CLASS0_BIT_PROB);
        p[MV_BITS..MV_BITS + 20].copy_from_slice(&DEFAULT_MV_BITS_PROB);
        p[MV_CLASS0_FR..MV_CLASS0_FR + 12].copy_from_slice(&DEFAULT_MV_CLASS0_FR_PROBS);
        p[MV_FR..MV_FR + 6].copy_from_slice(&DEFAULT_MV_FR_PROBS);
        Probs(p)
    }
}

/// Probabilities of the full token tree (9.3.2) for model probability `p`.
pub fn pareto(p: u8) -> [u8; 8] {
    let p = p.max(1) as usize;
    let x = (p - 1) / 2;
    let mut t = [0u8; 8];
    for n in 0..8 {
        t[n] = if p & 1 == 1 { PARETO_TABLE[x * 8 + n] } else { ((PARETO_TABLE[x * 8 + n] as u32 + PARETO_TABLE[(x + 1) * 8 + n] as u32) >> 1) as u8 };
    }
    t
}

/// The probability a recorded symbol is coded with.
#[derive(Clone, Copy, Debug)]
pub enum P {
    Fixed(u8),
    /// An entry of the frame's [`Probs`].
    Ctx(u16),
    /// Node `n` of the Pareto token tree derived from the coefficient probability at the index.
    Pareto(u16, u8),
}

#[derive(Clone, Copy)]
struct Sym {
    bit: bool,
    p: P,
}

/// Records the boolean decisions of a tile.
#[derive(Default)]
pub struct Writer {
    syms: Vec<Sym>,
}

impl Writer {
    #[inline]
    pub fn put(&mut self, bit: bool, p: P) {
        self.syms.push(Sym { bit, p });
    }
    #[inline]
    pub fn fixed(&mut self, bit: bool, prob: u8) {
        self.put(bit, P::Fixed(prob));
    }
    #[inline]
    pub fn ctx(&mut self, bit: bool, idx: usize) {
        self.put(bit, P::Ctx(idx as u16));
    }
    /// Write `value` with `tree` (leaves stored negated); node `i` uses `prob(i)`.
    pub fn tree(&mut self, tree: &[i8], value: u8, prob: impl Fn(usize) -> P) {
        for (node, bit) in tree_path(tree, value) {
            self.put(bit, prob(node));
        }
    }

    /// Add the 0/1 counts of every table entry this tile used.
    pub fn count(&self, counts: &mut [[u32; 2]]) {
        for s in &self.syms {
            if let P::Ctx(i) = s.p {
                counts[i as usize][s.bit as usize] += 1;
            }
        }
    }

    /// Entropy code the recorded symbols.
    pub fn emit(&self, probs: &Probs) -> Vec<u8> {
        let mut bc = BoolEncoder::new();
        bc.write(false, 128); // marker bit
        for s in &self.syms {
            let p = match s.p {
                P::Fixed(p) => p,
                P::Ctx(i) => probs.0[i as usize],
                P::Pareto(i, n) => pareto(probs.0[i as usize])[n as usize],
            };
            bc.write(s.bit, p);
        }
        bc.finish()
    }
}

/// The (node, bit) path to `value` in a tree (node = tree index / 2).
pub fn tree_path(tree: &[i8], value: u8) -> impl Iterator<Item = (usize, bool)> {
    struct Path {
        steps: [(u8, bool); 16],
        len: usize,
    }
    fn walk(tree: &[i8], node: usize, value: u8, out: &mut Path) -> bool {
        for b in 0..2 {
            let t = tree[node + b];
            out.steps[out.len] = ((node >> 1) as u8, b == 1);
            out.len += 1;
            if t <= 0 {
                if (-t) as u8 == value {
                    return true;
                }
            } else if walk(tree, t as usize, value, out) {
                return true;
            }
            out.len -= 1;
        }
        false
    }
    let mut p = Path { steps: [(0, false); 16], len: 0 };
    walk(tree, 0, value, &mut p);
    let Path { steps, len } = p;
    steps.into_iter().take(len).map(|(n, b)| (n as usize, b))
}

// ---------------------------------------------------------------- costs

/// Cost in 1/256 bit of coding a 0 (`[p][0]`) or a 1 (`[p][1]`) with probability `p`.
pub fn cost_table() -> &'static [[u32; 2]; 256] {
    use std::sync::OnceLock;
    static T: OnceLock<[[u32; 2]; 256]> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = [[0u32; 2]; 256];
        for (p, e) in t.iter_mut().enumerate() {
            let q = (p.max(1) as f64) / 256.0;
            e[0] = (-q.log2() * 256.0).round() as u32;
            e[1] = (-(1.0 - q).log2() * 256.0).round() as u32;
        }
        t
    })
}

#[inline]
pub fn cost(bit: bool, p: u8) -> u32 {
    cost_table()[p as usize][bit as usize]
}

/// Cost of `value` in a tree with probabilities `probs` (node i → probs[i]).
pub fn tree_cost(tree: &[i8], probs: &[u8], value: u8) -> u32 {
    tree_path(tree, value).map(|(n, b)| cost(b, probs[n])).sum()
}

// ---------------------------------------------------------------- forward updates

fn inv_recenter_nonneg(v: i32, m: i32) -> i32 {
    if v > 2 * m {
        v
    } else if v & 1 != 0 {
        m - ((v + 1) >> 1)
    } else {
        m + (v >> 1)
    }
}

/// `inv_remap_prob` (6.3.5).
fn inv_remap_prob(delta: usize, prob: u8) -> u8 {
    let v = inv_map_table()[delta.min(254)] as i32;
    let m = prob as i32 - 1;
    let r = if (m << 1) <= 255 { 1 + inv_recenter_nonneg(v, m) } else { 255 - inv_recenter_nonneg(v, 255 - 1 - m) };
    r.clamp(1, 255) as u8
}

/// The `decode_term_subexp` value that moves `old` to `new`.
fn delta_for(old: u8, new: u8) -> Option<usize> {
    match delta_table().get(old as usize * 256 + new as usize) {
        Some(&d) if d != NO_DELTA => Some(d as usize),
        _ => None,
    }
}

/// Marks "no delta reaches this probability" in [`delta_table`].
const NO_DELTA: u8 = 255;

/// `delta_for` for every (old, new) pair, indexed `old * 256 + new`: the smallest delta that
/// moves `old` to `new`. Filled from the largest delta down so the smallest one wins.
fn delta_table() -> &'static [u8] {
    use std::sync::OnceLock;
    static T: OnceLock<Vec<u8>> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = vec![NO_DELTA; 256 * 256];
        for old in 0..=255u8 {
            for d in (0..255usize).rev() {
                let new = inv_remap_prob(d, old);
                if let Some(e) = t.get_mut(old as usize * 256 + new as usize) {
                    *e = d as u8;
                }
            }
        }
        t
    })
}

/// Bits of `decode_term_subexp` for `d`.
fn subexp_bits(d: usize) -> u32 {
    match d {
        0..16 => 5,
        16..32 => 6,
        32..64 => 8,
        64..129 => 10,
        _ => 11,
    }
}

fn write_subexp(bc: &mut BoolEncoder, d: usize) {
    let d = d as u32;
    if d < 16 {
        bc.literal(0, 1);
        bc.literal(d, 4);
    } else if d < 32 {
        bc.literal(2, 2);
        bc.literal(d - 16, 4);
    } else if d < 64 {
        bc.literal(6, 3);
        bc.literal(d - 32, 5);
    } else {
        bc.literal(7, 3);
        if d < 129 {
            bc.literal(d - 64, 7);
        } else {
            let v = (d + 1) >> 1;
            bc.literal(v, 7);
            bc.literal((d + 1) & 1, 1);
        }
    }
}

fn counts_cost(c: [u32; 2], p: u8) -> u64 {
    c[0] as u64 * cost(false, p) as u64 + c[1] as u64 * cost(true, p) as u64
}

/// Whether table entry `i` is an MV probability (7-bit `update_mv_prob` rather than
/// `diff_update_prob`).
fn is_mv(i: usize) -> bool {
    i >= MV_JOINT
}

/// Choose updated probabilities: an entry changes when the bits saved on this frame's symbols
/// exceed the cost of signalling the change. `inter`: the inter-frame tables are updatable.
pub fn plan_updates(old: &Probs, counts: &[[u32; 2]], inter: bool) -> Probs {
    let mut new = old.clone();
    let flag = cost(true, 252) as i64 - cost(false, 252) as i64;
    let limit = if inter { NPROBS } else { INTER_MODE };
    for i in 0..limit {
        let c = counts[i];
        let n = c[0] + c[1];
        if n == 0 {
            continue;
        }
        let o = old.0[i];
        let ideal = ((c[0] as f64 * 256.0 / n as f64).round() as i32).clamp(1, 255) as u8;
        let cands: Vec<u8> = if is_mv(i) { vec![ideal | 1, ideal.saturating_sub(1).max(1) | 1] } else { vec![ideal] };
        let base = counts_cost(c, o) as i64;
        let mut best = (0i64, o);
        for p in cands {
            if p == o {
                continue;
            }
            let signal = if is_mv(i) {
                7 * 256
            } else {
                match delta_for(o, p) {
                    Some(d) => subexp_bits(d) as i64 * 256,
                    None => continue,
                }
            };
            let gain = base - counts_cost(c, p) as i64 - signal - flag;
            if gain > best.0 {
                best = (gain, p);
            }
        }
        new.0[i] = best.1;
    }
    new
}

/// `diff_update_prob` for one entry.
fn diff_update(bc: &mut BoolEncoder, old: &Probs, new: &Probs, i: usize) {
    let (o, n) = (old.0[i], new.0[i]);
    match (o != n).then(|| delta_for(o, n)).flatten() {
        Some(d) => {
            bc.write(true, 252);
            write_subexp(bc, d);
        }
        None => bc.write(false, 252),
    }
}

/// `update_mv_prob` for one entry.
fn mv_update(bc: &mut BoolEncoder, old: &Probs, new: &Probs, i: usize) {
    let n = new.0[i];
    if n != old.0[i] && n & 1 == 1 {
        bc.write(true, 252);
        bc.literal(n as u32 >> 1, 7);
    } else {
        bc.write(false, 252);
    }
}

/// The compressed header (6.3) of a frame coded with ONLY_4X4 transforms, single LAST
/// references and a fixed interpolation filter, moving the probabilities from `old` to `new`.
/// Entries `new` cannot signal are left at `old` (and must be coded with `old`): the caller
/// codes the tiles with the probabilities this returns.
pub fn compressed_header(old: &Probs, new: &Probs, lossless: bool, inter: bool) -> (Vec<u8>, Probs) {
    let mut bc = BoolEncoder::new();
    bc.write(false, 128);
    if !lossless {
        bc.literal(0, 2); // tx_mode = ONLY_4X4
    }
    // read_coef_probs: TX_4X4 only.
    let coef_changed = (COEF..COEF + 432).any(|i| new.0[i] != old.0[i]);
    bc.literal(coef_changed as u32, 1);
    if coef_changed {
        for i in 0..2 {
            for j in 0..2 {
                for k in 0..6 {
                    for l in 0..if k == 0 { 3 } else { 6 } {
                        for m in 0..3 {
                            diff_update(&mut bc, old, new, coef_index(i, j == 1, k, l) + m);
                        }
                    }
                }
            }
        }
    }
    for i in 0..3 {
        diff_update(&mut bc, old, new, SKIP + i);
    }
    if inter {
        for i in 0..21 {
            diff_update(&mut bc, old, new, INTER_MODE + i);
        }
        for i in 0..4 {
            diff_update(&mut bc, old, new, IS_INTER + i);
        }
        // reference_mode is SINGLE_REFERENCE (no compound: equal sign biases).
        for i in 0..10 {
            diff_update(&mut bc, old, new, SINGLE_REF + i);
        }
        for _ in 0..36 {
            bc.write(false, 252); // y mode probabilities (no intra blocks in inter frames)
        }
        for i in 0..48 {
            diff_update(&mut bc, old, new, PARTITION + i);
        }
        for j in 0..3 {
            mv_update(&mut bc, old, new, MV_JOINT + j);
        }
        for i in 0..2 {
            mv_update(&mut bc, old, new, MV_SIGN + i);
            for j in 0..10 {
                mv_update(&mut bc, old, new, MV_CLASS + i * 10 + j);
            }
            mv_update(&mut bc, old, new, MV_CLASS0_BIT + i);
            for j in 0..10 {
                mv_update(&mut bc, old, new, MV_BITS + i * 10 + j);
            }
        }
        for i in 0..2 {
            for j in 0..6 {
                mv_update(&mut bc, old, new, MV_CLASS0_FR + i * 6 + j);
            }
            for j in 0..3 {
                mv_update(&mut bc, old, new, MV_FR + i * 3 + j);
            }
        }
    }
    // What the decoder ends up with.
    let mut used = old.clone();
    for i in 0..NPROBS {
        let ok = if is_mv(i) { new.0[i] & 1 == 1 } else { delta_for(old.0[i], new.0[i]).is_some() };
        if ok && (inter || i < INTER_MODE) {
            used.0[i] = new.0[i];
        }
    }
    (bc.finish(), used)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remap_reaches_every_probability() {
        for old in 1..=255u8 {
            for new in 1..=255u8 {
                if new != old {
                    assert!(delta_for(old, new).is_some(), "{old} -> {new}");
                }
            }
        }
    }

    #[test]
    fn delta_table_matches_the_linear_search() {
        for old in 0..=255u8 {
            for new in 0..=255u8 {
                let slow = (0..255).find(|&d| inv_remap_prob(d, old) == new);
                assert_eq!(delta_for(old, new), slow, "{old} -> {new}");
            }
        }
    }

    #[test]
    fn inv_map_table_shape() {
        let t = inv_map_table();
        assert_eq!(&t[..4], &[7, 20, 33, 46]);
        assert_eq!(t[19], 254);
        assert_eq!(&t[20..27], &[1, 2, 3, 4, 5, 6, 8]);
        assert_eq!(&t[252..], &[252, 253, 253]);
    }
}
