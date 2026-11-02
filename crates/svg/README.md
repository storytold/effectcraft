# effectcraft-svg

SVG import for Visualab Effects: `.svg` files import as footage (rasterised at any scale, so
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
- styling through presentation attributes, the `style` attribute and `<style>` sheets with
  type, class, id and universal selectors (simple selectors and selector lists).

Not supported (ignored): text, images, filters, clip paths, masks, patterns, markers, animation.
XML parsing uses `roxmltree` (MIT/Apache-2.0). Test fixtures are original work (see
`tests/fixtures/*.attribution`).
