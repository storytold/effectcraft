# Testing

```sh
cargo test --workspace     # everything
cargo xtask ci             # fmt, clippy -D warnings, tests, layers, assets, wasm
```

## What the tests cover

- **Engine commands** (`crates/engine/src/tests*.rs`): every command family, including undo and
  redo, and that every menu entry resolves to a registered command.
- **Rendering** (`crates/render`): pixel checks on small compositions, for blend modes, masks,
  mattes, time remapping, 3D projection, lighting and shadows. Fast paths (the layer cache, the
  affine compositor, blurs, motion blur accumulation) are checked against their reference
  implementations.
- **Effects** (`crates/effects`): each effect has a determinism test and at least one behaviour
  test (identity at neutral settings, known pixel results, or simulations giving the same frame
  whether you seek straight to it or play up to it). A registry-wide test checks that effects
  which read the clock are declared time-dependent. `crates/effects/tests/sim_golden.rs` pins
  exact pixel hashes of the simulation effects the GPU shares plans with (CC Rainfall … Card
  Wipe) at several settings, times, bit depths and resolutions, so any change to their CPU output
  fails (re-pin with `SIM_GOLDEN_PRINT=1` after an intended change; pinned for aarch64 macOS,
  with the hashes another macOS release's libm gives for unchanged code in `LIBM_VARIANTS`).
- **Export** (`crates/export/tests`): every format is encoded and decoded back, checking frame
  count, size and pixels. When `ffmpeg`/`ffprobe` are installed they are used as an outside
  check; they are never linked or shipped.
- **Automation** (`crates/automation`, `apps/effectcraft-cli/tests`): MCP protocol round trips and
  the command-line tool's JSON output.
- **Interface** (`crates/ui-egui`): headless egui_kittest tests for panels and dialogs.

## Expression-driven Auto-Orient

`three_d::tests_orient` covers stopped expression motion in 2D and 3D, pauses
between different legs, a curved path, and a short closed excursion followed by a
long hold. `three_d::motion::tests` covers cache reuse/invalidation, cross-thread
reuse, cancellation, and a deliberately slow evaluation that must retain its heading.
Run these with `cargo test -p effectcraft-render --lib three_d` and the expression
integration coverage with `cargo test -p effectcraft-expr --lib auto_orient`.

For an end-to-end regression, enable Along Path on an arrow and set Position to:

```js
var s=0;
for(var i=0;i<100;i++){s+=Math.sin(i*0.37);}
var t=time-60;
var y=t<0?0:(t<0.1?1000*t:(t<0.2?200-1000*t:0));
[300+s,200+y,0]
```

Use a 30 fps, 1800-second composition. In a fresh process render 900 seconds
before rendering 60.2 seconds: both arrows must point up. Repeat the late seek
and compare CPU/GPU output. The first seek evaluates history and can be slow;
completed stationary intervals are reused. Search elapsed time must not select
a different heading. The expression engine's own limits still apply to each
expression evaluation, and preview cancellation stops history sampling between probes.
Expression failures stop the history search too; they must not trigger thousands of retries.

Motion identities use weak handles to the project's copy-on-write composition snapshots,
plus item names/source metadata and project settings. They do not serialize every layer on
each lookup or retain old document contents. Edits, cross-comp dependencies, source changes,
and undo/redo invalidate reuse. At most eight snapshot identities (4096 comps / 1 MiB of
metadata each) and 64 stationary intervals are kept; oversized identities disable reuse.
`toComp`/`toWorld` depend on those snapshots; nested pixel/font reads still disable reuse.
A closed, current-comp-only expression subset also allows skipping the proven stationary
tail after all keys in a 2D composition. Explicit time, dynamic member names, other comps,
unknown functions and external reads cannot use this shortcut. Tests assert bounded evaluation
counts for cold/warm keyed tails and errors, rather than machine-dependent timing thresholds.

The history search is explicitly bounded: composition-frame sampling, at most
60,000 frames in either direction and 65,536 probes shared with nested searches.
A short excursion entirely between sampled frames can be missed; movement outside
the search horizon is not recovered by sampling. The original evaluated-key fallback
is retained; algebraic expressions of `value` alone use the keyframe path directly. Bake
Position keys when these limits matter.
A work-budget warning is logged if the search cannot finish; this is not a claim
of exact history reconstruction for arbitrary JavaScript or arbitrary timelines.

## Looking at the interface without a window

```sh
cargo run -p effectcraft-ui-egui --example snapshot -- --out ui.png \
  --step '{"method":"engine.execute","params":{"command":"layer.select","params":{"layers":["#2"]}}}'
```

Each `--step` is a control-channel request; `{"method":"snap","params":{"path":"x.png"}}` writes an
intermediate image.

## Benchmarks

```sh
cargo run --release -p effectcraft-cli -- bench --n 10 --play 30
```

prints per-layer and per-effect timings for one frame, and playback timings with and without the
layer cache.

### Render benchmark (`cargo xtask bench`)

```sh
nice -n 10 cargo xtask bench --bin target/release/effectcraft-cli   # or no --bin: one `--release` build into target/bench
nice -n 10 cargo xtask bench --quick --compare before.jsonl        # 8 frames instead of 60, % change against an earlier run
cargo xtask bench --project F.ecproj --comp "Main"                 # any project's comp
```

measures memory, CPU and wall time of whole `effectcraft-cli` runs, so every optimisation has a
before and after number. It generates four deterministic scenarios into `target/bench/projects`
with engine commands (no committed media): `demo` (the built-in demo comp), `layers` (1080p,
100 solid, text and shape layers with Gaussian Blur, Glow, Levels, Curves, Drop Shadow and
keyframes), `precomps` (static and animated precomps nested three deep, each used several times)
and `footage` (a rendered PNG sequence and H.264 clip on several 1080p layers, plus a 4K still).
Each is opened (`open`), rendered at Full and Half (PNG sequence, deleted afterwards) and as one
frame. `/usr/bin/time -l` on macOS gives peak memory footprint, max RSS, user + sys CPU and
instructions retired; `/usr/bin/time -v` on Linux gives max RSS and CPU; elsewhere wall time only.

It prints a table and appends one JSON line per run to `target/bench/results.jsonl` (git rev,
date, load average, scenario, settings, metrics); `--compare OLD.jsonl` prints the % change against
the latest matching runs there, `--compare OLD --with NEW` compares two files without rendering.
The table notes the 1-minute load average: on a busy machine wall time is noise, so compare
peak memory and instructions retired, and rerun when the load differs a lot.

Results files allow blank lines; corrupt records or invalid comparison fields report the file
and line instead of being discarded. `--fail-over PCT` requires the same run keys on both sides,
successful latest runs, and at least one comparable resource metric per run. Failed latest runs,
missing coverage, empty results and wall-time-only measurements cannot pass the gate. Omit
`--fail-over` for an exploratory partial comparison; failed latest runs remain visible.
