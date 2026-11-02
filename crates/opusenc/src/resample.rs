//! 48 kHz → 8/12/16 kHz decimator feeding the SILK layer (own design).
//!
//! A linear-phase low-pass FIR (Kaiser-windowed sinc at 48 kHz) evaluated only at the retained
//! output phases. Its group delay ([`delay`]) is a whole number of 48 kHz samples, chosen so
//! that the SILK path (this filter + the decoder's internal-rate delay and resampler, RFC 6716
//! Table 54) lands within a fraction of a sample of the CELT path's 120-sample MDCT overlap.

/// Group delay of the decimator to `fs_out` in 48 kHz samples (the filter has `2 * delay + 1`
/// taps).
pub fn delay(fs_out: u32) -> usize {
    if fs_out == 8_000 { 88 } else { 83 }
}

/// Zeroth-order modified Bessel function of the first kind (power series).
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..64 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// Windowed-sinc low-pass with cutoff `fc` (Hz at 48 kHz), unit DC gain.
fn design(fc: f64, beta: f64, delay: usize) -> Vec<f32> {
    let wc = 2.0 * std::f64::consts::PI * fc / 48_000.0;
    let i0b = bessel_i0(beta);
    let mut h: Vec<f64> = (0..2 * delay + 1)
        .map(|k| {
            let m = k as f64 - delay as f64;
            let sinc = if m == 0.0 { wc / std::f64::consts::PI } else { (wc * m).sin() / (std::f64::consts::PI * m) };
            let r = m / delay as f64;
            sinc * bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / i0b
        })
        .collect();
    let s: f64 = h.iter().sum();
    for v in h.iter_mut() {
        *v /= s;
    }
    h.iter().map(|&v| v as f32).collect()
}

/// Dot product with eight independent partial sums, which the compiler can vectorise (a single
/// running sum is bound by the latency of the float addition). The sums are combined pairwise.
#[inline]
fn dot8(a: &[f32], b: &[f32]) -> f32 {
    let mut s = [0f32; 8];
    let (ca, ra) = a.as_chunks::<8>();
    let (cb, rb) = b.as_chunks::<8>();
    for (x, y) in ca.iter().zip(cb) {
        for l in 0..8 {
            s[l] += x[l] * y[l];
        }
    }
    let mut acc = ((s[0] + s[4]) + (s[1] + s[5])) + ((s[2] + s[6]) + (s[3] + s[7]));
    for (x, y) in ra.iter().zip(rb) {
        acc += x * y;
    }
    acc
}

/// Decimates one channel of 48 kHz audio by 3, 4 or 6.
#[derive(Clone)]
pub struct Downsampler {
    factor: usize,
    h: Vec<f32>,
    taps: usize,
    /// The last `taps - 1` input samples followed by the current frame.
    buf: Vec<f32>,
}

impl Downsampler {
    /// `fs_out` is 8000, 12000 or 16000.
    pub fn new(fs_out: u32) -> Self {
        let factor = (48_000 / fs_out) as usize;
        // Cutoff at 95 % of the output Nyquist frequency (pass band to ~88 %, stop band from ~110 %).
        let fc = 0.95 * fs_out as f64 / 2.0;
        let d = delay(fs_out);
        let taps = 2 * d + 1;
        Downsampler { factor, h: design(fc, 6.5, d), taps, buf: vec![0.0; taps - 1] }
    }

    /// Decimates `input` (a multiple of the factor long), appending to `out`. Output sample `j`
    /// is the filtered input at frame position `factor * j`, delayed by [`delay`].
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        debug_assert!(input.len().is_multiple_of(self.factor));
        let taps = self.taps;
        self.buf.truncate(taps - 1);
        self.buf.extend_from_slice(input);
        for j in 0..input.len() / self.factor {
            let t = taps - 1 + self.factor * j;
            let win = &self.buf[t + 1 - taps..=t];
            // h is symmetric, so the window can be taken in forward order.
            out.push(dot8(win, &self.h));
        }
        let n = self.buf.len();
        self.buf.copy_within(n - (taps - 1).., 0);
        self.buf.truncate(taps - 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(f: f64, n: usize) -> Vec<f32> {
        (0..n).map(|i| (2.0 * std::f64::consts::PI * f * i as f64 / 48_000.0).sin() as f32).collect()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    #[test]
    fn dot8_matches_sequential_sum() {
        for len in [0usize, 1, 7, 8, 9, 63, 167, 251] {
            let a: Vec<f32> = (0..len).map(|i| ((i * 37 % 101) as f32 - 50.0) / 50.0).collect();
            let b: Vec<f32> = (0..len).map(|i| ((i * 53 % 89) as f32 - 44.0) / 44.0).collect();
            let exact: f64 = a.iter().zip(&b).map(|(&x, &y)| x as f64 * y as f64).sum();
            assert!((dot8(&a, &b) as f64 - exact).abs() < 1e-4, "len {len}");
        }
    }

    #[test]
    fn passes_band_and_rejects_aliases() {
        for &(fs, pass, stop) in &[(16_000u32, 6_000.0, 8_600.0), (12_000, 4_500.0, 6_500.0), (8_000, 3_000.0, 4_500.0)] {
            for &(f, want_pass) in &[(pass, true), (stop, false)] {
                let mut d = Downsampler::new(fs);
                let x = tone(f, 9600);
                let mut y = Vec::new();
                for c in x.chunks(960) {
                    d.process(c, &mut y);
                }
                let r = rms(&y[y.len() / 2..]) * std::f64::consts::SQRT_2;
                if want_pass {
                    assert!((r - 1.0).abs() < 0.02, "{fs}: {f} Hz gain {r}");
                } else {
                    assert!(r < 0.01, "{fs}: {f} Hz leaks {r}");
                }
            }
        }
    }

    #[test]
    fn delay_is_exact() {
        let mut d = Downsampler::new(16_000);
        let mut x = vec![0f32; 960];
        x[300] = 1.0;
        let mut y = Vec::new();
        d.process(&x, &mut y);
        // The impulse at 300 shows up centred at 48 kHz time 300 + 83 = 383 (output 127.67).
        let peak = y.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i).unwrap_or(0);
        assert!((peak as f64 - (300.0 + delay(16_000) as f64) / 3.0).abs() < 1.0, "peak at {peak}");
    }
}
