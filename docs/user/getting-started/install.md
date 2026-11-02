---
title: Install EffectCraft
description: Download EffectCraft for Windows 11, macOS or Linux, install it, or install it through ArtCraft Launcher.
order: 10
verified: 0.9.2
---

The easiest way is the [EffectCraft page on getartcraft.com](https://getartcraft.com/apps/effectcraft). For a specific file, use the [latest release](https://github.com/storytold/effectcraft/releases/latest) on GitHub. In the file names, `<version>` stands for the version number, for example `0.9.2`.

## Windows 11

1. Open the [latest release](https://github.com/storytold/effectcraft/releases/latest).
2. Download `effectcraft-<version>-windows-x64.msi`. It fits most PCs (Intel and AMD, 64-bit).
3. Run the file and follow the installer. It asks where to install EffectCraft and offers a **Create a desktop shortcut** box.
4. Start **EffectCraft** from the Start menu.

The release also has an `arm64` installer for Snapdragon and other ARM PCs. The installers are code-signed and install for all users. Later versions install into the same folder.

To run it without installing, download `effectcraft-<version>-windows-x64-portable.zip` instead. It contains `effectcraft.exe` and the command-line tool `effectcraft-cli.exe`. The `arm64` build has a portable zip too.

There are also 32-bit `x86` builds, as an installer and a portable zip.

## macOS

1. Download `effectcraft-<version>-macos-universal.dmg`. It works on Apple silicon and Intel Macs and is signed and notarized.
2. Open it and drag EffectCraft to Applications.

The command-line tool is a separate download for macOS: `effectcraft-cli-<version>-macos-universal.zip`.

## Linux

Pick one file from the release. Replace `x86_64` with `aarch64` on ARM.

| File                                          | How to use it                            |
|-----------------------------------------------|------------------------------------------|
| `effectcraft-<version>-linux-x86_64.AppImage` | Run `chmod +x` on the file, then run it. |
| `effectcraft-<version>-linux-x86_64.deb`      | For Debian and Ubuntu.                   |
| `effectcraft-<version>-linux-x86_64.rpm`      | For Fedora, RHEL and openSUSE.           |
| `effectcraft-<version>-linux-x86_64.flatpak`  | `flatpak install --user <file>`          |
| `effectcraft-<version>-linux-x86_64.tar.gz`   | Unpack it anywhere.                      |

For RISC-V 64-bit there is a `.deb`, an `.rpm` and a `.tar.gz` (`riscv64` in the file name). They need glibc 2.39 or newer. There is also a FreeBSD x86_64 `.tar.gz`.

## In the browser

`effectcraft-web-<version>.zip` is the web build. It is a static site that runs in a modern browser and needs to be hosted on a web server. Projects, media and settings stay in the browser's private storage and on your computer.

## Install through ArtCraft Launcher

ArtCraft Launcher lists EffectCraft. Click **Install** on its card. See [Install and update Craft apps](https://getartcraft.com/docs/craft-launcher/guides/install-and-update-apps).

## Check the download

`SHA256SUMS.txt` on the release page lists a checksum for every file.

## Next

Open EffectCraft and make something: [Make your first composition](../guides/make-a-composition.md). If you only want to look around, choose **Open Demo Project** on the Home screen.
