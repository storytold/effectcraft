# Roto Brush with SAM 2

Roto Brush can follow an object through a clip with Meta's **SAM 2.1 Tiny** (Hiera-Tiny, 39M parameters,
Apache-2.0). It is optional: nothing changes until you install the model and choose it.

## Using it
1. Settings ▸ Roto Brush ▸ SAM 2.1 Tiny ▸ **Install from File…** with the published `model.safetensors`
   (155.9 MB). The file's size and SHA-256 are checked and the licence notice is kept next to it.
   Agents: `roto.model.install {id: "sam2.1-hiera-tiny", path: "model.safetensors"}`.
   SAM 2 has no in-app download until the weights are mirrored under storytold control.
   Each model card links its pinned Hugging Face `model.safetensors` checkpoint, rather than the
   project's different `.pt` files. Its checkpoint source and SHA-256 are retained in the notice.
2. Choose it (`roto.model.select {id: "sam2.1-hiera-tiny", wait: true}` for agents). Installing an
   unselected model does not activate it, even with `wait: true` on installation.
3. Paint strokes on the base frame as usual, then **Propagate**. One tracking pass follows the object through the
   whole span in both directions, using SAM 2's streaming memory, so it keeps tracking through occlusion and when the
   object leaves the frame and returns. Each frame's probability map is then refined by the same graph cut as every other
   Roto Brush frame: your strokes stay hard constraints and edges snap to colour.
4. `roto.status {frame}` reports `source` (`tracker`, `model` for a frame the model segmented on its own, or
   `classical`) and, for tracked frames, `tracker.frames[f]`: `confidence` (0–1, the model's object score times its
   predicted mask quality) and `occluded` (the object is not in the frame: an empty matte). `model` says what Settings
   chose and whether it is installed, loaded and loading; `notice` says when it is not the one in use for the layer.
   `tracker.saved` names the model the saved frames came from.

With SAM 2 chosen but not installed, or failing to load, Propagate refuses with the reason. A tracker failure during
propagation can fall back to classic; its warning is live progress only, as described under [Memory and limits](#memory-and-limits).
While it is still loading, `wait: true` (agents) loads it first; without `wait` it
refuses, and the load goes on in the background. Deselecting SAM 2 (choosing Classic) runs Propagate exactly as before,
and Roto Brush 1.0 never uses a model.

Tracked frames are saved in the project with the instance, as Freeze's are, so a saved project renders the same when it
reopens, in the app or with `effectcraft-cli`, without the model being loaded or chosen there. They apply while the
strokes and span they came from are unchanged. Changing a stroke or the span makes them stale until the next Propagate.
Choosing another model, or the classic engine, leaves them in place and `roto.status` says so. They retain the original
model tag and per-object cache keys when rendered or re-saved; the next Propagate replaces them using the selected
model (or classic engine). Object Matte saves nothing.

## Several objects in one instance
A Roto Brush instance can hold several objects. The **Object N** button in the Layer panel's bar picks the object new
strokes belong to (it cycles through the objects that have strokes), and **+** starts a new one. Agents: `roto.options
{object}` sets the current object, `roto.stroke` takes `object` (0–31; default the current one), `roto.clearStrokes`
takes `object` to remove one object's strokes, and `roto.status` lists `objects` (`{id, strokes}`) and `currentObject`.

With SAM 2, Propagate follows every object in **one pass** (the frame is encoded once, whatever the number of objects), each
from its own foreground and background strokes, and the mattes are united into the instance's one matte. Each object keeps
its own occlusion: the frame counts as absent only when every object is, and otherwise the least sure object sets the
frame's confidence on the span bar. A background stroke belongs to its object, so it separates that object from what
lies under it without cutting a hole in the others. Refine Edge strokes belong to the whole matte. The classic engine has
no objects: all foreground strokes make one object, and background strokes apply to it as before. Saved projects are
unchanged (strokes without an object are object 0, and nothing is written for object 0).

**Seeing and choosing objects.** The Layer panel's boundary view outlines each object in its own colour (object 1 in the
chosen boundary colour, the others from a palette), and the picker button carries the current object's colour. **Show
Object** (Roto Brush Matte, a slider: 0 shows all objects together, N shows object N alone) limits the instance's matte to
one object, so an effect after it can treat the pieces separately; an object the instance does not have shows nothing, and
with one object, or on frames the classic engine made, the setting changes nothing. Each object's matte is kept next to
the united one, in the session and in the saved tracking (`tracked` stores them only when there are several objects), so
Show Object renders the same after the project is reopened. Freeze records the Show Object that was set.

This is not several Roto Brush instances sharing a pass: each instance reads the layer after the effects above it, so
instances can see different pixels. Objects inside one instance always see the same ones.

## Sizes
Four checkpoints are in the registry (Settings ▸ Roto Brush); the size is detected from the file, and each is
pinned to a Hugging Face commit with its SHA-256. All run the same tracker; a bigger backbone costs time and memory.

| Model | File size | GPU encode (RX 7900 GRE) | Agreement with PyTorch |
|---|---|---|---|
| SAM 2.1 Tiny (`sam2.1-hiera-tiny`) | 155.9 MB | 0.24 s | encoder 6.8e-6 |
| SAM 2.1 Small (`sam2.1-hiera-small`) | 184.3 MB | 0.20 s | encoder 1.0e-5 |
| SAM 2.1 Base+ (`sam2.1-hiera-base-plus`) | 323.5 MB | 0.37 s | encoder 2.3e-5 |
| SAM 2.1 Large (`sam2.1-hiera-large`) | 897.9 MB | 0.72 s | encoder 5.2e-4 |

### How the sizes compare on the benchmark clips
`effectcraft-cli bench --roto --frames 24 --fixes --sam2 FILE`, release build, SAM 2 on the GPU (RX 7900 GRE), one run each.
Mean IoU (after the object returns, in brackets) and seconds per tracked frame:

| Clip | Tiny | Small | Base+ | Large |
|---|---|---|---|---|
| move | 0.992 | 0.988 | 0.986 | 0.994 |
| light | 0.990 | 0.985 | 0.987 | 0.993 |
| occlusion | 0.989 (0.987) | 0.988 (0.984) | 0.987 (0.984) | 0.989 (0.987) |
| reenter | 0.982 (0.975) | 0.981 (0.970) | 0.978 (0.966) | **0.694 (0.110)** |
| s / tracked frame | 0.69 | 0.70 | 0.79 | 1.17 |

The classic engine scores 0.98 / 0.98 / 0.49 / 0.62 on the same clips. Peak tracker state is 88 MB for every size (the
weights are not counted). The clips are synthetic and easy, so they cannot tell the sizes apart on accuracy: **no larger
size tracked better here, and Large lost the object after it left and came back** (IoU 0.11 after the return, where
Tiny, Small and Base+ found it again). That is the model's behaviour on this clip, not the port:
Large matches PyTorch stage by stage. Whether a larger size helps on real footage is not measured.

Settings therefore marks **Tiny** as recommended (the smallest download, and nothing larger did better), and says what was
measured for the others; Base+ and Large also warn when no GPU was found (they take several seconds a frame on a CPU).
The list carries `recommended` and `hint` per model (`roto.models`).

### Half-precision weights (optional)
`EFFECTCRAFT_SAM2_F16=1` keeps the weight matrices on the GPU as half precision, halving their memory (Large: about
900 MB to 450 MB). It is off by default: the video logit error against PyTorch grows from about 0.003 to 0.06 (Tiny)
and 0.12 (Large) with the same masks, and the speed gain is under 10% (Large encode 0.72 s to 0.66 s).

### What stays on the CPU
The mask decoder takes about 110 ms of a tracked frame (attention layers 42 ms, the two upscaling convolutions 50 ms).
Rewriting the upscaling convolutions as a matrix product and a pixel shuffle did not make them faster, and a GPU port
would save under 0.1 s of a 0.7 s frame, so it stays on the CPU.

Every size was checked stage by stage against PyTorch (patch embedding, each Hiera block, neck, prompt encoder,
decoder, memory encoder, memory attention) and on a whole clip forward and backward, on the CPU and the GPU: the video
logit error is at most 0.005 with the same masks. Large carries much larger activations (peaks near 300 in the third
stage), so its f32 rounding error is larger in absolute terms; its tests use a bound relative to the peak. Large needs
the GPU to run per block (the encode submits once per block so intermediates are freed); a GPU error of any kind falls
back to the CPU.

## Corrections re-track only what they change
A correction stroke changes the keys of the frames from it on (in both directions from the base), so Propagate tracks
only those again: the window starts at the nearest prompted frame at or before the first frame it needs (the corrected
frame, which restarts the object with its own prompt) and runs to the last frame it needs. Frames before the correction
keep their tracked results, and a Propagate with nothing changed does no tracking. The older non-conditioning memory
frames before the corrected frame are not rebuilt, so frames just after it can differ slightly from a from-scratch pass.
Tracked frames of a session are in memory; a project reopened in a new process re-tracks the whole span after a stroke
change (its saved tracking is stale as a whole).

## Memory and limits
The weights take about 156 MB. A model idle for 10 minutes is unloaded (a toast says so; the choice stays) and loads
again when a Propagate or a stroke needs it. The tracker holds the memory banks of the objects it follows (about 2 MB per
stored frame, pruned to the frames later ones can still look at) and refuses to grow past 2 GiB, or past a time limit when
one is set, with a clear error instead of running out of memory. Two runs of the same prompts give bit-identical masks on
the CPU.

Pending per-object probability maps have a separate 256 MiB budget, including their allocated capacity and frame
slots. This bounds the full-resolution results waiting to be united when objects first prompted on different frames
report in separate passes. Reaching the budget cancels tracking and falls back to the classic engine. The reason is
available as `trackerNote` in live job progress only. Completion removes that progress and shows a success toast, so
`wait: true` callers can miss the fallback reason; the completion toast alone does not confirm the tracker was used.
A single object's results need no pending buffer.

## How it works
`crates/sam2` is a Rust port (on [candle](https://github.com/huggingface/candle), native only) of the Hugging Face
`transformers` SAM 2 implementation, Apache-2.0 (see its `NOTICE`). Every stage is checked against PyTorch by
`crates/sam2/tests/it/reference.rs` (they skip without the weights; reference tensors come from
`tools/sam2_reference.py` and `tools/sam2_video_reference.py`):

| Stage | Max abs error vs PyTorch |
|---|---|
| Hiera backbone, 12 blocks | 2.6e-5 |
| Neck features / position encodings | 9.7e-6 / 6e-8 |
| Prompt embeddings | 1.2e-7 |
| Mask decoder (logits up to 11.6) | 1.1e-4 |
| Memory encoder | 3.5e-6 |
| Memory attention (rotary) | 2.6e-5 |
| 7-frame clip, forwards and backwards (9 frame results) | logits 0.0038, IoU 1.0000 |

SAM 2 is prompted with at most three foreground and three background points per frame: it was trained on a handful
of clicks, and measured confidence collapses with many (0.99 with 1–7 points, 0.25 with 16, 0.00 with 32).

## Speed (this PC: AMD RX 7900 GRE and a 16-thread CPU, release build)
SAM 2 runs on the GPU when there is one (wgpu compute kernels, so AMD, NVIDIA and Intel alike; candle itself has no AMD
backend) and on the CPU otherwise. The model loads on the GPU automatically; `EFFECTCRAFT_SAM2_DEVICE=cpu` (or `gpu`)
overrides it. A GPU that fails (no adapter, out of memory, a lost device) makes the CPU take over for the rest of the
session, and `roto.status` reports `tracker.device` (the GPU's name, or `CPU`).

| Step | CPU | GPU |
|---|---|---|
| Encode one 1024 × 1024 frame | 2.05 s | **0.24 s** |
| Memory attention (full bank of 7 memory frames) | 2.3 s | **0.29 s** |
| Memory encoder | 0.24 s | 0.05 s |
| Decoder (CPU on both) | 0.11 s | 0.11 s |
| **A tracked frame in all** | **4.3 s** | **0.7 s** |
| A click on an encoded frame | 0.10 s | 0.10 s |

A 300-frame clip takes about 21 minutes on the CPU and about 3.5 minutes on this GPU; the decoder, still on the CPU, is the
next 16 % of a GPU frame. The CPU numbers came down from 3.4 s per encode at the start of the speed
work (parallel GELU and bias add, cached position encodings, pooling without permutes); chunking the attention queries was
tried and did not help. The GPU kernels (`crates/sam2/src/gpu`) are tested against CPU definitions and PyTorch: the
encoder's 64 × 64 level matches PyTorch to 7e-6, the memory attention to 1.8e-5, and a clip tracked on the GPU matches
`Sam2VideoModel` as the CPU does (worst logit error 0.0032). `cargo run --release -p effectcraft-sam2 --example sam2_bench`
times the pieces on both devices (`EFFECTCRAFT_SAM2_PROFILE=1` prints a frame's breakdown).

## Benchmark
`effectcraft-cli bench --roto [--frames N] [--sam2 model.safetensors (any size; default: the first installed)] [--mobilesam mobile_sam.pt]` runs synthetic clips with
known mattes through all three. 24 frames, debug build, SAM 2 on the GPU; mean IoU (mean IoU after the object returns), seconds
per frame:

| Clip | Classical | MobileSAM | SAM 2.1 Tiny |
|---|---|---|---|
| move | 0.980 | 0.989 | 0.992 |
| light | 0.979 | 0.990 | 0.992 |
| occlusion (a bar hides the disk, then clears it) | 0.491 (0.000) | 0.494 (0.000) | 0.989 (0.988) |
| reenter (the disk leaves and comes back) | 0.618 (0.000) | 0.620 (0.000) | 0.982 (0.976) |
| s/frame | 0.01 | 0.5–1.3 | 0.8 |

MobileSAM prompts every frame independently from the flow-warped matte, so it follows the object while the flow does and
loses it when it is hidden; SAM 2's streaming memory is what lets it re-acquire the object. The synthetic clips are easy (one
textured disc), so the "move" and "light" rows mostly show all three working; the last two rows are the difference.

### Corrections needed (`--fixes`)
`bench --roto --fixes` also counts the correction rounds each tracker needs before every frame reaches IoU 0.9: after each
run it paints on the first frame under the target, up to 6 rounds. A round can add a short foreground stroke through the
true object's centre and a background stroke at the centroid of all false-positive pixels. With separate false-positive
blobs, that centroid can fall between them. The counts below are rounds, not individual strokes. 16 frames, debug build:

| Clip | Classical | MobileSAM | SAM 2.1 Tiny |
|---|---|---|---|
| move | 0 | 0 | 0 |
| light | 0 | 0 | 0 |
| occlusion | more than 6 | more than 6 | 0 |
| reenter | more than 6 | more than 6 | 0 |

The simulated user is simple (one stroke pair per round, always on the first bad frame), so "more than 6" means this
correction model never recovers the flow-based engines on those clips, not that no human could. Peak memory is not measured.
