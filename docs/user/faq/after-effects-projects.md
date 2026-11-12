---
title: Can EffectCraft open After Effects projects?
description: EffectCraft cannot open .aep or .aepx files, and After Effects plug-ins do not run. It saves .ecproj files and imports Lottie.
order: 10
verified: 0.9.2
---

No. EffectCraft cannot open After Effects projects (`.aep` or `.aepx`), and After Effects plug-ins do not run in it.

## What it opens instead

- Its own projects, saved as `.ecproj`. These are readable, versioned JSON files.
- Lottie animations, with **File ▸ Import ▸ Lottie...**. Each opens as a new composition.
- Photoshop and PDF or Illustrator files, imported as footage or as a composition. See [Import footage and image sequences](../guides/import-footage.md).

## What carries over

Expressions are JavaScript with the After Effects object model, such as `wiggle`, `loopOut` and `thisComp.layer("...")`. The panels, shortcuts and effect parameter names follow After Effects, so the skills carry over.

## Status

Reading `.aep` files is the largest known gap between EffectCraft and After Effects. It is listed in the project's [gap list](https://github.com/storytold/effectcraft/blob/main/docs/gaps.md), and the maintainers have not yet decided how to approach it.
