# EffectCraft on Android

`apps/effectcraft-android` runs the same engine and egui UI as the desktop app on an Android
tablet: one Rust `cdylib` (`libeffectcraft_android.so`) packaged into an APK by `cargo apk` and
started as a plain `android.app.NativeActivity`. There is no Java, no Kotlin and no Gradle —
`winit`'s `android-activity` glue owns the window, input and lifecycle, and eframe drives the
frames. The manifest is tablet-first: landscape in both directions (`sensorLandscape`),
resizable for split-screen, fullscreen, and rotation or resize re-lays the UI out instead of
restarting the activity.

This is the first slice: the app builds, installs and runs — Home screen, templates, the
Composition Settings dialog, the workspace with the Project, Composition, Preview, Properties
and Timeline panels, all driven by touch. What it does *not* have yet is at the bottom.

## Build and run

| Tool | Version | Install |
|---|---|---|
| Rust target | `aarch64-linux-android` (devices), `x86_64-linux-android` (emulator) | `rustup target add aarch64-linux-android x86_64-linux-android` |
| Android SDK | `build-tools` ≥ 34, `platforms` | `sdkmanager "build-tools;36.0.0" "platforms;android-34" "platform-tools"` |
| Android NDK | any recent (r26+) | `sdkmanager "ndk;28.2.13676358"` |
| cargo-apk | 0.10 | `cargo install cargo-apk --locked` |

```sh
cargo xtask android --target x86_64-linux-android --install --launch   # emulator (x86_64)
cargo xtask android --install --launch                                 # device (arm64)
cargo xtask android --release                                          # optimised APK
```

`ANDROID_HOME` locates the SDK and the NDK (`ANDROID_NDK_ROOT` wins if set; otherwise the newest
`<sdk>/ndk/<version>` is used). The APK lands in `<target>/<profile>/apk/effectcraft.apk`, signed
with the debug keystore (`~/.android/debug.keystore`), package `ai.storyteller.effectcraft` —
the desktop app's id — activity `android.app.NativeActivity`. `--install` and `--launch` drive
`adb`; without them the task only builds. A target-tagged copy (`effectcraft-<triple>.apk`)
sits beside it, so a device (arm64) build no longer overwrites an emulator (x86_64) one.

The task sets `CARGO_INCREMENTAL=0`: incremental's hardlinked artifacts corrupt easily on
network filesystems and WSL (`corrupt incremental compilation artifact … dep-graph.bin`, then
`E0463: can't find crate for idna_adapter` / `crate ndk required to be available in rlib
format`), and Android target dirs link slowly anyway.

`cargo xtask layers` assigns the crate to L6 like the other apps, and `cargo xtask ci` keeps it
building on the host: the shell itself is Android-only, while the in-app browser and the preview
audio it wires up are platform-neutral, so the gate lints and tests them on the desktop too.

## What the Android shell does differently

| Desktop | Android |
|---|---|
| `rfd` file and message dialogs | in-app browser (`Hooks::file_dialog_async` in `apps/effectcraft-android/src/file_browser.rs`): the workspace's own internal storage plus shared `Download`/`DCIM` when readable; the commands stay drivable with `paths`/`path` parameters too, as on the web |
| `cpal` preview audio | wired to AAudio through cpal (`src/audio_out.rs`, same design as the desktop app); needs API 26 (Android 8.0), so `min_sdk_version` is 26 |
| config directory (`~/…/EffectCraft`) | the app's own internal storage, `<internal data>/EffectCraft` (settings, shortcuts, auto-saves, models, `Plug-ins`, `Logs/`), from `AndroidApp::internal_data_path` — there is no `HOME` on Android |
| window geometry tracker, macOS menu bar, launch guard | not needed: one full-screen window, no menu bar |
| `WGPU_BACKEND` launch guard, AccessKit | Vulkan is preferred; AccessKit's winit glue has no `NativeActivity` backend, so `effectcraft-ui-egui` drops that feature for `target_os = "android"` (it needs `android-game-activity`, i.e. Java) |
| window over the desktop | the native window covers the whole display, system bars included: `AndroidApp::content_rect` reports what the tablet's taskbar leaves (`1920×1080` on a 1920×1200 tablet), and `Shell::fit_to_content` lays the egui UI out inside it, so nothing hides behind the bars |

**Graphics.** `gpu_backends` probes the Vulkan adapter before the window opens: when it is a
software rasteriser (SwiftShader in an emulator, llvmpipe) the window uses the OpenGL ES path
instead — on the emulator we test on, SwiftShader's Vulkan dies on our pipelines and takes the
emulator down with it, while its GL path runs them. `<internal data>/EffectCraft/gpu-backend`
(`gl` or `vulkan`) forces either. On the GL path a device without compute shaders (the
emulator's OpenGL ES 3.0 SwiftShader) also switches the compositor to the CPU renderer, which is
the same fallback an old desktop GPU gets.

**Input.** Touch is egui's pointer: tap to click, drag to scrub and to move layers. The soft
keyboard shows when a text field is focused: egui's `TextEdit` reports IME interest, egui-winit
forwards it to `Window::set_ime_allowed`, and winit 0.30's Android backend calls
`AndroidApp::show_soft_input` (`"On Android, the soft keyboard can now be shown using
Window::set_ime_allowed"`, winit 0.30 changelog). A hardware keyboard still works where one
is attached.

## Known gaps (first slice)

- **File dialogs**: the in-app browser covers internal storage and shared folders when
  readable; there is no system picker (that needs Java) and no runtime permission request yet,
  so shared storage outside `Download`/`DCIM` may be unreadable — type the path or copy the
  file into the app's folder in that case.
- **Preview audio**: wired through cpal/AAudio (API 26+); untested on a physical tablet, and
  there is no Bluetooth-route picker yet (Settings ▸ Audio lists what cpal reports).
- **Soft keyboard / IME**: shown via winit 0.30 on focus; untested on a physical tablet with
  gesture navigation.
- **System bars stay visible**: the taskbar is inset around (nothing is covered), not hidden.
  Hiding it needs a Java call on the UI thread, which a native activity cannot make without
  shipping a `classes.dex`.
- **Screen orientation** is locked to landscape; portrait and freeform windows work but are
  untested with the desktop-style dock layout.

## Debugging

```sh
adb logcat -s RustStdoutStderr        # the app's log records (stderr reaches logcat)
adb shell run-as ai.storyteller.effectcraft cat files/EffectCraft/Logs/effectcraft.log
adb shell am start -n ai.storyteller.effectcraft/android.app.NativeActivity
```

Start-up milestones (graphics adapter, content rect, first frame) are logged the same way as on
the desktop (`docs/agents.md`, Help ▸ Enable Logging).

## The one `unsafe` in the workspace

`android-activity`'s glue calls the entry point by symbol name, and Rust can only export a symbol
with `#[unsafe(no_mangle)]` — an attribute the workspace forbids (`unsafe_code = "forbid"`).
`apps/effectcraft-android` is therefore the single crate that sets `deny` instead of `forbid`, and
carries one `#[allow(unsafe_code)]` on that one attribute (`src/lib.rs`); its body holds no unsafe
code. Everything else in the workspace stays forbidden, and `cargo clippy --workspace -- -D
warnings` still fails on any other unsafe construct in this crate.
