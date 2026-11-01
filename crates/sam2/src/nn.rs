// Modified work: small layers used by the ported SAM 2 modules (Apache-2.0, see NOTICE).
//! Linear, layer norm and a few tensor helpers on candle.

use candle_core::{CpuStorage, CustomOp1, CustomOp2, Layout, Shape, Tensor};
use candle_nn::Linear;

use crate::weights::Weights;
use crate::{Error, Result};

pub fn linear(w: &Weights, prefix: &str, input: usize, output: usize) -> Result<Linear> {
    Ok(Linear::new(w.get(&format!("{prefix}.weight"), &[output, input])?, Some(w.get(&format!("{prefix}.bias"), &[output])?)))
}

/// Layer norm over the last dimension using candle's fused kernel (much faster than the
/// composed `candle_nn::LayerNorm` on large activations).
pub struct LayerNorm {
    weight: Tensor,
    bias: Tensor,
    eps: f32,
}

impl LayerNorm {
    pub fn forward(&self, x: &Tensor) -> Result<Tensor> {
        Ok(candle_nn::ops::layer_norm(&x.contiguous()?, &self.weight, &self.bias, self.eps)?)
    }
}

pub fn layer_norm(w: &Weights, prefix: &str, dim: usize, eps: f64) -> Result<LayerNorm> {
    Ok(LayerNorm { weight: w.get(&format!("{prefix}.weight"), &[dim])?, bias: w.get(&format!("{prefix}.bias"), &[dim])?, eps: eps as f32 })
}

/// Two-layer MLP with exact (erf) GELU, as `Sam2FeedForward` with `num_layers = 2`.
pub struct Mlp {
    pub proj_in: Linear,
    pub proj_out: Linear,
}

impl Mlp {
    pub fn load(w: &Weights, prefix: &str, dim: usize, hidden: usize) -> Result<Mlp> {
        Ok(Mlp { proj_in: linear(w, &format!("{prefix}.proj_in"), dim, hidden)?, proj_out: linear(w, &format!("{prefix}.proj_out"), hidden, dim)? })
    }

    pub fn forward(&self, x: &Tensor) -> Result<Tensor> {
        apply(&self.proj_out, &apply_gelu(&self.proj_in, x)?)
    }
}

/// `x + bias` (and, for [`bias_gelu`], then GELU) over rows of `x`, on all cores. Candle adds a bias
/// on one thread, which cost more than the matmul it followed on the wide activations of the early
/// Hiera blocks.
struct BiasAdd {
    gelu: bool,
}

impl CustomOp2 for BiasAdd {
    fn name(&self) -> &'static str {
        "bias_add_parallel"
    }

    fn cpu_fwd(&self, xs: &CpuStorage, xl: &Layout, bs: &CpuStorage, bl: &Layout) -> candle_core::Result<(CpuStorage, Shape)> {
        use rayon::prelude::*;
        let (CpuStorage::F32(x), CpuStorage::F32(b)) = (xs, bs) else { candle_core::bail!("bias_add_parallel needs f32") };
        let (Some((x0, x1)), Some((b0, b1))) = (xl.contiguous_offsets(), bl.contiguous_offsets()) else {
            candle_core::bail!("bias_add_parallel needs contiguous inputs")
        };
        let (x, b) = match (x.get(x0..x1), b.get(b0..b1)) {
            (Some(x), Some(b)) if !b.is_empty() && x.len() % b.len() == 0 => (x, b),
            _ => candle_core::bail!("bias_add_parallel: sizes do not fit"),
        };
        let cols = b.len();
        let mut out = vec![0f32; x.len()];
        let rows_per = (64 * 1024 / cols).max(1);
        let gelu = self.gelu;
        out.par_chunks_mut(rows_per * cols).zip(x.par_chunks(rows_per * cols)).for_each(|(o, i)| {
            for (orow, irow) in o.chunks_mut(cols).zip(i.chunks(cols)) {
                for ((a, v), bias) in orow.iter_mut().zip(irow).zip(b) {
                    let y = v + bias;
                    *a = if gelu { (candle_core::cpu::erf::erf_f32(y * std::f32::consts::FRAC_1_SQRT_2) + 1.0) * 0.5 * y } else { y };
                }
            }
        });
        Ok((CpuStorage::F32(out), xl.shape().clone()))
    }
}

fn bias_op(x: &Tensor, bias: &Tensor, gelu: bool) -> Result<Tensor> {
    if x.dtype() != candle_core::DType::F32 || !x.device().is_cpu() {
        let y = x.broadcast_add(bias)?;
        return Ok(if gelu { y.gelu_erf()? } else { y });
    }
    Ok(x.contiguous()?.apply_op2_no_bwd(&bias.contiguous()?, &BiasAdd { gelu })?)
}

fn matmul_bias(l: &Linear, x: &Tensor, gelu: bool) -> Result<Tensor> {
    let dims = x.dims();
    let (&last, lead) = dims.split_last().ok_or_else(|| Error::Compute("linear on a scalar".into()))?;
    let rows: usize = lead.iter().product();
    let y = x.reshape((rows, last))?.matmul(&l.weight().t()?)?;
    let y = match l.bias() {
        Some(b) => bias_op(&y, b, gelu)?,
        None if gelu => y.gelu_erf()?,
        None => y,
    };
    let mut out: Vec<usize> = lead.to_vec();
    out.push(y.dim(1)?);
    Ok(y.reshape(out)?)
}

/// Apply a linear layer to `[.., in]` through a flat 2-D matmul and a parallel bias add. Candle's
/// `Linear::forward` broadcasts the weight over every leading dimension (copying it per window) and
/// adds the bias on one thread: both slow on the 4-D activations of the backbone.
pub fn apply(l: &Linear, x: &Tensor) -> Result<Tensor> {
    matmul_bias(l, x, false)
}

/// [`apply`] followed by the exact GELU, in the same pass over the output.
pub fn apply_gelu(l: &Linear, x: &Tensor) -> Result<Tensor> {
    matmul_bias(l, x, true)
}

/// Exact (erf) GELU on all cores. Candle's own `gelu_erf` runs on one thread, which made it a third of
/// the time of the Hiera blocks; this gives the same values (it calls the same `erf` per element).
struct GeluErf;

impl CustomOp1 for GeluErf {
    fn name(&self) -> &'static str {
        "gelu_erf_parallel"
    }

    fn cpu_fwd(&self, storage: &CpuStorage, layout: &Layout) -> candle_core::Result<(CpuStorage, Shape)> {
        use rayon::prelude::*;
        let CpuStorage::F32(data) = storage else { candle_core::bail!("gelu_erf_parallel needs f32") };
        let Some((start, end)) = layout.contiguous_offsets() else { candle_core::bail!("gelu_erf_parallel needs a contiguous input") };
        let src = data.get(start..end).ok_or_else(|| candle_core::Error::Msg("gelu_erf_parallel: layout outside the storage".into()))?;
        let mut out = vec![0f32; src.len()];
        const CHUNK: usize = 16 * 1024;
        out.par_chunks_mut(CHUNK).zip(src.par_chunks(CHUNK)).for_each(|(o, i)| {
            for (a, v) in o.iter_mut().zip(i) {
                *a = (candle_core::cpu::erf::erf_f32(v * std::f32::consts::FRAC_1_SQRT_2) + 1.0) * 0.5 * v;
            }
        });
        Ok((CpuStorage::F32(out), layout.shape().clone()))
    }
}

/// Exact GELU of `x`, in parallel for f32 tensors.
pub fn gelu(x: &Tensor) -> Result<Tensor> {
    if x.dtype() != candle_core::DType::F32 || !x.device().is_cpu() {
        return Ok(x.gelu_erf()?);
    }
    Ok(x.contiguous()?.apply_op1_no_bwd(&GeluErf)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::Device;

    #[test]
    fn the_parallel_gelu_equals_candles_bit_for_bit() {
        let v: Vec<f32> = (0..101_000).map(|i| ((i as f32) * 0.00037).sin() * 6.0 - 0.5).collect();
        let x = Tensor::from_vec(v, (101, 1000), &Device::Cpu).unwrap();
        let (a, b) = (gelu(&x).unwrap(), x.gelu_erf().unwrap());
        assert_eq!(a.flatten_all().unwrap().to_vec1::<f32>().unwrap(), b.flatten_all().unwrap().to_vec1::<f32>().unwrap());
        // a transposed (non-contiguous) input works, and an empty one does too
        let t = x.t().unwrap();
        assert_eq!(gelu(&t).unwrap().dims(), t.dims());
        let e = Tensor::zeros((0, 4), candle_core::DType::F32, &Device::Cpu).unwrap();
        assert_eq!(gelu(&e).unwrap().elem_count(), 0);
    }
}
