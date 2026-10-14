# After Effects project export

> **Last reviewed:** 2026-10-14 · **Last updated:** 2026-10-14 · **Change:** first narrow exporter (`file.exportAep`), verified by opening its output in After Effects 2026 · **Target:** Adobe After Effects 2026

File ▸ Export ▸ After Effects Project… (`file.exportAep`) writes the open project as an `.aep` that
After Effects 2026 opens. It is deliberately narrow: what it writes is exact, tested and checked
against After Effects itself; everything else is **reported** and never dropped silently. The
importer ([aep-import.md](aep-import.md)) is the mirror image; both rest on the clean-room format
notes in [aep-format.md](aep-format.md).

## Design: edit a template, do not write from scratch

An `.aep` has many chunks whose meaning is not known, and After Effects only opens a file in which
they are consistent. A writer that builds the file from nothing therefore cannot work yet. The
exporter edits a **template** instead:

- `crates/aep-export/template/base.aep` is our own generated base project (the fixture `000-base`
  of `effectcraft-aep`: one composition, one red solid layer, the `Solids` folder), without the XMP
  packet after the container;
- `crates/aep-export/template/protos.aep` holds prototype chunks extracted (by
  `examples/mkprotos.rs`) from a project we generated with After Effects: the five Transform
  property records with a static value, the same five with two keys, and a null layer with its
  hidden solid. They are the source of the byte layouts that are not decoded.

Both are embedded with `include_bytes!`, parsed with `effectcraft-aep` on every export, and the
tree is rebuilt: `LIST Item` (composition, folder, footage) and layer blocks are **cloned** from the
template and only fields whose layout is verified ([aep-format.md](aep-format.md) section 3 and the
rules below) are edited. Undecoded bytes are copied unchanged from the template. The file ends with
a neutral XMP packet written by the crate (After Effects accepts a file without one too).

```text
EffectCraft Project --plan--> ProjectPlan (file units, losses reported)
                    --emit--> chunk tree cloned from the template --effectcraft_aep::write--> bytes
```

Where it lives: `crates/aep-export` (`effectcraft-aep-export`, **L3**, next to the importer; no file
access of its own, builds for wasm32). The command wrapper is in `crates/engine/src/commands/aep.rs`.
API: `export(&Project, &ExportOptions) -> Result<ExportResult { bytes, report }, AepExportError>` and
the dry run `analyze(&Project) -> ExportReport`. A refused or failed export writes nothing.

## The command

`file.exportAep {path, allowLosses?, dryRun?}`

- Without `allowLosses` an export that would lose anything is **refused** (nothing is written) and
  the error names the categories, e.g. `exporting would lose effects: 3, text layers: 1; allow
  losses to export anyway`. With `allowLosses: true` it writes the file and returns the same report.
- `dryRun: true` returns only the report. The result is `{path, bytes, report}`. The path is
  confined like every other file-writing command (`crates/automation/src/confine.rs`). From the
  shell: `effectcraft-cli exec file.exportAep '{"path":"out.aep","allowLosses":true}' in.ecproj --json`.

## What is exported

| Area | Exported |
|---|---|
| Project tree | Folders (nesting; empty ones too), compositions, solids (colour, size), item labels and names. Siblings are written in the order After Effects stores them (natural, case-insensitive name order) |
| Composition | size, exact frame rate (the time base After Effects picks for the rate), duration (whole frames), work area (whole frames), background (8 bit), pixel aspect (exact fraction), shutter angle and phase, display start, composition motion blur switch |
| Layers | solid, adjustment and null layers: name, in/out/start (exact units of the composition's time base), enabled/audio/solo/shy/locked/effects/motion blur/collapse/guide switches, draft quality, label, blend mode (Normal, Add, Multiply, Screen, Overlay, Difference), parent |
| Transform | Anchor Point, Position, Scale, Rotation, Opacity: the value when it differs from the default; keyframes with time, linear/bezier/hold on each side, temporal ease (speed and influence, per dimension for Scale), spatial tangents of Position and Anchor Point (automatic ones are written as After Effects computes them), roving and key labels |
| Expressions | on those five properties: source text and enable switch |
| Null layers | a null layer gets the hidden 100 x 100 white solid every null object has (`Null <id>` in the `Solids` folder) |

Units are the importer's inverse: Anchor Point is stored as a fraction of the source size, Scale and
Opacity as fractions (ease speeds are scaled the same way), a property at its default has no record
at all.

## What is reported

`ExportReport` (`serde`, JSON) has `exported` (counts), `losses` (per category: exact `count`, up to
20 `details`) and `limitations` (what the exporter cannot write at all, listed in every report).

| Category | Counted |
|---|---|
| `effects`, `masks`, `markers` | each effect, mask, layer or composition marker |
| `text_layers`, `shape_layers`, `cameras`, `lights`, `model_layers` | each such layer (skipped; the other layers are exported) |
| `footage_items` | each footage item (files, sequences, audio), and items in a folder loop or deeper than 64 levels |
| `unsupported_layers` | footage and precomposition layers, a solid layer whose solid is not exported |
| `blend_modes`, `track_mattes`, `three_d`, `time_stretch` | the layer is written as Normal / without matte / 2D / unstretched |
| `transform_properties` | an X/Y rotation or orientation that is not at its default |
| `approximated_keyframes` | an Anchor Point key with bezier ease (its ease units are not decoded; written linear), keys merged at the same time |
| `invalid_values` | NaN, infinity or a number beyond 1e9 (the default is written), a time beyond the 32-bit range |
| `composition_settings` | a size outside 1 to 30 000, a duration outside 1 frame to 3 hours, a frame rate that has no time base, a pixel aspect that is not an exact fraction |
| `composition_options` | guides, Essential Graphics, the Advanced 3D renderer (written as Classic 3D) |
| `parent_links` | a parent that was not exported or that closes a loop |
| `rounded_times` | a key or work area time that is not a whole unit / frame of the time base |
| `names` | a name longer than 255 bytes (cut) |

## Rules discovered (what After Effects needs for the file to open and read back right)

Evidence is the file name in `E:\Projekte\ae-corpus\export-tests\` (generated by `examples/lab.rs`,
opened in After Effects 2026 and read back through its scripting interface) or the reference
projects made with After Effects (`ref1`, `ref-fps`, `ref-folders`, `ref-longname`). **AE re-save**
means: the exported file was opened, saved again by After Effects and compared chunk by chunk with
`tools/aep_diff.py`; only the noise listed under "not rules" differed, so every byte the exporter
wrote was accepted as it is.

| # | Rule | Evidence |
|---|---|---|
| R1 | The XMP packet after the container is optional; a short neutral one is accepted. | `e0-noxmp`, `x1` |
| R2 | The base project can be edited in place (a renamed composition and layer open and read back). | `e0-rename` |
| R3 | **Time base.** NTSC: time scale 23 976, frame duration 1000 / 800 / 400 / 200 for 23.976 / 29.97 / 59.94 / 119.88 fps. Every other rate: the frame duration is the smallest power of two for which `rate x frame duration >= 24 576` (25 fps: 1024 / 25 600; 12 fps: 2048 / 24 576; 12.5 fps: 2048 / 25 600; 1 fps: 32 768 / 32 768; 240 fps: 128 / 30 720). The 16.16 rate at `cdta@156` is the float32 of the typed rate. | `ref-fps` (13 compositions made by After Effects), `z1` (6 rates written by the exporter, read back) |
| R4 | Composition duration is a whole number of frames (`frames x frame duration`); a 2 s composition at 29.97 fps lasts 2.002 s. | `x3`, `z1` |
| R5 | `cdta` of a new composition: bytes 16, 24, 32 and 40 are 600, byte 48 is the time scale. **The work area is two fractions**: start `@28 / @32`, end `@36 / @40` (absolute end, not duration); once a work area is set the denominators are the time scale, while the whole composition is `0 / 600` and `0xffffffff / 600`. With 600 written as the denominator of a set work area After Effects read the start as 51.2 s. | `y1` (wrong), `y1b` (right), fixture `035` |
| R6 | A composition item uses its own id plus the 11 ids after it for the built-in view layers (`DLay`, 6 x `SLay`, 3 x `CLay`, `SecL`, `ldta@0`); layer ids, item ids and the hidden null solids come from one counter; `head@12` is the next free id. | `x1`-`z1` |
| R7 | **Every property's `tdb4@12` follows the composition's time scale, including those of the view layers.** | `y1`, `z1` (AE re-save identical) |
| R8 | Items are stored in the order the project panel sorts them (natural order of the name, digit runs as numbers, case-insensitive). A composition item is followed by `FEE ` and the 7 f-records, a footage item by the 7 f-records, a folder by nothing. | `ref-folders`; AE accepted the same order in `x3`, `z1` |
| R9 | The `Solids` folder (id 0x19, named by `sfnm/sfid`) holds root-level solids and the hidden solids of nulls; solids with a folder keep it. A root folder named `Solids` in the project **is** that folder (one folder, not two). It is written even when empty (an empty project opens). | `ref1`, `x0`, `x3`, `x7`, `z1` |
| R10 | A **null layer** is an ordinary layer with `ldta@38` bit `0x80` whose source is a hidden 100 x 100 white solid; its Transform defaults are those of a 100 x 100 source. | `ref1`, `x3`, `z1` |
| R11 | Transform properties exist in the file only at a non-default value; they sit in the order Anchor Point, Position, X/Y Position, Scale, Orientation, X/Y/Z Rotation, Opacity, Environment (the exporter inserts into that order). | `x4`, `x5` |
| R12 | A keyed property is the keyed prototype with `lhd3` words 2, 3, 4, 7 and `ldat` rewritten (3, 5 and 20 keys); records are 128 bytes (Position / Anchor Point spatial, Scale plain), 48 bytes (Rotation, Opacity). | `x5`, `y2`, `y3` |
| R13 | **Scale ease is per dimension** (slot `i` of each block belongs to dimension `i`): three different eases read back as three different eases. Upgrades the E mark of aep-format.md 3.5. | `y2` |
| R14 | Automatic spatial tangents are written as the values After Effects computes (EffectCraft's Catmull-Rom); the flag at `@8` is 1. Roving is written as the flag; After Effects moves the key in time itself (4.525 s for the key at 4.5 s with neighbours at 3 s and 6 s). | `y2`, `z1` |
| R15 | An expression is a `Utf8` chunk before `tdum`; `tdb4` bytes 76, 77 and 120 are 1, byte 119 is 1 when it is disabled. A property with an expression but a default value gets a static record with the default. | `x6`, `y2` |
| R16 | Names: a layer's name is the `Utf8` child (80 characters, Japanese and umlauts read back unchanged); `ldta@64` holds the first 31 bytes. A **solid's name is in `opti@26`, up to 255 bytes** (a 140 character name saved by After Effects fills the field as far as it goes). Item names are the `Utf8` child. | `y1`, `z1`, `ref-longname` |
| R17 | `pgui` (16 bytes in a footage item) is a GUID; the exporter writes distinct deterministic ones. | `z1` |
| R18 | Folders clone the `Solids` folder item (`sfdt` 1); `idta@20..23` of a folder is 0 on export and set to 1 by After Effects on save (a UI state, not needed). | `x3`, `z1` re-save |

**Not rules (noise After Effects changes by itself on save):** `head` save counter, `nhed` /
`nnhd` session ids, `fdta`, `ppSn`, item modification times, `idta@23` of folders, `tdb4@119` of the
view layers (1 in the template, 0 after a save), and the cached arc length `f64 @16` of a spatial
key (0 as written, filled by After Effects when it saves a path with a curve).

## After Effects read-back

Each file was opened with `tools/open-in-ae.ps1` in After Effects 2026 and read back through its
scripting interface (comp size, rate, duration, work area, layer names / flags / timing / parents,
property values and keyframes with interpolation and ease); the last column says whether an
AE re-save of the same file differed.

| File | Content | Read back | AE re-save |
|---|---|---|---|
| `e0-noxmp`, `e0-rename` | template without XMP; renamed composition and layer | accepted, names read back | not run |
| `x1` | `C` 1920x1080, 25 fps, 5 s, red solid `L` | identical to the template | not run |
| `x2` | `Comp 2` 1280x720, 30 fps, 4 s, green 400x300 solid, layer `Layer X`; shutter phase -90, motion blur on | all read back | not run |
| `x3` | folder `Folder A` with a composition and a solid, a 29.97 fps composition, a null layer | tree, parents, 29.97 fps (2.002 s), null 100x100 | not run |
| `x4` | all five Transform values | Anchor [100,50], Position [300,200], Scale [50,60,100], Rotation 45, Opacity 25 | not run |
| `x5` | two linear keys on all five properties | 10 keys read back with values and times | not run |
| `x6` | bezier / hold sides with ease, 3 keys on Opacity and Position, an expression | interpolation names, influence 16.67 / 25 / 33.33 / 50 %, speed 100, expression `time*20` | not run |
| `y1`, `y1b` | adjustment, locked / shy / solo, hidden, guide, blends, label, draft, parents, timing, bg, pixel aspect 10/11, shutter, display start, work area, 80 character and Japanese names | everything read back; `y1` exposed R5 | identical (only noise) |
| `y2` | 5 Position keys (labels, manual tangents, roving), per-dimension Scale ease, Anchor keys, hold, disabled expression, 24 fps | all read back | only `ldat` arc lengths and the roving time |
| `y3` | 300 layers x 20 keys, 50 fps | 300 layers | not run |
| `x0` | empty project | opens, the `Solids` folder only | not run |
| `x7` | no layers, an empty folder, duplicate names, a project folder named `Solids` with two solids, a one-frame composition | 8 items, one `Solids` folder with the two solids and the hidden null solid, duplicate names kept | not run |
| `z1` | 6 rates, 6 compositions, folder, 100 character and umlaut solid names, nulls, keys, expressions | 6 rates, durations, 16 items | identical (only noise) |

## How fidelity is checked in this repository

- `crates/aep-export/tests/export.rs`: projects built in code are exported and parsed back with
  `effectcraft-aep`; every exported field of the composition header, the layer header, the solids,
  the Transform values, the keys (time, interpolation, value) and the expressions is compared; the
  container is consistent (writes back byte for byte); the report, the refusal and the JSON round
  trip; the output is deterministic; the embedded template holds no local data.
- `crates/aep-export/tests/roundtrip.rs`: every fixture of `crates/aep/tests/fixtures` is imported,
  exported and imported again; the two imported projects are equal for the exported subset (more
  than 100 compositions).
- `crates/aep-export/tests/robustness.rs`: empty names, huge and zero sizes, NaN and infinite
  numbers, non-positive frame rates, 2 000 layers with 20 000 keys, parent loops, folder loops,
  missing sources, 10 000 character names.
- `crates/engine/src/tests_aep_export.rs`: the command, refusal, dry run, bad input, the menu entry.

## Clean-room method

The embedded template and prototypes are our own generated projects; every layout the exporter
edits is established in [aep-format.md](aep-format.md) from our own corpus or in the table above
from experiments against After Effects itself. No source code or format documentation of any other
`.aep` reader or writer was used.

## Not done yet

Everything the importer does not read either: effects, masks, text, shapes, cameras, lights,
markers, track mattes, time remapping, layer styles, 3D rotations, precomposition layers
(`idpc`, nested composition flags), footage items (alias / path records), time stretch (the key
time scale of a stretched layer), the Advanced 3D renderer, project settings, the render queue.
Real-project fidelity (rendering an EffectCraft project and its After Effects export and diffing)
is not measured.
