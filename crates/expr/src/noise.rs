//! Deterministic hashing, random numbers and smooth noise for `wiggle`, `random` and `noise`.
//!
//! Everything is a pure function of its seeds, so a frame renders identically on any thread,
//! in any order, on native and on the web.

/// SplitMix64 finaliser: a good 64-bit mixing function.
pub fn hash64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// Combine several seeds into one.
pub fn mix(parts: &[u64]) -> u64 {
    parts.iter().fold(0x2545_f491_4f6c_dd1d, |h, p| hash64(h ^ hash64(*p)))
}

/// Uniform in `[0, 1)`.
pub fn unit(seed: u64) -> f64 {
    (hash64(seed) >> 11) as f64 / (1u64 << 53) as f64
}

/// Lattice value in `[-1, 1]`.
fn lattice(seed: u64, i: i64) -> f64 {
    unit(seed ^ hash64(i as u64)) * 2.0 - 1.0
}

fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Smooth 1D value noise in `[-1, 1]` (C2-continuous, bounded by the lattice values).
pub fn noise1(seed: u64, x: f64) -> f64 {
    if !x.is_finite() {
        return 0.0;
    }
    let i = x.floor();
    let u = fade(x - i);
    let i = i as i64;
    let (a, b) = (lattice(seed, i), lattice(seed, i.wrapping_add(1)));
    a + (b - a) * u
}

/// Smooth 3D value noise in `[-1, 1]` (AE's `noise()`: nearby inputs give nearby outputs).
pub fn noise3(x: f64, y: f64, z: f64) -> f64 {
    if !(x.is_finite() && y.is_finite() && z.is_finite()) {
        return 0.0;
    }
    let (fx, fy, fz) = (x.floor(), y.floor(), z.floor());
    let (u, v, w) = (fade(x - fx), fade(y - fy), fade(z - fz));
    let (ix, iy, iz) = (fx as i64, fy as i64, fz as i64);
    let g = |dx: i64, dy: i64, dz: i64| unit(mix(&[ix.wrapping_add(dx) as u64, iy.wrapping_add(dy) as u64, iz.wrapping_add(dz) as u64])) * 2.0 - 1.0;
    let l = |a: f64, b: f64, t: f64| a + (b - a) * t;
    let x00 = l(g(0, 0, 0), g(1, 0, 0), u);
    let x10 = l(g(0, 1, 0), g(1, 1, 0), u);
    let x01 = l(g(0, 0, 1), g(1, 0, 1), u);
    let x11 = l(g(0, 1, 1), g(1, 1, 1), u);
    l(l(x00, x10, v), l(x01, x11, v), w)
}

/// Per-dimension wiggle offsets: `Σ amp·mult^k · noise(freq·2^k·t)` over `octaves`.
/// With one octave every offset lies in `[-amp, amp]`.
pub fn wiggle(seed: u64, dims: usize, freq: f64, amp: f64, octaves: f64, amp_mult: f64, t: f64) -> Vec<f64> {
    let octaves = if octaves.is_finite() { octaves.clamp(1.0, 16.0) as usize } else { 1 };
    (0..dims)
        .map(|d| {
            let mut a = amp;
            let mut f = freq;
            let mut sum = 0.0;
            for k in 0..octaves {
                let s = mix(&[seed, d as u64, k as u64]);
                // Offset each octave's phase so octaves don't share lattice points.
                sum += a * noise1(s, f * t + unit(s) * 1000.0);
                a *= amp_mult;
                f *= 2.0;
            }
            sum
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_bounded_smooth_and_deterministic() {
        let mut prev = noise1(7, 0.0);
        for i in 1..10_000 {
            let x = i as f64 * 0.001;
            let v = noise1(7, x);
            assert!((-1.0..=1.0).contains(&v));
            assert!((v - prev).abs() < 0.01, "jump at {x}");
            prev = v;
        }
        assert_eq!(noise1(3, 1.25), noise1(3, 1.25));
        assert_ne!(noise1(3, 1.25), noise1(4, 1.25));
        assert!((-1.0..=1.0).contains(&noise3(0.3, 4.2, -7.5)));
    }

    #[test]
    fn huge_inputs_do_not_overflow() {
        // Found by the expression fuzzer (#786): `wiggle(9007199254740993, 30)` at a late time
        // put the lattice index at `i64::MAX`, and its neighbour overflowed.
        let w = wiggle(1, 2, 9_007_199_254_740_993.0, 30.0, 1.0, 0.5, 1e6);
        assert!(w.iter().all(|v| v.is_finite() && v.abs() <= 30.0));
        for x in [1e300, -1e300, 9.3e18, -9.3e18] {
            assert!(noise1(5, x).is_finite());
            assert!(noise3(x, -x, x).is_finite());
        }
    }

    #[test]
    fn wiggle_bounds() {
        for i in 0..2000 {
            let w = wiggle(99, 2, 3.0, 50.0, 1.0, 0.5, i as f64 / 30.0);
            assert!(w.iter().all(|v| v.abs() <= 50.0));
        }
    }
}
