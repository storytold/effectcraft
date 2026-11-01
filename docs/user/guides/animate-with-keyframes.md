---
title: Animate with keyframes
description: Fly a layer in with two Position keyframes and smooth the motion with Easy Ease.
order: 20
verified: 0.9.2
---

Click the stopwatch beside a property to start animating it, move the current-time indicator, then change the value. The second keyframe appears by itself. Select both keyframes and press F9 to ease the motion.

You need a composition with a layer. See [Make your first composition](make-a-composition.md).

Shortcuts on this page use Ctrl. On macOS, use Cmd instead of Ctrl and Option instead of Alt.

## Steps

1. In the Timeline, twirl the layer open, then open **Transform**.
2. Set the starting value. For a layer that flies in from the left, scrub or type a **Position** that is outside the frame.
3. Click the stopwatch beside **Position**. It turns blue and records a keyframe at the current time.
4. Drag the blue current-time indicator to about 0:00:01:00, or click in the time ruler.
5. Drag the layer to its final place in the Composition viewer, or scrub the **Position** value. Because the stopwatch is on, a second keyframe appears by itself.
6. Select both keyframes (drag a box around them, or click the **Position** name) and press F9. This is **Animation ▸ Keyframe Assistant ▸ Easy Ease**. The layer now glides in and settles.
7. Press the Spacebar, or use Play in the Preview panel, to watch it. Press it again to stop.

## Other things you can do

- Click the keyframe button in the layer's property row to add or remove a keyframe at the current time. Its tooltip reads **Add or remove keyframe**.
- Choose **Animation ▸ Keyframe Assistant ▸ Easy Ease In** (Shift+F9) or **Easy Ease Out** to ease one side only.
- Choose **Animation ▸ Toggle Hold Keyframe** for a keyframe that jumps instead of moving.
- Choose **Animation ▸ Keyframe Interpolation...** or **Keyframe Velocity...** to set the curve exactly.
- Hold Alt and click a stopwatch to add an expression.
- The Graph Editor shows value and speed graphs with draggable handles.

## Next

[Add an effect](add-an-effect.md).
