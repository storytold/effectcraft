# effectcraft-svg

SVG import for EffectCraft: `.svg` files import as footage (rasterised at any scale, so
Continuously Rasterize stays sharp) and Layer ▸ Create ▸ Create Shapes from Vector Layer turns
them into editable shape layers.

Implemented from the W3C **Scalable Vector Graphics (SVG) 1.1 (Second Edition)** recommendation
(16 August 2011) and the **SVG 2** candidate recommendation (4 October 2018) for the parts listed
here, plus **CSS Color Module Level 3** for colour keywords and functions:

- document: `svg` (width/height, `viewBox`, `preserveAspectRatio`), nested `svg`, `g`, `defs`,
  `use` (x/y, `href` / `xlink:href`), `symbol` (through `use`), `title`/`desc`/`metadata` ignored;
- shapes: `path` (all path data commands incl. elliptical arcs), `rect` (rx/ry), `circle`,
  `ellipse`, `line`, `polyline`, `polygon`;
- `transform` lists (matrix, translate, scale, rotate with centre, skewX, skewY);
- painting: `fill`, `fill-rule`, `fill-opacity`, `stroke`, `stroke-width`, `stroke-linecap`,
  `stroke-linejoin`, `stroke-miterlimit`, `stroke-dasharray`, `stroke-dashoffset`,
  `stroke-opacity`, `opacity`, `display`, `visibility`, `color`/`currentColor`;
- `linearGradient` and `radialGradient` (with `fx`/`fy` focal point), `gradientUnits`,
  `gradientTransform`, `spreadMethod`, `href` inheritance of stops and attributes;
- static (non-animated) `<pattern>` fill/stroke with `patternUnits`, `patternContentUnits`, `patternTransform`,
  tile `x`/`y`/`width`/`height`, local `href`/`xlink:href` inheritance and solid paints on
  supported shape/group children;
- styling through presentation attributes, the `style` attribute and `<style>` sheets with
  type, class, id and universal selectors (simple selectors and selector lists).

Pattern `viewBox`, external pattern references, nested paint servers and non-shape pattern content
are unsupported; invalid patterns use a `url(...)` fallback when present. Local pattern `href`
chains are depth-limited and cycles are rejected. Tiles render once and are sampled across the
painted area; both live surfaces and aggregate tile work are bounded. Pattern paints are
rasterised in SVG footage; editable shape layers retain geometry and convert only a supplied
`url(...)` fallback to solid fill/stroke.
Path expansion and gradient resolution have per-import work limits. The rasterizer also caps live
surface memory and cumulative pattern, clip-coverage, and source-image work. Over-budget imports
return an error; over-budget raster operations are skipped, using a paint fallback where available.

`rasterize`/`rasterize_with` drop content refused by a budget and return the image drawn so far
(an empty image if the canvas itself is refused). `try_rasterize`/`try_rasterize_with` return
`Err(RasterError)` instead (`SurfaceLimit` for memory, `WorkLimit` for work). Budgets per render:
512 MiB of live offscreen surfaces, 512 MiB of pattern tiles, 256 Mi units each of pattern
render work, clip-path rasterisation work and image sampling work, and 2^31 pixels touched across
all passes.

For SVG input, text, images, filters, clip paths, masks, markers and animation (including pattern
animation) are unsupported and ignored. The PDF importers' documents do use clip, mask, blend and
image nodes, which the rasterizer renders under the same budgets.
XML parsing uses `roxmltree` (MIT/Apache-2.0). Test fixtures are original work (see
`tests/fixtures/*.attribution`).
