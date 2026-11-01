# effectcraft-segment

Trained models for Roto Brush and face tracking, each kind behind one swappable interface.

- **`MaskModel`**: what Roto Brush asks of a model: given a frame and prompts (foreground /
  background points, a box, a prior mask), the foreground probability of every pixel. Roto Brush
  (`effectcraft-track`) only talks to this trait, so models can be added or swapped without
  touching it. The classical graph-cut segmenter stays built in as the fallback.
- **`face::FaceModel`**: what face tracking asks of a model: find a face in a region of a frame,
  then follow it from frame to frame. A model reports its own points plus a `Topology` naming
  which are the tracker's landmarks and which trace the face outline, so the tracker never
  depends on one model's layout. The classical face tracker stays built in as the fallback.
- **`MODELS`**: the registry. Every entry must be open source under a licence compatible with
  EffectCraft's MIT OR Apache-2.0 (`ALLOWED_LICENCES`), with its task, authors, homepage, licence
  URL, an app download URL when available, size and SHA-256. `load` refuses anything else.
  SAM 2 is Install from File only until a storytold-controlled mirror is available.
  The checkpoint `source` stays separate from the app download URL, so file-install guidance and
  licence notices identify the verified artifact even when app downloads are unavailable.
- **Weights are never bundled.** They are downloaded on demand (or installed from a file) by the
  engine (`roto.model.*` / `face.model.*`, Settings ▸ Roto Brush / Face Tracking), verified
  against the registry's SHA-256 (`sha256`, dependency-free) and kept with a notice naming their
  authors, licence and source (`notice`).
- **`pt`**: reads PyTorch checkpoints (`torch.save` zip archives and the pickle subset state dicts
  use), so official weights load exactly as published.
- **`tflite`**: reads TensorFlow Lite models (the FlatBuffer laid out by TensorFlow Lite's
  `schema.fbs`) and runs the float operators small vision models use (convolutions, depthwise
  convolutions, pooling, PReLU, padding, concatenation; float16 weights dequantised at load).
- **`mobilesam`**: MobileSAM in plain Rust: the TinyViT-5M image encoder and Segment Anything's
  prompt encoder and two-way-transformer mask decoder.
- **`mediapipe`**: MediaPipe Face Landmarker: the BlazeFace short-range detector (SSD anchors,
  weighted non-maximum suppression) and Face Mesh V2 (478 points), with the rotated crops
  MediaPipe's face landmarker graph uses.

All of it runs on `nn` kernels (rayon, and ndarray's safe `general_mat_mul`, which picks AVX/FMA
kernels at run time). No ML framework, no unsafe code.

## Models

| id | Task | Model | Authors | Licence | File | Size |
|---|---|---|---|---|---|---|
| `mobilesam` | Roto Brush | [MobileSAM](https://github.com/ChaoningZhang/MobileSAM): Segment Anything with a 5M-parameter TinyViT encoder | Chaoning Zhang et al., building on Meta AI's Segment Anything and Microsoft's TinyViT | Apache-2.0 | `mobile_sam.pt` from the official repository | 40.7 MB |
| `sam2.1-hiera-tiny` | Roto Brush | [SAM 2.1 Tiny](https://github.com/facebookresearch/sam2): Hiera-Tiny video object tracker with streaming memory | Meta AI (Nikhila Ravi et al.) | Apache-2.0 | `model.safetensors` of `facebook/sam2.1-hiera-tiny` (Hugging Face, pinned to a commit); loaded by `effectcraft-sam2` | 155.9 MB |
| `sam2.1-hiera-small` | Roto Brush | [SAM 2.1 Small](https://github.com/facebookresearch/sam2): Hiera-Small video object tracker with streaming memory | Meta AI (Nikhila Ravi et al.) | Apache-2.0 | `model.safetensors` of `facebook/sam2.1-hiera-small` (Hugging Face, pinned to a commit); loaded by `effectcraft-sam2` | 184.3 MB |
| `sam2.1-hiera-base-plus` | Roto Brush | [SAM 2.1 Base+](https://github.com/facebookresearch/sam2): Hiera-Base+ video object tracker with streaming memory | Meta AI (Nikhila Ravi et al.) | Apache-2.0 | `model.safetensors` of `facebook/sam2.1-hiera-base-plus` (Hugging Face, pinned to a commit); loaded by `effectcraft-sam2` | 323.5 MB |
| `sam2.1-hiera-large` | Roto Brush | [SAM 2.1 Large](https://github.com/facebookresearch/sam2): Hiera-Large video object tracker with streaming memory | Meta AI (Nikhila Ravi et al.) | Apache-2.0 | `model.safetensors` of `facebook/sam2.1-hiera-large` (Hugging Face, pinned to a commit); loaded by `effectcraft-sam2` | 897.9 MB |
| `mediapipe-face` | Face tracking | [MediaPipe Face Landmarker](https://developers.google.com/edge/mediapipe/solutions/vision/face_landmarker): BlazeFace detector and Face Mesh V2 | Google LLC | Apache-2.0 ([model card](https://storage.googleapis.com/mediapipe-assets/Model%20Card%20MediaPipe%20Face%20Mesh%20V2.pdf)) | `face_landmarker.task` (float16, version 1) from Google's model storage | 3.8 MB |

How they were checked:

- **MobileSAM**: the implementation follows the published architectures (MobileSAM, TinyViT and
  Segment Anything papers and their Apache-2.0 reference code). Its output was checked against an
  independent implementation (Hugging Face candle's, MIT/Apache-2.0) on the same weights:
  embeddings within 0.3% RMS (exact vs. tanh GELU) and 99.9% of mask pixels equal.
- **MediaPipe Face Landmarker**: the interpreter's outputs match TensorFlow Lite's own
  (`ai-edge-litert` 2.2.0) on both models to about one part in a million (a test pins them). The
  whole pipeline's 478 points on a test portrait are 0.85 px from MediaPipe's own Python pipeline
  on average (2.2 px at worst). Finding a face takes about 45 ms; following it, about 20 ms a
  frame (32-thread CPU).

Tests that need the weights run when `EFFECTCRAFT_MOBILESAM` / `EFFECTCRAFT_FACE_LANDMARKER`
point at the files, and are skipped otherwise.

## Adding a model

1. Pick a model whose code **and** weights are open source under an allowed licence. Check the
   weights' own terms: many models with MIT code ship weights trained on research-only data.
2. Add a `ModelInfo` to `MODELS` (its task and authors; pin the official file's SHA-256 and size),
   and credit it in the root `ATTRIBUTION.md`.
3. Implement `MaskModel` or `FaceModel` for it and add its loader to `load`.
4. Test it against a reference implementation; Settings, the `roto.model.*` / `face.model.*`
   commands and the tools pick it up from the registry.
