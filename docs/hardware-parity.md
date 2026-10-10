# Hardware parity

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-10 · **Change:** major (first hardware parity document, from the code and open issues against After Effects 2026 26.5) · **Target:** Adobe After Effects 2026

Each piece of hardware After Effects uses, per platform, against EffectCraft. Ours comes from the
code at `origin/main` 1446e27 (`crates/gpu`, `crates/render`, `crates/ui-egui`) and the issue
tracker; After Effects' from its documentation (Mercury GPU Acceleration, Multi-Frame Rendering,
hardware-accelerated decode and encode, Mercury Transmit) and its bundle.

**Overall ≈ 40%** (estimated). The GPU compositor and 280 GPU effects are a real strength and run
in the browser too; hardware video decode and encode, video-out devices, tablets and multiple
monitors are missing, and two GPU start-up failures are open.

| Feature | After Effects 2026 | EffectCraft macOS | Windows | Linux | Web | Notes / evidence |
|---|---|---|---|---|---|---|
| GPU compositing and effects | Mercury GPU (Metal; CUDA / OpenCL / DirectX on Windows) | yes, Metal via wgpu | Vulkan yes; **Direct3D 12 fails** (FXC can't compile the `pointwise` kernel, #613) | Vulkan | WebGPU, in frame workers | 280 of 306 effects on the GPU, checked against the CPU renderer; Fractal and the analysis/tool effects stay on the CPU ([effects-parity.md](effects-parity.md)) |
| CPU fallback and device-loss recovery | yes | yes | yes; Windows ARM64 Adreno panics after device loss (#591) | yes | yes (#225 fixed) | Out of video memory falls back to the CPU (#106) |
| GPU detection and choice | GPU info, renderer choice | Backend Auto per comp | "GPU isn't being detected" (#596); Intel HD 5500 fix unverified (#198) | yes | yes | No settings page listing adapters yet |
| Multi-core rendering | Multi-Frame Rendering | rayon pools; preview scheduler sized to the render pool | same | same | Web Workers, one engine per worker | No benchmark against Multi-Frame Rendering |
| Hardware video decode | H.264 / HEVC / ProRes (Apple silicon) | none (software, FilmCraft) | none | none | none | FilmCraft has VideoToolbox, Media Foundation/DXVA and VA-API paths to borrow (gap 15) |
| Hardware video encode | H.264 / HEVC via VideoToolbox, NVENC, Quick Sync (render queue and Media Encoder) | none | none | none | none | #498 asks for NVENC; our software encoders are slower and lack B-frames |
| Video output devices | Mercury Transmit to AJA, Blackmagic, second display | second preview window only (Video Preview) | same | same | — | No SDI/HDMI device output |
| HDR display | HDR monitoring on EDR / HDR displays | tone-mapped SDR view; no EDR output | no HDR output | no | no | Colour engine handles HDR maths (Rec. 2100 PQ/HLG output files) |
| Pen tablets (pressure, tilt) | yes, for Paint and Roto | Brushes panel has pressure options, but no tablet pressure is read | same | same | same | Paint strokes store pressure per vertex; input never fills it |
| Multiple monitors | panels and windows across screens | floating panels within one window only | same | same (#486) | — | gap 20 |
| Touch / trackpad gestures | pinch zoom in viewers | egui pointer and scroll; no pinch-to-zoom audited | — | — | — | — |
| Audio hardware | Core Audio / ASIO / WASAPI, device choice, latency | cpal output; preview sound late on Linux (#554) and after a start offset (#504) | cpal | cpal (ALSA) | Web Audio | No input or device latency settings |
| Machine learning acceleration | Apple Neural Engine / GPU for Object Matte, Roto Brush 3 | CPU inference (pure Rust, rayon) | CPU | CPU | CPU | No GPU inference for MobileSAM or MediaPipe |
| Memory management | RAM reserve, disk cache | RAM Reserved, Reduce Cache When Low on Memory, disk cache | same | same | OPFS disk cache | good |

## Estimate

**≈ 60–120 h**: Direct3D 12 and adapter start-up fixes 6–12 h; hardware decode on three platforms
20–35 h and encode 15–30 h (borrowing FilmCraft's work); multiple monitors 10–20 h; tablet pressure
4–8 h; HDR output 8–15 h. Needs Windows and Linux GPUs we don't have locally, and a human for
the Adreno and Intel cases.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-10 | major | First hardware parity document |
