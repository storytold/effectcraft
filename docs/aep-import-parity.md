# Bringing a production After Effects project to render parity

A case study of the `aep-import-parity` branch: what it took to import a real After Effects project
and re-render its delivery videos in EffectCraft so they match the originals, and how to repeat the
process on another project with [`tools/aep-parity`](../tools/aep-parity/README.md).

## The need

The starting point was an After Effects project from a client job: a production motion-design
project of about 170 comps and 3,200 layers, with a 3D camera using depth of field, lights, and
glows made from Fractal Noise. It was rendered to 8 delivery videos. Those videos had to be
re-rendered with new images swapped in, and the goal was to do that in EffectCraft instead of After
Effects.

"Imports without errors" was not enough. The re-rendered videos replace delivered ones, so they had
to look the same as the After Effects renders, apart from the swapped images.

## Starting point: #2558

The work builds on [storytold/effectcraft#2558](https://github.com/storytold/effectcraft/pull/2558)
by **vangel76**, "After Effects project import (.aep / .aepx); export follows". That PR provides the reader, the
importer and the `file.importAep` command, and it already imported this project with a short list
of reported losses. Without it none of this would have been possible. Thank you, vangel76.

This branch is stacked on #2558: it contains its commits, and the commits listed below come on top
of them.

## Method

### Measuring against the delivered videos

The reference is the set of videos After Effects rendered, not a screenshot or a guess. For each
video, a harness (`tools/aep-parity/measure.py`) renders the same comp in EffectCraft and compares
sampled frames:

- **plain**: EffectCraft's clean render against the delivered frame. A delivered video carries codec
  loss, so a perfect render still scores below 1.
- **encoded**: EffectCraft's render, encoded the way the reference was encoded (the x264 options
  replayed from its SEI, or, for files written by Adobe Media Encoder, an ABR encode with the
  measured GOP, B-frames, profile, level and bit rate), then compared.
- **ceiling**: the reference re-encoded with the same settings and compared with itself. This is
  roughly the best a perfect render can reach.

`gap = ceiling − encoded` (SSIM) is the number to drive towards 0.

Each run also writes contact sheets: reference, render, encoded render and a heat map of the luma
difference ×8. The heat maps show where a frame is wrong (a light's falloff, a layer sitting a few
pixels off, a blur that is too strong), which is what turns a score into a test case.

### The loop

About 20 rounds of:

1. measure all 8 videos;
2. read the heat maps and isolate one behaviour;
3. build a small case that shows it, observe what After Effects does with it, and write a unit test
   for that behaviour;
4. fix the importer or the engine (never the tools), rebuild and measure again.

A fix was kept only if the gap went down on every video. Evenly spaced samples miss short
transitions, so specific frame lists (`--frame-list`) were measured as well. At the end, the
8 re-rendered videos were judged by eye on side-by-side videos, at full speed and frame by frame.

### Clean room

Every fix follows [AGENTS.md](../AGENTS.md) section 2: it is based on renders and on what After
Effects does with a given input, never on Adobe code, plug-ins or presets.

The measuring tools call ffmpeg and ffprobe, installed on the machine that runs them, as separate
programs: to decode the reference videos, to score, and to encode the render windows and reference
copies behind the encoded and ceiling scores. For this project the 8 deliverables were also encoded
with that ffmpeg, to reproduce each reference's x264 settings (`rerender.py --encoder ffmpeg`; by
default `rerender.py` uses EffectCraft's own H.264 export). EffectCraft never links, bundles or
ships ffmpeg.

## What was fixed

Each item below is one commit with a unit test. "Observed" is how the gap showed up.

### Importer (`crates/aep`)

| Change | After Effects behaviour | Observed |
|---|---|---|
| **Separated Position dimensions** | A Position with separated dimensions keeps its value and keys in `ADBE Position_0/1/2`; the combined property keeps stale keys. They now import as X / Y / Z Position, each with its own keys and expression. | Layers sat at a stale position. |
| **Camera Blur Level** | Stored in percent. Only 100 % was accepted; any other value was reported and dropped. | Depth of field too strong or too weak. |
| **Light falloff** | Falloff type (None, Smooth, Inverse Square Clamped; stored 1-based), Falloff Start (EffectCraft's Radius) and Falloff Distance. | Lights with a falloff lit far layers at full intensity. |
| **Null anchor** | A null with no stored Anchor Point anchors at its top-left corner, not its centre. | Children of such nulls sat in the wrong place. |
| **Manual kerning** | The amount stored on a character is the gap between it and the next one. It becomes the next character's kerning, or the first-line indent at the start of a left-aligned paragraph. A gap at the start of a centred, right-aligned or right-to-left paragraph is still reported. | Manually kerned text was spaced differently from After Effects. |
| **Camera Lens Blur** | Mapped to `ec.blur.cameralens`: Blur Radius, the Iris properties, Highlight Gain and Saturation, Repeat Edge Pixels, Use Linear Working Space. The Blur Map, Focal Distance and Highlight Threshold are reported. | Every instance of the effect was dropped. |

### Engine (`crates/render`, `crates/keyframe`, `crates/gpu`, `crates/lottie`)

| Change | After Effects behaviour | Observed |
|---|---|---|
| **Depth-of-field blur aperture** | After Effects blurs far less than a lens with the Aperture's diameter. The blur kept one ratio to the circle of confusion, about 0.11–0.115 of the property, scaled with the aperture and by zoom / focus distance. EffectCraft now uses Aperture × 0.115 × zoom / focus. | Out-of-focus layers were far too soft. Fitted on four cameras at one zoom (apertures of about 100–300, two focus distances). |
| **Parallel light falloff** | A parallel light's rays share one direction, but its falloff is measured from the light's position, as for a point light. CPU and Classic 3D GPU paths. | Far layers were too bright under a parallel light with a falloff. |
| **Linear side facing a Bezier side** | In a segment with one linear and one Bezier side, the linear side's handle sits on the chord at 1/6 of the segment, not 1/3. The Lottie export writes the same handle. | A slide-in with a linear start and an eased end was off by tens of pixels mid-move; with 1/6 it matched within 1 px. |
| **Classic 3D specular** | The highlight is the Phong lobe (R·V)^shininess, not Blinn (N·H)^shininess. CPU and GPU. | Off-axis layers were up to 6 % brighter than After Effects. |
| **Precomp Motion Blur switch** | A precomp layer with its Motion Blur switch off still renders the motion blur that layers inside the nested comp ask for, with or without Switches Affect Nested Comps. Only Quality passes down. | Motion blur was missing inside nested comps. |
| **Shutter in time-stretched precomps** | The shutter spans the rendered frame. Inside a time-stretched or time-remapped precomp it spans the nested comp's time times the product of the precomp layers' source rates (at 50 % stretch, twice the nested comp's own shutter). Layer motion, mask motion blur, effects that read the shutter, Classic and Advanced 3D follow it. | Motion blur inside time-stretched precomps had the wrong length. |

The loop also found that a mask feather running off the layer lost its edge. The current head of
#2558 already fixes that (with its own test), so this branch does not change it.

`docs/aep-import.md` and `docs/architecture.md` are updated in the commits that change the behaviour
they describe.

## What could not be matched, and the workaround

### Fractal Noise glows

The project's glows are Fractal Noise solids, colourised, glowed and heavily blurred into soft pools
of light, composited in Add mode. EffectCraft runs the same effect chain, but its noise is its own
(clean room), and that shows:

- **The pools land elsewhere.** The positions of the pools in EffectCraft's render do not correlate
  with After Effects' at any frame. No setting of the importer or engine fixes that without copying
  the other application's noise.
- **The noise moves at a different speed.** Measured by temporal correlation, After Effects' noise
  changes about 3–4 times faster than EffectCraft's for the same Evolution keys.

These glows were the largest remaining difference after the engine fixes.

**Workaround, project level only:** `tools/aep-parity/glow_fit.py` replaces the content of the
selected glow layers with a fitted field: a grid of radial Gradient Ramp cones in the layers' own
space, with each cone's colour keyed every few frames and solved by non-negative ridge least squares
against the reference. The layers keep their masks, opacity keys, blending mode, parenting and
timing. The result is a JSX script applied after import. It is not an engine change and not part of
the importer; it only makes sense when a reference video exists. It needs to be fitted again if the
glow layers change, but not when footage is swapped.

### Smaller residuals

- **Transition sections.** Where image layers go through Camera Lens Blur (hexagon iris, diffraction
  fringe), 3D lighting and stretched motion blur at the same time, the gap stays at 0.011–0.018.
  EffectCraft's lens-blur kernel is its own, so pixel equality is not reachable clean-room. The
  difference is mostly a slight brightness and softness difference on the images in these frames,
  and was not visible at speed.
- **Text metrics.** One line set in a custom font sits about 15 px left and is about 1.4 % wider
  than in After Effects. It comes from the font's metrics, not from the importer.
- **Loop seams.** The last seconds of two looping videos drift slightly (smoothly, not as a jump).
  The likely cause is the edge of the last fitted glow keys or encoder warm-up.
- **Encoders.** Three references came from Adobe Media Encoder's H.264 encoder. They were
  approximated with x264 using the same profile, level, GOP shape, bit rate and a silent audio
  track, so the bitstreams cannot be identical.

## Results

All numbers are SSIM. The final rows include the fitted glow override described above.

| | Round 0 | Final |
|---|---|---|
| Plain, 8 sampled frames per video | 0.740 – 0.960 | 0.976 – 0.995 |
| Gap to the codec ceiling, 8 sampled frames per video | not measured | 0.0011 – 0.0045 |
| Plain, every frame (9,983 frames) | not measured | 0.975 – 0.996 |
| Encoded, every frame | not measured | 0.985 – 0.996 |
| Gap to the codec ceiling, every frame | not measured | 0.0012 – 0.0060 |
| Gap on transition frame sets | not measured | 0.011 – 0.018 |

Ranges are over the 8 videos. Rounds 0 to 13 used an older decode path that reads about 0.001–0.003
low; this does not change the picture.

Per video, sampled frames (plain SSIM):

| Video | Round 0 | Round 9 | Final (round 20) | Final gap |
|---|---|---|---|---|
| V1 | 0.922 | 0.987 | 0.992 | 0.0014 |
| V2 | 0.892 | 0.980 | 0.987 | 0.0013 |
| V3 | 0.960 | 0.994 | 0.995 | 0.0011 |
| V4 | 0.944 | 0.990 | 0.991 | 0.0014 |
| V5 | 0.864 | 0.980 | 0.989 | 0.0032 |
| V6 | 0.880 | 0.981 | 0.987 | 0.0013 |
| V7 | 0.803 | 0.979 | 0.990 | 0.0029 |
| V8 | 0.740 | 0.963 | 0.976 | 0.0045 |

- **Judged by eye:** side by side, at full speed and frame by frame, nothing noticeable was found in
  any of the 8 videos: images, timing, glows, text, motion blur and grain all match.
- **Reproducible:** every frame rendered in the last measuring round was byte-identical in the final
  full render.
- **Footage swap:** after replacing one image with `rerender.py --swap`, only the pixels of that
  image's layer changed (3.5–8.2 % of the frame, depending on the video).

## How to reproduce on your own project

You need the After Effects project (a collected project is easiest), the videos After Effects
rendered from it, your own `ffmpeg`/`ffprobe` with libx264 (for decoding and scoring), and Python
3.9 or newer (`numpy` and `Pillow` for the glow fit). The full guide is [tools/aep-parity/README.md](../tools/aep-parity/README.md).

```sh
cargo build --release -p effectcraft-cli
cp tools/aep-parity/parity_config.example.json my.json    # then edit: aep, relink, entries
T=tools/aep-parity

python3 -I $T/measure.py my.json --probe-offset <entry>   # check the time mapping (+0 expected)
python3 -I $T/measure.py my.json --run r0 --jobs 2        # baseline scores, sheets, heat maps
# fix what the heat maps show, rebuild, measure again with a new --run name

# optional: fit procedural glows that cannot match
G="python3 -I $T/glow_fit.py my.json"
$G probe && $G render && $G ref && $G fit
$G render --tags fit && $G check --with-fit
# add the generated overrides JSX to "scripts" in my.json, then measure again

# EffectCraft's own H.264 export; add --encoder ffmpeg to replay each reference's x264 settings
python3 -I $T/rerender.py my.json --jobs 2 --swap "images/old.jpg=/abs/new.jpg" --out out/
python3 -I $T/full_scores.py my.json --out out/           # every frame, without --swap
$T/side_by_side.sh refs/loop.mp4 out/main-loop.mp4 sbs.mp4   # review video, made by your ffmpeg
```

## Open items

- **Depth-of-field constant.** 0.115 × zoom / focus was fitted on one project, with four cameras at
  one zoom. It changes how every existing EffectCraft camera with depth of field renders (test and
  demo apertures were scaled so they keep their blur). It should be checked at other zooms and
  focus distances before it is treated as a general rule.
- **Render cost.** Each render process holds the whole project: 6–8 GB of RAM per process with
  this many layers, and about 10 GB of frame cache. The 8 videos took about 7 hours with 3 parallel
  processes.
- **Text metrics** of one custom font (above): about 15 px offset and 1.4 % width on one line.
- **Transition brightness** where Camera Lens Blur, 3D lights and motion blur combine (above).
- **Fractal Noise** stays different by design; the glow fit is a per-project workaround.
- **Tests.** Two tests (`engine text_edit::fonts_lists_bundled_and_installed_families`, which
  depends on the installed fonts, and `gpu adv3d::gpu_matches_cpu_wide_penumbrae`) failed on one
  machine on an older head of #2558, before any of these changes. Re-run on Linux with an NVIDIA
  RTX 2070, both pass on #2558's head `b54a76ff` and on this branch (whose `cargo xtask ci` also
  passes on #2558's later head `4a96a2ca`); the GPU test ran on the GPU (2 of 4,138 shadowed
  pixels beyond 2/255), it did not skip.
- **Manual kerning fixture test.** `manual_kerning_imports` needs the optional fixtures of #2558
  (`EFFECTCRAFT_AEP_FIXTURES`); without them it skips. The three kerning unit tests run without
  fixtures.

## Status

This work is shared as is. Its author does not have the time to carry it through review, and is
publishing it so the code and the findings are not lost. Everything described here worked locally,
on the project above, and produced the renders that were needed. Anyone is welcome to pick it up,
change it, split it into smaller pull requests or take only the parts that help. Good luck, and
thank you to the EffectCraft maintainers and to vangel76 for #2558.
