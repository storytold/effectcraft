<h1 align="center">EffectCraft Mobile</h1>

<p align="center">
  <b>Motion graphics and visual effects on Android: an arm64 build of EffectCraft, the open-source, clean-room reimplementation of Adobe After Effects, rebuilt in pure Rust.</b>
</p>

<p align="center">
  A fork of <a href="https://github.com/storytold/effectcraft">storytold/effectcraft</a> that adds an
  Android build. The engine, panels and commands are upstream's, unchanged; what is new here is a
  NativeActivity entry point, storage that a phone actually has, and file dialogs that work without
  a system picker.
</p>

<p align="center">
  <img alt="Status: young and moving fast" src="https://img.shields.io/badge/status-young%20and%20moving%20fast-e0368f?style=flat-square">
  <img alt="Written in Rust" src="https://img.shields.io/badge/rust-1.95%2B-b0206c?style=flat-square&logo=rust&logoColor=white">
  <img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-555?style=flat-square">
  <img alt="Platform: Android arm64" src="https://img.shields.io/badge/platform-Android%20arm64-3ddc84?style=flat-square&logo=android&logoColor=white">
</p>

<p align="center">
  <a href="#android">Android</a> ·
  <a href="#what-effectcraft-is">What it is</a> ·
  <a href="#animate">Animate</a> ·
  <a href="#effects">Effects</a> ·
  <a href="#3d">3D</a> ·
  <a href="#export">Export</a> ·
  <a href="#get-started">Get started</a> ·
  <a href="#where-it-stands">Status</a> ·
  <a href="#license-and-credits">License</a>
</p>

<br>

<p align="center">
  <img src="docs/images/effectcraft-hero.png" alt="EffectCraft's main window: the animated demo composition in the Composition panel, the Project panel, a Timeline with text, shape and solid layers, and the Properties panel showing the selected text layer's transform, font and paragraph settings" width="100%">
</p>

## Android

This fork exists for the phone. Everything below describes the same application on every platform;
this section is what is different about running it on a device.

- **It is the desktop app, not a port of it.** `apps/effectcraft-android` is a NativeActivity entry
  point that starts the same library the desktop and web builds use, so the engine, the panels and
  the command registry are shared rather than reimplemented.
- **Save, open and export on the device.** Android has no system file picker for this app (rfd has
  no backend there), so instead of failing with "no file dialog available" it serves the dialogs
  itself: projects save and open in `Projects/`, renders and stills land in `Exports/`, settings
  and auto-save state live in `config/`, all below the app-private storage directory. The Save
  prompts carry a Location row, and the browser can create folders.
- **Shared storage when you allow it.** The browser can reach the phone's internal storage
  (`/storage/emulated/0`) once **All files access** is granted. That is a special permission an app
  can never hold silently, so the browser opens the system settings page for it and the app asks
  again afterwards. Without it everything still works, inside the app-private directory.
- **Landscape, arm64, Android 9 (API 28) and up**, packaged as `ai.storyteller.effectcraft` with
  EffectCraft's own launcher icon at five densities.
- **Touch:** a menu taller than the screen scrolls when you drag it. On a phone in landscape the
  Edit menu has 34 rows and room for 21, so this is the difference between reaching Preferences
  and not. Panels that lay out their own rows (Effects & Presets, Properties, Effect Controls,
  Preview, Info, the media and render-queue lists…) scroll when you drag their empty space, and
  every scroll bar — including the timeline's layer list — drags under a finger: its track is
  4 points wide with a 20-point target around it.

### Building the APK

You need the [Android SDK and NDK](https://developer.android.com/studio) and
[`cargo-apk`](https://crates.io/crates/cargo-apk):

```sh
cargo install cargo-apk
cargo apk build --release          # target/release/apk/effectcraft.apk
adb install -r target/release/apk/effectcraft.apk
```

`cargo apk` reads `[package.metadata.android]` in `apps/effectcraft-android/Cargo.toml`: the
package id, the landscape orientation, the `MANAGE_EXTERNAL_STORAGE` permission and the launcher
icons in `apps/effectcraft-android/res/`, which `packaging/icons.sh` regenerates from the app's own
`effectcraft.svg`.

## What EffectCraft is

EffectCraft is for animated titles, motion graphics and compositing work: the kind of thing
people reach for After Effects to do. You build a composition out of layers (solids, shapes,
text, footage, other compositions), animate their properties with keyframes, stack effects on
them and render the result.

The aim is to feel familiar to anyone who has used After Effects, with the same panels (Project,
Composition, Timeline, Effect Controls, Effects & Presets) and the same keyframe behaviour, and
then to go further in a few places where it matters to us:

- **Lottie import and export built in**, so animations can go straight to the web and apps
  without a plugin.
- **A project file you can read.** Projects are versioned JSON (`.ecproj`), so they diff cleanly
  in version control.
- **Everything is scriptable.** Every menu item, timeline drag and property edit goes through one
  command registry, so the same actions are reachable from a command line, a JSON control
  channel and an MCP server for agents.
- **No FFmpeg.** Video and audio decoding and encoding are pure Rust: FilmCraft's codecs and
  EffectCraft's own VP9, AV1, HEVC and Opus encoders.

## Animate

The panels, menus and shortcuts follow After Effects, so your muscle memory carries over:
Project, Composition, Timeline, Effect Controls, Properties, Effects & Presets, Character,
Paragraph, Align, Info, Preview, Audio and the Render Queue, docked the way you expect. Drag
footage, comps and effects between them: onto the comp viewer, where they land under the pointer,
or into the Timeline, between layers and at the time you point to.

- **Layers of every kind:** solids, shapes, text, footage, nested compositions, nulls,
  adjustment layers, cameras and lights; parenting, track mattes, all 38 blend modes, motion blur.
- **Keyframes that behave the same:** linear, Bezier, hold, auto and continuous Bezier, roving
  keys, Easy Ease (F9), Keyframe Velocity and Interpolation dialogs, copy and paste at the current
  time, and a **Graph Editor** with value and speed graphs and draggable handles.
- **Time:** time remapping, time stretch, time-reverse, freeze frame, work area, markers, exact
  frame-accurate timing at every frame rate including 29.97 drop-frame.
- **Shapes and masks:** shape layers with trim paths, repeaters, round corners, offset, zig zag,
  twist, wiggle, merge paths and gradient strokes; masks drawn with the pen tool, with modes,
  feather, expansion and vertex editing in the viewer.
- **Text:** point and paragraph text with real shaping in any font installed on your machine, the
  Character and Paragraph panels, and text animators with range selectors.
- **Expressions:** JavaScript with the After Effects object model (`wiggle`, `loopOut`,
  `thisComp.layer("…")`, vector maths on arrays), an inline editor and the pick-whip.
- **Timeline like you know it:** twirl layers open to Transform, masks, effects and the rest; drag
  rows to reorder layers; rename with Enter or a double-click; the property shortcuts (A, P, S,
  R, T, M, F, E, L, U and their double presses AA, PP, SS, RR, TT, MM, FF, EE, LL, UU), Shift to
  add properties, Alt+Shift+A/P/S/R/T to key the current time, Ctrl+` to twirl selected layers.
- **Puppet tools:** Position, Advanced, Bend, Starch and Overlap pins on a mesh built from the
  layer, real-time pin recording, rotate/scale handles, pins driven by (or carrying) nulls for
  rigging, and follow-through for hair and cloth.

<p align="center">
  <img src="docs/images/effectcraft-graph-editor.png" alt="The Graph Editor showing an eased value curve for a text animator's Range Start, with the graph editor's button bar for hold, linear, auto Bezier and Easy Ease" width="100%">
</p>

## Effects

306 effects (every one of After Effects' 298, and more) across its categories, each with its parameter names, order and
defaults: blur and sharpen, channel, color correction (Curves, Levels, Hue/Saturation, Lumetri
Color…), distort (Warp, Bulge, Turbulent Displace, CC Power Pin…), generate (Fractal Noise,
Gradient Ramp, Stroke, Write-on, Audio Spectrum…), keying, matte, noise and grain, perspective,
**simulation** (CC Particle World, CC Rainfall, Shatter, Card Dance, Caustics, Wave World…),
stylize (Glow, CC Glass…), **time** (Echo, Posterize Time, Timewarp, Time Displacement…),
**audio** (Reverb, Parametric EQ, Delay, Stereo Mixer…), text, transitions, utility and expression
controls. All nine **Layer Styles** (Drop Shadow, Inner/Outer Glow, Bevel and Emboss, Satin,
overlays, Stroke) with Global Light. Preview plays audio in sync, with meters and waveforms.

<p align="center">
  <img src="docs/images/effectcraft-effects.png" alt="A CC Particle World burst with Glow, and the Effect Controls panel listing the particle system's parameters" width="100%">
</p>

## 3D

Classic 3D the way After Effects does it: 3D layers with orientation and material options,
one- and two-node **cameras** with depth of field, **lights** (parallel, spot, point, ambient)
with soft ray-traced shadows, layers that intersect correctly, orbit, pan and dolly camera
tools, and Front, Top, Left and Custom views.

<p align="center">
  <img src="docs/images/effectcraft-3d.png" alt="The 3D Showcase demo in Custom View 1: intersecting cards lit by a spot light with soft shadows on a gridded floor, with the camera frustum and light drawn as wireframes" width="100%">
</p>

## Export

A Render Queue like After Effects', with Render Settings and Output Modules: **H.264** MP4 and
**ProRes** MOV (Proxy to 4444 XQ with alpha) with audio, **HEVC** (Main / Main 10) and **AV1**
MP4, **WebM** (VP9 with inter frames and alpha, or AV1; Opus audio), PNG, JPEG, TIFF and 32-bit EXR sequences, animated GIF and WAV/AIFF. Field
rendering with 3:2 pulldown, effect/solo/guide/depth overrides, crop, region of interest and
resize, Render Settings and Output Module templates with defaults, post-render actions, storage
overflow and render logs. The same queue runs from the command line. Every encoder is pure
Rust (FilmCraft's H.264, ProRes and AAC; EffectCraft's own VP9, AV1, HEVC and Opus); there is no
FFmpeg inside.

**Lottie** goes both ways: File ▸ Export ▸ Lottie JSON… writes a composition (precomps, shape,
solid, image, text and null layers, eased and spatial keyframes, masks, track mattes, blend
modes, time remapping, optionally expressions) as `.json` or `.lottie`, and lists anything Lottie
cannot express; File ▸ Import ▸ Lottie… opens one as a new composition.

<p align="center">
  <img src="docs/images/effectcraft-render-queue.png" alt="The Render Queue panel with two compositions queued" width="100%">
</p>

## Get started

**On Android:** download the APK from this repository's
[Releases](https://github.com/Kaedo17/effectcraft-mobile/releases) page and install it, or build it
yourself as described under [Android](#building-the-apk).

**On desktop and in the browser:** the upstream project ships installers for every platform on
its [Releases](https://github.com/storytold/effectcraft/releases) page — a universal macOS app;
Windows MSIs and portable zips; Linux AppImage, Flatpak, deb, rpm and tar.gz; a FreeBSD tarball;
and the web build. This fork does not rebuild them.

To build the desktop app yourself you need [Rust](https://rustup.rs/) 1.95 or newer.

```sh
git clone https://github.com/storytold/effectcraft
cd effectcraft
cargo run --release -p effectcraft          # the app (add `-- --demo` to open the demo project)
cargo run --release -p effectcraft-cli -- render --out intro.mp4    # render the demo headless
cargo xtask web --serve 8765                # the browser build on http://127.0.0.1:8765/ (docs/web.md)
```

New to it? [docs/footage.md](docs/footage.md) covers importing footage and image sequences and
Interpret Footage.

To work on it: `cargo test --workspace`, and `cargo xtask ci` before every commit (format, lints,
tests, layering, asset attribution and the WebAssembly build). See [CONTRIBUTING.md](CONTRIBUTING.md),
[docs/architecture.md](docs/architecture.md) and [docs/testing.md](docs/testing.md). On Linux the
CI needs the ALSA development package; `cargo xtask ci` names the exact one for your distribution
rather than failing at link time.

## Where it stands

EffectCraft is young: its first commit was on 1 October 2026. Nearly every After Effects feature
exists, including all of its effects, but existing is not the same as behaving exactly like After
Effects, and we haven't measured that yet. Today it's a good place to try things and to tell us
what breaks. It isn't yet a replacement for After Effects on client work. In particular:

- **After Effects projects can't be opened.** Projects are saved as `.ecproj`, readable versioned
  JSON, but EffectCraft can't open `.aep` / `.aepx` files, and After Effects plug-ins don't run.
- **Behaviour still differs from After Effects in places.** Nothing yet compares our renders with
  After Effects automatically, so please report differences.
- **The Android build is a preview.** It is one platform (arm64), one orientation (landscape) and
  one storage model, against a desktop app that is still finding its feet. There is no system
  file picker, no keyboard-shortcut mapping worth the name on a touchscreen, and the desktop UI
  is not yet laid out for a small screen. It is the same engine, though, and the same commands.
- **macOS is the most tested platform.** Linux and Windows users have hit basic interaction
  bugs, which we're fixing first.
- **The trained models are new.** Roto Brush can use MobileSAM (an optional 40 MB download in
  Settings ▸ Roto Brush) and face tracking can use Google's MediaPipe Face Landmarker (an optional
  3.8 MB download in Settings ▸ Face Tracking). Neither is compared with After Effects yet.

EffectCraft is in **alpha**: about 91% of After Effects' features exist, and we estimate it is
about 45% of the way to replacing After Effects for professional work. The [ROADMAP](ROADMAP.md)
has the plan, [docs/target-app-parity.md](docs/target-app-parity.md) the measurement and
[docs/gaps.md](docs/gaps.md) every known shortfall. The web build ([docs/web.md](docs/web.md)) runs the full app in the browser.
[Bug reports](https://github.com/storytold/effectcraft/issues) are the most useful thing you can
send us right now; reports about the Android build belong
[here](https://github.com/Kaedo17/effectcraft-mobile/issues).

## How it's made

- **Engine first.** A stack of small crates with strict layering. The interface (egui) sits on
  top and nothing underneath depends on it.
- **Clean room.** We work from After Effects' public documentation and how it behaves, never
  from Adobe's files, icons, presets or code. No code is copied from GPL projects. The full rules
  are in [AGENTS.md](AGENTS.md).
- **Private by default.** No telemetry, and no network access unless you ask for it.
- **One family.** EffectCraft shares its time model and text engine design with FilmCraft, and
  gets its video and audio codecs from it.

## License and credits

EffectCraft is dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
Copyright (c) 2026 ArtCraft Team and the EffectCraft contributors. Required notices are in [NOTICE](NOTICE).

Bundled fonts, icons, images and other assets keep their own open licenses; each one is listed
with its author, source and license in [ATTRIBUTION.md](ATTRIBUTION.md).

EffectCraft is made by the ArtCraft team and their community, at
[getartcraft.com](https://getartcraft.com/). This fork adds the Android build and nothing else;
the upstream project, its releases and its desktop platforms are all still theirs. The ArtCraft
name and marks belong to the ArtCraft Team and are not included in this fork.

<sub>Adobe, Photoshop, Illustrator, Premiere Pro, Lightroom, Acrobat, After Effects and InDesign are trademarks or registered trademarks of Adobe Inc. in the United States and/or other countries. EffectCraft is an independent, open-source project and is not affiliated with, sponsored by or endorsed by Adobe Inc.; these names are used only to describe the workflows it is compatible with.</sub>

## Star history

[![Star History Chart](https://api.star-history.com/svg?repos=Kaedo17/effectcraft-mobile&type=Date&legend=top-left)](https://www.star-history.com/?repos=Kaedo17%2Feffectcraft-mobile&type=date&legend=top-left)
