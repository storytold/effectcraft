// Modified work: ported from Hugging Face `transformers` `models/sam2/modeling_sam2.py`
// (Sam2PatchEmbeddings, Sam2MultiScaleAttention, Sam2MultiScaleBlock, Sam2HieraDetModel;
// Apache-2.0, Copyright The HuggingFace Inc. team) to Rust on candle. See NOTICE.
//! The Hiera backbone: patch embedding, windowed positional embedding, and blocks with window or
//! global attention and query pooling at the stage changes. Activations are NHWC like the reference.

use candle_core::Tensor;
use candle_nn::{Linear, ops::softmax_last_dim};

use crate::nn::{LayerNorm, Mlp, apply, layer_norm, linear};
use crate::weights::Weights;
use crate::{Error, Result};

/// Hiera hyper-parameters (the released SAM 2.1 sizes differ only here).
#[derive(Clone, Debug, PartialEq)]
pub struct HieraConfig {
    pub embed_dims: Vec<usize>,
    pub blocks: Vec<usize>,
    pub heads: Vec<usize>,
    pub windows: Vec<usize>,
    pub global_blocks: Vec<usize>,
    pub query_stride: usize,
    pub query_pool_stages: usize,
    pub mlp_ratio: usize,
    pub eps: f64,
    pub pos_background: usize,
}

impl HieraConfig {
    /// `facebook/sam2.1-hiera-tiny`.
    pub fn tiny() -> HieraConfig {
        HieraConfig {
            embed_dims: vec![96, 192, 384, 768],
            blocks: vec![1, 2, 7, 2],
            heads: vec![1, 2, 4, 8],
            windows: vec![8, 4, 14, 7],
            global_blocks: vec![5, 7, 9],
            query_stride: 2,
            query_pool_stages: 3,
            mlp_ratio: 4,
            eps: 1e-6,
            pos_background: 7,
        }
    }

    /// `facebook/sam2.1-hiera-small`: the same widths as Tiny with deeper third and fourth stages.
    pub fn small() -> HieraConfig {
        HieraConfig { blocks: vec![1, 2, 11, 2], global_blocks: vec![7, 10, 13], ..HieraConfig::tiny() }
    }

    /// `facebook/sam2.1-hiera-base-plus`.
    pub fn base_plus() -> HieraConfig {
        HieraConfig {
            embed_dims: vec![112, 224, 448, 896],
            blocks: vec![2, 3, 16, 3],
            heads: vec![2, 4, 8, 16],
            global_blocks: vec![12, 16, 20],
            pos_background: 14,
            ..HieraConfig::tiny()
        }
    }

    /// `facebook/sam2.1-hiera-large`.
    pub fn large() -> HieraConfig {
        HieraConfig {
            embed_dims: vec![144, 288, 576, 1152],
            blocks: vec![2, 6, 36, 4],
            heads: vec![2, 4, 8, 16],
            windows: vec![8, 4, 16, 8],
            global_blocks: vec![23, 33, 43],
            ..HieraConfig::tiny()
        }
    }

    /// The configuration of a released size.
    pub fn for_size(size: crate::Size) -> HieraConfig {
        match size {
            crate::Size::Tiny => HieraConfig::tiny(),
            crate::Size::Small => HieraConfig::small(),
            crate::Size::BasePlus => HieraConfig::base_plus(),
            crate::Size::Large => HieraConfig::large(),
        }
    }
}

fn at<T: Copy>(v: &[T], i: usize, what: &str) -> Result<T> {
    v.get(i).copied().ok_or_else(|| Error::Weights(format!("Hiera config has no {what} for stage {i}")))
}

/// Max pool a `[B, H, W, C]` tensor by `stride` (kernel = stride, no padding, floor), without the
/// permutes a pooling layer would need: it groups the cells and takes the maximum over each group.
fn do_pool(x: &Tensor, stride: usize) -> Result<Tensor> {
    let (b, h, w, c) = x.dims4()?;
    if stride == 0 {
        return Err(Error::Input("pool stride 0".into()));
    }
    let (oh, ow) = (h / stride, w / stride);
    let x = if oh * stride != h || ow * stride != w { x.narrow(1, 0, oh * stride)?.narrow(2, 0, ow * stride)? } else { x.clone() };
    Ok(x.contiguous()?.reshape((b, oh, stride, ow, stride, c))?.max(2)?.max(3)?.contiguous()?)
}

/// Split `[B, H, W, C]` into `[B * nH * nW, ws, ws, C]` windows, zero padding the bottom/right.
fn window_partition(x: &Tensor, ws: usize) -> Result<(Tensor, (usize, usize))> {
    let (b, h, w, c) = x.dims4()?;
    if ws == 0 {
        return Err(Error::Input("window size 0".into()));
    }
    let (ph, pw) = ((ws - h % ws) % ws, (ws - w % ws) % ws);
    let x = x.pad_with_zeros(2, 0, pw)?.pad_with_zeros(1, 0, ph)?;
    let (nh, nw) = ((h + ph) / ws, (w + pw) / ws);
    let x = x.reshape((b, nh, ws, nw, ws, c))?.permute((0, 1, 3, 2, 4, 5))?.contiguous()?.reshape((b * nh * nw, ws, ws, c))?;
    Ok((x, (h + ph, w + pw)))
}

fn window_unpartition(win: &Tensor, ws: usize, padded: (usize, usize), orig: (usize, usize)) -> Result<Tensor> {
    if ws == 0 {
        return Err(Error::Input("window size 0".into()));
    }
    let (nh, nw) = (padded.0 / ws, padded.1 / ws);
    let (n, _, _, c) = win.dims4()?;
    let b = n / (nh * nw).max(1);
    let x = win.reshape((b, nh, nw, ws, ws, c))?.permute((0, 1, 3, 2, 4, 5))?.contiguous()?.reshape((b, padded.0, padded.1, c))?;
    Ok(x.narrow(1, 0, orig.0)?.narrow(2, 0, orig.1)?.contiguous()?)
}

struct Attention {
    qkv: Linear,
    proj: Linear,
    heads: usize,
    scale: f64,
    query_stride: Option<usize>,
}

impl Attention {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let (b, h, w, _) = x.dims4()?;
        let qkv = apply(&self.qkv, x)?.reshape((b, h * w, 3, self.heads, ()))?;
        let part = |i: usize| -> Result<Tensor> { Ok(qkv.narrow(2, i, 1)?.squeeze(2)?) };
        let (mut q, k, v) = (part(0)?, part(1)?, part(2)?);
        let (mut oh, mut ow) = (h, w);
        if let Some(s) = self.query_stride {
            let pooled = do_pool(&q.reshape((b, h, w, ()))?, s)?;
            (oh, ow) = (pooled.dim(1)?, pooled.dim(2)?);
            q = pooled.reshape((b, oh * ow, self.heads, ()))?;
        }
        let q = q.transpose(1, 2)?.contiguous()?;
        let k = k.transpose(1, 2)?.contiguous()?;
        let v = v.transpose(1, 2)?.contiguous()?;
        // Scaling the (small) queries first equals scaling the scores, and the fused softmax is
        // much faster than the generic one on the 4096 x 4096 global-attention maps.
        let att = (q * self.scale)?.matmul(&k.transpose(2, 3)?.contiguous()?)?;
        let att = softmax_last_dim(&att)?;
        let out = att.matmul(&v)?.transpose(1, 2)?.contiguous()?.reshape((b, oh, ow, ()))?;
        apply(&self.proj, &out)
    }
}

/// What a block is made of: its widths, heads, window (0: global attention) and query pooling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BlockPlan {
    pub dim: usize,
    pub dim_out: usize,
    pub heads: usize,
    pub window: usize,
    pub query_stride: Option<usize>,
}

impl BlockPlan {
    pub(crate) fn new(cfg: &HieraConfig, stage: usize, block_in_stage: usize, total: usize) -> Result<BlockPlan> {
        let first = stage > 0 && block_in_stage == 0;
        let dim = at(&cfg.embed_dims, if first { stage - 1 } else { stage }, "embed dim")?;
        let dim_out = at(&cfg.embed_dims, stage, "embed dim")?;
        let window = if first { at(&cfg.windows, stage - 1, "window")? } else { at(&cfg.windows, stage, "window")? };
        let window = if cfg.global_blocks.contains(&total) { 0 } else { window };
        let query_stride = (stage > 0 && stage <= cfg.query_pool_stages && block_in_stage == 0).then_some(cfg.query_stride);
        Ok(BlockPlan { dim, dim_out, heads: at(&cfg.heads, stage, "heads")?, window, query_stride })
    }

    /// Every block of the backbone, in order.
    pub(crate) fn all(cfg: &HieraConfig) -> Result<Vec<BlockPlan>> {
        let mut out = vec![];
        for (stage, n) in cfg.blocks.iter().enumerate() {
            for b in 0..*n {
                out.push(BlockPlan::new(cfg, stage, b, out.len())?);
            }
        }
        Ok(out)
    }
}

/// One Hiera block: pre-norm attention (window or global, optionally pooling the queries) and MLP.
pub struct Block {
    norm1: LayerNorm,
    attn: Attention,
    norm2: LayerNorm,
    mlp: Mlp,
    proj: Option<Linear>,
    window: usize,
    query_stride: Option<usize>,
}

impl Block {
    fn load(w: &Weights, cfg: &HieraConfig, stage: usize, block_in_stage: usize, total: usize) -> Result<Block> {
        let prefix = format!("vision_encoder.backbone.blocks.{total}");
        let plan = BlockPlan::new(cfg, stage, block_in_stage, total)?;
        let (dim, dim_out, heads) = (plan.dim, plan.dim_out, plan.heads);
        Ok(Block {
            norm1: layer_norm(w, &format!("{prefix}.layer_norm1"), dim, cfg.eps)?,
            attn: Attention {
                qkv: linear(w, &format!("{prefix}.attn.qkv"), dim, dim_out * 3)?,
                proj: linear(w, &format!("{prefix}.attn.proj"), dim_out, dim_out)?,
                heads,
                scale: ((dim_out / heads.max(1)) as f64).powf(-0.5),
                query_stride: plan.query_stride,
            },
            norm2: layer_norm(w, &format!("{prefix}.layer_norm2"), dim_out, cfg.eps)?,
            mlp: Mlp::load(w, &format!("{prefix}.mlp"), dim_out, dim_out * cfg.mlp_ratio)?,
            proj: (dim != dim_out).then(|| linear(w, &format!("{prefix}.proj"), dim, dim_out)).transpose()?,
            window: plan.window,
            query_stride: plan.query_stride,
        })
    }

    pub fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let h = self.norm1.forward(x)?;
        let residual = match (&self.proj, self.query_stride) {
            (Some(p), Some(s)) => do_pool(&apply(p, &h)?, s)?,
            (Some(p), None) => apply(p, &h)?,
            (None, _) => x.clone(),
        };
        let (hh, ww) = (h.dim(1)?, h.dim(2)?);
        let mut ws = self.window;
        let mut padded = (hh, ww);
        let h = if ws > 0 {
            let (win, pad) = window_partition(&h, ws)?;
            padded = pad;
            win
        } else {
            h
        };
        let mut h = self.attn.forward(&h)?;
        let mut orig = (hh, ww);
        if let Some(s) = self.query_stride {
            ws /= s.max(1);
            orig = (residual.dim(1)?, residual.dim(2)?);
            if ws > 0 {
                padded = (orig.0 + (ws - orig.0 % ws) % ws, orig.1 + (ws - orig.1 % ws) % ws);
            }
        }
        if ws > 0 {
            h = window_unpartition(&h, ws, padded, orig)?;
        }
        let h = (residual + h)?;
        let m = self.mlp.forward(&self.norm2.forward(&h)?)?;
        Ok((h + m)?)
    }
}

/// PyTorch `F.interpolate(mode="bicubic", align_corners=False)` of `[C, in_h, in_w]` to `[C, out_h, out_w]`.
fn bicubic_resize(src: &[f32], c: usize, in_hw: (usize, usize), out_hw: (usize, usize)) -> Vec<f32> {
    const A: f32 = -0.75;
    let near = |x: f32| ((A + 2.0) * x - (A + 3.0)) * x * x + 1.0;
    let far = |x: f32| ((A * x - 5.0 * A) * x + 8.0 * A) * x - 4.0 * A;
    let axis = |n_in: usize, n_out: usize| -> Vec<([usize; 4], [f32; 4])> {
        let scale = n_in as f32 / n_out.max(1) as f32;
        (0..n_out)
            .map(|o| {
                let s = (o as f32 + 0.5) * scale - 0.5;
                let i0 = s.floor();
                let t = s - i0;
                let w = [far(t + 1.0), near(t), near(1.0 - t), far(2.0 - t)];
                let last = n_in as i64 - 1;
                let idx = [-1i64, 0, 1, 2].map(|d| (i0 as i64 + d).clamp(0, last.max(0)) as usize);
                (idx, w)
            })
            .collect()
    };
    let (ys, xs) = (axis(in_hw.0, out_hw.0), axis(in_hw.1, out_hw.1));
    let mut out = vec![0f32; c * out_hw.0 * out_hw.1];
    for ch in 0..c {
        let plane = src.get(ch * in_hw.0 * in_hw.1..(ch + 1) * in_hw.0 * in_hw.1).unwrap_or(&[]);
        for (oy, (yi, yw)) in ys.iter().enumerate() {
            for (ox, (xi, xw)) in xs.iter().enumerate() {
                let mut acc = 0f32;
                for (a, wy) in yi.iter().zip(yw) {
                    for (b, wx) in xi.iter().zip(xw) {
                        acc += plane.get(a * in_hw.1 + b).copied().unwrap_or(0.0) * wy * wx;
                    }
                }
                if let Some(o) = out.get_mut((ch * out_hw.0 + oy) * out_hw.1 + ox) {
                    *o = acc;
                }
            }
        }
    }
    out
}

pub struct Hiera {
    cfg: HieraConfig,
    patch_w: Tensor,
    patch_b: Tensor,
    pos_embed: Vec<f32>,
    pos_embed_window: Vec<f32>,
    window0: usize,
    hidden: usize,
    pub blocks: Vec<Block>,
    stage_ends: Vec<usize>,
}

impl Hiera {
    pub fn load(w: &Weights, cfg: &HieraConfig) -> Result<Hiera> {
        let hidden = at(&cfg.embed_dims, 0, "embed dim")?;
        let window0 = at(&cfg.windows, 0, "window")?;
        let bg = cfg.pos_background;
        let mut blocks = vec![];
        let mut stage_ends = vec![];
        for (stage, n) in cfg.blocks.iter().enumerate() {
            for b in 0..*n {
                blocks.push(Block::load(w, cfg, stage, b, blocks.len())?);
            }
            stage_ends.push(blocks.len().saturating_sub(1));
        }
        let flat = |t: Tensor| -> Result<Vec<f32>> { Ok(t.flatten_all()?.to_vec1::<f32>()?) };
        Ok(Hiera {
            cfg: cfg.clone(),
            patch_w: w.get("vision_encoder.backbone.patch_embed.projection.weight", &[hidden, 3, 7, 7])?,
            patch_b: w.get("vision_encoder.backbone.patch_embed.projection.bias", &[hidden])?,
            pos_embed: flat(w.get("vision_encoder.backbone.pos_embed", &[1, hidden, bg, bg])?)?,
            pos_embed_window: flat(w.get("vision_encoder.backbone.pos_embed_window", &[1, hidden, window0, window0])?)?,
            window0,
            hidden,
            blocks,
            stage_ends,
        })
    }

    /// Patch embedding of `[B, 3, H, W]` pixels: `[B, H/4, W/4, C]` (before the positional embedding).
    pub(crate) fn config(&self) -> &HieraConfig {
        &self.cfg
    }

    pub fn patch_embed(&self, pixels: &Tensor) -> Result<Tensor> {
        let x = pixels.conv2d(&self.patch_w, 3, 4, 1, 1)?;
        let x = x.broadcast_add(&self.patch_b.reshape((1, self.hidden, 1, 1))?)?;
        Ok(x.permute((0, 2, 3, 1))?.contiguous()?)
    }

    /// Background embedding resized to the grid plus the tiled window embedding, `[1, H, W, C]`.
    pub(crate) fn pos(&self, h: usize, w: usize) -> Result<Tensor> {
        let bg = self.cfg.pos_background;
        let big = bicubic_resize(&self.pos_embed, self.hidden, (bg, bg), (h, w));
        let ws = self.window0.max(1);
        let mut out = vec![0f32; h * w * self.hidden];
        for c in 0..self.hidden {
            for y in 0..h {
                for x in 0..w {
                    let tile = self.pos_embed_window.get((c * ws + y % ws) * ws + x % ws).copied().unwrap_or(0.0);
                    let base = big.get((c * h + y) * w + x).copied().unwrap_or(0.0);
                    if let Some(o) = out.get_mut((y * w + x) * self.hidden + c) {
                        *o = base + tile;
                    }
                }
            }
        }
        Ok(Tensor::from_vec(out, (1, h, w, self.hidden), &candle_core::Device::Cpu)?)
    }

    /// Run all blocks. Returns the output of the last block of each stage (NHWC).
    pub fn forward(&self, pixels: &Tensor) -> Result<Vec<Tensor>> {
        let (_, _, ph, pw) = pixels.dims4()?;
        if ph == 0 || pw == 0 || ph % 4 != 0 || pw % 4 != 0 {
            return Err(Error::Input(format!("image size {pw}x{ph} must be a positive multiple of 4")));
        }
        let mut x = self.patch_embed(pixels)?;
        let (h, w) = (x.dim(1)?, x.dim(2)?);
        if h % self.window0 != 0 || w % self.window0 != 0 {
            return Err(Error::Input(format!("patch grid {w}x{h} must be a multiple of {}", self.window0)));
        }
        x = x.broadcast_add(&self.pos(h, w)?)?;
        let mut stages = vec![];
        for (i, b) in self.blocks.iter().enumerate() {
            x = b.forward(&x)?;
            if self.stage_ends.contains(&i) {
                stages.push(x.clone());
            }
        }
        Ok(stages)
    }

    /// Run the blocks, returning every block's output (for the stage-by-stage reference test).
    pub fn forward_all(&self, pixels: &Tensor) -> Result<(Tensor, Vec<Tensor>)> {
        let emb = self.patch_embed(pixels)?;
        let (h, w) = (emb.dim(1)?, emb.dim(2)?);
        let mut x = emb.broadcast_add(&self.pos(h, w)?)?;
        let mut outs = vec![];
        for b in &self.blocks {
            x = b.forward(&x)?;
            outs.push(x.clone());
        }
        Ok((emb, outs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::Device;

    #[test]
    fn bicubic_of_a_constant_is_that_constant() {
        let out = bicubic_resize(&[2.5; 49], 1, (7, 7), (32, 32));
        assert!(out.iter().all(|v| (v - 2.5).abs() < 1e-5));
    }

    #[test]
    fn bicubic_identity_size_is_identity() {
        let src: Vec<f32> = (0..49).map(|i| (i as f32 * 0.37).sin()).collect();
        let out = bicubic_resize(&src, 1, (7, 7), (7, 7));
        assert!(src.iter().zip(&out).all(|(a, b)| (a - b).abs() < 1e-5));
    }

    #[test]
    fn windows_round_trip_with_padding() {
        let x = Tensor::arange(0f32, 60.0, &Device::Cpu).unwrap().reshape((1, 5, 6, 2)).unwrap();
        let (win, pad) = window_partition(&x, 4).unwrap();
        assert_eq!((win.dims(), pad), (&[4, 4, 4, 2][..], (8, 8)));
        let back = window_unpartition(&win, 4, pad, (5, 6)).unwrap();
        let d = (back - x).unwrap().abs().unwrap().sum_all().unwrap().to_scalar::<f32>().unwrap();
        assert_eq!(d, 0.0);
    }

    #[test]
    fn pooling_equals_a_max_pool_layer_including_odd_sizes() {
        let x = Tensor::randn(0f32, 1.0, (2, 7, 9, 5), &Device::Cpu).unwrap();
        let want = x.permute((0, 3, 1, 2)).unwrap().contiguous().unwrap().max_pool2d_with_stride(2, 2).unwrap().permute((0, 2, 3, 1)).unwrap();
        let got = do_pool(&x, 2).unwrap();
        assert_eq!(got.dims(), want.dims());
        let d = (got - want).unwrap().abs().unwrap().sum_all().unwrap().to_scalar::<f32>().unwrap();
        assert_eq!(d, 0.0);
        assert!(do_pool(&x, 0).is_err());
    }

    #[test]
    fn zero_window_is_an_error_not_a_panic() {
        let x = Tensor::zeros((1, 4, 4, 2), candle_core::DType::F32, &Device::Cpu).unwrap();
        assert!(window_partition(&x, 0).is_err());
    }
}
