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
| **PNG Sequence** (EXR, TIFF, JPG… after the file type) | On: the whole run imports as one sequence. Off: just the picked file imports, as a still. |
| **Force alphabetical order** | Imports every image of that type in the folder in name order, whatever the numbering, and plays the files one after another (gaps in the numbering don't count). Use it for files that aren't numbered consistently. |
| **Frame rate (fps)** | The sequence's frame rate. The default comes from Settings ▸ Import ▸ Sequence Footage (30 fps). |

Other ways in:

- **Pick part of a run** (Shift-click the first and the last frame in the file dialog) to import
  just that range as one sequence.
- **Pick or drop every frame** of the run: it arrives as one sequence, not one item per frame.
- **Drop a folder**: its media files import, and each numbered run in it becomes a sequence.
- **In the browser** (the web app) there are no folders to look in, so pick or drop all the
  frames of the sequence at once.

Stills of any format EffectCraft reads can form a sequence: PNG, JPEG, TIFF, OpenEXR, BMP and
WebP. Layered and vector documents (PSD, SVG, PDF) import one by one.

**Missing frames.** Where the numbering skips a number (`shot_0003.png` is missing between
`0002` and `0004`), the sequence shows a placeholder of colour bars for that frame, as After
Effects does, so every other frame stays at its number. Render or copy the missing files into the
folder and choose File ▸ Reload Footage to fill the gaps.

Agents: `file.import {"paths": ["/shots/shot_0001.png"], "sequence": true, "frameRate": 24}`.
`sequence` is on by default; `"sequence": false` imports the file as a still, and
`"alphabetical": true` is Force Alphabetical Order.

## Interpret Footage

Select a footage item in the Project panel and choose **File ▸ Interpret Footage ▸ Main…**
(Ctrl+Alt+G / Cmd+Opt+G) to change how it is read. A change applies everywhere the footage is
used, and layers that ran to the end of the footage keep doing so.

| Setting | What it does |
|---|---|
| **Alpha** | *Straight - Unmatted*, *Premultiplied - Matted With Color* (with the matte colour), *Ignore* (opaque), or *Guess* (EffectCraft looks at the first frame). *Invert Alpha* flips it. |
| **Assume this frame rate** | The rate the frames play at. A 250-frame sequence at 25 fps lasts 10 seconds; at 24 fps, 10.4 seconds. |
| **Start Frame** (sequences) | The frame number at the footage's first frame. It defaults to the first file's number. A higher number trims the head (start a 1001–1250 render at 1010); a lower one adds placeholder frames before the first file. |
| Fields, pixel aspect, loop, colour profile, linear light | As in After Effects. |

Settings ▸ Import ▸ **Report Missing Frames** (on by default) lists the missing frame numbers
when a sequence imports; Interpret Footage lists them too.

**File ▸ Interpret Footage ▸ Remember Interpretation / Apply Interpretation** copy the alpha,
fields, pixel aspect, loop and colour settings to other footage.

Agents: `file.interpretFootage {"items": [id], "frameRate": 24, "alpha": "premultiplied",
"startFrame": 1001}` (`"startFrame": "file"` goes back to the first file's number).

## Replacing and reloading

- **File ▸ Replace Footage ▸ File…** (Ctrl+H) points the item at another file. Pick a numbered
  file to replace it with that file's whole sequence (`file.replaceFootage {"path": …,
  "sequence": false}` for one frame).
- **File ▸ Reload Footage** (Ctrl+Alt+L) reads the files again. A sequence picks up frames
  added to its folder (a render still in progress) and keeps its interpretation.
