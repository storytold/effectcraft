# effectcraft-aep

After Effects projects (`.aep`, `.aepx`) in one crate, the way `effectcraft-lottie` handles Lottie:

- `container`: reads and writes the container as a lossless chunk tree, with a typed, read-only
  view (below; its API is re-exported at the crate root);
- `import`: maps the typed view into an EffectCraft project and reports everything it does not
  import ([`docs/aep-import.md`](../../docs/aep-import.md));
- `export`: writes an EffectCraft project as an `.aep` by editing a generated template embedded
  from `template/`, and reports everything it does not export
  ([`docs/aep-export.md`](../../docs/aep-export.md)).

No file access: callers pass bytes, so the crate builds for wasm32.

## Container

An `.aep` is a `RIFX` container (big-endian RIFF) with form type `Egg!`: chunks of a 4-byte id, a
big-endian `u32` size and a body padded to an even length; `LIST` chunks start with a 4-byte list
type followed by chunks. `parse` builds the tree, `write` serialises it, and for every input that
parses, `write(parse(x)) == x` byte for byte (container and chunk sizes are recomputed from the
tree; pad bytes, odd bodies and the bytes after the container, an XMP packet, are kept).

The XML form of a project (`.aepx`) is read into the same tree by `read_aepx` (a small bounded
XML tokenizer, no dependency); `parse_any` picks the reader by content, so the typed view and the
importer take either form. The mapping is in `docs/aep-format.md`, section 1.

Bodies that look like lists but are not chunk sequences (for example `LIST btdk`, raw text) stay
opaque rather than failing the parse. Input is untrusted: sizes are checked against the remaining
input, nesting and chunk count are bounded (`Limits`), nothing is allocated from a declared size,
and truncated or damaged files return `Err` (or, inside the container, an opaque body).

The container layer decodes no chunk body. On top of it, a typed view (`Project`, `Item`, `Comp`, `Layer`,
`Property`, ...) reads the fields whose layout was verified against the values the test files were made
with: layer and composition headers (switches, exact times and time stretch, size, frame rate, work area, shutter phase, display start, renderer),
static transform values in After Effects' units with the "absent = default" rule, keyframe records
(interpolation sides, ease per component, plain and spatial templates), Time Remapping, expressions with their enable switch, effect
instances and parameter values, masks and bezier paths with tangents, shape layer groups, text documents
(paragraph and style runs, box, justification, tracking, faux styles, caps, leading), markers with
duration, strings and cue data, cameras and lights with their options, footage (stills, audio, image
sequences), solids and precomp links. Every accessor returns `Option`/an empty list on damaged data and never
panics. Which fields are verified, which are not decoded, and how it was found is in
[`docs/aep-format.md`](../../docs/aep-format.md).

## Clean room

Everything here comes from our own observations: project files that we generated ourselves with
After Effects, one change per file, compared byte by byte. No source code, specification or
format documentation of another reader was used, and no Adobe asset is used. The test fixtures are such generated files
(own test data): a few combined projects in `tests/fixtures/`, one composition per case
(`tests/fixtures/CASES.md`; the generator is in `tests/fixtures/tools/`). The tests reach them only
through `tests/common/mod.rs`, which hands a test its case as a project of its own cut from the
combined file (or reads the folder in `EFFECTCRAFT_AEP_FIXTURES`); a test whose fixture is missing
prints `SKIPPED: fixture <name> missing` and passes.
