# effectcraft-av1enc

Clean-room, pure-Rust AV1 encoder for EffectCraft's MP4 / WebM export (layer L0, `std` and rayon,
no `unsafe`, builds for `wasm32-unknown-unknown`).

Output is decoded **bit-exactly** by libdav1d (`ffmpeg -c:v libdav1d`, checked in
`tests/ffmpeg_oracle.rs` when ffmpeg is installed) and by FilmCraft's spec-derived
`filmcraft-av1` decoder (`tests/roundtrip.rs`): every decoded frame equals the encoder's own
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
  every frame; inter frames load them from `LAST_FRAME` (`primary_ref_frame` = 0).
  `TX_MODE_LARGEST`, `reduced_tx_set`, no segmentation, no delta q / lf.
- **Tile layout**: uniform tile spacing chosen from the frame size alone (never from the machine
  or thread count): tile columns are added while each stays at least 16 superblocks (1024
  samples) wide, and at least as many as MAX_TILE_WIDTH (4096) requires; tile rows only when a
  tile would exceed MAX_TILE_AREA (4096x2304). 1920x1080 is one tile, 3840x2160 two columns,
  7680x4320 four. Each tile starts from the frame's CDFs with a cleared above context, and
  AvailU / AvailL, intra edges (BlockDecoded) and motion vector candidates stop at its borders;
  the CDFs saved for the next frame are tile 0's (`context_update_tile_id` = 0). Every tile but
  the last is preceded by its size in the fewest bytes that fit (`tile_size_bytes_minus_1`).
  Tiles cost compression (about 0.6 % in size for two columns at 1080p, hence the 1024-sample
  minimum; nothing measurable at 2160p). Frames that now get more than one tile (3840x2160 and
  larger) code to different bytes than earlier versions' single tile; frames that stay one tile
  (1920x1080 and smaller) are unchanged.
- **Deblocking**: the normative loop filter (7.14) runs on the reconstruction; the frame level
  (the same for both luma directions and both chroma planes, no deltas, sharpness 0) is searched
  per frame for the lowest luma error.
- **Symbol coding**: the multi-symbol arithmetic encoder is the exact inverse of the spec's symbol
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
  towards the average target (key frames get a lower quantizer and a larger share; the first
  key frame is searched in a few passes).
- **Speed**: superblock rows are decided in parallel (rayon) as a wavefront, each row two
  superblocks behind the row above, since a decision reads only the last reconstructed row and
  motion vectors of the row above up to the superblock above-right. The output is bit-identical
  to serial decision order for the same tile layout, regardless of thread count
  (`tests/roundtrip.rs` pins the bitstream bytes). Changing the automatic tile layout can change
  the bitstream: 1080p uses one column, while 3840×2160 uses two. The split follows superblock
  width, so “below 4K” is not a compatibility boundary. Tiles are written in parallel. The loop filter runs its vertical-edge pass on superblock
  rows and its horizontal-edge pass on superblock columns in parallel (an edge only changes
  samples in its own row or column of 4x4 units, so the result equals the raster order), for
  the level search too. Transform trials and block decisions reuse scratch buffers.

## Limits

- Frames up to the level 6 limits: 16384 samples per side and 35 651 584 in all.
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
