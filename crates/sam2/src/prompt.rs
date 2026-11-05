// Modified work: ported from Hugging Face `transformers` `models/sam2/modeling_sam2.py`
// (Sam2PositionalEmbedding, Sam2MaskEmbedding, Sam2PromptEncoder; Apache-2.0, Copyright The
// HuggingFace Inc. team) to Rust on candle. See NOTICE.
//! The prompt encoder: clicks and boxes become sparse tokens, a mask or "no mask" becomes a dense map.

use candle_core::{Device, Tensor};

use crate::nn::{LayerNorm, gelu, layer_norm};
use crate::weights::Weights;
use crate::{Error, Result};

/// Side of the (square) model input in pixels; prompts are given in these coordinates.
pub const INPUT_SIZE: usize = 1024;
/// Side of the image-embedding grid (input / 16).
pub const GRID: usize = INPUT_SIZE / 16;
const DIM: usize = 256;

/// What the user pointed at, in model-input pixels (0..1024).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prompt {
    /// Clicks and whether each is foreground.
    pub points: Vec<([f32; 2], bool)>,
    /// A box `[x0, y0, x1, y1]`.
    pub bbox: Option<[f32; 4]>,
}

impl Prompt {
    /// Reject non-finite or absurd coordinates (they come from the user or an agent).
    pub fn validate(&self) -> Result<()> {
        let bad = |v: f32| !v.is_finite() || v.abs() > 4.0 * INPUT_SIZE as f32;
        if self.points.iter().any(|(p, _)| p.iter().any(|v| bad(*v))) || self.bbox.is_some_and(|b| b.iter().any(|v| bad(*v))) {
            return Err(Error::Input("prompt coordinates must be finite and near the frame".into()));
        }
        if self.points.len() > 64 {
            return Err(Error::Input("at most 64 points per prompt".into()));
        }
        Ok(())
    }
}

struct MaskEmbed {
    conv: [(Tensor, Tensor, usize, usize); 3],
    ln1: LayerNorm,
    ln2: LayerNorm,
}

/// Sparse and dense prompt embeddings.
pub struct PromptEncoder {
    gauss: Vec<f32>,
    point_embed: Vec<f32>,
    not_a_point: Vec<f32>,
    no_mask: Tensor,
    mask: MaskEmbed,
}

fn rows(t: Tensor) -> Result<Vec<f32>> {
    Ok(t.flatten_all()?.to_vec1::<f32>()?)
}

impl PromptEncoder {
    pub fn load(w: &Weights) -> Result<PromptEncoder> {
        let conv = |name: &str, i: usize, o: usize, k: usize, stride: usize| -> Result<(Tensor, Tensor, usize, usize)> {
            Ok((
                w.get(&format!("prompt_encoder.mask_embed.{name}.weight"), &[o, i, k, k])?,
                w.get(&format!("prompt_encoder.mask_embed.{name}.bias"), &[o])?,
                k,
                stride,
            ))
        };
        Ok(PromptEncoder {
            gauss: rows(w.get("prompt_encoder.shared_embedding.positional_embedding", &[2, DIM / 2])?)?,
            point_embed: rows(w.get("prompt_encoder.point_embed.weight", &[4, DIM])?)?,
            not_a_point: rows(w.get("prompt_encoder.not_a_point_embed.weight", &[1, DIM])?)?,
            no_mask: w.get("prompt_encoder.no_mask_embed.weight", &[1, DIM])?,
            mask: MaskEmbed {
                conv: [conv("conv1", 1, 4, 2, 2)?, conv("conv2", 4, 16, 2, 2)?, conv("conv3", 16, DIM, 1, 1)?],
                ln1: layer_norm(w, "prompt_encoder.mask_embed.layer_norm1", 4, 1e-6)?,
                ln2: layer_norm(w, "prompt_encoder.mask_embed.layer_norm2", 16, 1e-6)?,
            },
        })
    }

    /// Random-Fourier position encoding of coordinates already normalised to 0..1: `[n * 256]`.
    fn fourier(&self, coords01: &[[f32; 2]]) -> Vec<f32> {
        let half = DIM / 2;
        let mut out = vec![0f32; coords01.len() * DIM];
        for (n, c) in coords01.iter().enumerate() {
            let (x, y) = (2.0 * c[0] - 1.0, 2.0 * c[1] - 1.0);
            for j in 0..half {
                let g0 = self.gauss.get(j).copied().unwrap_or(0.0);
                let g1 = self.gauss.get(half + j).copied().unwrap_or(0.0);
                let v = 2.0 * std::f32::consts::PI * (x * g0 + y * g1);
                if let Some(o) = out.get_mut(n * DIM + j) {
                    *o = v.sin();
                }
                if let Some(o) = out.get_mut(n * DIM + half + j) {
                    *o = v.cos();
                }
            }
        }
        out
    }

    /// The position encoding of the whole `GRID x GRID` embedding, `[1, 256, GRID, GRID]`.
    pub fn image_positions(&self) -> Result<Tensor> {
        let coords: Vec<[f32; 2]> = (0..GRID * GRID).map(|i| [((i % GRID) as f32 + 0.5) / GRID as f32, ((i / GRID) as f32 + 0.5) / GRID as f32]).collect();
        let flat = self.fourier(&coords);
        Ok(Tensor::from_vec(flat, (GRID * GRID, DIM), &Device::Cpu)?.t()?.reshape((1, DIM, GRID, GRID))?.contiguous()?)
    }

    fn point_row(&self, i: usize) -> &[f32] {
        self.point_embed.get(i * DIM..(i + 1) * DIM).unwrap_or(&[])
    }

    /// Sparse tokens `[1, n, 256]`; the pinned reference pads both click-only and box prompts.
    pub fn sparse(&self, p: &Prompt) -> Result<Tensor> {
        p.validate()?;
        let norm = |c: [f32; 2]| [(c[0] + 0.5) / INPUT_SIZE as f32, (c[1] + 0.5) / INPUT_SIZE as f32];
        let mut tokens: Vec<f32> = vec![];
        let mut n = 0;
        let mut push = |v: &[f32]| {
            tokens.extend_from_slice(v);
            n += 1;
        };
        let pts: Vec<[f32; 2]> = p.points.iter().map(|(c, _)| norm(*c)).collect();
        let enc = self.fourier(&pts);
        for (k, (_, fg)) in p.points.iter().enumerate() {
            let e = enc.get(k * DIM..(k + 1) * DIM).unwrap_or(&[]);
            let label_row = self.point_row(usize::from(*fg));
            let v: Vec<f32> = e.iter().zip(label_row).map(|(a, b)| a + b).collect();
            push(&v);
        }
        match p.bbox {
            None => push(&self.not_a_point.clone()),
            Some(b) => {
                let corners = [norm([b[0], b[1]]), norm([b[2], b[3]])];
                let enc = self.fourier(&corners);
                for (k, row) in [2usize, 3].into_iter().enumerate() {
                    let e = enc.get(k * DIM..(k + 1) * DIM).unwrap_or(&[]);
                    let v: Vec<f32> = e.iter().zip(self.point_row(row)).map(|(a, b)| a + b).collect();
                    push(&v);
                }
                // _embed_boxes adds its own no-point token, separately from _embed_points.
                push(&self.not_a_point.clone());
            }
        }
        Ok(Tensor::from_vec(tokens, (1, n, DIM), &Device::Cpu)?)
    }

    /// Tokens for "no prompt" on a tracked frame: the reference feeds one point labelled -1 and
    /// then pads another, so two `not_a_point` tokens, `[1, 2, 256]`.
    pub fn sparse_none(&self) -> Result<Tensor> {
        let mut v = self.not_a_point.clone();
        v.extend_from_slice(&self.not_a_point);
        Ok(Tensor::from_vec(v, (1, 2, DIM), &Device::Cpu)?)
    }

    /// The dense embedding for "no mask prompt", `[1, 256, GRID, GRID]`.
    pub fn dense_empty(&self) -> Result<Tensor> {
        Ok(self.no_mask.reshape((1, DIM, 1, 1))?.broadcast_as((1, DIM, GRID, GRID))?.contiguous()?)
    }

    /// The dense embedding of a mask prompt `[1, 1, 4 * GRID, 4 * GRID]` (low-resolution logits).
    pub fn dense_mask(&self, mask: &Tensor) -> Result<Tensor> {
        let (c1, c2, c3) = (&self.mask.conv[0], &self.mask.conv[1], &self.mask.conv[2]);
        let step = |x: &Tensor, c: &(Tensor, Tensor, usize, usize)| -> Result<Tensor> {
            Ok(x.conv2d(&c.0, 0, c.3, 1, 1)?.broadcast_add(&c.1.reshape((1, c.1.dim(0)?, 1, 1))?)?)
        };
        let cl = |x: &Tensor, ln: &LayerNorm| -> Result<Tensor> { Ok(ln.forward(&x.permute((0, 2, 3, 1))?)?.permute((0, 3, 1, 2))?.contiguous()?) };
        let x = gelu(&cl(&step(mask, c1)?, &self.mask.ln1)?)?;
        let x = gelu(&cl(&step(&x, c2)?, &self.mask.ln2)?)?;
        step(&x, c3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny deterministic checkpoint exercises token labels without downloading trained weights.
    fn encoder() -> PromptEncoder {
        let mut map = std::collections::HashMap::new();
        for (name, shape) in [
            ("shared_embedding.positional_embedding", vec![2, DIM / 2]),
            ("no_mask_embed.weight", vec![1, DIM]),
            ("mask_embed.conv1.weight", vec![4, 1, 2, 2]),
            ("mask_embed.conv1.bias", vec![4]),
            ("mask_embed.conv2.weight", vec![16, 4, 2, 2]),
            ("mask_embed.conv2.bias", vec![16]),
            ("mask_embed.conv3.weight", vec![DIM, 16, 1, 1]),
            ("mask_embed.conv3.bias", vec![DIM]),
            ("mask_embed.layer_norm1.weight", vec![4]),
            ("mask_embed.layer_norm1.bias", vec![4]),
            ("mask_embed.layer_norm2.weight", vec![16]),
            ("mask_embed.layer_norm2.bias", vec![16]),
        ] {
            map.insert(format!("prompt_encoder.{name}"), Tensor::zeros(shape, candle_core::DType::F32, &Device::Cpu).unwrap());
        }
        let rows: Vec<f32> = (1..=4).flat_map(|row| vec![row as f32 * 10.0; DIM]).collect();
        map.insert("prompt_encoder.point_embed.weight".into(), Tensor::from_vec(rows, (4, DIM), &Device::Cpu).unwrap());
        map.insert("prompt_encoder.not_a_point_embed.weight".into(), Tensor::from_vec(vec![-1000.0f32; DIM], (1, DIM), &Device::Cpu).unwrap());
        PromptEncoder::load(&Weights::from_map(map).unwrap()).unwrap()
    }

    /// Transformers 5.19.0 pads click-only prompts in _embed_points and boxes in _embed_boxes.
    #[test]
    fn box_and_click_tokens_match_the_pinned_reference_padding() {
        let encoder = encoder();
        let points = vec![([10.0, 20.0], false), ([30.0, 40.0], true)];
        for points in [vec![], points.clone()] {
            let sparse = encoder.sparse(&Prompt { points: points.clone(), bbox: Some([0.0, 0.0, 100.0, 100.0]) }).unwrap();
            assert_eq!(sparse.dims(), &[1, points.len() + 3, DIM]);
            let rows = sparse.to_vec3::<f32>().unwrap();
            assert!(rows[0].last().unwrap().iter().all(|value| *value == -1000.0));
            for (token, label) in rows[0].iter().zip(if points.is_empty() { vec![30.0, 40.0] } else { vec![10.0, 20.0, 30.0, 40.0] }) {
                assert!(token[..DIM / 2].iter().all(|value| *value == label));
                assert!(token[DIM / 2..].iter().all(|value| *value == label + 1.0));
            }
        }
        let clicks = encoder.sparse(&Prompt { points, bbox: None }).unwrap();
        assert_eq!(clicks.dims(), &[1, 3, DIM]);
        assert!(clicks.to_vec3::<f32>().unwrap()[0][2].iter().all(|value| *value == -1000.0));
        let none = encoder.sparse_none().unwrap();
        assert_eq!(none.dims(), &[1, 2, DIM]);
        assert!(none.flatten_all().unwrap().to_vec1::<f32>().unwrap().iter().all(|value| *value == -1000.0));
    }

    #[test]
    fn hostile_prompts_are_rejected() {
        let ok = Prompt { points: vec![([10.0, 20.0], true)], bbox: None };
        assert!(ok.validate().is_ok());
        assert!(Prompt { points: vec![([f32::NAN, 0.0], true)], bbox: None }.validate().is_err());
        assert!(Prompt { points: vec![], bbox: Some([0.0, 0.0, f32::INFINITY, 5.0]) }.validate().is_err());
        assert!(Prompt { points: vec![([1e9, 0.0], false)], bbox: None }.validate().is_err());
        assert!(Prompt { points: vec![([1.0, 1.0], true); 65], bbox: None }.validate().is_err());
    }
}
