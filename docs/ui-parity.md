# UI parity

> **Last reviewed:** 2026-10-12 · **Last updated:** 2026-10-18 · **Change:** minor (shape group Contents row elided; path-editing layer bounds #1220; closed issue references re-audited against `upstream/main` 470ea7a3) · **Target:** Adobe After Effects 2026

How closely EffectCraft's interaction matches After Effects: tools, handles, snapping, nudging,
modifier keys, shortcuts, drag behaviour, numeric entry, panels, context menus and feel. Evidence
is the code at `origin/main` 1446e27, the 42 scripted UI test files in `crates/ui-egui/tests`
(real pointer and keyboard events, run headless), [fidelity/workflow-checks.md](fidelity/workflow-checks.md)
and the issue tracker. After Effects was not driven for this pass, so "same as After Effects" means
"matches its documentation and users have not reported a difference".

**Overall ≈ 65%** (estimated): the shape of the interface is After Effects' (menus, panels,
workspaces, shortcuts), and the scripted tests cover many interactions, but users keep finding
feel and platform differences, and the 26.x interface changes are not in yet.

| Area | Status | % | Evidence and gaps |
|---|---|---:|---|
| Menu bar | After Effects' menus, order, submenus and labels; native on macOS; keyboard-operable in-window bar | 95% | 587 entries in `crates/engine/src/menus.rs`; Adobe-service entries omitted by design; `ui_menus` tests (#279) |
| Panels and workspaces | Every After Effects panel; all standard workspaces; drag-to-dock, tabs, floating panels, maximise | 75% | `ui_tabs`, `ui_panels`; panels move into their own native windows on any screen (#486 open: no dragging tabs between those windows); #302, #301 and #320 (docking, stacked-panel headers, workspace persistence) are closed |
| Timeline | Switches, modes, parent & link, twirl-downs, reveal shortcuts (U, UU, P, S, R, T, A, E, M, F, Alt+Shift…), pick whips, graph editor, work area, markers | 75% | `ui_keyframes`, `ui_timeline_*`; expand precomps in place (#601); #456, #457, #569 and #355 (Graph Editor jitter and buttons, negative rotation, bulk keyframing) are closed; Proportional Scrubbing (26.2: Ctrl+Alt / Cmd+Opt scrub of hot text spreads the change over the selected layers by selection order, #770) matched for layers selected as a whole, not yet for individually selected properties; every layer has the Video switch, nulls, cameras and lights included (a hidden null, camera or light has no box or handles in the viewer and isn't picked there; other selected hidden layers keep their layer controls and paths, #1769); a row control (a switch, the label swatch, a Modes cell) clicked on one of several selected layers acts on the whole selection, with the layers the switch doesn't apply to left out (#1849); a shape group's inner Contents (After Effects' elided Vectors group) draws no row — the group's path, fill/stroke and Transform list directly under it |
| Composition viewer | Selection, handles with direction-aware cursors, rulers, guides, grids, snapping, channels, exposure, region of interest, snapshots, 3D views, Spacebar hand | 70% | `ui_viewer`, `ui_viewers`; Cmd/Ctrl +/− doesn't zoom (#570); anchor point grab on Linux (#492); #352 (snapping) is closed; no 3D transform gizmos (#392, #347); percentage guides (26.5) exist as View ▸ Add Percentage Guide and Convert Guides to Percentage / Pixels (a guide stores a percent of the comp, so it follows comp-size changes; behaviour inferred from the release note, not compared with After Effects); Copy Frame to Clipboard (26.3) is in the Composition menu and the viewer's context menu |
| Tools panel | All After Effects tools; long-press flyouts; Tool Creates Shape / Mask; Fill / Stroke options | 80% | #213, #227 fixed; #553 (pen and vertex tools on motion paths) is closed |
| Nudging and modifiers | Arrow nudge (1 px, Shift 10 px), Alt/Option duplicates and slips, Shift constrains | 75% | #290 nudging fixed; #491 (Linux multi-layer edits) is closed |
| Numeric entry | Scrubbable values, typed entry with Tab / Shift+Tab, angle revolutions, Escape cancels, non-finite rejected | 85% | `ui_angles`, `ui_inline_edit` (#93, #141, #361); rename arrows on Linux (#488) |
| Effect Controls | After Effects' twirl-downs, angle dial, point crosshair, eyedropper, curves/levels editors, effect menu on right-click | 70% | `ui_effect_controls`, `ui_fx_editors`; 26.5 rebuilt this panel with icon controls and effect colour labels: labels exist (a colour bar on the effect header and on the effect's Timeline row, right-click Label menu, `effect.setLabel`), the rebuilt controls are not matched |
| Context menus | Layer, property, Project, Timeline empty area, work area, labels with colours | 80% | `ui_blank_context_menus`, `ui_project_rows`; #227, #290, #407 fixed; a comp's Project row menu has Composition Settings… (#1058); layer context menu has Layer Styles (#1611) |
| Keyboard shortcuts | After Effects' defaults; Keyboard Shortcuts editor that rebinds and saves sets | 75% | Shortcut editor in Settings; #467 (adding shortcuts on Linux), #428 and #336 are closed; no shortcuts for scripts |
| Drag and drop | Project → Timeline / viewer / New Comp / Render Queue; effects onto layers; files from the OS | 65% | `ui_drag_drop`; OS drops fail on Wayland (winit) and some Linux setups (#268); #552, #351 (multi-file drops) and #511 (crash dragging over the import dialog) are closed |
| Text editing on canvas | Point and paragraph text, per-character styles, Character / Paragraph panels, OpenType and variable axes | 70% | `ui_text_edit`, `ui_font_search`; paragraph alignment inverted (#316); toolbar font options (#550); font search by typing (#583) is closed; the font menus' variable-font filter (26.3) landed (#741) |
| Preview feel | Spacebar / Numpad 0 previews, RAM preview, cache bars, adaptive resolution | 55% | `ui_ram_preview`, `ui_live_preview`; stutter (#595, #304); scrubbing lag (#329, closed by #784) |
| Home screen, Learn, templates | Home with recent projects, template gallery, interactive Learn tutorials | 70% | Layout nitpicks (#581); Home literals untranslated (#574) |
| Quick Apply (26.2) | Search to apply effects, presets or commands | partial | A command palette exists; not After Effects' Quick Apply |
| Accessibility | AccessKit enabled; focus labels tested; Project panel keyboard navigation and accessible names (#776) | 20% | No screen-reader pass or keyboard-navigation audit of the other panels; no contrast check |
| UI scale | Settings ▸ Appearance ▸ UI Scale 75–200% | beyond | After Effects follows the system's scaling (#284) |

Path editing (#1220): the Composition viewer suppresses a selected layer’s transform box while
using a Pen-family tool on a visible path, or while selecting its vertices with the Selection
tool. Path points, Bézier handles and the path’s free-transform controls remain available.
Layer transform tools and ordinary layer selection restore the box. Headless `ui_viewer` tests
cover shape paths and masks, corner-vertex dragging and undo, and layers without visible paths.

## Estimate

**≈ 60–120 h**: the open interaction issues above (25–45 h, about 0.2–0.5 h each with a scripted
test), 3D gizmos 8–15 h, multi-monitor panels 10–20 h, the 26.x interface changes 10–20 h, an
accessibility pass 15–30 h. Checking feel against After Effects needs the owner's Mac with After
Effects running; most of the rest parallelises.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-18 | minor | A shape group's inner Contents (After Effects' elided Vectors group) draws no row: the group's path, fill/stroke and Transform list directly under the group |
| 2026-10-13 | patch | Layer bounds are suppressed during path-point editing (#1220); shape/mask interaction regressions |
| 2026-10-12 | minor | Closed issues (#302, #320, #491, #511, #553 and others) marked as closed instead of open; panels in their own windows (#486) |
| 2026-10-12 | minor | Percentage guides and effect colour labels (26.5) added; rows updated |
| 2026-10-10 | major | First UI parity checklist |
