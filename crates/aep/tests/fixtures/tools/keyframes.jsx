// keyframes.jsx - the case table of the combined KEYFRAMES fixture project.
//
// One composition per case of the synthetic corpus (MANIFEST.md group D, the batch 2/3 key,
// ease, expression and time-stretch cases), named exactly like the case's single-change fixture
// (e.g. "058-key-opacity-2key"). Every case comp is a duplicate of the base comp "000-base"
// (comp C of 000-base: 1920x1080, 25 fps, 5 s, black, one red 1920x1080 solid "L") plus that
// case's change and its FROM chain (051 -> 052..055, 055 -> 056/057, 981 -> 982 -> 983).
//
// Same vocabulary and emitter as layers-comps.jsx: each step is [operation, args] of the After
// Effects MCP, `comp` is filled in. Key times are what the MCP's keyframe ops take (comp time;
// AE stores layer time, so the stretched cases land where the old fixtures have them).
// A keyed multi-dimensional property is read once (property.get) before the save: AE writes the
// first key's speed only after an evaluation (pilot lesson 3). Not for the linear colour keys of
// 210 and 598: their old fixtures were saved unevaluated (first key speed 0), and a read writes
// a speed there. 307 (bezier colour keys) was read in its recipe, so it keeps its read.
// Run: node keyframes.jsx <outdir>; then the sequence of layers-comps.jsx (no footage).

var LC = require("./layers-comps.jsx");
var BASE = LC.BASE;

var POS = ["Transform", "Position"];
var OPA = ["Transform", "Opacity"];
var SCA = ["Transform", "Scale"];
var ANC = ["Transform", "Anchor Point"];
function key(prop, t, v, layer) { return ["keyframe.add", { layer: layer || "L", property: prop, time: t, value: v }]; }
function get(prop, t, layer) { return ["property.get", { layer: layer || "L", property: prop, time: t }]; }
function ease(prop, k, a) { a.layer = "L"; a.property = prop; a.keyIndex = k; return ["keyframe.set_easing", a]; }
function interp(prop, k, a, layer) { a.layer = layer || "L"; a.property = prop; a.keyIndex = k; return ["keyframe.set_interpolation", a]; }
function timing(a) { a.layer = "L"; return ["layer.set_timing", a]; }
function cat() { var r = []; for (var i = 0; i < arguments.length; i++) r = r.concat(arguments[i]); return r; }
// keyframe.apply with an out ease on key 1 and an in ease on key 2 (597, 598, 730)
function applyEase(prop, layer, v0, v1) {
    return ["keyframe.apply", { layer: layer, property: prop, interp: "ease", keys: [
        { time: 0, value: v0, outSpeed: 50, outInfluence: 40 },
        { time: 2, value: v1, inSpeed: 30, inInfluence: 60 }] }];
}

var K051 = [key(POS, 0, [0, 0, 0]), key(POS, 1, [100, 0, 0])];
var K055 = cat(K051, [key(POS, 2.5, [200, 100, 0])]);
var POSKEYS = [key(POS, 0, [0, 0, 0]), key(POS, 1, [100, 0, 0])]; // 202-204
var OPAKEYS = [key(OPA, 0, 0), key(OPA, 2, 100)];
var GETPOS = [get(POS, 0.5)];
var FILL = ["Effects", "Fill", "Color"];
var K641 = [key(POS, 1, [100, 100, 0]), key(POS, 2, [500, 300, 0]), get(POS, 1.5)];
var K643 = [key(POS, 2, [100, 100, 0]), key(POS, 3, [500, 300, 0]), get(POS, 2.5)];
var K982 = [timing({ stretch: -100 }), timing({ startTime: 5 }), key(OPA, 0, 100), key(OPA, 4, 20)];

var CASES = [
    { id: "018-layer-stretch200", steps: [timing({ stretch: 200 })] },
    { id: "052-key-position-2key-ease", steps: cat(K051, [ease(POS, 1, { preset: "ease" }), ease(POS, 2, { preset: "ease" })], GETPOS) },
    { id: "053-key-position-2key-hold-key1", steps: cat(K051, [interp(POS, 1, { inType: "hold", outType: "hold" })], GETPOS) },
    { id: "054-key-position-2key-spatial-bezier", steps: cat(K051, [
        ["keyframe.set_spatial", { layer: "L", property: POS, keyIndex: 1, outTangent: [30, 60, 0], autoBezier: false, continuous: false }],
        ["keyframe.set_spatial", { layer: "L", property: POS, keyIndex: 2, inTangent: [-30, -60, 0], autoBezier: false, continuous: false }]],
        [get(POS, 0.25)]) },
    { id: "055-key-position-3key-inserted-t2.5", steps: cat(K055, GETPOS) },
    { id: "056-key-position-3key-roving-key2", steps: cat(K055, [["keyframe.set_roving", { layer: "L", property: POS, keyIndex: 2, roving: true }]], GETPOS) },
    { id: "057-key-position-3key-label9-key2", steps: cat(K055, [["keyframe.set_label", { layer: "L", property: POS, keyIndex: 2, label: 9 }]], GETPOS) },
    { id: "058-key-opacity-2key", steps: OPAKEYS },
    { id: "060-expr-opacity-wiggle", steps: [["expression.set", { layer: "L", property: OPA, expression: "wiggle(1,10)" }]] },
    { id: "202-key-out-hold-in-linear", steps: cat(POSKEYS, [interp(POS, 1, { inType: "linear", outType: "hold" })], GETPOS) },
    { id: "203-key-in-hold-out-linear", steps: cat(POSKEYS, [interp(POS, 1, { inType: "hold", outType: "linear" })], GETPOS) },
    { id: "204-key-ease-asymmetric", steps: cat(POSKEYS, [
        ease(POS, 1, { inSpeed: 5, inInfluence: 20, outSpeed: 50, outInfluence: 80 }),
        ease(POS, 2, { inSpeed: 7, inInfluence: 60, outSpeed: 9, outInfluence: 40 })], GETPOS) },
    { id: "205-key-opacity-ease-asymmetric", steps: cat(OPAKEYS, [
        ease(OPA, 1, { inSpeed: 5, inInfluence: 20, outSpeed: 20, outInfluence: 80 }),
        ease(OPA, 2, { inSpeed: 30, inInfluence: 60, outSpeed: 9, outInfluence: 40 })]) },
    { id: "206-key-scale-2key-linear", steps: [key(SCA, 0, [100, 100, 100]), key(SCA, 1, [200, 150, 100]), get(SCA, 0.5)] },
    { id: "210-key-fill-color-2key-linear", steps: [["effect.add", { layer: "L", matchName: "ADBE Fill" }],
        ["effect.set_property", { layer: "L", effectIndex: 1, property: ["Color"], value: [1, 0, 0], time: 0 }],
        ["effect.set_property", { layer: "L", effectIndex: 1, property: ["Color"], value: [0, 0.5, 1], time: 1 }]] },
    { id: "213-expr-opacity-wiggle-disabled", steps: [["expression.set", { layer: "L", property: OPA, expression: "wiggle(1,10)" }], ["expression.disable_all", {}]] },
    { id: "305-key-scale-bezier-ease", steps: [key(SCA, 0, [100, 100, 100]), key(SCA, 2, [200, 150, 100]),
        ["keyframe.set_easing", { layer: "L", property: SCA, keyIndex: 1, inSpeed: 0, inInfluence: 33, outSpeed: 20, outInfluence: 66 }],
        ["keyframe.set_easing", { layer: "L", property: SCA, keyIndex: 2, inSpeed: 10, inInfluence: 66, outSpeed: 0, outInfluence: 33 }],
        get(SCA, 1)] },
    { id: "307-key-fillcolor-bezier", steps: [["effect.add", { layer: "L", matchName: "ADBE Fill" }],
        ["effect.set_property", { layer: "L", effectIndex: 1, property: ["Color"], value: [1, 0, 0, 1], time: 0 }],
        ["effect.set_property", { layer: "L", effectIndex: 1, property: ["Color"], value: [0, 0, 1, 1], time: 2 }],
        get(FILL, 1),
        interp(FILL, 1, { inType: "bezier", outType: "bezier", temporalContinuous: false, temporalAutoBezier: false }),
        interp(FILL, 2, { inType: "bezier", outType: "bezier", temporalContinuous: false, temporalAutoBezier: false })] },
    { id: "316-layer-stretch50", steps: [timing({ stretch: 50 })] },
    { id: "317-layer-stretch150", steps: [timing({ stretch: 150 })] },
    { id: "597-key-anchor-ease-out50-40-in30-60", steps: [applyEase(ANC, "L", [960, 540], [1160, 640]), get(ANC, 1)] },
    { id: "598-key-ramp-point-ease-color-bezier", steps: [["effect.add", { layer: "L", matchName: "ADBE Ramp" }],
        applyEase(["Effects", "Gradient Ramp", "Start of Ramp"], "L", [100, 200], [500, 400]),
        get(["Effects", "Gradient Ramp", "Start of Ramp"], 1),
        key(["Effects", "Gradient Ramp", "Start Color"], 0, [1, 0, 0]),
        key(["Effects", "Gradient Ramp", "Start Color"], 2, [0, 0, 1]),
        interp(["Effects", "Gradient Ramp", "Start Color"], 1, { inType: "bezier", outType: "bezier" }),
        interp(["Effects", "Gradient Ramp", "Start Color"], 2, { inType: "bezier", outType: "bezier" })] },
    { id: "641-stretch-200-keys-position-1s-2s", steps: cat([timing({ stretch: 200 })], K641) },
    { id: "642-stretch-50-keys-position-1s-2s", steps: cat([timing({ stretch: 50 })], K641) },
    { id: "643-stretch-200-start-1-keys-position-2s-3s", steps: cat([timing({ startTime: 1, stretch: 200 })], K643) },
    { id: "730-key-anchor-ease-on-400x800-solid", steps: [["layer.create_solid", { name: "P", color: [0, 0, 1], width: 400, height: 800 }],
        applyEase(ANC, "P", [200, 400], [300, 500]), get(ANC, 1, "P")] },
    { id: "981-layer-stretch-minus100-start5", steps: [timing({ stretch: -100 }), timing({ startTime: 5 })] },
    { id: "982-layer-stretch-minus100-opacity-keys", steps: K982 },
    { id: "983-layer-stretch-minus50-opacity-keys", steps: cat(K982, [timing({ stretch: -50 })]) }
];

if (require.main === module) {
    LC.emit(CASES, LC.TEMPLATE, BASE, process.argv[2] || "ops-keyframes", null, 16);
}
module.exports = { CASES: CASES };
