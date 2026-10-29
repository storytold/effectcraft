# Contributing to EffectCraft

Thanks for helping. Please read [AGENTS.md](AGENTS.md) first: its rules on assets and clean-room
work apply to everyone, people and AI agents alike, and they are not negotiable.

## Setup

You need [Rust](https://rustup.rs/) 1.95 or newer.

On Linux the desktop app plays preview sound through `cpal`, so the build needs a usable ALSA
pkg-config package: an `alsa.pc` that pkg-config can find, naming the linkable ALSA runtime
library (usually `libasound.so.2`). `alsa-sys` runs pkg-config at build time; it does not compile
ALSA's headers. Without a usable package the build fails at link time with an unrelated-looking
error; `cargo xtask ci` probes pkg-config up front and names the package:

```sh
sudo dnf install alsa-lib-devel        # Fedora
sudo apt install libasound2-dev        # Debian / Ubuntu
sudo pacman -S alsa-lib                # Arch
```

If you cannot install the package, point `PKG_CONFIG_PATH` at a directory holding an `alsa.pc`
whose `Libs` line names your system's `libasound` (usually `libasound.so.2`); every CI run needs
that variable set.

```sh
cargo run -p effectcraft -- --demo       # the desktop app, with the demo project
cargo test --workspace
cargo xtask ci                           # what every commit must pass
```

The default `dev` profile keeps debug info to line tables: builds are fast and panics/backtraces
still name the file and line, but a debugger cannot show local variables. For an interactive
debugger session use the `dev-full` profile, which is the same build with full debug info:

```sh
cargo run --profile dev-full -p effectcraft -- --demo
```

## Ground rules

- **Clean room.** Work from After Effects' public documentation and its observable behaviour only.
  Never use Adobe artwork, presets or code, and never copy GPL/LGPL/AGPL code.
- **Every asset is attributed.** Add a `.attribution` sidecar and an [ATTRIBUTION.md](ATTRIBUTION.md)
  row; `cargo xtask assets` checks it.
- **Layering.** Nothing below the interface layer depends on UI or OS crates; see
  [docs/architecture.md](docs/architecture.md).
- **Everything is a command.** New features are engine commands with an id, label, menu path,
  shortcut and parameters, so the UI, the CLI and the MCP server get them at once. Interactive
  widgets register an automation id.
- **Never crash.** Non-test code returns errors instead of panicking: no `unwrap()`, `expect()`,
  `panic!`, `unreachable!`, `todo!`, `unimplemented!` or `unsafe`, checked indexing and arithmetic on
  input-derived values, and a regression test with every crash fix. See the "Never crash" section of
  [AGENTS.md](AGENTS.md).
- **Tests with every change**, and green `cargo xtask ci` before every commit. One task per commit.

## How to add…

- **An effect:** add an `EffectSpec` in `crates/effects` (AE display name, category, parameters
  with AE's names, defaults and ranges) and its render function, with a determinism test and a
  behaviour test. If it reads the clock directly, add it to `TIME_DEPENDENT`.
- **A command:** register it with `cmd!` in `crates/engine/src/commands/`, place it in the menu
  tree in `crates/engine/src/menus.rs`, and test it including undo.
- **A panel or dialog:** `crates/ui-egui/src/panels/`; look at the result with the headless
  snapshot tool described in [docs/testing.md](docs/testing.md).

## Talk to us

Questions, ideas and bug reports are welcome on [Discord](https://discord.gg/artcraft) and in
GitHub issues.
