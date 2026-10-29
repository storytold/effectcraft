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

## Justify All with the every-line composer

Measured on 19 October 2026 with After Effects 2026 (26.2) on box text (EXPORTER read-back t6):
with Justify All, AE stretches the paragraph's last line too and breaks the lines so that it is not
a lone word (4 / 4 / 4 words where the other justifications break 6 / 6 / 1). EffectCraft's
every-line composer ignored the last line's slack and left "twelve" alone on a stretched line; it now
counts that slack for Justify All, and every word starts within 1 px of AE's. Regression test:
`justify_all_balances_the_lines_with_the_every_line_composer`.

## Right-to-left point text with a justified paragraph

Measured on 19 October 2026 with After Effects 2026 (26.2) (EXPORTER read-back t7): right-to-left
point text whose paragraph has a justified code (justify last left / centre / right, justify all)
sits flush right at its anchor, like right-aligned text. EffectCraft placed it by the last-line
side (left / centre / left). Now flush right; left-to-right point text is unchanged (not measured).
Regression test: `rtl_point_text_with_a_justified_paragraph_is_flush_at_its_start`. Still open:
AE wraps right-to-left box text slightly wider (its word gaps are 1–2 px narrower than in
left-to-right text with the same font), so a 300 px box fits a word more per line.

## Text animator tracking

Measured on 19 October 2026 with After Effects 2026 (26.2): a text animator's Tracking Amount is in
pixels, not 1/1000 em like the Character panel's tracking (Amount 30 adds 30 px between neighbouring
characters at 80 px and at 40 px, half before and half after each), and a Subtract range selector
over characters the first selector leaves out keeps them at 0 instead of tracking them negatively.
With both, the EXPORTER's t8 layer D (Tracking 30 on characters 2–4 by index, a Subtract selector
over the last 20 %) puts every glyph where AE does (it ended 88 px short). The "Tracking In" text
preset now animates 40 → 0 px (was 400 → 0 thousandths of an em). Lottie export is unchanged (not
checked against a player). Regression tests: `animator_tracking_is_in_pixels_as_in_after_effects`,
`modes_combine`.

## Per-character 3D rotation order

Measured on 19 October 2026 with After Effects 2026 (26.2) (case 931: "H" at 400 px, per-character
3D, animator X / Y / Z Rotation 40 / 30 / 20): a character turns as a 3D layer does, Rx · Ry · Rz
(Z first). EffectCraft composed Rz · Ry · Rx, so the glyph faced another way (23 of 5184 grid points
off, max 255); now 3 at the anti-aliased edge (mean 0.015). Regression test:
`per_character_rotation_turns_z_first_as_a_3d_layer`.

## Rectangle path start point

Measured on 19 October 2026 with After Effects 2026 (26.2) on 400 × 300 shape rectangles with Trim
Paths: a plain rectangle starts at its top-right corner and runs clockwise (0–25 % covers the right
edge and 50 px of the bottom); with Roundness 40 it starts where the top-right arc meets the right
edge. Start 70 / End 30 draws 30–70 %. EffectCraft started at the top-left corner (and the rounded
rectangle elsewhere), so trimmed rectangles drew the wrong segment (86 of 5184 grid points off,
now 5 at stroke edges). `effectcraft_path::rect` now draws AE's order, which also gives Convert to
Bezier AE's vertex order. Regression test: `rectangles_start_where_after_effects_starts_them`.

## Still unmeasured

Rendered frame comparisons now have a deterministic `effectcraft_effects::fidelity::compare_frames`
helper. Corpus runners can use it with locally generated host/reference frames and fail on either a
mean or maximum channel error without committing proprietary renders.

Mixed interpolation sides, non-unit durations, temporal automatic/continuous Bezier, spatial speeds,
time remapping, expressions, actual rendered motion and other effects need separate corpus cases.
The initial probe makes no claim about those features.
