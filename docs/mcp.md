# MCP conventions

Run `effectcraft-cli mcp` for a headless session, or `effectcraft-cli mcp --bridge 9877`
to use the desktop app's control channel. Existing ports are unchanged.

## Confining file access

`--automation-read-root DIR` and `--automation-write-root DIR` (each repeatable, as in
PhotoCraft) confine what the agent can reach: files are read only under a read or write root and
written only under a write root; with no write root nothing is written. Every engine command
(`execute_command`, `command_run`, each `batch` step) is checked before it runs: its file
parameters must be absolute paths inside the roots (symbolic links resolved, no `..`), and so must
any other absolute path in its parameters. Save, Increment and Save and rendering the Render
Queue are checked against the files they write (the open project's, the queued outputs). Scripts
(`run_script`, `file.runScript`…), Execute File, Reveal in Finder, `$N` references to file paths
in a batch, and the bridge's UI-driving tools (`ui_click`, `ui_key`, `ui_type`, `ui_drag`,
`ui_set`, `script_ui`, `control`) are refused while confined. Without the flags nothing changes.

The server provides the same core conventions as FilmCraft #28:

| Tool | Arguments | Result |
| --- | --- | --- |
| `command_list` | `filter?`, `enabled_only?`, `schemas?` | Command catalog |
| `command_run` | `id`, `params?` | Engine command result |
| `command_batch` | `steps: [{id, params?}]`, `stop_on_error?` | `completed`, `failed`, per-step `results` |
| `doc_inspect` | none | Project overview and active composition |
| `render_preview` | `comp?`, `time?`, `max_side?`, `path?`, `inline?`, `transparent?` | PNG and frame information |

`command_batch` stops at the first error by default; false continues. Each edit has its
own undo step. The existing `batch` tool retains its atomic undo grouping and result references.
See [agents.md](agents.md) for the full catalog. Each tool has a title and read-only, destructive,
idempotent and open-world hints. Tools with an optional output path are conservatively annotated
as file writers.

Four older names duplicate a core tool. They are left out of `tools/list` but stay callable
with their own arguments, so existing workflows keep working; `effectcraft-cli mcp --all-tools`
lists them too (described as aliases):

| Hidden alias | Canonical tool |
| --- | --- |
| `list_commands` | `command_list` (same arguments) |
| `execute_command {command \| id, params?}` | `command_run {id, params?}` |
| `get_project` | `doc_inspect` (its `project` is the same summary) |
| `render_frame` | `render_preview` (same arguments, including `path` and `inline`) |

Unknown top-level tool argument keys return JSON-RPC `-32602` naming the key and accepted
arguments. EffectCraft already rejects unknown engine command parameters; that behaviour is
preserved. Tool failures (including escaped panics) return `isError: true`, and so does any JSON
result with a `failed` count above zero (a `command_batch` / `batch` with a failed step, a
render whose queue items did not all finish), even though the other steps or items succeeded:
read `results` / `items` for the details. Malformed JSON
returns `-32700` with a null id and the session keeps serving. The MCP backend owns its session
without a mutex; the engine's render-job mutexes already recover poisoned locks.

## Structured results

Sessions that negotiated MCP 2025-06-18 (and modern per-request clients) get each JSON tool
result as `structuredContent` as well as the JSON text block older clients read. It is always an
object: an array result is `{"items": [...]}`, a scalar `{"value": v}`, and an image reply carries
its metadata (`comp`, `time`, `width`, `height`, `path?`). Tools with a stable shape declare it as
`outputSchema` in `tools/list`: `get_state`, `doc_inspect`, `render_preview`, `undo`, `redo`,
`history`, `command_batch`, `batch` and `save_project`. Earlier revisions see neither field.
Long renders (below) reply with text only.

## Resources

Resources `effectcraft://document` and `effectcraft://commands` return JSON matching
`doc_inspect` and `command_list`. Clients declaring MCP 2026-07-28 in per-request `_meta`
receive `resultType: complete` and list/read cache hints; document reads are never cached.

`resources/templates/list` offers four templates, read with `resources/read`:

| Template | Content |
| --- | --- |
| `effectcraft://comp/{comp}` | `get_comp` JSON |
| `effectcraft://layer/{comp}/{layer}` | `get_layer` tree JSON; `?flat=true` for the flat list |
| `effectcraft://frame/{comp}/{time}` | PNG (`blob`, `image/png`) at comp time `time` seconds, longest side 960; `?max_side=N` (0 = full size, at most 8192) |
| `effectcraft://command/{id}` | `describe_command` JSON |

`{comp}` and `{layer}` take an id (a number), a name, or `#n` for a layer. Each segment is
percent-encoded (`Main%20%2F%20Intro`, `%231`). An unknown scheme, kind, comp, layer or command
returns `-32002`; malformed URIs (wrong segment count, bad escape or time, unknown query key)
return `-32602`.

```json
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"command_run","arguments":{"id":"comp.new","params":{"width":640,"height":360}}}}
{"jsonrpc":"2.0","id":2,"method":"resources/read","params":{"uri":"effectcraft://document"}}
{"jsonrpc":"2.0","id":3,"method":"resources/read","params":{"uri":"effectcraft://frame/1/0.5?max_side=320"}}
```

## Analysis tools

Headless agents can't look at a render or hear the mix, so read-only tools return numbers.
Each wraps an engine command, so the CLI, scripts (`app.run`) and the control channel get the same.

| Tool | Command | Result |
| --- | --- | --- |
| `get_layout` | `layer.bounds` | Per layer: `visible` + `why`, comp-space `bounds`, `corners`, `center`, `anchorInComp`, `onScreen`, `source`; optional `pixelBounds` and `overlaps` |
| `analyze_audio` | `audio.analyze` | Peak, true peak, RMS, integrated LUFS, clipped samples, silences, channels; optional waveform |

- **Geometry** is `sourceRectAtTime` through the transform, parents and, for 3D layers, the
  active camera. A 3D layer partly behind the camera reports `projected: false` and no bounds.
  The engine function is `effectcraft_engine::layout::layer_geometry`.
- **Audio** is the 48 kHz stereo mix preview and export play. Integrated loudness follows ITU-R
  BS.1770-4 (K-weighting, 400 ms blocks, absolute and relative gates). Levels are null for
  silence, and the true peak is a 4× oversampled estimate. A comp without audio returns
  `hasAudio: false`. One call covers at most an hour.

`analyze_audio {waveform: true}` returns the waveform as PNG content next to the JSON; the server
draws it from the command's min/max columns, so it works the same headless and bridged.

## Notifications

The server writes notifications on stdout after the reply to the request that caused them.

**Resource subscriptions** (`resources.subscribe: true`). `resources/subscribe {uri}` accepts any
readable URI, including template matches (an unknown or malformed one is an error, as for
`resources/read`); `resources/unsubscribe` removes it. When the project's revision changes
(`project.summary.revision`; every edit, undo and redo, never a query or a value set to what it
already was), each subscribed `document`, `comp`, `layer` and `frame` URI gets
`notifications/resources/updated {uri}`. This is conservative: every subscribed project URI is
reported, whichever comp changed. Command URIs never are. Headless sessions compare the revision
after every request. In bridge mode, while anything is subscribed, each request polls
`project.summary` once. Edits made in the app are therefore reported with the next request, not
as they happen.

**Logging** (`logging: {}`). `logging/setLevel {level}` takes an RFC 5424 level (`debug` …
`emergency`). Until then, `warning` and above are sent. Entries are `notifications/message`
with `logger: "effectcraft"` and `data: {message, …}`:

| Level | When |
| --- | --- |
| `warning` | An edit leaves new expression errors in the active comp (`data.errors`, from `expr.errors`; each error is reported once) |
| `warning` | `open_project` finds missing footage (headless; `data.missing`) |
| `error` | A `--autosave` checkpoint failed (also appended to the tool reply) |
| `error` / `info` | Session toasts (headless), such as "saved by a newer EffectCraft" |

In bridge mode only the expression check applies; the app shows its own toasts and footage
checks.

## Prompts

`prompts/list` and `prompts/get` serve workflow recipes. Each one is a single user message that spells
out the tool flow (canonical tool names, the layer-time versus comp-time pitfall, rendering to check):

| Prompt | Arguments |
| --- | --- |
| `title_card` | `text`, `style?` |
| `lower_third` | `name`, `title` |
| `port_ae_script` | `script` (After Effects ExtendScript, run through `run_script`) |
| `check_against_reference` | `reference_path`, `comp?`, `time?` (`render_compare` against the reference) |
| `animate_layer` | `layer`, `motion` |

Argument values are strings. A missing required argument, an unknown argument or prompt, or a
non-string value returns `-32602`.

## Progress and cancellation

Long calls don't block the server. While one runs, `ping` is answered at once and other
requests are served (see below); `notifications/cancelled` with `params.requestId` stops it, and
no response is sent for the cancelled request. Unknown and completed ids are ignored; a
cancelled request that is still waiting in line is dropped. A `params._meta.progressToken`
string or number opts into `notifications/progress` (`progress`, `total` when known, `message`),
at most ten per second and strictly increasing. Closing stdin lets a running call finish and
reply.

**Background-job commands.** `command_run` / `execute_command` of these commands, when they
block, start their engine job with `wait: false`, watch it through `jobs.list` (renders:
`renderQueue.list`) and reply what the blocking command returns. The table is `SPECS` in
`crates/automation/src/long_job.rs`.

| Command (blocks) | Progress | On cancel |
| --- | --- | --- |
| `renderQueue.render` (unless `wait: false`) | queue items | stops at the next frame batch; the exporter deletes the files it opened for the stopped item; finished items and skipped frames stay |
| `track.analyze`, `track.mask` (`wait: true`) | frames | stops; the frames analysed so far stay, as the one "Analyze Track" / "Track Mask" undo step (as After Effects' Stop) |
| `warp.analyze`, `track.warpStabilizer`, `camera.analyze`, `track.camera`, `camera.deletePoints` (`wait: true`) | frames (per phase) | nothing is written |
| `roto.propagate`, `roto.freeze` (`wait: true`) | frames | propagation keeps the frames it cached (no project change); Freeze writes nothing |
| `contentFill.generate`, `layer.sceneEditDetection`, `footage.check` (`wait: true`) | the task's steps | the task's result is discarded; the project is unchanged |
| `jobs.wait` | finished tasks | only the waiting stops; the tasks keep running |

The job works on a snapshot of the project (or writes its results when it ends, as in the app),
so other requests are served while it runs, including edits. Another long call, a script, and
everything sent after them wait in line until the job ends, then run in order; a second render
is refused at once instead (see "Requests during a render"). This works
headless and bridged (`--bridge`: the app runs the job; the server polls it).

**Scripts.** Headless `run_script` runs on a worker thread that owns the session meanwhile, so
other requests wait in line (in order) and `ping` is answered. A cancel or the `timeout`
(seconds, default 300, `0` = no limit; also `script.run {timeout}`) ends the script with an error
`try`/`catch` cannot intercept: checked every ~100k bytecode steps and in every object-model call.
An undo group the script left open is rolled back (Edit ▸ Redo restores it); edits outside undo
groups are kept; the output says which. Code inside a built-in's callback (`Array.forEach`, a
sort comparator) that never calls the object model is only stopped by the loop-iteration limit.
Bridged scripts honour `timeout` (in the app) but can't be cancelled.

Batch tools (`batch`, `command_batch`), single-frame tools, other synchronous commands
(`render.saveCurrentPreview`, `file.importAep`, `layer.autoTrace`, imports without `background`)
and every call with `wait: false` stay synchronous: no progress, no cancellation.

### Requests during a render

A render job owns a snapshot of the project taken when it started. While it runs:

- Every way of starting a second render is refused with `isError: true`: a blocking
  `renderQueue.render`, one with `wait: false`, or one inside a batch.
- Other commands, including edits, run normally and change the project. They do not affect the
  running job, which keeps rendering its snapshot; to render an edit, render again after the
  job ends. Queue items are matched by id when the job reports their status, so an item removed
  mid-render is left out of the reply and counted in `failed`.

`renderQueue.render {"wait": false}` returns the ids being rendered at once; poll
`renderQueue.list` for status and stop it with `renderQueue.stop`.
