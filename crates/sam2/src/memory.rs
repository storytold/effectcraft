// Modified work: ported from Hugging Face `transformers` `models/sam2_video/modeling_sam2_video.py`
// (Sam2VideoMemoryEncoder, Sam2VideoMaskDownSampler, Sam2VideoMemoryFuser; Apache-2.0, Copyright
// The HuggingFace Inc. team) to Rust on candle. See NOTICE.
//! The memory encoder: a frame's mask is down-sampled and fused with the frame's image features
//! into a compact memory (64 channels on the 64 x 64 grid) plus its position encoding.

use candle_core::Tensor;

use crate::Result;
use crate::neck::sine_position;
use crate::nn::{LayerNorm, apply, apply_gelu, gelu, layer_norm, linear};
use crate::weights::Weights;
use candle_nn::Linear;

struct Conv {
    w: Tensor,
    b: Tensor,
    padding: usize,
    stride: usize,
    groups: usize,
}

impl Conv {
    fn load(w: &Weights, name: &str, shape: &[usize], padding: usize, stride: usize, groups: usize) -> Result<Conv> {
        let out = shape.first().copied().unwrap_or(0);
        Ok(Conv { w: w.get(&format!("{name}.weight"), shape)?, b: w.get(&format!("{name}.bias"), &[out])?, padding, stride, groups })
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let y = x.conv2d(&self.w, self.padding, self.stride, 1, self.groups)?;
        Ok(y.broadcast_add(&self.b.reshape((1, self.b.dim(0)?, 1, 1))?)?)
    }
}

/// Layer norm over the channel dimension of `[B, C, H, W]`.
fn norm_channels_first(ln: &LayerNorm, x: &Tensor) -> Result<Tensor> {
    ln.forward(&x.permute((0, 2, 3, 1))?.contiguous()?)?.permute((0, 3, 1, 2))?.contiguous().map_err(Into::into)
}

struct FuserBlock {
    depthwise: Conv,
    norm: LayerNorm,
    pw1: Linear,
    pw2: Linear,
    scale: Tensor,
}

impl FuserBlock {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let h = norm_channels_first(&self.norm, &self.depthwise.forward(x)?)?;
        let h = h.permute((0, 2, 3, 1))?.contiguous()?;
        let h = apply(&self.pw2, &apply_gelu(&self.pw1, &h)?)?;
        let h = h.broadcast_mul(&self.scale)?.permute((0, 3, 1, 2))?.contiguous()?;
        Ok((x + h)?)
    }
}

pub struct MemoryEncoder {
    down: Vec<(Conv, LayerNorm)>,
    final_conv: Conv,
    feature_projection: Conv,
    fuser: Vec<FuserBlock>,
    projection: Conv,
    occlusion: Tensor,
}

/// A frame's memory: `features` is `[HW, 64]` rounded through bfloat16 exactly as the reference
/// stores it, `pos` is the `[HW, 64]` sine position encoding.
pub struct Memory {
    pub features: Tensor,
    pub pos: Tensor,
}

impl MemoryEncoder {
    pub fn load(w: &Weights) -> Result<MemoryEncoder> {
        let mut down = vec![];
        let mut ch = 1;
        for i in 0..4 {
            let out = ch * 4;
            let p = format!("memory_encoder.mask_downsampler.layers.{i}");
            down.push((Conv::load(w, &format!("{p}.conv"), &[out, ch, 3, 3], 1, 2, 1)?, layer_norm(w, &format!("{p}.layer_norm"), out, 1e-6)?));
            ch = out;
        }
        let fuser = (0..2)
            .map(|i| {
                let p = format!("memory_encoder.memory_fuser.layers.{i}");
                Ok(FuserBlock {
                    depthwise: Conv::load(w, &format!("{p}.depthwise_conv"), &[256, 1, 7, 7], 3, 1, 256)?,
                    norm: layer_norm(w, &format!("{p}.layer_norm"), 256, 1e-6)?,
                    pw1: linear(w, &format!("{p}.pointwise_conv1"), 256, 1024)?,
                    pw2: linear(w, &format!("{p}.pointwise_conv2"), 1024, 256)?,
                    scale: w.get(&format!("{p}.scale"), &[256])?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(MemoryEncoder {
            down,
            final_conv: Conv::load(w, "memory_encoder.mask_downsampler.final_conv", &[256, 256, 1, 1], 0, 1, 1)?,
            feature_projection: Conv::load(w, "memory_encoder.feature_projection", &[256, 256, 1, 1], 0, 1, 1)?,
            fuser,
            projection: Conv::load(w, "memory_encoder.projection", &[64, 256, 1, 1], 0, 1, 1)?,
            occlusion: w.get("occlusion_spatial_embedding_parameter", &[1, 64])?,
        })
    }

    /// `features`: `[1, 256, g, g]` image features; `mask`: `[1, 1, 16 g, 16 g]` already scaled
    /// (`20 * m - 10`). Returns the raw `[1, 64, g, g]` features and position encoding.
    pub fn encode(&self, features: &Tensor, mask: &Tensor) -> Result<(Tensor, Tensor)> {
        let mut m = mask.clone();
        for (conv, ln) in &self.down {
            m = gelu(&norm_channels_first(ln, &conv.forward(&m)?)?)?;
        }
        let m = self.final_conv.forward(&m)?;
        let mut x = (self.feature_projection.forward(features)? + m)?;
        for b in &self.fuser {
            x = b.forward(&x)?;
        }
        let x = self.projection.forward(&x)?;
        let pos = sine_position(32, x.dim(2)?, x.dim(3)?)?;
        Ok((x, pos))
    }

    /// The memory kept for a frame: the encoder output, plus the occlusion embedding when the
    /// object is absent, rounded through bfloat16 like the reference.
    pub fn memory(&self, features: &Tensor, mask: &Tensor, object_present: bool) -> Result<Memory> {
        let (x, _) = self.encode(features, mask)?;
        self.finish(&x.flatten_from(2)?.squeeze(0)?.t()?.contiguous()?, object_present)
    }

    /// A memory from the encoder's raw tokens `[HW, 64]`: the occlusion embedding when the object is absent,
    /// rounded through bfloat16, with the position encoding (shared by the CPU and GPU encoders).
    pub fn finish(&self, tokens: &Tensor, object_present: bool) -> Result<Memory> {
        let tokens = if object_present { tokens.clone() } else { tokens.broadcast_add(&self.occlusion)? };
        let features = tokens.to_dtype(candle_core::DType::BF16)?.to_dtype(candle_core::DType::F32)?;
        let pos = sine_position(32, 64, 64)?;
        Ok(Memory { features, pos: pos.flatten_from(2)?.squeeze(0)?.t()?.contiguous()? })
    }
}
