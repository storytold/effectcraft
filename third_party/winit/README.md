# Vendored winit 0.30.13 (Android input delta)

This is the workspace's `winit` fork — `storytold/winit` at the rev `main` pins — with
only `src/platform_impl/android/mod.rs` changed, vendored because EffectCraft's Android
build has no route to the fix through the pin: `eframe` (0.36 and 0.37) pins `winit` to
0.30, 0.30.13 is the last 0.30 release, `winit` 0.31 is still in beta, upstream has the
same gap (their code literally says `// TODO mouse events`), and the fork carries it too.

The workspace pins this copy with `[patch.crates-io]` in the root `Cargo.toml`
(replacing the fork pin there); nothing else in the tree may reference it directly.

## The delta (one file: `src/platform_impl/android/mod.rs`)

Upstream's Android backend translates every `MotionEvent` into a `Touch` event
and drops the rest, so a mouse attached to the device (Bluetooth, USB or DeX) has
no hover, no scroll wheel and no right or middle button; only touch works. The
patch adds one method, `EventLoop::dispatch_mouse_input`, and one call to it:

- pointers whose `Source` is `Mouse` or `Touchpad` are routed to the new method
  (touchscreen and touch-dragged stylus pointers keep the untouched `Touch` path);
- `Scroll` becomes `MouseWheel { delta: LineDelta(-h, v) }` from the event's `Hscroll`/`Vscroll`
  axes (Android's horizontal axis scrolls the viewport the other way round from winit's positive
  `x`; the vertical one already matches);
- `ButtonPress`/`ButtonRelease` become `MouseInput` for the button they changed (right,
  middle, back, forward, and primary if a device sends it that way), preceded by a `CursorMoved`;
  the button is read from the button state against what this loop has reported (in
  `changed_button_in_state`), *not* from Android's `AMotionEvent_getActionButton`: that symbol
  exists only from API 33 and a static reference to it keeps the whole library from loading on
  older devices — the published crate's `action_button()` call (winit's own and
  android-activity's) repeats this on Android 13+, where it was never noticed. A press lists
  the button in the state, a release omits it, and the state of the untouched double-report
  case (a device reporting the gesture's first press as `Down` *and* `ButtonPress`) agrees
  with what is already reported, so the semantics are the same as an `action_button()` read;
  only buttons moving in the event's direction count (a stale `reported` bit — a release the
  `Up` or a cancellation handled elsewhere did not clear — cannot name the button that did not
  move and swallow the one that did);
- the button that *starts* a gesture arrives as an ordinary `Down`/`Up` (per Android's
  button-event rules): `Down` is reported as the button in the event's `button_state()` (a
  touchpad tap has none, and is the primary button), and `Up`, whose state is already cleared, as
  the button the gesture reported down. A press or release already reported is not repeated, so a
  device that also sends `ButtonPress`/`ButtonRelease` for the first button does not look like a
  double click;
- `Move`/`HoverMove` become `CursorMoved`, `HoverEnter`/`HoverExit` become
  `CursorEntered`/`CursorLeft`, so the cursor tracks the mouse without a press;
- `Cancel`, and the loop's `MainEvent::LostFocus`, release every button still reported down
  (Android ends the gesture without an `Up` when the system takes it over, and a press that
  ran into losing the window never sees its `Up`) — a button left down would make egui drag
  forever and suppress its next press.

Which button a `Down` reported is remembered (a bitmask on the event loop), so `Up` releases the
same button(s) even though Android has cleared its state by then, and no button is reported twice.

Keyboard events are **not** touched here: neither the fork nor upstream emits
`ModifiersChanged` on Android (both ignore the meta state Android carries on every
`KeyEvent`), but EffectCraft fixes that on its own side, in `crates/ui-egui`, where
it is unit-testable — see `stamp_android_modifiers` there.

## Rebasing onto a newer winit

1. Take the new base (the fork rev `main` pins, or a newer published crate) as the
   base again: replace this directory with that tree, keeping this README, and drop
   what a published crate strips and EffectCraft cannot ship — `.github`, `docs/`,
   `.git*` metadata, and the images the docs and examples ship
   (`docs/res/*.svg`, `examples/data/*.png`): they are never compiled or shipped, and
   EffectCraft's `cargo xtask assets` gate attributes every asset in the repository
   file by file.
2. Re-apply the delta above; it is self-contained in the `MotionEvent` arm of
   `handle_input_event` plus the new method beside it. Search for
   `TODO mouse events` — as long as upstream still has it, the hunk applies in
   the same place.
3. If the fork or upstream implements Android mouse input, delete this directory,
   restore the fork pin in the root `Cargo.toml`, and delete
   `stamp_android_modifiers` in `crates/ui-egui` only after checking that the new
   `winit` emits `ModifiersChanged` there too.

Licensed Apache-2.0, as upstream; `LICENSE` is upstream's, and everything but
`src/platform_impl/android/mod.rs` is byte-identical to the fork tree, itself only
0.30.13 plus the fork's own commits; the only EffectCraft change is the one file.

This delta was written and validated on hardware for FilmCraft's Android port
(`Kaedo17/filmcraft-mobile` `685c9cc3`), then carried through PhotoCraft's port,
which EffectCraft's copy follows.
