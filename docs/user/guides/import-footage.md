---
title: Import footage and image sequences
description: Bring video, images and numbered image sequences into an EffectCraft project, and change how footage is read.
order: 50
verified: 0.9.2
---

Choose **File ▸ Import ▸ File...** (Ctrl+I) and pick your files, or drop files on the EffectCraft window. Numbered stills such as `shot_0001.png` can import as one sequence that plays like a movie.

Shortcuts on this page use Ctrl. On macOS, use Cmd instead of Ctrl and Option instead of Alt.

## Import files

1. Choose **File ▸ Import ▸ File...** to pick one file. Use **File ▸ Import ▸ Multiple Files...** to pick several.
2. Photoshop and PDF or Illustrator files first ask whether to import them as footage or as a composition.
3. The footage appears in the Project panel. Drag it onto the Composition viewer to make a layer where you drop it, or into the Timeline. If no composition is open, dropping it on the empty viewer makes a new composition from it.

You can also drop files or folders on the window. Files dropped on the Composition viewer become layers there. A dropped Photoshop file asks how to import it, as **File ▸ Import ▸ File...** does. The **Media Browser** (**Window ▸ Media Browser**) browses folders and imports what you pick.

## Import an image sequence

1. Choose **File ▸ Import ▸ File...** and pick any one numbered file of the run.
2. In the **Import Image Sequence** dialog, check the run it found. It also lists missing frames.
3. Tick the sequence option, which is named after the file type, for example **PNG Sequence**. When it is off, the files import as separate stills. It is off by default.
4. Set **Frame rate (fps)**. The default comes from **Sequence Footage** on the Import page of the preferences (**Edit ▸ Preferences ▸ Import...**; on macOS: **EffectCraft ▸ Settings... ▸ Import...**), which is 30 fps.
5. Confirm. The run arrives as one footage item named like `shot_[0001-0250].png`.

**Force alphabetical order** imports every image of that type in the folder in name order, whatever the numbering. Use it for files that are not numbered consistently.

Stills in PNG, JPEG, TIFF, OpenEXR, BMP, WebP, Targa (`.tga`), Radiance HDR (`.hdr`), DPX and Cineon (`.cin`) can form a sequence. PSD, SVG and PDF files import one by one.

If the numbering skips a frame, EffectCraft shows colour bars for it. Put the missing files in the folder and choose **File ▸ Reload Footage** to fill the gap.

## Change how footage is read

Select the footage in the Project panel and choose **File ▸ Interpret Footage ▸ Main...** (Ctrl+Alt+G). The dialog has **Alpha**, **Assume this frame rate** and **Start Timecode**, among others. A change applies everywhere the footage is used.

For example, a 250-frame sequence at 25 fps lasts 10 seconds. At 24 fps it lasts 10.4 seconds.

## Replace or reload footage

- **File ▸ Replace Footage ▸ File...** (Ctrl+H) points the item at another file.
- **File ▸ Reload Footage** (Ctrl+Alt+L) reads the files again. A sequence picks up frames added to its folder.

If footage shows as missing, see [Footage is missing or offline](../troubleshooting/footage-missing.md).

## Linux with Wayland

Dragging files onto the window does not work on native Wayland sessions. Use **File ▸ Import ▸ File...** instead.
