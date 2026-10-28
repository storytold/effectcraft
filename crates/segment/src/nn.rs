//! The few neural-network kernels MobileSAM needs, on token-major feature maps (`[pixels, channels]`
//! row-major, i.e. NHWC with a batch of one): dense layers on a parallel GEMM (ndarray's safe
//! `general_mat_mul`, which picks AVX/FMA kernels at run time), convolutions as patch matrices
//! times weights, depthwise and transposed convolutions, layer norm, exact GELU and softmax.
//! Weights are re-laid out once at load so every inner loop runs over contiguous channels.

use ndarray::linalg::general_mat_mul;
use ndarray::{ArrayView2, ArrayViewMut2};
use rayon::prelude::*;

/// Rows of the left matrix per parallel task.
const ROW_BLOCK: usize = 64;

/// `out[m×n] += a[m×k] · b[k×n]` (row-major), rows split across threads.
pub fn gemm(a: &[f32], m: usize, k: usize, b: &[f32], n: usize, out: &mut [f32]) {
    if m == 0 || n == 0 || a.len() < m * k || b.len() < k * n || out.len() < m * n {
        return;
    }
    let Ok(bv) = ArrayView2::from_shape((k, n), &b[..k * n]) else { return };
    let block = |r0: usize, oc: &mut [f32]| {
        let rows = oc.len() / n;
        if let (Ok(av), Ok(mut cv)) = (ArrayView2::from_shape((rows, k), &a[r0 * k..(r0 + rows) * k]), ArrayViewMut2::from_shape((rows, n), oc)) {
            general_mat_mul(1.0, &av, &bv, 1.0, &mut cv);
        }
    };
    if m <= ROW_BLOCK {
        block(0, &mut out[..m * n]);
    } else {
        out[..m * n].par_chunks_mut(ROW_BLOCK * n).enumerate().for_each(|(i, oc)| block(i * ROW_BLOCK, oc));
    }
}

/// A dense layer (also a 1×1 convolution): `y = x·Wᵀ + b`, with `W` stored transposed.
#[derive(Clone, Debug, Default)]
pub struct Linear {
    /// `[inp × out]`.
    pub wt: Vec<f32>,
    pub b: Vec<f32>,
    pub inp: usize,
    pub out: usize,
}

impl Linear {
    /// From PyTorch's `[out × inp]` weight (a 1×1 conv's `[out × inp × 1 × 1]` too).
    pub fn new(w: &[f32], b: Option<&[f32]>, out: usize, inp: usize) -> Option<Linear> {
        if w.len() != out * inp || b.is_some_and(|b| b.len() != out) {
            return None;
        }
        let mut wt = vec![0.0; inp * out];
        for o in 0..out {
            for i in 0..inp {
                wt[i * out + o] = w[o * inp + i];
            }
        }
        Some(Linear { wt, b: b.map_or_else(|| vec![0.0; out], <[f32]>::to_vec), inp, out })
    }

    /// `rows` tokens of `inp` channels → `rows × out`.
    pub fn forward(&self, x: &[f32], rows: usize) -> Vec<f32> {
        let mut y: Vec<f32> = (0..rows).flat_map(|_| self.b.iter().copied()).collect();
        gemm(x, rows, self.inp, &self.wt, self.out, &mut y);
        y
    }
}

/// A `ks×ks` convolution (any stride and padding, one group) on `h×w×inp` → `oh×ow×out`, as a
/// patch matrix times the weights, built a block of output rows at a time.
#[derive(Clone, Debug, Default)]
pub struct Conv {
    /// `[(ky·ks + kx)·inp + ci] × out`.
    pub wt: Vec<f32>,
    pub b: Vec<f32>,
    pub inp: usize,
    pub out: usize,
    pub ks: usize,
    pub stride: usize,
    pub pad: usize,
}

impl Conv {
    /// From PyTorch's `[out × inp × ks × ks]`.
    pub fn new(w: &[f32], b: Option<&[f32]>, out: usize, inp: usize, ks: usize, stride: usize, pad: usize) -> Option<Conv> {
        if w.len() != out * inp * ks * ks || b.is_some_and(|b| b.len() != out) || stride == 0 {
            return None;
        }
        let kk = ks * ks * inp;
        let mut wt = vec![0.0; kk * out];
        for o in 0..out {
            for ci in 0..inp {
                for t in 0..ks * ks {
                    wt[(t * inp + ci) * out + o] = w[(o * inp + ci) * ks * ks + t];
                }
            }
        }
        Some(Conv { wt, b: b.map_or_else(|| vec![0.0; out], <[f32]>::to_vec), inp, out, ks, stride, pad })
    }

    pub fn out_size(&self, n: usize) -> usize {
        (n + 2 * self.pad).saturating_sub(self.ks) / self.stride + 1
    }

    pub fn forward(&self, x: &[f32], h: usize, w: usize) -> (Vec<f32>, usize, usize) {
        let (oh, ow) = (self.out_size(h), self.out_size(w));
        (self.forward_sized(x, h, w, oh, ow, [self.pad; 2]), oh, ow)
    }

    /// An `oh×ow` output with `pad` (top, left) zero rows and columns before the input; what
    /// falls past the input's far edges is zero too (so asymmetric "same" padding works).
    pub fn forward_sized(&self, x: &[f32], h: usize, w: usize, oh: usize, ow: usize, pad: [usize; 2]) -> Vec<f32> {
        let kk = self.ks * self.ks * self.inp;
        let mut y: Vec<f32> = (0..oh * ow).flat_map(|_| self.b.iter().copied()).collect();
        if x.len() < h * w * self.inp || self.wt.len() < kk * self.out {
            return y;
        }
        y.par_chunks_mut(ow * self.out).enumerate().for_each(|(oy, row)| {
            // This output row's patches.
            let mut patch = vec![0.0f32; ow * kk];
            for ox in 0..ow {
                for ky in 0..self.ks {
                    let iy = (oy * self.stride + ky) as isize - pad[0] as isize;
                    if iy < 0 || iy >= h as isize {
                        continue;
                    }
                    for kx in 0..self.ks {
                        let ix = (ox * self.stride + kx) as isize - pad[1] as isize;
                        if ix < 0 || ix >= w as isize {
                            continue;
                        }
                        let src = (iy as usize * w + ix as usize) * self.inp;
                        let dst = ox * kk + (ky * self.ks + kx) * self.inp;
                        patch[dst..dst + self.inp].copy_from_slice(&x[src..src + self.inp]);
                    }
                }
            }
            if let (Ok(av), Ok(bv), Ok(mut cv)) = (
                ArrayView2::from_shape((ow, kk), &patch[..]),
                ArrayView2::from_shape((kk, self.out), &self.wt[..]),
                ArrayViewMut2::from_shape((ow, self.out), row),
            ) {
                general_mat_mul(1.0, &av, &bv, 1.0, &mut cv);
            }
        });
        y
    }
}

/// A depthwise `ks×ks` convolution (one filter per channel) on `h×w×c`.
#[derive(Clone, Debug, Default)]
pub struct Depthwise {
    /// `[tap × c]`.
    pub w: Vec<f32>,
    pub b: Vec<f32>,
    pub c: usize,
    pub ks: usize,
    pub stride: usize,
}

impl Depthwise {
    /// From PyTorch's `[c × 1 × 3 × 3]`.
    pub fn new(w: &[f32], b: Option<&[f32]>, c: usize, stride: usize) -> Option<Depthwise> {
        if w.len() != c * 9 || b.is_some_and(|b| b.len() != c) || stride == 0 {
            return None;
        }
        let mut wt = vec![0.0; 9 * c];
        for ch in 0..c {
            for t in 0..9 {
                wt[t * c + ch] = w[ch * 9 + t];
            }
        }
        Some(Depthwise { w: wt, b: b.map_or_else(|| vec![0.0; c], <[f32]>::to_vec), c, ks: 3, stride })
    }

    /// 3×3 with padding 1.
    pub fn forward(&self, x: &[f32], h: usize, w: usize) -> (Vec<f32>, usize, usize) {
        let (oh, ow) = ((h + 2 - 3) / self.stride + 1, (w + 2 - 3) / self.stride + 1);
        (self.forward_sized(x, h, w, oh, ow, [1, 1]), oh, ow)
    }

    /// An `oh×ow` output with `pad` (top, left) before the input, as [`Conv::forward_sized`].
    pub fn forward_sized(&self, x: &[f32], h: usize, w: usize, oh: usize, ow: usize, pad: [usize; 2]) -> Vec<f32> {
        let (c, ks) = (self.c, self.ks);
        let mut y = vec![0.0f32; oh * ow * c];
        if x.len() < h * w * c || self.w.len() < ks * ks * c || self.b.len() < c {
            return y;
        }
        y.par_chunks_mut(ow * c).enumerate().for_each(|(oy, row)| {
            for ox in 0..ow {
                let out = &mut row[ox * c..(ox + 1) * c];
                out.copy_from_slice(&self.b);
                for ky in 0..ks {
                    let iy = (oy * self.stride + ky) as isize - pad[0] as isize;
                    if iy < 0 || iy >= h as isize {
                        continue;
                    }
                    for kx in 0..ks {
                        let ix = (ox * self.stride + kx) as isize - pad[1] as isize;
                        if ix < 0 || ix >= w as isize {
                            continue;
                        }
                        let src = &x[(iy as usize * w + ix as usize) * c..][..c];
                        let tap = &self.w[(ky * ks + kx) * c..][..c];
                        for ((o, s), k) in out.iter_mut().zip(src).zip(tap) {
                            *o += s * k;
                        }
                    }
                }
            }
        });
        y
    }
}

/// A 2×2, stride-2 transposed convolution (each input pixel paints a 2×2 block):
/// `h×w×inp` → `2h×2w×out`.
#[derive(Clone, Debug, Default)]
pub struct Upconv {
    /// One dense layer per output tap (dy, dx).
    pub taps: Vec<Linear>,
    pub out: usize,
}

impl Upconv {
    /// From PyTorch's ConvTranspose2d `[inp × out × 2 × 2]`.
    pub fn new(w: &[f32], b: &[f32], inp: usize, out: usize) -> Option<Upconv> {
        if w.len() != inp * out * 4 || b.len() != out {
            return None;
        }
        let taps = (0..4)
            .map(|t| {
                let mut wt = vec![0.0; inp * out];
                for i in 0..inp {
                    for o in 0..out {
                        wt[i * out + o] = w[(i * out + o) * 4 + t];
                    }
                }
                Linear { wt, b: b.to_vec(), inp, out }
            })
            .collect();
        Some(Upconv { taps, out })
    }

    pub fn forward(&self, x: &[f32], h: usize, w: usize) -> Vec<f32> {
        let (ow, c) = (2 * w, self.out);
        let mut y = vec![0.0f32; 4 * h * w * c];
        for (t, lin) in self.taps.iter().enumerate() {
            let (dy, dx) = (t / 2, t % 2);
            let part = lin.forward(x, h * w);
            for p in 0..h * w {
                let (py, px) = (p / w, p % w);
                let dst = ((2 * py + dy) * ow + 2 * px + dx) * c;
                if let (Some(d), Some(s)) = (y.get_mut(dst..dst + c), part.get(p * c..(p + 1) * c)) {
                    d.copy_from_slice(s);
                }
            }
        }
        y
    }
}

/// Layer norm over the channels of each token.
#[derive(Clone, Debug, Default)]
pub struct Norm {
    pub w: Vec<f32>,
    pub b: Vec<f32>,
    pub eps: f32,
}

impl Norm {
    pub fn forward(&self, x: &mut [f32]) {
        let c = self.w.len();
        if c == 0 {
            return;
        }
        x.par_chunks_mut(c).for_each(|t| {
            let mean = t.iter().sum::<f32>() / c as f32;
            let var = t.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / c as f32;
            let inv = 1.0 / (var + self.eps).sqrt();
            for ((v, w), b) in t.iter_mut().zip(&self.w).zip(&self.b) {
                *v = (*v - mean) * inv * w + b;
            }
        });
    }
}

/// The error function (Abramowitz & Stegun 7.1.26 refined: |error| < 1.5e-7).
fn erf(x: f32) -> f32 {
    let s = x.signum();
    let x = x.abs() as f64;
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let y = 1.0 - (((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t - 0.284_496_736) * t + 0.254_829_592) * t * (-x * x).exp();
    s * y as f32
}

/// GELU (the exact, erf form, as PyTorch's `nn.GELU`), in place.
pub fn gelu(x: &mut [f32]) {
    x.par_chunks_mut(4096).for_each(|c| {
        for v in c {
            *v = 0.5 * *v * (1.0 + erf(*v * std::f32::consts::FRAC_1_SQRT_2));
        }
    });
}

pub fn relu(x: &mut [f32]) {
    for v in x {
        *v = v.max(0.0);
    }
}

/// `a += b`.
pub fn add(a: &mut [f32], b: &[f32]) {
    a.par_chunks_mut(4096).zip(b.par_chunks(4096)).for_each(|(a, b)| {
        for (x, y) in a.iter_mut().zip(b) {
            *x += y;
        }
    });
}

/// Softmax of each row of `n` values, in place.
pub fn softmax_rows(x: &mut [f32], n: usize) {
    if n == 0 {
        return;
    }
    for row in x.chunks_mut(n) {
        let m = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let mut s = 0.0;
        for v in row.iter_mut() {
            *v = (*v - m).exp();
            s += *v;
        }
        let inv = 1.0 / s.max(1e-30);
        for v in row.iter_mut() {
            *v *= inv;
        }
    }
}

/// Token counts and channel layout shared by the query, key and value tensors.
#[derive(Clone, Copy, Debug)]
pub struct AttentionShape {
    pub query_tokens: usize,
    pub key_tokens: usize,
    pub heads: usize,
    pub head_dim: usize,
}

/// Multi-head scaled dot-product attention with token-major tensors, heads side by side,
/// and an optional per-head bias `[heads × query_tokens × key_tokens]`.
pub fn attention(q: &[f32], k: &[f32], v: &[f32], shape: AttentionShape, bias: Option<&[f32]>) -> Vec<f32> {
    let AttentionShape { query_tokens: nq, key_tokens: nk, heads, head_dim: hd } = shape;
    let dim = heads * hd;
    let mut out = vec![0.0f32; nq * dim];
    if q.len() < nq * dim || k.len() < nk * dim || v.len() < nk * dim {
        return out;
    }
    let scale = 1.0 / (hd as f32).sqrt();
    for h in 0..heads {
        // This head's q, kᵀ and v as contiguous matrices.
        let qh: Vec<f32> = (0..nq).flat_map(|i| q[i * dim + h * hd..][..hd].iter().map(|x| x * scale)).collect();
        let mut kt = vec![0.0f32; hd * nk];
        for j in 0..nk {
            for d in 0..hd {
                kt[d * nk + j] = k[j * dim + h * hd + d];
            }
        }
        let vh: Vec<f32> = (0..nk).flat_map(|j| v[j * dim + h * hd..][..hd].iter().copied()).collect();
        let mut s = match bias.and_then(|b| b.get(h * nq * nk..(h + 1) * nq * nk)) {
            Some(b) => b.to_vec(),
            None => vec![0.0; nq * nk],
        };
        gemm(&qh, nq, hd, &kt, nk, &mut s);
        softmax_rows(&mut s, nk);
        let mut oh = vec![0.0f32; nq * hd];
        gemm(&s, nq, nk, &vh, hd, &mut oh);
        for i in 0..nq {
            out[i * dim + h * hd..][..hd].copy_from_slice(&oh[i * hd..][..hd]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_head_cross_attention_matches_direct_sums() {
        let shape = AttentionShape { query_tokens: 2, key_tokens: 3, heads: 2, head_dim: 2 };
        let q = [1.0, -0.5, 0.25, 2.0, -1.0, 0.75, 1.5, -0.25];
        let k = [0.5, 1.0, -0.5, 0.25, 1.5, -0.25, 0.75, 1.0, -1.0, 0.5, 2.0, -1.5];
        let v = [1.0, 2.0, 3.0, 4.0, -1.0, 0.5, 1.5, -2.0, 0.25, 0.75, -0.5, 2.5];
        let bias = [0.0, 0.5, -0.5, 1.0, 0.0, -1.0, -0.25, 0.25, 0.5, 0.0, -0.5, 1.0];
        for bias in [None, Some(bias.as_slice())] {
            let actual = attention(&q, &k, &v, shape, bias);
            for query in 0..shape.query_tokens {
                for head in 0..shape.heads {
                    let mut scores: Vec<f32> = (0..shape.key_tokens)
                        .map(|key| {
                            let dot: f32 = (0..shape.head_dim).map(|channel| q[query * 4 + head * 2 + channel] * k[key * 4 + head * 2 + channel]).sum();
                            dot / (shape.head_dim as f32).sqrt() + bias.map_or(0.0, |b| b[(head * 2 + query) * 3 + key])
                        })
                        .collect();
                    let max = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                    for score in &mut scores {
                        *score = (*score - max).exp();
                    }
                    let total: f32 = scores.iter().sum();
                    for channel in 0..shape.head_dim {
                        let expected: f32 = scores.iter().enumerate().map(|(key, score)| score / total * v[key * 4 + head * 2 + channel]).sum();
                        assert!((actual[query * 4 + head * 2 + channel] - expected).abs() < 1e-6);
                    }
                }
            }
        }
    }

    #[test]
    fn gemm_matches_naive() {
        let (m, k, n) = (130, 17, 9);
        let a: Vec<f32> = (0..m * k).map(|i| ((i * 7) % 13) as f32 - 6.0).collect();
        let b: Vec<f32> = (0..k * n).map(|i| ((i * 5) % 11) as f32 - 5.0).collect();
        let mut c = vec![1.0f32; m * n];
        gemm(&a, m, k, &b, n, &mut c);
        for i in 0..m {
            for j in 0..n {
                let want: f32 = 1.0 + (0..k).map(|p| a[i * k + p] * b[p * n + j]).sum::<f32>();
                assert_eq!(c[i * n + j], want);
            }
        }
    }

    #[test]
    fn convolutions_match_direct_sums() {
        // 3×3 stride-2 conv on a 5×4×2 input vs. the definition.
        let (h, w, inp, out) = (5, 4, 2, 3);
        let x: Vec<f32> = (0..h * w * inp).map(|i| (i % 7) as f32 * 0.5).collect();
        let wt: Vec<f32> = (0..out * inp * 9).map(|i| ((i % 5) as f32 - 2.0) * 0.25).collect();
        let b = [0.1, -0.2, 0.3];
        let conv = Conv::new(&wt, Some(&b), out, inp, 3, 2, 1).unwrap();
        let (y, oh, ow) = conv.forward(&x, h, w);
        assert_eq!((oh, ow), (3, 2));
        for oy in 0..oh {
            for ox in 0..ow {
                for o in 0..out {
                    let mut s = b[o];
                    for ci in 0..inp {
                        for ky in 0..3 {
                            for kx in 0..3 {
                                let (iy, ix) = ((oy * 2 + ky) as isize - 1, (ox * 2 + kx) as isize - 1);
                                if iy >= 0 && ix >= 0 && iy < h as isize && ix < w as isize {
                                    s += x[(iy as usize * w + ix as usize) * inp + ci] * wt[((o * inp + ci) * 3 + ky) * 3 + kx];
                                }
                            }
                        }
                    }
                    assert!((y[(oy * ow + ox) * out + o] - s).abs() < 1e-5);
                }
            }
        }
        // Depthwise == a grouped conv: compare with Conv on each channel alone.
        let dw_w: Vec<f32> = (0..inp * 9).map(|i| (i as f32 - 8.0) * 0.1).collect();
        let dw = Depthwise::new(&dw_w, None, inp, 1).unwrap();
        let (yd, _, _) = dw.forward(&x, h, w);
        for ci in 0..inp {
            let xc: Vec<f32> = (0..h * w).map(|p| x[p * inp + ci]).collect();
            let single = Conv::new(&dw_w[ci * 9..ci * 9 + 9], None, 1, 1, 3, 1, 1).unwrap();
            let (yc, _, _) = single.forward(&xc, h, w);
            for p in 0..h * w {
                assert!((yd[p * inp + ci] - yc[p]).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn gelu_and_softmax() {
        let mut x = [-3.0f32, -1.0, 0.0, 1.0, 3.0];
        gelu(&mut x);
        // PyTorch's exact GELU values.
        let want = [-0.004_049_7, -0.158_655_3, 0.0, 0.841_344_7, 2.995_950_3];
        for (a, b) in x.iter().zip(want) {
            assert!((a - b).abs() < 1e-5, "{a} {b}");
        }
        let mut s = [1.0f32, 2.0, 3.0];
        softmax_rows(&mut s, 3);
        assert!((s.iter().sum::<f32>() - 1.0).abs() < 1e-6 && s[2] > s[1]);
    }
}
