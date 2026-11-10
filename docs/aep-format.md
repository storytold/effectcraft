# After Effects project format (`.aep`)

> **Last reviewed:** 2026-11-05 · **Last updated:** 2026-11-07 · **Change:** Puppet (`ADBE FreePin3`, 3.7): the property tree from an external scripting listing, **?** until a byte sample exists (#3093); before that: footage: an image sequence without a filename prefix stores only the suffix `Utf8`, and one imported with Force alphabetical order stores its file list in `LIST StVc` / `StVS` (section 3, footage); before that: versions: `svap` is the version that wrote the file, `head` bytes 4 to 7 the version it is written for, and what a Save a Copy As 25.x / 24.x of After Effects 2026 changes (section 1, "Versions"); before that: the stretch numerator is signed (a negative one reverses the layer, 981-983); layer styles (3.12b, 950-980); before that: footage: `sspc@118` of an image sequence is its folder's modification time (860, read-back x1-x3); before that: text: paragraph `/24`, `/35` and style `/68` are neutral for 0 and 1 (732-734, edited in place); before that: keyframes: the ease of mask path keys (599, 700), the spatial speed unit of fractional values (597, 598, 730); before that: text: character and paragraph settings, manual kerning, vertical text, `/nil` is neutral (S17: 538-578, 595, 596); before that: cameras, lights and 3D rotations (S16: the auto Position of a two-node camera is where After Effects renders it, 537; camera and light defaults read back 255, 256; Orientation `otst/otda` 488), Timewarp (529-533), an expression on an effect parameter (534); before that: text: style slot /13 is superscript / subscript (526-528; every scripted text of the corpus is superscript), wrapping box text 517; before that: effects of batch 5 (409-450, 511-516: integer sliders, checkbox defaults in `pard`, arbitrary data of Hue/Saturation and Levels, the parameters of Invert, Fast Box Blur, Gaussian Blur, Drop Shadow, Glow; section 3.7); before that: shape items of batch 5 (451-484, 519: ellipse, fill and stroke opacity, fill rule, group transform, shape layer anchor, trim paths, rectangle roundness, polystar fields, gradient default, nested groups, the defaults of every item; section 3.9); before that: track mattes: mode at `ldta` byte 107 and the matte layer id at byte 160 (cases 402-408, section 3.1); before that: text: automatic leading is verified (400); Exposure (`ADBE Exposure2`, cases 389-397): group start / end parameter types 13 / 14, float slider valid minimum and slider range (section 3.7); before that: text: U+0003 is a paragraph break (386, 387); before that: a stored text Anchor Point is pixels (385, now V); before that: the XML form `.aepx` is read into the same chunk tree (section 1, "The XML form"); the `head` save counter is a `u32` at byte 16; text documents for the importer (stroke colour, automatic leading, the font of each run, the undecoded-slot check, the text-layer Anchor Point, the Source Text record); before that: third round of verified field layouts (explicit-default semantics, type-major bezier ease, exact time stretch and in / out points, Time Remapping state, comp pixel aspect / shutter phase / display start, Point default, Layer Control None, dropdown selection, one `mkif` per mask, polystar and radial gradient records, text styles, camera and light options, marker cue data, audio and image-sequence footage) added to the typed view of `effectcraft-aep` · **Target:** Adobe After Effects 2026

What `crates/aep` ([`effectcraft-aep`](../crates/aep/README.md)) reads and writes, and the evidence
for each statement. Everything here was observed on project files we generated ourselves; no
source code, specification or format documentation of any other reader was used (every layout is
backed by our own cases, see section 5). This is the container and a set of
verified field layouts, not a project importer: turning a parsed `.aep` into an EffectCraft
document is done by the `import` module of the same crate, see [aep-import.md](aep-import.md) (status and
gaps: [gaps.md](gaps.md) G4 and [file-format-parity.md](file-format-parity.md)).

Evidence is cited as **case numbers**: the files `NNN-*.aep` of our synthetic corpus, each made by
one scripted change to a base project. Batch 1 (cases 000 to 126) was made in one chain of
Save As; batch 2 (cases 200 to 259) was made in later sessions, each case starting again from the
same loaded base; batch 3 (cases 299 to 354) was made in three further sessions (299 is a Save As
without any edit, the reference for save noise). The base project `000-base` is one composition `C`
(1920x1080, 25 fps, 5 s) holding one red solid layer `L` (1920x1080) in the folder `Solids`. 374 of
these cases (369 `.aep`, and the `.aepx` of `000`, `058`, `060`, `110`, `124`) are the test fixtures. They
are a few combined projects, one composition per case named like the case and rebuilt from the
same recipe (`crates/aep/tests/fixtures/CASES.md`, the generator in `tools/` there), read from the
folder named by `EFFECTCRAFT_AEP_FIXTURES` (opt-in); `crates/aep/tests/it/common/mod.rs` hands a test its case as a project of its own cut from
the combined file, and a test whose fixture is missing prints `SKIPPED: fixture <name> missing` and passes. The tests named below run on them;
the `.aepx` cases are one pair, `aepx-pair.aep` and `aepx-pair.aepx`. The container
of every fixture is byte-identical to the generated file, with two exceptions that keep the
fixtures small and free of local data: the XMP packet after the container (which grows by 1.7 KB
with every save of the corpus chain) is replaced by the 5.8 KB packet of the base project in all
fixtures but `001` (and inside `<ProjectXMPMetadata>` of the five `.aepx` fixtures), and in every fixture that imports a file (a footage item: `126`, `350`-`354` and the later footage cases) the
file path and machine name inside the footage alias JSON are replaced by neutral text of the same
length.

**Read-back tests.** Some evidence comes from files we wrote ourselves (not After Effects), opened
in After Effects 2026 and read back value by value or rendered: an exporter in development, which is
not in this repository. They are named by an id (`e1`, `f1`, `o1`, `s1`, `t7`, `x1`-`x3`, ...) or by
their local folder (`export-tests\...`), which is not in the repository either.

Confidence codes in the field tables: **V** = a test in `crates/aep/tests/` asserts that the decoded
value equals the value the case was made with (the corpus manifest); **E** = decoded and consistent
in the case diffs, but no manifest value to assert against (the field is exposed only where a test
pins what the fixtures contain); fields that the corpus does not decide (**?**) or only suggests
(**H**) are not decoded; they are listed in section 4.

## 1. Container

| Fact | Evidence |
|---|---|
| The file starts with `RIFX`, a big-endian `u32` size and the form type `Egg!`; all integers in the structure are big-endian (the one exception, `iide`/`idpi`, is little-endian). | All 139 files of batches 1 and 2. |
| The declared size is `4 + the chunks`; it ends before the end of the file. The bytes after it are an XMP packet (`<?xpacket begin` ... `<?xpacket end=...?>`, UTF-8 XML, 5.8 KB in `000`) that grows with every save. | All files; test `header_and_trailer_facts`. |
| A chunk is a 4-byte id, a big-endian `u32` body size and the body; an odd-sized body is followed by one pad byte, always `0` in the corpus (15 622 odd bodies). | Test `odd_sized_chunks_keep_their_pad_byte_even_when_not_zero` keeps any value. |
| The chunks of every file tile the container exactly: no leftover bytes, no chunk larger than its parent. | 139 of 139 files parse without a repair and write back byte for byte. |
| A `LIST` chunk's body is a 4-byte list type followed by chunks. List types seen include `Fold`, `Item`, `Layr`, `tdgp`, `tdbs`, `list`, `Pin ` and `FEE ` (the space is part of the type). | `000`; test `list_types_ending_in_a_space_are_kept`. |
| Lists can be empty (size 4, only the type): `Ewst`, `Pefl`, `CPPl`, `LItm`. | `000`; test `empty_list_and_list_shorter_than_its_type`. |
| Nesting: at most 13 levels in the corpus (the path of a shape layer, 240). | All files; test `nesting_depth_is_below_the_limit`. |
| `LIST btdk` is not a chunk sequence: its body (7775 bytes in `110`) is text. A reader must keep it opaque. Only files with a text layer have it. | `110`-`114`, `243`-`248`; test `raw_text_lists_are_kept_opaque`. |
| Three non-`LIST` ids hold a chunk instead of data: `tdsn`, `pdnm`, `fnam`. Each holds exactly one `Utf8` chunk (a name or label, UTF-8 text, possibly empty). | `tdsn` 31 209 times, `fnam` 40, `pdnm` 32 over the corpus; test `nested_chunk_ids_hold_one_utf8_chunk`. |
| Ids can end in a space; the space is part of the id. In the corpus this happens for the list types `Pin ` and `FEE `. All ids are 4 printable ASCII bytes. | `Pin `, `FEE ` in all files but `001`; tests `list_types_ending_in_a_space_are_kept`, `ids_ending_in_a_space_are_kept`. |
| Every payload that is decoded here is plain: no compression, no encryption. Text is readable, numbers are IEEE big-endian doubles or integers. Entropy and decompression probes (zlib, raw deflate, gzip, lzma, bz2, LZ4 block) over all chunk ids of the corpus found no compressed chunk, and consecutive saves of a project differ in only a few bytes. **One body is not understood: `opti` (footage and render options) has a plain 125-byte header and then a high-entropy body (7.9 bits per byte); whether it is compressed is open.** The high entropy of `sdat` is a flat histogram (five identity ramps 0..255), not random data. | `010` (the layer name `Renamed` is readable in a `Utf8` chunk), `040` (`100.0` and `200.0` as doubles). |

### How the crate handles it

- `parse` builds a tree of `Chunk { id, body, pad }`; a body is `Data`, `Chunks` (the three
  nested ids), `List { list_type, children }` or `OpaqueList { list_type, data }`. The bytes after
  the container are kept as the trailer.
- `write` recomputes every size from the tree. For every input that parses, `write(parse(x)) == x`
  byte for byte, including pad bytes, an odd last body without a pad byte, and the trailer. The
  tests assert this for all fixtures and for every damaged input that still parses.
- A `LIST` (or nested id) whose body is not a well-formed chunk sequence, or that is nested deeper
  than `Limits::max_depth` (64), stays opaque instead of failing the parse. At the top level, a
  chunk that does not fit, a wrong form type or a container larger than the input is an error.
  Sizes are checked against the remaining input; nothing is allocated from a declared size; the
  chunk count is bounded (`Limits::max_chunks`).

### The XML form (`.aepx`)

After Effects also saves a project as XML (`.aepx`). Every case of the corpus was saved in both
forms from the same state, the `.aepx` right after the `.aep`. Comparing each pair element by
element with the chunk tree of the `.aep` gives the mapping below. It holds for all 223 pairs of
the corpus (batches 1 to 3 and the 29 cases 355 to 383 of a fourth batch made the same way, which
this document does not use otherwise): the tree read from the `.aepx` is the tree of the `.aep`, and written back as an
`.aep` it gives the same bytes, apart from the save counter and the XMP packet (last row).

| Fact | Evidence |
|---|---|
| The file is UTF-8 XML with the declaration `<?xml version="1.0" encoding="UTF-8"?>` and one root element `<AfterEffectsProject xmlns="http://www.adobe.com/products/aftereffects" majorVersion="1" minorVersion="0">`. Its children are the top-level chunks of the `.aep`, in the same order. The `RIFX` header and the `Egg!` form type have no element. Indentation is tabs and line feeds; there are no comments, no entities and no carriage returns in any sample. | All pairs; `000`. |
| A data chunk is an element named by its id, with the body as lowercase hex in the attribute `bdata` and no content: `<head bdata="0061...3b"/>`. Every body of the `.aep` comes back byte for byte, including large ones (`cdat`, `ldat`, `opti`). | `000` (1 183 `bdata` elements, all equal but `head`, last row), all pairs. |
| A `LIST` is an element named by its list type, holding its chunks as child elements: `<Fold>`, `<Item>`, `<Layr>`, `<tdgp>`, `<tdbs>`, `<list>`. An empty list is an element without children and without `bdata` (`<Pefl>`, `<CPPl>`, `<Ewst>`, `<LItm>`). The `LIST` id itself is not written. | `000`. |
| A trailing space of an id or list type is dropped: `Pin ` is `<Pin>`, `FEE ` is `<FEE>`. A name shorter than 4 characters is padded with spaces to read it back. | `000` (`FEE`, and `Pin` of the solid's footage), `126`. |
| The same name can be a list or a data chunk: `sspc` is a data chunk in a footage item (`<sspc bdata=.../>`) and a list in an effect instance (`<sspc>` with children). Whether an element has `bdata` decides. | `000` (footage), `071` (Slider Control). |
| The three nested ids `tdsn`, `pdnm` and `fnam` are written like lists: an element with one child, the `Utf8` chunk. | `000` (`tdsn`), `071` (`pdnm`, `fnam`). |
| A `Utf8` chunk is `<string>` with its text as content; an empty one is `<string/>`. The text is the chunk body exactly (no trimming). | `000` (`C`, `Solids`, the JSON strings of the colour settings, 19 empty ones), `010` (`Renamed`), `060` (`wiggle(1,10)`). |
| Two numeric chunks are written as text, in a `<string>` child: `numS` (4 bytes, big-endian) as a decimal integer (only `0` is sampled, so whether it is signed is not decided; the reader accepts -2^31 to 2^32-1), and `ppSn` (8 bytes, an `f64`) with nine decimals (`150.000000000`, `0.000000000`). A `ppSn` value that is not a multiple of 10^-9 would be rounded by the XML form (no sample). | `000`, `124` (two compositions: two of each), all pairs. |
| `LIST btdk` (the text document, raw text in the `.aep`, see 3.10) is written like a data chunk: `<btdk bdata=.../>`, the hex of the bytes after the list type. It is read back as an opaque list. | `110`-`114`, `243`-`248`, `334`-`340`. |
| The footage alias `alas` (a JSON object in the `.aep`) is the element `<fileReference>` inside `<Als2>`, with one attribute per JSON key, in the same (alphabetical) order: `ascendcount_base`, `ascendcount_target`, `fullpath`, `platform`, `server_name`, `server_volume_name`, `target_is_folder`. The JSON writes the two counts as numbers, `platform` `Win` as `1` and `target_is_folder` `0` / `1` as `false` / `true`, and escapes a backslash of the path as `\\`. Rebuilt this way, the JSON equals the `.aep` body byte for byte. Other platforms and non-ASCII paths were not sampled. | `125`, `126`, `350`-`354` (`352`-`354`: an image sequence, `target_is_folder` `1`). |
| The XMP packet that follows the container of an `.aep` is the CDATA content of `<ProjectXMPMetadata>`, a child of the root between the top-level `string` that follows `pdvc` and `<ExEn>`. It is not a chunk. | All pairs. |
| Pad bytes are not written. An odd body gets a zero pad byte when the tree is written as an `.aep`, which is what every `.aep` of the corpus has. | All pairs: the rewritten tree equals the `.aep` byte for byte. |
| The only differences inside the container are save state: the save counter, a big-endian `u32` at byte 16 of `head`, is one higher in the `.aepx` (only byte 19 changes in 222 pairs; in `314` it carries from `0x000000ff` to `0x00000100`), and the XMP packet has one more history entry. | All pairs. |

**What the XML form leaves out.** In the corpus, nothing: every chunk of every `.aep` has its
element (the count per id agrees in all 223 pairs, apart from the `<string>` children of `numS` and
`ppSn` and the renamed `alas`), and all 239 pairs of the current corpus read into the same chunk
tree (the local test `every_corpus_pair_gives_the_same_chunk_tree`). The reader still tolerates an
`.aepx` that lacks chunks: `elab`, `cmta` and `empd` are not read by the typed view, so a missing one
changes nothing it decodes, and a missing `tdbs` cannot be told apart from a property at its default
(the sparse rule of section 2): its value reads as the default. Elements that have no chunk
counterpart (a name that is not 1 to 4 characters) are skipped with their content.

### How the crate reads an `.aepx`

- `read_aepx` returns the same `AepFile` as `parse`; `parse_any` picks the reader by content (a
  `RIFX` header, or an optional byte order mark and white space followed by `<?xml` or
  `<AfterEffectsProject`). Everything above the container (the typed view, the importer) works on
  either form unchanged.
- The XML reader is a small tokenizer for exactly this subset: elements, attributes, text, the five
  predefined entities and character references, CDATA, comments and processing instructions. Text
  is taken as written (no line-ending normalisation). A document type declaration is refused, so no
  entity is ever expanded.
- Input is untrusted: bad UTF-8, an unknown entity, an unclosed or mismatched tag, invalid or
  odd-length hex, a number that does not parse or a second root element return `AepError::Xml`
  with the byte offset. It runs without recursion; the nesting depth (`Limits::max_depth`) and the
  element count (`Limits::max_chunks`) are bounded, and everything it allocates is bounded by the
  size of the input.
- Tests: `tests/aepx.rs` compares the five fixture pairs chunk by chunk and byte by byte, compares
  the typed view of both forms, and runs truncated, bit-flipped, deeply nested and oversized
  inputs; `crates/aep/tests/it/import/aepx.rs` imports both forms of each pair and expects the same
  project and report. An ignored test runs the comparison over the whole local corpus.

### Versions (`head`, `svap`) and files saved for 25.x and 24.x

Evidence: 12 families of the corpus (`layers-comps`, `footage-precomp`, `masks`, `shapes`, `text`,
`effects`, `3d`, `keyframes`, `markers`, `styles`, `settings-a`, `1200-lumetri`), each saved three
ways by After Effects 2026 (26.2.1x2) in one session: a normal save (`-26`), File ▸ Save a Copy As
25.x (`-25x`) and Save a Copy As 24.x (`-24x`), 36 files. All of them are copies made by 2026; no
file written by After Effects 24 or 25 itself was examined, so nothing below says what those versions
write on their own.

| Fact | Evidence |
|---|---|
| `svap` (4 bytes, the first top-level chunk) is the version of the After Effects that **wrote** the file: `0f 11 0e 02` in all 36 (a down-saved copy keeps the 2026 word) and in all 1 192 other `.aep` files of the local corpus folders. After Effects opens a file without `svap` (`t-nosvap`). | All 36; corpus scan. |
| `head` bytes 4 to 7 are the version the file is **written for**: `0f 11 0e 02` (= `svap`) in the 2026 saves, `0f 0b 06 61` in the 25.x copies, `0f 03 06 41` in the 24.x copies, the same word in every family. After Effects refuses a file whose word is zero ("created with an older version [0.0x0]", `u-zhead`). | All 36; `u-zhead`. |
| The word is packed (bit 31 first): bits 25-24 and 21-19 the major, 18-15 the minor, 14-11 the bug-fix, 10-8 a stage (6 in all), 7-0 the build; bits 31-26 (`000011`) and 23-22 (`00`) are the same in all samples and not decoded. `0f110e02` = **26.2.1x2**, which is what the saving After Effects reports for itself (`app.version` recorded by the corpus scripts) - this one sample fixes minor, bug-fix and build. The copies read as **25.6.0x97** and **24.6.0x65** (major **V** from three consecutive majors; minor, bug-fix and build of the copies **E**: one other word decides their positions). `effectcraft_aep::AeVersion`, `AepFile::file_version`, `AepFile::saved_by_version`. | Tests `decodes_the_sampled_words`, `the_container_of_a_copy_states_its_version_in_head_and_the_writer_in_svap`. |
| `head` bytes 0 to 1 (a `u16`) step with the major: 97 (`0x61`) in the 2026 saves, 96 in 25.x, 95 in 24.x; bytes 2 to 3 are 5, 9 and 6 (**?**). Bytes 8 to 15 differ between families, not between versions; bytes 16 to 19 are the save counter (one higher in each copy). Nothing is derived from bytes 0 to 3 (`AepFile::head_revision` exposes the `u16`). | All 36. |
| The XMP packet after the container is no guide: its `CreatorTool` is "Adobe After Effects 2026" in the copies too. | All 36. |
| **26 against 25.x** (per family, chunk tree aligned by item and layer id): besides `head`, `nhed` / `nnhd` differ in 6 bytes (`nhed` 20-23 and 26-27: `827fa930` / `0015` in the 2026 save, `00000000` / `011b` in both copies; session state, see section 4). Every **solid's** `Pin /CLRS` changes: the 2026 save has `dcui` (1 byte, 1), `apid` all `0xff` and an empty `Utf8`; the copies drop `dcui`, set `apid` to a GUID (`601e9973...`, the same in all) and embed a 935-byte JSON with an ICC profile (`baseColorProfile/colorProfileData`). A **footage** item that the 2026 save stores with an assigned profile (`mcsp` = 1 and a profile JSON) loses `mcsp` and that JSON in the copies, and its `epid` becomes all `0xff`. The rest is save noise: the footage alias `ascendcount_base` (the copies were saved in another folder), `sspc` byte 112 of a footage item (a modification time), the `oEXR` `opti` of an EXR item (the copies hold the display device name `\.\DISPLAY1` as UTF-16 at byte 126 and, in the 24.x copy, a few non-zero bytes from byte 2 102 that look like uninitialised memory; the 2026 save has zeros), the plug-ins' sequence data `sdat` of Lumetri Color (16 instances and the project's `EfdG`), the mask counter `mkif` byte 8. No chunk is added or reordered, and no `pard`, `cdat`, `ldta`, `cdta`, `tdb4`, keyframe, text or shape chunk differs. | All 12 families. |
| **25.x against 24.x**: only `head` (bytes 1, 3, 5 and 7), and the session noise in `opti` and `sdat` above. | All 12 families. |
| Lumetri Color in the copies has the parameter definitions (`pard`, valid ranges included) of the 2026 plug-in, byte for byte. | `1200-lumetri`. |

The importer reads none of the chunks that change (it reads the solid colour from `opti`, not
`CLRS`), so the copies import to the same project and report as the 2026 save
([aep-import.md](aep-import.md), "What is reported"; test
`copies_saved_as_25x_and_24x_import_like_the_2026_file`).

## 2. Typed view (`Project`, `Item`, `Comp`, `Layer`, `PropGroup`, `Property`, `KeyList`)

The structure the typed view is built on; each row has a test in `crates/aep/tests/it/container/typed.rs` against
the values the cases were made with. The field layouts that build on it (layer and composition
headers, values with their units, keyframes, effects, masks, shapes, text, markers, cameras,
lights, footage) are in section 3. Everything else is reachable as raw chunks.

| Fact | Evidence |
|---|---|
| Project items are `LIST Item` children of the top-level `LIST Fold`; a folder holds its children in a nested `LIST Sfdr`. | `000`: composition `C` and folder `Solids` at the top, the solid's footage item inside `Solids`. |
| The item kind is the first big-endian `u16` of `idta`: 1 folder, 4 composition, 7 footage. | `000` (`C`, `Solids`, the solid `L`); `001` has no composition. |
| The item name is the item's `Utf8` child (empty for a solid's footage item). | `000` (`C`, `Solids`), `036` (comp renamed: the `Utf8` grows from 1 to 12 bytes, `Renamed Comp`). |
| A composition item has a `cdta` whose `u32` at byte 4 is the frame duration and the `u32` at byte 8 the timescale: `fps = timescale / frame_duration` (25 fps: 1024 and 25 600; 29.97 fps: 800 and 23 976). Keyframe times are in these units. | `000`, `031` (30 fps: 1024, 30 720), `229`, `230`; `055` (key at 2.5 s = 64 000 units). |
| The layers of a composition are its `LIST Layr` children, top layer first. (The `DLay`, `SLay`, `CLay` and `SecL` lists next to them are not layers: their `ldta` names are `Default`, `Front`, `Left`, `Top`, `Back`, `Right`, `Bottom`, `Custom View 1` to `3` and `Markers`.) | `000` (one layer), `110` (text layer on top of `L`). |
| The layer name is the `Utf8` child of the layer; it is empty while the layer shows its source's name (new solids, precomp layers, default-named cameras and lights). | `010` (`L` renamed: 0 to 7 bytes, `Renamed`), `110` (`T`), `211` (`S100` is the solid's name, not the layer's). |
| The 3D layer switch is bit `0x04` of byte 38 of `ldta`. | `015` (only that byte changes from `00` to `04`; the 3D properties appear), `000`. |
| A layer holds its property tree in a `LIST tdgp`. Inside a group, a `tdmn` chunk (the match name: ASCII, NUL padded to 40 bytes) precedes the list it names: `tdgp` (a group), `tdbs` (a property) or another list (`otst`, `om-s`, the effect instance `sspc`). A `tdmn` named `ADBE Group End` closes the group and names nothing. | `000` (`ADBE Transform Group`), `071` (`ADBE Effect Parade`, `ADBE Slider Control`), `240`. |
| Properties are stored sparsely: a property at its default has no `tdbs` at all (the default depends on the layer and the composition). | `040`, `041`, `043`, `044`: each adds exactly one `tdmn` and one `tdbs` to `000`; `211`, `212` (new solids: Position absent, Anchor present only when changed); test `static_value_is_the_first_dims_doubles_of_cdat`. |
| A property (`tdbs`) holds `tdsb`, `tdsn`, the 124-byte header `tdb4`, then the static value `cdat` or a keyframe `LIST list`. `tdb4` begins `db 99 00`; byte 3 is the number of components (Position 3, Scale 3, Opacity 1, Slider 1, colour 4); byte 68 is 1 exactly when the property has a `list`. | `040`, `041`, `043`; `050` (the `cdat` is replaced by a `list`, byte 68 goes 0 to 1); `055`; test `property_header_facts_hold_in_every_property_of_every_fixture`. |
| The static value is the first `dims` big-endian doubles of `cdat`. Percent properties are stored as fractions: Scale 50 % is `0.5, 0.5, 1.0`, Opacity 25 % is `0.25`. | `040` (Position `100, 200, 0`), `041`, `043`, `071` (Slider `77`). |
| A `LIST list` is `lhd3` (52 bytes, 13 `u32`) and `ldat`. Word 0 is `0x00d00bee`, word 2 the record count, word 4 the record size, words 8 to 12 are zero; `len(ldat) = count * record size`; a list without records has no `ldat`. In the keyframe lists of properties, word 3 is `ceil(count / 4)` and word 7 the count rounded up to a multiple of 4 (not in the empty `Gide` lists, and not in path lists, where they are allocation capacities). | The first four relations hold for every `list` of every fixture (test `list_header_facts_hold_in_every_list_of_every_fixture`); word 3 and 7 on the keyframe lists of `050` (1 record) and `055` (3 records of 128 bytes, capacity 4). |
| A keyframe record starts with its time as a big-endian `i32` in the composition's time units (**layer time**). | `055` (0, 25 600, 64 000 for keys at 0 s, 1 s, 2.5 s), `058` (0 and 51 200 for keys at 0 s and 2 s). |

## 3. Verified field layouts

All integers are big-endian unless stated; offsets are byte offsets inside the chunk body; `f64`
is an IEEE-754 double, `f32` a float. Chunk paths start at the project item; `Layr` is a real layer
(`LIST Layr`), `tdgp[X]` is the `LIST tdgp` named `X` by the `tdmn` that precedes it. Each row lists
the cases that decide it. A row marked V is asserted by a test that compares the decoded value with
the value from the corpus manifest (test file in parentheses at the end of each table).

### 3.1 Layer header (`Layr/ldta`, 164 bytes; the decoder needs the first 140)

| Field | Offset | Type | Meaning | Evidence | Conf. |
|---|---|---|---|---|---|
| id | 0 | u32 | layer id, unique in the project; parent links use it | 000 (`0x1b`), 022 | V |
| quality | 5 | u8 | 2 = Best, 1 = Draft (other values are kept as `Other`; 0 on cameras and lights) | 000, 023 | V |
| **stretch** | 8 / 108 | i32 / u32 | **time stretch as the exact ratio `@8 / @108`**: 1/1 = 100 %, 2/1 = 200 %, **1/2 = 50 %, 3/2 = 150 %; a reversed layer has a negative numerator: -1/1 = -100 %, -1/2 = -50 % (981-983: start at the later end, in / out relative in source time, so the absolute in point is the later one; keys count source time in the composition's time scale)**. In and out points stay unscaled; the keyframe time scale `tdb4@12` of every property of the layer is 25 600 x stretch for stretch >= 1 (51 200 at 200 %, 38 400 at 150 %) and stays 25 600 at 50 % | 000, 018, 316, 317 | V |
| start | 12 / 16 | i32 / u32 | layer start time = num / den seconds; den = composition timescale (25 600), 1 in a layer made by the precomp operation | 000, 021 (12 800 / 25 600 = 0.5 s), 124 | V |
| in point | 20 / 24 | i32 / u32 | **relative to the start**: absolute = start + in * stretch (the exact ratio above). **Fractional times are exact multiples of 1/25 600 s: 0.3333 s became 8 532/25 600 = 0.33328125 s, and setting the in point moved the out point with it (duration kept)** | 000, 019 (25 600 / 25 600 = 1 s), 021 (stays 0 while the layer starts at 0.5 s), 318 | V |
| out point | 28 / 32 | i32 / u32 | same convention; 5 s = 128 000 / 25 600. **For footage layers the denominator is the footage time base, not 25 600: 48 000 / 48 000 for the 1 s audio layer (the sample rate), 3 / 30 for the three-frame image sequence (the frame rate); start and in point keep 25 600** | 000, 018 (stays 5 s, the layer ends at 10 s), 019 (6 s), 021 (5.5 s), 316, 317 (stays 5 s, ends at 2.5 s / 7.5 s), 318, 351, 353-354 | V |
| name is user-set | 37 bit `0x01` | flag | clear while the layer shows its source's name (000, 124, new solids, default-named camera and light), set when named | 000, 010, 022, 110, 113, 124, 211 | V |
| guide layer | 37 bit `0x02` | flag | | 026 | V |
| adjustment layer | 38 bit `0x02` | flag | | 025 | V |
| 3D layer | 38 bit `0x04` | flag | always set on cameras and lights | 015, 251 | V |
| solo | 38 bit `0x08` | flag | | 014 | V |
| auto-orient | 38 bit `0x40` | flag | orients towards the point of interest: set on two-node cameras (byte 38 is `0x44`) and on all lights, clear on a one-node camera (`0x04`) | 256 (`0x44`), 251 (`0x04`), 252, 254 | V |
| null object | 38 bit `0x80` | flag | set on a layer created as a null | 022, 220 | V |
| **video switch** | 39 bit `0x01` | flag | the eye icon (`enabled`); off gives byte 39 = `06` | 222 | V |
| **audio switch** | 39 bit `0x02` | flag | `audioEnabled` (also changes on a solid); off gives `05` | 223 | V |
| **effects switch** | 39 bit `0x04` | flag | `effectsActive`; off gives `03` | 224 | V |
| motion blur | 39 bit `0x08` | flag | | 017 | V |
| locked | 39 bit `0x20` | flag | | 013 | V |
| shy | 39 bit `0x40` | flag | | 012 | V |
| collapse transformations | 39 bit `0x80` | flag | continuously rasterize; set from creation on shape and text layers | 024, 238 | V |
| source id | 40 | u32 | `idta@16` of the source item (solid, footage or comp); 0 for shape, text and camera layers, `0xffff_ffff` for lights | 000, 022, 124, 126, 252 | V |
| label | 61 | u8 | label colour index (1 red, 5, 8 blue, 15, ...); a footage layer copies the label of its footage item (audio 7, image sequence 3) | 000 (1), 124 (15), 126 (5), 351 (7), 354 (3) | V |
| name copy | 64..95 | ASCII, NUL padded | copy of the user-set name; empty while the layer is unnamed | 000 (empty), 010 (`Renamed`) | V |
| **blend mode** | 99 | u8 | Normal 2, Add 4, Multiply 5, Screen 6, Overlay 7, Difference 26 (each case changes only this byte); set by name (906): Soft Light 8, Hard Light 9, Darken 10, Lighten 11, Exclusion 25, Color Dodge 27, Color Burn 28; by render (907: a script enum sweep wrote codes 2-34, each code rendered over a test image and matched one mode): Classic Difference 12, Hue 13, Saturation 14, Color 15, Luminosity 16, Stencil Alpha 17, Stencil Luma 18, Silhouette Alpha 19, Silhouette Luma 20, Linear Dodge 29, Linear Burn 30, Linear Light 31, Vivid Light 32, Pin Light 33, Hard Mix 34; 23 / 24 render as Color Dodge / Burn there (taken as the classic variants, E); 3 is written for Dissolve and Dancing Dissolve; 21 / 22 are Alpha Add / Luminescent Premul (not told apart). The reader keeps every code beyond the first six raw | 000, 016, 225-228, 906, 907 | V (6 + 22), E (23, 24) |
| layer type | 128 | u32 | 0 AV (solid, footage, precomp, null), 1 light, 2 camera, 3 text, 4 shape | AV: 000, 022, 126; text: 110, 113, 243; shape: 238-242; camera: 251, 253, 256; light: 252, 254 | V |
| parent id | 132 | u32 | 0 = no parent | 000, 022 (L gets the id of the new null layer) | V |
| **light type** | 136 | u32 | 1 = spot, 2 = point (0 on every other layer); parallel and ambient were not sampled and are kept raw | 252, 254 | V |
| **track matte mode** | 107 | u8 | 0 = none, 1 = alpha, 2 = alpha inverted, 3 = luma, 4 = luma inverted; bytes 104..106 are 0 in every file. Setting a matte changes only this byte and the matte layer id on the target layer, and turns the matte layer's video switch off (byte 39 `07` -> `06`) | 402-408 | V |
| **track matte layer** | 160 | u32 | the id (byte 0) of the matte layer, anywhere in the composition: `0x1d` (`M`, above) in 403-406 and 408, `0x1b` (`L`, below its target) in 407; 0 without a matte. A mode with id 0 has no case (After Effects 2026 writes the id; before 23.0 the matte was always the layer directly above) | 403-408 | V |

API: `LayerHeader` (`is_video_enabled`/`is_enabled`, `is_audio_enabled`, `is_effects_enabled`,
`is_auto_oriented`, `light_type`, `blend_mode`, `stretch` / `stretch_den`, `stretch_ratio`,
`stretch_percent`, `in_point`, `out_point`); the same switches on `Layer`; `Layer::track_matte`
(`TrackMatte`, `MatteMode`; test `tests/matte.rs`). `stretch` is now the
numerator of the ratio (it was the whole multiplier before 50 % and 150 % were sampled). Tests:
`tests/header.rs`, `tests/layer_switches.rs`. The layer name itself is the `Utf8` child of the
`Layr` (empty while the name is the source's); the layer then shows the name of its source item
(`Project::layer_display_name`).

### 3.2 Composition header (`Item/cdta`, 204 bytes; the decoder needs the first 176) and renderer

| Field | Offset | Type | Meaning | Evidence | Conf. |
|---|---|---|---|---|---|
| frame duration | 4 | u32 | time units per frame: 1024 at 25 and 30 fps, **800 at 29.97 fps, 1000 at 23.976 fps** | 000, 031, 229, 230 | V |
| timescale | 8 | u32 | time units per second: 25 600 at 25 fps, 30 720 at 30 fps, **23 976 at 29.97 and 23.976 fps**; fps = timescale / frame duration exactly as typed (29.97 is `23976 / 800`, not `30000 / 1001`) | 000, 031, 229, 230 | V |
| work area start | 28 | u32 | in time units | 035 (25 600 = 1 s) | V |
| work area end | 36 | u32 | start + duration in time units; `0xffff_ffff` = the whole composition | 000, 035 (76 800 = 3 s) | V |
| duration | 44 | u32 | in time units. **A duration is snapped to whole frames: 5 s at 29.97 fps is 120 000 units = 150 frames = 5.005005 s, at 23.976 fps 120 frames** | 000, 031, 032 (256 000), 229, 230 | V |
| background | 52..54 | u8 R, G, B | **8 bits per channel**: 0.5 is stored as 128 and reads back as 128/255 = 0.50196 | 000, 033, 231 | V |
| motion blur | 139 bit `0x08` | flag | the composition's switch | 038 | V |
| width, height | 140, 142 | u16, u16 | | 000, 030 (1280 x 720) | V |
| pixel aspect | 144 / 148 | u32 / u32 | numerator / denominator; **0.9091 is stored as the exact ratio 10 / 11 (read-back 0.90909090909091)** | 000 (1 / 1), 037 (2 / 1), 325 (10 / 11) | V |
| frame rate | 156 | u32 | 16.16 fixed point of the float32 rate (25 = `0x0019_0000`, 29.97 = 1 964 114 = 29.9700012, 23.976 = 1 571 291) | 000, 031, 229, 230 | V |
| shutter angle | 172 | u32 | degrees | 000 (180), 034 (90) | V |
| **shutter phase** | 180 | i32 | degrees: 0 by default, -90 = `ffffffa6`; the shutter angle at 172 does not change | 000, 326 | V |
| **display start** | 164 / 168 | u32 / u32 | the comp start timecode as `num / den` seconds: 0 by default; display start frame 100 at 25 fps is 102 400 / 25 600 = 4 s (the denominator is the timescale; at other frame rates it was not sampled) | 000, 327 | V (one sample) |
| renderer | `LIST PRin` / `prin` 4 | 32 ASCII | match name: `ADBE Escher` in the base, `ADBE Calder` in 039 | 000, 039 | V |
| renderer name | `prin` 52 | 32 ASCII | display name: `Classic 3D`, `Advanced 3D` | 000, 039 | V |
| composition name | `Item/Utf8` | text | | 000 (`C`), 124 (`Pre`) | V |

The inert `ADBE Time Remapping` record of every AV layer has `tduM` = the source's duration in seconds: 5 for 5 s compositions (124, 257, 258), 3 and 2 for a 3 s and a 2 s composition (600, 601), 1 for the 1 s WAV (351), 0.1 for the three-frame sequence (353), 0 on solids and stills (000, 126). Every property's keyframe time scale (`tdb4` bytes 12 to 15) follows the composition (23 976 in every
property of `L` after 229 and 230). The corpus manifest calls the classic renderer
`ADBE Advanced 3d`; the file stores `ADBE Escher` / `Classic 3D`. API: `CompHeader::frame_rate_exact`
(exact fraction), `duration_frames`, `shutter_phase`, `display_start`, `display_start_seconds`,
`display_start_frame`. Tests: `tests/header.rs`, `tests/layer_switches.rs`.

### 3.3 Items, footage and precomps

| Field | Chunk path | Offset | Type | Meaning | Evidence | Conf. |
|---|---|---|---|---|---|---|
| item kind | `Item/idta` | 0 | u16 | 1 folder, 4 composition, 7 footage | 000 | V |
| item id | `Item/idta` | 16 | u32 | what a layer's source id refers to (`iide` holds the same number little-endian) | 000 (`0x1a` solid, `0x0d` comp), 022, 124 | V |
| item label | `Item/idta` | 58 | u8 | label colour of the item | 000 (1 solid), 124 (15 for `Pre`) | V |
| used comp | `Item/idpi` | 0 | u32 **little-endian** | a composition that uses another as a precomp layer gets one `idpi` per used composition | 124 (`C` uses `Pre`) | V |
| used comp count | `Item/idpc` | 0 | u32 little-endian | the number of `idpi` chunks: one per distinct composition shown (the same one twice counts once); the order of the `idpi` carries no meaning (an After Effects re-save reverses it) | 124, 257, 258, 600, 602 (1), 601 (2) | V |
| footage type tag | `Pin /sspc` | 22 | 4 ASCII | `Soli` for a solid, `png!` for a png | 000, 126 | V |
| footage size | `Pin /sspc` | 30, 34 | u32, u32 | width, height in pixels | 000 (1920 x 1080), 022 (100 x 100 null), 126 (8 x 8), 211, 212, 259 | V |
| footage name | `Pin /opti` | 26 (solid), 58 (file) | ASCII, NUL terminated | the item's own `Utf8` name is empty for footage | 000 (`L`), 022 (`Null 1`), 126 (`test8x8.png`), 211 (`S100`), 259 (`Blue`) | V |
| **solid colour** | `Pin /opti` | 10, 14, 18, 22 | 4 x f32 | ARGB, 0..1: red [1, 0, 0] is `1, 1, 0, 0`, blue [0, 0, 1] is `1, 0, 0, 1` | 000, 259 | V |
| file path | `Pin /Als2/alas` | key `fullpath` | JSON string | alias JSON of a file footage item | 126 (the fixture holds a neutral path) | V |
| precomp linkage | `Layr/ldta` | 40 | u32 | the layer's source id is the id of the precomp's item | 124 | V |

Footage items beyond solids and stills (`Pin /sspc`, 222 bytes; audio and image sequences, batch 3):

| Field | `sspc` offset | Type | Meaning | Evidence | Conf. |
|---|---|---|---|---|---|
| file type tag | 22 | 4 ASCII | `png!`, `Soli`, **`WAVE`**; `TPIC` Targa (650), `8BPS` Photoshop (651), `TEXT` for both an Illustrator / PDF file (652) and an EPS (653); all four stills store the duration 0 / 600 | 126, 000, 350, 650-653 | V |
| width, height | 30, 34 | u32, u32 | 0 / 0 for audio; 64 x 64 for the sequence | 350, 352 | V |
| **duration** | 38 / 42 | u32 / u32 | seconds as `num / den`, unreduced: still 0/1, **WAV 48 000/48 000 (the sample rate is the denominator), image sequence 3/30 (the frame rate is the denominator)** | 126, 350, 352 | V |
| native frame rate | 56 | u32 | 30 for the sequence, 0 for stills, audio and solids | 126, 352 | V |
| frame rate (again) | 150 | u32 (unaligned) | the same rate as @56 for sequences and clips: 30 (353, 603), 25 (606, 612); 0 for stills and audio. Conforming a sequence to 25 fps (615 against its reference 614) changes @42 (the duration denominator), @56 and @150 from 30 to 25 together; for clips see @146 | 353, 603, 606, 612, 614, 615 | V |
| conformed rate of a clip | 146 | u32 (unaligned) | the rate a clip is conformed to (Interpret Footage), 0 when not conformed; the effective rate is then also at @150, while @38 / @42 (the duration in native frames over the native rate) and @56 stay native: the 25 fps 25-frame mp4 conformed to 30 stores 25 / 25, @56 25, @146 30, @150 30 (645 against 644), the layer's Time Remapping maximum becomes 25 / 30 s. A conformed image sequence keeps @146 at 0 and stores the new rate at @42, @56 and @150 (615) | 615, 644, 645 | V |
| bits per pixel | 60 | u32 | RGBA still 32, RGB sequence 24, 0 for solids and audio | 126, 352 | V |
| **sample rate** | 160 | f64 | 48 000.0 for the WAV; the same for an mp4 with sound (48 000.0 and 44 100.0 read back from AAC tracks of those rates), absent for an mp4 without (the importer takes a clip's audio from it) | 350, read-back vid-audio | V |
| **file size** | 208 | u32 | bytes of the source file(s): png 149, WAV 96 044 (44 + 48 000 x 2), three png of 168 bytes 504 | 126, 350, 352 | V |
| **sequence numbering** | 172, 176, 180 | u32 x 3 | first and last frame number and the digits of a sequence: 1, 3, 3 for `seq_001.png`..`seq_003.png` (353), 101, 103, 4 for `shot_0101.exr`..`shot_0103.exr` (603); 0 for single files (604, 605, 607). The name around the number is the pair of `Utf8` chunks between `Als2` and `opti` in `Pin ` (`seq_` / `.png`, `shot_` / `.exr`), absent on single files. **An empty prefix is not written**: `0001.png`..`0005.png` leave one `Utf8` (`.png`) there, so a single `Utf8` is the prefix when the first file name in `opti` (byte 58: `0001.png`) starts with it and the suffix when it ends with it; without that evidence the reader takes one starting with `.` as the suffix and reads nothing else. `Item::sequence_info`, `SequenceInfo::file_name` | 353, 603, 604, 605, 607; the prefix-less sequence of issue 3174 (After Effects 25.2) | V |
| **sequence file list** (Force alphabetical order) | `Pin /LIST StVc` | `StVS` (4 bytes, `u32` LE), then `Utf8` x count | a sequence imported with Force alphabetical order has, between `Als2` and `opti`, a `LIST StVc` instead of the prefix and suffix `Utf8`s: `StVS` holds the file count as a **little-endian** `u32` (`05 00 00 00`), followed by one `Utf8` per file name in order (`shot_0001.png`..`shot_0005.png`). The numbering at @172 / @176 is then `0xFFFFFFFF` and the digits @180 0; duration and rate are stored as for a numbered sequence (5/30, 30). The list replaces the numbering; a list without a readable `StVS` count, or with a name that is not a plain file name (`/`, `\`, `:`, a control character such as NUL, `.`, `..`), is not read. Without a stored item name the importer names the item after its first listed file (an assumption: what After Effects shows was not sampled). `SequenceInfo::files`, `SequenceInfo::nth_file_name` | the alphabetical sequence of issue 3174 (After Effects 25.2) | V |
| **sequence folder time** | 118 | u32 (unaligned) | seconds since 1904-01-01 UTC: the modification time of a sequence's folder when After Effects scanned it (860: 0xe6fb443a = 2026-10-19 03:51:22, the folder's and its files' time). With the folder's time After Effects keeps the stored numbering on open; with another (an older time, the first or the last file's time while the folder's differs: x1, x2 against x3) it scans the folder again and warns about a gap in a modal dialog | 860, `export-tests\sparse860\exp` x1-x3 | V |
| still duration denominator | 42 | u32 | 1 for png (126, 607), 600 for jpeg (605), 100 for OpenEXR (604): a still's duration is 0 over a per-format denominator | 126, 604, 605, 607 | V |

An **image sequence is ONE item** (`opti` holds the first file name `seq_001.png` at 58, 322 bytes like
a still, with the width and height at 20 and 24 and the PNG colour type at 36); its alias JSON points
at the sequence **folder** (`"target_is_folder":true`, `ascendcount_target` 2). The audio item has its
name in the item's `Utf8` (`test-tone.wav`), label 7 (sequence 3, still 5); its `opti` has no display
name. A footage layer shows the item's name while it has no name of its own (351, 354).

Test: `tests/header.rs` (precomp), `tests/text_markers_footage.rs` and `tests/batch3_objects.rs`
(footage), `tests/layer_switches.rs` (solid colour). API: `Footage::{duration, native_frame_rate,
bits_per_pixel, file_size, audio_sample_rate, is_sequence, sequence_frame_count}`.

### 3.4 Properties: sparse storage, static values and units

A layer's property tree contains a `tdbs` only for a property whose value differs from the default
that After Effects computes for the layer and composition. A reader supplies the defaults:

| Property (match name) | Stored | Unit in the API | Default when absent | Evidence | Conf. |
|---|---|---|---|---|---|
| `ADBE Position` | f64 x, y, z | pixels | the composition centre `[width / 2, height / 2, 0]` | 000, 040 (`100, 200, 0`), 030 (the comp becomes 1280 x 720, L stays 1920 x 1080: `960, 540, 0` is now stored), 211, 212 (absent on new solids) | V |
| `ADBE Anchor Point` | f64 x, y as a **fraction of the layer's source size**, z in pixels | px = fraction * source size | `0.5, 0.5, 0` (the layer centre) | 044 (`10, 20` px on 1920 x 1080 is `10/1920, 20/1080`), **211 (100 x 100 solid: `10, 20` is stored `0.1, 0.2`), 212 (400 x 300 solid: `100, 50` is stored `0.25, 1/6`)**: the composition size is not the divisor | V |
| `ADBE Anchor Point` of a **text** layer | f64 x, y, z (a text layer has no source) | pixels, as stored | `0, 0, 0` (the text origin) | absent in every other text sample (110-114, 243-248, 334-340, 386); **385 (text anchor set to `10, 20`) stores `10, 20, 0` as f64 pixels**, not a fraction of a size; test `a_stored_text_anchor_is_pixels` | V |
| `ADBE Scale` | f64 fractions | percent (fraction * 100) | 100 % | 000, 041 (`0.5, 0.5, 1.0`) | V |
| `ADBE Rotate Z` | f64 degrees | degrees | 0 | 000, 042 | V |
| `ADBE Opacity` | f64 fraction | percent (fraction * 100) | 100 % | 000, 043 (`0.25`) | V |
| mask feather | f64 x, y | pixels | `[0, 0]` | 090, 091 (`20, 20`), 208 | V |
| mask opacity | f64 fraction | percent | 100 % | 090, 091 (`0.5`) | V |
| mask expansion `ADBE Mask Offset` | f64 | pixels | 0 | 090, 091 (5) | V |
| effect slider | f64 | as set | the `pard` default | 071 (77) | V |
| effect colour | 4 x f64, ARGB 0..255 | ARGB fractions | the `pard` default | 073 (`[0.2, 0.6, 1]` is `255, 51, 153, 255`) | V |
| shape stroke width, round corners radius, light intensity | f64 | as set (pixels, pixels, percent) | not decoded as defaults except the light (100) | 239 (8), 238 (20), 254 (50) | V |
| shape group position, gradient start and end | f64 x, y | pixels | group position `[0, 0]` | 242, 241 | V |
| camera Position, point of interest | f64 x, y, z | pixels | see 3.12 | 251, 253, 256 | V |
| camera Zoom, light colour, cone angle, cone feather | f64 / ARGB 0..255 | pixels, `[A, R, G, B]` fractions, degrees, percent | Zoom 2 666.67 (1920 px comp), colour white, cone angle 90, cone feather 50 | 341, 343-345 | V |

`Project::layer_anchor_px` multiplies the anchor fraction by the size of the layer's source (a
solid or footage item, or the composition of a precomp). `Transform::text_anchor_px` gives the
anchor of a text layer (pixels as stored, `[0, 0, 0]` when absent).

**Explicit defaults (batch 3).** A value record exists exactly when the value differs from the
default; setting a value to its default never creates a record and setting it back removes it. A
Gaussian Blur read back as Blurriness **25** when added (the corpus manifest's "0" for rows 214 and
216 was wrong, with a correction note in the manifest); Blurriness set to 0 writes a record, 25 -> 40 -> 25
leaves the effect subtree byte-identical to the never-touched default (300, 301, and 214 / 078).
The same holds for Fill Opacity 1 -> 1 and 1 -> 0.5 -> 1 (302, 303) and Slider 0 -> 0 (304), and
for the five transform properties set to the value they already had (Position, Opacity, Anchor,
Scale, Rotation, 311-315: the layer tree is byte-identical to the unedited save 299). A writer may
therefore rely on "absent = default" and drop a record that equals the default. Exceptions are
records AE materialises together with a type switch (polygon, radial gradient: 3.9).
`EffectParam::is_default` returns this; `Tracked::source` tells `Default` from `Stored`.

**Time Remapping (batch 3).** Every layer has an inert `ADBE Time Remapping` property at the root of
its tree (`tdsb` 3, static value 0, no keys). Enabling the remap turns it into a keyed property
(`tdsb` 1, `tdb4` byte 5 bit 0 cleared, byte 68 = 1, no `cdat`) with two plain 48-byte keys whose value
(seconds) is at byte 8: 0 s -> 0 and 5 s -> 5 on a precomp layer (319), 0 s -> 0 and 0.1 s -> 0.1 on an
image-sequence layer (354). It cannot be enabled on a solid. API: `Layer::time_remap` (`TimeRemap`,
`TimeRemapKey`).

Property record (`LIST tdbs`: `tdsb`, `tdsn`, `tdb4`, then `cdat` or a `LIST list`, optionally a
`Utf8` expression, `tdum`, `tduM`):

| Field | Chunk path | Offset | Type | Meaning | Evidence | Conf. |
|---|---|---|---|---|---|---|
| magic | `tdb4` | 0..2 | `db 99 00` | every property | all fixtures | V |
| components | `tdb4` | 3 | u8 | values in `cdat` / in each key (1, 2, 3, 4 = ARGB) | 040, 041, 043, 073, 091, 206, 208, 210 | V |
| static flag | `tdb4` | 5 bit 0 | flag | set while the property has no keys (Position: 15 static; the bit is clear on an animated property) | 040, 055, 058 | V |
| **template flag** | `tdb4` | 7 | u8 | 0 = plain keyframe template, non-zero = spatial template (3 on Position, 1 on colours and paths) | 204-206, 208, 210 | V |
| time scale | `tdb4` | 12 | u32 | units per second of the keyframe times (25 600; 30 720 at 30 fps; 23 976 at 29.97 and 23.976 fps) | 055, 058, 122, 229, 230 | V |
| has keys | `tdb4` | 68 | u8 | 1 exactly when the property has a `LIST list` | every fixture (test) | V |
| keyed spatial-template flag | `tdb4` | 79 | u8 | 1 on a static colour or point effect value, 0 once it is keyed (359 / 364, 369 / 382); 0 on keyed and static sliders (362 / 363, 391 / 397); meaning **?** | 359, 362-364, 369, 382, 391, 397 | V (values) |
| expression flags | `tdb4` | 76, 77, 120 | u8 | all three are 1 when an expression exists. 76 and 77 are also 1 on a camera's or light's computed (auto) Position, which has no expression | 060, 213, 251 | V |
| **expression disabled** | `tdb4` | 119 | u8 | 1 = the expression is disabled; the text and bytes 76, 77, 120 stay. On the built-in view layers the byte flips by itself after the first save, so it counts only together with an expression | 060 (0), 213 (1) | V |
| state | `tdsb` | 0 | u32 | 1 = written (non-default value, keys or expression), 3 = default state. **A layer, camera, light, shape or text property in the default state holds After Effects' default** or a value with no effect on that layer (survey of 839 corpus and sample files: Material Options, cone 90 / 50 and falloff 500 / 500 of point lights, Position `[0, 0, -1333.33]` of ambient and environment lights, Opacity 1 and Scale `[1, 1, 1]` of cameras and lights, a polygon's Inner Radius 50, Miter Limit 4, the inert Time Remap 0), so the importer skips it. **An effect parameter's record in the default state holds the effective value** (3.7, stored value) | 040, 071, 073 (1); 3 on the unwritten properties of 000; 1200, 1400 (effects) | V (1), E (3), V (effects) |
| static value | `cdat` | 0 | `components` x f64 | the first `components` doubles are the value | 040, 041, 043, 071 | V |
| user name | `tdsn/Utf8` | | text | `-_0_/-` means "no custom name" | 091 (`Mask 1`), 238 (`Group 1`) | V |

Test: `tests/values.rs`, `tests/typed.rs`, `tests/keyframes.rs`.

### 3.5 Keyframes (`tdbs/list`: `lhd3` + `ldat`)

`lhd3` (13 `u32`): word 0 `0x00d0_0bee`, word 2 the record count, word 3 `ceil(count / 4)`, word 4
the record size, word 7 the count rounded up to a multiple of 4, words 8 to 12 zero; `ldat` is
exactly `count * record size` bytes (cases 050, 055; every list of every fixture). In path lists
(record size 8) words 3 and 7 are allocation capacities in vertices and four times that (12 records:
4 and 16; 9 records: 4 and 16 in the shape paths of 105 and 240, 3 and 12 in the open mask path 233),
not a function of the count.

**Record header (8 bytes, every kind).**

| Field | Offset | Type | Meaning | Evidence | Conf. |
|---|---|---|---|---|---|
| time | 0 | i32 | in the property's time scale; **layer time** (roving moved key 2 from 25 600 to 26 509 = 1.0355078125 s) | 055 (0, 25 600, 64 000), 056, 058 | V |
| **interpolation in** | 4 | u8 | the incoming side: 1 = linear, 2 = bezier, 3 = hold | 202 (in linear, out hold: `01 03`), 203 (in hold, out linear: `03 01`), 204, 205, 052, 053 | V |
| **interpolation out** | 5 | u8 | the outgoing side, same codes | same | V |
| label | 6..7, bits 6..9 | u16 | keyframe label: label 9 turns the flags `0x0007` into `0x0247` | 057 | V |
| roving | 6..7, bit `0x0020` | u16 | `0x0007` becomes `0x0027` | 056 | V |

A hold key has no further field (the value is unchanged); a linear key has all ease slots 0. Keys
added through the scripting interface are linear in and out.

**The template follows the property header (`tdb4` bytes 3 and 7), not the record size.** With
`n` components:

| Template | `tdb4[7]` | Size | Used by | Evidence |
|---|---|---|---|---|
| plain | 0 | `8 + 40 n`: 48 (n=1), 88 (n=2), 128 (n=3) | Opacity, Rotation, Slider (1); Mask Feather, Rectangle Size (2); Scale (3) | 205, 206, 208, 221 |
| spatial | non-zero | `56 + 24 n`: 128 (n=3), 152 (n=4) | Position (3), colours (4) | 204, 210 |
| mask path | | 64 | the path property of an animated mask (the path itself is in `shap`) | 093 |
| marker | | 16 | `mrst` marker times | 122, 123, 249 |

A 128-byte record is a Scale key (plain, 3 components) or a Position key (spatial): a reader must
look at the header (206 against 204). Layout of the plain template: value (`n` x `f64`) at 8, then
`4 n` ease doubles in **type-major order**: `[in speed x n][in influence x n][out speed x n][out
influence x n]`, starting at `8 + 8 n` (for `n = 1`: **at 16 in speed, at 24 in influence, at 32
out speed, at 40 out influence**). Scale key 1 of 305 (`n = 3`, in 0 / 33 %, out 20 / 66 %) is
`0,0,0, .33,.33,.33, .2,.2,.2, .66,.66,.66`; Mask Feather key 1 of 306 (`n = 2`, in 0 / 33, out
15 / 66) is `0,0, .33,.33, 15,15, .66,.66`. Component `i` uses slot `i` of each block (**V** for the
block layout and the units, Scale speed in fractions per second and Mask Feather speed in pixels
per second; **E** that different values per dimension sit in the slot of their dimension, because
the tool that made the corpus applies one ease to all dimensions; **V** since the exporter wrote three different eases for the three Scale dimensions and After Effects read the same three back, aep-export.md R13). `Keyframe::plain_ease_slots`
returns the doubles as stored, `Keyframe::ease_component(i)` one component's `KeyEase`. Layout of
the spatial template: `u32` at 8 (1 normally, 0 after the spatial auto-bezier and continuous
switches were turned off together, 054), `f64` arc length of the segment to the next key at 16
(100 in 204 key 1, 0 on the last key and right after creation; safe to ignore), **ease at 24 in
speed, at 32 in influence, at 40 out speed, at 48 out influence**, value (`n` x `f64`) at 56, spatial
in tangent (`n` x `f64`, relative offset from the value) at `56 + 8 n`, out tangent at `56 + 16 n`.

| Field | Where | Meaning | Evidence | Conf. |
|---|---|---|---|---|
| ease, Position | spatial @24..55 | 204 key 1: in speed 5 / influence 0.2, out speed 50 / influence 0.8; key 2: in 7 / 0.6, out 9 / 0.4. Speed in pixels per second, influence a fraction. **After Effects reports key 1's in speed and the last key's out speed as 0, but the file keeps the requested values (5 and 9): ignore those two slots** | 204, 052 (preset: speed 0, influence 0.33) | V |
| ease, Opacity | plain n=1 @16..47 | 205 key 1: in 0.05 / 0.2, out 0.2 / 0.8; key 2: in 0.3 / 0.6, out 0.09 / 0.4. Speed is in stored units per second, i.e. 100 times smaller than the percent per second shown in the UI; influence a fraction | 205 | V |
| value, Scale | plain n=3 @8 | 206: `[1, 1, 1]` and `[2, 1.5, 1]` (fractions of the manifest percent) | 206 | V |
| value, Rotation, Slider | plain n=1 @8 | degrees / as set (keyed slider 10 and 90) | 221 | V (slider), E (rotation, 207 is not a fixture) |
| value, Mask Feather | plain n=2 @8 | pixels `[0, 0]` and `[40, 20]` | 208 | V |
| value, colour | spatial n=4 @56 | ARGB doubles 0..255: `[255, 255, 0, 0]` and `[255, 0, 127.5, 255]`; flags `0x0001`, tangents zero; the project-level `EfdG` copy of the effect has the same list | 210 | V |
| value, Position | spatial n=3 @56 | pixels | 055 (`[0,0,0]`, `[100,0,0]`, `[200,100,0]`), 056 | V |
| tangents | spatial @`56+8n`, `56+16n` | 054: key 1 out `[30, 60, 0]`, key 2 in `[-30, -60, 0]` | 054 | V |
| arc length | spatial @16 | 100 on key 1, 0 on key 2 | 204 | E |
| **colour distance** | spatial n=4 @16 | for bezier colour keys the Euclidean ARGB distance to the next key, 255 sqrt(2) = 360.6245 for red to blue, 0 on the last key; linear colour keys have 0 (like the Position arc length, filled only for non-linear interpolation). The ease slots of a colour key are one entry (speed 0, influence 0) | 307 against 210 | V |
| **keyed Orientation** | `otst`: `tdbs` key list of 80-byte records (time, interpolation sides, flags at 0..7; `f64` 2.0 at 16 on the first key; the rest 0 for linear keys) and `otky` with one `otda` (three `f64` degrees) per key, in the same order. **After Effects interpolates by a quaternion slerp of the rotation `Rx Ry Rz`**: 703 (keys `[0,0,0]` at 0 s, `[10,20,30]` at 2 s, linear) reads back 0.5 s `[3.434, 4.469, 7.713]`, 1 s `[6.264, 9.353, 15.219]`, 1.5 s `[8.465, 14.560, 22.610]`, which slerp with that order reproduces to 0.001 degree (not per-component: 5 / 10 / 15 at 1 s; the order `Rz Ry Rx` gives `[3.58, 10.64, 14.32]`) | 703 | V (layout, linear), H (ease of bezier keys) |
| **light type** | `ldta` `u32` at 136: 0 parallel (701), 1 spot, 2 point, 3 ambient (702). A light created as an environment light by script without the Advanced 3D renderer is stored as ambient (704) | 701, 702, 704 | V |
| **ease, Anchor Point and effect points** | spatial @24..55 | the value is a fraction of the layer size, and **a speed is pixels per second divided by the layer height**: out speed 50 is 50 / 1080 = 0.046296 on the 1920 x 1080 L (597 Anchor Point, 598 Gradient Ramp Start), 50 / 800 = 0.0625 on a 400 x 800 solid (730; not the width, not the smaller side, not the composition height); influence a fraction. After Effects reads 597 back as 0.5 s [1004.41, 562.21], 1 s [1084.84, 602.42], 1.5 s [1137.13, 628.57] | 597, 598, 730 | V |
| **ease, colour switched to bezier** | spatial n=4 @24..55 | keyframe.set_interpolation bezier on two colour keys stores speed 0 and influence 0 on both sides, and After Effects interpolates them linearly (598 Start Color red to blue: 0.75 / 0.5 / 0.25 red at 0.5 / 1 / 1.5 s); a custom colour ease cannot be set by script | 598 | V |
| **ease, mask path** | 64-byte record @24..55 | in speed, in influence, out speed, out influence as in the spatial template: 599 out / in influence 0.75, 700 0.2 / 0.5, speeds 0 (the path at 0.5 s is 5.42 % / 28.0 % of the way). The `f64` at 16 is 1 (093) or 2 (599, 700) on the first key and 0 on the last (not decoded) | 093, 599, 700 | V (influence), ? (speed unit) |

API: `Keyframe::{interpolation_in, interpolation_out, kind, value, ease, ease_component, ease_components, plain_ease_slots, color_distance, spatial}`;
`Property::keys()` hands the property's template to its records. Marker record (16 bytes): time,
`03 03` hold bytes, 8 stale bytes. Mask path record (64 bytes): the common fields and the ease at byte 24 (599, 700). Test:
`tests/values.rs`, `tests/keyframes.rs`.

### 3.6 Expressions

| Field | Chunk path | Meaning | Evidence | Conf. |
|---|---|---|---|---|
| source | `tdbs/Utf8`, after `cdat` / `list` and before `tdum` | raw UTF-8 text, no length prefix, no NUL, no engine tag | 060 (`wiggle(1,10)` on Opacity) | V |
| flags | `tdb4` 76, 77, 120 | all 1 when present, all 0 otherwise | 060, 040 | V |
| **disabled** | `tdb4` 119 | 1 when disabled (`expression.disable_all`); the only byte that differs between 060 and 213 | 060, 213 | V |
| static value | `cdat` | kept while an expression exists (Opacity 1.0) | 060 | V |
| **on an effect parameter** | the parameter's own value record, as on any property: Slider Control's Slider with `time*10` holds the text and the static value 0 | 534 | V |

The expression engine is project-wide (`LIST ExEn/Utf8`, `javascript-1.0`). API:
`Property::expression_enabled`. Test: `tests/keyframes.rs`.

### 3.7 Effects (`tdgp[ADBE Effect Parade]/sspc`)

| Field | Chunk path | Offset | Type | Meaning | Evidence | Conf. |
|---|---|---|---|---|---|---|
| match name | the `tdmn` before the `sspc` | | text | `ADBE Slider Control`, `Pseudo/@@S1iPXtJFTHK4QaDbgJvLhw`, ... | 071, 073, 077, 080, 214 | V |
| instance name | `sspc/fnam/Utf8` | | text | the effect's own name (`Slider Control`, `Tint`); it stays the same when the instance is renamed (610) | 071, 073, 077, 080, 610 | V |
| **user name of an instance** | `sspc/tdgp/tdsn/Utf8` (the value group's name) | | text | `-_0_/-` (the placeholder every unnamed group has) while the instance shows the effect's name; the given name otherwise (610: `My Tint`, `fnam` still `Tint`). `Effect::user_name`, `Effect::display_name` | 356, 609, 610 | V |
| **fx switch** | `sspc/tdgp/tdsb` (the value group's flags, `u32`) | | flag | 1 on, 0 switched off in the Effect Controls (`fx-switch.aep`: two Gaussian Blurs on one layer in one save, the first switched off, the second on; read back `enabled` false / true). `Effect::enabled` | fx-switch | V |
| parameter count | `sspc/parT/parn` | 0 | u32 | including the implicit input layer and the built-in entry (3 for a slider) | 071 | V |
| parameter match name | `parT/tdmn` | | text | `<effect match name>-NNNN`; the last is `ADBE Effect Built In Params` | 071 | V |
| parameter type | `parT/pard` | 12 | u32 | 0 layer, 2 fixed slider (16.16), 3 angle, 4 checkbox, 5 colour, 6 point, 7 popup, 9 no data, 10 float slider, 12 path, **13 group start / 14 group end** (Exposure's Master, Red, Green and Blue twirl-downs; the end repeats the group's name; no value), **15 button** (Swap Colors of Tint and Gradient Ramp: empty name, label in `pdnm`) | 071, 073, 077, 080, 214, 215, 356, 368, 389 | V |
| UI flags | `pard` | 4 | u32 | 0x20 greyed out, 0x08 on a group end. **Stale:** After Effects does not reset it on undo (391-397 keep the flags of 390 although Channels is back to Master), so it is UI state and never read for meaning | 389-397 | V (values), E (meaning) |
| parameter name | `pard` | 16 | 32 ASCII | `Slider`, `Color`, `Menu`, `Blurriness`, `Shadow Color`, ... (empty for some checkboxes) | same | V |
| **default** | `pard` | 56 | by type | float slider: f64 (Slider 0, **Blurriness 25**); fixed slider and angle: 16.16 (drop shadow opacity 127.5, direction 135, distance 5, softness 0; Fill opacity 1.0 = 100 %; **Tint Amount to Tint 100, Ramp Scatter 0, Blend With Original 0**); **colour: ARGB bytes (drop shadow `ff000000` black, Fill `ffff0000` red; Ramp Start `ff000000`, End `ffffffff`; Tint Map Black To `00000000` and Map White To `00ffffff`: alpha 0, which is what After Effects reads back)**; popup: u32 index (1); **point: two 16.16 fractions of the layer size at 56 and 60 (Point Control 0.5, 0.5; Ramp Start 0.5, 0 and End 0.5, 1)** | 071, 077, 080, 214, 215, 356, 368, 383 | V |
| **valid maximum** | `pard` | 128 / 108 | 16.16 / f32 | the largest valid value of a fixed slider (128: Tint Amount 100, Ramp Scatter 512, Blend With Original **1.0**) and of a float slider (108: Blurriness 30 000; **Exposure 100, Offset 2, Gamma Correction 10**). The stored scale differs per slider (below), so this is the reader's only hint at it | 356, 368, 078, 389 | V |
| **float slider valid minimum, slider range** | `pard` | 104; 112 / 116 | f32; f32 x 2 | the smallest valid value (Exposure -100, Offset -2, Gamma Correction 0.1 as f32) and the range the slider covers (Exposure -4..4, Offset -0.5..0.5, Gamma 0.1..10); the slider range is repeated as `tdum` / `tduM` (f64) in each value record (391-394). @120 holds the default again as f32 (Gamma 1.0, Blurriness 25), u16 @124 the display decimals (**H**: Exposure 2, Offset 4) | 389, 391-394 | V |
| **definitions of a second instance** | top-level `LIST EfdG` | | | one `EfDf` per effect TYPE (`tdmn` = match name, then a copy of ONE instance's `sspc`, including that instance's value records); the `EfDC` count before them is a little-endian u32. When two layers carry the same effect type, only one instance keeps a full `parT`; **the other has an empty `LIST parT` (4 bytes)** but its own `tdgp` (input layer `-0000` with its own `tdpi`, the button record, and its value records). Its definitions come from the `EfdG` copy by match name, its values from its own records, never from the copy (383: the copy holds S100's Start [0.1, 0.2]; L's Ramp reads the 368 defaults). Which instance keeps the full `parT` is **?** (in 383 S100 is both first in the file and newest). The `EfDf` order follows the stack order (365 / 366); `Pefl/pjef` does not. **Some files have no `EfdG` at all** (seen in 17 projects, all from inside After Effects-made Motion Graphics templates). In each of them every effect type (196 in all; 2 to 14 Color Controls per file) has exactly one instance with a full `parT` and an empty one in every other instance, and the reader takes the others' definitions from that instance (`DefinitionSource::OtherInstance`, #3170). Which instance keeps it is **?** here too | 365, 366, 383; templates | V |
| popup items | `parT/pdnm/Utf8` | | text | items separated by `\|` | 077 (`Item 1\|Item 2\|Item 3`), 214 | V |
| stored value | `sspc/tdgp/<match>-NNNN` | | `tdbs` | present only for a parameter that differs from its default; `tdsb` 1, `cdat` as in 3.4. **Absent record = the `pard` default.** Some parameters are recorded in the default state (`tdsb` 3) and the record still holds the value After Effects uses: every one of the 25 such records in the corpus and the samples differs from its `pard` default and reads back as recorded (Lumetri Color's hidden controls High Dynamic Range, HDR Specular, the second Look, Legacy Tone And Saturation, Face Detection, Auto Color, Auto Intensity and SemanticAutoTone Progress, 1200; Cryptomatte's Selection Mode on, 1400). No effect record in the default state holds a value that does not count. The reader takes the value in either state (`EffectParam::recorded_value`, `EffectParam::value`); `EffectParam::stored_value` keeps to written records, and a record in any other state (no sample has one) is not read: the importer reports it as damaged data. That other effects record values so is H (no sample shows one) | see below; 1200, 1400 | V (1200, 1400), H (other effects) |

Stored values by type (`tdsb` 1, or 3 as above):

| Type | Stored | Evidence | Conf. |
|---|---|---|---|
| float slider | `cdat[0]` as set (Slider 77, Blurriness 40); a keyed slider has 48-byte plain records and no `cdat`. **The value is the float32 After Effects keeps, widened to f64: Offset 0.1 is `0.10000000149011612`, Gamma 2.2 is `2.200000047683716`** | 071, 216, 221, 393, 394, 397 | V |
| **group start / end** | the record always exists (`tdsb` 1, `cdat` 0, descriptor `00010000`), `tdsn` = the group name; no value (scripting: NO_VALUE) | 389-397 | V |
| **fixed slider** | `cdat[0]` as a plain double in the parameter's own scale: Tint Amount to Tint is a **percent** (0 -> 0.0, 37.5 -> 37.5; valid max 100), Gradient Ramp Scatter is **pixels** (12, 250.5; valid max 512), Blend With Original is a **0..1 fraction** (25 % -> 0.25, 100 % -> 1.0; valid max 1.0). Keyed: 48-byte plain records with the plain value (Amount 20 / 80) | 361-363, 378-381 | V |
| colour | ARGB doubles 0..255 (Tint Map Black To [0.2, 0.4, 0.6] -> `[255, 51, 102, 153]`, float32 precision; Ramp Start Color red `[255, 255, 0, 0]`); keyed: 152-byte spatial records with the ARGB value at 56 (210 Fill, 364 Tint Map White To) | 073, 357, 371; keyed: 210, 364 | V |
| **button** | the record always exists (`tdsb` 1, `cdat` 0, descriptor `00010000`) and carries no value (scripting: NO_VALUE). An exception to the sparse rule, like the Layer Control record | 356-383 | V |
| **popup (built-in)** | 1-based index as a double: Ramp Shape Radial = `2.0`; the selection is also written to `pard` @56 (only byte 59 changes, as for the dropdown 324). Default Linear = 1, item count u16 @60 = 2, `pdnm` `Linear Ramp\|Radial Ramp` | 368, 377 | V |
| **checkbox** | `cdat[0] = 1.0` when on, `0.0` when off; **the default is the `u32` at `pard` byte 56** (0 off: Checkbox Control 217, Shadow Only 420, Bypass 389; **1 on: Repeat Edge Pixels of Fast Box Blur and Gaussian Blur, read back 1 in 412 and 417**), and a record exists only when the value differs from it (416 and 419 switch Repeat Edge Pixels off: a record with 0.0, descriptor `00010004`). The corpus manifest's "off" for the Gaussian Blur of 214 was wrong. Exposure's Bypass Linear Light Conversion has an empty `pard` name and its label in `pdnm` (like the buttons) | 217, 389, 396, 412, 416, 417, 419, 420, 426 | V |
| **integer slider** (type 1) | Fast Box Blur's Iterations: default `i32` at `pard` 56 (3), valid maximum `i32` at 128 (50), the value record a plain double (1.0 for 1) with `tdum` / `tduM` 1 / 5 (the slider range) | 412, 414 | V |
| **arbitrary data** (type 11) | the value record is a 4-byte `cdat` 0 followed by a `LIST aRbs` holding the data (`aRbp`); `parT` has the default `aRbp` after the `pard`. Hue/Saturation's Master Hue, Saturation, Lightness and Levels' Input / Output Black and White and Gamma are NOT in their visible parameters (which keep no record) but in this data; see below | 443-446, 511-516 | V |
| **point** | two doubles, a **fraction of the layer size** (`tdb4@24` = the layer's width / height: 16 / 9 in 369, 374, 218, 1 on the 100 x 100 solid of 383; `@16` = 6.25e-12 in all): `[111, 222]` is `[111/1920, 222/1080]` (`tdb4[3]` = 2, `[5]` = 15). **The default is two 16.16 fractions 0.5 in `pard` @56 and @60, relative to the layer: a Point Control on a 100x100 solid reads back [50, 50]**. Gradient Ramp Start / End are stored the same way; values outside the layer give fractions outside 0..1 (`[2000, -100]` -> `[1.0417, -0.0926]`); on a 100x100 solid `[10, 20]` is `[0.1, 0.2]`. **Keyed: 104-byte spatial records (56 + 24 x 2), flags `0x0007`, `u32@8` = 1, the two fractions at 56, tangents zero**; `f64@16` holds a derived distance on linear keys too (1.7397 in 382: ignore it) | 218, 310, 369, 374, 382, 383 | V |
| **angle** | plain degrees: 90 is `90.0` | 219 | V |
| **layer control** | the `-0001` record always exists (`cdat` 0); the sibling `tdpi` (u32) is the **id of the chosen layer** (the new Null, `0x22`). **The default ("Layer 1" on the first layer) and an explicit 1 are the owning layer's own id (320); "None" is `tdpi` 0 (321).** The manifest's "None" for the default (076, 220 rows) was wrong. In the implicit first parameter `tdpi` is the id of the layer that owns the effect | 220, 320, 321 | V |
| **layer parameter** (any effect) | as the Layer Control: the `-NNNN` record always exists with `cdat` 0 and `tdpi` = the id of the chosen layer (Set Matte, Displacement Map, Compound Blur set to the layer below: `tdpi` 340 / 354 / 368 = that layer's id); the effect's implicit `-0000` record holds the owning layer's id | 904, 905 | V |
| **dropdown** | the items are in `pdnm` (`Alpha\|Beta\|...`), `pard` u16 @60 is the item count (5), `pard` @56 the selection (1 by default). Selecting item 3 adds a value record (`cdat[0]` = 3.0, descriptor `00010004`) AND writes 3 into `pard` @56. Setting the items re-creates the effect definition under a new match name (`Pseudo/@@KbKT0rCPQi2A0ytuyGLWSw`; display name `Dropdown Menu Control`) | 077, 323, 324 | V |

The default of Gaussian Blur: the default-only file is byte-identical to the one with Blurriness 25
and has no value record; `pard` holds 25 (f64 @56 and f32 @120); setting 40 adds a record and leaves
`pard` at 25, so `pard@56` is the default and not the current value (a popup selection is the one
exception, it overwrites `pard@56`). The corpus manifest's default of 0 was wrong: After Effects
reads a fresh Gaussian Blur as 25 (300, **V**; the conflict is resolved).

Because a popup selection overwrites `pard@56`, a popup whose definition comes from the `EfdG` copy
has no reliable default in the file (the slot holds the copied instance's selection); the reader
returns no default for it (`EffectParam::pard_default` is `None`), and a consumer uses the effect's
known default. Effect stack order is the order of the `sspc` children of the Effect Parade: moving an
effect (366) reorders them and the `EfDf` copies and changes nothing else (365, 366, **V**).

**Exposure (`ADBE Exposure2`, 389-397, V).** `parn` = 24: 0 the input layer, 1 `Channels:` (popup
`Master|Individual Channels`, 1-based, default 1), then four groups of five entries, Master 2-6,
Red 7-11, Green 12-16, Blue 17-21, each `[group start (13), Exposure, Offset, Gamma Correction,
group end (14)]` (the channel groups' sliders are named `Red Exposure`, `Red Offset`, `Red Gamma
Correction`, ...), 22 Bypass Linear Light Conversion (checkbox, default off) and 23 the built-in
entry. The three sliders are float sliders with the same `pard` in every group: Exposure (stops)
default 0, valid -100..100, slider -4..4; Offset default 0, valid -2..2, slider -0.5..0.5; Gamma
Correction default **1**, valid and slider 0.1..10. Each one-change case adds exactly one value
record (twice: layer tree and `EfdG`): Channels 2 (390), Exposure 1.5 (391) and -2.25 (392), Offset
0.1 (393), Gamma 2.2 (394), Channels 2 + Red Exposure 1 (395, two records), Bypass 1.0 (396). A
keyed Exposure (397: -1 at 0 s, 2 at 2 s, linear) has plain 48-byte records. With Channels = Master
the per-channel values are stored but inactive. The other byte changes against 389 are only the
stale UI flags `pard@4` and, for a popup selection, `pard@59`.

**Batch 5 effects (409-450, 511-516, 522-525; V).** Defaults as read back by After Effects for the
fresh effect (MANIFEST), each value change one case. The scale of a fixed slider is told by its
valid maximum (`pard` 128):

| Effect (match name) | Parameters (`parT` index: type, valid maximum, default) | Evidence |
|---|---|---|
| `ADBE Invert` | 1 Channel: popup of 16 items **including three separators `(-`** (RGB, Red, Green, Blue, `(-`, HLS, Hue, Lightness, Saturation, `(-`, YIQ, Luminance, In Phase Chrominance, Quadrature Chrominance, `(-`, Alpha), default 1, stored 1-based over all 16 (Red = 2); 2 Blend With Original: fixed, max 100 (percent), 50 stored 50.0 | 409-411 |
| `ADBE Box Blur2` (Fast Box Blur) | 1 Blur Radius: fixed, max 30 000, pixels (20); 2 Iterations: integer slider, default 3, max 50; 3 Blur Dimensions: popup (Horizontal and Vertical, Horizontal, Vertical), default 1; 4 Repeat Edge Pixels: checkbox, **default on** | 412-416 |
| `ADBE Gaussian Blur 2` | 1 Blurriness: float, default 25, max 30 000; 2 Blur Dimensions: popup as above (Vertical = 3); 3 Repeat Edge Pixels: checkbox, **default on** | 417-419 (and 214, 216, 300, 301) |
| `ADBE Drop Shadow` | 1 Shadow Color: default black; 2 Opacity: fixed, **max 255** (default 127.5 = 50 %, 204 = 80 %); 3 Direction: angle, default 135; 4 Distance: fixed, max 4 000, pixels, default 5; 5 Softness: fixed, max 30 000, default 0; 6 Shadow Only: checkbox, default off | 420-426 |
| `ADBE Glo2` (Glow) | 1 Glow Based On: popup (Alpha Channel, Color Channels), default 2; 2 Glow Threshold: fixed, **max 255** (default 153 = 60 %); 3 Glow Radius: fixed, max 1 000, default 10; 4 Glow Intensity: fixed, max 255, default 1; 5 Composite Original: popup (On Top, Behind, None), **default 2 (Behind)**; 6 Glow Operation: popup of 25 blend modes starting None, Normal, Add, ..., default 3 (Add); 7 Glow Colors: popup (Original Colors, A & B Colors, Arbitrary Map), default 1; 8 Color Looping: popup (Sawtooth A>B, Sawtooth B>A, Triangle A>B>A, Triangle B>A>B), default 3; 9 Color Loops: fixed, max 127, default 1; 10 Color Phase: angle; 11 A & B Midpoint: fixed, **max 0.99, a 0..1 fraction**, default 0.5; 12 Color A white; 13 Color B black; 14 Glow Dimensions: popup as Blur Dimensions | 427-441 |
| `ADBE HUE SATURATION` | 1 Channel Control: popup (Master, Reds, Yellows, Greens, Cyans, Blues, Magentas); 2 Channel Range: **arbitrary data** (below); 3 Master Hue (angle), 4 Master Saturation, 5 Master Lightness (fixed, max 100): **never stored, the values are in the data**; 6 Colorize: checkbox; 7 Colorize Hue: angle; 8 Colorize Saturation: fixed, max 100, default 25; 9 Colorize Lightness: fixed, max 100 | 443-450 |
| `ADBE Timewarp` | 1 Method: popup (Whole Frames, Frame Mix, Pixel Motion), default 3; 2 Adjust Time By: popup (Speed, Source Frame), default 1; 3 Speed: fixed, max 5 000, default 50, plain (25 stored 25.0; keyed: plain 48-byte records 50 / 200); 4 Source Frame: fixed, max 5 000, plain (12); the tuning, motion blur, matte (`Matte Layer`, `Warp Layer`: layer records always present, 0 = none) and crop parameters follow | 529-533 |
| `ADBE Easy Levels2` (Levels) | 1 Channel: popup (RGB, Red, Green, Blue, Alpha); 2 Histogram: **arbitrary data** (below); 3-7 Input Black, Input White, Gamma, Output Black, Output White: float sliders (max 10 000, Gamma 5), **never stored, the values are in the data**; 8 / 9 Clip To Output Black / White: popup (**On, Off, Off for 32 bpc Color**), default 3 | 511-516 |

Hue/Saturation data (180 bytes): `i32` master hue at 0 (444: 90), saturation at 4 (445: 50),
lightness at 8 (446: -30; V); then six colour ranges (Reds, Yellows, Greens, Cyans, Blues,
Magentas) of 28 bytes: four `i32` range points in degrees (default Reds 315, 345, 15, 45; Yellows
15, 45, 75, 105; ...; Magentas 255, 285, 315, 345) and the range's hue, saturation, lightness (E:
layout read from the default, no range was changed). The Colorize values are ordinary records
(447-450). Levels data (108 bytes): `u32` `0x000f10a7`, `u32` 1, then five channels (RGB, Red,
Green, Blue, Alpha) of five `f32` input black, input white, gamma, output black, output white; the
RGB values are V (512-516: 0.1, 0.8, 2, 0.2, 0.9 as float32), the other channels E (their defaults
0, 1, 1, 0, 1). After Effects' undo does not revert a change of these values (MANIFEST). API:
`Effect::arbitrary_data`, `HueSaturationData`, `LevelsData`; test `tests/effects_batch5.rs`.

API: `EffectParam::{stored_value, pard_default, value, is_default, is_animated, keys, valid_max,
valid_min, slider_range, point_px, layer_ref, popup_item_count, popup_selected_index,
popup_selected_item, borrowed_definition}`, `Effect::has_own_definitions`,
`Project::{effect_params, effect_definitions, definition_source}`, `ParamValue`,
`ParamKind::{Button, GroupStart, GroupEnd}`, `DefinitionSource`, `LayerRef`. Tests:
`tests/effects.rs`, `tests/batch3_core.rs`, `tests/batch3_objects.rs`, `tests/effects_batch4.rs`
(356, 357, 361, 363, 364, 366, 368, 369, 371, 374, 377, 378, 380, 382, 383),
`tests/effects_exposure.rs` (389-391, 393-397).

**Importer block 2026-11-05, #3110 (`1100-quickwins.aep`, After Effects 26.2.1; V for every
row: defaults from the defaults composition, the valid ranges from the minimum and maximum
compositions, the items from one composition per popup item).** Each set value is stored as set
(Decay 20 is `20.0`, Blend With Original 100 % is `1.0`); a value set to the default leaves no record
(HDR Amount 100):

| Effect (match name) | Parameters (`parT` index: type, valid range, default) |
|---|---|
| `ADBE HDR ToneMap` (HDR Highlight Compression) | 1 Amount: float, 0..100 (slider 0..100), default 100 |
| `ADBE Echo` | 1 Echo Time (seconds): float, -30 000..30 000 (slider -5..5), default -1/30 (float32); 2 Number Of Echoes: integer, max 30 000, default 1; 3 Starting Intensity: float, 0..1, default 1; 4 Decay: float, **valid 0..100, slider 0..1**, default 1; 5 Echo Operator: popup (Add, Maximum, Minimum, Screen, Composite In Back, Composite In Front, Blend), default 1 |
| `ADBE Linear Wipe` | 1 Transition Completion: fixed, max 100, default 0; 2 Wipe Angle: angle, default 90; 3 Feather: fixed, max 32 000, default 0 |
| `ADBE Offset` | 1 Shift Center To: point, default the layer centre; 2 Blend With Original: fixed, **max 1 (a 0..1 fraction)**, default 0 |
| `ADBE Deflicker` (Color Stabilizer) | 1 Stabilize: popup (Brightness, Levels, Curves), **default 2 (Levels)**; 2 Black Point, 3 Mid Point, 4 White Point: points, defaults 0.2 / 0.5 / 0.8 of the width at half the height; 5 Sample Size: fixed, max 50, default 3. The reference frame the Set Frame button picks is not among the parameters |

**Importer block 2026-11-05, #3112 (`1300-thirdparty.aep`; V for the parameter lists, H for the
data layouts, read from default instances only):**

| Effect (match name, plug-in version) | Parameters (`parT` index: type, valid range, default) |
|---|---|
| `CS Color Neutralizer` (CC Color Neutralizer, 1.3) | 1 / 7 / 13 Shadows / Midtones / Highlights Unbalance: colours (black, 127 grey, white; pickers: setting one sets the range's sliders, e.g. [0.27, 0.55, 0.85] gives Red 42.94, Green 1.18, Blue -44.12 = (mean - channel) x 150 / 255 on 8-bit values, and leaves no colour record); 2-6, 8-12, 14-18 a group per range (start, Red, Green, Blue, end) with float sliders -100..100 (slider -50..50), default 0; 19 Pinning: float 0..100 (slider 0..50); 20 Blend w. Original: float 0..100; 21-25 the Special group: 22 View popup (Final Result, Correction B to W, Correction Stair), default 1, 23 Black Point float 0..49.9, default 0, 24 White Point float 50.1..100, default 100 |
| `EXtractoR` (2.6.6) | 1 Channel Info (`EXtractoR-0013`): arbitrary data, 688 bytes, below; 2-6 the Process group: 3 Black Point and 4 White Point float sliders, valid ±3.4028235e38 (the float32 range; slider ±1 000), default 0 / 1; 5 UnMult: checkbox (empty `pard` name, the label in `pdnm`), default off |
| `IDentifier` (2.6.6) | 1 Channel Info (`IDentifier-0005`): arbitrary data, 148 bytes, below; 2 Display: popup (Colors, Luma Matte, Alpha Matte, Raw), default 1; 3 ID: integer slider, max 2 147 483 647, default 0 |

EXtractoR's Channel Info is the plug-in's own **little-endian** layout: `u32` 1 and 28 bytes, then
four 164-byte records (Red, Green, Blue, Alpha), each a kind in byte 0 (1 a channel of the footage by
name: `R`, `G`, `B` by default; 2 the input's own channel, named `(copy)`: Alpha by default) and the
channel name NUL-terminated from record byte 35. EffectCraft imports kind 2 as its EXtractoR's `(copy)`,
the effect input's channel of the same position as earlier effects left it, not as `A` (which names
an OpenEXR footage's own alpha plane). The rest is not read: it differs between instances
with the same channels (uninitialised-looking values, also in bytes 1..4 of a record: `02 00 00 32`
on Alpha in one definition's default, which is why only byte 0 is the kind). **A chosen channel**
(`extractor-07-preset-channel-z`, a preset of our own that reads `Z` into Red, Green and Blue) is kind
1 with its name (`Z`), and the `u32` at record byte 4 is then 13, which looks like the channel's
position in the EXR's sorted channel list of 14 (the defaults hold 0 there: H). IDentifier's data
differs between instances with the same settings except for **the chosen channel's name**,
NUL-terminated from byte 19, which is `Object ID` when no channel is chosen. After Effects lists
only integer (UINT) channels: in 1300's EXR (a FLOAT `ID` channel) it found none ("No ID channels
available"), so that block's preset `identifier-09-preset-channel-id` only sets ID 2; in 1310's
EXR (UINT `objectId` and `IndexOB.X` added) the preset `identifier-09-preset-channel-objectid-id2`
stores `objectId`, with 15 at byte 4 (the channel's position in the sorted list of 16, as for
EXtractoR: H). Without a chosen channel After Effects leaves the layer unchanged (1300 and 1310
defaults). The other bytes are not read. API: `ExtractorChannels`,
`identifier_channel_type`.

**Curves and Mesh Warp** (961-965, 985; applied from presets, the data is not scriptable): their
arbitrary-data parameter (type 11) always has a record (`tdbs` with a 4-byte `cdat`) and a `LIST
aRbs` / `aRbp`, also at the default. Curves (`ADBE CurvesCustom-0001`, 1644 bytes): `u16` 1, 1;
five 256-byte tables (RGB, Red, Green, Blue, Alpha) = the natural cubic spline through the points,
rounded (0 of 1280 entries differ); five 72-byte point records of 16 (`u16` in, `u16` out) slots,
the count at 64, an `i32` at 68 (-1 untouched, count - 2 in every edited sample: H). Mesh Warp
(`ADBE MESH WARP-0004`): `u32` rows + 1, `u32` columns + 1 (a 1 x 2-cell read-back test renders as made only in
this order, `export-tests\mesh`; AE does not rebuild the stored mesh when Rows / Columns change, 989), 12 bytes
(zeros, `0d 0d`), then per vertex row-major ten big-endian `f32`: position, handles top, right,
bottom, left as fractions of the layer (H: or of the comp), outward border handles (0, 0); Rows /
Columns / Quality are integer sliders with records only off their default (7, 7, 8). The `-NNNN`
suffix is a parameter id, not the `parT` position (Curves -0002 Channel before -0001; Mesh Warp
-0005 Quality before -0004). API: `CurvesData`, `MeshData`, `curve_lut`. [V; details: corpus FORMAT
S]

**Lumetri Color** (`ADBE Lumetri`, 1200: 63 compositions in After Effects 2026, 32 bpc, working space
None; by script: defaults, all set, one per section, valid minima and maxima, every popup item the script
can set, checkboxes toggled, keys, an expression; from presets made by hand, `-58` ... `-62`: an Input
LUT, a Look, the RGB curve, Hue vs Sat, the colour wheels): 131 `parT` entries. The `-NNNN` suffix is a
parameter id, not the position (`-0127` ... `-0130` sit between `-0005` and `-0026`), so the importer
keys by suffix. Float sliders (type 10) store the value After Effects shows; the valid ranges (`pard`
104 / 108) are wider than the slider's (112 / 116): Temperature / Tint -300..300 (slider -100..100),
Saturation 0..300 (0..200), Exposure -7..7 (-5..5), Contrast, Highlights, Shadows, Whites, Blacks,
HDR Specular, Tint Balance -150..150 (-100..100), Faded Film 0..150 (0..100), HDR White 100..1000,
HDR Range 100..10000, Refine Blur 0..1000 (0..30), Vignette Amount -5..5 (-3..3); the read-backs of
`-19-min` / `-20-max` equal the stored numbers. **Hidden controls are recorded in the default state**
(`tdsb` 3) and still hold the value After Effects reads back: High Dynamic Range `-0062` (1.0),
HDR Specular `-0064` (22), the second Look popup `-0128` (26.16 as set, read back 26), Legacy Tone And
Saturation `-0097` (its `pard` default reads off as the script saved the project and on once the
project was opened and saved again, `versions/1200-lumetri-26`, `-25x`, `-24x`; a fresh instance reads
back off), Auto Color `-0124`, Face Detection `-0102`, the Auto Intensity `-0123`
(`EffectParam::recorded_value`). Each section has an Active checkbox (blank `pard` name; `-0003` Basic
Correction, `-0023` Creative, `-0037` Curves, `-0118` RGB Curves, `-0104` / `-0105` / `-0107` / `-0109` /
`-0111` the hue / saturation curves, `-0046` Color Wheels, `-0068` HSL Secondary, `-0050` Vignette, on
by default); off, the section does nothing (`-03-sec-basic-correction`, `-17-sec-vignette` render as
`-00-defaults`). Arbitrary data (type 11, a record and `aRbs` in every instance; empty is `fe fe`):
`-0001` an XML copy of the grade written only when the Look changes (not kept current: `-01-set-all`
holds defaults for controls set after the Look; it names the LUT file, `Fuji F125 Kodak 2393 (by
Adobe).cube`); `-0096` the Look's name, UTF-16LE (`SL BLUE ICE`, `Fuji F125 Kodak 2393 (by Adobe)` in
`-59-preset-look-lut`; the popup `-0025` itself has no record, `-0128` can disagree with it:
`-07-sec-creative` names item 20 while `-0128` holds 23), `-0095` likewise for the Input LUT
(`ALEXA_Default_LogC2Rec709`, `-58-preset-input-lut`; the popups `-0005` / `-0127` keep no record,
`-0004`, `-0024` and the assets `-0125` / `-0126` stay `fe fe`: the LUTs are After Effects' own files,
named, not embedded); `-0129` the Look colour space's 16-byte id (the GUID `LookColorSpace` of `-0001`
in Windows layout: the first 4, 2 and 2 bytes little-endian; zero for None; the Color Space popup
`-0130` writes no record of its own, `-22-pop38-1` ... `-53-pop38-33`);
`-0032` Split Toning (32 zero bytes); `-0039` the four RGB curves and `-0042`, `-0106`, `-0108`,
`-0110`, `-0112` the hue / saturation curves, 520 bytes each: `i32` type (3 for the hue curves, 2 for
Luma vs Sat and Sat vs Sat, 0 for an RGB curve), `u32` point count, then little-endian `f64` pairs x, y
sorted by x (bytes after the counted points are stale). RGB: x and y 0..1, (0, 0), (1, 1) by default;
the first of the four is the master curve (`-60-preset-curves-rgb`: (0.491, 0.787) lifts R, G and B
alike, and the render follows a natural cubic spline through the points within 0.003); the order of
the other three is H (the `-0001` XML names `ToneCurvesLuma`, `Red`, `Green`, `Blue`; no sample moves
one). Hue / saturation: x the hue (0 red, 1/3 green) or the luma / saturation 0..1, y the offset from
neutral 0; a fresh curve has no points, the selector colours `-0113` ... `-0117` add three at y = 0,
the hue picked +- 0.0556. Hue vs Sat's y scales the saturation by 1 + y in linear light
(`-61-preset-curves-huesat`: (0.0155, 0.97) takes pure red's linear saturation to 1.96 times, (0.263,
-0.03) and (0.765, -0.02) green, cyan and blue to 0.97); the other four curves' scale is H. `-0047`
Color Wheels, 4 `f64` triples: hue in degrees (0 red, 120 green), brightness slider 0..1 (0.5
neutral), distance from the centre 0..1; 0, 0.5, 0 each by default; Shadows, Midtones, Highlights, then
a fourth wheel of unknown role (`-62-preset-wheels`: Shadows 229.1, 0.370, 0.547 towards blue with the
slider down; Midtones 0.1, 0.5, 0.395 towards red; Highlights 93.4, 0.727, 0.319 towards yellow-green
with the slider up; the fourth at its default); `-0085` the HSL correction wheels (8 triples, the same
default; layout H); `-0073` the HSL key, 40 bytes: a flag byte (1 once a colour was picked), then hue,
saturation, lightness, each centre, range, tolerance (`f32` LE, fractions) and an enabled byte (fresh:
0.5, 0, 0, on; picking RGB 0.02 0.55 0.85 gives hue centre 0.5605 = that colour's hue and lightness
centre 0.435 = its HSL lightness, range 0.05, tolerance 0.1: `-01-set-all`; whether range is the half
or full width is H); `-0122` the Auto Tone analysis (UTF-16 JSON). The project saved as 25.x and 24.x
copies (`versions/1200-lumetri-25x`, `-24x`: the 58 scripted compositions, saved by After Effects
2026) stores every Lumetri definition, valid range and value as the 26 save does. [V for the sliders,
checkboxes, popups, the `tdsb` 3 values, the master RGB curve, Hue vs Sat, the colour wheels and the
LUT names; H where marked]

**Cryptomatte plug-in** (match name `Cryptomatte`, the free Cryptomatte plug-in, 2.6.6 in our
sample; read from 88 instances in one production project, not made by us, and from our own After
Effects 2026 sample 1400 (corpus `aep/cryptomatte.aep`: a fresh instance, Selection Mode on, each
Output item, and two presets of our own that choose a layer and pick one object or material in the
plug-in's UI; scripting read-backs and renders per composition); H for what neither varies):
`parT` 0 the input layer; 1 Settings: **arbitrary data** (type 11, `Cryptomatte-0001`, a record and
`aRbs` / `aRbp` in every instance); 2 Output: popup (Colors, Matted Colors, Matted RGBA, Matte
Only), **`pard` default 1 = Colors** (bytes 56..64 `00000001 0004 0001`, in the instance's own and
the project's shared definition; scripting reads 1 from a fresh effect: 1400 `crypto-00-defaults`),
a written record (`tdsb` 1) holding the item number when set (1400: 2, 3, 4; production: 3 in 81,
4 in 7); 3 Selection Mode: checkbox (`pdnm` `fnord!`), no record when off, and when on **a
default-state record (`tdsb` 3) holding 1.0** that After Effects reads back as on and renders
(1400 `crypto-01-set-all`, `crypto-02-chk-toggled`; the general reader takes only `tdsb` 1
values, so the importer reads this one itself); the built-in entry. As in every project, only the
first instance has its own `parT`; the others borrow the project's (1400). The Settings data is
the plug-in's own layout, **little-endian**: `cry1`; bytes 4..32 differ between instances with
the same settings (zeros in one, pointer-like values in others: not read); the layer name at 32,
NUL-padded to 64 bytes (`CryptoObject`, `CryptoMaterial` in 1400: the EXR layer's name, which is
also its channel prefix there, so the sample cannot tell which of the two the plug-in stores); at
96 a `u32` 1 and a `u32` that is the same in every instance; at 104 the `u32` length of the
selection text including its NUL; at 108 **the MurmurHash3 (x86, 32-bit, seed 0) of the selection
text with its NUL** (V on the three texts of 1400: `<11a97486>` `0x27880767`, `<a5deeb9e>`
`0xf7374a5b`, the empty text `0x514e28b7`; not read); a zero byte at 112; the selection text from
113, the matte list as comma-and-space separated ID hashes `<11a97486>, <deadbeef>` (303 tokens in
production, all in that form). Each token is the 8-digit hash of a picked name as the EXR's
manifest lists it, i.e. the MurmurHash3 of the name (1400: `box_red` `11a97486` picked on
CryptoObject, `mat_paint` `a5deeb9e` on CryptoMaterial). After the text 2 to 3 bytes, zeros or
left-overs of a longer earlier text (1400's presets: 127 bytes, three zeros after the text). **A fresh effect** (the `parT`
default and the record alike, 116 bytes, 1400) is `cry1`, zeros to 96, then `u32`s 1,
`0x514e28b7`, 1, `0x514e28b7` and four zero bytes: no layer and an empty text (length 1, its NUL).
`0x514e28b7` is the hash of a single NUL byte, so the `u32` at 100 looks like the hash of a first
text that is empty in every sample, its length the `u32` 1 at 96 and its NUL the zero byte at 112
(H: one value). Layer and selection decode the same in the production project and in 1400's two
presets, and After Effects draws exactly the picked objects as selected (`crypto-06`: the box,
`crypto-07`: box and disc, the two objects of `mat_paint`). API: `CryptomatteData` (refuses data without `cry1`, a length past the end or
a non-zero byte at 112).

**Puppet (`ADBE FreePin3`; ?: external listing, not the corpus, #3093).** No file of the
corpus, the committed fixtures or the samples holds a Puppet. The tree below comes from outside
the corpus: a public scripting listing of the effect's properties (a forum post, an After Effects
release from before the Advanced and Bend pins and Density). It is not bytes we have seen; that
After Effects writes it by the general rule (a `tdmn` and the `LIST tdgp` or `tdbs` it names,
nested in the instance's value group `sspc/tdgp`, as masks, shapes and layer styles are) is
**H**:

```text
Puppet                ADBE FreePin3
  On Transparent      ADBE FreePin3 On Transparent
  arap                ADBE FreePin3 ARAP Group            (hidden)
    Auto-traced Shapes  ADBE FreePin3 Outlines
    Mesh              ADBE FreePin3 Mesh Group            (hidden)
      Mesh 1          ADBE FreePin3 Mesh Atom
        Mesh          ADBE FreePin3 Mesh                  (hidden: the triangulation)
        Triangles     ADBE FreePin3 Mesh Tri Count
        Expansion     ADBE FreePin3 Mesh Expansion
        Deform        ADBE FreePin3 PosPins
          Puppet Pin 1  ADBE FreePin3 PosPin Atom
            Vertex Offset ADBE FreePin3 PosPin Vtx Offset  (hidden, 2 values)
            Vertex Index  ADBE FreePin3 PosPin Vtx Index   (hidden; 4294967295 = none)
            Position      ADBE FreePin3 PosPin Position
        Overlap       ADBE FreePin3 HghtPins              (hidden while empty)
        Stiffness     ADBE FreePin3 StarchPins            (hidden while empty)
```

A pin's rest place is not a property: it is a vertex of the hidden mesh (Vertex Index) plus an
offset, so it cannot be read without decoding the mesh data. The unit of Position (layer pixels,
or a fraction of the layer as effect points are) is **?**. The match names of the Overlap and
Starch pin atoms and their properties, of the Advanced and Bend pins (Scale, Rotation) and of
Density, Mesh Rotation Refinement and the Puppet Engine are **?**, and so is which of them are
`parT` parameters; whether After Effects stores some `parT` parameters of a fresh Puppet even at
their defaults (the importer would then report them every time) is **?**; whether a Puppet
without pins has an (empty) Mesh Group, or no ARAP group at all, is **?**. The importer
(`import/puppet.rs`) therefore brings the effect in as EffectCraft's Puppet without meshes or
pins and reports, per mesh, Triangles, Expansion and per pin group the pins, how many are keyed
or carry an expression, and their atom match names; every entry outside this tree is named in the
report. The sample that settles it: a fresh Puppet with no pins in one composition and, in
another, a 400 x 800 solid in a larger composition (pixels against fractions) with a Puppet of
three Position pins (one keyed, one moved off its rest place without
keys), one Advanced, one Bend, one Starch and one Overlap pin, Triangles, Expansion and Density
off their defaults, a second mesh, and a scripting read-back of every property's match name and
value, the hidden ones included. Pins added by script are reported to come out disabled, so the
pins are placed by hand or by a preset.

### 3.8 Masks and bezier paths (`tdgp[ADBE Mask Parade]`)

| Field | Chunk path | Offset | Type | Meaning | Evidence | Conf. |
|---|---|---|---|---|---|---|
| inverted | `Mask Parade/mkif` | 0 | u8 | | 090 (0), 091 (1) | V |
| **mode** | `mkif` | 4 | u32 | Add 1, Subtract 2, **Intersect 3, Lighten 4, Darken 5, Difference 6, None 0** (329: the same ellipse path as 232) | 090, 091, 234-237, 329 | V |
| name | `tdgp[ADBE Mask Atom]/tdsn/Utf8` | | text | `Mask 1` | 090 | V |
| feather, opacity, expansion | `ADBE Mask Feather`, `ADBE Mask Opacity`, `ADBE Mask Offset` | | see 3.4 | stored only when not at the default; feather can be keyed (plain n=2 records) | 090, 091, 208 | V |
| path property | `om-s[ADBE Mask Shape]/tdbs` | | | animated: `tdb4` byte 68 = 1 and a `list` of 64-byte records (time and common fields only) | 090, 093 | V |
| path shapes | `om-s/omks/LIST shap` | | | one per keyframe, one for a static path | 090, 093 | V |
| path never set | (no `ADBE Mask Shape` entry in the atom) | | | a mask added without a path (scripted `addProperty`) stores no Mask Path at all, also no `om-s`; After Effects reads it back, also after reopening, as the **source rectangle** `[0,0] [0,h] [w,h] [w,0]` (clockwise from the top left, no tangents) and renders it so (901: expansion -100 leaves a 100 px border) | 900, 901 | V |
| box | `shap/shph` | 4, 8, 12, 16 | 4 x f32 | left, top, right, bottom **normalised to the layer (source) size** for masks (560 / 1920, 340 / 1080, 1360 / 1920, 740 / 1080; on a 100x100 solid a 60x40 rectangle has the box 0.2, 0.3, 0.8, 0.7 and decodes exactly with the layer size, not the 1920x1080 composition). It is the box of the vertices **and** of the handles | 090, 232, 309 | V |
| **open path** | `shph` | 3 | u8 | bit `0x08` set = open path (`0x09` against `0x01`); byte 20 is 1 in every sample, open or closed | 233 against 090, 232, 234 | V |
| points | `shap/list` (`lhd3` record size 8) | | f32 x, y | three per vertex in the order **vertex i, out handle i, in handle i + 1**, normalised inside the box (u = (x - left) / (right - left)); the last record is the in handle of vertex 0. A **tangent is the handle minus the vertex**, without a sign change | 090 (rect), 093 (two rects), 232 (ellipse: tangents `0.5523 x radius`, `[-220.91, 0]`, `[0, -110.46]`, ...), 233 (open path, 3 vertices) | V |

**There is one `mkif` per mask**, placed directly before its atom (two masks: `mkif tdgp mkif tdgp`,
328), so mode and inverted are reported per mask. `mkif` byte 8 is the running mask id counter of the
**layer** in the session (1, 2 for the two masks of 328; 329 then gets 3; undo does not reset it; a new
layer starts at 1 again): exposed as `Mask::counter`, a session counter and not a stable id. API:
`Mask::{mode, inverted, counter}`, `Project::mask_vertices_px` (vertices in layer pixels), `Layer::mask_atoms` (`MaskAtom`: the mask with its atom group, its path property and its shapes by position, for the keys and expressions of the mask properties). Tests:
`tests/masks.rs` (with a synthetic check of the handle order), `tests/batch3_objects.rs`.

### 3.9 Shape layers (`tdgp[ADBE Root Vectors Group]`)

A shape layer (type 4) holds `ADBE Vector Group` (user name `Group 1`) with `ADBE Vectors Group`
(the contents, in z-order, `tdsb` 0x401) and `ADBE Vector Transform Group`. Items are named by their
match name and carry their own user name. Colours are ARGB doubles 0..255. All sizes, positions and
path points are in pixels. **An item's eye switch is bit 0 of its group's `tdsb`** (1 on; 986: a
Fill at 0 is not drawn, 987: the same file at 1 draws it; both saved by After Effects from a patched
copy of 850, the switch is not scriptable) [V].

| Item (match name) | Property | Stored | Evidence | Conf. |
|---|---|---|---|---|
| `ADBE Vector Shape - Rect` ("Rectangle Path 1") | `ADBE Vector Rect Size` | f64 `[400, 300]` (2 components) | 238-242 | V |
| `ADBE Vector Shape - Group` ("Path 1") | `om-s[ADBE Vector Shape]` | the same `shph` + point list as a mask, with a box in **pixels** relative to the layer origin; vertices `[0,-100] [100,100] [-100,100]`, in tangents `[-10,-20] [30,20] [-60,10]`, out tangents `[40,0] [-20,30] [0,-50]`; the box covers vertices and handles: `-160, -120, 130, 130` | 240 | V |
| `ADBE Vector Graphic - Fill` | `ADBE Vector Fill Color` | `[255, 0, 255, 0]` | 238, 240, 242 | V |
| `ADBE Vector Graphic - Stroke` | `Stroke Color` `[255, 0, 255, 0]`, `Stroke Width` 8, **`Stroke Line Cap` 2.0 (round; butt = 1 is absent)**, **`Stroke Line Join` 2.0 (round; miter = 1 is absent)**, `Stroke Miter Limit` 4 with `tdsb` 3 once the join was touched | doubles | 239 | V |
| `ADBE Vector Filter - RC` ("Round Corners 1") | `ADBE Vector RoundCorner Radius` | f64 20 | 238 | V |
| `ADBE Vector Graphic - G-Fill` ("Gradient Fill 1") | `ADBE Vector Grad Start Pt` `[-200, 0]`, `ADBE Vector Grad End Pt` `[200, 0]` | 2 components, class 15; the gradient type (linear) and the colour stops stay at After Effects' default and are not stored | 241 | V (points) |
| `ADBE Vector Transform Group` | `ADBE Vector Position` | f64 `[50, 50]`; absent = `[0, 0]`, as are all other transform properties at default | 242 | V |

Batch 3 (all **V**; shape layer `Shape`, label 8):

| Item | Stored | Evidence |
|---|---|---|
| `ADBE Vector Shape - Star` ("Polystar Path 1") | After Effects' defaults are star, 5 points, outer radius 100, inner radius 50, rotation 0, roundness 0, so **a default star has no property record at all** (the 150 / 70 of batch 1 were stored). A **polygon** stores `ADBE Vector Star Type` 2.0 (`tdsb` 1, descriptor `00020000`) plus `Inner Radius` 50 and `Inner Roundess` 0 (After Effects' own spelling) as `tdsb` 3 default-state records; Points 5 and Outer Radius 100 stay absent | 330, 331 |
| `ADBE Vector Graphic - G-Fill` | `ADBE Vector Grad Type` 2.0 for radial (`tdsb` 1, descriptor `00020000`; linear is 1 and absent); Start Point `[0, 0]` is the default and absent, End Point `[200, 0]` stored; the radial-only `Grad HiLite Length` and `Grad HiLite Angle` are written with `tdsb` 1 and value 0 (absent in the linear 241). Colour stops are in no property | 241, 332 |
| two siblings with one match name | one group holds `Rectangle Path 1` (Size `[400, 300]` stored), `Rectangle Path 2` (Size `[100, 100]` is the default, no record; `ADBE Vector Rect Position` `[300, 0]`, 2 components, class 15) and `Fill 1`, in z-order: a reader keys the items by occurrence, not by match name | 333 |

Batch 5 (all **V**; shape layer `S`, Group 1 = Ellipse Path 1 [400,300] + Fill 1 green in 451, one change each): every value is stored **as After Effects shows it**, in the shape's own record (`tdsb` 1), and absent at its default:

| Property (match name) | Stored | Evidence |
|---|---|---|
| `ADBE Vector Shape - Ellipse`: `ADBE Vector Ellipse Size`, `... Ellipse Position` | pixels `[500,200]`, `[100,50]` | 452, 453 |
| `ADBE Vector Graphic - Fill`: `ADBE Vector Fill Opacity`, `ADBE Vector Fill Rule` | percent 50; popup 1-based, even-odd = 2 | 454, 455 |
| `ADBE Vector Transform Group` of a group: `ADBE Vector Anchor`, `Scale`, `Rotation`, `Group Opacity`, `Skew`, `Skew Axis` | pixels `[10,20]`; **percent** `[50,80]` (not a fraction as the layer's Scale); degrees 30; percent 40; 20; degrees 45 | 456-461 |
| layer `ADBE Anchor Point` of a shape layer | pixels `[10,20,0]` as stored (no source size), as on a text layer | 462 |
| `ADBE Vector Graphic - Stroke`: `ADBE Vector Stroke Opacity` | percent 50 (colour `[255,255,0,0]` and width 6 as in 239) | 463, 464 |
| `ADBE Vector Filter - Trim`: `Trim Start`, `Trim End`, `Trim Offset`, `Trim Type` | percent 20, 70; degrees 45; popup 1-based, individually = 2; a new Trim Paths has no record | 465-469 |
| `ADBE Vector Rect Roundness` | pixels 30 | 470, 471 |
| `ADBE Vector Shape - Star`: `Points`, `Rotation`, `Inner Roundess`, `Outer Roundess`, `Position`, `Outer Radius`, `Inner Radius` | 6; degrees 30; percent 20, 40; pixels `[50,-20]`, 150, 30 | 472-479 |
| `ADBE Vector Graphic - G-Fill` added: Start `[0,0]` and End `[100,0]` are the defaults, no record; **the colour stops have no record at their default**, which renders white to black, linear in the encoded values (481: 0.75 / 0.5 / 0.25 at a quarter, half and three quarters) | 480, 481 |
| a second top-level group, a group nested in a group (`ADBE Vector Group` inside `ADBE Vectors Group`), `ADBE Vector Graphic - G-Stroke` (width 10) | the same records as at the top level, in z-order | 482, 483, 484 |
| defaults of every item (519: AE read-back with no record stored): rectangle and ellipse size 100 x 100, position 0, roundness 0; star 5 points, radii 100 / 50, rotation and roundness 0; fill red `[1,0,0]`, opacity 100, rule 1; stroke white, width 2, cap / join 1, miter 4; gradients linear, start `[0,0]`, end `[100,0]`; trim 0 / 100 / 0, type 1; round corners radius 10; group transform anchor / position 0, scale 100, opacity 100; blend mode, composite order, shape direction 1 | none of them has a record | 519 |

The 3D-extrusion `ADBE Vector Materials Group` of a group changes its `tdsb` from 1 to 3 when a polystar or a gradient is added (472-484), with no property record.

API: `Layer::shape_groups` (`ShapeGroup`, `ShapeItem`, `Stroke`, and `ShapeGroup::details` with
`ShapeDetail::{Polystar, Gradient, RectPosition}`, `PolystarType`, `GradientType`). Other items
(ellipse, trim paths, repeater, dashes, ...) are listed as `ShapeItem::Other` with their match name;
the polystar is listed as `Other` and the gradient fill as `GradientFill`; both carry their extra
fields in `details` (indexed by `item_index`); see section 4. Tests: `tests/shapes.rs`,
`tests/batch3_objects.rs`.

### 3.10 Text documents (`btds[ADBE Text Document]/LIST btdk`)

`btdk` is not a chunk sequence but a PostScript-like dictionary (`/key value` pairs, `<< >>`, `[ ]`,
numbers, `true` / `false`, `/names`, strings as `(` + `fe ff` + UTF-16BE + `)` with `\` escapes). The
verified paths from the top level (keys `/98 /0 /1`; `RUN` = `/1/1[0]/0`):

| What | Path | Evidence | Conf. |
|---|---|---|---|
| text | `RUN/0`; a trailing `\r` ends the paragraph. **A hard line break is `\r` too: `A\rB` is stored `A\rB\r`** | 110 (`A`), 113 (`Hello`), 243 | V |
| **U+0003 in the text** | stored as it is (`A`, U+0003, `B` is `A\u{3}B\r`), with the paragraph runs of a CR (`[2, 2]`). After Effects lays it out exactly like `A\rB` (387): two composed lines, two paragraphs, baselines at y 0 and 98 (`text.measure` in AE). So U+0003 is a line break that starts a new paragraph; the importer maps it to `\r` | 386, 387; test `etx_is_stored_as_it_is_and_splits_the_paragraph_runs` | V |
| font size | `RUN/6/0[n]/0/0/6/1` (100 by default) | 110, 113 (60) | V |
| fill colour | `RUN/6/0[n]/0/0/6/53/0/1` = `[A R G B]` floats 0..1 | 110 (white), 113 (`[1 1 0.5 0]`), 248 | V |
| **paragraph runs** | `RUN/5/0[n]/1` = characters including the `\r` (`A\rB\r`: `[2, 2]`; `Hello\r`: `[6]`) | 243, 245 | V |
| **style runs** | `RUN/6/0[n]/1` = characters per style run (`A\rB\r`: `[4]`). **A range style splits `Hello\r`: characters 1 to 3 red gives `[1, 2, 3]` with the fills white, `[1 1 0 0]`, white** | 243, 248 | V |
| **justification** | `RUN/5/0[n]/0/0/5/0`: 0 = left, **1 = right (334)**, 2 = centre, **3 = justified with the last line left, 4 = last line right, 5 = last line centred, 6 = every line justified**; the scripting enum value of right (7414) is not what is stored | 245 (0), 246 (2), 334 (1), 565 (3), 567 (4), 566 (5), 568 (6) | V |
| **character settings (S17)** | one `text.set_style` change each against the point text 538 (`HHHH`, Times New Roman 100 px, normal baseline), the slot named is the only one that changes: `/6` horizontal scale as a factor (539: 1.0 -> 0.9), `/7` vertical scale (540: 1.5), `/9` baseline shift in px (541: 41), `/11` automatic kerning **0 none, 1 metrics (default), 2 optical** (542, 543), `/18` ligatures (off by default, 555; switching ligatures off in 551 changes nothing), `/35` baseline direction **1 with the stream (default), 2 rotated (573), 3 across the stream / tate-chu-yoko (576)**, `/36` tsume as a fraction (553: 0.5), `/52` no break (552), `/56` fill drawn (on by default, 546), `/57` stroke drawn (off by default, 544), `/58` **stroke over fill (on by default: 547 setting it on changes nothing, 550 setting it off gives false)**, `/62` line join **0 miter, 1 round** (548), `/63` stroke width in px (**1.0 by default**, 545: 5), `/70` digit set **0 default, 1 Arabic (571), 2 Hindi (578)**; with digit set Arabic the text `1234` has the same layer bounds as with the default (S17, not saved) | 538-578, 595, 596; tests `tests/text_style_slots.rs` | V |
| **manual kerning** | `RUN/8/0` (beside the style runs): one entry `<< /0 << /0 amount >> /1 length >>` per run of characters, amount in 1/1000 em; kerning 50 on `HHHH` gives four entries of length 1 with 50 and one for the closing `\r` without an amount, `RUN/7` = 50, and switches automatic kerning `/11` to 0 | 554 | V |
| **paragraph settings (S17)** | `RUN/5/0[n]/0/0/5/<slot>`, one change each against 538: `/1` first-line indent (560: 20), `/2` start indent (561: 30), `/3` end indent (562: 40), `/4` space before (563: 10), `/5` space after (564: 12), all in px; `/8` leading model **0 Roman, 1 Japanese** (572); `/9` automatic hyphenation (**on by default**, 556 off); `/21` Roman hanging punctuation (558); `/29` **every-line composer, off by default** (559 on; 557 setting it off changes nothing, so a new text uses the single-line composer); `/33` direction **0 left to right, 1 right to left** (569). Setting the Universal composer (570) changes nothing; the Latin/CJK composer cannot be set by script | 556-572 | V |
| **vertical text** | `lineOrientation` vertical right to left adds `/0/8/0[0]/0/2/1` = 2 to the frame dictionary (absent for horizontal text) | 575 | V |
| **box (paragraph) text** | frame dictionary `/0/8/0[0]/0`: `/1/0` = 32 floats = 16 x, y pairs tracing the rectangle (corners `[-200,-100] [200,-100] [200,100] [-200,100]`), `/2/0` = 1; both keys are absent for point text. Alignment changes only the glyph geometry, not the frame | 245, 246 (box 400 x 200, top-left `[-200, -100]`), 247 (point text: absent) | V |
| **tracking** | `RUN/6/0[n]/0/0/6/8` = 0, 100 in 247 | 247 | V |
| **faux bold / faux italic** | style slots `.../6/2` and `.../6/3`, `false` -> `true`; the font style stays Regular. **Side effect: the font list `/0/1/0` gains a new first entry (4 -> 5 fonts) with property `/3` = 2 (bold) or 1 (italic), the old entries shift up and the style's font index `/6/0` becomes 1** | 335, 336 | V |
| **caps** | style slot `.../6/12`: 0 normal, **1 small caps, 2 all caps** | 337, 338 | V |
| **superscript / subscript** | style slot `.../6/13`: **1 superscript, 0 normal, 2 subscript** (the only slot that changes between 526, 527 and 528). After Effects lays superscript and subscript out at 58.3 % of the size, 33.3 % of the size above / below the baseline: `HHHH` in Times New Roman 100 px is 288.9 px wide with its baseline at 0 when normal, 168.4 px with its baseline at -33.3 / +33.3 when superscript / subscript (`text.measure`, layer bounds). **Every scripted text of the corpus is superscript** (526: the character state After Effects was in; 243, 517 and the reference 244 too), which is why this slot never showed up as undecoded | 526-528, 243, 517 | V |
| **wrapping box text** | a 600 x 400 box at `[-300, -200]` (as in 245) with a sentence of 70 characters: the document holds the text, size and box only; After Effects composes four lines (`The quick brown fox ` / `jumps over the lazy dog ` / `and keeps running far ` / `away`), the line breaks are not stored | 517 | V |
| **leading** | style slot `.../6/5`: the scripted samples have the explicit leading 98.0 (100 px Times New Roman; `/4` false), leading 60 is 60.0; with automatic leading on, 400 stores 0.01 (see "automatic leading" below) | 339, 400 | V |
| **last run** | the last style run reaches the paragraph end and includes the final `\r` (two range styles on `Hello`: runs `[1, 2, 3]`, fills white / `[1 1 0 0]` / `[1 0 0 1]`, sizes 100 / 100 / 60) | 340 | V |
| font | `.../6/0` = index into the font list `/0/1/0[i]/0/0/0` (`TimesNewRomanPSMT`, ...). The list always holds entries no run uses (`Myriad-Roman`, `MyriadPro-Regular`, `AdobeInvisFont`), so only the fonts the runs point at matter | 110, 335 (index 1 is still `TimesNewRomanPSMT`) | E |
| **stroke colour** | `.../6/54/0/1` = `[A R G B]`, the fill's layout; white in the untouched samples, red `[1 1 0 0]` in 574, blue in 595. The stroke switch, width and order are slots `/57`, `/63`, `/58` (see "character settings") | 110, 113, 574, 595 | V |
| **automatic leading** | style slot `.../6/4`: `true` = automatic leading (lines advance by 1.2 x the size; `/5` is then not used), `false` = `/5` is the leading. **243: `/4` false, `/5` 98, the second line's baseline is stored 98 px below the first (`/1/1[0]/1/2[0]/6[0]/6[0]/6[0]/6[1]/0/0[1]`)**. **400 (Auto Leading switched on for `A` CR `B`, size 100, leading 98; against its unedited save 399): the run style changes in `/4` (false -> true) and `/5` (98 -> 0.01) only, the stored second baseline moves from 98 to 120 px below the first (1.2 x 100), and After Effects measures the same 120 px (`text.measure`)**; the rest of the diff is save noise (`head`, the comp's modification time, the text's `btgu/pgui`). That `/5` holds 0.01 under automatic leading is seen in 400 only | 243, 339 (`/5`); 400 (`/4`); test `automatic_leading_is_slot_4` | V |
| **undecoded slots** | the run style and paragraph dictionaries of an untouched document (244; 110 stores the same bytes) are the reference: a slot that is not decoded and differs from it is listed (`TextDocument::undecoded_style_fields`, `undecoded_paragraph_fields`); a slot the reference lacks is listed only when it is not 0, `false`, empty or `/nil` (a neutral value is taken to carry nothing, so it is not reported as a loss; `/nil` is how 244 stores its paragraph slots `/27`, `/28` and `/36`). Decoded style slots: `/0 /1 /2 /3 /4 /5 /6 /7 /8 /9 /11 /12 /13 /18 /35 /36 /52 /53 /54 /56 /57 /58 /62 /63 /70`; paragraph slots: `/0 /1 /2 /3 /4 /5 /8 /9 /21 /29 /33`. Not found by any scriptable setting (none of the `text.set_style` attributes tried in S17 changes them): style `/20 /38 /78 /87 /88`, paragraph `/31 /32`. **Paragraph `/24`, `/35` and style `/68`** (1, 1 and 0 in a scripted text) were edited in place in the wrapping box text 517 (732: `/24` 0, 733: `/35` 0, 734: `/68` 1, same length); After Effects opens each without a dialog, keeps the value on a re-save and composes the same four lines, baselines and bounds (`text.measure`, `layer.bounds`; 734 renders like 517), so 0 and 1 are not listed (`NEUTRAL_PARAGRAPH_VALUES`, `NEUTRAL_STYLE_VALUES`; their meaning stays unknown, Latin text only). **Paragraph `/34`** is 0 in a scripted text and 7 after Reset Paragraph (727: the only slot that changes); a justified 600 px box text of 88 characters composes the same four lines, baselines and bounds with both values (S18), so 0 and 7 are not listed (`NEUTRAL_PARAGRAPH_VALUES`), other values are | every text sample lists none; 725, 727 | V (reference), E (/34) |
| **Source Text record** | the `tdbs` in `btds[ADBE Text Document]` (next to the `btdk`) has no keys and no expression in every sample; Source Text keyframes (presumably one `btdk` per key) and expressions were not sampled, the reader only reports them (`Layer::source_text_property`, `Layer::text_document_count`) | 110-114, 243-248, 334-340 | E |

API: `TextDocument` (`paragraphs`, `style_runs`, `text_box`, `paragraph_texts`, `font_of`,
`incomplete_runs`, `undecoded_style_fields`, `undecoded_paragraph_fields`, `kerning`,
`line_orientation`), `StyleRun::{faux_bold, faux_italic, caps, auto_leading, leading, stroke_argb,
font_index, horizontal_scale, vertical_scale, baseline_shift, auto_kern, ligatures,
baseline_direction, tsume, no_break, apply_fill, apply_stroke, stroke_over_fill, line_join,
stroke_width, digit_set}`, `ParagraphRun::{first_line_indent, start_indent, end_indent, space_before,
space_after, leading_type, auto_hyphenate, hanging_roman, every_line_composer, direction}`, `Caps`,
`AutoKern`, `KernRun`, `Justification`.
A damaged run is never dropped silently: a run without a readable length is left out, a run with a
missing size, fill or font index takes them from the run before it, and both count in
`incomplete_runs`. Tests: `tests/text_markers_footage.rs`, `tests/batch3_objects.rs`,
`tests/text_fields.rs`.

### 3.11 Markers

`mrst[ADBE Marker]` holds a `tdbs` with a `list` of 16-byte records (the times; `tdb4` byte 12 is the
time scale) and one `LIST mrky/LIST Nmrd` per marker. Layer markers live in the layer's own tree,
composition markers in the `SecL` list named `Markers`.

| What | Chunk path | Meaning | Evidence | Conf. |
|---|---|---|---|---|
| time | the key list | 25 600 = 1 s | 122, 123, 249 | V |
| **duration** | `Nmrd/NmHd` (20 bytes: u32 4, u32 0, u32 duration, u32 600, u32 0) | duration / 600 = the marker duration in seconds (1 s is 600; 0 for none); 600 is the units per second; **0.5 s and 0.3 s read back exactly (509, 510)** | 249 (1 s), 122 (0), 509, 510 | V |
| **several markers** | the key records and the `Nmrd` lists in the same (time) order: 509 has two composition markers, 1 s `M1` label 2 and 2.5 s `M2` duration 0.5; a fractional time is a plain key time (0.52 s = 13 312 / 25 600, 510) | | 509, 510; test `tests/batch5.rs` | V |
| **strings** | five `Utf8` children of `Nmrd`, in the order **comment, chapter, URL, frame target, cue point name**; unused ones are empty | `LM`, `Chap`, `http://a.example/u`, `_blank`, `CueX` | 249 | V |
| **cue type** | `NmHd` u32 @0: **4 = event cue (also for markers without a cue point name), 5 = navigation cue** | 347 (4), 348 (5) | V |
| **cue parameters** | `NmHd` u32 @4 = the number of key / value pairs; the pairs follow the five fixed strings as `2 x pairs` extra `Utf8` (key, value, key, value): `speaker` = `A`, `n` = `2` with the cue point name `CueE` | 349 | V |
| **label** | `NmHd` u8 @16 (followed by three pad bytes): label 3 is `03 00 00 00`. The label is not in the keyframe flags (unlike the keyframe label of 057) | 347 | V |

API: `Marker::{label, cue_type, cue_params}`, `CueType`. Tests: `tests/text_markers_footage.rs`,
`tests/batch3_objects.rs`.

### 3.12 Cameras and lights

Both have no source item (`ldta@40` is 0 for a camera, `0xffff_ffff` for a light), type 2 or 1, the 3D
bit and byte 38 bit `0x40` (3.1); `ldta@136` is the light type. Their properties sit in the same
groups as on other layers, with these conventions:

| What | Path | Stored | Evidence | Conf. |
|---|---|---|---|---|
| camera Position, default (auto state) | `ADBE Transform Group/ADBE Position`, `tdb4` 76 and 77 = 1 although no expression exists | `[0, 0, -2666.67]` (x and y are 0 whatever the composition size; z = -zoom). A **one-node** camera reports `[0, 0, -2666.67]` (251). **Correction (S16): a reopened two-node camera reports `[0, 0, -2666.67]` too (256 read back), and After Effects renders it there**: the default two-node camera of 537 above the 3D solid of 486 renders from `[0, 0, -2666.67]` towards the point of interest, not like the composition's default view (537 differs from 486 in 24 % of the grid points, and EffectCraft's import with the stored position matches 537 but for 4 edge points). The `[960, 540, ...]` of the batch-3 notes was not reproduced (535 and 537, created by script, read back the stored value too); `CameraPosition::reported` keeps the old rule and is marked as contradicted | 256, 251, 537 | V |
| camera Position, set by the user | same, `tdb4` 76 and 77 = 0 | the absolute value `[100, 200, -1000]` | 253 | V |
| camera point of interest | `ADBE Anchor Point` | `[960, 540, 0]` in **pixels** (not a fraction); the record's `tdsb` is 1 on a two-node and 3 on the one-node camera (not asserted) | 251, 253, 256 | V (value), E (`tdsb`) |
| light Position, default | same as the camera | `[0, 0, -1333.33]`, as After Effects reports it (255 read back); the point of interest `[960, 540, 0]` | 254, 255 | V |
| light intensity | `ADBE Light Options Group/ADBE Light Intensity` | f64 50.0 percent, `tdsb` 1; absent at the default 100 | 254 (50), 252 (absent) | V |
| light type | `ldta@136` | 1 spot, 2 point | 254, 252 | V |
| **camera Zoom** | `ADBE Camera Options Group/ADBE Camera Zoom` | plain pixels (`tdsb` 1), absent at the default 2 666.67; the auto Position keeps `[0, 0, -2666.67]` and is not recomputed from the zoom | 341 (2000) | V |
| **camera depth of field** | same group: `ADBE Camera Depth of Field` 1.0 (checkbox descriptor `ffff0004`), `ADBE Camera Focus Distance` 800, `ADBE Camera Aperture` 30 (plain numbers), all `tdsb` 1. **Defaults (256 read back): Depth of Field off, Focus Distance = Zoom = 2 666.67, Aperture 25.309 336 332 958 4, Blur Level 100** | 342, 256 | V |
| **light Casts Shadows** | `ADBE Light Options Group/ADBE Casts Shadows` | a set record (`tdsb` 1) holding 0.0 (off) in every light | 252, 254, 255 | V |
| **3D rotations** | `ADBE Rotate X`, `ADBE Rotate Y` | plain degrees, absent at 0 | 486, 487 | V |
| **Orientation** | `LIST otst` = `tdbs` + `LIST otky/otda`: `otda` is three big-endian `f64` degrees; absent at `[0, 0, 0]`. Keyed orientation is not sampled | 488, 485 | V |
| **light colour** | `ADBE Light Options Group/ADBE Light Color` | ARGB doubles 0..255 (`[255, 255, 0, 0]` for red; 4 components, descriptor `0002ffff`), absent = white | 343 | V |
| **light cone** | same group: `ADBE Light Cone Angle` 60 and `ADBE Light Cone Feather 2` 30 (percent), each `tdsb` 1 and absent in the default spot light (90 and 50). Every point light stores them as `tdsb` 3 records with 90 and 50 (a point light created directly equals the converted one of 252, so the "odd" property set is not a conversion artefact) | 344, 345, 346, 252 | V |

A point light made by converting a spot light has a different property set (`Orientation`, `Rotate
X/Y/Z`, `Cone Angle`, `Cone Feather 2` in the default state) that a spot light lacks (252); whether that
is an artefact of the conversion is untested. API: `Layer::camera_position`,
`CameraPosition::reported`, `Layer::camera_point_of_interest`, `Layer::light_intensity`,
`Layer::{camera_zoom, camera_depth_of_field, camera_focus_distance, camera_aperture, light_color,
light_cone_angle, light_cone_feather}`, `DEFAULT_CAMERA_ZOOM` (2 666.67 is verified for the 1920 px
composition only), `Layer::orientation` (`Orientation`; test `orientation_is_otst_otda` in
`tests/batch5.rs`). Render checks against After Effects (20 px grid, 5 184 points): 486 1 point
off, 535 2, 536 0, 537 4 (anti-aliased edges); **488 (Orientation `[10, 20, 30]`): 272 points
(5.25 %) differ; the import carries the stored angles, and permuted or negated inputs match worse
(6.2 % to 34 %), so the cause is in EffectCraft's renderer and not isolated.** Tests: `tests/layer_switches.rs`, `tests/batch3_objects.rs`.

### 3.12a Project settings (top-level chunks; cases 620-634)

Each case is the base reopened and saved with one setting changed (`project.set_settings`), compared
with 620 (the base saved the same way); apart from the save noise (`head` byte 19, `nhed` / `nnhd`
bytes 20-27 / 28-35, `fdta`) only the bytes below changed.

| setting | where | stored | cases | conf |
|---|---|---|---|---|
| colour depth | `nhed` byte 15 (and `nnhd` byte 24) | 0 = 8, 1 = 16, 2 = 32 bits per channel (the base is 32) | 620, 621, 622, 631 | V |
| working space | `cpid` (16 bytes) and the `Utf8` right after `PwCs` | None: `cpid` all `0xff` and the JSON `{}`; a profile: JSON `{"baseColorProfile":{"colorProfileData":"<base64 ICC>","colorProfileName":"<name>"},"baseProfileType":2}` (`sRGB IEC61966-2.1` in the base, `Rec.709 Gamma 2.4` in 624) and `cpid` a 16-byte id of the profile; After Effects 26.2.1 leaves `colorProfileName` out in a fresh save (settings-c), and the name is then the profile's ICC `desc` tag (v2 `desc` or v4 `mluc`) | 620, 623, 624, 631, 902 | V (None, the name), ? (`cpid`) |
| working gamma | `dwga` `u32` @0 | 0 = 2.2, 1 = 2.4 | 625, 631 | V |
| Linearize Working Space | `lnrp` (1 byte, 1) inserted after `cpid` | absent when off | 626, 632 | V |
| Blend Colors Using 1.0 Gamma | `lnrb` (1 byte, 1) inserted after `cpid`, before `lnrp` | absent when off | 627, 632 | V |
| Compensate for Scene-referred Profiles | `acer` byte 0 | 1 on (base), 0 off | 628 | V |
| time display | `nhed` (and `nnhd`) byte 8 | 0 timecode, 1 frames; feet + frames: byte 8 = 1 and byte 11 = 1, byte 13 = frames per foot (16 = 35 mm, 40 = 16 mm; the base holds 16) | 629, 633, 634 | V |
| frame count start | `nhed` byte 14 (`nnhd` byte 20) | 2 for a count from 0 (base), 1 for a count from 1 | 630 | E (one value each) |

The audio sample rate of the project is `adfr` (`f64` 48 000 in every case, not varied). API:
`ProjectSettings::read` (`TimeDisplay`, `WorkingSpace`). Test: `tests/it/container/cases_600.rs`. The
fixture of 624 is not in the repository: its working-space JSON holds the profile data After
Effects embeds for that space.

### 3.12b Layer styles (`tdgp[ADBE Layer Styles]`; cases 950-980)

Every layer's root group holds `ADBE Layer Styles` (third, after Time Remapping and Transform), also
when no style is used (950). Inside: `ADBE Blend Options Group` (Global Light Angle `ADBE Global
Angle2`, Altitude `ADBE Global Altitude2`, and `ADBE Adv Blend Group` with `ADBE Layer Fill
Opacity2`, `ADBE R/G/B Channel Blend`, `ADBE Blend Interior`, `ADBE Blend Ranges`; 977), then one
switch group per style, `<style>/enabled` (`dropShadow`, `innerShadow`, `outerGlow`, `innerGlow`,
`bevelEmboss`, `chromeFX` = Satin, `solidFill` = Color Overlay, `gradientFill` = Gradient Overlay,
`patternFill`, `frameFX` = Stroke).

| field | layout | evidence | conf |
|---|---|---|---|
| applied | the switch group's `tdsb` 1 (2 = not applied); with the first style Layer Styles and Blending Options `tdsb` 3 -> 1; nothing else in the layer changes | 950-960 | V |
| switched off | the switch group's `tdsb` 0 (bit 0 is the eye, as on shape items): the style keeps its records and is not drawn; After Effects keeps 0 when it saves (988: a patched copy of 952, render = 950; the eye is not scriptable) | 988 | V |
| switch state 3 | not sampled (presumably bit 1 = not applied; the removal is not scriptable: `property.remove` refuses a non-indexed group, session R4) | - | ? |
| records | a parameter has a `tdbs` only when it differs from After Effects' default (defaults: the read-back of 951), in property order (`<style>/<param>`) | 951-977 | V |
| scalars | the value in the units shown: opacity, noise, choke / spread, range, jitter, depth, scale in percent, distance and size in pixels, angles and altitude in degrees; class (8, 9) with `tdum` / `tduM`, angles and altitude (4, 6) without | 966-977 | V |
| colours | ARGB 0..255, class (1, 1), 4 components | 967, 970, 977 | V |
| popups, checkboxes | class (4, 4), 1-based / 1.0 when on. Blend Mode (`mode2`, `highlightMode`, `shadowMode`): Photoshop's menu with its separators, 1 Normal, 2 Dissolve, 4 Darken, 5 Multiply, 6 Color Burn, 7 Linear Burn, 8 Darker Color, 10 Lighten, 11 Screen, 12 Color Dodge, 13 Linear Dodge, 14 Lighter Color, 16 Overlay, 17 Soft Light, 18 Hard Light, 19 Vivid Light, 20 Linear Light, 21 Pin Light, 22 Hard Mix, 24 Difference, 25 Exclusion, 27 Hue, 28 Saturation, 29 Color, 30 Luminosity, 32 Subtract, 33 Divide (3, 9, 15, 23, 26, 31 render Normal); Inner Glow Source 1 Edge, 2 Center; Stroke Position 1 Outside, 2 Inside, 3 Center; Bevel Style 1-5 Outer Bevel, Inner Bevel, Emboss, Pillow Emboss, Stroke Emboss (2 = default Inner Bevel); Color Type, Technique, Bevel Technique and Direction, Gradient Style 1-based in menu order | 978 (mode sweep rendered at 16 bpc), 979, 980, 977 | V (modes, Source, Position), E (the rest) |
| Gradient Overlay Offset | 2 components as shown, class (4, 6) | 977 | V (stored), H (unit) |
| keys | the plain 48-byte records (D.2), `tdb4@5` 1 -> 0, `@68` 1 | 976 | V |
| expressions | as on effect parameters: `tdb4@120` 1, the text before `tdum` | read-back test s1 (`export-tests\styles\s1`) | V |
| gradients | `<style>/gradient` is a `GCst` list, not decoded | - | ? |

At 32 bpc After Effects renders the modes 6, 7, 11, 12, 16-22 and 25 of a style as Normal (978);
at 16 bpc as their formulas. API: `effectcraft_aep::style_map` (table shared by importer and exporter).

### 3.13 What the tests cover

`tests/header.rs`, `values.rs`, `keyframes.rs`, `layer_switches.rs`, `effects.rs`, `effects_batch4.rs`, `effects_exposure.rs`, `masks.rs`,
`shapes.rs`, `text_markers_footage.rs`, `batch3_core.rs` and `batch3_objects.rs` compare the decoded fields with the manifest values of the
cases listed above; `typed.rs` checks the structural relations on every property and every list of
every fixture; `robustness.rs` runs the whole typed view over damaged copies of the decoded fixtures
(byte flips, truncation, damaged chunk bodies, every component count and template flag of the
keyframe header, damaged marker, layer-reference and footage chunks, every accessor of batch 3
including ease components with out-of-range indices) and over extreme values (zero and maximal
stretch ratios, display start and sequence durations).

## 4. Not decoded

Fields that the corpus leaves open (**?**), only suggests (**H**), or shows without a manifest value to
assert against (**E**, not exposed), and areas that were not sampled. None of these is decoded by the
crate; the raw bytes stay reachable through the chunk tree. Batch 3 settled most of the previous
list (explicit defaults and the Gaussian Blur default, the ease layout of several components, the
bezier colour distance, stretch and fractional times, Time Remapping, Layer Control None, the
dropdown, pixel aspect / shutter phase / display start, mask modes and the mask counter, polystar /
gradient / two-rectangle shapes, right justification, faux styles, caps, leading, camera zoom and
depth of field, light colour and cone, marker label and cue data, audio and sequence footage).

The fixtures are the small files of the corpus: as 369 single-case `.aep` files they used 31.4 MB on
disk (and the five `.aepx` 0.9 MB); combined into one project per family (16 files, and `fx-switch.aep`)
they use 27.2 MB in the corpus. In a combined file only the first instance of an effect
type keeps its parameter definitions (later ones read the project copy), layer and item ids are
the file's, and project settings are per file. Cases that exist in the corpus but have no fixture: 200 and 201 (the save-noise pair), 207 (Rotation keys), 209 (Rectangle Size keys), 244 (point text reference), 250 (comp marker at 2 s),
255 (default spot light, identical to 121), 257 and 258 (precomps of a 640 x 360 solid and of a
640 x 360 composition), most of batch 1 (011, 020, 036, 045, 050, 051, 061, 070, 072, 074-076, 078,
079, 092, 100-105, 111, 112, 120, 121, 125) and of batch 3: 308 (the 100x100 solid on its own,
309 and 310 contain it), 312-315 (no-op sets of Opacity, Anchor, Scale and Rotation: the layer tree is
identical to 299, only the session counters differ), 322 (byte-identical to 320), 346 (a point light
created directly, identical to 252 apart from the layer id) and 353 (354 without the remap). What they
establish is listed in the tables above with their case numbers; the tests do not assert it. Batch 4
without a fixture: 355 and 367 (unedited saves), 358-360, 362 (more Tint colours and amounts), 365 (Tint
above Gaussian Blur; 366 is the moved stack), 370, 372, 373, 375, 376, 379 and 381 (more Ramp points,
colours, scatter and blend values), 388 (unedited save) and 392 (Exposure -2.25: like 391 with
another value).

**Layer header (`ldta`)**

- Quality Wireframe, a track matte mode with matte layer id 0, frame blending and `autoOrient` on non-camera layers (not sampled); a parent that is not the first layer; layer names of 40 characters and non-ASCII names (a name longer than 31 characters: **?**, `ldta@64` holds 32 bytes); blend modes beyond the six known (Dissolve, Darken, Lighten, Color Dodge, Hue, ...); a negative start time; a stretch of 33.3 % (non-terminating ratio) and -100 %; a stretch on a layer that has keyframes (whether their times are rescaled); in / out points beyond the source duration.
- Bytes 56 (1 on AV, shape and text layers, 0 on camera and light; meaning **?**), 96 (2 on AV layers, **?**) and the camera extra at 148 / 152 (a double that equals the 36 mm film size in points: **E** for the value, **H** for the meaning). The start-time denominator of layers made by the API against the precomp operation (258 against 257: 25 600 against 1, **V** in the corpus, no fixture).

**Composition header (`cdta`, `PRin`)**

- Bytes 0 (**?**), 16 (600: **H**, the 1/600 s legacy time base), 24, 32, 40 (cache-like: they hold values the session used earlier, do not use them: **E**), 48 (**E**), 176 and 184 (360 in all files: **?**, probably the maximum shutter angle), 196 and 200 (128 and 16: **?**).
- Not sampled: a 3840 x 2160 composition, other pixel aspect fractions (1.2121, 1.4587), the preserve-frame-rate and preserve-resolution switches, draft 3D, `Hide shy layers`, frame blending, the 8-versus-16-bit project depth with a coloured background, the display start at 30 fps and with drop frame (whether the denominator at 168 is always the timescale).
- `PRin/prda` (12 bytes for Classic 3D, 52 for Advanced 3D: renderer parameters, **E**).
- Item `idta` byte 23 (0x20 composition and sequences or clips, 0x30 stills and solids, 4 audio, 0 folder; needed: zeroed, renders differ) and `idpc` (**V** in the corpus: 1 when the composition contains a nested composition), `head`, `nhed`, `nnhd`, `Ewst`, the `f*` records, `Gide`, `PTRE`, `LRdr` and `LSIf` bodies. `head` bytes 4 to 7 are the version the file is written for and `svap` the version that wrote it (section 1, "Versions"); `head` bytes 16 to 19 are the save counter (a big-endian `u32`; only its low byte changed between most saves, `314` shows the carry) and are the only bytes of the container that change in a Save As without an edit; item `idta` bytes 80 to 83 are the modification time (seconds since 1904-01-01); `nhed` / `nnhd` hold the id of the After Effects session (six sessions seen); `mkif` byte 8 is a running mask counter; `pgui` chunks are random GUIDs. All of that is edit-history or session state (**V** in the corpus) and is not decoded, except the mask counter.

**Properties and keyframes**

- `tdb4` bytes 4, 8..11 (a descriptor per property class: **E**; needed, without it keys are dropped), 16 (0.0001: **E**), 24 (the composition pixel aspect on spatial properties: **E**), 32..48 (1.0: **E**; 16..55 are needed by roving keys: zeroed, After Effects reads a roving key back at time 0), 56..67 (a class pair: **E**), 72..78 except 76 and 77 (**E**), 79 (**E**); `tdsb` bits other than 0x1 and 0x2 (group flags `0x401`, `0x11`, ...) and the `tdsb` 01 / 03 of the Vector Materials Group; `tdum` / `tduM` (**?**).
- Per-dimension ease values of a colour or a 2-D key (Scale is **V**, aep-export.md R13); a colour key with a non-zero ease (a colour has one ease entry; the sample reports 0 / 0); the ease units of a keyed effect Point (the 2-D spatial record itself is **V**, 382, with linear keys only).
- The 8 stale bytes of the marker record and the bytes after the common fields of the mask path record (uninitialised memory).
- Expressions in effect parameters, multi-line and non-ASCII expressions, the ExtendScript engine.
- Time Remapping with extra keys, with a stretch, on a nested layer; whether keys are rescaled by a stretch.

**Effects**

- `pard` byte 48 (flags: **?**), 68 / 72 (the point default as a percent pair: **V** in the corpus for 074 and 368, not exposed), 120 (the float slider default again as f32: **V**, not exposed), 124 (display decimals: **H**), 132..147 (fixed slider slider-max, default copy and flags; the low bit of @144 is set on the two percent-displayed sliders and clear on Scatter: **H** "display as percent"), the colour default copy at @60, and the stale memory after the parameter name. The fixed slider valid minimum is not decoded (the float slider one is, 389).
- Puppet (`ADBE FreePin3`, 3.7): the hidden mesh data, a pin's Vertex Index and Vertex Offset (its rest place), the unit of a pin's Position, the Overlap, Starch, Advanced and Bend pins, Density, Mesh Rotation Refinement and the Puppet Engine: no sample (**?**).
- Effects renamed, or stacked twice on ONE layer (`Gradient Ramp 2`: does the second keep an empty `parT`?); which instance keeps the full `parT` when a second instance is created on a layer below the first; expressions on effect parameters; keyed per-channel Exposure values and a keyed checkbox; Curves / Levels (arrays), Fractal Noise, and whether the `tdpi` of a Layer Control holds an id or an index after the referenced layer is reordered or deleted.

**Masks, shapes**

- Masks: locked, colour, feather falloff, rotobezier, keyed expansion, a path whose last vertex has only handles, the counter after a mask is deleted, a mask on a scaled layer (whether the box stays layer-relative), an animated path with three or more keys.
- Shape layers: open shape paths (in shape groups), stroke dashes, taper and wave with values, merge paths, repeater, offset paths and the other path operators, item blend modes and composite order, shape direction, gradient colour stops other than the default (**?**: they appear only as an `ADBE Vector Grad Colors` entry), highlight length and angle other than 0, gradient scale and rotation; why a radial gradient writes its highlight records with `tdsb` 1 and a polygon its inner radius with `tdsb` 3 (**?**; the polygon's records hold the defaults 50 and 0, as every non-effect record in the default state does: 3.4, state).

**Text**

- A real bold / italic font (the font-list entry against the faux flags, and why faux bold prepends a font-list entry: **?**), control characters other than U+0003 (**?**), style slots `/20 /38 /78 /87 /88` and paragraph slots `/31 /32` (no scriptable setting changes them: **?**), the meaning of paragraph `/24 /34 /35` and style `/68` (**?**, neutral for the sampled values), per-character offsets, vertical left-to-right text, text on a path, non-BMP characters, text animators and range selectors, source-text keyframes, and the roughly 700 other leaves of the dictionary.

**Camera, light**

- Camera focal length, angle of view and film size; the defaults of Focus Distance and Aperture (**?**, absent until depth of field is touched) and whether the default Zoom follows other composition widths (**H**: 50 mm on a 36 mm film gives width x 50 / 36); f-stop and blur level; a one-node camera with an animated point of interest; iris shape and rotation.
- Light parallel and ambient (`ldta@136` = 3, 4?), intensity 150 %, falloff radial and distance, casts shadows with darkness and diffusion, keyed colour; 3D layer orientation and X / Y rotation; the Material Options group (default in every sample).

**Markers, footage, precomps**

- Markers: a non-ASCII comment, a composition marker with a cue type (`SecL`), property-level markers, a protected region; the three pad bytes after the label byte.
- Footage: the audio words of `sspc` (`u16` @96 = 3, @98 = 4, @100 = 1, `u32` @156 = 8, set only for the 16-bit mono WAV: bits per sample, channels, sample format: **?**), stereo, 24-bit and 44.1 kHz WAV, the image sequence descriptor (`u32` @168 = 2, @172 = 1, @176 = 3, @180 = 3 and bytes @184..187 = `01 00 01 01`: first number, last number, digits, count: **?** with one sample), a sequence with another start number, four digits or a conformed frame rate, a missing file, footage inside a project folder, alpha and pixel-aspect interpretation, loop, a solid with alpha, `sspc` bytes @112, @114, @209..@212; `mnfo` does not hold the sequence frame rate.
- Precomps whose size, frame rate, duration or start differ from the parent (whether `ldta@12` / `@16` change), a time-remapped nested layer, collapse transformations, a precomp inside a precomp, a stretched layer inside a precomp (whether the property-time-scale rule follows).
- Save bookkeeping: whether the 1 748 bytes of XMP history per saved file are two history events or one, and whether the `head@16` steps have structure beyond "increases".

## 5. Method

The format is established only from files we generate and compare, never from another program's
parser or from Adobe's code.

1. **Generate.** A base project is built once. For each case exactly one thing is changed through
   After Effects' own scripting interface (a layer switch, a composition property, a static value,
   a keyframe, an effect, a mask, a text edit), the project is saved as `NNN-name.aep`, the change
   is undone and the base state is re-checked before the next case. A manifest lists every case
   with the exact change and the values used; the same state is also saved as the XML flavour of
   the project, which names the chunks and can be used as a cross-check. Batch 2 repeated this in
   later sessions, each case again from the same loaded base, and added two saves without an edit
   to isolate what a save changes.
2. **Diff.** A walker (our own code) parses each file into the chunk tree, and each case is
   compared with the base and with its parent case: chunk path, size and the offsets and bytes
   that changed. Save noise (a save counter, time stamps, the growing XMP history, per-session
   ids) is filtered. Because one change produces a handful of chunk differences, the field that
   holds a value is found by the byte offsets, and checked by changing the value (`040` against
   `041`, `055` against `051`, `202` against `203`).
3. **State a hypothesis, then verify it.** A statement enters this document only when the manifest
   value and the decoded value agree in the case files (the table above names them) and the same
   relation holds on every other file where it applies. A decoder script outside this repository
   checks every layout against the manifest values; the tests in `crates/aep/tests/` are the
   verification inside it: they decode the fixtures and compare with the manifest values (layer
   switches, in / out / start times, composition size, frame rate, work area and background,
   Position `100, 200, 0`, Scale 50 %, key times and ease, the interpolation sides, anchor
   fractions of two solid sizes, Slider 77, Blurriness 40, point, angle and checkbox values, mask
   vertices and tangents, mask modes, shape sizes and colours, text and fill colour, text runs,
   marker times and strings, camera positions, light type and intensity, solid colours), and check
   the structural relations on every chunk of every fixture. A field with several cases that agree
   is marked V in section 3; a field seen only in the case diffs is marked E.
4. **Skip what is not verified.** Anything without an agreeing manifest value, or with a
   conflicting case, stays undecoded (section 4).

The corpus itself and the helper scripts live outside this repository. Projects that were not made
for this purpose were opened locally only to test the reader's robustness and the importer's
coverage (does everything parse, which chunk ids occur); no layout in this document comes from them
and none of them, nor any content of them, is in the repository. The
cases above are the contributor's own test data, kept outside the repository (see `ATTRIBUTION.md`); the
repository holds no Adobe sample project, preset or asset.
