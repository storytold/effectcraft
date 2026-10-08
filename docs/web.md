# EffectCraft on the web

`apps/effectcraft-web` runs the same engine and egui UI as the desktop app in the browser: the
workspace compiled to `wasm32-unknown-unknown`, started by eframe's web runner on WebGPU (WebGL2
fallback). It is a static site: one `.wasm`, its `wasm-bindgen` JavaScript glue, `index.html`, a
few small scripts (worker, audio worklet, service worker), a web manifest and icons. Nothing is
uploaded anywhere: projects, media, settings and renders stay in the browser (its private storage)
and on the user's machine.

## Build and run

| Tool | Version | Install |
|---|---|---|
| Rust target | `wasm32-unknown-unknown` | `rustup target add wasm32-unknown-unknown` |
| wasm-bindgen CLI | exactly the `wasm-bindgen` crate version (0.2.129) | `cargo install wasm-bindgen-cli --version 0.2.129 --locked` |
| wasm-opt (optional) | any recent binaryen | `brew install binaryen`; used by `cargo xtask web` when found |

```sh
cargo xtask web                 # release build → <target>/web/dist
cargo xtask web --dev           # unoptimised build (faster to compile, slow to run)
cargo xtask web --serve 8765    # build, then serve dist on http://127.0.0.1:8765/
```

`<target>` is `$CARGO_TARGET_DIR` or `target`. `dist` holds `index.html`, `effectcraft_web.js` +
`effectcraft_web_bg.wasm` (+ `snippets/`, the browser glue `js/host.js`), `worker.js`,
`audio-worklet.js`, `sw.js` (its precache list and version filled in by the build),
`manifest.webmanifest`, `favicon.svg` and `icon-256.png` / `icon-512.png` (copied from
`assets/app-icon`). `--serve` is a tiny localhost-only static server that sends
`Cross-Origin-Opener-Policy` / `Cross-Origin-Embedder-Policy`, so the page is cross-origin
isolated like a production deployment should be. Any static server works
(`python3 -m http.server -d target/web/dist 8765`); it must serve `.wasm` as `application/wasm`.
Service workers need a secure context: `https://`, or `http://localhost` / `127.0.0.1`.

The release `.wasm` is about 58 MB before `wasm-opt` (v0.3.1; code: the effects, codecs, the
expression engine and the UI; data, mostly the bundled fonts; function names for readable
panics). Served compressed it is about 17 MB with gzip or 10 MB with Brotli, and the service
worker caches it after the first visit. Its name doesn't change between releases, so hosts
revalidate it rather than caching it for long (`packaging/web/README.md`).

On other targets the web crate holds only its portable storage model (`store.rs`, unit-tested by
`cargo test --workspace`); `cargo xtask wasm` (part of `cargo xtask ci`) checks the whole crate,
with every L0–L4 crate and `effectcraft-ui-egui`, for `wasm32-unknown-unknown`.

URL flags: `?empty` (a blank project; the session is not restored or recorded), `?demo` (the demo
project instead of the last session), `?home` (show the start screen), `?storage=indexeddb` /
`?storage=memory` (force a storage backend), `?noworkers` (render, analyse and draw viewer
frames on the page's thread), `?frameworkers=N` (frame workers, default 2; 0 = viewer frames on
the page's thread), `?nogpuworkers` (frame and job workers render on their CPU, without a WebGPU
device of their own), `?nosw` (don't register the service worker).

## How the desktop pieces map to the browser

| Desktop | Web |
|---|---|
| frame render thread pool (`ui-egui/src/frames.rs`) | frame workers (below), each with its own WebGPU device: `Frames::pump` hands queued frames to them, most important first (the viewer's frame, then prefetch); frames without effects that the page's GPU compositor can keep on the GPU render on the page. Without workers, queued frames render on the UI thread after each egui frame within 40 ms (24 ms while playing) |
| Render Queue on a background thread | a Web Worker running a second engine instance with its own WebGPU device (below); progress streams back, the page never waits |
| tracker / mask tracker / Warp Stabilizer / 3D Camera Tracker / Roto Brush Freeze threads, Content-Aware Fill tasks | the same workers; the analysed property group (or the fill layer) comes back as one undo step |
| `rayon` parallel loops (raster, effects, export batches) | rayon's global pool falls back to the calling thread when threads are unavailable: same code, serial |
| `std::time::Instant` / `SystemTime` (panic on wasm32) | `web-time` (re-exports `std::time` on native) |
| `std::fs` project reads/writes (`FsServices`) | `files::WebServices`: the virtual file table, persisted (below); saving also downloads the `.ecproj` |
| config directory: settings, shortcut presets, recent projects, the recovery sentinel | `store::WebConfig` in browser storage |
| auto-save folder | `/EffectCraft Auto-Save/` in browser storage (`ConfigStore::files`) |
| media reads (`MediaPool`, `probe`) | `files::WebImporter`: bytes from the file table, handed to `MediaPool::add_bytes` / `probe_bytes` |
| export writes (`effectcraft-export`) | the job's `sink` (`effectcraft_host::FileExporter { sink }`): files land in the table and download after the render; several files (an image sequence) download as one stored `.zip` |
| rfd file dialogs | `<input type=file>` for File ▸ Open / Import; drop files anywhere on the page (`.ecproj` opens, everything else imports); "Save As" / "Output To" pick a download name |
| Media Browser on the file system | browser storage and folders opened with the File System Access API (below) |
| system fonts | not scanned; the bundled fonts (Inter, Noto Serif, JetBrains Mono) are always there, plus BIZ UDPGothic Regular for Japanese (UI and text) when built with craft-fonts (`CRAFT_FONTS_DIR`, AGENTS.md; release builds do; +4.5 MB of `.wasm`) |
| TCP control channel / MCP | `window.effectcraft` (below) |
| cpal audio output | Web Audio (below) |
| GPU compositor on the desktop's wgpu device | the same compositor on eframe's WebGPU device, and GPU effects on each frame worker's own WebGPU device (below) |
| disk cache folder (Settings ▸ Disk ▸ Disk Cache) | viewer frames and slow layer buffers in the Origin Private File System, written by the frame workers (below) |
| (the file system's own tools) | the storage manager: Settings ▸ Disk ▸ Browser Storage and `storage.*` (below) |

### Persistence

Everything that lives in files on the desktop lives in the browser's **Origin Private File
System** (OPFS); where OPFS can't be written from the page (older Safari), **IndexedDB**; with
neither (some private windows), memory only. `effectcraft.info().storage.backend` says which.
The page asks for persistent storage (`navigator.storage.persist()`), which browsers grant to
installed apps, so the store isn't evicted under storage pressure.

The engine's storage traits are synchronous and the browser's are not, so the app works on an
in-memory mirror (`store.rs`): every stored entry is read before the app starts, reads and writes
hit the mirror, and changes flush in the background (one write per changed key, in order; OPFS
writes are atomic: a swap file replaced on close). Keys:

| Key | What |
|---|---|
| `config/prefs.json`, `config/shortcuts.json` | Settings (with File ▸ Open Recent's list) and keyboard shortcut presets |
| `config/session.lock` | the crash-recovery sentinel |
| `config/session.ecproj`, `config/session.json` | the session snapshot: the open project and editor state (active comp, time, selection), written at most once a second while they change |
| `files/<path>` | the file table: imported media, projects saved with File ▸ Save / Save As or `saveToBrowser`, auto-saves |

On the next visit the app reopens the snapshot (unsaved changes stay unsaved: the project is
modified until saved, `Session::mark_unsaved`, so File ▸ New, Open, Close Project and Revert still
ask to save it first), registers every stored media file with the media pool so its footage decodes, and File ▸
Open Recent reopens saved projects from storage. Without a snapshot (first visit) it opens the
demo project. Render outputs are not stored: they download.

### Background jobs: Web Workers

wasm threads need shared memory, which needs a nightly `build-std` toolchain, so this build is
single-threaded per instance. Long jobs run in **Web Workers** instead, each with its own instance
of the same module (the page compiles it once and posts the `WebAssembly.Module`, so nothing is
compiled twice) and its own memory (`crates/engine/src/offload.rs`, `src/worker.rs`):

1. The session's `Offload` serializes a `WorkerRequest`: the project JSON, the footage paths it
   reads, and the job (`render` with the resolved queue items and output paths; `warp`, `camera`,
   `track`, `maskTrack`, `rotoFreeze`, `rotoPropagate` with the target and analysis parameters;
   `contentFill` with its plan).
2. `js/host.js` takes an idle worker from its pool (or starts one), sends it the footage bytes it
   doesn't have yet, then the request.
3. The worker runs the job on a plain engine session and posts `WorkerReply`s: render progress
   (at most every 100 ms), item started/finished, rendered files (transferred), analysis
   progress, the analysed property group, `failed`, `done`; then `{type: "stats"}` with the
   job's GPU passes and readbacks. With a WebGPU device of its own the job's frames render on
   it in passes (below, **GPU in job workers**).
4. The page applies them every frame: `Session::poll_render` updates the queue items and the
   progress bar, `Session::poll_offload` writes an analysis result as one undo step (the effect /
   tracker / mask group is replaced; other edits made meanwhile are kept) and the downloads start.

Stopping a job terminates its worker (a worker can't be interrupted mid-frame): the item being
rendered becomes "User Stopped", an analysis writes nothing. Every job shows in Window ▸ Progress
(`jobs.list`) with its progress and a cancel button (`jobs.cancel`).

**Roto Brush propagation** runs in a job worker too (`WorkerJob::RotoPropagate`): the worker
streams the segmentations it computes back at most every 250 ms (`WorkerReply::Segs`: each
`FrameSeg` in a compact binary form, run-length coded planes, base64), the page puts them into its
segmentation cache and relays them to the frame workers. Stopping it keeps the frames propagated
so far.

**`wait: true`** (the default of `renderQueue.render` for agents, and an option of the analysis
and Roto Brush commands) no longer blocks the page: over `window.effectcraft` (the control
channel) such a command runs with `wait: false`, and the promise resolves when the job it started
ends, with what the blocking command returns (final render item statuses, the analysis status)
or rejects with the job's error (`ui-egui/src/control.rs`, `JobWaiter`). Engine commands executed
directly on the session (scripts) still block.

### Viewer frames: frame workers

Viewer frames (the frame on screen, RAM preview and prefetch) render in long-lived **frame
workers** (`src/frames.rs`, `crates/engine/src/remote.rs`), two by default. Each holds a replica
of the project:

1. Before a render, the page sends a worker the footage files it lacks, then a sync: the whole
   project the first time, afterwards a **diff** of the serialized project from the revision the
   worker holds (`remote::diff`: object members added / changed / removed, arrays element-wise
   when their length is unchanged; a slider drag sends a few hundred bytes, not the project).
2. Then the render request (`FrameMsg::Render`: revision, comp, time, render options). The worker
   renders on its CPU renderer and posts the frame back as premultiplied RGBA8 in a transferred
   buffer (`FrameReply::Frame`); the page puts it in the frame cache.
3. One frame per worker at a time, so the page always picks what renders next (the viewer's frame
   before prefetch); a request for a revision the worker does not hold is refused, a worker that
   lost its replica asks for the whole project again (`FrameReply::Resync`).

Frames without effects that the WebGPU compositor can finish alone still render on the page
(they stay on the GPU, no readback); everything else (frames with effects, 3D runs, the Software
Only renderer, WebGL2) goes to the workers. If no worker is usable, frames render on the page's
thread as before. `effectcraft.info().frameWorkers` reports `{workers, alive, busy, rendered,
failed, lastMs, syncs, syncBytes, gpu, gpuFrames, lastPasses, diskHits}` (`gpu`: each worker's
adapter, `{cpu: why}` without one, `null` until it reported).

**GPU in job workers (M13.30).** Each job worker opens its own WebGPU device too
(`workerJobInit`; `?nogpuworkers`: their CPU) and makes it the session's accelerator. A job
can't block on a readback either, so the job loops are futures rather than blocking loops:
`effectcraft_engine::offload::run_request_async` runs the request, and every frame it renders
goes through `effectcraft_render::passes` (`in_passes`, `comp_frame`): render a pass, and while
it missed, await the device (`Accelerator::settle`, the browser's event loop delivers the
readbacks) and render again, as `FrameServer::resume` does for viewer frames (the desktop drives
the same futures with `passes::block_on`; they never wait there).

- **Render Queue**: `effectcraft_export::export_async` renders each frame in passes (frames
  with deferred readbacks render one by one instead of in parallel batches) and encodes as
  before (`Exporter::export_async`). Renders use Backend Auto, as on the desktop: per comp
  the worker warms up both sides (GPU first) and then renders on the faster one, each frame's
  time measured across all its passes.
- **Analyses** (Warp Stabilizer, 3D Camera Tracker, Track Motion, mask and face tracking, Roto
  Brush freeze and propagation): their work functions are `async`; started with `wait` inside
  a job they are queued and awaited (`offload::run_or_queue`), and each input frame renders in
  passes with the GPU backend (`offload::job_accel`). Roto Brush's segmentation reads frames
  through a callback, so its frame source prefetches the frame and its neighbours in passes
  before each step. On the desktop analyses render their input on the CPU, as before.
- **Particles and Advanced 3D** read back under keys like effect chains (`crate::deferred`):
  a particle simulation by its request (system, steps, births), a rasterised scene by
  everything it uploads, a whole prepared run by its scenes and output; the first pass starts
  the readback and gets a placeholder (no particles, an empty target), later passes read the
  bytes. A run with depth of field resolves on the CPU (its blur needs a readback mid-run),
  each of its scenes still rasterised on the GPU. So frames with particles or Advanced 3D
  render on the device in frame workers and job workers alike.

`effectcraft.info().workers` reports `{running, idle, gpu: {adapter} | {cpu: why}, jobs:
[{kind, gpu, passes, readbacks, ms}]}` (the last jobs).

**Warp Stabilizer plans off the page (M13.32).** Solving a stabilization plan (Subspace Warp:
an eigen-decomposition per frame, then a mesh fit) takes seconds for a long clip, and
`warp.status` used to solve it on the page the first time it saw the analysis (a ≈ 5 s
stall). Now the analysis job solves the plan for the analysed settings and sends its summary
(`WorkerReply::WarpPlan`: per-frame warps, auto-scale, crop, valid fraction) with the result.
When the settings change later, `warp.status` reports `stabilizing: true` and starts a
`WorkerJob::WarpPlan` job (`jobs.list` id `warpPlan`) instead of solving it; the next status
after it ends has the plan. Frames with the effect render in the frame workers, which solve
their own plans. The desktop solves the plan in place, as before.

**Content-Aware Fill in a job worker (M13.30).** `contentFill.generate` (the panel's Generate
Fill Layer) sends its plan (`content_fill::FillPlan`: layer, frames, method, folder) to a job
worker (`WorkerJob::ContentFill`, `jobs.list` id `contentFill`): the worker renders the
layer's frames in passes on its device, fills them and writes the PNG sequence through its
services, which send each file to the page's browser storage (`{type: "store"}` messages, under
`/Fill/<layer>_Fill_<n>/`); then the plan comes back (`WorkerReply::Fill`) and the page adds
the fill layer as one undo step. (Before, the fill ran on the page's thread and could not
write its folder.)

**Threading design: one engine instance per worker (decided, M13.24).** Shared-memory threads
inside one instance (SharedArrayBuffer + atomics, a `wasm-bindgen-rayon`-style pool) need the
standard library rebuilt with atomics (`-Z build-std`, nightly Rust) and every crate's statics
made thread-safe for the browser's rules (no blocking on the page's thread). The repository
builds on stable Rust, so concurrency comes from workers that each run their own engine instance
with their own memory: job workers (renders, analyses), frame workers (viewer frames, each with
its own GPU device) and the page. What crosses between them is explicit: project diffs, footage
bytes, frames, Roto Brush segmentations, cache keys. The cost is memory per worker (a project
replica, the footage it reads, a layer cache) and serial loops inside one frame (rayon's pool
falls back to the calling thread). The page is still cross-origin isolated (`--serve` and the
service worker send COOP / COEP), so a threaded build (`build-std` with `+atomics,+bulk-memory`,
`wasm-bindgen-rayon`) could be added later without changing the deployment; it would parallelise
the loops inside a frame, not replace the workers.

### Media Browser

Window ▸ Media Browser browses a virtual tree (`src/browse.rs`):

- **Browser Storage**: every file in browser storage (imported media, saved projects, files added
  with **Add Files…** or **Upload Folder…**, plain file inputs that work in every browser). This is
  the persistent recent list where folders can't be opened. A picked file that can't be read is
  named in the status bar with the browser's reason, and the others are still added.
- **Folders**: **Open Folder…** (File System Access API, `showDirectoryPicker`, Chromium) lists a
  folder from the user's disk; only the listing is read, and a file's bytes are read when it is
  imported (then kept in browser storage). The folder handles are remembered in IndexedDB; after
  a reload the folders come back if the browser still grants access, else **Reconnect <name>**
  asks again.

`mediaBrowser.list` reports the places and these actions; `mediaBrowser.action {action}` runs one.

### Audio

Audio preview goes through **Web Audio**: an AudioWorklet (`audio-worklet.js`; a
ScriptProcessorNode where worklets are unavailable) plays interleaved stereo blocks. The mixdown
is the desktop's (`AudioPlayback`, `mix_comp`): without a feeder thread, `AudioPlayback::pump`
mixes ahead on every UI frame (≈ 350 ms queued) and the output device takes what keeps it
≈ 150 ms ahead of the playhead. A/V sync follows `AudioContext.currentTime`: the playback clock
is the frames played by the context clock minus its output latency, and the viewer shows the
frame under the audible sample, as on the desktop. The Audio panel's meters read the same feed.

Browsers start an AudioContext suspended until the user interacts with the page; the app resumes
it on the first pointer or key event, so audio plays from the first preview after any click or
key press.

### GPU

eframe starts on WebGPU where the browser has it (WebGL2 otherwise), and the GPU compositor
(`effectcraft-gpu`) runs on the same device: viewer frames are composited by compute shaders and
drawn straight from their texture, with no readback. The Info panel's pixel readout and the
eyedroppers read GPU frames back asynchronously (`Gpu::read_display_async`: the pixels arrive a
frame later). On WebGL2, or without a usable adapter, everything renders on the CPU.
`effectcraft.info().gpu` reports `{compositor, viewerOnGpu}`.

If the page's WebGPU device is lost (a driver reset, the browser reclaiming the GPU), the canvas
can't show anything new, while the project stays open and commands keep working. The page then
covers the canvas with a notice (`showDeviceLost` in `js/host.js`) offering **Save Project** (a
download) and **Reload**, which first writes pending changes to browser storage
(`effectcraft.flush()`) so the session comes back after the reload; when the write fails it says
so and offers **Reload Anyway**.

**GPU effects run in the frame workers.** A browser never lets JavaScript (or wasm) wait for a GPU
readback, not even in a worker: a buffer maps only once the thread returns to its event loop.
The CPU renderer, which runs GPU effect chains and reads their results back mid-frame, therefore
can't use the page's device. Instead each frame worker opens its own WebGPU device
(`navigator.gpu` in dedicated workers: Chromium, Safari Technology Preview; `Gpu::request_deferred`)
and renders with **deferred readbacks** (`crates/gpu/src/deferred.rs`; job workers do the same,
above):

1. Every readback the renderer needs is keyed by what determines its pixels: a GPU effect chain by
   its input buffer's pixels and the chain's parameters, the top-level frame by comp, time and
   render options.
2. A key seen for the first time records the GPU work, starts an asynchronous copy (`map_async`)
   and *misses*: the step returns a cheap placeholder (its input), and the pass is marked missed.
   From the first miss to the end of the pass, the layer cache drops inserts
   (`LayerCache::set_gate`), so no placeholder-derived buffer is kept.
3. The worker awaits the device (`Gpu::settled`) and renders the frame again
   (`FrameServer::resume`); keys whose bytes arrived are served from them. A pass without a miss
   is the frame. Chains that depend on other chains (a precomp with effects inside, effects that
   read other layers) take one more pass per level; independent chains resolve together. A chain
   whose kernels read data that missed while they ran (through the effect host) is not started,
   so a result is only ever computed from genuine inputs. After 24 passes a frame renders on the
   worker's CPU.

The whole frame renders on the GPU (the compositor walk with GPU effects, then one readback) when
it can; steps that would need a mid-walk readback (an Advanced 3D run the kernels decline, CPU-only
effects on an adjustment layer) make the frame composite on the CPU instead, with its GPU effect
chains still on the GPU. Backend **Auto** works in the workers: it picks CPU or GPU per comp from
the frames' total times across their passes (`AutoPick`), as on the desktop. Without WebGPU in
workers (Firefox, older Safari) or with `?nogpuworkers`, the workers render on their CPU, as
before. A frame with effects goes to a worker when the workers have a GPU, so the page never runs
an effect on its thread. Particles and Advanced 3D read back under keys too (M13.30, above).

`effectcraft.workerFrameCheck({time?, scale?, backend?})` renders the active comp's frame in a
worker (its GPU by default) and on the page's CPU and reports the differences (`{maxDiff,
meanDiff, over4, workerMs}` in 8-bit levels): the smoke test uses it to check the GPU workers'
pixels.

### Disk cache

Settings ▸ Disk ▸ Disk Cache works in the browser, backed by the **Origin Private File System**
(`src/diskcache.rs`): viewer frames are kept under `effectcraft-cache/v1/frames/<32 hex>.ecc`
between visits, in the desktop cache's format (`disk_cache::frame_entry`: 8-bit premultiplied
RGBA, LZ4, a checksum), keyed by the same content hash (the comp and everything it uses, frame,
scale, view, render options).

- A render request to a frame worker carries the frame's disk key; the worker posts the frame,
  then writes the entry with a **synchronous access handle** into a temporary file moved into
  place (`cacheWrite` in `js/host.js`; where `move()` is missing, the entry is written in place
  and its checksum catches a partial file), and reports `FrameReply::Stored`.
- The page keeps the index (`DiskIndex`, shared with the desktop's cache): rebuilt at startup from
  the folder listing (oldest files first, stale temporaries removed), updated by the workers'
  writes and by reads. The size limit is Maximum Disk Cache Size, at most half the origin's quota;
  the least recently used entries are deleted beyond it.
- A frame whose key is indexed is read and decoded on the page (asynchronously) instead of being
  rendered; a damaged or missing file is a miss and is forgotten. The timeline's blue cache bar
  shows the frames on disk.
- `cache.diskStats` reports the browser's cache (`{enabled, available, loaded, entries, frames,
  layers, bytes, maxBytes, hits, misses, writes, evictions, layerHits, layerWrites}`); Empty Disk
  Cache / `edit.purge {what: "disk"}` clears it. `effectcraft.info().diskCache` has the same.

**Layer buffers (M13.30).** Slow layer buffers (≥ 20 ms to render) go to the disk cache too
(`effectcraft-cache/v1/layers/<32 hex>.ecc`, `disk_cache::layer_entry`, keys salted per project
and footage as on the desktop). A layer lookup happens in the middle of a synchronous render,
where the browser's file system can't be read, so it is made synchronous by fetching first:

1. Each frame worker backs its layer cache with a `PrefetchStore` (`effectcraft_render::cache`):
   lookups are served from buffers fetched before the request; a lookup that finds nothing is
   recorded, and buffers to keep queue up as sealed entries.
2. The reply lists the frame's layer misses and the buffers served from the store
   (`FrameReply::Frame` `layerMisses` / `layerHits`); after posting the frame the worker writes
   the queued entries (sync access handles) and reports each (`FrameReply::Stored` `layer`).
3. The page keeps one index for frames and layers (`DiskIndex`: one least-recently-used order;
   hits touch entries) and remembers the workers' recent misses (`disk_cache::LayerPrefetch`).
   A render request names the recent misses that are on disk (`FrameMsg::Render` `prefetch`,
   at most 64); the worker reads those it lacks into its store before rendering, so their
   lookups hit (another worker's buffers, buffers from an earlier visit, buffers dropped by a
   purge).

### Storage manager

Settings ▸ Disk ▸ **Browser Storage** (shown in the browser only) reports where the data lives
(Origin Private File System, IndexedDB or memory), the origin's usage and quota
(`navigator.storage.estimate()`), what the store holds (projects, imported media, auto-saves:
count and size) and the disk cache, whether the storage is persistent (with **Request Persistent
Storage**, `navigator.storage.persist()`), and **Clear** buttons for the disk cache, imported
media, auto-saves and saved projects. Agents use the same through engine commands
(`crates/engine/src/storage.rs`, the session's `StorageHost`):

| Command | |
|---|---|
| `storage.info` | `{available, backend, usage, quota, persisted, files: {projects, media, autoSaves: {count, bytes}}, diskCache: {...}}` (`{available: false}` on the desktop) |
| `storage.persist` | ask for persistent storage; `persisted` in a later `storage.info` says whether it was granted |
| `storage.clear {what: diskCache\|media\|projects\|autoSaves\|all}` | delete them (settings stay); `{cleared, entries, bytes}` |

The widgets have automation ids `settings.storage.backend`, `.usage`, `.diskCache`, `.persist`
and `.clear.<what>`.

### Offline and install (PWA)

`manifest.webmanifest` (name, icons, standalone display) makes the app installable; `sw.js`
precaches every file of the build into a cache named after the build's hash, serves them
cache-first (the cached responses keep the server's COOP/COEP headers, so the page stays
cross-origin isolated offline), and drops older caches when a new build activates. After the
first visit the app loads without a network.

## `window.effectcraft`: the agent / test API

The control channel of the desktop app (`docs/control-protocol.md`) as promises. Results resolve
with the method's `result`, errors reject with the message.

| Call | |
|---|---|
| `effectcraft.request(method, params)` | any control method: `ui.inspect`, `ui.click`, `ui.key`, `ui.playback`, `ui.menu.invoke`, `render.frame`… |
| `effectcraft.execute(command, params)` | `engine.execute`: an engine command (same ids and params as MCP / `effectcraft-cli`) |
| `effectcraft.commands()` / `effectcraft.inspect()` | `engine.commands` / `ui.inspect` |
| `effectcraft.renderFrame(params)` | `render.frame` with `base64: true`: `{comp, time, width, height, png}` |
| `effectcraft.screenshot(params)` | `ui.screenshot`; without `path` the PNG comes back inline as `png` (base64) |
| `effectcraft.addFile(fileOrUrl, name?)` | put a `File`/`Blob` (or fetched URL) into the file table (stored) and open (`.ecproj`) or import it; resolves with its path |
| `effectcraft.files()` / `effectcraft.readFile(path)` | the file table `[{path, size}]` / a file's bytes (`Uint8Array`), e.g. a render |
| `effectcraft.saveToBrowser(path?)` | save the project to browser storage without downloading it (default: its path, or `/<name>.ecproj`); resolves with `{path, bytes}` |
| `effectcraft.listStored()` | `{backend, usage, quota, persisted, pending, files: [{path, size, modified}], config: [name]}` |
| `effectcraft.removeStored(path)` | delete a stored file |
| `effectcraft.flush()` | resolves once every change is written to browser storage; rejects when a write failed (quota exceeded…: the change stays pending and is retried) |
| `effectcraft.info()` | graphics backend, `gpu`, `storage`, `audio` (`{state, sampleRate, backend, posted, played, underruns}`), `workers`, `frameWorkers`, `diskCache`, `restored`, `webgpu`, `serviceWorker`, version, `crossOriginIsolated`, load timings |
| `effectcraft.workerFrameCheck(params)` | a frame rendered in a frame worker (its GPU by default) against the page's CPU render: `{width, height, maxDiff, meanDiff, over4, workerMs}` |

`window.effectcraftLoad` holds `{wasmMs, readyMs}` (module fetch + compile, and until the app runs).

```js
await effectcraft.execute("layer.newSolid", {color: "#ff8800"});
await effectcraft.request("ui.menu.invoke", {id: "renderQueue.add", params: {format: "gif", resolution: 0.25}});
await effectcraft.execute("renderQueue.render", {wait: false});  // renders in a worker; the GIF downloads
await effectcraft.saveToBrowser("/my-project.ecproj");            // File ▸ Open Recent has it after a reload
```

## Browser test

`apps/effectcraft-web/tests/smoke.mjs` drives headless Chrome over the DevTools protocol (Node ≥ 22,
no npm packages): load, the GPU path (WebGPU → GPU compositor, viewer on the GPU), the demo comp
in the viewer, `render.frame`, Render Queue GIF and PNG sequence (`.zip`) downloads, a background
render in a worker while measuring the page's event-loop gaps (must stay under 400 ms) and its
progress, the AudioContext starting on a (simulated) key press and a tone playing through it
with the meters moving, Save As and re-opening the saved project, persistence across a reload
(session snapshot, a stored project, imported media, a setting, Open Recent) and an offline
reload through the service worker. It writes screenshots and `report.json`:

```sh
cargo xtask web --serve 8765 &
node apps/effectcraft-web/tests/smoke.mjs --url http://127.0.0.1:8765/ --out target/web/smoke
```

`apps/effectcraft-web/tests/workers.mjs` checks the job-worker plumbing (`js/host.js`,
`web/worker.js`) under Node with a fake `Worker`, without a build: a replaced file is sent to a
reused worker again even at the same size, and a worker that fails to start fails its job and is
terminated (`node apps/effectcraft-web/tests/workers.mjs`).

`apps/effectcraft-web/tests/page.mjs` checks page-side helpers of `js/host.js` the same way with
a fake DOM: a picked file that can't be read is reported while the others are added, and the
device-lost notice saves, flushes before reloading and reports a failed flush
(`node apps/effectcraft-web/tests/page.mjs`).

It also checks the M13.10 paths: viewer frames rendered in frame workers while scrubbing with the
CPU renderer (project synced as diffs, event-loop gaps under 400 ms), a `wait: true` render that
replies when done while the page keeps running, and the Media Browser over browser storage; and
the M13.24 ones: GPU effects (Gaussian Blur, Glow) rendered on the frame workers' own WebGPU
devices in passes while the page's event loop stays free, the worker's GPU frame matching the
page's CPU render (`workerFrameCheck`), the storage manager (`storage.info`, `storage.persist`,
the Settings ▸ Disk widgets), and the disk cache: frames written to the Origin Private File
System, served from it after a reload without rendering, and cleared with `storage.clear`; and
the M13.30 ones: a Render Queue job and a Warp Stabilizer analysis on the job worker's own
WebGPU device (`workers.jobs`: GPU passes and readbacks per job) while the page stays
responsive, and layer buffers written to the disk cache and served from it after a reload and a
purge (`layerHits`); M13.32: editing after the Warp Stabilizer analysis (removing it,
pre-composing, a mask) keeps the page's event-loop gaps under 50 ms.

Native unit tests cover the storage model (`apps/effectcraft-web/src/store.rs`: write
coalescing, file table, `ConfigStore` / `FileOps`, auto-save and crash recovery through the
browser store) and the worker protocol (`crates/engine/src/offload.rs`: serde round trips of
every request and reply, a render through an in-process offload; `tests_roto.rs`: Roto Brush
propagation through an offload streaming segmentations; `tests_mask_warp.rs`: Warp Stabilizer
plans solved in the job, never by `warp.status` on the page), the frame-worker protocol
(`crates/engine/src/remote.rs`: diff / patch, a replica following edits renders the same pixels;
`crates/ui-egui/tests/ui_web_offload.rs`: the frame cache through a remote renderer, `wait: true`
deferred until the offloaded job ends) and the Media Browser's virtual tree (`media_browser.rs`,
`tests_panels.rs`). M13.24: deferred readbacks on a real device (`crates/gpu/src/tests_deferred.rs`:
whole-frame GPU and CPU-composited frames in passes match the CPU, placeholders never reach the
layer cache; `deferred.rs`: keys, late readbacks of an older frame dropped), the frame server's
passes and Auto timing (`remote.rs`, a deferred stand-in accelerator), the browser's routing and
disk cache (`crates/ui-egui/tests/ui_web_gpu_storage.rs`: a GPU frame worker renders effect frames,
the page's GPU keeps frames without effects, frames stored under their disk keys come back after a
"reload" without rendering, the blue cache bar sees them; the storage manager's widgets and
commands), the disk index (`disk_cache.rs`), the store's usage by kind and clearing (`store.rs`),
`storage.*` (`storage.rs`), and every WGSL float literal fitting f32 (browsers reject the whole
module otherwise; `tests.rs`). M13.30: job workers on a deferred device
(`crates/ui-egui/tests/ui_web_job_gpu.rs`: a Render Queue job and a Warp Stabilizer analysis
render their frames in passes on the device, with the CPU worker's pixels / result),
particles, Advanced 3D and the job pass driver in passes (`crates/gpu/src/tests_deferred_jobs.rs`),
layer buffers through the disk store and the prefetch plan (`remote.rs`
`layer_buffers_go_through_the_disk_store`).

## Gaps

- No shared-memory threads: each worker is a separate engine instance (memory per worker: the
  project replica and the footage it reads); see the threading design above.
- Browsers without WebGPU in workers render frames, Render Queue jobs and analyses on the
  workers' CPU. A frame renders at most 24 passes on the GPU, then on the CPU.
- A layer buffer is fetched from disk only after some worker missed it in a recent frame (the
  first frame after a reload renders its layers).
- Cancelling an analysis drops its partial result (the worker is terminated).
