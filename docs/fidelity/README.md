# Measured keyframe fidelity

The [AE 2026 replacement acceptance gates](ae-2026-acceptance.md) define the broader
target across workflows, rendered output, work-project compatibility, performance and platforms.

[Object Matte](object-matte.md) has a comparison plan (clips, prompt, per-frame IoU and the other
metrics) but no measurement yet.

The first G1 comparison measures eight original scalar Rotation cases against a live After Effects
2026 session through AEsync 2.0.4: Easy Ease, overlapping influences, asymmetric ease, overshoot,
descending values, equal endpoints, 100%/100% influence and 0.1%/100% influence. Each case has
21 samples over a one-second segment. This is a scalar temporal-interpolation measurement, not an
overall animation, rendering, spatial-path or application-parity score.

## Reproduce

With Node.js, Rust and an installed AEsync CLI, open a scratch After Effects project and run from
the EffectCraft checkout:

```sh
node examples/check-ae-ease.mjs /path/to/aesync/server/dist/cli.js
```

The external bridge must be answering. Signing into AEsync's embedded chat is not required.
The script creates a temporary comp with an original text layer, samples its Rotation property and
removes the comp on success or failure. It does not save, close or replace the current project.
Run in a scratch project: creating/removing the comp still changes the dirty flag and undo history.

`crates/keyframe/examples/ae_ease_samples.rs` evaluates the corresponding EffectCraft keys.
`examples/ae-ease-oracle.jsx` records live property samples and actual ease settings after the setters.
`examples/check-ae-ease.mjs` checks case metadata, applied AE settings, finite values and sample times,
then compares every value with an absolute tolerance of `1e-7` degrees. A failing score exits nonzero.

Raw host observations and EffectCraft samples stay in gitignored `plan/aftereffects/ref/ease/`.
Only the original harness and aggregate scores in `keyframe-ease.json` are tracked. No Adobe artwork,
presets, screenshots, frames or implementation code are included.

## Finding and validation

Measured on 5 October 2026 with After Effects `26.3x87` and AEsync 2.0.4. Before the fix,
EffectCraft normalized outgoing/incoming influence when their sum exceeded 100%. AE retains each
influence. That normalization caused maximum errors of 12.4581 degrees for 80%/80%, 8.8909 degrees
for the asymmetric case and 5.5636 degrees for the descending case. Removing normalization in both
scalar evaluation and the shared progress helper makes all eight cases pass; the largest difference
in the 168 samples is below `3e-11` degrees.

The numeric regression test checks AE-derived behavioral facts, including the formerly failing
sample at 0.1 seconds (0.59243939036327 degrees; previously 1.4621885963370382). Existing linear,
hold, spatial, roving and eased-range tests remain green.

Astra review also found premature Newton convergence near the flat midpoint of the 100%/100%
time curve. Its closed-form inverse and a parameter-based convergence check now have a separate
mathematical regression test at times within `1e-12` of the midpoint; that test is additional to
the 168 live AE samples.

## Auto-Bezier spatial tangents

Measured on 10 October 2026 with After Effects 2026 (26.3) by running the original
`examples/ae-spatial-tangents.jsx` in both hosts (`AfterFX.exe -r` and `effectcraft-cli script`):
three original Position paths, ten keys, with even and uneven key timing. The script pins spatial
interpolation to auto-Bezier, since an AE preference can default new keys to linear.

AE gives each auto-Bezier key an out tangent of (next key − previous key) / 6 and the mirrored in
tangent, whatever the key times; an end key uses its one neighbour, (neighbour − key) / 6. That is a
Catmull-Rom spline. EffectCraft gave end keys no tangent and scaled middle keys' tangents by the
lengths of their segments, so all ten tangents differed. Sampled Position along an auto-Bezier path
with Easy Ease was up to 25.1 px off, and a roving path up to 27.7 px.

After the fix, all ten tangents equal AE's to the printed 0.001 px, and the two path cases are within
0.05 px over 61 samples each; what remains is the arc-length table's sub-pixel speed. The regression
test `spatial_auto_bezier_tangents_follow_after_effects` checks the rule on other original keys,
including a timing change. AE's output stays local; only the script and these scores are tracked.

## Animation behavior follow-up (15 October 2026)

Thirteen original cases, 81 times each, were sampled through After Effects `26.0x67`
and an EffectCraft `0.10.0` development CLI. They cover non-unit durations, hold and
mixed interpolation, temporal auto/continuous Bezier, expressions, spatial auto-Bezier,
linear/eased roving, and separated Position. Aggregate results are in
[`animation-behavior.json`](animation-behavior.json); raw host output remains local.

The initial `0.9.2` comparison exposed four problems, now covered by focused regressions:

- Per-key scripting commands installed their temporary key selection before switching
  comps, which replaced it. They now activate first and restore both comp selections,
  including on errors. Comparisons no longer need to open each target comp first.
- Roving keys were forced to linear and timed by distance alone. Spatial roving now
  follows the fixed endpoints' ease across the entire path; automatic endpoints retain
  constant speed. The eased-path maximum component error fell from 84.7021 px to
  0.012425 px in the sampled case.
- Linear sides of mixed curves use one-sixth influence; automatic temporal endpoints
  use the adjacent slope. Inserting a numeric key into a Bezier segment subdivides its
  time/value curve, preserving derivatives rather than introducing zero-speed handles.
  Auto and continuous cases fell from 10.5759° / 8.1141° to below 0.0013°.
- Separated Position queries combine X/Y/Z followers for both pre- and post-expression
  reads. A follower-expression discrepancy of 9.99991 px is now below 4e-13 px.

All 13 sampled cases are below 0.002° for scalar rotation and 0.05 px for spatial
values. These are comparison tolerances, not exact parity or a general error bound.
The small spatial residual includes arc-length approximation; arbitrary query times
also differ slightly between hosts. An additional auto-roving probe checks stable
retiming; regression tests also check subdivision at several insertion times.

`tests_ae_behavior` modules in keyframe and script, and `ae_behavior_tests` in project,
cover the fixes. This is property-evaluation evidence, not Graph Editor gesture,
preview-latency, or all-project rendering acceptance. Thirty-six simple nested-comp
frame pairs also matched placement/clipping in the local investigation; deeper
mask/matte/effect/3D combinations remain unmeasured.

## Still unmeasured

Rendered frame comparisons now have a deterministic `effectcraft_effects::fidelity::compare_frames`
helper. Corpus runners can use it with locally generated host/reference frames and fail on either a
mean or maximum channel error without committing proprietary renders.

Broader combinations of mixed interpolation, temporal/spatial speeds, time remapping and
expressions, as well as actual rendered motion and other effects, need additional corpus cases.
The bounded probes above do not establish application-wide parity.
