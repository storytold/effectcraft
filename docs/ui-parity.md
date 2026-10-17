# UI parity

> **Last reviewed:** 2026-10-12 · **Last updated:** 2026-10-12 · **Change:** minor (closed issue references re-audited against `upstream/main` 470ea7a3) · **Target:** Adobe After Effects 2026

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
| Timeline | Switches, modes, parent & link, twirl-downs, reveal shortcuts (U, UU, P, S, R, T, A, E, M, F, Alt+Shift…), pick whips, graph editor, work area, markers | 75% | `ui_keyframes`, `ui_timeline_*`; expand precomps in place (#601); #456, #457, #569 and #355 (Graph Editor jitter and buttons, negative rotation, bulk keyframing) are closed; Proportional Scrubbing (26.2: Ctrl+Alt / Cmd+Opt scrub of hot text spreads the change over the selected layers by selection order, #770) matched for layers selected as a whole, not yet for individually selected properties; every layer has the Video switch, nulls, cameras and lights included (a null with its eye off has no box or handles in the viewer and isn't picked there) |
| Composition viewer | Selection, handles with direction-aware cursors, rulers, guides, grids, snapping, channels, exposure, region of interest, snapshots, 3D views, Spacebar hand | 70% | `ui_viewer`, `ui_viewers`; Cmd/Ctrl +/− doesn't zoom (#570); anchor point grab on Linux (#492); #352 (snapping) is closed; no 3D transform gizmos (#392, #347); percentage guides (26.5) exist as View ▸ Add Percentage Guide and Convert Guides to Percentage / Pixels (a guide stores a percent of the comp, so it follows comp-size changes; behaviour inferred from the release note, not compared with After Effects); Copy Frame to Clipboard (26.3) is in the Composition menu and the viewer's context menu |
| Tools panel | All After Effects tools; long-press flyouts; Tool Creates Shape / Mask; Fill / Stroke options | 80% | #213, #227 fixed; #553 (pen and vertex tools on motion paths) is closed |
| Nudging and modifiers | Arrow nudge (1 px, Shift 10 px), Alt/Option duplicates and slips, Shift constrains | 75% | #290 nudging fixed; #491 (Linux multi-layer edits) is closed |
| Numeric entry | Scrubbable values, typed entry with Tab / Shift+Tab, angle revolutions, Escape cancels, non-finite rejected | 85% | `ui_angles`, `ui_inline_edit` (#93, #141, #361); rename arrows on Linux (#488) |
| Effect Controls | After Effects' twirl-downs, angle dial, point crosshair, eyedropper, curves/levels editors, effect menu on right-click | 70% | `ui_effect_controls`, `ui_fx_editors`; 26.5 rebuilt this panel with icon controls and effect colour labels: labels exist (a colour bar on the effect header and on the effect's Timeline row, right-click Label menu, `effect.setLabel`), the rebuilt controls are not matched |
| Context menus | Layer, property, Project, Timeline empty area, work area, labels with colours | 80% | `ui_blank_context_menus`, `ui_project_rows`; #227, #290, #407 fixed; a comp's Project row menu has Composition Settings… (#1058); layer context menu has Layer Styles (#1611) |
| Keyboard shortcuts | After Effects' defaults; Keyboard Shortcuts editor that rebinds and saves sets | 85% | 351 of the 393 keyboard rows of the shortcut reference are default bindings. Panel and context scopes (one chord per scope: text formatting while editing text, mask modes, paint and camera-tool keys); double presses (`AA`, `UU`…) are preset shortcuts; held tool keys are momentary; four snapshot slots; numbered composition markers. The editor has Undo, Clear, drag to assign and clickable conflicts. Missing: the numeric keypad's shortcuts and those that need a feature that isn't there, all listed under [Missing keyboard shortcuts](#missing-keyboard-shortcuts); no shortcuts for scripts. #467 (adding shortcuts on Linux), #428 and #336 are closed |
| Drag and drop | Project → Timeline / viewer / New Comp / Render Queue; effects onto layers; files from the OS | 65% | `ui_drag_drop`; OS drops fail on Wayland (winit) and some Linux setups (#268); #552, #351 (multi-file drops) and #511 (crash dragging over the import dialog) are closed |
| Text editing on canvas | Point and paragraph text, per-character styles, Character / Paragraph panels, OpenType and variable axes | 70% | `ui_text_edit`, `ui_font_search`; paragraph alignment inverted (#316); toolbar font options (#550); font search by typing (#583) is closed; the font menus' variable-font filter (26.3) landed (#741) |
| Preview feel | Spacebar / Numpad 0 previews, RAM preview, cache bars, adaptive resolution | 55% | `ui_ram_preview`, `ui_live_preview`; stutter (#595, #304); scrubbing lag (#329, closed by #784) |
| Home screen, Learn, templates | Home with recent projects, template gallery, interactive Learn tutorials | 70% | Layout nitpicks (#581); Home literals untranslated (#574) |
| Quick Apply (26.2) | Search to apply effects, presets or commands | partial | A command palette exists; not After Effects' Quick Apply |
| Accessibility | AccessKit enabled; focus labels tested; Project panel keyboard navigation and accessible names (#776) | 20% | No screen-reader pass or keyboard-navigation audit of the other panels; no contrast check |
| UI scale | Settings ▸ Appearance ▸ UI Scale 75–200% | beyond | After Effects follows the system's scaling (#284) |

## Missing keyboard shortcuts

Every keyboard shortcut of the shortcut reference that has no default binding yet: the 42 of its
393 keyboard rows counted above (paired here where a row has an opposite, and the keys of a shape
being dragged out spelled out), and one that works at startup. `Cmd` is ⌘ on macOS and Ctrl
elsewhere. Mouse
gestures with modifier keys (Alt-click, Shift-drag…) are not part of this list. Two more are left
out on purpose: the Media Encoder queue (`Cmd+Alt+M`) and Mercury Transmit (keypad `/`).

### On the numeric keypad

The windowing layer reports a keypad key as its main-keyboard key, so these can't be bound without
also binding the main-row key, which already means something else. Only keypad `*` is told apart.

| Section | Result | Keys (keypad) |
|---|---|---|
| General | Open the selected layer, composition or footage item | `Enter` |
| General | Purge All Memory | `Cmd+Alt+/` |
| Previews | Preview only audio from the current time (macOS has `Ctrl+.` on the main keyboard) | `.` |
| Previews | Preview only audio in the work area (macOS has `Ctrl+Cmd+.` on the main keyboard) | `Alt+.` |
| Views | Turn display colour management on or off | `Shift+/` |
| Layers | Select a layer by its number | `0`–`9` |
| Layers | Toggle the selection of a layer by its number | `Shift+0`–`9` |
| Modifying layer properties | Increase / decrease Rotation by 1° | `+` / `-` |
| Modifying layer properties | Increase / decrease Rotation by 10° | `Shift++` / `Shift+-` |
| Modifying layer properties | Increase / decrease Opacity by 1% | `Ctrl+Alt++` / `Ctrl+Alt+-` |
| Modifying layer properties | Increase / decrease Opacity by 10% | `Ctrl+Alt+Shift++` / `Ctrl+Alt+Shift+-` |
| Modifying layer properties | Increase / decrease Scale by 1% | `Cmd++` or `Alt++` / `Cmd+-` or `Alt+-` |
| Modifying layer properties | Increase / decrease Scale by 10% | `Cmd+Shift++` or `Alt+Shift++` / `Cmd+Shift+-` or `Alt+Shift+-` |
| Paint tools | Set the opacity of a paint tool (9 = 90%, 1 = 10%) | `1`–`9` |
| Paint tools | Set the opacity of a paint tool to 100% | `.` |
| Paint tools | Set the flow of a paint tool | `Shift+1`–`9` |
| Paint tools | Set the flow of a paint tool to 100% | `Shift+.` |
| Markers | Set a marker at the current time and open its settings (macOS has `Ctrl+Alt+8` on the main keyboard) | `Alt+*` |

### On the main keyboard

Each needs something that isn't there yet, named in the last column.

| Section | Result | Keys | Needs |
|---|---|---|---|
| General | Interrupt a running script | `Esc` | A way to interrupt scripts |
| Preferences | Restore the default settings | Hold `Cmd+Alt+Shift` while starting | Reading the keys held at startup, and a prompt before settings are discarded |
| Views | Show Refine Edge X-ray | `Alt+X` | The X-ray view |
| Views | Prevent images from being rendered for previews | `Caps Lock` | The windowing layer reports neither the key nor its state |
| 3D layers | Switch to the Universal gizmo | `V` | 3D gizmo modes (only an axis gizmo is drawn) |
| 3D layers | Switch to the Position gizmo | `4` | 3D gizmo modes |
| 3D layers | Switch to the Scale gizmo | `5` | 3D gizmo modes |
| 3D layers | Switch to the Rotation gizmo | `6` | 3D gizmo modes |
| Masks | Enter free-transform mask editing mode | `Cmd+T` | A free-transform mode in the viewer (`path.freeTransform` takes numbers only) |
| Shape layers | Enter free-transform path editing mode | `Cmd+T` | The same free-transform mode |
| Shape layers | While dragging to create a shape: more / fewer points of a star or polygon, more / less roundness of a rounded rectangle | `Up Arrow` / `Down Arrow` | An adjustable draft of the shape being dragged out |
| Shape layers | While dragging to create a shape: outer roundness of a polygon or star, sharp / fully round corners of a rounded rectangle | `Left Arrow` / `Right Arrow` | The same draft |
| Shape layers | While dragging to create a shape: more / less inner roundness of a star | `Page Up` / `Page Down` | The same draft |
| Shape layers | While dragging to create a shape: reposition it | Hold `Space` | The same draft |
| Shape layers | While dragging to create a shape: change the outer radius of a star | `Cmd` | The same draft |
| Motion tracking | Move the feature region, search region and attach point by 1 pixel | Arrow keys | A track point selected for the keyboard |
| Motion tracking | Move the feature region, search region and attach point by 10 pixels | `Shift` + arrow keys | The same |
| Motion tracking | Move the feature region and search region by 1 pixel | `Alt` + arrow keys | The same |
| Motion tracking | Move the feature region and search region by 10 pixels | `Alt+Shift` + arrow keys | The same |

## Estimate

**≈ 60–120 h**: the open interaction issues above (25–45 h, about 0.2–0.5 h each with a scripted
test), 3D gizmos 8–15 h, multi-monitor panels 10–20 h, the 26.x interface changes 10–20 h, an
accessibility pass 15–30 h. Checking feel against After Effects needs the owner's Mac with After
Effects running; most of the rest parallelises.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-13 | minor | Keyboard shortcuts row updated after the scopes, contexts, editor and missing-keys work; the missing shortcuts listed |
| 2026-10-12 | minor | Closed issues (#302, #320, #491, #511, #553 and others) marked as closed instead of open; panels in their own windows (#486) |
| 2026-10-12 | minor | Percentage guides and effect colour labels (26.5) added; rows updated |
| 2026-10-10 | major | First UI parity checklist |
