# File format parity

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-10 · **Change:** major (first per-format table, measured against the After Effects 2026 26.5 bundle and this repository's import/export registries) · **Target:** Adobe After Effects 2026

Every format After Effects reads or writes, with EffectCraft's support. After Effects' list comes
from its installed bundle (26.5.0.89: `Info.plist` `CFBundleDocumentTypes`, `Plug-ins/Format`,
`Resources/usd_plugins`) and its public documentation; ours from `crates/media/src/lib.rs`
(`STILL_EXTENSIONS`, `VIDEO_EXTENSIONS`, `MODEL_EXTENSIONS`, FilmCraft's `AUDIO_EXTENSIONS`),
`crates/engine/src/commands/file.rs` (`DATA_EXTENSIONS`) and `OutputFormat::ALL` in
`crates/project/src/render_queue.rs`. Video codecs come from FilmCraft's pure-Rust crates
([architecture.md](architecture.md)).

**Overall ≈ 50%** (estimated, weighted by use): modern delivery and interchange formats are
strong, but After Effects' own project, template and preset formats, which decide whether
existing work moves over, are 0%.

Fidelity key: **exact** (bit-exact or within a stated tolerance of a reference), **good** (round
trips and renders, checked by tests, not against After Effects), **partial** (named limitations),
**none**.

## After Effects' own formats

| Format | After Effects | EffectCraft read | Write | Fidelity / notes | Tested by |
|---|---|---|---|---|---|
| Project `.aep` | R/W (native) | none | none | Main file format; needs an owner decision on clean-room scope (#190, #248: py-aep as an external converter). Blocks beta | — |
| XML project `.aepx` | R/W | none | none | Same decision as `.aep` | — |
| Templates `.aet` / `.aetx` | R/W | none | none | Own `.ectemplate` (Home ▸ Templates, Save as Template) | `templates.rs` tests |
| Motion Graphics template `.mogrt` | W (and R in Premiere) | none | none | Own `.ectemplate` Essential Graphics templates; FilmCraft rejects our `.fcgt` export (#611) | `essential.rs` tests |
| Animation preset `.ffx` | R/W, 621 bundled | none | none | Own JSON `.ecpreset`; `.ffx` is out of clean-room scope (AGENTS.md §2) | `animation.rs` tests |
| Render Settings / Output Module templates `.ars` / `.aom` | R/W | none | none | Templates are stored in the project and settings instead | `render_templates.rs` tests |
| Scripts `.jsx` | R (run) | yes | — | After Effects object model and ScriptUI ([scripting-parity.md](scripting-parity.md)) | `crates/script` tests |
| Binary scripts `.jsxbin` | R (run) | none | — | No public specification: out of scope | — |
| Motion graphics JSON `.mgjson` | R (data) | none | none | Media Browser lists it; import accepts only `.json` / `.csv` / `.tsv` data | — |
| Captions `.aecap`, graphics `.aegraphic` | R/W | none | none | — | — |
| Native `.ecproj` | — | yes | yes | Versioned JSON; schema migrations tested; absolute footage paths, no relinking (gap 18) | engine project tests |

## Footage: stills

| Format | After Effects | Read | Write | Fidelity / notes |
|---|---|---|---|---|
| PNG | R/W | yes | yes (sequence) | good |
| JPEG | R/W | yes | yes (sequence) | good |
| TIFF | R/W | yes | yes (sequence) | good |
| OpenEXR (multi-layer, Cryptomatte) | R/W | yes | yes (32-bit sequence) | good; HTJ2K compression fails (#482); EXRs without an unnamed RGB layer decode twice (#474) |
| Photoshop PSD / PSB | R (footage, composition) / W (layers, sequence) | yes (layers, groups, masks, styles, smart objects) | Save Frame As ▸ Photoshop Layers; no Photoshop sequence output | good, from Adobe's published spec |
| Illustrator `.ai`, PDF, EPS | R | yes (vector, Continuously Rasterize, Create Shapes) | — | partial: no JPX / JBIG2 images, no Illustrator procset EPS |
| SVG | R | yes | — | good; pasting SVG/AI as shape layers (26.3) missing |
| GIF | R/W | yes | yes (animated) | good |
| BMP | R | yes | — | good |
| WebP | — | yes | — | beyond After Effects |
| Targa `.tga` | R/W | none | none | Media Browser lists it but import refuses (gap 17) |
| DPX / Cineon `.dpx` `.cin` | R/W | none | none | Same; common in film VFX |
| Radiance `.hdr` | R/W | none | none | Same |
| SGI, PICT, ElectricImage, RLA/RPF images | R | none | none | Legacy; RPF camera data import exists (`keys.rpfCameraImport`) |
| Camera Raw (DNG, CR3, NEF…) | R (Camera Raw) | none | none | LightCraft has decoders that could be shared |
| HEIF / AVIF | R (platform) | none | none | — |

## Footage: video and audio

| Format | After Effects | Read | Write | Fidelity / notes |
|---|---|---|---|---|
| MP4 / MOV with H.264 | R/W | yes | yes (own encoder, + AAC) | decoder bit-exact on conformance streams (FilmCraft) |
| HEVC (H.265) | R/W | yes | yes (MP4 `hvc1`, Main / Main 10) | encoder bit-exact with ffmpeg's decode; no B-frames / SAO, larger files |
| AV1 | R | yes | yes (MP4 `av01`, WebM) | beyond After Effects' export; no B-frames / CDEF |
| Apple ProRes (422 family, 4444) | R/W | yes | yes | good; 4444 colour tag wrong (#340); no ProRes RAW |
| WebM / Matroska (VP9, AV1, Opus) | R (limited) | yes, incl. VP9 alpha | yes | beyond After Effects |
| MXF (OP1a / OP-Atom) | R/W | none (FilmCraft reads it; not wired) | none | Media Browser lists it |
| AVI | R | none | none | Media Browser lists it |
| DNxHD / DNxHR | R/W | unverified (FilmCraft decodes it; reachable only inside `.mov`) | none | — |
| BRAW, R3D, ARRIRAW | R (plug-ins / SDKs) | none | none | proprietary SDKs |
| Variable-frame-rate phone video | R | unverified | — | needs a real-media corpus |
| WAV, AIFF | R/W | yes | yes (16-bit PCM) | good |
| MP3, AAC / M4A, FLAC, Ogg, Opus | R | yes | AAC and Opus inside video only | MP3 layer can go silent (#324) |

## 3D models

| Format | After Effects 26 | Read | Notes |
|---|---|---|---|
| glTF / GLB | R | yes (meshes, PBR, skins, animations) | good |
| OBJ / MTL | R | yes | good |
| FBX | R (`usdFbx`) | none | gap 13 |
| USD / USDZ | R (`usd_plugins`) | none | gap 13 |
| STL | R (`usdStl`) | none | gap 13 |
| Substance 3D materials `.sbsar` | R (26.0) | none | gap 13 |

## Interchange and data

| Format | After Effects | Read | Write | Notes |
|---|---|---|---|---|
| Premiere Pro `.prproj` | R (import) | none | none | Out of clean-room scope pending a decision; we interchange through FCP7 XML, FCPXML, OTIO, EDL, AAF and OMF instead (beyond After Effects) |
| Lottie JSON / dotLottie | via third-party plug-ins | yes | yes | beyond After Effects; a warnings list names what Lottie can't express |
| JSON / CSV / TSV data | R | yes (`footage()`, data layers) | — | good |
| LUTs (`.cube`, `.3dl`, `.csp`, `.spi1d/3d`, CDL) | R | yes (Apply Color LUT, OCIO effects) | — | Apply Color LUT bug (#514) |
| OpenColorIO configs | R (OCIO 2.5.1, ACES 2.0 in 26.5) | yes (OCIO v2 subset, Blender configs) | — | partial: no ACES 2.0 output transforms |
| Fonts (TTF, OTF, TTC, variable) | R | yes | — | good |
| Mocha shapes | R (Mocha AE) | own documented JSON | — | Mocha's format is not public |
| Vanishing Point `.vpe` | R | none | — | no public specification; the menu entry explains why |

## Render queue outputs (`OutputFormat::ALL`, 12)

H.264 MP4, HEVC MP4, AV1 MP4, ProRes MOV, WebM (VP9 or AV1 + Opus), PNG / JPEG / TIFF / OpenEXR
sequences, animated GIF, WAV, AIFF. Missing against After Effects + Media Encoder: uncompressed
or lossless single-file video (#372), DPX / Targa / Photoshop sequences, MXF, DNxHR, hardware
encoders ([hardware-parity.md](hardware-parity.md)), and rendering safely over an existing file
(#325).

## Estimate

**≈ 130–230 h**: `.aep` / `.aepx` import 80–160 h (after the owner decision), wiring TGA / DPX /
HDR / MXF / AVI / DNx import and DPX/TGA output 10–20 h, FBX / USD / STL and Substance 25–45 h,
relinking 6–12 h, `.mogrt`-compatible templates 15–30 h (partly parallel with the rest).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-10 | major | First file format parity document, from the After Effects 26.5 bundle and this repository's import/export registries |
