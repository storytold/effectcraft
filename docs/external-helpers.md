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
| Inspect | `effect.helpers.list` → `{helpers: [{program, name, version, loads, running, effects, restarts, error}], blocked}`; `effect.plugins.list` shows each effect's `source` (`helper: …`) and last render `error`. |
| Errors | A failed render leaves the frame as it was and puts ⚠ after the effect's name in Effect Controls (hover for the message). The next render starts the helper again. An effect that stops its helper 3 times is turned off and listed in `helper_blocklist.txt` in the settings folder. `effect.helpers.scan {"retry": true}` turns it back on. A helper that stops 5 times in a minute stays off until the next scan. |
| Timeouts | `EFFECTCRAFT_HELPER_TIMEOUT` (seconds per render step, default 120) and `EFFECTCRAFT_HELPER_LOAD_TIMEOUT` (start-up, describe, load; default 600). A helper idle for 30 s is pinged before it is used again. |

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
available, otherwise the temp folder) and names it in `hello`. Images are addressed by an
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
| `{"op":"hello","version":1,"frames":"<path>","features":["layer_transform","comp_camera","comp_camera_time","comp_lights"]}` | `{"op":"hello","version":1,"name":"OpenFX","helper_version":"1.0","loads":["ofx"]}` | First message. The helper opens `frames` read/write. `loads` lists the extensions `load` accepts. A different `version` means EffectCraft won't use the helper. `features` lists optional callbacks EffectCraft answers (absent in older builds: ask only for what is listed). |
| `{"op":"describe"}` | `{"op":"effects","effects":[{"manifest":{…},"padding":true}],"errors":["…"]}` | `manifest` is a [plug-in API 2 manifest](plugins.md#plug-in-api-2-native-plug-ins) (id, name, category, params). `padding: true` means EffectCraft asks `padding` before each render. |
| `{"op":"load","path":"…"}` | `effects` (only the new ones) or `error` | A file or folder the user picked. Replayed after a restart. |
| `{"op":"padding","effect":"…","instance":7,"time":1.0,"scale":1.0,"layer_size":[w,h],"flat":[…],"texts":[…]}` | `{"op":"padding","pixels":3}` | Transparent pixels to add on each side before the render (at most 4096). |
| `{"op":"render","effect":"…","instance":7,"time":1.0,"frame":<image ref>,"layer_size":[w,h],"flat":[…],"texts":[…],"frame_rate":30,"frame_range":[0,10]}` | callbacks, then `{"op":"done"}` or `{"op":"error","message":"…"}` | Process `frame` in place: read it, write the result to the same ref, reply `done`. `flat` / `texts` are the parameter values flattened by plug-in API 2 rules. `instance` identifies the applied effect (stable across frames). `frame_range` is the layer's source range in layer-time seconds, or `null`. |
| `{"op":"destroy","instance":7}` | none | The instance is gone; free what it holds. |
| `{"op":"ping"}` | `{"op":"pong"}` | Heartbeat. |
| `{"op":"shutdown"}` | none | Exit. |

**Callbacks during a render.** Before it replies `done`, the helper may ask:

| Helper asks | EffectCraft answers |
|---|---|
| `{"op":"input_at","time":t}` | `{"op":"image","image":<image ref>\|null}`: the effect's input (the layer, its masks, the effects above) at layer time `t`, in its own extent. |
| `{"op":"layer_input","param":"<layer param id>","time":t}` | `image`: that layer at layer time `t` with effects and masks, resampled into this frame's grid. |
| `{"op":"params_at","time":t}` | `{"op":"params","values":[[flat…],[texts…]]\|null}` |
| `{"op":"layer_transform","param":"<layer param id>","time":t}` (only when `hello` listed `layer_transform`) | `{"op":"transform","transform":{"matrix":[[…4],[…4],[…4]],"opacity":o,"rect":[x0,y0,x1,y1]}\|null}`: where that layer sits at layer time `t`. `matrix` (rows of an affine 3 × 4) takes a point of the image `layer_input` hands over (x, y in its pixel grid, pixel (i, j) spanning i..i+1) at depth z (the other layer's own pixels, away from the viewer) to this layer's pixels (x, y, as a point parameter in layer pixels) and depth z. It carries the other layer's position, anchor, scale, rotation, orientation and parents, and undoes this layer's own transform; `opacity` is the other layer's (0–1), and `rect` its own extent (its pixels 0..width, 0..height) in that image grid. A particle emitter places particles on an emitter layer where it is drawn. |
| `{"op":"comp_camera"}` (only when `hello` listed `comp_camera`) | `{"op":"camera","camera":{"matrix":[[…4],[…4],[…4]],"zoom":z,"camera_layer":b}\|null}`: the comp camera at the render's time as this layer sees it. `matrix` takes this layer's pixels (x, y as a point parameter in layer pixels, z away from the viewer) to homogeneous X, Y, W: the point shows at (X / W, Y / W) in this layer's pixels, W being its depth in front of the camera; a length there shows `zoom / W` times as long. `camera_layer` is false for the comp's default view. `dof` (left out when the camera's Depth of Field is off) is `[focus distance, aperture, blur level]`: comp pixels from the eye, comp pixels, and 1 = 100 %. With `"time":t` (only when `hello` listed `comp_camera_time`) the answer is the camera at layer time `t` instead, as this layer sees it then (camera motion blur). |
| `{"op":"comp_lights","time":t}` (only when `hello` listed `comp_lights`) | `{"op":"lights","lights":[{"name":"Emitter","kind":2,"pos":[x,y,z],"dir":[x,y,z],"color":[r,g,b],"intensity":1.0,"cone":90.0},…]\|null}`: the comp's lights switched on at layer time `t`, in the same space as `comp_camera`'s points. `kind` 0 parallel, 1 spot, 2 point, 3 ambient; `color` is the light's colour and `intensity` its Intensity (1 = 100 %); `cone` the spot's whole Cone Angle in degrees. A particle system's Light(s) emitter emits from them. |

While a helper waits for an answer, EffectCraft may send another `render` (or `padding`). This
happens when producing the answer needs an effect of the same helper: for example, motion blur
asking for the previous frame of a layer that also has a retime effect from the same helper.
The helper must serve it completely (with its own callbacks, nested up to 16 deep) and then
keep waiting. Images for callbacks are placed after the frame in the exchange file and freed
when the render ends.

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
