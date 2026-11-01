---
title: EffectCraft does not use my graphics card
description: What to do when the report says GPU none (CPU compositing), the window picked another adapter, or GPU effects fail on Windows 11 with DirectX 12.
order: 10
verified: 0.9.2
---

Open **Help ▸ System Compatibility Report...** and read the **GPU:** and **Window graphics:** lines. If the window runs on the wrong adapter, set **Window Graphics** to **Automatic** in the preferences (**Edit ▸ Preferences ▸ Startup & Repair...**; on macOS: **EffectCraft ▸ Settings... ▸ Startup & Repair...**) and restart EffectCraft.

## Symptoms

- The report says `GPU: none (CPU compositing)` followed by a reason.
- The `Window graphics:` line names an adapter or backend you did not expect, for example an integrated or OpenGL adapter instead of your graphics card.

## Fix

1. Choose **Help ▸ System Compatibility Report...**. Copy the text.
2. Look at the `Window graphics:` line. If it shows the wrong adapter, continue. If it shows your card, go to **Still stuck?** below.
3. Choose **Edit ▸ Preferences ▸ Startup & Repair...**. On macOS, choose **EffectCraft ▸ Settings... ▸ Startup & Repair...**.
4. Set **Window Graphics** to **Automatic**. The other choice is **OpenGL (compatibility)**.
5. Close EffectCraft and start it again. The change applies from the next launch.
6. Open the report again and check that `Window graphics:` now names your card.

One user whose GPU was not detected on 0.8.0 fixed it this way: they set the window graphics to Automatic, and EffectCraft started through Vulkan ([issue #596](https://github.com/storytold/effectcraft/issues/596)).

If the window runs on your card but the report still says `GPU: none (CPU compositing)`, the reason after it says why the GPU compositor did not start. With **OpenGL (compatibility)**, compositing always runs on the CPU, because the GPU compositor needs DirectX 12, Vulkan, Metal or WebGPU.

## Why this happens

When the graphics driver stops EffectCraft as its window opens, the next launch switches to OpenGL by itself. That keeps the app usable. Setting **Window Graphics** to **Automatic** lets EffectCraft choose the graphics backend again.

The **GPU Information...** button on the Previews page of the preferences names the adapter the GPU compositor runs on. When compositing runs on the CPU, it says why after `GPU compositing is off:`. It also shows the `Window graphics:` line. For a bug report, still send the whole System Compatibility Report.

## Windows 11 with DirectX 12

On some Windows 11 PCs with NVIDIA GPUs, the GPU compositor fails to start on DirectX 12 because the FXC shader compiler rejects some of EffectCraft's GPU kernels. Two users reproduced this with an RTX 4060 laptop GPU and an RTX 3060. In their tests EffectCraft fell back to CPU compositing, and the same build started on Vulkan. This is still open in 0.9.2 as [issue #613](https://github.com/storytold/effectcraft/issues/613). Check the issue for updates.

The window can also take a long time to appear. On 0.8.0 with an RTX 5060 Ti, one user measured 69 seconds before the window showed on DirectX 12 without a working DXC shader compiler, against 3 seconds on Vulkan, which is what Automatic picked on that PC. If you close EffectCraft during that wait, the next launch uses OpenGL and has no GPU compositor until you set **Window Graphics** back to **Automatic** (steps above). So wait for the window on the first launch.

## Still stuck?

Ask in [Discord](https://discord.gg/artcraft) or open an issue in the [issue tracker](https://github.com/storytold/effectcraft/issues). Include the whole System Compatibility Report, your graphics card and driver version, and whether you also have an integrated GPU.
