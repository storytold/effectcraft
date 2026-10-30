//! Each kernel against a plain CPU computation. They skip when there is no GPU.

use std::sync::{Mutex, PoisonError};

use super::*;

/// The one device the tests share (several devices at once upset the Vulkan loader), held for the whole
/// test so the tests take turns.
pub(crate) struct Shared(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>, Arc<Gpu>);

impl std::ops::Deref for Shared {
    type Target = Gpu;
    fn deref(&self) -> &Gpu {
        &self.1
    }
}

pub(crate) fn gpu() -> Option<Shared> {
    static TURN: Mutex<()> = Mutex::new(());
    let guard = TURN.lock().unwrap_or_else(PoisonError::into_inner);
    match Gpu::shared() {
        Ok(g) => Some(Shared(guard, g)),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

/// Deterministic pseudo-random values in -1..1.
pub(crate) fn noise(n: usize, seed: u32) -> Vec<f32> {
    let mut s = seed.wrapping_mul(2654435761).wrapping_add(1);
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s % 20001) as f32 / 10000.0 - 1.0
        })
        .collect()
}

pub(crate) fn assert_close(got: &[f32], want: &[f32], tol: f32, what: &str) {
    assert_eq!(got.len(), want.len(), "{what}: length");
    let worst = got.iter().zip(want).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
    assert!(worst <= tol, "{what}: max abs error {worst} (tolerance {tol})");
}

fn gelu(x: f32) -> f32 {
    use candle_core::{Device, Tensor};
    let t = Tensor::new(&[x], &Device::Cpu).and_then(|t| t.gelu_erf()).and_then(|t| t.to_vec1::<f32>());
    t.map(|v| v[0]).unwrap_or(f32::NAN)
}

/// `C[b][i][j] = sum_k A[b][i][k] * B[b][k][j]` by the definition, with the strides given.
#[allow(clippy::too_many_arguments)]
fn naive(a: &[f32], sa: Strides, b: &[f32], sb: Strides, m: usize, n: usize, k: usize, batch: usize) -> Vec<f32> {
    let mut c = vec![0f32; batch * m * n];
    for z in 0..batch {
        for i in 0..m {
            for j in 0..n {
                let mut acc = 0f32;
                for l in 0..k {
                    acc += a[sa.offset + z * sa.batch + i * sa.row + l * sa.col] * b[sb.offset + z * sb.batch + l * sb.row + j * sb.col];
                }
                c[z * m * n + i * n + j] = acc;
            }
        }
    }
    c
}

#[test]
fn matmul_matches_the_definition_for_awkward_sizes_and_strides() {
    let Some(g) = gpu() else { return };
    // (m, n, k, batch): not multiples of the 64 x 64 x 16 tile, and degenerate ones
    for (m, n, k, batch) in [(100, 50, 37, 1), (1, 1, 1, 1), (65, 129, 17, 2), (64, 64, 16, 1), (3, 200, 96, 3), (130, 7, 300, 1)] {
        let (av, bv) = (noise(batch * m * k, 1), noise(batch * k * n, 2));
        let (a, b) = (g.upload(&av), g.upload(&bv));
        let c = g.zeros(batch * m * n).unwrap();
        let (sa, sb) = (Strides { offset: 0, batch: m * k, row: k, col: 1 }, Strides { offset: 0, batch: k * n, row: n, col: 1 });
        let sc = Strides { offset: 0, batch: m * n, row: n, col: 1 };
        g.matmul(&Matmul { m, n, k, batch, a: (&a, sa), b: (&b, sb), bias: None, c: (&c, sc), residual: None, alpha: 1.0, act: Act::None }).unwrap();
        g.flush().unwrap();
        assert_close(&g.download(&c).unwrap(), &naive(&av, sa, &bv, sb, m, n, k, batch), 2e-4 * (k as f32).sqrt(), &format!("{m}x{n}x{k} x{batch}"));
    }
}

#[test]
fn matmul_rejects_nonunit_output_columns() {
    let Some(g) = gpu() else { return };
    let value = g.upload(&[1.0]);
    let c = g.zeros(8).unwrap();
    for col in [0, 2] {
        let broadcast = Strides { offset: 0, batch: 0, row: 0, col: 0 };
        let out = Strides { offset: 0, batch: 0, row: 4, col };
        let op = Matmul {
            m: 2,
            n: 2,
            k: 1,
            batch: 1,
            a: (&value, broadcast),
            b: (&value, broadcast),
            c: (&c, out),
            bias: None,
            residual: None,
            alpha: 1.0,
            act: Act::None,
        };
        assert!(matches!(g.matmul(&op), Err(Error::Input(_))), "output column stride {col}");
    }
}

#[test]
fn matmul_zero_inner_dimension_checks_output_and_writes_bias() {
    let Some(g) = gpu() else { return };
    let empty = g.upload(&[]);
    let tiny = g.zeros(1).unwrap();
    let bias = g.upload(&[3.0, 5.0]);
    let mut op = Matmul {
        m: 2,
        n: 2,
        k: 0,
        batch: 1,
        a: (&empty, Strides::row_major(0)),
        b: (&empty, Strides::row_major(2)),
        c: (&tiny, Strides::row_major(2)),
        bias: Some(&bias),
        residual: None,
        alpha: 1.0,
        act: Act::None,
    };
    assert!(matches!(g.matmul(&op), Err(Error::Input(_))), "C is written even when K is zero");
    let c = g.zeros(4).unwrap();
    op.c = (&c, Strides::row_major(2));
    g.matmul(&op).unwrap();
    g.flush().unwrap();
    assert_eq!(g.download(&c).unwrap(), [3.0, 5.0, 3.0, 5.0]);
}

#[test]
fn matmul_rejects_overflowing_operand_offsets() {
    let Some(g) = gpu() else { return };
    let value = g.upload(&[1.0]);
    let c = g.zeros(4).unwrap();
    let broadcast = Strides { offset: 0, batch: 0, row: 0, col: 0 };
    let out = Strides { offset: usize::MAX, batch: 0, row: 1, col: 1 };
    let op = Matmul {
        m: 2,
        n: 1,
        k: 1,
        batch: 1,
        a: (&value, broadcast),
        b: (&value, broadcast),
        c: (&c, out),
        bias: None,
        residual: None,
        alpha: 1.0,
        act: Act::None,
    };
    assert!(matches!(g.matmul(&op), Err(Error::Input(_))));
}

#[test]
fn matmul_with_transposed_operands_bias_activation_and_residual() {
    let Some(g) = gpu() else { return };
    let (m, n, k) = (70, 90, 41);
    let (av, wv, bias, res) = (noise(m * k, 3), noise(n * k, 4), noise(n, 5), noise(m * n, 6));
    let (a, w, bi, r) = (g.upload(&av), g.upload(&wv), g.upload(&bias), g.upload(&res));
    let c = g.zeros(m * n).unwrap();
    // B is a weight stored [n, k] (a linear layer): B[k][j] = w[j * k + l]
    let sb = Strides { offset: 0, batch: 0, row: 1, col: k };
    let sa = Strides::row_major(k);
    g.matmul(&Matmul {
        m,
        n,
        k,
        batch: 1,
        a: (&a, sa),
        b: (&w, sb),
        bias: Some(&bi),
        c: (&c, Strides::row_major(n)),
        residual: Some(&r),
        alpha: 0.5,
        act: Act::Gelu,
    })
    .unwrap();
    g.flush().unwrap();
    let base = naive(&av, sa, &wv, sb, m, n, k, 1);
    let want: Vec<f32> = base.iter().enumerate().map(|(i, v)| gelu(v * 0.5 + bias[i % n]) + res[i]).collect();
    assert_close(&g.download(&c).unwrap(), &want, 1e-3, "linear + gelu + residual");
    // relu, and a transposed A (m contiguous)
    let at: Vec<f32> = (0..k * m).map(|i| av[(i % m) * k + i / m]).collect();
    let (a2, c2) = (g.upload(&at), g.zeros(m * n).unwrap());
    g.matmul(&Matmul {
        m,
        n,
        k,
        batch: 1,
        a: (&a2, Strides { offset: 0, batch: 0, row: 1, col: m }),
        b: (&w, sb),
        bias: None,
        c: (&c2, Strides::row_major(n)),
        residual: None,
        alpha: 1.0,
        act: Act::Relu,
    })
    .unwrap();
    g.flush().unwrap();
    let want: Vec<f32> = base.iter().map(|v| v.max(0.0)).collect();
    assert_close(&g.download(&c2).unwrap(), &want, 1e-3, "transposed A, relu");
}

#[test]
fn matmul_refuses_operands_that_overrun_their_buffers() {
    let Some(g) = gpu() else { return };
    let (a, b, c) = (g.upload(&[1.0; 6]), g.upload(&[1.0; 6]), g.zeros(4).unwrap());
    let ok = Matmul {
        m: 2,
        n: 2,
        k: 3,
        batch: 1,
        a: (&a, Strides::row_major(3)),
        b: (&b, Strides::row_major(2)),
        bias: None,
        c: (&c, Strides::row_major(2)),
        residual: None,
        alpha: 1.0,
        act: Act::None,
    };
    assert!(g.matmul(&ok).is_ok());
    let too_big = Matmul { m: 2, n: 2, k: 4, ..ok };
    assert!(matches!(g.matmul(&too_big), Err(Error::Input(_))));
    assert!(g.zeros(usize::MAX / 2).is_err());
    g.flush().unwrap();
}

// ---------------------------------------------------------------- the other kernels

fn run_and_read(g: &Gpu, out: &Buf) -> Vec<f32> {
    g.flush().unwrap();
    g.download(out).unwrap()
}

#[test]
fn layernorm_and_softmax_match_the_definition() {
    let Some(g) = gpu() else { return };
    for (rows, cols) in [(7, 96), (1, 1), (300, 768), (70_000, 8)] {
        let x = noise(rows * cols, 7);
        let (w, b) = (noise(cols, 8), noise(cols, 9));
        let (gx, gw, gb, gy) = (g.upload(&x), g.upload(&w), g.upload(&b), g.zeros(rows * cols).unwrap());
        g.layernorm(&gx, &gw, &gb, &gy, rows, cols, 1e-6, false).unwrap();
        let want: Vec<f32> = x
            .chunks(cols)
            .flat_map(|r| {
                let mean = r.iter().sum::<f32>() / cols as f32;
                let var = r.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / cols as f32;
                r.iter().enumerate().map(|(i, v)| (v - mean) / (var + 1e-6).sqrt() * w[i] + b[i]).collect::<Vec<_>>()
            })
            .collect();
        assert_close(&run_and_read(&g, &gy), &want, 2e-4, &format!("layernorm {rows}x{cols}"));
    }
    for (rows, cols) in [(5, 64), (3, 1), (100, 4096), (2, 28736), (70_000, 64)] {
        let x = noise(rows * cols, 10).iter().map(|v| v * 6.0).collect::<Vec<_>>();
        let gx = g.upload(&x);
        g.softmax(&gx, rows, cols).unwrap();
        let want: Vec<f32> = x
            .chunks(cols)
            .flat_map(|r| {
                let m = r.iter().cloned().fold(f32::MIN, f32::max);
                let e: Vec<f32> = r.iter().map(|v| (v - m).exp()).collect();
                let s: f32 = e.iter().sum();
                e.into_iter().map(|v| v / s).collect::<Vec<_>>()
            })
            .collect();
        assert_close(&run_and_read(&g, &gx), &want, 1e-5, &format!("softmax {rows}x{cols}"));
    }
}

#[test]
fn windows_pooling_and_add_match_naive_loops() {
    let Some(g) = gpu() else { return };
    let (h, w, c, ws) = (13, 22, 5, 8);
    let x = noise(h * w * c, 11);
    let (nwx, nwy) = (w.div_ceil(ws), h.div_ceil(ws));
    let (gx, parts, back) = (g.upload(&x), g.zeros(nwx * nwy * ws * ws * c).unwrap(), g.zeros(h * w * c).unwrap());
    g.window_partition(&gx, &parts, h, w, c, ws).unwrap();
    g.window_unpartition(&parts, &back, h, w, c, ws).unwrap();
    let mut want = vec![0f32; nwx * nwy * ws * ws * c];
    for win in 0..nwx * nwy {
        for y in 0..ws {
            for xx in 0..ws {
                let (gy, gxx) = ((win / nwx) * ws + y, (win % nwx) * ws + xx);
                for ch in 0..c {
                    if gy < h && gxx < w {
                        want[((win * ws + y) * ws + xx) * c + ch] = x[(gy * w + gxx) * c + ch];
                    }
                }
            }
        }
    }
    g.flush().unwrap();
    assert_close(&g.download(&parts).unwrap(), &want, 0.0, "window_partition");
    assert_close(&g.download(&back).unwrap(), &x, 0.0, "window round trip");
    assert!(g.window_partition(&gx, &parts, h, w, c, 0).is_err());

    // pooling a q third of a [nb, 8, 6, 3 * c] buffer in place
    let (nb, ph, pw, pc) = (3, 8, 6, 4);
    let qkv = noise(nb * ph * pw * 3 * pc, 12);
    let (gq, out) = (g.upload(&qkv), g.zeros(nb * (ph / 2) * (pw / 2) * pc).unwrap());
    g.maxpool2(&gq, &out, nb, ph, pw, pc, (ph * pw * 3 * pc, pw * 3 * pc, 3 * pc)).unwrap();
    let mut want = vec![];
    for img in 0..nb {
        for oy in 0..ph / 2 {
            for ox in 0..pw / 2 {
                for ch in 0..pc {
                    let at = |y: usize, xx: usize| qkv[((img * ph + y) * pw + xx) * 3 * pc + ch];
                    want.push(at(2 * oy, 2 * ox).max(at(2 * oy, 2 * ox + 1)).max(at(2 * oy + 1, 2 * ox)).max(at(2 * oy + 1, 2 * ox + 1)));
                }
            }
        }
    }
    assert_close(&run_and_read(&g, &out), &want, 0.0, "maxpool2");
    assert!(g.maxpool2(&gq, &out, nb, 7, pw, pc, (1, 1, 1)).is_err());

    let (a, b) = (noise(100_001, 13), noise(100_001, 14));
    let (ga, gb, go) = (g.upload(&a), g.upload(&b), g.zeros(a.len()).unwrap());
    g.add(&ga, &gb, &go, a.len()).unwrap();
    let want: Vec<f32> = a.iter().zip(&b).map(|(x, y)| x + y).collect();
    assert_close(&run_and_read(&g, &go), &want, 0.0, "add");
}

#[test]
fn patch_embed_rope_and_upsample_match_the_cpu_definitions() {
    use candle_core::{Device, Tensor};
    let Some(g) = gpu() else { return };
    // the 7 x 7 stride 4 convolution against candle's conv2d
    let (h, w, co) = (64usize, 96usize, 8usize);
    let (px, wt, bias) = (noise(3 * h * w, 15), noise(co * 147, 16), noise(co, 17));
    let (gp, gw, gb) = (g.upload(&px), g.upload(&wt), g.upload(&bias));
    let (ho, wo) = (h / 4, w / 4);
    let out = g.zeros(ho * wo * co).unwrap();
    g.patch_embed(&gp, &gw, &gb, &out, (h, w, co)).unwrap();
    let t = Tensor::from_vec(px, (1, 3, h, w), &Device::Cpu).unwrap();
    let k = Tensor::from_vec(wt, (co, 3, 7, 7), &Device::Cpu).unwrap();
    let want =
        t.conv2d(&k, 3, 4, 1, 1).unwrap().broadcast_add(&Tensor::from_vec(bias, (1, co, 1, 1), &Device::Cpu).unwrap()).unwrap().permute((0, 2, 3, 1)).unwrap();
    assert_close(&run_and_read(&g, &out), &want.flatten_all().unwrap().to_vec1::<f32>().unwrap(), 2e-4, "patch_embed");

    // rotary encoding: pairs rotated with row % table rows, later rows copied
    let (rows, cols, table, rotated) = (37usize, 16usize, 10usize, 30usize);
    let (x, cs, sn) = (noise(rows * cols, 18), noise(table * cols, 19), noise(table * cols, 20));
    let (gx, gc, gs, go) = (g.upload(&x), g.upload(&cs), g.upload(&sn), g.zeros(rows * cols).unwrap());
    g.rope(&gx, &gc, &gs, &go, rows, cols, table, rotated).unwrap();
    let mut want = x.clone();
    for r in 0..rotated {
        for j in 0..cols / 2 {
            let (x0, x1) = (x[r * cols + 2 * j], x[r * cols + 2 * j + 1]);
            let (t0, t1) = ((r % table) * cols + 2 * j, (r % table) * cols + 2 * j + 1);
            want[r * cols + 2 * j] = x0 * cs[t0] - x1 * sn[t0];
            want[r * cols + 2 * j + 1] = x1 * cs[t1] + x0 * sn[t1];
        }
    }
    assert_close(&run_and_read(&g, &go), &want, 1e-6, "rope");
    assert!(g.rope(&gx, &gc, &gs, &go, rows, 15, table, rotated).is_err());

    // neck: lateral + nearest upsample
    let (h, w, c) = (6usize, 10usize, 7usize);
    let (lat, prev) = (noise(h * w * c, 21), noise(h / 2 * (w / 2) * c, 22));
    let (gl, gp2, go) = (g.upload(&lat), g.upload(&prev), g.zeros(h * w * c).unwrap());
    g.upsample_add(&gl, &gp2, &go, h, w, c).unwrap();
    let want: Vec<f32> = (0..h * w * c).map(|i| lat[i] + prev[((i / c / w / 2) * (w / 2) + (i / c % w) / 2) * c + i % c]).collect();
    assert_close(&run_and_read(&g, &go), &want, 0.0, "upsample_add");
}

#[test]
fn convolutions_and_the_layernorm_gelu_epilogue_match_candle() {
    use candle_core::{Device, Tensor};
    let Some(g) = gpu() else { return };
    // (h, w, ci, co, k, stride, pad, groups): a strided 3 x 3, a depthwise 7 x 7, and a 1-channel input
    for (h, w, ci, co, k, stride, pad, groups) in
        [(9usize, 13usize, 3usize, 5usize, 3usize, 2usize, 1usize, 1usize), (10, 10, 4, 4, 7, 1, 3, 4), (16, 16, 1, 4, 3, 2, 1, 1)]
    {
        let (x, wt, b) = (noise(h * w * ci, 30), noise(co * (ci / groups) * k * k, 31), noise(co, 32));
        let (gx, gw, gb) = (g.upload(&x), g.upload(&wt), g.upload(&b));
        let (ho, wo) = ((h + 2 * pad - k) / stride + 1, (w + 2 * pad - k) / stride + 1);
        let out = g.zeros(ho * wo * co).unwrap();
        g.conv2d(&gx, &gw, &gb, &out, (h, w, ci, co), k, stride, pad, groups).unwrap();
        // candle works on NCHW: bring the channels-last input over, convolve, bring the result back
        let t = Tensor::from_vec(x, (1, h, w, ci), &Device::Cpu).unwrap().permute((0, 3, 1, 2)).unwrap().contiguous().unwrap();
        let kern = Tensor::from_vec(wt, (co, ci / groups, k, k), &Device::Cpu).unwrap();
        let want = t
            .conv2d(&kern, pad, stride, 1, groups)
            .unwrap()
            .broadcast_add(&Tensor::from_vec(b, (1, co, 1, 1), &Device::Cpu).unwrap())
            .unwrap()
            .permute((0, 2, 3, 1))
            .unwrap();
        assert_close(
            &run_and_read(&g, &out),
            &want.flatten_all().unwrap().to_vec1::<f32>().unwrap(),
            2e-4,
            &format!("conv {k}x{k} stride {stride} groups {groups}"),
        );
    }
    assert!(g.conv2d(&g.upload(&[0.0; 4]), &g.upload(&[0.0; 4]), &g.upload(&[0.0; 1]), &g.zeros(4).unwrap(), (2, 2, 1, 1), 5, 1, 0, 1).is_err());
    // layer norm followed by GELU in the same pass
    let (rows, cols) = (50usize, 96usize);
    let (x, wt, b) = (noise(rows * cols, 33), noise(cols, 34), noise(cols, 35));
    let (gx, gw, gb, gy) = (g.upload(&x), g.upload(&wt), g.upload(&b), g.zeros(rows * cols).unwrap());
    g.layernorm(&gx, &gw, &gb, &gy, rows, cols, 1e-6, true).unwrap();
    let want: Vec<f32> = x
        .chunks(cols)
        .flat_map(|r| {
            let mean = r.iter().sum::<f32>() / cols as f32;
            let var = r.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / cols as f32;
            r.iter().enumerate().map(|(i, v)| gelu((v - mean) / (var + 1e-6).sqrt() * wt[i] + b[i])).collect::<Vec<_>>()
        })
        .collect();
    assert_close(&run_and_read(&g, &gy), &want, 1e-3, "layernorm + gelu");
}

#[test]
fn half_precision_weights_give_the_same_product_to_half_precision() {
    let Some(g) = gpu() else { return };
    // odd sizes: the last word holds one value; the weight is a linear layer's [n, k]
    let (m, n, k) = (70, 91, 41);
    let (av, wv) = (noise(m * k, 3), noise(n * k, 4));
    let (a, w16, w32) = (g.upload(&av), g.upload_f16(&wv), g.upload(&wv));
    assert!(w16.f16 && w16.len == n * k);
    let (c16, c32) = (g.zeros(m * n).unwrap(), g.zeros(m * n).unwrap());
    let sb = Strides { offset: 0, batch: 0, row: 1, col: k };
    for (w, c) in [(&w16, &c16), (&w32, &c32)] {
        g.matmul(&Matmul {
            m,
            n,
            k,
            batch: 1,
            a: (&a, Strides::row_major(k)),
            b: (w, sb),
            bias: None,
            c: (c, Strides::row_major(n)),
            residual: None,
            alpha: 1.0,
            act: Act::None,
        })
        .unwrap();
    }
    g.flush().unwrap();
    let (got, want) = (g.download(&c16).unwrap(), g.download(&c32).unwrap());
    // f16 keeps 11 bits: each weight is off by at most 2^-11 relative
    assert_close(&got, &want, 2e-3 * (k as f32).sqrt(), "f16 vs f32 weights");
    assert!(got != want, "the weights really were rounded to half precision");
}
