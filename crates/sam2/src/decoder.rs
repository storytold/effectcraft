// Modified work: ported from Hugging Face `transformers` `models/sam2/modeling_sam2.py`
// (Sam2Attention, Sam2TwoWayAttentionBlock, Sam2TwoWayTransformer, Sam2MaskDecoder; Apache-2.0,
// Copyright The HuggingFace Inc. team) to Rust on candle. See NOTICE.
//! The two-way mask decoder: output tokens and prompt tokens attend to the image and back, then
//! hyper-networks turn the mask tokens into masks over the up-scaled image embedding.
//! One object per call (batch 1, one point batch).

use candle_core::{D, Tensor};
use candle_nn::{Linear, ops::sigmoid, ops::softmax};

use crate::nn::{LayerNorm, apply, gelu, layer_norm, linear};
use crate::weights::Weights;
use crate::{Error, Result};

const DIM: usize = 256;
const MASK_TOKENS: usize = 4;

struct Attention {
    q: Linear,
    k: Linear,
    v: Linear,
    o: Linear,
    heads: usize,
    scale: f64,
}

impl Attention {
    fn load(w: &Weights, prefix: &str, downsample: usize, heads: usize) -> Result<Attention> {
        let inner = DIM / downsample;
        Ok(Attention {
            q: linear(w, &format!("{prefix}.q_proj"), DIM, inner)?,
            k: linear(w, &format!("{prefix}.k_proj"), DIM, inner)?,
            v: linear(w, &format!("{prefix}.v_proj"), DIM, inner)?,
            o: linear(w, &format!("{prefix}.o_proj"), inner, DIM)?,
            heads,
            scale: ((inner / heads) as f64).powf(-0.5),
        })
    }

    /// `q`: `[1, nq, 256]`, `k` and `v`: `[1, nk, 256]`.
    fn forward(&self, q: &Tensor, k: &Tensor, v: &Tensor) -> Result<Tensor> {
        let split = |t: Tensor| -> Result<Tensor> { Ok(t.reshape((1, (), self.heads, t.dim(2)? / self.heads))?.transpose(1, 2)?.contiguous()?) };
        let (q, k, v) = (split(apply(&self.q, q)?)?, split(apply(&self.k, k)?)?, split(apply(&self.v, v)?)?);
        let att = softmax(&(q.matmul(&k.transpose(2, 3)?.contiguous()?)? * self.scale)?, D::Minus1)?;
        let out = att.matmul(&v)?.transpose(1, 2)?.contiguous()?;
        let (n, h, d) = (out.dim(1)?, out.dim(2)?, out.dim(3)?);
        apply(&self.o, &out.reshape((1, n, h * d))?)
    }
}

/// Feed-forward with ReLU, `Sam2FeedForward`: `proj_in`, `layers`, `proj_out`.
pub struct FeedForward {
    proj_in: Linear,
    layers: Vec<Linear>,
    proj_out: Linear,
    sigmoid_output: bool,
}

impl FeedForward {
    pub fn load(w: &Weights, prefix: &str, input: usize, hidden: usize, output: usize, num_layers: usize, sigmoid_output: bool) -> Result<FeedForward> {
        Ok(FeedForward {
            proj_in: linear(w, &format!("{prefix}.proj_in"), input, hidden)?,
            layers: (0..num_layers.saturating_sub(2)).map(|i| linear(w, &format!("{prefix}.layers.{i}"), hidden, hidden)).collect::<Result<_>>()?,
            proj_out: linear(w, &format!("{prefix}.proj_out"), hidden, output)?,
            sigmoid_output,
        })
    }

    pub fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let mut x = apply(&self.proj_in, x)?.relu()?;
        for l in &self.layers {
            x = apply(l, &x)?.relu()?;
        }
        let x = apply(&self.proj_out, &x)?;
        Ok(if self.sigmoid_output { sigmoid(&x)? } else { x })
    }
}

struct Block {
    self_attn: Attention,
    ln1: LayerNorm,
    token_to_image: Attention,
    ln2: LayerNorm,
    mlp: FeedForward,
    ln3: LayerNorm,
    ln4: LayerNorm,
    image_to_token: Attention,
    skip_first_pe: bool,
}

impl Block {
    fn forward(&self, queries: &Tensor, keys: &Tensor, query_pe: &Tensor, key_pe: &Tensor) -> Result<(Tensor, Tensor)> {
        let mut queries = if self.skip_first_pe {
            self.self_attn.forward(queries, queries, queries)?
        } else {
            let q = (queries + query_pe)?;
            (queries + self.self_attn.forward(&q, &q, queries)?)?
        };
        queries = self.ln1.forward(&queries)?;
        let (q, k) = ((&queries + query_pe)?, (keys + key_pe)?);
        queries = self.ln2.forward(&(&queries + self.token_to_image.forward(&q, &k, keys)?)?)?;
        queries = self.ln3.forward(&(&queries + self.mlp.forward(&queries)?)?)?;
        let (q, k) = ((&queries + query_pe)?, (keys + key_pe)?);
        let keys = self.ln4.forward(&(keys + self.image_to_token.forward(&k, &q, &queries)?)?)?;
        Ok((queries, keys))
    }
}

/// What the decoder returns for one object.
pub struct DecoderOutput {
    /// Mask logits `[1, k, 4 * GRID, 4 * GRID]` (`k` is 3 for multimask output, else 1).
    pub masks: Tensor,
    /// Predicted IoU per mask `[1, k]`.
    pub iou: Tensor,
    /// The output tokens of the chosen masks `[1, k, 256]` (the object pointer is made from these).
    pub sam_tokens: Tensor,
    /// Object-presence logit `[1, 1]` (negative: the object is not in the frame).
    pub object_score: Tensor,
}

pub struct MaskDecoder {
    obj_token: Tensor,
    iou_token: Tensor,
    mask_tokens: Tensor,
    layers: Vec<Block>,
    final_attn: Attention,
    final_ln: LayerNorm,
    up1: (Tensor, Tensor),
    up2: (Tensor, Tensor),
    up_ln: LayerNorm,
    conv_s0: (Tensor, Tensor),
    conv_s1: (Tensor, Tensor),
    hyper: Vec<FeedForward>,
    iou_head: FeedForward,
    obj_head: FeedForward,
}

fn conv_w(w: &Weights, name: &str, i: usize, o: usize, k: usize, transposed: bool) -> Result<(Tensor, Tensor)> {
    let shape = if transposed { [i, o, k, k] } else { [o, i, k, k] };
    Ok((w.get(&format!("{name}.weight"), &shape)?, w.get(&format!("{name}.bias"), &[o])?))
}

fn add_bias(x: Tensor, b: &Tensor) -> Result<Tensor> {
    Ok(x.broadcast_add(&b.reshape((1, b.dim(0)?, 1, 1))?)?)
}

impl MaskDecoder {
    pub fn load(w: &Weights) -> Result<MaskDecoder> {
        let layers = (0..2)
            .map(|i| {
                let p = format!("mask_decoder.transformer.layers.{i}");
                Ok(Block {
                    self_attn: Attention::load(w, &format!("{p}.self_attn"), 1, 8)?,
                    ln1: layer_norm(w, &format!("{p}.layer_norm1"), DIM, 1e-5)?,
                    token_to_image: Attention::load(w, &format!("{p}.cross_attn_token_to_image"), 2, 8)?,
                    ln2: layer_norm(w, &format!("{p}.layer_norm2"), DIM, 1e-5)?,
                    mlp: FeedForward::load(w, &format!("{p}.mlp"), DIM, 2048, DIM, 2, false)?,
                    ln3: layer_norm(w, &format!("{p}.layer_norm3"), DIM, 1e-5)?,
                    ln4: layer_norm(w, &format!("{p}.layer_norm4"), DIM, 1e-5)?,
                    image_to_token: Attention::load(w, &format!("{p}.cross_attn_image_to_token"), 2, 8)?,
                    skip_first_pe: i == 0,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(MaskDecoder {
            obj_token: w.get("mask_decoder.obj_score_token.weight", &[1, DIM])?,
            iou_token: w.get("mask_decoder.iou_token.weight", &[1, DIM])?,
            mask_tokens: w.get("mask_decoder.mask_tokens.weight", &[MASK_TOKENS, DIM])?,
            layers,
            final_attn: Attention::load(w, "mask_decoder.transformer.final_attn_token_to_image", 2, 8)?,
            final_ln: layer_norm(w, "mask_decoder.transformer.layer_norm_final_attn", DIM, 1e-5)?,
            up1: conv_w(w, "mask_decoder.upscale_conv1", DIM, DIM / 4, 2, true)?,
            up2: conv_w(w, "mask_decoder.upscale_conv2", DIM / 4, DIM / 8, 2, true)?,
            up_ln: layer_norm(w, "mask_decoder.upscale_layer_norm", DIM / 4, 1e-6)?,
            conv_s0: conv_w(w, "mask_decoder.conv_s0", DIM, DIM / 8, 1, false)?,
            conv_s1: conv_w(w, "mask_decoder.conv_s1", DIM, DIM / 4, 1, false)?,
            hyper: (0..MASK_TOKENS)
                .map(|i| FeedForward::load(w, &format!("mask_decoder.output_hypernetworks_mlps.{i}"), DIM, DIM, DIM / 8, 3, false))
                .collect::<Result<_>>()?,
            iou_head: FeedForward::load(w, "mask_decoder.iou_prediction_head", DIM, DIM, MASK_TOKENS, 3, true)?,
            obj_head: FeedForward::load(w, "mask_decoder.pred_obj_score_head", DIM, DIM, 1, 3, false)?,
        })
    }

    /// Project the two highest-resolution neck levels to the decoder's skip features
    /// (`conv_s0` on the 4x level, `conv_s1` on the 2x level), `[1, 32, 4g, 4g]` and `[1, 64, 2g, 2g]`.
    pub fn high_res(&self, level0: &Tensor, level1: &Tensor) -> Result<(Tensor, Tensor)> {
        let s0 = add_bias(level0.conv2d(&self.conv_s0.0, 0, 1, 1, 1)?, &self.conv_s0.1)?;
        let s1 = add_bias(level1.conv2d(&self.conv_s1.0, 0, 1, 1, 1)?, &self.conv_s1.1)?;
        Ok((s0, s1))
    }

    /// Predict masks. `image` and `pos`: `[1, 256, g, g]` (the lowest-resolution level with the
    /// no-memory embedding already added when there is no memory), `sparse`: `[1, n, 256]`,
    /// `dense`: `[1, 256, g, g]`, `high`: the skip features from [`MaskDecoder::high_res`].
    pub fn forward(&self, image: &Tensor, pos: &Tensor, sparse: &Tensor, dense: &Tensor, high: &(Tensor, Tensor), multimask: bool) -> Result<DecoderOutput> {
        let (_, c, g, gw) = image.dims4()?;
        if c != DIM || g != gw {
            return Err(Error::Input(format!("image embedding must be 256 x g x g, got {:?}", image.dims())));
        }
        let output_tokens = Tensor::cat(&[&self.obj_token, &self.iou_token, &self.mask_tokens], 0)?.unsqueeze(0)?;
        let tokens = Tensor::cat(&[&output_tokens, sparse], 1)?;
        let image = (image + dense)?;
        let flat = |t: &Tensor| -> Result<Tensor> { Ok(t.flatten_from(2)?.transpose(1, 2)?.contiguous()?) };
        let (mut keys, key_pe) = (flat(&image)?, flat(pos)?);
        let mut queries = tokens.clone();
        for l in &self.layers {
            (queries, keys) = l.forward(&queries, &keys, &tokens, &key_pe)?;
        }
        let (q, k) = ((&queries + &tokens)?, (&keys + &key_pe)?);
        let queries = self.final_ln.forward(&(&queries + self.final_attn.forward(&q, &k, &keys)?)?)?;

        let iou_out = queries.narrow(1, 1, 1)?.squeeze(1)?;
        let mask_tokens_out = queries.narrow(1, 2, MASK_TOKENS)?;
        let obj_out = queries.narrow(1, 0, 1)?.squeeze(1)?;

        let emb = keys.transpose(1, 2)?.contiguous()?.reshape((1, DIM, g, g))?;
        let (s0, s1) = high;
        let up = add_bias(emb.conv_transpose2d(&self.up1.0, 0, 0, 2, 1)?, &self.up1.1)?;
        let up = (up + s1)?;
        let up = self.up_ln.forward(&up.permute((0, 2, 3, 1))?)?.permute((0, 3, 1, 2))?.contiguous()?;
        let up = gelu(&up)?;
        let up = (add_bias(up.conv_transpose2d(&self.up2.0, 0, 0, 2, 1)?, &self.up2.1)? + s0)?;
        let up = gelu(&up)?;

        let hyper: Vec<Tensor> = self.hyper.iter().enumerate().map(|(i, m)| m.forward(&mask_tokens_out.narrow(1, i, 1)?.squeeze(1)?)).collect::<Result<_>>()?;
        let hyper = Tensor::stack(&hyper, 1)?;
        let (_, uc, uh, uw) = up.dims4()?;
        let masks = hyper.matmul(&up.reshape((1, uc, uh * uw))?)?.reshape((1, MASK_TOKENS, uh, uw))?;
        let iou = self.iou_head.forward(&iou_out)?;
        let object_score = self.obj_head.forward(&obj_out)?;

        let (masks, iou, sam_tokens) = if multimask {
            (masks.narrow(1, 1, 3)?, iou.narrow(1, 1, 3)?, mask_tokens_out.narrow(1, 1, 3)?)
        } else {
            let (m, i) = dynamic_single(&masks, &iou)?;
            (m, i, mask_tokens_out.narrow(1, 0, 1)?)
        };
        Ok(DecoderOutput { masks, iou, sam_tokens, object_score })
    }
}

/// Single-mask output with the stability fallback (`_dynamic_multimask_via_stability`): if the
/// first mask token's mask is unstable, use the best of the other three.
fn dynamic_single(masks: &Tensor, iou: &Tensor) -> Result<(Tensor, Tensor)> {
    const DELTA: f32 = 0.05;
    const THRESH: f32 = 0.98;
    let single = masks.narrow(1, 0, 1)?;
    let single_iou = iou.narrow(1, 0, 1)?;
    let logits = single.flatten_all()?.to_vec1::<f32>()?;
    let area_i = logits.iter().filter(|v| **v > DELTA).count() as f32;
    let area_u = logits.iter().filter(|v| **v > -DELTA).count() as f32;
    let stability = if area_u > 0.0 { area_i / area_u } else { 1.0 };
    if stability >= THRESH {
        return Ok((single, single_iou));
    }
    let multi_iou = iou.narrow(1, 1, 3)?.flatten_all()?.to_vec1::<f32>()?;
    let best = multi_iou.iter().enumerate().fold((0usize, f32::NEG_INFINITY), |b, (i, v)| if *v > b.1 { (i, *v) } else { b }).0;
    Ok((masks.narrow(1, 1 + best, 1)?, iou.narrow(1, 1 + best, 1)?))
}
