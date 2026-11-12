// Modified work: ported from Hugging Face `transformers` `models/sam2_video/modeling_sam2_video.py`
// (Sam2VideoMemoryAttention, Sam2VideoMemoryAttentionLayer, Sam2VideoRoPEAttention,
// Sam2VideoVisionRotaryEmbedding; Apache-2.0, Copyright The HuggingFace Inc. team) to Rust on
// candle. See NOTICE.
//! Memory attention: the current frame's features attend to themselves and to the memory bank
//! (memory frames plus object pointers) with axial 2-D rotary position encoding.

use candle_core::{D, Device, Tensor};
use candle_nn::{Linear, ops::softmax_last_dim};

use crate::nn::{LayerNorm, apply, layer_norm, linear};
use crate::weights::Weights;
use crate::{Error, Result};

const DIM: usize = 256;
const LAYERS: usize = 4;
/// Side of the feature grid the rotary encoding is defined on.
pub const GRID: usize = 64;

/// `cos` and `sin` of the axial rotary encoding for the `GRID x GRID` grid, each `[GRID², 256]`.
pub(crate) fn rope_tables(theta: f32) -> Result<(Tensor, Tensor)> {
    let freqs = DIM / 2 / 2; // per axis: head_dim / 4
    let inv: Vec<f32> = (0..freqs).map(|i| 1.0 / theta.powf(i as f32 / freqs as f32)).collect();
    let n = GRID * GRID;
    let (mut cos, mut sin) = (vec![0f32; n * DIM], vec![0f32; n * DIM]);
    for p in 0..n {
        let (x, y) = ((p % GRID) as f32, (p / GRID) as f32);
        for j in 0..DIM / 2 {
            let f = if j < freqs { x * inv.get(j).copied().unwrap_or(0.0) } else { y * inv.get(j - freqs).copied().unwrap_or(0.0) };
            for k in 0..2 {
                if let (Some(c), Some(s)) = (cos.get_mut(p * DIM + 2 * j + k), sin.get_mut(p * DIM + 2 * j + k)) {
                    *c = f.cos();
                    *s = f.sin();
                }
            }
        }
    }
    Ok((Tensor::from_vec(cos, (n, DIM), &Device::Cpu)?, Tensor::from_vec(sin, (n, DIM), &Device::Cpu)?))
}

/// `(x0, x1) -> (-x1, x0)` over adjacent pairs of the last dimension.
fn rotate_pairwise(x: &Tensor) -> Result<Tensor> {
    let (n, d) = x.dims2()?;
    let pairs = x.reshape((n, d / 2, 2))?;
    let (x1, x2) = (pairs.narrow(2, 0, 1)?.squeeze(2)?, pairs.narrow(2, 1, 1)?.squeeze(2)?);
    Ok(Tensor::stack(&[&x2.neg()?, &x1], D::Minus1)?.reshape((n, d))?)
}

fn rope(x: &Tensor, cos: &Tensor, sin: &Tensor) -> Result<Tensor> {
    Ok(((x * cos)? + (rotate_pairwise(x)? * sin)?)?)
}

struct RopeAttention {
    q: Linear,
    k: Linear,
    v: Linear,
    o: Linear,
}

impl RopeAttention {
    fn load(w: &Weights, prefix: &str, kv_dim: usize) -> Result<RopeAttention> {
        Ok(RopeAttention {
            q: linear(w, &format!("{prefix}.q_proj"), DIM, DIM)?,
            k: linear(w, &format!("{prefix}.k_proj"), kv_dim, DIM)?,
            v: linear(w, &format!("{prefix}.v_proj"), kv_dim, DIM)?,
            o: linear(w, &format!("{prefix}.o_proj"), DIM, DIM)?,
        })
    }

    /// `query`: `[N, 256]`, `key`/`value`: `[M, kv]`. The first `M - exclude` keys are rotated
    /// (their table is `cos`/`sin` tiled to length); the last `exclude` (object pointers) are not.
    fn forward(&self, query: &Tensor, key: &Tensor, value: &Tensor, tables: &(Tensor, Tensor), exclude: usize) -> Result<Tensor> {
        let (cos, sin) = tables;
        let q = rope(&apply(&self.q, query)?, cos, sin)?;
        let k = apply(&self.k, key)?;
        let v = apply(&self.v, value)?;
        let m = k.dim(0)?;
        let rotated = m.checked_sub(exclude).ok_or_else(|| Error::Input("more object pointers than memory tokens".into()))?;
        let k = if rotated == 0 {
            k
        } else {
            let n = cos.dim(0)?;
            let reps = if rotated == n {
                1
            } else if rotated % n == 0 {
                rotated / n
            } else {
                return Err(Error::Input(format!("memory length {rotated} is not a multiple of {n}")));
            };
            let tile = |t: &Tensor| -> Result<Tensor> { Ok(Tensor::cat(&vec![t; reps], 0)?) };
            let k_rot = rope(&k.narrow(0, 0, rotated)?, &tile(cos)?, &tile(sin)?)?;
            if exclude == 0 { k_rot } else { Tensor::cat(&[&k_rot, &k.narrow(0, rotated, exclude)?], 0)? }
        };
        let scale = (DIM as f64).powf(-0.5);
        let att = softmax_last_dim(&((q * scale)?.matmul(&k.t()?.contiguous()?)?))?;
        apply(&self.o, &att.matmul(&v)?)
    }
}

struct Layer {
    self_attn: RopeAttention,
    cross: RopeAttention,
    ln1: LayerNorm,
    ln2: LayerNorm,
    ln3: LayerNorm,
    lin1: Linear,
    lin2: Linear,
}

pub struct MemoryAttention {
    layers: Vec<Layer>,
    final_ln: LayerNorm,
    tables: (Tensor, Tensor),
}

impl MemoryAttention {
    pub fn load(w: &Weights) -> Result<MemoryAttention> {
        let layers = (0..LAYERS)
            .map(|i| {
                let p = format!("memory_attention.layers.{i}");
                Ok(Layer {
                    self_attn: RopeAttention::load(w, &format!("{p}.self_attn"), DIM)?,
                    cross: RopeAttention::load(w, &format!("{p}.cross_attn_image"), 64)?,
                    ln1: layer_norm(w, &format!("{p}.layer_norm1"), DIM, 1e-5)?,
                    ln2: layer_norm(w, &format!("{p}.layer_norm2"), DIM, 1e-5)?,
                    ln3: layer_norm(w, &format!("{p}.layer_norm3"), DIM, 1e-5)?,
                    lin1: linear(w, &format!("{p}.linear1"), DIM, 2048)?,
                    lin2: linear(w, &format!("{p}.linear2"), 2048, DIM)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(MemoryAttention { layers, final_ln: layer_norm(w, "memory_attention.layer_norm", DIM, 1e-5)?, tables: rope_tables(10000.0)? })
    }

    /// Condition the current frame on memory.
    ///
    /// `current` and `current_pos`: `[4096, 256]` (the lowest-resolution neck level and its
    /// position encoding, one row per cell); `memory` and `memory_pos`: `[M, 64]` (memory frames
    /// followed by `num_pointers` object-pointer tokens). Returns `[4096, 256]`.
    pub fn forward(&self, current: &Tensor, current_pos: &Tensor, memory: &Tensor, memory_pos: &Tensor, num_pointers: usize) -> Result<Tensor> {
        if current.dims() != [GRID * GRID, DIM] {
            return Err(Error::Input(format!("current features must be [{}, {DIM}], got {:?}", GRID * GRID, current.dims())));
        }
        let keys_pos = (memory + memory_pos)?;
        let mut out = (current + (current_pos * 0.1)?)?;
        for l in &self.layers {
            let q = l.ln1.forward(&out)?;
            out = (&out + l.self_attn.forward(&q, &q, &q, &self.tables, 0)?)?;
            let q = l.ln2.forward(&out)?;
            out = (&out + l.cross.forward(&q, &keys_pos, memory, &self.tables, num_pointers)?)?;
            let q = l.ln3.forward(&out)?;
            out = (&out + apply(&l.lin2, &apply(&l.lin1, &q)?.relu()?)?)?;
        }
        self.final_ln.forward(&out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_is_pairwise_and_tables_start_at_identity() {
        let x = Tensor::new(&[[1f32, 2.0, 3.0, 4.0]], &Device::Cpu).unwrap();
        let r = rotate_pairwise(&x).unwrap().to_vec2::<f32>().unwrap();
        assert_eq!(r, vec![vec![-2.0, 1.0, -4.0, 3.0]]);
        let (cos, sin) = rope_tables(10000.0).unwrap();
        // cell (0, 0): angle 0 everywhere
        let c = cos.narrow(0, 0, 1).unwrap().to_vec2::<f32>().unwrap();
        let s = sin.narrow(0, 0, 1).unwrap().to_vec2::<f32>().unwrap();
        assert!(c[0].iter().all(|v| *v == 1.0) && s[0].iter().all(|v| *v == 0.0));
    }
}
