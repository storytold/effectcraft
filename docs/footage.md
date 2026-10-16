# Footage: importing files and image sequences

How to bring footage into a project and tell EffectCraft how to read it. Everything here works
the way it does in After Effects, and every step is a command an agent can run too (shown in
`code`).

## Importing files

- **File ▸ Import ▸ File…** (Ctrl+I / Cmd+I) picks one or more files. Photoshop and PDF /
  Illustrator files first ask whether to import them as footage or as a composition.
- **Drop files or folders** on the window. Dropped on the Composition viewer,
  they also become layers there.
- **The Media Browser** (Window ▸ Media Browser) browses folders and imports what you pick.

Agents: `file.import {"paths": ["/shots/plate.mov", "/shots/logo.psd"]}`.

## Image sequences

VFX plates, 3D renders and hand-drawn animation usually come as numbered stills:
`shot_0001.png`, `shot_0002.png`, … EffectCraft imports such a run as **one footage item** that
plays like a movie. Its name shows the range, for example `shot_[0001-0250].png`, and its type in
the Project panel is *Image Sequence*.

**File ▸ Import ▸ File…:** pick any one numbered file of the run. The *Import Image Sequence*
dialog shows the run it found (and any missing frames) with these options:

| Option | What it does |
|---|---|
| **PNG Sequence** (EXR, TIFF, JPG… after the file type) | On: the whole run imports as one sequence. Off (the default, as in After Effects): the picked files import as stills. |
| **Force alphabetical order** | Imports every image of that type in the folder in name order, whatever the numbering, and plays the files one after another (gaps in the numbering don't count). Use it for files that aren't numbered consistently. |
| **Frame rate (fps)** | The sequence's frame rate. The default comes from Settings ▸ Import ▸ Sequence Footage (30 fps). |

Other ways in:

- **Pick part of a run** (Shift-click the first and the last frame in the file dialog) to import
  just that range as one sequence.
- **Pick every frame** of the run with the Sequence option on: it arrives as one sequence, not
  one item per frame.
- **Drop a folder**: its media files import, and each numbered run in it becomes a sequence.
  Alt-drag the folder to import its files one by one.
- **Dropped files** import as stills, one item each, as in After Effects; drop the folder (or use
  File ▸ Import ▸ File… with the Sequence option) for a sequence.
- **In the browser** (the web app) there are no folders to look in, so pick all the frames of
  the sequence at once with the Sequence option on.

Stills of any format EffectCraft reads can form a sequence: PNG, JPEG, TIFF, OpenEXR, BMP and
WebP. Layered and vector documents (PSD, SVG, PDF) import one by one.

**Missing frames.** Where the numbering skips a number (`shot_0003.png` is missing between
`0002` and `0004`), the sequence shows a placeholder of colour bars for that frame, as After
Effects does, so every other frame stays at its number. Render or copy the missing files into the
folder and choose File ▸ Reload Footage to fill the gaps.

Agents: `file.import {"paths": ["/shots/shot_0001.png"], "sequence": true, "frameRate": 24}`.
Without `sequence`, a numbered file brings its whole run and a folder's numbered runs import as
sequences, except dropped files (`"drag": true`), which import as stills as in After Effects.
`"sequence": true` is the Sequence option (one file brings its whole run), `"sequence": false`
imports stills and a folder's files one by one, and `"alphabetical": true` is Force Alphabetical
Order.

## Importing another project

File ▸ Import ▸ EffectCraft Project… (or an `.ecproj` / `.ecprojx` file given to File ▸ Import)
merges that project into the open one, inside a folder named after the file, as After Effects
imports an `.aep`. Every item, layer and property id moves past the open project's, so precomps,
parents, track mattes, Layer Control values, Essential Graphics and tracker targets keep pointing
at the right things. It is one undo step. Footage paths relative to the imported project resolve
next to it.

Agents: `file.importProject {"path": "/projects/titles.ecproj"}` returns the comps, the root comps,
`primaryComp` (the only root comp, if there is one) and any missing media.

## Interpret Footage

Select a footage item in the Project panel and choose **File ▸ Interpret Footage ▸ Main…**
(Ctrl+Alt+G / Cmd+Opt+G) to change how it is read. A change applies everywhere the footage is
used, and layers that ran to the end of the footage keep doing so.

| Setting | What it does |
|---|---|
| **Alpha** | *Straight - Unmatted*, *Premultiplied - Matted With Color* (with the matte colour), *Ignore* (opaque), or *Guess* (EffectCraft looks at the first frame). *Invert Alpha* flips it. |
| **Assume this frame rate** | The rate the frames play at. A 250-frame sequence at 25 fps lasts 10 seconds; at 24 fps, 10.4 seconds. |
| **Start Timecode** | *Use Source File Timecode* (0:00:00:00), or *Override Start*: the timecode the footage's first frame shows in the Footage panel. It relabels source time only; the frames stay the same. For a 1001–1250 render at 24 fps, Override Start 0:00:41:17 (frame 1001) makes source time count from the render's own frame numbers. |
| Fields, pixel aspect, loop | As in After Effects. |
| **Assign Profile**, **Interpret As Linear Light** | The colour profile the file's values are in (the working space converts from it), and whether they are linear light. |
| **Preserve RGB** | The file's values enter the composition unconverted: no colour profile, linear light or working-space conversion, and float files (OpenEXR, float TIFF) keep their linear values instead of being sRGB-encoded. Use it to convert chosen layers yourself with an OCIO effect, or to read data passes (depth, mist, normals) as numbers. |

Settings ▸ Import ▸ **Report Missing Frames** (on by default) lists the missing frame numbers
when a sequence imports; Interpret Footage lists them too.

**File ▸ Interpret Footage ▸ Remember Interpretation / Apply Interpretation** copy the alpha,
fields, pixel aspect, loop and colour settings (Preserve RGB too) to other footage.

Agents: `file.interpretFootage {"items": [id], "frameRate": 24, "alpha": "premultiplied",
"startTimecode": 1001, "preserveRgb": true}` (timecode or a frame number; `"overrideStart":
false` goes back to Use Source File Timecode).

## Multi-layer OpenEXR

Float OpenEXR renders hold linear light; EffectCraft sRGB-encodes them as they are read, like
After Effects with Interpret As Linear Light for 32 bpc, unless the item preserves RGB (above).
A multi-layer file shows its main RGBA channels, as in After Effects. Renders that keep every
pass in named layers (Blender's `ViewLayer.Combined`, its compositor's File Output `Image`,
Nuke's `beauty`) show the beauty pass: the layer named Combined, Beauty, Image, RGBA, RGB or
Color, else the first colour layer that isn't a data pass. Cryptomatte, depth, mist, normals,
vectors, positions and IDs are never the picture. EXtractoR reads every other layer and channel.

## Replacing and reloading

- **File ▸ Replace Footage ▸ File…** (Ctrl+H) points the item at another file. Pick a numbered
  file to replace it with that file's whole sequence (`file.replaceFootage {"path": …,
  "sequence": false}` for one frame).
- **Replace with Precomposition (used N times)**, right-clicking a footage item or solid in the
  Project panel, puts the item in a new composition of its size,
  pixel aspect and frame rate, in the same folder, and switches every layer in every composition
  that used the item to that composition. Layers keep their timing, transforms, effects and
  masks, and the item stays in the project. The composition lasts as long as the footage, or as
  the longest layer that showed a still or solid. One undo step (`file.replaceWithPrecomp
  {"item"?}`).
- **Relinking moved files.** `footage.relink {"item": id, "path": …}` points a missing item at
  its file's new location, keeping its interpretation, and relinks every other missing item that
  was in the same place relative to it (the whole folder moved, a drive was renamed, a project
  made on Windows opened on macOS); File ▸ Replace Footage on a missing item does the same. One
  undo step; `footage.missing` lists what is still missing.
- **A project moved with its footage** finds it again on open: the footage check that runs after
  File ▸ Open tries each missing file at the same place under the project's new folder, without
  marking the project modified.
- **File ▸ Reload Footage** (Ctrl+Alt+L) reads the files again. A sequence picks up frames
  added to its folder (a render still in progress) and keeps its interpretation.

**File ▸ Dependencies ▸ Relink Missing Footage…** searches a chosen folder and its subfolders
for missing source files on the desktop. It relinks unique exact filenames whose decoded type,
dimensions, codec and audio/video flags agree with the saved footage (movies and audio also
require matching native frame rate and frame count). The Progress panel reports completion;
the result includes each candidate's high or low confidence and reason. Confidence is a metadata
heuristic, not proof of identical contents. Duplicate filenames remain unresolved even if only
one decodes successfully; use Replace Footage to choose them manually.

Relinking preserves item names, interpretation, layer timing and the saved sequence range, and
the entire operation is one undo step. Sequences require every saved frame in the new folder;
extra frames do not extend the range. Existing files are untouched. Layer-specific/page-specific
footage, models and embedded data are left unresolved. Symlinks are skipped, unreadable folders
fail the search without edits, and searches are limited to 100,000 entries. Browser folder
relinking is unavailable.

Agents: `file.relinkFootage {"folder": "/moved/shots", "dryRun": true, "wait": true}` returns
a report without edits. Omit `dryRun` to apply unique high-confidence matches. Without `wait`,
the command returns a cancellable job; `jobs.wait` / `jobs.list` expose its report.
