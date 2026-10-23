// Original work for EffectCraft; the maths follows the Hugging Face `transformers` SAM 2 video
// implementation (Apache-2.0, see NOTICE).
//! The memory encoder on the GPU: the mask is down-sampled by four strided convolutions, fused with the
//! frame's features by two ConvNeXt-style blocks, and projected to the 64 channels of a memory.

use std::sync::Arc;

use candle_core::{Device, Tensor};

use super::{Act, Buf, GLinear, GNorm, Gpu};
use crate::weights::Weights;
use crate::{Error, Result};

struct GConv {
    w: Buf,
    b: Buf,
    ci: usize,
    co: usize,
    k: usize,
}

struct GFuser {
    dw: GConv,
    norm: GNorm,
    pw1: GLinear,
    /// The layer scale is folded into this layer's weights.
    pw2: GLinear,
}

pub struct GpuMemoryEncoder {
    gpu: Arc<Gpu>,
    down: Vec<(GConv, GNorm)>,
    final_conv: GLinear,
    feature_projection: GLinear,
    fuser: Vec<GFuser>,
    projection: GLinear,
}

fn conv(g: &Gpu, w: &Weights, name: &str, ci: usize, co: usize, k: usize, groups: usize) -> Result<GConv> {
    let flat = |t: Tensor| -> Result<Vec<f32>> { Ok(t.flatten_all()?.to_vec1::<f32>()?) };
    Ok(GConv {
        w: g.upload(&flat(w.get(&format!("{name}.weight"), &[co, ci / groups, k, k])?)?),
        b: g.upload(&flat(w.get(&format!("{name}.bias"), &[co])?)?),
        ci,
        co,
        k,
    })
}

impl GpuMemoryEncoder {
    pub fn load(gpu: Arc<Gpu>, w: &Weights) -> Result<GpuMemoryEncoder> {
        let mut down = vec![];
        let mut ch = 1;
        for i in 0..4 {
            let out = ch * 4;
            let p = format!("memory_encoder.mask_downsampler.layers.{i}");
            down.push((conv(&gpu, w, &format!("{p}.conv"), ch, out, 3, 1)?, GNorm::load(&gpu, w, &format!("{p}.layer_norm"), out)?));
            ch = out;
        }
        let fuser = (0..2)
            .map(|i| {
                let p = format!("memory_encoder.memory_fuser.layers.{i}");
                let scale = w.get(&format!("{p}.scale"), &[256])?.flatten_all()?.to_vec1::<f32>()?;
                Ok(GFuser {
                    dw: conv(&gpu, w, &format!("{p}.depthwise_conv"), 256, 256, 7, 256)?,
                    norm: GNorm::load(&gpu, w, &format!("{p}.layer_norm"), 256)?,
                    pw1: GLinear::load(&gpu, w, &format!("{p}.pointwise_conv1"), 256, 1024)?,
                    pw2: GLinear::load_scaled(&gpu, w, &format!("{p}.pointwise_conv2"), 1024, 256, &scale)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(GpuMemoryEncoder {
            final_conv: GLinear::load_conv1x1(&gpu, w, "memory_encoder.mask_downsampler.final_conv", 256, 256)?,
            feature_projection: GLinear::load_conv1x1(&gpu, w, "memory_encoder.feature_projection", 256, 256)?,
            projection: GLinear::load_conv1x1(&gpu, w, "memory_encoder.projection", 256, 64)?,
            down,
            fuser,
            gpu,
        })
    }

    /// `features`: `[1, 256, 64, 64]`; `mask`: `[1, 1, 1024, 1024]` already scaled (`20 m - 10`). Returns the raw
    /// memory tokens `[4096, 64]` (the occlusion embedding and bfloat16 rounding are the caller's).
    pub fn encode(&self, features: &Tensor, mask: &Tensor) -> Result<Tensor> {
        let g = &self.gpu;
        let _turn = g.exclusive();
        let flat = |t: &Tensor| -> Result<Vec<f32>> { Ok(t.contiguous()?.flatten_all()?.to_vec1::<f32>()?) };
        if features.dims() != [1, 256, 64, 64] || mask.dims() != [1, 1, 1024, 1024] {
            return Err(Error::Input(format!("memory encoder input shapes {:?} / {:?}", features.dims(), mask.dims())));
        }
        let tokens = features.permute((0, 2, 3, 1))?.contiguous()?;
        let ftok = g.upload(&flat(&tokens)?);
        let mut cur = g.upload(&flat(mask)?);
        let (mut h, mut w) = (1024usize, 1024usize);
        for (c, ln) in &self.down {
            let (ho, wo) = (h / 2, w / 2);
            let raw = g.zeros(ho * wo * c.co)?;
            g.conv2d(&cur, &c.w, &c.b, &raw, (h, w, c.ci, c.co), c.k, 2, 1, 1)?;
            let normed = g.zeros(ho * wo * c.co)?;
            g.layernorm(&raw, &ln.w, &ln.b, &normed, ho * wo, c.co, 1e-6, true)?;
            (cur, h, w) = (normed, ho, wo);
        }
        let n = h * w;
        if n != 4096 {
            return Err(Error::Compute(format!("the mask down-sampler ended at {w}x{h}, not 64x64")));
        }
        let m = g.zeros(n * 256)?;
        g.linear(&cur, n, &self.final_conv, &m, Act::None, None)?;
        let mut x = g.zeros(n * 256)?;
        g.linear(&ftok, n, &self.feature_projection, &x, Act::None, Some(&m))?;
        for b in &self.fuser {
            let d = g.zeros(n * 256)?;
            g.conv2d(&x, &b.dw.w, &b.dw.b, &d, (h, w, 256, 256), 7, 1, 3, 256)?;
            let normed = g.zeros(n * 256)?;
            g.layernorm(&d, &b.norm.w, &b.norm.b, &normed, n, 256, 1e-6, false)?;
            let hidden = g.zeros(n * 1024)?;
            g.linear(&normed, n, &b.pw1, &hidden, Act::Gelu, None)?;
            let next = g.zeros(n * 256)?;
            g.linear(&hidden, n, &b.pw2, &next, Act::None, Some(&x))?;
            x = next;
        }
        let out = g.zeros(n * 64)?;
        g.linear(&x, n, &self.projection, &out, Act::None, None)?;
        g.flush()?;
        Ok(Tensor::from_vec(g.download(&out)?, (n, 64), &Device::Cpu)?)
    }
}
