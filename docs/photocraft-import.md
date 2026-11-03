# Import PhotoCraft documents

Choose **File ▸ Import ▸ File…** and select a `.pcraft` document. Numbered PhotoCraft documents import individually, rather than as an image sequence.

**Footage** reads the document's saved composite as one still image. Its dimensions are the saved image's actual dimensions. The currently pinned shared reader exposes PhotoCraft's saved preview, which can be smaller than the canvas. Import reports both sizes when they differ. Full-resolution composite selection requires a later shared-reader update.

**Composition** creates a composition with canvas-sized layer footage. It preserves layer names, visibility, opacity, recognized blend modes and group nesting. Groups become precompositions; pass-through groups use Collapse Transformations. Raster layers and saved pixel caches for text, shapes and smart objects remain linked to the source file. Text, shapes and smart objects are imported as pixels, without their native editing controls.

Layer pixels decode when a frame needs them. Missing caches or unsupported pixel data produce media decoding errors when used. Layered import supports RGB and grayscale pixels with 8-bit, 16-bit or floating-point samples. Enabled raster masks apply their density and feather. Native effects, vector and group masks, and embedded color profiles are unsupported, so a layered composition can differ from the saved appearance. Choose Footage when that appearance matters.

The dialog offers Footage and Composition. The command API also accepts `compositionLayerSizes`, but reports that PhotoCraft layers retain the full canvas bounds. A drag preference for retaining layer sizes selects Composition in this dialog.

## Command API

```json
{"command":"file.import","params":{"paths":["/path/artwork.pcraft"],"importAs":"footage"}}
```

Use `"importAs":"composition"` for layers. Import uses the existing still-footage duration preference and creates one undoable import operation for each layered document. Compatibility messages accompany the command's import results.

## Limits

PhotoCraft input is limited to 512 MiB. Whole-file ZIP reads share that limit, checked from the archive signature before allocating the input, so renaming a bundle does not bypass it. Decoded composites and layer canvases are limited to 16,777,216 pixels. Composition import also respects EffectCraft's composition size limit, allows up to 4,096 layers and 100 levels of group nesting, and rejects unsupported CMYK or Lab layer data. Footage can still import a supported saved composite when the document's layers exceed those limits.

Unsupported layer kinds are skipped with a message naming the layer. A nonempty document with no supported pixel layers cannot import as a composition; the error recommends Footage. Invalid input reports an error before adding a composition.
