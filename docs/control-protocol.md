# Control protocol

The desktop app exposes a control channel for agents, tests and the MCP bridge:

```sh
effectcraft --control 9877        # or EFFECTCRAFT_CONTROL_PORT=9877
```

- **Transport:** TCP on `127.0.0.1` only, one JSON object per line in each direction. Connections are
  persistent; requests on one connection are answered in order.
- **Request:** `{"id": 1, "method": "engine.execute", "params": {...}}` (`id` is echoed back, `params`
  defaults to `{}`).
- **Reply:** `{"id": 1, "ok": true, "result": ...}` or `{"id": 1, "ok": false, "error": "message"}`.
- Requests run on the UI thread between frames. Input methods (`ui.click`, `ui.key` and similar)
  reply after the synthetic input has been processed. Methods that need an element that isn't drawn
  yet are retried for a few frames. A request times out after 60 s.

Quick test:

```sh
printf '{"id":1,"method":"engine.execute","params":{"command":"comp.new","params":{"name":"A"}}}\n' | nc 127.0.0.1 9877
```

## Engine

| Method | Params | Result |
|---|---|---|
| `engine.execute` | `{command, params?}` | The command's result. Every engine command (`effectcraft-cli commands`) runs exactly as it does headless, so it never opens a dialog: `comp.new {}` creates a default comp. UI-only ids (`tool.*`, `view.*`, `window.*`, `playback.*`, `timeline.*`, `app.*`) fall through to the menu dispatcher. Unknown top-level params are rejected, and the error lists the accepted keys. |
| `ui.menu.invoke` | `{id, params?}` | Like a menu click: commands with empty params may open their dialog (New Composition, Solid Settings, file pickers). |
| UI commands (via either) | | `timeline.column {column, visible?}` (av, keys, label, num, comment, switches, modes, parent, in, out, duration, stretch), `timeline.sourceName {value?}`, `timeline.search {query}`, `timeline.reveal.<kind>` / `timeline.revealAdd.<kind>` (the property shortcuts and Shift+shortcut: position, scale, rotation, opacity, anchor, effects, masks, feather, levels, animated, and the double-press sets maskPath, maskOpacity, timeRemap, expressions, material, paint, selected, missingEffects, waveform), `timeline.keyAt.<anchor\|position\|scale\|rotation\|opacity>` (Alt+Shift+A/P/S/R/T), `timeline.twirlSelected` (Ctrl/Cmd+`), `timeline.collapseAll`, `flowchart.options {layers?, effects?, solids?, direction?: lr\|tb, comp?}`, `flowchart.graph {comp?}` → `{nodes, edges}`, `window.workspace {name}` (all After Effects workspaces). |
| `engine.commands` | `{filter?, enabledOnly?}` | `[{id, label, menu, shortcut, params, enabled, why}]` |
| `ui.menu.list` | `{}` | The menu tree with enablement. |
| `render.frame` | `{comp?, time?, max_side?, path?, base64?}` | Renders a comp frame through the session, independent of the viewer zoom or resolution. `comp` is an id or name (default: the active comp), `time` is in comp seconds (default: the CTI), `max_side` caps the longest side (0 = full size). Writes a PNG to `path` (default: a temp file) and returns `{comp, time, width, height, path}`. With `base64: true` it returns `{…, png: "<base64>"}` and writes no file. |

Useful query commands for `engine.execute` (they aren't journaled or undoable): `project.summary`,
`comp.info {comp?}`, `layer.tree {layer, comp?, depth?, time?}` (every node has a `path`),
`prop.get {layer, path, comp?, time?}`, `editor.state`, `command.list {filter?, enabledOnly?, schemas?}`,
`command.describe {command}` (params doc and JSON Schema), `app.capabilities` (version, counts,
export formats, parity summary), `jobs.list` (Progress panel jobs, e.g. the background
`footage.check` after File ▸ Open).

Every engine command is reachable through `engine.execute`, and every interactive widget of every
panel registers an element id; both are enforced by `crates/ui-egui/tests/ui_agent_audit.rs`.

## UI state

| Method | Params | Result |
|---|---|---|
| `ui.inspect` | `{}` | Window size, pixels per point, fps, serialized `UiState`, playback, time, active comp, selection, open dialog, last render ms, viewer request-to-install timing. |
| `ui.elements` | `{prefix?}` | `[{id, label, rect: [x, y, w, h]}]`: the widgets drawn in the last frame, in points. |
| `ui.set` | `{tool?, workspace?, theme?, focused?, viewer?, timeline?, menuBar?, home?}` | `viewer: {zoom (null = fit), res: Full\|Half\|Third\|Quarter\|Auto, pan: [x,y], grid, rulers, safeMargins, transparencyGrid}`; `timeline: {pps (null = fit), start, graphEditor, showModes, openLayers: [ids], openGroups: [uids]}`. |
| `ui.panel.show` / `ui.panel.close` | `{panel}` | Panels: `Project`, `Composition`, `Timeline`, `EffectControls`, `EffectsPresets`, `Info`, `Preview`, `Character`, `Paragraph`, `Align` and others. |
| `ui.playback` | `{action: play\|stop\|toggle\|status}` | `{playing, time, audio, levelsDb, peaksDb}` (audio: an audio preview is running; L/R meter levels and held peaks in dBFS) |
| `ui.resize` | `{width, height}` | Resizes the window (points). |
| `ui.focus` | `{}` | Activates the window and takes keyboard focus. Agents normally avoid this, because the app is started without stealing focus. |
| `app.quit` | `{force?}` | Closes the app. A modified project shows the Save / Don't Save / Cancel prompt first (automation ids `dialog.unsaved.save`, `dialog.unsaved.dontSave`, `dialog.unsaved.cancel`) unless `force: true`. |

`ui.inspect.viewerTiming` has `pending` and the latest completed `last` sample (initially null).
The sample identifies `content`, `comp`, `frame`, `scale` (render scale × 1000), `view` and `opts`.
`totalMs` measures the first request for the desired frame through successful viewer texture
installation or reuse. Duplicate paints and revision-only changes do not restart it; superseded
frames cannot complete the current demand. This is not physical display presentation or input latency.

`queueMs`, `preparationMs` and `installationMs` divide that interval at job start and RAM availability.
Preparation includes disk lookup, conversion and, for remote workers, scheduling/message transfer;
it is not pure shader execution time. Work already completed by prefetch is excluded. A RAM hit
has `cached: true`, zero queue/preparation, and its remaining lookup/install wait in `installationMs`.
If it is evicted and rendered again before installation, `cached` becomes false. Phase values are
null when bounded job metadata is unavailable; total timing remains available. Purging clears timing.
The original `renderMs` readout retains its existing urgent-job timing semantics.

## Input

Points are logical window coordinates. A target is `{id}` (the element's centre; add `fx`/`fy` between
0 and 1 to pick a spot inside it) or `{x, y}`. `modifiers` is `{shift, alt, command, ctrl}`, where
`command` is ⌘ on macOS and Ctrl elsewhere.

| Method | Params |
|---|---|
| `ui.click` | `{id \| x,y, button?: left\|right\|middle, count?, modifiers?}` |
| `ui.move` | `{id \| x,y}` |
| `ui.drag` | `{from: target, to: target, steps? (12), modifiers?}`: press, move, release |
| `ui.scroll` | `{id \| x,y, dx, dy}` |
| `ui.key` | `{key}`, for example `Space`, `F9`, `Cmd+Z`, `Cmd+Shift+Z`, `PageDown`, `Escape` |
| `ui.type` | `{text}`: text input into the focused field |

## Locating things

| Method | Params | Result |
|---|---|---|
| `ui.viewer.locate` | `{x, y}` (comp pixels) | `{x, y}` window point |
| `ui.viewer.hit` | `{id \| x,y}` (window point) | `{x, y}` comp pixel |
| `ui.timeline.locate` | `{layer (id), time?}` | `{x, y}` window point on the layer bar |

## Screenshots

| Method | Params | Result |
|---|---|---|
| `ui.screenshot` | `{path?, panel?, id?}` | `{path, width, height}`: a PNG of the window, or cropped to one panel or element. |

Element ids are stable, for example `tools.Selection`, `panel.Timeline`, `panel.tab.EffectControls`,
`panel.tab.Timeline.<comp>` (one Timeline tab per open comp; `panel.tab.Timeline` is the shown
one), `panel.tab.Viewer(<n>)` and `viewers.<n>` (View ▸ New Viewer's other Composition viewers;
`viewers.0` is the Composition panel while another viewer is active), `header.workspace.Animation`, `project.item.<id>` (a double-click opens the item, Enter
renames it), `viewer.comp`, `viewer.handle.<layer>.<i>`,
`timeline.layer.<id>.bar`, `timeline.layer.<id>.nestedMarker.<i>` (a precomp's comp markers), `timeline.layer.<id>.twirl`, `timeline.layer.<id>.row` (click selects,
Enter or a double-click renames, drag reorders), `timeline.prop.<uid>.stopwatch`,
`timeline.key.<uid>.<frame>`, `timeline.cti`, `effectControls.prop.<uid>.value` and
`effects.item.<name>`. The Composition viewer adds `viewer.magnification`, `viewer.resolution`,
`viewer.roi`, `viewer.grid`, `viewer.channel`, `viewer.exposure`, `viewer.snapshot`,
`viewer.showSnapshot`, `viewer.fastPreviews` (their popup entries are `viewer.<menu>Item.<n>`),
`viewer.ruler.top|left|origin`, `viewer.mask.<uid>.vertex.<i>`, `viewer.shapePath.<uid>.vertex.<i>`,
`viewer.motionPath.<layer>.<key>[.in|.out]`, `viewer.puppetPin.<uid>[.rotate|.scale]`,
`viewer.freeTransform.handle.<i>` and
`viewer.regionOfInterest`; the Graph Editor adds `timeline.graph.transformBox[.<i>]`,
`timeline.graph.snap` and `timeline.graph.reference`. Use `ui.elements` to see what is on screen.

Viewer state that agents drive headless too: `view.snapping`, `view.channel {channel, colorized?}`,
`view.exposure {stops | delta}`, `view.resetExposure`, `view.takeSnapshot`, `view.showSnapshot`,
`view.fastPreviewMode {mode}`, `view.setRegionOfInterest {rect}`, `view.addGuide`,
`view.moveGuide`, `view.removeGuide`; editing: `shape.newPath` (Pen on shape layers),
`mask.insertVertex`, `mask.convertVertex`, `mask.deleteVertices`, `path.freeTransform` (masks and
shape paths by uid), `keys.setSpatialTangents` (motion-path handles) and `keys.transform` (Graph
Editor transform box, timeline Alt-drag scaling).

The MCP server's bridge mode (`effectcraft-cli mcp --bridge 9877`) is a thin client of this protocol;
see [agents.md](agents.md).
