# Generator of the combined test projects

The test projects of the corpus (opt-in, the folder named by `EFFECTCRAFT_AEP_FIXTURES`) were made from scratch with Adobe After Effects 2026, driven through an
After Effects MCP server (its `batch.run` of atomic operations; `eval.run`, i.e. running
ExtendScript, is disabled on that server, so the tables below are written in MCP operation terms
and small Node scripts turn them into `batch.run` lists). Each file holds one composition per
case, named like the case (`../CASES.md`); every case composition starts as a duplicate of the
base composition `000-base` (1920x1080, 25 fps, 5 s, black, one red 1920x1080 solid `L`) and adds
the case's change (and the chain of cases it was made from).

Paths in the scripts are written `<corpus>/...`: a local working folder outside the repository
that holds an empty project (`combined/gen/empty-work.aep`, a copy of the corpus's `empty.aep`), the test
media the footage cases import (`synthetic/test-media/`: a png, a WAV, png and OpenEXR sequences,
an mp4, Targa, Photoshop, Illustrator and EPS stills, all made by the contributor), the earlier
single-case files some cases are merged from, and the contributor's own animation presets
(`presets/`). Set them before running.

## Files

| file | what |
|---|---|
| `effects.jsx` | effects cases (the trimmed table of the cases the tests use) and the templates `000-base`, `520-solid-M-grey-960x540` |
| `to-mcp.js` | turns `effects.jsx` into `ops/batch-NN.json` |
| `masks.jsx`, `shapes.jsx`, `styles.jsx` | masks, shapes and styles cases (styles also 963, 964, 985, 340, 734) |
| `mss-to-mcp.js` | turns one of those three into `ops/<family>/batch-NN.json` and `merge.json` |
| `layers-comps.jsx`, `keyframes.jsx` | layer / composition / transform / matte / blend-mode cases, and key / ease / expression / stretch cases; each runs under Node and writes its own batches; `layers-comps.jsx` also holds the `.aep` / `.aepx` pair |
| `text.jsx` | text cases; runs under Node and writes its batches |
| `fam-common.jsx`, `fam-to-mcp.js` | shared helpers and translator of the families `3d.jsx`, `footage-precomp.jsx`, `markers.jsx`, `settings.jsx` (settings: one project per group of cases, `settings-a/b/c`) |
| `scrub.py` | fixture hygiene of a saved `.aep`: footage alias paths replaced by neutral ones of the same length (`C:\fixtures\...`), host name `HOST000...`, neutral ascend counts, the XMP packet replaced by the base project's |
| `layers-comps-scrub-aepx.py` | the same XMP swap for the `.aepx` of the pair |
| `patch-text.py` | the byte edits of the text cases 732 and 733 (see below) |

## Run order (per file)

1. Make sure After Effects shows no dialog. Save the open project somewhere harmless first
   (opening a project over a dirty untitled one raises a Save As dialog), then open
   `<corpus>/combined/gen/empty-work.aep`.
2. Translate the table (`node to-mcp.js ops`, `node mss-to-mcp.js <family>`,
   `node layers-comps.jsx <outdir>`, `node keyframes.jsx <outdir>`, `node text.jsx <outdir>`,
   `node fam-to-mcp.js <family> setup` then `... cases <out> ids-<out>.json`) and run the batches
   in order with `batch.run` (template batch first; check `failed` is 0 after each).
   - Footage cases: import the media first (`project.import_file`); the translators take the new
     item id (`stepsItemId`, or the tagged ids of the setup batch) and write the footage batches.
   - `mss-to-mcp.js`: after the template, merge each project of `merge.json`
     (`project.merge`) and rename its comp as listed; after the cases delete the `src-*` comps.
   - A keyed multi-dimensional property is read once (`property.get`) before the save, or After
     Effects does not write the first key's speed (the tables do this where the case needs it).
3. Save the project (`ae_save_project`); for the pair, save as `aepx-pair.aep` and right after as
   `aepx-pair.aepx`.
4. Text only: `python -I patch-text.py text.aep text-patched.aep` makes the same one-byte edit
   of a paragraph slot in the text documents of 732 and 733 as the earlier single-case files
   (built as case 517; the slot is not reachable by scripting).
5. Scrub: `python -I scrub.py <saved>.aep <fixture>.aep <base project>.aep` (and
   `layers-comps-scrub-aepx.py` for the `.aepx`). Set `SCRUB_FORBID` to the local folder and
   machine names that must not remain; the script fails if one does.
6. Check: every case composition must import like the earlier single-case file (the tests in
   `../../` are that check; `../CASES.md` lists the cases).

Project settings have one value per project, so the settings cases are split over three files.
In a fresh save After Effects 26.2.1 no longer writes the working-space profile name, only the
profile data; the reader then takes the name from the profile's own description.

`fx-switch.aep` is not generated: it is the base project with two Gaussian Blurs (Blurriness 40) on `L`, added through the MCP server, the first switched off by hand in the Effect Controls (the server has no operation for the fx switch), saved once and scrubbed with `scrub.py`.
