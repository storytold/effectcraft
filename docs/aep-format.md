# After Effects project format (`.aep`)

> **Last reviewed:** 2026-10-13 · **Last updated:** 2026-10-13 · **Change:** third round of verified field layouts (explicit-default semantics, type-major bezier ease, exact time stretch and in / out points, Time Remapping state, comp pixel aspect / shutter phase / display start, Point default, Layer Control None, dropdown selection, one `mkif` per mask, polystar and radial gradient records, text styles, camera and light options, marker cue data, audio and image-sequence footage) added to the typed view of `effectcraft-aep` · **Target:** Adobe After Effects 2026

What `crates/aep` ([`effectcraft-aep`](../crates/aep/README.md)) reads and writes, and the evidence
for each statement. Everything here was observed on project files we generated ourselves; no
source code, specification or format documentation of any other reader was used (every layout is
backed by our own cases, see section 5). This is the container and a set of
verified field layouts, not a project importer: turning a parsed `.aep` into an EffectCraft
document is future work (see [gaps.md](gaps.md) G4 and [file-format-parity.md](file-format-parity.md)).

Evidence is cited as **case numbers**: the files `NNN-*.aep` of our synthetic corpus, each made by
one scripted change to a base project. Batch 1 (cases 000 to 126) was made in one chain of
Save As; batch 2 (cases 200 to 259) was made in later sessions, each case starting again from the
same loaded base; batch 3 (cases 299 to 354) was made in three further sessions (299 is a Save As
without any edit, the reference for save noise). The base project `000-base` is one composition `C`
(1920x1080, 25 fps, 5 s) holding one red solid layer `L` (1920x1080) in the folder `Solids`. 151 of
these files are the test fixtures in `crates/aep/tests/fixtures/` (11.7 MB together; the small ones
of each group, the rest are named in section 4); the tests named below run on them. The container
of every fixture is byte-identical to the generated file, with two exceptions that keep the
fixtures small and free of local data: the XMP packet after the container (which grows by 1.7 KB
with every save of the corpus chain) is replaced by the 5.8 KB packet of the base project in all
fixtures but `001`, and in the fixtures that import a file (`126`, `350`, `351`, `352`, `354`) the
file path and machine name inside the footage alias JSON are replaced by neutral text of the same
length.

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

## 2. Typed view (`Project`, `Item`, `Comp`, `Layer`, `PropGroup`, `Property`, `KeyList`)

The structure the typed view is built on; each row has a test in `crates/aep/tests/typed.rs` against
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
| **stretch** | 8 / 108 | u32 / u32 | **time stretch as the exact ratio `@8 / @108`**: 1/1 = 100 %, 2/1 = 200 %, **1/2 = 50 %, 3/2 = 150 %**. In and out points stay unscaled; the keyframe time scale `tdb4@12` of every property of the layer is 25 600 x stretch for stretch >= 1 (51 200 at 200 %, 38 400 at 150 %) and stays 25 600 at 50 % | 000, 018, 316, 317 | V |
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
| **blend mode** | 99 | u8 | Normal 2, Add 4, Multiply 5, Screen 6, Overlay 7, Difference 26; any other code is kept raw. Each case changes only this byte | 000, 016, 225-228 | V (6 modes) |
| layer type | 128 | u32 | 0 AV (solid, footage, precomp, null), 1 light, 2 camera, 3 text, 4 shape | AV: 000, 022, 126; text: 110, 113, 243; shape: 238-242; camera: 251, 253, 256; light: 252, 254 | V |
| parent id | 132 | u32 | 0 = no parent | 000, 022 (L gets the id of the new null layer) | V |
| **light type** | 136 | u32 | 1 = spot, 2 = point (0 on every other layer); parallel and ambient were not sampled and are kept raw | 252, 254 | V |

API: `LayerHeader` (`is_video_enabled`/`is_enabled`, `is_audio_enabled`, `is_effects_enabled`,
`is_auto_oriented`, `light_type`, `blend_mode`, `stretch` / `stretch_den`, `stretch_ratio`,
`stretch_percent`, `in_point`, `out_point`); the same switches on `Layer`. `stretch` is now the
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

Every property's keyframe time scale (`tdb4` bytes 12 to 15) follows the composition (23 976 in every
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
| footage type tag | `Pin /sspc` | 22 | 4 ASCII | `Soli` for a solid, `png!` for a png | 000, 126 | V |
| footage size | `Pin /sspc` | 30, 34 | u32, u32 | width, height in pixels | 000 (1920 x 1080), 022 (100 x 100 null), 126 (8 x 8), 211, 212, 259 | V |
| footage name | `Pin /opti` | 26 (solid), 58 (file) | ASCII, NUL terminated | the item's own `Utf8` name is empty for footage | 000 (`L`), 022 (`Null 1`), 126 (`test8x8.png`), 211 (`S100`), 259 (`Blue`) | V |
| **solid colour** | `Pin /opti` | 10, 14, 18, 22 | 4 x f32 | ARGB, 0..1: red [1, 0, 0] is `1, 1, 0, 0`, blue [0, 0, 1] is `1, 0, 0, 1` | 000, 259 | V |
| file path | `Pin /Als2/alas` | key `fullpath` | JSON string | alias JSON of a file footage item | 126 (the fixture holds a neutral path) | V |
| precomp linkage | `Layr/ldta` | 40 | u32 | the layer's source id is the id of the precomp's item | 124 | V |

Footage items beyond solids and stills (`Pin /sspc`, 222 bytes; audio and image sequences, batch 3):

| Field | `sspc` offset | Type | Meaning | Evidence | Conf. |
|---|---|---|---|---|---|
| file type tag | 22 | 4 ASCII | `png!`, `Soli`, **`WAVE`** | 126, 000, 350 | V |
| width, height | 30, 34 | u32, u32 | 0 / 0 for audio; 64 x 64 for the sequence | 350, 352 | V |
| **duration** | 38 / 42 | u32 / u32 | seconds as `num / den`, unreduced: still 0/1, **WAV 48 000/48 000 (the sample rate is the denominator), image sequence 3/30 (the frame rate is the denominator)** | 126, 350, 352 | V |
| native frame rate | 56 | u32 | 30 for the sequence, 0 for stills, audio and solids | 126, 352 | V |
| bits per pixel | 60 | u32 | RGBA still 32, RGB sequence 24, 0 for solids and audio | 126, 352 | V |
| **sample rate** | 160 | f64 | 48 000.0 for the WAV | 350 | V |
| **file size** | 208 | u32 | bytes of the source file(s): png 149, WAV 96 044 (44 + 48 000 x 2), three png of 168 bytes 504 | 126, 350, 352 | V |

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
solid or footage item, or the composition of a precomp).

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
| expression flags | `tdb4` | 76, 77, 120 | u8 | all three are 1 when an expression exists. 76 and 77 are also 1 on a camera's or light's computed (auto) Position, which has no expression | 060, 213, 251 | V |
| **expression disabled** | `tdb4` | 119 | u8 | 1 = the expression is disabled; the text and bytes 76, 77, 120 stay. On the built-in view layers the byte flips by itself after the first save, so it counts only together with an expression | 060 (0), 213 (1) | V |
| state | `tdsb` | 0 | u32 | 1 = written (non-default value, keys or expression), 3 = default state | 040, 071, 073 (1); 3 on the unwritten properties of 000 | V (1), E (3) |
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
the tool that made the corpus applies one ease to all dimensions). `Keyframe::plain_ease_slots`
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

API: `Keyframe::{interpolation_in, interpolation_out, kind, value, ease, ease_component, ease_components, plain_ease_slots, color_distance, spatial}`;
`Property::keys()` hands the property's template to its records. Marker record (16 bytes): time,
`03 03` hold bytes, 8 stale bytes. Mask path record (64 bytes): the common fields only. Test:
`tests/values.rs`, `tests/keyframes.rs`.

### 3.6 Expressions

| Field | Chunk path | Meaning | Evidence | Conf. |
|---|---|---|---|---|
| source | `tdbs/Utf8`, after `cdat` / `list` and before `tdum` | raw UTF-8 text, no length prefix, no NUL, no engine tag | 060 (`wiggle(1,10)` on Opacity) | V |
| flags | `tdb4` 76, 77, 120 | all 1 when present, all 0 otherwise | 060, 040 | V |
| **disabled** | `tdb4` 119 | 1 when disabled (`expression.disable_all`); the only byte that differs between 060 and 213 | 060, 213 | V |
| static value | `cdat` | kept while an expression exists (Opacity 1.0) | 060 | V |

The expression engine is project-wide (`LIST ExEn/Utf8`, `javascript-1.0`). API:
`Property::expression_enabled`. Test: `tests/keyframes.rs`.

### 3.7 Effects (`tdgp[ADBE Effect Parade]/sspc`)

| Field | Chunk path | Offset | Type | Meaning | Evidence | Conf. |
|---|---|---|---|---|---|---|
| match name | the `tdmn` before the `sspc` | | text | `ADBE Slider Control`, `Pseudo/@@S1iPXtJFTHK4QaDbgJvLhw`, ... | 071, 073, 077, 080, 214 | V |
| instance name | `sspc/fnam/Utf8` | | text | `Slider Control` | 071, 073, 077, 080 | V |
| parameter count | `sspc/parT/parn` | 0 | u32 | including the implicit input layer and the built-in entry (3 for a slider) | 071 | V |
| parameter match name | `parT/tdmn` | | text | `<effect match name>-NNNN`; the last is `ADBE Effect Built In Params` | 071 | V |
| parameter type | `parT/pard` | 12 | u32 | 0 layer, 2 fixed slider (16.16), 3 angle, 4 checkbox, 5 colour, 6 point, 7 popup, 9 no data, 10 float slider, 12 path | 071, 073, 077, 080, 214, 215 | V |
| parameter name | `pard` | 16 | 32 ASCII | `Slider`, `Color`, `Menu`, `Blurriness`, `Shadow Color`, ... (empty for some checkboxes) | same | V |
| **default** | `pard` | 56 | by type | float slider: f64 (Slider 0, **Blurriness 25**); fixed slider and angle: 16.16 (drop shadow opacity 127.5, direction 135, distance 5, softness 0; Fill opacity 1.0 = 100 %); **colour: ARGB bytes (drop shadow `ff000000` black, Fill `ffff0000` red)**; popup: u32 index (1) | 071, 077, 080, 214, 215 | V |
| popup items | `parT/pdnm/Utf8` | | text | items separated by `\|` | 077 (`Item 1\|Item 2\|Item 3`), 214 | V |
| stored value | `sspc/tdgp/<match>-NNNN` | | `tdbs` | present only for a parameter that differs from its default; `tdsb` 1, `cdat` as in 3.4. **Absent record = the `pard` default** | see below | V |

Stored values by type (all `tdsb` 1):

| Type | Stored | Evidence | Conf. |
|---|---|---|---|
| float slider | `cdat[0]` as set (Slider 77, Blurriness 40); a keyed slider has 48-byte plain records and no `cdat` | 071, 216, 221 | V |
| colour | ARGB doubles 0..255 | 073; keyed: 210 | V |
| **checkbox** | `cdat[0] = 1.0` when on; off is the default and has no record (`tdb4[8..11]` = `00010004`). A checkbox parameter without a record reads as off | 217; the Repeat Edge Pixels checkbox of 214 | V |
| **point** | two doubles, a **fraction of the layer size**: `[111, 222]` is `[111/1920, 222/1080]` (`tdb4[3]` = 2, `[5]` = 15). **The default is two 16.16 fractions 0.5 in `pard` @56 and @60, relative to the layer: a Point Control on a 100x100 solid reads back [50, 50]** | 218, 310 | V |
| **angle** | plain degrees: 90 is `90.0` | 219 | V |
| **layer control** | the `-0001` record always exists (`cdat` 0); the sibling `tdpi` (u32) is the **id of the chosen layer** (the new Null, `0x22`). **The default ("Layer 1" on the first layer) and an explicit 1 are the owning layer's own id (320); "None" is `tdpi` 0 (321).** The manifest's "None" for the default (076, 220 rows) was wrong. In the implicit first parameter `tdpi` is the id of the layer that owns the effect | 220, 320, 321 | V |
| **dropdown** | the items are in `pdnm` (`Alpha\|Beta\|...`), `pard` u16 @60 is the item count (5), `pard` @56 the selection (1 by default). Selecting item 3 adds a value record (`cdat[0]` = 3.0, descriptor `00010004`) AND writes 3 into `pard` @56. Setting the items re-creates the effect definition under a new match name (`Pseudo/@@KbKT0rCPQi2A0ytuyGLWSw`; display name `Dropdown Menu Control`) | 077, 323, 324 | V |

The default of Gaussian Blur: the default-only file is byte-identical to the one with Blurriness 25
and has no value record; `pard` holds 25 (f64 @56 and f32 @120); setting 40 adds a record and leaves
`pard` at 25, so `pard@56` is the default and not the current value (a popup selection is the one
exception, it overwrites `pard@56`). The corpus manifest's default of 0 was wrong: After Effects
reads a fresh Gaussian Blur as 25 (300, **V**; the conflict is resolved). API:
`EffectParam::{stored_value, pard_default, value, is_default, point_px, layer_ref, popup_item_count,
popup_selected_index, popup_selected_item}`, `ParamValue`, `LayerRef`. Tests: `tests/effects.rs`,
`tests/batch3_core.rs`, `tests/batch3_objects.rs`.

### 3.8 Masks and bezier paths (`tdgp[ADBE Mask Parade]`)

| Field | Chunk path | Offset | Type | Meaning | Evidence | Conf. |
|---|---|---|---|---|---|---|
| inverted | `Mask Parade/mkif` | 0 | u8 | | 090 (0), 091 (1) | V |
| **mode** | `mkif` | 4 | u32 | Add 1, Subtract 2, **Intersect 3, Lighten 4, Darken 5, Difference 6, None 0** (329: the same ellipse path as 232) | 090, 091, 234-237, 329 | V |
| name | `tdgp[ADBE Mask Atom]/tdsn/Utf8` | | text | `Mask 1` | 090 | V |
| feather, opacity, expansion | `ADBE Mask Feather`, `ADBE Mask Opacity`, `ADBE Mask Offset` | | see 3.4 | stored only when not at the default; feather can be keyed (plain n=2 records) | 090, 091, 208 | V |
| path property | `om-s[ADBE Mask Shape]/tdbs` | | | animated: `tdb4` byte 68 = 1 and a `list` of 64-byte records (time and common fields only) | 090, 093 | V |
| path shapes | `om-s/omks/LIST shap` | | | one per keyframe, one for a static path | 090, 093 | V |
| box | `shap/shph` | 4, 8, 12, 16 | 4 x f32 | left, top, right, bottom **normalised to the layer (source) size** for masks (560 / 1920, 340 / 1080, 1360 / 1920, 740 / 1080; on a 100x100 solid a 60x40 rectangle has the box 0.2, 0.3, 0.8, 0.7 and decodes exactly with the layer size, not the 1920x1080 composition). It is the box of the vertices **and** of the handles | 090, 232, 309 | V |
| **open path** | `shph` | 3 | u8 | bit `0x08` set = open path (`0x09` against `0x01`); byte 20 is 1 in every sample, open or closed | 233 against 090, 232, 234 | V |
| points | `shap/list` (`lhd3` record size 8) | | f32 x, y | three per vertex in the order **vertex i, out handle i, in handle i + 1**, normalised inside the box (u = (x - left) / (right - left)); the last record is the in handle of vertex 0. A **tangent is the handle minus the vertex**, without a sign change | 090 (rect), 093 (two rects), 232 (ellipse: tangents `0.5523 x radius`, `[-220.91, 0]`, `[0, -110.46]`, ...), 233 (open path, 3 vertices) | V |

**There is one `mkif` per mask**, placed directly before its atom (two masks: `mkif tdgp mkif tdgp`,
328), so mode and inverted are reported per mask. `mkif` byte 8 is the running mask id counter of the
**layer** in the session (1, 2 for the two masks of 328; 329 then gets 3; undo does not reset it; a new
layer starts at 1 again): exposed as `Mask::counter`, a session counter and not a stable id. API:
`Mask::{mode, inverted, counter}`, `Project::mask_vertices_px` (vertices in layer pixels). Tests:
`tests/masks.rs` (with a synthetic check of the handle order), `tests/batch3_objects.rs`.

### 3.9 Shape layers (`tdgp[ADBE Root Vectors Group]`)

A shape layer (type 4) holds `ADBE Vector Group` (user name `Group 1`) with `ADBE Vectors Group`
(the contents, in z-order, `tdsb` 0x401) and `ADBE Vector Transform Group`. Items are named by their
match name and carry their own user name. Colours are ARGB doubles 0..255. All sizes, positions and
path points are in pixels.

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
| font size | `RUN/6/0[n]/0/0/6/1` (100 by default) | 110, 113 (60) | V |
| fill colour | `RUN/6/0[n]/0/0/6/53/0/1` = `[A R G B]` floats 0..1 | 110 (white), 113 (`[1 1 0.5 0]`), 248 | V |
| **paragraph runs** | `RUN/5/0[n]/1` = characters including the `\r` (`A\rB\r`: `[2, 2]`; `Hello\r`: `[6]`) | 243, 245 | V |
| **style runs** | `RUN/6/0[n]/1` = characters per style run (`A\rB\r`: `[4]`). **A range style splits `Hello\r`: characters 1 to 3 red gives `[1, 2, 3]` with the fills white, `[1 1 0 0]`, white** | 243, 248 | V |
| **justification** | `RUN/5/0[n]/0/0/5/0`: 0 = left, **1 = right (334)**, 2 = centre; the scripting enum value of right (7414) is not what is stored; the justified variants are unknown (kept raw) | 245 (0), 246 (2), 334 (1) | V (0, 1, 2) |
| **box (paragraph) text** | frame dictionary `/0/8/0[0]/0`: `/1/0` = 32 floats = 16 x, y pairs tracing the rectangle (corners `[-200,-100] [200,-100] [200,100] [-200,100]`), `/2/0` = 1; both keys are absent for point text. Alignment changes only the glyph geometry, not the frame | 245, 246 (box 400 x 200, top-left `[-200, -100]`), 247 (point text: absent) | V |
| **tracking** | `RUN/6/0[n]/0/0/6/8` = 0, 100 in 247 | 247 | V |
| **faux bold / faux italic** | style slots `.../6/2` and `.../6/3`, `false` -> `true`; the font style stays Regular. **Side effect: the font list `/0/1/0` gains a new first entry (4 -> 5 fonts) with property `/3` = 2 (bold) or 1 (italic), the old entries shift up and the style's font index `/6/0` becomes 1** | 335, 336 | V |
| **caps** | style slot `.../6/12`: 0 normal, **1 small caps, 2 all caps** | 337, 338 | V |
| **leading** | style slot `.../6/5`: automatic leading is stored as 98.0 (100 px Times New Roman), leading 60 is 60.0 | 339 | V |
| **last run** | the last style run reaches the paragraph end and includes the final `\r` (two range styles on `Hello`: runs `[1, 2, 3]`, fills white / `[1 1 0 0]` / `[1 0 0 1]`, sizes 100 / 100 / 60) | 340 | V |
| font | `.../6/0` = index into the font list `/0/1/0[i]/0/0/0` (`TimesNewRomanPSMT`, ...) | 110, 335 | E |

API: `TextDocument` (`paragraphs`, `style_runs`, `text_box`, `paragraph_texts`), `StyleRun::{faux_bold,
faux_italic, caps, leading, font_index}`, `Caps`, `Justification::Right`. Tests:
`tests/text_markers_footage.rs`, `tests/batch3_objects.rs`.

### 3.11 Markers

`mrst[ADBE Marker]` holds a `tdbs` with a `list` of 16-byte records (the times; `tdb4` byte 12 is the
time scale) and one `LIST mrky/LIST Nmrd` per marker. Layer markers live in the layer's own tree,
composition markers in the `SecL` list named `Markers`.

| What | Chunk path | Meaning | Evidence | Conf. |
|---|---|---|---|---|
| time | the key list | 25 600 = 1 s | 122, 123, 249 | V |
| **duration** | `Nmrd/NmHd` (20 bytes: u32 4, u32 0, u32 duration, u32 600, u32 0) | duration / 600 = the marker duration in seconds (1 s is 600; 0 for none); 600 is the units per second | 249 (1 s), 122 (0) | V |
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
| camera Position, default (auto state) | `ADBE Transform Group/ADBE Position`, `tdb4` 76 and 77 = 1 although no expression exists | `[0, 0, -2666.67]` (x and y are 0 whatever the composition size; z = -zoom). A **two-node** camera reports `[960, 540, -2666.67]` (x, y = composition centre), a **one-node** camera reports `[0, 0, -2666.67]` | 256, 251 | V |
| camera Position, set by the user | same, `tdb4` 76 and 77 = 0 | the absolute value `[100, 200, -1000]` | 253 | V |
| camera point of interest | `ADBE Anchor Point` | `[960, 540, 0]` in **pixels** (not a fraction); the record's `tdsb` is 1 on a two-node and 3 on the one-node camera (not asserted) | 251, 253, 256 | V (value), E (`tdsb`) |
| light Position, default | same as the camera | `[0, 0, -1333.33]` | 254 | E |
| light intensity | `ADBE Light Options Group/ADBE Light Intensity` | f64 50.0 percent, `tdsb` 1; absent at the default 100 | 254 (50), 252 (absent) | V |
| light type | `ldta@136` | 1 spot, 2 point | 254, 252 | V |
| **camera Zoom** | `ADBE Camera Options Group/ADBE Camera Zoom` | plain pixels (`tdsb` 1), absent at the default 2 666.67; the auto Position keeps `[0, 0, -2666.67]` and is not recomputed from the zoom | 341 (2000) | V |
| **camera depth of field** | same group: `ADBE Camera Depth of Field` 1.0 (checkbox descriptor `ffff0004`), `ADBE Camera Focus Distance` 800, `ADBE Camera Aperture` 30 (plain numbers), all `tdsb` 1; the defaults of focus distance and aperture are unknown | 342 | V |
| **light colour** | `ADBE Light Options Group/ADBE Light Color` | ARGB doubles 0..255 (`[255, 255, 0, 0]` for red; 4 components, descriptor `0002ffff`), absent = white | 343 | V |
| **light cone** | same group: `ADBE Light Cone Angle` 60 and `ADBE Light Cone Feather 2` 30 (percent), each `tdsb` 1 and absent in the default spot light (90 and 50). Every point light stores them as `tdsb` 3 records with 90 and 50 (a point light created directly equals the converted one of 252, so the "odd" property set is not a conversion artefact) | 344, 345, 346, 252 | V |

A point light made by converting a spot light has a different property set (`Orientation`, `Rotate
X/Y/Z`, `Cone Angle`, `Cone Feather 2` in the default state) that a spot light lacks (252); whether that
is an artefact of the conversion is untested. API: `Layer::camera_position`,
`CameraPosition::reported`, `Layer::camera_point_of_interest`, `Layer::light_intensity`,
`Layer::{camera_zoom, camera_depth_of_field, camera_focus_distance, camera_aperture, light_color,
light_cone_angle, light_cone_feather}`, `DEFAULT_CAMERA_ZOOM` (2 666.67 is verified for the 1920 px
composition only). Tests: `tests/layer_switches.rs`, `tests/batch3_objects.rs`.

### 3.13 What the tests cover

`tests/header.rs`, `values.rs`, `keyframes.rs`, `layer_switches.rs`, `effects.rs`, `masks.rs`,
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

The fixtures are the small files of the corpus (the budget for all fixtures is 12 MB; they use
11.7 MB). Cases that exist in the corpus but have no fixture: 200 and 201 (the save-noise pair), 207 (Rotation keys), 209 (Rectangle Size keys), 244 (point text reference), 250 (comp marker at 2 s),
255 (default spot light, identical to 121), 257 and 258 (precomps of a 640 x 360 solid and of a
640 x 360 composition), most of batch 1 (011, 020, 036, 045, 050, 051, 061, 070, 072, 074-076, 078,
079, 092, 100-105, 111, 112, 114, 120, 121, 125) and of batch 3: 308 (the 100x100 solid on its own,
309 and 310 contain it), 312-315 (no-op sets of Opacity, Anchor, Scale and Rotation: the layer tree is
identical to 299, only the session counters differ), 322 (byte-identical to 320), 346 (a point light
created directly, identical to 252 apart from the layer id) and 353 (354 without the remap). What they
establish is listed in the tables above with their case numbers; the tests do not assert it.

**Layer header (`ldta`)**

- Quality Wireframe, track mattes, frame blending and `autoOrient` on non-camera layers (not sampled); a parent that is not the first layer; layer names of 40 characters and non-ASCII names (a name longer than 31 characters: **?**, `ldta@64` holds 32 bytes); blend modes beyond the six known (Dissolve, Darken, Lighten, Color Dodge, Hue, ...); a negative start time; a stretch of 33.3 % (non-terminating ratio) and -100 %; a stretch on a layer that has keyframes (whether their times are rescaled); in / out points beyond the source duration; the whole `ADBE Layer Styles` subtree.
- Bytes 56 (1 on AV, shape and text layers, 0 on camera and light; meaning **?**), 96 (2 on AV layers, **?**) and the camera extra at 148 / 152 (a double that equals the 36 mm film size in points: **E** for the value, **H** for the meaning). The start-time denominator of layers made by the API against the precomp operation (258 against 257: 25 600 against 1, **V** in the corpus, no fixture).

**Composition header (`cdta`, `PRin`)**

- Bytes 0 (**?**), 16 (600: **H**, the 1/600 s legacy time base), 24, 32, 40 (cache-like: they hold values the session used earlier, do not use them: **E**), 48 (**E**), 176 and 184 (360 in all files: **?**, probably the maximum shutter angle), 196 and 200 (128 and 16: **?**).
- Not sampled: a 3840 x 2160 composition, other pixel aspect fractions (1.2121, 1.4587), the preserve-frame-rate and preserve-resolution switches, draft 3D, `Hide shy layers`, frame blending, the 8-versus-16-bit project depth with a coloured background, the display start at 30 fps and with drop frame (whether the denominator at 168 is always the timescale).
- `PRin/prda` (12 bytes for Classic 3D, 52 for Advanced 3D: renderer parameters, **E**).
- Item `idta` byte 20 and `idpc` (**V** in the corpus: 1 when the composition contains a nested composition), `head`, `nhed`, `nnhd`, `Ewst`, the `f*` records, `Gide`, `PTRE`, `LRdr` and `LSIf` bodies. `head` byte 19 is the save counter and is the only byte of the container that changes in a Save As without an edit; item `idta` bytes 80 to 83 are the modification time (seconds since 1904-01-01); `nhed` / `nnhd` hold the id of the After Effects session (six sessions seen); `mkif` byte 8 is a running mask counter; `pgui` chunks are random GUIDs. All of that is edit-history or session state (**V** in the corpus) and is not decoded, except the mask counter.

**Properties and keyframes**

- `tdb4` bytes 4, 8..11 (a descriptor per property class: **E**), 16 (0.0001: **E**), 24 (the composition pixel aspect on spatial properties: **E**), 32..48 (1.0: **E**), 56..67 (a class pair: **E**), 72..78 except 76 and 77 (**E**), 79 (**E**); `tdsb` bits other than 0x1 and 0x2 (group flags `0x401`, `0x11`, ...) and the `tdsb` 01 / 03 of the Vector Materials Group; `tdum` / `tduM` (**?**).
- Per-dimension ease values: that dimension `i` uses slot `i` of each block (**E**; the tool that made the corpus applies one ease to all dimensions); a colour key with a non-zero ease (a colour has one ease entry; the sample reports 0 / 0); a keyed 2-D Point control (record size) and a spatial 2-D record.
- The 8 stale bytes of the marker record and the bytes after the common fields of the mask path record (uninitialised memory).
- Expressions in effect parameters, multi-line and non-ASCII expressions, the ExtendScript engine.
- Time Remapping with extra keys, with a stretch, on a nested layer; whether keys are rescaled by a stretch.

**Effects**

- `pard` byte 48 (flags: **?**), 68 / 72 (the point default as a percent pair: **E**), 104 / 108 (float slider valid minimum and maximum: **E**), 116 / 120 (slider range and default: **E**), 124, 128..147 (fixed slider ranges: **E**), and the stale memory after the parameter name. Minimum and maximum are not exposed.
- The stored units of fixed sliders (Fill Opacity as a record), effects that are switched off, renamed or stacked twice (`Fill 2`, `parn`, `EfdG` duplication), Curves / Levels (arrays), Gradient Ramp, Fractal Noise, and whether the `tdpi` of a Layer Control holds an id or an index after the referenced layer is reordered or deleted.

**Masks, shapes**

- Masks: locked, colour, feather falloff, rotobezier, keyed expansion, a path whose last vertex has only handles, the counter after a mask is deleted, a mask on a scaled layer (whether the box stays layer-relative), an animated path with three or more keys.
- Shape layers: polystar with points other than 5, rotation and roundness (inner and outer), a polygon with other points; ellipse position and rectangle roundness; open shape paths; stroke dashes, taper, wave; fill rule; merge paths; trim offset; repeater offset and composite; group scale, rotation, anchor, opacity, skew; several groups and nested groups; gradient colour stops (**?**, in no property of any sample), highlight values other than 0, gradient scale; why a radial gradient writes its highlight records with `tdsb` 1 and a polygon its inner radius with `tdsb` 3 (**?**).

**Text**

- Justified variants (3 to 6) of box text, a real bold / italic font (the font-list entry against the faux flags, and why faux bold prepends a font-list entry: **?**), stroke, per-character offsets, paragraph spacing and indents, baseline shift, vertical text, text on a path, non-BMP characters, text animators and range selectors, source-text keyframes, and the roughly 700 other leaves of the dictionary.

**Camera, light**

- Camera focal length, angle of view and film size; the defaults of Focus Distance and Aperture (**?**, absent until depth of field is touched) and whether the default Zoom follows other composition widths (**H**: 50 mm on a 36 mm film gives width x 50 / 36); f-stop and blur level; a one-node camera with an animated point of interest; iris shape and rotation.
- Light parallel and ambient (`ldta@136` = 3, 4?), intensity 150 %, falloff radial and distance, casts shadows with darkness and diffusion, keyed colour; 3D layer orientation and X / Y rotation; the Material Options group (default in every sample).

**Markers, footage, precomps**

- Markers: a duration that is not a whole second (checks the 1/600 unit), two markers (order of the record and `Nmrd`), a fractional time, a non-ASCII comment, a composition marker with label and cue type (`SecL`), property-level markers; the three pad bytes after the label byte.
- Footage: the audio words of `sspc` (`u16` @96 = 3, @98 = 4, @100 = 1, `u32` @156 = 8, set only for the 16-bit mono WAV: bits per sample, channels, sample format: **?**), stereo, 24-bit and 44.1 kHz WAV, the image sequence descriptor (`u32` @168 = 2, @172 = 1, @176 = 3, @180 = 3 and bytes @184..187 = `01 00 01 01`: first number, last number, digits, count: **?** with one sample), a sequence with another start number, four digits or a conformed frame rate, a video file with audio, a missing file, footage inside a project folder, alpha and pixel-aspect interpretation, loop, a solid with alpha, `sspc` bytes @112, @114, @209..@212; `mnfo` does not hold the sequence frame rate.
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

The corpus itself and the helper scripts live outside this repository. Larger production projects
were examined locally for statistics only (which chunk ids occur and how often); no layout in this
document comes from them and none of them, nor any content of them, is in the repository. Only the
151 fixtures above are committed, as the contributor's own test data (see `ATTRIBUTION.md`); the
repository holds no Adobe sample project, preset or asset.
