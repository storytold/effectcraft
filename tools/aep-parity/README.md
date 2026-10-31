# aep-parity: measure and close the gap between After Effects and EffectCraft renders

These scripts compare EffectCraft renders of an imported After Effects project with the videos After
Effects rendered from the same project, frame by frame. They find where the two differ, re-render the
deliverables, and fit a replacement for procedural layers whose exact pattern cannot be reproduced
clean-room.

They were written to move a production motion-design project into EffectCraft: about 170 comps and
3,200 layers, a 3D camera with depth of field, lights and Fractal Noise glows, delivered as 8 videos.
The goal was to re-render those videos with new images. The measure → fix → re-measure loop below
found the importer and engine gaps that this branch fixes. Each fix was made in the engine or the
importer, never in the tools. With the fixes and the glow fit, the EffectCraft renders could not be
told apart from the After Effects ones by eye.

Nothing here is specific to that project. Every comp name, path, layer selector and constant comes
from a JSON config.

## Requirements

- An `effectcraft-cli` release build: `cargo build --release -p effectcraft-cli`.
  - The scripts look for `--cli`, then `$EFFECTCRAFT_CLI`, then the config's `cli`, then
    `target/release/effectcraft-cli` in this checkout.
- Python 3.9 or newer. `measure.py`, `rerender.py` and `full_scores.py` need only the standard
  library. `glow_fit.py` also needs `numpy` and `Pillow`.
- `ffmpeg` and `ffprobe` on `PATH`, built with libx264. They are external programs you run on
  your own machine; EffectCraft never links, bundles or ships them. The scripts call them to:
  - read and decode the reference videos, and compute SSIM / PSNR;
  - encode the short render windows and reference copies behind the encoded and ceiling scores;
  - draw contact sheets and the side-by-side review video;
  - encode deliverables, but only with `rerender.py --encoder ffmpeg`. By default EffectCraft's own
    H.264 export writes them.
- The After Effects project. A collected project (File ▸ Dependencies ▸ Collect Files) is easiest.
- The videos After Effects rendered from it.

Run every script with `python3 -I`. Each script finds `parity_common.py` next to itself.

## Files

| file | what it does |
|---|---|
| `parity_config.example.json` | Example config with made-up names. Copy it next to your project and edit it. |
| `measure.py` | The parity harness: plain, encoded and codec-ceiling SSIM/PSNR for sampled frames, contact sheets with heat maps, and `scores.jsonl`. `--probe-offset` checks the time mapping. |
| `glow_fit.py` | Fits keyed radial-gradient fields to the reference video. They replace the content of procedural glow layers. Writes an overrides JSX. |
| `rerender.py` | Full render: import, relink, overrides, optional footage swaps, then EffectCraft's own H.264 export of each reference's frame range (or, with `--encoder ffmpeg`, chunked PNG frames encoded with the reference's x264 settings). |
| `full_scores.py` | Plain, encoded and ceiling scores over every frame of the `rerender.py` output. |
| `side_by_side.sh` | Reference and EffectCraft side by side in one video, for eyeballing. A review helper that runs your own ffmpeg. |
| `parity_common.py` | Shared helpers: config, project preparation, CLI calls, ffprobe, encoder replay, SSIM. |
| `jsx/` | Templates of the scripts run in EffectCraft's After Effects-style scripting host. The tools prepend a `var CFG = {...};` line and `_select.jsx` to each one. |
| `jsx/inventory.jsx` | Read-only listing of comps, effect matchNames, text fonts and the layers of chosen comps. Use it to write the config. |

## Config

Paths in the config are relative to the config file.

| key | meaning |
|---|---|
| `aep` | The `.aep` / `.aepx` to import (`file.importAep` with `allowLosses`). Use `project` instead to start from an existing `.ecproj`. |
| `work_dir` | Holds everything the tools write: projects, renders, scores. |
| `relink` | `[{from, to}]` path prefixes. A collected project keeps the author's absolute footage paths; footage under `from` is repointed to `to` + the rest of the path. |
| `scripts` | JSX applied to every rendered project after import and relink. The glow overrides go here once they are fitted. |
| `entries` | One per reference video. Fields are listed below. |
| `glow_fit` | Settings of `glow_fit.py`, described in that section. |

Fields of an entry:

- `name`: the entry's id.
- `comp`: the comp that was rendered.
- `reference`: the video After Effects rendered.
- `offset`: video frame = comp frame + offset.
- `frames`: how many evenly spaced frames to score, or a list of video frames.
- `vf`: an optional ffmpeg filter that maps the comp-size render to the video's stored frame, for
  example `scale=1280:720` for a 720p encode of a 1080p comp, or `crop=…,scale=…`. `rerender.py`'s
  default encoder applies only a plain `scale=W:H` (as the output module's resize); anything else
  needs `--encoder ffmpeg`.
- `project_scripts`: optional JSX applied to this entry's project only. Use it when a reference was
  rendered from an older state of the project (a footage item that has since changed) and you
  restore that state for scoring.

Projects are cached in `work_dir` by the content of the scripts applied to them. `--reimport`
re-imports the `.aep` and drops every cached project.

To write the config, list the comps and effects first:

```sh
CLI=target/release/effectcraft-cli
$CLI run file.importAep '{"path":"collect/My Project.aep","allowLosses":true}' file.saveAs '{"path":"/tmp/p.ecproj"}' --empty
(echo 'var CFG = {"comps":["Main Loop"]};'; cat tools/aep-parity/jsx/_select.jsx tools/aep-parity/jsx/inventory.jsx) > /tmp/inv.jsx
$CLI script /tmp/inv.jsx /tmp/p.ecproj
```

## Workflow

### 1. Check the time mapping, then measure

```sh
python3 -I tools/aep-parity/measure.py my.json --probe-offset main-loop      # best offset should be +0
python3 -I tools/aep-parity/measure.py my.json --run r0 --jobs 2 --notes "baseline"
```

`measure.py` renders a short window (`--enc-window`, 12 frames) around each sampled frame and scores
it three ways (SSIM All on yuv444p, with PSNR next to it):

- **plain**: the clean render against the reference. A delivered video carries codec loss, so even a
  perfect render scores below 1.
- **encoded**: the render encoded the way the reference was encoded, then scored. Encoding a window
  lets the GOP, B-frames and rate control behave as they do in the delivered file.
  - If the reference was written by x264, the options in its SEI are replayed one by one.
  - If not (Adobe Media Encoder's own H.264, for example), the GOP length, B-frame run, profile,
    level and average bit rate are measured, and an ABR encode with that shape is used.
- **ceiling**: the reference re-encoded with the same settings, scored against itself. This is
  roughly what a perfect render can reach.

`gap = ceiling − encoded` is the number to drive towards 0. A large plain-versus-encoded difference
on its own means nothing. `self` (the encoded render against the clean render) estimates first-
generation codec loss. The ceiling re-encodes content that is already encoded, so it runs slightly
high.

Results are written to `work_dir/measure/<run>/`:

- `ref-K`, `ren-K`, `raw-K`, `enc-K` and `ceil-K` PNGs for each sampled frame K.
- `<entry>-sheet.png`: one row per frame showing reference | render | encoded render | heat map of
  |encoded − reference| luma × 8.
- One JSON line per entry and run appended to `work_dir/scores.jsonl`.

Use `--frame-list ENTRY=a,b,c --suffix -tr` to score specific frames, such as transitions, which
evenly spaced samples tend to miss.

### 2. Fix what the heat maps show

The heat maps show where a frame is wrong. Make a small test project or a unit test that isolates the
behaviour, fix the importer or the engine, rebuild, and run step 1 again with a new `--run` name.

Things this loop found on the production project, all fixed on this branch with unit tests:

- separated Position X/Y/Z dimensions
- camera Blur Level and depth-of-field aperture
- light falloff
- parallel-light falloff origin
- null anchors
- manual kerning
- linear-to-Bezier key influence
- Classic 3D specular lobe
- nested motion blur under a precomp layer with its switch off
- the Camera Lens Blur mapping
- motion blur in time-stretched precomps

A fix is kept only if the gap goes down on every entry.

### 3. Fit procedural glows that cannot match (optional)

Some looks come from a generator whose exact pattern belongs to the other application. A typical
case: a Fractal Noise solid, colourised, glowed and heavily blurred into soft pools of light, in Add
mode. EffectCraft runs the same effect chain, but its noise lattice is its own, so the pools land in
different places on every frame.

`glow_fit.py` replaces the content of those layers with a field fitted to the reference:

- **The field.** A grid of radial Gradient Ramp cones in the layers' own layer space. Each cone's
  colour is keyed every `step` frames.
- **What stays.** The layers keep their masks, opacity keys, blending mode, parenting and timing. The
  field goes through the same chain as the noise did, and every other layer is left as it was.
- **The model.** Render = N + U · F.
  - N is the render with the selected layers disabled.
  - U, the transfer, is measured by rendering a flat grey field through the same chain.
  - F, the field, is solved per sampled frame with a non-negative ridge least-squares fit against
    (reference − N).
- **Precomps.** A comp holding glow layers can be used as a precomp several times. Each instance is
  fitted in that comp's own time.
- **Cross-fades.** Where two instances of different comps overlap, those frames are fitted jointly.
  Each comp's transfer is measured on its own, and a ridge term ties the result to the keys fitted on
  either side.

```sh
G="python3 -I tools/aep-parity/glow_fit.py my.json"
$G probe                      # lists the selected layers and their precomp instances
$G render && $G ref           # nn + unit renders and reference frames, every `step` frames, at 1/downscale
$G xrender && $G xref         # cross-fade frames (only if probe shows overlapping instances)
$G fit                        # -> work_dir/glow_fit/params.json and the overrides JSX
$G render --tags fit && $G check --with-fit       # RMS residual in levels: nn / predicted / fit render
$G xrender --tags fit && $G xcheck --with-fit
```

Then add the overrides file to `scripts` in the config and run `measure.py` again. You only need to
re-fit if the glow layers change; swapping footage does not need one.

Settings (`glow_fit` in the config):

| key | default | meaning |
|---|---|---|
| `select` | required | `{layer_name, effect_match_name, comps}`. A layer is selected when every given field matches. Imported Fractal Noise reports `ADBE Fractal Noise`. |
| `targets` | required | Entries whose reference drives the fit. The reference must cover the whole comp; crop maps cannot be inverted. `ref_vf` (`{entry: filter}`) adds a filter before the downscale. |
| `step` | 10 | One key every `step` target frames. |
| `downscale`, `fit_downscale` | 4, 2 | Render size, and the further box downsampling of the fit grid. |
| `grid` | spacing 160×160, radius 320 | Cone grid in layer pixels. Keep the radius about twice the spacing. Starting values, not measured truths. |
| `lambda` | 50 | Ridge weight of the per-frame fit. |
| `smooth` | 1 | [1 2 1] temporal smoothing of the keys. |
| `xfade` | step 2, pad 24, lambda 500 | Sampling, padding and ridge weight of the joint cross-fade fit. |
| `unit_level` | 0.25 | Grey level of the transfer probe. |
| `overrides` | `glow_overrides.jsx` | Output file, relative to the config. |

`fit --refine` (after `render --tags fit`) fits again on the residual of the fitted render. This
absorbs clipping and other non-linear parts of the chain.

Limits:

- The fit assumes 2D layers with static transforms. Parenting, 3D, animated transforms and
  time-stretched or remapped precomps are reported as warnings and ignored.
- Precomps are followed one level deep from the target comp.
- An instance overlapping another instance of the same comp is not jointly fitted. The single-
  instance keys are interpolated across those frames.
- The method suits low-frequency light, such as a heavily blurred glow. A cone field cannot
  reproduce detail finer than its grid. On the synthetic smoke test (a lightly blurred noise), the
  fit cut the RMS error from about 100 to about 5 levels and raised PSNR by 5–7 dB. SSIM went down
  slightly, because the noise detail was finer than the grid.
- Always check the result with `measure.py`.

### 4. Re-render the deliverables

```sh
python3 -I tools/aep-parity/rerender.py my.json --frames 0-5           # smoke test, PNG frames, no encode
python3 -I tools/aep-parity/rerender.py my.json --jobs 2 \
    --swap "images/photo-12.jpg=/abs/path/new-photo-12.jpg" --out out/
python3 -I tools/aep-parity/full_scores.py my.json --out out/            # without --swap: scores vs the originals
tools/aep-parity/side_by_side.sh refs/loop.mp4 out/main-loop.mp4 sbs.mp4 --font /path/to/a-font.ttf
```

What `rerender.py` does:

- **Swaps.** `--swap OLD=NEW` replaces a footage file, the scripting form of File ▸ Replace Footage.
  OLD is matched against the end of the footage path and must match exactly one item.
- **Encoding, by default.** EffectCraft renders each entry's frame range straight to H.264 with its
  own exporter (the render queue's H.264 output), one render process per entry, `--jobs` at a time.
  The video takes the reference's frame rate and average bit rate, and its stored size when `vf` is
  a plain `scale=W:H`. EffectCraft's H.264 encoder takes a bit rate, not x264's options, so the
  GOP, profile, level and pixel aspect are its own and no silent audio track is added.
- **Encoding with `--encoder ffmpeg`.** For a bitstream as close as possible to the reference's, the
  comp is rendered to PNG frames in `work_dir/frames/<comp>-<project>/` (`--chunk` frames per render
  process, by comp frame), then the `ffmpeg` on your `PATH` encodes them with the reference's
  geometry, pixel aspect and x264 settings (replayed from its SEI, or the measured GOP shape), plus a
  silent audio track shaped like the reference's if it has one. This is an external tool you run on
  your own machine to reproduce a reference file's exact x264 settings; EffectCraft never links,
  bundles or ships it. Use the same PNG frames with `--no-encode` to get `full_scores.py`'s plain
  score.
- **Resuming.** Projects and PNG frames already rendered are kept. A lock on `work_dir` refuses a
  second run on the same folder, because two runs would wipe each other's chunk folders.
  `--reimport` deletes the frames too. After an engine rebuild without `--reimport` they are stale:
  delete `work_dir/frames` yourself before re-rendering.

`full_scores.py` re-encodes each reference with x264 for its ceiling, so its gap compares like with
like only for videos from `--encoder ffmpeg`. Videos from EffectCraft's own exporter also carry the
difference between the two encoders.

Memory: each render process holds the whole project. A large project can take several GB per
process, so choose `--jobs` with that in mind.

## Smoke test

The scripts were exercised end to end on a synthetic project built with the CLI's scripting host:

- two precomps with a blurred Fractal Noise solid, cross-fading in a rotated, offset 320×180 comp
- a moving solid
- an image footage item that has to be relinked

The references were rendered from a copy with different noise settings and encoded with ffmpeg: one
with an x264 SEI, and one with the SEI stripped to exercise the measured-GOP path.

A generated `.aep` from the importer's test corpus (see [crates/aep](../../crates/aep/README.md);
the fixtures are opt-in through `EFFECTCRAFT_AEP_FIXTURES`) can be used the same way. Its comps have
no reference videos, so render them in After Effects first.
