# Roadmap detail

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-10 · **Change:** major (new file: milestones moved from ROADMAP.md, current focus and next steps with Opus 5.5 estimates) · **Target:** Adobe After Effects 2026

Forward-looking detail behind the [ROADMAP](../ROADMAP.md). The work list itself, one gap at a
time, is [gaps.md](gaps.md); the numbers are in [target-app-parity.md](target-app-parity.md).
There is no `cargo xtask scorecard` or `cargo xtask parity` in this repository yet.

## Current focus

In this order (gap numbers from [gaps.md](gaps.md)):

1. **Crashes and the CI gate** (gaps 3–4, G3): fix the eight open crash reports; run
   `cargo xtask ci` on every pull request. 15–30 h.
2. **Fidelity harness** (gap 2, G1): extend the AEsync ease oracle to rendered frames; a corpus
   case for every P0 feature, then P1. 60–100 h; needs After Effects running on the owner's Mac.
3. **Real-user reliability** (gaps 5, 6, 8, 9, 11, G2): the open Linux and Windows interaction,
   mask, effect and text bugs, each with a scripted test. 60–120 h, parallel.
4. **Owner decision on `.aep`** (gap 1, G4): then the importer. 80–160 h after the decision.
5. **Performance** (gap 5, G5): benchmarks at 1080p and 4K; real-time cached playback. 40–70 h.

## Alpha gate

The core workflows of a motion designer in After Effects, checked on EffectCraft's main platform
(macOS) from the scripted UI tests, the live After Effects ease oracle and the open issues
(standard: craftrules `standards/progress-docs.md`, "Stages"). **All six pass, so EffectCraft is
alpha**: each works end to end and the work saves and reopens; what's rough in them is listed, but
none blocks the workflow.

| Workflow | Works end to end? | Evidence | Hours to clear the rough edges |
|---|---|---|---|
| 1. Set up: new comp, import footage, stills, PSD/AI and audio, arrange and trim layers | yes | `file.import` and image-sequence tests, `ui_drag_drop`, `ui_project_*`; rough: a still imports as a sequence (#431), an MP3 layer can go silent (#324), multi-file drops (#552) | 2–4 |
| 2. Animate: keyframes, easing in the Graph Editor, motion paths, parenting | yes | 168 live samples match After Effects 26.3 ([fidelity/](fidelity/README.md)); `ui_keyframes`; rough: Graph Editor jitter and clipped buttons (#456, #457), negative rotation drag (#569) | 3–6 |
| 3. Motion graphics: text layers with animators, shape layers with operators | yes | `ui_text_edit`, `ui_timeline_shapes`, text and shape engine tests; rough: paragraph alignment inverted (#316), CJK text in comps (#453) | 3–6 |
| 4. Composite: masks, track mattes, blend modes, keying and effects | yes | Compositor and mask tests, 306 effects with CPU/GPU agreement; rough: mask bugs (#510, #513, #527), 32-bpc blend differences (#494), effect bugs (#580 crash, #514) | 6–12 |
| 5. Preview with audio (RAM preview, cache) | yes | `ui_ram_preview`, `ui_live_preview`; rough: stutter on cached frames (#595, #304), late audio (#554, #504) | 10–20 |
| 6. Save, reopen and deliver: `.ecproj`, auto-save, Render Queue to H.264 / ProRes / sequences | yes | Versioned-schema reopen tests, crash recovery, render-queue and encoder tests; rough: the macOS default output folder is `/`, so a render fails until an output path is chosen (#446), ProRes 4444 colour tag (#340), projects don't relink when moved | 1–3 |

The gate is not beta: beta also needs After Effects' own `.aep` projects to open (gap 1) and
≈ 75% ready for real work.

## Milestones

The original build plan; all but M7 and M12 are done. What is left after them is measured
against After Effects in [gaps.md](gaps.md), not as milestones.

| | Milestone | State |
|---|---|---|
| M0 | Skeleton: crates, compositor, the After Effects style shell, control channel, `cargo xtask` | Done (web build: [web.md](web.md)) |
| M1 | Keyframes: temporal and spatial interpolation, Easy Ease, roving, velocity | Done |
| M2 | Compositing: 38 blend modes, track mattes, parenting, adjustment layers | Done |
| M3 | Project operations: settings dialogs, layer commands, `.ecproj`, undo, After Effects menu bar | Done |
| M4 | Preview: precomps, motion blur, cached playback | Done |
| M5 | Timeline depth: graph editor, keyframe clipboard and dialogs, time remapping, pick-whips, expression editor | Done |
| M6 | Shapes, masks and footage: shape operators, masks and the pen tool, video and image import | Done |
| M7 | 3D: 3D layers, cameras, lights, shadows, depth of field, 3D views and camera tools | In review (3D gizmos, FBX/USD, Substance still missing: gaps 12–13) |
| M8 | Expressions with the After Effects object model | Done |
| M9 | Text and effects: text animators, layer styles, 306 effects incl. time and audio effects | Done |
| M10 | Export: render queue, H.264, ProRes, image sequences, GIF, audio | Done |
| M11 | Animation tools: audio, Lottie, presets, Motion Sketch, Wiggler, Smoother | Done (preset library: gap 7) |
| M12 | Performance: layer cache, parallel and GPU compositing, disk cache, motion tracking; GPU versions of the remaining CPU-only effects | Mostly done (benchmarks and hardware video: gaps 5, 15) |
| M13 | Puppet, paint, Roto Brush, tracking, plug-in API, timeline depth, trained models | Done |
| M14 | Built for agents: MCP server, CLI, control channel, Settings, shortcut editor, auto-save, crash recovery | Done |
| M15 | The web app (WebAssembly, WebGPU) | Done |

## What's next, with estimates

| Next | Gaps | Opus 5.5 hours | Parallel? | Needs a human |
|---|---|---:|---|---|
| Crash fixes and a PR CI gate | 3, 4 | 15–30 | yes | GPUs for #591 / #613 |
| G1 fidelity harness and P0/P1 corpus | 2 | 60–100 | after the harness | After Effects runs on the owner's Mac |
| Open bug backlog (Linux, Windows, masks, effects, text, 3D) | 5, 6, 8, 9, 11, 12, 24 | 100–190 | yes | Linux / Windows testers |
| `.aep` / `.aepx` import | 1 | 80–160 | partly | Owner decision on clean-room scope |
| Performance benchmarks and real-time preview | 5, 15 | 50–100 | partly | — |
| Localization depth | 16 | 60–110 | yes | Native-speaker review |
| Preset library, 26.x features, Object Matte | 7, 14, 21 | 50–95 | yes | — |
| Formats: VFX stills, MXF, 3D models, templates | 13, 17, 18, 22, 23 | 80–160 | yes | — |
| Plug-in ecosystem, multi-monitor, accessibility | 19, 20, 25 | 65–130 | yes | Owner decision on SDK hosting |

To beta (≈ 75% ready and `.aep` opening): **≈ 350–620 h**. To full parity: **≈ 850–1,500 h**.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-10 | minor | Alpha gate: six core workflows checked; all pass, stage stays alpha |
| 2026-10-10 | major | New file: milestones from ROADMAP.md, current focus, next steps with estimates |
