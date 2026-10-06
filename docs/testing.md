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
  fails (re-pin with `SIM_GOLDEN_PRINT=1` after an intended change; pinned for aarch64 macOS).
- **Export** (`crates/export/tests`): every format is encoded and decoded back, checking frame
  count, size and pixels. When `ffmpeg`/`ffprobe` are installed they are used as an outside
  check; they are never linked or shipped.
- **Automation** (`crates/automation`, `apps/effectcraft-cli/tests`): MCP protocol round trips and
  the command-line tool's JSON output.
- **Scripting** (`crates/script`): scripts edit and read back the engine object model. Solid creation
  preserves explicit source pixel aspect and optional duration, including non-square compositions;
  invalid pixel aspect values are rejected before creating a layer, source or Solids folder. The
  documented range is 0.01–100 ([Adobe scripting guide, page 98](https://fendrafx.com/wp-content/uploads/After-Effects-CS6-Scripting-Guide.pdf#page=98)).
  Omitted pixel aspect retains EffectCraft's existing composition-default convenience.
- **Interface** (`crates/ui-egui`): headless egui_kittest tests for panels and dialogs.

## Looking at the interface without a window

```sh
cargo run -p effectcraft-ui-egui --example snapshot -- --out ui.png \
  --step '{"method":"engine.execute","params":{"command":"layer.select","params":{"layers":["#2"]}}}'
```

Each `--step` is a control-channel request; `{"method":"snap","params":{"path":"x.png"}}` writes an
intermediate image.

## Benchmarks

Preview dispatch regressions run with
`cargo test -p effectcraft-ui-egui --release --lib frames::tests`. Paused edits/scrubs allow
two interactive renders and retain only the newest pending request, including same-revision
scrubs. Revisiting a cached or already rendering frame cancels the obsolete pending edit;
queued prefetch is promoted without duplication. Playback uses its separate urgent path.
Promoting a queued paused frame to playback wakes native workers even when an earlier capped
worker has already returned. Secondary composition views use independent pane demands on the
same queue: one secondary render at a time, with a shared two-render limit for paused/secondary
work and priority below the primary viewer. Paint epochs retire hidden panes' queued work,
while shared frame keys and running jobs retain their owners.
Passive Composition panels use the same one-secondary-render limit. Scheduling age belongs to
the pane, so continuous scrubbing cannot repeatedly put the first-painted auxiliary view ahead
of the others. Dynamic pane demand is capped at 64 slots, with four reserved fixed auxiliary/locked
slots; retirement frees capacity and requests a repaint so previously denied visible panes can retry.
Synthetic CPU fixtures cover shared ownership, promotion, fairness, capacity, pane closure and reuse
of installed pixels after raw-cache eviction.
Tiny CPU footage held by channels verifies that 100 requests render only the two held frames
and the final requested frame, without another viewer request. Other regressions inject a frame
failure, discard remote callbacks, consume a completion repaint before remote capacity is freed,
and retry the same key. They check queue liveness and ownership, not elapsed-time speedups.
Viewer tests hold original 2×2 footage behind a channel and require auxiliary/locked pane
redraws to return before releasing that render. Additional CPU-only fixtures check custom cameras,
draft/nested preferences, content reuse across unrelated edits, distinct subframe ticks, and
display-profile changes without altering shared raw pixels.
An installed pane texture survives raw RAM eviction without scheduling a replacement render;
a changed display profile still requests original pixels when conversion needs them.
Renderer-toolbar checks read the actual painted text for an Advanced 3D composition and click
the control for both renderers. The control opens Composition Settings at its renderer tab with
the current draft values and does not change the document merely by opening it. Adobe's public
[preview reference](https://helpx.adobe.com/after-effects/desktop/view-and-preview/preview-video-and-audio/previewing.html)
documents this settings shortcut; the test verifies UI behavior without creating a GPU device.
The toolbar scrolls horizontally when a narrow pane cannot fit its controls. Geometry and pointer
tests check that scrolled controls remain clickable, the toolbar respects its parent panel's clip,
and an invisible Show Snapshot button cannot capture presses in a neighboring panel. They do not
replace visual acceptance on a live desktop display.
Actual memory-purge and same-metadata Reload Footage commands invalidate installed main,
auxiliary/locked and passive textures too. Synthetic pixels prove a same-key request can replace
the old image; late browser readbacks are detached and deliberate comparison snapshots survive a
memory-only purge. Queue tests additionally hold a remote completion across a purge: queued old
work is retired, running slots remain occupied until completion, and pre-purge pixels cannot refill
RAM. These fixtures do not establish complete persistent-cache exclusion for a purge racing with
a write submitted after an already published frame, or invalidate every renderer's intermediate
cache race; those require separate end-to-end checks.

Auto renderer-selection tests use fixed measured costs to check tie/re-probe policy, recovery
from a zero-resolution warm-up estimate, and rejection of invalid timing samples without
altering history. They do not depend on a zero-delay mock winning against a loaded machine.

Viewer zoom-quality changes replace installed main, auxiliary/locked and passive samplers while
preserving shared raw preview pixels. A tiny CPU fixture switches Linear to Nearest without a
second render, and checks that a theme-only change retains installed textures.
Timeline row and duration-bar context-menu fixtures duplicate the selected group when its member
is right-clicked, and duplicate only the clicked layer when it was outside the selection.
`cargo test -p effectcraft-ui-egui --release --test ui_render_queue_overflow` checks mouse-wheel
access to the last item in an overflowing queue and prevents clipped rows from capturing clicks
on the fixed status footer. It also reaches the last expanded item's inline output editor and
scrolls while another panel has focus. It paints only the Render Queue in a CPU egui context: no compositor
or exporter starts. Adobe's [general UI reference](https://helpx.adobe.com/after-effects/desktop/get-started/get-familiar-with-the-interface/general-user-interface-items.html)
documents wheel scrolling in the Render Queue, including when the panel is inactive.
`ui_project_compact` uses only original folders to resize the Project panel through short and
nearly collapsed heights, preserving the item list and finite scroll state. It reproduces the
old scrollbar minimum-height panic without starting thumbnails or a renderer.
`ui_character_scroll` exercises a short Character panel without a renderer: wheel scrolling
reveals Ligatures, clicking changes the document through the command system, and Undo restores
the original settings. Selected-character formatting leaves unselected text unchanged, and
scrolling outside the panel does not move its controls.
`ui_effect_header_contrast` checks the painted effect header against its text color in both
themes, selected and unselected, with a minimum contrast ratio of 4.5:1. These are CPU interaction
and paint checks; live visual acceptance remains a separate check.

`ui_dock_compact` checks small split geometry in both orientations and all sizing modes,
including gutters larger than the available extent and actual gutter dragging. Groups stay
finite, nonnegative and inside their parent; saved split sizes stay valid. These fixtures
reproduce the old inverted minimum-size clamp without launching a window or GPU.

Gaussian Blur clamps evaluated Blurriness to its declared hard range of 0–3000 before deriving
the CPU/GPU sigma; its 0–50 slider range and ordinary-value output are unchanged. Tiny CPU tests
cover non-finite inputs, extreme evaluated values and the existing ordinary-value raster result.
Run the Gaussian tests in debug as well as release to catch radius arithmetic panics. These are
parameter-boundary checks, not GPU fidelity or blur-performance measurements; hostile buffer
scales and other effects' radius calculations need separate coverage. Renderer disk keys use a
new version so pixels from the previous rendering implementation cannot survive as cache hits.

Layer-cache persistence regressions run with `cargo test -p effectcraft-render --release --lib`.
Small original buffers and explicit channel gates verify that RAM pixels are available while
persistence is blocked, shared inputs are retained once, active encoding retains its admission
reservation, failed writes can retry, flush covers encoding, and purge drains work without allowing
concurrent producers to recreate deleted entries. These checks do not use a GPU or timing thresholds.

Native shared layer buffers and owned RGBA frames are compressed on the existing single disk writer.
The viewer publishes RAM pixels before optional frame persistence and checks dimensions/byte limits
before allocating a fallible RGBA copy. Owned-frame tests cover original allocation retention,
independent frame/layer keys, active reservations, full-queue retry, failed writes, flush and purge.
Its admission limit
is 64 queued/active jobs and 1 GiB of conservatively reserved memory, including pinned input capacity
and encoder/sealing scratch. Reservations include worst-case compression expansion and payload
growth. Duplicate keys are suppressed through completion. This is a queue budget;
it excludes RAM preview images and producer allocations before admission. Full or oversized
optional persistence requests are dropped and counted by `cache.diskStats`; RAM/rendered pixels are
unaffected. Browser/custom stores retain their existing shared-hook default behavior. Actual latency
and throughput improvements require separate benchmarks; passing persistence tests does not measure
them.

Disk-cache decoders validate checked dimensions, the LZ4 size prefix, and a 256 MiB uncompressed
entry limit before allocating. Both decompression and the separate float-pixel buffer use fallible
allocation. Native file reads are bounded too; malformed entries (including correctly checksummed
ones) count as misses, are removed, and can be replaced by a later render on native builds. The limit applies to
optional persistence, not composition/render dimensions: 8K RGBA8 frames fit, while larger float
layers render without a disk entry. Tests use tiny original payloads, overflowing dimension metadata,
forged size prefixes and helper boundary checks; they do not allocate the advertised huge images.
Browser frame/layer write admission uses the same raw limit. OPFS reads receive the shared encoded
file limit before calling `File.arrayBuffer()`; `node apps/effectcraft-web/tests/cache-read.mjs`
checks that boundary with tiny mock files, without starting a browser or graphics device.

```sh
cargo run --release -p effectcraft-cli -- bench --n 10 --play 30
```

prints per-layer and per-effect timings for one frame, and playback timings with and without the
layer cache.
