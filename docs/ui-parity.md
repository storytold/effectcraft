# UI parity

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-10 · **Change:** major (first UI parity checklist against After Effects 2026 26.5) · **Target:** Adobe After Effects 2026

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
| Panels and workspaces | Every After Effects panel; all standard workspaces; drag-to-dock, tabs, floating panels, maximise | 75% | `ui_tabs`, `ui_panels`; dropping a panel on a stacked panel doesn't make a tab (#302); stacked-panel headers (#301); saved workspaces don't persist (#320); no panels on a second monitor (#486) |
| Timeline | Switches, modes, parent & link, twirl-downs, reveal shortcuts (U, UU, P, S, R, T, A, E, M, F, Alt+Shift…), pick whips, graph editor, work area, markers | 75% | `ui_keyframes`, `ui_timeline_*`; Graph Editor jitter and hidden buttons (#456, #457); rotation can't go negative by dragging (#569); bulk keyframing (#355); expand precomps in place (#601); Proportional Scrubbing (26.2) missing |
| Composition viewer | Selection, handles with direction-aware cursors, rulers, guides, grids, snapping, channels, exposure, region of interest, snapshots, 3D views, Spacebar hand | 70% | `ui_viewer`, `ui_viewers`; Cmd/Ctrl +/− doesn't zoom (#570); inaccurate snapping (#352); anchor point grab on Linux (#492); no 3D transform gizmos (#392, #347); percentage guides (26.5); copy frame to clipboard (26.3) |
| Tools panel | All After Effects tools; long-press flyouts; Tool Creates Shape / Mask; Fill / Stroke options | 80% | #213, #227 fixed; pen and vertex tools on motion paths (#553) |
| Nudging and modifiers | Arrow nudge (1 px, Shift 10 px), Alt/Option duplicates and slips, Shift constrains | 75% | #290 nudging fixed; Linux multi-layer edits (#491) |
| Numeric entry | Scrubbable values, typed entry with Tab / Shift+Tab, angle revolutions, Escape cancels, non-finite rejected | 85% | `ui_angles`, `ui_inline_edit` (#93, #141, #361); rename arrows on Linux (#488) |
| Effect Controls | After Effects' twirl-downs, angle dial, point crosshair, eyedropper, curves/levels editors, effect menu on right-click | 70% | `ui_effect_controls`, `ui_fx_editors`; 26.5 rebuilt this panel with icon controls and effect colour labels, not matched |
| Context menus | Layer, property, Project, Timeline empty area, work area, labels with colours | 80% | `ui_blank_context_menus`; #227, #290, #407 fixed |
| Keyboard shortcuts | After Effects' defaults; Keyboard Shortcuts editor that rebinds and saves sets | 75% | Shortcut editor in Settings; adding shortcuts fails on Linux (#467); requests for more (#428, #336); no shortcuts for scripts |
| Drag and drop | Project → Timeline / viewer / New Comp / Render Queue; effects onto layers; files from the OS | 65% | `ui_drag_drop`; OS drops fail on Wayland (winit) and some Linux setups (#268); multi-file drops (#552, #351); crash dragging over the import dialog (#511) |
| Text editing on canvas | Point and paragraph text, per-character styles, Character / Paragraph panels, OpenType and variable axes | 70% | `ui_text_edit`; paragraph alignment inverted (#316); font search by typing (#583); toolbar font options (#550); variable-font filter (26.3) |
| Preview feel | Spacebar / Numpad 0 previews, RAM preview, cache bars, adaptive resolution | 55% | `ui_ram_preview`, `ui_live_preview`; stutter (#595, #304), scrubbing lag (#329), Shift+Space default (#551) |
| Home screen, Learn, templates | Home with recent projects, template gallery, interactive Learn tutorials | 70% | Layout nitpicks (#581); Home literals untranslated (#574) |
| Quick Apply (26.2) | Search to apply effects, presets or commands | partial | A command palette exists; not After Effects' Quick Apply |
| Accessibility | AccessKit enabled; focus labels tested | 20% | No screen-reader pass, keyboard-navigation audit or contrast check |
| UI scale | Settings ▸ Appearance ▸ UI Scale 75–200% | beyond | After Effects follows the system's scaling (#284) |

## Estimate

**≈ 60–120 h**: the open interaction issues above (25–45 h, about 0.2–0.5 h each with a scripted
test), 3D gizmos 8–15 h, multi-monitor panels 10–20 h, the 26.x interface changes 10–20 h, an
accessibility pass 15–30 h. Checking feel against After Effects needs the owner's Mac with After
Effects running; most of the rest parallelises.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-10 | major | First UI parity checklist |
