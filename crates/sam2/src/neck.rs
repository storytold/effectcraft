// Modified work: ported from Hugging Face `transformers` `models/sam2/modeling_sam2.py`
// (Sam2VisionNeck, Sam2SinePositionEmbedding, Sam2VisionModel; Apache-2.0, Copyright The
// HuggingFace Inc. team) to Rust on candle. See NOTICE.
//! The feature pyramid neck on top of Hiera, and the vision encoder that joins them.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use candle_core::{Device, Tensor};

use crate::hiera::{Hiera, HieraConfig};
use crate::weights::Weights;
use crate::{Error, Result};

/// Sine position encoding `[1, 2 * num_pos_features, h, w]`, normalised (`Sam2SinePositionEmbedding`).
/// It depends only on the sizes, so each is built once (it was 150 ms of a 2.7 s frame).
pub fn sine_position(num_pos_features: usize, h: usize, w: usize) -> Result<Tensor> {
    static CACHE: OnceLock<Mutex<HashMap<(usize, usize, usize), Tensor>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(t) = cache.lock().unwrap_or_else(PoisonError::into_inner).get(&(num_pos_features, h, w)) {
        return Ok(t.clone());
    }
    let t = build_sine_position(num_pos_features, h, w)?;
    let mut map = cache.lock().unwrap_or_else(PoisonError::into_inner);
    if map.len() >= 16 {
        map.clear();
    }
    map.insert((num_pos_features, h, w), t.clone());
    Ok(t)
}

fn build_sine_position(num_pos_features: usize, h: usize, w: usize) -> Result<Tensor> {
    let eps = 1e-6f32;
    let scale = 2.0 * std::f32::consts::PI;
    let dim_t: Vec<f32> = (0..num_pos_features).map(|i| 10000f32.powf(2.0 * (i / 2) as f32 / num_pos_features as f32)).collect();
    let axis = |n: usize, k: usize| -> Vec<f32> {
        // value of one coordinate at index k of an axis of length n, over the feature dimension
        let e = (k + 1) as f32 / (n as f32 + eps) * scale;
        dim_t.iter().enumerate().map(|(i, d)| if i % 2 == 0 { (e / d).sin() } else { (e / d).cos() }).collect()
    };
    let (ys, xs): (Vec<_>, Vec<_>) = ((0..h).map(|k| axis(h, k)).collect(), (0..w).map(|k| axis(w, k)).collect());
    let mut out = vec![0f32; 2 * num_pos_features * h * w];
    for y in 0..h {
        for x in 0..w {
            for f in 0..num_pos_features {
                let (py, px) = (ys.get(y).and_then(|v| v.get(f)), xs.get(x).and_then(|v| v.get(f)));
                if let (Some(py), Some(px)) = (py, px) {
                    if let Some(o) = out.get_mut((f * h + y) * w + x) {
                        *o = *py;
                    }
                    if let Some(o) = out.get_mut(((num_pos_features + f) * h + y) * w + x) {
                        *o = *px;
                    }
                }
            }
        }
    }
    Ok(Tensor::from_vec(out, (1, 2 * num_pos_features, h, w), &Device::Cpu)?)
}

struct Conv1x1 {
    w: Tensor,
    b: Tensor,
    out: usize,
}

impl Conv1x1 {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        Ok(x.conv2d(&self.w, 0, 1, 1, 1)?.broadcast_add(&self.b.reshape((1, self.out, 1, 1))?)?)
    }
}

/// The FPN neck: a 1x1 conv per stage, top-down additions on the two lowest-resolution levels.
pub struct Neck {
    convs: Vec<Conv1x1>,
    top_down: Vec<usize>,
    pos_features: usize,
}

/// What the neck produces, both in the neck's own order (lowest resolution first, all four
/// levels) as `[B, C, H, W]`.
pub struct NeckOutput {
    pub features: Vec<Tensor>,
    pub positions: Vec<Tensor>,
}

impl Neck {
    pub fn load(w: &Weights, channels: &[usize], fpn: usize, top_down: &[usize]) -> Result<Neck> {
        let convs = channels
            .iter()
            .enumerate()
            .map(|(j, c)| {
                Ok(Conv1x1 {
                    w: w.get(&format!("vision_encoder.neck.convs.{j}.weight"), &[fpn, *c, 1, 1])?,
                    b: w.get(&format!("vision_encoder.neck.convs.{j}.bias"), &[fpn])?,
                    out: fpn,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Neck { convs, top_down: top_down.to_vec(), pos_features: fpn / 2 })
    }

    /// `stages`: Hiera stage outputs, NHWC, highest resolution first.
    pub fn forward(&self, stages: &[Tensor]) -> Result<NeckOutput> {
        let n = self.convs.len().checked_sub(1).ok_or_else(|| Error::Weights("neck has no convolutions".into()))?;
        if stages.len() != self.convs.len() {
            return Err(Error::Input(format!("neck expects {} stages, got {}", self.convs.len(), stages.len())));
        }
        let (mut features, mut positions) = (vec![], vec![]);
        let mut prev: Option<Tensor> = None;
        for i in (0..=n).rev() {
            let stage = stages.get(i).ok_or_else(|| Error::Input("missing stage".into()))?;
            let conv = self.convs.get(n - i).ok_or_else(|| Error::Weights("missing neck convolution".into()))?;
            let lateral = conv.forward(&stage.permute((0, 3, 1, 2))?.contiguous()?)?;
            let cur = match (&prev, self.top_down.contains(&i) && i != n) {
                (Some(p), true) => {
                    let (h, w) = (p.dim(2)? * 2, p.dim(3)? * 2);
                    (lateral + p.upsample_nearest2d(h, w)?)?
                }
                _ => lateral,
            };
            positions.push(sine_position(self.pos_features, cur.dim(2)?, cur.dim(3)?)?);
            features.push(cur.clone());
            prev = Some(cur);
        }
        Ok(NeckOutput { features, positions })
    }
}

/// Hiera plus neck: `Sam2VisionModel`.
pub struct VisionEncoder {
    pub hiera: std::sync::Arc<Hiera>,
    pub neck: Neck,
    levels: usize,
}

/// The multi-scale image features the decoder and memory modules use, highest resolution first
/// (256², 128², 64² for a 1024 input), `[B, C, H, W]`.
pub struct ImageFeatures {
    pub features: Vec<Tensor>,
    pub positions: Vec<Tensor>,
}

impl VisionEncoder {
    /// The encoder of whichever size the checkpoint is.
    pub fn load(w: &Weights) -> Result<VisionEncoder> {
        let cfg = HieraConfig::for_size(crate::Size::detect(w)?);
        let channels: Vec<usize> = cfg.embed_dims.iter().rev().copied().collect();
        Ok(VisionEncoder { hiera: std::sync::Arc::new(Hiera::load(w, &cfg)?), neck: Neck::load(w, &channels, 256, &[2, 3])?, levels: 3 })
    }

    pub fn forward(&self, pixels: &Tensor) -> Result<ImageFeatures> {
        let stages = self.hiera.forward(pixels)?;
        let out = self.neck.forward(&stages)?;
        // keep the last `levels` of the neck order and reverse: high to low resolution
        let keep = |v: Vec<Tensor>| -> Vec<Tensor> {
            let skip = v.len().saturating_sub(self.levels);
            v.into_iter().skip(skip).rev().collect()
        };
        Ok(ImageFeatures { features: keep(out.features), positions: keep(out.positions) })
    }
}
