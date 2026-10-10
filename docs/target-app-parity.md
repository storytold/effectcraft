# Parity with After Effects

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-10 · **Change:** major (full re-measure against After Effects 2026 26.5; merged `docs/parity.md` into this file; reconciled the stale per-area rows) · **Target:** Adobe After Effects 2026 (26.5.0.89)

The authoritative assessment of how close EffectCraft is to After Effects, and how much work is
left. [gaps.md](gaps.md) lists every known shortfall one at a time; the
[ROADMAP](../ROADMAP.md) summarises both. Deep dives: [effects-parity.md](effects-parity.md),
[scripting-parity.md](scripting-parity.md), [file-format-parity.md](file-format-parity.md),
[ui-parity.md](ui-parity.md), [hardware-parity.md](hardware-parity.md),
[localization-parity.md](localization-parity.md), and the live measurements in
[fidelity/](fidelity/README.md).

## Headline

| Number | Value | Kind |
|---|---|---|
| **Feature breadth** (does each After Effects 26.5 feature exist?) | **≈ 91%** | Estimated, weighted by area (table below). The self-graded 92-item checklist says 89 done / 3 partial (≈ 99%), but it was written before After Effects 26.2 and 26.5 and does not count animation presets, project formats or hardware; see the appendix |
| **Ready for real work** (can an After Effects professional do client work here?) | **≈ 45%** (range 40–50%) | Estimated: 60% feature depth (≈ 50%, table below), 15% opening and delivering work files (≈ 35%), 10% performance (≈ 35%), 10% stability (≈ 45%), 5% platforms (≈ 65%) |
| **Stage** | **alpha** | All six core workflows pass the [alpha gate](roadmap.md#alpha-gate) on macOS and ready (≈ 45%) is above the ≈ 40% bar, but `.aep` projects can't be opened, fidelity is measured for one narrow case, and 134 issues are open ([ROADMAP](../ROADMAP.md#stage)) |
| **Mainstream practitioner** (the typical motion designer's weekly work) | **≈ 47%** | Estimated: everyday depth 72% × interaction 0.90 × stability 0.90 × file exchange 0.80 ([below](#mainstream-practitioner)) |
| **Essentials user** (core features only) | **≈ 52%** | Estimated: core depth 77% × launch/stability 0.88 × clarity 0.90 × opening files 0.85 ([below](#essentials-user)) |
| Remaining to **beta** (≈ 75% ready, `.aep` opens) | **≈ 350–620 Opus 5.5 agent-hours** | Estimated |
| Remaining to **full parity** | **≈ 850–1,500 Opus 5.5 agent-hours** | Estimated |

### Readiness by audience

| Audience | Ready % | Opus 5.5 agent wall-clock hours to ~95% | Work that dominates |
|---|---:|---:|---|
| Full target (ready for real work) | ≈ 45% | ≈ 850–1,500 h | `.aep` import, a fidelity corpus for every feature, the bug backlog, performance, hardware video, formats, localization, ecosystem |
| Mainstream practitioner | ≈ 47% | ≈ 280–530 h | `.aep` exchange (80–160 h), stability and crash fixes (40–80 h), fidelity of the everyday features (40–70 h), everyday-area bugs and depth (60–110 h), interaction feel (30–60 h), preview speed (25–50 h) |
| Essentials user | ≈ 52% | ≈ 150–300 h | Opening the `.aep` files tutorials ship, basic subset (60–120 h), start-up and stability (20–40 h), core-feature bugs and clarity (30–60 h), preview with sound (20–40 h), fidelity of the core features (20–40 h) |

Each audience's hours are a subset of the next (essentials ≤ mainstream ≤ full). They are
calibrated like the rest of this file ([calibration](#calibration-of-the-hours)): a behaviour bug
with its regression test ≈ 0.1–0.3 h, a mid-size feature 1–4 h. About 60–70% parallelises across
5–8 agents. The serial parts: the `.aep` owner decision before the importer, and the fidelity
harness before per-feature scores. Every audience needs After Effects running on the owner's Mac
for reference captures.

The previous headline (5 October: breadth ≈ 99%, real use ≈ 30–50%) moved for these reasons:
breadth went **down** because this pass measured against 26.5 rather than our own catalogue and
found missing features the catalogue never listed (Object Matte, Substance materials, ACES 2.0
configs, proportional scrubbing, copy frame to clipboard, paste SVG/AI as shapes, percentage
guides, After Effects' 621 animation presets, FBX/USD/STL models, Media Encoder-style hardware
encoding); real use moved **up** to the upper half of the old range because the 5–10 October
work closed 179 issues, most of them user-reported bugs fixed with regression tests, added six partial UI languages
and a first live After Effects fidelity measurement. It is still an estimate: nothing renders a
corpus in both apps yet.

## How this was measured

- **Target.** Adobe After Effects 2026, the copy installed on the owner's Mac:
  `CFBundleShortVersionString` 26.5.0, build 26.5.0.89. The one live fidelity comparison
  ([fidelity/README.md](fidelity/README.md)) used 26.3x87.
- **From the installed bundle (file names and listings only, per [AGENTS.md](../AGENTS.md) §2):**
  `Info.plist` `CFBundleDocumentTypes` (26 document types: `.aep`, `.aepx`, `.aet`, `.aetx`,
  `.mogrt`, `.mgjson`, `.aecap`, `.aegraphic`, `.ffx`, `.ars`, `.aom`, `.jsx`, `.psd`, `.cin`,
  `.mov`, `.pct`, `.pic`…); the `Plug-ins/Format` importers (AIFF, Animate, JPEG, OpenEXR, PNG,
  Premiere Pro import, Photoshop layers in and out, Radiance, SGI, SVG, Targa, the standard
  multi-format plug-in); `Plug-ins/Effects` (306 `.plugin` bundles in 231 entries);
  `Presets` (**621** `.ffx` animation presets); `Scripts` (11 sample scripts, Startup / Shutdown /
  ScriptUI Panels folders); `Resources/usd_plugins` (USD, FBX, glTF, OBJ, STL); `Resources/MLModels`
  (FastMask, ShotCutDetection) and tokenizer folders (Whisper, BERT, Marian); `.lproj`
  localisations (11 languages, [localization-parity.md](localization-parity.md)).
  After Effects was not launched for this pass, so there is no menu dump.
- **From Adobe's public release notes** for 26.2 (April 2026: Object Matte, displacement on
  parametric meshes, Quick Apply, Proportional Scrubbing), 26.3 (June 2026: Advanced 3D depth of
  field, paste SVG/AI as shapes, faster mask tracker, copy frame to clipboard, Curl Noise,
  variable-font filter) and 26.5 (September 2026: percentage guides, Object Matte disk cache,
  effect colour labels, a rebuilt Effect Controls panel, ACES 2.0 through OpenColorIO 2.5.1).
- **From this repository at `origin/main` 1446e27 (10 October 2026):** 587 menu entries in the
  After Effects-shaped menu tree (`crates/engine/src/menus.rs`), 411 registered engine commands,
  306 registered effects (280 on the GPU, [effects.md](effects.md)), 12 export formats
  (`OutputFormat::ALL`), the import extension lists (`crates/media/src/lib.rs`), 2,704 tests, ≈ 329,000
  lines of Rust, 6 UI language catalogs, and the 134 open / 179 closed GitHub issues.
- **Presence is not parity.** A feature counts toward *breadth* when it exists and works in the
  common case; it counts toward *ready* in proportion to how much of the area a professional
  could rely on, discounted for open bugs in it, for being unmeasured against After Effects, and
  for being checked on one platform only.

### Calibration of the hours

Hours are **Opus 5.5 agent wall-clock hours**: one agent session working sequentially, tests and
verification included. Calibrated against this repository's own history:

- The whole repository (≈ 329,000 lines including tests, 1,142 commits, 233 merged PRs) was built
  between 1 and 10 October 2026, typically by 4–8 agents at once: roughly 700–1,000 agent-hours,
  or ≈ 350–450 lines per agent-hour. Getting to today's ≈ 45% took that; the estimate to full
  parity is about 1–1.5× it again, because what is left is measurement, compatibility and
  long-tail behaviour rather than new code.
- Named arcs (from the audit notes kept below): Classic 3D ≈ 2.7 h, timeline depth (graph editor,
  keyframe dialogs, time remapping, pen and masks) ≈ 2.3 h, 85 effects ≈ 1.2 h, the After Effects
  menu bar ≈ 1.3 h, layer styles ≈ 0.6 h; a wave the audit priced at 21 h took ≈ 6 agent-hours
  but landed at 80–90%.
- Recent PRs (commit-to-merge): seven Timeline/viewer issues (#436) ≈ 0.6 h; eight tweaks from
  #290 (#383) ≈ 1 h; image-sequence import (#384) ≈ 4 h; panel i18n infrastructure plus 384
  Chinese strings (#398) ≈ 2–3 h; one 618-row menu language (#401 Ukrainian) ≈ 1.5–2 h; EXR
  beauty layer + OCIO v2 configs (#449) ≈ 1.3 h. So a user-reported behaviour bug costs ≈ 0.1–0.3 h,
  a mid-size feature 1–4 h.
- What history can't calibrate: fidelity work (each corpus case needs After Effects running,
  which only the owner's Mac has), `.aep` import (blocked on an owner decision), and anything
  needing hardware we don't have. Those carry the widest ranges.
- **Parallelism:** about 70% of the hours parallelise across 5–8 agents (effects fidelity,
  languages, bugs, formats are independent). Serial chains: the G1 harness before per-feature
  fidelity scores; the `.aep` decision before `.aep` work. **Needs a human:** the `.aep`
  clean-room decision, running After Effects for oracle captures, native-speaker review of every
  language, Windows/Linux GPU hardware for #591/#596/#613, and an optional decision on hosting
  After Effects SDK plug-ins.

## By feature area

Weights are the share of a typical motion designer's working time in After Effects (animation,
compositing and effects dominate; audio is light). Hours are to full parity for the area.

| Area | Weight | Breadth | Ready | Hours | Biggest gaps (details in [gaps.md](gaps.md)) |
|---|---:|---:|---:|---:|---|
| Project, footage and import | 8% | 92% | 55% | 25–45 | No relative paths or relinking when a project moves; Media Browser lists TGA/DPX/HDR/AVI/MXF it can't import; any image imports as a sequence (#431); MP3 layer goes silent (#324); multi-file drops (#552, #351) |
| Compositions, layers and compositing | 13% | 98% | 60% | 30–55 | 32-bpc blend modes differ from After Effects (#494); mask modes (#527, #510, #513); separated Position edits (#539, #540); expand precomps in place (#601) |
| Animation: keyframes, graph editor, motion paths, puppet | 15% | 97% | 55% | 35–60 | Only scalar ease is measured against After Effects (168 samples); Graph Editor jitter and clipped buttons (#456, #457); bulk keyframing (#355); pen tools on motion paths (#553); puppet lag and jump (#586, #318); Proportional Scrubbing (26.2) |
| Expressions and scripting | 7% | 93% | 65% | 20–40 | `.jsxbin`, Startup/Shutdown folders, `#include`; external editor (#364); see [scripting-parity.md](scripting-parity.md) |
| Shapes, masks and roto | 9% | 93% | 45% | 30–50 | Several open mask bugs (#510, #513, #527); Roto Brush size (#509); Mesh Warp can't be dragged (#303); paste SVG/AI as shape layers (26.3) |
| Text and typography | 7% | 94% | 50% | 20–35 | CJK text in comps renders wrongly (#453); paragraph alignment inverted (#316); squeezed text at 4:3 (#483); font search (#583); soft text at Fit (#417); variable-font filter (26.3) |
| Effects and animation presets | 14% | 85% | 40% | 70–130 | All 306 effects exist, but rendered output is unmeasured against After Effects; 8 built-in presets against After Effects' 621; open effect bugs (#580 crash, #514, #526, #463, #369, #473); effect colour labels (26.5); see [effects-parity.md](effects-parity.md) |
| 3D (Classic and Advanced) | 6% | 82% | 35% | 35–65 | No FBX/USD/STL models, no Substance materials, mesh displacement unverified; no 3D transform gizmos (#392, #347); Z position (#489); reflections (#261); camera aperture (#260); bevel options (#315) |
| Tracking and AI-assisted tools | 5% | 80% | 35% | 35–70 | No Object Matte (26.2); Roto Brush (MobileSAM) and face tracking (MediaPipe) unmeasured against After Effects; camera-tracker view (#505) |
| Preview and cache | 7% | 92% | 40% | 25–50 | Playback stutters on cached frames (#595, #304); slow scrubbing (#329); late audio (#554, #504, #335); no benchmark against After Effects |
| Render queue and export | 7% | 85% | 55% | 25–45 | No hardware encoding; ProRes 4444 colour tag (#340); rendering over an existing file (#325); default macOS output location (#446); uncompressed single-file output (#372); HEVC/AV1 without B-frames |
| Audio | 2% | 85% | 45% | 6–12 | Audio sync and crackle (#335); audio to keyframes depth |
| **Weighted** | 100% | **≈ 91%** | **≈ 50%** | **≈ 360–660** | |

Feature depth (≈ 50%) feeds the overall *ready* number with the other dimensions below.

## By dimension

| Dimension | % | Hours | Evidence | Doc |
|---|---:|---:|---|---|
| Features, breadth | 91% | (in features) | Table above | this file |
| Features, ready for real work | 50% | 360–660 | Table above | this file, [gaps.md](gaps.md) |
| UI/UX fidelity | 65% | 60–120 | After Effects-shaped menus (587 entries), panels, workspaces, shortcuts; many interaction issues still open; no multi-monitor; Effect Controls not yet like 26.5's | [ui-parity.md](ui-parity.md) |
| File formats | 50% | 130–230 | `.aep` / `.aepx` / `.mogrt` / `.ffx` can't be read; strong modern codecs (H.264, HEVC, AV1, VP9, ProRes) | [file-format-parity.md](file-format-parity.md) |
| Hardware | 40% | 60–120 | wgpu GPU on Metal/Vulkan/Direct3D 12/WebGPU with CPU fallback; no hardware decode/encode; Direct3D 12 compositor fails (#613); no video-out hardware | [hardware-parity.md](hardware-parity.md) |
| Localization | 12% | 60–110 + review | Average coverage of After Effects' 10 non-English languages (measured catalog rows over ≈ 3,900 strings): menus 618/618 in 6 languages, panels in 4, effects in none; After Effects ships 11 languages fully | [localization-parity.md](localization-parity.md) |
| Performance | 35% | 50–100 | Internal numbers only; users report stutter and slow scrubbing | [gaps.md](gaps.md#g5-performance-at-real-world-scale) |
| Stability | 45% | 40–80 | Open crash reports #580, #591, #501, #426, #341, #306, #234, #511; no CI runs the tests on pull requests | [gaps.md](gaps.md#g3-stability-and-a-green-main) |
| Platforms | 65% | 30–60 | Releases for macOS, Windows x64/x86/ARM64, Linux, FreeBSD and the web (more than After Effects), but Linux and Windows users hit basic bugs | [gaps.md](gaps.md#g2-real-user-reliability-on-every-platform) |
| Ecosystem and plug-ins | 20% | 40–80 | After Effects scripts and expressions mostly carry over; no After Effects SDK plug-ins, `.ffx` presets or `.mogrt`; our WebAssembly plug-in API has no third-party plug-ins yet | [plugins.md](plugins.md) |
| AI features | 35% | 40–80 (overlaps Tracking) | Roto Brush (MobileSAM) and face tracking (MediaPipe) as optional downloads; no Object Matte; the After Effects bundle ships Whisper, BERT and Marian tokenizers and a shot-cut model we have no equivalent of | [gaps.md](gaps.md) |
| Agent control (beyond After Effects) | ahead | — | Every command is drivable over MCP, the control channel and the CLI | [agents.md](agents.md) |

**Totals.** Summing the rows double-counts: AI overlaps Tracking, performance overlaps Preview,
UI overlaps every area. With the overlaps removed: **≈ 850–1,500 h to full parity**, of which
**≈ 350–620 h to beta**: `.aep` import (80–160 h, after the owner decision), the G1 fidelity
harness and P0/P1 corpus (60–100 h), the open bug and crash backlog (40–80 h), performance to
real-time preview at 1080p (40–70 h), Linux/Windows reliability (30–50 h), and feature depth in
animation, masks, effects and text (100–160 h).

## Ready for real work: how the number is built

The full-target number is a written weighted sum, not a single judgement:

| Component | Weight | Value | Source |
|---|---:|---:|---|
| Feature depth | 60% | 50% | Weighted feature-area table above |
| Opening and delivering work files | 15% | 35% | [file-format-parity.md](file-format-parity.md): `.aep` / `.mogrt` / `.ffx` 0%, delivery formats strong |
| Performance | 10% | 35% | By dimension, below |
| Stability | 10% | 45% | By dimension, below |
| Platforms | 5% | 65% | By dimension, below |
| **Result** | | **46.2%**, reported as **≈ 45%** (40–50%) | Rounded down to the nearest 5: no live render comparison exists yet |

Checked on 10 October 2026 when the mainstream and essentials numbers were added: it is unchanged.

## Mainstream practitioner and essentials user

Two narrower questions, computed the same way in every craft app (craftrules
`standards/progress-docs.md`, "Numbers"). The stage still follows the full-target number and the
[alpha gate](roadmap.md#alpha-gate).

### Mainstream practitioner

**≈ 47%** (estimated).

*Can the typical professional motion designer do their weekly work here?* Areas are the alpha-gate
workflows and their everyday tools. Left out: third-party plug-ins and `.ffx` preset libraries,
generative/cloud AI (Object Matte, Firefly), Adobe services and team features, specialist hardware
(video I/O, tablets), languages other than English, and areas a typical designer uses rarely
(Advanced 3D models, 3D camera tracking, Roto Brush). Depth is for the everyday subset of each area
(the common effects, the common formats), which is why it sits above the full feature-area table.

| Area (everyday subset) | Weight | Depth | Evidence |
|---|---:|---:|---|
| Project and import: MOV/MP4, PNG/JPEG sequences, PSD/AI, WAV/MP3 | 10% | 75% | Import tests; #431 still imported as a sequence, #324 MP3 silence |
| Comps, layers, blend modes, mattes, parenting, precomps | 16% | 80% | Compositor tests; #494 32-bpc blend modes, #539/#540 separated Position |
| Keyframes, Graph Editor, easing, motion paths | 20% | 75% | 168 live samples match After Effects; #456/#457, #553, #569 |
| Expressions: `wiggle`, `loopOut`, linking, sliders | 6% | 80% | `crates/expr` tests; no live expression oracle yet |
| Shape layers and masks | 12% | 65% | #510, #513, #527 mask bugs; #303 Mesh Warp |
| Text and text animators | 12% | 70% | #316 alignment inverted, #453 CJK, #583 font search |
| Everyday effects: blurs, glow, fill/tint, curves/levels, keying, drop shadow, displacement | 10% | 70% | All present with GPU paths; unmeasured against After Effects; #514 Apply Color LUT |
| Preview (RAM preview with audio) | 8% | 55% | #595, #304 stutter; #554, #504 late audio |
| Render to H.264 / ProRes / PNG sequences | 6% | 75% | Encoder tests; #446 default macOS output folder, #340 ProRes 4444 tag |
| **Average depth** | 100% | **72.2%** | |

Discounts for what still stops real work:

| Discount | Factor | Evidence |
|---|---:|---|
| Interaction fidelity | ×0.90 | UI parity ≈ 65% ([ui-parity.md](ui-parity.md)): Graph Editor jitter, snapping (#352), viewer zoom keys (#570), no multi-monitor (#486) |
| Stability on real machines | ×0.90 | Eight open crash reports (#580, #591, #501, #426, #341, #306, #234, #511); Direct3D 12 compositor fails (#613); no CI on pull requests |
| File exchange with After Effects users | ×0.80 | Can't open `.aep`, `.aepx`, `.mogrt` or `.ffx` sent by colleagues or bought as templates; PSD, AI, footage and rendered deliverables exchange fine |

72.2% × 0.90 × 0.90 × 0.80 = **46.8%, ≈ 47%**. It sits only just above the full number because
the gap is not in After Effects' long tail: fidelity, stability and `.aep` exchange hit
mainstream users as hard as specialists.

### Essentials user

**≈ 52%** (estimated).

*Can someone who only touches the core features (a social-media animator, a student, a
first-time user) get their work done?* No advanced options, pro workflows or exchange edge cases.

| Core feature | Weight | Depth | Evidence |
|---|---:|---:|---|
| Launch, new project, undo, save, reopen | 10% | 85% | Versioned project reopen tests, auto-save and crash recovery |
| Import a video, picture or song | 10% | 75% | #431 a picture imported as a sequence, #324 MP3 silence |
| New comp; move, scale, rotate, fade and trim layers | 15% | 80% | `ui_viewer`, Timeline tests |
| Add and style text | 10% | 70% | #316, #583 |
| Draw shapes and simple masks | 10% | 70% | Shape tool tests; #513 mask drawing feedback |
| Keyframes and Easy Ease | 15% | 85% | Ease measured against After Effects |
| Apply a few common effects | 10% | 75% | Effects & Presets search, Effect Controls tests |
| Preview with sound | 10% | 65% | #595 stutter, #554 late audio |
| Export an MP4 | 10% | 80% | H.264 encoder tests; #446 |
| **Average depth** | 100% | **76.75%** | |

| Discount | Factor | Evidence |
|---|---:|---|
| Launch and stability | ×0.88 | Start-up failures (#501, #234 macOS 27, #596 GPU not detected, #613 Direct3D 12); #426 timeline crash while playing |
| Discoverability and UI clarity | ×0.90 | After Effects' dense layout reproduced faithfully; Home screen and interactive Learn tutorials help; renders fail silently on macOS until an output folder is chosen (#446); layout nitpicks (#581) |
| Opening files people send | ×0.85 | Tutorial projects and templates arrive as `.aep` / `.mogrt`, which don't open |

76.75% × 0.88 × 0.90 × 0.85 = **51.7%, ≈ 52%**.

### User evidence (GitHub, 10 October 2026)

Counted from all 315 issues and their comments, excluding the maintainer account and
agent-generated maintainer replies:

- **Praise:** 5 people in 5 issues say so outright: "felt almost exactly like After Effects.
  Really impressive" (#611), "performance has been excellent" (#326), "Awesome project" (#322),
  "Great work" (#293), "great tool and AE alternative" (#290). About 6 more open with "would love…"
  in a feature request.
- **"Switched from After Effects" reports:** none found. #468 is a working motion designer's
  list of Mac issues, so people are trying it on real work.
- **Open issues (136):** ≈ 95 are core-path bugs (masks, keyframes, timeline, preview, audio,
  text, render, crashes; this includes 15 community `fix(...)` proposals), ≈ 40 are feature
  requests or niche (plug-in hosting, OpenFX, ffmpeg export, Discord presence, Store listing,
  rigging tools). Mostly core-path bugs, which matches the discounts above.

## Out of scope by design

Adobe services (Team Projects, Libraries, Stock, Media Encoder, Dynamic Link, Frame.io,
Exchange, Firefly), third-party bundles (Cineware / Cinema 4D, Mocha), and formats without a
public specification or permissive data (`.prproj`, `.jsxbin`, Vanishing Point `.vpe`, the
predefined CJK CMaps in PDF import, Kodak film emulations) are not counted against breadth.
`.aep` / `.aepx` *are* counted: they are the main file format and decide beta.

---

## Appendix: feature-checklist audit history

Moved here from `docs/parity.md` on 10 October 2026 and kept as evidence. It scores the 92-item
catalogue in `plan/aftereffects/feature-catalog.md`, graded by the agents that built the
features; read it as breadth only. **Its per-area table ("By area", below) was stale**
(Interface 75%, Project 68%, Animation 70%, Paint 0% next to a 99% headline) and is superseded by
the feature-area table above; it is kept for its "biggest gaps" notes, which record what
landed when.

### Checklist status (audit at commit `d39c0e8`, 4 October 2026; updated 5 October for M3.9–M4.12)

| Measure | Value |
|---|---|
| **Feature parity, weighted by tier** (P0 ×3, P1 ×2, P2 ×1) | **≈ 99%** counting every partial feature as half done; ≈ 99.8% using per-feature fractions |
| Unweighted | ≈ 98.4% (half-credit) / 99.6% (fractions) |
| P0 / P1 / P2 | 100% / 98% / 90% (half-credit); 100% / 99.7% / 97.5% (fractions) |
| Features done / partial / missing | 89 / 3 / 0 of 92 |
| **Effects** | **306** effects, every one implemented in full ([effects.md](effects.md)); 280 run on the GPU |
| Disabled menu entries left | 1, on purpose: Import ▸ Vanishing Point (.vpe), whose format has no public specification |
| Remaining work | ≈ 8.5 agent-hours at the pace measured so far (≈ 26 on the conservative audit scale); ≈ 3.5 without the learned-model items |
| **Wall-clock estimate** | **≈ 2–3 hours** with five agents in parallel; ≈ 1 hour without the learned-model items |

Partial features:

| id | Tier | Done | What is missing |
|---|---|---|---|
| EFF-5 GPU effects | P1 | 0.98 | Fractal (its escape iteration needs f64: WGSL has none and Metal's fast math defeats double-f32 emulation) and the analysis / tool effects (Camera-Shake Deblur, Detail-preserving Upscale, Puppet, Rolling Shutter Repair, Warp Stabilizer, Mocha Shape, Roto Brush, Match Grain, Paint, Camera Tracker, Face Tracker) render on the CPU. M13.28 moved the remaining pixel effects (CC blur / glass family, Color Link, Cineon, HDR, Inner/Outer Key, Basic 3D…) and the geometry and audio generators (Lightning, Advanced Lightning, Beam, Lens Flare, Radio Waves, Vegas, Stroke, Scribble, Write-on, Paint Bucket, Eyedropper Fill, CC Glue Gun, CC Threads, Audio Spectrum / Waveform, Basic / Path Text) onto the GPU; M13.29 the particle effects (CC Particle World / Systems II, Particle Playground, CC Ball Action, CC Pixel Polly, CC Scatterize) and Curl Noise, and removed the Shatter wireframe, Foam extras, Timewarp matte, strong Warp Fisheye / Twist and custom `.ocio` fallbacks. The 13 audio filters stay on the CPU by design |
| MSK-4 Roto Brush | P2 | 0.85 | Roto Brush 2.0 / 3.0 can use a trained model since M13.35 (MobileSAM, optional download); its quality has not been compared with After Effects (G1), so the score stands |
| TRK-3 Face tracking | P2 | 0.9 | Can use a trained model since M13.36 (MediaPipe Face Landmarker, optional download); the classical fitter is weak on profile and occluded faces; neither is compared with After Effects (G1), so the score stands; Rolling Shutter Ripple is approximated |

Since this audit: the project operations (M3.9–M3.14) and preview (M4.5–M4.12) passes fixed
behaviour inside features already counted as done, so the counts above stand: unsaved-changes
prompts and a modified mark that follows undo, Solid / Layer Settings with "Affect all layers that
use this solid", lock-aware layer commands, New Comp from Selection's options, menu-bar fixes,
project files that always reopen; RAM preview keys and eviction, Cache Before Playback that fits,
Cache Frames When Idle, Preserve frame rate / resolution when nested, nested comps past their end
and one motion blur gate (see the update sections below). Their code was checked against
[AGENTS.md](../AGENTS.md)'s "Never crash" rules (M4.12).

Before this audit: M13.22 and M13.23 put 70 more effects on the GPU (236) and composite every
Advanced 3D mode on the GPU; M13.24 runs GPU effects in the browser's render workers with a disk
cache in browser storage and a storage manager; M13.25 added the Home template gallery, Save as
Template and View ▸ Simulate Output ▸ My Custom RGB; M13.26 Premiere Pro interop through FCP7 XML,
FCPXML, OTIO, EDL, AAF and OMF (import and export, with pre-renders for layers a timeline can't
hold); WebM VP9 alpha now imports (FilmCraft); M13.27 pins the CPU simulation effects with golden
hashes.

Out of clean-room scope (no public specification or no permissively licensed data): native
`.prproj`, `.jsxbin`, Vanishing Point `.vpe`, the predefined CJK CMaps in PDF import, Kodak film
emulations. Adobe service integrations (Team Projects, Libraries, Media Encoder, Dynamic Link,
Frame.io, Exchange) and third-party plug-ins (Cinema 4D, Mocha) are intentionally absent.

#### Previous audit (commit `58163a2`, 3 October 2026, evening): ≈ 98%

87 done / 5 partial / 0 missing.

#### Previous audit (commit `978e8d7`, 3 October 2026): ≈ 94%

80 done / 12 partial / 0 missing.

#### Previous audit (commit `fa26ad9`, 2 October 2026, late): ≈ 93%

By area: Layers 98%, Output 95%, Compositions 95%, Automation 96%, Paint 95%, Text 95%, Import 96%,
Animation 94%, Masks 94%, Preview 94%, Interface 93%, Shapes 93%, 3D 91%, Audio 90%, Project 90%,
Tracking 96%, Effects 85%, Web 65%.

What is left, in priority order: the web app's depth (threads, storage, audio, non-blocking
renders); stroke taper/wave and multi-segment dashes; variable mask feather points; camera iris/bokeh and focus-link commands; 
approximated effects (Key Cleaner) and Liquify's viewer brush; JPX / JBIG2 images in
imported PDF/AI files (mesh shadings and CCITT landed in M13.12) and Illustrator procset EPS; and codec depth (B-frames, SAO/CDEF; HEVC and AV1 export and Opus SILK/hybrid landed in M13.4). Face tracking and a true Subspace Warp
landed in M13.3. ScriptUI (dialogs that block `show()`, palettes, dockable ScriptUI panels, File ▸ Scripts install
and sample scripts), puppet pin recording and the remaining "better than After Effects" items — a
versioned effect plug-in API with sandboxed WebAssembly plug-ins ([plugins.md](plugins.md)),
branching undo history in the History panel, and a bit-identical rendering test across runs and
thread counts — landed in M13.1 (GPU particles landed in M12.7). Render Queue field render, crop/resize, templates and logs, and VP9 inter frames landed in M10.2.

The sections below are the original audit (morning of 2 October, ≈ 64%) and its updates, kept for
history.

### Summary

| Measure | Value |
|---|---|
| **Feature parity, weighted by tier** (P0 ×3, P1 ×2, P2 ×1) | **≈ 64%** |
| Feature parity, unweighted | ≈ 58% |
| P0 (it isn't After Effects without it) | ≈ 75% |
| P1 (professional daily use) | ≈ 38% |
| P2 (long tail) | ≈ 17% |
| Features done / partial / missing | 18 / 60 / 14 of 92 |
| **Effects** | **298 of 298** After Effects 2026 effects by name (100%, after M9.11, Warp Stabilizer and the 3D Camera Tracker; was 257) |
| Remaining work | ≈ **167 agent-hours** (139 h of features + ≈ 20% for 1:1 polish against After Effects) |
| Wall-clock estimate | ≈ **42–50 hours** with five agents working in parallel and one integrating; ≈ 31–35 h for P0 + P1 only |

"Agent-hours" are hours of one autonomous coding agent (Claude Opus 5.5) working in its own git
worktree with tests. The estimate is calibrated on this repository's own history: Classic 3D
(cameras, lights, shadows, depth of field, views, tools) took one agent about 2.7 h; timeline
depth (graph editor, keyframe dialogs, time remapping, pen and masks) about 2.3 h; 85 effects
about 1.2 h; the After Effects menu bar about 1.3 h; layer styles about 0.6 h; the web build
about 0.8 h. What is left is harder on average (tracking, roto, puppet, GPU, Advanced 3D), and
sequential chains (tracker → mask tracking → Warp Stabilizer → 3D camera tracker; GPU
compositor → GPU effects) limit how much can run in parallel.

Work in progress at the time of the audit, not yet counted: motion tracking, preferences and the
keyboard shortcut editor, auto-save and crash recovery, paint and puppet tools, Lottie import and
export.

### Update after the second wave (same day, commit `e1f7241`)

Motion tracking (1-, 2- and 4-point, stabilize, corner pin), paint (Brush, Clone Stamp, Eraser
with write-on) and the puppet tools, preferences, a keyboard shortcut editor that rebinds,
auto-save and crash recovery, Lottie import and export, Effect Controls widgets (angle dial,
point crosshair, eyedropper, curves and levels editors, effect copy/paste) and 20 more menu
commands landed. Re-scoring only the features they touch:

| Measure | Before | After |
|---|---|---|
| Feature parity, weighted | 63.7% | **≈ 70%** |
| P0 / P1 / P2 | 75% / 38% / 17% | 78% / 56% / 17% |
| Remaining, audit estimate | 167 agent-hours | ≈ 141 agent-hours |

**Calibration.** The audit priced this wave at about 21 agent-hours; five agents delivered it in
about 1.5 hours of wall-clock time (roughly 6 agent-hours of actual work), though some features
landed at 80–90% rather than fully done. So the per-feature estimates above are conservative.
Remaining wall-clock time with five parallel agents is therefore somewhere between **≈ 12–15
hours** at the pace observed so far and **≈ 35–40 hours** if the audit's figures hold. The
hardest remaining systems (Roto Brush, Warp Stabilizer, the 3D camera tracker, Advanced 3D, GPU
rendering, on-canvas text editing) are where the conservative figure is most likely to be right.

### Update at the end of 2 October 2026

A third wave landed: viewer interactions (snapping, rulers, channels, exposure, snapshots, region
of interest, shape pen and pen-tool family, motion-path handles, graph editor transform box),
on-canvas text editing with per-character styles and paragraph settings, render fidelity (8/16/32
bpc, Rec.709/Rec.2020/P3 working spaces and linear blending, frame blending with optical flow,
collapse transformations, slip edits), a GPU compositor with 16 GPU effects, Wiggler/Smoother/
Motion Sketch, the marker dialog, precompose options and the comp navigator, Project panel folder
moves/rename/columns/thumbnails, drag-and-drop docking with floating panels, and the tracking
library groundwork for mask tracking and Warp Stabilizer.

| Measure | Morning | Now |
|---|---|---|
| Feature parity, weighted | 63.7% | **≈ 79%** |
| Unweighted | 57.8% | ≈ 74% |
| P0 / P1 / P2 | 75% / 38% / 17% | 87% / 67% / 21% |
| Remaining (audit-scale agent-hours) | ≈ 167 | ≈ 100 |
| Wall clock, five parallel agents | 42–50 h | **≈ 8–10 h at today's measured pace; ≈ 25 h by the audit's conservative figures** |

What is left is concentrated in large systems: the Warp Stabilizer and mask-tracking UI on top of
the new tracking library, and wasm threads. (The missing effect categories, 3D Channel, Immersive
Video and OCIO, landed in M9.11; the scripting object model in M14.4; Roto Brush and Refine Edge in
M6.7; native macOS menus, the Composition Flowchart, Timeline column/search depth, the Home screen
and every After Effects workspace in the UI-polish wave; PSD/SVG import, WebM/WAV/AIFF output and
the disk cache in the formats wave; the 3D Camera Tracker in M12.6.)

### By area (stale; superseded on 2026-10-10 by the feature-area table at the top)

| Area | Weighted parity | Remaining (agent-hours) | Biggest gaps |
|---|---|---|---|
| Layers | 88% | 4.8 | motion blur of collapsed precomps (their own motion and the parent camera's) and of animated layer content (lock-aware layer commands, delete keeping children in place, step-by-step Arrange, Transform keeping depth and separated dimensions and links kept on duplicate / paste landed in M3.11; frame blending, collapse transformations and slip edit in M4.3–M4.4) |
| Output | 97% | 0.8 | Render Settings complete (field render + 3:2 pulldown, effects/solo/guide/depth/blending/blur overrides, time sampling, storage overflow), Output Module crop/ROI/resize, alpha modes, post-render actions, PCM formats, templates with defaults, render logs, Notify (M10.2); WebM VP9 key + inter frames with motion search, loop filter and rate control; M13.4: HEVC export (MP4 `hvc1`, `effectcraft-hevcenc`: Main / Main 10, IDR + P slices, quarter-pel motion, deblocking, bit-exact with ffmpeg) and AV1 export (MP4 `av01` and WebM, `effectcraft-av1enc`: 8/10-bit key + inter frames, quarter-pel motion, loop filter, bit-exact with libdav1d) with profile, level, bitrate / constant-quality and key-frame interval options; Opus SILK (NB/MB/WB, mid/side stereo) and hybrid (SWB/FB) modes chosen by bitrate and Audio/Voice tuning, with the Opus bitrate in the Output Module. HEVC/AV1 import decodes through FilmCraft (MP4/MOV/MKV/WebM); WebM VP9 alpha (BlockAdditions) imports as straight alpha. Left: HEVC/AV1 have no B-frames, multi-reference, SAO/CDEF/restoration or alpha (compression below mature encoders); Opus has no FEC/DTX and only 20 ms frames; Photoshop sequence output, overflow for movies only checks at file creation |
| Audio | 85% | 0.5 | audio to keyframes |
| Import | 97% | 0.5 | JPX / JBIG2 images and predefined non-Identity CJK CMaps inside PDF/AI files, Illustrator EPS relying on Adobe procsets, PSD 3D layers (M13.12: mesh shadings — free-form / lattice Gouraud triangles, Coons and tensor patches — and function-based shadings, CCITT G3 / G4 images, knockout / isolated transparency groups with group alpha and soft masks on the group's result, `/W2` vertical metrics, EPS text with embedded Type 1 / CFF or bundled fonts, and Create Shapes from Vector Layer keeping images as parented footage layers in paint order; M13.6: PDF/AI text with embedded TrueType / CFF / Type 1 / Type 3 fonts and standard-14 fallback to the bundled fonts, Flate / DCT / inline / stencil images with soft masks, Indexed and ICC alternates, luminosity / alpha soft masks, the 16 blend modes, tiling patterns, calculator functions, any page via `file.import page` and the Import dialog, clip groups kept by Create Shapes from Vector Layer as layer masks / Merge Paths; PSD smart-object perspective quads and placed-layer warps — named styles and custom quilt meshes — baked as placed; PDF / PDF-compatible AI / EPS vector footage with Continuously Rasterize, layered composition import and Create Shapes from Vector Layer, and PSD smart objects with embedded files landed in M13.2; PSD as footage/composition/retain layer sizes, SVG footage earlier) |
| Automation | ≈ 99% | 0.1 | `.jsxbin` (AUT-2: concave / self-intersecting `onDraw` fills, real images in `drawImage`, image controls and icon buttons landed in M13.11; ScriptUI resource strings, `onDraw`/ScriptUIGraphics, live `onChanging`, `Socket`, Essential Graphics hooks — `addToMotionGraphicsTemplate(As)`, `canAddToMotionGraphicsTemplate`, `exportAsMotionGraphicsTemplate`, `motionGraphicsTemplateName`, controller count/names — and Watch Folder landed in M13.7; the core object model landed in M14.4; ScriptUI windows/dialogs/dockable panels with `scriptui.*` agent commands, File ▸ Scripts install + sample scripts, and the effect plug-in API (EFF-6, WebAssembly) landed in M13.1) |
| Shapes | ≈ 80% | 1.5 | Lottie can't carry stroke taper/wave (stroke Taper and Wave, Dash 2/Gap 2/Dash 3/Gap 3 and radial-gradient Highlight Length/Angle landed in M13.5; pen tool for shape paths and vertex editing in M6.5) |
| Compositions | ≈ 81% | 3.3 | Mocha-style planar tracks for templates; nested comp markers on the precomp layer bar (Preserve frame rate / resolution when nested and nested comps showing nothing past their end landed in M4.9–M4.10; Pre-compose keeping the comp's settings and New Comp from Selection's dialog in M3.12; CMP-7: Essential Graphics mirrored and linked properties landed in M13.11; Font and uniform Scale controls, Composition ▸ Open in Essential Graphics, Save Frame As ▸ Photoshop Layers / ProEXR and the VR Comp Editor landed in M13.7; the marker dialog, Composition Flowchart, Essential Graphics with master properties, `.ectemplate` templates and Responsive Design — Time landed: CMP-6, CMP-7) |
| Animation | 70% | 9.0 | puppet depth beyond pins, recording, rigging and follow-through (G2: the expression pick whip scrolls the Timeline when held at its top or bottom, #362; dropped on one of a property's values it picks that dimension (`position[0]`), #363; every property has a property pick whip in the Parent & Link column, which adds the reference as its expression or replaces the one it has, and pick whips drop on Effect Controls properties too, #363; the inline expression field grows while typing and its bottom edge drags it taller or shorter, #371; an angle's revolutions scrub and take typing in Effect Controls, the Timeline and Properties, #93; puppet pin recording with Record Options landed in M13.1; pin selection, rotate/scale handles, nulls for pins and Follow-Through in M13.14–M13.16), Wiggler/Smoother/Motion Sketch (motion-path handles and the graph editor transform box landed in M5.8; keyframe colour labels and Select Keyframe Label Group, Graph Editor snapping to markers / layer ends in M13.5) |
| Text | ≈ 96% | 0.3 | no extruded strokes (G2: the font menus list every installed family with its own styles — before, only a project asking for a font read the system fonts — and `text.fonts` / MCP `list_fonts` list them for agents; M13.12: the Variable Font Axes animator re-spaces the text — advances follow the animated axes; M13.6: variable font axes in the character style — `layer.setText variations`, the Character panel's Variable Font Axes fields — shape with HVAR / gvar advances and draw at that design-space position; OpenType features — stylistic sets, discretionary ligatures, contextual / stylistic alternates, swash, titling, ordinals, fractions, figure styles, true small caps / all small caps and superior / inferior glyphs with faux fallback — per character with the Character panel's OpenType popup and `text.fontFeatures` landed in M13.2; vertical Roman / Tate-Chu-Yoko, forced LTR paragraphs, caret on animated and path text, Variable Font Axes and Lottie style runs landed in M13.5; extruded, bevelled text in M7.6; per-character styles, paragraph settings, on-canvas editing and the `sourceText` style API in M9.9–M9.10) |
| Web | 98% | 0.1 | No shared-memory threads inside one engine instance (the decided design is one engine instance per worker; a threaded build needs nightly `build-std`) (M13.30: Render Queue, analyses and Content-Aware Fill render on each job worker's own WebGPU device in passes, particles and Advanced 3D read back under keys, layer buffers in the browser's disk cache; browser storage, Web Audio, Web Worker renders/analyses, WebGPU viewer and offline install landed in M15.2; viewer frames in frame workers fed by project diffs, Roto Brush propagation in a worker, non-blocking `wait: true` jobs and a browser Media Browser (File System Access folders, browser storage) in M13.10; WEB-1 in M13.24: GPU effects and the GPU compositor in the frame workers on their own WebGPU devices with deferred readbacks (frames render in passes; Backend Auto per comp), the disk cache in the Origin Private File System (written by the workers with sync access handles, LRU under the settings' limit, served after a reload), the storage manager (Settings ▸ Disk ▸ Browser Storage, `storage.info` / `storage.persist` / `storage.clear`), and a WGSL constant Chrome rejected (it disabled every GPU kernel in Chrome) fixed) |
| 3D | 88% | 5.5 | reflections of other layers (Advanced 3D surfaces reflect only the Environment light; Material Options ▸ Appears in Reflections is stored but has no visible effect yet, #261), multi-view layouts, the Extended Viewer for Advanced 3D comps (Classic 3D Extended Viewer landed in M13.5 UI completion); collapsed precomps of another size seen through the parent's camera render (fixed in M13.2); stereo rigs, orbit nulls, lights controlled by the camera, cameras/lights from glTF models, environment backgrounds, Advanced 3D motion blur, blend modes and track mattes landed in M7.7; Classic 3D iris-shaped bokeh with highlights, progressive depth of field on tilted layers and the focus-link commands landed in M13.5; Advanced 3D depth of field with the iris and highlight options, collapsed precomps as real Advanced 3D geometry and extruded text/shape strokes landed in M13.8; Advanced 3D (glTF/OBJ models, primitives, extruded text and shapes, PBR, image-based light, shadow maps, GPU rasteriser) in M7.4–M7.6; Advanced 3D end to end on the GPU (motion blur, iris depth of field, compositing) in M13.11 |
| Effects | ≈ 85% | 3.0 | GPU versions of the remaining effects (280 run on the GPU since M13.29, EFF-5: M13.28 added the remaining pixel effects and the geometry and audio generators (Lightning, Advanced Lightning, Beam, Lens Flare, Radio Waves, Vegas, Stroke, Scribble, Write-on, Paint Bucket, Eyedropper Fill, CC Glue Gun, CC Threads, Audio Spectrum / Waveform, Basic / Path Text); M13.29 added the particle effects (CC Particle World / Systems II, Particle Playground, CC Ball Action, CC Pixel Polly, CC Scatterize, Curl Noise) and removed the fallbacks of Shatter's wireframe views, Foam's extras, Timewarp's Matte Layer, strong Warp Fisheye / Twist bends and custom `.ocio` configs; M13.23 added the 3D Channel and Immersive Video families, Apply Color LUT, the OCIO effects, Color Profile Converter and the simulations' render passes (Card Wipe included); M13.22 added the CC light family, the CC transitions, Block Dissolve, Radial Shadow, CC Bender / Blobbylize / Cylinder / Sphere / Spotlight / Environment, 3D Glasses, Numbers, Timecode and the time effects joined the 166 of M13.13; Fractal and the analysis / tool effects still render on the CPU) and the missing controls listed as partial in [effects.md](effects.md) (every After Effects effect exists since M9.11, M12.5 and M12.6; parameter names, order, twirl-downs, popups, units and defaults were aligned in M9.12) |
| Interface | 75% | 5.0 | more Learn tutorials and pixel-level fidelity of dialogs (G2 on 9 October: the viewer's resize cursors point the way each handle scales and turn with the layer, #393; Ctrl/Cmd-click on the Timeline's or the Composition panel's current-time display toggles Timecode / Frames, #370; Tab / Shift+Tab in a value being typed commits it and types in the next / previous value of the panel (Timeline, Effect Controls), #361; right-clicking a property's name in the Timeline opens its menu (Reset, Edit Value…, Separate Dimensions, Add / Remove Expression, Add Property to Essential Graphics), #407; the in-window menu bar works from the keyboard (Alt, arrows, Enter, Escape) and the app's shortcuts wait while a menu is open, #279; the Timeline's composition marker bin and numpad * add comp markers, #275; Settings ▸ Appearance ▸ UI Scale (75–200 %, beyond After Effects, which follows the system's scaling), #284; G2 on 7 October: Project items, files and effects dropped on the Composition viewer, Project items dropped in the Timeline between layers and at a time, #85, #88, #89; popup menus moved to fit the window take clicks, so Render Queue templates apply, #117; View ▸ New Viewer landed in M13.32; an automation id on every control of Composition and Solid Settings, Group / Ungroup Shapes in the Layer menu, Window ▸ Learn, the flowchart shortcuts and Reveal in Explorer landed in M3.10 and M3.13; the Home ▸ Templates gallery (eight original built-in templates, user templates from File ▸ Save as Template…) and View ▸ Simulate Output ▸ My Custom RGB… landed in M13.25; Timeline layer reordering by drag, a non-snapping viewer pan, a working rename field and twirl arrows, and the full set of property reveal shortcuts — double presses, Alt+Shift keyframes, Ctrl+` — landed in M13.17–M13.20; the Home ▸ Learn tab with interactive tutorials and a UI fidelity pass landed in M13.5 UI completion; visual editors for Lumetri RGB / hue-saturation curves, Colorama's output cycle wheel, Glow's colour map and Reshape's correspondence points (viewer handles), a scrolling Preview panel and AE-style Composition / Timeline tabs (close, label swatch, viewer lock) landed in M13.10; Timeline outline and Project panel columns scroll horizontally, the Layer Style dialog, ROI resize handles, Pan Behind snapping and 3D Reference Axes landed in M13.5; native macOS menu bar, Timeline columns/search/reveal-add, Home screen with recent projects and all AE workspaces landed; viewer rulers/snapping/channels/snapshots landed in M0.13) |
| Project | ≈ 68% | 5.5 | OCIO displays beyond the built-in tone map; relative footage paths and relinking when a project moves (unsaved-changes prompts, a modified mark that follows undo and project files that name their version and always reopen landed in M3.9 and M3.14; auto-save and crash recovery in M3.7; Color Engine with OCIO/ACES working spaces, HDR compand/tone mapping, Rec. 2100 PQ/HLG output, Feet + Frames, display colour management, Simulate Output and the locked viewer landed in M7.7; proxies and Interpret Footage fields / pixel aspect / alpha guess landed: PRJ-8, PRJ-3; image sequence import options (Sequence, Force Alphabetical Order, frame rate, picked ranges, folders, the browser), missing-frame placeholders and Interpret Footage Start Timecode landed: #297; Interpret Footage ▸ Preserve RGB: #411) |
| Masks & roto | 74% | 5.0 | (audit estimates; Roto Brush 2.0 / 3.0's trained model, MobileSAM in the swappable `segment` module, has since landed in M13.35; variable-width mask feather points with the Mask Feather tool landed in M13.5; mask tracking and Mask Interpolation landed in M6.6; Roto Brush & Refine Edge with graph-cut segmentation, flow propagation, edge matting, decontamination and Freeze in M6.7) |
| Preview | 77% | 2.8 | GPU effects for the audio-driven effects and the stepped particle systems; content-keyed RAM preview frames (an edit drops every comp's), audio scrubbing, more snapshot slots (G2 on 7 October: Audio, Lock and Shy switches keep the cached frames and playback with audio waits for frames instead of skipping them, #103; running out of video memory renders the frame on the CPU and keeps fewer GPU frames instead of crashing, #106; RAM preview keys with the render options, least-recently-shown eviction, Auto resolution while playing, Cache Before Playback that fits, an achieved-fps readout, Cache Frames When Idle and purges that clear what they name landed in M4.5–M4.8; 3D channel, VR, OCIO, Card Wipe and the simulations' render passes run on the GPU since M13.23; Advanced 3D layers with blend modes / track mattes / Preserve Transparency and environment backgrounds composite on the GPU since M13.22; pooled GPU textures and fused quantisation since M13.13: Lower Third GPU warm 21 → 4 ms/frame, and Auto now picks the CPU or the GPU per comp from measured frame times; Advanced 3D runs — raster, motion blur, iris depth of field, compositing — and wireframes run on the GPU since M13.11: CPU warm 3394 → GPU warm 290 ms/frame at 1920×1080 (11.7×; Half 968 → 85 ms), GPU ≠ CPU on 0.005 % of pixels, on an M4 Pro under load; Classic 3D bokeh depth of field — iris shapes, highlights, fringe, progressive blur on tilted planes — runs in WGSL since M13.6: 3D Showcase GPU warm 205 → 54 ms/frame at full size on an M4 Pro; the Preview panel's five shortcuts with their own Include / Loop / Cache Before Playback / Range / Play From / Frame Rate / Skip / Resolution / Full Screen / stop options and `playback.settings.get/set` landed in M13.8; Classic 3D runs and adjustment layers composite on the GPU since M12.7; persistent disk cache with the blue cache bar landed; region of interest, snapshots, exposure and Fast Previews landed in M0.13) |
| Tracking | ≈ 96% | 0.3 | face tracking's trained model (MediaPipe Face Landmarker, M13.36) is an optional download; without it the classical (skin model + feature components + shape model) fitter is weak on profile views and occluded faces; Rolling Shutter Ripple is approximated by Subspace Warp's mesh density (face tracking (Outline Only / Detailed Features with Face Track Points and Extract & Copy Face Measurements), Subspace Warp's content-preserving mesh warp on subspace-smoothed trajectories, and radial lens distortion (k1, k2) in the camera bundle adjustment with Undistort Footage landed in M13.3; Rolling Shutter Repair in M9.11; point tracker, mask tracking, Warp Stabilizer and the 3D Camera Tracker in M6.x / M12.5 / M12.6) |
| Paint | 0% | 4.0 | Brush, Clone Stamp, Eraser |

### Effects still missing

The full catalogue with GPU / 32-bpc badges and per-effect status is [effects.md](effects.md)
(generated from the registry).

None: the 3D Camera Tracker landed in M12.6. Boris FX
Mocha and Cineware are third-party and not counted.

#### Added in M9.11 (39 effects)

| Category | Effects | Notes |
|---|---|---|
| 3D Channel | 3D Channel Extract, Cryptomatte, Depth Matte, Depth of Field, EXtractoR, Fog 3D, ID Matte, IDentifier | Read auxiliary channels: multi-layer OpenEXR (any `layer.channel`, Cryptomatte manifests from the header; EXtractoR picks a layer and channels from popups of the footage's layers and channels, `layer.channels`, #295; the footage itself shows the beauty pass, never Cryptomatte or depth, #412) or the compositor's own depth / layer-ID / normal / UV / Cryptomatte pass for precomps of 3D comps. |
| Immersive Video | VR Blur, VR Chromatic Aberrations, VR Color Gradients, VR Converter, VR De-Noise, VR Digital Glitch, VR Fractal Noise, VR Glow, VR Plane to Sphere, VR Rotate Sphere, VR Sharpen, VR Sphere to Plane | Seam-aware equirectangular processing (wrap-around, pole continuation, latitude-scaled filters), mono / over-under / side-by-side layouts; VR Converter between equirectangular, cube maps (4:3, 3:2, 6:1), sphere map and 2D. |
| Color Correction | OCIO CDL / Color Space / Display / File / Look Transform, Color Stabilizer | Built-in minimal OCIO-style config (ACES2065-1, ACEScg, ACEScct, sRGB, Rec.709, Rec.2020, Display P3, linear variants, XYZ) from published matrices and transfer functions; `.cube` / `.3dl` / `.csp` / `.spi1d` / `.spi3d` / `.spimtx` LUTs and ASC `.cc` / `.ccc` / `.cdl`. Custom `.ocio` config files are read from the OCIO v2 documentation (Configuration ▸ Custom: colorspaces and display colorspaces with the scene / display reference spaces joined by the default view transform, roles, displays with v1 and v2 views, view transforms, looks; Matrix, File, Exponent, ExponentWithLinear, Log, LogAffine, CDL, Range, Allocation, ColorSpace, Look, DisplayView, Builtin (matrices and display curves) and Group transforms), so Blender 4.x / 5.x configs give the Standard, AgX, Filmic and False Color views (#410). Grading Primary / Tone transforms, the ACES output and gamut-compression builtins and HLG pass colours through, and the effect shows a warning naming them (`effect.warning`). |
| Distort | CC Flo Motion, Liquify, Rolling Shutter Repair | Liquify strokes are effect data replayed into a displacement mesh (`liquify.stroke` / `liquify.clear` commands; no interactive viewer brush yet). Rolling Shutter Repair uses the KLT tracker. |
| Blur & Sharpen | Camera-Shake Deblur, CC Radial Blur | Deblur substitutes aligned patches from sharper neighbouring frames. |
| Audio | Compressor, Distortion, Gate | Applied in the mixdown like the other audio effects. |
| Simulation | CC Hair, Particle Playground | Particle Playground: cannon, grid, layer exploder, layer map, gravity, repel, wall, persistent property mapper (no Particle Exploder, text particles or ephemeral mapper yet). |
| Keying | Key Light | The full Keylight 1.2 control set under a generic name ("Keylight" is a vendor trademark); `lookup("Keylight (1.2)")` and the Effects & Presets search ("keylight") find it. Keyers' colour eyedroppers sample the effect's input (`effect.pickColor`), so a screen that is already keyed can still be picked. |
| Utility | Color Profile Converter | Our colour spaces and ACES; rendering intents (perceptual gamut compression, relative / absolute colorimetric, saturation). |
| Matte | Mocha shape | Mocha's export format is not public: reads a documented JSON shape format instead. |

### Update: M13.5 long-tail polish

Stroke Taper / Wave / multi-segment dashes and the radial gradient highlight (SHP-2);
variable-width mask feather points (MSK-1; mask motion blur verified by tests); the Adaptive
Sample Limit driving per-layer motion-blur samples (LYR-10); Classic 3D depth of field with
the camera's iris shape, rotation, roundness, aspect ratio, diffraction fringe and highlights,
progressive blur on tilted layers, and Link Focus Distance to Point of Interest / to Layer and
Set Focus Distance to Layer (3D-1); Timeline outline and Project panel column scrolling;
keyframe colour labels and Select Keyframe Label Group (ANM-1); vertical Roman text,
Tate-Chu-Yoko, forced LTR paragraphs, the editing caret on animated / path text and Lottie
style runs; the Layer Style dialog (LYR-9); Graph Editor snapping to markers and layer ends
(ANM-4; the reference graph existed); viewer ROI resize handles, Pan Behind snapping and 3D
Reference Axes; Animate Text ▸ Variable Font Axes. Remaining in these rows: Lottie can't carry
taper/wave (the Variable Font Axes animator changes advances since M13.12, the Character panel's
axes since M13.6; Advanced 3D's DOF has iris shapes since M13.8).

### Update: M13.8 Preview panel, Align panel, Advanced 3D completion

- **Preview panel (PRV-1)**: Shortcut popup (Spacebar, Shift+Spacebar, Numpad 0, Shift+Numpad
  0, Alt+Numpad 0), each shortcut with its own saved options (settings `preview`): Include
  Video / Audio / Overlays / Layer Controls, Loop, Cache Before Playback, Range (Work Area,
  Work Area Extended By Current Time, Entire Duration, Play Around Current Time with pre/post
  roll), Play From (Range Start / Current Time), Frame Rate (Auto or a rate), Skip, Resolution
  (Auto, Full … Quarter, Custom), Full Screen, "If caching, play cached frames" and "Move time to
  preview time". `playback.settings.get/set`; playback follows the plan (range, start, skip grid,
  rate, loop, cache-first, audio-only, resolution override, hidden overlays / layer controls,
  maximized viewer); the keys trigger their shortcut (`playback.toggle {shortcut?}`,
  `playback.ramPreview`, `playback.preview.shiftSpacebar|shiftNumpad0|altNumpad0`). Full Screen
  maximizes the Composition panel rather than taking over the display.
- **Align panel (UI-1)**: `layer.align {edge, to: composition|selection}` and `layer.distribute
  {mode}` (edges or centres) on content bounds through transform and parents, one undo step;
  Align Layers to dropdown and the Distribute Layers row.
- **Advanced 3D (3D-3)**: depth of field with the camera's iris and highlight options (the
  Classic 3D bokeh kernel, by each pixel's depth); collapsed precomps add their nested layers
  as real geometry lit by the parent (2D collapsed precomps draw nested 3D layers with the
  parent's renderer); text and shape strokes extrude as bevelled meshes in paint order.

### Update: M13.22 GPU effects, part A

- **GPU ports (EFF-5)**: 30 more effects run on the GPU (196 GPU effects), each matching the
  CPU oracle within 1/255 (8 bpc) / 1e-3 (32 bpc) on every tested pixel, directly on a buffer
  (full and half resolution, as adjustment) and composited at 8 and 32 bpc:
  - the CC light family: CC Light Rays, CC Light Burst 2.5, CC Light Sweep, CC Light Wipe
    (`gpu::fx_light`);
  - transitions and perspective (`gpu::fx_transition`): Block Dissolve (block indices computed
    on the CPU per column / row sub-sample, so the hash sees the CPU's integers), CC Glass Wipe,
    CC Grid Wipe, CC Image Wipe, CC Jaws, CC Line Sweep, CC Radial ScaleWipe, CC Scale Wipe, CC
    Twister, CC WarpoMatic, Radial Shadow, CC Bender, CC Blobbylize, CC Cylinder, CC Sphere, CC
    Spotlight, CC Environment, 3D Glasses; other layers they read (gradient, reveal, backside,
    environment, stereo views) are fitted on the CPU and uploaded;
  - Numbers and Timecode (`gpu::fx_text`): the glyph coverage (fill and stroke ring) is
    rasterised on the CPU with the effect's own stroke font and composited on the GPU with the
    box, fill and stroke;
  - time (`gpu::fx_time`): Time Difference, Time Displacement, CC Force Motion Blur, CC Wide
    Time, Pixel Motion Blur and Timewarp fetch their frames through the host as Echo does and
    combine them on the GPU (weighted sums one frame per pass, in the CPU's order); Time
    Displacement's per-pixel times and Timewarp's motion vectors (block matching and smoothing)
    are computed on the CPU, Timewarp's Whole Frames / Frame Mix / Pixel Motion frame building
    and shutter average on the GPU.
- **Fallbacks and deviations**: Timewarp with a Matte Layer renders on the CPU
  (`gpu_supported`); Card Wipe (the 3D card renderer shared with Card Dance and Shatter) and the
  3D Camera Tracker stay on the CPU. CC Environment now fades nearly transparent environment
  texels to black continuously (colour / max(alpha, 1e-3), on both paths) instead of a hard
  1e-6 un-premultiply cut-off, which turned the filtered map's float noise into colour. CC
  Sphere / CC Environment keep their longitude on the side of the ±180° seam the ray's sign
  puts it (a GPU's approximate `atan2` can land across it).
- **Advanced 3D compositing on the GPU (3D-3, PRV-3)**: runs with blend modes, track mattes
  (2D, or 3D mattes drawn through the camera) or Preserve Transparency no longer read back:
  `Renderer::split_adv_run` hands `draw_run`'s split path to the GPU (main scene, then each
  special layer far to near, hidden behind the nearer main scene and composited through the 2D
  kernels), and Environment Light Background skies draw in a kernel in Classic and Advanced 3D
  comps. The GPU walk matches the CPU compositor on the same rasters exactly at 8 and 32 bpc,
  with only the frame's own readback.

### Update: M13.13 GPU performance and the remaining GPU ports

- **Small comps on the GPU (PRV-3)**: the GPU frame allocated (and zero-initialised) several
  full-frame RGBA f32 textures per layer; working textures now come from a pool (reused once
  every encoder that could read them is submitted), transparent inputs share one zero texture,
  readback staging buffers are reused and the 8/16 bpc quantisation after each layer is fused
  into the layer's composite. `bench --gpu` on an M4 Pro under load (both binaries run back to
  back, median of 15): Lower Third 1920×1080 GPU warm 21.0 → 4.1 ms/frame (viewer 11.1 → 2.6;
  Half 10.1 → 3.1), EffectCraft Intro 27.5 → 7.8 (viewer 21.7 → 3.6), 3D Showcase ≈ 95 → ≈ 60
  (viewer ≈ 80 → ≈ 40), Adjustment Layers 93 → 36. In steady state the only transfer left is
  the frame's own readback (0.0 MB uploads per frame on every demo comp; the adjustment
  footprint is now cached and uploaded once).
- **Auto picks per comp**: `Backend::Auto` (Mercury GPU Acceleration) times each comp on both
  compositors (`render::AutoPick`: per comp, scale and path; warm-up, capped stalls, re-probe
  every 48 frames) and renders on the faster one, in the viewer and previews (a Render Queue
  job renders every frame on the project's renderer, so its runs don't differ by timing). A light comp
  stays on the CPU, which composites only the layers' bounds: Lower Third Auto 0.8 ms/frame
  (CPU warm 1.7, GPU warm 4.1, which is mostly the 32 MB readback).
- **GPU ports (EFF-5)**: Warp, Bezier Warp, Reshape, Smear, CC Bend It, CC Page Turn, Cartoon,
  Color Emboss, Circle, Ellipse, Iris Wipe, Bevel Alpha, Bevel Edges and Gaussian Blur (Legacy)
  run on the GPU (166 GPU effects); Median, Median (Legacy) and Dust & Scratches take any radius
  (a sliding 512-bin histogram per row segment above radius 4); CC Power Pin's Perspective
  below 100 % runs on the GPU. All match the CPU oracle within 1/255 (8 bpc) / 1e-3 (32 bpc) on
  every tested pixel except: Smear and Reshape allow 0.1 % (f32 point-in-outline tests on
  boundary pixels; measured 0); Warp's Fisheye and Twist bent past 50 % or with a Horizontal /
  Vertical Distortion render on the CPU (`gpu_supported`), since their Newton inverse wanders
  chaotically near the crease and f32 settles on other pixels.

### Update: M13.11 Advanced 3D on the GPU, Essential Graphics mirrors, ScriptUI paint

- **Advanced 3D on the GPU (PRV-3, 3D-3)**: a whole Advanced 3D run renders on the device
  (`gpu::adv3d`, `Renderer::prepare_adv_run`, `Accelerator::render_3d`): every motion-blur
  sub-sample's scene is rasterised (depth buffer, PBR, image-based light, shadow maps), then
  compute kernels resolve the 2×2 supersampling, average the sub-samples (nearest depth), apply
  the depth-based iris depth of field (the Classic 3D bokeh row spans and prefix-sum gather,
  highlight boost, progressive levels) and composite over the GPU canvas without a readback.
  Meshes from extruded text and shapes, cards, glTF models and collapsed precomps all go through
  it. Layers with blend modes, track mattes or Preserve Transparency (and environment
  backgrounds) keep the CPU's 2D compositing path, their scenes still rendered by
  `render_3d`. Wireframe-quality outlines draw on the GPU (exactly the CPU's pixels). The CPU
  stays the oracle: GPU-vs-CPU tests on lit scenes with shadows, environment light, DOF (hexagon
  with fringe and highlights, Fast Rectangle, radii at the 48 px cap), motion blur, extrusions
  and collapsed precomps agree within 1/255 on ≥ 99 % of pixels (measured: ≤ 0.01 %; the rest
  are silhouette pixels the two rasterisers cover differently), and the GPU post kernels match
  the CPU post-processing of the same GPU raster on ≥ 99.9 % (measured: all pixels). Fixed on
  the way: NaN environment lookups for straight up/down normals on Metal (`atan2(0, 0)`).
  `bench --gpu --adv3d` (1920×1080 Advanced 3D comp: PBR primitives, bevelled extruded text, a
  shadow-casting spot, point and environment lights, hexagonal-iris DOF with highlights,
  8-sample motion blur): CPU warm 3394 → GPU warm 290 ms/frame at 1920×1080 (11.7×; Half 968 → 85 ms), GPU ≠ CPU on 0.005 % of pixels, on an M4 Pro under load.
- **Essential Graphics mirrors (CMP-7)**: adding a property that is already in the panel adds a
  *mirror* (its own name and place; one value in the comp and in every instance — editing,
  reverting or pushing either acts on both; Font / Scale `as` controls mirror per kind);
  `essential.addMirror`. A property control can also drive further properties of the comp
  (`essential.linkProperty` / `unlinkProperty`): links follow the main property both ways and
  take each instance's value. Panel badges and a ⋯ menu (Add Mirror, Link Selected Property,
  Unlink); `addToMotionGraphicsTemplate` on a property already present adds a mirror, and
  mirrors count as controllers.
- **ScriptUI (AUT-2)**: `fillPath` fills any path (non-zero winding trapezoid tessellation:
  concave, self-intersecting, holes) instead of egui's convex-only polygons;
  `ScriptUI.newImage` / ScriptUIImage hold an image file or embedded PNG/JPEG bytes, drawn by
  `drawImage`, image controls and icon buttons; script-host threads are compiled out of the
  web build (no dead code warnings).

### Update: M13.5 UI completion & fidelity

**Extended Viewer** (Settings ▸ 3D, the viewer's Extended Viewer button, `view.extendedViewer`):
custom 3D views (and the Active Camera view with Draft 3D on) render the visible pasteboard
around the comp frame (the frame is outlined) so 3D layers reaching past the frame stay
visible; Classic 3D planes project through the offset canvas natively, Advanced 3D comps show
the frame only. **Home ▸ Learn**: three original interactive tutorials (Animate a title, Track
and attach, Make a 3D scene) whose steps highlight their UI target by automation id, advance when
the user runs the step's command, and can be performed with Show me (`learn.list`,
`learn.start`, `learn.step`, `learn.stop`, `learn.state`). **UI fidelity pass** against After
Effects 2026 captures: compact 19 pt Timeline / Project rows, flat rows with dark hairlines,
dark switch wells, the selected layer / item name in a light cell, label-coloured bars muted
toward grey, the time navigator with blue end handles and the work area bar (with the cache
bar under it) below the ruler, open twirl chevrons, a 16 pt current-time display, the
workspace bar order (Default, Review, Learn, Small Screen, Standard) and a single breadcrumb row
above the viewer. Not yet: Composition Settings / Render Queue / Settings dialogs compared
pixel by pixel (no reference captures of them yet).

### Update: M13.6 panels and content tools

The last placeholder panels became real panels with automation ids: **Lumetri Scopes**
(waveform RGB / Luma / YC, vectorscope YUV / HLS, histogram, parade RGB / YUV; Rec. 601 / 709 /
2020, 8-bit or float scale, clamp; `scopes.analyze`), **Footage** (double-click footage: its own
time ruler, play, In/Out, Overlay Edit and Ripple Insert Edit; `footage.*`), **Media Browser**
(desktop file system, favourites, thumbnails, import and drag to the Project panel or timeline;
`mediaBrowser.*`), **Metadata** (codec, size, rate, duration, colour profile, file dates; item
and project comments; `item.metadata`) and **Progress** (every render and analysis with
progress and cancel; `jobs.*`). **Content-Aware Fill** (panel and Layer ▸ New ▸ Content-Aware
Fill Layer): Object / Surface / Edge Blend, Work Area / Entire Duration, Alpha Expansion,
Lighting Correction and a reference frame from a layer, rendered to a PNG sequence in a Fill
layer above the source as a background job. **Scene Edit Detection** (markers, split, split and
precompose), **Auto-trace** (alpha / RGB / luminance, current frame or work area, to the layer
or a new layer) and **Align Video to Data** (JSON / CSV / TSV time keys) replace their disabled
menu entries. Not yet: creating a reference frame by editing a still (After Effects hands it to
Photoshop), Content-Aware Fill's learned model (ours is PatchMatch + flow propagation), and Media
Browser in the web app (landed in M13.10: browser storage and File System Access folders).

### Update: M13.12 import and text leftovers, test hardening

- **PDF / AI**: mesh shadings — free-form and lattice-form Gouraud triangle meshes (types 4, 5),
  Coons and tensor-product patch meshes (6, 7, tessellated to triangles) — and function-based
  shadings (type 1, with multi-input sampled functions), for `sh` and shading-pattern fills,
  rasterised into image nodes; CCITT Group 3 (1-D and 2-D) and Group 4 images from ITU-T T.4 /
  T.6 (checked against an independent encoder's output); knockout and isolated transparency
  groups, the current alpha / blend mode / soft mask applied to a transparency group XObject's
  combined result, and blend modes inside clipping groups reaching the backdrop; `/W2` / `/DW2`
  vertical metrics and CMap `WMode`. Not done: JBIG2 images (deferred) and predefined CJK CMaps (no permissively licensed
  pure-Rust source of the CMap data; Adobe's CMap resources are Adobe assets).
- **EPS**: text through the PostScript interpreter — `findfont` / `scalefont` / `makefont` /
  `selectfont` / `setfont`, `show`, `ashow`, `widthshow`, `awidthshow`, `xshow` / `yshow` /
  `xyshow`, `kshow`, `glyphshow`, `charpath`, `stringwidth`, re-encoded fonts (`definefont`
  copies with a new `/Encoding`) — with embedded Type 1 and CFF programs or the bundled fonts.
- **Create Shapes from Vector Layer**: images become PNG footage layers parented to the shape
  layer, and the shapes between images are split into layers of their own so the stack keeps
  the document's paint order; soft masks and clipped images are reported.
- **Text**: the Variable Font Axes animator re-spaces the text (advances at the animated
  design-space position), not only the outlines.
- **Hardening**: PDF / EPS parsers survive truncated and corrupted files (fixed an
  out-of-range stream slice and unbounded PostScript recursion); WebAssembly plug-ins get a
  memory cap, a manifest length limit and slider schema checks, with tests for missing exports,
  wrong signatures and versions, fuel exhaustion, traps and bit-identical output; script
  `Socket` validates ports, and ScriptUI resource strings accept trailing array commas.

### Update: text and viewer fidelity (G1)

Fixes from measured reports:

- Glyphs that touch or overlap ("ff") are drawn as one outline, so no light seam shows where
  their anti-aliased edges meet (#415). Characters of other colours, with Blur, or under an
  Inter-Character Blending mode are still composited one by one.
- Interior layer styles (Gradient / Color Overlay, Satin, Inner Glow, Inner Shadow, the inside of
  Stroke) blend onto the layer's colour inside its alpha, so an anti-aliased edge no longer shows
  a rim of the layer's own fill (#416). Not yet: Inner Bevel still shades an anti-aliased edge
  over the layer's own colour.
- Text layers are always rasterised at their on-screen scale, sharp at any Scale as in After
  Effects, and an unscaled layer at a fractional position is no longer rasterised a quarter octave
  too large (#390).
- The viewer's Auto resolution renders the pixels the magnification needs (Full above 50 %), and
  frames shown below 100 % are averaged (GPU frames through mip levels, CPU frames by a whole
  factor) instead of minified bilinearly (#417). Not yet: the viewer resolution and magnification
  are not remembered between launches.

### Update: plug-ins and extensions as in After Effects; ease presets become a ScriptUI panel

EffectCraft's core keeps After Effects parity; what After Effects users get from third parties is
an extension, on the same extension points After Effects has ([plugins.md](plugins.md) compares
them: scripts, ScriptUI panels, effect plug-ins, with a guide to writing a panel).

- **ScriptUI panels** in `Scripts/ScriptUI Panels/` of the settings folder are listed at the
  bottom of the Window menu and dock like any panel (as before); the panels open when the app quits
  now open again at the next launch (`window.restoreScriptPanels`), as After Effects reopens its
  workspace's panels.
- **Scripting API** (parity fixes the Ease Presets panel needed): `app.settings` lasts between
  runs (`script_settings.json`, `script.settings.get` / `save`); per-key methods
  (`setTemporalEaseAtKey`, `setInterpolationTypeAtKey`, `removeKey`, the continuous / auto-Bezier /
  roving setters) no longer change the user's key selection; `keyInTemporalEase` /
  `keyOutTemporalEase` report linear and auto-Bezier sides as they play, and
  `keyIn/OutSpatialTangent` the tangents the motion path plays; `selectedProperties` includes the
  properties of selected keyframes; `isInterpolationTypeValid` is false for Linear and Bezier on
  values that only hold.
- **Ease presets** (#254), modelled on a third-party panel, moved out of the core: they are the
  bundled ScriptUI panel `extensions/scriptui-panels/Ease Presets.jsx` (Window ▸ Ease
  Presets.jsx), written only against the public scripting API. A curve is the out side of the
  first key and the in side of the second, in the Keyframe Velocity dialog's terms (influence in
  percent, speed relative to the segment's average speed); clicking a preset eases every pair of
  neighbouring selected keyframes in one undo step, per dimension and along the motion path for
  spatial properties, leaving the keys' other sides as they play. The twelve built-in curves are
  unchanged; user presets (Save, Rename, Delete) are kept with `app.settings`, and those saved by
  the core panel of v0.6.0 (`ease_presets.json`) move there the first time settings load. The
  panel shows the working curve as a drawing and its four numbers as fields (the value graph's
  handles don't drag: ScriptUI panels get no mouse-drag events here). The `keys.easePreset.*`
  commands and `EaseCurve` are gone. Not yet: curves with more than one bend (bounce and elastic
  shapes), preset import / export files.
- **Snapping options** (#265): the Tools bar's menu has After Effects' two options, Snap Edges
  Extended and Snap to Features in Collapsed Compositions and Text Layers (the layers inside
  collapsed precomp layers snap); the per-feature toggles are gone and every feature snaps.

### Update: M5.9–M5.14 keyframes, M3.15 clipboard, M12.8 responsiveness

- **The app froze every 10 s on Windows** (M12.8): the cache budgets' memory reading started
  PowerShell on the UI thread and waited about a second for it. It runs on a background thread
  now, every 30 s.
- **Ctrl+C / Ctrl+X / Ctrl+V** (M3.15) reach Edit ▸ Copy, Cut and Paste in the app (the window
  layer sends them as clipboard events, which the shortcut dispatcher ignored), and copying puts
  a line on the system clipboard so Ctrl+V always arrives.
- **Timeline keyframes** (M5.9–M5.12): a drag follows the pointer to the end (it let go after a
  frame or two), moves every selected key as one undo step, never deletes keys it passes over,
  moves keys of time-stretched layers with the pointer, and Shift snaps to the current time,
  keys, layer ends and markers with a time / offset tooltip; Shift+click toggles selection,
  Ctrl+click switches Linear ↔ Auto Bezier, Ctrl+Alt+click toggles Hold; keys of locked layers
  can't be selected; Toggle Hold off restores Bezier; double-click edits a key's value; Select
  Equal / Previous / Following Keyframes; roving keys draw as dots.
- **Graph Editor** (M5.11): a drag moves every selected key in time and value from where it was
  grabbed, Shift keeps one axis; the transform box and Alt-drag scaling never merge keys on the
  way and move them however slowly they are dragged; the same Shift / Ctrl / Ctrl+Alt clicks.
- **Paste** (M5.13): keys copied from several layers paste onto as many layers in order; Paste
  Reversed Keyframes follows Paste's rules (stretch, target property, Hold-only properties).
- **J / K and Info** (M5.14): J / K also stop at the work area; the Info panel shows a selected
  key's property, time and value.

Since (M5.15): J / K and Select All Keyframes use what the Timeline shows — the revealed
properties' keys, layer and comp markers and the work area (`visible: [{layer, prop}]` on
`time.nextKey` / `previousKey` / `keys.selectAll`; without it, agents get every property of the
layers); the Graph Editor has Auto-Select Graph Type (the default: the speed graph when only
spatial properties are shown, else the value graph; choosing Value or Speed turns it off). Graph
Editor keys are small squares whatever their interpolation, as in After Effects.

Since (M13.34): audio scrubbing. Ctrl-dragging the current-time indicator (Cmd on macOS) plays
one frame (30–100 ms) of the comp's mix at each new frame, as in After Effects. Snippets fade in
and out and replace what is still queued, so the sound follows the pointer without lagging.
Holding still repeats nothing, the Audio panel meters follow, and the output closes after an idle
second. Agents call `playback.scrubAudio {time}`.

Since (M13.33): a precomp layer's bar shows its comp's markers, as outlined, read-only markers.
They are mapped through the layer's timing (start, stretch, time remapping, Responsive Design
stretch) and shown within its In–Out range. Hovering names them ("beat (marker in Pre)"), and a
double-click opens the nested comp at the marker. Agents read the same list with
`markers.nested {layer}`.

Since (M13.35): Roto Brush can use a trained segmentation model. MSK-4 keeps its score until G1
measures it against After Effects ([gaps.md](gaps.md)).
- The model sits behind a swappable interface: `effectcraft_segment::MaskModel` plus a registry
  where every entry must have an open-source licence compatible with ours, a source URL, a size
  and a SHA-256.
- The first model is **MobileSAM** (Apache-2.0: a TinyViT-5M encoder with Segment Anything's
  decoder), running in plain Rust (rayon and ndarray's safe GEMM, no ML framework). It reads the
  official `mobile_sam.pt` checkpoint directly and checks it against a pinned SHA-256.
- Roto Brush **Version 2.0 / 3.0** uses the model chosen in **Settings ▸ Roto Brush**; Version
  1.0 and an uninstalled model use the classic graph cut.
- On the base frame, points along the strokes prompt it. When propagating, the flow-warped matte
  prompts it (its box, points deep inside it and the matte itself). The model's probability map
  becomes a strong prior for the same graph cut, so strokes stay hard constraints and edges still
  snap to colour. A model whose mask disagrees with the flow (IoU under 0.5) is ignored for that
  frame.
- Weights are never bundled. Download (verified; uses the system `curl`) or Install from File
  puts them in the settings' `models` folder, and they load in the background.
- On the moving-disc test sequence it scores IoU 0.989 on the base frame and at worst 0.980 over
  20 propagated frames (classic: 0.973). The encoder takes about 0.7 s a frame on a 32-thread
  CPU, 1.3 s on 4 threads; its embedding is cached, so new strokes on the same frame only rerun
  the decoder (about 0.1 s).
- Agents use `roto.models`, `roto.model.select`, `roto.model.download`, `roto.model.install` and
  `roto.model.remove`. The browser build uses the classic engine for now.

Since (M13.36): face tracking can use a trained model too. TRK-3 keeps its score until G1 measures
it against After Effects ([gaps.md](gaps.md)).
- The model sits behind `effectcraft_segment::face::FaceModel` (find a face in a region, follow
  it frame to frame), in the same registry as Roto Brush's. A model reports its own points and
  says which are the tracker's landmarks and which trace the face outline, so models can be
  swapped without touching the tracker.
- The first model is **MediaPipe Face Landmarker** (Google, Apache-2.0): the BlazeFace
  short-range detector and Face Mesh V2 (478 points). Its official `face_landmarker.task` bundle
  (3.8 MB, pinned SHA-256) runs on our own TensorFlow Lite interpreter in plain Rust, which matches
  TensorFlow Lite's to about one part in a million.
- **Settings ▸ Face Tracking** chooses it. Outline Only keys the mesh's face oval into the mask;
  Detailed Features keys the mesh's eye, pupil, brow, nose and lip points; chin and jaw come from
  the outline as with the classical fitter, so Extract & Copy Face Measurements means the same with
  either. When the model finds no face in the mask, the classical fitter runs.
- On a test portrait its points are 0.85 px from Google's own pipeline on average (2.2 px at
  worst). Finding a face takes about 45 ms; following it, about 20 ms a frame (32-thread CPU).
- Agents use `face.models`, `face.model.select`, `face.model.download`, `face.model.install` and
  `face.model.remove`; `track.mask` reports the engine it tried first as `faceModel`.

Since (M13.32): View ▸ New Viewer (Alt+Shift+N, also in the Composition panel menu) opens
another Composition viewer as its own panel, as in After Effects. The viewer in use is locked,
so it keeps its comp. One viewer is active: it is the interactive one and shows the active comp.
The others show their own comp's frame at that comp's current time, and a click (or their tab)
makes them active. Opening a comp while the active viewer is locked shows it in an unlocked
viewer, or in a new one if every viewer is locked. Closing a viewer hands over to another one,
with that viewer's comp. Each viewer's tab is named after its comp, with its own lock.

Since (M4.14): RAM preview frames are keyed by their comp's content, not the project revision. An
edit keeps the cached frames (the green bar) of every comp it doesn't touch, and undo finds the
frames of the state it returns to, as in After Effects. The identity costs about 0.1 ms on the
1500-layer test project: it hashes the addresses of the comps a comp draws, and the RAM cache
keeps a project snapshot for every identity with frames, so those addresses can't be reused
meanwhile. Footage, solids, proxies and the project settings are hashed by value. At most 64
states are kept, so a long drag doesn't hold on to every intermediate state. The disk cache's
content key now also covers proxies and Use Proxy.

Since (M13.30–M13.31): the Tools bar and About dialog carry the ArtCraft mark. The
Home screen is laid out like After Effects' and covers the whole workspace. A left rail holds New
Project / Open Project, the Home, Templates and Learn pages and, at its foot, the community
links. The Home page has a "Welcome to EffectCraft" heading, quick-start tiles (New Composition,
Open Demo Project, Import Footage, New from Template) and the recent projects as a filterable
table with thumbnails and Name / Opened ("3 hours ago") / Size / Kind columns. New automation ids:
`home.filter`, `home.templates`, `home.file.import`.

Since (M4.13): motion blur follows a collapsed precomp layer's own motion. Its nested layers are
drawn with the precomp layer's transform at every sub-sample, and with the containing comp's
shutter, as they are drawn into its frames. Shape and text layers whose content animates within
the shutter (paths, Trim Paths, shape transforms, text animators) are drawn at Samples Per Frame
times and averaged, as After Effects does for shape layers; content that holds still is drawn
once.

Since (M3.15): the Timeline shows one tab per open comp, as in After Effects. Double-clicking a
comp in the Project panel opens it as a new tab next to the comps already open; it no longer
renames (Enter or the context menu still do). A tab's × closes that comp's Timeline, not the
panel. A tab dragged over a tab strip shows an insertion mark between the tabs it will land
between, and lands there; this reorders a group's own tabs too.

Since (M5.16): Key Light (After Effects' Keylight 1.2) is checked end to end on a green-screen
plate with an uneven screen, spill and soft edges (`effect.apply "Keylight (1.2)"`, pick, Clip
Black / White, render). The colour eyedroppers of Keying effects sample the effect's input
rather than the keyed frame (Ctrl/Cmd+click averages 5 × 5 pixels); agents do the same with
`effect.pickColor {effect, param, x, y, average?}` (effect space). Searching "keylight" in
Effects & Presets, or `list_effects`, finds it.

### Update: M4.9–M4.11 nested comps and motion blur

- **Composition Settings ▸ Advanced ▸ Preserve frame rate when nested or in render queue** and
  **Preserve resolution when nested** (model, renderer, Render Queue, dialog, `comp.settings`,
  `comp.info`, `CompItem.preserveNestedFrameRate` / `preserveNestedResolution`; Pre-compose
  copies them).
- **Nested comps past their end** show nothing (flattened, through a proxy, or collapsed); they
  kept drawing layers whose bars ran on.
- **Motion blur gate**: mask motion blur, effects that read the shutter and Advanced 3D follow
  the render options and the precomp layers' switches like layer motion does; mask motion blur
  uses the comp's Samples Per Frame.

Not yet: collapsed precomps don't blur with the precomp layer's own motion or the parent
camera's; animated content inside a layer (shape paths, text animators, nested frames) isn't
re-rendered per sub-sample; nested comp markers on the precomp layer bar.

### Update: M4.5–M4.8 cached playback

- **RAM preview keys** carry the render options (Fast Previews Draft / Fast Draft, Realtime
  Shadows), so a mode switch never shows frames rendered the other way; eviction drops the least
  recently shown frame; frames of an earlier revision are freed as soon as the project changes;
  the green bars count exactly what the viewer shows (scale, 3D view, region of interest).
- **Playback**: viewer Resolution Auto keeps its scale while playing (it halved it, so cached
  frames weren't reused); Cache Before Playback cuts a range larger than the RAM preview budget
  to the frames that fit and plays them (it waited forever); the Info panel shows the achieved
  frame rate and whether it is real time.
- **Cache Frames When Idle** renders the work area in the background after a second of quiet.
- **Edit ▸ Purge**: disk and snapshot purges keep the RAM preview; memory purges also drop
  decoded footage frames.

Not yet: content-keyed RAM frames (an edit drops every comp's RAM frames; the disk cache keeps
content keys), audio scrubbing, four snapshot slots.

### Update: M3.14 project files

Project files name the version that wrote them (`savedBy`); opening one that a newer EffectCraft
saved warns that what this version doesn't know is lost on saving (`file.open` reports
`savedBy`). Saving checks that the file reads back first, so a value JSON can't hold (NaN)
fails the save with a message instead of writing a project that can never be opened (the file
on disk is left as it was). Increment and Save and Save a Copy keep an XML project (`.ecprojx`)
XML; a file that isn't a project says so by name.

### Update: M3.13 menu bar

Group Shapes (Ctrl+G) and Ungroup Shapes (Ctrl+Shift+G) moved to the Layer menu itself, where
After Effects has them; Window ▸ Learn opens the Home screen's Learn tab; Composition ▸
Composition Flowchart is Ctrl+Shift+F11 and Window ▸ Flowchart Ctrl+F11 (the flowchart had
Window's shortcut); Layer ▸ Transform ▸ Center In View shows Ctrl+Home; "Reveal in Finder" reads
"Reveal in Explorer" on Windows (and "Reveal in File Manager" on Linux). Not yet: View ▸ New
Viewer (several unlocked Composition viewers at once; View ▸ Split with New Locked Viewer adds
the one locked viewer the interface has).

### Update: M3.12 Pre-compose and New Comp from Selection

- **Pre-compose** makes the new composition with the original's settings (pixel aspect,
  background, motion blur shutter / samples / switch, frame blending switch, 3D renderer); it
  only kept the size, frame rate and background.
- **File ▸ New Comp from Selection** is one undo step (it was two per item) and takes each
  item's pixel aspect. With several items selected it opens After Effects' dialog: Single or
  Multiple Compositions, Use Dimensions From, Still Duration (default: Settings ▸ Import ▸ Still
  Footage), Add to Render Queue and Sequence Layers with Overlap, Duration and Transition
  (`file.newCompFromSelection {single, dimensionsFrom, duration, addToRenderQueue, sequence,
  overlap, overlapDuration, transition}`). `layer.addItem {duration}` sets a still's length.

### Update: M3.11 layer commands

- **Lock**: locked layers can't be selected (a Timeline click, Select All, Ctrl+Up / Down step
  over them, as do shy layers while hidden) and are left alone by Clear / Delete, Arrange, the
  Layer ▸ Transform commands and Center Anchor Point (an error when every target is locked).
- **Deleting a parent** unparents its children where they are (their Position, Rotation and
  Scale are re-expressed in the composition, like the pick-whip), in the same undo step; they
  used to jump.
- **Bring Forward / Send Backward** move each selected layer one step past the next unselected
  layer, so a non-contiguous selection keeps its gaps (it was squeezed into one block).
- **Layer ▸ Transform**: Center In View and Fit to Comp keep a 3D layer's depth (Z was reset to
  0) and Fit keeps Scale / Anchor Z; Reset, Center and Fit set X / Y Position when the dimensions
  are separated (they changed the hidden combined Position, so nothing moved); Center Anchor
  Point keeps layers with separated dimensions in place.
- **Duplicate and paste** keep parents and track mattes: a copy follows the copy of its parent /
  matte when both were copied, else the original in the same composition (paste dropped them
  always).

### Update: M3.10 Layer Settings and settings dialogs

Layer ▸ Layer Settings on a solid or adjustment layer opens Solid Settings on the layer's solid
(it did nothing before): name, size, Pixel Aspect Ratio, colour and **Affect all layers that use
this solid**, which starts off as in After Effects, so a change to a solid other layers share
gives this layer a new solid of its own; nulls ask for their name, and text and shape layers
have no settings (the entry is disabled). New solids take a Pixel Aspect Ratio too. The
Composition Settings / New Composition and Solid Settings dialogs register an automation id
for every control (`dialog.comp.*`, `dialog.solid.*`), Composition Settings takes any typed
frame rate besides the list, New Composition starts from the settings of the last composition
made with it, and their colour buttons show the stored sRGB colour (they showed it lighter).

### Update: M3.9 unsaved changes

Closing a modified project asks first, as After Effects does: Quit and the window's close
button, File ▸ New Project, Open Project, Open Recent, Close Project, the demo project and a Home
template show Save / Don't Save / Cancel (Save asks for a path when the project is untitled, and
cancelling that keeps the project open); File ▸ Revert asks to discard. The window title shows
the project's path and `*` while it has unsaved changes. The modified mark now follows undo
(undoing, jumping in the History panel or rolling back a batch to the saved state clears it),
and edits that change nothing record no undo step. Agents' `engine.execute` / MCP calls are never
asked; the control channel's `app.quit {force: true}` skips the prompt.

### Update: M13.25 Home template gallery, Simulate Output ▸ My Custom RGB

**Home ▸ Templates** (UI-7; File ▸ New ▸ New Project from Template…): a gallery of eight
original built-in project templates authored in code from engine commands — Lower Third, Title
Card, Logo Reveal, Kinetic Type, Social Square 1080×1080, Vertical 9:16 Story, Slideshow and 3D
Text Orbit — each with Essential Graphics controls on its main comp and a thumbnail rendered by
our own renderer the first time the gallery shows it. **File ▸ Save as Template…** writes the
open project as an `.ectemplate` (the open template container: `manifest.json` with
`kind: "project"`, `project.ecproj`, `poster.png`, `thumb.txt`) into the config Templates
folder (in the browser: the settings store); the gallery lists built-in and user templates,
user cards can be deleted, and opening any template makes an untitled copy. Commands:
`templates.list`, `templates.thumbnail`, `templates.create`, `templates.saveAs`,
`templates.delete`, `file.newFromTemplate`; automation ids `home.tab.templates`,
`home.templates.<id>` / `.open` / `.delete`, `home.templates.saveCurrent`. M13.30: saving embeds
the footage files (default on, capped at 256 MB with a warning; the rest stays linked) and
creating from the template extracts them next to the new project (or into browser storage).

**View ▸ Simulate Output ▸ My Custom RGB…**: a dialog defines a custom output device by
primaries and white point (CIE xy) and a gamma or the sRGB curve, or reads them from an RGB
matrix/TRC ICC profile (v2/v4: colorants, `wtpt`, `chad`, `curv`/`para` tone curves; table and
parametric curves are fitted to a gamma unless they are the sRGB curve); the definition is kept
in Settings (`customRgb`) and simulated like the built-in profiles, with Preserve RGB
(`view.customRgb`, `view.simulateOutput {profile: "myCustom"}`). Generic dialog forms now register
`form.field.<key>`, `form.ok` and `form.cancel` automation ids. M13.30: LUT-based (`A2B0`)
ICC profiles too (`lut8Type`, `lut16Type`, `lutAToBType` / `lutBToAType`, evaluated from the
public ICC specification and baked into a 3D LUT for the viewer).

#### M13.26: Premiere Pro interop via timeline interchange

| Feature | After Effects | EffectCraft | Status |
|---|---|---|---|
| File ▸ Import ▸ Adobe Premiere Pro Project… | reads `.prproj` | reads Final Cut Pro XML (Premiere's File ▸ Export ▸ Final Cut Pro XML), FCPXML, OTIO, EDL, AAF and OMF (`file.importTimeline`): comps, layers by track, timing/speed/reverse/holds, Motion and Opacity keyframes, dissolves as opacity keys, nested sequences as precomps, audio layers with levels, bins as folders, missing media as placeholders | ≈ 85% |
| File ▸ Export ▸ Adobe Premiere Pro Project… | writes `.prproj` | writes Final Cut Pro XML `.xml` (also FCPXML, OTIO, EDL, AAF, OMF; `file.exportTimeline`): clips per layer with timing, Motion, Opacity and levels; precomps nested; rendered-only layers pre-rendered to ProRes 4444 with alpha | ≈ 85% |

Not yet: native `.prproj` (no public specification; awaits a decision), media embedded in AAF/OMF
(not extracted), Premiere effects other than Motion/Opacity/Volume, speed
ramps, titles/graphics, and Dynamic Link.

### Update: M13.23 GPU effects, part B (EFF-5)

40 more effects run on the GPU (236 in all, with M13.22's 30), each checked against the CPU oracle (≤ 1/255 at
8 bpc, ≤ 1e-3 at 32 bpc) directly on a buffer (full and half resolution, adjustment) and
composited at 8 and 32 bpc:

- **3D Channel** (`gpu::fx_depth`): 3D Channel Extract (every channel, Anti-alias), Cryptomatte,
  Depth Matte, Depth of Field, EXtractoR, Fog 3D (with a Gradient Layer), ID Matte and
  IDentifier. The layer's auxiliary channels upload as an extra texture at the aux resolution and
  are resampled onto the buffer with the CPU's index arithmetic (tested at 1× and 2× aux
  resolution and on padded buffers). Cryptomatte's ranks are reduced per aux pixel on the CPU
  (selection coverage and ID colours), then looked up on the GPU.
- **Immersive Video** (`gpu::fx_vr`): VR Blur, Chromatic Aberrations, Color Gradients, Converter
  (all nine projections and cube layouts), De-Noise (guided filter and median), Digital Glitch,
  Fractal Noise, Glow, Plane to Sphere, Rotate Sphere, Sharpen and Sphere to Plane, in mono and
  both stereo layouts: the equirectangular maths (seam wrap, pole continuation, latitude-widened
  box blurs) ported operation for operation. Exception: VR Converter re-projections that cross a
  cube-face edge or a fisheye rim may pick the neighbouring face for directions within f32
  rounding of the edge (the CPU decides in f64): up to 0.5 % of pixels allowed (measured 0).
- **Colour management** (`gpu::fx_lut`): Apply Color LUT, OCIO CDL / Color Space / Display /
  File / Look Transform and Color Profile Converter compile to colour programs
  (`effects::color_program`, next to the CPU effects) interpreted per pixel; 1D / 3D LUTs and
  cineSpace shapers are read from a storage buffer with the CPU's nearest / trilinear /
  tetrahedral interpolation and inverses (not a hardware 3D texture: its filtering rounds the
  weights and cannot do tetrahedral). Lumetri's Input LUT and Look no longer force the CPU.
  Custom `.ocio` configurations compile to the same programs (one of more than 255 steps
  renders on the CPU). Exception: nearest-neighbour lattice lookups
  may pick the other lattice point for 8 bpc inputs exactly on a rounding boundary (up to 1 %
  of pixels allowed).
- **Simulation render passes** (`gpu::fx_sim`): CC Rainfall, CC Snowfall, CC Star Burst, CC
  Bubbles, CC Drizzle, CC Hair, CC Mr. Mercury, Caustics, Wave World, Foam, Shatter, Card Dance
  and Card Wipe. The per-frame simulation / layout stays on the CPU and is shared with the CPU
  effect as a plan (sprites, textured pieces, blobs, drops, the wave grid, Caustics' prepared
  layers); a tiled rasteriser composites each pixel's items in the CPU's order and shading, with
  layer-dependent colours (refracting rain, star-burst and hair roots, bubble refraction)
  sampled per pixel. Piece plans read the frame back once (their gradient maps and textures
  default to the layer). Shatter's wireframe views and Foam's User Defined texture,
  Environment Map and flow-map preview render on the CPU.

### Update: M13.28 GPU effects, part C (EFF-5)

37 more effects run on the GPU, each checked against the CPU oracle (≤ 1/255 at 8 bpc, ≤ 1e-3 at
32 bpc) directly on a buffer with masks, another layer and an audio track (full and half
resolution, adjustment) and composited at 8 and 32 bpc:

- **Pixel ports** (`gpu::fx_pixel2`): CC Cross Blur, CC Radial Blur, CC Vector Blur, Reduce
  Interlace Flicker, CC Composite, Color Link, Cineon Converter, HDR Compander, HDR Highlight
  Compression, Grow Bounds, CC Overbrights, CC Block Load, CC Burn Film, CC Glass, CC HexTile,
  CC Mr. Smoothie, CC Plastic, Inner/Outer Key, CC Simple Wire Removal and Basic 3D.
  Inner/Outer Key's trimap and Cleanup strokes and CC Glass / Plastic's light are built next to
  the CPU effect and shared; Color Link's sample statistics run on the CPU over the read-back
  layer (none when a Source Layer is chosen). Plane blurs (`util::gauss_plane`) sum each box
  window directly, as the CPU's f64 running sums keep the small values that normalised blurs
  divide. CC Plastic now reads its Bump Layer (it read a nonexistent parameter id).
- **Generators** (`gpu::fx_gen2`): Lightning, Advanced Lightning, Beam, Lens Flare, Radio Waves,
  Vegas, Stroke, Scribble, Write-on, Paint Bucket, Eyedropper Fill, CC Glue Gun, CC Threads,
  Audio Spectrum, Audio Waveform, Basic Text and Path Text. The geometry stays on the CPU and is
  shared with the CPU effect as a per-frame plan (`effects::bolt_plan`, `lightning_segments`,
  `paint_plan`, `marks_plan` with the FFT / peak analysis, `text_passes`, `radio_plan`, Paint
  Bucket's flood-filled region, Eyedropper Fill's sample); segments and convex polygons are
  binned into 16 × 16 tiles and rasterised on the GPU with the CPU's coverage functions, glows
  and compositing included. The CPU effects were refactored to draw from the same plans with
  bit-identical output (`crates/effects/tests/gen_golden.rs`: 288 frame hashes pinned before
  the refactor). Exception: CC Threads decides band membership and fibre shade per pixel, and at
  round parameter values (the defaults) pixel centres sit exactly on band edges, where f32 and
  the CPU's f64 can fall on either side: up to 5 % of pixels allowed (measured ≤ 3.6 % at half
  resolution with the defaults, 0 at generic values).
- **Fractal** stays on the CPU: in f32, orbits near the set escape at other iterations on 0.3–1.5 %
  of the pixels even at low magnification, WGSL has no f64 and Metal's fast math defeats
  double-f32 emulation.

### Update: M13.29 GPU effects, part D (EFF-5)

7 more effects run on the GPU (280 in all, with M13.28's 37), and five CPU fallbacks are gone. Each case is
checked against the CPU oracle (≤ 1/255 at 8 bpc, ≤ 1e-3 at 32 bpc) directly on a buffer (full
and half resolution) and composited at 8 and 32 bpc; the CPU refactors behind the shared plans
are pinned bit for bit by golden hashes (`crates/effects/tests/particle_golden.rs`,
`timewarp_golden.rs`, `sim_golden.rs`).

- **Particle render passes** (`gpu::fx_particles`): CC Particle World and CC Particle Systems II
  (every particle type and transfer mode; the stepped simulation stays on the CPU or the GPU
  `ParticleSim`, sorted and shaded as the CPU does), Particle Playground (cannon, grid, text
  strokes, layer exploder, property mappers, repel, wall; the Layer Map particles' frames are
  packed into one atlas), CC Ball Action, CC Pixel Polly and CC Scatterize (their plans read the
  frame back once: the particles take the layer's colours and, for Ball Action's twist, its
  luminance). Plans too large for one item table draw in several passes, each over the previous
  one, with the same per-pixel arithmetic. Since #397 CC Particle World sees its particles
  through the comp's active camera layer, as After Effects does, and through Extras ▸ Effect
  Camera (Distance, Rotation X / Y / Z, FOV) when there is none; both compositors draw the same
  plan, and the layer cache keys the effects that read the comp camera or lights by them.
- **Curl Noise**: the fBm potential, its curl and the streamline trace as three kernels.
- **Former fallbacks**: Shatter's wireframe views (the default view) draw their lines as
  sprites; Foam's User Defined texture, Environment Map and flow-map preview draw over the
  bubble sprites; Timewarp with a Matte Layer carries the masked frames and the foreground's and
  background's own vectors in its plan and builds both layers on the GPU; custom `.ocio` configs
  compile to the colour program (matrix and offset, exponent, log / log-affine, range, CDL, file
  LUTs, groups, inverses). Warp's Fisheye / Twist past 50 % or with a distortion keeps the CPU's
  f64 Newton solve (f32, even with more iterations, lands on other roots near the crease): the
  CPU solves the inverse map from the geometry alone and the GPU samples along it, so the chain
  stays on the GPU without a readback.

### Update: M13.30 web depth and polish

- **GPU in the browser's job workers** (WEB-1): each job worker opens its own WebGPU device
  (deferred readbacks) like the frame workers. The Render Queue's export and the analyses'
  frame loops are futures (`effectcraft_render::passes`, `offload::run_request_async`): every
  frame renders in passes on the device and the job awaits the GPU between them, so Render
  Queue renders and the input frames of Warp Stabilizer, 3D Camera Tracker, Track Motion, mask
  tracking and Roto Brush (prefetched before each step) use the GPU. GPU particles and
  Advanced 3D (`raster_3d`, `render_3d`) read back under keys, so they run on the device in
  frame and job workers too (a depth-of-field run still resolves on the CPU, its scenes on
  the GPU). Content-Aware Fill became a job-worker job too (its files go back to browser
  storage; before, it ran on the page and could not write its folder). `effectcraft.info().workers`
  reports the job workers' adapter and each job's passes and readbacks; the headless-Chrome
  smoke test measures them.
- **Layer buffers in the browser's disk cache**: frame workers back their layer caches with a
  `PrefetchStore`; the page plans which layer entries to read before a request from the
  previous frames' misses (`LayerPrefetch` over the shared `DiskIndex`, one LRU order for
  frames and layers), and slow buffers are written to `effectcraft-cache/v1/layers`.
- **Templates embed footage**: `templates.saveAs {embedFootage (default true), embedLimitMB
  (default 256)}`; `templates.create {projectPath?, footageDir?}` extracts it.
- **ICC A2B0 profiles** in View ▸ Simulate Output ▸ My Custom RGB (see above).

### Highest-value gaps, in order

1. ~~On-canvas text editing and per-character styles~~ (landed: M9.9–M9.10).
2. ~~Effect Controls widgets: angle dial, point crosshair, eyedropper, curves and levels editors.~~ (second wave, 2 October)
3. ~~Viewer basics: snapping, rulers, channel view, snapshots, exposure, a drawable region of interest.~~ (M0.13)
4. ~~A GPU (wgpu) compositor, then GPU effects~~ (M12.2: 2D compositing and 16 GPU effects; M12.7:
   Classic 3D runs, adjustment layers, 58 GPU effects and GPU particles; M13.9: 152 GPU effects).
5. ~~Pen tool for shape paths, shape vertex editing, free transform~~ (M6.5); the Layer viewer.
6. ~~Motion-path handles in the viewer; graph editor transform box and snapping.~~ (M5.8)
7. ~~Point tracking and stabilization~~ (M6.x; Warp Stabilizer M12.5, 3D Camera Tracker M12.6).
8. ~~Frame blending, collapse transformations, slip edit.~~ (M4.3–M4.4)
9. ~~A real 8/16/32-bit pipeline with linear blending and colour management.~~ (third wave; OCIO / ACES in M7.7)
10. ~~Drag-to-dock and floating panels, saved workspaces, native macOS menus.~~
11. ~~Auto-save, crash recovery, recent projects~~ (M3.7, M14); unsaved-changes prompts (M3.9).
12. ~~Expression gaps: `sampleImage`, `footage()`~~ (landed with data footage, the error bar and the Expression Language menu; the `sourceText` style API landed in M9.9).
13. ~~Lottie, WebM, SVG and PSD import~~ (landed; a disk cache too).
14. ~~Puppet and paint tools~~ (second wave; puppet rigging and follow-through in M13.14–M13.16).
15. ~~Preferences and a shortcut editor that can rebind; real Wiggler, Smoother and Motion Sketch; the marker dialog.~~ (second and third waves)
16. ~~Motion blur of collapsed precomps and of animated content inside a layer (shape paths, text
    animators).~~ (M4.13)
17. ~~Content-keyed RAM preview frames, so an edit keeps the frames of comps it doesn't touch.~~
    (M4.14)
18. ~~View ▸ New Viewer (several Composition viewers)~~ (M13.32); ~~nested comp markers on the
    precomp layer bar~~ (M13.33), ~~audio scrubbing~~ (M13.34).

At the original audit the engine underneath (keyframes, expressions, shape operators, text
animators, Classic 3D, all 38 blend modes, the render queue) was already deep, and most of what was
missing was interactive tooling, settings stored but not rendered, about 69 disabled menu entries
and the large systems (tracking, puppet, paint, roto). All of those have landed since; one menu
entry stays disabled on purpose (Import ▸ Vanishing Point), and what is left is listed at the
top of this page and in items 16–18 above.


## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-10 | minor | Readiness-by-audience table: hours to ≈ 95% for each of the three numbers; the full number stays an additive weighted sum (unchanged) |
| 2026-10-10 | minor | Added mainstream practitioner (≈ 47%) and essentials user (≈ 52%) numbers with weights, discounts and GitHub user evidence; wrote down how the full ready number is built (unchanged at ≈ 45%) |
| 2026-10-10 | minor | Stage checked against the core-workflow alpha gate: passes, still alpha |
| 2026-10-10 | major | Full re-measure against After Effects 2026 26.5 (installed bundle, 26.2–26.5 release notes, repository counts); two numbers (breadth ≈ 91%, ready ≈ 45%); feature-area and dimension tables with Opus 5.5 hours; `docs/parity.md` merged in as the appendix and its stale per-area table superseded |
| 2026-10-05 | minor | Checklist updated for M3.9–M4.12 (as `docs/parity.md`) |
| 2026-10-04 | major | Checklist audit at `d39c0e8`: ≈ 99% weighted breadth, 89 / 3 / 0 of 92 |
| 2026-10-02 | major | First audit: ≈ 64% weighted breadth |
