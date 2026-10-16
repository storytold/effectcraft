# Object Matte: comparison plan (not measured yet)

**Status: untested.** There is no After Effects 26.2+ comparison of Object Matte yet. Nothing on
this page is a measurement. It lists what to compare once an After Effects reference session
is available, so the first run can be repeated and scored without guesswork.

EffectCraft side, as of #783: the Object Matte effect (`ec.matte.objectmatte`), the Object Matte
tool in the Composition viewer, `objectMatte.apply` / `.propagate` / `.freeze` /
`.status`, and segmentations kept in the disk cache. The point (and an optional box) is
segmented by Roto Brush's engine: MobileSAM when it is installed (Settings ▸ Roto Brush, with
the box as the model's box prompt), otherwise the classical graph cut. The result is tracked
frame by frame with optical flow, and the model re-prompts each frame from the carried matte.
Record which engine was active, because the two are separate cases.

## Test clips

All original, generated or openly licensed; none of them goes into the repository.

1. **Synthetic disk**: the moving textured disk over a textured background from
   `crates/engine/src/tests_roto.rs` (`draw(center(f))`, 320×240, 25 fps, 25 frames, radius
   40 px, about 4 px a frame to the right). Export it as a PNG sequence. Its ground truth is
   analytic (`gt(center(f))`).
2. **Synthetic disk with a jump**: the same disk with `jumping(f)` (it jumps at frame 8). This
   checks how each tool handles losing the object.
3. **Two objects**: the disk plus a second still, differently coloured square that touches it
   on some frames, prompted once with a point only and once with a box around the disk. This
   checks the box prompt.
4. **Real footage**: one openly licensed clip (CC0 or CC BY) with a person or object moving
   against clutter, at least 1080p and 5 s long. Record its URL, licence and SHA-256 next to the
   results, and keep the clip itself out of the repository.

## Prompt and settings

- Reference frame 0. The point at the object's centre, in layer pixels (clip 1: `[90, 110]`).
  The box, where used, is `[centre − 55, centre + 55]` on each axis.
- Track forward to the last frame (Frames After = clip length − 1, Frames Before = 0).
- Matte refinement at its defaults on both sides: Feather 0, Contrast 0, Shift Edge 0, no
  Invert. Output the matte itself (View ▸ Matte in EffectCraft, the alpha channel in After
  Effects) as a full-resolution PNG sequence.
- Record the After Effects build, the OS, the GPU, and the exact click position in After
  Effects. It is a viewer click, so convert it to layer pixels with the comp's zoom.

Keep the After Effects mattes in gitignored `plan/aftereffects/ref/object-matte/`. Only an
aggregate results file (like `keyframe-ease.json`) and the comparison script may be committed.

## Metrics (per frame, then mean / worst)

- **IoU against After Effects**: binarize both mattes at alpha 0.5, then intersection over
  union. This is the headline number. `effectcraft_track::roto::iou` computes it, and
  `roto.status` already reports it for Roto Brush through `compareTo`; Object Matte needs the
  same for exported mattes.
- **IoU against ground truth** for the synthetic clips, for both applications. It tells
  "differs from After Effects" apart from "worse than After Effects".
- **Boundary error**: the mean and 95th-percentile distance (px) between the two mattes' 0.5
  contours, which shows edge disagreement that IoU dilutes on large objects.
- **Soft-edge error**: the mean absolute alpha difference inside a 3 px band around the After
  Effects contour.
- **Temporal stability**: the frame-to-frame IoU change of each matte on clip 1, where the
  object moves rigidly, so jitter shows.
- **Drift**: the frame where IoU against ground truth first drops below 0.9 (clips 1 and 2).
- **Time**: seconds per frame to track the clip, warm and cold, on the same machine, with the
  disk cache on and off.

Proposed pass bar (a maintainer decision, not yet agreed): mean IoU against After Effects
≥ 0.95 and worst ≥ 0.90 on clips 1 and 3, with MobileSAM. With the classical engine the numbers
are reported but not gated.

## Reproduce (to write)

The script does not exist yet. It should: generate the synthetic clips; run EffectCraft
headless (`effectcraft-cli run … objectMatte.apply … objectMatte.propagate {"wait":true}`, then
render the matte); drive After Effects through ExtendScript or AEsync to apply Object Matte with
the same prompt and render its alpha; then compute the metrics above into a JSON results file.
