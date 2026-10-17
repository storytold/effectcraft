#!/usr/bin/env python
"""Dump SAM 2 video reference tensors (CPU, float32) for the Rust port's reference tests.

Usage: python -I tools/sam2_video_reference.py MODEL_DIR OUT.safetensors
Runs `Sam2VideoModel` on a 7-frame synthetic clip (a textured disc moving right, hidden behind a bar on
frames 4 and 5), prompted with one click on frame 2, then tracks forward (2..6) and backward (2..0).
Saves the preprocessed frames, every frame's low-resolution mask and object score, the stored memory
features and object pointers, and the inputs/outputs of the memory encoder and memory attention.
"""
import sys

import numpy as np
import torch
import torch.nn.functional as F
from safetensors.torch import save_file
from transformers import Sam2VideoModel
from transformers.models.sam2_video.modeling_sam2_video import Sam2VideoInferenceSession

T, SIZE, PROMPT_FRAME = 7, 256, 2


def clip():
    rng = np.random.default_rng(3)
    yy, xx = np.mgrid[0:SIZE, 0:SIZE].astype(np.float32)
    bg = 0.3 + 0.2 * np.sin(xx / 9.0) * np.cos(yy / 7.0)
    frames = []
    for t in range(T):
        cx, cy = 80 + 12 * t, 128
        disc = np.hypot(xx - cx, yy - cy) < 34
        img = np.stack([bg, bg * 0.8 + 0.1, bg * 0.6 + 0.3], -1)
        tex = 0.85 + 0.1 * np.sin((xx - cx) / 5)
        img[disc] = np.stack([tex, 0.4 + 0.1 * np.cos((yy - cy) / 5), 0.15 + 0 * xx], -1)[disc]
        if t in (4, 5):  # a bar hides the disc
            img[:, int(cx) - 50 : int(cx) + 50] = 0.2
        img += rng.normal(0, 0.01, img.shape).astype(np.float32)
        frames.append((img.clip(0, 1) * 255).astype(np.uint8))
    return frames, (80 + 12 * PROMPT_FRAME, 128)


def main(model_dir, out):
    torch.manual_seed(0)
    torch.set_num_threads(4)
    model = Sam2VideoModel.from_pretrained(model_dir).eval()
    frames, click = clip()
    mean = torch.tensor([0.485, 0.456, 0.406]).view(1, 3, 1, 1)
    std = torch.tensor([0.229, 0.224, 0.225]).view(1, 3, 1, 1)
    x = torch.from_numpy(np.stack(frames)).permute(0, 3, 1, 2).float() / 255.0
    x = F.interpolate(x, size=(1024, 1024), mode="bilinear", align_corners=False)
    pixels = (x - mean) / std

    taps = {"clip.pixel_values": pixels}
    first = {"enc": True, "attn": True}

    def enc_hook(_m, args, kwargs, out):
        if first["enc"]:
            first["enc"] = False
            vf = args[0] if args else kwargs["vision_features"]
            masks = args[1] if len(args) > 1 else kwargs["masks"]
            taps["mem.enc.vision_features"] = vf.detach().float().clone()
            taps["mem.enc.masks"] = masks.detach().float().clone()
            taps["mem.enc.out_features"] = out[0].detach().float().clone()
            taps["mem.enc.out_pos"] = out[1].detach().float().clone()

    def attn_hook(_m, args, kwargs, out):
        if first["attn"]:
            first["attn"] = False
            for k in ("current_vision_features", "memory", "current_vision_position_embeddings", "memory_posision_embeddings"):
                taps[f"mem.attn.{k}"] = kwargs[k].detach().float().clone()
            taps["mem.attn.num_pointers"] = torch.tensor([float(kwargs["num_object_pointer_tokens"])])
            taps["mem.attn.out"] = out.detach().float().clone()

    model.memory_encoder.register_forward_hook(enc_hook, with_kwargs=True)
    model.memory_attention.register_forward_hook(attn_hook, with_kwargs=True)

    sess = Sam2VideoInferenceSession(video=pixels, video_height=SIZE, video_width=SIZE, dtype=torch.float32)
    obj_idx = sess.obj_id_to_idx(1)
    scale = 1024.0 / SIZE
    sess.add_point_inputs(
        obj_idx,
        PROMPT_FRAME,
        {
            "point_coords": torch.tensor([[[[click[0] * scale, click[1] * scale]]]], dtype=torch.float32),
            "point_labels": torch.tensor([[[1]]], dtype=torch.int32),
        },
    )
    sess.obj_with_new_inputs = [1]

    def record(tag, o):
        taps[f"video.{tag}.{o.frame_idx}.pred_masks"] = o.pred_masks.detach().float().clone()
        taps[f"video.{tag}.{o.frame_idx}.score"] = o.object_score_logits.detach().float().clone()

    with torch.no_grad():
        record("prompt", model(sess, frame_idx=PROMPT_FRAME))
        for o in model.propagate_in_video_iterator(sess, start_frame_idx=PROMPT_FRAME):
            record("fwd", o)
        for o in model.propagate_in_video_iterator(sess, start_frame_idx=PROMPT_FRAME, reverse=True):
            record("rev", o)

    # The same object prompted with a mask instead of a click (binary, at the model's 1024 x 1024).
    msess = Sam2VideoInferenceSession(video=pixels, video_height=SIZE, video_width=SIZE, dtype=torch.float32)
    mobj = msess.obj_id_to_idx(1)
    yy, xx = np.mgrid[0:1024, 0:1024].astype(np.float32)
    cx, cy = (80 + 12 * PROMPT_FRAME) * scale, 128 * scale
    disc = torch.from_numpy((np.hypot(xx + 0.5 - cx, yy + 0.5 - cy) < 34 * scale).astype(np.float32))[None, None]
    taps["maskprompt.input"] = disc.clone()
    msess.add_mask_inputs(mobj, PROMPT_FRAME, disc)
    msess.obj_with_new_inputs = [1]
    with torch.no_grad():
        o = model(msess, frame_idx=PROMPT_FRAME)
        taps[f"maskprompt.prompt.{o.frame_idx}.pred_masks"] = o.pred_masks.detach().float().clone()
        taps[f"maskprompt.prompt.{o.frame_idx}.score"] = o.object_score_logits.detach().float().clone()
        for o in model.propagate_in_video_iterator(msess, start_frame_idx=PROMPT_FRAME):
            taps[f"maskprompt.fwd.{o.frame_idx}.pred_masks"] = o.pred_masks.detach().float().clone()
            taps[f"maskprompt.fwd.{o.frame_idx}.score"] = o.object_score_logits.detach().float().clone()
        for o in model.propagate_in_video_iterator(msess, start_frame_idx=PROMPT_FRAME, reverse=True):
            taps[f"maskprompt.rev.{o.frame_idx}.pred_masks"] = o.pred_masks.detach().float().clone()
            taps[f"maskprompt.rev.{o.frame_idx}.score"] = o.object_score_logits.detach().float().clone()
    md = msess.output_dict_per_obj[mobj]
    for kind in ("cond_frame_outputs", "non_cond_frame_outputs"):
        for f, oo in md[kind].items():
            taps[f"maskstate.{f}.object_pointer"] = oo["object_pointer"].detach().float().clone()

    d = sess.output_dict_per_obj[obj_idx]
    for kind in ("cond_frame_outputs", "non_cond_frame_outputs"):
        for f, o in d[kind].items():
            taps[f"state.{f}.maskmem_features"] = o["maskmem_features"].detach().float().clone()
            taps[f"state.{f}.maskmem_pos_enc"] = o["maskmem_pos_enc"].detach().float().clone()
            taps[f"state.{f}.object_pointer"] = o["object_pointer"].detach().float().clone()
    save_file({k: v.contiguous() for k, v in taps.items()}, out)
    print(f"wrote {len(taps)} tensors to {out}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
