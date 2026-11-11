# Roadmap detail

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-11-11 · **Change:** minor (counts and statuses re-checked against `origin/main` 4ac6cdb2: the counts re-measured, the status of every issue and pull request cited checked, `.aep` import and export and OpenFX helpers described as they are; the percentages and hours are the 12 October estimates and are not re-estimated; see gaps.md for the full list of what changed) · **Target:** Adobe After Effects 2026

Forward-looking detail behind the [ROADMAP](../ROADMAP.md). The work list itself, one gap at a
time, is [gaps.md](gaps.md); the numbers are in [target-app-parity.md](target-app-parity.md).
There is no `cargo xtask scorecard` or `cargo xtask parity` in this repository yet.

## Current focus

In this order (gap numbers from [gaps.md](gaps.md)):

1. **Crashes and security** (gaps 3 and 26, G3): fix the open crash reports (44 issues are labelled
   critical) and the 16 open security audit findings. `cargo xtask ci` already runs on every pull
   request (gap 4, #2204). 15–30 h (12 October; not re-estimated).
2. **Fidelity harness** (gap 2, G1): extend the AEsync ease oracle to rendered frames; a corpus
   case for every P0 feature, then P1. 60–100 h; needs After Effects running on the owner's Mac.
3. **Real-user reliability** (gaps 5, 6, 8, 9, 11, G2): the open Linux and Windows interaction,
   mask, effect and text bugs, each with a scripted test. 60–120 h, parallel.
4. **`.aep` coverage and the owner decision** (gap 1, G4): the importer and exporter exist; the decision on clean-room scope (#190) is open. Not re-estimated (was 80–160 h).
5. **Performance** (gap 5, G5): benchmarks at 1080p and 4K; real-time cached playback. 40–70 h.

## Alpha gate

The core workflows of a motion designer in After Effects, checked on EffectCraft's main platform
(macOS) from the scripted UI tests, the live After Effects ease oracle and the open issues
**All six pass, so EffectCraft is
alpha**: each works end to end and the work saves and reopens; what's rough in them is listed, but
none blocks the workflow.

| Workflow | Works end to end? | Evidence | Hours to clear the rough edges |
|---|---|---|---|
| 1. Set up: new comp, import footage, stills, PSD/AI and audio, arrange and trim layers | yes | `file.import` and image-sequence tests, `ui_drag_drop`, `ui_project_*`; rough edges of 12 October (#431, #324, #552) are closed | 2–4 (12 October) |
| 2. Animate: keyframes, easing in the Graph Editor, motion paths, parenting | yes | 168 live samples match After Effects 26.3 ([fidelity/](fidelity/README.md)); `ui_keyframes`; rough edges of 12 October (#456, #457, #569) are closed | 3–6 (12 October) |
| 3. Motion graphics: text layers with animators, shape layers with operators | yes | `ui_text_edit`, `ui_timeline_shapes`, text and shape engine tests; rough: CJK text in comps (#453, open); paragraph alignment (#316) is closed | 3–6 (12 October) |
| 4. Composite: masks, track mattes, blend modes, keying and effects | yes | Compositor and mask tests, 308 effects (CPU/GPU agreement checked per effect when there were 306); rough: effect bugs (#580 crash, open); mask bugs (#510, #513, #527), 32-bpc blend differences (#494) and #514 are closed | 6–12 (12 October) |
| 5. Preview with audio (RAM preview, cache) | yes | `ui_ram_preview`, `ui_live_preview`; rough: stutter on cached frames (#595, #304) and sound that starts late (#3176) are open; #554 and #504 are closed | 10–20 (12 October) |
| 6. Save, reopen and deliver: `.ecproj`, auto-save, Render Queue to H.264 / ProRes / sequences | yes | Versioned-schema reopen tests, crash recovery, render-queue and encoder tests; rough: ProRes 4444 colour tag (#340), projects don't relink when moved (gap 18); the macOS default output folder (#446) is closed | 1–3 (12 October) |

The rough edges above were re-listed by issue status on 2026-11-11; the six workflows were not re-run, and the hours are the
12 October estimates.

The gate is not beta: beta also needs After Effects' own `.aep` projects to import reliably (gap 1) and
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
| Crash fixes and the security audit | 3, 26 | 15–30 (12 October) | yes | GPUs for #613 (#591 is closed); macOS and Linux Wayland testers |
| G1 fidelity harness and P0/P1 corpus | 2 | 60–100 | after the harness | After Effects runs on the owner's Mac |
| Open bug backlog (Linux, Windows, masks, effects, text, 3D) | 5, 6, 8, 9, 11, 12, 24 | 100–190 | yes | Linux / Windows testers |
| `.aep` / `.aepx` coverage and File ▸ Open | 1 | not re-estimated (was 80–160) | partly | Owner decision on clean-room scope (#190) |
| Performance benchmarks and real-time preview | 5, 15 | 50–100 | partly | — |
| Localization depth | 16 | 60–110 | yes | Native-speaker review |
| Preset library, 26.x features, Object Matte | 7, 14, 21 | 50–95 | yes | — |
| Formats: VFX stills, MXF, 3D models, templates | 13, 17, 18, 22, 23 | 80–160 | yes | — |
| Plug-in ecosystem, multi-monitor, accessibility | 19, 20, 25 | 65–130 | yes | Owner decision on SDK hosting |

To beta (≈ 75% ready and `.aep` opening): **≈ 350–620 h**. To full parity: **≈ 850–1,500 h**.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-11 | minor | Re-audit against `origin/main` 4ac6cdb2: current focus and the next-steps table name the security audit and no longer ask for a CI gate (#2204); the alpha-gate rough edges are re-listed by issue status (workflows not re-run); the `.aep` entries say the importer exists; hours not re-estimated |
| 2026-10-10 | minor | Alpha gate: six core workflows checked; all pass, stage stays alpha |
| 2026-10-10 | major | New file: milestones from ROADMAP.md, current focus, next steps with estimates |
