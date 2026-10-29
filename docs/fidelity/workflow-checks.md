# Selection and preview workflow checks

These checks use original compositions and public interaction behavior. No Adobe assets or
implementation code are included. Adobe's [selection documentation](https://helpx.adobe.com/after-effects/desktop/work-with-layers/select-and-arrange-layers/selecting-arranging-layers.html)
describes Timeline range selection and Composition Shift-click/Shift-marquee selection.

`crates/ui-egui/tests/it/ui_workflow_selection.rs` exercises actual pointer and keyboard events:

- Timeline Shift-click selects the visible range, skipping locked layers; Ctrl/Command-click toggles a layer.
- A drag beginning in the empty Timeline outline selects the intersected layer rows without reordering them.
- Project Shift-click selects the displayed range and Ctrl/Command-click toggles an item.
- Project Clear uses the undoable project-item deletion command.
- Dragging a composition from Project into Render Queue queues that composition.
- Render Queue Delete removes its selected queue item without deleting selected Timeline layers.
- Composition marquee detects intersections through a layer's interior, including Shift toggle selection.
- Gaussian Blur parameter scrubbing activates Adaptive Resolution; release restores settled resolution.
  An explicitly selected Full resolution is preserved.

The existing `ui_tabs` tests exercise panel docking, tab rearrangement and splitter resizing.
`ui_viewer`, `ui_effect_controls`, and `ui_ram_preview` check neighboring interactions and cached-frame behavior.

## Preview scheduling

CPU frames now enter the RAM cache and request repaint before optional RGBA conversion and
disk-cache compression. Persistence still uses the same content key and runs on the worker;
disk hits are not rewritten. The render-duration display ends at publication and excludes this
post-publication persistence work. It still omits queue waiting and is not interaction latency.

The native preview scheduler now uses the actual render-pool capacity instead of the machine's
logical CPU count. Normal paused prefetch schedules at most two frames and leaves one worker
available where the pool permits it. A two-worker browser pool schedules at most one background
frame; a one-worker pool does not do normal paused prefetch. Explicit cache-before-playback and
Cache Frames When Idle retain their separate policies.

Dragging render-affecting properties in Effect Controls or Timeline now participates in the
existing adaptive-preview policy. This reduces the requested pixel count while scrubbing when
Auto resolution and Adaptive Resolution are selected. It does not change the blur kernel.

The checks establish interaction and resolution behavior, not a measured input-to-frame latency
improvement or complete visual Gaussian Blur parity. Remaining measurements include cold/warm
1080p and 4K latency, stale-frame work, CPU/GPU selection, blur impulse profiles, and edge behavior.

## Live reference limits

AESync 2.0.4 answered from AE 26.3x87. Original four-tile compositions were used for desktop
interaction study, and extended Timeline selection was verified by querying selected layers
after a keyboard gesture. Automated native drags did not produce reliable marquee observations;
those results are not claimed as a pixel-perfect AE oracle. Shape/path selection and modifier
drag cadence still need controlled live comparison.

- Composition right-click selects an unselected hit layer, preserves an already selected group,
  and exposes the shared Timeline layer actions. The pointer-driven regression test invokes
  Duplicate from the actual popup and verifies both single-layer and group behavior.

## Wider UI review

The opt-in `ui_visual_review` suite captures all 34 built-in panels at 1600 × 1000 and
1024 × 768, plus common dialogs and Light theme workspaces. Captures use an original fixture
and do not establish every panel control's behavior or live input-to-frame performance.

Additional regression checks cover modal pointer and text isolation, Escape rollback of live
Layer Style previews, cancellation with bounded undo history, Project edit command routing,
filtered Project Select All, and deleting camera track points without deleting the layer/effect.
Layer Style Cancel restores a captured project/history checkpoint rather than replaying Undo.
Filtered Project Select All omits structural folders so deleting search results cannot delete
unrelated descendants. Its exact AE parity remains to be checked live.

The Composition context menu's Select submenu lists visible, unlocked layers under its original
click position, allowing selection of an obscured layer. Reference:
[Adobe layer selection manual](https://helpx.adobe.com/after-effects/desktop/work-with-layers/select-and-arrange-layers/selecting-arranging-layers.html).
Workspace behavior reference:
[Adobe workspace guide](https://www.adobe.com/learn/after-effects/web/customize-workspaces).
The broader reference is the [official AE user guide](https://helpx.adobe.com/after-effects/desktop.html).
The [publisher's 2026 Classroom in a Book sample](https://ptgmedia.pearsoncmg.com/images/9780135561430/samplepages/9780135561430_Sample.pdf)
was also reviewed. Its Roto Brush lesson supplies future behavior cases for foreground/background
strokes, alpha display modes, frame propagation, and freezing. Those cases are reference backlog,
not verified EffectCraft parity; book artwork and lesson files are not copied into this repository.

Theme fixes use matched token backgrounds/text for fields, secondary buttons, notifications,
and About links. Compact viewer footers omit render duration when it would collide with timecode.
Further design work includes full toolbar overflow, tighter grouping of controls in wide panels,
and additional small-window/dynamic panel fixtures.

GPU compositor initialization now catches backend setup panics and retains CPU compositing.
This covers a reproduced Direct3D FXC shader compiler failure; it does not repair an OS driver
failure or establish the cause of the reported Windows bugcheck. The GPU parity test is separately
reported when this device cannot create its compute pipeline, never counted as a passed test.
Native GPU threshold regressions now compare prepared quantized buffers directly with the CPU:
quantization uses the CPU-rounded reciprocal and matching half-up arithmetic; channel Arithmetic
refines unpremultiplication, and median division refines the residual twice before strict Dust &
Scratches thresholds. These checks retain the existing image tolerances. Power Pin admits a small
f32 inverse-solve error at exact quad edges and clamps accepted coordinates to the source domain.
The original half-scale right-edge fixture and neighboring quantization-boundary samples reproduce
the failures. Full backend qualification and performance measurement remain separate acceptance work.