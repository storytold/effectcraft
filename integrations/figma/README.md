# Figma → EffectCraft

Bring Figma layers into the open EffectCraft composition as **native, editable layers**:
shape layers with real paths and fills, text layers you can retype, precomps for frames, nulls for
groups, track mattes for masks, layer styles for shadows.

There are two ways in, and both use the same importer (the engine command `file.importFigma`):

- **Agents and scripts** pass a Figma link (or node JSON) to `file.importFigma`. No clicking in Figma.
- **People** select layers in Figma and click **Send** in the EffectCraft Transfer plug-in.

## Agents: one command

```sh
export FIGMA_TOKEN=figd_…   # Figma ▸ Settings ▸ Security ▸ Personal access tokens (File content: read)
effectcraft-cli exec file.importFigma --bridge 9877 \
  --params '{"url":"https://www.figma.com/design/FILEKEY/Name?node-id=12-345"}'
```

- `--bridge 9877` runs it in the open app. Leave it out to run headless (add `--save-as out.ecproj`).
  Over MCP, call `execute_command` with `{"command":"file.importFigma","params":{…}}`.
- `url` is a link to the frame or layer (Figma: right-click ▸ Copy link to selection).
  `fileKey` + `nodeIds: ["12:345", …]` does the same.
- `target`: `newComp` (frame size → comp size), `activeComp` (centred in the open comp), or `auto`
  (the default: frames get new comps, anything else goes into the active comp).
- `payload` takes Figma node JSON directly: the REST `GET /v1/files/:key/nodes?geometry=paths` reply, a
  node or an array of nodes. Use `payloadPath` for a JSON file. Images can be included as
  `"images": {"<node id>": {"png": "<base64>"} | {"path": "/file.png"}}`.
- `dryRun: true` returns the plan (comps, layers, what would be rasterised, and `assetPaths` for image files) and changes nothing.
  Confined agents validate each destination against their write roots, including existing symbolic links, before importing.
  Older bridge apps that omit these paths must be updated before a confined import can proceed.
- The token comes from `token`, `FIGMA_TOKEN`, or one stored with
  `effectcraft-cli exec figma.setToken --params '{"token":"figd_…"}'` (kept beside the settings,
  readable only by you). It is never journaled, logged or saved in projects, and it is sent only to
  `api.figma.com`. `figma.status` says which token is in use.
- Network access uses the system's `curl` (macOS, Windows 10+ and most Linux systems ship it).

The reply lists the new comps and layers, how many came in natively, which nodes were rasterised and
why, and notes about anything approximated.

## People: the Figma plug-in

1. **Install once.** In the Figma desktop app: **Plugins ▸ Development ▸ Import plugin from
   manifest…** and pick `integrations/figma/plugin/manifest.json` from this repository.
2. **Start the bridge** (with EffectCraft open; its control channel is on 9877 when
   **Settings ▸ Let Agents Control EffectCraft** is on, or `effectcraft --control 9877`):

   ```sh
   effectcraft-cli figma-bridge
   ```

   It listens on `http://127.0.0.1:7789` and prints a **pairing code**. Options:
   `--port 7789`, `--control 9877` (the app's control port), `--code CODE`. The code is stored in the
   EffectCraft settings folder, so it stays the same next time.
3. **Send.** In Figma, run **Plugins ▸ Development ▸ EffectCraft Transfer**. Paste the pairing
   code once. The dot turns green when EffectCraft is reachable. Select layers, choose **New comp**
   or **Into active comp**, then click **Send**. Each transfer is one undo step in EffectCraft.

To use another port, change it in `manifest.json` (`devAllowedDomains`) and in `ui.html`
(`BRIDGE`), then start the bridge with `--port`.

The bridge listens on 127.0.0.1 only, accepts requests only on the loopback host name (no DNS
rebinding) and only from the plug-in (origin `null`), figma.com or non-browser clients, and imports
only with the pairing code. Any sandboxed page shares the plug-in's `null` origin, so the code is
the real protection: it is 12 characters from the operating system's secure random generator
(`--code` needs at least 12), far beyond guessing over HTTP. There is no lockout after wrong codes,
since any page could trigger one and lock the plug-in out. Payloads are written to a per-user
folder only you can read and deleted after the import; their images must be inline (base64), so a
payload can't make the app read a local file.
The code file in the settings folder (`figma-bridge-code`) is created readable only by you. One
transfer can be up to 64 MB (embedded image renders count); send a very large selection in several
parts. The bridge writes the payload to a temporary file and runs `file.importFigma {payloadPath}`
in the app.

## What transfers natively

| Figma | EffectCraft |
|---|---|
| Frame, component, instance, section (selected on its own) | A composition the size of the frame, with its fill as a background shape layer |
| Nested frame | Precomp. Clipping frames clip to their size, rounded corners become a layer mask. Opacity, blend mode, shadows and blur go on the precomp layer |
| Group | A null named after the group, with its children parented to it. A group with opacity, a blend mode, effects or a mask role becomes a precomp |
| Rectangle | Rectangle path (size, roundness). Different corner radii become an editable bezier path |
| Ellipse | Ellipse path. Arcs and donuts become a bezier path |
| Polygon, star | Bezier path from the geometry. A plug-in payload without geometry becomes a Polystar |
| Line, vector, boolean operation | Editable bezier paths. Boolean operations are flattened to their result path |
| Solid fills, linear and radial gradients | Fill and Gradient Fill (several fills stack in Figma's order) |
| Strokes | Stroke or Gradient Stroke: width, caps, joins, miter, dash and gap. Inside and outside alignment use an inset or outset copy of the shape (Offset Paths for free-form paths) |
| Text | Paragraph text: font family and style, size, colour, line height, letter spacing, alignment, upper case and small caps, paragraph spacing, and per-character style runs |
| Opacity, blend modes, hidden layers | Layer opacity, blend mode, video switch off |
| Drop shadow, inner shadow | Layer Styles ▸ Drop Shadow / Inner Shadow (colour, opacity, angle, distance, size, spread) |
| Layer blur | Gaussian Blur |
| Mask layers | Alpha or luma track mattes for the layers above the mask (the matte layer is hidden) |

Every layer's anchor point is at the centre of its Figma object, so scaling and rotating behave as
they do in Figma. Each layer's comment holds its Figma node id.

## What comes in as an image

These come in as PNG renders at 2× (the `scale` parameter), placed exactly over the node:

- Layers with image, video, pattern or emoji fills (the whole layer, with its stroke and corners).
  A frame with an image fill comes in as one image, children included.
- Node types without an EffectCraft counterpart: widgets, embeds, stickies, connectors, tables, and
  so on.

Renders come from Figma's image export (`GET /v1/images` or the plug-in's `exportAsync`). They are
saved next to a saved project (`<project> Figma Assets/`) or in the settings folder, and imported as
footage in a `<name> (Figma)` folder.

## Approximations and gaps

- Angular and diamond gradients come in as radial gradients. Elliptical radial gradients become circular.
- Background blur is skipped. Only the first drop shadow and the first inner shadow are kept (one of
  each per layer style). Shadow spread maps to a percentage.
- Underline and strikethrough aren't supported. Lowercase and title case change the text itself.
- A text layer whose box was auto-width gets a little extra width, so different font metrics don't
  wrap it. The vertical position follows Figma's line height. Fonts that aren't installed fall back
  to Inter, which is bundled.
- A vector that only has `strokeGeometry` (REST without `fillGeometry`) is rasterised. Ask for
  `geometry=paths`, which `file.importFigma` always does.
- Skewed transforms are placed without the skew (noted). The root frame's own opacity and effects
  stay in Figma when it becomes the composition.
- Auto-layout, constraints, components and variables aren't kept. Each node arrives as it looks.

REST responses are read through bounded pipes rather than shared temporary files. The system curl configuration is disabled so it cannot override redirect or output handling.
