//! MobileSAM (C. Zhang et al., "Faster Segment Anything: Towards Lightweight SAM for Mobile
//! Applications", 2023; Apache-2.0) in plain Rust: the TinyViT-5M image encoder (Wu et al.,
//! "TinyViT", ECCV 2022) feeding Segment Anything's prompt encoder and two-way-transformer mask
//! decoder (Kirillov et al., "Segment Anything", ICCV 2023), loaded straight from the official
//! `mobile_sam.pt` checkpoint. The image goes in at 1024 pixels on its long side; the encoder
//! gives a 64×64×256 embedding, cached per image so that new prompts on the same frame only run
//! the (fast) decoder.

use std::f32::consts::PI;
use std::sync::Mutex;

use rayon::prelude::*;

use crate::nn::{self, Conv, Depthwise, Linear, Norm, Upconv};
use crate::pt::Weights;
use crate::{Prompt, Result};

const IMG: usize = 1024;
const EMB: usize = 64;
const DIM: usize = 256;
const LOW: usize = 256;
const MEAN: [f32; 3] = [123.675, 116.28, 103.53];
const STD: [f32; 3] = [58.395, 57.12, 57.375];

/// Weight lookup by name with the expected element count.
struct W<'a>(&'a Weights);

impl W<'_> {
    fn get(&self, name: &str, n: usize) -> Result<&[f32]> {
        let t = self.0.get(name).ok_or_else(|| format!("checkpoint is missing {name}"))?;
        if t.data.len() != n {
            return Err(format!("{name}: expected {n} values, found {}", t.data.len()));
        }
        Ok(&t.data)
    }
    fn linear(&self, p: &str, out: usize, inp: usize, bias: bool) -> Result<Linear> {
        let w = self.get(&format!("{p}.weight"), out * inp)?;
        let b = if bias { Some(self.get(&format!("{p}.bias"), out)?) } else { None };
        Linear::new(w, b, out, inp).ok_or_else(|| format!("{p}: bad shape"))
    }
    fn norm(&self, p: &str, c: usize, eps: f32) -> Result<Norm> {
        Ok(Norm { w: self.get(&format!("{p}.weight"), c)?.to_vec(), b: self.get(&format!("{p}.bias"), c)?.to_vec(), eps })
    }
    /// A conv + batch norm (eval) folded into one weight and bias: `[out × inp/groups × k × k]`.
    fn conv_bn(&self, p: &str, out: usize, per: usize) -> Result<(Vec<f32>, Vec<f32>)> {
        let w = self.get(&format!("{p}.c.weight"), out * per)?;
        let g = self.get(&format!("{p}.bn.weight"), out)?;
        let b = self.get(&format!("{p}.bn.bias"), out)?;
        let m = self.get(&format!("{p}.bn.running_mean"), out)?;
        let v = self.get(&format!("{p}.bn.running_var"), out)?;
        let mut wf = w.to_vec();
        let mut bf = vec![0.0; out];
        for o in 0..out {
            let s = g[o] / (v[o] + 1e-5).sqrt();
            wf[o * per..(o + 1) * per].iter_mut().for_each(|x| *x *= s);
            bf[o] = b[o] - m[o] * s;
        }
        Ok((wf, bf))
    }
    fn pointwise_bn(&self, p: &str, out: usize, inp: usize) -> Result<Linear> {
        let (w, b) = self.conv_bn(p, out, inp)?;
        Linear::new(&w, Some(&b), out, inp).ok_or_else(|| format!("{p}: bad shape"))
    }
    fn depthwise_bn(&self, p: &str, c: usize, stride: usize) -> Result<Depthwise> {
        let (w, b) = self.conv_bn(p, c, 9)?;
        Depthwise::new(&w, Some(&b), c, stride).ok_or_else(|| format!("{p}: bad shape"))
    }
    fn conv3_bn(&self, p: &str, out: usize, inp: usize, stride: usize) -> Result<Conv> {
        let (w, b) = self.conv_bn(p, out, inp * 9)?;
        Conv::new(&w, Some(&b), out, inp, 3, stride, 1).ok_or_else(|| format!("{p}: bad shape"))
    }
}

/// MBConv: 1×1 expand, depthwise 3×3, 1×1 project, residual.
struct MbConv {
    c1: Linear,
    c2: Depthwise,
    c3: Linear,
}

impl MbConv {
    fn forward(&self, x: &[f32], h: usize, w: usize) -> Vec<f32> {
        let mut t = self.c1.forward(x, h * w);
        nn::gelu(&mut t);
        let (mut t, _, _) = self.c2.forward(&t, h, w);
        nn::gelu(&mut t);
        let mut t = self.c3.forward(&t, h * w);
        nn::add(&mut t, x);
        nn::gelu(&mut t);
        t
    }
}

/// Patch merging between stages: 1×1, depthwise 3×3 (stride 2, or 1 into the last stage), 1×1.
struct Merge {
    c1: Linear,
    c2: Depthwise,
    c3: Linear,
}

impl Merge {
    fn forward(&self, x: &[f32], h: usize, w: usize) -> (Vec<f32>, usize, usize) {
        let mut t = self.c1.forward(x, h * w);
        nn::gelu(&mut t);
        let (mut t, oh, ow) = self.c2.forward(&t, h, w);
        nn::gelu(&mut t);
        (self.c3.forward(&t, oh * ow), oh, ow)
    }
}

/// A TinyViT block: windowed attention (with learned relative-position biases), a depthwise
/// "local conv", then an MLP.
struct Block {
    norm: Norm,
    qkv: Linear,
    proj: Linear,
    /// `[heads × n × n]` for the `ws × ws` window.
    bias: Vec<f32>,
    heads: usize,
    ws: usize,
    local: Depthwise,
    mlp_norm: Norm,
    fc1: Linear,
    fc2: Linear,
}

impl Block {
    fn new(w: &W, p: &str, dim: usize, heads: usize, ws: usize) -> Result<Block> {
        let n = ws * ws;
        // Offsets between window positions, numbered in first-seen order (as TinyViT does).
        let points: Vec<(i64, i64)> = (0..ws as i64).flat_map(|r| (0..ws as i64).map(move |c| (r, c))).collect();
        let mut offsets: Vec<(i64, i64)> = vec![];
        let mut idx = Vec::with_capacity(n * n);
        for a in &points {
            for b in &points {
                let o = ((a.0 - b.0).abs(), (a.1 - b.1).abs());
                let i = match offsets.iter().position(|x| *x == o) {
                    Some(i) => i,
                    None => {
                        offsets.push(o);
                        offsets.len() - 1
                    }
                };
                idx.push(i);
            }
        }
        let table = w.get(&format!("{p}.attn.attention_biases"), heads * offsets.len())?;
        let no = offsets.len();
        let bias = (0..heads).flat_map(|h| idx.iter().map(move |i| table[h * no + i])).collect::<Vec<f32>>();
        Ok(Block {
            norm: w.norm(&format!("{p}.attn.norm"), dim, 1e-5)?,
            qkv: w.linear(&format!("{p}.attn.qkv"), 3 * dim, dim, true)?,
            proj: w.linear(&format!("{p}.attn.proj"), dim, dim, true)?,
            bias,
            heads,
            ws,
            local: w.depthwise_bn(&format!("{p}.local_conv"), dim, 1)?,
            mlp_norm: w.norm(&format!("{p}.mlp.norm"), dim, 1e-5)?,
            fc1: w.linear(&format!("{p}.mlp.fc1"), 4 * dim, dim, true)?,
            fc2: w.linear(&format!("{p}.mlp.fc2"), dim, 4 * dim, true)?,
        })
    }

    fn forward(&self, x: &mut Vec<f32>, h: usize, w: usize) {
        let dim = self.proj.out;
        let (ws, heads) = (self.ws, self.heads);
        let hd = dim / heads;
        // Pad the grid to whole windows (zeros, as TinyViT does), then norm + qkv once.
        let (ph, pw) = (h.div_ceil(ws) * ws, w.div_ceil(ws) * ws);
        let mut pad = vec![0.0f32; ph * pw * dim];
        for y in 0..h {
            pad[y * pw * dim..(y * pw + w) * dim].copy_from_slice(&x[y * w * dim..(y + 1) * w * dim]);
        }
        self.norm.forward(&mut pad);
        let qkv = self.qkv.forward(&pad, ph * pw);
        // Per window: gather q, k, v (each head's qkv is [q | k | v]), attend.
        let (nwy, nwx) = (ph / ws, pw / ws);
        let n = ws * ws;
        let outs: Vec<Vec<f32>> = (0..nwy * nwx)
            .into_par_iter()
            .map(|win| {
                let (wy, wx) = (win / nwx, win % nwx);
                let (mut q, mut k, mut v) = (vec![0.0f32; n * dim], vec![0.0f32; n * dim], vec![0.0f32; n * dim]);
                for t in 0..n {
                    let (ty, tx) = (wy * ws + t / ws, wx * ws + t % ws);
                    let src = &qkv[(ty * pw + tx) * 3 * dim..][..3 * dim];
                    for hh in 0..heads {
                        let s = &src[hh * 3 * hd..][..3 * hd];
                        q[t * dim + hh * hd..][..hd].copy_from_slice(&s[..hd]);
                        k[t * dim + hh * hd..][..hd].copy_from_slice(&s[hd..2 * hd]);
                        v[t * dim + hh * hd..][..hd].copy_from_slice(&s[2 * hd..]);
                    }
                }
                nn::attention(&q, &k, &v, nn::AttentionShape { query_tokens: n, key_tokens: n, heads, head_dim: hd }, Some(&self.bias))
            })
            .collect();
        // Un-window into the real grid, project, residual.
        let mut a = vec![0.0f32; h * w * dim];
        for (win, o) in outs.iter().enumerate() {
            let (wy, wx) = (win / nwx, win % nwx);
            for t in 0..n {
                let (ty, tx) = (wy * ws + t / ws, wx * ws + t % ws);
                if ty < h && tx < w {
                    a[(ty * w + tx) * dim..][..dim].copy_from_slice(&o[t * dim..][..dim]);
                }
            }
        }
        let a = self.proj.forward(&a, h * w);
        nn::add(x, &a);
        // Local depthwise conv (no residual), then the MLP with its residual.
        let (lc, _, _) = self.local.forward(x, h, w);
        *x = lc;
        let mut m = x.clone();
        self.mlp_norm.forward(&mut m);
        let mut m = self.fc1.forward(&m, h * w);
        nn::gelu(&mut m);
        let m = self.fc2.forward(&m, h * w);
        nn::add(x, &m);
    }
}

/// The TinyViT-5M encoder with SAM's neck.
struct Encoder {
    pe1: Conv,
    pe2: Conv,
    stage0: Vec<MbConv>,
    merges: Vec<Merge>,
    stages: Vec<Vec<Block>>,
    neck1: Linear,
    neck_ln1: Norm,
    neck2: Conv,
    neck_ln2: Norm,
}

const DIMS: [usize; 4] = [64, 128, 160, 320];
const DEPTHS: [usize; 4] = [2, 2, 6, 2];
const HEADS: [usize; 4] = [2, 4, 5, 10];
const WINDOWS: [usize; 4] = [7, 7, 14, 7];

impl Encoder {
    fn new(w: &W) -> Result<Encoder> {
        let p = "image_encoder";
        let stage0 = (0..DEPTHS[0])
            .map(|i| {
                let b = format!("{p}.layers.0.blocks.{i}");
                Ok(MbConv {
                    c1: w.pointwise_bn(&format!("{b}.conv1"), DIMS[0] * 4, DIMS[0])?,
                    c2: w.depthwise_bn(&format!("{b}.conv2"), DIMS[0] * 4, 1)?,
                    c3: w.pointwise_bn(&format!("{b}.conv3"), DIMS[0], DIMS[0] * 4)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mut merges = vec![];
        let mut stages = vec![];
        for l in 0..3 {
            let (inp, out) = (DIMS[l], DIMS[l + 1]);
            let d = format!("{p}.layers.{l}.downsample");
            merges.push(Merge {
                c1: w.pointwise_bn(&format!("{d}.conv1"), out, inp)?,
                c2: w.depthwise_bn(&format!("{d}.conv2"), out, if out == 320 { 1 } else { 2 })?,
                c3: w.pointwise_bn(&format!("{d}.conv3"), out, out)?,
            });
            let s = l + 1;
            stages
                .push((0..DEPTHS[s]).map(|i| Block::new(w, &format!("{p}.layers.{s}.blocks.{i}"), DIMS[s], HEADS[s], WINDOWS[s])).collect::<Result<Vec<_>>>()?);
        }
        let n2 = w.get(&format!("{p}.neck.2.weight"), DIM * DIM * 9)?;
        Ok(Encoder {
            pe1: w.conv3_bn(&format!("{p}.patch_embed.seq.0"), DIMS[0] / 2, 3, 2)?,
            pe2: w.conv3_bn(&format!("{p}.patch_embed.seq.2"), DIMS[0], DIMS[0] / 2, 2)?,
            stage0,
            merges,
            stages,
            neck1: w.linear(&format!("{p}.neck.0"), DIM, DIMS[3], false)?,
            neck_ln1: w.norm(&format!("{p}.neck.1"), DIM, 1e-6)?,
            neck2: Conv::new(n2, None, DIM, DIM, 3, 1, 1).ok_or("neck: bad shape")?,
            neck_ln2: w.norm(&format!("{p}.neck.3"), DIM, 1e-6)?,
        })
    }

    /// `IMG×IMG×3` normalised pixels → `EMB×EMB×DIM`.
    fn forward(&self, x: &[f32]) -> Vec<f32> {
        let (mut t, h, w) = self.pe1.forward(x, IMG, IMG);
        nn::gelu(&mut t);
        let (mut t, mut h, mut w) = {
            let (t2, h2, w2) = self.pe2.forward(&t, h, w);
            (t2, h2, w2)
        };
        for b in &self.stage0 {
            t = b.forward(&t, h, w);
        }
        for (m, blocks) in self.merges.iter().zip(&self.stages) {
            let (t2, h2, w2) = m.forward(&t, h, w);
            (t, h, w) = (t2, h2, w2);
            for b in blocks {
                b.forward(&mut t, h, w);
            }
        }
        let mut t = self.neck1.forward(&t, h * w);
        self.neck_ln1.forward(&mut t);
        let (mut t, _, _) = self.neck2.forward(&t, h, w);
        self.neck_ln2.forward(&mut t);
        t
    }
}

/// SAM's attention with optional down-projection (`internal = DIM / rate`).
struct Attn {
    q: Linear,
    k: Linear,
    v: Linear,
    out: Linear,
}

impl Attn {
    fn new(w: &W, p: &str, rate: usize) -> Result<Attn> {
        let i = DIM / rate;
        Ok(Attn {
            q: w.linear(&format!("{p}.q_proj"), i, DIM, true)?,
            k: w.linear(&format!("{p}.k_proj"), i, DIM, true)?,
            v: w.linear(&format!("{p}.v_proj"), i, DIM, true)?,
            out: w.linear(&format!("{p}.out_proj"), DIM, i, true)?,
        })
    }
    fn forward(&self, q: &[f32], nq: usize, k: &[f32], v: &[f32], nk: usize) -> Vec<f32> {
        let (q, k, v) = (self.q.forward(q, nq), self.k.forward(k, nk), self.v.forward(v, nk));
        let heads = 8;
        let o = nn::attention(&q, &k, &v, nn::AttentionShape { query_tokens: nq, key_tokens: nk, heads, head_dim: self.q.out / heads }, None);
        self.out.forward(&o, nq)
    }
}

fn plus(a: &[f32], b: &[f32]) -> Vec<f32> {
    a.iter().zip(b).map(|(x, y)| x + y).collect()
}

struct TwoWayBlock {
    self_attn: Attn,
    norm1: Norm,
    t2i: Attn,
    norm2: Norm,
    lin1: Linear,
    lin2: Linear,
    norm3: Norm,
    i2t: Attn,
    norm4: Norm,
    skip_pe: bool,
}

/// The decoder: prompt encoder, two-way transformer, mask head.
struct Decoder {
    gauss: Vec<f32>,
    /// `EMB×EMB×DIM` positional encoding of the embedding grid.
    dense_pe: Vec<f32>,
    point_embed: [Vec<f32>; 4],
    not_a_point: Vec<f32>,
    no_mask: Vec<f32>,
    md1: Conv,
    md_ln1: Norm,
    md2: Conv,
    md_ln2: Norm,
    md3: Linear,
    blocks: Vec<TwoWayBlock>,
    final_attn: Attn,
    norm_final: Norm,
    iou_token: Vec<f32>,
    mask_tokens: Vec<f32>,
    up1: Upconv,
    up_ln: Norm,
    up2: Upconv,
    hyper: Vec<[Linear; 3]>,
    iou_head: [Linear; 3],
}

impl Decoder {
    fn new(w: &W) -> Result<Decoder> {
        let pe = "prompt_encoder";
        let md = "mask_decoder";
        let gauss = w.get(&format!("{pe}.pe_layer.positional_encoding_gaussian_matrix"), 2 * DIM / 2)?.to_vec();
        let pt = |i: usize| w.get(&format!("{pe}.point_embeddings.{i}.weight"), DIM).map(<[f32]>::to_vec);
        let conv = |p: &str, out: usize, inp: usize, ks: usize| -> Result<Conv> {
            let wt = w.get(&format!("{p}.weight"), out * inp * ks * ks)?;
            let b = w.get(&format!("{p}.bias"), out)?;
            Conv::new(wt, Some(b), out, inp, ks, 2, 0).ok_or_else(|| format!("{p}: bad shape"))
        };
        let blocks = (0..2)
            .map(|i| {
                let p = format!("{md}.transformer.layers.{i}");
                Ok(TwoWayBlock {
                    self_attn: Attn::new(w, &format!("{p}.self_attn"), 1)?,
                    norm1: w.norm(&format!("{p}.norm1"), DIM, 1e-5)?,
                    t2i: Attn::new(w, &format!("{p}.cross_attn_token_to_image"), 2)?,
                    norm2: w.norm(&format!("{p}.norm2"), DIM, 1e-5)?,
                    lin1: w.linear(&format!("{p}.mlp.lin1"), 2048, DIM, true)?,
                    lin2: w.linear(&format!("{p}.mlp.lin2"), DIM, 2048, true)?,
                    norm3: w.norm(&format!("{p}.norm3"), DIM, 1e-5)?,
                    i2t: Attn::new(w, &format!("{p}.cross_attn_image_to_token"), 2)?,
                    norm4: w.norm(&format!("{p}.norm4"), DIM, 1e-5)?,
                    skip_pe: i == 0,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let upconv = |p: &str, inp: usize, out: usize| -> Result<Upconv> {
            Upconv::new(w.get(&format!("{p}.weight"), inp * out * 4)?, w.get(&format!("{p}.bias"), out)?, inp, out).ok_or_else(|| format!("{p}: bad shape"))
        };
        let mlp3 = |p: &str, out: usize| -> Result<[Linear; 3]> {
            Ok([
                w.linear(&format!("{p}.layers.0"), DIM, DIM, true)?,
                w.linear(&format!("{p}.layers.1"), DIM, DIM, true)?,
                w.linear(&format!("{p}.layers.2"), out, DIM, true)?,
            ])
        };
        let mut d = Decoder {
            gauss,
            dense_pe: vec![],
            point_embed: [pt(0)?, pt(1)?, pt(2)?, pt(3)?],
            not_a_point: w.get(&format!("{pe}.not_a_point_embed.weight"), DIM)?.to_vec(),
            no_mask: w.get(&format!("{pe}.no_mask_embed.weight"), DIM)?.to_vec(),
            md1: conv(&format!("{pe}.mask_downscaling.0"), 4, 1, 2)?,
            md_ln1: w.norm(&format!("{pe}.mask_downscaling.1"), 4, 1e-6)?,
            md2: conv(&format!("{pe}.mask_downscaling.3"), 16, 4, 2)?,
            md_ln2: w.norm(&format!("{pe}.mask_downscaling.4"), 16, 1e-6)?,
            md3: w.linear(&format!("{pe}.mask_downscaling.6"), DIM, 16, true)?,
            blocks,
            final_attn: Attn::new(w, &format!("{md}.transformer.final_attn_token_to_image"), 2)?,
            norm_final: w.norm(&format!("{md}.transformer.norm_final_attn"), DIM, 1e-5)?,
            iou_token: w.get(&format!("{md}.iou_token.weight"), DIM)?.to_vec(),
            mask_tokens: w.get(&format!("{md}.mask_tokens.weight"), 4 * DIM)?.to_vec(),
            up1: upconv(&format!("{md}.output_upscaling.0"), DIM, DIM / 4)?,
            up_ln: w.norm(&format!("{md}.output_upscaling.1"), DIM / 4, 1e-6)?,
            up2: upconv(&format!("{md}.output_upscaling.3"), DIM / 4, DIM / 8)?,
            hyper: (0..4).map(|i| mlp3(&format!("{md}.output_hypernetworks_mlps.{i}"), DIM / 8)).collect::<Result<Vec<_>>>()?,
            iou_head: mlp3(&format!("{md}.iou_prediction_head"), 4)?,
        };
        d.dense_pe = (0..EMB * EMB).flat_map(|i| d.pe(((i % EMB) as f32 + 0.5) / EMB as f32, ((i / EMB) as f32 + 0.5) / EMB as f32)).collect();
        Ok(d)
    }

    /// Random Fourier positional encoding of a point in [0, 1]².
    fn pe(&self, x: f32, y: f32) -> Vec<f32> {
        let half = DIM / 2;
        let (cx, cy) = (2.0 * x - 1.0, 2.0 * y - 1.0);
        let a: Vec<f32> = (0..half).map(|j| 2.0 * PI * (cx * self.gauss[j] + cy * self.gauss[half + j])).collect();
        a.iter().map(|v| v.sin()).chain(a.iter().map(|v| v.cos())).collect()
    }

    fn mlp3(layers: &[Linear; 3], x: &[f32]) -> Vec<f32> {
        let mut t = layers[0].forward(x, 1);
        nn::relu(&mut t);
        let mut t = layers[1].forward(&t, 1);
        nn::relu(&mut t);
        layers[2].forward(&t, 1)
    }

    /// Mask logits (`LOW×LOW`, in the padded `IMG` square) and their predicted IoU for the
    /// prompt (coordinates already in `IMG` space).
    fn forward(&self, emb: &[f32], points: &[([f32; 2], bool)], bbox: Option<[f32; 4]>, mask: Option<&[f32]>, multimask: bool) -> (Vec<f32>, f32) {
        // Sparse prompt tokens.
        let mut sparse: Vec<f32> = vec![];
        for (p, fg) in points {
            let mut e = self.pe((p[0] + 0.5) / IMG as f32, (p[1] + 0.5) / IMG as f32);
            let add = &self.point_embed[usize::from(*fg)];
            e.iter_mut().zip(add).for_each(|(a, b)| *a += b);
            sparse.extend(e);
        }
        match bbox {
            Some(b) => {
                for (i, (x, y)) in [(b[0], b[1]), (b[2], b[3])].into_iter().enumerate() {
                    let mut e = self.pe((x + 0.5) / IMG as f32, (y + 0.5) / IMG as f32);
                    e.iter_mut().zip(&self.point_embed[2 + i]).for_each(|(a, b)| *a += b);
                    sparse.extend(e);
                }
            }
            // Points without a box get a padding "not a point".
            None if !points.is_empty() => sparse.extend(&self.not_a_point),
            None => {}
        }
        // Dense prompt: the mask input, or "no mask".
        let dense: Vec<f32> = match mask {
            Some(m) if m.len() == LOW * LOW => {
                let (mut t, h, w) = self.md1.forward(m, LOW, LOW);
                self.md_ln1.forward(&mut t);
                nn::gelu(&mut t);
                let (mut t, h, w) = self.md2.forward(&t, h, w);
                self.md_ln2.forward(&mut t);
                nn::gelu(&mut t);
                self.md3.forward(&t, h * w)
            }
            _ => (0..EMB * EMB).flat_map(|_| self.no_mask.iter().copied()).collect(),
        };
        // Tokens: IoU, four mask tokens, prompts.
        let mut tokens: Vec<f32> = self.iou_token.clone();
        tokens.extend(&self.mask_tokens);
        tokens.extend(&sparse);
        let nt = tokens.len() / DIM;
        let nk = EMB * EMB;
        let mut keys = plus(emb, &dense);
        let pe = &self.dense_pe;
        let qpe = tokens.clone();
        let mut queries = tokens;
        for b in &self.blocks {
            // Self attention.
            queries = if b.skip_pe {
                b.self_attn.forward(&queries, nt, &queries, &queries, nt)
            } else {
                let q = plus(&queries, &qpe);
                plus(&queries, &b.self_attn.forward(&q, nt, &q, &queries, nt))
            };
            b.norm1.forward(&mut queries);
            // Tokens to image.
            let q = plus(&queries, &qpe);
            let k = plus(&keys, pe);
            queries = plus(&queries, &b.t2i.forward(&q, nt, &k, &keys, nk));
            b.norm2.forward(&mut queries);
            // MLP.
            let mut m = b.lin1.forward(&queries, nt);
            nn::relu(&mut m);
            queries = plus(&queries, &b.lin2.forward(&m, nt));
            b.norm3.forward(&mut queries);
            // Image to tokens.
            let q = plus(&queries, &qpe);
            let k = plus(&keys, pe);
            keys = plus(&keys, &b.i2t.forward(&k, nk, &q, &queries, nt));
            b.norm4.forward(&mut keys);
        }
        let q = plus(&queries, &qpe);
        let k = plus(&keys, pe);
        queries = plus(&queries, &self.final_attn.forward(&q, nt, &k, &keys, nk));
        self.norm_final.forward(&mut queries);
        // Upscale the image tokens 4× and dot them with each mask token's hypernetwork output.
        let mut up = self.up1.forward(&keys, EMB, EMB);
        self.up_ln.forward(&mut up);
        nn::gelu(&mut up);
        let mut up = self.up2.forward(&up, 2 * EMB, 2 * EMB);
        nn::gelu(&mut up);
        let c = DIM / 8;
        let iou = Self::mlp3(&self.iou_head, &queries[..DIM]);
        let pick = if multimask { (1..4).max_by(|a, b| iou[*a].total_cmp(&iou[*b])).unwrap_or(0) } else { 0 };
        let hyper = Self::mlp3(&self.hyper[pick], &queries[(1 + pick) * DIM..][..DIM]);
        let logits: Vec<f32> = up.par_chunks(c).map(|px| px.iter().zip(&hyper).map(|(a, b)| a * b).sum()).collect();
        (logits, iou.get(pick).copied().unwrap_or(0.0))
    }
}

/// An image's embedding and how it was fitted into the model's square.
#[derive(Clone)]
pub struct Embedding {
    pub data: Vec<f32>,
    pub w: usize,
    pub h: usize,
    /// Image pixels → model pixels.
    pub scale: f32,
}

pub struct MobileSam {
    enc: Encoder,
    dec: Decoder,
    /// The last image embedded (by content hash): new prompts on the same frame reuse it.
    cache: Mutex<Option<(u64, Embedding)>>,
}

fn hash_rgb(rgb: &[[f32; 3]], w: usize, h: usize) -> u64 {
    let mut s: u64 = 0xcbf2_9ce4_8422_2325 ^ ((w as u64) << 32 | h as u64);
    for p in rgb.iter().step_by(7) {
        for v in p {
            s = (s ^ v.to_bits() as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }
    s
}

impl MobileSam {
    pub fn from_weights(w: &Weights) -> Result<MobileSam> {
        let w = W(w);
        Ok(MobileSam { enc: Encoder::new(&w)?, dec: Decoder::new(&w)?, cache: Mutex::new(None) })
    }

    /// Embed an image (straight RGB 0–1, row-major `w×h`), or reuse the last embedding.
    pub fn embed(&self, rgb: &[[f32; 3]], w: usize, h: usize) -> Result<Embedding> {
        if w == 0 || h == 0 || rgb.len() != w * h {
            return Err("empty image".into());
        }
        let key = hash_rgb(rgb, w, h);
        if let Ok(c) = self.cache.lock()
            && let Some((k, e)) = c.as_ref()
            && *k == key
        {
            return Ok(e.clone());
        }
        let scale = IMG as f32 / w.max(h) as f32;
        let (rw, rh) = (((w as f32 * scale).round() as usize).clamp(1, IMG), ((h as f32 * scale).round() as usize).clamp(1, IMG));
        // Resize (box filter when shrinking, bilinear when growing), normalise, pad.
        let mut x = vec![0.0f32; IMG * IMG * 3];
        x.par_chunks_mut(IMG * 3).enumerate().take(rh).for_each(|(y, row)| {
            for xo in 0..rw {
                let px = sample(rgb, w, h, (xo as f32 + 0.5) / scale, (y as f32 + 0.5) / scale, 1.0 / scale);
                for c in 0..3 {
                    row[xo * 3 + c] = (px[c] * 255.0 - MEAN[c]) / STD[c];
                }
            }
        });
        let e = Embedding { data: self.enc.forward(&x), w, h, scale };
        if let Ok(mut c) = self.cache.lock() {
            *c = Some((key, e.clone()));
        }
        Ok(e)
    }

    /// Foreground probability (`w×h`) for a prompt on an embedded image, and the predicted IoU.
    pub fn predict(&self, e: &Embedding, prompt: &Prompt) -> (Vec<f32>, f32) {
        let s = e.scale;
        let points: Vec<([f32; 2], bool)> = prompt.points.iter().map(|(p, fg)| ([p[0] * s, p[1] * s], *fg)).collect();
        let bbox = prompt.bbox.map(|b| b.map(|v| v * s));
        // A prior mask (probabilities at image size) → logits on the low-res grid.
        let mask: Option<Vec<f32>> = prompt.mask.as_ref().filter(|m| m.len() == e.w * e.h).map(|m| {
            (0..LOW * LOW)
                .map(|i| {
                    let (gx, gy) = (((i % LOW) as f32 + 0.5) * 4.0 / s, ((i / LOW) as f32 + 0.5) * 4.0 / s);
                    if gx >= e.w as f32 || gy >= e.h as f32 {
                        return -20.0;
                    }
                    let p = m[(gy as usize).min(e.h - 1) * e.w + (gx as usize).min(e.w - 1)];
                    ((p - 0.5) * 40.0).clamp(-20.0, 20.0)
                })
                .collect()
        });
        let multimask = points.len() == 1 && bbox.is_none() && mask.is_none();
        let (logits, iou) = self.dec.forward(&e.data, &points, bbox, mask.as_deref(), multimask);
        // Low-res logits (the padded square) → image pixels, bilinear, then sigmoid.
        let prob: Vec<f32> = (0..e.w * e.h)
            .into_par_iter()
            .map(|i| {
                let (x, y) = ((i % e.w) as f32 + 0.5, (i / e.w) as f32 + 0.5);
                let (u, v) = (x * s / 4.0 - 0.5, y * s / 4.0 - 0.5);
                let l = bilinear(&logits, LOW, LOW, u, v);
                1.0 / (1.0 + (-l).exp())
            })
            .collect();
        (prob, iou)
    }
}

fn bilinear(g: &[f32], w: usize, h: usize, u: f32, v: f32) -> f32 {
    let (u, v) = (u.clamp(0.0, (w - 1) as f32), v.clamp(0.0, (h - 1) as f32));
    let (x0, y0) = (u.floor() as usize, v.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (u - x0 as f32, v - y0 as f32);
    let at = |x: usize, y: usize| g.get(y * w + x).copied().unwrap_or(0.0);
    (at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx) * (1.0 - fy) + (at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx) * fy
}

/// The image's colour around `(x, y)`: a `foot`-pixel box average when shrinking, else bilinear.
fn sample(rgb: &[[f32; 3]], w: usize, h: usize, x: f32, y: f32, foot: f32) -> [f32; 3] {
    if foot <= 1.0 {
        let (u, v) = ((x - 0.5).clamp(0.0, (w - 1) as f32), (y - 0.5).clamp(0.0, (h - 1) as f32));
        let (x0, y0) = (u.floor() as usize, v.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (fx, fy) = (u - x0 as f32, v - y0 as f32);
        let p = |x: usize, y: usize| rgb[y * w + x];
        return [0, 1, 2].map(|c| (p(x0, y0)[c] * (1.0 - fx) + p(x1, y0)[c] * fx) * (1.0 - fy) + (p(x0, y1)[c] * (1.0 - fx) + p(x1, y1)[c] * fx) * fy);
    }
    let r = foot / 2.0;
    let (xa, xb) = (((x - r).floor().max(0.0)) as usize, ((x + r).ceil() as usize).min(w));
    let (ya, yb) = (((y - r).floor().max(0.0)) as usize, ((y + r).ceil() as usize).min(h));
    let mut s = [0.0f32; 3];
    let mut n = 0.0;
    for yy in ya..yb.max(ya + 1).min(h) {
        for xx in xa..xb.max(xa + 1).min(w) {
            let p = rgb[yy * w + xx];
            for c in 0..3 {
                s[c] += p[c];
            }
            n += 1.0;
        }
    }
    s.map(|v| v / f32::max(n, 1.0))
}
