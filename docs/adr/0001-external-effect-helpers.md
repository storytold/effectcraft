# ADR 0001: External effect helpers

- Status: proposed
- Date: 2026-10-11
- Context: review of storytold/effectcraft#508 (an in-process OpenFX host)

## Context

People want to use effects that already exist as native plug-ins: OpenFX suites (Sapphire,
ReelSmart Motion Blur, Twixtor, ...) and, later, other native formats. storytold/effectcraft#508
hosted OpenFX in-process. That needs `unsafe` to call a C ABI and runs closed-source native
code in EffectCraft's own process, which conflicts with two non-negotiables:

- **Never crash** (AGENTS.md): one null dereference in a third-party plug-in takes the whole
  app, and the user's unsaved work, down with it. `catch_unwind` can't catch a segfault, and a
  blocklist only helps after the first crash.
- **No `unsafe`** in the workspace (`unsafe_code = "forbid"`).

## Decision

EffectCraft never loads native plug-in code into its own process. Native effects are served by
**external effect helpers**: separate programs that EffectCraft starts as child processes and
talks to over a small, versioned protocol. The core gains one generic, safe-Rust crate,
`effectcraft-helper-host` (L3); format-specific hosts (OpenFX, ...) live in helper programs
that are built, released and installed separately.

- **Crash containment.** The process boundary contains everything a helper does. A helper
  that exits, hangs past a timeout or breaks the protocol is stopped. The render that hit it
  fails with a message shown on the effect, the frame stays as it was, and the helper starts
  again for the next render. An effect that takes its helper down three times is turned off and
  remembered in `helper_blocklist.txt` until the user retries.
- **Same effects surface.** Each remote effect is registered through the existing plug-in API 2
  (`EffectPlugin`), so the UI, the property tree, keyframes, expressions, scripting, the CLI, the
  control channel and MCP need nothing new.
- **Protocol** (`docs/external-helpers.md`): JSON lines over the helper's stdin / stdout
  (`hello` → `describe` → `render` with callbacks → `done`, plus `padding`, `load`, `destroy`,
  `ping`, `shutdown`). Pixels go through a **frame exchange file** (in `/dev/shm` on Linux, the
  temp folder elsewhere) addressed by offsets, never through the pipe. Callbacks (`input_at`,
  `layer_input`, `params_at`) let temporal effects (motion blur, retiming) read other frames
  while rendering; nested renders through the same helper are supported.
- **Discovery.** EffectCraft looks in `helpers/` next to the executable (and
  `Contents/Helpers` in a macOS bundle), `helpers/` in the settings folder, and
  `EFFECTCRAFT_HELPER_PATH`. Helper programs are named `effectcraft-helper-*`.
- **Security.** Helpers are local programs the user installed, just like the plug-ins they host.
  EffectCraft runs only programs from those locations, passes them nothing beyond the protocol
  (no network, no project files), and runs no helper on the web build.

## Alternatives considered

- **In-process host behind a feature flag** (#508). Rejected: a crash is still a crash, and it
  needs `unsafe` in the workspace.
- **`ipc-channel` for transport.** Its wire format and shared-memory handles are Rust-specific
  (and differ per OS), so a helper written in C++ against the OpenFX reference host couldn't
  speak it. The chosen transport, JSON lines plus a plain file, works from any language without
  a library.
- **Memory-mapped shared memory on the EffectCraft side.** Mapping needs `unsafe` (or a crate
  whose safe API is unsound if the other process truncates the mapping). Positional reads and
  writes on a file in the page cache cost one copy per frame each way and stay safe. A helper is
  free to map the file.
- **WebAssembly-compiled plug-ins.** Not available for closed-source native plug-ins.

## Consequences

- Native effects cost one copy of the frame each way plus process round trips. That's fine for
  CPU plug-ins working on whole frames.
- Renders through one helper are serialized (one process, one conversation at a time). A pool
  of helper processes is a possible later improvement.
- Out of scope for the core: any OpenFX (or other native-format) code. The OpenFX helper is a
  separate program in its own repository (an example: [ec-extension-ofx](https://github.com/glumbis/ec-extension-ofx), unofficial) that reuses the
  existing Rust OpenFX host behind `serve()`. A C++ helper built on the OpenFX HostSupport
  library is an equally valid way to implement the same protocol.
- Later: the ArtCraft launcher can install helpers into `helpers/` the same way it installs
  apps.
