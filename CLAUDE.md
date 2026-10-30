# EffectCraft — instructions for agents

EffectCraft is a clean-room, open-source, pure-Rust motion graphics and visual effects compositor targeting Adobe After Effects parity (and beyond). Native on macOS, Windows, Linux; web via WASM. Sibling of `../photocraft` (Photoshop), `../pdfcraft` (Acrobat), `../vectorcraft` (Illustrator), `../filmcraft` (Premiere), `../lightcraft` (Lightroom) and `../designcraft` (InDesign), with the same conventions.

## Start every session here
`plan/` is maintainer-local (gitignored). Public equivalents: [`ROADMAP.md`](ROADMAP.md), [`docs/`](docs/).
1. Read `plan/STATUS.md` (current milestone, next task, running agents, blockers).
2. Read the task in `plan/execution-plan.md` §3, the relevant section of `plan/architecture.md`, and the crate README/docs you touch. Feature ids: `plan/aftereffects/feature-catalog.md`; effects: `plan/aftereffects/effects-list.md`; UI reference: `plan/aftereffects/README.md`.
3. Read [`docs/gaps.md`](docs/gaps.md): every known shortfall, ranked, and the prioritised workstreams; the numbers are in [`docs/target-app-parity.md`](docs/target-app-parity.md). Fidelity, reliability and stability (G1–G3) come before new features.
4. Follow the autonomous operation protocol (`plan/execution-plan.md` §7). Don't stop to ask.

## Non-negotiables
**Read [`AGENTS.md`](AGENTS.md) first; its rules override everything here.** No Adobe assets, every asset licensed + attributed (`cargo xtask assets`).
- **Never crash** (AGENTS.md "Never crash"; outranks feature work: a crash loses people's work). Non-test code has no `unwrap()`/`expect()`/`panic!`/`unreachable!`/`todo!`/`unimplemented!` and no `unsafe`: return `Result` and use `?`, `ok_or`, `let … else`. Input-derived indices, slices and arithmetic use `get()` and checked/saturating math; recursion is depth-limited; locks handle poisoning. The app's last-resort guard turns an escaped panic into an error and keeps the document. Every crash fix gets a regression test. Standard: [craftrules `standards/never-crash.md`](https://github.com/storytold/craftrules/blob/main/standards/never-crash.md).
- **Clean-room.** Behaviour and public docs only. No GPL/LGPL/AGPL code. ffmpeg only as an external test oracle.
- **Pure Rust.** Media codecs come from FilmCraft crates (git deps behind `crates/media`, see `plan/adr/0001`).
- **Layering** (`cargo xtask layers`): nothing below L5 depends on egui/eframe/winit/rfd/cpal; L0–L4 build for wasm32.
- **Exact time:** `effectcraft_time::Tick` (254 016 000 000/s). Keyframe times are **layer time**.
- **Time goes through `chrono`** (workspace dependency). Never hand-roll UTC/calendar conversion (days-to-date algorithms, leap-year math); a crate owns only its output format string. The one exception is range extension: past the years chrono can represent, shift the date by whole 400-year Gregorian cycles (146,097 days, after which the calendar repeats), let chrono compute the month and day, and add the cycles back to the year (splitting seconds into hours, minutes and seconds is plain arithmetic).
- **Property tree:** everything animatable is a `Property` in the layer's `PropGroup` tree, addressed by paths (`transform/position`, `effects/#1/blurriness`, `@uid`).
- **Everything is a command** (`crates/engine`): id, label, menu path, shortcut, params, enabled(), run(). UI, CLI, control channel and MCP dispatch by id.
- **Everything is agent-drivable:** every interactive widget registers an automation id; UI state is serde.
- **Community links** (Help menu, About, header Discord button): https://discord.gg/artcraft, https://getartcraft.com, https://getartcraft.com/apps/effectcraft, https://github.com/storytold/effectcraft.
- **Quality gates** before every commit: `cargo xtask ci` (fmt, clippy -D warnings, tests, layers, assets, wasm).
- **New workspace crates** must include `[lints] workspace = true` in `Cargo.toml` to inherit `[workspace.lints]`. Production crate roots must declare the required crate-level lint attributes before modules or other crate items; crate-level `//!` documentation may precede them.
- **Commits:** one task id per commit (`M6.3: trim paths`). Only green states.

## Running and looking at the app
- `cargo run -p effectcraft -- --control 9877` opens the desktop app with the JSON-lines control server (`docs/control-protocol.md`).
- For UI work, **look at the result**. Headless (no window, works while the user is busy on the machine): `cargo run -p effectcraft-ui-egui --example snapshot -- --out ui.png --step '{"method":"engine.execute","params":{"command":"layer.select","params":{"layers":["#2"]}}}'` (steps are control-channel requests; `--script steps.jsonl`; `{"method":"snap","params":{"path":…}}` for intermediate shots). Or drive the live app via the control channel and take `ui.screenshot` (needs the window visible on screen).
- After Effects reference: drive AE via ExtendScript (`osascript -e 'tell application "Adobe After Effects 2026" to DoScriptFile "x.jsx"'`) and capture only AE's window (`screencapture -l <windowid>`); keep captures in `plan/aftereffects/ref/` (gitignored), never commit them, never capture the Home screen.
- Parallel agents: separate git worktrees and `CARGO_TARGET_DIR=target/agent-<name>`; keep every `Cargo.toml` valid (the `crates/*` glob).

## CI workflows
- Workflows in `.github/workflows/` call the shared workflows in [`storytold/craft-common-actions`](https://github.com/storytold/craft-common-actions) at `@main`, with their inputs and secrets. No other jobs: if no shared workflow does what's needed, it doesn't go here.
- Don't add or change CI in a feature or fix PR. If the existing workflows can't build or test a change, link an existing craft-common-actions issue or PR, or open a new issue, or a PR only for a small fix that is safe for every app and does not increase CI time.
- Some workflow files here still run their own jobs. Don't add to them. Moving a job into craft-common-actions is its own task, never part of another PR, and the job stays here until a shared workflow runs it.
