# Proposal: local HTML panels with a constrained script bridge

Status: design review only. There is no panel launcher, new script command or bridge
implementation in this change. This proposal follows the
[review of #330](https://github.com/storytold/effectcraft/pull/330#issuecomment-6082006351).
It is independent of the effect-input fixes and the proposed plug-in context API.

## Goal and compatibility boundary

Let an explicitly opened local HTML panel call EffectCraft's existing scripting object
model through a small asynchronous interface. An optional adapter could support the
`CSInterface.evalScript` calling convention used by some extensions. This is not an Adobe
CEP runtime or a promise that arbitrary AE extensions run unchanged.

Propose a browser-hosted first version. Docking, Node.js, native modules, JSXBIN, native CEP
services, remote web panels and automatic extension discovery are outside this version.
Unsupported capabilities must be reported explicitly. A panel that depends on those
capabilities needs conversion or cannot run.

## Addressing the review

| Concern | Required behavior before an implementation can land |
| --- | --- |
| Caller-controlled `readRoot` bypasses Allow Scripts | Remove `readRoot` from generic `script.run`. Control-channel and MCP callers cannot add read roots. |
| Remote code inherits the bridge's authority | Strict CSP on every HTML response, local scripts only, no remote scripts or `eval`. |
| Script execution blocks the UI | Use a cancellable script-host worker and a nonblocking UI hand-off; never evaluate panel code inline on the UI thread. |
| Adobe app ID | Report `appId: "EffectCraft"`, EffectCraft's own version and explicit capability flags; never `AEFT`. |

### Filesystem authority

Preserve the normal script permission policy and Allow Scripts gate. Serving an HTML
file from a selected directory does not grant its host script access to that directory,
the filesystem root, or arbitrary `$.evalFile` paths. Neither a context ID nor a field in a
control-channel/MCP request can widen access.

For a declared entry-point host script, the launcher may read the explicitly selected
local bundle file and submit its bounded source text as code. Its canonical path must
remain inside the selected bundle; reject traversal, escaping symlinks, oversized files
and unsupported binary scripts. Any subsequent host-script file reads and writes still
obey the ordinary server-side script permissions. General bundle read grants would need
a separate, server-owned authorization design and are not proposed here.

Reject obsolete `readRoot` arguments with an actionable error; do not silently accept
them. Cover both direct commands and the control/MCP paths with regression tests while
Allow Scripts is off, including a request that supplies `/`.

### Browser and bridge boundary

Bind an ephemeral listener to loopback. Require an unpredictable capability and exact
origin validation for bridge POSTs; validate the Host header as well. Keep the capability
out of query strings, logs, remote requests and referrers. GETs cannot execute scripts.
No permissive CORS. Bound requests, files, concurrent work and the execution queue.

The first version serves local external JavaScript files and injects its own bridge as a
local external script. An example minimum policy is:

```text
default-src 'none';
script-src 'self';
style-src 'self';
img-src 'self' data:;
font-src 'self';
connect-src 'self';
object-src 'none';
frame-src 'none';
worker-src 'none';
base-uri 'none';
form-action 'none';
frame-ancestors 'none'
```

Set CSP as an HTTP response header on all document responses, including error pages,
along with `X-Content-Type-Options: nosniff` and `Referrer-Policy: no-referrer`. Do not
enable inline scripts, event attributes, `unsafe-eval`, remote scripts, frames or workers.
Existing panels that use inline handlers need conversion. Inline-script compatibility
would require a separately reviewed mechanism, not a broad CSP exemption.

Only serve an allowlisted set of static MIME types from the canonical bundle. The bridge
and evaluation endpoint have reserved paths that a bundle cannot replace. An explicit
external-link action may open a separate tab with no opener; it must not navigate the
privileged panel into remote content. Tests must exercise the policy in a real browser,
including blocked remote scripts, inline handlers, dynamic evaluation and embedded frames.

### Threading, session ownership and cancellation

Extend the existing native threaded script host rather than create an inline panel
execution path. A background thread alone is insufficient: waiting on its result with a
blocking receive from the UI thread still freezes the app.

Proposed lifecycle:

1. A panel evaluation is enqueued and returns a job ID immediately. Calls from one context
   execute in order, with a bounded queue. The UI polls completions between frames.
2. JavaScript contexts live only on their owning worker. The live editable session has
   one owner during a script transaction. The UI keeps an immutable display snapshot and
   remains able to repaint, report progress and cancel; conflicting project edits wait.
3. Commands and undo operate on that owned session, using existing engine operations.
   Do not replay mutations after completion and do not replace a concurrently edited
   project with a stale snapshot. ScriptUI modal pauses require the existing explicit
   session hand-off; they cannot introduce a second session owner.
4. The UI/control-side cancellation path is available while a job runs. Cancellation must
   interrupt pure JavaScript loops as well as native host calls, through a checked engine
   interrupt/budget hook. Loop limits alone are not user cancellation. Long native work
   must check cancellation cooperatively.
5. On success, error or cancellation, return the session, close undo groups, release job
   resources and issue one completion. Cancellation keeps completed commands undoable;
   it must not leave an open undo group or a missing live session. A failed context can
   be discarded without losing the project. Never fall back to inline execution if a
   worker cannot start.

An implementation must establish the interrupt mechanism before enabling panel scripts.
If the JavaScript engine cannot interrupt safely, this feature remains disabled until an
interruptible host is available. No hard thread termination or unsafe code is proposed.

Contexts have server-issued IDs, bounded lifetime/count and isolation from the console
and other panels. Close and cancel on explicit shutdown; a lease handles abandoned
launchers/tabs. A lost network reply must never trigger automatic mutation replay. Job
status lets the caller discover completion instead.

## Decisions requested

1. Is browser-hosted local HTML a useful first step, or should a native panel host come first?
2. Should an explicit experimental preference also gate launch, in addition to the CSP and
   user-selected local bundle? CSP remains mandatory either way.
3. Is the proposed exclusive session hand-off compatible with the current UI and script
   host, or should commands instead be marshalled to the UI under a transaction lease?
4. Which cancellable JavaScript execution hook and native-call cancellation contract should
   the shared script host expose?
5. Should the first interface be EffectCraft-only, with the `evalScript` adapter added later?

## Acceptance and rollout

First agree on the trust model and session/cancellation design. Then implement the shared
host infrastructure with tests, followed by the local static server/CSP, then the adapter.
Keep each independently reviewable. Plugin API 2 is not a dependency.

Required acceptance coverage: unauthorized filesystem reads with Allow Scripts off;
path/symlink escapes; Host/origin/token failures; CSP enforcement in a browser; a long loop
that leaves the UI responsive and cancels promptly; cancellation inside a native call;
context isolation/cleanup; ordered callbacks; exactly-once completion after a lost reply;
undo after success, script errors and cancellation; worker startup/failure paths; and
honest host metadata. Run `cargo xtask ci` and inspect the user-visible loading, error,
progress and cancellation states before enabling the feature.
