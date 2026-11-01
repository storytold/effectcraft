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
/// Only small tables are retained globally; the neck owns its larger feature-pyramid tables.
pub fn sine_position(num_pos_features: usize, h: usize, w: usize) -> Result<Tensor> {
    static CACHE: OnceLock<PositionCache> = OnceLock::new();
    CACHE.get_or_init(|| PositionCache::new(1024 * 1024 / size_of::<f32>())).get(num_pos_features, h, w)
}

const MAX_POSITION_ELEMENTS: usize = 128 * 1024 * 1024 / size_of::<f32>();

struct PositionCache {
    entries: Mutex<HashMap<(usize, usize, usize), Tensor>>,
    max_elements: usize,
}

impl PositionCache {
    fn new(max_elements: usize) -> Self {
        Self { entries: Mutex::new(HashMap::new()), max_elements }
    }

    fn get(&self, features: usize, h: usize, w: usize) -> Result<Tensor> {
        let key = (features, h, w);
        if let Some(t) = self.entries.lock().unwrap_or_else(PoisonError::into_inner).get(&key) {
            return Ok(t.clone());
        }
        let t = build_sine_position(features, h, w)?;
        let count = t.elem_count();
        if count <= self.max_elements {
            let mut map = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
            let retained = map.values().fold(0usize, |n, t| n.saturating_add(t.elem_count()));
            if map.len() >= 16 || retained.saturating_add(count) > self.max_elements {
                map.clear();
            }
            map.insert(key, t.clone());
        }
        Ok(t)
    }
}

fn build_sine_position(num_pos_features: usize, h: usize, w: usize) -> Result<Tensor> {
    let count = num_pos_features.checked_mul(2).and_then(|n| n.checked_mul(h)).and_then(|n| n.checked_mul(w));
    let count =
        count.filter(|n| *n > 0 && *n <= MAX_POSITION_ELEMENTS).ok_or_else(|| Error::Input("sine position dimensions are empty or exceed 128 MiB".into()))?;
    let eps = 1e-6f32;
    let scale = 2.0 * std::f32::consts::PI;
    let dim_t: Vec<f32> = (0..num_pos_features).map(|i| 10000f32.powf(2.0 * (i / 2) as f32 / num_pos_features as f32)).collect();
    let axis = |n: usize, k: usize| -> Vec<f32> {
        // value of one coordinate at index k of an axis of length n, over the feature dimension
        let e = (k + 1) as f32 / (n as f32 + eps) * scale;
        dim_t.iter().enumerate().map(|(i, d)| if i % 2 == 0 { (e / d).sin() } else { (e / d).cos() }).collect()
    };
    let (ys, xs): (Vec<_>, Vec<_>) = ((0..h).map(|k| axis(h, k)).collect(), (0..w).map(|k| axis(w, k)).collect());
    let mut out = vec![0f32; count];
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
    positions: PositionCache,
}

/// What the neck produces, both in the neck's own order (lowest resolution first, all four
/// levels) as `[B, C, H, W]`.
pub struct NeckOutput {
    pub features: Vec<Tensor>,
    pub positions: Vec<Tensor>,
}

impl Neck {
    /// Position tables shared by the CPU and GPU paths, released with this neck.
    pub(crate) fn position(&self, h: usize, w: usize) -> Result<Tensor> {
        self.positions.get(self.pos_features, h, w)
    }

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
        Ok(Neck { convs, top_down: top_down.to_vec(), pos_features: fpn / 2, positions: PositionCache::new(MAX_POSITION_ELEMENTS) })
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
            positions.push(self.position(cur.dim(2)?, cur.dim(3)?)?);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_tables_reject_empty_overflowing_and_oversized_dimensions() {
        for (features, h, w) in [(0, 1, 1), (1, 0, 1), (1, 1, 0), (usize::MAX, 2, 2), (128, usize::MAX, 2), (128, 1024, 1024)] {
            assert!(matches!(sine_position(features, h, w), Err(Error::Input(_))), "{features}, {h}, {w}");
        }
    }

    #[test]
    fn position_cache_bounds_retained_elements_and_preserves_the_encoding() {
        let neck = Neck { convs: vec![], top_down: vec![], pos_features: 2, positions: PositionCache::new(64) };
        let cache = &neck.positions;
        let first = neck.position(2, 4).unwrap();
        assert_eq!(first.dims(), &[1, 4, 2, 4]);
        assert_eq!(first.flatten_all().unwrap().to_vec1::<f32>().unwrap(), neck.position(2, 4).unwrap().flatten_all().unwrap().to_vec1::<f32>().unwrap());
        neck.position(3, 4).unwrap();
        let entries = cache.entries.lock().unwrap();
        assert_eq!(entries.len(), 1, "older tables are evicted to fit the budget");
        assert!(entries.values().map(Tensor::elem_count).sum::<usize>() <= 64);
        drop(entries);
        let large = neck.position(8, 8).unwrap();
        assert_eq!(large.dims(), &[1, 4, 8, 8]);
        assert_eq!(cache.entries.lock().unwrap().len(), 1, "a table over budget is returned without retention");
    }
}
