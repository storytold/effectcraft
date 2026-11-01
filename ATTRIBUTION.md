# Attribution

Every non-code asset in EffectCraft, with its author, source and licence. Each repository file listed here also has
a `<file>.attribution` sidecar with full details. The rules are in [AGENTS.md](AGENTS.md) §1: no Adobe
iconography, images or other Adobe assets; open licences only; every asset attributed.
`cargo xtask assets` checks that this index and the sidecars cover every asset in the repository.
The complete licence texts and copyright notices for the bundled fonts, including the fonts embedded
by the pinned egui dependency, are reproduced in [`NOTICE`](NOTICE), which every package ships; the
`assets/fonts/OFL-*.txt` files are the upstream copies kept beside the fonts.

## Bundled dependency fonts

These fonts are embedded by the pinned egui dependency (epaint_default_fonts 0.37 at
`6d518fc799681571963842f95b55a6b68431b15a`); no font file is added to this repository.
egui-icons is always embedded, `default_fonts` adds Ubuntu Light and Hack, and
`monochrome_emoji_fonts` adds Noto Emoji and emoji-icon-font. The complete copyright notices
and licence texts are in [`NOTICE`](NOTICE).

| Asset | Author | Source | Licence |
|---|---|---|---|
| `epaint_default_fonts/fonts/egui-icons.ttf` | John Slegers (emoji-icon-font) | https://github.com/storytold/egui/blob/6d518fc799681571963842f95b55a6b68431b15a/crates/epaint_default_fonts/fonts/egui-icons.txt; original https://github.com/jslegers/emoji-icon-font | MIT (complete copyright and permission text in `NOTICE`) |
| `epaint_default_fonts/fonts/emoji-icon-font.ttf` | John Slegers | https://github.com/storytold/egui/blob/6d518fc799681571963842f95b55a6b68431b15a/crates/epaint_default_fonts/fonts/emoji-icon-font-mit-license.txt; original https://github.com/jslegers/emoji-icon-font | MIT (complete copyright and permission text in `NOTICE`) |
| `epaint_default_fonts/fonts/NotoEmoji-Regular.ttf` | Google Inc. (The Noto Project Authors) | https://github.com/storytold/egui/blob/6d518fc799681571963842f95b55a6b68431b15a/crates/epaint_default_fonts/fonts/OFL.txt | OFL-1.1 (complete text in `NOTICE`) |
| `epaint_default_fonts/fonts/Ubuntu-Light.ttf` | Canonical Ltd. and Dalton Maag | https://github.com/storytold/egui/blob/6d518fc799681571963842f95b55a6b68431b15a/crates/epaint_default_fonts/fonts/UFL.txt | Ubuntu Font Licence 1.0 (complete text in `NOTICE`) |
| `epaint_default_fonts/fonts/Hack-Regular.ttf` | Source Foundry Authors; Bitstream Inc. | https://github.com/storytold/egui/blob/6d518fc799681571963842f95b55a6b68431b15a/crates/epaint_default_fonts/fonts/Hack-Regular.txt; original https://github.com/source-foundry/Hack | MIT AND Bitstream-Vera (complete texts in `NOTICE`) |

## Asset files

| Asset | Author | Source | Licence |
|---|---|---|---|
| `assets/fonts/Inter-Regular.ttf` | The Inter Project Authors | https://github.com/rsms/inter | OFL-1.1 (`assets/fonts/OFL-Inter.txt`) |
| `assets/fonts/Inter-Medium.ttf` | The Inter Project Authors | https://github.com/rsms/inter | OFL-1.1 (`assets/fonts/OFL-Inter.txt`) |
| `assets/fonts/Inter-SemiBold.ttf` | The Inter Project Authors | https://github.com/rsms/inter | OFL-1.1 (`assets/fonts/OFL-Inter.txt`) |
| `assets/fonts/Inter-Bold.ttf` | The Inter Project Authors | https://github.com/rsms/inter | OFL-1.1 (`assets/fonts/OFL-Inter.txt`) |
| `assets/fonts/Inter-Italic.ttf` | The Inter Project Authors | https://github.com/rsms/inter | OFL-1.1 (`assets/fonts/OFL-Inter.txt`) |
| `assets/fonts/NotoSerif-Regular.ttf` | The Noto Project Authors | https://github.com/notofonts/latin-greek-cyrillic | OFL-1.1 (`assets/fonts/OFL-NotoSerif.txt`) |
| `assets/fonts/NotoSansSymbols2-Regular.ttf` | The Noto Project Authors | https://github.com/notofonts/symbols | OFL-1.1 (`assets/fonts/OFL-NotoSansSymbols2.txt`) |
| `assets/fonts/JetBrainsMono-Regular.ttf` | The JetBrains Mono Project Authors | https://github.com/JetBrains/JetBrainsMono | OFL-1.1 (`assets/fonts/OFL-JetBrainsMono.txt`) |
| `apps/effectcraft-web/web/favicon.svg` | EffectCraft contributors | Original work: web app icon, a copy of `assets/app-icon/effectcraft-small.svg` (the unicorn app icon) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) |
| `packaging/macos/dmg/background.svg` | @XusBadia | Original work: macOS DMG Finder window background, derived from the EffectCraft app icon (`assets/app-icon/effectcraft.svg`); rendered to a TIFF by `packaging/macos/package.sh` at package time | MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`) |
| `assets/app-icon/effectcraft-1024.png` | EffectCraft contributors | Original work: effectCraft app icon, 1024 px (owner's ArtCraft unicorn drawing, vectorised) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) |
| `assets/app-icon/effectcraft-macos-512.png` | EffectCraft contributors | Original work: effectCraft runtime Dock icon (macOS) (owner's ArtCraft unicorn drawing, vectorised) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) |
| `assets/app-icon/effectcraft-small.svg` | EffectCraft contributors | Original work: effectCraft app icon, light vector (owner's ArtCraft unicorn drawing, vectorised) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) |
| `assets/app-icon/effectcraft.icns` | EffectCraft contributors | Original work: effectCraft macOS app icon (owner's ArtCraft unicorn drawing, vectorised) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) |
| `assets/app-icon/effectcraft.ico` | EffectCraft contributors | Original work: effectCraft Windows app icon (owner's ArtCraft unicorn drawing, vectorised) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) |
| `assets/app-icon/effectcraft.svg` | EffectCraft contributors | Original work: effectCraft app icon, master vector (owner's ArtCraft unicorn drawing, vectorised) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) |
| `assets/app-icon/hicolor/128x128/apps/ai.storyteller.effectcraft.png` | EffectCraft contributors | Original work: effectCraft app icon, Linux hicolor 128x128 (owner's ArtCraft unicorn drawing, vectorised) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) |
| `assets/app-icon/hicolor/16x16/apps/ai.storyteller.effectcraft.png` | EffectCraft contributors | Original work: effectCraft app icon, Linux hicolor 16x16 (owner's ArtCraft unicorn drawing, vectorised) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) |
| `assets/app-icon/hicolor/24x24/apps/ai.storyteller.effectcraft.png` | EffectCraft contributors | Original work: effectCraft app icon, Linux hicolor 24x24 (owner's ArtCraft unicorn drawing, vectorised) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) |
| `assets/app-icon/hicolor/256x256/apps/ai.storyteller.effectcraft.png` | EffectCraft contributors | Original work: effectCraft app icon, Linux hicolor 256x256 (owner's ArtCraft unicorn drawing, vectorised) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) |
| `assets/app-icon/hicolor/32x32/apps/ai.storyteller.effectcraft.png` | EffectCraft contributors | Original work: effectCraft app icon, Linux hicolor 32x32 (owner's ArtCraft unicorn drawing, vectorised) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) |
| `assets/app-icon/hicolor/48x48/apps/ai.storyteller.effectcraft.png` | EffectCraft contributors | Original work: effectCraft app icon, Linux hicolor 48x48 (owner's ArtCraft unicorn drawing, vectorised) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) |
| `assets/app-icon/hicolor/512x512/apps/ai.storyteller.effectcraft.png` | EffectCraft contributors | Original work: effectCraft app icon, Linux hicolor 512x512 (owner's ArtCraft unicorn drawing, vectorised) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) |
| `assets/app-icon/hicolor/64x64/apps/ai.storyteller.effectcraft.png` | EffectCraft contributors | Original work: effectCraft app icon, Linux hicolor 64x64 (owner's ArtCraft unicorn drawing, vectorised) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) |
| `assets/app-icon/hicolor/scalable/apps/ai.storyteller.effectcraft.svg` | EffectCraft contributors | Original work: effectCraft app icon, Linux hicolor scalable (owner's ArtCraft unicorn drawing, vectorised) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) |
| `crates/lottie/tests/fixtures/bouncing-ball.json` | EffectCraft contributors | Original work: hand-written Lottie test fixture (bouncing ball, eased position keys) | MIT OR Apache-2.0 |
| `crates/lottie/tests/fixtures/shapes-gradient.json` | EffectCraft contributors | Original work: hand-written Lottie test fixture (star, rectangle, gradient fill, dashed stroke, trim paths) | MIT OR Apache-2.0 |
| `crates/lottie/tests/fixtures/precomp-matte-text.json` | EffectCraft contributors | Original work: hand-written Lottie test fixture (precomp, track matte, parented text) | MIT OR Apache-2.0 |
| `crates/lottie/tests/fixtures/legacy-masks.json` | EffectCraft contributors | Original work: hand-written Lottie test fixture (legacy keyframes, masks) | MIT OR Apache-2.0 |
| `crates/svg/tests/fixtures/basic-shapes.svg` | EffectCraft contributors | Original work: hand-written SVG test fixture (basic shapes, groups, transforms) | MIT OR Apache-2.0 |
| `crates/svg/tests/fixtures/paths-gradients.svg` | EffectCraft contributors | Original work: hand-written SVG test fixture (path data, gradients, CSS, use) | MIT OR Apache-2.0 |
| `crates/model/tests/fixtures/quad.gltf` | EffectCraft contributors | Original work: glTF 2.0 test fixture (textured quad, node hierarchy, translation animation; data URIs) | MIT OR Apache-2.0 |
| `crates/model/tests/fixtures/quad.glb` | EffectCraft contributors | Original work: binary glTF test fixture (the same quad with its texture in the BIN chunk) | MIT OR Apache-2.0 |
| `crates/model/tests/fixtures/cube.obj` | EffectCraft contributors | Original work: Wavefront OBJ test fixture (unit cube, two materials) | MIT OR Apache-2.0 |
| `crates/model/tests/fixtures/cube.mtl` | EffectCraft contributors | Original work: MTL test fixture (materials of cube.obj) | MIT OR Apache-2.0 |

## Screenshots

Screenshots of EffectCraft itself, rendered headlessly, showing only procedurally generated demo content.

| Asset | Author | Source | Licence |
|---|---|---|---|
| `docs/images/effectcraft-hero.png` | EffectCraft contributors | Original work: screenshot of EffectCraft (EffectCraft main window) | MIT OR Apache-2.0 |
| `docs/images/effectcraft-3d.png` | EffectCraft contributors | Original work: screenshot of EffectCraft (EffectCraft Classic 3D) | MIT OR Apache-2.0 |
| `docs/images/effectcraft-graph-editor.png` | EffectCraft contributors | Original work: screenshot of EffectCraft (EffectCraft Graph Editor) | MIT OR Apache-2.0 |
| `docs/images/effectcraft-effects.png` | EffectCraft contributors | Original work: screenshot of EffectCraft (EffectCraft effects) | MIT OR Apache-2.0 |
| `docs/images/effectcraft-render-queue.png` | EffectCraft contributors | Original work: screenshot of EffectCraft (EffectCraft Render Queue panel with the demo compositions queued for H.264 and ProRes export) | MIT OR Apache-2.0 |

## Code-drawn assets (original work, MIT OR Apache-2.0)

- UI icons drawn in code: `crates/ui-egui/src/icons.rs`, including the original filled favorite star (2026-10-11, EffectCraft contributors).
- Procedural demo content (solids, shapes, gradients, text animations): `crates/engine/src/demo.rs`.
- Built-in project templates (Lower Third, Title Card, Logo Reveal, Kinetic Type, Social Square, Vertical Story,
  Slideshow, 3D Text Orbit) built from engine commands, and their thumbnails rendered by our own renderer:
  `crates/engine/src/templates.rs`.
- Text animator presets (Typewriter, Fade Up Characters, Bounce In Words, Tracking In, Scramble, Blur In, Jitter,
  Drop In Lines): `crates/engine/presets/text_animators.json` (sidecar `.attribution`).
- Transition animation presets (23: wipes, clock and fan reveals, blinds, dissolves, card flips, CC wipes, fades):
  `crates/engine/presets/transitions.json` (sidecar `.attribution`).
- Backgrounds animation presets (23: drifting gradients, fog, cloud and smoke, plasma and magma, cells and crystals, scrolling grids
  and checks, pulse rings, bokeh, light sweep): `crates/engine/presets/backgrounds.json` (sidecar `.attribution`).
- Behaviors animation presets (22: wiggle, shake, float, orbit, bounce, drift, pendulum, spin, pulse, heartbeat, blink, strobe, flicker):
  `crates/engine/presets/behaviors.json` (sidecar `.attribution`).
- Sample scripts (File ▸ Scripts) and ScriptUI panel: `crates/engine/scripts/Rename Layers.jsx`,
  `crates/engine/scripts/Sort Layers by In Point.jsx`, `crates/engine/scripts/Create Null at Selected Layers.jsx`,
  `crates/engine/scripts/Render Queue Batch.jsx`, `crates/engine/scripts/ScriptUI Panels/Layer Tools.jsx`
  (sidecars `.attribution`).
- Bundled extension (ScriptUI panel) with its twelve built-in ease presets:
  `extensions/scriptui-panels/Ease Presets.jsx` (sidecar `.attribution`).
- Example effect plug-in: `examples/plugins/posterize-bands` (plug-in API v1, WebAssembly).

## Trained models (optional downloads, not bundled)

The repository and the installers contain no model weights. These models are downloaded only when
a user asks (Settings ▸ Roto Brush, Settings ▸ Face Tracking, or the `roto.model.*` /
`face.model.*` commands), unmodified from their authors' official location, checked against a
pinned SHA-256 and stored in the user's `models` folder with a `<file>.NOTICE.txt` naming their
authors, licence and source. EffectCraft runs them with its own Rust implementation and is not
affiliated with or endorsed by their authors.

| Model | Authors | Source | Licence |
|---|---|---|---|
| MobileSAM (`mobile_sam.pt`) | Chaoning Zhang et al., building on Meta AI's Segment Anything and Microsoft's TinyViT | https://github.com/ChaoningZhang/MobileSAM | Apache-2.0 |
| MediaPipe Face Landmarker (`face_landmarker.task`: BlazeFace short-range detector, Face Mesh V2, blendshapes) | Google LLC | https://developers.google.com/edge/mediapipe/solutions/vision/face_landmarker (model cards: https://storage.googleapis.com/mediapipe-assets/MediaPipe%20BlazeFace%20Model%20Card%20(Short%20Range).pdf, https://storage.googleapis.com/mediapipe-assets/Model%20Card%20MediaPipe%20Face%20Mesh%20V2.pdf) | Apache-2.0 |

## Formats and parameters from other open-source projects

Written in Rust for EffectCraft; no code is copied. They follow published specifications and
configuration from these Apache-2.0 projects:

- `crates/segment/src/tflite.rs` reads TensorFlow Lite files using the field layout of TensorFlow
  Lite's `schema.fbs` (The TensorFlow Authors, https://github.com/tensorflow/tensorflow,
  Apache-2.0).
- `crates/segment/src/mediapipe.rs` follows MediaPipe's published face landmarker graph: the SSD
  anchor layout, detection decoding, crop sizes and the face-mesh point indices it uses (Google
  LLC, https://github.com/google-ai-edge/mediapipe, Apache-2.0).
- `crates/segment/src/mobilesam.rs` follows the MobileSAM, TinyViT and Segment Anything
  architectures and their Apache-2.0 reference code.

## First-party brand marks

The ArtCraft name, wordmark and logos are trademarks of the ArtCraft Team. They are not open source and are not
covered by the project licence; they may be used only unmodified and only as part of this repository and EffectCraft,
under [`docs/brand/LICENSE-brand.txt`](docs/brand/LICENSE-brand.txt). Forks and modified versions must remove them
(AGENTS.md §1.8).

| File | Author | Source | Licence |
|---|---|---|---|
| `docs/brand/artcraft-logo-white.png` | ArtCraft Team | Original work: first-party ArtCraft brand mark (ArtCraft wordmark, light ink for dark backgrounds) | ArtCraft brand terms, not open source (`docs/brand/LICENSE-brand.txt`) |
| `docs/brand/artcraft-logo-white.svg` | ArtCraft Team | Original work: first-party ArtCraft brand mark (ArtCraft wordmark, light ink for dark backgrounds) | ArtCraft brand terms, not open source (`docs/brand/LICENSE-brand.txt`) |
| `docs/brand/artcraft-logo.png` | ArtCraft Team | Original work: first-party ArtCraft brand mark (ArtCraft wordmark (symbol + "ARTCRAFT"), dark ink for light backgrounds) | ArtCraft brand terms, not open source (`docs/brand/LICENSE-brand.txt`) |
| `docs/brand/artcraft-logo.svg` | ArtCraft Team | Original work: first-party ArtCraft brand mark (ArtCraft wordmark (symbol + "ARTCRAFT"), dark ink for light backgrounds) | ArtCraft brand terms, not open source (`docs/brand/LICENSE-brand.txt`) |
| `docs/brand/artcraft-mark-black.png` | ArtCraft Team | Original work: first-party ArtCraft brand mark (ArtCraft symbol ("A" mark), black) | ArtCraft brand terms, not open source (`docs/brand/LICENSE-brand.txt`) |
| `docs/brand/artcraft-mark-black.svg` | ArtCraft Team | Original work: first-party ArtCraft brand mark (ArtCraft symbol ("A" mark), black) | ArtCraft brand terms, not open source (`docs/brand/LICENSE-brand.txt`) |
| `docs/brand/artcraft-mark.png` | ArtCraft Team | Original work: first-party ArtCraft brand mark (ArtCraft symbol ("A" mark), blue) | ArtCraft brand terms, not open source (`docs/brand/LICENSE-brand.txt`) |
| `docs/brand/artcraft-mark.svg` | ArtCraft Team | Original work: first-party ArtCraft brand mark (ArtCraft symbol ("A" mark), blue) | ArtCraft brand terms, not open source (`docs/brand/LICENSE-brand.txt`) |

- Appearance theme previews, drawn procedurally from original shapes: `crates/ui-egui/src/panels/settings.rs` (original work, MIT OR Apache-2.0).
