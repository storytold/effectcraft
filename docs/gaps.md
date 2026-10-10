# Where EffectCraft falls short

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-10 · **Change:** major (re-measured against After Effects 2026 26.5; ranked gap list with estimates; numbers reconciled with target-app-parity.md) · **Target:** Adobe After Effects 2026 (26.5.0.89)

An honest assessment of how far EffectCraft is from being a real replacement for After Effects,
and the work that closes the gap. This document is meant for contributors and agents choosing
what to work on. [target-app-parity.md](target-app-parity.md) holds the numbers and how they were
measured; the [ROADMAP](../ROADMAP.md) summarises both. Estimates marked "≈" are judgements from
the evidence listed, not measurements. Hours are Opus 5.5 agent wall-clock hours, calibrated in
[target-app-parity.md](target-app-parity.md#calibration-of-the-hours). Update this file when the
evidence changes.

## Two different questions

**Feature breadth ≈ 91%** answers *does each After Effects feature exist?* Every one of After
Effects' effects exists by name, along with the menus, panels, Classic and Advanced 3D, tracking,
expressions, scripting and export. The self-graded 92-item checklist (appendix of
target-app-parity.md) says ≈ 99%; measuring against After Effects 26.5 itself, rather than our
catalogue, finds what that checklist never listed: the 26.2–26.5 features (Object Matte,
Substance materials, ACES 2.0, Proportional Scrubbing, paste SVG/AI as shapes…), the 621 bundled
animation presets, project and template formats, and hardware encoding.

**Ready for real work ≈ 45%** answers *can someone who uses After Effects for a living do real
work in EffectCraft?* It is still estimated, not measured. Four things hold it back: projects we
can't open (`.aep`), behaviour nobody has checked against After Effects (one live comparison so
far: scalar ease, 168 samples), reliability outside macOS, and the third-party plug-ins and
presets professional projects depend on. It sits higher than the 5 October estimate (30–50%)
because 179 issues were closed with regression tests between 5 and 10 October.

## By dimension

| Dimension | % | Hours | Evidence |
|---|---:|---:|---|
| Breadth of features | ≈ 91% | (below) | All 306 effects, the After Effects menus (587 entries) and panels, Classic and Advanced 3D, tracking, expressions, scripting, render queue. Missing: entries 7, 13, 14, 16 and 21 below |
| Behaves like After Effects | ≈ 50%, mostly unmeasured | 360–660 (features) | The first live G1 comparison checks eight scalar Rotation/ease cases (168 samples) against After Effects 26.3x87 via AEsync 2.0.4: all pass after fixing overlapping temporal influences, maximum error below 3e-11 degrees ([fidelity/README.md](fidelity/README.md)). Nothing else is compared. An outside contributor found four bugs in features marked done within a day (PRs #6–#10); users have found well over a hundred more since, and 134 issues are open |
| Opening existing After Effects work | ≈ 0% | 80–160 | `.aep` / `.aepx` / `.mogrt` / `.ffx` can't be read; After Effects plug-ins can't run. Expressions and the scripting object model are strong, so scripts and expressions carry over |
| UI/UX fidelity | ≈ 65% | 60–120 | [ui-parity.md](ui-parity.md) |
| File formats | ≈ 50% | 130–230 | [file-format-parity.md](file-format-parity.md) |
| Hardware | ≈ 40% | 60–120 | [hardware-parity.md](hardware-parity.md) |
| Localization | ≈ 12% | 60–110 + review | [localization-parity.md](localization-parity.md) |
| Stability | ≈ 45% | 40–80 | 23 never-crash PRs landed on 4 October and the "Never crash" rules bind every crate, but crash reports are open (#580 Chromatic Aberrations pan/tilt, #591 Windows ARM64 device loss, #501 crash on startup, #426 timeline drag while playing, #341 quit on Intel Mac, #306 divider drag, #234 macOS 27 exit, #511 drag over import dialog), and no CI workflow runs `cargo xtask ci` on pull requests |
| Real-user experience | ≈ 60%, uneven by platform | (in G2) | 179 issues closed 5–10 October, nearly all with regression tests ([log below](#real-user-report-log-510-october)); 134 open, many from Linux and Windows. Most checking still happens on macOS |
| Performance | unknown against After Effects, ≈ 35% | 50–100 | Internal numbers only (e.g. Advanced 3D 290 ms/frame at 1080p on the GPU on an M4 Pro). Users report stutter on cached frames (#595, #304), slow scrubbing (#329), late audio (#554). No 4K / hundreds-of-layers project tested |
| Media formats | ≈ 75% | (in file formats) | H.264, ProRes, HEVC, AV1, VP9, image sequences and audio exist. The HEVC / AV1 encoders have no B-frames, multi-reference or SAO / CDEF, so files are larger than from mature encoders. Camera formats (BRAW, R3D, ProRes RAW, variable-frame-rate phone video) are unverified |
| AI-assisted tools | ≈ 35% | 40–80 | Roto Brush 2.0 / 3.0 with MobileSAM (M13.35) scores IoU 0.989 on the base frame of our synthetic moving-disc test and ≥ 0.980 over 20 propagated frames (classic: 0.973). Face tracking with MediaPipe Face Landmarker (M13.36) matches Google's own pipeline to 0.85 px on a test portrait. Neither is compared with After Effects; Object Matte (26.2) is missing |
| Maturity | early | — | First commit 1 October 2026. ≈ 329,000 lines of almost entirely agent-written Rust, 2,704 tests, 313 GitHub issues. After Effects has around 30 years of edge cases behind it |

## Every known gap, ranked

**Alpha blockers: none.** Every core workflow passes the [alpha gate](roadmap.md#alpha-gate); the
entries below are what stands between alpha and beta (gap 1 decides beta). The rough edges the
gate lists live in gaps 5, 8, 9, 10, 11 and 17.

Ranked by user impact × how many users hit it. Each entry: what's missing, evidence, impact,
estimate, the workstream (G1–G9, below) and the parity doc it belongs to. Small single-issue bugs
are grouped by area; the issue tracker has the rest.

| # | Gap | Evidence | Impact | Estimate | Work · doc |
|---:|---|---|---|---:|---|
| 1 | **`.aep` / `.aepx` projects can't be opened** | No reader; candidate route py-aep (#248, MIT, external converter) awaits an owner decision on clean-room scope; #190 | The biggest barrier to switching: existing work, templates and client projects stay in After Effects | 80–160 h | G4 · [file-format-parity.md](file-format-parity.md) |
| 2 | **Fidelity unmeasured** beyond scalar ease | [fidelity/](fidelity/README.md): 168 samples, one property type; `compare_frames` helper exists, no corpus | Every "done" feature may still differ; users find the differences | 60–100 h harness + P0/P1 corpus; more per feature | G1 · [target-app-parity.md](target-app-parity.md) |
| 3 | **Open crash reports** | #580, #591, #501, #426, #341, #306, #234, #511 | A crash loses work; outranks features ([AGENTS.md](../AGENTS.md)) | 10–25 h | G3 · this file |
| 4 | **No CI gate on pull requests** | `.github/workflows` has release, packaging-lint, FreeBSD and Windows ARM64 jobs only; `cargo xtask ci` runs locally | Regressions reach main unseen; 49 community PRs landed in one batch (#598) | 4–8 h | G3 · this file |
| 5 | **Playback and scrubbing feel** | #595 stutter on cached frames, #304, #329 slow scrubbing, #554 / #504 / #335 audio late or crackling, #586 puppet lag | Preview is where motion designers spend their day | 25–50 h | G5 · [hardware-parity.md](hardware-parity.md) |
| 6 | **Linux and Windows interaction bugs** | #491 multi-layer edits, #488 rename arrows, #493, #492, #467 shortcuts, #268 drag and drop, #320 workspaces not persisting, #302 docking, #613 Direct3D 12 compositor, #596 GPU not detected | Many reports come from Linux; some Windows GPUs fail to start the compositor | 25–45 h | G2 · [ui-parity.md](ui-parity.md) |
| 7 | **Animation presets: 8 against 621** | After Effects ships 621 `.ffx` presets (Behaviors, Backgrounds, Shapes, Synthetics, Text, Transitions…); we ship 8 original text-animator presets and read only our `.ecpreset` | Presets are how many designers start; tutorials assume them | 20–40 h (original presets; `.ffx` reading is out of clean-room scope) | G8 · [effects-parity.md](effects-parity.md) |
| 8 | **Mask and shape bugs** | #510, #513, #527 (first Intersect/Darken mask), #509 Roto Brush size, #303 Mesh Warp, #553 pen tools on motion paths | Masks are core compositing | 10–20 h | G2 · [target-app-parity.md](target-app-parity.md) |
| 9 | **Effect behaviour bugs** | #580, #526 Fractal reset, #514 Apply Color LUT, #463 Curl Noise, #369 Motion Tile output size, #473 Cryptomatte selection, #474 multi-layer EXR decoded twice, #534–#536 community fixes pending | Each one breaks a look someone built | 10–20 h | G1/G2 · [effects-parity.md](effects-parity.md) |
| 10 | **Colour pipeline differences** | #494 32-bpc blend modes clamp and apply opacity differently; #495 no Working Gamma 2.4; #340 ProRes 4444 tagged bt709 but sRGB-encoded; ACES 2.0 / OCIO 2.5 configs (26.5) missing; #546 / #547 viewer colour | Wrong colour in delivery is a client-visible failure | 15–30 h | G1 · [file-format-parity.md](file-format-parity.md) |
| 11 | **Text rendering and layout** | #453 CJK text in comps, #316 paragraph alignment inverted, #483 squeezed at 4:3, #417 soft at Fit, #583 font search, #550 toolbar font options | Text is in almost every motion graphic | 10–20 h | G2 · [ui-parity.md](ui-parity.md) |
| 12 | **3D interaction and depth** | No 3D transform gizmos (#392, #347); Z position (#489); camera aperture (#260); reflections (#261); bevel options (#315); 3D issue reopened (#307) | Advanced 3D is After Effects' growth area since 2023 | 20–35 h | G2 · [ui-parity.md](ui-parity.md) |
| 13 | **3D model formats and materials** | After Effects 26 imports FBX, USD/USDZ, STL besides glTF and OBJ (`usd_plugins`), and Substance 3D materials with mesh displacement (26.0–26.2); we read glTF and OBJ | Product shots and 3D scenes come as FBX/USDZ | 25–45 h | — · [file-format-parity.md](file-format-parity.md) |
| 14 | **Object Matte** (26.2, disk cache in 26.5) | Not present; MobileSAM is already in `effectcraft-segment` and could back it | The new one-click roto; Roto Brush covers part of it | 15–30 h | G7 · [target-app-parity.md](target-app-parity.md) |
| 15 | **Hardware video decode and encode** | No VideoToolbox, NVENC, Quick Sync or VA-API; #498 asks for it; FilmCraft has VideoToolbox / NVENC / VA-API paths to borrow | 4K playback and export speed | 25–45 h | G5/G6 · [hardware-parity.md](hardware-parity.md) |
| 16 | **Localization depth** | Menus in 6 languages; panels in 4; effect names and ~1,650 effect parameter names untranslated; French, German, Korean, Italian, Russian (After Effects ships them) absent; #599 Hangul boxes on macOS | After Effects is used heavily in Japan, China, Korea, Brazil, Europe | 60–110 h + native review | G9 · [localization-parity.md](localization-parity.md) |
| 17 | **Media Browser lists files import refuses** | `media_browser.rs` lists TGA, DPX, HDR, AVI, MXF; `media::is_importable` has none of them; #431 any image imports as a sequence; #324 MP3 goes silent | Confusing failures on common VFX formats (DPX, TGA) | 15–30 h | G6 · [file-format-parity.md](file-format-parity.md) |
| 18 | **Relative paths and relinking** | Projects store absolute footage paths; no relink when a project moves | Moving or sharing a project breaks it | 6–12 h | G4 · [file-format-parity.md](file-format-parity.md) |
| 19 | **Plug-in ecosystem** | No After Effects SDK hosting (#600, #327 OpenFX, #334, #376 plug-in manager); our WebAssembly API has no third-party plug-ins | Many pro projects depend on Trapcode, Element, Saber… | 40–80 h for API growth; SDK hosting needs an owner decision | G8 · [scripting-parity.md](scripting-parity.md) |
| 20 | **Multi-monitor and video output** | #486 multi-screen, floating panels on other screens, Mercury Transmit to video hardware (we have a second preview window only) | Two-screen setups are standard for professionals | 10–20 h | G2 · [hardware-parity.md](hardware-parity.md) |
| 21 | **Small 26.x features** | Proportional Scrubbing (26.2), copy frame to clipboard and paste SVG/AI as shapes (26.3), variable-font filter (26.3), percentage guides and effect colour labels (26.5), the rebuilt Effect Controls (26.5) | Users of the current version expect them | 15–25 h | — · [ui-parity.md](ui-parity.md) |
| 22 | **Encoder efficiency** | HEVC / AV1 have no B-frames, multi-reference or SAO / CDEF; Opus no FEC / DTX; #372 uncompressed single-file output; #325 render over existing file | Larger files than After Effects + Media Encoder | 20–40 h | G6 · [file-format-parity.md](file-format-parity.md) |
| 23 | **Essential Graphics interchange** | Templates are `.ectemplate`, not `.mogrt`; #338 Scale exposes X and Y; #611 FilmCraft rejects exported `.fcgt` | Premiere editors can't use our templates | 15–30 h | — · [file-format-parity.md](file-format-parity.md) |
| 24 | **Timeline depth requests** | #601 expand precomps in place, #602 reorderable render stack, #604 guide-only effects, #355 bulk keyframes, #425 keyframe confusion, #428 more shortcuts | Power-user speed | 15–30 h | G2 · [ui-parity.md](ui-parity.md) |
| 25 | **Accessibility** | AccessKit is enabled in eframe and tests read focus labels, but no screen-reader pass, keyboard navigation audit or contrast check | Excludes users; After Effects supports screen readers poorly too | 15–30 h | G9 · [ui-parity.md](ui-parity.md) |

## Where we're going: workstreams in priority order

Each workstream lists what "done" means, so progress can be measured rather than self-graded.

### G1. Measure fidelity against After Effects

The most important missing piece: it turns every other estimate here into a measurement.

- Build a corpus of original test projects (our own content, no Adobe assets), one or more per
  feature id in `plan/aftereffects/feature-catalog.md`: keyframe interpolation of every kind,
  blend modes, track mattes, each effect at default and at non-default settings, text animators,
  expressions, 3D, motion blur, time remapping.
- Drive After Effects through ExtendScript (see CLAUDE.md) to record property values at sampled
  times and render reference frames; render the same projects in EffectCraft headless.
- Compare values exactly and frames with a perceptual metric; publish a per-feature fidelity
  score and a fidelity column in target-app-parity.md.
- Clean room: After Effects output stays local in `plan/aftereffects/ref/` (gitignored). Commit
  only our projects, the harness and the scores, never After Effects frames.
- Done when: every P0 and P1 feature has at least one corpus project, scores are reproducible from
  one command, and target-app-parity.md reports breadth and measured fidelity side by side.

### G2. Real-user reliability on every platform

- Fix user issues as they come in, each with a regression test, and answer the reporter (#41–#47
  and #63–#68 are handled, as are the Windows report that the font menus missed installed fonts
  and the reports of 6 October, #85, #88, #89, #93, #103, #106 and #117, and of 7 October, #198,
  #201, #203–#206, #208, #209 and #212, and of 8 October, #225, #226 and #229–#231; #227 is
  handled except multi-monitor support; Wayland file drops wait on winit).
- Run the headless snapshot and control-channel checks on Linux (X11 and Wayland) and Windows, not
  only macOS. Interactions that only fail with real input (docking drags, viewer pan, drag-and-drop
  import, inline rename) need scripted input tests.
- Regression evidence for [#66](https://github.com/storytold/effectcraft/issues/66): scripted
  pointer tests in `ui_viewer` and `ui_text_edit`, checked headlessly on Linux, cover viewer clicks,
  marquee selection and text picking when layers are hidden or excluded by solo. They also check
  hidden/locked solo layers and solo layers outside their active time range.
- Regression evidence for [#67](https://github.com/storytold/effectcraft/issues/67): scripted
  input tests in `ui_project_delete`, checked headlessly on Linux, cover Delete/Backspace on
  Project items, multiple selection and undo. They check that an unrelated Timeline layer stays,
  an empty Project selection does nothing, viewer focus still deletes layers, and typing or
  dialogs do not delete items.
- Regression evidence for [#484](https://github.com/storytold/effectcraft/issues/484): the
  `shift_delete_*` input tests in `ui_project_delete`, checked headlessly on Windows, press
  Shift+Delete both as a key and as the Cut event Windows sends for it. Items in use go without
  the prompt, with their layers, in one undo step, for single items, multiple selections and
  folder contents. Nothing happens with an empty selection, in other panels, while typing or with
  a dialog open, and the next Delete still asks.
- Regression evidence for the reports of 6 October, checked headlessly on Windows: real pointer
  drags in `ui_drag_drop` (Project items into the Timeline between layers and at a time, and onto
  the viewer; an effect onto a layer in the viewer: #85, #88, #89), `ui_render_queue` (a template
  chosen from a menu moved to fit the window, #117), `angle_revolutions_scrub_and_take_typing`
  (#93), `audio_lock_and_shy_switches_keep_the_frames` and
  `preview_with_audio_shows_every_frame_and_sounds_once_cached` (#103), and
  `a_frame_whose_render_panics_is_released` (#106).
- Follow-up angle-input regressions in `ui_angles` (#141), checked headlessly on Linux and
  Windows, verify that Escape cancels edits and non-finite entries leave the value and undo
  history unchanged in Effect Controls, Timeline and Properties. Valid entries still commit on
  Enter or click-away.
- Regression evidence for the reports of 7 October, checked headlessly on Linux: `ui_viewer`
  `full_resolution_frames_wider_than_the_texture_limit_fit` (#201), the desktop app's
  `no_arguments_start_an_empty_project` (#204) and `device_limits_fit_the_adapter` (#198),
  `audio_spectrum_follows_time_through_the_layer_cache` (#209),
  `preview_with_audio_sounds_once_rendering_keeps_up` (#208), `ui_timeline_shapes` (the Add menu
  and Change Renderer, #206; deselecting below the layers, Alt+Shift+P and the toolbar's stroke,
  #205; linked Mask Feather, #203), `dragging_in_the_empty_area_box_selects_items` and
  `selecting_a_mask_selects_its_points` (#203), and `input_keys_ignore_later_effects` (#212).
- Regression evidence for the reports of 8 October, checked headlessly on Windows:
  `a_short_project_panel_draws` (#231), `viewer_press_prefers_a_selected_layer_under_the_pointer`
  (#230), `match_system_uses_the_system_language_where_there_is_a_catalog` (#229) and
  `apps/effectcraft-web/tests/page.mjs` (#226, #225); the #225, #226 and #231 reproductions also
  pass in the dev web build in headless Chrome 154 (device pixel ratio 2, a rejected file read,
  `GPUDevice.destroy()`).
- Regression evidence for the Turbulent Displace, Wave Warp and Bulge pinning of #227:
  `edge_pinning_displaces_all_of_a_centred_shape_layer` (render) and
  `edge_pinning_on_a_shape_layer` (GPU against CPU).
- Regression evidence for the effect points of #227 (shape and text layers measure effect points
  from the top-left of their comp-sized bounds, so Twirl, Bulge, CC Lens, Ripple and Turbulent
  Displace's Offset default to the layer centre): `default_effect_points_sit_at_the_centre_of_a_shape_layer`
  and `layer_parameters_see_a_whole_shape_layer` (render), `effect_points_on_a_shape_layer` (GPU
  against CPU), and the schema 1 → 2 conversion in
  `opening_a_schema_1_project_moves_shape_layer_effect_points_into_effect_space` (engine) and
  `effect_points_move_into_effect_space_unless_left_at_their_default` /
  `puppet_paint_liquify_and_roto_data_move_into_effect_space` (effects); see
  [architecture.md](architecture.md) §3 for what the conversion keeps.
- Regression evidence for the interface items of [#227](https://github.com/storytold/effectcraft/issues/227),
  checked headlessly on Windows: `spacebar_taps_preview_and_held_spacebar_pans_the_viewer` and
  `spacebar_drag_scrolls_the_time_graph` (Spacebar previews on its release, held it is the Hand
  tool), `right_click_in_effect_controls_shows_the_effect_menu`,
  `project_items_dropped_on_new_comp_make_a_composition` and
  `dragging_over_layer_switches_sets_them_all`.
- Regression evidence for the shape layer items of
  [#227](https://github.com/storytold/effectcraft/issues/227), checked headlessly on Windows:
  `shape_tools_draw_into_the_selected_shape_layer` and `ui_viewer`
  `shape_tool_draws_into_the_selected_shape_layer` (a shape tool adds a group to the selected
  shape layer), `shape_contents_copy_cut_and_paste_between_shape_layers` and
  `ctrl_c_and_ctrl_v_copy_a_shape_group_into_another_shape_layer` (Copy, Cut and Paste of shape
  items), `duplicate_with_shape_items_selected_duplicates_them_in_place` and
  `ctrl_d_duplicates_the_selected_shape_group_in_its_layer` (Duplicate), and
  `shape_tool_options_paint_new_shapes`, `masks_of_every_shape_tool_kind`,
  `tool_creates_mask_draws_a_mask_on_the_selected_shape_layer` and
  `fill_options_paint_the_next_shape` (Tool Creates Shape / Mask, Fill and Stroke Options).
- Done when: no open bug blocks a basic workflow (import, arrange, animate, preview, render) on
  any of the three desktop platforms, and every bug users report gets triaged within a day.

### Real-user report log (5–10 October)

Kept from the 5 October assessment's "Real-user experience" row, as evidence of what users hit and what was fixed:

Issues #41–#47 (all from the Linux AppImage 0.1.1, 5 October): panning the viewer snaps back, panels can't be resized or rearranged, drag-and-drop and double-click import don't work, layer rename gets stuck, the Layer Settings arrow does nothing, the Project panel clips, the Wayland window icon is generic. Issues #63–#68 (macOS, 5 October): scaling a layer by its handles goes wrong and can stick at 0, two 3D compasses, the viewer lags while a layer is dragged (a frame renders in 20 ms but showed after ≈ 100 ms, behind RAM preview prefetch), the Discord and other links do nothing, Delete doesn't delete in the Project panel, hidden layers can be selected in the viewer. All but the last two were fixed or confirmed fixed with a test on 5–6 October (those two have community PRs); file drops still can't work on Wayland (winit has no support). The panning, rename and arrow bugs had been fixed in v0.2.0 already; nobody had told the reporter. A Windows user (6 October, on Discord) found the font menus listed only the three bundled fonts: installed fonts were read only when a project asked for one, on every platform. Fixed with a regression test on 6 October; the menus now list every installed family and its own styles, and agents get `text.fonts` / `list_fonts`. Seven reports of 6 October (Linux .deb and Windows): Project items and files couldn't be dropped on the viewer (#85), nor effects on a layer there (#88); drops in the Timeline ignored where they landed (#89); an angle's revolutions couldn't be edited (#93); toggling Audio, Lock or Shy threw away the RAM preview and playback with audio skipped frames (#103); running out of video memory panicked every frame thread and left frames stuck (#106); choosing a Render Settings template in the Render Queue did nothing (#117: any popup menu moved up to fit the window closed on the press). All seven were fixed with regression tests on 7 October. Reports of 7 October (Linux, macOS, Windows): the app opened the demo project on every launch, so a new comp landed in it (#204); a comp wider than the GPU's texture limit (11000 × 2200) at Full resolution stopped the window drawing (#201); an Intel HD Graphics 5500 couldn't start the app (#198: most likely the window's device asked for limits that GPU doesn't have, and the failure was silent); Audio Spectrum / Waveform stood still in the viewer (#209) and preview sound waited for the whole work area to cache (#208); a layer couldn't be deselected from the time graph, a shape layer's Fill and Stroke showed only with a shape tool and a drawn shape had no stroke to change, Alt+Shift+P didn't reveal Position (#205); a mask selected in the Timeline wasn't selected in the viewer, Mask Feather had no link and took negative values, the Project panel had no selection box (#203); shape layers had no Add menu for Trim Paths and the other operations, and Classic 3D comps hid Geometry Options (#206); dragging Puppet pins re-rendered the layer under the Puppet at every move (#212). All were fixed with regression tests the same day (#219–#222); the #198 fix couldn't be checked on that GPU. Reports of 8 October (browser, Windows, macOS): an empty project at 1280 × 800 with device pixel ratio 2 crashed on its first frame, because the Project panel's scroll bar clamped its thumb to a track shorter than the thumb (#231, any short panel on every platform); Media Browser ▸ Add Files… dropped a file it couldn't read without a word (#226); after the page's WebGPU device was lost, the browser editor froze on its last frame while edits went on (#225); a press in the viewer always took the topmost layer, so a selected layer behind others couldn't be dragged (#230); the language didn't follow the system's (#229); and a list of missing After Effects behaviour (#227): drawing into, copying, pasting and duplicating shape contents, Tool Creates Shape / Mask and Fill / Stroke Options, Turbulent Displace, Wave Warp and Bulge pinning all but a shape layer's bottom-right quadrant, a Spacebar hold that previewed instead of being the Hand tool, the Effect menu in Effect Controls, dropping items on New Composition and dragging over layer switches. All were fixed with regression tests the same day (#235, #237–#241); multi-monitor support and the Project panel's empty-area menu (community PR #210) remain from #227, and effect points on shape and text layers still sit half a comp off. A list of tweaks from an Intel iMac (#290): the layer bar didn't show where extended footage runs past its source, and Enable Time Remapping then ended at the extended out point; footage items had no Interpret Footage in their menu and the work area bar no menu; Label menus named the labels without their colours; the arrow keys didn't nudge layers; a straight motion path had no Bezier handles; Delete on Time Remap deleted the layer; a mask being drawn with the Pen blacked out its layer until it was closed; a double-click on a mask point didn't select the whole mask. All were fixed with regression tests the same day; the rest of #290 had been fixed already (#221, #238, #255) or came with community PRs #210 (the Timeline's empty-area menu) and #213 (tool flyouts on a long press). A report of 9 October (#397): CC Particle World ignored the comp camera and looked flat. It now sees its particles through an active camera layer, as After Effects does, and through its own Extras ▸ Effect Camera (Distance, Rotation X / Y / Z, FOV, added) otherwise; the layer cache also missed camera and light moves for the effects that read them (Card Dance, Shatter, Card Wipe, Caustics). Fixed with regression tests the same day. Most checking happens on macOS. Localisation covers the menus in Japanese, Simplified and Traditional Chinese, and Ukrainian, and since 9 October the cataloged panels, dialogs and tooltips too (Simplified and Traditional Chinese, and Ukrainian; the Japanese rows of the panel catalog are still open); no accessibility work

### G3. Stability and a green main

- Keep `cargo xtask ci` green on the current stable toolchain. A toolchain release that breaks the
  gate is a P0.
- Fuzz the inputs we don't control: project files, imported media and documents, expressions,
  scripts, control-channel and MCP requests. Every crash found becomes a regression test
  (AGENTS.md "Never crash").
- Run the gate on every pull request in CI (gap 4): today no workflow runs the tests on PRs.
- Fix the open crash reports first (gap 3).
- Done when: the gate is green on stable, runs on every pull request, and the fuzzers run
  regularly without new crashes.

### G4. Open After Effects projects

- `.aep` / `.aepx` import is the largest single barrier to switching. It needs an **owner
  decision** on clean-room scope (whether and how the format may be studied), like the open
  `.prproj` question in `plan/STATUS.md`.
- A candidate for that decision (#248): [py-aep](https://github.com/forticheprod/py-aep), an MIT
  licensed Python library that reads and writes `.aep` files, reported by a user as working well.
  It can't be linked into the pure-Rust app. It could run as an external converter (`.aep` to
  `.ecproj`, the way ffmpeg is used only from outside), or its format knowledge could be ported.
  The second means reading another implementation of the format, which AGENTS.md rules out
  unless the clean-room scope allows it.
- Also in this area: relative footage paths and relinking when a project moves (gap 18).
- Done when: the decision is recorded and, if approved, a corpus of real-world-shaped projects
  opens with its comps, layers, keyframes, effects and expressions intact.

### G5. Performance at real-world scale

- Benchmark projects at 1080p and 4K with many layers, heavy effects, long footage and nested
  comps; record preview frame times, RAM preview fill and render times on reference machines.
- Compare with After Effects on the same machine where possible.
- Done when: benchmark numbers are tracked over time and regressions fail a check.

### G6. Media depth

- Encoder efficiency: B-frames and multi-reference for HEVC / AV1, SAO / CDEF / restoration, Opus
  FEC / DTX.
- Verify decode of the camera and phone formats people actually bring, including variable frame
  rate; document what is not supported.

### G7. Learned models for Roto Brush and face tracking

- Decided on 5 October 2026 for Roto Brush: licensed weights with pure-Rust inference, if the
  impact on the system is low, in a composable, swappable module. Done in M13.35: the
  `effectcraft-segment` crate (a `MaskModel` interface, a registry that only accepts
  open-source licences, MobileSAM under Apache-2.0), weights downloaded on demand and verified,
  Settings ▸ Roto Brush to choose. Next: measure it against After Effects (G1); faster encoders
  (GPU) for long shots.
- Face tracking: decided the same way on 5 October 2026, done in M13.36: MediaPipe Face
  Landmarker (Google, Apache-2.0: BlazeFace detector and the 478-point Face Mesh V2) on a pure-Rust
  TensorFlow Lite interpreter, behind a `FaceModel` interface in the same crate, a 3.8 MB optional
  download chosen in Settings ▸ Face Tracking. The classical fitter stays as the fallback. Next:
  measure both against After Effects (G1).

### G8. Plug-in ecosystem

- After Effects SDK plug-ins cannot run in EffectCraft. Our own WebAssembly plug-in API exists
  ([plugins.md](plugins.md)). Grow it: documentation, examples, and original effects that cover
  what the most common third-party plug-ins are used for (particles, glows, 3D objects, sabers).
- Features that come from third parties in After Effects are extensions in EffectCraft too, built
  on the same extension points as After Effects: scripts, ScriptUI panels and effect plug-ins
  ([plugins.md](plugins.md#extension-points-compared-with-after-effects) compares them). ScriptUI
  panels in the user's ScriptUI Panels folder are listed in the Window menu, dock like any panel
  and open again at the next launch. Missing: the Startup and Shutdown script folders, keyboard
  shortcuts for scripts, ExtendScript's `#include` / `#target` directives and `.jsxbin`.
- Done so far: ease presets (#254) cover what ease-curve plug-ins are used for: easing curves kept
  by name and applied to pairs of keyframes. Modelled on a third-party panel, they are not core: a
  ScriptUI panel bundled as an optional extension (`extensions/scriptui-panels/Ease Presets.jsx`,
  Window ▸ Ease Presets.jsx) on the public scripting API, which needed `app.settings` that last,
  per-key methods that keep the key selection, and eases reported as they play
  ([plugins.md](plugins.md#bundled-extensions), [target-app-parity.md](target-app-parity.md)).

### G9. Reach

- Localisation of the interface ([localization-parity.md](localization-parity.md)): since 6–10
  October the menus are translated into Japanese, Simplified and Traditional Chinese, Ukrainian,
  Spanish and Brazilian Portuguese, and the panel catalog covers the first four. Next: effect and
  parameter names, the remaining panel strings, French, German and Korean, and native-speaker
  review of all of them.
- Accessibility (keyboard navigation, screen readers, contrast), user documentation and
  tutorials.

## For agents choosing work

1. Read `plan/STATUS.md` for owner blockers and running work, then this file.
2. Prefer G1–G3 over new features. A feature that exists but behaves differently from After
   Effects is not done.
3. When you fix a behaviour bug in a feature target-app-parity.md marks as done, add it to the evidence
   above, and update its row in the ranked list (remove it when the gap is closed). That is how this assessment stays honest.
4. Don't raise target-app-parity.md's numbers without evidence: a G1 score, a user-facing check on more than
   one platform, or both.


## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-10 | minor | Alpha gate checked: no alpha blockers |
| 2026-10-10 | major | Re-measured against After Effects 2026 26.5: two numbers (breadth ≈ 91%, ready ≈ 45%), dimension table with hours, a ranked list of 25 gaps with evidence, impact, estimate and parity doc; the real-user evidence moved into a report log under G2; links follow the rename of `parity.md` to `target-app-parity.md` |
| 2026-10-08 | minor | User issues of 6–8 October and their regression evidence |
| 2026-10-05 | major | First honest assessment: breadth ≈ 99% vs real use ≈ 30–50%; workstreams G1–G9 |
