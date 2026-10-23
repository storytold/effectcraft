#!/usr/bin/env python
"""Dump per-stage SAM 2 reference tensors (CPU, float32) for the Rust port's reference tests.

Usage: python -I tools/sam2_reference.py MODEL_DIR OUT.safetensors
MODEL_DIR holds the facebook/sam2.1-hiera-tiny files (config.json, model.safetensors, ...).
Versions are pinned in tools/requirements-sam2-reference.txt. The input is synthetic, so no
media is committed; the weights are never copied into the repo.
"""
import sys

import numpy as np
import torch
from safetensors.torch import save_file
import torch.nn.functional as F
from transformers import Sam2Model


def synthetic_image(size=256):
    """A textured disc over a textured background (deterministic)."""
    rng = np.random.default_rng(7)
    yy, xx = np.mgrid[0:size, 0:size].astype(np.float32)
    bg = 0.3 + 0.2 * np.sin(xx / 9.0) * np.cos(yy / 7.0)
    disc = np.hypot(xx - size * 0.45, yy - size * 0.5) < size * 0.22
    img = np.stack([bg, bg * 0.8 + 0.1, bg * 0.6 + 0.3], -1)
    img[disc] = np.stack([0.85 + 0.1 * np.sin(xx / 5), 0.4 + 0.1 * np.cos(yy / 5), 0.15 + 0 * xx], -1)[disc]
    img += rng.normal(0, 0.01, img.shape).astype(np.float32)
    return (img.clip(0, 1) * 255).astype(np.uint8)


def main(model_dir, out):
    torch.manual_seed(0)
    torch.set_num_threads(1)
    model = Sam2Model.from_pretrained(model_dir).eval()
    image = synthetic_image()
    # Preprocess by hand (no torchvision needed): resize to 1024x1024, ImageNet mean/std.
    x = torch.from_numpy(image).permute(2, 0, 1)[None].float() / 255.0
    x = F.interpolate(x, size=(1024, 1024), mode="bilinear", align_corners=False)
    mean = torch.tensor([0.485, 0.456, 0.406]).view(1, 3, 1, 1)
    std = torch.tensor([0.229, 0.224, 0.225]).view(1, 3, 1, 1)
    scale = 1024.0 / image.shape[0]
    inputs = {
        "pixel_values": (x - mean) / std,
        "input_points": torch.tensor([[[[115.0 * scale, 128.0 * scale]]]]),
        "input_labels": torch.tensor([[[1]]], dtype=torch.int32),
    }
    taps = {}

    def tap(name):
        def hook(_m, _i, o):
            def flat(x):
                if isinstance(x, torch.Tensor):
                    yield x
                elif isinstance(x, (tuple, list)):
                    for y in x:
                        yield from flat(y)
                elif hasattr(x, "to_tuple"):  # a ModelOutput (the neck)
                    yield from flat(x.to_tuple())

            for k, t in enumerate(flat(o)):
                taps[f"{name}.{k}"] = t.detach().float().clone().contiguous()
        return hook

    # Stage taps by module path: patch embed, each Hiera block, the neck, prompt encoder, decoder.
    for name, mod in model.named_modules():
        parts = name.split(".")
        if name in ("vision_encoder.backbone.patch_embed", "vision_encoder.neck", "prompt_encoder", "mask_decoder") or (
            len(parts) == 4 and parts[:3] == ["vision_encoder", "backbone", "blocks"]
        ):
            mod.register_forward_hook(tap(name))
    import time
    with torch.no_grad():
        t0 = time.perf_counter()
        res = model(**inputs, multimask_output=True)
        print(f"forward (1 thread, first run): {time.perf_counter() - t0:.2f} s")
    taps["input.pixel_values"] = inputs["pixel_values"].float()
    taps["out.iou_scores"] = res.iou_scores.float()
    taps["out.pred_masks"] = res.pred_masks.float()
    save_file({k: v.clone().contiguous() for k, v in taps.items()}, out)
    print(f"wrote {len(taps)} tensors to {out}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
