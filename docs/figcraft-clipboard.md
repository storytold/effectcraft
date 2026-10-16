# Paste from FigCraft

In FigCraft, select artwork and use ordinary **right-click → Copy**, **Edit → Copy**, or
**Cmd+C / Ctrl+C**. In EffectCraft, use **Edit → Paste** or **Cmd+V / Ctrl+V**.
The explicit **Copy for EffectCraft** action remains optional. Frames create new compositions; other selections go into
the active composition, or create one when none is open. Each paste is an independent copy and
one undo step. Later edits in either application do not change the other copy.

Shapes, paths, fills, strokes, text, groups and nested frames become native editable layers.
PNG/JPEG image fills retain their original bitmap bytes as editable footage, with native geometry
mattes for cropping and the surrounding text/vector children preserved. This does not flatten
artwork into screenshots. Assets are copied into persistent project/import storage, so a later
clipboard change does not break the pasted project.

FigCraft snapshots preserve root rendered bounds and clipping, multiple shadows, native linear,
radial, angular and diamond gradients (including the radial width handle), and exported text
baseline/line-height metrics. Unsupported features still produce warnings: pattern/tiled fills,
image colour filters, image strokes and some advanced text/effects are not transferred. Font
availability and the apps' rasterizers can still affect appearance. Pasting while editing text
keeps the ordinary text-paste behavior.

## Agent command

`edit.pasteFigCraft {clipboard, target?: auto|newComp|activeComp, comp?}` takes the system
clipboard text: the SVG from ordinary Copy, or the explicit action's JSON. `auto` follows the behavior above. New compositions use 30 fps and a
10-second duration; pasting into an existing composition uses its frame rate and duration, including
nested compositions. The result includes `comp`, `layers`, `native`, `warnings` and `notes`.

The current envelope is `{format: "figcraft.clipboard", version: 2, nodes: [...],
warnings: [...], assets: {id: {mime, data, width, height}}}`. Assets contain canonical base64 of
original PNG/JPEG bytes and are deduplicated by image id. Image paints refer to an asset and carry
a pixel-to-node affine transform. The receiver also accepts legacy version 1 snapshots without
assets. Both apps must be updated for version 2; old receivers reject it rather than silently
pasting a flattened fallback.
Nodes use the public Figma-shaped geometry fields understood by the existing mapper; this is a
FigCraft transfer format, not Figma's clipboard. Ordinary Copy embeds its standard padded-base64
encoding in `<metadata id="figcraft-clipboard-v1">…</metadata>` inside SVG; other applications
can still use the SVG. EffectCraft reads only its exact metadata marker and leaves ordinary SVG
paste unchanged. Duplicate, malformed or oversized marked payloads return an error without
falling back to SVG or anything previously copied inside EffectCraft.

The SVG carrier is capped at 128 MiB and decoded JSON at 64 MiB, with 20,000 source nodes and
64 node nesting levels. JSON nesting is independently bounded before parsing. Images are checked
before project mutation: canonical encoding, format signature, declared dimensions, complete
decode, 32 MiB encoded-file bytes per image, 16,384 pixels per side, 64M pixels per image and
512M pixels across assets. Each decoder has a 256 MiB allocation limit. These pixel budgets are
validation limits, not a promise about the application's total memory use. Caller-supplied file
paths, fragment references and URL fetch inputs remain forbidden.

Automation sessions with file-access confinement reject `edit.pasteFigCraft`: checking only the
project path cannot safely confine bitmap writes through a sibling asset-directory symlink.
Ordinary desktop clipboard paste is unaffected.


## Verification (14 October 2026)

Actual FigCraft right-click Copy and Cmd+C through the system clipboard, followed by EffectCraft
menu Paste and Cmd+V, produced ten native layers (three text, seven shapes, including an editable
vector) without images. Cut/Undo, plain-text copying and the optional explicit action also passed.
Native checks covered text editing, one-step paste undo/redo and viewer restoration, independent
copies, malformed-carrier rejection without mutation, identical pixels after save/reopen, and
standalone line/shadow bounds (220 × 120 in both applications). Earlier explicit-transfer checks
also covered nested editable text and existing-comp timing (24 fps / 20 seconds).

A synthetic linear-gradient region differs by at most 1/255 per channel between the apps;
text metrics still differ and are warned. These checks do not establish general pixel parity.
Scoped engine/UI compilation and clippy with all targets and warnings denied passed, as did the
native app build. Regression tests were compiled but not executed under the fork's daytime
rule; full CI was not run.

## Fidelity follow-up (15 October 2026)

Source and receiver synthetic regressions cover bounded embedded images, crop placement,
clipped image frames with editable children, original JPEG preservation, repeated asset references,
malformed inputs, larger selections, frame overflow, multiple shadows, gradient transforms and
text metrics. The same private 529-frame snapshot was retested through native ordinary Copy/Paste: all 529
pasted and passed Undo, with zero blank targets (previously 51) and zero size-limit failures
(previously nine). All 953 stored bitmap files matched original encoded source-image bytes.
Four image/mixed-content/overflow/large-board projects rendered identically after save/reopen.
Malformed embedded-image data was rejected without changing the current project; a legacy v1
snapshot still pasted nine native layers.

The inspected overflow examples now retain matching source/target dimensions (404×370,
189×75 and 244×244). A synthetic comparison covering all four gradient kinds differed by at
most 1/255 per channel in the sampled interiors, and both text lines occupied the same vertical
ink bands. Small rasterization differences remain; these checks do not establish universal
pixel equality. The large roughly 2,000-layer board completed a 640-pixel-side preview render
in about 1.1 seconds in this run; that is not a controlled speedup claim or a full-resolution
benchmark. Larger renders are awaited asynchronously rather than judged by a control timeout.

Scoped all-target compilation, strict Clippy, WASM checks and native builds passed. New Rust
regressions were compiled but not executed under the fork's daytime rule; full CI was not run.
A final optional legacy Undo/Redo/keyboard smoke was interrupted by an offscreen QA window
before its next request ran; the completed native 529-frame pass is unaffected.
