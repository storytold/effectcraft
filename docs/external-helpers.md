# External effect helpers

An **external effect helper** is a separate program that offers effects to EffectCraft. Each
helper runs as a child process and talks to EffectCraft over the protocol below. EffectCraft
registers every effect a helper lists as an ordinary plug-in effect: it shows up in Effects &
Presets, has normal properties (keyframes, expressions, Effect Controls) and renders through
the helper. Helpers exist to run native code, such as an OpenFX plug-in host, **outside**
EffectCraft's process. If a helper crashes or hangs, EffectCraft shows the error on the effect
and carries on. See [ADR 0001](adr/0001-external-effect-helpers.md) for why.

## Using helpers

| What | How |
|---|---|
| Install | Put the helper program (`effectcraft-helper-<name>`, `.exe` on Windows) in `helpers/` next to EffectCraft, in `helpers/` in the [settings folder](preferences.md), or in a folder named in `EFFECTCRAFT_HELPER_PATH` (a path list; entries can be programs or folders). Subfolders one level down are searched too. |
| Start | Helpers run native code, so nothing starts them unless the user asks: Effect ▸ Scan for Effect Helpers (`effect.helpers.scan`) starts them. To start them every time EffectCraft launches, opt in with `EFFECTCRAFT_HELPERS=1`. It is an environment variable, not a setting, so writing to the settings folder can't turn it on. Agents confined with `--automation-read-root` / `--automation-write-root` can't run `effect.helpers.scan` or `effect.helpers.load`, and scripts can run them only with Allow Scripts to Write Files and Access Network on. |
| Load more plug-ins | Effect ▸ Load Plug-ins Through Helper… (`effect.helpers.load {path}`) hands a file or folder to the helpers that accept it (an OpenFX helper takes `.ofx` plug-ins and `.ofx.bundle` folders). |
| Inspect | `effect.helpers.list` → `{helpers: [{program, name, version, loads, running, processes, max_processes, effects, restarts, error, features}], blocked}` (`processes` is how many processes run now, `max_processes` how many the helper allows, `features` the [extensions](#extensions) it announced); `effect.plugins.list` shows each effect's `source` (`helper: …`) and last render `error`. |
| Errors | A failed render leaves the frame as it was and puts ⚠ after the effect's name in Effect Controls (hover for the message). The next render starts the helper again. An effect that stops its helper 3 times is turned off and listed in `helper_blocklist.txt` in the settings folder; the 3 stops are counted per effect across all processes of the helper (see [Parallel rendering](#parallel-rendering)), not per process. `effect.helpers.scan {"retry": true}` turns it back on. A helper that restarts 5 times in a minute (counted over all its processes) stays off until the next scan. |
| Timeouts | `EFFECTCRAFT_HELPER_TIMEOUT` (seconds per render step, default 120), `EFFECTCRAFT_HELPER_LOAD_TIMEOUT` (start-up, describe, load; default 600) and `EFFECTCRAFT_HELPER_EDIT_TIMEOUT` (seconds one command waits for [`param_changed`](#parameter-changes) answers in all, fractions allowed, default 2, at most 600). A helper idle for 30 s is pinged before it is used again. |

Helpers are not available in the web build.

## Protocol (version 1)

EffectCraft starts the helper with the single argument `--effectcraft-helper`, with stdin,
stdout and stderr piped. The environment variable `EFFECTCRAFT_HELPER_STATE_DIR` names a folder
where the helper may keep its own records (for example, plug-ins that crashed it while
loading).

- **Requests** (EffectCraft → helper): one JSON object per line on stdin.
- **Replies** (helper → EffectCraft): one JSON object per line on stdout, prefixed with
  `@ec-helper ` (with the space). Lines without the prefix, or text before it, are logged and
  otherwise ignored, so plug-ins that print to stdout don't break the protocol. stderr is
  logged (whatever bytes it holds).
- Every object has an `"op"`. Lines are at most 256 MB.
- The helper exits when stdin closes or on `shutdown`.

### Frame exchange file

Pixels never go through the pipe. EffectCraft creates a file (in `/dev/shm` on Linux when
available, otherwise the temp folder; on Windows with the temporary-file attribute, so the
system keeps it in memory where it can) and names it in `hello`. EffectCraft and the reference
helper side (`serve`) use positional reads and writes at the image offsets. No image may end
beyond 64 GiB in the file. Each process has its own file, and the images it holds at once are
stacked in it: the outermost render's frame at offset 0, then its callback answers; a nested
render (see below) starts after the images still in use by the render it is nested in. All of
them share the 64 GiB, so in 32 bpc one process holds about 128 images of 7680×4320 at once,
or 16 of 16384×16384. Images are addressed by an
**image ref**:

```json
{"offset": 0, "width": 1920, "height": 1080, "origin": [0, 0], "scale": 1}
```

The image is `width × height` pixels starting `offset` bytes into the file. Each pixel is
premultiplied RGBA as four `f32`s in native byte order (16 bytes), rows top to bottom, and an
image is at most 2²⁸ pixels. `origin` is the buffer position of layer pixel (0, 0); `scale` is
buffer pixels per layer pixel. EffectCraft decides every offset, so the helper only reads and
writes where it's told. On Unix, EffectCraft unlinks the file once the helper has opened it, so
nothing is left behind if either side dies. Helpers may map the file instead of reading it.

### Messages

| Request | Reply | |
|---|---|---|
| `{"op":"hello","version":1,"frames":"<path>"}` | `{"op":"hello","version":1,"name":"OpenFX","helper_version":"1.0","loads":["ofx"],"max_processes":4,"features":["param_changed","masks"]}` | First message. The helper opens `frames` read/write. `loads` lists the extensions `load` accepts. `max_processes` (optional, default 1) is how many processes of the helper EffectCraft may run at once, see [Parallel rendering](#parallel-rendering). `features` (optional) lists the [extensions](#extensions) the helper understands. A different `version` means EffectCraft won't use the helper. |
| `{"op":"describe"}` | `{"op":"effects","effects":[{"manifest":{…},"padding":true,"notify_params":["p114"],"masks":true}],"errors":["…"]}` | `manifest` is a [plug-in API 2 manifest](plugins.md#plug-in-api-2-native-plug-ins) (id, name, category, params). `padding: true` means EffectCraft asks `padding` before each render. `notify_params` (optional): see [Parameter changes](#parameter-changes). `masks: true` (optional): see [Masks](#masks). |
| `{"op":"load","path":"…"}` | `effects` (only the new ones) or `error` | A file or folder the user picked. Replayed after a restart. |
| `{"op":"padding","effect":"…","instance":7,"time":1.0,"scale":1.0,"layer_size":[w,h],"flat":[…],"texts":[…]}` | `{"op":"padding","pixels":3}` | Transparent pixels to add on each side before the render (at most 4096). With [masks](#masks), also `"masks":[…]`. |
| `{"op":"render","effect":"…","instance":7,"time":1.0,"frame":<image ref>,"layer_size":[w,h],"flat":[…],"texts":[…],"frame_rate":30,"frame_range":[0,10]}` | callbacks, then `{"op":"done"}` or `{"op":"error","message":"…"}` | Process `frame` in place: read it, write the result to the same ref, reply `done`. `flat` / `texts` are the parameter values flattened by plug-in API 2 rules. `instance` identifies the applied effect (stable across frames). `frame_range` is the layer's source range in layer-time seconds, or `null`. With [masks](#masks), also `"masks":[…]`. |
| `{"op":"param_changed","effect":"…","instance":7,"param":"p114","time":1.0,"layer_size":[w,h],"values":{…},"texts":[…]}` | `{"op":"set_params","values":{"p3":[1,0.4,0,1],"p4":36}}` or `error` | The user changed a parameter listed in `notify_params`; only to helpers with the `param_changed` [extension](#extensions). See [Parameter changes](#parameter-changes). |
| `{"op":"destroy","instance":7}` | none | The instance is gone; free what it holds. Not sent by EffectCraft yet (see [Parallel rendering](#parallel-rendering)). |
| `{"op":"ping"}` | `{"op":"pong"}` | Heartbeat. |
| `{"op":"shutdown"}` | none | Exit. |

**Callbacks during a render.** Before it replies `done`, the helper may ask:

| Helper asks | EffectCraft answers |
|---|---|
| `{"op":"input_at","time":t}` | `{"op":"image","image":<image ref>\|null}`: the effect's input (the layer, its masks, the effects above) at layer time `t`, in its own extent. |
| `{"op":"layer_input","param":"<layer param id>","time":t}` | `image`: that layer at layer time `t` with effects and masks, resampled into this frame's grid. |
| `{"op":"params_at","time":t}` | `{"op":"params","values":[[flat…],[texts…]]\|null}` |
| `{"op":"layer_masks","param":"<layer param id>","time":t}` | `{"op":"masks","masks":[…]\|null}`: the masks of that layer at layer time `t`, see [Masks](#masks). |

While a helper waits for an answer, EffectCraft may send another `render` (or `padding`). This
happens when producing the answer needs an effect of the same helper: for example, motion blur
asking for the previous frame of a layer that also has a retime effect from the same helper.
The helper must serve it completely (with its own callbacks, nested up to 16 deep) and then
keep waiting. Images for callbacks are placed after the frame in the exchange file and freed
when the render ends.

### Extensions

A helper lists the protocol extensions it understands in `hello`: `"features": ["param_changed", "masks"]`.
EffectCraft uses an extension only with helpers that list it, so a helper without `features` (or
with an empty list) is spoken to exactly as described above; the protocol version stays 1.
Entries EffectCraft doesn't know are ignored, as is anything that isn't an array of strings.

| Feature | What it adds |
|---|---|
| `param_changed` | [Parameter changes](#parameter-changes): `notify_params` in `describe` / `load`, the `param_changed` request and the `set_params` reply. |
| `masks` | [Masks](#masks): `masks: true` in `describe` / `load`, `masks` in `render` / `padding`, the `layer_masks` callback. |

### Parameter changes

Some effects set other parameters when the user changes one: a preset menu that rewrites a dozen
values, a "link" checkbox, a mode menu that resets dependent controls. A helper with the
`param_changed` extension lists those parameters per effect, in `describe` / `load`:
`"notify_params": ["p114", "p109"]` (ids of the manifest's parameters; unknown ids are ignored).

- **When.** The user changes one of them: in Effect Controls, the timeline, with `prop.set`,
  pasting values, a script, any other edit, when the parameter's value at the edit's time changed
  (the time of a keyframe the edit set, else the current time). Turning the stopwatch on, changing
  a keyframe's interpolation, ease or label, or editing an expression changes no value and sends
  nothing. Never while rendering, never for expression or keyframe evaluation, never for undo and
  redo.
- **Request.** `param_changed` names the effect, the `instance` (the same id renders have), the
  `param` the user changed, the layer `time` of the change in seconds (where the user's keyframe
  went, else the current time) and the layer size. `values` holds every parameter's value at that
  time, keyed by id, in property units: sliders and angles as numbers, checkboxes `true`/`false`,
  popups 0-based, points `[x, y]` and point3 `[x, y, z]` in layer pixels (not buffer pixels),
  colours `[r, g, b, a]` 0-1, layers the layer id or `null`. Text and hidden parameters are in
  `texts`, as in `render`. The instance may be one the process hasn't rendered yet.
- **Answer.** `set_params` with the parameters the helper changes, in the same units (text and
  hidden ones as strings), or `{"values":{}}`. EffectCraft writes them into the user's edit: one
  undo step, at the same time and the same way as the user's value (a new keyframe when the
  property is animated). Ids that aren't the effect's, values of the wrong type or shape, numbers
  that aren't finite, popup indices out of range or not whole and layers that aren't in the comp
  are ignored entry by entry; slider values are clamped to the slider range. Text and hidden
  values take at most 16 MB per answer in all (enough for opaque plug-in data such as a look-up
  table; it keeps one answer from bloating the project and every undo step): a string that would
  pass that is left out, and logged. A
  change the answer makes doesn't send another `param_changed`. `ui` in the answer (per-parameter
  state such as `{"p12": {"enabled": false}}`) is accepted and ignored for now: Effect Controls
  can't grey out a single parameter yet.
- **Time.** Edits wait for the answers on the thread that draws the interface, so the wait is
  bounded per command: one command (an edit in Effect Controls, a `prop.set`, an `engine.batch`
  or a script with every edit it makes) waits at most `EFFECTCRAFT_HELPER_EDIT_TIMEOUT` (default
  2 s) for all its answers together, the wait for a free process and writing the request
  included. Each request is given what is left; once that is spent, or an answer failed, the rest
  of the command sends no more `param_changed` (logged). One edit reports at most 8 changes
  (pasting into many effects), one after another, each with the values the answers before it set.
  It uses a process that runs and is idle, between renders, preferring one that has tried every
  `load`: never one inside a render's conversation, and it never starts a process (after a crash
  the next render starts the helper again, and the first change after that is reported). A
  process that still has to replay a `load` for the first time isn't asked; a `load` it tried and
  that failed (a folder that went away) is retried before renders but doesn't hold edits up. When
  the helper isn't running, is busy for the whole time, answers `error`, misses
  the time, exits or answers anything but `set_params` / `error`, the user's change stays and
  nothing else changes. A helper that missed the time or broke the protocol is stopped (its late
  answer would be out of step), as after any timeout, but this doesn't count towards turning the
  effect off. The next render starts it again, and that start does count towards the 5 restarts a
  minute: a helper that keeps missing the edit timeout ends up off until the next scan, so give
  slow helpers a longer `EFFECTCRAFT_HELPER_EDIT_TIMEOUT`.

Rust helpers built on `serve` announce `param_changed` and answer it with
`EffectPlugin::notify_params` / `EffectPlugin::param_changed` of their effects (see
[plug-ins](plugins.md)).

### Masks

Effects that work on shapes (stroke or fill along a mask, follow a path) get mask paths from a
helper with the `masks` extension:

- **The effect's own layer.** An effect described with `"masks": true` gets its layer's enabled
  masks in every `render` and `padding`, evaluated at the render time (the layer's mask
  animation, top to bottom):

  ```json
  "masks": [{"name":"Mask 1","closed":true,"mode":"add","inverted":false,
             "opacity":1.0,"feather":[0,0],"expansion":0,
             "vertices":[[x,y],…],"in":[[dx,dy],…],"out":[[dx,dy],…]}]
  ```

  `mode` is `none`, `add`, `subtract`, `intersect`, `lighten`, `darken` or `difference`;
  `opacity` 0-1. Vertices are in buffer pixels, mapped exactly like point parameters in `flat`
  (layer pixels × `scale` + `origin`; for text and shape layers measured in the effect's
  comp-sized layer bounds, like their point parameters); `in` / `out` tangents are relative to
  their vertex and, like `feather` and `expansion`, in buffer pixels (× `scale`). In `padding`
  they are in the buffer before padding (where `flat`'s points are); in `render`, in `frame`.
  Effects without `masks: true` (and every effect of a helper without the extension) get no
  `masks` field.
- **Another layer.** During a render the helper may ask `layer_masks` for the layer chosen in
  one of the effect's layer parameters, at a layer time. The answer is that layer's masks in the
  same format, in buffer pixels of this render's frame, placed where `layer_input` puts that
  layer's pixels (centred when the layers differ in size), or `null` when no layer is chosen. For
  a text layer the glyph outlines at that time follow its masks, one closed path per contour,
  named by the character and in `difference` mode (what Create Masks from Text makes, but live).
  An effect reading another layer's masks keeps that layer as a dependency like `layer_input`:
  its cached frames are made again when that layer's masks (or text) change.
- **Bounds.** One message carries at most 1024 masks and 65 536 vertices in all (about 8 MB of
  JSON); masks beyond that are left out, in order. A mask whose numbers aren't all finite, or
  whose tangent lists don't match its vertices, is left out.
- A manifest parameter kind for picking one of the layer's masks isn't there yet: use a popup or
  a slider for the mask number.

Rust helpers built on `serve` announce `masks`; `Provider::wants_masks` decides which effects
ask for them, and the effects read them with `PluginHost::masks` / `PluginHost::layer_masks` (see
[plug-ins](plugins.md)).

### Parallel rendering

By default a helper is one process with one conversation, so renders through it take turns. A
helper that can do better says so in `hello`: `"max_processes": N`. EffectCraft then runs up to
`N` processes of the same program, each with its own pipes, exchange file and conversation
(protocol version 1, nothing else changes), and gives every render to a free process.

- `N` must be a whole number from 1 (JSON `4`; `4.0`, `"4"`, `0`, negative values and
  anything else count as absent, that is 1: exactly the single process described above). Values
  above 8 are clamped to 8, and to the machine's number of cores. `EFFECTCRAFT_HELPER_MAX_PROCESSES`
  (a number) replaces the core count in that cap.
- Processes start lazily: another one starts only when a render needs a process and all running
  ones are busy. Each process starts with `hello` and `describe` like any other. Extra
  processes stay alive until the helper is stopped (EffectCraft quits): they are not
  stopped when idle.
- Every process builds its own copy of an effect instance, on the first `render` (or `padding`)
  with an `instance` it doesn't know. State that an instance holds exists once per process, so
  `N` also multiplies the helper's memory use. Consecutive frames of one instance can land on
  different processes: don't keep anything between renders that the next one can't rebuild.
- `load` is sent to every running process; a process that is busy, and one started later,
  loads the paths before its next request (the same replay as after a restart). It is never sent
  while the helper waits for a callback answer. Loading a path again (a folder with new
  plug-ins) reaches every process again; a process that starts later loads each path once. A
  process that answers a replayed `load` with `error` is asked again before its next request.
- `destroy` can be sent to every idle running process (`effectcraft_helper_host::destroy_instance`),
  without waiting for busy ones. This is API only: EffectCraft does not send `destroy` itself
  yet, so a helper keeps its instances until it stops.
- A render's callbacks (`input_at`, ...) and nested renders stay on the process that owns the
  render, and renders nested in it (see above) go to that process too.
- A crash or hang stops that one process and fails the render it was working on. The renders in
  the other processes finish. Stops are counted per effect across the whole pool, not per
  process: an effect that stopped 3 processes (the same slot or different ones) is turned off.
  The limit of 5 restarts per minute is for the helper as a whole too. Every start of the first
  process counts; for the other processes of the pool, the first start of each is not a restart
  (it fills the pool), and every later start of it is, also when the process before exited while
  it was starting up.
- A render that finds all processes busy waits for one, at most `EFFECTCRAFT_HELPER_LOAD_TIMEOUT`
  seconds, then fails with a message instead of hanging.

### Errors and restarts

`error` replies are per request: the helper carries on. EffectCraft stops (kills) the helper
when it exits, closes stdout, sends a reply that doesn't parse or doesn't fit, or misses a
timeout, or stops reading its stdin for longer than the timeout. The request then fails. The next request starts a fresh process, runs `hello`, replays
earlier `load`s and recreates instances lazily: the helper sees a `render` for an `instance` it
doesn't know and creates it.

## Writing a helper

Any language works: read lines, write prefixed lines, read and write a file. Rust helpers can
use `effectcraft_helper_host::serve::serve` with a `Provider` that lists `EffectPlugin`s;
`crates/helper-host/tests/it/main.rs` is a complete example and the protocol test that runs
in `cargo xtask ci`. An unofficial OpenFX helper built this way lives in its own repository,
[ec-extension-ofx](https://github.com/glumbis/ec-extension-ofx).
