# After Effects project import

> **Last reviewed:** 2026-10-19 · **Last updated:** 2026-10-19 · **Change:** Curves and Mesh Warp are imported (961-965, 985; FORMAT S of the corpus); before that: a shape item's eye switch is imported (bit 0 of its group flags, 986 / 987: a switched-off Fill under Trim Paths is no longer drawn, render equal to After Effects), and a layer style's (switch group `tdsb` 0, 988); before that: a negative time stretch (Time Reverse Layer) is imported: in and out point in order, the layer plays backwards (981-983: render equal to After Effects at 7 points); before that: After Effects' None on Compound Blur, Displacement Map and Set Matte arrives as an explicit None (`NO_LAYER`), not as the owning layer; before that: layer styles are imported (the nine styles and Blending Options, 951-979; Global Light becomes the composition's); before that: the 27 effects of round 3 are in the shared table (`effectcraft_aep::effect_map`), which the exporter writes from too; before that: the importer round 3 is merged into the exporter line; before that: a layer of a clip, sequence or sound ends at its source's end (After Effects ends it there when it opens a project, export test f1), unless it is remapped or looped or runs less than a frame past; before that: keys of shape colours are read (the reader took their 152-byte spatial records for plain ones: `tdb4` byte 7 is 0 there, 854); before that: the effect table moved to `effectcraft_aep::effect_map` (shared with the exporter); point defaults in percent and a popup default slot of 0 (After Effects' save of a project it opened, e1 / e2 re-saves) are read; before that: the justification of right-to-left paragraphs is read relative to the direction (t7); before that: 24 more layer blend modes (906, 907); before that: 27 more effects are imported (batch 6: 903-905), with layer parameters; before that: a Rec. 709 working space also sets the output space (After Effects renders in the working space, 902); before that: a mask whose path was never set arrives as the source rectangle (900, 901); before that: Offset Paths' Copy Offset arrives as EffectCraft's (After Effects' value minus 1, 828); before that: the importer and exporter lines are merged (project settings, image sequences, conformed clips and effect instance names come with the importer's mattes, shapes, text settings and animators, effects, markers, time remapping, cameras and lights); before that: parallel and ambient lights (701, 702) and keyed Orientation (703) are imported; before that: masks on text and shape layers (705, 706: AE vs EffectCraft render 12 and 0 of 5184 points, glyph edges) and Merge Paths, Offset Paths, Repeater (707-710, 731; merge renders equal but for edges; Repeater copy opacity and Offset Paths' outline differ in EffectCraft's renderer) are imported; before that: keyframes: the ease of Anchor Point, effect point, effect colour and mask path keys is imported (597-599, 700, 730: within 0.5 px of After Effects' read-backs); before that: effects: Directional Blur, Radial Blur, CC Radial Blur, Venetian Blinds, Channel Blur, Black & White, Tritone, Luma Key, Simple Choker, Matte Choker and Transform are imported (711-723; render checks: CC Radial Blur and Channel Blur equal, Directional Blur within 5/255; Venetian Blinds, Radial Blur, Black & White (8/255), Tritone (19/255), Luma Key, Matte Choker and Transform differ in EffectCraft's renderer, see STATUS); before that: text: character and paragraph settings (stroke, scales, baseline shift, kerning, ligatures, tsume, indents, spacing, justified text, composer, direction, vertical text; 538-578) and text animators with range selectors (579-594, 114) are imported; render checks 594 (keyed range selector at 0 / 0.5 / 1 / 1.5 / 2.5 s), 595, 596 equal but for glyph edges; before that: cameras, spot and point lights, X / Y Rotation and Orientation, Timewarp and expressions on effect parameters are imported (render checks 486, 535-537 equal but for edges; 488 differs by 5.25 %, cause in EffectCraft's renderer not isolated); before that: text: superscript and subscript are imported (526-528); before that: effects: Invert, Fast Box Blur, Gaussian Blur, Drop Shadow, Glow, Fill, Hue/Saturation, Levels and five expression controls are imported, popups are mapped by item name, checkbox defaults come from the file; before that: shape layers are imported (groups, paths, paints, trim paths, round corners; 451-484, 519 added); before that: markers and Time Remapping are imported (509, 510 added); before that: track mattes are imported (402-408); before that: text: automatic leading is verified (400); effects: Exposure is imported (Channels, all twelve sliders, Bypass, keys), next to Tint and Gradient Ramp; before that: text: U+0003 is a paragraph break (386, 387); before that: a stored text Anchor Point is pixels (385, now V); before that: effects: Tint and Gradient Ramp are imported (all parameters and keys), other effects stay reported; `.aepx` is accepted; text layers are imported (Source Text with style runs, paragraphs and box text; fonts by PostScript name, missing fonts listed); layer styles are reported; before that: layer masks · **Target:** Adobe After Effects 2026

File ▸ Import ▸ After Effects Project… (`file.importAep`) reads an `.aep` or its XML form `.aepx`
and adds its items to the open project, in a folder named after the file (as After Effects does when it imports a project).
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
  access, the caller passes the bytes, a "does this footage path exist" callback and a "is this
  font installed" callback (`ImportOptions::has_font`; the engine passes the renderer's own font
  lookup, `ImportOptions::new` takes every font as installed);
- the command (`file.importAep`, `crates/engine/src/commands/aep.rs`) is a thin wrapper: read the
  file, run the importer inside one undoable edit, open the first composition, return the report.

API: `import(&mut Project, bytes, &ImportOptions) -> Result<ImportResult, AepImportError>` and the
dry runs `analyze(bytes, exists) -> ImportReport` (every font counts as installed) and
`analyze_with(bytes, &ImportOptions)`. An error (not a project, damaged container,
refused losses) leaves the project unchanged.

## The command

`file.importAep {path, allowLosses?, dryRun?}`

- Without `allowLosses` an import that would lose anything is **refused** (nothing changes) and the
  error names the categories, e.g. `importing would lose effects: 3, markers: 1; allow losses to
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
| Project settings | colour depth, working space (None, sRGB, Rec. 709 Gamma 2.4; another one is reported as `project settings`), working gamma, Linearize Working Space, Blend Colors Using 1.0 Gamma, time display (timecode, frames, feet + frames 35 / 16 mm), frame count start (620-634). After Effects renders in the working space (902: in a Rec. 709 Gamma 2.4 project an sRGB PNG step of 128 comes out as 135 and a grey 0.5 solid as 128, in the Render Queue output and in the captures), while EffectCraft converts its output to `output_space` (sRGB when unset): a Rec. 709 working space therefore also sets the output space to Rec. 709 (902 within 1 level of After Effects, was up to 13). They come in `ImportResult::settings`; the library never changes the project's own settings, `file.importAep` applies them when the project has no compositions yet (or with `applySettings: true`) |
| Composition | name, size, exact frame rate, duration, work area, background colour (8 bit), pixel aspect, composition motion blur switch, shutter angle and phase (as stored), display start time, Classic/Advanced 3D renderer |
| Layers | solid, null, adjustment, footage, precomposition and **text** layers: name (the source's name when the layer has none; for a text layer its first line), in/out/start and time stretch (exact ratios, e.g. 50 % and 150 %); a layer of a clip, sequence or sound ends at its source's end as After Effects ends it (unless remapped, looped or less than a frame past; f1), enabled/audio/solo/shy/locked/effects/motion blur/collapse/3D/guide switches, draft quality, label, blend mode (Normal, Multiply, Add, Screen, Overlay, Difference; Soft Light, Hard Light, Darken, Lighten, Exclusion, Color Dodge, Color Burn by name, 906; Classic Difference, Hue, Saturation, Color, Luminosity, the Stencil and Silhouette modes, Classic Color Dodge / Burn, Linear Dodge / Burn, Linear, Vivid and Pin Light, Hard Mix by render, 907: each code rendered by After Effects matched exactly one EffectCraft mode; 906 renders within 1/255 of After Effects), parent, **track matte** (alpha, alpha inverted, luma, luma inverted; the matte layer anywhere in the composition, also below its target; 402-408) |
| Transform | Anchor Point, Position, Scale, Rotation, Opacity, **X and Y Rotation** (486, 487) and **Orientation**, static (488) or keyed (703: key values, times, linear and hold sides; a bezier side becomes linear and is reported; After Effects slerps between the keys, EffectCraft interpolates each angle, so in-between values differ, e.g. 703 at 1 s `[6.26, 9.35, 15.22]` against `[5, 10, 15]`; an expression-driven orientation is reported): the stored value, or the default when the file stores none; keyframes with time, linear/bezier/hold on each side, temporal ease (speed and influence, per dimension for Scale), automatic or manual spatial tangents, roving and key labels |
| Expressions | on the transform and mask properties and on effect, camera and light parameters (534): source text and enable switch |
| Cameras and lights | **cameras** (one-node and two-node, from `ldta` byte 38 bit `0x40`): point of interest, Position as stored (also the auto Position `[0, 0, -zoom]`, where After Effects renders it, 537), orientation and rotations, Zoom (absent: `-z` of the auto Position, else the 50 mm default), Depth of Field, Focus Distance (absent: the zoom), Aperture (absent: 25.309 at 1920 px, read back from 256; scaled with the width, which is not sampled, E); **spot, point, parallel and ambient lights** (light type 1, 2, 0, 3: 701, 702): point of interest (not for point and ambient lights), Position, Intensity, Color, Cone Angle, Cone Feather, Casts Shadows; keys and expressions as for the transform; 251-256, 341-345, 535-537, 701, 702. Environment lights are reported |
| Text | the Source Text of a text layer: the text (paragraphs separated by CR; a hard line break is a CR too; U+0003 is laid out by After Effects exactly like a CR (386 against 387) and becomes a CR), per style run the font (PostScript name), size, fill and stroke colour, whether fill and stroke are drawn, stroke width, stroke over fill (one switch for the text), tracking, leading (automatic or explicit), faux bold, faux italic, all caps, small caps, superscript and subscript (526-528), horizontal and vertical scale, baseline shift, automatic kerning (metrics, optical, none), ligatures, tsume (538-578); per paragraph the justification (left, centre, right and the four justified variants), first-line / start / end indent, space before / after, direction, composer, hanging punctuation (in a right-to-left paragraph the stored justification is relative to the direction: After Effects lays 0 and 3 out flush right and 1 and 4 flush left, export test t7, so they arrive as right / left); point text or box text (box size and top-left in layer pixels); vertical right-to-left text (575) |
| Text animators | every animator with its name and its range selectors (Start, End, Offset, or the Index range when Units is Index; Units, Based On, Mode, Amount, Shape, Smoothness, Ease High / Low, Randomize Order, Random Seed) and the properties Anchor Point, Position, Scale, Skew, Skew Axis, Rotation, Opacity, Fill Color, Stroke Color, Stroke Width, Tracking Amount, Line Spacing, Blur, Character Offset and Character Range, with keys and expressions (579-594, 114, 728). Values are stored as After Effects shows them; popups are 1-based in the file |
| Masks | every mask of an imported layer, in stack order (on text and shape layers, which have no source, the stored mask box is in layer pixels: 705, 706): name, path (vertices and tangents in layer pixels, open or closed), mode (None, Add, Subtract, Intersect, Lighten, Darken, Difference), inverted, feather (x, y in pixels), opacity (percent), expansion (pixels); path keyframes (time, linear or hold sides, roving, label), feather / opacity / expansion keyframes with ease |
| Effects | **Curves** (961-964: the five curves' points from the arbitrary data as EffectCraft's `x,y` points in 0..1; render within 6 levels: EffectCraft interpolates monotone, After Effects stores a natural spline) and **Mesh Warp** (965, 985: Rows, Columns, Quality and the Distortion Mesh as per-vertex offsets in layer pixels; Bezier handles moved on their own are kept as eight extra numbers of the vertex for the export and reported, EffectCraft draws the mesh bilinear; the header is rows + 1, columns + 1; render 985: 1.4 % of pixels > 2 levels around the moved vertex), **Tint**, **Gradient Ramp**, **Exposure**, **Invert**, **Fast Box Blur**, **Gaussian Blur**, **Drop Shadow**, **Glow**, **Fill**, **Hue/Saturation**, **Levels**, **Timewarp** (Method, Adjust Time By, Speed with keys, Source Frame; 529-533), **Directional Blur**, **Radial Blur**, **CC Radial Blur**, **Venetian Blinds**, **Channel Blur**, **Black & White**, **Tritone**, **Luma Key**, **Simple Choker**, **Matte Choker**, **Transform** (all parameters; 711-723) and the expression controls **Slider**, **Angle**, **Checkbox**, **Color** and **Point Control** (table below) on any imported layer (also text and shape layers and layers with masks), in stack order, with the instance name, every parameter (static value or default) and its keyframes; also the second and later instances of a type, whose parameter definitions are only in the project's shared definition list. Other effects are reported (below) |
| Layer styles | Drop Shadow, Inner Shadow, Outer Glow, Inner Glow, Bevel and Emboss, Satin, Color Overlay, Gradient Overlay and Stroke when applied (switch group `tdsb` 1): every parameter, starting from After Effects' defaults (Use Global Light is off there), with keys on scalars and colours and expressions; Blending Options: Fill Opacity, the channel switches, Blend Interior Styles as Group, Use Blend Ranges, and Global Light Angle / Altitude, which the composition adopts (one setting per composition here). Units and popups through `effectcraft_aep::style_map` (aep-format 3.12b). Render check against After Effects (`proof\styles`): drop shadow, inner shadow, satin, colour overlay, stroke within a few levels; glow and bevel intensities and the Gradient Overlay direction differ in EffectCraft's renderer |
| Shape layers | groups (nested, with anchor, position, scale, skew, skew axis, rotation, opacity), rectangle (size, position, roundness), ellipse (size, position), polystar (star or polygon, points, position, rotation, radii, roundness), bezier path (vertices and tangents in layer pixels, path keys), fill (colour, opacity, fill rule), stroke (colour, opacity, width, cap, join, miter limit), gradient fill and stroke (linear or radial, start and end point; the default white-to-black colours), trim paths (start, end, offset, trim multiple shapes), round corners (radius), merge paths (mode; 707, 708), offset paths (amount, line join, miter limit, copies, copy offset; 710; After Effects offsets copy *i* by Amount x (*i* + Copy Offset), default 1, EffectCraft by Amount x (*i* + 1 + copyOffset), so the copy offset arrives 1 less, also its keys; an expression on it is kept and reported; 826-828, 828 renders equal to After Effects on every grid point), repeater (copies, offset, composite, transform; 709), in file order, with keyframes and expressions, the operators' defaults as After Effects reads them back (731); the layer anchor in pixels; 238-242, 330-333, 451-484, 519, 705-710, 731 |
| Markers | layer and composition markers: time (layer time for layer markers, exact frames), duration (1/600 s units, 509, 510), comment, chapter, URL, frame target, label, cue point (name, event or navigation, parameters); 122, 123, 249, 347-349, 509, 510 |
| Time Remapping | an enabled remap becomes the `timeRemap` property (source seconds, keys in layer time with their interpolation and ease, expression), first in the layer's properties as Layer > Time > Enable Time Remapping puts it; 319, 354 |
| Footage | a link to the stored path; **missing-footage placeholder** when the path does not exist where the import runs (listed in `missingFootage`), a normal link when it does. The importer never opens footage. **An image sequence** (its alias is the folder) becomes a sequence of its files, named from the stored numbering (`shot_0101.exr` .. `shot_0103.exr` in 603, `seq_001.png` .. in 353), at its native frame rate; the first file is the footage's path and the one checked. Sequences and clips take the stored native frame rate and duration (603, 606). A sequence of more than 100 000 frames, or beyond a million file names in one import, links only its first file and is reported |

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
- **Masks without a path.** A mask whose path was never set stores no Mask Path entry (900); After Effects reads it back as the source rectangle `[0,0] [0,h] [w,h] [w,0]`, so it is imported as that rectangle (901, expansion -100: EffectCraft equal to After Effects on every pixel). On a text or shape layer, which has no source, it is reported.
- **Masks.** The path is stored as `float32` points normalised inside a box that is normalised to
  the layer's **source** size (090 on a 1920 x 1080 solid: `[560, 340] ... [1360, 740]`; 309 on a
  100 x 100 solid: `[20, 30] ... [80, 70]`), so the vertices become layer pixels with the origin at
  the layer's top left, as EffectCraft's masks are. A tangent is the handle minus the vertex (232:
  the ellipse imports as EffectCraft's own 0.5523 x radius ellipse); the open bit makes an open
  path (233). Feather and expansion are pixels, opacity a fraction (091: 20 / 20, 50 %, 5;
  Subtract, inverted). Feather keys use the plain template with one ease per dimension in pixels
  per second (208 linear, 306 bezier). The path keys are 64-byte records whose ease is not
  decoded: a bezier side becomes linear and is reported as an approximated keyframe (093 has two
  linear keys at 0 s and 1 s). Opacity and expansion keys and mask expressions use the same
  verified property templates as the Transform properties but have **no sample** in the corpus.
  Mask colour, lock, feather falloff, motion blur and RotoBezier are not decoded: the masks get
  EffectCraft's default colour cycle and default switches (listed under `limitations`).
- **Text layers.** A text layer has no source. Its layer space is the text's: for point text the
  origin is the alignment point on the first baseline (left edge for left text, centre for centred,
  right edge for right text), for box text the box is placed by its stored top-left corner; this is
  how EffectCraft lays out text too, so nothing is converted. The **Anchor Point rule**: on a text
  layer the stored anchor is taken as **pixels as stored** (not a fraction of a source size) and an
  absent anchor is the text origin `[0, 0, 0]`, After Effects' default for new text. The default is
  verified (no other text sample stores an anchor, and the samples' Position default is the comp
  centre: 113 imports at `[960, 540]` with the baseline origin there); "pixels as stored" is
  verified by 385 (Anchor Point `10, 20` is stored as `10, 20` and imports as `[10, 20, 0]`;
  confidence V, see aep-format.md 3.4). The layer is created like a new text layer (spanning
  the composition, anchor at the origin), so Position, Scale, Rotation and Opacity keys, their
  expressions and the parent link import as on any layer.
- **Fonts.** The font is kept as its PostScript name (`TimesNewRomanPSMT`); EffectCraft resolves
  family names, full names and PostScript names, so an installed font is found as it is. A font
  that is not installed keeps its name in the document (it is found as soon as it is installed),
  is drawn with the renderer's fallback font and is listed once in `missingFonts`. A missing font
  is not a loss and never refuses the import. Only the fonts the style runs use are checked (the
  stored font list also holds unused entries such as `AdobeInvisFont`). The style is `Regular`;
  faux bold and italic are imported as such. Real bold and italic fonts were not sampled.
- **Leading.** A run with automatic leading gets EffectCraft's automatic leading (120 % of the
  size, the same rule as After Effects: 400 lays its second line out 120 px below the first at
  size 100); otherwise the stored leading in pixels (243: 98, 339: 60).
- **Superscript.** Style slot /13 is superscript (1) or subscript (2); After Effects draws such text
  at 58.3 % of the size, 33.3 % of the size above or below the baseline, as EffectCraft does
  (526-528). Before this was decoded, superscript text arrived at its full size: 1.7 times as wide
  and tall as in After Effects, with its baseline 33 % of the size too low. Synthetic check: 517, a 600 px box whose text wraps into
  four lines in both programs once the size is right. The slot equalled the reference document, because every scripted text of the corpus is
  superscript (526), so it was never reported. Render check after the fix: 243, 526-528 equal but
  for anti-aliased edges (at most 0.15 % of the points); 517 has the same lines, but its first
  baseline sits about 16 px lower than in After Effects (the first-line offset of superscript box
  text, open).
- **Style runs.** After Effects' last run also covers the final CR of the text, which EffectCraft's
  text does not have; the runs are cut to the text. A range style splits `Hello` into runs of 1,
  2 and 2 characters (248, 340).
- **Null objects.** The hidden placeholder solid of a null object is not an item (null layers
  have no source item here).
- **Track mattes.** The mode and the matte layer's id are in the target layer's header
  (aep-format.md 3.1). The matte layer can be any layer of the composition (407: below its
  target); it keeps its stored video switch (After Effects switches it off when the matte is set,
  403; 408 has it switched back on), and EffectCraft, like After Effects, uses a hidden layer as a
  matte. A mode without a matte layer id has no corpus case (After Effects 2026 always writes the
  id); it is read as the layer directly above, which is how track mattes worked before After
  Effects 23.0. Tests: `crates/aep-import/tests/mattes.rs` (403-408, the id-less form built
  in-test, hostile ids and modes).

### Effects

A table in `crates/aep/src/effect_map.rs` (shared with the exporter, which writes the inverse) maps an effect's match name and each parameter's
index in the file's parameter list to an EffectCraft effect id, a parameter id and a unit
conversion; it also names the parameter type the file must declare and, for sliders, the valid
maximum it was verified with (`pard@128` for fixed sliders, `pard@108` for float sliders). A parameter whose type or maximum differs is reported,
not guessed. `supported_effects()` lists the table. The effect is built with EffectCraft's own
defaults (`effectcraft_effects::instantiate` on the layer's source size), then every mapped
parameter gets the file's value:

| After Effects (match name, index) | EffectCraft | Conversion | Evidence |
|---|---|---|---|
| `ADBE Tint` 1 Map Black To, 2 Map White To | `black`, `white` | ARGB 0..255 -> RGBA 0..1, alpha 1 | 356, 357; keys 364 |
| `ADBE Tint` 3 Amount to Tint | `amount` | stored percent (valid max 100) x 1 | 356, 361, 362; keys 363 |
| `ADBE Ramp` 1 Start of Ramp, 3 End of Ramp | `start`, `end` | fraction of the layer size x source size = layer pixels; outside 0..1 stays outside | 368, 369, 374, 383; keys 382 |
| `ADBE Ramp` 2 Start Color, 4 End Color | `startColor`, `endColor` | ARGB 0..255 -> RGBA 0..1, alpha 1 | 368, 371 |
| `ADBE Ramp` 5 Ramp Shape | `shape` | 1-based popup -> 0-based (Radial 2 -> 1) | 368, 377 |
| `ADBE Ramp` 6 Ramp Scatter | `scatter` | pixels (valid max 512) x 1 | 368, 378 |
| `ADBE Ramp` 7 Blend With Original | `blend` | 0..1 fraction (valid max 1) x 100 = percent | 368, 380 |
| `ADBE Exposure2` 1 Channels | `channels` | 1-based popup -> 0-based (Individual Channels 2 -> 1) | 389, 390, 395 |
| `ADBE Exposure2` 3, 8, 13, 18 Exposure (Master, Red, Green, Blue) | `master/exposure`, `red/redExposure`, `green/greenExposure`, `blue/blueExposure` | stops (float slider, valid max 100) x 1 | 389, 391, 392, 395; keys 397 |
| `ADBE Exposure2` 4, 9, 14, 19 Offset | `master/offset`, `red/redOffset`, `green/greenOffset`, `blue/blueOffset` | plain (valid max 2) x 1 | 389, 393 |
| `ADBE Exposure2` 5, 10, 15, 20 Gamma Correction | `master/gamma`, `red/redGamma`, `green/greenGamma`, `blue/blueGamma` | plain (valid max 10, default 1) x 1 | 389, 394 |
| `ADBE Exposure2` 22 Bypass Linear Light Conversion | `bypassLinearLight` | checkbox: record 1.0 = on, no record = off | 389, 396 |
| `ADBE Invert` 1 Channel, 2 Blend With Original | `channel`, `blend` | popup by item name (the file's 16 items include three separators); percent x 1 | 409-411 |
| `ADBE Box Blur2` 1 Blur Radius, 2 Iterations, 3 Blur Dimensions, 4 Repeat Edge Pixels | `radius`, `iterations`, `dimensions`, `repeatEdge` | pixels (valid max 30 000); integer (valid max 50); popup by name; checkbox, **default on** | 412-416, 525 |
| `ADBE Gaussian Blur 2` 1 Blurriness, 2 Blur Dimensions, 3 Repeat Edge Pixels | `blurriness`, `dimensions`, `repeatEdge` | float (valid max 30 000, default 25); popup by name; checkbox, **default on** | 214, 216, 417-419, 522 |
| `ADBE Drop Shadow` 1 Shadow Color, 2 Opacity, 3 Direction, 4 Distance, 5 Softness, 6 Shadow Only | `color`, `opacity`, `direction`, `distance`, `softness`, `shadowOnly` | colour; 0..255 (valid max 255) x 100 / 255 = percent; degrees; pixels (4 000); pixels (30 000); checkbox | 080, 420-426, 523 |
| `ADBE Glo2` 1 Glow Based On, 2 Threshold, 3 Radius, 4 Intensity, 5 Composite Original, 6 Glow Operation, 7 Glow Colors, 8 Color Looping, 9 Color Loops, 10 Color Phase, 11 A & B Midpoint, 12 Color A, 13 Color B, 14 Glow Dimensions | `based`, `threshold`, `radius`, `intensity`, `operation`, `glowOperation`, `colors`, `colorLooping`, `colorLoops`, `colorPhase`, `abMidpoint`, `colorA`, `colorB`, `glowDimensions` | popups by name (After Effects' defaults Color Channels, Behind, Add, Original Colors, Triangle A>B>A); threshold 0..255 -> percent; radius pixels (1 000); intensity (255); loops (127); phase degrees; midpoint 0..0.99 x 100 = percent; colours | 427-441, 524 |
| `ADBE Fill` 2 All Masks, 3 Color, 4 Invert, 5 / 6 Horizontal / Vertical Feather, 7 Opacity | `allMasks`, `color`, `invert`, `horizontalFeather`, `verticalFeather`, `opacity` | checkboxes; colour; pixels (999); 0..1 x 100 = percent. Fill Mask (a path reference) is reported when set | 079, 210, 215, 302, 303, 307 |
| `ADBE HUE SATURATION` 1 Channel Control, 2 Channel Range data, 6 Colorize, 7 Colorize Hue, 8 Colorize Saturation, 9 Colorize Lightness | `channelControl`; `hue`, `saturation`, `lightness` from the data; `colorize`, `colorizeHue`, `colorizeSaturation`, `colorizeLightness` | popup by name; the master hue (degrees), saturation and lightness are the `i32` of the arbitrary data (the visible Master parameters keep no record); the colour ranges are reported when they are not at their defaults (not sampled) | 443-450 |
| `ADBE Easy Levels2` 1 Channel, 2 Histogram data, 8 / 9 Clip To Output Black / White | `channel`; `inBlack`, `inWhite`, `gamma`, `outBlack`, `outWhite` from the data; `clipToOutputBlack`, `clipToOutputWhite` | popups by name (After Effects' items On, Off, Off for 32 bpc Color are in another order here); the RGB values are the `f32` of the arbitrary data; the per-channel values are reported when they are not at their defaults (not sampled) | 511-516 |
| Batch 6 (903 defaults, 904 / 905 set values): `ADBE Brightness & Contrast 2`, `ADBE Turbulent Displace`, `ADBE Fractal Noise`, `ADBE Laser` (Beam), `CC Radial Fast Blur`, `ADBE Noise2`, `ADBE Pro Levels2` (Levels (Individual Controls)), `ADBE Corner Pin`, `ADBE Grid`, `ADBE Sharpen`, `ADBE Linear Color Key2`, `ADBE Radial Wipe`, `ADBE Remove Color Matting`, `CC Bend It`, `CC Power Pin`, `ADBE 4ColorGradient`, `ADBE Change Color`, `ADBE Extract`, `ADBE Noise HLS2`, `ADBE Scatter`, `CC Light Sweep`, `ADBE Compound Blur`, `ADBE Displacement Map`, `ADBE Set Matte3`, `ADBE Gaussian Blur` (legacy), `ADBE OFMotionBlur` (Pixel Motion Blur), `ADBE Layer Control` | the EffectCraft effect of the same name, every parameter EffectCraft has | sliders as stored (each checked against its valid maximum); Beam Length, Time and Softness and CC Power Pin Perspective are 0..1 fractions, percentages here; points as layer pixels; popups by item name (Set Matte's `Red Channel` ... is `Red` ...); twirl-down group markers and the Levels histogram carry nothing; **layer parameters** (below) | 903, 904, 905; 220, 320, 321 |
| `ADBE Slider Control`, `ADBE Angle Control`, `ADBE Checkbox Control`, `ADBE Color Control`, `ADBE Point Control` (parameter 1) | `ec.control.slider` `slider`, `ec.control.angle` `angle`, `ec.control.checkbox` `checkbox`, `ec.control.color` `color`, `ec.control.point` `point` | float (valid max 1 000 000) as stored; degrees; checkbox; colour; point in layer pixels | 071, 073, 217-219, 221, 304, 310 |

- **Nothing to import:** the implicit input-layer parameter (index 0), the Swap Colors buttons
  (Tint 4, Ramp 8: they always have a record and hold no value), Exposure's twirl-down group
  markers (start / end of Master 2 / 6, Red 7 / 11, Green 12 / 16, Blue 17 / 21: always a record,
  no value) and the closing built-in entry.
- **Defaults.** A parameter without a value record has the default in the file's `pard`, also a
  checkbox (Repeat Edge Pixels is on by default, 412 / 417) and a popup. After Effects' defaults
  differ from EffectCraft's in places (Gaussian Blur's Blurriness 25, Repeat Edge Pixels on, Glow's
  Composite Original Behind), so they are always set from the file. A popup whose definitions are
  borrowed from the project's shared list has no trustworthy default slot (a selection writes into
  it): the table's After Effects default (read back for a fresh effect) is used.
- **Popups by name.** A popup value is the 1-based index of the file's item list (separators
  included, 410); it is mapped to the EffectCraft item of the same name, because the orders differ
  (Levels' Clip To Output Black / White: On, Off, Off for 32 bpc Color in After Effects, Off, On,
  ... here). An item EffectCraft does not have (Glow's None, Dissolve, Stencil ... operations) is
  reported and the parameter keeps its default. A popup may come without its item
  names (`pdnm`), also in the project-level definitions; the importer then uses After Effects'
  item list as the corpus stores it (368, 389, 409, 412, 417, 427, 443, 511), but only when the
  item count in `pard` agrees; otherwise the parameter is reported (test
  `popups_without_item_names_use_after_effects_items`). Before, such a popup was reported as damaged
  data.
- **Layer parameters** (Set Matte's Take Matte From Layer, Displacement Map's map layer, Compound
  Blur's Blur Layer, Layer Control): the file stores the chosen layer's id (`tdpi`, the same id
  as the parent link; 0 = None; the owning layer's own id = "this layer", the default; 220, 320,
  321, 904 E22-E24). The id is resolved once every layer of the composition exists (the layer can
  sit below); a layer that was not imported is reported and the parameter stays unset. "This
  layer" is the owning layer for Layer Control and EffectCraft's unset layer for the effects,
  whose unset layer is the layer itself. None is unset for Layer Control and the explicit None
  `NO_LAYER` (`Value::Layer(Some(0))`, never a layer id) for the effects, so it is written back as
  None (After Effects renders Set Matte None and self differently: `export-tests\fx27\ae\layer-none`). After Effects hands the named layer's **source** to the
  effect, without its masks and effects (904 E24: Set Matte's luminance from a grey solid that
  carries a Gradient Ramp is the grey's, 0.5 everywhere), so the companion choice is set to
  Source (EffectCraft's own default is Effects & Masks; where After Effects stores another choice
  is not sampled). Keys and expressions on a layer parameter are reported.
- **Render check, batch 6** (904, one effect per composition on a PNG of grey and colour bands,
  1 s, 20 px grid): Remove Color Matting, Displacement Map, Set Matte equal (Set Matte was 4 640
  points off before the Source rule); Change Color within 1/255; Sharpen 0 grid points (edges
  only); Corner Pin 12, CC Power Pin 24 edge points; differences in EffectCraft's renderer
  (values checked in the imported project): Brightness & Contrast up to 6/255, Levels (Individual
  Controls) up to 27/255, 4-Color Gradient up to 34/255, Linear Color Key and Extract on the
  coloured bands (Extract keeps pure blue at 231 in After Effects, EffectCraft keys it out:
  luminance weights), Radial Wipe edge / feather, CC Light Sweep, Compound Blur, Beam (89 points),
  CC Radial Fast Blur, CC Bend It (After Effects' Bend 30 bends far more than EffectCraft's),
  Turbulent Displace and Fractal Noise (other noise). Noise, Noise HLS and Scatter are random and
  were not compared.
- **Render check** against After Effects at 1 s (20 px grid, 5 184 points): Invert 410 / 411,
  Hue/Saturation 444-447, Levels 515 / 516, Fast Box Blur 525: every point equal or within 1/255;
  Gaussian Blur 522, Drop Shadow 523 within 2/255; Glow 524: 27 points differ, all on the one-pixel
  column just outside the layer: After Effects' Glow draws a dark halo about 10 px wide outside the
  layer (red 0.55, 0.65, 0.74, 0.92 at 0, 1, 2, 5 px), EffectCraft's Glow none (an effect
  difference, recorded for a separate fix).
- **Colour.** After Effects stores the colour its picker shows as ARGB doubles 0..255 in the
  project's working-space encoding; EffectCraft's parameter colours are straight RGBA 0..1 that the
  render pipeline hands to the effect as stored (`crates/render/src/color.rs`: authored colours are
  in the working space's encoding; no conversion is applied to effect parameters). So the value is
  divided by 255 with no transfer curve, as the solid colour already is. **Alpha:** Tint's defaults
  store alpha 0 (After Effects reads them back as `[0,0,0,0]` and `[1,1,1,0]`), set values store
  255. After Effects colour parameters have no editable alpha and both EffectCraft effects use only
  red, green and blue (CPU and GPU paths), so every imported colour gets alpha 1, EffectCraft's
  own default.
- **Exposure.** The sliders are float sliders whose stored value is the float32 After Effects keeps
  (Offset 0.1 arrives as 0.10000000149, Gamma 2.2 as 2.2000000477), imported as stored. Units are
  the same in both programs (stops, plain offset, plain gamma). The ranges are not: After Effects
  accepts Exposure -100..100 (slider -4..4) and Gamma Correction 0.1..10, EffectCraft's Exposure
  declares -20..20 (slider -5..5) and 0.01..9.99. A value outside EffectCraft's range is imported
  unchanged and not reported (the effect renders it; editing it in EffectCraft clamps it to
  EffectCraft's range). With Channels = Master the per-channel values are inactive in both programs
  and are imported as stored. Keys on the Bypass checkbox are not sampled: reported, not imported.
- **Second instances.** When several layers carry the same effect type, only one instance has its
  parameter definitions; the others take them from the file's shared list by match name but read
  their values only from their own records (383: L's Ramp is the default although the shared copy
  holds S100's Start [10, 20]). An instance without any readable definitions is reported.
- **Keyframes** of effect parameters are layer time like all keys. Linear keys are verified (363,
  364, 382). For a bezier slider key the ease speed is scaled like the value (the transform rule;
  inferred, not sampled for effects). An effect point's speed, like the Anchor Point's, is stored
  divided by the layer height (597, 598, 730; EffectCraft's values at 0.5 / 1 / 1.5 s within 0.5 px
  of After Effects'). Colour keys switched to bezier store speed 0 and influence 0 and After Effects
  interpolates them linearly (598), so they arrive linear; another colour ease and the ease of
  popups are not decoded, such bezier sides become linear and are reported in
  `approximated_keyframes`.
- **Not decoded:** the per-effect on/off switch (imported effects are on; listed in
  `limitations`), and expressions on effect parameters (reported, the value is imported).

## What is reported

`ImportReport` (`serde`, returned as JSON) has `imported` (counts, including `textLayers`),
`losses` (per category: `count` exact, up to 20 `details`), `missingFootage`, `missingFonts`
(PostScript names; not losses) and `limitations`. The categories:

| Category | Counted |
|---|---|
| `effects` | each effect on an imported layer that is not in the table (by name and match name); for an imported effect: each stored parameter outside the table, a parameter whose type or valid maximum differs from the table, an expression on a parameter, an effect without readable parameter definitions |
| `masks` | a mask that is not imported because its path cannot be used (a Mask Path entry without shapes, a mask on a text or shape layer whose path was never set (its default is not sampled), unreadable, no vertices, a point count that is not a multiple of three, more than 65 536 vertices, numbers that are not finite), a mask mode code outside the seven known ones (imported as Add) |
| `layer_styles` | per layer: a style in switch state 3 (not sampled; state 0 is imported as a style switched off, 988), Pattern Overlay, an unknown style or parameter, a gradient (`GCst`, not decoded: EffectCraft's default gradient stays), keys on a popup or checkbox, a value that does not fit, styles on a layer that takes none here (adjustment, camera, light) |
| `markers` | a marker with an unknown cue type (imported as an event cue) |
| `text_layers` ("text layer features") | per text layer, what of it is not imported: wiggly and expression selectors and unmapped animator properties, Path Options or More Options with set values, Source Text keyframes or an expression, character style or paragraph fields that are not decoded and differ from the default (named by their slot, e.g. `/68`), an unknown capitalisation or justification code, manual kerning, No Break, the stroke line join, the Japanese leading model, a digit set other than the default and Arabic, control characters of unknown meaning (they are kept in the text; U+0003 is not one of them, see Text above). The text layer itself is imported |
| `shape_layers` ("shape layers") | what of a shape layer is not imported: shape items other than groups, rectangle, ellipse, polystar, path, fill, stroke, gradient fill and stroke, trim paths and round corners (merge paths, repeater, offset paths, ...), stroke dashes, taper and wave with values, item blend modes and composite order, shape direction, gradient colour stops other than the default, gradient highlight values other than 0, material options, groups nested deeper than 32, a line cap or join of the unsampled third kind (imported as it), a path without a readable path. The layer itself is imported |
| `lights` | each environment light (the layer is skipped; the other layers of the composition are imported) |
| `blend_modes` | a blend mode code that is not mapped (1, 3 = Dissolve or Dancing Dissolve, 21 / 22 = Alpha Add or Luminescent Premul, codes above 34; the layer stays Normal) |
| `transform_properties` | a stored transform property that is not imported (keyed or expression-driven orientation, separated position dimensions), and the stored camera and light options that are not mapped (Blur Level, Iris, Falloff, ...) |
| `time_remapping` | an enabled Time Remap without a readable keyframe |
| `approximated_keyframes` | a key imported in a simpler form (unknown interpolation code, bezier ease that cannot be decoded: a colour key with a non-zero ease, non-finite ease data; a mask path key with a speed other than 0, imported unverified) |
| `parent_links` | a parent that was not imported (a skipped camera, a missing layer) or that forms a loop |
| `track_mattes` | a track matte of unknown mode, whose matte layer was not imported or does not exist, a layer matted by itself, or a matte of the older "layer above" form on the top layer |
| `unsupported_layers` | an unknown layer type, a layer without a usable source, a precomposition loop (the layer is cut) |
| `unsupported_items` | a project item of an unknown kind |
| `damaged_data` | a header, key or value that could not be read, numbers that are not finite or beyond ±1e9 (default used); an unreadable text document (the text layer is imported with empty text), damaged style runs |

`limitations` states what the reader cannot see at all, so it can be neither imported nor counted:
frame blending, 3D material options, audio levels, guides, footage duration/frame rate, the mask colour, lock, feather falloff, motion blur and RotoBezier switches, the per-effect on/off switch, and for text per-character 3D, text on a path and style slots of unknown meaning (stored values of these that differ from the default are reported as text layer features). They are listed in every report.

## How fidelity is checked

- `crates/aep-import/tests/aepx.rs`: the `.aepx` of five fixtures imports to the same project and
  report as their `.aep` (the reader turns both into the same chunk tree, see
  [aep-format.md](aep-format.md), section 1, "The XML form"); over the whole local corpus all pairs
  import alike.
- `crates/aep-import/tests/effects_batch5.rs`: Invert, Fast Box Blur, Gaussian Blur, Drop Shadow,
  Glow, Hue/Saturation, Levels, the expression controls and Fill import without losses with the
  values they were made with (410-448, 512-516; 071, 073, 080, 214-219, 221, 303, 304), including
  After Effects' defaults where they differ from EffectCraft's.
- `crates/aep-import/tests/effects.rs`: Tint, Gradient Ramp and Exposure from the batch-4 fixtures
  (356-397; Exposure: defaults 389, Channels 390, Exposure 391, Offset 393, Gamma 394, Red
  Exposure 395, Bypass 396, keys 397 with 0.5 at 1 s):
  every converted value against the case's value (colours, points in pixels including outside the
  layer and on a 100x100 solid, popup, scatter, blend in percent, amount), keys and the value
  between them (363: 50 at 1 s; 364: grey at 1 s; 382: [950, 500] at 1 s), the defaults equal to
  EffectCraft's, stack order with a reported Gaussian Blur (366), the second instance (383), and
  lossless imports without `allowLosses`.
- `crates/aep-import/tests/combined.rs`: masks and effects on the same layer, an effect on a text
  layer, and the same through `.aepx` (built in-test from the fixtures: the effect parade of one
  fixture grafted onto the layer of another).
- `crates/aep-import/tests/import.rs`: the synthetic fixtures of `crates/aep/tests/fixtures` (232 files)
  (each the same one-solid project with one change) are imported and the resulting project is
  compared with the value the fixture was made with: composition size/rate/duration/work
  area/background, layer names, switches and timing, blend modes, parent links, static
  Position/Scale/Rotation/Opacity/Anchor with their units, key times and values, interpolation sides,
  ease, tangents, expressions, the NTSC rates, and one report category per unsupported feature.
  The report round-trips through JSON.
- `crates/aep-import/tests/masks.rs`: every mask fixture (090, 091, 093, 208, 232-237, 306, 309,
  328, 329) imports without losses with the values it was made with: vertices and tangents,
  open/closed, mode, inverted, feather, opacity, expansion, path and feather keys with ease.
- `crates/aep-import/tests/shapes.rs`: every shape fixture (238-242, 330-333, 451, 457, 462, 464,
  467, 469, 471, 473, 482, 483, 519) imports without losses: items in file order, names, sizes,
  positions, group transform, layer anchor, paints, trim paths, polystar fields, the defaults of
  every item. Render check against After Effects at 1 s on a 20 px grid (5 184 points per frame):
  451-482 equal but for 2-4 anti-aliased edge points per frame (at most 29/255 there), 519 3 edge
  points.
- `crates/aep-import/tests/mattes.rs` and `markers.rs`: track mattes (403-408; render check 403-408:
  equal on every grid point, 406 within 1/255), markers (122, 123, 249, 347-349, 509, 510) and Time
  Remapping (319, 354).
- `crates/aep-import/tests/text.rs`: every text fixture (110, 113, 114, 243, 245-248, 334-340,
  385, 386) imports with the values it was made with (text, paragraphs, size, fill, tracking,
  leading, faux bold/italic, caps, justification, box size and position, the style runs of 248 and
  340, the anchor rule with 385's stored anchor `[10, 20]` px, 386's U+0003 as a paragraph break
  like 243's CR while another control character is still reported), 114 imports lossless, a missing font is listed and refuses nothing, a text
  layer built from 113 with 022's parent link and 058's Opacity keys imports parented with both
  keys, and hostile text documents (garbage, truncations, lone surrogates, a 2 MB text, run
  lengths that overflow or exceed the text, NaN sizes and colours, a font index outside the list,
  50 000 levels of nesting) never panic and give usable Source Text.
- `crates/aep-import/tests/shapes_round2.rs`: masks on 705's text and 706's shape layer in layer
  pixels, the operators of 707-710 and their defaults (731), Offset Paths' Copy Offset (828).
- `crates/aep-import/tests/ease_round2.rs`: 597, 598, 599, 700, 730 between their keys within 0.5 px
  (colour 1e-3) of After Effects' read-backs.
- `crates/aep-import/tests/effects_round2.rs`: 711-723 import with the values they were made
  with (712: the eleven effects at After Effects' defaults; Set Matte and CC Radial Blur's default
  Type Scratch are reported).
- `crates/aep-import/tests/effects_round3.rs`: the 24 effects of 905 with their values and After
  Effects' defaults, the layer parameters of 905 and the Layer Controls of 220, 320, 321, hostile
  layer ids (missing, 0, out of range) reported or neutral.
- `crates/aep-import/tests/text_anim.rs`: the animators and range selectors of 579-594 and 114
  (every selector setting, every mapped property, 594's keyed Start with its ease), the character
  and paragraph settings of 538-578, 595, 596, and what is reported (548 a line join without a
  stroke is no loss; 552, 554, 572, 578 are reported).
- `crates/aep-import/tests/robustness.rs`: every fixture truncated at 40 places, with bit flips and
  runs of `0x00`/`0xff`, and files rewritten with hostile values in valid structure (zero or
  maximal sizes and time bases, NaN/infinity/huge numbers in every property, stretch 0, self and
  mutual parents, a precomposition that contains itself), hostile masks (no vertices, 10 points,
  196 611 points, NaN points and boxes, a short shape header, an unknown mode, a missing `mkif`,
  path keys without their shapes), and for effects: every parameter type in every
  definition, extreme defaults and maxima, valid and slider ranges, truncated and missing definitions, a wrong
  float-slider maximum, a group marker retyped as a slider, an Exposure beyond EffectCraft's range, non-finite and
  out-of-range values, an unknown match name, no shared definition list, 2 000 definitions and 200
  copies of an effect. The result is an error or a report; the project is always valid (keys
  sorted, finite values including effect parameters, no parent loops, mask paths finite with
  matching tangents).
- `crates/engine/src/tests_aep_import.rs`: the command, undo/redo, refusal, dry run, bad input and
  the menu entry.

No Adobe project or asset is used: the tests read only the generated fixtures. The render checks named
above compare EffectCraft with After Effects on the same generated cases; they are made by hand
and are not part of the tests.

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

Effects outside the table (Lumetri; EXtractoR: its channel choice is arbitrary data; Stroke: its path is a mask reference), environment lights, the ease of bezier Orientation keys, wiggly and expression selectors and the other text animator properties (Character Value, Character Alignment other than its default, the Hue / Saturation / Brightness / Opacity variants, 3D rotations, variable font axes), manual kerning, No Break, the stroke line join, the Japanese leading model, Source Text keyframes, layer-style gradients, Pattern Overlay and the switch state 3, Mesh Warp handles moved on their own,
separated dimensions, footage interpretation and the project settings (colour depth, working space)
need verified layouts in the reader first.
