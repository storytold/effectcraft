# Testing

```sh
cargo test --workspace     # everything
cargo xtask ci             # fmt, clippy -D warnings, tests, pdf-deps, lock-patches, layers, assets, wasm
cargo xtask ci --affected  # the same gates, scoped to crates your change can affect
```

The After Effects test projects of `crates/aep` are not in the repository; without them the
`crates/aep`, engine and ui-egui tests that need one print `SKIPPED: fixture <name> missing` and
pass. They live in [storytold/effectcraft-corpus](https://github.com/storytold/effectcraft-corpus).
`cargo xtask aep-corpus` fetches them at the commit pinned in `crates/aep/tests/fixtures/corpus.lock`,
checks every file against the SHA-256 listed there (any mismatch fails and installs nothing) and
installs them in `corpus/aep`. Point `EFFECTCRAFT_AEP_FIXTURES` at that folder to run those tests
too, and set `EFFECTCRAFT_AEP_FIXTURES_REQUIRED=1` as well to make a missing project fail its test
instead of skipping (a corpus run that must not pass by skipping):

```sh
cargo xtask aep-corpus
EFFECTCRAFT_AEP_FIXTURES=corpus/aep EFFECTCRAFT_AEP_FIXTURES_REQUIRED=1 cargo test -p effectcraft-aep
```

The `dev` profile keeps debug info to line tables (fast builds, readable backtraces). To debug
interactively — local variables, parameters, column breakpoints — build with the `dev-full`
profile: `cargo run --profile dev-full -p effectcraft -- --demo` (see `CONTRIBUTING.md`).

`cargo xtask ci` builds in the dev profile and runs the tests with `cargo nextest run` (plus
`cargo test --doc`) when `cargo-nextest` is installed, and with `cargo test` otherwise. It then
runs `missing_translations_report` with `--nocapture`, which prints the interface strings each
language has no translation for. Those strings show in English; the report always passes.

Each crate with several integration-test files has one test binary, `tests/it/main.rs`, whose
modules are the files in `tests/it/`. Run one file with `cargo test -p <crate> --test it <module>::`
or `cargo nextest run -p <crate> -E 'binary(it) and test(/^<module>::/)'`.

## Affected-only CI

Use `cargo xtask ci --affected` to run the gates for the crates a change can affect. The full
`cargo xtask ci` remains the required check before a release and when affected mode falls back
because a workspace-wide input changed.

```sh
cargo xtask ci --affected                  # base: the merge-base of HEAD with main
cargo xtask ci --affected --base HEAD~3    # any git revision
cargo xtask ci --affected --dry-run        # print the plan only
```

It collects files that differ from the base (committed, staged, unstaged and untracked), preserving
Unicode and whitespace in their names. Each file maps to the workspace package whose directory
holds it and every package whose sources point at it with a `../` literal, such as
`include_bytes!("../../../assets/fonts/Inter-Regular.ttf")` or
`concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/preferences.md")`. Deleted and renamed inputs
still select their readers, including when the input's entire directory no longer exists.

The affected set is those packages plus everything that depends on them (transitively through
normal and build dependencies) and every package that uses one of them as a dev-dependency.
Clippy and tests use the same dev profile as full CI. Both modes use the same test runner:
nextest plus doctests when installed, otherwise cargo test plus doctests. GPU tests run in one
libtest process, and the translation report reruns with visible output when ui-egui is affected.
The audio-backend probe runs before building affected crates.

Formatting, PDF dependency policy, lock-patch and layer checks always run. Assets are checked
when an asset, an `.attribution` sidecar or `ATTRIBUTION.md` changes. The wasm check selects
affected L0–L4 and web crates. Every step runs even when an earlier step fails; the final result
names failed steps and includes timings.

Changes to the root `Cargo.toml`, `Cargo.lock`, `.cargo/`, `xtask/`, `rust-toolchain(.toml)`,
`clippy.toml` or `.config/nextest.toml` can change every crate's build or test policy, so they run
the full CI instead and say why. Changes no crate reads (most of `docs/`, `README.md`, `.github/`,
`packaging/`) only get the cheap checks.

Limits: a crate that reads a repo file at run time through a path that is not a `../` literal in its
sources is not detected, and building a subset of packages can unify features differently from the
whole workspace. Non-UTF-8 Git paths stop the command rather than silently omitting checks.
Run the full `cargo xtask ci` before a release.

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
  which read the clock are declared time-dependent. `crates/effects/tests/it/sim_golden.rs` pins
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
`--repeat N` runs every scenario N times; with N > 1, one `"kind": "summary"` line per key follows
the runs. Its `metrics` are the medians of the successful runs (byte and instruction counts
rounded), `summary` holds `runs`, `failed` and median/p90/min/max per metric (p90 is the nearest
rank, so it equals the max for N < 10), and its other fields (`date`, `load`, ...) are those of the
key's last run; scripts that average runs should skip `kind: summary` lines. One failed run marks
the summary failed. Comparisons use the latest line per key, so a key is a median only if its
summary is the latest line (a later single run, or a batch that aborted before its summaries,
leaves a raw run latest). The table labels each side `median of N` or `1 run` and warns when they
differ; `--fail-over` refuses such keys.
The table notes the 1-minute load average: on a busy machine wall time is noise, so compare
peak memory and instructions retired, and rerun when the load differs a lot.

Results files allow blank lines; corrupt records or invalid comparison fields report the file
and line instead of being discarded. `--fail-over PCT` requires the same run keys on both sides,
successful latest runs, and at least one comparable resource metric per run. Failed latest runs,
missing coverage, empty results and wall-time-only measurements cannot pass the gate. Omit
`--fail-over` for an exploratory partial comparison; failed latest runs remain visible.

## Unexpected desktop exit diagnostics

The desktop starts a separate, lightweight reporter before loading fonts, projects or the GPU.
A normal exit (including a handled startup error) sends a clean marker only after desktop
teardown. macOS finishes at the end of its successful exit hook because its native event loop
terminates without returning to main; other platforms finish after the event loop returns.
A teardown panic leaves reporting armed. An abort or other unexpected process termination
closes the pipe without that marker.
The helper saves a local report under the session log folder's `Crash Reports` directory and
shows a native error dialog. Control-server launches save the report without a modal dialog.
Reports contain version/platform, UTC detection and observer-start times, observed duration,
GUI uptime at the last target-present sample, and at most the last 64 KiB of the matching
session's log. A separate ring retains the last 32 timestamped, fixed-vocabulary backend and
GPU observations even if startup messages have fallen out of the log tail. `WGPU_BACKEND`
values are normalized to known backend names; arbitrary environment text is not copied.
Logging filters and the session file size cap can omit later events from the log tail.
They are never uploaded automatically. Review logs for private project paths before sharing.

On Windows the helper acquires a process handle before acknowledging readiness and retains
it until the GUI exits. It records the real OS exit status in unsigned hexadecimal, with a
small lookup of known status names. These bits are **not a verified exception record**:
software can explicitly set the same code, and an access violation does not identify the
faulting module or prove a driver crash. Other platforms report exit status as unavailable.
A lost pipe while the GUI is still present, or a failed/absent process refresh, is not reported
as confirmed termination.

The helper samples only the target process and system RAM at roughly one-second intervals,
retaining a count, last-attempt time, sampled resident-memory maximum, Windows private-commit
maximum (virtual memory on other platforms), and system available-RAM minimum. This is a
fixed-size summary, not a memory dump. OS query failure can leave stale or zero values, and
sampling misses short spikes. System commit limits/usage and GPU VRAM budgets are not captured.
No threshold is treated as proof of memory exhaustion. Typed wgpu OOM, internal/validation
errors and device-loss notifications are observations; device destruction is distinguished
from unknown loss, and neither establishes why the process later exited.

Run `cargo test -p effectcraft crash_report::tests` for bounded protocol, normal shutdown,
actual subprocess abort, unrelated child handles, concurrent sessions, missing log/helper,
log rotation, bounded retained events, live-GUI pipe loss, and report-write failure coverage.
Windows also tests a real subprocess exit with explicit `0xC0000017` bits, without exhausting
memory or pretending that the exit was an actual OOM exception. The three ignored tests are subprocess entry
points launched by these tests, not manual tests.

These are observations, not memory recovery or a causal diagnosis: a pipe closing does not
prove an out-of-memory failure. System-wide exhaustion, killing the whole process tree,
or an unavailable native dialog service can also prevent the helper from reporting. Existing
auto-save recovery remains responsible for restoring previously saved work.

Native exception records, faulting-module attribution and driver/TDR confirmation require
separately collected OS event evidence or a native dump. They are deliberately not gathered
here: dumps can contain document contents and credentials. Any future dump workflow must be
explicitly opt-in, bounded, local-first and separately reviewed before sharing. This reporter
must remain useful when those sources are unavailable.
