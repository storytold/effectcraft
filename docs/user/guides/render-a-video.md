---
title: Render a video
description: Add a composition to the Render Queue, choose a format such as H.264, pick where to save and click Render.
order: 40
verified: 0.9.2
---

Select the composition, choose **Composition ▸ Add to Render Queue** (Ctrl+M), set the output file, then click **Render** in the Render Queue panel. The default format is H.264, saved as an `.mp4` file named after the composition.

Shortcuts on this page use Ctrl. On macOS, use Cmd instead of Ctrl and Option instead of Alt.

## Steps

1. Select the composition in the Project panel or open it.
2. Choose **Composition ▸ Add to Render Queue** (Ctrl+M). You can also drag compositions onto the Render Queue panel. Open the panel with **Window ▸ Render Queue** (Ctrl+Alt+0).
3. The queue lists the item with its **Comp Name**, **Status**, **Started** and **Render Time**. Twirl the item open to see **Render Settings:**, **Output Module:** and **Output To**.
4. Click **Output Module:** and choose a format. The menu entries read **Format: H.264**, **Format: QuickTime (ProRes)** and so on.
5. Click the file name next to **Output To** to choose where to save. Alt-click it to edit the name template instead.
6. Check that the **Render** tick box at the left of the item is on. It is on by default, and only ticked items render.
7. Click **Render**. The status changes to **Rendering** with a percentage. Click **Stop** to cancel.

Tick **Notify** to get a sound and a notification when the queue finishes.

## Formats

The **Output Module:** menu lists the formats your build can export:

| Menu entry               | File type                   |
|--------------------------|-----------------------------|
| H.264                    | `.mp4`                      |
| QuickTime (ProRes)       | `.mov`                      |
| QuickTime (Uncompressed) | `.mov`                      |
| HEVC (H.265)             | `.mp4`                      |
| AV1                      | `.mp4`                      |
| WebM (VP9 + Opus)        | `.webm`                     |
| PNG Sequence             | `.png`, one file per frame  |
| JPEG Sequence            | `.jpg`, one file per frame  |
| TIFF Sequence            | `.tif`, one file per frame  |
| OpenEXR Sequence         | `.exr`, one file per frame  |
| Animated GIF             | `.gif`                      |
| WAV                      | `.wav`, audio only          |
| AIFF                     | `.aif`, audio only          |

To keep transparency, choose **Channels: RGB + Alpha** in the **Output Module:** menu. It appears for formats that can store alpha, such as QuickTime (ProRes), QuickTime (Uncompressed) and PNG Sequence. H.264, HEVC, AV1 and JPEG Sequence cannot.

## Render only part of a composition

Click **Render Settings:** and choose **Time Span: Work Area Only** or **Time Span: Length of Comp**. The same menu sets **Quality: Best** or **Quality: Draft** and the resolution (**Resolution: Full**, **Half**, **Third** or **Quarter**).

## Export a Lottie animation instead

Choose **File ▸ Export ▸ Lottie JSON...** to write a composition as `.json` or `.lottie`. EffectCraft lists anything Lottie cannot express. To open one, choose **File ▸ Import ▸ Lottie...**.
