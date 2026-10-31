# Exporting a tracked camera to Blender

After the **3D Camera Tracker** finishes solving, click **Export Scene to Blender...** in its
Effect Controls and save the `.py` file. In Blender 4.0 or newer, open it in the Scripting
workspace's Text Editor and choose **Run Script**. The script creates and activates a new scene;
it preserves existing scenes and objects. Save the resulting `.blend` normally.

The script uses Blender's public Python API, with no add-on or Python dependencies beyond
Blender itself. For unattended use: `blender --background --python "Tracked Scene.py"`.
That imports the scene in memory; it does not automatically save a `.blend`.

## Transferred data

- Camera position and quaternion rotation baked on every analyzed frame, with consistent
  quaternion signs and linear interpolation; zoom/lens animation, including Variable Zoom.
- Tracking input/video resolution, horizontal sensor fit, exact rational composition FPS,
  frame range and composition markers within the analyzed interval. The first analyzed frame
  becomes Blender frame 1 by default; the original composition timestamps remain in metadata.
- The original video or still as the camera's background image, at full opacity, refreshed on
  frame changes. Source-frame offsets are baked too, accounting for trim, stretch, source FPS
  and Time Remap. This is a viewport background, not a render/compositor background.
- Focus distance, DOF switch, f-stop, iris blade count, rotation and aspect sampled from the
  tracker's linked **3D Tracker Camera**, including keyframes and expressions. Create that
  camera and adjust its Camera Options before exporting to transfer custom focus settings.
- Solved 3D points as a non-rendering vertex mesh, with point attributes for track ID,
  reprojection error and first/last visible frame. A chosen ground plane becomes Blender's XY
  plane, with its origin preserved and a non-rendering wire plane as a placement guide.
- Shutter duration (not automatically enabled), plus the original shutter phase in metadata.
- A Blender Text datablock named `EffectCraft Tracking Metadata.json` containing the complete
  solve, optional original 2D tracks, per-frame timings, source interpretation, diagnostics and
  export warnings. Radial distortion coefficients are also camera custom properties.

Coordinates are converted from EffectCraft's right/down/forward world to Blender's
right/forward/up world, including the camera's local up and negative-Z viewing direction.
Export uses the tracking input's framing, not the footage layer's final 2D placement in the
composition. Camera pose and lens always come from the solve; subsequent manual pose/lens edits
on the linked camera are not substituted for the tracked motion.

## What cannot be inferred

Monocular tracking cannot recover absolute scene scale, physical sensor size, optical focus,
f-stop or exposure from feature tracks. The default sensor width is an assumption of **36 mm**;
the focal length in millimeters is derived from it while preserving the solved field of view.
The default scale is **0.01 Blender units per composition-world pixel**, not a measurement.
Without a linked camera, DOF is disabled and focus distance uses the default solved scene depth.
Blender's physical DOF approximates EffectCraft's blur model: Blur Level and Iris Roundness are
retained in metadata, but have no direct Blender equivalent.

Brown-model radial `k1`/`k2` coefficients use radius normalized to half the image diagonal.
They are preserved, **not** assigned to Blender's different lens-distortion model. For a precise
match when distortion is nonzero, use an undistorted plate; the script prints a warning.
The tracker assumes square pixels, so Blender renders square pixels; any non-square source
interpretation is retained in metadata and flagged.

The `.py` references the video; it does not contain a copy of the video's bytes. When moving it,
keep the video next to the script (same filename), or relink the background image in Blender.
Missing/unsupported media produces a warning while retaining the imported camera and geometry.
Masks and effects applied before the tracker are not baked into the original media. Export
requires a direct video/still footage layer at the analyzed size; render precomps, sequences or
size-changing preprocessing to a video and track that video first.

## Agent / control-channel command

```json
{
  "command": "camera.exportBlender",
  "params": {
    "layer": "#1",
    "path": "/output/Tracked Scene.py",
    "sensorWidth": 36,
    "unitScale": 0.01,
    "startFrame": 1,
    "includePoints": true
  }
}
```

`comp`, `layer` and `effect` select the tracker as with `camera.analyze`. Without explicit
selection, the selected tracker (or first tracker in the composition) is used. `sensorWidth`,
`unitScale`, `startFrame` and `includePoints` are optional. The reply contains file size,
exported frame/point counts and warnings. Writing uses the host's atomic file service.
Calls issued by scripts require **Allow Scripts to Write Files and Access Network** in
Settings ▸ Scripting & Expressions. Export from the UI or control channel remains available
with that preference off.

Export is limited to 100,000 analyzed frames, 100,000 points and 64 MiB of serialized scene data;
invalid, stale or incomplete solves return an actionable error without modifying the document.

## Verification

Engine tests check argument validation, escaped/Unicode paths, fractional FPS, trim/stretch,
focus/f-stop, zoom, ground placement, malformed solves and write failures. To additionally run
the generated artifact against an installed Blender's public API:

```sh
EFFECTCRAFT_BLENDER=/path/to/blender cargo test -p effectcraft-engine tests::blender -- --nocapture
```

Optionally set `EFFECTCRAFT_BLENDER_PLATE` to a local synthetic 640×360 video to exercise media
loading and source-frame offsets. The oracle checks every synthetic point's projection through
the imported camera on every frame, as well as animation, DOF, timing and scene preservation.
