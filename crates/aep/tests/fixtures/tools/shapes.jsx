// shapes.jsx - case table of the combined SHAPES fixture project (<corpus>/combined/shapes.aep).
//
// One composition per case of the synthetic corpus (MANIFEST.md, shapes family), named exactly
// like the case's single-change fixture. Every case comp is a duplicate of the base comp
// "000-base" (comp C of 000-base: 1920x1080, 25 fps, 5 s, black, one red solid "L") plus the
// case's new shape layer on top (named "S", or "Shape" for the batch-3 cases 330-333, as in the
// originals) built with the same MCP operations as the original (shape.add_* or property.add of
// the match name; the two store different records), and the case's change / FROM chain
// (451 -> 457/462/464/467/469/471/473/482/483, 850 -> 851-857). Written in MCP operation
// terms (eval.run is disabled on the MCP); mss-to-mcp.js turns it into batch.run lists, "$"
// stands for the case comp.
//
// 986 (Fill eye switched off) is not scriptable (the shape item switch has no MCP operation):
// the original file is merged into the project (project.merge) and its comp renamed; it is an
// imported original, not a regenerated case.

var FAMILY = "shapes";
var OUT_AEP = "<corpus>/combined/shapes.aep";

function op(o, a) { return { operation: o, args: a }; }
function shape(name) { return [op("layer.create_shape", { comp: "$", name: name || "S" }), op("shape.add_group", { comp: "$", layer: name || "S", name: "Group 1" })]; }
function add(kind, extra, layer, group) {
    var a = { comp: "$", layer: layer || "S", groupIndex: group || 1 };
    for (var k in extra) { a[k] = extra[k]; }
    return op("shape." + kind, a);
}
function padd(path, mn, name, layer) {
    var a = { comp: "$", layer: layer || "S", property: path, matchName: mn };
    if (name) { a.name = name; }
    return op("property.add", a);
}
function G(rest) { return ["Contents", "Group 1"].concat(rest); }
function C(item, prop) { return G(["Contents", item, prop]); }
function set(path, v, layer) { return op("property.set", { comp: "$", layer: layer || "S", property: path, value: v }); }
function keys(path, ks, layer) { return op("keyframe.apply", { comp: "$", layer: layer || "S", property: path, keys: ks }); }
function get(path, t, layer) { return op("property.get", { comp: "$", layer: layer || "S", property: path, time: t }); }
function cat() { var r = []; for (var i = 0; i < arguments.length; i++) { r = r.concat(arguments[i]); } return r; }

var GREEN = [0, 1, 0, 1];
var RECT43 = add("add_rect", { size: [400, 300] });
var RECT42 = add("add_rect", { size: [400, 200] });
var S451 = cat(shape(), [add("add_ellipse", { size: [400, 300] }), add("add_fill", { color: GREEN })]);
var S850 = cat(shape(), [RECT42, add("add_fill", { color: [1, 0, 0, 1] }), add("add_trim_paths", {})]);

var CASES = [
    { id: "238-shape-rect-rounded-corners", steps: cat(shape(), [RECT43, add("add_fill", { color: GREEN }), add("add_rounded_corners", { radius: 20 })]) },
    { id: "239-shape-stroke-green-w8-round", steps: cat(shape(), [RECT43, add("add_stroke", { color: GREEN, width: 8 }), set(C("Stroke 1", "Line Cap"), 2), set(C("Stroke 1", "Line Join"), 2)]) },
    { id: "240-shape-custom-path-tangents", steps: cat(shape(), [add("add_path", { vertices: [[0, -100], [100, 100], [-100, 100]], inTangents: [[-10, -20], [30, 20], [-60, 10]], outTangents: [[40, 0], [-20, 30], [0, -50]], closed: true }), add("add_fill", { color: GREEN })]) },
    { id: "241-shape-gradient-fill-linear", steps: cat(shape(), [RECT43, add("add_gradient_fill", { gradientType: "linear", startPoint: [-200, 0], endPoint: [200, 0] })]) },
    { id: "242-shape-group-position-50-50", steps: cat(shape(), [RECT43, add("add_fill", { color: GREEN }), set(G(["Transform", "Position"]), [50, 50])]) },
    { id: "330-shape-polystar-star-5-50-100", steps: cat(shape("Shape"), [add("add_polystar", { starType: "star", points: 5, outerRadius: 100, innerRadius: 50 }, "Shape"), add("add_fill", { color: GREEN }, "Shape")]) },
    { id: "331-shape-polystar-polygon-5-100", steps: cat(shape("Shape"), [add("add_polystar", { starType: "polygon", points: 5, outerRadius: 100 }, "Shape"), add("add_fill", { color: GREEN }, "Shape")]) },
    { id: "332-shape-gradient-fill-radial", steps: cat(shape("Shape"), [add("add_rect", { size: [400, 300] }, "Shape"), add("add_gradient_fill", { gradientType: "radial", startPoint: [0, 0], endPoint: [200, 0] }, "Shape")]) },
    { id: "333-shape-group-two-rects", steps: cat(shape("Shape"), [add("add_rect", { size: [400, 300] }, "Shape"), add("add_rect", { size: [100, 100], position: [300, 0] }, "Shape"), add("add_fill", { color: GREEN }, "Shape")]) },
    { id: "451-shape-ellipse-400x300-fill", steps: S451 },
    { id: "457-shape-group-scale-50-80", steps: cat(S451, [set(G(["Transform", "Scale"]), [50, 80])]) },
    { id: "462-shape-layer-anchor-10-20", steps: cat(S451, [set(["Transform", "Anchor Point"], [10, 20])]) },
    { id: "464-shape-stroke-opacity-50", steps: cat(S451, [add("add_stroke", { color: [1, 0, 0, 1], width: 6 }), set(C("Stroke 1", "Opacity"), 50)]) },
    { id: "467-shape-trim-end-70", steps: cat(S451, [add("add_trim_paths", {}), set(C("Trim Paths 1", "End"), 70)]) },
    { id: "469-shape-trim-type-2", steps: cat(S451, [add("add_trim_paths", {}), set(C("Trim Paths 1", "Trim Multiple Shapes"), 2)]) },
    { id: "471-shape-rect-roundness-30", steps: cat(S451, [add("add_rect", { size: [200, 100] }), set(C("Rectangle Path 1", "Roundness"), 30)]) },
    { id: "473-shape-star-points-6", steps: cat(S451, [add("add_polystar", { starType: "star", points: 5, outerRadius: 100 }), set(C("Polystar Path 1", "Points"), 6)]) },
    { id: "482-shape-two-groups", steps: cat(S451, [op("shape.add_group", { comp: "$", layer: "S", name: "Group 2" }), add("add_rect", { size: [200, 100] }, "S", 2), add("add_fill", { color: [0, 0, 1, 1] }, "S", 2)]) },
    { id: "483-shape-nested-group-rect", steps: cat(S451, [padd(G(["Contents"]), "ADBE Vector Group", "Group 1"),
        padd(G(["Contents", "Group 1", "Contents"]), "ADBE Vector Shape - Rect"),
        set(G(["Contents", "Group 1", "Contents", "Rectangle Path 1", "Size"]), [100, 50])]) },
    { id: "519-shape-all-items-default", steps: cat(shape(), [
        padd(G(["Contents"]), "ADBE Vector Shape - Rect"), padd(G(["Contents"]), "ADBE Vector Shape - Ellipse"),
        padd(G(["Contents"]), "ADBE Vector Shape - Star"), padd(G(["Contents"]), "ADBE Vector Graphic - Fill"),
        padd(G(["Contents"]), "ADBE Vector Graphic - Stroke"), padd(G(["Contents"]), "ADBE Vector Graphic - G-Fill"),
        padd(G(["Contents"]), "ADBE Vector Graphic - G-Stroke"), padd(G(["Contents"]), "ADBE Vector Filter - Trim"),
        padd(G(["Contents"]), "ADBE Vector Filter - RC")]) },
    { id: "707-shape-merge-subtract-rect-ellipse", steps: cat(shape(), [RECT42, add("add_ellipse", { size: [200, 200] }), add("add_merge_paths", { mode: "subtract" }), add("add_fill", { color: GREEN })]) },
    { id: "708-shape-merge-intersect-rect-ellipse", steps: cat(shape(), [RECT42, add("add_ellipse", { size: [200, 200] }), add("add_merge_paths", { mode: "intersect" }), add("add_fill", { color: GREEN })]) },
    { id: "709-shape-repeater-3-pos150-60-rot10-scale90-80-op40", // shape.add_repeater fails in this MCP version ("null is not an object" after adding the
    // Repeater); property.add + property.set gives the same imported result as the original.
    steps: cat(shape(), [RECT42, add("add_fill", { color: GREEN }), padd(G(["Contents"]), "ADBE Vector Filter - Repeater"),
        set(C("Repeater 1", "Copies"), 3), set(C("Repeater 1", "Transform").concat(["Position"]), [150, 60]),
        set(C("Repeater 1", "Transform").concat(["Rotation"]), 10), set(C("Repeater 1", "Transform").concat(["Scale"]), [90, 80]),
        set(C("Repeater 1", "Transform").concat(["End Opacity"]), 40)]) },
    { id: "710-shape-offset-paths-20-round", steps: cat(shape(), [RECT42, add("add_offset_paths", { amount: 20, lineJoin: "round" }), add("add_fill", { color: GREEN })]) },
    { id: "731-shape-merge-offset-repeater-defaults", steps: cat(shape(), [RECT42, padd(G(["Contents"]), "ADBE Vector Filter - Merge"), padd(G(["Contents"]), "ADBE Vector Filter - Offset"), padd(G(["Contents"]), "ADBE Vector Filter - Repeater")]) },
    { id: "828-shape-offset-paths-20-copies3-copyoffset1.5-stroke", steps: cat(shape(), [RECT42, add("add_offset_paths", { amount: 20 }), add("add_stroke", { color: GREEN, width: 2 }), set(C("Offset Paths 1", "Copies"), 3), set(C("Offset Paths 1", "Copy Offset"), 1.5)]) },
    { id: "851-shape-key-trim-end-0-100-linear", steps: cat(S850, [keys(C("Trim Paths 1", "End"), [{ time: 0, value: 0, interp: "linear" }, { time: 2, value: 100, interp: "linear" }])]) },
    { id: "852-shape-key-trim-end-bezier-out0-60-in20-30", steps: cat(S850, [keys(C("Trim Paths 1", "End"), [{ time: 0, value: 0, interp: "ease", outSpeed: 0, outInfluence: 60 }, { time: 2, value: 100, interp: "ease", inSpeed: 20, inInfluence: 30 }])]) },
    { id: "853-shape-key-group-position-0-200-100-linear", steps: cat(S850, [keys(G(["Transform", "Position"]), [{ time: 0, value: [0, 0], interp: "linear" }, { time: 2, value: [200, 100], interp: "linear" }]), get(G(["Transform", "Position"]), 1)]) },
    { id: "854-shape-key-fill-color-red-blue-linear", steps: cat(S850, [keys(C("Fill 1", "Color"), [{ time: 0, value: [1, 0, 0, 1], interp: "linear" }, { time: 2, value: [0, 0, 1, 1], interp: "linear" }]), get(C("Fill 1", "Color"), 1)]) },
    { id: "855-shape-key-trim-offset-0-90-hold", steps: cat(S850, [keys(C("Trim Paths 1", "Offset"), [{ time: 0, value: 0, interp: "linear" }, { time: 2, value: 90, interp: "hold" }])]) },
    { id: "856-shape-trim-end-expression-time50", steps: cat(S850, [op("expression.set", { comp: "$", layer: "S", property: C("Trim Paths 1", "End"), expression: "time*50" })]) },
    { id: "857-shape-key-group-scale-100-50-150-linear", steps: cat(S850, [keys(G(["Transform", "Scale"]), [{ time: 0, value: [100, 100], interp: "linear" }, { time: 2, value: [50, 150], interp: "linear" }]), get(G(["Transform", "Scale"]), 1)]) }
];
// Imported originals (project.merge of the synthetic file, comp "C" renamed to the case id).
var MERGED = [
    { id: "986-shape-fill-switched-off-green", path: "<corpus>/synthetic/986-shape-fill-switched-off-green.aep", why: "Fill eye switch off: not scriptable" }
];
var EXTRA_CASES = ["000-base"];
