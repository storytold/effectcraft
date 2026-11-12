//! Excitation coding (encoder side of RFC 6716 §4.2.7.8): rate level, per-block pulse counts
//! with LSB escapes, the recursive shell code, LSBs and signs, written in exactly the order the
//! decoder reads them.

use super::tables::{EXC_LSB, EXC_SIGN, PULSE_COUNT, RATE_LEVEL, SHELL_2, SHELL_4, SHELL_8, SHELL_16};
use crate::range::RangeEncoder;

/// Largest pulse magnitude the quantiser produces (keeps every 16-sample block codable).
pub const MAX_PULSE: i32 = 500;

pub fn icdf_bits(icdf: &[u8], s: usize) -> f32 {
    let hi = if s == 0 { 256 } else { icdf[s - 1] as u32 };
    let p = (hi - icdf[s] as u32).max(1) as f32 / 256.0;
    -p.log2()
}

fn count_table(nls: usize) -> &'static [u8] {
    if nls == 10 { PULSE_COUNT[10] } else { PULSE_COUNT[9] }
}

fn shell(enc: &mut RangeEncoder, v: &[i32], count: i32) {
    let n = v.len();
    if n == 1 || count == 0 {
        return;
    }
    let tab = match n {
        16 => SHELL_16[(count - 1) as usize],
        8 => SHELL_8[(count - 1) as usize],
        4 => SHELL_4[(count - 1) as usize],
        _ => SHELL_2[(count - 1) as usize],
    };
    let (a, b) = v.split_at(n / 2);
    let left: i32 = a.iter().sum();
    enc.icdf(left as usize, tab, 8);
    shell(enc, a, left);
    shell(enc, b, count - left);
}

/// Codes the frame's pulses (`len` a multiple of 16).
pub fn encode(enc: &mut RangeEncoder, pulses: &[i32], signal_type: usize, quant_offset_type: usize) {
    let nblk = pulses.len() / 16;
    let mut nls = vec![0usize; nblk];
    let mut sums = vec![0i32; nblk];
    let mut hi = vec![0i32; pulses.len()];
    for b in 0..nblk {
        let blk = &pulses[b * 16..(b + 1) * 16];
        let mut s = 0usize;
        loop {
            let sum: i32 = blk.iter().map(|p| p.abs() >> s).sum();
            if sum <= 16 || s == 10 {
                sums[b] = sum.min(16);
                break;
            }
            s += 1;
        }
        nls[b] = s;
        for (h, p) in hi[b * 16..(b + 1) * 16].iter_mut().zip(blk) {
            *h = p.abs() >> s;
        }
    }
    // Rate level with the cheapest pulse-count symbols.
    let rl_tab = RATE_LEVEL[signal_type >> 1];
    let mut best = (f32::MAX, 0usize);
    for rl in 0..9 {
        let mut bits = icdf_bits(rl_tab, rl);
        for b in 0..nblk {
            bits += if nls[b] == 0 { icdf_bits(PULSE_COUNT[rl], sums[b] as usize) } else { icdf_bits(PULSE_COUNT[rl], 17) };
        }
        if bits < best.0 {
            best = (bits, rl);
        }
    }
    let rl = best.1;
    enc.icdf(rl, rl_tab, 8);
    for b in 0..nblk {
        if nls[b] == 0 {
            enc.icdf(sums[b] as usize, PULSE_COUNT[rl], 8);
        } else {
            enc.icdf(17, PULSE_COUNT[rl], 8);
            for j in 1..nls[b] {
                enc.icdf(17, count_table(j), 8);
            }
            enc.icdf(sums[b] as usize, count_table(nls[b]), 8);
        }
    }
    for b in 0..nblk {
        if sums[b] > 0 {
            shell(enc, &hi[b * 16..(b + 1) * 16], sums[b]);
        }
    }
    for b in 0..nblk {
        if nls[b] > 0 {
            for p in &pulses[b * 16..(b + 1) * 16] {
                let a = p.abs();
                for j in (0..nls[b]).rev() {
                    enc.icdf(((a >> j) & 1) as usize, &EXC_LSB, 8);
                }
            }
        }
    }
    let base = 7 * (quant_offset_type + (signal_type << 1));
    for b in 0..nblk {
        let p = sums[b] | ((nls[b] as i32) << 5);
        if p > 0 {
            let tab = EXC_SIGN[base + ((p & 0x1F) as usize).min(6)];
            for &q in &pulses[b * 16..(b + 1) * 16] {
                if q != 0 {
                    enc.icdf((q > 0) as usize, tab, 8);
                }
            }
        }
    }
}
