# Sony photo RAW import

Sony `.arw` photographs enter the normal still-footage pipeline through File > Import,
drag-and-drop, the Media Browser, `file.import`, and `effectcraft_media::probe_bytes`.
Numbered camera photographs remain individual stills, rather than automatically becoming
an image sequence. Replacing/reloading footage and the existing decoded-frame cache work
as for other stills. This is photo RAW support; AVI and RAW video are separate work.

## Decoder and colour

The media crate uses LightCraft's first-party `lightcraft-raw`, `lightcraft-tiff` and `lightcraft-color` crates,
MIT OR Apache-2.0, pinned to `b70ae1741e8236757377ca85eecaafb8788a82ab`. They have no UI
dependencies. The RAW adapter uses their public API, not a second implementation of the
Sony codec. No LibRaw, dcraw, Adobe binaries, Adobe camera matrices or GPL decoder code
are linked, copied or bundled. Their format sources are TIFF 6.0 / TIFF-EP, the public
DNG Specification 1.7 colour model, ITU-T T.81 Annex H (lossless JPEG), public tag-name
documentation and Sony ARW2 prose descriptions, including H. Dietz, *Sony ARW2 Compression:
Artifacts And Credible Repair* (Electronic Imaging 2016), and black-box analysis of files.

Pixels come from decompressed sensor samples, with CFA demosaicing (PPG), black subtraction,
white-level normalization, clip-aware highlight reconstruction, as-shot white balance,
camera-to-Rec.2020 colour conversion, conversion to linear sRGB, then EXIF orientation.
EffectCraft receives opaque float RGBA,
sRGB-encoded by default, with negative values and highlight headroom retained. Working Gamma
2.4 uses the same float-footage transfer as other linear float stills, with a separate cached frame.
Interpret
Footage > Preserve RGB keeps the developed linear-sRGB values, like float still footage.

LightCraft's spectral camera matrices are used when the camera is in its calibration table.
They are fitted from rawtoaces-data v1.1.0 (Apache-2.0) and CIE colour-matching functions
(CC-BY-4.0), not measured by EffectCraft. Their provenance is recorded in `NOTICE`,
`ATTRIBUTION.md` and the adapter's attribution sidecar. Without calibration, the documented neutral fallback treats white-balanced
camera RGB as linear sRGB and logs that limitation. It is a scene-referred starting point,
without the camera JPEG's tone curve, lens corrections or Lightroom edits. Camera colour
accuracy and Adobe Camera Raw look parity are not claimed. XMP sidecars are not applied.

Unsupported compression, corrupt data and allocation-limit violations return errors;
an embedded JPEG is never silently substituted for RAW sensor data. Files are limited to
256 MiB, sensor and developed images to 72 million pixels and 16384 pixels on either edge.
A 61 MP float RGBA frame occupies about 0.91 GiB in the normal cache, with temporary RAW,
mosaic and RGB buffers during first decode. This is substantially heavier than its
compressed `.arw` file. The existing cache always retains its newest frame even if it
exceeds the configured budget; additional photographs compete for that budget and cause
evictions. It also retains up to four spare pixel buffers outside the cache budget.
Use proxies for many high-resolution photographs. No reduced-resolution RAW decode or interactive RAW-development dialog is
included in this change.

## Coverage and local verification

Initial target: Sony ILCE-7RM4 (A7R IV), compressed ARW2 (`SonyRawFileType = 2`, TIFF
compression 32767, one full sensor strip and complete 32-pixel row blocks), sensor
dimensions 9600 x 6376 and nominal 14-bit data. Other Sony bodies
are not automatically considered validated by this integration. Uncompressed Bayer data
is covered by synthetic tests only. Lossless JPEG ARW tiles and older compressed layouts
are deliberately unsupported by this initial adapter until representative integration and
allocation-limit tests exist; LightCraft's wider decoder coverage is not an EffectCraft
support promise.

Four private A7R-IV compressed ARW2 photographs passed sensor import and a CPU composition
render on Windows with Rust 1.95, both directly through MediaPool and through the native
host's `file.import`, `comp.new`, `layer.addItem` and session renderer: three landscape-format photographs at 9504 x 6336 and one EXIF-8
portrait at 6336 x 9504. As-shot WB and LightCraft's fitted A7R-IV spectral profile were used;
float output stayed finite, negative/out-of-gamut values and headroom above 1 were retained,
and repeated frame requests returned the cached frame. Before/after SHA256s confirmed
unchanged originals. The debug native host verification process peaked at 2838 MiB working set;
its first import plus 1280-pixel composition/PNG checks took 2.6-4.6 seconds per photo
under concurrent builds. These are integration observations, not a comparative performance
benchmark or calibrated colour-accuracy certification.
The locally built desktop application also displayed an imported A7R-IV photograph in a
full-size composition through its normal command/control path; a window screenshot and
composition render confirmed visible sensor-derived footage. Browser RAW decoding and
packaged application builds have not been runtime-verified.

Run synthetic regressions with `cargo test -p effectcraft-media raw::tests --lib`.
They construct original small TIFF/EP mosaics and constant compressed ARW2 blocks in memory,
without any JPEG, and exercise orientation and cache
reuse, Preserve RGB and truncation/oversized/corrupt inputs; they contain no personal files.

For private local photographs:

```text
cargo run -p effectcraft-media --example sony_raw -- OUT_DIR PHOTO.ARW ...
cargo run -p effectcraft-host --example sony_raw_import -- OUT_DIR PHOTO.ARW ...
```

This reports camera, CFA, sample counts, levels, WB, output dimensions, finite float pixels
and frame-cache reuse; it writes only reduced PNG inspection images into `OUT_DIR`.
The host example additionally drives the same `file.import`, `comp.new`, `layer.addItem`
commands and session renderer as the desktop/CLI frontends.
Use an output directory outside tracked files. Original photographs, XMPs and Lightroom
catalogs are read-only test inputs and must not be committed or uploaded. A local test
permission does not grant an open-source asset licence.
