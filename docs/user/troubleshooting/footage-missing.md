---
title: Footage is missing or offline
description: Find missing footage in an EffectCraft project, relink it to a folder or replace it with another file.
order: 20
verified: 0.9.2
---

Choose **File ▸ Dependencies ▸ Relink Missing Footage...** and pick the folder where your files are now. For files that stay missing, choose **File ▸ Replace Footage ▸ File...**.

Shortcuts on this page use Ctrl. On macOS, use Cmd instead of Ctrl and Option instead of Alt.

## Symptoms

- A footage item in the Project panel is flagged as missing after you open a project.
- EffectCraft shows a message that footage items are missing.

## Fix

1. Choose **File ▸ Dependencies ▸ Find Missing Footage**. EffectCraft selects every missing item in the Project panel and shows how many it found.
2. Choose **File ▸ Dependencies ▸ Relink Missing Footage...** and pick the folder that holds your files. EffectCraft searches that folder and its subfolders.
3. It relinks files whose names are unique and whose type, dimensions and codec match what the project saved. The whole relink is one undo step.
4. For anything left over, select the item and choose **File ▸ Replace Footage ▸ File...** (Ctrl+H), then pick the file yourself. This also covers files with duplicate names in the folder, which Relink leaves alone.
5. After you copy or render files into a folder, choose **File ▸ Reload Footage** (Ctrl+Alt+L) to read them again.

Relinking keeps item names, interpretation and layer timing. It does not change your files. For image sequences, every saved frame must be in the new folder. Relink Missing Footage is not available in the browser version.

## Why this happens

A project stores where each footage file is. If you move or rename the files, or open the project on another computer, the project cannot find them.

Projects built with the command-line tool or an agent can use relative paths such as `bg.mp4`. Before 0.8.0, the app looked for those in its own working directory instead of next to the project file. Since 0.8.0 it also looks next to the project file ([pull request #676](https://github.com/storytold/effectcraft/pull/676)). The reporter on macOS has not confirmed the fix yet ([issue #652](https://github.com/storytold/effectcraft/issues/652)).

## Still stuck?

Ask in [Discord](https://discord.gg/artcraft) or open an issue in the [issue tracker](https://github.com/storytold/effectcraft/issues). Say which system you use, whether the project came from another computer or from the command-line tool, and where the footage is stored.
