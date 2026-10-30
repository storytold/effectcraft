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
Queue are checked against the files they write (the open project's, the queued outputs);
`renderQueue.add`, `render.preRender` and `file.createProxy` without an output file need a saved
project, since an unsaved one's default output would go to the home folder (#2479). Scripts
(`run_script`, `file.runScript`…), Execute File, Reveal in Finder, `$N` references to file paths
in a batch, and the bridge's UI-driving tools (`ui_click`, `ui_key`, `ui_type`, `ui_drag`,
`ui_set`, `script_ui`, `control`) are refused while confined. Without the flags nothing changes.

The server provides the same core conventions as FilmCraft #28:

| Tool | Arguments | Result |
| --- | --- | --- |
| `command_list` | `filter?`, `enabled_only?`, `schemas?`, `compact?` | Command catalog |
| `command_run` | `id`, `params?` | Engine command result |
| `command_batch` | `steps: [{id, params?}]`, `stop_on_error?` | `completed`, `failed`, per-step `results` |
| `doc_inspect` | none | Project overview and active composition |
| `render_preview` | `comp?`, `time?`, `max_side?`, `transparent?` | PNG and frame information |

`command_batch` stops at the first error by default; false continues. Each edit has its
own undo step. The existing `batch` tool retains its atomic undo grouping and result references.
Existing tools, including `list_commands`, `execute_command`, and `batch`, remain listed
because existing workflows use them; there are no hidden aliases. See [agents.md](agents.md)
for the full catalog. Each tool has a title and read-only, destructive, idempotent and
open-world hints. Tools with an optional output path are conservatively annotated as file writers.

Unknown top-level tool argument keys return JSON-RPC `-32602` naming the key and accepted
arguments. EffectCraft already rejects unknown engine command parameters; that behaviour is
preserved. Tool failures (including escaped panics) return `isError: true`, and so does any JSON
result with a `failed` count above zero (a `command_batch` / `batch` with a failed step, a
render whose queue items did not all finish), even though the other steps or items succeeded:
read `results` / `items` for the details. Malformed JSON
returns `-32700` with a null id and the session keeps serving. The MCP backend owns its session
without a mutex; the engine's render-job mutexes already recover poisoned locks.

Resources `effectcraft://document` and `effectcraft://commands` return JSON matching
`doc_inspect` and `command_list`. Clients declaring MCP 2026-07-28 in per-request `_meta`
receive `resultType: complete` and list/read cache hints; document reads are never cached.

```json
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"command_run","arguments":{"id":"comp.new","params":{"width":640,"height":360}}}}
{"jsonrpc":"2.0","id":2,"method":"resources/read","params":{"uri":"effectcraft://document"}}
```

## Progress and cancellation

Headless `command_run` / `execute_command` calls to `renderQueue.render` (unless `wait: false`)
run the existing background job while the MCP server keeps reading stdin. A
`params._meta.progressToken` string or number opts into `notifications/progress`, at most ten
per second, strictly increasing with a queue-item total. No token means no notifications.
`ping`, inspection and other requests are served while rendering. Closing stdin lets the
pending render finish and reply.

### Requests during a render

A render job owns a snapshot of the project taken when it started. While it runs:

- Every way of starting a second render is refused with `isError: true`: a blocking
  `renderQueue.render`, one with `wait: false`, or one inside a batch.
- Other commands, including edits, run normally and change the project. They do not affect the
  running job, which keeps rendering its snapshot; to render an edit, render again after the
  job ends. Queue items are matched by id when the job reports their status, so an item removed
  mid-render is left out of the reply and counted in `failed`.

Send `notifications/cancelled` with `params.requestId` to stop the matching request at the next
frame batch. No response is sent for that cancelled request. Unknown and completed ids are
ignored. The exporter records the exact files it opens, including storage overflow paths, and
removes those files on cancellation; it never scans for similarly named files. Finished earlier
queue items and skipped existing frames remain. Custom exporters implement the same cleanup
contract. A failed queue item gives `isError: true`.

Bridge calls, `wait: false`, batch tools and single-frame tools retain synchronous/polling
behaviour and do not report MCP progress or cancellation. `renderQueue.render {"wait": false}`
returns the ids being rendered at once; poll `renderQueue.list` for status and stop it with
`renderQueue.stop`. Use a direct blocking command call
for an MCP-cancellable render.
