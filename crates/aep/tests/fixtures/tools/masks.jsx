// masks.jsx - case table of the combined MASKS fixture project (<corpus>/combined/masks.aep).
//
// One composition per case of the synthetic corpus (MANIFEST.md, masks family), named exactly
// like the case's single-change fixture. Every case comp is a duplicate of the base comp
// "000-base" (comp C of 000-base: 1920x1080, 25 fps, 5 s, black, one red 1920x1080 solid "L")
// plus that case's change (and its FROM chain), as MCP operations (the AE MCP has eval.run
// disabled, so the table is written in MCP operation terms; mss-to-mcp.js turns it into
// batch.run lists, "$" stands for the case comp).
//
// Keyed multi-dimensional properties are read once (property.get) before the save: AE writes
// the first key's speed only after an evaluation (pilot lesson 3).

var FAMILY = "masks";
var OUT_AEP = "<corpus>/combined/masks.aep";

function op(o, a) { return { operation: o, args: a }; }
function maskAdd(layer, shape, size, center, extra) {
    var a = { comp: "$", layer: layer, shape: shape, size: size, center: center };
    for (var k in extra) { a[k] = extra[k]; }
    return op("mask.add", a);
}
function rect800(layer) { return maskAdd(layer || "L", "rect", [800, 400], [960, 540], {}); }
function maskProp(i, name) { return ["Masks", "Mask " + i, name]; }
function get(layer, path, t) { return op("property.get", { comp: "$", layer: layer, property: path, time: t }); }

var CASES = [
    { id: "090-mask-rect", steps: [rect800()] },
    { id: "091-mask-rect-props", steps: [rect800(), op("mask.set_props", { comp: "$", layer: "L", maskIndex: 1, feather: [20, 20], opacity: 50, mode: "Subtract", inverted: true, expansion: 5 })] },
    { id: "093-mask-path-2keys", steps: [rect800(),
        op("mask.set_path", { comp: "$", layer: "L", maskIndex: 1, shape: "rect", size: [400, 200], center: [960, 540], time: 0 }),
        op("mask.set_path", { comp: "$", layer: "L", maskIndex: 1, shape: "rect", size: [800, 400], center: [960, 540], time: 1 }),
        get("L", maskProp(1, "Mask Path"), 0.5)] },
    { id: "208-key-mask-feather-2key-linear", steps: [rect800(),
        op("keyframe.add", { comp: "$", layer: "L", property: maskProp(1, "Mask Feather"), time: 0, value: [0, 0] }),
        op("keyframe.add", { comp: "$", layer: "L", property: maskProp(1, "Mask Feather"), time: 1, value: [40, 20] }),
        get("L", maskProp(1, "Mask Feather"), 0.5)] },
    { id: "232-mask-ellipse", steps: [maskAdd("L", "ellipse", [800, 400], [960, 540], {})] },
    { id: "233-mask-open-path", steps: [op("mask.add", { comp: "$", layer: "L", mode: "Add" }),
        op("mask.set_path", { comp: "$", layer: "L", maskIndex: 1, vertices: [[500, 300], [960, 700], [1400, 300]], closed: false })] },
    { id: "234-mask-mode-intersect", steps: [maskAdd("L", "rect", [800, 400], [960, 540], { mode: "Intersect" })] },
    { id: "235-mask-mode-lighten", steps: [maskAdd("L", "rect", [800, 400], [960, 540], { mode: "Lighten" })] },
    { id: "236-mask-mode-darken", steps: [maskAdd("L", "rect", [800, 400], [960, 540], { mode: "Darken" })] },
    { id: "237-mask-mode-difference", steps: [maskAdd("L", "rect", [800, 400], [960, 540], { mode: "Difference" })] },
    { id: "306-key-maskfeather-bezier-ease", steps: [rect800(),
        op("keyframe.apply", { comp: "$", layer: "L", property: maskProp(1, "Mask Feather"), keys: [
            { time: 0, value: [0, 0], interp: "ease", inSpeed: 0, inInfluence: 33, outSpeed: 15, outInfluence: 66 },
            { time: 2, value: [50, 30], interp: "ease", inSpeed: 8, inInfluence: 66, outSpeed: 0, outInfluence: 33 }] }),
        get("L", maskProp(1, "Mask Feather"), 1)] },
    { id: "309-solid-100x100-mask-rect", steps: [op("layer.create_solid", { comp: "$", name: "S100", color: [1, 1, 1], width: 100, height: 100 }),
        maskAdd("S100", "rect", [60, 40], [50, 50], { mode: "Add" })] },
    { id: "328-mask-two-ellipse-rect", steps: [maskAdd("L", "ellipse", [800, 400], [960, 540], {}), maskAdd("L", "rect", [400, 300], [700, 500], {})] },
    { id: "329-mask-ellipse-mode-none", steps: [maskAdd("L", "ellipse", [800, 400], [960, 540], { mode: "None" })] },
    { id: "599-key-mask-path-ease-infl75", steps: [maskAdd("L", "rect", [200, 200], [960, 540], {}),
        op("mask.set_path", { comp: "$", layer: "L", maskIndex: 1, shape: "rect", size: [200, 200], center: [960, 540], time: 0 }),
        op("mask.set_path", { comp: "$", layer: "L", maskIndex: 1, shape: "rect", size: [600, 400], center: [960, 540], time: 2 }),
        op("keyframe.set_easing", { comp: "$", layer: "L", property: maskProp(1, "Mask Path"), keyIndex: 1, inSpeed: 0, outSpeed: 0, inInfluence: 75, outInfluence: 75 }),
        op("keyframe.set_easing", { comp: "$", layer: "L", property: maskProp(1, "Mask Path"), keyIndex: 2, inSpeed: 0, outSpeed: 0, inInfluence: 75, outInfluence: 75 }),
        get("L", maskProp(1, "Mask Path"), 1)] },
    { id: "700-key-mask-path-ease-out20-in50", steps: [maskAdd("L", "rect", [200, 200], [960, 540], {}),
        op("mask.set_path", { comp: "$", layer: "L", maskIndex: 1, shape: "rect", size: [200, 200], center: [960, 540], time: 0 }),
        op("mask.set_path", { comp: "$", layer: "L", maskIndex: 1, shape: "rect", size: [600, 400], center: [960, 540], time: 2 }),
        op("keyframe.set_easing", { comp: "$", layer: "L", property: maskProp(1, "Mask Path"), keyIndex: 1, inSpeed: 0, outSpeed: 0, inInfluence: 20, outInfluence: 20 }),
        op("keyframe.set_easing", { comp: "$", layer: "L", property: maskProp(1, "Mask Path"), keyIndex: 2, inSpeed: 0, outSpeed: 0, inInfluence: 50, outInfluence: 50 }),
        get("L", maskProp(1, "Mask Path"), 1)] },
    { id: "706-mask-on-shape-rect-0-m150-100-150", steps: [op("layer.create_shape", { comp: "$", name: "S" }),
        op("shape.add_group", { comp: "$", layer: "S", name: "Group 1" }),
        op("shape.add_rect", { comp: "$", layer: "S", groupIndex: 1, size: [400, 200] }),
        op("shape.add_fill", { comp: "$", layer: "S", groupIndex: 1, color: [0, 1, 0, 1] }),
        maskAdd("S", "rect", [100, 300], [50, 0], {})] },
    { id: "900-mask-added-no-path", steps: [op("property.add", { comp: "$", layer: "L", property: ["ADBE Mask Parade"], matchName: "ADBE Mask Atom" })] },
    { id: "901-mask-no-path-expansion-m100", steps: [op("property.add", { comp: "$", layer: "L", property: ["ADBE Mask Parade"], matchName: "ADBE Mask Atom" }),
        op("property.set", { comp: "$", layer: "L", property: maskProp(1, "Mask Expansion"), value: -100 })] }
];
var TEMPLATES = [];
var EXTRA_CASES = ["000-base"];
