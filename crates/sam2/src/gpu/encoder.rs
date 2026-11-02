// Original work for EffectCraft; the maths follows the Hugging Face `transformers` SAM 2
// implementation (Apache-2.0, see NOTICE).
//! The vision encoder on the GPU: patch embedding, the Hiera blocks and the neck, with the decoder's two
//! skip convolutions. Activations stay on the GPU between the kernels; only the three tensors the rest of
//! the model needs come back.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use candle_core::{Device, Tensor};

use super::{Act, Buf, GLinear, GNorm, Gpu, Matmul, Strides};
use crate::hiera::{BlockPlan, Hiera, HieraConfig};
use crate::weights::Weights;
use crate::{Error, Result};

struct GBlock {
    plan: BlockPlan,
    norm1: GNorm,
    qkv: GLinear,
    proj: GLinear,
    norm2: GNorm,
    mlp_in: GLinear,
    mlp_out: GLinear,
    block_proj: Option<GLinear>,
}

/// What the rest of the model takes from the encoder (CPU tensors, NCHW).
pub struct GpuFeatures {
    /// The lowest-resolution neck level `[1, 256, 64, 64]`.
    pub low: Tensor,
    /// The decoder's skip features: `conv_s0` of the highest level `[1, 32, 256, 256]` and `conv_s1` of the
    /// next `[1, 64, 128, 128]`.
    pub high: (Tensor, Tensor),
}

pub struct GpuEncoder {
    gpu: Arc<Gpu>,
    cfg: HieraConfig,
    patch_w: Buf,
    patch_b: Buf,
    blocks: Vec<GBlock>,
    stage_ends: Vec<usize>,
    neck: Vec<GLinear>,
    top_down: Vec<usize>,
    skip0: GLinear,
    skip1: GLinear,
    /// The position embedding of each grid size, uploaded on first use.
    pos: Mutex<HashMap<(usize, usize), Buf>>,
    /// The CPU backbone, which owns the position embedding tables.
    cpu: Arc<Hiera>,
    eps: f32,
}

impl GpuEncoder {
    /// Upload the backbone, the neck and the skip convolutions. `cpu` supplies the position embedding.
    pub fn load(gpu: Arc<Gpu>, w: &Weights, cpu: Arc<Hiera>) -> Result<GpuEncoder> {
        let cfg = cpu.config().clone();
        let flat = |t: Tensor| -> Result<Vec<f32>> { Ok(t.flatten_all()?.to_vec1::<f32>()?) };
        let hidden = cfg.embed_dims.first().copied().unwrap_or(96);
        let mut blocks = vec![];
        for (total, plan) in BlockPlan::all(&cfg)?.into_iter().enumerate() {
            let p = format!("vision_encoder.backbone.blocks.{total}");
            blocks.push(GBlock {
                plan,
                norm1: GNorm::load(&gpu, w, &format!("{p}.layer_norm1"), plan.dim)?,
                qkv: GLinear::load(&gpu, w, &format!("{p}.attn.qkv"), plan.dim, plan.dim_out * 3)?,
                proj: GLinear::load(&gpu, w, &format!("{p}.attn.proj"), plan.dim_out, plan.dim_out)?,
                norm2: GNorm::load(&gpu, w, &format!("{p}.layer_norm2"), plan.dim_out)?,
                mlp_in: GLinear::load(&gpu, w, &format!("{p}.mlp.proj_in"), plan.dim_out, plan.dim_out * cfg.mlp_ratio)?,
                mlp_out: GLinear::load(&gpu, w, &format!("{p}.mlp.proj_out"), plan.dim_out * cfg.mlp_ratio, plan.dim_out)?,
                block_proj: (plan.dim != plan.dim_out).then(|| GLinear::load(&gpu, w, &format!("{p}.proj"), plan.dim, plan.dim_out)).transpose()?,
            });
        }
        let mut stage_ends = vec![];
        let mut n = 0;
        for b in &cfg.blocks {
            n += b;
            stage_ends.push(n.saturating_sub(1));
        }
        let channels: Vec<usize> = cfg.embed_dims.iter().rev().copied().collect();
        let neck = channels
            .iter()
            .enumerate()
            .map(|(j, c)| GLinear::load_conv1x1(&gpu, w, &format!("vision_encoder.neck.convs.{j}"), *c, 256))
            .collect::<Result<Vec<_>>>()?;
        Ok(GpuEncoder {
            patch_w: gpu.upload(&flat(w.get("vision_encoder.backbone.patch_embed.projection.weight", &[hidden, 3, 7, 7])?)?),
            patch_b: gpu.upload(&flat(w.get("vision_encoder.backbone.patch_embed.projection.bias", &[hidden])?)?),
            skip0: GLinear::load_conv1x1(&gpu, w, "mask_decoder.conv_s0", 256, 32)?,
            skip1: GLinear::load_conv1x1(&gpu, w, "mask_decoder.conv_s1", 256, 64)?,
            gpu,
            eps: cfg.eps as f32,
            cfg,
            blocks,
            stage_ends,
            neck,
            top_down: vec![2, 3],
            pos: Mutex::default(),
            cpu,
        })
    }

    fn linear(&self, x: &Buf, rows: usize, l: &GLinear, out: &Buf, act: Act, residual: Option<&Buf>) -> Result<()> {
        self.gpu.linear(x, rows, l, out, act, residual)
    }

    fn norm(&self, x: &Buf, rows: usize, dim: usize, n: &GNorm) -> Result<Buf> {
        let out = self.gpu.zeros(rows * dim)?;
        self.gpu.layernorm(x, &n.w, &n.b, &out, rows, dim, self.eps, false)?;
        Ok(out)
    }

    fn pos_buffer(&self, h: usize, w: usize) -> Result<Buf> {
        let mut map = self.pos.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(b) = map.get(&(h, w)) {
            return Ok(b.clone());
        }
        let table = self.cpu.pos(h, w)?.flatten_all()?.to_vec1::<f32>()?;
        let buf = self.gpu.upload(&table);
        map.insert((h, w), buf.clone());
        Ok(buf)
    }

    /// One Hiera block on `[h * w, dim]` tokens; returns the new tokens and grid size.
    fn block(&self, b: &GBlock, x: &Buf, h: usize, w: usize) -> Result<(Buf, usize, usize)> {
        let g = &self.gpu;
        let BlockPlan { dim, dim_out, heads, window, query_stride } = b.plan;
        let n = h * w;
        let normed = self.norm(x, n, dim, &b.norm1)?;
        // the skip connection: from the block's input, or (stage change) its projection, pooled with the queries
        let (oh, ow) = query_stride.map_or((h, w), |s| (h / s, w / s));
        if query_stride.is_some_and(|s| s != 2) {
            return Err(Error::Input("only a query stride of 2 is supported on the GPU".into()));
        }
        let residual = match (&b.block_proj, query_stride) {
            (Some(p), Some(_)) => {
                let proj = g.zeros(n * dim_out)?;
                self.linear(&normed, n, p, &proj, Act::None, None)?;
                let pooled = g.zeros(oh * ow * dim_out)?;
                g.maxpool2(&proj, &pooled, 1, h, w, dim_out, (n * dim_out, w * dim_out, dim_out))?;
                pooled
            }
            (Some(p), None) => {
                let proj = g.zeros(n * dim_out)?;
                self.linear(&normed, n, p, &proj, Act::None, None)?;
                proj
            }
            (None, _) => x.clone(),
        };
        // windows (or the whole grid for global attention)
        let (hw, nwin, t, win_h, win_w) = if window > 0 {
            let (nwx, nwy) = (w.div_ceil(window), h.div_ceil(window));
            let buf = g.zeros(nwx * nwy * window * window * dim)?;
            g.window_partition(&normed, &buf, h, w, dim, window)?;
            (buf, nwx * nwy, window * window, window, window)
        } else {
            (normed, 1, n, h, w)
        };
        let tok = nwin * t;
        let qkv = g.zeros(tok * 3 * dim_out)?;
        self.linear(&hw, tok, &b.qkv, &qkv, Act::None, None)?;
        // queries, pooled when the stage changes
        let (q_buf, q_rs, tq) = match query_stride {
            Some(s) => {
                let (qh, qw) = (win_h / s, win_w / s);
                let pooled = g.zeros(nwin * qh * qw * dim_out)?;
                g.maxpool2(&qkv, &pooled, nwin, win_h, win_w, dim_out, (t * 3 * dim_out, win_w * 3 * dim_out, 3 * dim_out))?;
                (pooled, dim_out, qh * qw)
            }
            None => (qkv.clone(), 3 * dim_out, t),
        };
        let hd = dim_out / heads.max(1);
        let scale = (hd as f32).powf(-0.5);
        let scores = g.zeros(nwin * tq * t)?;
        let attn = g.zeros(nwin * tq * dim_out)?;
        for hh in 0..heads {
            g.matmul(&Matmul {
                m: tq,
                n: t,
                k: hd,
                batch: nwin,
                a: (&q_buf, Strides { offset: hh * hd, batch: tq * q_rs, row: q_rs, col: 1 }),
                b: (&qkv, Strides { offset: dim_out + hh * hd, batch: t * 3 * dim_out, row: 1, col: 3 * dim_out }),
                bias: None,
                c: (&scores, Strides { offset: 0, batch: tq * t, row: t, col: 1 }),
                residual: None,
                alpha: scale,
                act: Act::None,
            })?;
            g.softmax(&scores, nwin * tq, t)?;
            g.matmul(&Matmul {
                m: tq,
                n: hd,
                k: t,
                batch: nwin,
                a: (&scores, Strides { offset: 0, batch: tq * t, row: t, col: 1 }),
                b: (&qkv, Strides { offset: 2 * dim_out + hh * hd, batch: t * 3 * dim_out, row: 3 * dim_out, col: 1 }),
                bias: None,
                c: (&attn, Strides { offset: hh * hd, batch: tq * dim_out, row: dim_out, col: 1 }),
                residual: None,
                alpha: 1.0,
                act: Act::None,
            })?;
        }
        let projected = g.zeros(nwin * tq * dim_out)?;
        self.linear(&attn, nwin * tq, &b.proj, &projected, Act::None, None)?;
        let attn_img = if window > 0 {
            let ws = window / query_stride.unwrap_or(1);
            let img = g.zeros(oh * ow * dim_out)?;
            g.window_unpartition(&projected, &img, oh, ow, dim_out, ws)?;
            img
        } else {
            projected
        };
        let n2 = oh * ow;
        let x2 = g.zeros(n2 * dim_out)?;
        g.add(&residual, &attn_img, &x2, n2 * dim_out)?;
        let normed2 = self.norm(&x2, n2, dim_out, &b.norm2)?;
        let hidden = g.zeros(n2 * b.mlp_in.out)?;
        self.linear(&normed2, n2, &b.mlp_in, &hidden, Act::Gelu, None)?;
        let out = g.zeros(n2 * dim_out)?;
        self.linear(&hidden, n2, &b.mlp_out, &out, Act::None, Some(&x2))?;
        Ok((out, oh, ow))
    }

    /// Encode normalised `[1, 3, H, W]` pixels (`H` and `W` multiples of 32).
    pub fn encode(&self, pixels: &Tensor) -> Result<GpuFeatures> {
        let g = &self.gpu;
        let _turn = g.exclusive();
        let (_, c, ph, pw) = pixels.dims4()?;
        let window0 = self.cfg.windows.first().copied().unwrap_or(8);
        if c != 3 || ph == 0 || pw == 0 || ph % 4 != 0 || pw % 4 != 0 || (ph / 4) % window0 != 0 || (pw / 4) % window0 != 0 {
            return Err(Error::Input(format!("image size {pw}x{ph} does not fit the encoder")));
        }
        let hidden = self.cfg.embed_dims.first().copied().unwrap_or(96);
        let px = g.upload(&pixels.contiguous()?.flatten_all()?.to_vec1::<f32>()?);
        let (mut h, mut w) = (ph / 4, pw / 4);
        let tokens = g.zeros(h * w * hidden)?;
        g.patch_embed(&px, &self.patch_w, &self.patch_b, &tokens, (ph, pw, hidden))?;
        let mut x = g.zeros(h * w * hidden)?;
        g.add(&tokens, &self.pos_buffer(h, w)?, &x, h * w * hidden)?;
        let mut stages: Vec<(Buf, usize, usize, usize)> = vec![];
        for (i, b) in self.blocks.iter().enumerate() {
            let (nx, nh, nw) = self.block(b, &x, h, w)?;
            (x, h, w) = (nx, nh, nw);
            // submit per block so the GPU can free the previous block's intermediates (Large would otherwise run out of memory)
            g.flush()?;
            if self.stage_ends.contains(&i) {
                stages.push((x.clone(), h, w, b.plan.dim_out));
            }
        }
        // the neck, from the lowest resolution up: lateral 1x1 convolutions, and the top-down sum on the levels that have it
        let n = self.neck.len();
        if stages.len() != n {
            return Err(Error::Compute("the backbone returned the wrong number of stages".into()));
        }
        let mut levels: Vec<(Buf, usize, usize)> = vec![];
        let mut prev: Option<(Buf, usize, usize)> = None;
        for i in (0..n).rev() {
            let (buf, sh, sw, _) = &stages[i];
            let conv = self.neck.get(n - 1 - i).ok_or_else(|| Error::Compute("missing neck convolution".into()))?;
            let lateral = g.zeros(sh * sw * 256)?;
            self.linear(buf, sh * sw, conv, &lateral, Act::None, None)?;
            let cur = match (&prev, self.top_down.contains(&i) && i != n - 1) {
                (Some((p, _, _)), true) => {
                    let sum = g.zeros(sh * sw * 256)?;
                    g.upsample_add(&lateral, p, &sum, *sh, *sw, 256)?;
                    sum
                }
                _ => lateral,
            };
            prev = Some((cur.clone(), *sh, *sw));
            levels.push((cur, *sh, *sw));
        }
        // levels: lowest resolution first. The decoder needs the third (64 x 64) and the skip convolutions of the last two.
        let [_, (l2, h2, w2), (l1, h1, w1), (l0, h0, w0)] = <[_; 4]>::try_from(levels).map_err(|_| Error::Compute("the neck must have four levels".into()))?;
        let s0 = g.zeros(h0 * w0 * 32)?;
        self.linear(&l0, h0 * w0, &self.skip0, &s0, Act::None, None)?;
        let s1 = g.zeros(h1 * w1 * 64)?;
        self.linear(&l1, h1 * w1, &self.skip1, &s1, Act::None, None)?;
        g.flush()?;
        let nchw = |buf: &Buf, hh: usize, ww: usize, ch: usize| -> Result<Tensor> {
            let v = g.download(buf)?;
            Ok(Tensor::from_vec(v, (1, hh, ww, ch), &Device::Cpu)?.permute((0, 3, 1, 2))?.contiguous()?)
        };
        Ok(GpuFeatures { low: nchw(&l2, h2, w2, 256)?, high: (nchw(&s0, h0, w0, 32)?, nchw(&s1, h1, w1, 64)?) })
    }
}
