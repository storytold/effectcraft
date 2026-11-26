# EffectCraft app icon

**Creature:** a unicorn, as an engraved head-and-shoulders bust looking out of the tile.

**Style:** an ink engraving portrait in the Crafting Apps' owl-template framing (the FilmCraft owl):
full-bleed colour field, tight head-and-shoulders crop, the body running off the bottom and right edges.

**Tile:** 512 × 512 viewBox, rounded square `rx=112`, clipped; no border or roundel.

**Palette** (exactly three colours):

| Colour | Hex | Used for |
|---|---|---|
| App colour (EffectCraft pink) | `#e0368f` | the full-bleed field |
| Paper | `#efe9dc` | the figure |
| Ink | `#0b0b0c` | line work and contour |

**Provenance:** the project owner drew it in ArtCraft (a 2880 px engraving-style image). It was
vectorised with potrace, which keys out the field and traces the ink.
Licence: see [LICENSE.txt](LICENSE.txt) (MIT OR Apache-2.0, like the repo).

## Files

| File | What |
|---|---|
| `effectcraft.svg` | master vector, traced at 2048 px (canonical) |
| `effectcraft-small.svg` | lighter vector (~350 KB), traced at 1024 px; also the web favicon |
| `effectcraft-1024.png` | 1024 px render |
| `effectcraft-macos-512.png` | runtime Dock icon on macOS, on Apple's 824/1024 grid |
| `effectcraft.icns` | macOS icon (Apple grid) |
| `effectcraft.ico` | Windows icon, 16–256 px; embedded in `effectcraft.exe` by `apps/effectcraft/build.rs` |
| `hicolor/<n>x<n>/apps/ai.storyteller.effectcraft.png` | Linux icon theme, 16–512 px; the 256 px one is the runtime icon on Windows and Linux |
| `hicolor/scalable/apps/ai.storyteller.effectcraft.svg` | Linux scalable icon |

App ID: `ai.storyteller.effectcraft` (Wayland app ID, `packaging/linux/ai.storyteller.effectcraft.desktop`).

## Nightly channel

The nightly derivatives keep the foreground artwork, alpha and Mac padding unchanged. Only the
background contribution changes: the stable pink field `#e0368f` becomes the deeper `#5e2443`.
There is no letter badge or crescent; the app, its window and the Launcher identify the channel in
text.

`effectcraft-nightly.png` and its nine-frame ICO (16–256 px) use the existing native 256 px master.
`effectcraft-nightly-1024.png`, `effectcraft-nightly-macos-512.png` and the eleven-slot ICNS use the
padded Mac masters, never an upscale. Input/output hashes, the palette-fit recipe and container
members are recorded in [`nightly-provenance.json`](nightly-provenance.json). Stable source files
are not recoloured.

## Regenerate

After replacing `effectcraft.svg` / `effectcraft-small.svg`, run `packaging/icons.sh` (needs `resvg`; `iconutil`
on macOS for the `.icns`). Copy `effectcraft-small.svg` to `apps/effectcraft-web/web/favicon.svg` too.
