# Control protocol

The desktop app exposes a control channel for agents, tests and the MCP bridge:

```sh
effectcraft --control 9877        # or EFFECTCRAFT_CONTROL_PORT=9877
```

The port must be a number from 1 to 65535, and `--control` overrides the variable. A bad port or an
unknown option prints an error and the app exits with status 1 before it opens a window.

- **Transport:** TCP on `127.0.0.1` only, one JSON object per line in each direction. Connections are
  persistent; requests on one connection are answered in order.
- **Request:** `{"id": 1, "method": "engine.execute", "params": {...}}` (`id` is echoed back, `params`
  defaults to `{}`).
- **Reply:** `{"id": 1, "ok": true, "result": ...}` or `{"id": 1, "ok": false, "error": "message"}`.
- Requests run on the UI thread between frames. Input methods (`ui.click`, `ui.key` and similar)
  reply after the synthetic input has been processed. Methods that need an element that isn't drawn
  yet are retried for a few frames. A request times out after 60 s.
- **Only requests are read:** every line must be a JSON object with a string `method` (blank lines are
  skipped). Anything else (text that isn't JSON, a JSON array or number, an object without `method`,
  invalid UTF-8, a line longer than 4 MiB) gets one error reply ending in "closing the connection", and
  the server closes the connection, so nothing sent after it runs. An HTTP request (for example a web
  page's cross-origin `fetch` to `127.0.0.1:<port>`) therefore can't smuggle a command in its body: its
  request line is rejected first. At most 16 connections are served at once; further ones get an error
  line and are closed. The port has no authentication, so only enable it while you use it.

Quick test:

```sh
printf '{"id":1,"method":"engine.execute","params":{"command":"comp.new","params":{"name":"A"}}}\n' | nc 127.0.0.1 9877
```

## Engine

| Method | Params | Result |
|---|---|---|
| `engine.execute` | `{command, params?}` | The command's result. Every engine command (`effectcraft-cli commands`) runs exactly as it does headless, so it never opens a dialog: `comp.new {}` creates a default comp. UI-only ids (`tool.*`, `view.*`, `window.*`, `playback.*`, `timeline.*`, `app.*`) fall through to the menu dispatcher. Unknown top-level params are rejected, and the error lists the accepted keys. |
| `ui.menu.invoke` | `{id, params?}` | Like a menu click: commands with empty params may open their dialog (New Composition, Solid Settings, file pickers). |
| Copy a Media Browser source path | `ui.menu.invoke {id: "mediaBrowser.copySourcePath", params: {path}}` | Copies the exact non-empty path string to the clipboard and returns `{path}`. Supports file/folder paths and virtual source identifiers. Missing, empty or non-string paths return an error. |
| UI commands (via either) | | `timeline.column {column, visible?}` (av, keys, label, num, comment, switches, modes, parent, in, out, duration, stretch), `timeline.sourceName {value?}`, `timeline.search {query}`, `timeline.reveal.<kind>` / `timeline.revealAdd.<kind>` (the property shortcuts and Shift+shortcut: position, scale, rotation, opacity, anchor, effects, masks, feather, levels, animated, and the double-press sets maskPath, maskOpacity, timeRemap, expressions, material, paint, selected, missingEffects, waveform), `timeline.keyAt.<anchor\|position\|scale\|rotation\|opacity>` (Alt+Shift+A/P/S/R/T), `timeline.twirlSelected` ``(Ctrl/Cmd+`)``, `timeline.collapseAll`, `flowchart.options {layers?, effects?, solids?, direction?: lr\|tb, comp?}`, `flowchart.graph {comp?}` → `{nodes, edges}`, `window.workspace {name}` (all After Effects workspaces). |
| `engine.commands` | `{filter?, enabledOnly?}` | `[{id, label, menu, shortcut, params, enabled, why}]` |
| `ui.menu.list` | `{}` | The menu tree with enablement. |
| `render.frame` | `{comp?, time?, max_side?, path?, base64?}` | Renders a comp frame through the session, independent of the viewer zoom or resolution. `comp` is an id or name (default: the active comp), `time` is in comp seconds (default: the CTI), `max_side` caps the longest side (0 = full size). Writes a PNG to `path` (default: a temp file) and returns `{comp, time, width, height, path}`. With `base64: true` it returns `{…, png: "<base64>"}` and writes no file. |

Useful query commands for `engine.execute` (they aren't journaled or undoable): `project.summary`,
`comp.info {comp?}`, `layer.tree {layer, comp?, depth?, time?}` (every node has a `path`),
`prop.get {layer, path, comp?, time?}` (a popup (enum) property also lists its `options`, `[{value, label}]`, as `layer.tree` does), `editor.state`, `command.list {filter?, enabledOnly?, schemas?}`,
`command.describe {command}` (params doc and JSON Schema), `app.capabilities` (version, counts,
export formats, parity summary), `jobs.list` (Progress panel jobs, e.g. the background
`footage.check` after File ▸ Open).

Every engine command is reachable through `engine.execute`, and every interactive widget of every
panel registers an element id; both are enforced by `crates/ui-egui/tests/it/ui_agent_audit.rs`.

## UI state

| Method | Params | Result |
|---|---|---|
| `ui.inspect` | `{}` | Window size, pixels per point, fps, serialized `UiState`, playback, time, active comp, selection, open dialog, last render ms, `keyboardFocus`, `windowFocused`, `workspaceStorageError`. |
| `ui.elements` | `{prefix?}` | `[{id, label, rect: [x, y, w, h]}]`: the widgets drawn in the last frame, in points. |
| `ui.set` | `{tool?, workspace?, theme?, focused?, viewer?, timeline?, menuBar?, home?}` | `viewer: {zoom (null = fit), res: Full\|Half\|Third\|Quarter\|Auto, pan: [x,y], grid, rulers, safeMargins, transparencyGrid}`; `timeline: {pps (null = fit), start, graphEditor, showModes, openLayers: [ids], openGroups: [uids]}`. |
| `ui.panel.show` / `ui.panel.close` | `{panel}` | Panels: `Project`, `Composition`, `Timeline`, `EffectControls`, `EffectsPresets`, `Info`, `Preview`, `Character`, `Paragraph`, `Align` and others. |
| `ui.playback` | `{action: play\|stop\|toggle\|status}` | `{playing, time, audio, levelsDb, peaksDb}` (audio: an audio preview is running; L/R meter levels and held peaks in dBFS) |
| `ui.resize` | `{width, height}` | Resizes the window (points). |
| `ui.focus` | `{}` | Activates the window and takes keyboard focus. Agents normally avoid this, because the app is started without stealing focus. |
| `app.quit` | `{force?}` | Closes the app. A modified project shows the Save / Don't Save / Cancel prompt first (automation ids `dialog.unsaved.save`, `dialog.unsaved.dontSave`, `dialog.unsaved.cancel`) unless `force: true`. |

`keyboardFocus` is `null` when no egui widget has keyboard focus. Otherwise it contains the
actual widget `id` (hexadecimal), its `rect: [x, y, w, h]` when available, an `automationId`
when the widget registered its response identity, and `registryCandidates` containing exact
rectangle matches. Candidates alone do not establish widget identity. Header, toolbar and
dock-tab controls register their response identity. `ui.focused` continues to identify the
focused panel; it is separate from keyboard widget focus. `windowFocused` reports the native
window focus received by egui, which can differ from the retained widget focus.

When a configuration store is available, current dock and floating geometry, focused and
maximized panels, and named workspace snapshots persist in `workspaces.json`. The current
layout and named snapshots are independent: changing a divider does not overwrite the named
snapshot until `window.saveWorkspace` runs. Changes save after pointer release, at most once
per second, with a final save on normal exit. Version 1 files are limited to 1 MiB and bounded
workspace trees; extra viewer IDs must be between 1 and 65,535. Native reads enforce the size
limit while reading and report I/O and UTF-8
errors. Unreadable, invalid or newer files leave the default workspace available and are
preserved for the entire session, including after later layout changes. Load and save errors
appear in the status message and the nullable
`workspaceStorageError` field; a failed write remains eligible for retry.
`window.saveWorkspace`, `window.saveWorkspaceAs` and `window.editWorkspaces` (rename, delete)
write the file immediately. Through `ui.menu.invoke` they return an error when it isn't written
(a failed write, a file preserved after a failed load, or no configuration store);
`engine.execute` hands them to the UI and returns `{frontend}` first, so check
`workspaceStorageError` there. The changed workspaces still apply for the session, and a later
save retries them.

## Input

Points are logical window coordinates. A target is `{id}` (the element's centre; add `fx`/`fy` between
0 and 1 to pick a spot inside it) or `{x, y}`. `modifiers` is `{shift, alt, command, ctrl}`, where
`command` is ⌘ on macOS and Ctrl elsewhere.

| Method | Params |
|---|---|
| `ui.click` | `{id \| x,y, button?: left\|right\|middle, count?, modifiers?}` |
| `ui.move` | `{id \| x,y}` |
| `ui.drag` | `{from: target, to: target, steps? (12), modifiers?}`: press, move, release |
| `ui.scroll` | `{id \| x,y, dx?, dy?, unit?: point\|line, source?: wheel\|trackpad, pinch?, modifiers?}`: `line` is a mouse wheel (zooms the viewers); `point` (default) is a pixel-precise scroll; `source: trackpad` pans the viewers (as on macOS); with no `source` the viewers take a scroll in points as a wheel until one moves on both axes (a trackpad: from then on it pans); `pinch` is a trackpad pinch's zoom factor |
| `ui.key` | `{key}`, for example `Space`, `F9`, `Cmd+Z`, `Cmd+Shift+Z`, `PageDown`, `Escape` |
| `ui.type` | `{text}`: text input into the focused field |

Shortcut presets also accept `Physical+<shortcut>` for a keyboard position, using egui's
US-position names (for example `Physical+6` and `Physical+Minus` distinguish AZERTY's
`-` and `)` keys). Physical bindings take precedence over logical bindings for that key
press. `ui.key` accepts the same notation to test them. Ordinary shortcuts remain layout-aware.
`time.workAreaStart` and `time.workAreaEnd` navigate to the first and last included work-area
frames and can be assigned through `shortcuts.set`.

## Locating things

| Method | Params | Result |
|---|---|---|
| `ui.viewer.locate` | `{x, y}` (comp pixels) | `{x, y}` window point |
| `ui.viewer.hit` | `{id \| x,y}` (window point) | `{x, y}` comp pixel |
| `ui.timeline.locate` | `{layer (id), time?}` | `{x, y}` window point on the layer bar |

## Screenshots

| Method | Params | Result |
|---|---|---|
| `ui.screenshot` | `{path?, panel?, id?}` | `{path, width, height}`: a PNG of the window, or cropped to one panel or element. The panel or element rectangle is resolved when the capture is issued (after the UI settles), so it follows a panel that moved; a panel or id that is not on screen is retried and then reported, never replaced by a whole-window shot. |

Element ids are stable, for example `tools.Selection`, `panel.Timeline`, `panel.tab.EffectControls`,
`panel.tab.Timeline.<comp>` (one Timeline tab per open comp; `panel.tab.Timeline` is the shown
one), `panel.tab.Viewer(<n>)` and `viewers.<n>` (View ▸ New Viewer's other Composition viewers;
`viewers.0` is the Composition panel while another viewer is active), `header.workspace.Animation`, `project.item.<id>` (a double-click opens the item, Enter
renames it), `viewer.comp`, `viewer.handle.<layer>.<i>`,
`timeline.layer.<id>.bar`, `timeline.layer.<id>.nestedMarker.<i>` (a precomp's comp markers), `timeline.layer.<id>.twirl`, `timeline.layer.<id>.row` (click selects,
Enter or a double-click renames, drag reorders), `timeline.prop.<uid>.stopwatch`,
`timeline.key.<uid>.<frame>` (and `timeline.keyAt.<uid>.<ticks>`, exact in layer-time ticks, for
keys on subframes), `timeline.cti`, `effectControls.prop.<uid>.value` (an angle's
revolutions are `timeline.prop.<uid>.revolutions`, `effectControls.prop.<uid>.revolutions` and
`properties.prop.<uid>.revolutions`) and `effects.item.<name>`. The Composition viewer adds `viewer.magnification`, `viewer.resolution`,
`viewer.roi`, `viewer.grid`, `viewer.channel`, `viewer.exposure`, `viewer.snapshot`,
`viewer.showSnapshot`, `viewer.fastPreviews` (their popup entries are `viewer.<menu>Item.<n>`),
`viewer.ruler.top|left|origin`, `viewer.mask.<uid>.vertex.<i>`, `viewer.shapePath.<uid>.vertex.<i>`,
`viewer.motionPath.<layer>.<key>[.in|.out]`, `viewer.puppetPin.<uid>[.rotate|.scale]`,
`viewer.freeTransform.handle.<i>` and
`viewer.regionOfInterest`; the Graph Editor adds `timeline.graph.transformBox[.<i>]`,
`timeline.graph.snap` and `timeline.graph.reference`. Use `ui.elements` to see what is on screen.

Viewer state that agents drive headless too: `view.snapping`, `view.snappingOptions
{edgesExtended?, collapsedFeatures?, toggle?}` (the Tools bar's Snapping options,
`header.snappingOptions`: After Effects' Snap Edges Extended and Snap to Features in Collapsed
Compositions and Text Layers), `view.channel {channel, colorized?}`,
`view.exposure {stops | delta}`, `view.resetExposure`, `view.takeSnapshot`, `view.showSnapshot`,
`view.fastPreviewMode {mode}` (one mode each, for shortcuts:
`view.fastPreviews.off|adaptive|draft|fastDraft|wireframe`, and for the channels
`view.channel.red|green|blue|alpha|redColorized|greenColorized|blueColorized|rgbStraight`), `view.res.full|half|third|quarter|auto {comp?}` and
`view.res.custom {factor, comp?}` (the comp's Resolution, saved with the project but no undo
step; `ui.set {viewer: {res}}` sets the active comp's, and `ui.inspect` reports it as
`ui.viewer.res`), `view.setRegionOfInterest {rect}`, `view.addGuide {percent?}`,
`view.addPercentGuide`, `view.convertGuides {to: percent|pixels, index?}`,
`view.moveGuide` (comp pixels, whatever the guide's unit), `view.removeGuide`, `effect.setLabel {layer?, effect, label}`; editing: `shape.newPath` (Pen on shape layers),
`shape.newShape` (the shape tools: a new group in the selected shape layer, else a new shape layer),
`shape.toolOptions` (the Tools bar's Tool Creates Shape / Mask and Fill and Stroke Options),
`mask.insertVertex`, `mask.convertVertex`, `mask.deleteVertices`, `path.freeTransform` (masks and
shape paths by uid), `keys.setSpatialTangents` (motion-path handles) and `keys.transform` (Graph
Editor transform box, timeline Alt-drag scaling).

The MCP server's bridge mode (`effectcraft-cli mcp --bridge 9877`) is a thin client of this protocol;
see [agents.md](agents.md).

Keyframe command times (`prop.set`, `prop.addKey`, `keys.set`, `keys.select`) default to layer
seconds. Set `timeBase: "comp"` for comp seconds; `keys.set.newTime` and every
`keys.select.keys[].time` use the same input base. Omitted `prop.set` or `prop.addKey` times use
the target comp's CTI. Supplied write times snap to its nearest comp frame before conversion
to stored layer time, including on stretched or reversed layers. Existing `keys.set` neighbor
constraints and roving adjustment can change the final stored time.

Explicit bases add `layerTime` and `compTime` to unsplit `prop.set` and `prop.addKey` replies,
alongside `value` or `count`. Separated Position `prop.set` keeps an array of per-axis replies.
Explicit `keys.select` replies contain `count` and `keys[]`, with timing on each key in the
resulting selection. Key metadata's `time` remains layer seconds regardless of the input base.
Without `timeBase`, these three commands retain their existing JSON value/count replies,
including outside the layer's `[in, out)` range. Timing metadata carries outside-range warnings;
`keys.set` and `prop.get.keys[]` report both bases and warnings without an explicit base too.
`prop.get.time` is comp seconds. For these commands, `path` or `prop` identifies a property, not a group.
Set a layer's offset with `layer.timing {layer, start}` (comp seconds).

Panels dock in craft-ui's shared docking (`craft_ui::docking`): split frames, tab groups,
stacked panels and floating windows. Every change, by pointer, menu or command, is one
transaction that checks the whole result; an invalid one returns an error and leaves the layout
unchanged. No move or float may leave the docked area empty; closing its last panel is allowed.
Panels name themselves by id (`"EffectControls"`), by title (`"Effect Controls"`) where a command
takes a name, or as `{"Viewer":42}` and `{"ScriptPanel":77}` for viewers and ScriptUI panels.

- `window.dockAction {action}` applies one craft-ui `Action` in its JSON form. For example,
  `{"action":{"ResizeSplit":{"path":[],"size":{"FixedFirst":360}}}}` resizes the root split
  (path entries are `false` for the first child and `true` for the second), and
  `{"action":{"MoveFloating":{"panel":"Info","rect":[45,65,420,320]}}}` moves and resizes the
  floating window holding Info. `Open` and `OpenAt` are refused: panels open through the panel
  commands (`window.panel`, `ui.panel.show`), which run the viewer and ScriptUI lifecycles.
  Closing runs them too. Pointer gestures dispatch through this command.
- craft-ui's commands: `window.panel.move`, `.float`, `.dock`, `.activate`, `.close`,
  `.closeGroup`, `.closeOthers`, `.minimize`, `.iconic`, `.window` and `.layout`, with the params
  of craft-ui's `DockCommand` (`docs/ui-docking.md` in craft-libs). `window.panel.<Panel>`
  (`window.panel.Align`) opens and shows that panel. `window.panel.activate` on a closed panel
  reopens it where it was.
- `ui.dock {operation, panel, …}` or `ui.dock {action}`: craft-ui's `ui.dock` operations
  (`open`, `close`, `activate`, `float {rect}`, `moveFloating {rect}`,
  `move {target, zone?, before?}`, `setStackOpen {open}`, `resizeStack {height}`,
  `resizeSplit {path, size}`; `rect` is `[x, y, width, height]`). It returns
  `{"docking": workspace}`. `ui.dock.reset` returns the current workspace to its saved or
  built-in layout and returns the same. Raw `Open` and `OpenAt` actions are refused in both
  `ui.dock {action}` and `window.panel.layout {action}`, as in `window.dockAction`; use the
  panel open commands instead.
- `window.dockPanel {panel, anchor?, zone?, before?}` docks as `window.panel.dock`: `zone` is
  `center`, `tab`, `left`, `right`, `top`, `bottom`, `above` or `below`; without an anchor a
  side zone docks along that edge of the whole workspace. `window.floatPanel {panel, rect?}`
  requires exactly four finite numbers when `rect` is given and otherwise floats a 420x320
  window in the middle of the app window. `window.closePanel {panel}` closes it.
- `window.panelWindow {panel, window?}` puts a panel in a floating window of its own with a
  desktop place over the panel's place in the main window, and returns `{"window": bool}`. A
  docked panel floats first. `window` false docks it again, as a tab of the first docked group;
  without `window` it toggles.

On desktop every floating window is an OS window of its own (craft-ui's `native-windows`), which
can go on another monitor; on Wayland, on the web and where egui embeds viewports, floating
windows stay in the main window. Keys pressed in a floating window that none of its widgets uses
run the main window's shortcuts, and files dropped on one import as on the main window. While a
modal dialog is open, floating windows take no input.

The automation ids of the docking chrome are `panel.tab.<Panel>`, `panel.drag.<Panel>` (a
group's drag bar), `panel.tabs.more.<Panel>`, `panel.menu.<Panel>` (≡), `panel.header.<Panel>`
(a stacked panel's open and close button), `panel.tab.<Panel>.lock` (viewer lock),
`panel.tab.Timeline.<comp>` and `panel.tab.Timeline.close` (the Timeline's comp tabs),
`panel.float.<n>.move`, `.close`, `.resize` and `.resize.<Edge>` for floating window `n`,
`dock.gutter.<path>` for dividers (`a`/`b` per split level; a stack's gap below entry `i` adds
`s<i>`) and `dock.corner.<outer>.<inner>` where two dividers meet.

Saved workspaces (`workspaces.json`, version 2) hold craft-ui `Workspace`s: the layout, the
closed panels with where each goes back, the lock, and each floating window's desktop place. A
version 1 file (EffectCraft 0.10 and earlier) and the older unversioned record of
named workspaces load with every panel in place: the current layout, the named workspaces,
floating windows and their native window places. A column of stacked frames becomes tab groups
split top to bottom: each open frame keeps all its tabs and its height (as a fixed size), and a
closed frame's tabs join the open frame above it. A native window's place moves from the OS
window's outer position to its content's, by the main window's title bar, before the first save.
Panels that were closed stay closed and reopen at their default place. The first save writes version 2 and keeps
the earlier file as `workspaces.old.json`. A damaged, unknown or newer file loads as the default
layout and is left unchanged.
