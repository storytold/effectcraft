# effectcraft-av1enc

Clean-room, pure-Rust AV1 encoder for EffectCraft's MP4 / WebM export (layer L0, `std` only,
no `unsafe`, builds for `wasm32-unknown-unknown`).

Output is decoded **bit-exactly** by libdav1d (`ffmpeg -c:v libdav1d`, checked in
`tests/it/ffmpeg_oracle.rs` when ffmpeg is installed) and by FilmCraft's spec-derived
`filmcraft-av1` decoder (`tests/it/roundtrip.rs`): every decoded frame equals the encoder's own
reconstruction, so prediction never drifts.

## What is implemented

- **Stream**: Main profile (`seq_profile` 0), 4:2:0, 8 or 10 bit, BT.709 colour description,
  limited or full range. Sequence header with one operating point; `seq_level_idx` given or the
  lowest Annex A level whose MaxPicSize / MaxH/VSize / MaxDisplayRate (and, in bitrate mode, Main
  tier MaxBitrate) fit. Order hints, film grain, superres, CDEF, loop restoration, intra edge
  filter, filter intra, inter-intra, masked / distance compound, warped motion and dual filter
  are disabled in the sequence header.
- **Packets**: one temporal unit per frame as stored in MP4 / Matroska samples: OBUs with size
  fields, no temporal delimiter; key frames start with the sequence header OBU, followed by one
  `OBU_FRAME` (frame header + tile group). Every frame is shown, in input order.
  `Encoder::av1c()` returns the `AV1CodecConfigurationRecord` (also the WebM `CodecPrivate`).
- **Frames**: a `KEY_FRAME` every `keyint` frames, `INTER_FRAME`s otherwise. Inter frames use
  slot 0 (the previous reconstruction) for all seven references and refresh slot 0 only, so
  `LAST_FRAME` is always the previous frame. CDFs adapt per symbol and are saved at the end of
  every frame; inter frames load them from `LAST_FRAME` (`primary_ref_frame` = 0). One tile,
  `TX_MODE_LARGEST`, `reduced_tx_set`, no segmentation, no delta q / lf.
- **Deblocking**: the normative loop filter (7.14) runs on the reconstruction; the frame level
  (the same for both luma directions and both chroma planes, no deltas, sharpness 0) is searched
  per frame for the lowest luma error.
- **Tiles**: the multi-symbol arithmetic encoder is the exact inverse of the spec's symbol
  decoder (8.2), including the exit process padding; CDF adaptation as in 8.2.6.
- **Partitions**: 64x64 superblocks, `PARTITION_NONE` / `PARTITION_SPLIT` down to 8x8 chosen by
  a rate-distortion cost (SSE + λ·estimated bits); blocks crossing the frame edge are split as
  the syntax requires. 64x64 blocks are only used as inter skip blocks.
- **Intra**: DC, V, H, Paeth, smooth, smooth-V, smooth-H for luma and chroma,
  ranked by SATD with a rate-distortion choice among the best three; the spec's edge preparation
  and above-right / below-left availability.
- **Inter**: single reference (`LAST_FRAME`), `NEWMV` / `NEARESTMV` / `NEARMV` / `GLOBALMV`
  chosen from the spec's motion vector prediction stack (7.10.2, mirrored exactly); motion search
  from neighbour / co-located / parent candidates, a whole-sample diamond search and half- then
  quarter-sample refinement (`allow_high_precision_mv` = 0); the normative 8-tap `EIGHTTAP`
  prediction for luma and chroma; skip blocks; per-block intra / inter choice.
- **Residual**: one transform per plane and block (4x4 ... 32x32): for intra luma `DCT_DCT` or
  the mode's preferred `ADST` combination (rate-distortion choice over the two best modes), the
  mode-dependent type for intra chroma, `DCT_DCT` for inter blocks; dead-zone quantisation with the
  spec's `Dc_Qlookup` / `Ac_Qlookup`; the normative dequantisation and inverse transforms in the
  reconstruction loop; full coefficient coding (all_zero, tx type, eob_pt, eob_extra,
  coeff_base_eob, coeff_base, coeff_br, dc_sign, Golomb) with the exact contexts.
- **Rate control**: `ConstantQ(base_q_idx)`; `Bitrate { kbps }` adapts `base_q_idx` per frame
  towards the average target (key frames get a lower quantizer and a larger share of the budget of
  their group of `keyint` frames, the inter frames the rest; the first key frame is searched in a
  few passes, a later key frame that misses its budget by more than 19 % is coded again; the
  deviation from the budget is paid back over 16 frames).

## Limits

- One tile: width and height up to 4096 and at most 2304 superblocks (4096x2304 samples).
- No CDEF or loop restoration, no compound prediction, no rectangular or 4xN blocks, no
  directional intra modes other than V / H, no CfL or palette, no transform-size splits, so
  compression is below mature encoders.
- 4:2:0 only (Main profile), no 12 bit.

## Specification and provenance

Implemented from the **AV1 Bitstream & Decoding Process Specification, version 1.0.0 with
Errata 1** (AOMediaCodec, 2019; the edition FilmCraft's decoder follows) and the **AV1 Codec
ISO Media File Format Binding** (v1.2.0) for `av1C`.

The constant tables (default CDFs, scan orders, quantizer lookup tables, interpolation filter
taps, smooth weights, block / transform size tables, ...) and the decoder-side processes the
encoder must reproduce exactly (inverse transforms, intra and inter prediction rounding, context
derivations, motion vector prediction) are mirrored from FilmCraft's first-party, spec-derived
`filmcraft-av1` decoder (MIT OR Apache-2.0), which is also the round-trip oracle. No source of
libaom, dav1d, rav1e, SVT-AV1, ffmpeg or any other third-party encoder or decoder was consulted;
ffmpeg / libdav1d is used only as an external test oracle.
