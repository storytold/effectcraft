# OpenFX plug-ins

EffectCraft can load [OpenFX](https://openfx.readthedocs.io/) (OFX) image-effect plug-ins and use
them as ordinary effects: commercial suites that ship as OFX (for example RE:Vision ReelSmart Motion
Blur and Twixtor, Boris FX Sapphire) and the free OFX collections run alongside the built-in
effects. OFX is the open plug-in standard of Nuke, DaVinci Resolve, Vegas, Natron and others; its
API headers are BSD-3-Clause.

An OFX effect behaves like any other EffectCraft effect: it appears in Effects & Presets under the
category the plug-in declares, its parameters show up in Effect Controls, they take keyframes and
expressions, and it can be applied by id from the control protocol
(`effect.apply {"effect": "<id>"}`). The id is the OFX identifier, lower-cased (for example
`org.effectcraft.test.invert`).

## Opt-in: the `openfx` feature

OFX plug-ins are native libraries that run inside the app's process, **without a sandbox**, unlike
EffectCraft's own WebAssembly plug-ins ([plugins.md](plugins.md)). Hosting them also needs `unsafe`
Rust to call the C interface, which the rest of the workspace forbids. So the host is a separate
crate (`crates/ofx`, `effectcraft-ofx`), the only one allowed `unsafe`, and it is **off by
default**: builds only include it with the `openfx` feature.

```sh
cargo run -p effectcraft --features openfx           # the desktop app
cargo build --release -p effectcraft -p effectcraft-cli --features openfx
```

Without the feature the OFX commands are disabled ("OpenFX plug-ins are not available in this
build") and nothing native is loaded. There is no OFX on the web build.

## Installing OFX plug-ins

Install the plug-in with its own installer. OFX plug-ins live in `.ofx.bundle` folders in standard
locations, and EffectCraft looks in the same places as other hosts:

| OS | Standard folder |
|---|---|
| Windows | `C:\Program Files\Common Files\OFX\Plugins` |
| macOS | `/Library/OFX/Plugins` |
| Linux | `/usr/OFX/Plugins` |

In addition, every folder listed in the `OFX_PLUGIN_PATH` environment variable is searched
(separated by `;` on Windows, `:` elsewhere). Folders are scanned recursively for bundles.

Nothing needs configuring: at start-up the desktop app scans these folders (the
`effect.plugins.scanOfx` command) and registers everything it finds. Large suites register many
effects (Sapphire around 290); loading them takes a few seconds.

## Loading a plug-in by hand

* **Effect ▸ Load OpenFX Plug-in…** asks for an `.ofx` binary (inside `X.ofx.bundle/Contents/<platform>`).
* Control protocol: `effect.plugins.loadOfx {"path": "..."}` loads an `.ofx` binary, an `.ofx.bundle`
  folder or a folder to scan, and returns the loaded effects and any errors;
  `effect.plugins.scanOfx {}` rescans every default location and `OFX_PLUGIN_PATH` and returns
  `{paths, loaded, errors}`.
* To skip the start-up scan (for example to rule OFX out when something misbehaves) set the
  environment variable `EFFECTCRAFT_NO_OFX=1`.

## A plug-in that crashes while loading

A native crash inside a plug-in (in its library start-up code, `Load`, `Describe` or
`DescribeInContext`) takes the whole app down, and since the standard folders are scanned at every
start-up, one bad plug-in would make EffectCraft crash on every launch. So, like other OFX hosts,
EffectCraft keeps a **crash blocklist** in its config folder (`%APPDATA%\EffectCraft` on Windows,
`~/Library/Application Support/EffectCraft` on macOS, `~/.config/effectcraft` on Linux, or
`EFFECTCRAFT_CONFIG_DIR`):

* Before it opens a plug-in binary the process writes the binary's path to `ofx_loading.<pid>.txt`,
  keeps that file locked, and deletes it once the binary has been loaded and described
  (successfully or with an ordinary error).
* If the process dies in between, the file is still there and its lock is gone. The next process
  that loads plug-ins moves the path to `ofx_blocklist.txt` (one path per line) and the binary is
  **skipped** from then on. The skip shows up in the loader's `errors` ("... skipped: it crashed
  EffectCraft while loading last time ...") and on stderr at start-up. The rest of the plug-ins load
  normally. A marker that is still locked belongs to another EffectCraft process (the CLI next to
  the app) that is loading right now, and is left alone.
* To try a blocklisted plug-in again (after updating or re-licensing it), run
  `effect.plugins.scanOfx {"retry": true}` from the control protocol, or delete its line (or the whole
  `ofx_blocklist.txt`) and restart. A plug-in that crashes again is simply blocklisted again.

This only covers loading. A crash while an effect is rendering is not caught and is not
blocklisted; if that keeps happening, start with `EFFECTCRAFT_NO_OFX=1` or uninstall the plug-in.

## How OFX maps onto EffectCraft

* Contexts: Filter is preferred, then General, Transition and Generator. A General effect's extra
  input clips (every clip except `Source`) become **Layer** parameters named after the clip: pick the
  layer to use as that input. A Transition's `SourceTo` clip works the same way.
* Parameters: doubles and integers become sliders, booleans checkboxes, choices popups, RGBA colours,
  strings text fields; custom parameters carry opaque plug-in data and are hidden. Groups and pages
  are folded into parameter names.
* 2D and 3D doubles follow their OFX double type. Only position values (type XY or XY absolute)
  become an on-screen point: a 2D position a point, a 3D position a 3-value point. Every other 2D or
  3D double, including the plain default, becomes one slider per component, named `<name>_x`,
  `_y` and `_z`. A plug-in that wants a viewer point for a value must declare it as a position.
* The effect's output may be larger than its input (glows, blurs, borders); EffectCraft pads the
  layer buffer by the plug-in's region of definition before rendering, measured on the real layer.
* Pixels are float RGBA, premultiplied. Plug-ins that only support 8- or 16-bit are converted.
* Temporal plug-ins (motion blur, echo, retiming) can ask for the input at other times and for other
  layers; EffectCraft renders those on demand. Each applied effect gets its own pool of OFX
  instances, so plug-ins that keep state between frames never mix two layers up.

## Retimers (Twixtor)

Time-remapping plug-ins such as RE:Vision **Twixtor** (OFX version) work. Twixtor offers the Filter
and General contexts; in General its optional extra inputs (motion vectors, foreground matte) are
Layer parameters like any other extra clip.

* `paramGetIntegral` and `paramGetDerivative` work for double, 2D / 3D double and colour parameters,
  so a plug-in can integrate an animated Speed % curve to find the source frame. They sample the
  animated value through the host: the derivative is a central difference over one frame, the
  integral composite Simpson with steps of at most one frame (at most 4096 steps per call, so ranges
  longer than that are coarser). Every sample evaluates the effect's parameters, so integrating from
  frame 0 late in a long clip costs time on every render. Outside a render the value is constant.
  Integer, choice and string parameters return `kOfxStatErrUnsupported`.
* The source clip's `FrameRange` / `UnmappedFrameRange`, the effect duration and the timeline
  suite's bounds are the layer's real source range (footage and nested comps: their duration; Time
  Remap layers: the visible span), converted with the comp frame rate. Layers without a duration
  (stills, solids, text, shapes) keep the fallback of frames 0 to 99999.
* `clipGetImage` on the source clip accepts fractional times (frame 12.5 renders the layer at that
  time; footage shows the frame the media layer picks for it, blended if the layer's Frame Blending
  is on) and fails with `kOfxStatFailed` outside the frame range instead of inventing a frame.
* Limits: no GPU, no on-screen overlays (the speed graph and motion-vector tools are unavailable), no
  host-supplied motion vectors (Twixtor's own optical flow runs on the CPU and is slow at high
  resolution). The extra clips' frame ranges are assumed to equal the source's.

## Licensing

EffectCraft does not include, license or activate any third-party plug-in.

* Commercial plug-ins need their own OFX licences. Without one they render a watermark or report an
  "unlicensed" status; vendors often sell OFX licences separately from their After Effects ones.
  Licence activation is done with the vendor's own tools.
* The host identifies itself to plug-ins as "EffectCraft". It is **not a supported host** for any
  vendor: do not ask the vendor for support, and a plug-in whose licence is tied to a list of known
  hosts may refuse to run.

## Known limitations

* **No GPU.** Plug-ins are rendered on the CPU through the OFX CPU API. Plug-ins that only render
  with OpenGL, CUDA or Metal fall back to their CPU path, or fail to load if they have none.
* **No on-screen overlays.** The plug-ins' viewer widgets (handles, tracking boxes) do not appear;
  use the parameters instead. Push buttons that open dialogs do nothing.
* **Native code runs in-process.** An OFX plug-in is a native library with full access to the
  machine. A bug in one can crash EffectCraft, and a malicious one can do anything the app can.
  Only install plug-ins you trust. (This is why the host is opt-in; EffectCraft's own plug-ins are
  sandboxed WebAssembly.)
* **Glows on transparent layers.** Some plug-ins (Sapphire's glows) add light without adding alpha,
  so on a layer with transparent areas the glow outside the opaque pixels disappears. On footage, or
  a precomp with a background, it shows as in other hosts.
* Desktop only: there is no OFX on the web build. Tested on Windows x64 with RSMB 7, Twixtor 8 and
  Sapphire 2026.5; the macOS and Linux loaders are implemented but untested with commercial plug-ins.
* Plug-in messages (errors, licence notices) are formatted with the common printf conversions (`%s`,
  `%d`, `%u`, `%x`, `%c`, `%p`, `%f`, `%e`, `%g`, with width and precision) and at most 8 arguments;
  anything else is shown as written. On Apple Silicon the arguments cannot be read and the raw
  format string is shown.
* Some plug-ins rely on host features that are not implemented (analysis passes, multi-threaded
  frame rendering, custom interpolation, host-side clip caching). Those plug-ins may load with
  reduced functionality or report errors in the loader's `errors` list.

## How it fits together

* **Plug-in API 2** (`effectcraft_effects::plugin`, [plugins.md](plugins.md#plug-in-api-2-native-plug-ins))
  is what the host is built on: more parameter kinds (3D points, layers, text, hidden data), padding
  for regions of definition larger than the layer, and a `PluginHost` with the effect's input at other
  times (`input_at`), other layers (`layer_input`), parameters at other times (`params_at`), the frame
  rate, the source's frame range and a stable per-instance key. WebAssembly plug-ins keep ABI 1.
* **`crates/ofx`**: `ffi` (the C ABI transcribed from the OpenFX headers), `props` (property sets),
  `params` (parameter storage and the mapping above), `clips` (images and pixel conversion; OFX is
  y-up, rows are flipped on the way in and out), `instance` (effect handles, geometry, the
  per-render context and actions), `effect` (the `EffectPlugin` implementation), `suites`
  (property, parameter, image effect, memory, multithread, message, progress, timeline, interact),
  `loader` (bundle discovery and registration) and `blocklist`.
* **Engine / hosts**: `Session::ofx_loader`, `ofx_search_paths` and `ofx_clear_blocklist` are set by
  `effectcraft-host` with the `openfx` feature; the commands are `effect.plugins.loadOfx` and
  `effect.plugins.scanOfx`; the desktop app runs the scan at start-up.

## Testing the host

`examples/ofx/ec-test-ofx` is a small dependency-free OFX plug-in (Invert, Echo, Border, Mix and an
effect with every parameter type) that exercises the host. The integration tests in
`crates/ofx/tests` load it; build it into a bundle first:

```sh
cargo build --release --manifest-path examples/ofx/ec-test-ofx/Cargo.toml
# lay out the bundle (Windows shown; use Contents/Linux-x86-64 and libec_test_ofx.so on Linux)
mkdir -p target/ofx-test/ec-test-ofx.ofx.bundle/Contents/Win64
cp examples/ofx/ec-test-ofx/target/release/ec_test_ofx.dll target/ofx-test/ec-test-ofx.ofx.bundle/Contents/Win64/ec-test-ofx.ofx
EC_TEST_OFX_BUNDLE=$PWD/target/ofx-test/ec-test-ofx.ofx.bundle cargo test -p effectcraft-ofx
```

Without `EC_TEST_OFX_BUNDLE` the integration tests print a note and pass without testing anything
(the unit tests always run). You can also put the bundle in a standard OFX folder or on
`OFX_PLUGIN_PATH` to see the "EC Test" effects in an `openfx` build of the app.
