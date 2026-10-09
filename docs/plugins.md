# Plug-ins and extensions

EffectCraft's core aims at After Effects parity. What After Effects users get from third parties
(effect plug-ins, scripts, ScriptUI panels) is an extension in EffectCraft too, and EffectCraft
offers the same kinds of extension points as After Effects so that anyone can write their own.

## Extension points compared with After Effects

From After Effects' public documentation (the user guide's scripting pages and the Scripting
Guide):

| After Effects | EffectCraft | |
|---|---|---|
| Effect plug-ins (C/C++ SDK, `.aex` / `.plugin`) | [Effect plug-ins](#effect-plug-ins): WebAssembly modules (plug-in API v1) or Rust; the `Plug-ins` folder next to the settings loads at start-up | Different ABI: After Effects SDK plug-ins can't run (G8). Builds with the opt-in `openfx` feature also load [OpenFX](ofx.md) plug-ins, the cross-host standard many vendors ship |
| Scripts folder: its scripts are listed in File ▸ Scripts | `Scripts/` in the settings folder (File ▸ Scripts ▸ Install Script File… copies a script there), listed in File ▸ Scripts with the bundled samples | Same |
| File ▸ Scripts ▸ Run Script File… (`.jsx`, `.jsxbin`) | `.jsx` / `.js` | Gap: `.jsxbin` (an undocumented binary encoding) |
| ExtendScript preprocessor directives (`#include`, `#target`, `#targetengine`) | Not understood: a script that starts with one doesn't parse | Gap |
| `Scripts/Startup` and `Scripts/Shutdown`: run in alphabetical order when the app starts and quits | Not run | Gap |
| `Scripts/ScriptUI Panels`: listed at the bottom of the Window menu, dockable like the app's own panels, `this` is the panel | `Scripts/ScriptUI Panels/` in the settings folder (File ▸ Scripts ▸ Install ScriptUI Panel… copies one there), listed at the bottom of the Window menu, docked like any panel, `this` is the panel; the panels open when the app quits open again at the next launch | Same. Where a panel was docked isn't kept: workspace layouts aren't saved between launches |
| Keyboard shortcuts for scripts (the Keyboard Shortcuts editor lists the scripts of the Scripts folder) | Shortcuts bind to commands and menu entries; the File ▸ Scripts and Window menu entries of scripts aren't offered | Gap |
| Running a script from the command line (`afterfx -r script.jsx`) | `effectcraft-cli script`, the control channel's and MCP's `script.run` | Same purpose, other entry points |
| `app.settings` (kept in the preferences between sessions) | Kept in `script_settings.json` in the settings folder (`script.settings.get` / `script.settings.save`) | Same |
| Allow Scripts to Write Files and Access Network | Settings ▸ Scripting & Expressions | Same |

The settings folder is `~/Library/Application Support/EffectCraft` on macOS, `%APPDATA%\EffectCraft`
on Windows and `$XDG_CONFIG_HOME/effectcraft` (or `~/.config/effectcraft`) on Linux
([preferences.md](preferences.md)); the web app keeps it in browser storage.

## Scripts

Scripts use After Effects' scripting object model (`app`, `app.project`, `CompItem`, layers,
`Property` with keyframes and eases, `app.beginUndoGroup`…), so most scripts written for After
Effects' documented API run unchanged. Every edit a script makes is an engine command: undoable,
journaled and the same as the UI's action. The `effectcraft-script` crate's docs
(`crates/script/src/lib.rs`) list the object model.

* Run one: File ▸ Scripts ▸ Run Script File…, the Script Console panel, `effectcraft-cli script
  file.jsx`, or `script.run {code, name}` over the control channel and MCP.
* Install one: File ▸ Scripts ▸ Install Script File… (or copy it into `Scripts/` in the settings
  folder). It is listed in File ▸ Scripts at once (After Effects asks for a restart).
* `file.scripts.list` lists installed and bundled scripts and panels; `file.runScript {name}` runs
  one; `file.uninstallScript {name}` removes an installed one.

## ScriptUI panels

A ScriptUI panel is a script that builds its user interface into the panel it is given as `this`.
Put it in `Scripts/ScriptUI Panels/` in the settings folder (or use File ▸ Scripts ▸ Install
ScriptUI Panel…) and it is listed at the bottom of the Window menu under its file name; choosing it
runs the script and docks its panel as a tab, which you can move, float, maximize and close like any
other panel. Closing the tab closes the panel (its `onClose` runs). Panels open when EffectCraft
quits open again at the next launch (`window.restoreScriptPanels`, which the desktop and web apps
run at start-up; the list is `scriptui_panels.json` in the settings folder).

Run from File ▸ Scripts instead, the same script gets no panel, so it should make a floating
palette itself. A minimal panel:

```js
// Nudge.jsx: moves the selected layers 10 px right, as a dockable panel.
(function (thisObj) {
  var ui = thisObj instanceof Panel ? thisObj : new Window("palette", "Nudge", undefined, { resizeable: true });
  ui.orientation = "column";
  ui.alignChildren = ["fill", "top"];
  var go = ui.add("button", undefined, "Nudge Right", { name: "nudge" });
  var status = ui.add("statictext", undefined, "Select layers, then click.");
  go.onClick = function () {
    var comp = app.project.activeItem;
    if (!(comp instanceof CompItem)) return;
    var layers = comp.selectedLayers;
    app.beginUndoGroup("Nudge Right");
    for (var i = 0; i < layers.length; i++) {
      var p = layers[i].transform.position;
      var v = p.value;
      if (p.numKeys === 0) p.setValue([v[0] + 10, v[1]]);
    }
    app.endUndoGroup();
    status.text = "Moved " + layers.length + " layer(s).";
  };
  if (ui instanceof Window) ui.show(); else ui.layout.layout(true);
})(this);
```

Supported controls: `panel`, `group`, `button`, `iconbutton`, `statictext`, `edittext`,
`checkbox`, `radiobutton`, `slider`, `scrollbar`, `progressbar`, `dropdownlist`, `listbox`,
`tabbedpanel` / `tab` and `image`, with ScriptUI's automatic layout (`orientation`,
`alignChildren`, `alignment`, `margins`, `spacing`, `preferredSize`), resource strings, event
handlers (`onClick`, `onChange`, `onChanging`, `onClose`, `addEventListener`) and `onDraw` with
ScriptUIGraphics. Mouse events (`mousedown`, `mousemove`…) aren't sent to scripts yet. Agents use
a panel like a user: `scriptui.list`, `scriptui.get` (its controls), `scriptui.click`,
`scriptui.set` and `scriptui.close`; every control has the automation id
`scriptui.<window>.<control id or name>`. The bundled sample `Layer Tools.jsx`
(`crates/engine/scripts/ScriptUI Panels/`) and the Ease Presets extension below are longer
examples.

## Bundled extensions

Optional tools that ship with EffectCraft but are not part of its core: they are scripts on the
public scripting API, like the third-party scripts After Effects users install, and are listed
with the bundled samples (`file.scripts.list` reports them as `extension`). A script of the same
name in your Scripts folders takes their place.

### Ease Presets (`extensions/scriptui-panels/Ease Presets.jsx`)

Window ▸ Ease Presets.jsx: easing curves kept by name and applied to pairs of neighbouring
selected keyframes (#254; it was a core panel in v0.6.0). A curve is the out side of a segment's
first key and the in side of its second, in the Keyframe Velocity dialog's terms: influence in
percent, and speed relative to the segment's average speed (1 = as fast as a straight line, 0 = at
rest), so one curve fits any duration and change of value.

* Click a preset to apply it to every pair of neighbouring selected keyframes of each selected
  property, in one undo step (per dimension, and along the motion path for spatial properties;
  the keys' other sides keep playing as they did). The keys stay selected, so you can try another.
* The drawing shows the working curve; type its numbers (Out % and Speed, In % and Speed) and
  click Apply, or click From Keys to read the curve between the first selected pair.
* Save keeps the working curve under the typed name (replacing a preset of that name); Rename
  and Delete act on the selected preset of your own. The twelve built-in presets can't change.
* User presets are kept with `app.settings` (section `Ease Presets`, key `userPresets`, in
  `script_settings.json` in the settings folder). Presets saved by the v0.6.0 core panel
  (`ease_presets.json`) move there the first time a later version loads its settings; the old
  file is left as it was.

It uses only After Effects' documented scripting API (`comp.selectedProperties`,
`Property.selectedKeys`, `keyInTemporalEase` / `keyOutTemporalEase`, `setTemporalEaseAtKey` with
`KeyframeEase`, `setInterpolationTypeAtKey`, `app.settings`, ScriptUI), so it is also a worked
example of a keyframe tool.

## Effect plug-ins

EffectCraft loads third-party effects through a small, versioned plug-in API (**API version 2**; WebAssembly ABI 1).
A plug-in effect behaves exactly like a built-in one: it is listed in Effects & Presets (and the
Effect menu) under its category, applied by id (`effect.apply {"effect": "org.example.posterize"}`)
or display name, its parameters are ordinary properties (keyframes, expressions, Effect Controls,
`prop.set`), and it is saved in projects by id.

There are two ways to write one:

| | WebAssembly plug-in | Rust plug-in |
|---|---|---|
| What | a `.wasm` (or `.wat`) module | a type implementing `effectcraft_effects::plugin::EffectPlugin` |
| Loaded by | Effect ▸ Load Effect Plug-in…, `effect.plugins.load {path \| folder}`, or the `Plug-ins` folder next to the settings at start-up | `register_plugin(Arc::new(MyEffect))` in an app that links EffectCraft's crates |
| Sandbox | yes: no imports (no files, clock or network), fuel-limited, at most 1 GiB of memory, deterministic floats | none (it is your code) |
| Platforms | desktop and CLI (the wasmi interpreter, crate feature `effectcraft-plugin/wasm`); not the web build | everywhere |

`effect.plugins.list` lists what is loaded: `{api, wasm, plugins: [{id, name, category, version,
author, source, params}]}`.

### The contract

* **Pixels** are premultiplied RGBA `f32` (0–1; values may exceed 1 in 32 bpc projects), row
  major, processed **in place**. The buffer is the layer's (after masks and the effects above),
  possibly at preview resolution (`scale` = buffer pixels per layer pixel) and padded.
* **Parameters** arrive flattened to `f64`s in the order they are declared: slider, angle,
  checkbox (0 / 1) and popup (0-based index) take one value, point two (already in **buffer
  pixels**), colour four (RGBA 0–1).
* **Time** is layer time in seconds.
* Rendering must be **deterministic** and may run on several threads at once (a WebAssembly
  plug-in gets one instance per thread).
* A failed render (an error code, a trap, running out of fuel) leaves the frame unchanged.

### The manifest

Every plug-in describes itself with a JSON manifest:

```json
{
  "api": 1,
  "id": "org.effectcraft.example.posterize-bands",
  "name": "Posterize Bands",
  "category": "Stylize",
  "version": "1.0.0",
  "author": "EffectCraft contributors",
  "description": "Quantizes luminance into tinted bands.",
  "params": [
    {"id": "levels", "name": "Levels", "type": "slider", "default": 4, "min": 2, "max": 64, "sliderMax": 16, "decimals": 0},
    {"id": "mix", "name": "Tint Amount", "type": "slider", "default": 50, "min": 0, "max": 100},
    {"id": "tint", "name": "Tint", "type": "color", "default": [1.0, 0.55, 0.2, 1.0]},
    {"id": "invert", "name": "Invert Bands", "type": "checkbox", "default": false}
  ]
}
```

* `id`: stable, reverse-domain; letters, digits, `.`, `_`, `-`. `ec.` is reserved for built-ins.
  Ids must be unique (a second plug-in with a taken id is refused).
* `category`: one of the Effects & Presets categories (`Blur & Sharpen`, `Stylize`, …) or a new
  one, which appears after the built-in categories.
* Parameter types: `slider` (`default`, `min`, `max`, optional `sliderMin`, `sliderMax`,
  `decimals`), `angle` (`default` degrees), `checkbox` (`default` bool), `popup` (`options`,
  `default` index), `point` (`default` as fractions of the layer size, `[0.5, 0.5]` = centre),
  `color` (`default` RGBA 0–1). Parameter ids are letters, digits and `_`, unique; at most 256
  parameters; a slider needs `min ≤ default ≤ max` (and `sliderMin ≤ sliderMax`); the manifest
  is at most 1 MiB.

### WebAssembly ABI (v1)

A module with **no imports** that exports:

| export | signature | |
|---|---|---|
| `memory` | memory | linear memory shared with the host |
| `ec_api_version` | `() -> i32` | must return `1` |
| `ec_manifest_ptr`, `ec_manifest_len` | `() -> i32` | the UTF-8 JSON manifest in memory |
| `ec_alloc` | `(bytes: i32) -> i32` | a buffer of at least `bytes` bytes, 8-byte aligned; it may reuse the previous one |
| `ec_render` | `(pixels: i32, width: i32, height: i32, params: i32, nparams: i32, time: f64, scale: f64) -> i32` | process the pixels in place; return 0 on success |

For each frame the host calls `ec_alloc` once for a buffer holding the parameters (`nparams`
little-endian `f64`s) followed by the pixels (`width × height × 4` little-endian `f32`s), fills it,
calls `ec_render`, and reads the pixels back. Each call gets a fuel budget proportional to the
frame size; a module that loops forever stops with an error instead of hanging the app.

#### Example

[`examples/plugins/posterize-bands`](../examples/plugins/posterize-bands) is a complete plug-in in
plain Rust (no dependencies):

```sh
cd examples/plugins/posterize-bands
cargo build --release --target wasm32-unknown-unknown
# then, in EffectCraft: Effect ▸ Load Effect Plug-in… → target/wasm32-unknown-unknown/release/posterize_bands.wasm
# or headless:
effectcraft-cli exec effect.plugins.load '{"path": "target/wasm32-unknown-unknown/release/posterize_bands.wasm"}'
```

Any language that compiles to WebAssembly works (C, Zig, AssemblyScript…) as long as the module
has no imports. `crates/plugin/src/tests.rs` has a plug-in written directly in WebAssembly text.

### Rust plug-ins

```rust
use std::sync::Arc;
use effectcraft_effects::plugin::*;

struct Swap(PluginManifest);

impl EffectPlugin for Swap {
    fn manifest(&self) -> &PluginManifest { &self.0 }
    fn render(&self, frame: &mut PluginFrame, params: &PluginParams, _time: f64) -> Result<(), String> {
        if params.b("on") {
            for p in frame.pixels.iter_mut() { p.swap(0, 2); }
        }
        Ok(())
    }
}

register_plugin(Arc::new(Swap(manifest)))?; // before the UI builds its menus
```

`PluginParams` reads values by parameter id (`f`, `b`, `point`, `color`, and in API 2 `point3`,
`layer`, `text`). At most 1024 plug-in effects can be registered per process.

## Plug-in API 2 (native plug-ins)

API 2 is for Rust plug-ins and hosts like the OpenFX bridge (`effectcraft-ofx`, see
[`ofx.md`](ofx.md)); the WebAssembly ABI stays at 1. Manifests with `"api": 1` or `2` are accepted,
`effect.plugins.list` reports `api: 2`, and up to 1024 plug-in effects can be registered.

* **New parameter types**: `point3` (`default` `[x, y, z]`; x, y arrive in buffer pixels, z as is),
  `layer` (another layer of the composition; flattens to its id, -1 for none, read with
  `PluginParams::layer`), `text` (a string field) and `hidden` (a string that is not shown, for
  opaque plug-in data). `text` / `hidden` take no slot in `flat`; their strings are in
  `PluginParams::texts`, one per such parameter in declaration order, read with
  `PluginParams::text(id)`. `slider` gains `"integer": true` (shown with 0 decimals) and every
  parameter an optional `"group"` path (`"Advanced/Edges"`; informational). WebAssembly modules
  can't use `point3`, `layer`, `text` or `hidden`.
* **`PluginFrame`** also carries `origin` (buffer pixels of layer pixel (0, 0); padding shifts it)
  and `layer_size` (layer pixels).
* **`EffectPlugin::padding(params, time, scale, layer_size, instance_key) -> u32`** asks for that
  many transparent buffer pixels on every side before rendering (glows, regions of definition
  larger than the input); at most 4096. `layer_size` is the layer's size in layer pixels (so
  sizes relative to the frame can be measured) and `instance_key` identifies the applied effect
  instance (see `PluginHost::instance_key`).
* **`EffectPlugin::render_v2(frame, params, time, host)`** (default: calls `render`) gets a
  `PluginHost` with `input_at(layer_time)` (the effect's input at another time, in its own extent;
  the effects above this one are applied), `layer_input(param_id, layer_time)` (a layer parameter's
  layer, effects and masks applied, resampled into the frame's grid), `params_at(layer_time)`,
  `frame_rate()`, `time()`, `instance_key()` (a stable id of the applied effect instance: the
  same on every frame, different per layer and per application; plug-ins that keep state between
  frames pool their native instances by it) and `frame_range()` (the layer's source time range in
  layer-time seconds, `None` when it has none). Images are `PluginImage { width, height, pixels, origin, scale }`.
  It may run on several threads at once.
* **Loading OpenFX plug-ins** (builds with the `openfx` feature, see [ofx.md](ofx.md)): Effect ▸
  Load OpenFX Plug-in…, `effect.plugins.loadOfx {path}` (an `.ofx` binary, an `.ofx.bundle` or a
  folder), or `effect.plugins.scanOfx` (the standard OpenFX folders plus `OFX_PLUGIN_PATH`; run at
  start-up unless `EFFECTCRAFT_NO_OFX=1`).

### Compatibility

The API version is checked when a plug-in loads: a manifest or module for another version is
refused with a message. Within a version the contract above does not change; additions (new
parameter types, new optional exports) bump the version.
