# After Effects project import

> **Last reviewed:** 2026-10-13 · **Last updated:** 2026-10-13 · **Change:** first narrow importer (`file.importAep`) on top of the clean-room `effectcraft-aep` reader · **Target:** Adobe After Effects 2026

File ▸ Import ▸ After Effects Project… (`file.importAep`) reads an `.aep` and adds its items to the
open project, in a folder named after the file (as After Effects does when it imports a project).
It is deliberately narrow: what it brings over is exact and tested, everything else is **reported**
and never dropped silently. Wider coverage grows with the reader's verified field layouts
([aep-format.md](aep-format.md)); this page lists what exists today.

## Where it lives

`crates/aep-import` (`effectcraft-aep-import`, **L3**) sits between the reader `effectcraft-aep`
(L1, the typed view of the file) and the project model `effectcraft-project` (L2), the same place
`effectcraft-lottie` has for Lottie. Why a crate of its own and not a module of the engine:

- the PSD importer lives in the engine (`crates/engine/src/psd_import.rs`) because it creates
  raster layers from engine services; the `.aep` import needs nothing of the engine, only the
  project model, so it can be built, tested and fuzzed alone and is available to every frontend;
- it must build for wasm32 (`cargo xtask layers` and `cargo xtask wasm` cover it): it does no file
  access, the caller passes the bytes and a "does this footage path exist" callback;
- the command (`file.importAep`, `crates/engine/src/commands/aep.rs`) is a thin wrapper: read the
  file, run the importer inside one undoable edit, open the first composition, return the report.

API: `import(&mut Project, bytes, &ImportOptions) -> Result<ImportResult, AepImportError>` and the
dry run `analyze(bytes, exists) -> ImportReport`. An error (not a project, damaged container,
refused losses) leaves the project unchanged.

## The command

`file.importAep {path, allowLosses?, dryRun?}`

- Without `allowLosses` an import that would lose anything is **refused** (nothing changes) and the
  error names the categories, e.g. `importing would lose effects: 3, masks: 1; allow losses to
  import anyway`. With `allowLosses: true` it imports and returns the same report.
- `dryRun: true` returns only the report.
- The result is `{folder, comps, items, report}`; one undo step removes everything. The first
  composition opens in the viewer. The path is confined like every other file-reading command in
  the automation layer; from the shell:
  `effectcraft-cli exec file.importAep '{"path":"in.aep","allowLosses":true}' out.ecproj --save`.

## What is imported

| Area | Imported |
|---|---|
| Project tree | Folders (nesting and order), compositions, solids (colour, size), footage items; item labels |
| Composition | name, size, exact frame rate, duration, work area, background colour (8 bit), pixel aspect, composition motion blur switch, shutter angle and phase (as stored), display start time, Classic/Advanced 3D renderer |
| Layers | solid, null, adjustment, footage and precomposition layers: name (the source's name when the layer has none), in/out/start and time stretch (exact ratios, e.g. 50 % and 150 %), enabled/audio/solo/shy/locked/effects/motion blur/collapse/3D/guide switches, draft quality, label, blend mode (Normal, Multiply, Add, Screen, Overlay, Difference), parent |
| Transform | Anchor Point, Position, Scale, Rotation, Opacity: the stored value, or the default when the file stores none; keyframes with time, linear/bezier/hold on each side, temporal ease (speed and influence, per dimension for Scale), automatic or manual spatial tangents, roving and key labels |
| Expressions | on those five properties: source text and enable switch |
| Footage | a link to the stored path; **missing-footage placeholder** when the path does not exist where the import runs (listed in `missingFootage`), a normal link when it does. The importer never opens footage |

Units, the "absent = default" rule and the keyframe templates are the reader's
([aep-format.md](aep-format.md), sections 3.3–3.6). The importer converts them to EffectCraft's:

- **Anchor Point** is stored as a fraction of the source size and becomes pixels; **Scale** and
  **Opacity** are stored as fractions and become percent (ease speeds are scaled the same way);
  Position is pixels, Rotation degrees.
- **Time.** Keyframe times are layer time and become exact `Tick`s. A time that falls on a frame
  of the composition is `FrameRate::tick_of(frame)`, so frames stay frames; other times round to
  the nearest tick. The NTSC rates are stored as typed (`23976/800` is 29.97, `23976/1000` is
  23.976) and become the exact rates 30000/1001 and 24000/1001 (59.94 and 119.88 likewise); every
  other rate is kept as the stored fraction. A layer's in/out are `start + offset × stretch`,
  computed on the stored denominators with the exact stretch ratio.
- **Position.** Absent means the composition centre at the time of the import. A composition that
  was resized without moving the layer stores the old centre, and so does the project.
- **Spatial tangents.** The switch at byte 8 of a spatial key (1 normally, 0 after the tangents
  were set by hand) is read as "automatic spatial tangents": 1 becomes EffectCraft's automatic
  tangents, 0 the stored tangents. That reading is a hypothesis from one sample pair (055/054), see
  the open list in aep-format.md. Anchor Point keys keep their values and tangents, but their ease
  is not decoded (bezier sides become linear and are reported).
- **Null objects.** The hidden placeholder solid of a null object is not an item (null layers
  have no source item here).

## What is reported

`ImportReport` (`serde`, returned as JSON) has `imported` (counts), `losses` (per category:
`count` exact, up to 20 `details`), `missingFootage` and `limitations`. The categories:

| Category | Counted |
|---|---|
| `effects` | each effect on an imported layer, by name |
| `masks` | each mask |
| `markers` | each layer and composition marker |
| `text_layers`, `shape_layers`, `cameras`, `lights` | each such layer (the layer is skipped; the other layers of the composition are imported) |
| `blend_modes` | a blend mode outside the six above (the layer stays Normal) |
| `transform_properties` | a stored transform property other than the five (X/Y rotation, orientation, separated position dimensions) |
| `time_remapping` | an enabled Time Remap (decoded by the reader, but the project has no time remap yet) |
| `approximated_keyframes` | a key imported in a simpler form (unknown interpolation code, bezier ease that cannot be decoded: Anchor Point ease or non-finite ease data) |
| `parent_links` | a parent that was not imported (a skipped camera, a missing layer) or that forms a loop |
| `unsupported_layers` | an unknown layer type, a layer without a usable source, a precomposition loop (the layer is cut) |
| `unsupported_items` | a project item of an unknown kind |
| `damaged_data` | a header, key or value that could not be read, numbers that are not finite or beyond ±1e9 (default used) |

`limitations` states what the reader cannot see at all, so it can be neither imported nor counted:
track mattes, frame blending, layer styles, 3D material options, audio levels, guides, and footage duration/frame rate. They are listed in every report.

## How fidelity is checked

- `crates/aep-import/tests/import.rs`: the synthetic fixtures of `crates/aep/tests/fixtures` (151 files)
  (each the same one-solid project with one change) are imported and the resulting project is
  compared with the value the fixture was made with: composition size/rate/duration/work
  area/background, layer names, switches and timing, blend modes, parent links, static
  Position/Scale/Rotation/Opacity/Anchor with their units, key times and values, interpolation sides,
  ease, tangents, expressions, the NTSC rates, and one report category per unsupported feature.
  The report round-trips through JSON.
- `crates/aep-import/tests/robustness.rs`: every fixture truncated at 40 places, with bit flips and
  runs of `0x00`/`0xff`, and files rewritten with hostile values in valid structure (zero or
  maximal sizes and time bases, NaN/infinity/huge numbers in every property, stretch 0, self and
  mutual parents, a precomposition that contains itself). The result is an error or a report; the
  project is always valid (keys sorted, finite values, no parent loops).
- `crates/engine/src/tests_aep_import.rs`: the command, undo/redo, refusal, dry run, bad input and
  the menu entry.

No Adobe project or asset is used: the tests read only the generated fixtures. The comparison of
an imported real project against After Effects itself (rendering both and diffing) is not done yet
and is the next fidelity step.

## Clean-room method

The importer reads the file only through `effectcraft-aep`, whose every field was established by
one-change-per-file experiments on projects we generated ourselves ([aep-format.md](aep-format.md),
section 5). The importer adds no knowledge of the format of its own: when the reader has no
verified field for something, the importer reports it instead of guessing. No source code or format
documentation of another `.aep` reader or converter was used; the evidence for every field is the
generated corpus and its case numbers, not a claim about what was or was not seen elsewhere.

## Export

The other direction, File ▸ Export ▸ After Effects Project… (`file.exportAep`), is described in
[aep-export.md](aep-export.md).

## Not done yet

Effects, masks, shape and text layers, cameras, lights, markers, track mattes, time remapping,
layer styles, 3D rotations, separated dimensions, footage interpretation and the project settings
(colour depth, working space) need verified layouts in the reader first. `.aepx` (the XML
flavour) is not read.
