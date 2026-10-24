# effectcraft-vp8enc

A VP8 key-frame encoder: the lossy picture inside a WebP image or animation frame (a `VP8 `
chunk). WebP's lossy frames are VP8 key frames, so that is all this encodes; there are no inter
frames.

* **Prediction**: every macroblock is coded with the best of the four 16×16 luma modes (DC, V, H,
  TM, with the second-order Walsh–Hadamard "Y2" block of the subblocks' DC coefficients) or
  B_PRED (each 4×4 subblock with the best of its ten modes, predicted from the subblocks before
  it), and the best of the four 8×8 chroma modes.
* **Choice**: by rate–distortion cost, squared error plus λ times the bits, the bits counted from
  the token and mode probabilities in the contexts a decoder uses.
* **Quantization**: one quality setting (0–100) picks the frame's quantizer index; luma DC, Y2 and
  chroma factors follow from it as the format derives them. Macroblocks left with nothing to code
  are skipped (`mb_skip_coeff`).
* **Probabilities**: the coefficient token probabilities are updated per frame where the frame's
  own token counts save more than the update costs.
* **Loop filter**: the level is chosen from the quantizer (normal filter, sharpness 0). Intra
  prediction reads the unfiltered reconstruction, so the filter shapes only the decoded picture.
* **Colour**: RGBA → BT.601 studio-range Y′CbCr 4:2:0, chroma averaged per 2×2 weighted by alpha
  so colour under full transparency does not tint visible edges.

The encoder reconstructs exactly what a decoder reconstructs before loop filtering (prediction
with the frame-edge values of §12, the 16-bit inverse transforms of §14) and returns it with the
frame; the tests require a decoder (ffmpeg, when installed) to reproduce it exactly, and the
`image` crate's WebP decoder to read the frame back.

Written from **RFC 6386, *VP8 Data Format and Decoding Guide* (November 2011)**, sections 1–19 (the
format description and Annex A's bitstream syntax). The constant tables (§11.5, §13.4, §13.5,
§14.1) are generated from the RFC's text. Its Attachment One, the reference decoder source, was
not used; neither was any other encoder or decoder's source. Where the RFC's text leaves a detail
to that source — the coefficient scan order and the Y2 and chroma dequantization factors — the
encoder uses the format's known values, checked by decoding its output with independent decoders.
The forward transforms are derived from §14's inverse transforms (see `src/transform.rs`).
