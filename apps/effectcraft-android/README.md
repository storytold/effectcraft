# effectcraft-android

EffectCraft on Android: a small `apps/` crate that fills EffectCraft's `Hooks` with Android
equivalents and shapes the workspace for a phone, leaving the engine, the UI, the theme and the
command table untouched. `apps/effectcraft` is not compiled.

## What the shell does

- **Entry point / storage** — builds as `libmain.so`, loaded with `System.loadLibrary`. `$HOME` is
  set to `AndroidApp::internal_data_path()`, so the app's own `config_dir()` resolves to
  `<files>/.config/effectcraft` (settings, autosave, plugins).
- **File pickers** — `ACTION_OPEN_DOCUMENT` through the Activity; picked footage is copied into
  `<filesDir>/import/` and imported with the app's own `file.import` / `file.open` commands. Saves
  and renders write to `<files>/exports/` first and are published to the `ACTION_CREATE_DOCUMENT`
  destination once the file stops growing; cancelling leaves the render in `exports/`.
- **Phone workspace** — `phone_layout()` installs a "Phone" workspace in the app's own dock model
  (Composition above, Timeline below, Project/Effect Controls as tabs of the same dock) and puts it in
  `saved_workspaces`, so Window ▸ Workspace can switch away and back. Hit targets go to 40 pt.
- **Command palette** — long-press the top 8% for 500 ms. File commands go through the app's own
  `ui.menu.invoke` dispatcher (which calls `Hooks`, then the same engine command); everything else
  goes to `engine.execute`.
- **Fonts** — the system CJK face is copied into `$HOME/.fonts/` so EffectCraft's own font scanner
  finds it, and appended to every family after the theme is live as a second line of defence.
- **GPU** — the app's `GpuFailureBridge` is installed unchanged: a lost device or an uncaught GPU
  error is reported in the UI and the compositor falls back to the CPU instead of crashing.
- **Self-test** — a background thread decodes an embedded 320×240 H.264 clip and renders a frame
  through the app's own render queue, proving decode, compositor and encode on the device.

## Known gaps

- Audio output is not wired (`audio_device: None`): previews play silently instead of failing.
- The system clipboard is not wired; folder questions answer with the app's own `exports/`.
- Plugins that need a real filesystem picker get the export directory.
- Launcher icons are the Android Studio template's placeholder art.

## Build

```bash
cargo ndk -t arm64-v8a -o apps/effectcraft-android/jniLibs build --release -p effectcraft-android
cd apps/effectcraft-android/android && ./gradlew assembleRelease
```

`jniLibs/` and the Gradle output are gitignored; the APK is not part of this change.
