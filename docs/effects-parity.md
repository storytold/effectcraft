# Effects parity

> **Last reviewed:** 2026-10-12 · **Last updated:** 2026-10-12 · **Change:** minor (effect colour labels; earlier change: parameter count and closed issue references re-audited against `upstream/main` 470ea7a3 and the generated effects.md) · **Target:** Adobe After Effects 2026

Effects are the deepest single area of After Effects. This page separates the four questions
that a single "306 of 306" hides: does each effect exist, does it run fast, does it look like
After Effects', and does the preset library exist. The per-effect catalogue is the generated
[effects.md](effects.md); the overall numbers are in [target-app-parity.md](target-app-parity.md).

## Summary

| Question | Value | Kind |
|---|---|---|
| Effects present | **306** registered, 3,017 parameters; all of After Effects' effects by name (298 at the M9.11 count) plus Obsolete and our own | Measured: registry, [effects.md](effects.md). After Effects 26.5's `Plug-ins/Effects` holds 306 `.plugin` bundles in 231 entries (one bundle can hold several effects, so this is a cross-check, not a match) |
| Parameter names, order, popups, units, defaults | aligned in M9.12 | Agent-checked against After Effects' documentation, not against a live dump |
| On the GPU | **280** (92%) | Measured: registry flags; GPU vs CPU agreement tested per effect |
| 32-bit float, no clamping | 306 | Measured: registry flags |
| **Rendered output matches After Effects** | **partly measured** | Some colour effects are checked against After Effects 2026 renders of our own test projects: Tint, Tritone, Black & White, Exposure, Levels, Curves, Luma Key, Extract, 4-Color Gradient and Fill within 1–3/255, Brightness & Contrast up to 6.2/255 (Color Correction, Generate and Keying rows below); every other effect is unmeasured. `effectcraft_effects::fidelity::compare_frames` exists for the corpus ([fidelity/README.md](fidelity/README.md)); simulation effects are pinned by golden hashes of our own output (M13.27) |
| Animation presets | **76** original presets (8 text-animator, 23 Transitions, 23 Backgrounds, 22 Behaviors) against After Effects' **621** `.ffx` presets | Measured: `crates/engine/presets/text_animators.json`, `crates/engine/presets/transitions.json`, `crates/engine/presets/backgrounds.json`, `crates/engine/presets/behaviors.json`; the After Effects `Presets` folder |
| 26.x additions | Curl Noise (26.3) present (#463 open); effect colour labels (26.5) present in Effect Controls only, not in the Timeline; the rebuilt Effect Controls (26.5) missing | Release notes |

**Breadth ≈ 85%** (presets weigh in; the effects themselves are ≈ 100%). **Ready ≈ 40%**: until a
corpus compares renders, each effect is assumed to differ somewhere, and users keep confirming it.

## By category (from [effects.md](effects.md))

| Category | Effects | On the GPU | Known differences or bugs |
|---|---:|---:|---|
| 3D Channel | 8 | 8 | Cryptomatte selection can't be set in the UI and the GPU path is slow (#473; quoted names with commas fixed); multi-layer EXR decode shared (#474 open: f16 storage and cache sizing remain) |
| Audio | 13 | 0 | CPU by design (applied in the mixdown) |
| Blur & Sharpen | 16 | 15 | Gaussian Blur interaction and output to be measured first ([fidelity/ae-2026-acceptance.md](fidelity/ae-2026-acceptance.md)) |
| Channel | 13 | 13 | — |
| Color Correction | 40 | 40 | Tint, Tritone and Black & White (sliders and Tint) match After Effects 2026 renders (Rec. 601 luminance); Levels (Individual Controls) matches After Effects 2026 in 32 bpc (channel controls before RGB, "Off for 32 bpc Color" unclipped; plain Levels' channel order not measured); Brightness & Contrast (non-legacy) close to After Effects 2026 ramps, not identical (up to 6.2/255 off; the test allows 7/255) (negative settings invert the positive curves); Curves interpolates a natural cubic spline through its points, as After Effects 2026 (exact on its grey steps); Tint has Swap Colors (#963); ACES 2.0 / OCIO 2.5 (26.5); Apply Color LUT (#514) and 32-bpc blending (#494) are closed |
| Distort | 39 | 35 | Liquify has no viewer brush; #303 (Mesh Warp drag) and #534 (CC Power Pin Unstretch) are closed |
| Expression Controls | 9 | 9 | — |
| Generate | 26 | 25 | 4-Color Gradient's Blend matches After Effects 2026 (weights 1 / (d² + Blend²)); Fill's Opacity is the filled layer's opacity, as After Effects 2026 renders it (with a Fill Mask it still mixes with the original, not measured); Fractal on the CPU (needs f64); #526 (Reset) and #536 (Write-on marks) are closed |
| Immersive Video | 12 | 12 | — |
| Keying | 12 | 12 | Luma Key and Extract key on Rec. 601 luminance, as After Effects 2026; Key Cleaner approximated; #533 (wire-removal Frame Offset) is closed |
| Matte | 6 | 4 | Mocha shape reads our documented JSON, not Mocha's format |
| Noise & Grain | 13 | 12 | Curl Noise (#463); Match Grain on the CPU |
| Obsolete | 9 | 9 | Luma Key matches After Effects 2026 on a grey ramp (hard cut, Tolerance only for Similar / Dissimilar) |
| Paint | 1 | 0 | Tablet pressure not read ([hardware-parity.md](hardware-parity.md)) |
| Perspective | 10 | 9 | Chromatic Aberrations pan/tilt crash (#580) |
| Simulation | 18 | 18 | Particle Playground lacks Particle Exploder, text particles, ephemeral mapper; CC Particle World camera fixed (#397) |
| Stylize | 25 | 25 | Motion Tile output size (#369) |
| Text | 2 | 2 | — |
| Time | 8 | 8 | #535 (Pixel Motion Blur, Vector Detail) is closed |
| Transition | 17 | 17 | — |
| Utility | 9 | 7 | — |

Third-party bundles in After Effects (Cineware, Mocha AE) are out of scope; Keylight is
implemented under a generic name ("Key Light").

## Estimate

**≈ 70–130 h**: a fidelity corpus case per effect at default and one non-default setting, compared
with After Effects renders, and the fixes they turn up (40–80 h, most of it parallel, but every
reference render needs After Effects on the owner's Mac); an original preset library covering
After Effects' preset categories (Shapes, Synthetics, Text, Image effects remain; Backgrounds, Behaviors and
Transitions exist) 15–35 h; the open effect bugs 10–20 h. Moving Fractal and the analysis effects to
the GPU is performance, not parity.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-19 | minor | Tritone uses Rec. 601 luminance, measured against After Effects 2026 renders of a red solid and a grey ramp (CPU and GPU) |
| 2026-10-19 | minor | Black & White's Tint keeps the gray's luminosity (Color blend), measured against After Effects 2026 renders (CPU and GPU) |
| 2026-10-19 | minor | Levels: "Off for 32 bpc Color" no longer clips in 32 bpc projects, unclipped values extrapolate below Output Black; Levels (Individual Controls) applies the channel controls before the RGB ones, measured against After Effects 2026 (CPU and GPU) |
| 2026-10-19 | minor | Brightness & Contrast: negative Brightness / Contrast are the inverse curves of the positive ones (Brightness −40: 20/255 → 6/255 off After Effects 2026; still up to 6.2/255 off, not the same pixels), CPU and GPU |
| 2026-10-19 | minor | Luma Key: Key Out Brighter / Darker cut hard at Threshold, measured against After Effects 2026 renders of a ramp (CPU and GPU) |
| 2026-10-19 | minor | Luma Key and Extract: Rec. 601 luminance (was Rec. 709), measured against After Effects 2026 (CPU and GPU) |
| 2026-10-19 | minor | 4-Color Gradient: each point weighs 1 / (d² + Blend²), measured against After Effects 2026 at Blend 10 / 50 / 100 / 300 (CPU and GPU) |
| 2026-10-19 | minor | Fill without a mask: the layer becomes the fill colour at its alpha × Opacity (was a mix with the original), measured against After Effects 2026 (CPU and GPU) |
| 2026-10-19 | minor | Curves: natural cubic spline through the points, clamped to 0..1 (was a monotone spline), measured against After Effects 2026 (CPU and GPU); Lumetri's curves and Glow's Arbitrary Map keep the monotone spline |
| 2026-10-18 | minor | Tint uses Rec. 601 luminance weights, measured against After Effects 2026 renders of red, green, blue and grey solids (CPU and GPU) |
| 2026-10-18 | minor | Exposure converts to linear light with a pure 2.2 power curve, as measured in After Effects (Offset 0.0005 / 0.002 / 0.02 / 0.1 on black: 8 / 15 / 43 / 90), not the piecewise sRGB curve; its sliders take After Effects' ranges (Exposure −100..100, slider −4..4; Offset −2..2, slider −0.5..0.5; Gamma Correction 0.1..10) |
| 2026-10-12 | minor | Parameters 3,014 to 3,017 (effects.md); #303, #514, #526, #533–#536 marked closed instead of pending |
| 2026-10-12 | minor | Effect colour labels (26.5) added in Effect Controls |
| 2026-10-12 | minor | Tint's Swap Colors button and agent-drivable command (#963); undo/redo, instance targeting, animated layer-time edits and invalid-input tests |
| 2026-10-10 | major | First effects parity checklist |
