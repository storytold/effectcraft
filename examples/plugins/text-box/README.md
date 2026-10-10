# TextBox

A linked Rust pixel effect for EffectCraft, id `org.effectcraft.text-box`, category **Text**.
Apply to a text layer using Effect → Text → TextBox or find TextBox in Effects & Presets.
No additional layers are created. The standard host registers the plugin at session startup.

Parameters are ordinary animatable effect properties: Padding X/Y, Fill Color/Opacity,
Corner Radius, Offset X/Y, Padding Left/Right/Top/Bottom, Stroke Width/Color, Matte.
Side padding **adds** to X/Y padding; set X/Y to zero for fully independent sides.
All six padding controls accept negative values (down to -10000 px), so the box can be
smaller than its source. If either dimension reaches zero, the box disappears instead of
flipping inside out.
Stroke is centered on the rectangle edge. Radius clamps to half the shorter rectangle edge.
All distances are layer pixels and are scaled for preview resolution.

**Matte** (off by default for compatibility) clips the source to the rounded rectangle's
interior. It follows padding, offset and corner radius, with antialiased edges. Its coverage
is independent of Fill Opacity, Fill Color alpha and Stroke Color alpha: set Fill Opacity
to zero for a completely invisible clipping box. Stroke Width does not enlarge the matte.
With Matte on, a collapsed box hides the source too. Layer Transform Opacity still controls
the combined text and box normally; use Fill Opacity to fade only the background.

Bounds come from finite alpha > 0, including antialiased pixels. Multiline text is measured
as one box. Leading/trailing spaces and blank lines do not enlarge it unless they change
placement of visible glyphs. Fully transparent input produces no box. The source remains
above the box; with Matte off, it stays visible even when offsets move the box away from it. Upstream masks/effects affect
bounds; downstream effects process both text and box. Use TextBox before glow/shadow effects
if the box should follow glyphs rather than their glow/shadow.

The plugin scans input once, then renders output rows in parallel. It stores no mutable render
state and works on concurrent frames. No independent bounds cache: the existing renderer caches
processed layer buffers. A separate cache without a trusted input revision would require a full
pixel hash and would not eliminate the O(N) input pass. Output has a 16M-pixel (256 MiB) allocation
limit; excessive geometry returns an error, logged by the host, retaining the unmodified frame.
This is an explicit failure, never silent clipping.

## Build and test

From the repository root:

```sh
cargo test -p effectcraft-text-box
cargo test -p effectcraft-host --test text_box --test plugins
cargo run --release -p effectcraft
```

This is a source-linked plugin, not a `.wasm`/DLL to load via Load Effect Plug-in. WASM v1 cannot
resize a framebuffer. `render_buffer` is a general Rust plugin host extension; no text-specific
logic was added to the renderer or text-layer implementation. Its default adapter preserves
existing in-place plugins and restores the original frame when they fail.
