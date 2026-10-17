// Modified work: adapts the ported SAM 2 video tracker to EffectCraft's `ObjectTracker`
// interface (Apache-2.0, see NOTICE).
//! SAM 2 as the Roto Brush's object tracker: prompts on some frames, the whole clip tracked
//! forwards and backwards, each frame reported as it is finished.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use candle_core::Tensor;
use effectcraft_segment::{ClipFrame, ObjectTracker, TrackRequest, TrackedFrame};

use crate::mask::{Sam2Mask, preprocess};
use crate::prompt::{INPUT_SIZE, Prompt};
use crate::resize::mask_to_model;
use crate::session::{Cancel, Direction, FrameMask, FrameSource, ObjectId, VideoTracker};
use crate::{Error, Result};

/// Most frames one tracking request may cover.
const MAX_CLIP: usize = 100_000;

type Frames<'a> = &'a mut dyn FnMut(i64) -> Option<Arc<ClipFrame>>;

/// Feeds the host's frames to the tracker, remembering each frame's size for the output.
struct Source<'a> {
    first: i64,
    count: usize,
    fetch: RefCell<Frames<'a>>,
    sizes: Rc<RefCell<HashMap<usize, (usize, usize)>>>,
}

impl FrameSource for Source<'_> {
    fn num_frames(&self) -> usize {
        self.count
    }

    fn frame(&self, index: usize) -> Result<Tensor> {
        let f = (self.fetch.borrow_mut())(self.first + index as i64)
            .ok_or_else(|| Error::Input(format!("frame {} is not available", self.first + index as i64)))?;
        self.sizes.borrow_mut().insert(index, (f.w, f.h));
        preprocess(&f.rgb, f.w, f.h)
    }
}

fn to_frame(m: &FrameMask, size: (usize, usize)) -> Result<TrackedFrame> {
    let (w, h) = size;
    let logits = Tensor::from_slice(&m.logits, (1, 1, 256, 256), &candle_core::Device::Cpu)?;
    let prob = candle_nn::ops::sigmoid(&logits.upsample_bilinear2d(h, w, false)?)?.flatten_all()?.to_vec1::<f32>()?;
    let presence = 1.0 / (1.0 + (-m.object_score).exp());
    Ok(TrackedFrame { prob, w, h, confidence: (presence * m.iou.clamp(0.0, 1.0)).clamp(0.0, 1.0), occluded: m.occluded })
}

/// Most objects one request may track.
const MAX_OBJECTS: usize = 32;

impl Sam2Mask {
    fn run_track(&self, req: &TrackRequest, frames: Frames<'_>, cancel: &dyn Fn() -> bool, out: &mut dyn FnMut(usize, i64, TrackedFrame)) -> Result<()> {
        if req.count == 0 || req.count > MAX_CLIP {
            return Err(Error::Input(format!("a tracking request needs 1 to {MAX_CLIP} frames, got {}", req.count)));
        }
        if req.objects.is_empty() || req.objects.len() > MAX_OBJECTS || req.objects.iter().any(|o| o.is_empty()) {
            return Err(Error::Input(format!("tracking needs 1 to {MAX_OBJECTS} objects, each with a prompt on at least one frame")));
        }
        let sizes = Rc::new(RefCell::new(HashMap::new()));
        let source = Source { first: req.first, count: req.count, fetch: RefCell::new(frames), sizes: sizes.clone() };
        // Prompt coordinates are in the frame's pixels and the model works at 1024 x 1024, so the
        // prompted frames' sizes are needed first.
        let mut prompt_frames = vec![];
        for (oi, prompts) in req.objects.iter().enumerate() {
            for (frame, p) in prompts {
                let idx = usize::try_from(frame - req.first)
                    .ok()
                    .filter(|i| *i < req.count)
                    .ok_or_else(|| Error::Input(format!("prompt frame {frame} is outside the clip")))?;
                if !sizes.borrow().contains_key(&idx) {
                    let f = (source.fetch.borrow_mut())(*frame).ok_or_else(|| Error::Input(format!("frame {frame} is not available")))?;
                    sizes.borrow_mut().insert(idx, (f.w, f.h));
                }
                prompt_frames.push((oi, idx, p));
            }
        }
        let mut tracker = VideoTracker::new(self.video_model().clone(), Box::new(source))?;
        let object_id = |oi: usize| ObjectId(oi as u32 + 1);
        // each object's first prompted frame: its pass starts there, forwards and backwards
        let mut starts: std::collections::BTreeMap<usize, Vec<ObjectId>> = Default::default();
        let mut first_of: HashMap<usize, usize> = HashMap::new();
        for (oi, idx, p) in prompt_frames {
            if cancel() {
                return Err(Error::Cancelled);
            }
            let (w, h) = sizes.borrow().get(&idx).copied().ok_or_else(|| Error::Compute("frame size unknown".into()))?;
            // A prompt that is only a mask (probability per pixel of the frame) prompts with the mask.
            if let (true, true, Some(m)) = (p.points.is_empty(), p.bbox.is_none(), p.mask.as_ref()) {
                if m.len() != w * h {
                    return Err(Error::Input(format!("a mask prompt of {} values does not fit a {w}x{h} frame", m.len())));
                }
                let binary: Vec<u8> = m.iter().map(|v| u8::from(*v >= 0.5)).collect();
                tracker.prompt_mask(idx, object_id(oi), &mask_to_model(&binary, w, h)?)?;
                let e = first_of.entry(oi).or_insert(idx);
                *e = (*e).min(idx);
                continue;
            }
            let (sx, sy) = (INPUT_SIZE as f32 / w as f32, INPUT_SIZE as f32 / h as f32);
            let prompt = Prompt {
                points: p.points.iter().map(|(c, fg)| ([c[0] * sx, c[1] * sy], *fg)).collect(),
                bbox: p.bbox.map(|b| [b[0] * sx, b[1] * sy, b[2] * sx, b[3] * sy]),
            };
            tracker.prompt(idx, object_id(oi), &prompt)?;
            let e = first_of.entry(oi).or_insert(idx);
            *e = (*e).min(idx);
        }
        for (oi, idx) in first_of {
            starts.entry(idx).or_default().push(object_id(oi));
        }
        let flag = Cancel::new();
        let mut failure: Option<Error> = None;
        let mut emit = |m: FrameMask| {
            if cancel() {
                flag.cancel();
            }
            let oi = (m.object.0 as usize).saturating_sub(1);
            match to_frame(&m, sizes.borrow().get(&m.frame).copied().unwrap_or((0, 0))) {
                Ok(t) => out(oi, req.first + m.frame as i64, t),
                Err(e) => {
                    failure.get_or_insert(e);
                    flag.cancel();
                }
            }
        };
        for (start, ids) in starts {
            tracker.propagate_objects(Some(&ids), start, Direction::Forward, None, &flag, &mut emit)?;
            if start > 0 {
                // the start frame was reported by the forward pass; the backward pass passes it again
                tracker.propagate_objects(Some(&ids), start, Direction::Backward, None, &flag, &mut |m| {
                    if m.frame != start {
                        emit(m);
                    }
                })?;
            }
        }
        self.peak.store(tracker.peak_state_bytes() as u64, std::sync::atomic::Ordering::Relaxed);
        failure.map_or(Ok(()), Err)
    }
}

impl ObjectTracker for Sam2Mask {
    fn track(
        &self,
        req: &TrackRequest,
        frames: &mut dyn FnMut(i64) -> Option<Arc<ClipFrame>>,
        cancel: &dyn Fn() -> bool,
        out: &mut dyn FnMut(usize, i64, TrackedFrame),
    ) -> effectcraft_segment::Result<()> {
        self.touch();
        let r = self.run_track(req, frames, cancel, out).map_err(|e| e.to_string());
        self.touch();
        r
    }
}
