# Linked compositions: review and validation notes

This proposal adds saved-project compositions as footage-like sources. The destination owns
layer instances; source layers and dependency items remain outside its asset list. The first
version includes multi-comp import, manual Reload Footage, stable composition identities,
offline errors, preview/render/audio, and destination-side color interpretation.

The supported subset and identity migration proposal are in
[linked-compositions.md](linked-compositions.md). Automatic watching, external-link nesting,
relinking, Make Local, arbitrary OCIO configurations and fine-grained linked-frame caching are
follow-ups. This change does not claim to complete the application's broader OCIO work.

## Review decisions

Confirm these before merging a production feature:

1. Save As/increment-save preserve identity; an explicit fork workflow would issue new IDs.
2. Older sources acquire IDs only through their own successful save. Older writers can discard
   these additive schema-2 fields; maintainers must decide versioning and downgrade behavior.
3. Native entropy and browser Web Crypto are required. Native cross-process regression and
   WASM compilation do not substitute for a browser runtime compatibility check.
4. Agree on the source identity and worker transport boundary with
   [Craft Link #3138](https://github.com/storytold/effectcraft/pull/3138). Its cross-application
   integration and this saved `.ecproj` feature should share policy where they overlap.

No maintainer agreement or remote submission is implied by this local proposal.

## Hardening coverage

- Save/Save As/increment-save refuse linked project, media, sequence and proxy overwrites;
  normalized paths, native aliases, missing sources, failed refreshes and undo/redo are covered.
- Composition depth uses memoized subtree height, so shared tails cannot evade the limit;
  cycles and absent compositions are rejected.
- Worker snapshots use a versioned shared-source table before serialization, bounded to
  64 MiB total, eight sources, 16 MiB per source and 64 links. Malformed tables, duplicate IDs,
  nested envelopes and excessive item/layer counts are rejected. Divergent saved copies with
  shared IDs remain distinct.
- Native identities are checked across fresh processes. Missing/duplicate persistent IDs
  cannot be saved. Browser builds select the Web Crypto entropy backend.
- Mixed Interpret Footage selections preserve individual settings unless explicitly edited.
  Offline source paths/reasons are visible and accessible; unmanaged color is labelled assumed.
- Cache fallback applies to comps that can read links, including expression references;
  unrelated comps can retain their normal caching.

## Reproduction

Use a disposable checkout. Some pre-existing demo tests replace their own generated projects
under `target/test-out`; never run those tests over artist-edited demo files.

```sh
cargo xtask ci --affected --base 3376eaa06a0b0eb28bff571ccbd94c060dc2c6dd
cargo test -p effectcraft-cli --test it linked_movie_refresh_and_cli_export -- --nocapture
```

On Windows, a full workspace test build can try to replace the running `xtask.exe`. For that
case, build xtask in this checkout, copy its executable outside the Cargo output location,
and run that copy with `ci --affected --base ...` from the same checkout. Pin
`RUSTUP_TOOLCHAIN` to the installed validation toolchain so dependency directory overrides do
not select a different compiler. This runs the same gates without changing repository CI.
Close the sandbox application before a build replaces its executable.

The native media regression authors its RGBA PNG sequence and stereo PCM WAV in Rust. The
shipping CLI encodes a ProRes 4444 movie; the native host imports that movie into a 24 fps
source, links it into a 30 fps destination, warms rendering/audio, changes saved source
dimensions/duration/opacity, replaces the hidden movie, tests missing project/media recovery,
then exports and decodes a second movie. Assertions cover changed pixels, partial alpha,
transparent canvas, audible audio, silent trims and unchanged destination layer settings.
It requires no downloaded media or external encoder. Generated files are retained under
`target/test-out/linked-native-media` for inspection.

## Results

Validation used Windows x64, Rust 1.99.1, base `3376eaa06`, a disposable checkout with the
feature changes overlaid, and separate temporary/configuration directories. Existing artist
demo projects were not used by the tests.

The complete `ci --affected` selection expanded to full workspace CI because `Cargo.lock`
changed. The completed run took 13 minutes 5 seconds. Formatting, workspace all-target
clippy with warnings denied, audio-backend policy, PDF dependency policy, lock-patches,
layers, attribution (43 assets), and WASM compilation (31 crates) passed. **The test gate
failed. There is no claim of a green full CI run.**

The run exposed identity-sensitive parser comparison tests and stale menu/error/command
expectations. Those were corrected while preserving content comparisons and behavior
assertions. Final focused verification is listed below when complete.

The remaining full-suite failures need separate triage: ordinary media Opus/AAC/rotation
round-trips, a glTF fixture substitution, an automation relative-path fixture substitution,
and viewer/GPU assertions. For the automation fixture, the parsed footage path remained the
permitted inside-root image; the text replacement changed its name rather than its escaped
Windows path. Those fixture lines are unchanged from the base. These are not presented as
proven failures of a pristine base runtime; a separate baseline run is still needed.

### Export blocker

This section records the initial hardening result. The subsequent
[rapid-seek fix](linked-preview-stall.md) addresses the reproduced decoder scheduling
deadlock and passed ten default-parallel export round trips, without the serial workaround.

Cold full-length parallel export stalled before creating the output, both for the linked
30 fps destination and for its ordinary 24 fps source comp containing only a movie layer.
Three-frame exports succeeded. A full 36-frame linked export with `RAYON_NUM_THREADS=1`
completed and ffprobe reported 64 × 48 ProRes 4444 with alpha (`yuva444p12le`), 30 fps,
1.2 seconds, plus 48 kHz stereo PCM (57,600 sample frames).

Source inspection identifies a plausible existing deadlock: a movie frame holds the decoder
mutex while the ProRes decoder runs nested Rayon work in the shared decode pool; another
queued frame of that movie can re-enter the same mutex. This diagnosis is an inference from
code and the ordinary/linked/serial comparisons, not a captured native stack trace. Decoder
code was left unchanged. The regression retains its assertions and now bounds each export
subprocess to 30 seconds so a stall fails instead of hanging CI indefinitely.

At that point, parallel export remained blocked. After the preview follow-up, a clean baseline
comparison, browser runtime entropy, and maintainer agreement on identity compatibility still
remain gates before submission/merge. No
test was removed, disabled or newly ignored to obtain a passing result.

### Focused verification and performance

After the fixes, workspace all-target clippy with `-D warnings` passed again, and every
workspace test binary built successfully. Focused execution then passed:

| Check | Result |
| --- | --- |
| Engine library | 1,292 passed; six pre-existing ignored tests |
| Project model / transport / native IDs | 86 passed |
| AEP parser/import library | 90 passed |
| Linked desktop UI workflows | Nine passed; none ignored |
| Native movie refresh/export/decode, `RAYON_NUM_THREADS=1` | One passed; none ignored |
| Native movie refresh/export/decode, default parallelism | One bounded run passed; the next repeat timed out |

The native media pass includes all video, alpha, audio, local trim/transform, changed-size/
duration, replacement-media and missing-file recovery assertions. Its serial configuration
is explicit; it does not resolve or replace the failed default parallel export result.
The final sandbox application and CLI build also passed under their existing executable names.

A bounded parallel run passed its complete assertions, but the next repeat stalled and
failed in 31.16 seconds through the new export timeout. Earlier full-length linked and
ordinary source exports also stalled without a decoder code change. This is an intermittent
failure requiring investigation, not a resolved issue.

The CPU benchmark used a Ryzen 9 5950X (16 cores/32 logical processors), the dev build, and a
960 × 540, 30 fps composition with 120 static 160 × 90 solids, each with Gaussian Blur 12 and
35% opacity. Three fresh processes per case rendered three uncached warm-up samples, then
30 playback frames without/with a layer cache, rotating case order. The numbers below are
the median of the three reported cached-playback frame medians; they are diagnostic dev-build
timings, not release performance targets.

| Case | Median frame time | Layer cache after 30 frames |
| --- | ---: | --- |
| No link | 507.16 ms | 3,480 hits; 120 entries; 47.4 MB |
| Same comp, unused linked asset | 544.21 ms | 3,480 hits; 120 entries; 47.4 MB |
| Linked 320 × 180 comp placed above those layers | 670.63 ms | Bypassed; zero entries |

Unused links retain caching, with about 7.3% overhead in this sample. The used-link case is
about 32.2% slower than the baseline, combining the extra source render and the conservative
cache bypass. This comparison does not isolate those two costs. Reach scanning and cache keys
remain performance follow-ups; broad cache disabling is reduced, not fully solved.

### Demonstration

The native test retains three rendered checkpoints: `before.png`, `source-refresh.png` and
`media-refresh.png`. They show the destination before source editing, after a saved source
opacity/size/duration change plus Reload Footage, and after hidden source movie replacement
plus Reload Footage. Destination placement, scale, opacity and trims are asserted unchanged.
The local review packet includes a nine-second video of these checkpoints, three seconds each.
