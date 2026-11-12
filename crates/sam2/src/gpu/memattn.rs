// Original work for EffectCraft; the maths follows the Hugging Face `transformers` SAM 2 video
// implementation (Apache-2.0, see NOTICE).
//! Memory attention on the GPU: four layers of self-attention over the current frame and cross-attention to
//! the memory bank, with axial rotary position encoding. The attention is one head of 256 channels; the
//! score matrix against a full memory bank is 4096 x 28736 floats, which stays on the GPU.

use std::sync::Arc;

use candle_core::{Device, Tensor};

use super::{Act, Buf, GLinear, GNorm, Gpu, Matmul, Strides};
use crate::memattn::{GRID, rope_tables};
use crate::weights::Weights;
use crate::{Error, Result};

const DIM: usize = 256;
const LAYERS: usize = 4;

struct GAttn {
    q: GLinear,
    k: GLinear,
    v: GLinear,
    o: GLinear,
}

struct GLayer {
    self_attn: GAttn,
    cross: GAttn,
    ln1: GNorm,
    ln2: GNorm,
    ln3: GNorm,
    lin1: GLinear,
    lin2: GLinear,
}

pub struct GpuMemoryAttention {
    gpu: Arc<Gpu>,
    layers: Vec<GLayer>,
    final_ln: GNorm,
    cos: Buf,
    sin: Buf,
}

fn attn(g: &Gpu, w: &Weights, prefix: &str, kv: usize) -> Result<GAttn> {
    Ok(GAttn {
        q: GLinear::load(g, w, &format!("{prefix}.q_proj"), DIM, DIM)?,
        k: GLinear::load(g, w, &format!("{prefix}.k_proj"), kv, DIM)?,
        v: GLinear::load(g, w, &format!("{prefix}.v_proj"), kv, DIM)?,
        o: GLinear::load(g, w, &format!("{prefix}.o_proj"), DIM, DIM)?,
    })
}

impl GpuMemoryAttention {
    pub fn load(gpu: Arc<Gpu>, w: &Weights) -> Result<GpuMemoryAttention> {
        let layers = (0..LAYERS)
            .map(|i| {
                let p = format!("memory_attention.layers.{i}");
                Ok(GLayer {
                    self_attn: attn(&gpu, w, &format!("{p}.self_attn"), DIM)?,
                    cross: attn(&gpu, w, &format!("{p}.cross_attn_image"), 64)?,
                    ln1: GNorm::load(&gpu, w, &format!("{p}.layer_norm1"), DIM)?,
                    ln2: GNorm::load(&gpu, w, &format!("{p}.layer_norm2"), DIM)?,
                    ln3: GNorm::load(&gpu, w, &format!("{p}.layer_norm3"), DIM)?,
                    lin1: GLinear::load(&gpu, w, &format!("{p}.linear1"), DIM, 2048)?,
                    lin2: GLinear::load(&gpu, w, &format!("{p}.linear2"), 2048, DIM)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let (cos, sin) = rope_tables(10000.0)?;
        let flat = |t: Tensor| -> Result<Vec<f32>> { Ok(t.flatten_all()?.to_vec1::<f32>()?) };
        Ok(GpuMemoryAttention {
            final_ln: GNorm::load(&gpu, w, "memory_attention.layer_norm", DIM)?,
            cos: gpu.upload(&flat(cos)?),
            sin: gpu.upload(&flat(sin)?),
            layers,
            gpu,
        })
    }

    fn norm(&self, x: &Buf, rows: usize, n: &GNorm) -> Result<Buf> {
        let out = self.gpu.zeros(rows * DIM)?;
        self.gpu.layernorm(x, &n.w, &n.b, &out, rows, DIM, 1e-5, false)?;
        Ok(out)
    }

    fn rope(&self, x: &Buf, rows: usize, rotated: usize) -> Result<Buf> {
        let out = self.gpu.zeros(rows * DIM)?;
        self.gpu.rope(x, &self.cos, &self.sin, &out, rows, DIM, GRID * GRID, rotated)?;
        Ok(out)
    }

    /// `softmax(q . k^T / 16) . v` for `[nq, 256]` queries and `[nk, 256]` keys and values, through `scores`
    /// (at least `nq * nk` elements); returns `[nq, 256]`.
    fn attention(&self, q: &Buf, k: &Buf, v: &Buf, nq: usize, nk: usize, scores: &Buf) -> Result<Buf> {
        let g = &self.gpu;
        g.matmul(&Matmul {
            m: nq,
            n: nk,
            k: DIM,
            batch: 1,
            a: (q, Strides::row_major(DIM)),
            b: (k, Strides { offset: 0, batch: 0, row: 1, col: DIM }),
            bias: None,
            c: (scores, Strides::row_major(nk)),
            residual: None,
            alpha: (DIM as f32).powf(-0.5),
            act: Act::None,
        })?;
        g.softmax(scores, nq, nk)?;
        let out = g.zeros(nq * DIM)?;
        g.matmul(&Matmul {
            m: nq,
            n: DIM,
            k: nk,
            batch: 1,
            a: (scores, Strides::row_major(nk)),
            b: (v, Strides::row_major(DIM)),
            bias: None,
            c: (&out, Strides::row_major(DIM)),
            residual: None,
            alpha: 1.0,
            act: Act::None,
        })?;
        Ok(out)
    }

    /// As [`crate::memattn::MemoryAttention::forward`]: `current` and `current_pos` `[4096, 256]`, `memory` and
    /// `memory_pos` `[M, 64]`, the last `num_pointers` memory rows being object pointers (not rotated).
    pub fn forward(&self, current: &Tensor, current_pos: &Tensor, memory: &Tensor, memory_pos: &Tensor, num_pointers: usize) -> Result<Tensor> {
        let g = &self.gpu;
        let _turn = g.exclusive();
        let n = GRID * GRID;
        if current.dims() != [n, DIM] || current_pos.dims() != [n, DIM] {
            return Err(Error::Input(format!("current features must be [{n}, {DIM}], got {:?}", current.dims())));
        }
        let (m, md) = memory.dims2()?;
        if md != 64 || memory_pos.dims() != [m, 64] {
            return Err(Error::Input(format!("memory must be [M, 64] with a matching position encoding, got {:?}", memory.dims())));
        }
        let rotated = m.checked_sub(num_pointers).ok_or_else(|| Error::Input("more object pointers than memory tokens".into()))?;
        if rotated % n != 0 {
            return Err(Error::Input(format!("memory length {rotated} is not a multiple of {n}")));
        }
        let flat = |t: &Tensor| -> Result<Vec<f32>> { Ok(t.contiguous()?.flatten_all()?.to_vec1::<f32>()?) };
        let mut x = g.upload(&flat(&(current + (current_pos * 0.1)?)?)?);
        let mem = g.upload(&flat(memory)?);
        let keys_in = g.upload(&flat(&(memory + memory_pos)?)?);
        let scores = g.zeros(n * n.max(m))?;
        for l in &self.layers {
            // self-attention over the frame
            let q_in = self.norm(&x, n, &l.ln1)?;
            let (q, k, v) = (g.zeros(n * DIM)?, g.zeros(n * DIM)?, g.zeros(n * DIM)?);
            g.linear(&q_in, n, &l.self_attn.q, &q, Act::None, None)?;
            g.linear(&q_in, n, &l.self_attn.k, &k, Act::None, None)?;
            g.linear(&q_in, n, &l.self_attn.v, &v, Act::None, None)?;
            let (q, k) = (self.rope(&q, n, n)?, self.rope(&k, n, n)?);
            let a = self.attention(&q, &k, &v, n, n, &scores)?;
            let x1 = g.zeros(n * DIM)?;
            g.linear(&a, n, &l.self_attn.o, &x1, Act::None, Some(&x))?;
            // cross-attention to the memory bank
            let q_in = self.norm(&x1, n, &l.ln2)?;
            let q = g.zeros(n * DIM)?;
            g.linear(&q_in, n, &l.cross.q, &q, Act::None, None)?;
            let q = self.rope(&q, n, n)?;
            let (k, v) = (g.zeros(m * DIM)?, g.zeros(m * DIM)?);
            g.linear(&keys_in, m, &l.cross.k, &k, Act::None, None)?;
            g.linear(&mem, m, &l.cross.v, &v, Act::None, None)?;
            let k = self.rope(&k, m, rotated)?;
            let a = self.attention(&q, &k, &v, n, m, &scores)?;
            let x2 = g.zeros(n * DIM)?;
            g.linear(&a, n, &l.cross.o, &x2, Act::None, Some(&x1))?;
            // feed-forward
            let f_in = self.norm(&x2, n, &l.ln3)?;
            let hidden = g.zeros(n * 2048)?;
            g.linear(&f_in, n, &l.lin1, &hidden, Act::Relu, None)?;
            let x3 = g.zeros(n * DIM)?;
            g.linear(&hidden, n, &l.lin2, &x3, Act::None, Some(&x2))?;
            x = x3;
        }
        let out = self.norm(&x, n, &self.final_ln)?;
        g.flush()?;
        Ok(Tensor::from_vec(g.download(&out)?, (n, DIM), &Device::Cpu)?)
    }
}
