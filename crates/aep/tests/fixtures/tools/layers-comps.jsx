// layers-comps.jsx - the case table of the combined LAYERS-COMPS fixture project.
//
// One composition per case of the synthetic corpus (MANIFEST.md groups A, B, C and the layer /
// comp header, anchor, matte and blend-mode cases), named exactly like the case's single-change
// fixture (e.g. "016-layer-blend-multiply"). Every case comp is a duplicate of the base comp
// "000-base" (comp C of 000-base: 1920x1080, 25 fps, 5 s, black, one red 1920x1080 solid "L")
// plus that case's change (and its FROM chain), so the importer's result for the old fixture's
// comp C must equal the case comp here.
//
// The table is in the vocabulary of the After Effects MCP (`eval.run` is disabled there): each
// step is [operation, args]; `comp` is filled in with the case comp. `$STEPS` in an args value
// is replaced by the item id of the imported steps.png footage (case 906).
// Also holds the .aep/.aepx pair table (PAIR, below) and the emitter keyframes.jsx uses.
// Run: node layers-comps.jsx <outdir> [stepsItemId]  -> <outdir>/batch-00.json (template),
// batch-NN.json (cases), footage-906.json (needs the steps.png item id).
// Sequence: save the dirty project to scratch-close.aep, project.open
// gen/empty-work.aep (copy of 001-empty.aep), batch-00, batch-01..NN, project.import_file
// test-media/steps.png -> id N, rerun with N, footage-906.json, Save As layers-comps.aep, then
// scrub.py (footage path, host name, XMP):
//   python -I gen/scrub.py layers-comps.aep layers-comps-scrubbed.aep <fixtures>/000-base.aep
// The pair: Save As aepx-pair.aep, then at once Save As aepx-pair.aepx, then
//   python -I gen/scrub.py aepx-pair.aep aepx-pair-scrubbed.aep <fixtures>/000-base.aep
//   python -I gen/layers-comps-scrub-aepx.py aepx-pair.aepx aepx-pair-scrubbed.aepx <fixtures>/000-base.aep
// (the .aepx scrub puts the base XMP packet into the ProjectXMPMetadata CDATA, so both halves
// carry the same trailer). The *-scrubbed files are the fixtures; the raw saves keep real paths.

var STEPS_PNG = "<corpus>/synthetic/test-media/steps.png";
var BASE = "000-base";

function sp(props) { return ["layer.set_props", { layer: "L", props: props }]; }
function cp(props) { return ["comp.set_props", { props: props }]; }
function timing(a) { a.layer = "L"; return ["layer.set_timing", a]; }
function blend(mode) { return ["layer.set_blend_mode", { layer: "L", mode: mode }]; }
function tr(a) { a.layer = a.layer || "L"; return ["transform.set", a]; }
function solid(name, color, w, h) { return ["layer.create_solid", { name: name, color: color, width: w, height: h }]; }
var M = solid("M", [0.5, 0.5, 0.5], 960, 540); // 402: grey 960x540 on top, the base of 403-408
function matte(layer, matteLayer, type) { return ["layer.set_track_matte", { layer: layer, matteLayer: matteLayer, matteType: type }]; }

var CASES = [
    { id: "010-layer-name", steps: [sp({ name: "Renamed" })] },
    { id: "012-layer-shy", steps: [sp({ shy: true })] },
    { id: "013-layer-locked", steps: [sp({ locked: true })] },
    { id: "014-layer-solo", steps: [sp({ solo: true })] },
    { id: "016-layer-blend-multiply", steps: [blend("multiply")] },
    { id: "017-layer-motionblur", steps: [sp({ motionBlur: true })] },
    { id: "019-layer-inpoint1", steps: [timing({ inPoint: 1 })] },
    { id: "021-layer-starttime0.5", steps: [timing({ startTime: 0.5 })] },
    { id: "022-layer-parent-null", steps: [["layer.create_null", { name: "Null" }], ["layer.set_parent", { layer: "L", parentLayer: 1 }]] },
    { id: "023-layer-quality-draft", steps: [sp({ quality: "draft" })] },
    { id: "024-layer-collapse", steps: [sp({ collapseTransformation: true })] },
    { id: "025-layer-adjustment", steps: [sp({ adjustmentLayer: true })] },
    { id: "026-layer-guide", steps: [sp({ guideLayer: true })] },
    { id: "030-comp-size-1280x720", steps: [cp({ width: 1280, height: 720 })] },
    { id: "031-comp-fps30", steps: [cp({ frameRate: 30 })] },
    { id: "032-comp-duration10", steps: [cp({ duration: 10 })] },
    { id: "033-comp-bg-green", steps: [cp({ bgColor: [0, 1, 0] })] },
    { id: "034-comp-shutter90", steps: [cp({ shutterAngle: 90 })] },
    { id: "035-comp-workarea-1-2", steps: [["comp.set_work_area", { start: 1, duration: 2 }]] },
    { id: "037-comp-pixelaspect2", steps: [cp({ pixelAspect: 2 })] },
    { id: "038-comp-motionblur", steps: [cp({ motionBlur: true })] },
    { id: "040-static-position-100-200", steps: [tr({ position: [100, 200, 0] })] },
    { id: "041-static-scale-50", steps: [tr({ scale: [50, 50, 100] })] },
    { id: "042-static-rotation-45", steps: [tr({ rotation: 45 })] },
    { id: "043-static-opacity-25", steps: [tr({ opacity: 25 })] },
    { id: "044-static-anchor-10-20", steps: [tr({ anchorPoint: [10, 20, 0] })] },
    { id: "211-anchor-100x100-solid-10-20", steps: [solid("S100", [1, 1, 1], 100, 100), tr({ layer: "S100", anchorPoint: [10, 20, 0] })] },
    { id: "212-anchor-400x300-solid-100-50", steps: [solid("S400", [1, 1, 1], 400, 300), tr({ layer: "S400", anchorPoint: [100, 50, 0] })] },
    { id: "222-layer-video-off", steps: [sp({ enabled: false })] },
    { id: "223-layer-audio-off", steps: [sp({ audioEnabled: false })] },
    { id: "224-layer-effects-off", steps: [sp({ effectsActive: false })] },
    { id: "225-layer-blend-screen", steps: [blend("screen")] },
    { id: "226-layer-blend-add", steps: [blend("add")] },
    { id: "227-layer-blend-overlay", steps: [blend("overlay")] },
    { id: "228-layer-blend-difference", steps: [blend("difference")] },
    { id: "229-comp-fps-29.97", steps: [cp({ frameRate: 29.97 })] },
    { id: "230-comp-fps-23.976", steps: [cp({ frameRate: 23.976 })] },
    { id: "231-comp-bg-grey-0.5", steps: [cp({ bgColor: [0.5, 0.5, 0.5] })] },
    { id: "259-solid-blue-0-0-1", steps: [solid("Blue", [0, 0, 1], 1920, 1080)] },
    { id: "299-save-noise", steps: [] },
    { id: "310-solid-100x100-pointcontrol", steps: [solid("S100", [1, 1, 1], 100, 100), ["effect.add", { layer: "S100", matchName: "ADBE Point Control" }]] },
    { id: "311-position-default-explicit", steps: [["property.set", { layer: "L", property: ["Transform", "Position"], value: [960, 540, 0] }]] },
    { id: "318-layer-inpoint-0.3333", steps: [timing({ inPoint: 0.3333 })] },
    { id: "325-comp-pixelaspect-0.9091", steps: [cp({ pixelAspect: 0.9091 })] },
    { id: "326-comp-shutterphase-m90", steps: [cp({ shutterPhase: -90 })] },
    { id: "327-comp-displaystartframe-100", steps: [cp({ displayStartFrame: 100 })] },
    { id: "403-matte-L-alpha-M", steps: [M, matte("L", "M", "alpha")] },
    { id: "404-matte-L-alphainv-M", steps: [M, matte("L", "M", "alphaInverted")] },
    { id: "405-matte-L-luma-M", steps: [M, matte("L", "M", "luma")] },
    { id: "406-matte-L-lumainv-M", steps: [M, matte("L", "M", "lumaInverted")] },
    { id: "407-matte-M-alpha-L-below", steps: [M, matte("M", "L", "alpha")] },
    { id: "408-matte-L-alpha-M-visible", steps: [M, matte("L", "M", "alpha"), ["layer.set_enabled", { layer: "M", enabled: true }]] }
];

// Case 906: L deleted; S = steps.png; 13 solids 300x300 [0.9,0.5,0.2] named by blend mode, in
// this order (each on top), at a 320 x 360 px grid (6 per row from [160,180]).
var MODES = ["normal", "add", "multiply", "screen", "overlay", "softLight", "hardLight", "difference", "exclusion", "colorDodge", "colorBurn", "darken", "lighten"];
function steps906() {
    var s = [["layer.delete", { layer: "L" }], ["layer.create_footage", { sourceItemId: "$STEPS", name: "S" }]];
    for (var i = 0; i < MODES.length; i++) {
        var m = MODES[i];
        s.push(solid(m, [0.9, 0.5, 0.2], 300, 300));
        s.push(tr({ layer: m, position: [160 + 320 * (i % 6), 180 + 360 * Math.floor(i / 6)] }));
        s.push(["layer.set_blend_mode", { layer: m, mode: m }]);
    }
    return s;
}
CASES.push({ id: "906-blendmodes-13-solids-on-steps", steps: steps906(), footage: true });

// The template comp (case 000-base itself) and the comp every case starts from.
var TEMPLATE = [
    ["comp.create", { name: BASE, width: 1920, height: 1080, fps: 25, duration: 5, bgColor: [0, 0, 0] }],
    ["layer.create_solid", { comp: BASE, name: "L", color: [1, 0, 0], width: 1920, height: 1080 }]
];

// ---------------------------------------------------------------- the .aep/.aepx pair
// aepx-pair.aep + aepx-pair.aepx: the `.aepx` tests (crates/aep/tests/aepx.rs,
// crates/aep-import/tests/aepx.rs, the 110 grafts of crates/aep-import/tests/combined.rs) need
// an .aep and an .aepx saved by AE from the same state; they read 000-base (lossless import,
// damage), 058-key-opacity-2key (damage), 060-expr-opacity-wiggle (the `wiggle(1,10)` Utf8),
// 110-text-A (the `btdk` list, lossless text import) and 124-precomp-Pre. All five go into ONE
// small project (template 000-base + four case comps), saved as .aep and right after as .aepx
// with no edit in between; it replaces the five old pairs. (058/060 as in keyframes.jsx.)
// Run: node layers-comps.jsx <outdir> pair
var PAIR = [
    { id: "058-key-opacity-2key", steps: [["keyframe.add", { layer: "L", property: ["Transform", "Opacity"], time: 0, value: 0 }], ["keyframe.add", { layer: "L", property: ["Transform", "Opacity"], time: 2, value: 100 }]] },
    { id: "060-expr-opacity-wiggle", steps: [["expression.set", { layer: "L", property: ["Transform", "Opacity"], expression: "wiggle(1,10)" }]] },
    // 110: new point text layer T, text "A" (AE defaults: Times New Roman, 100 px, white fill)
    { id: "110-text-A", steps: [["layer.create_text", { text: "A", name: "T" }]] },
    // 124: L precomposed (move all attributes) into a new comp "Pre"
    { id: "124-precomp-Pre", steps: [["comp.precompose", { layerIndices: [1], newName: "Pre", moveAll: true }]] }
];

// ---------------------------------------------------------------- MCP batch emitter (node)
function emit(cases, template, base, out, stepsId, per) {
    var fs = require("fs");
    var path = require("path");
    fs.mkdirSync(out, { recursive: true });
    function op(comp, st) {
        var a = {};
        for (var k in st[1]) {
            var v = st[1][k];
            a[k] = v === "$STEPS" ? stepsId : v;
        }
        if (a.comp === undefined) a.comp = comp;
        return { operation: st[0], args: a };
    }
    fs.writeFileSync(path.join(out, "batch-00.json"), JSON.stringify(template.map(function (t) { return { operation: t[0], args: t[1] }; })));
    var n = 1, cur = [], count = 0, total = 0;
    function flush() {
        if (!cur.length) return;
        fs.writeFileSync(path.join(out, "batch-" + (n < 10 ? "0" : "") + n + ".json"), JSON.stringify(cur));
        n++; cur = []; count = 0;
    }
    for (var i = 0; i < cases.length; i++) {
        var c = cases[i];
        var list = [{ operation: "comp.duplicate", args: { comp: c.from || base, newName: c.id } }];
        for (var j = 0; j < c.steps.length; j++) list.push(op(c.id, c.steps[j]));
        if (c.footage) {
            if (stepsId !== null) fs.writeFileSync(path.join(out, "footage-" + c.id.slice(0, 3) + ".json"), JSON.stringify(list));
            continue;
        }
        cur = cur.concat(list); total++;
        if (++count >= per) flush();
    }
    flush();
    console.log("batches", n - 1, "cases", total);
}

if (typeof require !== "undefined" && typeof module !== "undefined" && require.main === module) {
    if (process.argv[3] === "pair") emit(PAIR, TEMPLATE, BASE, process.argv[2] || "ops-aepx-pair", null, 16);
    else emit(CASES, TEMPLATE, BASE, process.argv[2] || "ops-layers-comps", process.argv[3] ? Number(process.argv[3]) : null, 20);
}
if (typeof module !== "undefined") module.exports = { CASES: CASES, TEMPLATE: TEMPLATE, BASE: BASE, emit: emit };
