# `.aep` container — structural notes and provenance

This file is the written answer to the question raised on #251 by @bflatastic:

> *.aep has no public spec, and the chunk layout matches community reverse-engineering projects.
> Please state where the format knowledge came from.*

It is deliberately split into **what we derive** and **what we verify against**, because
`AGENTS.md` §2 draws a hard line at reading third-party decoder source while implementing a format.

---

## 1. Specification-tier sources

`.aep` has no published container specification. The *nearest* authoritative publication is Adobe's
own object model for project files, which documents **semantics** (what a precomp layer is, what an
item's type means) even though it says nothing about byte layout:

- Adobe, *After Effects Scripting Guide* — `Item`, `ItemType`, `AVItem`, `FootageItem`, `CompItem`,
  `FolderItem`, `Layer`, `AVLayer`, `LayerType`, `FootageSource`, `PlaceholderSource`, `SolidSource`.
- Adobe, *After Effects Expression Reference* — property match names (`ADBE Transform Group`,
  `ADBE Anchor Point`, `ADBE Vector Group`, `ADBE Text Properties`, `ADBE Effect Parade`).

These fix the **target model**: a project is a tree of items; an item is one of folder /
composition / footage; a precomp layer is an `AVLayer` whose `.source` is a `CompItem`. Our parser
must reproduce that model. They do **not** give offsets.

## 2. Where the byte layout comes from

Because there is no spec text, the layout is established by **black-box differential analysis of
files we author ourselves**:

1. Author a minimal project in After Effects. Save. Note the exact tree (Comps panel, layer list,
   layer types, source items).
2. Diff its bytes against a project that differs by **exactly one thing** (add one solid, rename one
   item, convert a layer to a precomp).
3. The byte range that changes is the field for that thing. Repeat.
4. Encode the finding as an assertion in the synthetic fixture for that feature (see
   [CHECKLIST.md](CHECKLIST.md) S1), so the claim is machine-checked forever after.

This is the same method the community parsers used, performed independently. The community
structural maps (Kaitai-style chunk lists, published in-tree schemas) are used **only as a
cross-check** — to confirm that an offset we found by differential probing lands where someone else
also observed it. Reading their *source* while implementing is out of bounds under §2; reading a
published *chunk name table* to check a hypothesis is not.

## 3. Container layout (as confirmed by our fixtures)

```
RIFX <u32 be filesize> Egg!
  LIST <len> <listtype>
     <chunkid> <u32 be len> <padding if len odd>
```

Big-endian throughout — **except** the small number of fields that are demonstrably little-endian
in practice (see §6). A `RIFF` (little-endian) container is accepted by the sniffer, but real After
Effects writes `RIFX`.

### Chunk identifiers observed

| FourCC | Meaning |
|---|---|
| `LIST` | Container; the four bytes after the header are the list type |
| `Fold` | Project root folder (list type) |
| `Item` | A project item (list type) |
| `Sfdr` | Sub-folder block; child `Item` lists live under it |
| `idta` | Item descriptor: **type** and **item id** |
| `cdta` | Composition descriptor: dimensions, frame rate, duration, background |
| `cmta` | Comment data |
| `fdta` | Folder data |
| `Layr` | An AV layer (list type) |
| `SLay` / `Sstl` | Solid-related layer chunk |
| `CLay` | Camera layer |
| `SecL` | Markers / section layer |
| `ldta` | Layer descriptor: **source item id**, quality, timing, attribute bits |
| `tdgp` | Property group stream (`tdmn` match name + `tdbs` value blocks) |
| `tdmn` | Match name (ASCII) |
| `tdbs` / `cdat` | A property's value block; `cdat` holds big-endian `f64`s |
| `Utf8` / `Utf1` | Item or layer name (UTF-8 / UTF-16BE) |
| `fnam` | Footage filename |
| `alas` | Asset link: JSON blob containing `"fullpath"` |
| `Pin ` | Footage pin block (**trailing space**; `Pin` and `pin` are distinct four-CCs) |
| `sspc` | Footage sequence descriptor: width, height, duration, frame rate |
| `opti` | Footage options; offset 4 holds the footage source kind |
| `iide` | Item id, stored **little-endian** |
| `nhed` | Project head; offset 15 holds bits-per-channel depth |
| `ExEn` | Expression engine string |

## 4. Item identity — the field that decides everything

`idta` carries the two values the whole import hangs off:

| Offset | Type | Meaning |
|---:|---|---|
| 0 | `u16` be | **Item type**: `0x01` Folder · `0x04` Composition · `0x07` Footage |
| 16 | `u32` be | **Item id** — the value layer references resolve against |

Fallback: `iide` holds the same id **little-endian** at its chunk offset 0. Treat it as a
cross-check, not a primary read.

`ldta` carries the layer's binding:

| Offset | Type | Meaning |
|---:|---|---|
| 4 | `u16` be | Layer quality (0 wireframe · 1 draft · 2 best) |
| 37..40 | `u8[3]` | Layer attribute bitfield (3D, solo, shy, lock, adjustment, collapse) |
| 40 | `u32` be | **Source item id** — the `idta` id this layer renders |

**A layer is a precomp layer if and only if its `ldta` source id resolves, through the project item
table, to an item whose `idta` type is `0x04`.** It is a footage layer if that item is `0x07`, and a
folder reference (invalid on a layer) if `0x01`. Nothing about the layer's `tdgp` content
participates in that decision — testing `ADBE Vector` / `ADBE Text Properties` first is what
currently misclassifies precomp layers as shape layers.

`cdta` (composition descriptor), from our fixtures:

| Offset | Type | Meaning |
|---:|---|---|
| 4 | `u32` be | Frame-rate divisor |
| 8 | `u32` be | Frame-rate dividend |
| 40 | `u32` be | Duration dividend (seconds) |
| 44 | `u32` be | Duration divisor |
| 48 | `u8[3]` | Background colour RGB |
| 136 / 138 | `u16` be | Width / height (version-dependent layout) |
| 140 / 142 | `u16` be | Width / height (other layout) |
| 152 | `u16` be | Frame rate, integer part |
| 156 / 158 | `u16` be | Frame rate, integer / fractional parts |

## 5. Footage sub-kinds

A `0x07` item's kind is in `opti` at offset 4:

| Value | Kind |
|---:|---|
| `0x09` | Solid — name is a padded string at offset 26 |
| `0x02` | Placeholder — name at offset 10 |

Dimensions and duration come from the `Pin ` → `sspc` block. Note the four-CC is `Pin ` **with a
trailing space**.

## 6. Known endianness / layout traps

- `iide` is little-endian inside an otherwise big-endian container. Read each field with the
  endianness its fixture confirms; do not assume uniformity.
- `cdta` frame rate appears both as a `u16` integer part with a `u16` fractional part (156 / 158)
  **and** as a `u32` (168) depending on the After Effects version that wrote the file. Prefer the
  integer+fraction pair; fall back to the `u32` only when the pair is out of range.
- Composition width/height sit at `cdta` 140 / 142 **or** 136 / 138 depending on writer version.
  The fixture set must pin this per version rather than guess.
- Chunk payloads are padded to even length (`len % 2`). Getting this wrong desynchronises the whole
  tree walk after the first odd-length chunk.

## 7. What this file is not

It is **not** a transcription of any third-party parser. It records what our own fixtures show, with
the community structural map used only to check that a hypothesis is not idiosyncratic to one file.
Where a claim here is not yet backed by a generated fixture, it is marked tentative and must not be
relied on — see [CHECKLIST.md](CHECKLIST.md) S1 for the fixture that will settle it.

