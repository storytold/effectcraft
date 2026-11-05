# effectcraft-plugins

The EffectCraft side of the plug-in host (craft-abi 1, L5). Native only: the lifecycle of
`craft-plugin-host` is native only, so no web crate may depend on this one. `cargo xtask wasm`
proves that.

| Module | What it holds |
|---|---|
| `bridge` | `Bridge`: how a host call of a guest reaches the `Session` (the session is lent to the command body through `Session::dyn_scope`, other threads queue jobs for `pump`), and the checkpoint that a faulted call restores. |
| `services` | `AppServices`: `craft_plugin_host::services::Services` for EffectCraft. |
| `doc` | `doc.query` answers (`what`: `project`, `comp`, `selection`). |
| `host` | `PluginHost::start`: manager, hook layer, surfaces, bridge, host commands. The install flow (`install_bytes`, `install_path`, `answer`). |
| `commands` | The host's own commands `plugin.app.effectcraft.*` (manage, list, install, grant, enable, disable, remove, revoke). A plug-in can neither list nor run them. |
| `formats` | `format` contributions: `attach` registers a `foreign` still reader over the plug-in `FormatBook`. |
| `surfaces` | `Surfaces`: the `SurfaceSink` that keeps live panels and effects. |
| `effect` | `effect` contributions as entries of the effect registry (`effectcraft_effects::plugin`). One copy in and one copy out per frame at most: the registry's premultiplied `f32` buffer goes to the host as is, and only a contribution that declares `u8` or straight alpha gets a converted copy. A v1 `ec_` module takes the same path through its shim. Disable and remove unload the effect; a reload reuses its slot and spec. Parameters are in id order, because a contribution's `params` is a map. |
| `ui` | `frame`: serves queued host calls, then draws the permission prompt, the manager view and panel windows. |

## Why the session is lent

`Services` methods take `&self`, and a command body gets `&mut Session` only for its own call.
The guest runs inside that call. `Session::dyn_scope` (one seam in `effectcraft-engine`) swaps the
real session into a box on a stack for the length of the body; a host call such as `cmd.execute`
borrows it from the stack. Callers on other threads (`after` and `change` hooks) put a job in a
mailbox that the app serves once per frame (`ui::frame`).

## Native commands

`Session::native_dynamic_owners` lists owners of dynamic commands that are native code, not guests
(`craft.link`, set by `effectcraft_link::install_back_command`). `dyn_scope` skips them, because
their body reads the session it is given. `tests/lifecycle.rs` fails if `link.back` loses that.

## Known gaps

- A command that a guest runs through `cmd.execute` is reported to hooks as `Origin::Plugin`
  (`craft_command::Origin::Plugin`, used in `src/bridge.rs`).
- The checkpoint covers the project, the editor state, the undo and redo stacks and the command journal.
  It cannot un-write a file or stop a job that a command started.
- A `before` gate runs while the session is borrowed above it, so `cmd.execute` and `doc.query`
  from a gate fail (`E_UNSUPPORTED` / a failure string). Gates are pure decisions.
- `doc.query` serves `metadata` only. `pixels` answers `E_UNSUPPORTED`.
- The plug-in windows are not in the automation registry (`ui.elements`); the host commands are the
  agent-drivable surface.
- `format` contributions open stills (Import of an unknown extension, through `effectcraft_media::foreign`). Save is not wired: the Render Queue writes through `effectcraft-export` output formats, which have no plug-in hook.
- Effects only cover `number`, `int`, `angle`, `bool`, `choice`, `point2` and `color` parameters, with `f32` or `u8` samples.
- The host keeps its plug-ins in `<config>/Plug-ins-host`. `<config>/Plug-ins` still loads through `effect.plugins.load`.
- Plug-in panels open in floating windows. They are not docked.
- The macOS native menu bar does not show the Plugins menu (it is built from the fixed menu list).
