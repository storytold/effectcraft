// Modified work: ported from Hugging Face `transformers` `models/sam2_video/modeling_sam2_video.py`
// (Sam2VideoModel and Sam2VideoInferenceSession: memory-conditioned single-frame inference,
// memory bank selection, object pointers, propagation; Apache-2.0, Copyright The HuggingFace
// Inc. team) to Rust on candle. See NOTICE.
//! The video tracker: per-object memory banks, prompts on any frame, propagation forwards and
//! backwards with streaming results and cancellation.

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use candle_core::{DType, Tensor};
use candle_nn::{Linear, ops::sigmoid};

use crate::decoder::FeedForward;
use crate::gpu::Gpu;
use crate::gpu::encoder::GpuEncoder;
use crate::gpu::memattn::GpuMemoryAttention;
use crate::gpu::memenc::GpuMemoryEncoder;
use crate::memattn::MemoryAttention;
use crate::memory::{Memory, MemoryEncoder};
use crate::model::{Encoded, ImageModel};
use crate::neck::ImageFeatures;
use crate::nn::{apply, linear};
use crate::prompt::{INPUT_SIZE, Prompt};
use crate::resize::antialias_bilinear;
use crate::weights::Weights;
use crate::{Error, Result};

/// Mask logit used where the object is absent (the reference's `NO_OBJ_SCORE`).
const NO_OBJECT: f32 = -1024.0;
/// Frames of memory the attention looks back over.
const MEMORY_FRAMES: usize = 7;
/// Object pointers kept per query.
const MAX_POINTERS: usize = 16;
/// Largest clip the session accepts.
const MAX_FRAMES: usize = 1_000_000;
const GRID: usize = 64;

/// Frames of the clip, already resized to the model input and normalised: `[1, 3, 1024, 1024]`.
pub trait FrameSource {
    fn num_frames(&self) -> usize;
    fn frame(&self, index: usize) -> Result<Tensor>;
}

/// Cooperative cancellation shared with the caller.
#[derive(Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Cancel {
        Cancel::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Backward,
}

/// One object's mask on one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameMask {
    pub frame: usize,
    pub object: ObjectId,
    /// Mask logits at 256 x 256 (positive is inside the object); `-1024` everywhere when the
    /// object is absent.
    pub logits: Vec<f32>,
    /// The model's own estimate of the mask quality (0..1).
    pub iou: f32,
    /// Object-presence logit; at or below zero the object is not in the frame.
    pub object_score: f32,
    pub occluded: bool,
}

/// Which device runs the heavy parts (the backbone, the neck and the memory attention).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// Candle on the CPU: the reference path.
    Cpu,
    /// wgpu compute on the GPU (any vendor); an error if there is none.
    Gpu,
    /// The GPU when there is a real one and it loads, else the CPU. The environment variable
    /// `EFFECTCRAFT_SAM2_DEVICE` (`cpu`, `gpu` or `auto`) overrides the choice.
    Auto,
}

/// The GPU path: kernels for the backbone and memory attention. Device failures disable it;
/// memory-attention budget limits fall back to the CPU for that call only.
struct GpuParts {
    encoder: GpuEncoder,
    attention: GpuMemoryAttention,
    memory_encoder: GpuMemoryEncoder,
    ok: AtomicBool,
    adapter: String,
}

/// The weights of everything the video tracker needs.
pub struct VideoModel {
    gpu: Option<GpuParts>,
    pub image: ImageModel,
    memory_encoder: MemoryEncoder,
    memory_attention: MemoryAttention,
    temporal_pos: Vec<f32>,
    no_object_pointer: Tensor,
    pointer_proj: FeedForward,
    pointer_time_proj: Linear,
    /// `mask_downsample`: a 4 x 4 stride-4 convolution taking a mask prompt to the prompt encoder's resolution.
    mask_down: (Tensor, Tensor),
}

impl VideoModel {
    /// On the CPU: the reference path every test compares against.
    pub fn load(w: &Weights) -> Result<VideoModel> {
        VideoModel::load_with(w, Backend::Cpu)
    }

    /// On the GPU when there is one (see [`Backend::Auto`]).
    pub fn load_auto(w: &Weights) -> Result<VideoModel> {
        VideoModel::load_with(w, Backend::Auto)
    }

    pub fn load_with(w: &Weights, backend: Backend) -> Result<VideoModel> {
        let backend = match (backend, std::env::var("EFFECTCRAFT_SAM2_DEVICE").ok().as_deref()) {
            (Backend::Auto, Some("cpu")) => Backend::Cpu,
            (Backend::Auto, Some("gpu")) => Backend::Gpu,
            (b, _) => b,
        };
        let image = ImageModel::load(w)?;
        let gpu = match backend {
            Backend::Cpu => None,
            Backend::Gpu => Some(Self::load_gpu(w, &image)?),
            Backend::Auto => match Self::load_gpu(w, &image) {
                Ok(g) => Some(g),
                Err(e) => {
                    log::info!("SAM 2 runs on the CPU: {e}");
                    None
                }
            },
        };
        Ok(VideoModel {
            gpu,
            image,
            memory_encoder: MemoryEncoder::load(w)?,
            memory_attention: MemoryAttention::load(w)?,
            temporal_pos: w.get("memory_temporal_positional_encoding", &[MEMORY_FRAMES, 1, 1, 64])?.flatten_all()?.to_vec1::<f32>()?,
            no_object_pointer: w.get("no_object_pointer", &[1, 256])?,
            pointer_proj: FeedForward::load(w, "object_pointer_proj", 256, 256, 256, 3, false)?,
            pointer_time_proj: linear(w, "temporal_positional_encoding_projection_layer", 256, 64)?,
            mask_down: (w.get("mask_downsample.weight", &[1, 1, 4, 4])?, w.get("mask_downsample.bias", &[1])?),
        })
    }

    fn load_gpu(w: &Weights, image: &ImageModel) -> Result<GpuParts> {
        let gpu = Gpu::shared()?;
        Ok(GpuParts {
            encoder: GpuEncoder::load(gpu.clone(), w, image.vision.hiera.clone())?,
            attention: GpuMemoryAttention::load(gpu.clone(), w)?,
            memory_encoder: GpuMemoryEncoder::load(gpu.clone(), w)?,
            ok: AtomicBool::new(true),
            adapter: gpu.adapter_name().to_string(),
        })
    }

    /// The device the heavy parts run on now: the GPU's name, or "CPU".
    pub fn device_name(&self) -> String {
        match &self.gpu {
            Some(g) if g.ok.load(Ordering::Relaxed) => g.adapter.clone(),
            _ => "CPU".to_string(),
        }
    }

    /// Run on the CPU from now on (a GPU that misbehaves, or the user's choice). Weights and results are
    /// unchanged; only the device differs.
    pub fn disable_gpu(&self) {
        if let Some(g) = &self.gpu {
            g.ok.store(false, Ordering::Relaxed);
        }
    }

    fn gpu_failed(&self, what: &str, e: &Error) {
        if let Some(g) = &self.gpu
            && g.ok.swap(false, Ordering::Relaxed)
        {
            log::warn!("SAM 2 {what} failed on the GPU ({e}); the CPU takes over");
        }
    }

    /// Encode a normalised `[1, 3, 1024, 1024]` frame on the GPU if it is working, else on the CPU.
    pub fn encode(&self, pixels: &Tensor) -> Result<Encoded> {
        if let Some(g) = self.gpu.as_ref().filter(|g| g.ok.load(Ordering::Relaxed)) {
            match g.encoder.encode(pixels) {
                Ok(f) => {
                    let none = || Tensor::zeros((1, 0, 0, 0), DType::F32, &candle_core::Device::Cpu);
                    let pos = self.image.vision.neck.position(GRID, GRID)?;
                    let features = ImageFeatures { features: vec![none()?, none()?, f.low], positions: vec![none()?, none()?, pos] };
                    return Ok(Encoded { features, high: f.high });
                }
                Err(e) => self.gpu_failed("encoding", &e),
            }
        }
        self.image.encode(pixels)
    }

    /// The memory of a frame (its features and the scaled mask), on the GPU if it is working, else on the CPU.
    fn memorize(&self, features: &Tensor, mask: &Tensor, present: bool) -> Result<Memory> {
        if let Some(g) = self.gpu.as_ref().filter(|g| g.ok.load(Ordering::Relaxed)) {
            match g.memory_encoder.encode(features, mask).and_then(|t| self.memory_encoder.finish(&t, present)) {
                Ok(m) => return Ok(m),
                Err(e) => self.gpu_failed("memory encoding", &e),
            }
        }
        self.memory_encoder.memory(features, mask, present)
    }

    /// Memory attention on the GPU if it is working, else on the CPU.
    fn attend(&self, current: &Tensor, current_pos: &Tensor, memory: &Tensor, memory_pos: &Tensor, pointers: usize) -> Result<Tensor> {
        if let Some(g) = self.gpu.as_ref().filter(|g| g.ok.load(Ordering::Relaxed)) {
            match g.attention.forward(current, current_pos, memory, memory_pos, pointers) {
                Ok(t) => return Ok(t),
                Err(Error::Budget(reason)) => log::info!("SAM 2 memory attention on the CPU for this call: {reason}"),
                Err(e) => self.gpu_failed("memory attention", &e),
            }
        }
        self.memory_attention.forward(current, current_pos, memory, memory_pos, pointers)
    }

    pub fn open(path: &Path) -> Result<VideoModel> {
        VideoModel::load(&Weights::open(path)?)
    }
}

/// What the tracker keeps for a frame it has processed.
struct FrameOut {
    memory: Memory,
    /// `[1, 256]`
    pointer: Tensor,
    /// The mask of a conditioning frame (returned again, unchanged, when propagation passes it).
    result: Option<FrameMask>,
}

#[derive(Default)]
struct ObjectState {
    cond: BTreeMap<usize, FrameOut>,
    non_cond: BTreeMap<usize, FrameOut>,
}

struct FrameFeatures {
    /// Lowest-resolution level `[1, 256, 64, 64]` and its position encoding.
    low: Tensor,
    pos: Tensor,
    high: (Tensor, Tensor),
}

/// What a tracker may hold and take. A call that would exceed it stops with [`Error::Budget`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Budget {
    /// Memory banks, results and cached frame features (bytes).
    pub max_state_bytes: usize,
    /// Wall-clock seconds one `propagate` call may take (`None`: no limit).
    pub max_seconds: Option<f64>,
}

impl Default for Budget {
    fn default() -> Budget {
        Budget { max_state_bytes: 2 << 30, max_seconds: None }
    }
}

/// Bytes of one stored frame: memory features and position encoding `[4096, 64]`, the pointer.
const FRAME_BYTES: usize = 2 * GRID * GRID * 64 * 4 + 256 * 4;
/// Bytes of a conditioning frame's kept result (`256 x 256` logits).
const RESULT_BYTES: usize = 4 * GRID * 4 * GRID * 4;
/// Bytes of one frame's cached neck features (lowest level and position, two skip levels).
const FEATURE_BYTES: usize = (2 * 256 * GRID * GRID + 32 * 16 * GRID * GRID + 64 * 4 * GRID * GRID) * 4;

pub struct VideoTracker<'a> {
    model: Arc<VideoModel>,
    frames: Box<dyn FrameSource + 'a>,
    objects: BTreeMap<ObjectId, ObjectState>,
    cache: VecDeque<(usize, Arc<FrameFeatures>)>,
    budget: Budget,
    peak: usize,
}

fn sine_embedding_1d(positions: &[f32], dim: usize) -> Result<Tensor> {
    let half = dim / 2;
    let dim_t: Vec<f32> = (0..half).map(|i| 10000f32.powf(2.0 * (i / 2) as f32 / half as f32)).collect();
    let mut out = Vec::with_capacity(positions.len() * dim);
    for p in positions {
        out.extend(dim_t.iter().map(|d| (p / d).sin()));
        out.extend(dim_t.iter().map(|d| (p / d).cos()));
    }
    Ok(Tensor::from_vec(out, (positions.len(), dim), &candle_core::Device::Cpu)?)
}

fn scalar(t: &Tensor) -> Result<f32> {
    Ok(t.flatten_all()?.to_vec1::<f32>()?.first().copied().unwrap_or(0.0))
}

impl<'a> VideoTracker<'a> {
    pub fn new(model: Arc<VideoModel>, frames: Box<dyn FrameSource + 'a>) -> Result<VideoTracker<'a>> {
        let n = frames.num_frames();
        if n == 0 || n > MAX_FRAMES {
            return Err(Error::Input(format!("a clip needs between 1 and {MAX_FRAMES} frames, got {n}")));
        }
        Ok(VideoTracker { model, frames, objects: BTreeMap::new(), cache: VecDeque::new(), budget: Budget::default(), peak: 0 })
    }

    pub fn num_frames(&self) -> usize {
        self.frames.num_frames()
    }

    /// Replace the limits (see [`Budget`]).
    pub fn set_budget(&mut self, budget: Budget) {
        self.budget = budget;
    }

    /// Bytes held now: stored frames, kept results and cached features.
    pub fn state_bytes(&self) -> usize {
        let frames: usize = self.objects.values().map(|o| (o.cond.len() + o.non_cond.len()) * FRAME_BYTES + o.cond.len() * RESULT_BYTES).sum();
        frames + self.cache.len() * FEATURE_BYTES
    }

    /// The most [`VideoTracker::state_bytes`] reached so far.
    pub fn peak_state_bytes(&self) -> usize {
        self.peak
    }

    fn check_budget(&mut self) -> Result<()> {
        let used = self.state_bytes();
        self.peak = self.peak.max(used);
        if used > self.budget.max_state_bytes {
            return Err(Error::Budget(format!("the tracker holds {} MB, over its {} MB limit", used >> 20, self.budget.max_state_bytes >> 20)));
        }
        Ok(())
    }

    /// Forget every object and cached frame (the clip stays).
    pub fn reset(&mut self) {
        self.objects.clear();
        self.cache.clear();
    }

    fn features(&mut self, frame: usize) -> Result<Arc<FrameFeatures>> {
        if let Some((_, f)) = self.cache.iter().find(|(i, _)| *i == frame) {
            return Ok(f.clone());
        }
        let pixels = self.frames.frame(frame)?;
        let enc = self.model.encode(&pixels)?;
        let (low, pos) = match (enc.features.features.get(2), enc.features.positions.get(2)) {
            (Some(l), Some(p)) => (l.clone(), p.clone()),
            _ => return Err(Error::Compute("vision encoder returned too few levels".into())),
        };
        let f = Arc::new(FrameFeatures { low, pos, high: enc.high });
        self.cache.push_back((frame, f.clone()));
        while self.cache.len() > 2 {
            self.cache.pop_front();
        }
        Ok(f)
    }

    /// The memory bank for `frame`: memory frames and object pointers with their position
    /// encodings, plus how many trailing tokens are pointers.
    fn memory_bank(&self, obj: &ObjectState, frame: usize, reverse: bool) -> Result<(Tensor, Tensor, usize)> {
        let (mut feats, mut poss) = (vec![], vec![]);
        let temporal = |off: usize| -> Result<Tensor> {
            // The reference indexes with `offset - 1`, so the conditioning frames (offset 0) use the last row.
            let row = if off == 0 { MEMORY_FRAMES - 1 } else { off - 1 };
            let v = self.model.temporal_pos.get(row * 64..(row + 1) * 64).ok_or_else(|| Error::Weights("temporal position table".into()))?;
            Ok(Tensor::from_slice(v, (1, 64), &candle_core::Device::Cpu)?)
        };
        for out in obj.cond.values() {
            feats.push(out.memory.features.clone());
            poss.push(out.memory.pos.broadcast_add(&temporal(0)?)?);
        }
        for off in (1..MEMORY_FRAMES).rev() {
            let prev = if reverse { frame.checked_add(off) } else { frame.checked_sub(off) };
            if let Some(out) = prev.and_then(|p| obj.non_cond.get(&p)) {
                feats.push(out.memory.features.clone());
                poss.push(out.memory.pos.broadcast_add(&temporal(off)?)?);
            }
        }
        // Object pointers: conditioning frames on the already-tracked side, then recent frames.
        let n_frames = self.frames.num_frames();
        let max_ptrs = n_frames.min(MAX_POINTERS);
        let (mut offsets, mut pointers): (Vec<f32>, Vec<Tensor>) = (vec![], vec![]);
        let sign = if reverse { -1.0f32 } else { 1.0 };
        for (t, out) in &obj.cond {
            if if reverse { *t >= frame } else { *t <= frame } {
                offsets.push((frame as f32 - *t as f32) * sign);
                pointers.push(out.pointer.clone());
            }
        }
        for off in 1..max_ptrs {
            let r = if reverse { frame.checked_add(off) } else { frame.checked_sub(off) };
            let Some(r) = r.filter(|r| *r < n_frames) else { break };
            if let Some(out) = obj.non_cond.get(&r) {
                offsets.push(off as f32);
                pointers.push(out.pointer.clone());
            }
        }
        let mut n_pointer_tokens = 0;
        if !pointers.is_empty() {
            let max_diff = (max_ptrs.saturating_sub(1)).max(1) as f32;
            let norm: Vec<f32> = offsets.iter().map(|o| o / max_diff).collect();
            let pe = apply(&self.model.pointer_time_proj, &sine_embedding_1d(&norm, 256)?)?;
            let ptrs = Tensor::cat(&pointers, 0)?; // [P, 256]
            let p = ptrs.dim(0)?;
            let tokens = ptrs.reshape((p * 4, 64))?;
            let pe = pe.unsqueeze(1)?.broadcast_as((p, 4, 64))?.reshape((p * 4, 64))?;
            n_pointer_tokens = p * 4;
            feats.push(tokens);
            poss.push(pe.contiguous()?);
        }
        if feats.is_empty() {
            return Err(Error::Input("the object has no conditioning frame; add a prompt first".into()));
        }
        Ok((Tensor::cat(&feats, 0)?, Tensor::cat(&poss, 0)?, n_pointer_tokens))
    }

    /// Run the model on one frame for one object and store the result.
    fn step(&mut self, id: ObjectId, frame: usize, prompt: Option<&Prompt>, reverse: bool, conditioning: bool) -> Result<FrameMask> {
        // EFFECTCRAFT_SAM2_PROFILE=1 prints where a frame's time goes (stderr).
        let profile = std::env::var_os("EFFECTCRAFT_SAM2_PROFILE").is_some();
        let mut lap = std::time::Instant::now();
        let mut mark = |what: &str| {
            if profile {
                eprintln!("sam2 frame {frame}: {what} {:.0} ms", lap.elapsed().as_secs_f64() * 1e3);
            }
            lap = std::time::Instant::now();
        };
        let feats = self.features(frame)?;
        mark("encode");
        let model = self.model.clone();
        let tokens = |t: &Tensor| -> Result<Tensor> { Ok(t.flatten_from(2)?.squeeze(0)?.t()?.contiguous()?) };
        let conditioned = if conditioning {
            feats.low.broadcast_add(model.image.no_memory())?
        } else {
            let obj = self.objects.get(&id).ok_or_else(|| Error::Input("unknown object".into()))?;
            let (mem, mem_pos, n_ptr) = self.memory_bank(obj, frame, reverse)?;
            let out = model.attend(&tokens(&feats.low)?, &tokens(&feats.pos)?, &mem, &mem_pos, n_ptr)?;
            out.t()?.contiguous()?.reshape((1, 256, GRID, GRID))?
        };
        mark("memory attention");
        let num_points = prompt.map_or(0, |p| p.points.len() + 2 * usize::from(p.bbox.is_some()));
        let multimask = num_points <= 1;
        let sparse = match prompt {
            Some(p) => model.image.prompt.sparse(p)?,
            None => model.image.prompt.sparse_none()?,
        };
        let dense = model.image.prompt.dense_empty()?;
        let out = model.image.decode_embedded(&conditioned, &feats.high, &sparse, &dense, multimask)?;

        mark("decoder");
        let score = scalar(&out.object_score)?;
        let present = score > 0.0;
        let ious = out.iou.flatten_all()?.to_vec1::<f32>()?;
        let best = if multimask { ious.iter().enumerate().fold((0usize, f32::NEG_INFINITY), |b, (i, v)| if *v > b.1 { (i, *v) } else { b }).0 } else { 0 };
        let low = out.masks.narrow(1, best, 1)?;
        let low = if present { low } else { Tensor::full(NO_OBJECT, low.dims(), low.device())? };
        let token = if multimask { out.sam_tokens.narrow(1, best, 1)? } else { out.sam_tokens.narrow(1, 0, 1)? }.squeeze(1)?;
        let pointer = if present { model.pointer_proj.forward(&token)? } else { model.no_object_pointer.clone() };

        let high = low.upsample_bilinear2d(INPUT_SIZE, INPUT_SIZE, false)?;
        let for_memory = if prompt.is_some() { high.gt(0f32)?.to_dtype(DType::F32)? } else { sigmoid(&high)? };
        let for_memory = ((for_memory * 20.0)? - 10.0)?;
        mark("upsample and mask");
        let memory = model.memorize(&feats.low, &for_memory, present)?;
        mark("memory encoder");

        let iou = ious.get(best).copied().unwrap_or(0.0);
        let result = FrameMask { frame, object: id, logits: low.flatten_all()?.to_vec1::<f32>()?, iou, object_score: score, occluded: !present };
        let state = self.objects.entry(id).or_default();
        let entry = FrameOut { memory, pointer, result: conditioning.then(|| result.clone()) };
        if conditioning {
            state.non_cond.remove(&frame);
            state.cond.insert(frame, entry);
        } else {
            state.non_cond.insert(frame, entry);
            Self::prune(state, frame);
        }
        self.check_budget()?;
        Ok(result)
    }

    /// Drop memories that no later frame in either direction can still look at.
    fn prune(state: &mut ObjectState, frame: usize) {
        let keep = |f: usize, cond: &BTreeMap<usize, FrameOut>| f.abs_diff(frame) <= MAX_POINTERS || cond.keys().any(|c| f.abs_diff(*c) <= MEMORY_FRAMES);
        let doomed: Vec<usize> = state.non_cond.keys().copied().filter(|f| !keep(*f, &state.cond)).collect();
        for f in doomed {
            state.non_cond.remove(&f);
        }
    }

    fn check_frame(&self, frame: usize) -> Result<()> {
        if frame >= self.frames.num_frames() {
            return Err(Error::Input(format!("frame {frame} is outside the clip of {} frames", self.frames.num_frames())));
        }
        Ok(())
    }

    /// Prompt an object on a frame (clicks and/or a box). The frame becomes one of the object's
    /// conditioning frames when it has not been tracked yet; the mask comes back at once.
    pub fn prompt(&mut self, frame: usize, object: ObjectId, prompt: &Prompt) -> Result<FrameMask> {
        self.check_frame(frame)?;
        prompt.validate()?;
        if prompt.points.is_empty() && prompt.bbox.is_none() {
            return Err(Error::Input("a prompt needs at least one point or a box".into()));
        }
        let tracked = self.objects.get(&object).is_some_and(|o| o.non_cond.contains_key(&frame));
        if tracked {
            // A correction on a tracked frame is conditioned on the memory like any tracked frame.
            let reverse = false;
            return self.step(object, frame, Some(prompt), reverse, false);
        }
        self.objects.entry(object).or_default();
        self.step(object, frame, Some(prompt), false, true)
    }

    /// Prompt an object on a frame with a mask: `[1, 1, 1024, 1024]` of 0 and 1 (see
    /// [`crate::resize::mask_to_model`]). The mask becomes the frame's output as it is (no decode of
    /// the mask itself); the decoder only makes the object pointer. Like a click prompt, the frame is a
    /// conditioning frame unless it has been tracked already.
    pub fn prompt_mask(&mut self, frame: usize, object: ObjectId, mask: &Tensor) -> Result<FrameMask> {
        self.check_frame(frame)?;
        if mask.dims() != [1, 1, INPUT_SIZE, INPUT_SIZE] {
            return Err(Error::Input(format!("a mask prompt must be [1, 1, {INPUT_SIZE}, {INPUT_SIZE}], got {:?}", mask.dims())));
        }
        let values = mask.flatten_all()?.to_vec1::<f32>()?;
        if values.iter().any(|v| !v.is_finite()) {
            return Err(Error::Input("a mask prompt must be finite".into()));
        }
        let tracked = self.objects.get(&object).is_some_and(|o| o.non_cond.contains_key(&frame));
        self.objects.entry(object).or_default();
        self.step_mask(object, frame, mask, &values, !tracked)
    }

    /// The reference's `_use_mask_as_output`: the mask is the output, the decoder makes the pointer.
    fn step_mask(&mut self, id: ObjectId, frame: usize, mask: &Tensor, values: &[f32], conditioning: bool) -> Result<FrameMask> {
        let feats = self.features(frame)?;
        let model = self.model.clone();
        let present = values.iter().any(|v| *v > 0.0);
        let high = ((mask * 20.0)? - 10.0)?;
        let low = antialias_bilinear(&high.flatten_all()?.to_vec1::<f32>()?, (INPUT_SIZE, INPUT_SIZE), (4 * GRID, 4 * GRID))?;
        // The pointer: the decoder with the mask as its dense prompt and no clicks, on the frame's features
        // without the no-memory embedding.
        let down = mask.conv2d(&model.mask_down.0, 0, 4, 1, 1)?.broadcast_add(&model.mask_down.1.reshape((1, 1, 1, 1))?)?;
        let dense = model.image.prompt.dense_mask(&down)?;
        let out = model.image.decode_embedded(&feats.low, &feats.high, &model.image.prompt.sparse_none()?, &dense, true)?;
        let ious = out.iou.flatten_all()?.to_vec1::<f32>()?;
        let best = ious.iter().enumerate().fold((0usize, f32::NEG_INFINITY), |b, (i, v)| if *v > b.1 { (i, *v) } else { b }).0;
        let token = out.sam_tokens.narrow(1, best, 1)?.squeeze(1)?;
        let decoder_present = scalar(&out.object_score)? > 0.0;
        let pointer = if decoder_present { model.pointer_proj.forward(&token)? } else { model.no_object_pointer.clone() };
        let pointer = if present { pointer } else { model.no_object_pointer.clone() };
        let memory = model.memorize(&feats.low, &high, present)?;
        let score = if present { 10.0 } else { -10.0 };
        let result = FrameMask { frame, object: id, logits: low, iou: 1.0, object_score: score, occluded: !present };
        let state = self.objects.entry(id).or_default();
        let entry = FrameOut { memory, pointer, result: conditioning.then(|| result.clone()) };
        if conditioning {
            state.non_cond.remove(&frame);
            state.cond.insert(frame, entry);
        } else {
            state.non_cond.insert(frame, entry);
            Self::prune(state, frame);
        }
        self.check_budget()?;
        Ok(result)
    }

    /// Track every object from `start` through `max_frames` frames (the rest of the clip when
    /// `None`) in `direction`, calling `out` for each object on each frame as it is finished.
    pub fn propagate(&mut self, start: usize, direction: Direction, max_frames: Option<usize>, cancel: &Cancel, out: &mut dyn FnMut(FrameMask)) -> Result<()> {
        self.propagate_objects(None, start, direction, max_frames, cancel, out)
    }

    /// [`VideoTracker::propagate`] for the listed objects only (`None`: every prompted object).
    pub fn propagate_objects(
        &mut self,
        only: Option<&[ObjectId]>,
        start: usize,
        direction: Direction,
        max_frames: Option<usize>,
        cancel: &Cancel,
        out: &mut dyn FnMut(FrameMask),
    ) -> Result<()> {
        self.check_frame(start)?;
        let n = self.frames.num_frames();
        let reverse = direction == Direction::Backward;
        let span = max_frames.unwrap_or(n).min(n);
        let ids: Vec<ObjectId> = self.objects.iter().filter(|(id, o)| !o.cond.is_empty() && only.is_none_or(|l| l.contains(id))).map(|(id, _)| *id).collect();
        if ids.is_empty() {
            return Err(Error::Input("no object has been prompted".into()));
        }
        let started = std::time::Instant::now();
        for k in 0..=span {
            let frame = if reverse { start.checked_sub(k) } else { Some(start + k).filter(|f| *f < n) };
            let Some(frame) = frame else { break };
            for id in &ids {
                if cancel.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                if let Some(limit) = self.budget.max_seconds.filter(|l| started.elapsed().as_secs_f64() > *l) {
                    return Err(Error::Budget(format!("tracking took more than {limit} s")));
                }
                let mask = self.mask_at(*id, frame, reverse)?;
                out(mask);
            }
        }
        Ok(())
    }

    /// Object `id` on `frame`: the result stored with its prompt there, else the tracker's step.
    fn mask_at(&mut self, id: ObjectId, frame: usize, reverse: bool) -> Result<FrameMask> {
        let stored = self.objects.get(&id).and_then(|o| o.cond.get(&frame)).and_then(|o| o.result.clone());
        match stored {
            Some(m) => Ok(m),
            None => self.step(id, frame, None, reverse, false),
        }
    }

    /// Track every listed object from its own prompted frame (`starts`: object, frame) in one pass
    /// in `direction`, frame by frame with all the objects that have started: a frame is finished
    /// for all of them before the next begins, so a caller that assembles the objects of a frame
    /// does not hold frames waiting for a pass that has not got there. A backward pass leaves out
    /// each object's own start frame (the forward pass reported it).
    pub fn propagate_each(&mut self, starts: &[(ObjectId, usize)], direction: Direction, cancel: &Cancel, out: &mut dyn FnMut(FrameMask)) -> Result<()> {
        let n = self.frames.num_frames();
        for (id, start) in starts {
            self.check_frame(*start)?;
            if self.objects.get(id).is_none_or(|o| o.cond.is_empty()) {
                return Err(Error::Input("no object has been prompted".into()));
            }
        }
        if starts.is_empty() {
            return Err(Error::Input("no object has been prompted".into()));
        }
        let reverse = direction == Direction::Backward;
        let started = std::time::Instant::now();
        for (id, frame) in pass_order(starts, n, reverse) {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            if let Some(limit) = self.budget.max_seconds.filter(|l| started.elapsed().as_secs_f64() > *l) {
                return Err(Error::Budget(format!("tracking took more than {limit} s")));
            }
            let mask = self.mask_at(id, frame, reverse)?;
            out(mask);
        }
        Ok(())
    }
}

/// The (object, frame) steps of [`VideoTracker::propagate_each`], in order: frame by frame from the
/// first start (the last, backwards), each frame for the objects whose pass has reached it.
fn pass_order(starts: &[(ObjectId, usize)], frames: usize, reverse: bool) -> impl Iterator<Item = (ObjectId, usize)> + '_ {
    let first = if reverse { starts.iter().map(|s| s.1).max() } else { starts.iter().map(|s| s.1).min() };
    let span = match first {
        Some(f) if reverse => f + 1,
        Some(f) => frames.saturating_sub(f),
        None => 0,
    };
    (0..span).flat_map(move |k| {
        let frame = if reverse { first.unwrap_or(0).saturating_sub(k) } else { first.unwrap_or(0) + k };
        starts.iter().filter_map(move |(id, start)| {
            let reached = if reverse { *start > frame } else { *start <= frame };
            reached.then_some((*id, frame))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(starts: &[(u32, usize)], frames: usize, reverse: bool) -> Vec<(u32, usize)> {
        let starts: Vec<(ObjectId, usize)> = starts.iter().map(|(o, s)| (ObjectId(*o), *s)).collect();
        pass_order(&starts, frames, reverse).map(|(o, f)| (o.0, f)).collect()
    }

    /// Forward, every frame is finished for all the objects that have started before the next frame
    /// begins; an object joins the pass at its own start frame.
    #[test]
    fn the_forward_pass_goes_frame_by_frame_over_the_objects_that_have_started() {
        assert_eq!(order(&[(1, 0), (2, 3)], 6, false), [(1, 0), (1, 1), (1, 2), (1, 3), (2, 3), (1, 4), (2, 4), (1, 5), (2, 5)]);
        // Prompts on the same frame go together; the pass starts at the earliest start.
        assert_eq!(order(&[(1, 2), (2, 2)], 4, false), [(1, 2), (2, 2), (1, 3), (2, 3)]);
        assert_eq!(order(&[(1, 2)], 4, false), [(1, 2), (1, 3)]);
    }

    /// Backward, from the last start down to frame 0, leaving out each object's own start frame.
    #[test]
    fn the_backward_pass_goes_frame_by_frame_and_skips_each_objects_own_start() {
        assert_eq!(order(&[(1, 0), (2, 3)], 6, true), [(2, 2), (2, 1), (2, 0)]);
        assert_eq!(order(&[(1, 2), (2, 3)], 6, true), [(2, 2), (1, 1), (2, 1), (1, 0), (2, 0)]);
        assert!(order(&[(1, 0)], 6, true).is_empty(), "nothing before frame 0");
    }

    /// A frame is finished for every object before the next begins, however far apart the prompts
    /// are: the number of frames waiting for an object never grows with the clip.
    #[test]
    fn no_frame_waits_for_an_object_that_has_not_reached_it() {
        let steps = order(&[(1, 0), (2, 1000)], 2000, false);
        let mut waiting = 0usize;
        let mut worst = 0usize;
        let mut seen = std::collections::BTreeMap::<usize, usize>::new();
        for (_, f) in steps {
            let n = seen.entry(f).or_default();
            *n += 1;
            // From frame 1000 on, both objects report each frame back to back.
            if f >= 1000 && *n == 2 {
                waiting -= 1;
            } else if f >= 1000 {
                waiting += 1;
            }
            worst = worst.max(waiting);
        }
        assert!(worst <= 1, "{worst} frames waiting");
    }
}
