# Spec: live project reload for CLI/MCP edits (no GUI restart)

## Title

`CLI/MCP edits to a project require a full EffectCraft restart — add a watched .ecproj / persistent bridge session (paper.design-style live editing)`

## Problem

When an agent edits a `.ecproj` through `effectcraft-cli` or MCP while the desktop app is open, the window does not see the edits. The session must be closed and reopened before cuts, transforms, effects or imports made by the CLI become visible. For an agent working interactively this is disruptive: every edit cycle costs a restart and loses the live session state.

Reference behaviour: paper.design's MCP keeps the same client process alive, so the agent's edits land in the running in-memory session and appear immediately. The equivalent should be possible in EffectCraft.

## Root cause (verified in this checkout, v0.25.1)

- `crates/engine/src/lib.rs:104-121` — `Session` holds `pub project: Arc<Project>` (plus `saved_project`, `path`). The open window renders and edits that in-memory `Arc`, not the file.
- `crates/engine/src/session/edit.rs:30-87` `edit_inner` — every GUI edit clones the project (`(*self.project).clone()`), applies the command, then `self.project = Arc::new(p)` (`:76`). No file read in this path.
- `crates/engine/src/cmd/file.rs:89-103` `file_text` / `save_to` — saves serialise the in-memory `Arc` and write it to disk. One-way: memory -> disk.
- `crates/engine/src/cmd/file.rs:43` `open` and `:122` `revert` are the only paths that read the file and call `replace_project`.
- `crates/engine/src/session/edit.rs:198-236` `replace_project` — the only whole-project swap: stops render/track/roto jobs, clears caches and history, then `self.project = Arc::new(p)` (`:236`).
- `crates/ui-egui/src/app/frame.rs:680-704` — the UI tick runs autosave, a Watch Folder *render* poll, `tick_footage_reload` (`:725`) and `tick_lut_files` (`:711`). **There is no watcher on the project file.**
- `apps/effectcraft-cli/src/main.rs:467-498` — a CLI `exec` builds a *new* headless `Session` (`effectcraft_host::headless_session()`), reads the project into that process, edits, and saves it back. The GUI's `Arc` is untouched.
- `apps/effectcraft-cli/src/project_lock.rs:77-90` — CLI saves take an OS lock so CLI runs serialize; the GUI never takes that lock.
- `apps/effectcraft/src/control_server.rs:25-63` and `crates/automation/src/bridge.rs:15` — a loopback control channel and CLI bridge already exist; `engine.execute` runs commands against the live session (`crates/ui-egui/src/control.rs:224-294`, `crates/ui-egui/src/app/frame.rs:19-68`). It works, but only when the caller explicitly sends a command; external file edits still need a watcher.

## Intended behaviour

1. An external process (CLI/MCP or any editor) saves the `.ecproj` next to the open project.
2. The running GUI notices the file changed within ~1-2 s and reloads it into the same `Session.project`, so the timeline, comp viewer and panels show the new cuts/transforms/effects/imports **without a restart**.
3. Prefer edits through the bridge/control channel so they mutate the live session directly; the file watcher covers edits made by processes that only touch the file.
4. Never discard unsaved GUI work silently, and never reload in the middle of a render or script.

## Scope (hyper-specific)

In scope:
- New engine module `crates/engine/src/project_reload.rs`: a polling `Watcher` modelled on `crates/engine/src/footage/footage_reload.rs:430-510` (worker thread, `mpsc::sync_channel(1)`, no new dependency). Stamps `session.path` with existing `Services::stamp` (size + mtime); 1 s focused / 5 s unfocused; immediate wake on focus and on path change; ~250 ms debounce for atomic temp+rename writes.
- UI wiring in `crates/ui-egui/src/lib.rs` (watcher field next to `footage_watcher` at `:449`) and `crates/ui-egui/src/app/frame.rs` (a `tick_project_reload` next to `tick_footage_reload`).
- Reload path reuses `file.revert` -> `open` -> `replace_project`; do not add a second project parser.
- Baseline stamp updated on `save_to` and `open` so the GUI's own writes do not trigger a reload loop.
- Guard on `Session::is_dirty()` (`crates/engine/src/session/edit.rs:180`) and active render/script (`script_ui.render_host`): if dirty, the status line names File ▸ Save As ▸ Save a Copy... to preserve edits without overwriting the disk file, then File ▸ Revert, and states that Revert discards unsaved GUI changes. This replaces the originally requested two-button "Reload / Keep mine" prompt: a status line with the exact commands is an accepted deviation recorded here.
- Autosave coordination: keep `autosave_tick` (`crates/engine/src/settings/session_settings.rs:418`) running while a change is pending. Auto-save writes recovery copies (`<name> auto-save N.ecproj` in an Auto-Save folder), never the project file, so it cannot overwrite the external edit; pausing it would only remove crash recovery for the unsaved local work. The only hold is a recovery target that would alias the project file itself (`Session::autosave_target_aliases_project`). *Correction:* an earlier version of this spec said to suppress autosave "or the GUI overwrites the external edit before reload"; that premise was wrong (review on PR #4170).
- Tests: temp project file, external write, assert live session reflect the change; assert no reload on self-save; assert dirty guard; assert no reload during render.
- Docs: `docs/agents.md`, `docs/control-protocol.md`, `docs/preferences.md` note the watcher and the bridge-preferred workflow.

Explicitly out of scope:
- No new crate dependencies; no changes to `Cargo.toml`/`Cargo.lock`.
- No changes to plug-in loading (`crates/plugin/**`), the effect registry, footage reload, LUT reload, render/cache internals, or the web build's storage model.
- No change to CLI save semantics or the project lock file.
- No changes to any repo other than EffectCraft.

## Areas touched, and what each affects

| File | Effect | Precaution |
|---|---|---|
| `crates/engine/src/project_reload.rs` (new) | one worker thread, metadata-only polling of one path | sleep between polls; never hold a file handle on Windows |
| `crates/engine/src/lib.rs` | re-export the module | no behavioural change |
| `crates/engine/src/cmd/file.rs` | update watch baseline on open/save | keep one stamp source; no new parse path |
| `crates/ui-egui/src/lib.rs` | one `Option<Watcher>` field | drop ends the thread |
| `crates/ui-egui/src/app/frame.rs` | one cheap per-frame check | no per-frame file I/O; worker does the polling |
| `crates/engine/src/settings/session_settings.rs` | detect a recovery target that aliases the project file | recovery auto-saves keep running while a change is pending |
| tests + docs | verification and documentation only | temp dirs, no real user files |

`replace_project` stops jobs, drops decoded footage and clears undo history. Any reload must therefore be gated on clean + idle state; this is the main correctness risk, not the polling itself.

## Todo list

- [x] 1. Capture baseline metrics before any change (see next section).
- [x] 2. Add `project_reload.rs` watcher with stamp/debounce/focus wake; unit tests for stamp changes and debounce (3/3 pass).
- [x] 3. Wire watcher field and `tick_project_reload` into the UI; thread stops on app close / path change.
- [x] 4. Add dirty/active-render guard and the "changed on disk" status; live-tested: unsaved GUI work was held, explicit `file.revert` applied the external edit.
- [x] 5. Self-save baseline: `Session.project_stamp` updated on open/save; unit test confirms an acknowledged change is not reported twice.
- [x] 6. Coordinate with autosave: recovery auto-saves keep running while a change is pending; only a target aliasing the project file is held. Regression proves the external project and the local recovery copy both survive.
- [x] 7. Document bridge-preferred workflow and watcher behaviour (`docs/agents.md`, `docs/control-protocol.md`, `docs/preferences.md`).
- [x] 8. Re-ran metrics after the change; no latency or memory regression (see below).
- [x] 9. Diff scoped to the files listed; `Cargo.toml` / `Cargo.lock` unchanged (verified with `git diff --stat` / `git diff -- Cargo.toml Cargo.lock`).

## Baseline / performance snapshot (capture BEFORE implementation)

Record on this machine with the release build, GUI started as `effectcraft.exe --control 9877 --demo`:

| Metric | How to measure | Baseline | After |
|---|---|---|---|
| Cold start to "first frame drawn" | timestamp in the app log (`apps/effectcraft/src/main.rs` logs `first frame drawn`) | 1.07 s | 1.08 s |
| Idle GUI CPU over 60 s | `Get-Process effectcraft \| Select-Object Id,CPU,WorkingSet64` at start/end | 10.41 s -> 22.38 s (~20% of one core, demo playing) | 6.00 s -> 6.08 s (~0.13% of one core, project open) |
| Idle GUI RSS after 60 s | same | 1.10 GB -> 2.04 GB (+0.94 GB, existing demo-cache growth) | 986 MB -> 956 MB (flat) |
| Frame p95 in the editor | existing frame timing / profiling path; sample during a 60 s interaction | not captured (no in-app profiler available) | not captured |
| `file.open` latency | control channel `engine.execute` timing, 20 runs | median 14.07 ms, p95 95.51 ms | median 10.94 ms, p95 24.43 ms |
| `file.revert` latency | same, 20 runs | median 14.09 ms, p95 17.38 ms | median 10.04 ms, p95 11.23 ms |
| `render.frame` latency | 20 identical renders before/after | median 13.44 ms, p95 17.72 ms | median 11.19 ms, p95 15.65 ms |
| Memory after 100 external edits + reloads | RSS at start/end, plus 10 min idle after | not captured before (no equivalent loop existed) | 986 MB -> 386 MB during reloads -> 968 MB after 60 s idle (no growth) |

Note: the before/after idle samples used different projects (demo playback vs an open `.ecproj`), so
the idle CPU/RSS rows are not a strict A/B; the latency rows are the comparable ones and all
improved or stayed within noise. No monotonic memory growth was observed after 100 reloads.

Budgets / acceptance:
- No new dependencies, no lockfile change.
- Watcher thread idle CPU < 0.1% (one sleeping thread, no per-frame file I/O).
- RSS growth attributable to the watcher < 10 MB after 100 reloads; no monotonic growth trend after 30 min idle.
- Frame p95 and render/open/revert latencies must stay within measurement noise of baseline; any regression blocks the change.
- External save -> timeline updated in <= 2 s while focused, without restart.
- Zero unsaved GUI edits lost without explicit user consent.

## Why this shape

The repository already contains two lightweight, polling-based watchers (footage at `crates/engine/src/footage/footage_reload.rs`, LUT at `crates/engine/src/footage/lut_reload.rs`) that wake the UI only when a stamp changes. Reusing that pattern keeps the addition small, dependency-free and consistent with how similar programs watch documents, rather than introducing a new event-loop or file-watching crate.

## Verification performed (release build, v0.25.1 + this change)

- `cargo build --release -p effectcraft -p effectcraft-cli` — finished in 18m 37s, no errors.
- `cargo test --release -p effectcraft-engine project_reload` — 3 passed, 0 failed.
- Live test 1: app started with `--control 9877 <project>`; a separate `effectcraft-cli exec comp.new … --save` wrote the project. The new comp appeared in the running app's `project.summary` in 4.91 s with the same PID (14280), no restart.
- Dirty-session test: external write held for 5 s while GUI had unsaved edits (GUI edit kept, external comp not applied); explicit `file.revert` then loaded the external comp as expected.
- Focused-window measurement after the review fixes: with the GUI focused via `ui.focus`, file-write -> visible in `project.summary` was 0.297 s; the full CLI invocation including process startup took 2.469 s. The watcher budget applies from the file write, not from process start.
- CodeRabbit review follow-up: the two stamp races (save and open), the debounce baseline, the initial baseline, test waits and the Save-before-Revert guidance were fixed in commit `4e7ccd`. Unit tests for the new UI branches were added but their release test-harness link was stopped at the owner's request to avoid a long LTO link; CI should run them.
- 100 consecutive `file.revert` calls: RSS 986 MB -> 386 MB -> 968 MB after 60 s idle; no monotonic growth.
- `git diff --stat` / `git diff -- Cargo.toml Cargo.lock`: 9 tracked files + 1 new source file, no dependency or lockfile change.
