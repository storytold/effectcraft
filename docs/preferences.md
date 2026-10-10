# Settings, keyboard shortcuts, auto-save and crash recovery

## Settings (Preferences)

EffectCraft ▸ Settings (macOS) or Edit ▸ Preferences (Windows/Linux), `Cmd+Alt+;`. The dialog has
After Effects 2026's pages (General, Startup & Repair, Project, Composition, Previews, Appearance,
Grids & Guides, Labels, Type, Import, Export, Audio, Disk, Memory & CPU, Video, 3D, Scripting &
Expressions) with OK, Cancel, Previous and Next. Older page names still open the right page
(`prefs.open {"page": "Auto-Save"}` opens Project, `Media & Disk Cache` opens Disk, `Memory &
Performance` opens Memory & CPU).

The model is `effectcraft_engine::prefs::Prefs`: serde, versioned (`version`), addressed by dotted
keys (`general.undoLevels`, `autoSave.intervalMinutes`, `labels.3.name`). It is stored as
`prefs.json` through the session's `ConfigStore` (the desktop app uses the platform config
directory: `~/Library/Application Support/EffectCraft` on macOS, `%APPDATA%\EffectCraft` on
Windows, `$XDG_CONFIG_HOME/effectcraft` on Linux; the web app keeps it in browser storage: the
Origin Private File System, or IndexedDB where OPFS can't write, see [web.md](web.md)). Loading migrates older layouts, keeps unknown keys (from newer versions) and
falls back to defaults for values that don't parse.

Commands (CLI, MCP, control channel):

| Command | Parameters |
|---|---|
| `prefs.get` | `{key?}` (omit for everything) |
| `prefs.set` | `{key, value}` or `{values: {key: value}}` |
| `prefs.reset` | `{page?}` (omit for everything) |
| `prefs.open` | `{page?}` opens the Settings dialog |
| `prefs.pages` | the page schema: every row, its key, type, range and whether it is wired |

### Settings that change behaviour

- `general.language`: interface language (`system` / `en` / `ja` / `zh-hans` / `zh-hant` / `uk` / `es` / `pt-br`), Settings ▸ General ▸ Language; menu labels change immediately. `system` (Match System, the default) follows the operating system's interface language where EffectCraft has a translation and is English otherwise: a Simplified Chinese locale (`zh`, `zh-CN`, `zh_CN.UTF-8`, `zh-Hans-CN`, `zh-SG`) selects `zh-hans`, a Traditional one (`zh-TW`, `zh-HK`, `zh-Hant`, `zh_MO`) selects `zh-hant`, a Ukrainian one (`uk`, `uk-UA`, `uk_UA.UTF-8`) selects `uk`, a Spanish one (`es`, `es-ES`, `es_MX.UTF-8`) selects `es`, a Portuguese one (`pt`, `pt-BR`, `pt_PT.UTF-8`) selects `pt-br`; the browser build stays in English with it. Choose **Español** or **Português (Brasil)** for Spanish or Brazilian Portuguese menus (panels stay in English for now); choose **Українська** for Ukrainian menus and cataloged panel, dialog, button and tooltip text; Ukrainian uses the bundled UI fonts. Native Japanese, Simplified and Traditional Chinese UI uses installed system fonts; the web host must supply a Japanese font. The same setting picks the CJK fallback for the interface *and* the Han preference of the text engine, so the composition's own text is drawn with the same language's glyph forms (the text engine's preference is process-global); swapping the language reinstalls the fonts, so it applies without a restart. Two catalogs back the text: the menus are keyed by `(command, English label)` (`crates/ui-egui/src/i18n.rs`), and the rest of the interface - panels, dialogs, buttons, tooltips - is keyed by its English source string (`crates/ui-egui/src/i18n/ui.rs`, with rows for Japanese, Simplified and Traditional Chinese and Ukrainian), where a panel writes `tr("Rename")`, or `tr_args("Proxy Use: {}", &[&quality])` when the row carries a value. A row that is missing falls back to English, so a new string shows up untranslated rather than empty; the tests in that module fail on an unused row and on a hard-coded string in a converted panel.
- `general.undoLevels`: Levels of Undo
- `general.pathPointSize`: Path Point and Handle Size
- `general.recentItems`: Recent Projects Shown
- `general.showToolTips`: Show Tool Tips
- `general.createLayersAtCompStart`: Create Layers at Composition Start Time
- `general.defaultSpatialLinear`: Default Spatial Interpolation to Linear
- `startup.showHomeOnLaunch`: Show Home Screen When Launching
- `startup.offerCrashRecovery`: Offer to Open the Latest Auto-Save After a Crash
- `startup.windowGraphics`: Window Graphics (`auto` / `gl`), what the desktop window draws with from the next launch. `auto` lets the platform pick (DirectX 12, Vulkan or Metal); `gl` uses OpenGL, with CPU compositing, for graphics drivers that crash with the others. A launch leaves a `launch-pending` marker in the settings folder until its window has been drawing for a few seconds (or the app quits normally before that); a launch that stops earlier (a graphics driver crashing as the window opens or as its surface is re-created) leaves the marker behind, and the next launch switches to `gl` and says so (not on macOS, which has no OpenGL backend). `WGPU_BACKEND` overrides both.
- `project.useTemplate`: New Project Loads Template
- `project.templatePath`: Template Project
- `autoSave.enabled`: Automatically Save Projects
- `autoSave.intervalMinutes`: Save Every
- `autoSave.maxVersions`: Maximum Project Versions
- `autoSave.location`: Auto-Save Location
- `autoSave.folder`: Custom Location
- `autoSave.saveOnRenderStart`: Save When Starting Render Queue
- `composition.showRenderingProgress`: Show Rendering Progress in Info Panel and Flowchart
- `previews.adaptiveResolutionLimit`: Adaptive Resolution Limit (the lowest resolution Fast Previews ▸
  Adaptive Resolution drops to while you drag; a playing preview keeps the viewer's resolution)
- `previews.cacheFramesWhenIdle`: Cache Frames When Idle (Composition ▸ Preview): after a second
  without input or edits, the viewer renders the work area into the RAM preview in the
  background, from the current time on, until it is cached or the budget is full
- `previews.fastPreviews`: Fast Previews (Draft 3D, Faster Effects)
- `appearance.appearanceMode`: Appearance Mode (`auto` / `dark` / `light`). `auto` (Sync with System) follows the operating system's light or dark appearance while the app runs: on Linux the desktop app reads the XDG desktop portal (`org.freedesktop.appearance` `color-scheme`) once and then listens for its `SettingChanged` signal on a worker thread, repainting only when the value changes (nothing polls); `gsettings` runs at most once at start-up, by absolute path, when the portal gives no answer. Elsewhere it uses what the window system reports, and the app leaves egui on its System preference so the window keeps reporting changes (a pinned Light or Dark would fix the macOS window appearance and hide the next switch). With no answer it shows the dark theme. New installs stay `dark`; Sync with System is opt-in. The Tools bar's appearance button (a monitor, sun or moon) cycles Sync with System, Light and Dark (`view.theme.toggle`, Next Appearance Mode).
- `appearance.darkTheme`: Dark Theme (`dark` / `darker`), shown in dark appearance
- `appearance.lightTheme`: Light Theme (`light`), shown in light appearance. Settings ▸ Appearance shows the mode above a light and a dark theme card, each with a preview of the EffectCraft window in that theme.
- `appearance.theme`: the single theme from before appearance modes, kept for older settings files and automation. A saved file with only `theme` loads as a fixed Dark or Light mode showing it; setting it (`prefs.set`, `ui.set {"theme": …}`, `view.theme.dark|darker|light`) selects that theme and fixes the mode to its family. While the mode is Dark or Light it mirrors the theme in use.
- `appearance.brightness`: Brightness
- `appearance.uiScale`: UI Scale, the size of the whole interface (75–200 %, on top of the
  display's own scaling), applied as soon as it is chosen
- `appearance.useLabelColorForHandles`: Use Label Color for Layer Handles and Paths
- `grids.gridColor`: Color
- `grids.gridSpacing`: Gridline Every
- `grids.gridSubdivisions`: Subdivisions
- `grids.guideColor`: Color
- `grids.actionSafe`: Action-safe
- `grids.titleSafe`: Title-safe
- `import.stillFootage`: Still Footage
- `import.stillSeconds`: Still Duration
- `import.sequenceFps`: Sequence Footage
- `audio.outputDevice`: Default Output (device list from the desktop host)
- `audio.outputLeft`: Left (Audio Output Mapping)
- `audio.outputRight`: Right (Audio Output Mapping)
- `memory.layerCacheMb`: Layer Cache (in the browser at most 512 MB, see [web.md](web.md))
- `memory.mediaCacheMb`: Footage Frame Cache (in the browser at most 256 MB)
- `memory.previewCacheMb`: Preview (RAM) Cache
- `threeD.defaultRenderer`: Default 3D Renderer
- `labels.N.name` / `labels.N.color`: the 16 label names and colours, used by the Label menu,
  the timeline, project panel, render queue and viewer handles
- `general.switchesAffectNestedComps`: a precomp layer's Quality (Draft / Wireframe) and Motion
  Blur switches limit the layers of the nested comp (viewer, Render Queue, `render`)
- `general.preserveConstantVertexCount`: adding or deleting mask / shape path vertices at one
  keyframe does the same on every other keyframe (added vertices split the matching segment)
- `general.syncTimeRelatedItems`: moving the current time moves it in the comps nested in this
  one and in the comps that nest it (through the precomp layers' timing)
- `general.expressionPickWhipCompact`: off, the pick whip writes match names
  (`thisComp.layer("A")("transform")("opacity")`)
- `general.createSplitLayersAbove`: where Edit ▸ Split Layer puts the new layer
- `startup.showHomeOnOpenProject`: the Home screen comes up after File ▸ Open / Open Recent
- `composition.motionPath`, `composition.motionPathSeconds`, `composition.motionPathKeyframes`:
  how much of a position motion path the viewer draws (all, none, N seconds or N keyframes
  around the current time)
- `previews.showInternalWireframes`: outlines of the layers inside a selected collapsed precomp
- `previews.zoomQuality`: Faster = nearest-neighbour viewer scaling, More Accurate = bilinear
- `previews.displayProfile`: the monitor's colour space for View ▸ Use Display Color Management
- `appearance.useLabelColorForTabs`: Composition / Timeline tabs show the comp's label colour,
  Effect Controls / Properties the layer's
- `appearance.cycleMaskColors`: new masks cycle through the mask colours (off: all the first)
- `appearance.useGradients`: soft gradient on panel tab strips
- `appearance.inWindowMenuBarMac`: on macOS, draw the menu bar inside the window instead of the
  native menu bar
- `grids.gridStyle`, `grids.guideStyle`: lines, dashed lines or dots (grid, proportional grid,
  guides); `grids.proportionalHorizontal` / `grids.proportionalVertical`: View ▸ Show
  Proportional Grid divisions
- `type.textEngine`: South Asian and Middle Eastern shows the paragraph direction and the
  World-Ready composers in the Paragraph panel
- `type.fontPreview`: a "Sample" preview in each font of the Character panel's font menu
- `type.recentFonts`: how many recently used fonts head the font menu
- `type.fontNamesInEnglish`: off, fonts with a native-language family name show it
- `import.reportMissingFrames`: gaps in an image sequence's numbering are reported on import
  (they show as colour-bar placeholders, as in After Effects; see [footage.md](footage.md))
- `import.autoReloadFootage`: Automatically Reload Footage, `off`, `nonSequence` (the default, as
  in After Effects) or `all` (image sequences too). Every two seconds,
  and as soon as the window comes back to the front, the desktop app compares each footage file's
  size and modification time with what they were, and reloads what another app saved: a
  Photoshop file edited in PhotoCraft or Photoshop shows its new pixels without File ▸ Reload
  Footage. It is not an undo step and doesn't modify the project; a layer item stays on its layer
  (found again by name), and a file that can't be read yet (still being written) is tried again once it changes again.
  `footage.reloadChanged` runs the same check once (CLI, agents)
- `import.unlabeledAlpha`: alpha of TGA / TIFF / movie footage (Ask opens Interpret Footage;
  Guess takes premultiplied for movies, straight for stills)
- `import.dragImportAs`: layered files dropped on the window import as footage or a comp
- `export.defaultOutputFolder`: where relative Render Queue outputs go
- `export.segmentSequences`, `export.segmentSequenceFiles`: sequences split into numbered
  folders of N files; `export.segmentMovies`, `export.segmentMovieMb`: movies split into
  numbered files of about N MB (cut at the frame count the output's data rate gives)
- `export.appendBitsToName`: `Comp 1.png` → `Comp 1_16bpc.png`
- `audio.previewSampleRate`: the rate previews are mixed at (and the device is opened at, when
  it supports it)
- `disk.diskCacheEnabled`, `disk.diskCacheMaxGb`, `disk.diskCacheFolder`: the persistent disk
  cache of processed layers and frames (in the browser: frames in the Origin Private File System,
  at most half the origin's quota; the folder setting does not apply there). The Disk page also
  shows **Browser Storage** in the web app: usage and quota, persistent storage, Clear buttons
  (`storage.info` / `storage.persist` / `storage.clear`, see [web.md](web.md))
- `disk.mediaCacheFolder`: audio waveform summaries kept between sessions (`Peaks/`)
- `disk.conformedMediaFolder`: decoded (conformed) footage audio, written once per file and
  sample rate and read back instead of decoding again
- `memory.ramReservedGb`: the cache budgets together leave this much physical memory free
- `memory.reduceCacheWhenLow`: cache budgets halve while the system is low on memory (checked
  every 30 s by the desktop app, on a background thread)
- `video.enableOutput`, `video.device`, `video.outputDuringPlayback`, `video.mirrorOnMonitor`,
  `video.disableWhenBackground`: Video Preview, a second window (or full screen on the display
  it is on) showing the composition frame, Mercury Transmit-style
- `threeD.showReferenceAxes`: world X/Y/Z axes in the viewer corner of comps with 3D layers
- `threeD.extendedViewer`: 3D layers reaching beyond the comp frame are outlined on the
  pasteboard
- `threeD.realtimeShadows`: draft (Fast Previews) renders keep 3D shadows
- `scripting.allowScriptsWriteFiles`: the scripting file / network gate (and `File.execute()`)
- `scripting.warnExecutingFiles`: `File.execute()` asks before opening a file
- `scripting.editorFontSize`, `scripting.syntaxHighlighting`, `scripting.lineNumbers`,
  `scripting.autoComplete` (Tab accepts), `scripting.bracketMatching`, `scripting.wordWrap`:
  the Timeline's expression editor
- `scripting.errorBanner`: a banner along the bottom of the Composition panel names the first
  failing expression (click: reveal it)
- `roto.model` (Settings ▸ Roto Brush): the segmentation model Roto Brush 2.0 / 3.0 use,
  `classical` (built in) or `mobilesam` (once installed). The page lists every registered model
  with its authors, licence, size, source and status, with Download, Install from File… and
  Remove (`roto.models` / `roto.model.*`); weights live in the `models` folder next to the
  settings, each with a `.NOTICE.txt` naming its authors and licence
- `face.model` (Settings ▸ Face Tracking): the model face tracking uses, `classical` (built in) or
  `mediapipe-face` (once installed); the same page of models (`face.models` / `face.model.*`)

### Display-only settings

These rows are in the dialog for After Effects parity and are stored, but have no EffectCraft
behaviour to change. `prefs.pages` reports them with `"live": false`; a test keeps this list in
step with the schema.

- `general.useSystemColorPicker`: EffectCraft has one colour picker on every platform and the
  web; there is no pure-Rust way to open the system colour panels.
- `composition.hardwareAcceleratePanels`: the Composition, Layer and Footage panels are always
  drawn by the GPU (egui on wgpu).
- `scripting.enableJsDebugger`: the JavaScript engine has no step debugger; script errors report
  their file and line in the Script Console.

Effect Controls uses compact, fixed 20 pt parameter rows, as After Effects does: each group level is indented beneath its effect, sibling names and values align, and a gap separates effects. The timeline keeps its fixed 19 pt rows. Both follow the whole-interface UI Scale.

## Keyboard shortcuts

Edit ▸ Keyboard Shortcuts (`Cmd+Alt+'`) opens the shortcut editor: an on-screen keyboard showing
which keys are assigned for the held modifiers (purple: application-wide, green: panel-specific,
both: split), modifier toggles, a searchable list of every command (all engine commands, menu
entries with bound parameters and the frontend's tool / panel commands), and a preset menu.

- **Presets.** "EffectCraft Default" is After Effects' default layout and is read-only; editing
  it creates a "Custom" copy. Custom presets store only their differences from the default and
  can be duplicated, renamed, deleted, exported and imported (JSON). The active preset is what
  the menus show and what the shortcut dispatcher runs.
- **Assigning.** Select a command, then press keys in the shortcut field (or type them), or
  click a key on the keyboard with the modifiers toggled. Conflicts with other commands in an
  overlapping scope are listed before you assign.
- **Scope.** Menu and engine commands are application-wide; timeline commands work in the
  Timeline and viewer commands in the Composition panel.

Commands: `shortcuts.list {query?, keys?, assigned?}`, `shortcuts.set {command, params?, keys}`,
`shortcuts.reset {command?}`, `shortcuts.preset {op: select|new|duplicate|delete|rename, name?,
from?, newName?}`, `shortcuts.export {preset?, path?}`, `shortcuts.import {path? | preset?}`,
`shortcuts.conflicts`.

## Auto-save and crash recovery

- Every *n* minutes (Settings ▸ Project ▸ Auto-Save) a project with unsaved changes is written
  to an `EffectCraft Auto-Save` folder next to it (or the custom folder) as
  `<name> auto-save N.ecproj`. Slots rotate through 1…maximum versions, overwriting the oldest.
  Untitled projects go to the custom folder, or to `EffectCraft Auto-Save` in the config directory.
- Every project and auto-save write is atomic: a temporary file is written and flushed, then
  renamed over the target, so a crash mid-write leaves the previous file intact.
- While the app runs, `session.lock` in the config directory records the open project and its
  latest auto-save. A clean quit removes it. If it is still there at the next launch, the app
  offers to open the latest auto-save (Settings ▸ Startup & Repair can turn this off).
- File ▸ Open Recent lists recent projects (stored in the settings), File ▸ Revert reloads the
  saved project, File ▸ Increment and Save saves `Intro.ecproj` as `Intro 2.ecproj`.

Headless MCP clients can opt in with `effectcraft-cli mcp --autosave`. Unlike the desktop's
interval, this writes changed dirty projects before sending each tool reply, with separate
version slots per server under `EffectCraft Auto-Save/MCP/` (or the custom folder's `MCP/`).
It reads settings without sharing the desktop's writable settings store or `session.lock`.
See [headless recovery](agents.md#keeping-headless-work-across-restarts) for how to reopen a
checkpoint after a client restart.

Commands: `file.autoSave` (now), `file.recoveryInfo`, `file.openRecent {index? | path?}`,
`file.clearRecent`, `file.revert`, `file.incrementAndSave`.
