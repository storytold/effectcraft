# effectcraft-vp9enc

A small VP9 encoder for WebM export (profile 0, 8-bit 4:2:0, BT.709).

* **Key frames**: 8×8 intra blocks (DC / V / H / TM prediction chosen per block), 4×4 DCT/ADST
  transforms, or the Walsh–Hadamard transform at quality 100 (mathematically lossless).
* **Inter frames**: one reference (the previous frame, LAST), 16×16 or 8×8 blocks chosen by a
  cost comparison; motion search from the predicted vectors (NearestMv / NearMv / last frame's
  co-located vector) with a whole-sample cross/diamond search refined to half and quarter
  samples with the regular 8-tap interpolation filters; NEARESTMV / NEARMV / ZEROMV / NEWMV
  chosen by bit cost; blocks with no coefficients worth coding are skipped; 4×4 DCT residual
  with a rate–distortion check per transform block. Inter frames are coded error resilient
  (`error_resilient_mode` = 1: no dependence on earlier probability contexts or motion vectors),
  so `UsePrevFrameMvs` is never needed. No high-precision (1/8) vectors, no compound prediction,
  no intra blocks inside inter frames.
* **Loop filter**: the in-loop deblocking filter (§8.8) on every lossy frame; the level is the
  best of three trials against the source (sharpness 0, no mode/reference deltas).
* **Probabilities**: per-frame forward updates in the compressed header (`diff_update_prob` /
  `update_mv_prob`) of the coefficient, skip, inter-mode, is-inter, reference, partition and
  motion-vector probabilities. Tiles are recorded as symbols first, the frame's own counts pick
  the updates that pay for their signalling, then the symbols are entropy coded with the updated
  probabilities. There is no backward adaptation (`refresh_frame_context` = 0).
* **Rate control**: constant quality (quality → `base_q_idx`) or a target bitrate: a per-frame
  q index from a bits ≈ complexity / step model per frame type, key frames getting a larger share
  of each key-frame interval, with the running over/undershoot paid back over about a second.
* **Key frame interval** is configurable (1 = every frame a key frame, the old intra-only mode).
* **Threads**: up to four tile columns, as many as the width allows (2 from 512 px, 4 from
  1024 px), code in parallel; so do the colour conversion, the reference's quarter-sample phase
  planes, the loop-filter level trials and the chroma loop filter. The tile layout depends on the
  frame size only, so a frame codes to the same bytes on every machine. Frames 512 px or wider
  therefore code to different bytes than before tile columns were added (deterministically, and
  still decoded bit-exactly); narrower frames are unchanged.

The encoder reconstructs exactly what a decoder reconstructs (inter prediction with the
decoder's vector clamping and edge-clamped reference fetches, integer inverse transforms, the
loop filter) and exposes it (`Vp9Encoder::reconstruction`); the tests require FilmCraft's VP9
decoder (and ffmpeg, when installed) to output exactly that reconstruction, frame by frame, for
moving synthetic sequences at several sizes and qualities, with tile columns, and lossless.

On a moving-gradient sequence (192×108, 24 frames) inter coding is 3.9–7× smaller than all-intra
at the same quality setting with equal or better PSNR.

Written from the **VP9 Bitstream & Decoding Process Specification, version 0.6 (31 March
2016)**: the encoder emits the uncompressed and compressed headers (§6.2, §6.3), partition, mode
info, motion vector and token syntax (§6.4), derives motion vector candidates with the §6.4
`find_mv_refs` / `find_best_ref_mvs` processes, and reconstructs with the specification's intra
and inter prediction (§8.5), integer inverse transforms (§8.7) and loop filter (§8.8) so it stays
in step with any conformant decoder. The boolean encoder is the inverse of the §9.2 decoder in
the form given in RFC 6386 §7.3. Spec tables (§10) are the specification's; the key-frame tables
were transcribed from FilmCraft's first-party decoder `filmcraft-vp9` (MIT OR Apache-2.0), which
is also the decoder the tests round-trip through and whose behaviour (loop filter edge rules,
vector candidate order) the encoder was checked against; ffmpeg is used only as an external test
oracle. No libvpx or other third-party encoder/decoder source was consulted.
