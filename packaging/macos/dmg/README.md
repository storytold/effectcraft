# macOS DMG window

What Finder shows when the DMG opens: a 660 × 400 pt background with the app icon and the
`Applications` link side by side. `package.sh` renders the background from `background.svg` and copies it
and the layout into the image, so no bitmap is committed. The DMG still builds with
`hdiutil makehybrid` (no mounted device, no Finder scripting on CI).

| File | What |
|---|---|
| `background.svg` | Source of the background: the app icon (`assets/app-icon/effectcraft.svg`, linked, not copied) cropped as a cover on the Visualab Effects colour field (`#e0368f`), Ink and Paper. Its text (Inter, JetBrains Mono) is outlined, so rendering it needs no fonts. `package.sh` renders it with [resvg](https://github.com/linebender/resvg) 0.48.1 at 1x (660 × 400 px) and 2x (1320 × 800 px) and joins both into `.background/background.tiff` (`tiffutil -cathidpicheck`); `release.yml` installs that resvg on the macOS runner. |
| `dmg-layout.DS_Store` | Finder's view settings for the volume: window size, icon size 128, Visualab Effects.app at (326, 205), `Applications` at (574, 205), and the background. Goes to `.DS_Store` in the image; named so it isn't mistaken for (or ignored like) a Finder-generated `.DS_Store`. |
| `generate.py` | Writes `dmg-layout.DS_Store` from the layout above. |

## The volume name has no version

The mounted volume is called `Visualab Effects`, not `Visualab Effects <version>`. The layout points at the background
through an alias that includes the volume name, and Finder resolves it by that name: with a
versioned name the window keeps its size and icon positions but shows no background (tested).
The DMG file name (`effectcraft-<version>-macos-<arch>.dmg`) still carries the version.

## Rules

- **Finder draws the icon labels in black in light and dark mode** when a window has a background,
  so the area under both icons stays light (Paper).
- **Nothing goes inside the icon boxes:** artwork keeps 10 pt clear of each 128 pt icon box and of
  the label strip under it.

## Regenerate

- **The background:** edit `background.svg`. `package.sh` renders it on every build; to preview it,
  `resvg --skip-system-fonts -w 1320 background.svg /tmp/bg@2x.png`. If you add text, outline it
  (`usvg` from resvg converts text to paths), since no fonts are loaded.
- **The layout:** edit the constants in `generate.py`, then run, on any OS:

  ```sh
  pip install ds_store==1.3.3 mac_alias==2.2.3
  python3 packaging/macos/dmg/generate.py
  ```

  `dmg-layout.DS_Store` is written from scratch and byte-for-byte reproducible: its background
  alias holds only the volume name and `/.background/background.tiff`, nothing from the machine that
  ran it. The window is 660 × 432 with Finder's 32 pt title bar; the content area is 660 × 400.

This follows VectorCraft's `packaging/macos/dmg/` (storytold/vectorcraft#493).
