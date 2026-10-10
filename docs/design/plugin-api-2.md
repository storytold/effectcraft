# Proposal: effect plug-in context and optional controls

Status: design review, not an implemented or stable API. The shipping contract remains
[API 1](../plugins.md). This proposal follows the
[review of #330](https://github.com/storytold/effectcraft/pull/330#issuecomment-6082006351).
The rendering and Effect Controls fixes are a separate change; HTML panels are a separate
proposal. Neither feature is a prerequisite for accepting those fixes.

## Problem and scope

API 1 supplies the current image, flattened parameters and layer time. Effects that need
another layer's pixels, a linked point or a mask path cannot express those dependencies.
Effects with optional sections also need a declarative way to show controls when enabled,
without removing saved values, keyframes or expression references.

Propose an opt-in context API for Rust and sandboxed WebAssembly effects, plus grouped
parameters and conditional visibility. Keep API 1 modules, manifests and projects working
without changes. No HTML, script execution, filesystem access or native extension loading
is part of this proposal.

## Proposed contract

### Parameters and visibility

- Stable slash-separated parameter IDs identify nested groups. Group display names can
  change independently of property paths. Duplicate IDs and group/parameter collisions
  are rejected at load time.
- Numeric, checkbox and popup controls can drive inclusive visibility conditions on a
  parameter or group. Ancestor conditions are ANDed. Conditions use evaluated values at
  the current time, and cannot execute plug-in code during UI layout.
- Hidden controls retain their values, keys, expressions and command accessibility. Hidden
  points do not leave viewer handles. Re-enabling a section restores its controls and
  handles. Effect Controls and the timeline use the same visibility predicate.
- An optional-section count or checkbox is an ordinary undoable property change. This
  first version does not add arbitrary plug-in UI callbacks or dynamically create/delete
  properties when a section is revealed.
- Proposed additional types: `layer` (pixels and source stage), `layerpoint` (linked anchor
  with an availability flag), and `mask` (own-layer mask selection).

### Sources and dependencies

Each layer input explicitly selects Source, Masks, or Effects & Masks. Source and Masks
may refer to the owner; its complete Effects & Masks output is unavailable while that
same stack is executing. Missing or cyclic inputs return an explicit unavailable result,
never a transparent image that falsely looks like a successful read.

Use the renderer's dependency hashes for selected pixels, transforms, evaluated values
and transitive references. Do not disable the entire layer cache merely because a layer
parameter exists. Self Source/Masks reads remain cacheable. Unknown-time expression reads
and cyclic or over-budget dependency graphs may conservatively decline caching. Tests
must compare cached and fresh renders after same-frame dependency edits.

Time-dependent effects declare that dependency. API 1's existing time argument needs a
separate compatibility decision: use conservative time-aware keys, or introduce an
optional declaration whose absence preserves the existing rendering contract.

### Coordinates after #289

Use the effect-space model now on `main`. Do not reintroduce or access the removed
`EffectEnv::bounds_origin` field. Points and masks supplied to an effect must be normalized
to its effect space before preview scale and buffer padding are applied:

```text
buffer_point = effect_space_point * scale + buffer_offset
```

A linked anchor travels from the selected layer through composition space to the owner's
layer space, then through the renderer's layer-to-effect-space conversion. Singular
transforms or missing layers set availability to false. The UI uses the inverse mapping
for handles, including text/shape layers with negative geometry bounds.

Referenced images retain their own effect-space mapping. The context must identify that
mapping explicitly rather than assume all layer images share the owner's coordinates.
Raw mask paths include evaluated enabled/inverted flags, name and feather; they do not
silently bake mask mode, opacity or expansion into geometry. The mask index remains stable
when a mask is disabled.

### ABI and failure behavior

A candidate API 2 adds `ec_render_v2` with a pointer/length for a context descriptor;
API 1's `ec_render` stays unchanged. JSON versus a binary descriptor, and the precise
field names, are intentionally open for review. Native Rust effects receive equivalent
typed data and can retain their existing `render` implementation through a default method.

Referenced pixels and context are read-only inputs from the project's perspective. Only
validated output pixels are committed. An error, trap, exhausted fuel, invalid offset or
non-finite result preserves both the input pixels and original buffer bounds. Padding
must be declared before rendering and bounded before allocating.

Candidate limits, to be agreed before implementation:

| Resource | Proposed upper bound |
| --- | --- |
| API 1 controls | Existing 256, unchanged |
| API 2 controls / group depth | 2,048 / 8 |
| Visibility rules | 4,096 |
| Distinct referenced images / total referenced pixels | 64 / 16 megapixels |
| Mask paths / total flattened points | 1,024 / 262,144 |
| Padding / padded output | 4,096 pixels per edge / 64 megapixels |
| Context JSON / total transfer | 32 MiB / 512 MiB |

All count, alignment and offset arithmetic is checked before allocation or access.
Existing WebAssembly memory/fuel limits and the no-import sandbox continue to apply.

## Decisions requested

1. Is a new ABI version preferable to optional capabilities on API 1?
2. Should layer pixels, linked points and mask geometry land together or in smaller stages?
3. Is a JSON descriptor acceptable for the first version, or should the context be binary?
4. Are declarative group visibility and the proposed budgets appropriate?
5. What source-stage and coordinate metadata should be public, and how should unavailable
   inputs be represented without changing built-in effects' existing source semantics?

## Implementation and acceptance plan

After agreement on the contract, implement in stages: manifest validation and visibility;
typed renderer context and dependency keys; then the WebAssembly transport. Each stage
must work independently of HTML panels.

Acceptance tests must cover API 1 compatibility; grouped visibility in both editors;
hidden-handle removal and undo; self and external Source/Masks reads; transitive cache
invalidation; transformed linked anchors; shape/text effect-space coordinates at reduced
resolution and after padding; disabled masks; deterministic time evaluation; malformed
manifests and offsets; and atomic rollback after a failed render. Run `cargo xtask ci`
and inspect headless UI snapshots before proposing implementation for merge.
