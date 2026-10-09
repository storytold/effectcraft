# Driving EffectCraft from agents

Everything in EffectCraft is an engine command with a stable id and JSON params. Menus, shortcuts,
panel gestures, the CLI, the control channel and MCP all go through the same commands, so anything a
user can do, an agent can do too. There are three ways in:

| Interface | Best for | Needs a window |
|---|---|---|
| **MCP** (`effectcraft-cli mcp`) | Claude Code and other MCP clients | no (headless), or yes with `--bridge` |
| **CLI** (`effectcraft-cli exec/get/set/...`) | one-shot scripting and CI; JSON output with `--json` | no |
| **Control channel** (`effectcraft --control 9877`) | driving and seeing the live UI; see [control-protocol.md](control-protocol.md) | yes |

## MCP setup

Build once with `cargo build --release -p effectcraft-cli`, then register the server. For Claude
Code, add a `.mcp.json` at the project root:

```json
{
  "mcpServers": {
    "effectcraft": {
      "command": "/path/to/effectcraft/target/release/effectcraft-cli",
      "args": ["mcp"]
    }
  }
}
```

This repository ships a ready-made [`.mcp.json`](../.mcp.json): `effectcraft` (headless, demo
project loaded) and `effectcraft-app` (bridged to a desktop app started with `--control 9877`). Both
run through `cargo run --release`, so the first start compiles; run
`cargo build --release -p effectcraft-cli` once beforehand to avoid an MCP startup timeout.

You can also register it from the command line:
`claude mcp add effectcraft -- /path/to/target/release/effectcraft-cli mcp`. Other clients (Claude
Desktop, Cursor and the like) take the same `command` and `args`.

- **Headless** (`["mcp"]`): an in-process session with no window. Add `"--demo"` or
  `"--project", "file.ecproj"` to start with content. Startup is instant. It renders on the CPU
  unless you add `"--gpu"` (`["mcp", "--gpu"]`), which attaches the GPU compositor on a device of
  its own; if no adapter is usable the server says why and exits. `get_project`'s `renderer`
  (`render.backend`) reports `active` (`gpu` / `cpu`) and, on the CPU, `why`.
- **Bridge** (`["mcp", "--bridge", "9877"]`): drives a running `effectcraft --control 9877`, so you
  see every change live. Bridge mode adds `screenshot` and the `ui_*` tools.

The server speaks JSON-RPC 2.0 over stdio, one message per line, and supports MCP protocol versions
2025-06-18, 2025-03-26 and 2024-11-05 (`initialize`, `ping`, `tools/list`, `tools/call`).

### Keeping headless work across restarts

Start `effectcraft-cli mcp --autosave` (MCP args `["mcp", "--autosave"]`) to preserve
unsaved headless work. Each tool call that changes a dirty project writes a checkpoint before
its reply is sent, so even killing the server after that reply leaves the work on disk. EOF
and transport errors also flush outstanding changes. Queries do not rotate the saved versions.
A failed checkpoint adds a warning to the tool reply; the edit remains in memory and the next
call retries. Use `save_project {path}` to a writable location before disconnecting if warned.

Each server has its own folder under `<config directory>/EffectCraft Auto-Save/MCP/` (or
`<custom Auto-Save folder>/MCP/`). The maximum-version setting applies within that folder.
`--autosave` enables these checkpoints regardless of the desktop's Auto-Save toggle and interval;
it reads settings without writing desktop preferences, shortcuts or crash-recovery state.
`EFFECTCRAFT_CONFIG_DIR` overrides the platform config directory. Session folders are retained
for recovery; remove old folders once their work is saved elsewhere.

`initialize` instructions and `_meta.effectcraftAutoSave` list the current folder and up to five
previous dirty sessions; `get_project.autosave` also reports the latest checkpoint and errors.
Recover explicitly with `open_project {"path":"<checkpoint .ecproj>"}`, then
`save_project {"path":"Recovered.ecproj"}`. Prior sessions may still be running and are never
opened automatically. Replacing a dirty project with `open_project` or `file.newProject` discards
it intentionally; save it first. The metadata describes the current project in each session.

Without `--autosave`, headless work is in memory until explicitly saved. The flag is headless
only; a bridged desktop app owns its auto-saves. An edit interrupted before its reply is received
may be absent from the last checkpoint.

### Tools

| Tool | What it does |
|---|---|
| `list_commands {filter?, enabled_only?, schemas?}` | Discover command ids, their param docs (and, with `schemas`, a JSON Schema each), and whether each can run now. |
| `describe_command {command}` | One command in full: label, menu, shortcut, params doc, JSON `schema`, enabled / `why`. |
| `execute_command {command, params?}` | Run any command (undoable). Unknown parameter keys are rejected with the accepted list. |
| `get_state` | Editor state (active comp, time, selections, tool) plus `app`: version, command and effect counts, export formats, parity summary. |
| `run_script {code, name?}` | Run JavaScript with the After Effects-style scripting object model (`app.project`, `comp.layers.addText(…)`, `layer.property("ADBE Transform Group").property("ADBE Position").setValueAtTime(…)`…). Returns `{ok, result, output, error: {message, line, column}}`; edits are undoable. |
| `get_project` / `get_comp {comp?}` | Project items, comp settings and layers. |
| `get_layer {layer, comp?, time?, depth?, flat?}` | A layer's property tree (`depth` limits how many group levels expand). Every node has a `path`. |
| `get_property {layer, path, comp?, time?}` | Value at a time, keyframes and expression. |
| `set_property {layer, path, value?, time?, expression?, comp?}` | Sets a static value. With `time` it sets a keyframe; with `expression` it sets an expression. |
| `add_keyframe {layer, path, time+value \| keys:[...], interpolation?, comp?}` | Adds keys, then optionally applies linear/bezier/hold/easyEase. |
| `list_effects {filter?}` | Effect ids, names, categories, GPU / 32-bpc support and parameters. |
| `list_fonts {query?, rescan?}` | Font families text layers can use, bundled and installed, with their styles, origin and own-language name; `rescan` picks up fonts installed since launch. |
| `add_effect {layer, effect, values?, comp?}` | Apply an effect and set its parameters in one call and one undo step (a failing value leaves nothing applied); returns the instance path (`effects/#n`) and its parameter paths. |
| `render_frame {comp?, time?, max_side?, path?, inline?, transparent?}` | Returns a PNG image of a frame; `transparent: true` keeps the alpha (as an RGB + Alpha render writes it) instead of compositing over the comp background. |
| `open_project {path \| demo \| new}` / `save_project {path?}` | Open and save files. |
| `undo {steps?}` / `redo {steps?}` | History. |
| `history {goto?}` | The branching undo history (History panel): every state with its index, id, label, branch depth and whether it's current; `goto` (an index or id) jumps to any state, on any branch, without losing the others. |
| `script_ui {action?, window?, widget?, value?, result?}` | Drive ScriptUI windows that scripts opened: `list` (default) the open windows, `get` a window's control tree, `click` a button / checkbox / radio button, `set` text, a slider or a list item, `close` a window (`result`: what a dialog's `show()` returns, default 2 = Cancel). |
| `batch {steps: [{command, params}], label?, atomic?}` | Several commands in one call and one undo step (`engine.batch`); `"$N.key"` in a param is step N's result (`"$1.layer"`). A failing step rolls the batch back and names the step. |
| *(bridge)* `screenshot {panel?, id?, max_side?, path?}`, `ui_inspect`, `ui_elements {prefix?}`, `ui_click`, `ui_drag`, `ui_key`, `ui_type`, `ui_set`, `control {method, params}` | Look at and operate the live window. |

Layers are referenced by id (from `get_comp`), `"#n"` (1-based index from the top) or name. Comps are
referenced by id or name, and default to the active comp. Property paths come from `get_layer`, for
example `transform/position`, `transform/opacity`, `effects/#1/blurriness` or `@57` (by uid). Times
are in seconds. Keyframe times are layer time, which equals comp time unless the layer is offset or
stretched.

Command params are checked against each command's `params` doc. An unknown key, such as
`layer.select {"index": 2}`, returns an error that lists the accepted keys (here `layers, add, toggle`)
instead of being ignored.

### A typical session

1. `execute_command {"command":"comp.new","params":{"name":"Intro","width":1920,"height":1080,"frameRate":30,"duration":4}}`
2. `execute_command {"command":"layer.newText","params":{"text":"Hello","size":160,"fill":"#ffffff"}}` returns `{"layer": 2}`.
3. `add_keyframe {"layer":2,"path":"transform/position","keys":[{"time":0,"value":[-300,540]},{"time":1.5,"value":[960,540]}],"interpolation":"easyEase"}`
4. `execute_command {"command":"effect.apply","params":{"layer":2,"effect":"Gaussian Blur"}}`, then `set_property {"layer":2,"path":"effects/#1/blurriness","value":8}`
5. `render_frame {"time":1.0}` lets you look at the result, and `save_project {"path":"intro.ecproj"}` saves it.

### Cookbook

Short recipes; every step is one tool call.

| Goal | Calls |
|---|---|
| Orient yourself | `get_state` → `get_project` → `get_comp {}` |
| Find the right command | `list_commands {"filter":"mask"}` → `describe_command {"command":"mask.new"}` (params + JSON Schema) |
| Title card | `execute_command comp.new {...}` → `execute_command layer.newText {"text":"Hi","size":120}` → `render_frame {"time":0}` |
| Animate | `add_keyframe {"layer":2,"path":"transform/scale","keys":[{"time":0,"value":[0,0]},{"time":0.5,"value":[100,100]}],"interpolation":"easyEase"}` |
| Effect with settings | `list_effects {"filter":"glow"}` → `add_effect {"layer":2,"effect":"Glow","values":{"threshold":40,"radius":30}}` → `set_property {"layer":2,"path":"effects/#1/intensity","value":2}` |
| Key a green screen | `add_effect {"layer":2,"effect":"Keylight (1.2)"}` → `execute_command effect.pickColor {"layer":2,"effect":1,"param":"screenColour","x":20,"y":20,"average":true}` (a screen pixel, layer space; samples the effect's input) → `set_property {"layer":2,"path":"effects/#1/screenMatte/clipBlack","value":10}` → `render_frame {"transparent":true}` |
| Drive with an expression | `set_property {"layer":2,"path":"transform/rotation","expression":"time*90"}` |
| Inspect a layer | `get_layer {"layer":2,"flat":true}` (every property with its `path`, value, key count) |
| Check the result | `render_frame {"time":1.5,"max_side":640}`; in bridge mode `screenshot {"panel":"Timeline"}` |
| Undo a mistake | `undo {}`, or `history {}` then `history {"goto": n}` |
| Missing media after opening | `execute_command footage.check {"wait":true}` → `{checked, missing, probed}`; then `file.findMissing {"what":"footage"}` |
| Save | `save_project {"path":"out.ecproj"}` |

The desktop app checks footage in the background after every open (a "Checking footage" job in
the Progress panel, `jobs.list`); headless sessions run `footage.check` when they want it.

### Pitfalls

Things that tripped up a long agent-driven session (a multi-scene 3D piece built entirely over MCP).

- **Layer time vs comp time.** `add_keyframe`, `set_property` with `time`, `prop.addKey`,
  `keys.select` and `keys.set` take layer time; `get_property` and `render_frame` take comp time;
  `run_script`'s `setValueAtTime` and `keyTime` use comp time, as in After Effects. On a layer that
  doesn't start at 0, such as a nested comp placed later, subtract the layer's start time from the
  comp time before keying (unstretched layers), or set the keys from `run_script`
  ([#257](https://github.com/storytold/effectcraft/issues/257)).
- **Moving a layer in time.** `execute_command layer.timing {"layers":["Scene 2"],"start":5.1}` sets
  the start time in comp seconds and moves the in and out points with it.
- **Loops in scripts.** `run_script` can call any engine command with `app.run(id, params)`, for
  example `app.run("layer.new3dPrimitive", {kind: "cube", position: [x, y, z]})` inside a loop. A
  script that throws keeps the edits it already made: wrap it in `app.beginUndoGroup()` /
  `app.endUndoGroup()` so one `undo {}` removes them all. (`batch` rolls back on its own.)
- **Save early in headless mode.** Without `--autosave` the headless server keeps the project in
  memory only, so a server restart loses unsaved work: start it with `mcp --autosave` (see above)
  or call `save_project {"path": …}` after each milestone.
- **Environment light.** `layer.newLight` also accepts `"kind":"Environment"`: an equirectangular
  image that metallic surfaces reflect. Set its `lightOptions/source` to a footage, comp or solid
  layer by layer id; with no source it uses the comp's Environment Layer (`layer.environment`)
  ([#261](https://github.com/storytold/effectcraft/issues/261)).

### Cookbook (from end-to-end QA)

The scenarios in `crates/automation/tests/qa/` and `apps/effectcraft-cli/tests/qa_template.rs`
build real projects through these interfaces; they are worked examples of everything below.

* **Name things when you create them**: `layer.newText`, `layer.newShape`, `layer.newSolid`,
  `layer.newNull`, `layer.newCamera` and `layer.newLight` take `name`, so later calls can say
  `"layer": "Title"`. A reference that matches no layer is an error, never a silent no-op.
* **Nested comps**: `comp.settings {preserveFrameRate, preserveResolution}` (Composition Settings ▸
  Advanced); `comp.info` / `get_comp` report both. A precomp layer shows nothing outside its
  nested comp's span.
* **Several footage items at once**: `file.newCompFromSelection {single, dimensionsFrom, duration,
  sequence, overlap, overlapDuration, transition, addToRenderQueue}` is one undo step (select the
  items first: `project.select`).
* **Locked layers** are left alone: `edit.clear`, `layer.arrange`, `layer.transform` and
  `layer.centerAnchor` skip them and fail when every target is locked ("… is locked"); Select
  All and `layer.selectNext` / `selectPrevious` step over them. Unlock with
  `layer.setSwitch {"layers": [...], "switch": "lock", "value": false}` or `layer.unlockAll`.
* **Solid Settings**: `layer.settings {layer, name?, color?, width?, height?, pixelAspect?,
  affectAll?}` edits a solid or adjustment layer's solid (and renames the layer and the solid).
  A solid shared by several layers (duplicates share theirs) changes for all of them unless
  `affectAll: false`, which gives this layer its own copy; the reply names it (`{layer, solid}`).
  Cameras and lights take their own settings through the same command.
* **Use the paths replies give you**: `effect.apply` returns `{effects: [uid], paths:
  ["effects/#2"]}`; `layer.addShapeItem {kind, group?: uid|path}` returns `{uid, path}` (e.g.
  `contents/group/contents/gfill`), ready for `set_property`. `@uid` works as a whole path.
* **One call, one undo step**: `batch {"steps":[{"command":"layer.newShape","params":{"kind":"ellipse","name":"Ring"}},{"command":"layer.addShapeItem","params":{"layer":"$1.layer","kind":"trim"}},{"command":"prop.addKey","params":{"layer":"$1.layer","path":"transform/opacity","time":0,"value":0}}]}`
  (a string param that is exactly `"$N"` or `"$N.key"` is replaced; `$2.path` is the trim's path).
* **Expressions**: `set_property`/`get_property` replies carry `evaluated` (what renders) next to
  `value` (the keyframed value) and `expressionError` when the expression fails. An expression with
  a syntax error is kept but disabled, so it renders `value` and reports the syntax error.
* **Keyframes like a person**: `keys.select {keys, toggle: true}` (Shift+click), `keys.selectEqual`
  / `selectPrevious` / `selectFollowing`, `keys.move {delta, merge}` (steps sharing a merge key are
  one drag: each applies to the keys as they were before it, so a key passed over survives) and
  `keys.transform {…, merge, fromStart: true}` (values are the whole transform since the drag
  started). Keys of locked layers can't be selected. Copy / paste: `keys.copy`, then `keys.paste`
  at the current time (keys copied from several layers go to as many selected layers in order).
  `time.nextKey` / `time.previousKey` / `keys.selectAll` take `visible: [{layer, prop}]` to act
  only on those properties (the Timeline passes its revealed ones, as J / K and Ctrl+Alt+A do).
* **Easing with Keyframe Velocity**: select keys by path, `keys.select {"keys":[{"layer":"Ring","path":"contents/trim/end","time":0}]}`,
  then `keys.velocity {"outSpeed":0,"outInfluence":33.33}`.
* **Gradients**: `set_property {"path":"contents/gfill/colors","value":["#0080ff","#ffff00"]}` or
  `{"colors":[[0,"#0080ff"],[1,[1,1,0]]],"opacities":[[0,1],[1,0.5]]}`.
* **Image sequences**: `file.import {"paths":["/r/shot_0001.png"]}` imports the file's whole
  numbered run as one item (several picked frames: that range; a folder: each run in it;
  `"sequence": false`: one still; `"alphabetical": true`: Force Alphabetical Order, the files
  play one after another) at `frameRate` or 30 fps (Settings ▸ Import). Missing frames show
  colour bars, as in After Effects. `file.interpretFootage {"items":[id], "frameRate":12,
  "startFrame":1001}` conforms them (the item and layers that ran to its end get the new
  length). See [footage.md](footage.md).
* **Multi-layer OpenEXR**: `layer.channels {layer}` lists a footage layer's EXR layers and
  channels (`{"layers":[{"name":"diffuse","rgba":["diffuse.R","diffuse.G","diffuse.B",""]}]}`);
  EXtractoR shows them when its `red` / `green` / `blue` / `alpha` are set to those names
  (`prop.set {"path":"effects/#1/red","value":"depth.Z"}`). Effect Controls offers them as popups.
* **Masks**: `layer.addMask {layer, shape: rect|ellipse, rect}` first, then `layer.mask.set
  {field: feather|expansion|opacity, value}`.
* **Renders**: `renderQueue.add {comp, format: h264|prores|webm|png…, channels: rgba,
  proresProfile: 4444, output}` then `renderQueue.render {"wait": true}`. Check a render by
  importing it (`file.import`, `file.newCompFromSelection`) and `render_frame {transparent: true}`
  for its alpha (ProRes 4444 and WebM VP9 alpha both read back as straight alpha).
* **Essential Graphics**: controls can be addressed by name (`essential.set {"layer":"#1",
  "control":"Title","value":"John Smith"}`); a command that targets an explicit `comp` runs even
  when the active comp would disable it (`essential.exportTemplate {"comp":"Lower Third", …}`).
* **History**: Levels of Undo defaults to 32 (`prefs.set {"key":"general.undoLevels","value":99}`);
  `history` lists branches and `history {"goto": id}` jumps between them. A command that changes
  nothing (a value set to what it already is) records no undo step. `get_state` reports `dirty`
  (unsaved changes); it follows undo, so undoing back to the saved state clears it.
* **Unsaved changes**: commands run through MCP / `engine.execute` never ask to save. In the
  desktop app, `ui.menu.invoke` of a command that closes a modified project (`file.open`,
  `file.newProject`, `file.closeProject`, `file.revert`…) and the window's close button show the
  Save / Don't Save / Cancel prompt (`ui_click` on `dialog.unsaved.save` / `dontSave` / `cancel`;
  `dialog.unsaved.revert` for Revert); the control channel's `app.quit {"force": true}` skips it.

### Motion tracking

Trackers live on the tracked layer (`Motion Trackers ▸ Tracker n ▸ Track Point n`) and are driven
with `track.*` commands. `track.analyze` runs in the background in the app (poll `track.status`);
pass `"wait": true` to block until it finishes, which is what the CLI and headless MCP want.

1. `execute_command {"command":"track.motion","params":{"layer":"clip.mov"}}` creates a Transform
   tracker (Motion Target: the layer above). Use `track.stabilize` for Stabilize, or `track.new` with
   `kind` `transform|stabilize|affine|perspective|raw` and `rotation`/`scale`.
2. `execute_command {"command":"track.setPoint","params":{"point":1,"center":[812,440],"featureSize":[40,40],"searchSize":[96,96]}}`
3. `execute_command {"command":"track.analyze","params":{"direction":"forward","wait":true}}` keys Feature
   Center, Confidence and Attach Point on every frame from the current time to the layer's end.
4. `execute_command {"command":"track.apply","params":{"dimensions":"xy"}}` keys the target's Position
   (and Rotation/Scale), the layer's own Anchor Point and Position for Stabilize, or a Corner Pin
   effect for `affine`/`perspective` tracks. `track.options` sets the channel, blur/enhance,
   adapt-feature and "If Confidence is Below" behaviour; `track.status` reports the track points.

```sh
effectcraft-cli run clip.ecproj track.motion '{"layer":"#2"}' \
  track.setPoint '{"point":1,"center":[812,440]}' \
  track.analyze '{"wait":true}' track.apply '{}' --save
```

### Mask tracking and Mask Interpolation

`track.mask` follows the pixels inside a mask and keys its Mask Path on every frame (vertices and
tangents move with the fitted motion). `method` is `position`, `positionScale`,
`positionScaleRotation` (the default, also kept as the Tracker panel's Method), `positionScaleRotationSkew`,
`perspective`, `faceOutline` (Face Tracking (Outline Only): the mask, drawn around a face, becomes
the face outline on every frame) or `faceDetailed` (Face Tracking (Detailed Features): also keys a
Face Track Points effect with one point per landmark — eyebrows, eyes, pupils, nose tip,
nostrils, mouth corners and lips, chin, jaw); `direction` is `forward|backward|frameForward|frameBackward` from the current
time (or `start` / `end` in seconds). Like `track.analyze` it runs in the background in the app
(progress in `track.status` under `mask`, `track.stop` cancels and keeps what was tracked) and is
one undo step; `"wait": true` blocks. `track.extractFaceMeasurements {layer?}` (Extract & Copy Face
Measurements) keys a Face Measurements effect from the Face Track Points (head position, scale and
orientation X/Y/Z, eye openness, eyebrow distance from eye, mouth openness, width and offset), puts
those keys on the keyframe clipboard and returns them per frame plus a tab-separated `clipboard`
text.

Face tracking's trained model works like Roto Brush's (below): `face.models` lists the built-in
tracker and MediaPipe Face Landmarker with its authors and licence; `face.model.download {id}` /
`face.model.install {path, id?}` install it (verified); `face.model.select {id:
classical|mediapipe-face}` chooses it; `face.model.remove {id}` deletes it. Downloads and loading
run in the background; `"wait": true` on download, install or select returns once the model is in
use (or with its error), which scripts need before tracking. `track.mask` with a
face method returns `faceModel`, the engine it tries first (the classical one when the model finds
no face in the mask).

```sh
effectcraft-cli run clip.ecproj face.model.download '{"id":"mediapipe-face","wait":true}' face.model.select '{"id":"mediapipe-face","wait":true}' \
  track.mask '{"layer":"#1","mask":1,"method":"faceDetailed","direction":"forward","wait":true}' --save
```

`mask.interpolate` (Window ▸ Mask Interpolation ▸ Apply) adds in-between Mask Path keys between
each pair of selected Mask Path keys (or the existing keys at `times`, in seconds), giving both
ends the same vertex count with a matched correspondence. Options: `keyframeRate` (number or
`"auto"` = the comp rate), `keyframeFields`, `linearVertexPaths`, `bendingResistance`, `quality`,
`addVertices` (number or `false`) with `addVerticesUnit` `pixels|total|percent`, `matchingMethod`
`auto|curve|polyline`, `oneToOne`, `firstVerticesMatch`. `mask.interpolationOptions` sets the
panel's defaults.

```sh
effectcraft-cli exec track.mask '{"layer":"#2","mask":1,"method":"perspective","wait":true}' clip.ecproj --save
effectcraft-cli exec mask.interpolate '{"layer":"#2","mask":"Mask 1","times":[0,2],"addVertices":10}' clip.ecproj --save --json
```

### Warp Stabilizer

Effect ▸ Distort ▸ Warp Stabilizer (`effect.apply {"effect":"Warp Stabilizer"}`), Animation ▸ Warp
Stabilizer VFX and the Tracker panel's button (`track.warpStabilizer`, which applies the effect and
starts analysing) stabilize a layer. `warp.analyze {layer?, effect?, wait?}` analyses every frame
between the layer's In and Out points in the background ("Analyzing in background (step 1 of 2)",
then "Stabilizing..."; `warp.status` reports `progress`, `banner`, whether the effect is
`analyzed`, and the current frame's `warp` matrix, `autoScale` and `crop`); `warp.cancel` stops it
without writing anything. The analysis is stored in the effect (saved with the project) and is
one undo step; trimming, slipping, time-remapping or replacing the layer's source clears it and
the app re-analyses automatically (headless: run `warp.analyze` again). Settings are ordinary
properties under the instance's groups, e.g. `effects/#1/stabilization/result` (0 Smooth Motion,
1 No Motion), `effects/#1/stabilization/smoothness`, `effects/#1/stabilization/method`,
`effects/#1/borders/framing` (0 Stabilize Only … 3 Stabilize, Synthesize Edges),
`effects/#1/borders/autoScale/maximumScale`, `effects/#1/advanced/showTrackPoints`. Method 3
(Subspace Warp, the default) bends a mesh per frame to the smoothed feature trajectories, so
scenes with depth stabilise where one perspective transform cannot;
`effects/#1/advanced/rollingShutterRipple` (0 Automatic Reduction, 1 Enhanced Reduction: a finer,
softer mesh) tunes it.

```sh
effectcraft-cli run shaky.ecproj track.warpStabilizer '{"layer":"#1","wait":true}' \
  prop.set '{"layer":"#1","path":"effects/#1/stabilization/result","value":1}' --save
effectcraft-cli exec warp.analyze '{"layer":"#1","wait":true}' shaky.ecproj --save --json
effectcraft-cli exec warp.status '{"layer":"#1"}' shaky.ecproj --json
```

### 3D Camera Tracker

Animation ▸ Track Camera and the Tracker panel's button (`track.camera {layer?, shotType?:
fixed|variable|specify, aov?, solveMethod?: auto|typical|flat|tripod, detailed?, lensDistortion?,
undistort?, wait?}`) apply
Effect ▸ Perspective ▸ 3D Camera Tracker (or reuse the layer's) and analyse it in the background:
"Analyzing in background (step 1 of 2)" tracks features, "Solving camera" solves.
`camera.analyze {layer?, effect?, wait?}` re-runs it, `camera.cancel` stops it without writing
anything. `camera.solveStatus` reports `progress`/`banner`, `analyzed`, `solved`, `methodUsed`,
`averageError` (pixels), `focalLength`, `horizontalAngleOfView`, the current frame's `camera`
(position, orientation, zoom), `groundPlane` and the solved `lensDistortion` (`k1`, `k2`, normalised by
half the frame diagonal) when Solve Lens Distortion (`effects/#1/advanced/lensDistortion`, also on
with Detailed Analysis) is on; `effects/#1/advanced/undistort` renders the footage undistorted so
the solved camera's 3D layers line up. `camera.points {time?}` lists the solved points
visible now with their `id`, `comp` position, `depth`, `world` position and `error`.

Select points with `camera.selectPoints {points, add?, toggle?}` (the viewer's click / Shift-click /
marquee), then the right-click menu's commands: `camera.setGroundPlane {points?}` (Set Ground Plane
and Origin), `camera.createFromSolve {kind: text|solid|null|shadowCatcher, points? | target:
{center, normal, size?}, multiple?}` (Create Text / Solid / Null / Shadow Catcher, Camera and Light,
Create Multiple …), `camera.deletePoints {points?, wait?}` (re-solves; with Auto-delete Points
Across Time the same feature's other tracks go too) and `camera.create` (the Create Camera button).
Each is one undo step; the first create adds the one-node "3D Tracker Camera" keyed on every frame.
Changing the layer's frames clears the analysis; changing `effects/#1/shotType`,
`effects/#1/horizontalAngleOfView`, `effects/#1/advanced/solveMethod` re-solves the stored tracks.

```sh
effectcraft-cli run shot.ecproj track.camera '{"layer":"#1","wait":true}' \
  camera.points '{}' --json
effectcraft-cli run shot.ecproj camera.createFromSolve '{"kind":"solid","points":[12,40,77]}' --save
```

### Roto Brush & Refine Edge

The Roto Brush tool (Alt+W cycles Roto Brush / Refine Edge) paints in the Layer panel; agents use
`roto.stroke {layer, kind: fg|bg|refine|refineErase, points: [[x, y], …], frame?, radius?}`
(layer pixels, layer frames). The first foreground stroke applies the Roto Brush & Refine Edge
effect and sets the base frame with a span of 20 frames each side. `roto.propagate {layer,
direction?: forward|backward|both, to?, wait?}` segments the span (in the background unless
`wait`); strokes on any other frame correct it and propagation restarts from there.
`roto.span {start?, end?}`, `roto.freeze {wait?}` / `roto.unfreeze`, `roto.clearStrokes {frame?,
kind?}`, `roto.cancel` and `roto.options {diameter?, refineDiameter?, view?: alphaBoundary|alpha|
alphaOverlay|none}` complete the set; every edit is one undo step. `roto.status {layer, frame?,
matte?, compute?, compareTo?}` reports the base frame, span, computed and stroked frames, frozen
state, job progress and, for a frame, the matte's area, centroid, RLE matte and IoU against a
reference. Matte settings are ordinary properties (`effects/#1/rotoBrushMatte/searchRadius`,
`effects/#1/refineEdgeMatte/decontaminateEdgeColors`, …).

Trained model (Roto Brush 2.0 / 3.0):
- `roto.models` lists the built-in engine and every registered model, with its authors,
  licence, size, URL, SHA-256 and whether it is installed, selected and active.
- `roto.model.download {id}` fetches and verifies the official weights in the background;
  `roto.model.install {path, id?}` installs a file you already have.
- `roto.model.select {id: classical|mobilesam}` chooses which one Roto Brush uses. Loading runs
  in the background; `"wait": true` (also on download and install) returns once it is in use.
- `roto.model.remove {id}` deletes the installed weights.
- The `version` property (`effects/#1/version`: 0 = 1.0, 1 = 2.0, 2 = 3.0) picks between the
  classic engine (1.0) and the chosen model (2.0, 3.0).

```sh
effectcraft-cli run clip.ecproj roto.model.install '{"path":"mobile_sam.pt"}' roto.model.select '{"id":"mobilesam","wait":true}' \
  roto.stroke '{"layer":"#1","points":[[300,200],[360,230]],"radius":10}' roto.propagate '{"layer":"#1","wait":true}' --save
```

```sh
effectcraft-cli run clip.ecproj roto.stroke '{"layer":"#1","points":[[300,200],[360,230]],"radius":10}' \
  roto.stroke '{"layer":"#1","kind":"bg","points":[[40,40],[600,40]],"radius":12}' \
  roto.propagate '{"layer":"#1","wait":true}' roto.freeze '{"layer":"#1","wait":true}' --save
```

### Text: styles and editing

Source Text holds character style runs and per-paragraph settings. `layer.setText` changes the
whole layer, or only characters `range: [start, end]` (character indices): character attributes
(`font`, `size`, `fill`, `tracking`, `kerning: metrics|optical|<1/1000 em>`, `tsume`,
`baseline: superscript|subscript`, `allCaps`…) split the text into runs; paragraph attributes
(`justify`, `indentLeft`, `indentFirst`, `spaceBefore`, `direction`, `composer`,
`hangingPunctuation`…) apply to the paragraphs the range touches. Editing mirrors the Type tool:

1. `layer.newText {"text":"", "box":[100,100,600,300], "edit":true}` makes paragraph text and
   starts editing (`position` instead of `box` makes point text; `vertical: true` vertical type).
2. `text.insert {"text":"Hello world"}` types at the caret (replacing the selection);
   `text.setSelection {"start":6,"end":11}` selects "world"; `text.moveCaret {"to":"wordLeft",
   "extend":true}` moves like the arrow keys; `text.delete {"word":true}` is Alt+Backspace.
3. `layer.setText {"range":[6,11], "size":40, "fill":"#ff5500"}` styles the selection (the
   Character panel does the same); with an empty range it sets the style the next typed text takes.
4. `edit.copy`, `edit.paste`, `edit.pasteTextMatchFormatting` and `edit.pasteTextFormattingOnly`
   work on the selected text; `text.endEdit` commits (an empty Type-tool layer is removed).

Fonts: `text.fonts` (MCP `list_fonts`) lists every family a text layer can use, bundled and installed, with its
styles, origin (`bundled`, `system`, `user`) and own-language name (`{"query":"gothic"}` filters
by name; `{"rescan":true}` first picks up fonts installed since launch). Pass a listed `family` and
one of its `styles` to `layer.setText {"font", "style"}`; `text.fontFeatures` says which OpenType
options a font draws with its own glyphs.

In expressions, `text.sourceText.style` / `getStyleAt(i, t)` read styles and the setters
(`setFontSize(v, start?, count?)`, `setFillColor`, `setText`, `setJustification`…) return a
styled document.

### Scripts and ScriptUI windows

Scripts that build ScriptUI windows publish them to the session; agents drive them like a user:

1. `scriptui.list` → `[{window, title, kind: dialog|palette|window|panel, script, modal, size}]`.
2. `scriptui.get {"window": id}` → the control tree (`type`, `name`, `text`, `value`, `checked`,
   `items`, `selection`, laid-out `bounds`, `handlers`…).
3. `scriptui.click {"widget": "ok"}` presses a button / toggles a checkbox / picks a radio button or
   tab; `scriptui.set {"widget": "#4", "value": "Shot_"}` types into edit text, moves a slider or
   picks a list item (index or text); `scriptui.close {"result": 2}` closes. Controls are addressed
   by id, `#id`, `properties.name` or text; `window` can be omitted when one window is open. Each
   returns the handler run's `{ok, output, error}`.

A dialog's `show()` waits for the user: the `script.run` / `file.runScript` reply carries
`"waiting": true`, and the script continues (its final output arrives in the reply of the click
that closes the dialog). File ▸ Scripts: `file.scripts.list`, `file.runScript {"name": …}`,
`file.installScript` / `file.installScriptUIPanel {"path": …}`, `window.scriptPanel {"name": …}`.

### Timeline: layer order, properties and shortcuts

* `layer.arrange {"layers": [...], "above": "Layer Name"}` moves layers to just above another
  one (what dragging Timeline rows does: the selected layers move together, one undo step);
  `to: front|forward|backward|back` and `index` still work.
* The reveal shortcuts are UI commands (`ui.menu.invoke` or the keys themselves): A Anchor Point,
  P Position, S Scale, R Rotation, T Opacity, M Mask Path, F Mask Feather, E Effects, L Audio
  Levels, U keyframed properties; pressed twice quickly AA Material Options, PP paint / Roto
  Brush / Puppet, SS selected properties, RR Time Remap, TT Mask Opacity, MM all mask
  properties, FF missing effects, EE Expressions, LL Waveform, UU everything changed from its
  default. Shift+key adds or removes a set; the same key again hides it; Ctrl/Cmd+` twirls the
  selected layers open or closed. With no layer selected they apply to every layer.
* `keys.toggleTransform {"layers"?: [...], "prop": "position"}` (Alt+Shift+A/P/S/R/T) adds or
  removes a keyframe at the current time on that Transform property of each layer, in one undo
  step (separated X/Y/Z Position; Orientation and X/Y/Z Rotation on 3D layers).
* Rename a layer with Enter (or double-click a name): the whole name is selected, Enter or a
  click elsewhere commits, Escape cancels; `layer.rename {layer, name}` does it directly.

### History, puppet recording, plug-ins

* `edit.history.list` lists every undo state as a tree (undoing then editing keeps the undone
  states as a branch); `edit.history.goto {"index": n}` (or `id`, or `steps`) jumps to any of
  them. MCP: the `history` tool.
* `puppet.recordPin {"layer": "#1", "pin": "Puppet Pin 1", "samples": [[t, x, y]…]}` records a
  drag (t = seconds since it began, layer space) into Position keys at the comp frame rate from the
  current time; `puppet.recordOptions {speed, smoothing, useDraftDeformation, showMesh}`.
* `puppet.selectPins {"layer": "#1", "pins": ["Puppet Pin 1"], "add"?, "toggle"?}` selects pins
  like the viewer's click / Shift-click (`pins: []` deselects); Edit ▸ Clear (Delete) then removes
  the selected pins, not their layer (`puppet.removePin {pins?}`). Advanced and Bend pins turn by
  their ring and scale by its square in the viewer (Shift: 15° / 5 % steps; automation ids
  `viewer.puppetPin.<uid>.rotate` / `.scale`), or `puppet.setPin {rotation, scale}`. A drag
  outside the art (or Alt-drag) marquee-selects pins; Edit ▸ Select All with a pin selected selects
  every pin of that kind; `puppet.recordPin {..., "pins": [...]}` records several pins moving
  together (⌘/Ctrl-drag one of several selected pins).
* Rigging pins: `paths.pointsFollowNulls {"layer": "#1", "pins"?: [...]}` gives each Position /
  Advanced pin a null that drives it (parent the nulls to each other to build limbs);
  `paths.nullsFollowPoints {..., "pins": [...]}` makes nulls that ride on pins (parent props to
  them). Without `pins` they use the selected pins, or every pin of a layer with no path.
* Follow-through (hair, cloth, tails): `puppet.follow {"leader": "Puppet Pin 1", "pins": [...],
  "delay": 0.1, "amount": 100, "cascade": true}` makes the pins trail the leader's motion (the
  k-th nearest by k × delay with `cascade`) through Position expressions; without `leader` /
  `pins` the first selected pin leads the other selected pins.
* `effect.plugins.load {"path": "x.wasm"}` / `effect.plugins.list`: WebAssembly effect plug-ins
  ([plugins.md](plugins.md)), then `effect.apply` by id like a built-in.

## CLI

Each invocation runs a headless engine with no window. It opens the demo project unless you pass
`--project F.ecproj`, a positional `*.ecproj` or `--empty`. Add `--json` for one compact JSON document
on stdout. Errors print `{"error": ...}` and exit with status 1; usage errors, such as an unknown
option, exit with status 2 before anything runs.

```sh
effectcraft-cli info --json
effectcraft-cli commands --filter keys
effectcraft-cli exec --list --schemas --json                     # every command with its params JSON Schema
effectcraft-cli exec comp.new --params '{"name":"Main","width":1280,"height":720}' --empty --save-as main.ecproj
effectcraft-cli exec layer.newSolid '{"color":"#3366ff"}' main.ecproj --save
effectcraft-cli props Main '#1' --project main.ecproj           # flat property list with paths
effectcraft-cli set Main '#1' transform/opacity 40 main.ecproj --save
effectcraft-cli set Main '#1' transform/position '[100,360]' --time 0 main.ecproj --save
effectcraft-cli get Main '#1' transform/position --time 0.5 main.ecproj --json
effectcraft-cli run main.ecproj comp.open '{"comp":"Main"}' time.set '{"time":1}' --json
effectcraft-cli render-frame main.ecproj --time 1 --max-side 640 --out f.png --json
effectcraft-cli exec layer.newNull --bridge 9877                 # same commands, against the live app
effectcraft-cli exec file.exportLottie '{"comp":"Main","path":"main.json","includeExpressions":true}' main.ecproj --json
effectcraft-cli exec file.importLottie '{"path":"anim.json"}' main.ecproj --save
```

Scripts written against After Effects' documented scripting API run with `script`:

```sh
effectcraft-cli script build.jsx --save-as main.ecproj     # empty project unless one is given
effectcraft-cli script --eval 'app.project.item(1).numLayers' main.ecproj --json
effectcraft-cli script tweak.jsx --bridge 9877              # against the live app
```

`writeLn`/`$.writeln`/`alert` output is printed, then the value of the last expression. A script
error exits with status 1 and `file:line:col: message`. Scripts may read files only in the
project's folder and may not write files or use the network unless the user turns on Preferences ▸
Scripting & Expressions ▸ Allow Scripts to Write Files and Access Network.

`file.exportLottie` returns `{path, bytes, warnings}`: the warnings list every feature Lottie
cannot express (most effects, cameras and lights, layer styles, audio, video footage…), so an
agent can check what a player will not show. A `.lottie` path writes a dotLottie archive.
`file.importLottie` returns `{comp, items, warnings}` and opens the new composition.

Premiere Pro interop goes through timeline interchange (MCP: the `execute_command` tool; control channel: `engine.execute`
with the same ids):

```sh
effectcraft-cli exec file.importTimeline '{"path":"edit.xml"}' main.ecproj --save      # FCP7 XML / .fcpxml / .otio / .edl / .aaf / .omf
effectcraft-cli exec file.exportTimeline '{"comp":"Main","path":"Main.xml"}' main.ecproj --json
effectcraft-cli exec file.exportTimeline '{"comp":"Main","path":"Main.otio","prerender":"none"}' main.ecproj
```

`file.importTimeline` returns `{format, folder, comps, allComps, items, missing, warnings}`
(`missing` lists media imported as placeholders). `file.exportTimeline` writes Final Cut Pro XML
(`.xml`, which Premiere Pro opens with File ▸ Import) unless `format` or the extension says
`fcpxml`, `otio`, `edl`, `aaf` or `omf`; `prerender` (`unsupported` by default, `all`, `none`) controls which
layers are rendered to ProRes 4444 movies next to the document. It returns `{path, format,
sequences, videoTracks, audioTracks, clips, prerendered, warnings}`. Native `.prproj` files are
not read or written.

`<comp>` is an id or name, or `-` for the active comp. `<value>` is JSON (`50`, `[960,540]`,
`"#ff0000"`) or a bare string.

## Seeing the UI

To work on the UI, start the app with `cargo run -p effectcraft -- --control 9877` (add `--demo` to
open the demo project; without it the app starts with an empty project) and use MCP bridge
mode or the raw control channel. A good loop is: `ui_elements` to find an id, `ui_click` or `ui_drag`
to act, then `screenshot {"panel":"Timeline"}` to check the result. `render_frame` shows the
composition itself at any zoom.
