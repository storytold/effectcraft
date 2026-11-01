# effectcraft-pdf

Vector footage from PDF files, PDF-compatible Adobe Illustrator files (`.ai`, which are PDF
documents) and Encapsulated PostScript (`.eps`). Files parse into the same render tree as SVG
(`effectcraft_svg::Doc`), so they rasterise at any scale (Continuously Rasterize), convert to shape
layers (Layer ▸ Create ▸ Create Shapes from Vector Layer) and import as compositions with one
layer per file layer (File ▸ Import ▸ Composition). Composition - Retain Layer Sizes uses each
layer's artwork bounds and keeps its placement on the page. Images keep their rectangular bounds;
clipping paths constrain the bounds, while soft masks do not change them.

Specifications used: **ISO 32000-1:2008** (PDF 1.7; the public Adobe edition) for the file
structure, filters, content streams, graphics state, colour spaces, functions, shadings, text,
fonts, images, patterns, transparency (blend modes, soft masks) and optional content; Adobe
**Technical Note #5176** (The Compact Font Format Specification) and **#5177** (The Type 2
Charstring Format) for CFF font programs; **Adobe Type 1 Font Format** (version 1.1) for Type 1
font programs; the **PostScript Language Reference, third edition** and the **Encapsulated
PostScript File Format Specification 3.0** for EPS; **ITU-T T.4** (07/2003) and **ITU-T T.6**
(11/1988) for CCITT fax images. TrueType font programs are read through skrifa and JPEG (DCT)
images through zune-jpeg (both permissively licensed, pure Rust). PDF document/object loading uses the shared `craft-pdf` parser described below. The retained
drawing, PostScript, font and filter implementations were developed from the public
specifications without copying third-party implementations. Test files (including the CFF
and Type 1 fonts and the CCITT data) are generated in code ([`write`](src/write.rs) and the tests). One CCITT fixture was
produced once by libtiff's `tiffcp` (an external encoder used only as a fixture generator, like
ffmpeg elsewhere) to check the code tables against an independent implementation.

## What is read

- **Structure:** `craft-pdf` resolves cross-reference tables/streams and indirect objects into
  the app's existing object model. References retain object and generation numbers. A bounded
  compatibility repair recovers referenced objects from object streams when damaged xrefs omit
  them; every recovery or unresolved reference is reported in `Doc::skipped` (up to 64 messages).
  The existing page tree preserves inherited resources, MediaBox / CropBox and `/Rotate`.
  Content/image filters remain app-owned: Flate (with PNG predictors), LZW, ASCIIHex, ASCII85,
  RunLength and CCITTFax. Encrypted files remain refused; `page_count` returns zero when the
  reader refuses a document.
- **Content:** `q`/`Q`, `cm`, line width / cap / join / miter / dash, ExtGState (`CA`, `ca`, `LW`,
  `LC`, `LJ`, `ML`, `D`), all path construction and painting operators (non-zero and even-odd),
  clipping (`W`, `W*`) as clipping groups, colour in DeviceGray / RGB / CMYK, ICCBased (by
  component count), CalGray / CalRGB, Lab, Indexed, Separation and DeviceN (tint transforms of
  function types 0, 2 and 3), axial and radial shadings as pattern fills and through `sh`, form
  XObjects (matrix, bounding-box clip, nested resources).
- **Layers:** top-level optional-content marked sequences (`/OC … BDC`, Illustrator's layers)
  become the document's layers, named after their OCG; content outside them is gathered into
  layers "Layer N".
- **EPS:** a PostScript interpreter for the subset exported vector EPS uses: operand and
  dictionary stacks, procedures and `def`/`bind`/`load`/`where`, arithmetic, comparisons,
  `if`/`ifelse`/`for`/`repeat`/`loop`/`forall`, arrays and strings, `gsave`/`grestore`,
  `save`/`restore`, matrices (`concat`, `translate`, `scale`, `rotate`, `transform` …), path
  construction including `arc`/`arcn` and relative operators, `fill`/`eofill`/`stroke`,
  `clip`/`eoclip`, `rectfill`/`rectstroke`/`rectclip`, gray / RGB / CMYK / HSB colour. DOS EPS
  binary headers and `%%HiResBoundingBox` / `%%BoundingBox` are honoured; `%%BeginData` /
  `%%BeginBinary` / preview sections are skipped. **Text:** `findfont` / `findresource`,
  `scalefont`, `makefont`, `selectfont`, `setfont`, `currentfont`, `definefont` (re-encoded
  copies made with `forall` and a new `/Encoding`), `StandardEncoding` / `ISOLatin1Encoding`,
  `show`, `ashow`, `widthshow`, `awidthshow`, `xshow` / `yshow` / `xyshow`, `kshow` / `cshow`
  (the procedure is not run), `glyphshow`, `charpath` (glyph outlines join the path, e.g. for
  clipping) and `stringwidth`. Fonts embedded in the file — Type 1 programs (`… currentfile
  eexec … cleartomark`, binary or hexadecimal) and CFF font sets (`StartData`) — are found by
  name before the program runs; other fonts are drawn with the bundled fonts as for PDF. Each
  `show` becomes one compound shape named "Text: …".

- **Text** (§9): text objects and state (`Tc`, `Tw`, `Tz`, `TL`, `Ts`, `Tr`, `Tf`, `Td`,
  `TD`, `Tm`, `T*`), `Tj`, `TJ` (with kerning), `'` and `"`; render modes fill / stroke /
  fill-and-stroke / invisible and the clipping modes (glyph outlines clip until `Q`). Glyphs
  become outlines (one compound shape per show operator, named "Text: …" after the string
  through `ToUnicode` or the glyph names). Fonts: simple fonts with `/Widths`, base encodings
  (Standard, WinAnsi, MacRoman), `/Differences` and built-in encodings; composite (Type 0) fonts
  with Identity-H / Identity-V and embedded CMaps (`cidrange` / `cidchar`, `/WMode`), `/W` and
  `/DW` widths, vertical metrics (`/W2`, `/DW2`: the vertical displacement and the position
  vector from the horizontal to the vertical origin) and `CIDToGIDMap`. Embedded programs: TrueType (`FontFile2`, cmap (3,1) / (3,0) /
  (1,0) and `post` names), CFF (`FontFile3` `/Type1C`, `/CIDFontType0C`, `/OpenType`;
  name-keyed and CID-keyed with FDArray / FDSelect, flex, hint masks, `seac`), Type 1
  (`FontFile`, eexec, `/Subrs`, flex and hint replacement through othersubrs, `seac`) and Type 3
  glyph procedures. Fonts that are not embedded (the standard 14 and others) are drawn with
  the bundled OFL fonts by name: Inter (sans serif, Regular / Italic / SemiBold / Bold), Noto
  Serif (Times and other serif names) and JetBrains Mono (Courier), with their own advances
  when the PDF gives no widths.
- **Images** (§8.9): image XObjects and inline images (`BI … ID … EI`, abbreviated keys);
  1, 2, 4, 8 and 16 bits per component with `/Decode`; any colour space above (Indexed through
  its palette, ICCBased through its `/Alternate` space); DCT (JPEG, through zune-jpeg; Adobe
  inverted CMYK) and the general filters; stencil masks (`/ImageMask`) in the fill colour;
  soft masks (`/SMask` images) and colour-key and explicit `/Mask`s. Images become image nodes
  of the render tree (bilinear, box-filtered when minified).
- **Transparency:** blend modes (`/BM`, all 16 PDF modes) and soft masks in the graphics state
  (`/SMask` with `/S /Luminosity` and `/BC` backdrops, or `/S /Alpha`) as groups of the render
  tree, open until the state changes or `Q`: what is painted while a soft mask is set is
  rendered to an offscreen buffer and the mask applies to that combined result. Transparency
  group XObjects (§11.6.6) composite as one object: the current alpha, blend mode and soft mask
  apply to the group's result, and its contents start from the initial alpha. Isolated groups
  (`/I true`) composite over a transparent backdrop; non-isolated groups over the page, with
  the backdrop's contribution removed again (§11.4.8) so group alpha and masks apply only to
  the group; knockout groups (`/K true`) composite each child with the group's initial
  backdrop. Clipping groups are not transparency groups: blend modes inside them reach the
  backdrop.
- **Patterns:** tiling patterns (coloured and uncoloured, `XStep` / `YStep`, the cell clipped
  to `/BBox`, at most 4096 tiles per fill) for fills; strokes with a tiling pattern use its
  colour (uncoloured) or grey.
- **Shadings** (§8.7.4.5): axial and radial shadings become gradients. Function-based shadings
  (type 1, `Domain` and `Matrix`) and the mesh shadings — free-form (type 4, edge flags 0–2)
  and lattice-form (type 5) Gouraud-shaded triangle meshes, Coons patch meshes (type 6) and
  tensor-product patch meshes (type 7, flags 0–3 sharing the previous patch's edges) with
  `BitsPerCoordinate` / `BitsPerComponent` / `BitsPerFlag` / `Decode` and an optional
  parametric `Function` — are rasterised into an image node in page space (up to 4 pixels per
  point, at most 1024 pixels a side, 2×2 supersampled): patches are tessellated into triangles
  (up to 48×48 cells), colours (or `t`) interpolated across each triangle. They paint through
  `sh` (their own extent only) and as shading-pattern fills (clipped to the path).
- **Functions:** sampled (one or several inputs, multilinear interpolation), exponential,
  stitching and PostScript calculator (type 4) functions.
- **Pages:** any page ([`parse_page`], [`page_count`]); File ▸ Import takes `page` (from 1) and
  the Import dialog asks for it; footage remembers its page (`Footage::page`).

## Limitations

- JPX and JBIG2 images, predefined CJK CMaps other than Identity-H / Identity-V, and transfer
  functions (`/TR`) are not read. JBIG2 is deferred. Predefined CMaps (`UniGB-UCS2-H`,
  `90ms-RKSJ-H` …) need the CMap data, and there is no permissively licensed pure-Rust source
  of it (Adobe's CMap resources are Adobe assets, which this project does not ship); text in
  such fonts is reported in `Doc::skipped`. Mesh shadings are raster images in the render tree
  (sharp to about 4× the document size). Knockout uses each child's coverage as its shape (a
  child with constant opacity knocks out fully where it covers). What a document skipped is
  listed in `Doc::skipped`.
- Illustrator EPS files whose drawing depends on Adobe's procedure-set resources (defined in
  the file through constructs beyond this subset) and Level 3 `shfill` gradients in EPS draw
  only what the subset understands; save such artwork as PDF-compatible `.ai` or PDF instead.
  EPS images are skipped; Type 3 (procedure) fonts defined in EPS files draw nothing (reported).
- The EPS operand stack holds at most 100,000 values. Numeric `copy` and `aload` check the
  complete result before copying; exceeding it stops execution with an operand-stack-limit
  note in `Doc::skipped`, keeping artwork already drawn; other operators drop a value pushed past
  it. `astore` consumes its operands before returning its array, so it does not increase stack
  depth.
  The interpreter bounds what one operation can cost a hostile file: the memory an operator keeps
  is charged to an allocation budget or capped by a fixed limit listed below, and its running time
  is bounded by a fixed limit or paid for in proportion to the bytes or elements it touches:
  elements through an operation budget of 8,000,000, bytes it scans or copies through the
  allocation budget. Operators that reorder or search the operand stack
  (`roll`, `counttomark`, `setcolor` counting its operands) are bounded only by its 100,000 values,
  not yet paid per value.
  The allocation budget is 256 MiB, cumulative and never refunded when an object is dropped or
  overwritten. What an operator keeps is charged before it is allocated: array and procedure slots
  at `size_of::<V>()`, string bytes, and kept text at its exact length (a number, a boolean or a
  string that isn't UTF-8 is converted for the moment first, to measure it). That covers
  constructors, intervals, conversions (`cvs`, `cvn`), bulk operand and snapshot copies (`aload`,
  `copy`, `getinterval`, `dictstack`, `setdash`, and `forall`, which over a dictionary charges both the copies
  of its keys and the key-value pairs of its snapshot), matrices (six slots each, a font's
  included), encoding arrays (256 slots and every glyph name's text, including each `.notdef`),
  the dash array `gsave` or `save` copies and the one each stroke keeps, executable strings
  (charged conservatively before tokenization) and the copies `eq`
  and `ne` compare. A dictionary charges a new entry once: its key text and a fixed slot for the
  key's `String` and the value (`def`, `store`, `put`, `>>`, `defineresource`, `definefont`, a new
  font for `findfont` or `selectfont`, and the per-key copies `scalefont`, `makefont` and
  `selectfont` make); replacing an existing key's value reuses the entry. Hashing or comparing a
  key reads every byte, so looking one up (`get`, `known`, `load`, `where`, `undef`, `store`,
  `findfont`, `selectfont`, `findresource`, `findencoding`, `undefinefont`, a resource category),
  replacing its value, or comparing it with `eq` or `ne` charges its scan: a string at its
  worst-case lossy UTF-8 length, a name at its own length; a number key's text is formatted for the
  moment and its scan is not charged. `load` of an undefined name charges the name its operator keeps.
  A matrix is read only when its array is exactly six numbers, so `concat`, `setmatrix`,
  `makefont` and a font's stored matrix do not scan an array a program can make any length. A
  `SubFileDecode` end-of-data marker longer than 255 bytes is not searched for (the search costs
  its length at each file position) and its data is not skipped; a real marker is short.
  A font is built for drawing per base name and encoding (its first 256 glyph names, or the one
  glyph `glyphshow` draws). Each new one charges the font, its glyph-name table and its cache key,
  and, when its program is embedded in the file, the program's length for the parsed copy the font
  keeps, so re-encoding one embedded program charges it again. Drawing with a font already built
  charges only the string shown; the font is looked up by the identity of its name objects, and by
  their text only when those objects are new. A string font or glyph name is copied, charged and length-capped (below)
  each time it is used. A string's glyph outlines count against the operation budget and the path
  limit (1,048,576 elements) element by element as they are built, so `show` and its variants
  (`ashow`, `widthshow`, `awidthshow`, `kshow`, `cshow`, `xshow`, `yshow`, `xyshow`, `glyphshow`)
  and `charpath` pay for the glyphs they draw; `stringwidth` builds no outlines and is paid by the
  string's bytes.
  `array` and `string` requests are clamped to 1,000,000 slots and 10,000,000 bytes. At most 1,024
  graphics-state copies (`gsave`, `save`) are kept at once, `begin` keeps at most 64 dictionaries
  on the stack, and procedures nest at most 256 deep.
  A name or dictionary key made or kept at runtime is at most 255 bytes of text: converted by
  `cvn`, kept by `load` as the operator it makes, made by `findfont`, `selectfont` or `definefont`
  from a string, a string font or glyph name when it is drawn, or any new dictionary key, including
  a name read from the file. One byte longer stops the program (`limitcheck` in a conforming
  interpreter; PostScript's documented limit is 127, PostScript Language Reference, 3rd ed.,
  Appendix B). A name read from the file is not capped when it is read, pushed or run, since the
  file bounds how many there are: one too long to be any defined name or operator is not looked up
  (that would hash it against every dictionary on the stack) but noted as an unknown operator, so
  running it in a loop costs a constant however long the name. Inline image data an `image` cannot
  step over (an indirect data source, an unknown image operator, a bare `readhexstring`) reaches
  the scanner as names, so a cap there would reject it; the seventeen reference files this was
  checked against create names of at most 85 bytes, and real generator output at most 28.
  Reaching a limit stops execution with a note in `Doc::skipped` (operation limit, allocation
  limit, operand stack limit, path limit, recursion limit, graphics state depth limit, name length
  limit), keeping artwork already drawn, rather than continuing with partial results.
  This is not a process-memory guarantee. Outside the allocation budget, each bounded another way:
  the file's source tokens (the names, strings and procedures the scanner reads) and the one copy
  of each embedded font program taken from the file before the program runs, both bounded by the
  file's length; the operand stack (100,000 values); paths, including those in graphics-state
  copies and clips, and the drawn output, whose elements the operation budget and path limit bound
  (each stroke's dash copy aside, charged above); each font's outlines of the glyphs it has drawn
  (at most 256 per font); the font cache's identity index (cleared at 1,024 entries); running a
  name (a lookup of at most 255 bytes against at most 64 dictionaries); a hash table's spare
  buckets, a vector's spare capacity, and allocator overhead.
- **Create Shapes from Vector Layer:** clips that enclose the whole page become layer masks
  (the first clip Add, later ones Intersect). Nested clipping groups become Merge Paths: each
  closed shape inside them is a group with its geometry merged into one compound path, the clip
  paths, and Merge Paths ▸ Intersect before its fill and stroke, so fills match exactly and
  strokes follow the clipped outline; open stroked paths inside clips stay unclipped. Blend
  modes become the groups' Blend Mode. Images become footage layers: each is written as
  `<file> Image <n>.png` next to the source and imported, placed by its matrix (anchor at its
  top-left corner; position, rotation and scale — a skew is reported) and parented to the shape
  layer; the shapes drawn between images go to shape layers of their own (`<name> Outlines
  <n>`), so the layer stack keeps the document's paint order. Soft masks, and the clips and
  masks of images, are not converted (the command's `skipped` list says so).

## Shared parser setup and bounds

The workspace pins `craft-pdf` to `craft-libs` Git revision
`92d392cff132eaf53e13afff77f38b5c11b73aa0`, with `default-features = false` and only the explicit `read`
feature. This excludes the backend's bundled fonts and Adobe CMaps and preserves EffectCraft's
font and asset policies. The shared parser does not change the supported drawing operators,
image codecs, continuously rasterized SVG tree, editable shapes, or EPS interpreter.
This revision removes an unchecked lazy-array read and preserves later values and pages after
sign-only numeric entries; the app exercises those inputs through its object adapter, public
page parser and rasterizer on native and WASM targets. It also bounds xref, object-stream and
graphics-state work, reads the full `/W[0]` xref type width, and caps JBIG2 segment references
and bitmap allocation.

The root Cargo patch pins Hayro-syntax and Hayro-jbig2 to the same `storytold/craft-libs` revision. `cargo xtask pdf-deps` checks this graph and refuses
an enabled Hayro interpreter, rasterizer or CMap dependency. The shared revision must be
published before a fresh app checkout can resolve it; no sibling checkout is required.

Input is limited to 512 MiB. The object adapter admits at most 100,000 document/referenced
objects, 1,000,000 converted values and 64 inline nesting levels, with a 512 MiB retained
object-data budget. References are queued once rather than recursively expanded. Native
unwinding panics during shared loading or lazy object conversion become an error and discard
partial state. These limits are not an overall process-memory or execution-time guarantee;
WASM aborts and process-wide allocation failures cannot be caught. Missing or repaired objects
are reported rather than silently presented as a complete import.

The shared parser receives EffectCraft's existing ordinary object/xref stream limit at open:
256 MiB on 64-bit targets and 64 MiB on 32-bit targets, including WASM. Immutable document
limits apply during normal parsing, initial xrefs, fallback and repair. Malformed object-stream
compatibility recovery also obeys that limit and the adapter's remaining retained-data budget.
Content/image filter decoding still uses EffectCraft's existing bounded filter chain; the
retained-data cap is not a bound on every parser cache or temporary allocation.
