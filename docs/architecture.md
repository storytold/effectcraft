# Architecture

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-10 · **Change:** minor (crate table checked against `crates/`: effect count 306, VP9 inter frames) · **Target:** Adobe After Effects 2026

EffectCraft is a stack of small Rust crates. The engine knows nothing about the user interface;
the egui frontend, the command-line tool and the MCP server all sit on top of the same `Session`
and drive it through the same command registry.

## 1. Crates and layering

Dependencies only point downward (or along the few same-layer edges listed below). `cargo xtask
layers` enforces this, and fails the build if a crate below L5 depends on egui, eframe, winit, rfd,
cpal or muda. Everything in L0 to L4, the egui UI and the web app also build for
`wasm32-unknown-unknown` (`cargo xtask wasm`); see [web.md](web.md).

| Layer | Crate (`effectcraft-…`) | Responsibility |
|---|---|---|
| L0 | `time` | `Tick` (254 016 000 000 per second), rational frame rates incl. NTSC, SMPTE and drop-frame timecode |
| L0 | `geom` | Vectors, matrices, quaternions, the layer transform (anchor, position, scale, orientation, rotation) |
| L0 | `color` | sRGB and linear, HSL/HSV, luminance, the 38 blend modes, label colors |
| L0 | `vp9enc` | VP9 encoder (profile 0, 8-bit 4:2:0, key and inter frames with motion search, loop filter and rate control; lossless at quality 100) for WebM export, from the VP9 bitstream specification |
| L0 | `opusenc` | Opus encoder: RFC 6716 SILK (NB/MB/WB), hybrid (SWB/FB) and CELT (FB) modes chosen by bitrate and application (audio / voice), 48 kHz 20 ms packets, mono/stereo, and the RFC 7845 `OpusHead`, for WebM export audio |
| L0 | `hevcenc` | HEVC (H.265) encoder from ITU-T H.265: Main / Main 10 4:2:0, IDR + P slices (quarter-pel motion, merge/AMVP), CABAC, deblocking, bitrate or constant-QP rate control, for MP4 (`hvc1`) export |
| L0 | `av1enc` | AV1 encoder from the AV1 bitstream specification: Main profile 8/10-bit 4:2:0, key + inter frames (quarter-pel motion), deblocking, for MP4 (`av01`) and WebM export |
| L1 | `raster` | Premultiplied float images, sampling, affine and projective warps, blurs, compositing (parallel with rayon) |
| L1 | `keyframe` | Animated values, keyframes with temporal ease and spatial Bezier, roving, hold, velocity |
| L1 | `segment` | Swappable trained models for Roto Brush and face tracking: the `MaskModel` and `FaceModel` interfaces, a registry of open-licensed models (authors, licence, URL, size, SHA-256), PyTorch checkpoint and TensorFlow Lite readers (with an interpreter), SHA-256, MobileSAM (TinyViT + Segment Anything decoder) and MediaPipe Face Landmarker (BlazeFace + Face Mesh V2) on rayon and ndarray's safe GEMM |
| L1 | `psd` | Photoshop PSD/PSB reader (layers, groups, masks, blend modes, text, layer effects, adjustment layers; 8/16/32-bit; RGB/CMYK/Gray/Lab) and a minimal writer, from Adobe's published format specification |
| L1 | `path` | Bezier paths, path operators (trim, offset, round corners, zig zag, twist, merge…), stroking, coverage masks |
| L2 | `project` | The document: items, compositions, layers, the property tree, render queue model, `.ecproj` serde |
| L2 | `text` | Fonts, shaping, layout, per-glyph geometry, text animators and selectors |
| L2 | `effects` | The effect registry (306 effects) and their CPU implementations |
| L2 | `svg` | SVG import (W3C SVG 1.1/2 static subset: shapes, paths, transforms, `use`, CSS, gradients) and rasterisation at any scale |
| L2 | `pdf` | PDF, PDF-compatible Illustrator (`.ai`) and EPS (PostScript subset) vector footage into the `svg` render tree: paths, fills, strokes, shadings, tiling patterns, clipping, text (embedded TrueType / CFF / Type 1 / Type 3 fonts; the bundled `text` fonts for the standard 14), images, blend modes, soft masks, optional-content layers |
| L2 | `model` | 3D models for Advanced 3D: glTF 2.0 (`.gltf`/`.glb`) and OBJ/MTL import (meshes, PBR metallic-roughness materials and textures, node hierarchy, skins, animations), parametric primitives, extruded/bevelled outline meshes and polygon triangulation |
| L2 | `track` | Motion tracking: feature/search region point tracking (pyramid normalized cross-correlation, Lucas–Kanade sub-pixel refinement), confidence, homography/affine/similarity solves |
| L3 | `render` | Evaluation and compositing: sources, masks, effects, transforms, 3D, motion blur, mattes, blending, layer cache, audio mixdown |
| L3 | `media` | Footage decoding (FilmCraft's pure-Rust codecs), image sequences, Photoshop and SVG stills, frame cache |
| L3 | `expr` | The expression engine (JavaScript via boa) with the After Effects object model |
| L3 | `export` | Render queue encoding: H.264, HEVC and AV1 MP4, ProRes, WebM (VP9 + alpha or AV1, Opus), PNG/JPEG/TIFF/EXR sequences, GIF, WAV/AIFF |
| L3 | `gpu` | The GPU compositor and GPU effects on wgpu compute shaders (Metal, Vulkan, Direct3D 12, WebGPU), checked against the CPU renderer |
| L3 | `interchange` | Premiere Pro interop through timeline interchange (FCP7 XML, FCPXML, OTIO, EDL, AAF, OMF via FilmCraft's `filmcraft-interchange`): sequences ↔ compositions (§6a) |
| L3 | `lottie` | Lottie JSON / dotLottie import and export (layers, precomps, eased and spatial keyframes, shapes, masks, mattes) with a warnings list for what Lottie cannot express |
| L3 | `plugin` | Effect plug-ins: loads sandboxed WebAssembly effect modules (plug-in API v1; the wasmi interpreter behind the `wasm` feature, on for native hosts) into the effect registry ([plugins.md](plugins.md)) |
| L4 | `engine` | `Session`: project, branching undo history, editor state, the command registry and menus, the ScriptUI window model (`scriptui`) |
| L4 | `script` | Scripting: JavaScript (boa) with an After Effects-style object model (`app.project`, comps, layers, properties, render queue) and ScriptUI (dialogs, palettes, dockable panels; script-host threads so dialogs block `show()`), whose edits run engine commands; AE match names |
| L4 | `host` | A fully wired `Session` (media, expressions, scripting, WebAssembly plug-ins, exporter) for the frontends |
| L5 | `ui-egui` | The desktop interface: docking, panels, viewer, timeline, graph editor, dialogs, control channel |
| L5 | `automation` | The MCP server, headless or bridged to the running app |
| L6 | apps `effectcraft`, `effectcraft-cli`, `effectcraft-web` | Desktop app; command-line tool (render, exec, get/set, MCP); the browser app (wasm32, [web.md](web.md)) |

Allowed same-layer edges: `path → keyframe, raster`, `text → path`, `pdf → svg, text`, `effects → project, text, path, track`,
`media / expr / export / gpu → render`, `export → media`, `lottie → format`, `host → engine, script`,
`script → engine`.

## 2. Time

All time is an integer `Tick`, 254 016 000 000 per second, the least common multiple of every
broadcast frame rate (×1001) and audio sample rate, so frame and sample positions are exact.
Keyframe times are in **layer time**; `layer_time = (comp_time - start_time) / stretch`, and a
Time Remap property overrides the source time. Commands snap layer times, keyframe times and
comp durations to whole frames, as After Effects does. Timecode is only a display.

## 3. Document model

```
Project { settings, items, render_queue }
Item    { id, name, label, parent folder, kind: Folder | Comp | Footage | Solid }
Comp    { size, pixel aspect, frame rate, duration, start timecode, background, work area,
          layers (index 0 = layer #1), markers, motion blur shutter, renderer }
Layer   { id, name, source (footage | comp | solid | text | shape | null | camera | light),
          in/out/start, stretch, switches, blend mode, track matte, parent, props }
```

Everything animatable is a `Property` in the layer's `PropGroup` tree, addressed by a path such as
`transform/position`, `effects/#1/blurriness`, `masks/#1/feather`, or by `@uid`. Effects, masks,
text animators and shape contents are groups in the same tree.

A project file (`.ecproj`) is the project as readable JSON, headed by `savedBy` (the EffectCraft
version that wrote it) and `schema`; `.ecprojx` is the same data as XML (File ▸ Save a Copy As
XML). Saving goes through `Project::to_file_json`, which checks that the text reads back before
anything is written (JSON can't hold NaN or infinity), and files are written atomically. Opening
a file that a newer EffectCraft saved shows a warning: fields this version doesn't know are
dropped if it saves the project. Save, Save As, Save a Copy and Increment and Save keep the
file's format (`.ecprojx` stays XML).

`schema` changes when a saved value changes meaning; a project from an older schema is converted
when it is opened (`effectcraft_engine::upgrade_effects`, which also refreshes effect instances
from the registry) and saved with the current one, which older versions refuse to open rather than
render wrongly. Schema 2 put effect positions on shape and text layers in effect space (§4,
#227). Opening a schema 1 project moves what an effect on a shape or text layer (not an adjustment
layer, whose effects see the comp below as before) holds as a position by half the comp size,
so points that were set keep their place in the comp: point parameters with their keyframes, Paint
stroke paths and clone positions, Puppet mesh seeds, pin rest points and positions, and Liquify
and Roto Brush strokes. A point parameter still at its default and not animated is left alone:
the old default sat at the comp's bottom-right corner, where most effects did nothing, and the
same value is now the layer centre, as in After Effects. Expression controls (Point Control…)
keep their values, because their meaning is up to the expressions that read them, and expressions
are not rewritten, so an expression that computes an effect point on a shape or text layer from
other values may need `+ [thisComp.width / 2, thisComp.height / 2]`. Effects that use the layer
bounds (edge pinning, wipes, Offset, generators) now cover the whole comp-sized bounds instead of
their bottom-right quarter. Tracking data of effects that analyse footage (Warp Stabilizer, 3D
Camera Tracker, Mocha shape) is left as it is: footage has a source rectangle, whose effect space
did not change.

## 4. Rendering

`Renderer::comp_frame(comp, t)` walks the layers bottom to top. For each visible layer it renders
the **source** (solid, footage frame, text, shape contents, or a nested comp), applies **masks**,
then **effects** in order, then **layer styles**, then the **transform** into comp space (with motion blur sub-samples
when enabled), the **track matte**, and finally **blends** into the accumulator. Adjustment layers
apply their effects to the accumulator.

Effects run in **effect space** (`effectcraft_render::effect_bounds`): layer pixels measured from
the top-left of the layer's effect bounds, which are the source rectangle, or for layers without
one (shape, text) a comp-sized rectangle centred on the layer's origin that their content
surrounds. A shape layer's default effect point (the bounds' centre) is therefore its content's
origin, as in After Effects. The renderer moves the layer buffer into effect space for the whole
stack (`Buf::rebase`, on the CPU and the GPU alike) and hands effects everything else in it too:
masks, other layers' pixels for layer parameters (each in its own effect space), the layer's own
frames at other times (`Renderer::layer_input`, also what Puppet, Roto Brush and the Layer panel
read) and the comp camera. Effect point handles, Puppet pins and the Layer panel map effect space
to the comp with `EvalCtx::effect_to_comp`. Adjustment layers run their effects on the comp below
as it is.

Runs of 3D layers are composited per pixel through the
active camera, with lights, shadows and depth of field (Classic 3D), or rasterised as meshes by
the Advanced 3D renderer (below).

**Advanced 3D** (Composition Settings ▸ 3D Renderer ▸ Advanced 3D, `comp.renderer`;
`crates/render/src/three_d/adv`, `crates/model`). A run of 3D layers becomes one scene of
world-space triangles: 3D model layers (glTF 2.0 / OBJ footage, mapped from +Y-up model units
into the layer by Geometry Options ▸ Model Scale; skins and animation clips played along layer
time), primitive layers (Layer ▸ New ▸ Cube/Sphere/Plane/Torus/Cone/Cylinder, parametric in
Geometry Options), text and shape layers with Geometry Options ▸ Extrusion Depth (glyph and
fill outlines flattened, triangulated with holes and extruded with Angular/Concave/Convex
bevels, per character so per-character 3D carries over), and a textured card for every other
3D layer. Classic 3D doesn't draw model or primitive layers, so adding one switches a Classic 3D
comp to Advanced 3D in the same undo step, as in After Effects. Shading is physically based in linear light (glTF metallic-roughness; Cook–Torrance
GGX specular, Smith–Schlick geometry, Schlick Fresnel, Lambert diffuse, after Karis 2013):
parallel, spot and point lights with After Effects' falloffs, ambient lights, image-based
light from an Environment light whose Source (or the comp's Environment Layer, Layer ▸
Environment Layer) is an equirectangular image (cosine irradiance map; roughness-indexed mip
chain with the split-sum environment BRDF fit), and percentage-closer-filtered shadow maps
(spot: perspective, parallel: orthographic over the scene, point: six cube faces). Cards keep
Material Options (Accepts Lights/Shadows, Casts Shadows On/Only, Ambient, Diffuse, Specular);
models and primitives add Base Color, Metallic, Roughness, Emissive. Rasterisation runs at 2×2
supersampling with a reversed-Z depth buffer; opaque triangles resolve visibility first, the
transparent ones are sorted back to front and blended. The software rasteriser
(`adv::raster`) is the reference and the headless/CI path; `effectcraft-gpu` implements
`Accelerator::raster_3d` with a wgpu render pipeline (`advanced3d.wgsl`) that repeats the
shading step for step, and tests compare the two. Resolve, depth of field (a gather blur by
each pixel's circle of confusion from the camera's Depth of Field settings) and the conversion
back to the working encoding run on the CPU for both. **Motion blur** renders the scene at
sub-samples spread over the comp's shutter (angle, phase, samples per frame) — layers with the
Motion Blur switch, the camera and the lights at the sub-sample time, other layers at the frame
time — and averages them. Layers with a **blend mode, a track matte or Preserve Transparency**
are rendered on their own through the same camera and lights, hidden where the rest of the run
is nearer, and composited from the farthest to the nearest through the 2D path (a 3D matte is
itself drawn through the camera). **Environment Light Background** layers (Layer ▸ Light) draw
their equirectangular image behind the run, looked up by each pixel's view direction. Not yet:
collapsed 3D precomps (drawn flattened), morph targets, extruded strokes.

A **layer cache** keeps each layer's finished pixels (source, masks and effects) keyed by a hash of
its evaluated inputs, excluding the transform. Static and transform-only layers render once;
editing one layer re-renders only that layer. Each Render Queue job keeps its own layer cache
for its frames. Effects that read the clock directly are declared in
`effects::TIME_DEPENDENT`, and a test checks every registered effect against that list.

The viewer's **RAM preview** (`ui-egui/src/frames.rs`) keeps finished frames keyed by comp
content, frame, scale, 3D view / region of interest and a hash of the other render options
(Draft / Fast Previews, shadows, nested switches). Edits preserve unaffected comps' frames,
and undo can reuse earlier content. During rapid edits at the same time and view, the viewer
shows the newest completed revision while the exact requested revision renders; out-of-order
completions cannot move it backwards. This avoids freezing until a slider or layer drag stops.
Frames at other times or with other render options are never substituted. Obsolete queued jobs
are dropped; running jobs finish into the cache. When the budget (Settings ▸ Memory & CPU) is
full the least recently shown frames go first. The timeline's green bar counts cached frames
of the current content and viewer settings.
Resolution Auto renders the pixels the magnification needs, as in After Effects (Full above
50 %, Half down to 33.3 %, Third down to 25 %, then Quarter). Below 100 % the viewer averages
instead of skipping pixels (Viewer Zoom Quality More Accurate): GPU frames carry a mip chain
sampled trilinearly, CPU frames go up averaged down by the whole factor that leaves about one
texel per screen pixel.

A persistent **disk cache** (`render::disk_cache`, Settings ▸ Media & Disk Cache) backs both
the layer cache and the viewer's RAM preview: layer buffers that were slow to render and every
preview frame are written (LZ4, atomic rename, checksummed) under 128-bit content keys — the
comp's content hash (the comp, what it uses, footage file size and time) plus frame, scale and
view for frames; the layer key salted with the footage fingerprint for layers — so entries
survive restarts and never go stale. LRU eviction keeps it under the size limit; Edit ▸ Purge
empties it, `cache.diskStats` reports it, and the timeline draws disk-only frames in blue. In the
browser the frames live in the Origin Private File System: frame workers write them, the page
keeps the index (`DiskIndex`) and reads hits ([web.md](web.md#disk-cache)).

**Deferred readbacks** (`effectcraft-gpu` `deferred`): where a readback can't be waited for (WebGPU
in a browser worker) a frame renders in passes. `Accelerator::frame_begin` / `pass_begin` /
`pass_missed` / `miss_gate`: a readback not there yet starts asynchronously and the step returns a
placeholder; the pass is missed, the layer cache's gate drops inserts until the pass ends, and the
caller (`remote::FrameServer::resume`) renders again once the device delivered. Readbacks are keyed
by content (a chain's input pixels and parameters; the frame's comp, time and options), so later
passes are served from them; a pass without a miss is the frame.

**Time effects** (Echo, Posterize Time, Timewarp…) read the layer at other times through
`EffectHost::self_at`: the renderer renders the layer's source and masks (plus, optionally, the
effects before it) at that layer time via `Renderer::layer_input`, cached under separate *input*
keys, with a nesting-depth guard.

**Audio** is mixed by `render::audio::mix_comp` in blocks: Audio switch, solo, Audio Levels and
the Effect > Audio effects (`effects::audio_fx`, applied per layer with a pre-roll so blocks are
independent). Export and preview playback share it; the desktop app plays it through cpal and the
audio clock drives preview playback (`ui-egui::audio`). The sound starts once the frames ahead
are cached; until then, and whenever playback reaches a frame that isn't cached, every frame
shows as it renders, silently, rather than being skipped. Layer and frame caches ignore the
switches that don't change pixels (Audio, Lock, Shy, Hide Shy Layers: `Switches::pixels`,
`Comp::same_pixels`), so toggling them keeps the cached frames.

**Layer styles** (Layer ▸ Layer Styles; `crates/render/src/styles.rs`) render in layer space and
may grow the layer's bounds. Drop Shadow and Outer Glow become separate passes composited below the
layer with their own blend modes; the interior styles, Stroke and Bevel and Emboss are baked into
the layer body. Interior styles are clipped to the layer: they blend onto its colour inside its
shape and keep its alpha, so an anti-aliased edge mixes the style and the background only. Layer
opacity fades the whole stack; Knockout and the R/G/B channel switches apply at composite time. Styled pixels are cached separately from the layer content, so editing a style
reuses the cached source/masks/effects. Global Light is one setting per comp, mirrored into every
layer's Blending Options and kept in step after each edit.

**Paint and Puppet** are effects whose instances carry nested groups. Paint (`effects::paint`)
holds Brush / Clone / Eraser strokes (Path, Stroke Options, Transform, hidden Duration span);
strokes rasterize as brush-tip dabs and composite in order, and the layer cache key includes which
strokes are visible at the frame. Puppet (`effects::puppet`) holds meshes and pins: the mesh is
traced from the input alpha, expanded, and triangulated (Delaunay + constraint recovery, cached
by content); pins drive an as-rigid-as-possible solve (Igarashi et al. 2005) each frame and the
input is texture-mapped through the deformed triangles. Pins are selected like properties
(`puppet.selectPins`, marquee, Select All by kind) and rigged with expressions: Points Follow
Nulls / Nulls Follow Points take pins as well as paths, and `puppet.follow` gives follower pins a
Position expression that trails the leader by a delay. The renderer flattens nested effect
groups into `Params` keys (`effects::flatten_params`). Commands: `paint.*`, `puppet.*`.

Half, Third and Quarter resolution render proportionally fewer pixels end to end. Footage
starts at the render's size where the source can make it more cheaply than the full frame
(`FootageSource::frame_at_size`): movies in opaque 4:2:0 Y'CbCr are box-filtered to half size
before conversion (a quarter of the conversions; the result differs from converting first only
in blocks where colours clip) and resampled from there, as other frames are from full size.

**GPU compositor** (`crates/gpu`; Project Settings ▸ Video Rendering and Effects ▸ Mercury GPU
Acceleration, the default, or Mercury Software Only; `render.backend`). The CPU renderer is the
reference and keeps rendering layer content (sources, masks, CPU effects, layer styles) into the
layer cache. The render crate defines an `Accelerator` trait; `effectcraft_gpu::Gpu` implements it
on wgpu compute shaders. `RenderOpts::backend` picks `Cpu`, `Gpu` or `Auto` (an accelerator
attached and the project's renderer the GPU). Auto renders each comp's top-level frames on
whichever compositor measured faster for it (`render::auto::AutoPick`, kept by the accelerator):
per comp, output scale and path (readback or viewer display) it averages CPU and GPU frame times
(warm-up on both sides, the fastest warm-up frame starting the average, single stalls capped),
picks the GPU unless the CPU is clearly faster, and re-measures the other side every 48 frames.
Render Queue jobs don't use Auto: every frame of a job renders on the project's renderer (the
GPU compositor with Mercury GPU Acceleration), so runs of a render don't differ by timing.
Light comps (a few small layers, which the CPU composites only within their bounds) then stay on
the CPU instead of paying for full-frame passes and a readback. A GPU frame walks the comp like
`draw_comp`: cached layer buffers are uploaded once per buffer, then transformed with the CPU's
sampling (nearest, bilinear, Catmull-Rom bicubic, the same minification pre-filter), motion-blur
sub-samples are accumulated, and track mattes, Preserve Transparency, layer style passes,
knockout, all 38 blend modes (a WGSL port of `color::blend`), the 8/16 bpc clamp-and-quantise steps
and colour-space conversions run on the GPU. Classic 3D runs composite on the GPU too
(`gpu::classic3d`): the render crate prepares the planes (`Renderer::prepare_3d_run`: layer buffers
with their bokeh depth of field, homographies per motion-blur sub-sample, materials, lights, track
mattes), the GPU packs the buffers into one atlas texture and one kernel does the per-pixel
fragment sort (intersecting planes, coplanar stack order), Blinn-Phong lighting, ray-cast shadows
against the caster planes (Shadow Diffusion, Light Transmission) and blending. Adjustment layers
run their effect stacks on the GPU-resident comp (`Renderer::run_effects_on` with an `FxTarget`):
runs of GPU effects stay on the device and only non-GPU effects read back and upload. Advanced 3D
runs render on the GPU end to end (`gpu::adv3d`, `Renderer::prepare_adv_run` /
`Accelerator::render_3d`): each motion-blur sub-sample's scene is rasterised (`advanced3d.wgsl`),
then compute kernels (`adv3d.wgsl`) resolve the 2×2 supersampling, average the sub-samples
(nearest depth), apply the depth-based iris depth of field with the Classic 3D bokeh spans
(`three_d::bokeh::kernel_spans`, highlight boost, progressive blur levels) and composite over the
GPU canvas; only the depth of field reads back its 8-byte radius range. Runs whose layers need
the 2D compositing path (blend modes, track mattes, Preserve Transparency) come to the GPU as a
`three_d::adv::SplitRun` (`Renderer::split_adv_run`, `draw_run`'s steps as data): the plain
layers render as one scene over the canvas, then each special layer, farthest first, renders on
its own, is hidden where the main scene is nearer (`occlude`), and goes through the 2D matte /
Preserve Transparency / blend kernels (a 3D track matte renders solo through the camera).
Environment Light Background layers draw their sky in a kernel (`adv_sky`, from
`Renderer::sky_draw`) before the rest of the run, in Classic and Advanced 3D comps. Wireframe
outlines draw on the GPU from the CPU's pixel list (`Renderer::wireframe_pixels`). GPU effects
(`effects::GPU_EFFECTS`, 280 of them: blurs, colour correction, keying incl. Key Light, mattes,
channel, stylize, distortion and warps (Warp, Bezier Warp, Smear, Reshape, CC Bend It, CC Page
Turn, CC Bender, CC Blobbylize), Cartoon, bevels, shapes, the transitions (CC light family, CC
transitions, Block Dissolve), perspective (Radial Shadow, CC Cylinder / Sphere / Spotlight /
Environment, 3D Glasses), generators, noise, grain, text (Numbers, Timecode), time, 3D Channel (aux channels as an
extra texture), Immersive Video, colour management (colour programs from
`effects::color_program`, LUTs in a storage buffer) and the simulations' render passes (the
simulation stays on the CPU and hands its plan — sprites, pieces, blobs, grids — to a tiled
rasteriser; the particle effects too, `gpu::fx_particles`: CC Particle World / Systems II,
Particle Playground with its Layer Map frames in one atlas, CC Ball Action, CC Pixel Polly, CC
Scatterize; plans too large for one item table draw in several passes over the previous one),
and the geometry generators (Lightning, Advanced Lightning, Radio Waves, Stroke, Scribble,
Vegas, Write-on, Audio Spectrum / Waveform, Basic / Path Text: bolts, strokes, waves, audio
marks and glyphs are planned on the CPU, shared with the CPU effect, and rasterised in tiles
with the CPU's coverage functions); see
[effects.md](effects.md)) repeat the CPU effect's steps (padding, box radii, parameters, hashes)
as kernels, in one module per family (`gpu::fx_*` with `shaders/fx_*.wgsl`); consecutive GPU
effects run as one chain with one upload and one readback. Statistics that need the whole frame
(Auto Levels / Contrast / Color, Equalize, Shadow/Highlight, Color Stabilizer, Remove Grain's
noise level) are measured on the CPU from one readback and applied on the GPU; effects reading
other frames (Echo, Posterize Time, Time Difference, Time Displacement, CC Force Motion Blur, CC
Wide Time, Pixel Motion Blur, Timewarp) upload the frames the host renders (`time_frames`) and
combine them on the GPU; Time Displacement's per-pixel times and Timewarp's motion vectors
(`time_fx::timewarp_plan`) are computed on the CPU. Numbers and Timecode rasterise their glyph
coverage on the CPU (`textfx::text_layer`) and composite it on the GPU. Other layers an effect
reads (gradients, reveals, environments) are fitted on the CPU (`util::fit_layer`) and uploaded.
Where f32 cannot follow the CPU's f64, the CPU solves the part that needs it and the GPU uses
the result: Warp's Fisheye and Twist past 50 % or with a distortion (a Newton inverse that
wanders chaotically near the crease) sample the CPU's inverse map (`warp_inverse_map`). Settings
a kernel cannot match still render on the CPU (`effects::catalog::gpu_supported`: Cell
Pattern's HQ variants, Turbulent Displace's locked pinning and the controls in
`CPU_ONLY_CONTROLS`). Tests render
scenes on both paths and compare them (≤ 1/255 at 8 bpc, ≤ 1e-3 at 32 bpc); they skip without an
adapter. Working textures (premultiplied RGBA f32) come from a pool: a released texture is reused
once every encoder that could still read it has been submitted, transparent inputs share one
zero texture per size, readback staging buffers are reused, and the 8/16 bpc quantisation after
each layer is fused into the layer's composite kernel; under test, pooled scratch images start
as NaNs (a kernel that skips pixels fails the oracle) and validation errors panic. The desktop viewer builds the
`Gpu` on egui-wgpu's device and shows frames from GPU textures without reading them back
(`ui-egui::frames`); headless renders, the CLI (unless `--gpu`) and CI use the CPU. Outside
tests, device errors are logged rather than fatal. A viewer frame that runs out of video memory
(`Gpu::within_memory`) renders on the CPU, the uploaded layers and pooled textures are freed, and
the RAM preview keeps half as many GPU frames from then on. A frame whose render panics is
rendered again on the CPU or released, never left in progress. Native readbacks and timing waits
share a finite deadline; failure retires pending readbacks exactly once, and late results are
discarded. The desktop executable and browser page own their shared device's error/loss handlers and forward
failures to the compositor through a weak notifier. Libraries borrowing a device preserve its
host's handlers. A host fault disables shared-device preview acceleration without editing the project or its
renderer preference; materialized CPU frames remain usable and late GPU textures cannot return
to the preview cache. Scoped allocation failures retain the adaptive budget/CPU retry behavior.

**Recovery boundary:** the host installs handlers through eframe's CreationContext, after egui's
presentation pipelines but before EffectCraft's pipelines. This does not cover that earlier egui
startup window. eframe 0.36.2 supports lost-surface recreation, not replacing its live device and
all presentation resources. Device loss therefore requires restarting the presentation backend;
CPU compositing alone cannot restore that display. No automatic restart or persistent safe-mode
sentinel is implemented here. Browser scope futures are not synchronously awaited; the browser page
forwards shared-device errors, while owned worker devices install their own handlers. On the web
(WebGPU) the GPU composites viewer frames; steps that need a readback fall back to the CPU (the
Info panel's pixel readout reads GPU frames back asynchronously). **GPU particles**
(`effects::psim`, `gpu::particles`): CC Particle World, CC Particle Systems II and Particle
Playground cannons without interacting forces evolve every particle independently, so the GPU
backend (reached through `EffectHost::particles` when the GPU compositor is active) simulates one
particle per invocation from the CPU's birth schedule and keeps per-key state checkpoints on the
GPU, like `SimCache`; the CPU simulation stays the oracle (tests compare ids and positions).
`effectcraft-cli bench --gpu` reports CPU vs GPU ms/frame, speed-up, Auto's ms/frame, pixel
agreement and the steady-state upload / readback MB per frame for every comp plus an
adjustment-layer comp.

**Colour and bit depth** (`crates/render/src/color.rs`, `crates/color/src/space.rs`). Pixels are
`f32`, but 8 and 16 bpc projects clamp and quantise each layer after its source and masks and
after every effect, and the comp after every layer (16 bpc uses 0..32768), so over-range values
(Add, Screen, Exposure…) only survive in 32 bpc; blend modes without HDR support clamp their
inputs in 32 bpc. With a working space (sRGB, Rec. 709, Rec. 2020, Display P3; matrices derived
from the published primaries), footage is converted from its colour profile (stream metadata or
Interpret Footage, else sRGB) and the top-level comp is converted to the sRGB display. Linearize
Working Space runs everything in linear light; Blend Colors Using 1.0 Gamma linearises only for
blending. Layer cache keys include these settings.

**Frame blending**: footage and precomp layers whose source time falls between source frames
(rate conform, stretch, remap) blend the two neighbouring frames, by cross-fade (Frame Mix) or by
hierarchical block-matching optical flow and a bidirectional warp (Pixel Motion,
`raster::flow`).

**Collapse Transformations**: a collapsed precomp layer without masks, effects or styles draws
its nested layers straight into the parent with concatenated transforms (one resample), so
nested blend modes and adjustment layers act on the parent's layers; nested 3D layers use the
parent's camera and lights and, when the precomp layer is 3D, are depth-sorted with the parent's
3D run. Masks, effects or styles force a flattened render. Text layers are always rasterised at
their on-screen scale, as text is vector in After Effects; on shape layers and vector footage the
switch is Continuously Rasterize and does the same. Quality: Draft samples
nearest-neighbour, Wireframe draws the layer bounds. Slip edit (`layer.slip`, Alt+PageUp/Down,
dragging the source bar in the timeline) moves the source under fixed in/out points.

**Nested comps** show nothing before they start or after they end (`Comp::covers`), even when
the precomp layer is trimmed or remapped past them. A comp with **Preserve frame rate when nested
or in render queue** shows only its own frames when nested (`EvalCtx::nested_time` floors the
precomp layer's source time to its frame rate) and renders at its own rate in the Render Queue;
one with **Preserve resolution when nested** renders at full size inside a comp drawn at a lower
resolution (the buffer's scale places it).

**Motion blur** has one gate, `Renderer::mb_on`: the render options (Render Settings ▸ Motion
Blur, previews), the comp's switch and the layer's switch, limited by the precomp layers above
when switches affect nested comps. Layer motion, mask motion blur (the comp's Samples Per Frame,
four in Draft), effects that read the shutter and Advanced 3D all follow it, and the layer cache
key of a layer with masks or effects records it.

**Motion tracking** (Animation ▸ Track Motion / Stabilize Motion, Window ▸ Tracker): a tracker
is a `Tracker` group under the layer's Motion Trackers group, with Track Point groups (Feature
Center, Feature Size, Search Offset, Search Size, Confidence, Attach Point, Attach Point Offset)
keyframed per analysed frame. `track.analyze` renders the layer's source frames
(`Renderer::layer_source`) on a background thread (`engine::tracking`, polled like the render
queue; blocking with `wait`), and `effectcraft-track` matches each point in parallel. One undo step
covers an analysis. `track.apply` keys the target's Position/Rotation/Scale, the tracked layer's
Anchor Point and Position (Stabilize), or a Corner Pin effect (Parallel / Perspective).

**Mask tracking** (`track.mask`, `engine::mask_track`) runs the same kind of background job over
the layer's source frames: `effectcraft-track`'s mask tracker detects features inside the mask,
tracks them with pyramidal Lucas–Kanade and fits the chosen model (position … perspective) with
RANSAC; the motion moves the Mask Path's vertices and tangents and is keyed per frame. **Mask
Interpolation** (`mask.interpolate`) uses `effectcraft-path`'s smart interpolation (arc-length
vertex insertion, shape-context matching, rigid in-betweens) to key in-between shapes.
The two **Face Tracking** methods (`effectcraft_track::face`) fit a face inside the mask instead:
a skin colour model from the first frame, the face outline along rays from the skin component's
centre (an area-moment ellipse gives centre, size and roll), facial features as the non-skin
components inside it, and an active-shape point distribution model trained on synthetic face
shapes (our own generator, no external weights) that fills and checks the landmarks. The outline
keys the Mask Path; Detailed Features keys a Face Track Points effect, and
`track.extractFaceMeasurements` derives a keyed Face Measurements effect (and copies its keys).
With a trained face model chosen (`effectcraft_segment::face::FaceModel`, Settings ▸ Face
Tracking; `Session::models` hands it to the mask track), `FaceTracker::new_with` lets the model
find the face in the mask and follow it; the model's outline and named points replace the
classical steps, and chin and jaw still come from the outline so the measurements agree.

**Warp Stabilizer** (`effects::warp_stab`, `engine::warp`): `warp.analyze` renders the layer's
input to the effect (`Renderer::layer_input`: source, masks and the effects above it) for every
frame on a background thread and stores `effectcraft-track`'s `WarpAnalysis` (per-frame
translation / similarity / homography fits) as JSON in the effect's hidden Analysis parameter,
with a key of the layer's source, In/Out, start, stretch and Time Remap. `Session::edit` clears
analyses whose key no longer matches and queues them; the desktop app re-analyses them in the
background. Rendering derives the stabilization plan (smoothed camera path or No Motion, framing,
auto-scale; cached per analysis and settings) and warps each frame. Subspace Warp also stores long
feature trajectories in the analysis, smooths them in a low-rank subspace (Liu et al. 2011) and
warps each frame with a content-preserving mesh (Liu et al. 2009) fitted to the smoothed
positions, rendered as a per-pixel mesh lookup; Synthesize Edges fills the
borders from neighbouring frames read with `EffectHost::self_at`. The effect lives in the
effects crate, hence the `effects → track` edge.

**Essential Graphics** (`project::essential`, `engine::commands::essential`): a comp's
`essential` field lists its exposed controls (properties, Media Replacement, groups, comments).
Every precomp layer of such a comp gets an Essential Properties group (`GroupKind::Essential`,
children `eg<control id>`), kept in step by `Session::edit`; editing a child overrides it for
that instance. The renderer draws an instance with overrides from a copy of the project with the
overridden source properties set (`essential::with_overrides`). Templates are `.ectemplate`
ZIPs: `manifest.json` (controls), `project.ecproj` (the comp and its dependencies), `media/…` and
`poster.png`; importing offsets every id past the project's (`essential::offset_ids`).
**Responsive Design — Time**: a time-stretched precomp maps time piecewise so its protected
marker regions play at 100 % (`eval::responsive_source_time`).

**Proxies and interpretation**: an item's `proxy` (footage or comp) is decoded in its place at
the source's nominal size when `RenderOpts::proxy` (Render Settings ▸ Proxy Use) allows it.
Interpret Footage's Separate Fields turns each field into a frame at twice the rate (the other
lines interpolated); pixel aspect stretches footage, solids and precomps in the comp
(`EvalCtx::par_ratio`, not inherited by children); Invert Alpha and Interpret As Linear Light
apply after decoding.

**3D Camera Tracker** (`effects::camera_tracker`, `engine::camera_track`, `effectcraft-track`'s
`camtrack`): `camera.analyze` / `track.camera` render the layer's input to the effect on a
background thread and run structure from motion in two steps. Step 1 follows Shi–Tomasi features
through the clip with pyramidal Lucas–Kanade (forward–backward checked, re-detected where the frame
has none) into long 2D tracks. Step 2 solves the camera: parallax keyframes; a two-view start from
the essential matrix (normalised 8-point in RANSAC) or a planar homography decomposition; incremental
resection and triangulation; a sparse Levenberg–Marquardt bundle adjustment (Schur complement over
the points, Huber loss) with the focal length fixed, shared, or per frame; a log-spaced and
golden-section focal search for Fixed Angle of View / Variable Zoom; a rotation-only model for tripod
pans, chosen by Auto Detect when it explains the tracks as well; with Solve Lens Distortion (or
Detailed Analysis) radial distortion `k1, k2` is adjusted jointly in a final bundle adjustment on
the raw tracks and the solve repeated on tracks undistorted with that estimate; Undistort Footage
renders the input through the inverse model. Tracks and solve are stored as JSON
in hidden effect parameters with keys: changing the layer's frames clears both, changing Shot Type,
Angle of View, Solve Method or deleting points only re-solves. The solve's canonical frame maps to
comp space so the first frame's camera is the default comp camera (or so a chosen ground plane is
the X-Z plane at the origin); `camera.createFromSolve` keys a one-node "3D Tracker Camera" on every
frame and places text, solids, nulls or a shadow catcher and light on the target plane.

**Roto Brush & Refine Edge** (`effectcraft_track::roto`, `effects::roto`, `engine::roto`):
strokes (foreground, background, Refine Edge) are stored as JSON in the effect's hidden Strokes
parameter with the base frame and segmentation span. Each frame is segmented by graph cut
(Boykov–Jolly hard constraints, GrabCut colour mixtures, our own Boykov–Kolmogorov max-flow,
coarse to fine) and propagated to the next frame by warping the matte with block optical flow and
re-cutting in a Search Radius band; frames with correction strokes are re-cut with the warped
matte as a soft prior. With Version 2.0 / 3.0 and a trained model chosen (`effectcraft_segment`,
Settings ▸ Roto Brush), the model's foreground probability, prompted by the strokes or by the
warped matte, becomes a strong prior for the same cut (falling back to the classic result when it
disagrees with the flow); the model's id is part of the chain seed. Refine Edge bands get guided-filter + closed-form matting and
foreground-colour decontamination. Segmentations are derived data: each frame's chain key (Input
Key, settings, strokes from the base frame out) indexes a process-wide cache that renders fill on
demand (`self_at`) and `roto.propagate` fills in the background; `Session::edit` keeps the Input
Key in step with the layer's source, masks and upstream effects. Freeze stores final 8-bit mattes
in the effect.

**Content-Aware Fill** (`raster::inpaint`, `engine::commands::content_fill`): the hole is
where the layer's masked source (`Renderer::layer_input` with no effects) is under half
opacity, grown by Alpha Expansion. Optical flow between neighbouring membrane-filled frames is
*completed* inside the holes (membrane interpolation of the surrounding tiles' motion); each
missing pixel follows it forwards and backwards to the nearest frame (or reference frame) where
it is visible, the two candidates blended by temporal distance, with Lighting Correction adding
the membrane of the border mismatch (Poisson-style). What no frame shows is synthesised by
PatchMatch (Barnes et al. 2009) inside Wexler et al.'s (2007) multi-scale EM: Object refines each
frame starting from the previous result warped along the flow; Surface carries one synthesised
fill through the range; Edge Blend is the membrane fill alone. The frames are written as a PNG
sequence and imported into a "Fill" layer above the source. It runs as a generic **background
task** (`engine::jobs`: a closure with progress and cancel on a thread, inline on wasm32 or
with `wait`, whose result is applied on the UI thread as one undo step); the Progress panel and
`jobs.list` / `jobs.cancel` show these tasks together with the render queue and the analyses.

**Scene Edit Detection** (`raster::cuts`) compares consecutive frames by colour-histogram
distance and motion-compensated (block-flow warped) luma error, takes their geometric mean and
marks frames that exceed a threshold and several times their neighbours' median; the cuts
become layer markers, split layers, or split-and-precomposed layers. **Auto-trace**
(`path::trace`) runs marching squares on the chosen channel, Douglas–Peucker simplification and
cubic fitting with Corner Roundness blending chord and smooth tangents; holes become Subtract
masks, and a work-area trace keys Mask Path per frame.

**Align Video to Data** reads data footage (File ▸ Import of `.json`, `.csv`, `.tsv`, kept as
text in the project). Supported data: JSON as an array of objects or an object holding one
(`{"samples": [...]}`), CSV/TSV with a header row (quoted fields allowed). The time key is the
`key` parameter or the first field named `time`, `timestamp`, `t`, `date`, `datetime`, `utc`,
`gps_time`, `seconds` or `time_s`; values may be ISO 8601 date-times (with `Z` or `±hh:mm`),
times of day (`hh:mm:ss.sss`), timecode (`hh:mm:ss:ff` at the footage rate), Unix seconds or
Unix milliseconds. The layer's Start Time becomes `dataStart + videoStart − firstSample`
(`videoStart` given, or the file's creation date on the desktop; reduced to a time of day when
the data has no dates).

**Lumetri Scopes** (`raster::scopes`) are density plots of the viewer frame (rendered over the
comp background) with BT.601 / BT.709 / BT.2020 luma and colour-difference coefficients.

## 5. Expressions

Expressions are JavaScript, run by boa, with After Effects' object model (`thisComp`, `thisLayer`,
`time`, `value`, `wiggle`, `loopOut`, vector maths on arrays, and so on). A syntax error keeps the
text, disables the expression and shows a warning, like After Effects. `sampleImage` renders the
sampled layer through the evaluating renderer's footage source (`EvalCtx::footage`), cached per
thread and frame; `footage(name)` reads data footage (JSON, CSV, TSV imported with File ▸ Import,
text kept in the project) through `sourceData`, `sourceText` and `dataValue`. `expr.errors`
lists failing expressions for the viewer's error bar. Each thread keeps one boa context; boa
leaves values on its stack whenever an exception unwinds through call frames (every pending
host request does), so a run that hits the stack limit gets a fresh context and is retried once.

## 5a. Scripting

`effectcraft-script` runs JavaScript (boa, a fresh context per run; the Script Console keeps one)
with an object model written from the behaviour the After Effects Scripting Guide documents:
`app`, `Project`, `ItemCollection`, `CompItem`/`FootageItem`/`FolderItem`, `LayerCollection`,
`AVLayer`/`TextLayer`/`ShapeLayer`/`CameraLayer`/`LightLayer`, `PropertyGroup`/`Property`
(values, keyframes, eases, expressions), `TextDocument`, `Shape`, `KeyframeEase`, `MarkerValue`,
`RenderQueue`/`RenderQueueItem`/`OutputModule`, `File`/`Folder` and the enums. The model lives in
`prelude.js`; it reads the session through `__query` (JSON snapshots of items, layers, property
nodes, values and keys) and edits only through `__exec`, which runs engine commands, so every
scripted edit is undoable and identical to the UI's. While a script runs the session is moved
into a thread-local the natives borrow (nothing borrowed enters the JS heap); the comp a call
targets is made active for that command only. `app.beginUndoGroup`/`endUndoGroup` fold the undo
steps in between into one named step. Match names (`ADBE Transform Group`, `ADBE Position`,
`ADBE Gaussian Blur 2`, `ADBE Vector Shape - Rect`…) come from a table in `matchnames.rs`; effect
parameters are `<effect match name>-0001…`. `File` reads are limited to the project's folder, and
writes and the network are refused, unless Preferences ▸ Scripting & Expressions ▸ Allow Scripts
to Write Files and Access Network is on. Entry points: the `script.run` command
(`Session::script`, set by the host), File ▸ Scripts ▸ Run Script File… (`.jsx`/`.js`; `.json`
command scripts still run as steps), Window ▸ Script Console, `effectcraft-cli script` and the
MCP `run_script` tool.

## 6. Commands, the UI seam and automation

Every action is a command: an id, a label, a menu path, a shortcut, a parameter description,
`enabled()` and `run()`. The menu bar is a tree of command ids. The desktop UI, the command line,
the JSON control channel and the MCP server all dispatch by id, so anything a person can do from a
menu, an agent can do too. Every interactive widget registers an automation id, so agents can also
inspect and click the interface. See [agents.md](agents.md) and
[control-protocol.md](control-protocol.md).

Settings (`engine::prefs`), keyboard shortcut presets (`engine::shortcuts`) and auto-save /
crash recovery (`engine::autosave`) live in the engine too; they persist through a
`ConfigStore` the frontend provides (a directory on the desktop; the Origin Private File System,
or IndexedDB, on the web). Auto-saves go through the store's `FileOps` (the file system by
default, the browser storage on the web).

Renders and analyses run on background threads on the desktop. Where there are no threads (the
browser), `engine::offload` sends them to a second engine instance instead: the session's
`Offload` ships the serialized project, the footage it reads and the job (`WorkerRequest`) to a
Web Worker, and the worker's replies (progress, item status, the analysed property group) are
applied on the UI thread.
The shortcut dispatcher and the menus read the active preset. See
[preferences.md](preferences.md).

## 6a. Premiere Pro interop (timeline interchange)

After Effects' File ▸ Import / Export ▸ Adobe Premiere Pro Project… go through the public
interchange formats Premiere Pro itself reads and writes, not the native `.prproj` file (it has no
public specification; supporting it awaits a decision). `crates/interchange` (L3, no file I/O)
maps FilmCraft's timeline model (`filmcraft-project`, parsed and written by `filmcraft-interchange`,
pinned at the same git revision as the other `filmcraft-*` crates; pure Rust, builds for wasm32)
to compositions and back:

- **Import** (`file.importTimeline {path, format?, edlFrameRate?}`): Final Cut Pro 7 XML (`xmeml`,
  Premiere's File ▸ Export ▸ Final Cut Pro XML), FCPXML 1.9–1.11, OpenTimelineIO, CMX 3600 EDL, AAF and OMF.
  Sequences become comps (size, rate, duration, pixel aspect, start timecode, markers); video
  tracks become layers stacked by track (top track = top layer, later clips of a track above
  earlier ones); clips become footage layers with their in/out/start, speed and reverse (time
  stretch) and frame holds (Time Remap); Motion (position, scale / scale width, rotation, anchor
  point, Scale to Frame Size) and Opacity (blend mode) with keyframes, keyframe times converted
  from clip time to layer time; cross dissolves and dips become overlapping layers with opacity
  keyframes; nested sequences become precomps; audio tracks become audio-only layers with Audio
  Levels (clip volume + gain + track volume, crossfades as level keyframes); colour mattes and
  adjustment layers become solids; bins become Project panel folders under a folder named after
  the document; media that cannot be found become placeholders with the document's metadata
  (`missing` in the result). Effects, effect masks, titles/graphics and speed ramps are reported
  in `warnings`.
- **Export** (`file.exportTimeline {comp?, path, format?, prerender?, precomps?}`): one sequence
  per comp, one track per layer (bottom layer = V1), footage clips with their timing, Motion,
  Opacity and audio levels (linked audio clips for layers with sound), precomps as nested
  sequences, full-frame solids as colour mattes. Layers a timeline cannot represent (text, shapes,
  3D, effects, masks, track mattes, parenting, expressions, animated time remapping, non-uniform
  scale, blend modes Premiere lacks) are pre-rendered to ProRes 4444 with alpha next to the
  document (`prerender: unsupported`, the default; `all` renders every visual layer; `none` leaves
  them out with a warning) and referenced as media. The menu keeps After Effects' label "Adobe
  Premiere Pro Project…", but the file written is Final Cut Pro XML (`.xml`), which Premiere Pro
  imports with File ▸ Import; the dialog says so. FCPXML, OTIO, EDL, AAF and OMF are the other formats.

AAF and OMF (FilmCraft's structured-storage and Bento readers/writers) import and export too;
media embedded in them is not extracted, and the AAF writer keeps neither speed nor nesting.

## 7. Performance of everyday operations

`effectcraft-cli bench --ops [--small] [--layers N] [--comps N] [--footage N]` measures the
operations people do all day on a large generated project (`engine::perf::large_project`: 200
comps, 5,000 layers in the main comp plus 10 in each other comp, 300 footage items — 100 image
sequences of 240 frames, 100 movies, 100 stills — a 20-deep precomp chain, expressions on every
4th layer, Gaussian Blur on every 6th, parenting). The engine side is `engine::perf::ops_bench`;
the UI side (`ui-egui::bench`) drives the real `EffectcraftApp` headless: layout, painting and
tessellation, no window or GPU upload.

What keeps it fast:

- **Lazy open.** A project file carries its footage metadata, so File ▸ Open is read + parse +
  swap. Nothing is decoded until a frame needs it, expressions compile on first evaluation
  (cached by text), and system fonts are scanned on first need (a text layer asking for a family
  that isn't bundled, or a font menu or `text.fonts` listing the families); the desktop app starts
  that scan on a background thread at launch, reading name tables only. The footage files are checked afterwards in the background (`footage.check`, a
  "Checking footage" job in the Progress panel): missing items are flagged, items saved without
  metadata are probed, and the project is not marked modified. The desktop app does this after
  every open (`Session::check_footage_on_open`); headless sessions run `footage.check {wait:true}`.
- **No deep copies per frame.** Panels hold `Arc<Comp>` snapshots (`Session::active_comp_arc`,
  `Project::comp_arc`) instead of cloning a comp's layers every frame.
- **Virtualised lists.** The Timeline and the Project panel draw (and register automation ids
  for) only the rows in view; the Project panel scrolls vertically and reveals an item selected
  elsewhere. Menus listing every layer (track matte, parent) are built only while open.
- **Off the UI thread.** Project panel thumbnails render on a background thread (the previous
  thumbnail stays up meanwhile); auto-save snapshots the project (an `Arc` clone) and serialises
  and writes compact JSON on a background thread (`AutoSaveState::background`, on in the desktop
  app; inline on wasm32).
- **Undo is a pointer swap.** History stores `Arc<Project>` snapshots and comps are `Arc`s, so
  undo and redo of any edit are constant time. An edit copies the comps it changes (a one-layer
  edit in a 5,000-layer comp copies that comp: ~8 ms). An edit that changes nothing (a value
  set to what it already was) records no step: `Project::same_content` compares the comps the
  edit left shared by pointer, so the check costs about as much as the copy.
- **Modified follows undo.** The session keeps the snapshot it last opened or saved
  (`Session::saved_project`); the project is unmodified whenever the current snapshot is that
  one, so undoing (or jumping in the History panel, or rolling back a batch) to the saved state
  clears the modified mark. The window title shows the project and `*` while it is modified, and
  closing a modified project (Quit, the window's close button, New Project, Open, Open Recent,
  Close Project, Revert, the demo project, a Home template) asks Save / Don't Save / Cancel first
  (`ui-egui/src/panels/unsaved.rs`). Agents' `engine.execute` calls are never asked.

Numbers (Apple Silicon, 14 cores, release build, best of 3–20 runs; the machine was shared with
other builds, load average 150–220, so treat them as orders of magnitude). "Before" is the
commit before M13.14 with the same benchmark code:

| Operation | Before | After |
|---|---|---|
| Startup to first full UI frame (open + app + 2 frames) | 2,119 ms | 432 ms (1,482 ms at load 220) |
| File ▸ Open (125 MB pretty JSON; parse is ~95% of it) | 1,049 ms | 269–733 ms |
| Save (pretty JSON, 125 MB) | 845 ms | 275–359 ms |
| Auto-save, UI-thread cost | 530 ms (pretty, synchronous) | 0.1–0.2 ms (compact, background; 69–172 ms off-thread) |
| Steady frame, Standard workspace | 862 ms | 0.5–6.9 ms |
| Timeline frame scrolling 5,000 layers | 1,725 ms | 1.5–8.1 ms |
| Timeline frame, 500 layers twirled open | 1,508 ms | 1.8–5.5 ms |
| Project panel frame, 503 items, all folders open | 1.2 ms (first screenful only, no scrolling) | 0.4–2.8 ms (scrolling) |
| Small edit (one property) / undo + redo | 14.7 ms / 0.01 ms | 8.4–19.7 ms / 0.01 ms |
| Large edit (switch on 5,000 layers) / undo / redo | 62 / 0.03 / 0.01 ms | 18–57 / 0.02 / 0.01 ms |
| First rendered frame of the 5,000-layer comp at ¼ (viewer thread) | 5.7 s | 2.7–3.6 s (unchanged code; load) |

The before frame times were dominated by an O(layers²) menu build per visible timeline row and
by every panel deep-cloning the active comp each frame. Left for later: per-layer structural
sharing (so a one-layer edit doesn't copy its comp), a binary or compact project format (pretty
JSON parsing is now the bulk of open), and rendering 5,000-layer comps faster.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-10 | minor | Status line added; crate table checked against `crates/` (every crate listed); effect count and VP9 encoder description corrected |
