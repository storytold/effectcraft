# EffectCraft Codecs Migration to craft-libs

> **Tracking Issue:** [storytold/effectcraft#2813](https://github.com/storytold/effectcraft/issues/2813)  
> **Upstream PR:** [storytold/craft-libs#504](https://github.com/storytold/craft-libs/pull/504)  
> **Author:** `el-j`  

## Overview
Pure-Rust media codecs originally hosted in EffectCraft (`effectcraft-hevcenc`, `effectcraft-av1enc`, `effectcraft-vp9enc`) are now centralized inside `craft-libs` as the canonical One True Source for all ArtCraft applications.

### Canonical Locations in `craft-libs`
- `crates/hevcenc` (`craft-hevcenc`): H.265 / HEVC Main, Main 10, Intra/Inter encoder
- `crates/av1enc` (`craft-av1enc`): AV1 Main profile video encoder
- `crates/vp9enc` (`craft-vp9enc`): VP9 Profile 0/2 video encoder
- `crates/codecs` (`craft-codecs`): Umbrella crate re-exporting all video and audio encoders

This centralization enables `effectcraft`, `filmcraft`, and `encodercraft` to share identical, SIMD-accelerated, zero-panic codec implementations without cross-application dependencies.
