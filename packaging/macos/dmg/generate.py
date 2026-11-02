#!/usr/bin/env python3
"""Regenerate dmg-layout.DS_Store for the macOS DMG window (see README.md here).

Needs:
    pip install ds_store==1.3.3 mac_alias==2.2.3

Runs on any OS, no Mac needed. The file comes from this script alone, so it carries nothing
from the machine that made it: no local paths, user or disk names, dates or volume UUIDs.
(background.tiff is rendered from background.svg by packaging/macos/package.sh at package time.)

    python3 packaging/macos/dmg/generate.py
"""

import datetime
import os

from ds_store import DSStore
from mac_alias import Alias, TargetInfo, VolumeInfo

HERE = os.path.dirname(os.path.abspath(__file__))

# Keep in sync with package.sh (volume name) and README.md (layout).
VOLUME = "Visualab Effects"
WIDTH, HEIGHT = 660, 400  # window content, pt; the background's 1x size
TITLE_BAR = 32
ICON_SIZE = 128
ICONS = {"Visualab Effects.app": (326, 205), "Applications": (574, 205)}
# A fixed date for the alias. Finder never matches it against the image
# (each build is a new volume): it finds the background by volume name and path.
FIXED_DATE = datetime.datetime(2026, 10, 8, tzinfo=datetime.timezone.utc)


def background_alias():
    # What Finder stores for /.background/background.tiff on the volume, minus anything about the
    # machine (dmgbuild writes the same kind of alias from a mounted image). Disk type 5 (ejectable),
    # the volume flags and the folder/file ids (25, 26) are what Finder wrote on a test image; the ids
    # are only hints, as the volume's creation date.
    volume = VolumeInfo(VOLUME, FIXED_DATE, b"H+", 5, 0x0D02, b"\0\0")
    volume.posix_path = f"/Volumes/{VOLUME}"
    target = TargetInfo(0, "background.tiff", 25, 26, FIXED_DATE, b"\0\0\0\0", b"\0\0\0\0")
    target.folder_name = ".background"
    target.cnid_path = [25]
    target.carbon_path = f"{VOLUME}:.background:\0background.tiff"
    target.posix_path = "/.background/background.tiff"
    return Alias(volume=volume, target=target).to_bytes()


def write_ds_store(out):
    if os.path.exists(out):
        os.remove(out)
    with DSStore.open(out, "w+") as d:
        d["."]["bwsp"] = {
            "ShowStatusBar": False,
            "ShowToolbar": False,
            "ShowTabView": False,
            "ContainerShowSidebar": False,
            "ShowSidebar": False,
            "WindowBounds": f"{{{{200, 528}}, {{{WIDTH}, {HEIGHT + TITLE_BAR}}}}}",
        }
        d["."]["icvp"] = {
            "viewOptionsVersion": 1,
            "backgroundType": 2,
            "backgroundImageAlias": background_alias(),
            "backgroundColorRed": 1.0,
            "backgroundColorGreen": 1.0,
            "backgroundColorBlue": 1.0,
            "gridOffsetX": 0.0,
            "gridOffsetY": 0.0,
            "gridSpacing": 100.0,
            "arrangeBy": "none",
            "showIconPreview": True,
            "showItemInfo": False,
            "labelOnBottom": True,
            "textSize": 16.0,
            "iconSize": float(ICON_SIZE),
        }
        d["."]["vSrn"] = ("long", 1)
        for name, pos in ICONS.items():
            d[name]["Iloc"] = pos


def main():
    write_ds_store(os.path.join(HERE, "dmg-layout.DS_Store"))


if __name__ == "__main__":
    main()
