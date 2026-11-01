# ADR 0002: SAM 2 video tracker for the Roto Brush

Status: accepted (maintainer answers, 2026-10-15). Plan: `plan/roto-sam2.md` (maintainer-local).

## Context
The Roto Brush's classical engine loses an object that is occluded or leaves and re-enters the frame
(`effectcraft-cli bench --roto`: IoU 0.49 on the occlusion clip, 0.62 on re-entry, 0 after the object returns).
Meta's SAM 2.1 (Apache-2.0 code and checkpoints) has streaming memory built for this.

## Decisions
- **D1** A native-only `candle` dependency may enter the workspace, in its own crate (`effectcraft-sam2`), excluded from wasm32.
- **D2** `effectcraft-sam2` is licensed Apache-2.0 only (it derives from Apache-2.0 Hugging Face `transformers` code); `effectcraft-segment` stays MIT OR Apache-2.0.
- **D3** Shared candle helpers are vendored with a NOTICE for now; a shared crate with LightCraft can follow.
- **D4** Weights are never hosted or bundled by us: direct from Hugging Face, a mirror, or a user folder, with the SHA-256 pinned in the registry.
- **D5** SAM 2 is opt-in at first; it becomes the default for tracking only after the benchmark shows it should.
- **D6** Hand-written kernels (the MobileSAM way) remain the Phase 6 route for AMD GPUs if candle cannot get there.

## Consequences
Without the model installed nothing changes. Every ported file carries Apache-2.0 §4(b) modified-work headers.
