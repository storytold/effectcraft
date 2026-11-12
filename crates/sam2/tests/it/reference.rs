//! Stage-by-stage comparison with the PyTorch / transformers reference (`tools/sam2_reference.py`).
//!
//! Needs the `facebook/sam2.1-hiera-tiny` weights (default `<repo>/sam2-tiny`, or the folder in
//! `EFFECTCRAFT_SAM2_DIR`) and the fixture tensors (`<repo>/plan/sam2-ref/image.safetensors`, or
//! `EFFECTCRAFT_SAM2_REF`). Without them every test here skips, so CI stays green.
#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashMap;
use std::path::PathBuf;

use candle_core::{Device, Tensor};
use effectcraft_sam2::hiera::{Hiera, HieraConfig};
use effectcraft_sam2::weights::Weights;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn weights_file() -> PathBuf {
    std::env::var_os("EFFECTCRAFT_SAM2_DIR").map_or_else(|| repo().join("sam2-tiny"), PathBuf::from).join("model.safetensors")
}

fn fixture_file() -> PathBuf {
    std::env::var_os("EFFECTCRAFT_SAM2_REF").map_or_else(|| repo().join("plan/sam2-ref/image.safetensors"), PathBuf::from)
}

/// The weights and the reference tensors, or `None` (the test then skips).
fn load() -> Option<(Weights, HashMap<String, Tensor>)> {
    let (wf, rf) = (weights_file(), fixture_file());
    if !wf.exists() || !rf.exists() {
        eprintln!("skipped: SAM 2 weights or reference tensors not found ({} / {})", wf.display(), rf.display());
        return None;
    }
    let w = Weights::open(&wf).unwrap();
    let r = candle_core::safetensors::load(&rf, &Device::Cpu).unwrap();
    note_size(&w);
    Some((w, r))
}

fn max_abs(a: &Tensor, b: &Tensor) -> f32 {
    assert_eq!(a.dims(), b.dims(), "shape mismatch");
    (a - b).unwrap().abs().unwrap().flatten_all().unwrap().max(0).unwrap().to_scalar::<f32>().unwrap()
}

/// The largest magnitude in a tensor.
fn peak(t: &Tensor) -> f32 {
    t.abs().unwrap().flatten_all().unwrap().max(0).unwrap().to_scalar::<f32>().unwrap()
}

/// Set when the checkpoint is Base+ or Large (24 and 48 blocks): f32 rounding error is relative and accumulates with
/// depth, and these backbones carry much larger activations (Large peaks at 140). Tiny and Small keep the strict bounds.
static DEEP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn note_size(w: &Weights) {
    let size = effectcraft_sam2::Size::detect(w).unwrap();
    DEEP.store(matches!(size, effectcraft_sam2::Size::BasePlus | effectcraft_sam2::Size::Large), std::sync::atomic::Ordering::Relaxed);
}

/// `base`, or for the deep backbones the larger of `base` and 5e-4 of the activations' peak.
fn tol(base: f32, want: &Tensor) -> f32 {
    if DEEP.load(std::sync::atomic::Ordering::Relaxed) { base.max(5e-4 * peak(want)) } else { base }
}

/// `base`, ×4 for the deep backbones.
fn loose(base: f32) -> f32 {
    if DEEP.load(std::sync::atomic::Ordering::Relaxed) { base * 4.0 } else { base }
}

/// f32 activations differ from PyTorch by accumulation order only; the Hiera blocks stay well
/// inside this bound (measured max 2.6e-5, 2026-10-15).
const HIERA_TOL: f32 = 1e-4;

#[test]
fn hiera_matches_pytorch_block_by_block() {
    let Some((w, r)) = load() else { return };
    let hiera = Hiera::load(&w, &HieraConfig::for_size(effectcraft_sam2::Size::detect(&w).unwrap())).unwrap();
    let pixels = r.get("input.pixel_values").unwrap();
    let (emb, blocks) = hiera.forward_all(pixels).unwrap();
    let e = max_abs(&emb, r.get("vision_encoder.backbone.patch_embed.0").unwrap());
    eprintln!("patch_embed max abs error {e:e}");
    assert!(e < 1e-4, "patch embed error {e}");
    assert_eq!(blocks.len(), HieraConfig::for_size(effectcraft_sam2::Size::detect(&w).unwrap()).blocks.iter().sum::<usize>());
    for (i, b) in blocks.iter().enumerate() {
        let want = r.get(&format!("vision_encoder.backbone.blocks.{i}.0")).unwrap();
        let e = max_abs(b, want);
        eprintln!("block {i}: max abs error {e:e}");
        let t = tol(HIERA_TOL, want);
        assert!(e < t, "block {i} error {e} (tolerance {t}, peak {})", peak(want));
    }
}

#[test]
fn hiera_is_deterministic_and_rejects_bad_sizes() {
    let Some((w, r)) = load() else { return };
    let hiera = Hiera::load(&w, &HieraConfig::for_size(effectcraft_sam2::Size::detect(&w).unwrap())).unwrap();
    let pixels = r.get("input.pixel_values").unwrap();
    let a = hiera.forward(pixels).unwrap();
    let b = hiera.forward(pixels).unwrap();
    assert_eq!(a.len(), 4);
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(max_abs(x, y), 0.0);
    }
    let odd = pixels.narrow(2, 0, 1022).unwrap();
    assert!(hiera.forward(&odd).is_err());
}

/// The neck adds 1x1 convolutions and nearest-neighbour sums to the Hiera output; the sine
/// position encodings are exact functions of the grid size.
const NECK_TOL: f32 = 1e-4;

#[test]
fn neck_and_vision_encoder_match_pytorch() {
    let Some((w, r)) = load() else { return };
    let enc = effectcraft_sam2::neck::VisionEncoder::load(&w).unwrap();
    let pixels = r.get("input.pixel_values").unwrap();
    let stages = enc.hiera.forward(pixels).unwrap();
    let out = enc.neck.forward(&stages).unwrap();
    assert_eq!((out.features.len(), out.positions.len()), (4, 4));
    for (i, f) in out.features.iter().enumerate() {
        let e = max_abs(f, r.get(&format!("vision_encoder.neck.{i}")).unwrap());
        eprintln!("neck feature {i}: max abs error {e:e}");
        let want = r.get(&format!("vision_encoder.neck.{i}")).unwrap();
        assert!(e < tol(NECK_TOL, want), "neck feature {i} error {e} (peak {})", peak(want));
    }
    for (i, p) in out.positions.iter().enumerate() {
        let e = max_abs(p, r.get(&format!("vision_encoder.neck.{}", i + 4)).unwrap());
        eprintln!("neck position {i}: max abs error {e:e}");
        assert!(e < NECK_TOL, "neck position {i} error {e}");
    }
    // The vision encoder keeps the three highest-resolution levels, highest first.
    let feats = enc.forward(pixels).unwrap();
    let dims: Vec<usize> = feats.features.iter().map(|f| f.dim(2).unwrap()).collect();
    assert_eq!(dims, vec![256, 128, 64]);
    let e = max_abs(feats.features.first().unwrap(), r.get("vision_encoder.neck.3").unwrap());
    assert!(e < tol(NECK_TOL, r.get("vision_encoder.neck.3").unwrap()));
}

/// Click-prompted single image: prompt embeddings and decoder outputs against `Sam2Model`.
const DECODER_TOL: f32 = 1e-3;

#[test]
fn prompt_encoder_and_decoder_match_pytorch() {
    use effectcraft_sam2::model::ImageModel;
    use effectcraft_sam2::prompt::Prompt;
    let Some((w, r)) = load() else { return };
    let model = ImageModel::load(&w).unwrap();
    let pixels = r.get("input.pixel_values").unwrap();
    let prompt = Prompt { points: vec![([460.0, 512.0], true)], bbox: None };

    let (sparse, dense) = model.embed_prompt(&prompt).unwrap();
    let want_sparse = r.get("prompt_encoder.0").unwrap().squeeze(0).unwrap();
    let e = max_abs(&sparse, &want_sparse);
    eprintln!("sparse prompt: max abs error {e:e}");
    assert!(e < 1e-5, "sparse prompt error {e}");
    let e = max_abs(&dense, r.get("prompt_encoder.1").unwrap());
    assert!(e < 1e-6, "dense prompt error {e}");

    let enc = model.encode(pixels).unwrap();
    let out = model.segment(&enc, &prompt, true).unwrap();
    let want_masks = r.get("mask_decoder.0").unwrap().squeeze(0).unwrap();
    let e = max_abs(&out.masks, &want_masks);
    eprintln!("masks: max abs error {e:e} (logit range {:?})", want_masks.flatten_all().unwrap().max(0).unwrap().to_scalar::<f32>());
    assert!(e < loose(DECODER_TOL), "mask logits error {e}");
    let e = max_abs(&out.iou, &r.get("mask_decoder.1").unwrap().squeeze(0).unwrap());
    eprintln!("iou: max abs error {e:e}");
    assert!(e < 1e-4, "iou error {e}");
    let e = max_abs(&out.object_score, &r.get("mask_decoder.3").unwrap().squeeze(0).unwrap());
    eprintln!("object score: max abs error {e:e}");
    assert!(e < 1e-4, "object score error {e}");
    let e = max_abs(&out.sam_tokens, &r.get("mask_decoder.2").unwrap().squeeze(0).unwrap());
    assert!(e < loose(1e-4), "sam token error {e}");
}

fn video_fixture_file() -> PathBuf {
    std::env::var_os("EFFECTCRAFT_SAM2_VIDEO_REF").map_or_else(|| repo().join("plan/sam2-ref/video.safetensors"), PathBuf::from)
}

/// The weights and the video reference tensors (`tools/sam2_video_reference.py`), or `None`.
fn load_video() -> Option<(Weights, HashMap<String, Tensor>)> {
    let (wf, rf) = (weights_file(), video_fixture_file());
    if !wf.exists() || !rf.exists() {
        eprintln!("skipped: SAM 2 weights or video reference tensors not found ({} / {})", wf.display(), rf.display());
        return None;
    }
    let w = Weights::open(&wf).unwrap();
    note_size(&w);
    Some((w, candle_core::safetensors::load(&rf, &Device::Cpu).unwrap()))
}

/// Measured max 3.5e-6 (memory features), 2.6e-5 (memory attention), 2026-10-15.
const MEMORY_TOL: f32 = 1e-4;

#[test]
fn memory_encoder_matches_pytorch() {
    let Some((w, r)) = load_video() else { return };
    let enc = effectcraft_sam2::memory::MemoryEncoder::load(&w).unwrap();
    let (feat, pos) = enc.encode(r.get("mem.enc.vision_features").unwrap(), r.get("mem.enc.masks").unwrap()).unwrap();
    let e = max_abs(&feat, r.get("mem.enc.out_features").unwrap());
    eprintln!("memory features: max abs error {e:e}");
    assert!(e < MEMORY_TOL, "memory feature error {e}");
    let e = max_abs(&pos, r.get("mem.enc.out_pos").unwrap());
    eprintln!("memory position encoding: max abs error {e:e}");
    assert!(e < 1e-5, "memory position error {e}");
}

#[test]
fn memory_attention_matches_pytorch() {
    let Some((w, r)) = load_video() else { return };
    let att = effectcraft_sam2::memattn::MemoryAttention::load(&w).unwrap();
    let get = |k: &str| r.get(k).unwrap().squeeze(1).unwrap();
    let pointers = r.get("mem.attn.num_pointers").unwrap().to_vec1::<f32>().unwrap()[0] as usize;
    let out = att
        .forward(
            &get("mem.attn.current_vision_features"),
            &get("mem.attn.current_vision_position_embeddings"),
            &get("mem.attn.memory"),
            &get("mem.attn.memory_posision_embeddings"),
            pointers,
        )
        .unwrap();
    let want = r.get("mem.attn.out").unwrap().squeeze(0).unwrap().squeeze(0).unwrap();
    let e = max_abs(&out, &want);
    eprintln!("memory attention: max abs error {e:e} (pointers {pointers})");
    assert!(e < MEMORY_TOL, "memory attention error {e}");
}

/// Frames of the video fixture, served one at a time.
struct Clip(Tensor);

impl effectcraft_sam2::session::FrameSource for Clip {
    fn num_frames(&self) -> usize {
        self.0.dim(0).unwrap()
    }
    fn frame(&self, i: usize) -> effectcraft_sam2::Result<Tensor> {
        Ok(self.0.narrow(0, i, 1)?.contiguous()?)
    }
}

fn iou(a: &[f32], b: &[f32]) -> f64 {
    let (mut i, mut u) = (0usize, 0usize);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (*x > 0.0, *y > 0.0);
        i += usize::from(x && y);
        u += usize::from(x || y);
    }
    if u == 0 { 1.0 } else { i as f64 / u as f64 }
}

/// Through the clip the memory bank feeds back on itself (bfloat16 memories, rotary attention), so
/// the bound on logits is looser than for a single stage; the masks themselves must agree.
/// Measured max 0.0038 over 9 frames forwards and backwards, 2026-10-15.
const VIDEO_LOGIT_TOL: f32 = 0.02;
const VIDEO_MIN_IOU: f64 = 0.995;

fn check_video_tracking(backend: effectcraft_sam2::session::Backend) {
    use effectcraft_sam2::prompt::Prompt;
    use effectcraft_sam2::session::{Cancel, Direction, ObjectId, VideoModel, VideoTracker};
    let Some((w, r)) = load_video() else { return };
    let model = std::sync::Arc::new(match VideoModel::load_with(&w, backend) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    });
    let clip = r.get("clip.pixel_values").unwrap().clone();
    let mut tracker = VideoTracker::new(model, Box::new(Clip(clip))).unwrap();
    let obj = ObjectId(1);
    let first = tracker.prompt(2, obj, &Prompt { points: vec![([416.0, 512.0], true)], bbox: None }).unwrap();

    let check = |tag: &str, m: &effectcraft_sam2::session::FrameMask| {
        let want = r.get(&format!("video.{tag}.{}.pred_masks", m.frame)).unwrap().flatten_all().unwrap().to_vec1::<f32>().unwrap();
        let want_score = r.get(&format!("video.{tag}.{}.score", m.frame)).unwrap().flatten_all().unwrap().to_vec1::<f32>().unwrap()[0];
        let err = m.logits.iter().zip(&want).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        let j = iou(&m.logits, &want);
        eprintln!("{tag} frame {}: logit error {err:.4}, IoU {j:.4}, score {:.3} (want {want_score:.3})", m.frame, m.object_score);
        assert!(j >= VIDEO_MIN_IOU, "{tag} frame {} IoU {j}", m.frame);
        assert!(err < VIDEO_LOGIT_TOL, "{tag} frame {} logit error {err}", m.frame);
        assert!((m.object_score - want_score).abs() < 0.05, "{tag} frame {} score {} vs {want_score}", m.frame, m.object_score);
        assert_eq!(m.occluded, want_score <= 0.0);
    };
    check("prompt", &first);
    let cancel = Cancel::new();
    let mut fwd = vec![];
    tracker.propagate(2, Direction::Forward, None, &cancel, &mut |m| fwd.push(m)).unwrap();
    assert_eq!(fwd.iter().map(|m| m.frame).collect::<Vec<_>>(), vec![2, 3, 4, 5, 6]);
    for m in &fwd {
        check("fwd", m);
    }
    let mut rev = vec![];
    tracker.propagate(2, Direction::Backward, None, &cancel, &mut |m| rev.push(m)).unwrap();
    assert_eq!(rev.iter().map(|m| m.frame).collect::<Vec<_>>(), vec![2, 1, 0]);
    for m in &rev {
        check("rev", m);
    }
}

#[test]
fn video_tracking_matches_sam2_video_model() {
    check_video_tracking(effectcraft_sam2::session::Backend::Cpu);
}

/// The same clip on the GPU: every frame's mask against `Sam2VideoModel`, forwards and backwards. The GPU path
/// is within 1e-5 of PyTorch stage by stage; through the clip the bfloat16 memories can round the other way
/// on a few values, so the bound on logits is the video one, and the masks must agree.
#[test]
fn video_tracking_on_the_gpu_matches_sam2_video_model() {
    check_video_tracking(effectcraft_sam2::session::Backend::Gpu);
}

#[test]
fn tracker_rejects_hostile_input_and_honours_cancel() {
    use effectcraft_sam2::Error;
    use effectcraft_sam2::prompt::Prompt;
    use effectcraft_sam2::session::{Cancel, Direction, ObjectId, VideoModel, VideoTracker};
    let Some((w, r)) = load_video() else { return };
    let model = std::sync::Arc::new(VideoModel::load(&w).unwrap());
    let clip = r.get("clip.pixel_values").unwrap().narrow(0, 0, 3).unwrap();
    let mut t = VideoTracker::new(model.clone(), Box::new(Clip(clip.clone()))).unwrap();
    let click = Prompt { points: vec![([416.0, 512.0], true)], bbox: None };
    let obj = ObjectId(7);
    // outside the clip, empty and non-finite prompts, nothing prompted yet
    assert!(matches!(t.prompt(3, obj, &click), Err(Error::Input(_))));
    assert!(matches!(t.prompt(0, obj, &Prompt::default()), Err(Error::Input(_))));
    assert!(matches!(t.prompt(0, obj, &Prompt { points: vec![([f32::NAN, 1.0], true)], bbox: None }), Err(Error::Input(_))));
    let mut sink = |_m| {};
    assert!(matches!(t.propagate(0, Direction::Forward, None, &Cancel::new(), &mut sink), Err(Error::Input(_))));
    assert!(matches!(t.propagate(9, Direction::Forward, None, &Cancel::new(), &mut sink), Err(Error::Input(_))));
    // an empty clip is refused; a cancelled run stops with Cancelled and keeps the tracker usable
    assert!(VideoTracker::new(model.clone(), Box::new(Clip(clip.narrow(0, 0, 0).unwrap()))).is_err());
    t.prompt(0, obj, &click).unwrap();
    let cancel = Cancel::new();
    cancel.cancel();
    assert!(matches!(t.propagate(0, Direction::Forward, None, &cancel, &mut sink), Err(Error::Cancelled)));
    let mut seen = vec![];
    t.propagate(0, Direction::Forward, Some(1), &Cancel::new(), &mut |m| seen.push(m.frame)).unwrap();
    assert_eq!(seen, vec![0, 1]);
    // a frame source that returns the wrong size is an error, not a crash
    struct Bad;
    impl effectcraft_sam2::session::FrameSource for Bad {
        fn num_frames(&self) -> usize {
            2
        }
        fn frame(&self, _i: usize) -> effectcraft_sam2::Result<Tensor> {
            Ok(Tensor::zeros((1, 3, 64, 64), candle_core::DType::F32, &Device::Cpu)?)
        }
    }
    let mut bad = VideoTracker::new(model, Box::new(Bad)).unwrap();
    assert!(matches!(bad.prompt(0, obj, &click), Err(Error::Input(_))));
}

#[test]
fn sam2_works_as_a_mask_model() {
    use effectcraft_segment::{MaskModel, Prompt};
    let Some((w, _)) = load() else { return };
    let (fw, fh) = (256usize, 192usize);
    let mut rgb = vec![[0f32; 3]; fw * fh];
    let inside = |x: usize, y: usize| ((x as f32 - 120.0).powi(2) + (y as f32 - 96.0).powi(2)).sqrt() < 50.0;
    for y in 0..fh {
        for x in 0..fw {
            let bg = 0.3 + 0.2 * (x as f32 / 9.0).sin() * (y as f32 / 7.0).cos();
            rgb[y * fw + x] = if inside(x, y) { [0.85 + 0.1 * (x as f32 / 5.0).sin(), 0.4, 0.15] } else { [bg, bg * 0.8 + 0.1, bg * 0.6 + 0.3] };
        }
    }
    let model = effectcraft_sam2::mask::Sam2Mask::new(std::sync::Arc::new(effectcraft_sam2::session::VideoModel::load(&w).unwrap()));
    let prompt = Prompt { points: vec![([120.0, 96.0], true)], bbox: None, mask: None };
    let p = model.segment(&rgb, fw, fh, &prompt).unwrap();
    assert_eq!(p.len(), fw * fh);
    assert!(p.iter().all(|v| (0.0..=1.0).contains(v)));
    let (mut i, mut u) = (0usize, 0usize);
    for y in 0..fh {
        for x in 0..fw {
            let (a, b) = (p[y * fw + x] > 0.5, inside(x, y));
            i += usize::from(a && b);
            u += usize::from(a || b);
        }
    }
    let j = i as f64 / u as f64;
    eprintln!("SAM 2 as a MaskModel: disc IoU {j:.3}");
    assert!(j > 0.9, "IoU {j}");
    // The same frame again is served from the cached encoding, and bad prompts are errors.
    let again = model.segment(&rgb, fw, fh, &Prompt { points: vec![([20.0, 20.0], false), ([120.0, 96.0], true)], bbox: None, mask: None }).unwrap();
    assert_eq!(again.len(), fw * fh);
    assert!(model.segment(&rgb, fw, fh, &Prompt::default()).is_err());
    assert!(model.segment(&rgb, fw + 1, fh, &prompt).is_err());
}

#[test]
fn sam2_tracks_a_clip_through_the_object_tracker_interface() {
    use effectcraft_segment::{ClipFrame, MaskModel, Prompt, TrackRequest};
    let Some((w, _)) = load() else { return };
    let (fw, fh, n) = (128usize, 96usize, 6usize);
    let centre = |t: usize| (40.0 + 8.0 * t as f32, 48.0f32);
    let inside = |t: usize, x: usize, y: usize| {
        let c = centre(t);
        ((x as f32 - c.0).powi(2) + (y as f32 - c.1).powi(2)).sqrt() < 26.0
    };
    let frames: Vec<std::sync::Arc<ClipFrame>> = (0..n)
        .map(|t| {
            let mut rgb = vec![[0f32; 3]; fw * fh];
            for y in 0..fh {
                for x in 0..fw {
                    let bg = 0.3 + 0.2 * (x as f32 / 9.0).sin() * (y as f32 / 7.0).cos();
                    rgb[y * fw + x] =
                        if inside(t, x, y) { [0.85 + 0.1 * ((x as f32 - centre(t).0) / 5.0).sin(), 0.4, 0.15] } else { [bg, bg * 0.8 + 0.1, bg * 0.6 + 0.3] };
                }
            }
            std::sync::Arc::new(ClipFrame { rgb, w: fw, h: fh })
        })
        .collect();
    let model = effectcraft_sam2::mask::Sam2Mask::new(std::sync::Arc::new(effectcraft_sam2::session::VideoModel::load(&w).unwrap()));
    assert!(model.tracker().is_some());
    let c = centre(1);
    let req = TrackRequest::single(100, n, vec![(101, Prompt { points: vec![([c.0, c.1], true)], bbox: None, mask: None })]);
    let mut seen = std::collections::BTreeMap::new();
    let mut get = |f: i64| frames.get((f - 100) as usize).cloned();
    model
        .tracker()
        .unwrap()
        .track(&req, &mut get, &|| false, &mut |o, f, t| {
            assert_eq!(o, 0);
            assert!(seen.insert(f, t).is_none(), "frame {f} reported twice");
        })
        .unwrap();
    assert_eq!(seen.keys().copied().collect::<Vec<_>>(), (100..100 + n as i64).collect::<Vec<_>>());
    for (f, t) in &seen {
        assert_eq!((t.w, t.h, t.prob.len()), (fw, fh, fw * fh));
        let k = (f - 100) as usize;
        let (mut i, mut u) = (0usize, 0usize);
        for y in 0..fh {
            for x in 0..fw {
                let (a, b) = (t.prob[y * fw + x] > 0.5, inside(k, x, y));
                i += usize::from(a && b);
                u += usize::from(a || b);
            }
        }
        let j = i as f64 / u.max(1) as f64;
        eprintln!("frame {f}: IoU {j:.3}, confidence {:.2}, occluded {}", t.confidence, t.occluded);
        assert!(j > 0.85 && !t.occluded && t.confidence > 0.3, "frame {f}: IoU {j}");
    }
    // Cancelling before the work starts, and bad requests, are errors.
    assert!(model.tracker().unwrap().track(&req, &mut get, &|| true, &mut |_, _, _| {}).is_err());
    let none = TrackRequest::single(100, n, vec![]);
    assert!(model.tracker().unwrap().track(&none, &mut get, &|| false, &mut |_, _, _| {}).is_err());
    let outside = TrackRequest::single(100, n, vec![(500, Prompt { points: vec![([1.0, 1.0], true)], bbox: None, mask: None })]);
    assert!(model.tracker().unwrap().track(&outside, &mut get, &|| false, &mut |_, _, _| {}).is_err());
}

/// Two objects in one pass: each follows its own disc, every frame is reported once per object, and
/// objects may start on different frames.
#[test]
fn sam2_tracks_two_objects_in_one_pass() {
    use effectcraft_segment::{ClipFrame, MaskModel, Prompt, TrackRequest};
    let Some((w, _)) = load() else { return };
    let (fw, fh, n) = (160usize, 120usize, 4usize);
    // A red disc moving right and a blue one moving left, never touching.
    let a = |t: usize| (36.0 + 6.0 * t as f32, 40.0f32);
    let b = |t: usize| (124.0 - 6.0 * t as f32, 84.0f32);
    let hit = |c: (f32, f32), x: usize, y: usize| ((x as f32 - c.0).powi(2) + (y as f32 - c.1).powi(2)).sqrt() < 20.0;
    let frames: Vec<std::sync::Arc<ClipFrame>> = (0..n)
        .map(|t| {
            let mut rgb = vec![[0f32; 3]; fw * fh];
            for y in 0..fh {
                for x in 0..fw {
                    let bg = 0.45 + 0.1 * (x as f32 / 9.0).sin() * (y as f32 / 7.0).cos();
                    rgb[y * fw + x] = if hit(a(t), x, y) {
                        [0.9, 0.15, 0.1]
                    } else if hit(b(t), x, y) {
                        [0.1, 0.25, 0.9]
                    } else {
                        [bg, bg * 0.9, bg * 0.8]
                    };
                }
            }
            std::sync::Arc::new(ClipFrame { rgb, w: fw, h: fh })
        })
        .collect();
    let model = effectcraft_sam2::mask::Sam2Mask::new(std::sync::Arc::new(effectcraft_sam2::session::VideoModel::load(&w).unwrap()));
    let click = |c: (f32, f32)| Prompt { points: vec![([c.0, c.1], true)], bbox: None, mask: None };
    // object 0 is prompted on frame 0, object 1 on frame 2
    let req = TrackRequest { first: 10, count: n, objects: vec![vec![(10, click(a(0)))], vec![(12, click(b(2)))]] };
    let mut get = |f: i64| frames.get((f - 10) as usize).cloned();
    let mut seen: std::collections::BTreeMap<(usize, i64), effectcraft_segment::TrackedFrame> = Default::default();
    model.tracker().unwrap().track(&req, &mut get, &|| false, &mut |o, f, t| assert!(seen.insert((o, f), t).is_none(), "object {o} frame {f} twice")).unwrap();
    assert_eq!(seen.len(), 2 * n, "{:?}", seen.keys().collect::<Vec<_>>());
    for ((o, f), t) in &seen {
        let k = (f - 10) as usize;
        let c = if *o == 0 { a(k) } else { b(k) };
        let (mut i, mut u) = (0usize, 0usize);
        for y in 0..fh {
            for x in 0..fw {
                let (p, q) = (t.prob[y * fw + x] > 0.5, hit(c, x, y));
                i += usize::from(p && q);
                u += usize::from(p || q);
            }
        }
        let j = i as f64 / u.max(1) as f64;
        eprintln!("object {o} frame {f}: IoU {j:.3}, confidence {:.2}", t.confidence);
        assert!(j > 0.85, "object {o} frame {f}: IoU {j}");
    }
    let too_many = TrackRequest { first: 10, count: n, objects: vec![vec![(10, click(a(0)))]; 40] };
    assert!(model.tracker().unwrap().track(&too_many, &mut get, &|| false, &mut |_, _, _| {}).is_err());
    let empty = TrackRequest { first: 10, count: n, objects: vec![] };
    assert!(model.tracker().unwrap().track(&empty, &mut get, &|| false, &mut |_, _, _| {}).is_err());
}

/// A mask prompt instead of clicks: the mask is the prompt frame's output, and tracking forwards and
/// backwards from it matches `Sam2VideoModel` given the same mask.
#[test]
fn mask_prompted_tracking_matches_sam2_video_model() {
    use effectcraft_sam2::session::{Cancel, Direction, ObjectId, VideoModel, VideoTracker};
    let Some((w, r)) = load_video() else { return };
    let model = std::sync::Arc::new(VideoModel::load(&w).unwrap());
    let mut tracker = VideoTracker::new(model, Box::new(Clip(r.get("clip.pixel_values").unwrap().clone()))).unwrap();
    let obj = ObjectId(1);
    let first = tracker.prompt_mask(2, obj, r.get("maskprompt.input").unwrap()).unwrap();
    let check = |tag: &str, m: &effectcraft_sam2::session::FrameMask| {
        let want = r.get(&format!("maskprompt.{tag}.{}.pred_masks", m.frame)).unwrap().flatten_all().unwrap().to_vec1::<f32>().unwrap();
        let want_score = r.get(&format!("maskprompt.{tag}.{}.score", m.frame)).unwrap().flatten_all().unwrap().to_vec1::<f32>().unwrap()[0];
        let err = m.logits.iter().zip(&want).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        let j = iou(&m.logits, &want);
        eprintln!("mask prompt {tag} frame {}: logit error {err:.4}, IoU {j:.4}, score {:.3} (want {want_score:.3})", m.frame, m.object_score);
        assert!(j >= VIDEO_MIN_IOU, "{tag} frame {} IoU {j}", m.frame);
        assert!(err < VIDEO_LOGIT_TOL, "{tag} frame {} logit error {err}", m.frame);
        assert!((m.object_score - want_score).abs() < 0.05, "{tag} frame {} score {} vs {want_score}", m.frame, m.object_score);
    };
    check("prompt", &first);
    let cancel = Cancel::new();
    let mut fwd = vec![];
    tracker.propagate(2, Direction::Forward, None, &cancel, &mut |m| fwd.push(m)).unwrap();
    fwd.iter().for_each(|m| check("fwd", m));
    let mut rev = vec![];
    tracker.propagate(2, Direction::Backward, None, &cancel, &mut |m| rev.push(m)).unwrap();
    rev.iter().for_each(|m| check("rev", m));
    assert_eq!((fwd.len(), rev.len()), (5, 3));
    // hostile masks are errors
    let bad = Tensor::zeros((1, 1, 8, 8), candle_core::DType::F32, &Device::Cpu).unwrap();
    assert!(tracker.prompt_mask(2, obj, &bad).is_err());
    let nan = (Tensor::zeros((1, 1, 1024, 1024), candle_core::DType::F32, &Device::Cpu).unwrap() + f64::NAN).unwrap();
    assert!(tracker.prompt_mask(2, obj, &nan).is_err());
    assert!(tracker.prompt_mask(99, obj, r.get("maskprompt.input").unwrap()).is_err());
}

/// The tracker interface takes a mask as a prompt: the disc's true mask on frame 1 tracks the disc.
#[test]
fn a_mask_prompt_through_the_object_tracker_interface() {
    use effectcraft_segment::{ClipFrame, MaskModel, Prompt, TrackRequest};
    let Some((w, _)) = load() else { return };
    let (fw, fh, n) = (128usize, 96usize, 4usize);
    let centre = |t: usize| (40.0 + 8.0 * t as f32, 48.0f32);
    let inside = |t: usize, x: usize, y: usize| ((x as f32 - centre(t).0).powi(2) + (y as f32 - centre(t).1).powi(2)).sqrt() < 26.0;
    let frames: Vec<std::sync::Arc<ClipFrame>> = (0..n)
        .map(|t| {
            let mut rgb = vec![[0f32; 3]; fw * fh];
            for y in 0..fh {
                for x in 0..fw {
                    let bg = 0.3 + 0.2 * (x as f32 / 9.0).sin() * (y as f32 / 7.0).cos();
                    rgb[y * fw + x] = if inside(t, x, y) { [0.85, 0.4, 0.15] } else { [bg, bg * 0.8 + 0.1, bg * 0.6 + 0.3] };
                }
            }
            std::sync::Arc::new(ClipFrame { rgb, w: fw, h: fh })
        })
        .collect();
    let model = effectcraft_sam2::mask::Sam2Mask::new(std::sync::Arc::new(effectcraft_sam2::session::VideoModel::load(&w).unwrap()));
    let mask: Vec<f32> = (0..fw * fh).map(|i| f32::from(inside(1, i % fw, i / fw))).collect();
    let mut get = |f: i64| frames.get(f as usize).cloned();
    let req = TrackRequest::single(0, n, vec![(1, Prompt { points: vec![], bbox: None, mask: Some(mask) })]);
    let mut seen = std::collections::BTreeMap::new();
    model
        .tracker()
        .unwrap()
        .track(&req, &mut get, &|| false, &mut |_, f, t| {
            seen.insert(f, t);
        })
        .unwrap();
    assert_eq!(seen.len(), n);
    for (f, t) in &seen {
        let (mut i, mut u) = (0usize, 0usize);
        for y in 0..fh {
            for x in 0..fw {
                let (a, b) = (t.prob[y * fw + x] > 0.5, inside(*f as usize, x, y));
                i += usize::from(a && b);
                u += usize::from(a || b);
            }
        }
        let j = i as f64 / u.max(1) as f64;
        eprintln!("mask-prompted frame {f}: IoU {j:.3}");
        assert!(j > 0.85 && !t.occluded, "frame {f}: IoU {j}");
    }
    // a mask that does not fit the frame is an error
    let bad = TrackRequest::single(0, n, vec![(1, Prompt { points: vec![], bbox: None, mask: Some(vec![1.0; 5]) })]);
    assert!(model.tracker().unwrap().track(&bad, &mut get, &|| false, &mut |_, _, _| {}).is_err());
}

/// The same prompts on the same frames give the same masks, bit for bit, run after run (CPU).
#[test]
fn tracking_is_deterministic() {
    use effectcraft_sam2::prompt::Prompt;
    use effectcraft_sam2::session::{Cancel, Direction, ObjectId, VideoModel, VideoTracker};
    let Some((w, r)) = load_video() else { return };
    let model = std::sync::Arc::new(VideoModel::load(&w).unwrap());
    let clip = r.get("clip.pixel_values").unwrap().narrow(0, 1, 3).unwrap();
    let run = || {
        let mut t = VideoTracker::new(model.clone(), Box::new(Clip(clip.clone()))).unwrap();
        t.prompt(0, ObjectId(1), &Prompt { points: vec![([416.0, 512.0], true)], bbox: None }).unwrap();
        let mut out = vec![];
        t.propagate(0, Direction::Forward, None, &Cancel::new(), &mut |m| out.push(m)).unwrap();
        out
    };
    let (a, b) = (run(), run());
    assert_eq!(a.len(), 3);
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(x.logits, y.logits, "frame {} differs between runs", x.frame);
        assert_eq!((x.iou.to_bits(), x.object_score.to_bits()), (y.iou.to_bits(), y.object_score.to_bits()));
    }
}

/// Hard limits: a tracker over its memory budget or past its time budget stops with Error::Budget.
#[test]
fn the_tracker_stops_at_its_budget() {
    use effectcraft_sam2::Error;
    use effectcraft_sam2::prompt::Prompt;
    use effectcraft_sam2::session::{Budget, Cancel, Direction, ObjectId, VideoModel, VideoTracker};
    let Some((w, r)) = load_video() else { return };
    let model = std::sync::Arc::new(VideoModel::load(&w).unwrap());
    let clip = r.get("clip.pixel_values").unwrap().narrow(0, 1, 3).unwrap();
    let click = Prompt { points: vec![([416.0, 512.0], true)], bbox: None };
    // memory: one frame's worth is more than a 1 MB limit
    let mut t = VideoTracker::new(model.clone(), Box::new(Clip(clip.clone()))).unwrap();
    t.set_budget(Budget { max_state_bytes: 1 << 20, max_seconds: None });
    assert!(matches!(t.prompt(0, ObjectId(1), &click), Err(Error::Budget(_))));
    // default limit: fine, and the bytes held are reported
    let mut t = VideoTracker::new(model.clone(), Box::new(Clip(clip.clone()))).unwrap();
    t.prompt(0, ObjectId(1), &click).unwrap();
    let held = t.state_bytes();
    assert!(held > 2 << 20 && held < 200 << 20, "{held}");
    // time: a limit of zero stops the first propagation
    t.set_budget(Budget { max_state_bytes: 2 << 30, max_seconds: Some(0.0) });
    let r = t.propagate(0, Direction::Forward, None, &Cancel::new(), &mut |_| {});
    assert!(matches!(r, Err(Error::Budget(_))), "{r:?}");
}

/// The GPU encoder (wgpu kernels, any vendor) against the CPU path and PyTorch on the same image. The two
/// float implementations differ by accumulation order and the GPU's erf (1.5e-7), compounded through 12
/// blocks. Measured (RX 7900 GRE, 2026-10-15): 6.8e-6 against PyTorch, 1.2e-5 against the CPU path.
const GPU_TOL: f32 = 1e-4;

#[test]
fn gpu_encoder_matches_the_cpu_path_and_pytorch() {
    use effectcraft_sam2::gpu::{Gpu, encoder::GpuEncoder};
    use effectcraft_sam2::model::ImageModel;
    let Some((w, r)) = load() else { return };
    let gpu = match Gpu::shared() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    eprintln!("GPU: {}", gpu.adapter_name());
    let model = ImageModel::load(&w).unwrap();
    let pixels = r.get("input.pixel_values").unwrap();
    let cpu = model.encode(pixels).unwrap();
    let enc = GpuEncoder::load(gpu, &w, model.vision.hiera.clone()).unwrap();
    let t = std::time::Instant::now();
    let got = enc.encode(pixels).unwrap();
    eprintln!("GPU encode (first run, includes shader compile): {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
    let t = std::time::Instant::now();
    let got = (enc.encode(pixels).unwrap(), got).0;
    eprintln!("GPU encode: {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
    let low_cpu = cpu.features.features.get(2).unwrap();
    let e = max_abs(&got.low, low_cpu);
    eprintln!("low level vs CPU: max abs error {e:e}");
    assert!(e < tol(GPU_TOL, low_cpu), "low level error {e} (peak {})", peak(low_cpu));
    let e = max_abs(&got.low, r.get("vision_encoder.neck.1").unwrap());
    eprintln!("low level vs PyTorch: max abs error {e:e}");
    assert!(e < tol(GPU_TOL, low_cpu), "low level vs PyTorch error {e}");
    let (e0, e1) = (max_abs(&got.high.0, &cpu.high.0), max_abs(&got.high.1, &cpu.high.1));
    eprintln!("skip features vs CPU: {e0:e} / {e1:e}");
    assert!(e0 < tol(GPU_TOL, &cpu.high.0) && e1 < tol(GPU_TOL, &cpu.high.1), "skip feature errors {e0} / {e1}");
}

/// The GPU memory attention against PyTorch (the fixture's frame: one memory frame and four pointer tokens).
/// Measured on the RX 7900 GRE, 2026-10-15; the CPU path is at 2.6e-5.
const GPU_ATTN_TOL: f32 = 1e-3;

#[test]
fn gpu_memory_attention_matches_pytorch() {
    use effectcraft_sam2::gpu::{Gpu, memattn::GpuMemoryAttention};
    let Some((w, r)) = load_video() else { return };
    let gpu = match Gpu::shared() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let att = GpuMemoryAttention::load(gpu, &w).unwrap();
    let get = |k: &str| r.get(k).unwrap().squeeze(1).unwrap();
    let pointers = r.get("mem.attn.num_pointers").unwrap().to_vec1::<f32>().unwrap()[0] as usize;
    let args = (
        get("mem.attn.current_vision_features"),
        get("mem.attn.current_vision_position_embeddings"),
        get("mem.attn.memory"),
        get("mem.attn.memory_posision_embeddings"),
    );
    let out = att.forward(&args.0, &args.1, &args.2, &args.3, pointers).unwrap();
    let want = r.get("mem.attn.out").unwrap().squeeze(0).unwrap().squeeze(0).unwrap();
    let e = max_abs(&out, &want);
    eprintln!("GPU memory attention vs PyTorch: max abs error {e:e}");
    assert!(e < GPU_ATTN_TOL, "GPU memory attention error {e}");
    // a full bank (7 memory frames and 16 pointers of 4 tokens): the same maths on a bigger problem, against the CPU path
    let cpu = effectcraft_sam2::memattn::MemoryAttention::load(&w).unwrap();
    let m = 7 * 4096 + 64;
    let mem = Tensor::randn(0f32, 1.0, (m, 64), &Device::Cpu).unwrap();
    let mem_pos = Tensor::randn(0f32, 1.0, (m, 64), &Device::Cpu).unwrap();
    let t = std::time::Instant::now();
    let big = att.forward(&args.0, &args.1, &mem, &mem_pos, 64).unwrap();
    eprintln!("GPU memory attention, full bank: {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
    let want = cpu.forward(&args.0, &args.1, &mem, &mem_pos, 64).unwrap();
    let e = max_abs(&big, &want);
    eprintln!("full bank vs CPU: max abs error {e:e}");
    assert!(e < GPU_ATTN_TOL, "full-bank error {e}");
    // bad shapes are errors
    assert!(att.forward(&args.0, &args.1, &mem, &mem_pos, m + 1).is_err());
    assert!(att.forward(&args.0.narrow(0, 0, 10).unwrap(), &args.1, &mem, &mem_pos, 64).is_err());
}

/// The GPU can be switched off in the middle of a clip: the CPU takes over, the device name changes, and
/// the masks still match `Sam2VideoModel` (the memory banks built on the GPU are read by the CPU path).
#[test]
fn switching_the_gpu_off_mid_clip_keeps_the_tracking_right() {
    use effectcraft_sam2::prompt::Prompt;
    use effectcraft_sam2::session::{Backend, Cancel, Direction, ObjectId, VideoModel, VideoTracker};
    let Some((w, r)) = load_video() else { return };
    let Ok(model) = VideoModel::load_with(&w, Backend::Gpu) else {
        eprintln!("skipped: no GPU");
        return;
    };
    let model = std::sync::Arc::new(model);
    assert_ne!(model.device_name(), "CPU");
    let mut t = VideoTracker::new(model.clone(), Box::new(Clip(r.get("clip.pixel_values").unwrap().clone()))).unwrap();
    t.prompt(2, ObjectId(1), &Prompt { points: vec![([416.0, 512.0], true)], bbox: None }).unwrap();
    let mut seen = vec![];
    t.propagate(2, Direction::Forward, Some(1), &Cancel::new(), &mut |m| seen.push(m)).unwrap();
    model.disable_gpu();
    assert_eq!(model.device_name(), "CPU");
    t.propagate(2, Direction::Forward, None, &Cancel::new(), &mut |m| seen.push(m)).unwrap();
    let last = seen.last().unwrap();
    assert_eq!(last.frame, 6);
    let want = r.get("video.fwd.6.pred_masks").unwrap().flatten_all().unwrap().to_vec1::<f32>().unwrap();
    let j = iou(&last.logits, &want);
    assert!(j >= VIDEO_MIN_IOU, "frame 6 after the switch: IoU {j}");
}

/// The GPU memory encoder against the CPU one on the fixture's frame (a mask, the frame's features). The CPU
/// path matches PyTorch to 3.5e-6; the GPU is held to the same output.
#[test]
fn gpu_memory_encoder_matches_the_cpu_one() {
    use effectcraft_sam2::gpu::{Gpu, memenc::GpuMemoryEncoder};
    let Some((w, r)) = load_video() else { return };
    let Ok(gpu) = Gpu::shared() else {
        eprintln!("skipped: no GPU");
        return;
    };
    let cpu = effectcraft_sam2::memory::MemoryEncoder::load(&w).unwrap();
    let enc = GpuMemoryEncoder::load(gpu, &w).unwrap();
    let (features, mask) = (r.get("mem.enc.vision_features").unwrap(), r.get("mem.enc.masks").unwrap());
    let t = std::time::Instant::now();
    let got = enc.encode(features, mask).unwrap();
    eprintln!("GPU memory encoder: {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
    let want = r.get("mem.enc.out_features").unwrap().flatten_from(2).unwrap().squeeze(0).unwrap().t().unwrap().contiguous().unwrap();
    let e = max_abs(&got, &want);
    eprintln!("GPU memory encoder vs PyTorch: max abs error {e:e}");
    assert!(e < 1e-3, "memory encoder error {e}");
    // the whole memory (with the occlusion embedding and bfloat16 rounding) equals the CPU one
    let (a, b) = (cpu.finish(&got, false).unwrap(), cpu.memory(features, mask, false).unwrap());
    let e = max_abs(&a.features, &b.features);
    assert!(e < 0.05, "memory features differ by {e} (bfloat16 steps are 2^-8 of the value)");
    assert_eq!(max_abs(&a.pos, &b.pos), 0.0);
    // wrong shapes are errors
    assert!(enc.encode(&features.narrow(1, 0, 8).unwrap(), mask).is_err());
}

/// The tracker reports the most memory it held (memory banks, kept results, cached frames), and the model
/// reports it for the last tracking run.
#[test]
fn the_tracker_reports_its_peak_memory() {
    use effectcraft_segment::{ClipFrame, MaskModel, Prompt, TrackRequest};
    let Some((w, _)) = load() else { return };
    let model = effectcraft_sam2::mask::Sam2Mask::new(std::sync::Arc::new(effectcraft_sam2::session::VideoModel::load(&w).unwrap()));
    assert_eq!(model.peak_state_bytes(), Some(0));
    let (fw, fh) = (64usize, 48usize);
    let frame = std::sync::Arc::new(ClipFrame {
        rgb: (0..fw * fh).map(|i| if (i % fw).abs_diff(32) < 12 && (i / fw).abs_diff(24) < 12 { [0.9, 0.2, 0.1] } else { [0.2, 0.3, 0.5] }).collect(),
        w: fw,
        h: fh,
    });
    let mut get = |_f: i64| Some(frame.clone());
    let req = TrackRequest::single(0, 3, vec![(0, Prompt { points: vec![([32.0, 24.0], true)], bbox: None, mask: None })]);
    model.tracker().unwrap().track(&req, &mut get, &|| false, &mut |_, _, _| {}).unwrap();
    let peak = model.peak_state_bytes().unwrap();
    // three stored frames (about 2 MB each) and the cached frame features
    assert!(peak > 6 << 20 && peak < 400 << 20, "peak {peak}");
}
