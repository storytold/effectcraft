# Expressions and scripting parity

> **Last reviewed:** 2026-10-12 · **Last updated:** 2026-10-12 · **Change:** minor (`#include` / `#target` removed from the estimate (landed in #759)) · **Target:** Adobe After Effects 2026

Expressions and ExtendScript are how After Effects work is automated and how much of it is
built: rigs, templates, studio pipelines. They are also what carries over from After Effects
today, while `.aep` projects can't. Architecture: [architecture.md](architecture.md) §5 and §5a;
extension points: [plugins.md](plugins.md); overall numbers:
[target-app-parity.md](target-app-parity.md).

**Breadth ≈ 93%, ready ≈ 65%** (estimated). The object models are written from Adobe's public
Expression Language Reference and Scripting Guide (behaviour only, AGENTS.md §2) and tested with
our own scripts; no real-world script library has been run against both apps.

| Feature | After Effects 2026 | EffectCraft | Status | Evidence / gap |
|---|---|---|---|---|
| Expression language | JavaScript engine (default) and Legacy ExtendScript | JavaScript (boa) with legacy array maths (`value + [10, 0]`) | good | `crates/expr` tests; no separate Legacy ExtendScript engine mode |
| Expression object model | `thisComp`, `thisLayer`, `wiggle`, `loopOut`, `valueAtTime`, `sampleImage`, `footage()`, text style API… | Same members, from the reference, including `propertyGroup(countUp)` and a group's `propertyIndex` for name-independent rigs (#1619) | good | `crates/expr/src/tests.rs`, `tests/property_group.rs`; behaviour of edge cases unmeasured (G1) |
| Expression editing | Inline field, error bar, pick whip, Expression Language menu, external editors | Inline field that grows, error bar, pick whips (property and value), language menu | partial | External editor support (#364) |
| Expression performance | Cached, multi-frame | Per-thread boa contexts | unknown | No benchmark |
| Scripting object model | `app`, `Project`, items, layers, properties, keys, eases, render queue, `TextDocument`, `Shape`, markers, `File`/`Folder`, `Socket`, `app.settings` | Same classes, edits run through undoable engine commands; match names table; layers, groups and properties are callable (`layer("Effects")(1)` is `.property(...)`) | good | `crates/script` tests (≈ 10 test files) |
| `$.evalFile`, `BridgeTalk` | Evaluate another script file; identify the host application and message other Adobe applications | `$.evalFile` evaluates a file relative to the calling file (nested at most 32 deep, under the scripts' file gate) and returns its last value; `BridgeTalk` names the host honestly (`appName` `effectcraft`, its version); messaging other applications throws "not supported" | partial | `tests_evalfile.rs` (#760); no inter-application messaging |
| ScriptUI | Dialogs, palettes, dockable panels, resource strings, `onDraw` / ScriptUIGraphics, mouse events | Same, dockable panels reopen at launch | good | `ui_scriptui`, `tests_ui*` |
| `app.scheduleTask` / `cancelTask` | One-shot and repeating tasks (polling, panel refresh) until `cancelTask` | Same: repeating tasks run every `delay` ms (at least 50 ms, at most 64 pending per script) until `cancelTask`, until they throw (reported once, then cancelled), or until their owner goes: a script's last window closing, or the project closing for scripts without windows; frontends tick them every frame, agents with `script.runTasks` | good | `tests_tasks.rs` (#773); one-shot tasks of a script file still run right after it, not after their delay |
| Scripts menu, ScriptUI Panels folder | yes | yes | same | [plugins.md](plugins.md) |
| Startup / Shutdown folders | yes | not run | missing | plugins.md |
| `#include`, `#target` directives | yes | `#include` / `//@include` / `#includepath` inline files relative to the including script (once each, nested at most 32 deep, under the scripts' file gate; errors name the included file and line); `#target`, `#targetengine`, `#strict` are ignored | good | `tests_include.rs` (#759); installed scripts and panels have no folder of their own, so they include from the project's folder or absolute paths |
| `.jsxbin` | yes | no | out of scope | No public specification |
| Keyboard shortcuts for scripts | yes | no | missing | plugins.md |
| Command-line rendering `aerender` | yes | `effectcraft-cli render`, `script`, `exec` | same purpose | `apps/effectcraft-cli` |
| Essential Graphics scripting hooks | `addToMotionGraphicsTemplate…` | yes (M13.7) | good | — |
| Agent control (MCP, control channel, every command) | none | yes | beyond | [agents.md](agents.md), [control-protocol.md](control-protocol.md); `list_effects` omits parameter types and ranges (#441) |
| Real-world script corpus | — | none run | gap | Run popular free scripts (the 11 sample scripts After Effects ships are a good start, run as behaviour tests, never copied) |

## Estimate

**≈ 20–40 h**: `#include` / `#target` (landed, #759) and Startup / Shutdown folders 4–8 h as first estimated (Startup / Shutdown remain), script shortcuts 2–4 h,
a corpus of public scripts and expressions with the fixes it finds 10–20 h, an expression
performance benchmark 4–8 h.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-16 | minor | Expression object model row names `propertyGroup()` and a group's `propertyIndex` (#1619) |
| 2026-10-12 | minor | Estimate no longer counts `#include` / `#target` (#759, `tests_include.rs`); the table row was already current |
| 2026-10-10 | major | First expressions and scripting checklist |
