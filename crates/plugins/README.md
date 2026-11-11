# effectcraft-plugins

The EffectCraft side of the plug-in host (craft-abi 1, L5). Native only: the lifecycle of
`craft-plugin-host` is native only, so no web crate may depend on this one. `cargo xtask wasm`
proves that.

| Module | What it holds |
|---|---|
| `bridge` | `Bridge`: how a host call of a guest reaches the `Session` (`Services::lend_session` lends the session for the length of a guest call, other threads queue jobs for `pump`), and the checkpoint that a faulted call restores. |
| `services` | `AppServices`: `craft_plugin_host::services::Services` for EffectCraft. |
| `doc` | `doc.query` answers (`what`: `project`, `comp`, `selection`). |
| `host` | `PluginHost::start`: manager, hook layer, surfaces, bridge, host commands. The install flow (`install_bytes`, `install_path`, `answer`). |
| `commands` | The host's own commands `plugin.app.effectcraft.*` (manage, list, install, grant, enable, disable, remove, revoke). A plug-in can neither list nor run them. |
| `formats` | `format` contributions: `attach` registers a `foreign` still reader over the plug-in `FormatBook`. |
| `surfaces` | `Surfaces`: the `SurfaceSink` that keeps live panels and effects. |
| `effect` | `effect` contributions as entries of the effect registry (`effectcraft_effects::plugin`). One copy in and one copy out per frame at most: the registry's premultiplied `f32` buffer goes to the host as is, and only a contribution that declares `u8` or straight alpha gets a converted copy. A v1 `ec_` module takes the same path through its shim. Disable and remove unload the effect; a reload reuses its slot and spec. Parameters are in id order, because a contribution's `params` is a map. |
| `ui` | `serve`: serves queued host calls on every logic tick, also while the window is hidden. `frame`: draws the permission prompt, the manager view and panel windows. |

## Why the session is lent

`Services` methods take `&self`, and a command body gets `&mut Session` only for its own call.
The guest runs inside that call. The plug-in host calls `Services::lend_session` around the guest
call, which reaches `Bridge::lend_any`. It swaps the real session into a box on a stack (the live
stack) and leaves an idle spare in its place. A host call such as `cmd.execute` or `doc.query`
borrows the box from the top of the stack while the guest runs. Unwinding puts the session back.
A `before` gate lends the same way, so its host calls reach the session too
(`tests/session_access.rs::a_before_gate_reaches_the_session_for_its_host_calls`).

Callers on other threads (`after` and `change` hooks) put a job in the mailbox. The app serves the
mailbox on every logic tick (`ui::serve`, the `per_tick` hook), so a hidden window serves it too.

A native dynamic command, such as `link.back` from `craft.link`, never runs a guest. Its body
reads the session it is given, and the lend does not touch it
(`tests/session_access.rs::a_native_dynamic_command_gets_the_real_session_with_the_plugin_host_running`).

## Origin and host commands

A command that a guest runs through `cmd.execute` reaches the gates as `Origin::Plugin`
(`a_nested_cmd_execute_reaches_the_gates_as_origin_plugin`). The commands that command runs in turn
(the steps of `engine.batch` or `file.runScript`) get `Origin::Execute`. So the host commands
`plugin.app.effectcraft.*` do not look at the origin. They refuse while `Bridge::in_plugin_command`
is true, which holds for the whole stack of a command that a plug-in asked for.

## Known gaps

- The checkpoint covers the project, the editor state, the undo and redo stacks and the command journal.
  It cannot un-write a file or stop a job that a command started.
- `doc.query` serves `metadata` only. `pixels` answers `E_UNSUPPORTED`.
- The plug-in windows are not in the automation registry (`ui.elements`); the host commands are the
  agent-drivable surface.
- `format` contributions open stills (Import of an unknown extension, through `effectcraft_media::foreign`). Save is not wired: the Render Queue writes through `effectcraft-export` output formats, which have no plug-in hook.
- Effects only cover `number`, `int`, `angle`, `bool`, `choice`, `point2` and `color` parameters, with `f32` or `u8` samples.
- The host keeps its plug-ins in `<config>/Plug-ins-host`. `<config>/Plug-ins` still loads through `effect.plugins.load`.
- Plug-in panels open in floating windows. They are not docked.
- The macOS native menu bar does not show the Plugins menu (it is built from the fixed menu list).
