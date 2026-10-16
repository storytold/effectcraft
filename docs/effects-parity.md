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
| **Rendered output matches After Effects** | **unmeasured** | No effect is compared with an After Effects render yet. `effectcraft_effects::fidelity::compare_frames` exists for the corpus ([fidelity/README.md](fidelity/README.md)); simulation effects are pinned by golden hashes of our own output (M13.27) |
| Animation presets | **54** original presets (8 text-animator, 23 Transitions, 23 Backgrounds) against After Effects' **621** `.ffx` presets | Measured: `crates/engine/presets/text_animators.json`, `crates/engine/presets/transitions.json`, `crates/engine/presets/backgrounds.json`; the After Effects `Presets` folder |
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
| Color Correction | 40 | 40 | Tint has Swap Colors (#963); ACES 2.0 / OCIO 2.5 (26.5); Apply Color LUT (#514) and 32-bpc blending (#494) are closed |
| Distort | 39 | 35 | Liquify has no viewer brush; #303 (Mesh Warp drag) and #534 (CC Power Pin Unstretch) are closed |
| Expression Controls | 9 | 9 | — |
| Generate | 26 | 25 | Fractal on the CPU (needs f64); #526 (Reset) and #536 (Write-on marks) are closed |
| Immersive Video | 12 | 12 | — |
| Keying | 12 | 12 | Key Cleaner approximated; #533 (wire-removal Frame Offset) is closed |
| Matte | 6 | 4 | Mocha shape reads our documented JSON, not Mocha's format |
| Noise & Grain | 13 | 12 | Curl Noise (#463); Match Grain on the CPU |
| Obsolete | 9 | 9 | — |
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
After Effects' preset categories (Behaviors, Shapes, Synthetics, Text, Image effects remain; Backgrounds and
Transitions exist) 15–35 h; the open effect bugs 10–20 h. Moving Fractal and the analysis effects to
the GPU is performance, not parity.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-12 | minor | Parameters 3,014 to 3,017 (effects.md); #303, #514, #526, #533–#536 marked closed instead of pending |
| 2026-10-12 | minor | Effect colour labels (26.5) added in Effect Controls |
| 2026-10-12 | minor | Tint's Swap Colors button and agent-drivable command (#963); undo/redo, instance targeting, animated layer-time edits and invalid-input tests |
| 2026-10-10 | major | First effects parity checklist |
