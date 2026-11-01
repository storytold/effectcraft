# Mask and shape path editing

## Layer selection bounds

Layer controls follow closed enabled masks, including their ordered modes, inversion, opacity and
expansion. These are geometric selection bounds: feather, effects and footage alpha do not define
the box. Render allocation and effect coordinates continue to use source/content bounds.

Evaluated geometry is reused by a per-thread LRU, including across separate evaluation contexts.
Keys compare source bounds, evaluated paths, modes, inversion, opacity and expansion exactly.
Expressions are evaluated before lookup, so changes in dependencies or time cannot reuse stale
geometry. Storage is limited to 64 entries and 16,384 path elements. Rectangular masks (including clipping) and contained convex
Add masks use an exact direct calculation; other combinations use the curve arrangement.
Malformed input, a failed arrangement, or geometry above the interactive complexity limit of
4,096 input elements falls back to source bounds. A valid empty region has no selection box.
This fallback keeps controls usable and does not change the rendered mask.

## Free Transform Points

Select mask/shape paths in Timeline or select vertices in Composition, then use Free Transform
Points (Command/Ctrl+T with the default keymap). Explicit vertex subsets are retained. Multiple
paths on 2D layers can share a transform in composition space. Corner handles scale, inside the
box moves, and the outside edge rotates. Shift preserves proportions during corner scaling,
constrains translation to a composition axis, and snaps rotation to 45 degrees. Command/Ctrl
scales about the pivot; Command/Ctrl+Shift combines pivot scaling with uniform proportions.

Each gesture recomputes geometry from pointer-down, including modifier changes without pointer
motion. Zero/negative scale remains recoverable in the same gesture. Numeric `path.freeTransform`
commands stay incremental unless `fromStart:true` and a gesture merge key are supplied.

A pivot in the center follows geometry; an off-center pivot stays relative to its reference layer.
Returning it to the center restores following. Pivot snapping uses the box and visible path
vertices; Option/Alt temporarily disables snapping. The box retains its orientation between
gestures. Each completed geometry or pivot gesture has interactive Undo/Redo while the tool is
active. Pivot-only changes do not dirty the file. Exiting/restarting the tool clears its temporary
UI history; geometry remains in ordinary document history. `engine.execute edit.undo` operates on
document history; `ui.menu.invoke` exercises interactive history. Tool state is not serialized.

Implementation: `panels/free_transform.rs` owns the session targets, pivot, orientation and bounded
history. The engine owns project snapshots and exposes gesture/history APIs to the frontend.
Native menu state includes interactive Undo/Redo labels, including pivot-only steps.

## Shape paths

Editable Bezier Path items in shape layers use the same point selection, marquee, Free Transform,
pivot, modifiers and per-gesture Undo/Redo as mask paths. A shared selection may contain both
families. Shape group transforms remain part of the path-to-composition mapping; changing points
does not rewrite the layer or group transform. Unselected vertices remain unchanged.

Select a Path item or its Path property in Timeline to edit its vertices. Parametric Rectangle,
Ellipse and Polystar items retain their controls; use Convert To Bezier Path before editing their
individual vertices. Creation-time options also apply to shape creation. New shape items are selected and revealed in
Timeline. A newly closed Bezier path selects all of its vertices; an open Pen path keeps its last
vertex active until drawing ends with Enter/Escape. New parametric items clear the previous point selection.

## Timeline and creation

Command/Ctrl toggles property selection; Shift selects a visible range. New masks select their
vertices and reveal their Timeline row. Duplicating selected masks inserts copies below their
originals with fresh property IDs and one document Undo step. Overlapping reveal filters do not
create duplicate property rows.

During shape/mask creation, creation keys and the wheel adjust draft parameters. The same wheel
gesture does not also zoom Composition. Double-click creation keeps the Tools bar modifier rules.
Masks fit the selected layer in local source coordinates: source dimensions for solids, footage
and precompositions, or evaluated content bounds for text and shapes. Existing masks, layer
position, rotation and scale do not change that rectangle. Empty content creates no mask. New
shape layers retain the composition-based sizing and default shape geometry. Center Anchor Point in Layer
Content is available in the Transform menu; animated Position is compensated at the current time
(a key is set there if needed), without shifting other Position keys. These interaction changes are primarily validated in 2D.

Mask creation on rotated/scaled layers converts the complete preview path into layer space,
including both Bezier tangent arrays; it does not rebuild an axis-aligned rectangle.

## Provenance

Drawing options and selected creation-time input code were adapted from pravdomil's PR
[#1300](https://github.com/storytold/effectcraft/pull/1300), commit
`49a1374c1a67a9a6ace426301e017a3cebeb0883`.
This is a selective adaptation of code from this MIT OR Apache-2.0 repository.
No Adobe implementation or assets are included.
